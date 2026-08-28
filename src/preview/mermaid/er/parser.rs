//! mermaid `erDiagram` source in, [`ErDiagram`] out.
//!
//! # Reference, and where konoma leaves it
//!
//! The grammar is read out of `erDiagram.jison` and `erDb.ts` at `mermaid@11.17.2`. Four rules
//! decide what a reader sees and none of them is guessable:
//!
//! * **The lexer is `%options case-insensitive`.** `ERDIAGRAM`, `PK`, `pk`, `Only One` and
//!   `ZERO OR MORE` are all legal, and a parser that matches them case-sensitively silently
//!   refuses real diagrams. This is the only one of konoma's four mermaid grammars that works
//!   this way.
//! * **A cardinality has a symbol form and a word form for the same thing.** `||`, `one`,
//!   `only one` and `1` are one token; so are `}o`, `zero or more`, `zero or many`, `many`,
//!   `many(0)` and `0+`. Reading only the symbols would leave
//!   `CAR 1 to zero or more NAMED-DRIVER` unparsed — and it is in mermaid's own documentation.
//! * **The mirrored spelling is the picture.** `||--o{` puts `||` next to the left entity and
//!   `{` next to the right one, so the character nearest an entity is the mark drawn nearest it.
//!   That is what decides which end a crow's foot goes on, and getting it backwards inverts the
//!   meaning of every relationship in the diagram.
//! * **An attribute block is its own lexer state.** Inside `{ … }` the tokens are
//!   `ATTRIBUTE_WORD`, `ATTRIBUTE_KEY` and `COMMENT`, so `string name PK "the key"` is four
//!   things and not one, and a `}` closes the block wherever it appears.
//!
//! # Deliberate divergences
//!
//! * **`direction` is a statement only when the statement starts with it**, as in the other two
//!   parsers. mermaid's `.*direction\s+LR[^\n]*` would turn an entity called
//!   `SHIP_direction_LR` into a direction statement.
//! * **a `direction` inside a `subgraph` does not turn the diagram sideways**, for the reason
//!   [`render::lay_out_spec`] gives at length. mermaid's own grammar already refuses to apply it
//!   at depth (`if (!yy.subgraphDepth)`), so the difference is only about the nested case.
//! * **`class`, `classDef` and `style` do not bring an entity into existence.**
//! * **`title` is not a statement.** The ER lexer has no `title` rule at all — the `title
//!   title_value` production is unreachable — so `title X` would lex as two words. konoma reads a
//!   title only from front matter, which `preprocess` handles for every diagram kind.
//!
//! [`render::lay_out_spec`]: crate::preview::mermaid::render::lay_out_spec

use super::model::{
    Attribute, Cardinality, Direction, ErDiagram, Identification, ParseError, Relationship,
    SubGraph,
};
use crate::preview::mermaid::flowchart::preprocess::preprocess;
use crate::preview::mermaid::flowchart::text::decode_label;

/// Ceiling on how many relationships one diagram may declare, mirroring mermaid's `maxEdges`.
const MAX_RELATIONSHIPS: usize = 500;

/// [`MAX_RELATIONSHIPS`], for the test that checks the ceiling really holds.
#[cfg(test)]
pub const MAX_RELATIONSHIPS_FOR_TESTS: usize = MAX_RELATIONSHIPS;

/// How deep `subgraph` may nest before the parser stops opening frames.
const MAX_DEPTH: usize = 32;

/// Parses a mermaid ER diagram.
///
/// # Errors
///
/// Only where drawing would be worse than not drawing — see [`ParseError`].
pub fn parse(src: &str) -> Result<ErDiagram, ParseError> {
    let pre = preprocess(src);
    let lines: Vec<&str> = pre.text.split('\n').collect();
    let header = find_header(&lines)?;

    let mut scanner = Scanner::new(pre.title);
    scanner.line(&header.trailing, header.line_index + 1)?;
    for (i, line) in lines.iter().enumerate().skip(header.line_index + 1) {
        scanner.line(line, i + 1)?;
    }
    scanner.finish()
}

/// Whether this source is an ER diagram. The routing predicate
/// `preview::markdown::mermaid_to_svg` asks; see [`super::super::class::is_class_diagram`] for
/// why it reads the header rather than trying [`parse`].
pub fn is_er_diagram(src: &str) -> bool {
    let pre = preprocess(src);
    let lines: Vec<&str> = pre.text.split('\n').collect();
    find_header(&lines).is_ok()
}

