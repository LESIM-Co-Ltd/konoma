//! Reading mermaid's `mindmap` language — an indented tree of shaped nodes.
//!
//! Read from `mindmap/parser/mindmap.jison` and `mindmapDb.ts` at tag `mermaid@11.17.2`.
//!
//! # Indentation is counted in characters, not in levels
//!
//! `SPACELIST` is `[\s]+` and the parser passes `$1.length`. So the *depth* of a node is the
//! number of whitespace characters before it, and `mindmapDb.getParent` walks backwards for the
//! last node with a **strictly smaller** count. Two things follow:
//!
//! * a tab counts as one, so mixing tabs and spaces reorders the tree;
//! * indentation does not have to be consistent. mermaid's own documentation shows
//!   `A` at 8, `B` at 12 and `C` at 10 and says C is a child of A — which is exactly what
//!   "the last node with a smaller count" gives, and is pinned by a test here.
//!
//! The first node's own count becomes the baseline and is subtracted from every later one, so a
//! whole diagram indented by four is the same diagram.
//!
//! # The shape is decided by **both** brackets
//!
//! `getType(start, end)` — `(` with `)` is a rounded rectangle and `(` with anything else is a
//! cloud, which is what makes `id)text(` a cloud and `id(text)` not. Copying only the opening
//! bracket gets that pair wrong.
//!
//! # What is parsed and not drawn
//!
//! `::icon(...)` and `:::class` decorate the node before them. konoma reads both — a line the
//! grammar has a rule for that the parser has none for becomes a phantom node
//! (`docs/FEATURE-MERMAID-RENDERER.md` §2-3) — and draws neither: an icon needs a font konoma
//! does not ship, and a class is a colour rule.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{find_header, first_word, ParseError, Source};

/// The keyword.
pub const KEYWORD: &str = "mindmap";

/// What a mindmap node is drawn as. `mindmapDb`'s `nodeType`, in its own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeShape {
    /// No brackets at all: the text with a line under it. `nodeType.DEFAULT` / `NO_BORDER`.
    #[default]
    NoBorder,
    /// `(text)`
    RoundedRect,
    /// `[text]`
    Rect,
    /// `((text))`
    Circle,
    /// `)text(`
    Cloud,
    /// `))text((`
    Bang,
    /// `{{text}}`
    Hexagon,
}

/// One node of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The id an edge would refer to it by. Generated when the node was written without one.
    pub id: String,
    /// The text drawn inside it.
    pub label: String,
    /// What it is drawn as.
    pub shape: NodeShape,
    /// How deep it is, relative to the first node written.
    ///
    /// **Signed**, because `addNode` computes `level - baseLevel` and does not clamp: a node
    /// indented *less* than the first one gets a negative level, and that is the difference
    /// between "a second root" (mindmap refuses it) and "a card outside every column" (kanban
    /// refuses it, and would otherwise silently become another column).
    pub level: isize,
    /// Index of its parent in [`Mindmap::nodes`], or `None` for the root.
    pub parent: Option<usize>,
    /// `::icon(...)`, read and not drawn.
    pub icon: Option<String>,
    /// `:::class`, read and not drawn.
    pub classes: Vec<String>,
}

/// A parsed mindmap.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mindmap {
    /// Every node, in source order. Node 0 is the root.
    pub nodes: Vec<Node>,
}

/// `mindmap`'s own answers to the two questions `kanban` answers differently.
const DIALECT: Dialect = Dialect {
    kind: KEYWORD,
    single_root: true,
    at_ends_the_id: false,
};

/// Whether this source is a mindmap.
pub fn is_mindmap(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid mindmap.
pub fn parse(src: &str) -> Result<Mindmap, ParseError> {
    let Some(source) = find_header(src, &[KEYWORD]) else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };
    let mut map = Mindmap::default();
    read_tree(&mut map, &source, &DIALECT, |_, _| false)?;
    if map.nodes.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "node",
        });
    }
    Ok(map)
}

