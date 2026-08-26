//! mermaid `classDiagram` source in, [`ClassDiagram`] out.
//!
//! # Reference, and where konoma leaves it
//!
//! The grammar is read out of `classDiagram.jison`, `classDb.ts` and `classTypes.ts` at
//! `mermaid@11.17.2`, not out of a rendered picture — `docs/FEATURE-MERMAID-RENDERER.md` §0-2
//! tracks the *syntax* and nothing else. The rules that cannot be guessed are cited where they
//! are implemented. Four of them decide what a reader sees, so they are worth stating up front:
//!
//! * **A relationship is three tokens, not one arrow.** `relation : relationType? lineType
//!   relationType?`, so `<|--`, `--|>`, `*--*`, `--`, `..`, `()--` and `--()` are all legal and
//!   are all built the same way. Matching a fixed list of arrow spellings would miss most of them.
//! * **A generic is part of the *name*, not of the class.** `splitClassNameAndType` cuts
//!   `Square~Shape~` into the class `Square` and the type `Shape`, so `Square~Shape~ --> Foo` and
//!   `Square --> Foo` name the same class. Keeping the `~Shape~` in the id would draw two boxes.
//! * **A member is taken apart, not printed.** `ClassMember.parseMember` pulls a visibility, a
//!   name, parameters, a return type and a classifier out of one line, and the drawn text is
//!   rebuilt from those. Printing the line as written would put `+deposit(amount) bool` where
//!   mermaid puts `+deposit(amount) : bool`.
//! * **A bare identifier declares nothing.** mermaid's `memberStatement : className` has an empty
//!   action, so a stray word is *not* a class. That is the §2-3 rule — "parsed but not drawn" —
//!   and it is what stops a line of prose in a fence from becoming a box.
//!
//! # Deliberate divergences
//!
//! Each is a place where mermaid's own behaviour is a quiet wrong answer rather than a feature,
//! and each follows a decision one of the earlier stages already took:
//!
//! * **`direction` is a statement only when the statement starts with it.** mermaid's lexer
//!   matches `.*direction\s+TB[^\n]*`, so `Foo : go direction TB` becomes a direction statement
//!   and the member disappears. Same divergence as [`state::parser`].
//! * **a `direction` inside a `namespace` does not turn the diagram sideways**, for the reason
//!   [`render::lay_out_spec`] gives at length.
//! * **`cssClass`, `style`, `click`, `link` and `callback` do not bring a class into existence.**
//!   mermaid's `setCssClass` silently adds one for an unknown id, so a typo grows a box.
//! * **a `--o` whose `o` is followed by a word character is not an aggregation.** mermaid's
//!   `\s*o` rule makes `A --o orange` an aggregation pointing at a class called `range`; konoma
//!   only reads `o` as a mark when what follows it is not part of an identifier, so
//!   `A --oops` reaches the class `oops`. It cannot fix `A --o orange`, which is genuinely
//!   ambiguous, and that case is pinned by a test so the ambiguity is recorded rather than hidden.
//! * **three or more `-` (or `.`) in a row are one line token.** mermaid lexes `A --- B` as a
//!   line plus a `MINUS` that becomes part of the next class's name, giving a class called `-B`.
//!
//! [`state::parser`]: crate::preview::mermaid::state::parser
//! [`render::lay_out_spec`]: crate::preview::mermaid::render::lay_out_spec

use super::model::{
    ClassDiagram, Direction, Line, Marker, Member, MemberKind, Namespace, Note, ParseError,
    Relation,
};
use crate::preview::mermaid::flowchart::preprocess::preprocess;
use crate::preview::mermaid::flowchart::text::decode_label;

/// Ceiling on how many relationships one diagram may declare, mirroring mermaid's `maxEdges`.
const MAX_RELATIONS: usize = 500;

/// [`MAX_RELATIONS`], for the test that checks the ceiling really holds.
#[cfg(test)]
pub const MAX_RELATIONS_FOR_TESTS: usize = MAX_RELATIONS;

/// How deep `namespace X { … }` may nest before the parser stops opening frames.
const MAX_DEPTH: usize = 32;