// ---------------------------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------------------------

struct Header {
    line_index: usize,
    trailing: String,
}

fn find_header(lines: &[&str]) -> Result<Header, ParseError> {
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let t = line.trim_start();
        // Case-insensitive, because the whole lexer is.
        if starts_ci(t, "erDiagram") {
            let rest = &t[9..];
            if !rest.chars().next().is_some_and(is_plain_id_char) {
                return Ok(Header {
                    line_index: i,
                    trailing: rest.to_string(),
                });
            }
        }
        let header: String = t
            .chars()
            .take_while(|c| !c.is_whitespace())
            .take(40)
            .collect();
        return Err(ParseError::NotAnErDiagram { header });
    }
    Err(ParseError::Empty)
}

fn is_plain_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `s` starts with `kw`, ignoring ASCII case.
///
/// Written out rather than `s[..kw.len()].eq_ignore_ascii_case(kw)` because that slice panics the
/// moment `s` starts with a multi-byte character — and this lexer is case-insensitive everywhere,
/// so the comparison happens on every line. `"This ❤ Unicode"` found it.
fn starts_ci(s: &str, kw: &str) -> bool {
    s.len() >= kw.len() && s.is_char_boundary(kw.len()) && s[..kw.len()].eq_ignore_ascii_case(kw)
}

/// Whether `s` ends with `kw`, ignoring ASCII case, on a character boundary.
fn ends_ci(s: &str, kw: &str) -> bool {
    let Some(cut) = s.len().checked_sub(kw.len()) else {
        return false;
    };
    s.is_char_boundary(cut) && s[cut..].eq_ignore_ascii_case(kw)
}

// ---------------------------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------------------------

struct OpenBlock {
    entity: String,
    line: usize,
}

struct Scanner {
    out: ErDiagram,
    /// The entity whose attribute block is open, if any.
    block: Option<OpenBlock>,
    /// Open `subgraph`s, outermost first.
    frames: Vec<(String, usize)>,
    acc_descr_block: bool,
    relationships: usize,
    subgraphs: usize,
    claimed: std::collections::HashSet<String>,
}

impl Scanner {
    fn new(title: Option<String>) -> Scanner {
        Scanner {
            out: ErDiagram::new(Direction::default(), title),
            block: None,
            frames: Vec::new(),
            acc_descr_block: false,
            relationships: 0,
            subgraphs: 0,
            claimed: std::collections::HashSet::new(),
        }
    }

    fn frame(&self) -> Option<String> {
        self.frames.last().map(|(id, _)| id.clone())
    }

    fn line(&mut self, raw: &str, line: usize) -> Result<(), ParseError> {
        if self.acc_descr_block {
            match raw.find('}') {
                Some(end) => {
                    self.push_acc_descr(&raw[..end]);
                    self.acc_descr_block = false;
                }
                None => self.push_acc_descr(raw),
            }
            return Ok(());
        }
        // Inside `ENTITY { … }` every line is an attribute, and a `}` closes the block.
        if let Some(open) = &self.block {
            let entity = open.entity.clone();
            match raw.find('}') {
                Some(end) => {
                    self.attribute(&entity, &raw[..end], line)?;
                    self.block = None;
                    let rest = raw[end + 1..].to_string();
                    return self.line(&rest, line);
                }
                None => return self.attribute(&entity, raw, line),
            }
        }

        let text = raw.trim();
        if text.is_empty() {
            return Ok(());
        }
        if let Some(rest) = acc_descr_block_start(text) {
            match rest.find('}') {
                Some(end) => self.push_acc_descr(&rest[..end]),
                None => {
                    self.push_acc_descr(rest);
                    self.acc_descr_block = true;
                }
            }
            return Ok(());
        }
        // `{` opens an attribute block; everything before it is the entity declaration.
        if let Some(i) = block_open(text)? {
            let head = text[..i].trim().to_string();
            let rest = text[i + 1..].to_string();
            let Some(id) = self.declare_entity(&head, line)? else {
                return Ok(());
            };
            self.block = Some(OpenBlock { entity: id, line });
            return self.line(&rest, line);
        }
        self.statement(text, line)
    }