/// The two places `kanban.jison` differs from `mindmap.jison` inside the shared reader.
///
/// Both are one line of lexer or one line of db, and both change what a valid diagram means, so
/// they are named rather than branched on a keyword string.
pub(crate) struct Dialect {
    /// The keyword, for error messages.
    pub kind: &'static str,
    /// Whether a second node at the outermost level is an error.
    ///
    /// `mindmapDb.addNode` throws "There can be only one root"; `kanbanDb.getSection` treats every
    /// node at the first node's level as **another column**. Same grammar, opposite answer.
    pub single_root: bool,
    /// Whether `NODE_ID` stops at `@`.
    ///
    /// `kanban.jison`'s is `[^\(\[\n\)\{\}@]+`; `mindmap.jison`'s has no `@`. So `a@{ … }` is
    /// a card with metadata on a board and a node *called* `a@{` on a mindmap.
    pub at_ends_the_id: bool,
}

/// The shared half of `mindmap` and `kanban`: the same lexer, the same indentation rule.
///
/// `extra` is offered every line's trailing text after the node — `kanban`'s `@{ … }` shape data,
/// which `mindmap`'s lexer has no rule for. It returns whether it consumed the rest.
pub(crate) fn read_tree(
    map: &mut Mindmap,
    source: &Source,
    dialect: &Dialect,
    mut extra: impl FnMut(&mut Node, &str) -> bool,
) -> Result<(), ParseError> {
    let kind = dialect.kind;
    let Source {
        lines,
        header_index,
        header_rest,
        ..
    } = source;
    let mut baseline: Option<usize> = None;
    let mut auto = 0usize;

    let mut raw: Vec<(usize, &str)> = Vec::new();
    if !header_rest.is_empty() {
        raw.push((header_index + 1, header_rest.as_str()));
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        raw.push((i + 1, line.as_str()));
    }
    let rows = join_quoted_rows(&raw);

    for (number, line) in rows {
        let line = line.as_str();
        // `\s*\%\%.*` is a SPACELINE — a comment is a blank line, not a statement.
        if line.trim_start().starts_with("%%") || line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let body = line.trim_start();

        // `::icon(` and `:::` decorate the node before them, and are checked first because
        // `:::urgent` would otherwise lex as a NODE_ID.
        if let Some(rest) = body.strip_prefix("::icon(") {
            let icon = rest.split(')').next().unwrap_or("").trim().to_string();
            if let Some(node) = map.nodes.last_mut() {
                node.icon = Some(icon);
            }
            continue;
        }
        if let Some(rest) = body.strip_prefix(":::") {
            if let Some(node) = map.nodes.last_mut() {
                node.classes
                    .extend(rest.split_whitespace().map(str::to_string));
            }
            continue;
        }

        let (mut node, tail) = read_node(body, number, dialect, &mut auto)?;
        let tail = tail.trim();
        if !tail.is_empty() && !extra(&mut node, tail) {
            return Err(ParseError::Unexpected {
                kind,
                line: number,
                text: tail.to_string(),
            });
        }

        // `addNode`: the first node sets the baseline and is the root at level 0.
        let level = match baseline {
            None => {
                baseline = Some(indent);
                0
            }
            Some(base) => indent as isize - base as isize,
        };
        node.level = level;
        // `getParent`: the last node written with a strictly smaller level.
        node.parent = map.nodes.iter().rposition(|n| n.level < level);
        if dialect.single_root && node.parent.is_none() && !map.nodes.is_empty() {
            // Upstream throws "There can be only one root".
            return Err(ParseError::Invalid {
                line: number,
                message: format!(
                    "{kind} can have only one root, and {:?} is a second",
                    node.label
                ),
            });
        }
        map.nodes.push(node);
    }
    Ok(())
}

/// Joins the rows a quoted label runs across.
///
/// The lexer's `NSTR` and `NSTR2` states have no rule for a newline, so `["]` … `["]` and
/// `["][`]` … `[`]["]` both **span lines** — which is how the documentation's multi-line
/// markdown label is written. A reader that works one line at a time sees an unterminated
/// bracket and refuses a diagram mermaid draws, so the rows are joined before anything else
/// looks at them. The line number kept is the first row's, which is where an author looking for
/// the error would start.
fn join_quoted_rows(raw: &[(usize, &str)]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut i = 0usize;
    while i < raw.len() {
        let (number, first) = raw[i];
        let mut text = first.to_string();
        i += 1;
        while let Some(open) = open_quote(&text) {
            let Some((_, next)) = raw.get(i) else { break };
            text.push('\n');
            text.push_str(next);
            i += 1;
            let _ = open;
        }
        out.push((number, text));
    }
    out
}

