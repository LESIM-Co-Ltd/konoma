//! mermaid's `treemap-beta` language.
//!
//! Read from `treemap.langium`, `valueConverter.ts`, `parser.ts`, `db.ts` and `utils.ts` at tag
//! `mermaid@11.17.2`.
//!
//! * **Every name is quoted.** `Section STRING2` is `/"[^"]*"|'[^']*'/`, with no bare-word
//!   alternative. `Section 1` on its own is not a section.
//! * **A leaf is `"name" (":" | ",") number`.** The comma is not a typo in the grammar — either
//!   separator is accepted.
//! * **A number may contain commas and is parsed as a float with them removed**:
//!   `parseFloat(input.replace(/,/g, ''))`, so `1,234.5` is 1234.5. Reading it as `1` would make a
//!   tile a thousandth of the size the author meant.
//! * **Indentation is counted in characters, and a tab counts as one.** The value converter is
//!   `INDENTATION → input.length`. So a tab-indented file nests exactly as a one-space-indented
//!   one does.
//! * **Nesting is by strictly-greater indent, and only a section can have children**
//!   (`buildHierarchy` pops the stack `while (top.level >= item.level)` and only pushes non-leaf
//!   items). A leaf indented under a leaf therefore becomes the *sibling* of that leaf, not its
//!   child — which is not what the indentation looks like, and is what upstream does.
//! * `classDef` and `:::` are parsed here and read by `render::chart::treemap` /
//!   `render::style::cascade` to paint the tile they name (§2-3 only required that they not
//!   become phantom tiles; `docs/STATUS.md`'s 2026-08-28 entry is why they are drawn now too).

use super::{find_header, first_word, read_quoted, Envelope, ParseError, Preamble, TitleSyntax};

/// The keywords, longest first.
pub const KEYWORDS: &[&str] = &["treemap-beta", "treemap"];

/// The name used in messages.
pub const KEYWORD: &str = "treemap";

/// One node of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The quoted name.
    pub name: String,
    /// A leaf's value. `None` for a section, whose value is the sum of its children.
    pub value: Option<f64>,
    /// The `:::name` class. Resolved against [`Treemap::class_defs`] by `render::style::cascade`.
    pub class: Option<String>,
    /// Children, for a section.
    pub children: Vec<Node>,
}

impl Node {
    /// The area this node stands for: its own value, or its children's total.
    pub fn total(&self) -> f64 {
        match self.value {
            Some(v) => v,
            None => self.children.iter().map(Node::total).sum(),
        }
    }
}

/// A parsed treemap.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Treemap {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// The top-level nodes.
    pub roots: Vec<Node>,
    /// `classDef name styles`, `styles` kept as one un-split string — `render::chart::treemap`
    /// splits it once, with the flowchart parser's own `\,`-escaping comma splitter.
    pub class_defs: Vec<(String, String)>,
}

/// Whether this source is a treemap.
pub fn is_treemap(src: &str) -> bool {
    find_header(src, KEYWORDS).is_some()
}