    fn push_acc_descr(&mut self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        match &mut self.out.acc_descr {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(t);
            }
            None => self.out.acc_descr = Some(t.to_string()),
        }
    }

    fn statement(&mut self, t: &str, line: usize) -> Result<(), ParseError> {
        if let Some(dir) = direction_statement(t) {
            if self.frames.is_empty() {
                self.out.direction = dir;
            }
            return Ok(());
        }
        if let Some(v) = keyword_value(t, "accTitle") {
            self.out.acc_title = Some(v);
            return Ok(());
        }
        if let Some(v) = keyword_value(t, "accDescr") {
            self.out.acc_descr = Some(v);
            return Ok(());
        }
        if t.eq_ignore_ascii_case("accDescr") || starts_with_word(t, "accDescr") {
            return Ok(());
        }
        // `classDef`/`class`/`style` are recognised so they cannot become entities (§2-3), and —
        // since `render::style` gained a cascade every diagram language can feed — read for real
        // rather than dropped. An id these name that is not (yet) a known entity is simply not
        // found by `entity_mut`, the same "must not conjure a box" rule the flowchart parser
        // follows for its own `class`/`style` statements.
        if let Some(rest) = word_rest(t, "classDef") {
            self.class_def(rest);
            return Ok(());
        }
        if let Some(rest) = word_rest(t, "class") {
            self.class_statement(rest);
            return Ok(());
        }
        if let Some(rest) = word_rest(t, "style") {
            self.style_statement(rest);
            return Ok(());
        }
        if t.eq_ignore_ascii_case("end") {
            self.frames.pop();
            return Ok(());
        }
        if starts_with_word(t, "subgraph") {
            return self.open_subgraph(&t[8..], line);
        }
        if let Some((r, from_css, to_css)) = read_relationship(t) {
            self.add_relationship(r, &from_css, &to_css);
            return Ok(());
        }
        // `ENTITY`, `ENTITY["alias"]`, `ENTITY:::css` — a declaration on its own.
        self.declare_entity(t, line)?;
        Ok(())
    }

    /// `classDef name decl,decl` — mirrors the flowchart parser's own `class_def`, and reuses its
    /// comma-splitter so a literal comma still escapes the same way (`\,`) in either language.
    fn class_def(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let Some((names, styles)) = rest.split_once(char::is_whitespace) else {
            return;
        };
        let styles = crate::preview::mermaid::flowchart::parser::split_styles(styles);
        for name in names.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            self.out
                .class_defs
                .push(crate::preview::mermaid::flowchart::ClassDef {
                    name: name.to_string(),
                    styles: styles.clone(),
                });
        }
    }

    /// `class id,id name` — a class assigned separately from `:::`. Applied only to an entity that
    /// already exists, the same "must not conjure a box" rule [`Self::declare_entity`]'s `:::`
    /// handling follows.
    fn class_statement(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let Some((ids, class_name)) = rest.split_once(char::is_whitespace) else {
            return;
        };
        let class_name = class_name.trim();
        if class_name.is_empty() {
            return;
        }
        for id in ids.split(',') {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            if let Some(e) = self.out.entity_mut(id) {
                if !e.css_classes.iter().any(|c| c == class_name) {
                    e.css_classes.push(class_name.to_string());
                }
            }
        }
    }

    /// `style id decl,decl` — declarations for one entity directly, with no named class.
    fn style_statement(&mut self, rest: &str) {
        let rest = rest.trim().trim_end_matches(';');
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return;
        };
        let styles = crate::preview::mermaid::flowchart::parser::split_styles(styles);
        if let Some(e) = self.out.entity_mut(id.trim()) {
            e.own_styles.extend(styles);
        }
    }

    /// `subgraph id`, `subgraph id[title]`.
    ///
    /// `subgraphHeader` is `SUBGRAPH entityName SQS subgraphTitle SQE separator`: the bracket is
    /// followed by the end of the line and nothing else — not even a style separator, which the
    /// entity forms do allow. Anything after it is refused for the reason [`Self::declare_entity`]
    /// gives.
    fn open_subgraph(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        if self.frames.len() > MAX_DEPTH {
            return Ok(());
        }
        let t = rest.trim();
        let (name, title) = match read_bracket_title(t) {
            Alias::Absent => (t.to_string(), None),
            Alias::Unclosed => {
                return Err(ParseError::UnclosedBracket {
                    text: format!("subgraph {t}"),
                    line,
                })
            }
            Alias::Present {
                name,
                title,
                rest: "",
            } => (name, Some(title)),
            Alias::Present { .. } => {
                return Err(ParseError::BracketIsNotADeclaration {
                    text: format!("subgraph {t}"),
                    line,
                })
            }
        };
        let id = if name.trim().is_empty() {
            self.subgraphs += 1;
            format!("subGraph{}", self.subgraphs - 1)
        } else {
            unquote(&name)
        };
        let title = title
            .map(|t| decode_label(&t))
            .unwrap_or_else(|| id.clone());
        let parent = self.frame();
        if !self.out.subgraphs.iter().any(|s| s.id == id) {
            self.out.subgraphs.push(SubGraph {
                id: id.clone(),
                title,
                members: Vec::new(),
                parent: parent.clone(),
            });
            if let Some(p) = &parent {
                let child = id.clone();
                if let Some(s) = self.out.subgraphs.iter_mut().find(|s| &s.id == p) {
                    if !s.members.contains(&child) {
                        s.members.push(child);
                    }
                }
            }
        }
        self.frames.push((id, line));
        Ok(())
    }

    /// `ENTITY`, `ENTITY["alias"]`, `"Quoted Name"`, with an optional `:::cssClass`.
    ///
    /// # A bracket that is not an alias makes the whole diagram refuse
    ///
    /// `erDiagram.jison` puts `SQS entityName SQE` in the **declaration** rules only
    /// (`entityName SQS entityName SQE`, plus the same with `STYLE_SEPARATOR idList` and/or an
    /// attribute block after it). **No relationship rule takes a bracket**, and a `subgraph`
    /// header's bracket must be followed by a separator. So a line like
    /// `p[Person] ||--o| a["Customer Account"] : has` is not a diagram mermaid draws differently
    /// — it is one mermaid throws on.
    ///
    /// Anything else that reaches here is prose, and this parser's rule for prose is that it
    /// declares nothing (`a_bare_word_declares_nothing`). But *silently* declaring nothing is the
    /// wrong answer for a statement that is clearly trying to be one: the reader gets a diagram
    /// with an entity or an edge quietly missing, which is the same lie as a phantom node wearing
    /// the other face. **So the diagram is refused**, `mermaid_to_svg` gets `None`, and the fence
    /// degrades to the text diagram with the source in it — konoma's standing promise for a
    /// diagram it cannot draw, and the outcome mermaid's own syntax error produces.
    ///
    /// Skipping just the statement was the alternative and it is worse in exactly the way
    /// konoma's "no symptomatic fixes" rule describes: the symptom (a wrong label) goes away and
    /// the reader is left with a drawing that disagrees with the source and says nothing about it.
    ///
    /// **Known divergence.** `A[one] B[two]` on one line is two declarations upstream — `line` is
    /// `statement` and statements need no separator between them — and konoma refuses it. This
    /// scanner is line-based and has nowhere to put a second statement, and of the two ways to be
    /// wrong, refusing is the one that does not invent a label.
    fn declare_entity(&mut self, raw: &str, line: usize) -> Result<Option<String>, ParseError> {
        let t = raw.trim();
        if t.is_empty() {
            return Ok(None);
        }
        let (head, alias, mut css) = match read_bracket_title(t) {
            Alias::Absent => (t.to_string(), None, Vec::new()),
            Alias::Unclosed => {
                return Err(ParseError::UnclosedBracket {
                    text: t.to_string(),
                    line,
                })
            }
            Alias::Present { name, title, rest } => {
                // Only `:::classes` may follow the alias. The `{` of an attribute block never
                // reaches here — `Scanner::line` splits the statement at it before calling.
                let (before, classes) = split_style_separator(rest);
                if !before.trim().is_empty() {
                    return Err(ParseError::BracketIsNotADeclaration {
                        text: t.to_string(),
                        line,
                    });
                }
                (name, Some(title), classes)
            }
        };
        let (name, own_css) = split_style_separator(&head);
        css.extend(own_css);
        let id = unquote(&name);
        if id.is_empty() || !(is_quoted(&name) || is_entity_name(&id)) {
            return Ok(None);
        }
        let alias = alias.map(|a| decode_label(&unquote(&a)));
        self.out.intern(&id, alias.as_deref());
        self.place(&id);
        if !css.is_empty() {
            if let Some(e) = self.out.entity_mut(&id) {
                for name in css {
                    if !e.css_classes.contains(&name) {
                        e.css_classes.push(name);
                    }
                }
            }
        }
        Ok(Some(id))
    }

    /// One line of an attribute block: `type name [PK, FK] ["comment"]`.
    fn attribute(&mut self, entity: &str, raw: &str, line: usize) -> Result<(), ParseError> {
        let t = raw.trim();
        if t.is_empty() {
            return Ok(());
        }
        let (words, comment) = split_comment(t, line)?;
        let mut parts = words.split_whitespace();
        let (Some(kind), Some(name)) = (parts.next(), parts.next()) else {
            // `attribute : attributeType attributeName` — a single word is not an attribute.
            return Ok(());
        };
        let keys: Vec<String> = parts
            .flat_map(|p| p.split(','))
            .map(|k| k.trim().to_uppercase())
            .filter(|k| matches!(k.as_str(), "PK" | "FK" | "UK"))
            .collect();
        if let Some(e) = self.out.entity_mut(entity) {
            e.attributes.push(Attribute {
                kind: kind.to_string(),
                name: name.to_string(),
                keys,
                comment,
            });
        }
        Ok(())
    }

    fn place(&mut self, id: &str) {
        if self.claimed.contains(id) {
            return;
        }
        self.claimed.insert(id.to_string());
        let Some(frame) = self.frame() else { return };
        if let Some(e) = self.out.entity_mut(id) {
            e.parent = Some(frame.clone());
        }
        if let Some(s) = self.out.subgraphs.iter_mut().find(|s| s.id == frame) {
            s.members.push(id.to_string());
        }
    }

    fn add_relationship(&mut self, mut r: Relationship, from_css: &[String], to_css: &[String]) {
        if self.relationships >= MAX_RELATIONSHIPS {
            return;
        }
        // An endpoint may name a subgraph (`title1 ||--|| title2 : links`), in which case it is
        // not an entity and must not be interned as one.
        for (id, css) in [(r.from.clone(), from_css), (r.to.clone(), to_css)] {
            if self.out.subgraphs.iter().any(|s| s.id == id) {
                continue;
            }
            self.out.intern(&id, None);
            self.place(&id);
            if let Some(e) = self.out.entity_mut(&id) {
                for name in css {
                    if !e.css_classes.contains(name) {
                        e.css_classes.push(name.clone());
                    }
                }
            }
        }
        r.id = format!("r{}", self.relationships);
        self.relationships += 1;
        self.out.relationships.push(r);
    }

    fn finish(mut self) -> Result<ErDiagram, ParseError> {
        if let Some(open) = &self.block {
            return Err(ParseError::UnclosedBlock {
                id: open.entity.clone(),
                line: open.line,
            });
        }
        if let Some((id, line)) = self.frames.first() {
            return Err(ParseError::UnclosedSubgraph {
                id: id.clone(),
                line: *line,
            });
        }
        let known: Vec<String> = self.out.entities.iter().map(|e| e.id.clone()).collect();
        let frames: Vec<String> = self.out.subgraphs.iter().map(|s| s.id.clone()).collect();
        let placed = |id: &String| known.iter().any(|k| k == id) || frames.iter().any(|f| f == id);
        self.out
            .relationships
            .retain(|r| placed(&r.from) && placed(&r.to));
        if self.out.entities.is_empty() {
            return Err(ParseError::NoEntities);
        }
        Ok(self.out)
    }
}

