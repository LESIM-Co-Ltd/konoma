//! Reading mermaid's `block-beta` language — blocks on an explicit column grid.
//!
//! Read from `block/parser/block.jison` and `blockDB.ts` at tag `mermaid@11.17.2`.
//!
//! # This grammar has no statement separator
//!
//! `[\s]+` and `[\n]+` are both *skipped*, so `a b c` on one line and `a`, `b`, `c` on three are
//! the same diagram. Everything is a token stream, which is why this module has a lexer in it
//! rather than a line loop like its siblings.
//!
//! # `columns` is the whole layout
//!
//! `columns 3` says how many cells a row has; `a:2` says a block is two cells wide; `space:3`
//! leaves three empty. That grid is the *content* of a block diagram — it is what the author is
//! saying — so konoma's renderer places these on the grid rather than handing them to a layered
//! layout engine, which has no way to honour "these two are side by side".
//!
//! `columns auto` is `-1`, and means "one row, as wide as it needs to be". It is also the default,
//! which is why `block-beta\n a b c` is a row of three.
//!
//! # Where konoma is deliberately more permissive
//!
//! Inside a shape, upstream's `NODE` state opens a string on `["]`, so its own documentation
//! always writes `a["A label"]`. konoma also accepts `a[A label]`, unquoted, the way the flowchart
//! grammar does. Being more permissive cannot lose a diagram; it can accept one mermaid refuses,
//! and that is the trade recorded here rather than left for a reader to discover.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::Envelope;
use crate::preview::mermaid::chart::{
    find_header, first_word, starts_ci, ParseError, Preamble, Source, TitleSyntax,
};
use crate::preview::mermaid::flowchart::{Arrow, Shape, Stroke};

/// The keywords, longest first.
pub const KEYWORDS: &[&str] = &["block-beta", "block"];

/// `columns auto`.
pub const AUTO: i32 = -1;

/// Which way a block arrow points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// `right`
    Right,
    /// `left`
    Left,
    /// `up`
    Up,
    /// `down`
    Down,
    /// `x` — both ways horizontally.
    X,
    /// `y` — both ways vertically.
    Y,
}

/// One thing on the grid.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A block: a shape with a label.
    Node(Node),
    /// `space` / `space:n` — cells left empty on purpose.
    Space {
        /// How many cells.
        width: usize,
    },
    /// `block:id … end` / `block … end` — a grid inside a cell.
    Composite(Composite),
}

impl Item {
    /// How many cells it takes.
    pub fn width(&self) -> usize {
        match self {
            Item::Node(n) => n.width,
            Item::Space { width } => *width,
            Item::Composite(c) => c.width,
        }
    }

    /// Its id, if it has one an edge can name.
    pub fn id(&self) -> Option<&str> {
        match self {
            Item::Node(n) => Some(&n.id),
            Item::Space { .. } => None,
            Item::Composite(c) => Some(&c.id),
        }
    }
}

/// A block.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The id an edge names it by.
    pub id: String,
    /// The words inside it. Falls back to the id, as upstream does.
    pub label: String,
    /// Its outline.
    pub shape: Shape,
    /// How many columns it spans (`a:2`).
    pub width: usize,
    /// `<["Label"]>(right)` — which ways the arrow points. `None` for an ordinary block.
    pub arrow: Option<Vec<Dir>>,
}

/// A nested grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Composite {
    /// The id, written (`block:ID`) or generated.
    pub id: String,
    /// Whether the source wrote the id, which is what decides whether the frame is titled.
    pub named: bool,
    /// How many columns it has of its own. [`AUTO`] until it says.
    pub columns: i32,
    /// How many of the parent's columns it spans.
    pub width: usize,
    /// What is inside it.
    pub children: Vec<Item>,
}

/// One link.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// Source id.
    pub from: String,
    /// Target id.
    pub to: String,
    /// The words on the line.
    pub label: Option<String>,
    /// What is drawn at the ends.
    pub arrow: Arrow,
    /// Line style.
    pub stroke: Stroke,
}