/// Parses a mermaid treemap.
pub fn parse(src: &str) -> Result<Treemap, ParseError> {
    let Some(source) = find_header(src, KEYWORDS) else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };
    let mut tm = Treemap::default();
    let mut env = Envelope::new(source.front_matter_title);
    // The flat list `buildHierarchy` reads: (indent, node).
    let mut flat: Vec<(usize, Node)> = Vec::new();

    let mut feed: Vec<(usize, String)> = Vec::new();
    if !source.header_rest.is_empty() {
        feed.push((source.header_index + 1, source.header_rest.clone()));
    }
    for (i, line) in source
        .lines
        .iter()
        .enumerate()
        .skip(source.header_index + 1)
    {
        feed.push((i + 1, line.clone()));
    }

    for (number, line) in feed {
        if line.trim().is_empty() {
            continue;
        }
        if env.read(&line, TitleSyntax::Terminal) {
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let t = line.trim();
        if let Some(rest) = super::strip_ci(t, "classDef") {
            let rest = rest.trim().trim_end_matches(';');
            let (name, styles) = match rest.split_once(char::is_whitespace) {
                Some((n, s)) => (n.trim().to_string(), s.trim().to_string()),
                None => (rest.to_string(), String::new()),
            };
            if name.is_empty() {
                return Err(ParseError::Invalid {
                    line: number,
                    message: "classDef needs a name".to_string(),
                });
            }
            tm.class_defs.push((name, styles));
            continue;
        }
        let chars: Vec<char> = t.chars().collect();
        let Some((name, used)) = read_quoted(&chars, 0) else {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            });
        };
        let consumed: usize = chars[..used].iter().map(|c| c.len_utf8()).sum();
        let mut rest = t[consumed..].trim();
        let mut class = None;
        // The class selector may follow either the name (a section) or the value (a leaf).
        if let Some((before, after)) = rest.split_once(":::") {
            // Only when it is not the leaf separator: `"x" ::: c` versus `"x" : 5`.
            class = Some(after.trim().to_string());
            rest = before.trim();
        }
        let node = if rest.is_empty() {
            Node {
                name,
                value: None,
                class,
                children: Vec::new(),
            }
        } else {
            let Some(value_text) = rest.strip_prefix([':', ',']) else {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: rest.to_string(),
                });
            };
            let value_text = value_text.trim();
            let Some(value) = number_with_commas(value_text) else {
                return Err(ParseError::BadNumber {
                    line: number,
                    text: value_text.to_string(),
                    why: "a leaf's value must be a number",
                });
            };
            Node {
                name,
                value: Some(value),
                class,
                children: Vec::new(),
            }
        };
        flat.push((indent, node));
    }

    tm.preamble = env.preamble;
    tm.roots = build_hierarchy(flat);
    if tm.roots.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "node",
        });
    }
    if tm.roots.iter().map(Node::total).sum::<f64>() <= 0.0 {
        // Every tile's area is its share of the whole, so a total of zero has no chart in it —
        // upstream draws a row of zero-width rectangles.
        return Err(ParseError::Invalid {
            line: 0,
            message: "every value is zero, so no tile has any area".to_string(),
        });
    }
    Ok(tm)
}

/// `buildHierarchy`, transcribed: pop while the top of the stack is at or beyond this indent, then
/// attach; push only if this node can hold children.
fn build_hierarchy(flat: Vec<(usize, Node)>) -> Vec<Node> {
    // Indices into an arena, because a tree of owned nodes cannot be built with a stack of
    // references in safe Rust without copying at every step.
    let mut arena: Vec<Node> = Vec::with_capacity(flat.len());
    let mut children: Vec<Vec<usize>> = Vec::with_capacity(flat.len());
    let mut roots: Vec<usize> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (arena index, indent)

    for (indent, node) in flat {
        let is_leaf = node.value.is_some();
        arena.push(node);
        children.push(Vec::new());
        let me = arena.len() - 1;
        while stack.last().is_some_and(|(_, level)| *level >= indent) {
            stack.pop();
        }
        match stack.last() {
            Some((parent, _)) => children[*parent].push(me),
            None => roots.push(me),
        }
        if !is_leaf {
            stack.push((me, indent));
        }
    }

    fn build(i: usize, arena: &[Node], children: &[Vec<usize>]) -> Node {
        let mut node = arena[i].clone();
        node.children = children[i]
            .iter()
            .map(|c| build(*c, arena, children))
            .collect();
        node
    }
    roots.iter().map(|r| build(*r, &arena, &children)).collect()
}

/// `NUMBER2` is `/[0-9_\.\,]+/` and the converter is `parseFloat(input.replace(/,/g, ''))`.
///
/// The commas are removed, the underscores are not — `parseFloat("1_2")` is 1 in JavaScript, so a
/// value with an underscore silently truncates upstream. konoma refuses it instead: a tile drawn
/// at a twelfth of its value is a lie, and an error message is not.
fn number_with_commas(text: &str) -> Option<f64> {
    if text.is_empty()
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b',')
    {
        return None;
    }
    let cleaned: String = text.chars().filter(|c| *c != ',').collect();
    super::parse_number(&cleaned, false)
}