// ---------------------------------------------------------------------------------------------
// Relationships
// ---------------------------------------------------------------------------------------------

/// The word spellings of a cardinality, longest first so `one or more` beats `one`.
const CARDINALITY_WORDS: &[(&str, Cardinality)] = &[
    ("one or zero", Cardinality::ZeroOrOne),
    ("one or more", Cardinality::OneOrMore),
    ("one or many", Cardinality::OneOrMore),
    ("zero or one", Cardinality::ZeroOrOne),
    ("zero or more", Cardinality::ZeroOrMore),
    ("zero or many", Cardinality::ZeroOrMore),
    ("only one", Cardinality::OnlyOne),
    ("many(0)", Cardinality::ZeroOrMore),
    ("many(1)", Cardinality::OneOrMore),
    ("many", Cardinality::ZeroOrMore),
    ("one", Cardinality::OnlyOne),
    ("1+", Cardinality::OneOrMore),
    ("0+", Cardinality::ZeroOrMore),
    ("1", Cardinality::OnlyOne),
];

/// The symbol spellings written to the **left** of the line, next to the first entity.
const CARDINALITY_LEFT: &[(&str, Cardinality)] = &[
    ("||", Cardinality::OnlyOne),
    ("|o", Cardinality::ZeroOrOne),
    ("}o", Cardinality::ZeroOrMore),
    ("}|", Cardinality::OneOrMore),
    ("u", Cardinality::MdParent),
];