/// Parses a mermaid class diagram.
///
/// # Errors
///
/// Only where drawing would be worse than not drawing — see [`ParseError`].
pub fn parse(src: &str) -> Result<ClassDiagram, ParseError> {
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

/// Whether this source is a class diagram — i.e. whether its header keyword says so.
///
/// This is the routing predicate `preview::markdown::mermaid_to_svg` asks. Like its two siblings
/// it is deliberately *not* "does [`parse`] succeed": a class diagram this parser refuses is
/// still konoma's to answer for, and handing it to a second renderer would let that renderer
/// paint over a bug in the new one (§6).
pub fn is_class_diagram(src: &str) -> bool {
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

/// Longest first, so `classDiagram` cannot claim the `classDiagram-v2` prefix.
const CLASS_KEYWORDS: &[&str] = &["classDiagram-v2", "classDiagram"];

fn find_header(lines: &[&str]) -> Result<Header, ParseError> {
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let t = line.trim_start();
        for kw in CLASS_KEYWORDS {
            let Some(rest) = t.strip_prefix(kw) else {
                continue;
            };
            // `classDiagrams` is a word, not a header.
            if rest.chars().next().is_some_and(is_plain_id_char) {
                continue;
            }
            return Ok(Header {
                line_index: i,
                trailing: rest.to_string(),
            });
        }
        let header: String = t
            .chars()
            .take_while(|c| !c.is_whitespace())
            .take(40)
            .collect();
        return Err(ParseError::NotAClassDiagram { header });
    }
    Err(ParseError::Empty)
}

fn is_plain_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------------------------

/// One open `{ … }`.
enum Frame {
    /// `namespace X {` — statements inside it belong to the namespace.
    Namespace { id: String, line: usize },
    /// `class X {` — every line inside it is a member of `X`, whatever it looks like.
    Body { id: String, line: usize },
    /// A `{` after something konoma could not read. Its contents land in the enclosing document,
    /// so nothing is invented from a brace whose head was not a declaration.
    Transparent { line: usize },
}

struct Scanner {
    out: ClassDiagram,
    stack: Vec<Frame>,
    acc_descr_block: bool,
    relations: usize,
    notes: usize,
    /// The namespace each id was first written in — a class can only be in one frame.
    claimed: std::collections::HashSet<String>,
}

impl Scanner {
    fn new(title: Option<String>) -> Scanner {
        Scanner {
            out: ClassDiagram::new(Direction::default(), title),
            stack: Vec::new(),
            acc_descr_block: false,
            relations: 0,
            notes: 0,
            claimed: std::collections::HashSet::new(),
        }
    }

    /// The namespace statements are currently landing in, if any.
    fn namespace(&self) -> Option<String> {
        self.stack.iter().rev().find_map(|f| match f {
            Frame::Namespace { id, .. } => Some(id.clone()),
            _ => None,
        })
    }

    /// The class body statements are currently landing in, if any.
    fn body(&self) -> Option<String> {
        match self.stack.last() {
            Some(Frame::Body { id, .. }) => Some(id.clone()),
            _ => None,
        }
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
        // Inside `class X { … }` the lexer's own start condition takes over: `[^{}\n]*` is one
        // MEMBER, so a line that would be a statement anywhere else is a member here.
        if let Some(id) = self.body() {
            match raw.find('}') {
                Some(end) => {
                    self.member(&id, &raw[..end]);
                    self.stack.pop();
                    let rest = raw[end + 1..].to_string();
                    return self.line(&rest, line);
                }
                None => {
                    self.member(&id, raw);
                    return Ok(());
                }
            }
        }

        let text = strip_comment(raw);
        if let Some(rest) = acc_descr_block_start(&text) {
            match rest.find('}') {
                Some(end) => self.push_acc_descr(&rest[..end]),
                None => {
                    self.push_acc_descr(rest);
                    self.acc_descr_block = true;
                }
            }
            return Ok(());
        }
        // Re-read the frame between segments rather than once: `class A { +x }` opens a body and
        // then hands it a member, all on one line, and the lexer's `class-body` start condition
        // takes effect the moment the `{` is seen.
        for seg in segments(&text, line)? {
            match seg {
                Segment::Statement(s) => match self.body() {
                    Some(id) => self.member(&id, &s),
                    None => self.statement(&s, line)?,
                },
                Segment::Open(head) => self.open(&head, line),
                Segment::Close => {
                    self.stack.pop();
                }
            }
        }
        Ok(())
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

    /// A `{` was reached; `head` is whatever preceded it on the line.
    fn open(&mut self, head: &str, line: usize) {
        let t = head.trim();
        if self.stack.len() > MAX_DEPTH {
            self.stack.push(Frame::Transparent { line });
            return;
        }
        if let Some(rest) = keyword(t, "namespace") {
            let (name, label) = read_class_label(rest.trim());
            let id = self.add_namespace(&name, label.as_deref());
            self.stack.push(Frame::Namespace { id, line });
            return;
        }
        if let Some(rest) = keyword(t, "class") {
            let id = self.declare_class(rest);
            match id {
                Some(id) => self.stack.push(Frame::Body { id, line }),
                None => self.stack.push(Frame::Transparent { line }),
            }
            return;
        }
        self.stack.push(Frame::Transparent { line });
    }

    fn finish(mut self) -> Result<ClassDiagram, ParseError> {
        if let Some(frame) = self.stack.first() {
            let (id, line) = match frame {
                Frame::Namespace { id, line } => (format!("namespace {id}"), *line),
                Frame::Body { id, line } => (format!("class {id}"), *line),
                Frame::Transparent { line } => ("{".to_string(), *line),
            };
            return Err(ParseError::UnclosedBody { id, line });
        }
        // A relationship whose endpoint never became a class cannot be drawn. Both ends are
        // interned when the relationship is read, so this only fires for a name a later pass
        // dropped — but it is the same guard stage 2 keeps, and for the same reason.
        // A note whose class never appeared is still drawn, floating: mermaid's `getData` skips
        // the tie-line when `this.classes.get(note.class)` misses, and keeps the note node.
        let ids: Vec<String> = self.out.classes.iter().map(|c| c.id.clone()).collect();
        for note in &mut self.out.notes {
            if note.target.as_ref().is_some_and(|t| !ids.contains(t)) {
                note.target = None;
            }
        }
        let known: Vec<String> = self.out.classes.iter().map(|c| c.id.clone()).collect();
        let namespaces: Vec<String> = self.out.namespaces.iter().map(|n| n.id.clone()).collect();
        let placed =
            |id: &String| known.iter().any(|k| k == id) || namespaces.iter().any(|n| n == id);
        self.out
            .relations
            .retain(|r| placed(&r.from) && placed(&r.to));
        if self.out.classes.is_empty() && self.out.notes.is_empty() {
            return Err(ParseError::NoClasses);
        }
        Ok(self.out)
    }

    /// One statement, already free of `{`, `}` and comments.
    fn statement(&mut self, s: &str, _line: usize) -> Result<(), ParseError> {
        let t = s.trim();
        if t.is_empty() {
            return Ok(());
        }
        if let Some(dir) = direction_statement(t) {
            // Only the outermost document's direction is the diagram's; see the module docs.
            if self.namespace().is_none() {
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
        if t == "accDescr" || t.starts_with("accDescr ") {
            return Ok(());
        }
        // Recognised so they cannot become classes, and then dropped: konoma does not colour
        // classes and has nowhere to send a click. §2-3 is the whole reason this arm exists.
        if is_ignored_statement(t) {
            return Ok(());
        }
        if let Some(rest) = keyword(t, "note for") {
            let (target, text) = split_note_target(rest);
            self.add_note(text, Some(target));
            return Ok(());
        }
        if let Some(rest) = keyword(t, "note") {
            self.add_note(quoted(rest.trim()).unwrap_or_default(), None);
            return Ok(());
        }
        if let Some(rest) = keyword(t, "namespace") {
            // A `namespace X` with no body. mermaid's grammar requires `{`, so this draws
            // nothing; registering it anyway would put an empty frame on the page.
            let _ = rest;
            return Ok(());
        }
        if let Some(rest) = keyword(t, "class") {
            self.declare_class(rest);
            return Ok(());
        }
        // `<<interface>> Shape` — the standalone annotation form.
        if let Some(rest) = t.strip_prefix("<<") {
            if let Some((annotation, tail)) = rest.split_once(">>") {
                let name = tail.trim();
                if !name.is_empty() {
                    let (id, generic) = split_generic(&decode_name(name));
                    self.out.intern(&id, &generic);
                    self.place(&id);
                    if let Some(c) = self.out.class_mut(&id) {
                        c.annotations.push(annotation.trim().to_string());
                    }
                }
                return Ok(());
            }
        }
        if let Some(relation) = read_relation(t) {
            self.add_relation(relation);
            return Ok(());
        }
        // `Animal : +int age` — the one-line member form.
        if let Some(i) = label_colon(t) {
            let name = t[..i].trim();
            let member = t[i + 1..].trim();
            if !name.is_empty() {
                let (name, css) = split_style_separator(name);
                let (id, generic) = split_generic(&decode_name(&name));
                if !id.is_empty() {
                    self.out.intern(&id, &generic);
                    self.place(&id);
                    self.css(&id, &css);
                    self.member(&id, member);
                }
            }
            return Ok(());
        }
        // A bare word. mermaid's `memberStatement : className` does nothing, and neither does
        // this: §2-3's rule is that a statement must be *recognised*, not that it must draw.
        Ok(())
    }

    /// `class Name`, `class Name["Label"]`, `class Name~T~`, `class Name:::css`,
    /// `class Name <<annotation>>` — and every combination of those.
    fn declare_class(&mut self, rest: &str) -> Option<String> {
        // The label comes off first: `class A["a << b"]` has a `<<` in its *text*, and an
        // annotation scan that ran first would eat half of it.
        let (mut name, label) = read_class_label(rest.trim());
        // `<<interface>>` may sit after the name, with or without a space.
        let mut annotation = None;
        if let Some(start) = name.find("<<") {
            if let Some(end) = name[start..].find(">>") {
                annotation = Some(name[start + 2..start + end].trim().to_string());
                let mut without = name[..start].to_string();
                without.push_str(&name[start + end + 2..]);
                name = without.trim().to_string();
            }
        }
        let (name, css) = split_style_separator(&name);
        let (id, generic) = split_generic(&decode_name(&name));
        if id.is_empty() {
            return None;
        }
        self.out.intern(&id, &generic);
        self.place(&id);
        self.css(&id, &css);
        if let Some(label) = label {
            if let Some(c) = self.out.class_mut(&id) {
                c.label = decode_label(&label);
            }
        }
        if let Some(a) = annotation {
            if !a.is_empty() {
                if let Some(c) = self.out.class_mut(&id) {
                    c.annotations.push(a);
                }
            }
        }
        Some(id)
    }

    /// One line of a class body, or the right-hand side of `Animal : …`.
    ///
    /// mermaid's `addMember`: `<<…>>` is an annotation, a `)` past the first character makes it a
    /// method, and anything else is an attribute.
    fn member(&mut self, class_id: &str, raw: &str) {
        let text = raw.trim();
        if text.is_empty() {
            return;
        }
        if let Some(inner) = text.strip_prefix("<<").and_then(|r| r.strip_suffix(">>")) {
            if let Some(c) = self.out.class_mut(class_id) {
                c.annotations.push(inner.trim().to_string());
            }
            return;
        }
        let is_method = text.find(')').is_some_and(|i| i > 0);
        let member = parse_member(text, is_method);
        if let Some(c) = self.out.class_mut(class_id) {
            if is_method {
                c.methods.push(member);
            } else {
                c.attributes.push(member);
            }
        }
    }

    fn css(&mut self, id: &str, classes: &[String]) {
        if classes.is_empty() {
            return;
        }
        if let Some(c) = self.out.class_mut(id) {
            for name in classes {
                if !c.css_classes.contains(name) {
                    c.css_classes.push(name.clone());
                }
            }
        }
    }

    /// Records which frame an id belongs to, the first time it is seen. A later mention never
    /// moves it: [`clusters::Tree`] needs one parent per node.
    ///
    /// [`clusters::Tree`]: crate::preview::mermaid::render::clusters::Tree
    fn place(&mut self, id: &str) {
        if self.claimed.contains(id) {
            return;
        }
        self.claimed.insert(id.to_string());
        let Some(ns) = self.namespace() else { return };
        if let Some(c) = self.out.class_mut(id) {
            c.parent = Some(ns.clone());
        }
        if let Some(n) = self.out.namespaces.iter_mut().find(|n| n.id == ns) {
            n.members.push(id.to_string());
        }
    }

    /// mermaid's `addNamespace`: a dotted name creates every ancestor, and a namespace written
    /// inside another is qualified by it (`resolveQualifiedId`).
    fn add_namespace(&mut self, name: &str, label: Option<&str>) -> String {
        let name = decode_name(name);
        let qualified = match self.namespace() {
            Some(prefix) => format!("{prefix}.{name}"),
            None => name.clone(),
        };
        let parts: Vec<&str> = qualified.split('.').collect();
        let mut id = String::new();
        let mut parent: Option<String> = None;
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                id.push('.');
            }
            id.push_str(part);
            let leaf = i == parts.len() - 1;
            let text = match (leaf, label) {
                (true, Some(l)) => decode_label(l),
                _ => (*part).to_string(),
            };
            if !self.out.namespaces.iter().any(|n| n.id == id) {
                self.out.namespaces.push(Namespace {
                    id: id.clone(),
                    label: text,
                    members: Vec::new(),
                    parent: parent.clone(),
                });
                if let Some(p) = &parent {
                    let child = id.clone();
                    if let Some(n) = self.out.namespaces.iter_mut().find(|n| &n.id == p) {
                        if !n.members.contains(&child) {
                            n.members.push(child);
                        }
                    }
                }
            } else if leaf {
                if let Some(l) = label {
                    let text = decode_label(l);
                    if let Some(n) = self.out.namespaces.iter_mut().find(|n| n.id == id) {
                        n.label = text;
                    }
                }
            }
            parent = Some(id.clone());
        }
        qualified
    }

    /// mermaid's `addNote`, which stores the class *name* and lets `getData` decide later whether
    /// there is a class to tie the note to. Resolving it here instead would break the documented
    /// example, where the `note for MyClass` is written **above** `class MyClass` — see
    /// [`Scanner::finish`], which is where an unknown target is dropped.
    fn add_note(&mut self, text: String, target: Option<String>) {
        let text = decode_label(&text);
        if text.is_empty() {
            return;
        }
        let target = target.map(|t| split_generic(&decode_name(t.trim())).0);
        self.notes += 1;
        let id = format!("note{}", self.notes - 1);
        let parent = self.namespace();
        if let Some(ns) = &parent {
            let note_id = id.clone();
            if let Some(n) = self.out.namespaces.iter_mut().find(|n| &n.id == ns) {
                n.members.push(note_id);
            }
        }
        self.out.notes.push(Note {
            id,
            text,
            target,
            parent,
        });
    }

    fn add_relation(&mut self, mut r: Relation) {
        if self.relations >= MAX_RELATIONS {
            return;
        }
        let (from_id, from_generic) = split_generic(&r.from);
        let (to_id, to_generic) = split_generic(&r.to);
        if from_id.is_empty() || to_id.is_empty() {
            return;
        }
        self.out.intern(&from_id, &from_generic);
        self.place(&from_id);
        self.out.intern(&to_id, &to_generic);
        self.place(&to_id);
        r.id = format!("r{}", self.relations);
        r.from = from_id;
        r.to = to_id;
        self.relations += 1;
        self.out.relations.push(r);
    }
}

// ---------------------------------------------------------------------------------------------
// Statements that exist only so they cannot become classes (§2-3)
// ---------------------------------------------------------------------------------------------

/// `classDef`, `cssClass`, `style`, `click`, `callback`, `link` — recognised, then dropped.
///
/// `class` is **not** here: in a class diagram it is the declaration keyword, and `cssClass` is
/// the styling one. (In a flowchart it is the other way round, which is exactly the kind of
/// difference that has to be read out of the grammar rather than remembered.)
fn is_ignored_statement(t: &str) -> bool {
    for kw in ["classDef", "cssClass", "style", "click", "callback", "link"] {
        if keyword(t, kw).is_some() {
            return true;
        }
    }
    false
}

/// `kw` followed by whitespace, returning the rest of the statement.
fn keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(kw)?;
    if rest.is_empty() || !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// `accTitle: text` / `accDescr: text`.
fn keyword_value(s: &str, kw: &str) -> Option<String> {
    let rest = s.strip_prefix(kw)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?;
    Some(rest.trim().to_string())
}

/// `direction LR` and friends — but only when the statement *starts* with the word.
fn direction_statement(s: &str) -> Option<Direction> {
    let rest = keyword(s, "direction")?;
    Direction::parse(rest.trim())
}

// ---------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------

/// Splits `id:::className` into the two. The `:::` is the style separator, never a label.
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

/// `splitClassNameAndType`: `Square~Shape~` is the class `Square` with the generic `Shape`.
///
/// Upstream's condition is `indexOf('~') > 0`, so a name that *starts* with `~` keeps it — which
/// is what makes the package-visibility member `~field` survive being passed through here.
pub fn split_generic(id: &str) -> (String, String) {
    match id.find('~') {
        Some(i) if i > 0 => {
            let generic = id[i + 1..].trim_end_matches('~').trim().to_string();
            (id[..i].trim().to_string(), generic)
        }
        _ => (id.trim().to_string(), String::new()),
    }
}

/// Takes the backticks off a `` `Class Name!` `` and decodes `#nn;` entities.
///
/// The backtick form is the only way to write a class name with a space in it
/// (`classLiteralName : BQUOTE_STR`), so a parser that leaves the backticks on draws them.
pub fn decode_name(raw: &str) -> String {
    let t = raw.trim();
    let t = match (t.strip_prefix('`'), t.strip_suffix('`')) {
        (Some(_), Some(_)) if t.len() >= 2 => &t[1..t.len() - 1],
        _ => t,
    };
    decode_label(t)
}

/// Splits `Name["Label"]` into the two. `classLabel : SQS STR SQE`.
fn read_class_label(s: &str) -> (String, Option<String>) {
    let t = s.trim();
    let Some(open) = t.find('[') else {
        return (t.to_string(), None);
    };
    let Some(close) = t.rfind(']') else {
        return (t.to_string(), None);
    };
    if close < open {
        return (t.to_string(), None);
    }
    let inner = t[open + 1..close].trim();
    let label = quoted(inner).unwrap_or_else(|| inner.to_string());
    (t[..open].trim().to_string(), Some(label))
}

/// The contents of a `"…"`, or `None` when the text is not one quoted string.
fn quoted(s: &str) -> Option<String> {
    let t = s.trim();
    let inner = t.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.to_string())
}

/// `note for X "text"` — the class name is the first word, the text is the quoted rest.
fn split_note_target(rest: &str) -> (String, String) {
    let t = rest.trim();
    match t.find('"') {
        Some(i) => {
            let target = t[..i].trim().to_string();
            let text = quoted(&t[i..]).unwrap_or_else(|| t[i + 1..].to_string());
            (target, text)
        }
        None => {
            let mut parts = t.splitn(2, char::is_whitespace);
            (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("").to_string(),
            )
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Relationships
// ---------------------------------------------------------------------------------------------

/// Reads `A "1" <|-- "*" B : label`, or returns `None` when the statement has no line token.
///
/// The three-token shape is the grammar's (`relation : relationType? lineType relationType?`), so
/// the search is for the **line** first and the marks are read off either side of it.
pub fn read_relation(t: &str) -> Option<Relation> {
    let limit = label_colon(t).unwrap_or(t.len());
    let head = &t[..limit];
    let (line_start, line_end, line) = find_line_token(head)?;

    let (start, left_end) = read_start_marker(head, line_start);
    let (end, right_start) = read_end_marker(head, line_end);

    let (from_raw, start_cardinality) = split_trailing_quoted(head[..left_end].trim());
    let (end_cardinality, to_raw) = split_leading_quoted(head[right_start..].trim());
    let (from, _) = split_style_separator(&from_raw);
    let (to, _) = split_style_separator(&to_raw);
    let from = decode_name(&from);
    let to = decode_name(&to);
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let label = if limit < t.len() {
        let l = decode_label(t[limit + 1..].trim());
        (!l.is_empty()).then_some(l)
    } else {
        None
    };
    Some(Relation {
        id: String::new(),
        from,
        to,
        start,
        end,
        line,
        label,
        start_cardinality,
        end_cardinality,
    })
}

/// The `--` or `..` of a relationship: its byte range, and which kind it is.
///
/// A run of three or more is taken whole — see the module docs for why that leaves mermaid.
fn find_line_token(s: &str) -> Option<(usize, usize, Line)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    let mut ticked = false;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'"' if !ticked => quoted = !quoted,
            b'`' if !quoted => ticked = !ticked,
            b'-' | b'.' if !quoted && !ticked && bytes[i + 1] == bytes[i] => {
                let c = bytes[i];
                let mut j = i;
                while j < bytes.len() && bytes[j] == c {
                    j += 1;
                }
                let kind = if c == b'-' { Line::Solid } else { Line::Dotted };
                return Some((i, j, kind));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The mark written immediately before the line, and where the class name ends.
fn read_start_marker(s: &str, line_start: usize) -> (Marker, usize) {
    let head = &s[..line_start];
    let trimmed = head.trim_end();
    let base = trimmed.len();
    for (token, marker) in MARKERS {
        if let Some(rest) = trimmed.strip_suffix(*token) {
            // `o` is also a letter. mermaid's own `\s*o` rule does not check, which is why
            // `Zoo -- Bar` needs the check here: the `o` of `Zoo` is not an aggregation.
            if *token == "o" && rest.chars().next_back().is_some_and(is_plain_id_char) {
                continue;
            }
            if rest.trim().is_empty() {
                continue;
            }
            return (*marker, base - token.len());
        }
    }
    (Marker::None, base)
}

/// The mark written immediately after the line, and where the class name begins.
fn read_end_marker(s: &str, line_end: usize) -> (Marker, usize) {
    let tail = &s[line_end..];
    for (token, marker) in MARKERS {
        if let Some(rest) = tail.strip_prefix(*token) {
            if *token == "o" && rest.chars().next().is_some_and(is_plain_id_char) {
                continue;
            }
            if rest.trim().is_empty() {
                continue;
            }
            return (*marker, line_end + token.len());
        }
    }
    (Marker::None, line_end)
}

/// Every `relationType` spelling, two-character forms first so `<|` beats `<`.
const MARKERS: &[(&str, Marker)] = &[
    ("<|", Marker::Extension),
    ("|>", Marker::Extension),
    ("()", Marker::Lollipop),
    ("<", Marker::Dependency),
    (">", Marker::Dependency),
    ("*", Marker::Composition),
    ("o", Marker::Aggregation),
];

/// Splits `Customer "1"` into the name and the cardinality.
fn split_trailing_quoted(s: &str) -> (String, Option<String>) {
    let t = s.trim_end();
    let Some(inner) = t.strip_suffix('"') else {
        return (t.to_string(), None);
    };
    let Some(open) = inner.rfind('"') else {
        return (t.to_string(), None);
    };
    let card = inner[open + 1..].to_string();
    (
        t[..open].trim().to_string(),
        (!card.trim().is_empty()).then_some(card),
    )
}

/// Splits `"*" Ticket` into the cardinality and the name.
fn split_leading_quoted(s: &str) -> (Option<String>, String) {
    let t = s.trim_start();
    let Some(rest) = t.strip_prefix('"') else {
        return (None, t.to_string());
    };
    let Some(close) = rest.find('"') else {
        return (None, t.to_string());
    };
    let card = rest[..close].to_string();
    (
        (!card.trim().is_empty()).then_some(card),
        rest[close + 1..].trim().to_string(),
    )
}

// ---------------------------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------------------------

/// `ClassMember.parseMember`, without the regex.
///
/// The method form is upstream's `/([#+~-])?(.+)\((.*)\)([\s$*])?(.*)([$*])?/` written out: the
/// name runs to the **last** `(` that still has a `)` after it, and the parameters run to the
/// **last** `)`. That is what a greedy `.+` followed by a greedy `(.*)` does, and it matters for
/// a parameter list that itself contains brackets.
pub fn parse_member(input: &str, is_method: bool) -> Member {
    let text = input.trim();
    let mut visibility = None;
    let mut rest = text;
    if let Some(first) = text.chars().next() {
        if matches!(first, '+' | '-' | '#' | '~') {
            visibility = Some(first);
            rest = &text[first.len_utf8()..];
        }
    }

    if !is_method {
        let mut classifier = None;
        let mut id = rest;
        if let Some(last) = rest.chars().next_back() {
            if matches!(last, '$' | '*') {
                classifier = Some(last);
                id = &rest[..rest.len() - last.len_utf8()];
            }
        }
        return Member {
            visibility,
            name: parse_generic_types(id.trim()),
            kind: MemberKind::Attribute,
            parameters: String::new(),
            return_type: String::new(),
            classifier,
        };
    }

    let (name, parameters, tail) = match rest.rfind(')') {
        Some(close) => match rest[..close].rfind('(') {
            Some(open) => (
                rest[..open].to_string(),
                rest[open + 1..close].to_string(),
                rest[close + 1..].to_string(),
            ),
            None => (rest.to_string(), String::new(), String::new()),
        },
        None => (rest.to_string(), String::new(), String::new()),
    };

    // `([\s$*])?` — one character, then `(.*)` takes the rest. A whitespace there trims to
    // nothing, which is what makes the fallback below reachable.
    let mut chars = tail.chars();
    let mut classifier = match chars.next() {
        Some(c) if c.is_whitespace() => None,
        Some(c @ ('$' | '*')) => Some(c),
        _ => None,
    };
    let return_type = match tail.chars().next() {
        Some(c) if c.is_whitespace() || matches!(c, '$' | '*') => tail[c.len_utf8()..].trim(),
        _ => tail.trim(),
    };
    let mut return_type = return_type.to_string();
    if classifier.is_none() {
        if let Some(last) = return_type.chars().next_back() {
            if matches!(last, '$' | '*') {
                classifier = Some(last);
                return_type.truncate(return_type.len() - last.len_utf8());
            }
        }
    }

    Member {
        visibility,
        name: parse_generic_types(name.trim()),
        kind: MemberKind::Method,
        parameters: parse_generic_types(parameters.trim()),
        return_type: parse_generic_types(return_type.trim()),
        classifier,
    }
}

/// mermaid's `parseGenericTypes`: matched pairs of `~` become `<` and `>`, outermost first.
///
/// Written out rather than approximated with a replace, because the nesting is the point:
/// `List~List~int~~` has to come out as `List<List<int>>`, and pairing the tildes from the
/// outside in is the only rule that gets that right. The comma handling is upstream's too — it
/// splits on commas so that `Map~K, V~` is processed as one set rather than as two halves with
/// one tilde each.
pub fn parse_generic_types(input: &str) -> String {
    let parts: Vec<&str> = split_keeping_commas(input);
    let mut out = String::new();
    let mut i = 0;
    while i < parts.len() {
        let mut set = parts[i].to_string();
        if parts[i] == "," && i > 0 && i + 1 < parts.len() {
            let prev = parts[i - 1];
            let next = parts[i + 1];
            if count(prev, '~') == 1 && count(next, '~') == 1 {
                set = format!("{prev},{next}");
                i += 1;
                // Undo the previous set, which was pushed unprocessed.
                let previous = process_set(prev);
                out.truncate(out.len() - previous.len());
            }
        }
        out.push_str(&process_set(&set));
        i += 1;
    }
    out
}

/// `input.split(/(,)/)` — the separators are kept as their own entries.
fn split_keeping_commas(input: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in input.char_indices() {
        if c == ',' {
            out.push(&input[start..i]);
            out.push(&input[i..i + 1]);
            start = i + 1;
        }
    }
    out.push(&input[start..]);
    out
}

fn count(s: &str, c: char) -> usize {
    s.chars().filter(|x| *x == c).count()
}

/// One set of `parseGenericTypes`: pair the tildes from the outside in.
fn process_set(input: &str) -> String {
    if count(input, '~') <= 1 {
        return input.to_string();
    }
    let mut leading = false;
    let mut work = input;
    if !count(input, '~').is_multiple_of(2) && input.starts_with('~') {
        work = &input[1..];
        leading = true;
    }
    let mut chars: Vec<char> = work.chars().collect();
    loop {
        let first = chars.iter().position(|c| *c == '~');
        let last = chars.iter().rposition(|c| *c == '~');
        match (first, last) {
            (Some(f), Some(l)) if f != l => {
                chars[f] = '<';
                chars[l] = '>';
            }
            _ => break,
        }
    }
    let body: String = chars.into_iter().collect();
    if leading {
        format!("~{body}")
    } else {
        body
    }
}

// ---------------------------------------------------------------------------------------------
// Line segmentation
// ---------------------------------------------------------------------------------------------

enum Segment {
    Statement(String),
    /// A `{` was reached; the payload is the text before it.
    Open(String),
    /// A `}` was reached.
    Close,
}

/// Cuts one line into statements at the `{` and `}` that open and close a body.
///
/// Everything after the `:` that starts a `LABEL` is text: the token is `":"{1}[^:\n;]+`, which
/// runs to the end of the line and happily contains braces. So `Foo : uses {braces}` is one
/// member, not a body.
fn segments(line: &str, line_no: usize) -> Result<Vec<Segment>, ParseError> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut start = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let Some(end) = line[i + 1..].find('"') else {
                    return Err(ParseError::UnclosedString { line: line_no });
                };
                i += 1 + end + 1;
                continue;
            }
            b'`' => {
                let Some(end) = line[i + 1..].find('`') else {
                    return Err(ParseError::UnclosedString { line: line_no });
                };
                i += 1 + end + 1;
                continue;
            }
            b':' => {
                if line[i..].starts_with(":::") {
                    i += 3;
                    continue;
                }
                break;
            }
            b'{' => {
                out.push(Segment::Open(line[start..i].to_string()));
                i += 1;
                start = i;
                continue;
            }
            b'}' => {
                out.push(Segment::Statement(line[start..i].to_string()));
                out.push(Segment::Close);
                i += 1;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(Segment::Statement(line[start..].to_string()));
    Ok(out)
}

/// Byte index of the `:` that starts a `LABEL`, ignoring quoted text, backticks and `:::`.
pub fn label_colon(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    let mut ticked = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' if !ticked => quoted = !quoted,
            b'`' if !quoted => ticked = !ticked,
            b':' if !quoted && !ticked => {
                if s[i..].starts_with(":::") {
                    i += 3;
                    continue;
                }
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The text after the `{` of an `accDescr {`, when that is what the line is.
fn acc_descr_block_start(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix("accDescr")?;
    let rest = rest.trim_start();
    rest.strip_prefix('{')
}

/// Removes an end-of-line `%%` comment.
///
/// `preprocess` has already taken out whole comment lines; this is the rest of the lexer's rule,
/// which skips `%%` wherever a token could start. It stops at the `:` that begins a `LABEL`,
/// because that token runs to the end of the line and a `%%` inside it is text — which is how
/// `Task : 50%% done` keeps its text.
fn strip_comment(line: &str) -> String {
    let limit = label_colon(line).unwrap_or(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    while i + 1 < limit {
        match bytes[i] {
            b'"' => quoted = !quoted,
            b'%' if !quoted && bytes[i + 1] == b'%' && !line[i..].starts_with("%%{") => {
                return line[..i].to_string();
            }
            _ => {}
        }
        i += 1;
    }
    line.to_string()
}