/// A parsed block diagram.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockDiagram {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// The outermost grid's column count. [`AUTO`] until `columns` says.
    pub columns: i32,
    /// What is on the grid, in source order.
    pub items: Vec<Item>,
    /// The links, in source order.
    pub edges: Vec<Edge>,
}

/// Whether this source is a block diagram.
pub fn is_block_diagram(src: &str) -> bool {
    find_header(src, KEYWORDS).is_some()
}

/// Parses a mermaid block diagram.
pub fn parse(src: &str) -> Result<BlockDiagram, ParseError> {
    let Some(Source {
        lines,
        header_index,
        header_rest,
        front_matter_title,
    }) = find_header(src, KEYWORDS)
    else {
        return Err(ParseError::NotThisChart {
            expected: "block",
            header: first_word(src),
        });
    };

    // The envelope is line-based; the body is not. So the accessibility statements are taken off
    // first and the rest is handed to the lexer as one string.
    let mut env = Envelope::new(front_matter_title);
    let mut body = String::new();
    if !header_rest.is_empty() {
        body.push_str(&header_rest);
        body.push('\n');
    }
    for line in lines.iter().skip(header_index + 1) {
        if env.read(line, TitleSyntax::RawRestOfLine) {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    let mut p = Parser {
        s: &body,
        i: 0,
        auto: 0,
        declared: Vec::new(),
    };
    let mut diagram = BlockDiagram {
        columns: AUTO,
        ..BlockDiagram::default()
    };
    let (items, edges, columns) = p.read_block(true)?;
    diagram.items = items;
    diagram.edges = edges;
    diagram.columns = columns;
    diagram.preamble = env.preamble;
    if diagram.items.is_empty() {
        return Err(ParseError::NoData {
            kind: "block",
            wanted: "block",
        });
    }
    Ok(diagram)
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
    auto: usize,
    /// Every id declared **anywhere in the diagram**, nesting included.
    ///
    /// `blockDB.setBlock` keys on the id with no notion of scope, so writing `C` again — at any
    /// level — refers to the block already called `C` rather than making a second one. Without
    /// this, `block:ID … C … end` followed by `C --> D` draws two blocks called `C` and routes
    /// the line to the wrong one, which is a line that says something the source did not.
    declared: Vec<String>,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.s[self.i..]
    }

    /// `[\s]+` and `[\n]+` are both skipped by the lexer.
    ///
    /// Comments need no rule here: this lexer's own `\s*\%\%.*` is commented out upstream, and
    /// `preprocess` has already taken whole-line comments off before the source arrives.
    fn skip_space(&mut self) {
        let rest = self.rest();
        let trimmed = rest.trim_start();
        self.i += rest.len() - trimmed.len();
    }

    fn eat(&mut self, text: &str) -> bool {
        if starts_ci(self.rest(), text) {
            self.i += text.len();
            return true;
        }
        false
    }

    /// Reads a grid until `end` (or the end of the source, for the outermost one).
    #[allow(clippy::type_complexity)]
    fn read_block(&mut self, outermost: bool) -> Result<(Vec<Item>, Vec<Edge>, i32), ParseError> {
        let mut items: Vec<Item> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut columns = AUTO;
        // The id of the block a link would start from: the one most recently read.
        let mut last: Option<String> = None;

        loop {
            self.skip_space();
            if self.rest().is_empty() {
                if outermost {
                    return Ok((items, edges, columns));
                }
                return Err(ParseError::Invalid {
                    line: 0,
                    message: "a `block:` was opened and never closed with `end`".to_string(),
                });
            }
            if self.starts_word("end") {
                self.eat("end");
                if outermost {
                    return Err(ParseError::Unexpected {
                        kind: "block",
                        line: 0,
                        text: "end".to_string(),
                    });
                }
                return Ok((items, edges, columns));
            }
            if let Some(n) = self.read_columns() {
                columns = n;
                continue;
            }
            if let Some(width) = self.read_space() {
                items.push(Item::Space { width });
                last = None;
                continue;
            }
            // `classDef` / `class` / `style` take the rest of their line and are appearance only.
            if self.starts_word("classDef")
                || self.starts_word("class")
                || self.starts_word("style")
            {
                let end = self.rest().find('\n').unwrap_or(self.rest().len());
                self.i += end;
                continue;
            }
            if self.starts_block_keyword() {
                let (composite, inner) = self.read_composite()?;
                last = Some(composite.id.clone());
                items.push(Item::Composite(composite));
                // A nested block's own links belong to the diagram, not to it: an endpoint may
                // name a block outside. Upstream flattens the same way.
                edges.extend(inner);
                continue;
            }
            if let Some((arrow, stroke, label)) = self.read_link()? {
                self.skip_space();
                let Some(from) = last.clone() else {
                    return Err(ParseError::Unexpected {
                        kind: "block",
                        line: 0,
                        text: "a link with nothing before it".to_string(),
                    });
                };
                let node = self.read_node()?;
                let to = node.id.clone();
                // `nodeStatement link node` yields the node too, so `A --> B` declares B — unless
                // B is already a block somewhere, in which case the link names it.
                match items.iter_mut().find(|i| i.id() == Some(to.as_str())) {
                    Some(Item::Node(n)) => merge_node(n, node),
                    Some(_) => {}
                    None if self.declared.contains(&to) => {}
                    None => {
                        self.declared.push(to.clone());
                        items.push(Item::Node(node));
                    }
                }
                edges.push(Edge {
                    from,
                    to: to.clone(),
                    label,
                    arrow,
                    stroke,
                });
                last = Some(to);
                continue;
            }
            let node = self.read_node()?;
            last = Some(node.id.clone());
            // An id that has already been declared is the **same block** written again, not a
            // second one: `blockDB.setBlock` keys on the id. Writing `A space B` and then
            // `A("Start")-->B` gives A its shape without giving the grid a third cell.
            if let Some(existing) = items.iter_mut().find(|i| i.id() == Some(node.id.as_str())) {
                if let Item::Node(n) = existing {
                    merge_node(n, node);
                }
                continue;
            }
            // Declared inside another block: this is a reference to it, not a second block.
            if self.declared.contains(&node.id) {
                continue;
            }
            self.declared.push(node.id.clone());
            items.push(Item::Node(node));
        }
    }

    fn starts_word(&self, word: &str) -> bool {
        let rest = self.rest();
        rest.len() >= word.len()
            && starts_ci(rest, word)
            && !rest[word.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
    }

    fn starts_block_keyword(&self) -> bool {
        let rest = self.rest();
        if starts_ci(rest, "block:") {
            return true;
        }
        self.starts_word("block")
    }

    /// `"columns"\s+"auto"` / `"columns"\s+[\d]+`.
    fn read_columns(&mut self) -> Option<i32> {
        if !self.starts_word("columns") {
            return None;
        }
        let after = self.rest()["columns".len()..].trim_start();
        let used = self.rest().len() - after.len();
        if starts_ci(after, "auto") {
            self.i += used + 4;
            return Some(AUTO);
        }
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digits == 0 {
            return None;
        }
        let n: i32 = after[..digits].parse().ok()?;
        self.i += used + digits;
        Some(n)
    }

    /// `space[:]\d+` / `space`.
    fn read_space(&mut self) -> Option<usize> {
        if !self.rest().to_ascii_lowercase().starts_with("space") {
            return None;
        }
        let after = &self.rest()["space".len()..];
        if let Some(rest) = after.strip_prefix(':') {
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if digits > 0 {
                let n: usize = rest[..digits].parse().ok()?;
                self.i += "space".len() + 1 + digits;
                return Some(n);
            }
        }
        // `space` alone, but not `spaceship`.
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        self.i += "space".len();
        Some(1)
    }

    /// `block:id nodeStatement document end` / `block document end`.
    fn read_composite(&mut self) -> Result<(Composite, Vec<Edge>), ParseError> {
        let named = self.eat("block:");
        if !named {
            self.eat("block");
        }
        let (id, width) = if named {
            let id_end = self
                .rest()
                .find(|c: char| c.is_whitespace() || c == ':')
                .unwrap_or(self.rest().len());
            let id = self.rest()[..id_end].to_string();
            self.i += id_end;
            let width = self.read_size().unwrap_or(1);
            (id, width)
        } else {
            self.auto += 1;
            (format!("block{}", self.auto), 1)
        };
        self.declared.push(id.clone());
        let (children, edges, columns) = self.read_block(false)?;
        Ok((
            Composite {
                id,
                named,
                columns,
                width,
                children,
            },
            edges,
        ))
    }

    /// `':'\d+` — the `:2` in `a:2`.
    fn read_size(&mut self) -> Option<usize> {
        let rest = self.rest();
        let after = rest.strip_prefix(':')?;
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digits == 0 {
            return None;
        }
        let n = after[..digits].parse().ok()?;
        self.i += 1 + digits;
        Some(n)
    }

    fn read_node(&mut self) -> Result<Node, ParseError> {
        // `[^\(\[\n\-\)\{\}\s\<\>:=]+`
        let id_end = self
            .rest()
            .find(|c: char| {
                matches!(
                    c,
                    '(' | '[' | '\n' | '-' | ')' | '{' | '}' | '<' | '>' | ':' | '='
                ) || c.is_whitespace()
            })
            .unwrap_or(self.rest().len());
        let id = self.rest()[..id_end].to_string();
        if id.is_empty() {
            let text: String = self.rest().chars().take(24).collect();
            return Err(ParseError::Unexpected {
                kind: "block",
                line: 0,
                text,
            });
        }
        self.i += id_end;
        // `<["Label"]>(dirs)`
        if self.rest().starts_with("<[") {
            let (label, dirs) = self.read_block_arrow()?;
            let width = self.read_size().unwrap_or(1);
            return Ok(Node {
                id,
                label,
                shape: Shape::Rect,
                width,
                arrow: Some(dirs),
            });
        }
        let (label, shape) = match self.read_shape()? {
            Some(pair) => pair,
            None => (id.clone(), Shape::Rect),
        };
        let width = self.read_size().unwrap_or(1);
        Ok(Node {
            id,
            label,
            shape,
            width,
            arrow: None,
        })
    }

    /// The bracket pairs, longest opener first so `(((` never lexes as `((`.
    fn read_shape(&mut self) -> Result<Option<(String, Shape)>, ParseError> {
        const OPENERS: &[(&str, &str, Shape)] = &[
            ("(((", ")))", Shape::DoubleCircle),
            ("((", "))", Shape::Circle),
            ("([", "])", Shape::Stadium),
            ("[[", "]]", Shape::Subroutine),
            ("[(", ")]", Shape::Cylinder),
            ("[/", "/]", Shape::LeanRight),
            ("[\\", "\\]", Shape::LeanLeft),
            ("{{", "}}", Shape::Hexagon),
            ("{", "}", Shape::Diamond),
            (">", "]", Shape::Odd),
            ("(", ")", Shape::RoundedRect),
            ("[", "]", Shape::Rect),
        ];
        // The two trapezoids are the mixed pairs, decided by the *closing* bracket (§2-6 rule 3).
        for (open, close, shape) in OPENERS {
            if !self.rest().starts_with(open) {
                continue;
            }
            let after = &self.rest()[open.len()..];
            let (text, used, actual_close) = read_label(after)?;
            let closed = &after[used..];
            let shape = if *open == "[/" && closed.starts_with("\\]") {
                Shape::Trapezoid
            } else if *open == "[\\" && closed.starts_with("/]") {
                Shape::InvTrapezoid
            } else {
                *shape
            };
            let close_len = if actual_close.is_empty() {
                if !closed.starts_with(close) {
                    // The mixed pairs, and anything else, are checked here.
                    let alt = ["\\]", "/]"];
                    match alt.iter().find(|a| closed.starts_with(**a)) {
                        Some(a) => a.len(),
                        None => {
                            return Err(ParseError::Invalid {
                                line: 0,
                                message: format!("`{open}` was opened and never closed"),
                            })
                        }
                    }
                } else {
                    close.len()
                }
            } else {
                actual_close.len()
            };
            self.i += open.len() + used + close_len;
            return Ok(Some((text, shape)));
        }
        Ok(None)
    }

    /// `<["Label"]>(right, down)`.
    fn read_block_arrow(&mut self) -> Result<(String, Vec<Dir>), ParseError> {
        self.i += 2; // `<[`
        let (label, used, _) = read_label(self.rest())?;
        self.i += used;
        if !self.rest().starts_with("]>") {
            return Err(ParseError::Invalid {
                line: 0,
                message: "a block arrow's `<[` was never closed with `]>`".to_string(),
            });
        }
        self.i += 2;
        self.skip_space();
        let Some(after) = self.rest().strip_prefix('(') else {
            return Err(ParseError::Invalid {
                line: 0,
                message: "a block arrow needs a direction in brackets".to_string(),
            });
        };
        let Some(end) = after.find(')') else {
            return Err(ParseError::Invalid {
                line: 0,
                message: "a block arrow's direction list was never closed".to_string(),
            });
        };
        let mut dirs = Vec::new();
        for word in after[..end].split(',') {
            dirs.push(match word.trim().to_ascii_lowercase().as_str() {
                "right" => Dir::Right,
                "left" => Dir::Left,
                "up" => Dir::Up,
                "down" => Dir::Down,
                "x" => Dir::X,
                "y" => Dir::Y,
                other => {
                    return Err(ParseError::Invalid {
                        line: 0,
                        message: format!("`{other}` is not one of right, left, up, down, x, y"),
                    })
                }
            });
        }
        if dirs.is_empty() {
            return Err(ParseError::Invalid {
                line: 0,
                message: "a block arrow needs at least one direction".to_string(),
            });
        }
        self.i += 1 + end + 1;
        Ok((label, dirs))
    }

    /// A link, if one starts here: `-->`, `---`, `==>`, `-.->`, `~~~`, and `-- "x" -->`.
    ///
    /// Transcribed from the four `LINK` rules and the three `START_LINK` rules rather than
    /// recalled, because the shortest legal link is **three characters** (`\-\-+[-xo>]`): `--` on
    /// its own is a `START_LINK` and needs a label after it, and `---` is a plain line. A reader
    /// that treats `--` as a link swallows the label.
    #[allow(clippy::type_complexity)]
    fn read_link(&mut self) -> Result<Option<(Arrow, Stroke, Option<String>)>, ParseError> {
        let rest = self.rest();
        // `[xo<]?` decorates the *tail*, and only when a shaft follows it.
        let (tail, at) = match rest.as_bytes().first() {
            Some(c @ (b'<' | b'x' | b'o'))
                if matches!(rest.as_bytes().get(1), Some(b'-') | Some(b'=') | Some(b'.')) =>
            {
                (*c, 1)
            }
            _ => (0, 0),
        };
        let body = &rest[at..];

        if let Some((len, head, stroke)) = match_link(body) {
            let arrow = arrow_of(tail, head);
            self.i += at + len;
            return Ok(Some((arrow, stroke, None)));
        }
        // `START_LINK LINK_LABEL STR LINK` — `-- "x" -->`.
        let Some((len, stroke)) = match_start_link(body) else {
            return Ok(None);
        };
        let after = body[len..].trim_start();
        let Some(quoted) = after.strip_prefix('"') else {
            return Ok(None);
        };
        let Some(end) = quoted.find('"') else {
            return Err(ParseError::Invalid {
                line: 0,
                message: "a link label's `\"` was never closed".to_string(),
            });
        };
        let label = quoted[..end].trim().to_string();
        let closing = quoted[end + 1..].trim_start();
        let Some((len2, head, _)) = match_link(closing) else {
            return Err(ParseError::Invalid {
                line: 0,
                message: format!("the link labelled `{label}` has no second half"),
            });
        };
        // Everything from the start of the link to the end of its second half.
        let consumed = rest.len() - closing.len() + len2;
        self.i += consumed;
        Ok(Some((arrow_of(tail, head), stroke, Some(label))))
    }
}

/// How long a complete link is, what its head is, and what it is drawn as.
fn match_link(body: &str) -> Option<(usize, u8, Stroke)> {
    let b = body.as_bytes();
    // `\s*\~\~[\~]+\s*` — three tildes at least, and no head at all.
    if body.starts_with("~~~") {
        let n = b.iter().take_while(|c| **c == b'~').count();
        return Some((n, 0, Stroke::Invisible));
    }
    // `\s*[xo<]?\-?\.+\-[xo>]?\s*` — the dotted shaft is the one with a `.` in it.
    let dotted_start = usize::from(b.first() == Some(&b'-'));
    let dots = b[dotted_start..].iter().take_while(|c| **c == b'.').count();
    if dots > 0 && b.get(dotted_start + dots) == Some(&b'-') {
        let mut len = dotted_start + dots + 1;
        let head = match b.get(len) {
            Some(c @ (b'x' | b'o' | b'>')) => {
                len += 1;
                *c
            }
            _ => 0,
        };
        return Some((len, head, Stroke::Dotted));
    }
    for (shaft, stroke) in [(b'=', Stroke::Thick), (b'-', Stroke::Normal)] {
        let n = b.iter().take_while(|c| **c == shaft).count();
        if n < 2 {
            continue;
        }
        match b.get(n) {
            Some(c @ (b'x' | b'o' | b'>')) => return Some((n + 1, *c, stroke)),
            // `\-\-+[-xo>]` — the run itself may supply the last character, but then there have
            // to be three of them. Two dashes and nothing else is a `START_LINK`.
            _ if n >= 3 => return Some((n, 0, stroke)),
            _ => {}
        }
    }
    None
}

/// How long a `START_LINK` is: exactly `--`, `==` or `-.`, then whitespace.
fn match_start_link(body: &str) -> Option<(usize, Stroke)> {
    for (text, stroke) in [
        ("--", Stroke::Normal),
        ("==", Stroke::Thick),
        ("-.", Stroke::Dotted),
    ] {
        if body.starts_with(text)
            && body[text.len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            return Some((text.len(), stroke));
        }
    }
    None
}

/// What the two ends together are drawn as.
fn arrow_of(tail: u8, head: u8) -> Arrow {
    match (tail, head) {
        (b'<', b'>') => Arrow::DoublePoint,
        (b'x', b'x') => Arrow::DoubleCross,
        (b'o', b'o') => Arrow::DoubleCircle,
        (b'<', _) => Arrow::Point,
        (b'x', _) => Arrow::Cross,
        (b'o', _) => Arrow::Circle,
        (_, b'>') => Arrow::Point,
        (_, b'x') => Arrow::Cross,
        (_, b'o') => Arrow::Circle,
        _ => Arrow::None,
    }
}

/// Reads a label out of a shape or a link, quoted or not, and says how many bytes it used and
/// what closed it.
fn read_label(after: &str) -> Result<(String, usize, &'static str), ParseError> {
    if let Some(rest) = after.strip_prefix("\"`") {
        let Some(end) = rest.find("`\"") else {
            return Err(ParseError::Invalid {
                line: 0,
                message: "an unclosed markdown string".to_string(),
            });
        };
        return Ok((rest[..end].trim().to_string(), end + 4, ""));
    }
    if let Some(rest) = after.strip_prefix('"') {
        let Some(end) = rest.find('"') else {
            return Err(ParseError::Invalid {
                line: 0,
                message: "an unclosed `\"`".to_string(),
            });
        };
        return Ok((rest[..end].trim().to_string(), end + 2, ""));
    }
    let end = after.find([']', ')', '}', '\n']).unwrap_or(after.len());
    Ok((after[..end].trim().to_string(), end, ""))
}

#[cfg(test)]
mod tests;

/// A block written a second time keeps whatever the second mention adds.
///
/// Only what was *said* is taken: writing `A` again after `A["label"]` must not blank the label,
/// which is what a plain overwrite would do, and `A space B` followed by `A("Start")` is exactly
/// that shape.
fn merge_node(into: &mut Node, from: Node) {
    if from.label != from.id {
        into.label = from.label;
    }
    if from.shape != Shape::Rect {
        into.shape = from.shape;
    }
    if from.width != 1 {
        into.width = from.width;
    }
    if from.arrow.is_some() {
        into.arrow = from.arrow;
    }
}