/// The symbol spellings written to the **right** of the line, next to the second entity.
const CARDINALITY_RIGHT: &[(&str, Cardinality)] = &[
    ("||", Cardinality::OnlyOne),
    ("o|", Cardinality::ZeroOrOne),
    ("o{", Cardinality::ZeroOrMore),
    ("|{", Cardinality::OneOrMore),
];

/// Reads `CUSTOMER ||--o{ ORDER : places`, or `None` when the statement has no relationship
/// operator in it.
pub fn read_relationship(t: &str) -> Option<(Relationship, Vec<String>, Vec<String>)> {
    let (head, label) = split_role(t);
    let (start, end, identification) = find_operator(head)?;

    let left = head[..start].trim_end();
    let right = head[end..].trim_start();
    let (from_raw, from_cardinality) = strip_trailing_cardinality(left);
    let (to_cardinality, to_raw) = strip_leading_cardinality(right);

    let (from_name, from_css) = split_style_separator(from_raw.trim());
    let (to_name, to_css) = split_style_separator(to_raw.trim());
    let from = unquote(&from_name);
    let to = unquote(&to_name);
    let usable = |raw: &str, id: &str| !id.is_empty() && (is_quoted(raw) || is_entity_name(id));
    if !usable(&from_name, &from) || !usable(&to_name, &to) {
        return None;
    }
    Some((
        Relationship {
            id: String::new(),
            from,
            to,
            from_cardinality,
            to_cardinality,
            identification,
            label,
        },
        from_css,
        to_css,
    ))
}