/// Whether a row leaves a quoted description open, and which kind.
fn open_quote(row: &str) -> Option<bool> {
    let b = row.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'"' {
            let markdown = b.get(i + 1) == Some(&b'`');
            let (needle, skip) = if markdown { ("`\"", 2) } else { ("\"", 1) };
            match row[i + skip..].find(needle) {
                Some(end) => i = i + skip + end + needle.len(),
                None => return Some(markdown),
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Reads one `nodeWithId` / `nodeWithoutId`, returning it and whatever followed on the line.
fn read_node<'a>(
    body: &'a str,
    number: usize,
    dialect: &Dialect,
    auto: &mut usize,
) -> Result<(Node, &'a str), ParseError> {
    let kind = dialect.kind;
    // `[^\(\[\n\)\{\}]+` (`mindmap`), plus `@` on a kanban board.
    let id_end = body
        .find(|c| matches!(c, '(' | '[' | ')' | '{' | '}') || (dialect.at_ends_the_id && c == '@'))
        .unwrap_or(body.len());
    let id = body[..id_end].trim().to_string();
    let rest = &body[id_end..];

    let Some((open, label, close, after)) = read_bracketed(rest) else {
        if id.is_empty() {
            return Err(ParseError::Unexpected {
                kind,
                line: number,
                text: body.trim().to_string(),
            });
        }
        return Ok((
            Node {
                id: id.clone(),
                label: id,
                shape: NodeShape::NoBorder,
                level: 0,
                parent: None,
                icon: None,
                classes: Vec::new(),
            },
            rest,
        ));
    };

    let shape = shape_of(open, close);
    // `nodeWithoutId` uses the description as the id, which is what makes two nodes with the same
    // words the same id upstream too.
    let id = if id.is_empty() {
        *auto += 1;
        label.clone()
    } else {
        id
    };
    Ok((
        Node {
            id,
            label,
            shape,
            level: 0,
            parent: None,
            icon: None,
            classes: Vec::new(),
        },
        after,
    ))
}

/// Splits `(…)`, `((…))`, `[…]`, `{{…}}`, `)…(`, `))…((` into open, text, close and tail.
///
/// Inside the `NODE` state the lexer takes `["]` … `["]` as one description, and `["][`]` …
/// `[`]["]` as a markdown one, so a bracket inside quotation marks does not close the node.
fn read_bracketed(s: &str) -> Option<(&str, String, &str, &str)> {
    const OPENERS: &[&str] = &["))", "((", "{{", "(-", "-)", "(", "[", ")"];
    let open = OPENERS.iter().find(|o| s.starts_with(**o))?;
    let mut rest = &s[open.len()..];

    // A quoted description: everything up to the closing quote, brackets included.
    let mut text = String::new();
    if let Some(after) = rest.strip_prefix("\"`") {
        let end = after.find("`\"")?;
        text.push_str(&after[..end]);
        rest = &after[end + 2..];
    } else if let Some(after) = rest.strip_prefix('"') {
        let end = after.find('"')?;
        text.push_str(&after[..end]);
        rest = &after[end + 1..];
    } else {
        // `<NODE>[^\)\]\(\}]+` — the description runs to the first closing bracket.
        let end = rest.find([')', ']', '(', '}']).unwrap_or(rest.len());
        text.push_str(&rest[..end]);
        rest = &rest[end..];
    }

    const CLOSERS: &[&str] = &["))", "((", "}}", "(-", "-)", ")", "]", "("];
    let close = CLOSERS.iter().find(|c| rest.starts_with(**c))?;
    Some((open, text.trim().to_string(), close, &rest[close.len()..]))
}

/// `mindmapDb.getType` — **both** brackets decide.
fn shape_of(open: &str, close: &str) -> NodeShape {
    match open {
        "[" => NodeShape::Rect,
        "(" => {
            if close == ")" {
                NodeShape::RoundedRect
            } else {
                NodeShape::Cloud
            }
        }
        "((" => NodeShape::Circle,
        ")" => NodeShape::Cloud,
        "))" => NodeShape::Bang,
        "{{" => NodeShape::Hexagon,
        _ => NodeShape::NoBorder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Mindmap {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_example_builds_the_documented_tree() {
        let m = ok("mindmap\n  root((mindmap))\n    Origins\n      Long history\n      Popularisation\n        British popular psychology author Tony Buzan\n    Research\n    Tools\n");
        assert_eq!(m.nodes[0].label, "mindmap");
        assert_eq!(m.nodes[0].shape, NodeShape::Circle);
        assert_eq!(m.nodes[0].parent, None);
        assert_eq!(m.nodes[1].label, "Origins");
        assert_eq!(m.nodes[1].parent, Some(0));
        assert_eq!(m.nodes[3].label, "Popularisation");
        assert_eq!(m.nodes[3].parent, Some(1));
        assert_eq!(m.nodes[4].parent, Some(3));
    }

    /// Every shape the documentation shows, in the shape it shows it.
    #[test]
    fn every_documented_shape_is_read_as_that_shape() {
        for (src, want) in [
            ("mindmap\n    id[I am a square]", NodeShape::Rect),
            (
                "mindmap\n    id(I am a rounded square)",
                NodeShape::RoundedRect,
            ),
            ("mindmap\n    id((I am a circle))", NodeShape::Circle),
            ("mindmap\n    id))I am a bang((", NodeShape::Bang),
            ("mindmap\n    id)I am a cloud(", NodeShape::Cloud),
            ("mindmap\n    id{{I am a hexagon}}", NodeShape::Hexagon),
            ("mindmap\n    I am the default shape", NodeShape::NoBorder),
            // Not from the documentation — from `getType`, which reads **both** brackets: `(`
            // closed by `)` is a rounded rectangle and `(` closed by anything else is a cloud.
            // A reader that copies only the opening bracket gets this pair the same.
            ("mindmap\n    id(also a cloud(", NodeShape::Cloud),
        ] {
            assert_eq!(ok(src).nodes[0].shape, want, "{src}");
        }
    }

    /// The documentation's own "unclean indentation" example: A at 8, B at 12, C at 10, and C is
    /// a child of A because 10 is still more than A's 4… and B's 12 is not smaller than 10.
    #[test]
    fn indentation_does_not_have_to_be_consistent() {
        let m = ok("mindmap\nRoot\n    A\n        B\n      C\n");
        assert_eq!(m.nodes[1].label, "A");
        assert_eq!(m.nodes[2].label, "B");
        assert_eq!(m.nodes[2].parent, Some(1));
        assert_eq!(m.nodes[3].label, "C");
        assert_eq!(m.nodes[3].parent, Some(1), "C hangs off A, not off B");
    }

    #[test]
    fn a_second_root_is_refused_the_way_upstream_refuses_it() {
        assert!(matches!(
            parse("mindmap\nRoot\nAlso root\n"),
            Err(ParseError::Invalid { .. })
        ));
    }

    #[test]
    fn an_icon_and_a_class_decorate_the_node_before_them() {
        let m = ok("mindmap\n    Root\n        A\n        ::icon(fa fa-book)\n        B(B)\n        :::urgent large\n");
        assert_eq!(m.nodes[1].icon.as_deref(), Some("fa fa-book"));
        assert_eq!(m.nodes[2].classes, ["urgent", "large"]);
        assert_eq!(m.nodes.len(), 3, "a decoration is not a node");
    }

    #[test]
    fn a_quoted_markdown_label_keeps_its_newlines_and_brackets() {
        let m = ok("mindmap\n    id1[\"`**Root** with\na second line`\"]\n");
        assert_eq!(m.nodes[0].label, "**Root** with\na second line");
        assert_eq!(m.nodes[0].shape, NodeShape::Rect);
    }

    #[test]
    fn a_node_written_without_an_id_is_named_by_its_own_words() {
        let m = ok("mindmap\n  (just a label)\n");
        assert_eq!(m.nodes[0].id, "just a label");
        assert_eq!(m.nodes[0].shape, NodeShape::RoundedRect);
    }

    #[test]
    fn a_header_with_no_nodes_is_refused() {
        assert!(matches!(parse("mindmap\n"), Err(ParseError::NoData { .. })));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "mindmap\n  root",
            "MINDMAP\n  root",
            "mindmap",
            "mindmaps\n  a",
            "kanban\n  a",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_mindmap(src), !refused, "{src:?}");
        }
    }
}