/// Splits the `: role` off the end of a statement.
fn split_role(t: &str) -> (&str, Option<String>) {
    let bytes = t.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => quoted = !quoted,
            b':' if !quoted => {
                if t[i..].starts_with(":::") {
                    i += 3;
                    continue;
                }
                let role = t[i + 1..].trim();
                let role = unquote(role);
                return (
                    &t[..i],
                    (!role.trim().is_empty()).then(|| decode_label(&role)),
                );
            }
            _ => {}
        }
        i += 1;
    }
    (t, None)
}

/// The relationship operator: a symbol (`--`, `..`, `.-`, `-.`) or a word (`to`,
/// `optionally to`). Returns its byte range and what it means.
fn find_operator(s: &str) -> Option<(usize, usize, Identification)> {
    let bytes = s.as_bytes();
    let mut quoted = false;
    // Walked by character rather than by byte: `一 ||--|| 二` is a legal ER diagram, and slicing
    // two bytes out of a three-byte character panics.
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if i + 1 < bytes.len() {
            let kind = match (bytes[i], bytes[i + 1]) {
                (b'-', b'-') => Some(Identification::Identifying),
                (b'.', b'.') | (b'.', b'-') | (b'-', b'.') => Some(Identification::NonIdentifying),
                _ => None,
            };
            if let Some(kind) = kind {
                return Some((i, i + 2, kind));
            }
        }
        for (word, kind) in [
            ("optionally to", Identification::NonIdentifying),
            ("to", Identification::Identifying),
        ] {
            if word_at(s, i, word) {
                return Some((i, i + word.len(), kind));
            }
        }
    }
    None
}

/// Whether `word` sits at `i` with a word boundary on both sides, ignoring case.
fn word_at(s: &str, i: usize, word: &str) -> bool {
    if !s.is_char_boundary(i) || i + word.len() > s.len() {
        return false;
    }
    if !s.is_char_boundary(i + word.len()) || !s[i..i + word.len()].eq_ignore_ascii_case(word) {
        return false;
    }
    let before = s[..i].chars().next_back();
    let after = s[i + word.len()..].chars().next();
    !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
}

/// Takes the cardinality off the end of the text to the left of the operator.
fn strip_trailing_cardinality(s: &str) -> (String, Cardinality) {
    let t = s.trim_end();
    for (token, card) in CARDINALITY_LEFT {
        if let Some(rest) = t.strip_suffix(*token) {
            // `u` is also a letter, and mermaid's own rule (`u(?=[.\-|])`) only reads it as a
            // mark when the operator follows it directly.
            if *token == "u" && rest.chars().next_back().is_some_and(is_name_char) {
                continue;
            }
            if !rest.trim().is_empty() {
                return (rest.trim().to_string(), *card);
            }
        }
    }
    for (word, card) in CARDINALITY_WORDS {
        if ends_ci(t, word) {
            let cut = t.len() - word.len();
            let rest = &t[..cut];
            if rest.trim().is_empty() || rest.chars().next_back().is_some_and(is_name_char) {
                continue;
            }
            return (rest.trim().to_string(), *card);
        }
    }
    (t.to_string(), Cardinality::default())
}

/// Takes the cardinality off the front of the text to the right of the operator.
fn strip_leading_cardinality(s: &str) -> (Cardinality, String) {
    let t = s.trim_start();
    for (token, card) in CARDINALITY_RIGHT {
        if let Some(rest) = t.strip_prefix(*token) {
            if !rest.trim().is_empty() {
                return (*card, rest.trim().to_string());
            }
        }
    }
    for (word, card) in CARDINALITY_WORDS {
        if starts_ci(t, word) {
            let rest = &t[word.len()..];
            if rest.trim().is_empty() || rest.chars().next().is_some_and(is_name_char) {
                continue;
            }
            return (*card, rest.trim().to_string());
        }
    }
    (Cardinality::default(), t.to_string())
}

/// The characters `UNICODE_TEXT` accepts in an entity name: word characters, `-`, `*`, `.`, and
/// anything outside ASCII.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '*' | '.') || !c.is_ascii()
}

/// Whether a name is something the ER lexer could have produced, once its quotes are off.
///
/// The check exists so a line that is prose rather than a declaration cannot become an entity:
/// `ENTITY_NAME` is `\"[^"%\r\n\v\b\\]+\"` and `UNICODE_TEXT` has no spaces in it, so a bare
/// phrase with a space is not a name.
fn is_entity_name(id: &str) -> bool {
    !id.is_empty() && id.chars().all(is_name_char)
}

/// Whether the text is one `"…"`. A quoted name is `ENTITY_NAME`, which *does* allow spaces —
/// mermaid's own `erDiagram\n "This ❤ Unicode"` is one entity — so the no-spaces rule above
/// applies to the bare `UNICODE_TEXT` form only.
fn is_quoted(s: &str) -> bool {
    let t = s.trim();
    t.len() >= 2 && t.starts_with('"') && t.ends_with('"')
}

// ---------------------------------------------------------------------------------------------
// Small readers
// ---------------------------------------------------------------------------------------------

/// Byte index of the `{` that opens an attribute block, if the line has one outside quotes.
///
/// **A `{` preceded by `o` or `|` is a cardinality, not a block.** `o\{` and `|\{` are their own
/// lexer rules and win over the bare `"{"` by longest match, so `CUSTOMER ||--o{ ORDER : places`
/// opens nothing — reading its `{` as a block start swallows the rest of the diagram, which is
/// exactly what the first draft of this parser did.
fn block_open(s: &str) -> Result<Option<usize>, ParseError> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => quoted = !quoted,
            b'{' if !quoted => {
                if i > 0 && matches!(bytes[i - 1], b'o' | b'O' | b'|') {
                    i += 1;
                    continue;
                }
                return Ok(Some(i));
            }
            _ => {}
        }
        i += 1;
    }
    Ok(None)
}

/// Splits an attribute line into its words and its quoted comment.
fn split_comment(s: &str, line: usize) -> Result<(String, String), ParseError> {
    let Some(i) = s.find('"') else {
        return Ok((s.to_string(), String::new()));
    };
    let Some(end) = s[i + 1..].find('"') else {
        return Err(ParseError::UnclosedString { line });
    };
    Ok((
        s[..i].to_string(),
        decode_label(&s[i + 1..i + 1 + end]).to_string(),
    ))
}

/// What a statement reads as once its `NAME[title]` has been taken off the front.
enum Alias<'a> {
    /// No `[` outside quotes — the whole statement is the name.
    Absent,
    /// `NAME [title] rest`, with `rest` whatever follows the closing bracket.
    Present {
        /// Everything before the `[`.
        name: String,
        /// Everything between the brackets.
        title: String,
        /// Everything after the `]`, trimmed. Empty when the bracket ends the statement.
        rest: &'a str,
    },
    /// A `[` outside quotes that no `]` outside quotes closes.
    Unclosed,
}

/// Splits `NAME["title"]` / `NAME [title]` into the name, the title, and what follows.
///
/// # Both brackets are found outside quotes, and the closing one is the **first** that matches
///
/// `ENTITY_NAME` is `\"[^"%\r\n\v\b\\]+\"`, so a name or a title may hold a bracket of its own:
/// `A["a ] b"]` is one entity and `"a [ b"[t]` is another. Scanning with `find`/`rfind` gets both
/// of those wrong, and `rfind` gets something worse wrong — it reads to the **last** `]` on the
/// line, so a bracket pair can span everything that comes after the declaration. That is what
/// made `p[Person] ||--o| a["Customer Account"] : has` — a line mermaid's grammar has no rule for
/// — into one entity labelled `Person] ||--o| a["Customer Account"`, with the relationship gone.
fn read_bracket_title(s: &str) -> Alias<'_> {
    let t = s.trim();
    let mut quoted = false;
    let mut open = None;
    for (i, c) in t.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => {
                open = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(open) = open else {
        return Alias::Absent;
    };
    let mut quoted = false;
    let mut close = None;
    for (i, c) in t[open + 1..].char_indices() {
        match c {
            '"' => quoted = !quoted,
            ']' if !quoted => {
                close = Some(open + 1 + i);
                break;
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return Alias::Unclosed;
    };
    Alias::Present {
        name: t[..open].trim().to_string(),
        title: t[open + 1..close].trim().to_string(),
        rest: t[close + 1..].trim(),
    }
}

/// Removes one enclosing pair of `"`.
fn unquote(s: &str) -> String {
    let t = s.trim();
    match (t.strip_prefix('"'), t.strip_suffix('"')) {
        (Some(_), Some(_)) if t.len() >= 2 => t[1..t.len() - 1].to_string(),
        _ => t.to_string(),
    }
}

/// Splits `id:::className` into the two.
fn split_style_separator(s: &str) -> (String, Vec<String>) {
    match s.find(":::") {
        Some(i) => {
            let classes: Vec<String> = s[i + 3..]
                .split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            (s[..i].trim().to_string(), classes)
        }
        None => (s.trim().to_string(), Vec::new()),
    }
}

/// `kw` at the start of the statement, followed by whitespace. Case-insensitive, like the lexer.
fn starts_with_word(s: &str, kw: &str) -> bool {
    starts_ci(s, kw) && s[kw.len()..].starts_with(char::is_whitespace)
}

/// [`starts_with_word`], but returns what follows the keyword (trimmed) instead of just `bool`.
fn word_rest<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    if starts_with_word(s, kw) {
        Some(s[kw.len()..].trim())
    } else {
        None
    }
}

/// `accTitle: text` / `accDescr: text`.
fn keyword_value(s: &str, kw: &str) -> Option<String> {
    if !starts_ci(s, kw) {
        return None;
    }
    let rest = s[kw.len()..].trim_start();
    let rest = rest.strip_prefix(':')?;
    Some(rest.trim().to_string())
}

/// `direction LR` and friends — but only when the statement *starts* with the word.
fn direction_statement(s: &str) -> Option<Direction> {
    if !starts_with_word(s, "direction") {
        return None;
    }
    Direction::parse(s[9..].trim())
}

/// The text after the `{` of an `accDescr {`, when that is what the line is.
fn acc_descr_block_start(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if !starts_ci(t, "accDescr") {
        return None;
    }
    t[8..].trim_start().strip_prefix('{')
}
