//! mermaid `stateDiagram` / `stateDiagram-v2` source in, [`StateDiagram`] out.
//!
//! # Reference, and where konoma leaves it
//!
//! The grammar is read out of `stateDiagram.jison` and `stateDb.ts` at `mermaid@11.17.2`, not
//! out of a rendered picture — `docs/FEATURE-MERMAID-RENDERER.md` §0-2 tracks the *syntax* and
//! nothing else. Rules that cannot be guessed are cited where they are implemented. The three
//! that matter most, because getting any of them wrong changes which boxes exist:
//!
//! * **`[*]` is not one node.** `docTranslator` renames it to `<owner>_start` when an arrow
//!   *leaves* it and `<owner>_end` when an arrow *enters* it, where the owner is the document it
//!   was written in. So every `[*] -->` in one block shares a single start dot, every `--> [*]`
//!   shares a single end dot, and a block has its own pair. Treating each `[*]` as its own node
//!   would draw four dots where mermaid draws two.
//! * **`--` restructures the block it is in.** A block containing dividers becomes a block of
//!   anonymous sub-blocks, one per concurrent region — and the `[*]` inside each region then
//!   belongs to *that region*, not to the block.
//! * **a `note` is a node.** `dataFetcher` gives it an id, a shape and a link back to its state.
//!
//! Doing all three here means the renderer receives states, links and a nesting relation — the
//! same three things a flowchart gives it — which is what lets stage 2 reuse stage 1's layout.
//!
//! # Deliberate divergences
//!
//! Each is a place where mermaid's own behaviour is a quiet wrong answer rather than a feature,
//! and each follows a decision the flowchart parser already took:
//!
//! * **`direction` is a statement only when the statement starts with it.** mermaid's lexer
//!   matches `.*direction\s+TB[^\n]*`, so `s1 : go direction TB` becomes a direction statement
//!   and the state's text disappears.
//! * **a `direction` inside a block does not change the diagram's axis.** In mermaid it does —
//!   the lexer action calls `setDirection`, which unshifts the statement into the *root*
//!   document — so a nested statement silently turns the whole picture sideways. konoma reads it
//!   and ignores it, the same choice stage 1d took for a flowchart's `subgraph`.
//! * **`class` and `style` do not bring a state into existence.** mermaid's `handleStyleDef`
//!   calls `addState` for an unknown id, so a typo in a style line silently adds a box.
//! * **a `-` may appear inside an id** unless a `-` or `>` follows it, so `my-state --> other`
//!   reads as two states. mermaid's `ID` token excludes `-` outright and rejects the diagram.
//!   Being more permissive here can only turn a diagram mermaid refuses into one konoma draws.
//! * **a state's members belong to the first block that claims them.** mermaid lets a later
//!   mention move a state into another block (`Object.assign` in `insertOrUpdateNode`), which
//!   would give konoma's cluster tree two parents for one node.

use std::collections::HashMap;

use super::model::{Direction, Kind, NotePosition, ParseError, StateDiagram, Transition};
use crate::preview::mermaid::flowchart::preprocess::preprocess;
use crate::preview::mermaid::flowchart::text::decode_label;

/// Ceiling on how many transitions one diagram may declare, mirroring mermaid's `maxEdges`.
/// A guard, not a style rule.
const MAX_TRANSITIONS: usize = 500;

/// [`MAX_TRANSITIONS`], for the test that checks the ceiling really holds.
#[cfg(test)]
pub const MAX_TRANSITIONS_FOR_TESTS: usize = MAX_TRANSITIONS;

/// How deep `state x { … }` may nest before the parser stops opening blocks.
///
/// mermaid's own cluster handling gives up past ten; this is a bound on the recursion the
/// translation pass does, so that a `.md` file full of `{` cannot blow the stack.
const MAX_DEPTH: usize = 32;

/// Parses a mermaid state diagram.
///
/// # Errors
///
/// Only where drawing would be worse than not drawing — see [`ParseError`].
pub fn parse(src: &str) -> Result<StateDiagram, ParseError> {
    let pre = preprocess(src);
    let lines: Vec<&str> = pre.text.split('\n').collect();
    let header = find_header(&lines)?;

    let mut scanner = Scanner::new();
    scanner.line(&header.trailing, header.line_index + 1)?;
    for (i, line) in lines.iter().enumerate().skip(header.line_index + 1) {
        scanner.line(line, i + 1)?;
    }
    let mut doc = scanner.finish()?;

    let mut counter = Counters::default();
    translate("root", &mut doc, &mut counter, 0);

    let mut out = StateDiagram::new(scanner.direction.unwrap_or(header.direction), pre.title);
    out.acc_title = scanner.acc_title.take();
    out.acc_descr = scanner.acc_descr.take();
    let mut collector = Collector::default();
    let roots = collector.collect(&mut out, None, &doc, 0);
    out.roots = roots;
    collector.finish(&mut out);
    for (ids, style) in &scanner.class_statements {
        for id in ids {
            if let Some(state) = out.state_mut(id) {
                if !state.classes.contains(style) {
                    state.classes.push(style.clone());
                }
            }
        }
    }
    if out.states.is_empty() {
        return Err(ParseError::NoStates);
    }
    Ok(out)
}

/// Whether this source is a state diagram — i.e. whether its header keyword is `stateDiagram`
/// or `stateDiagram-v2`.
///
/// This is the routing predicate `preview::markdown::mermaid_to_svg` asks. Like
/// [`flowchart::is_flowchart`](crate::preview::mermaid::flowchart::is_flowchart) it is
/// deliberately *not* "does [`parse`] succeed": a state diagram this parser refuses is still
/// konoma's to answer for, and handing it to a second renderer would let that renderer paint
/// over a bug in the new one.
pub fn is_state_diagram(src: &str) -> bool {
    let pre = preprocess(src);
    let lines: Vec<&str> = pre.text.split('\n').collect();
    find_header(&lines).is_ok()
}

// ---------------------------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------------------------

struct Header {
    direction: Direction,
    line_index: usize,
    trailing: String,
}

/// Longest first, so `stateDiagram` cannot claim the `stateDiagram-v2` prefix.
///
/// Both spellings are drawn the same way. mermaid keeps a v1 renderer for the shorter one, but
/// the difference is entirely in how the picture is drawn (v1 has no composite states at all),
/// and §0-1 settles that konoma draws in its own idiom either way.
const STATE_KEYWORDS: &[&str] = &["stateDiagram-v2", "stateDiagram"];

fn find_header(lines: &[&str]) -> Result<Header, ParseError> {
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let t = line.trim_start();
        for kw in STATE_KEYWORDS {
            let Some(rest) = t.strip_prefix(kw) else {
                continue;
            };
            // `stateDiagrams` is a state called that, not a header.
            if rest.chars().next().is_some_and(is_plain_id_char) {
                continue;
            }
            return Ok(Header {
                direction: Direction::default(),
                line_index: i,
                trailing: rest.to_string(),
            });
        }
        let header: String = t
            .chars()
            .take_while(|c| !c.is_whitespace())
            .take(40)
            .collect();
        return Err(ParseError::NotAStateDiagram { header });
    }
    Err(ParseError::Empty)
}

/// True for a character that can only ever be part of an identifier.
fn is_plain_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------------------------------
// The raw tree — statements as written, before `[*]`, `--` and notes are resolved
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    id: String,
    kind: Kind,
    classes: Vec<String>,
}

impl Endpoint {
    fn new(id: &str) -> Endpoint {
        let (id, classes) = split_style_separator(id.trim());
        Endpoint {
            id,
            kind: Kind::Simple,
            classes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawState {
    id: String,
    kind: Kind,
    descriptions: Vec<String>,
    classes: Vec<String>,
    note: Option<(NotePosition, String)>,
    doc: Option<Vec<Raw>>,
}

impl RawState {
    fn new(id: &str, kind: Kind) -> RawState {
        let (id, classes) = split_style_separator(id.trim());
        RawState {
            id,
            kind,
            descriptions: Vec::new(),
            classes,
            note: None,
            doc: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Raw {
    State(RawState),
    Relation {
        from: Endpoint,
        to: Endpoint,
        descr: Option<String>,
    },
    /// A `--` between two concurrent regions of a block.
    Divider,
}

// ---------------------------------------------------------------------------------------------
// Scanning: source text -> raw tree
// ---------------------------------------------------------------------------------------------

/// One open `{ … }`.
struct Frame {
    /// The state the block belongs to. `None` for a `{` that followed nothing konoma could read
    /// as a state declaration; such a frame is *transparent* — its contents land in the
    /// enclosing document rather than in a frame of their own.
    owner: Option<RawState>,
    body: Vec<Raw>,
    line: usize,
}

/// A `note … of x` whose text has not arrived yet.
struct PendingNote {
    target: String,
    position: NotePosition,
    lines: Vec<String>,
    line: usize,
}

struct Scanner {
    stack: Vec<Frame>,
    note: Option<PendingNote>,
    acc_descr_block: bool,
    direction: Option<Direction>,
    acc_title: Option<String>,
    acc_descr: Option<String>,
    /// `class a,b styleName` statements, kept until every state is known.
    ///
    /// A `class` line may name a state that has only ever appeared as the end of a transition,
    /// or one that is declared further down the file, so it cannot be applied while scanning.
    /// It is applied at the end and **only to states that exist** — konoma does not let a style
    /// line bring a state into existence (see the module docs).
    class_statements: Vec<(Vec<String>, String)>,
}

impl Scanner {
    fn new() -> Scanner {
        Scanner {
            stack: vec![Frame {
                owner: None,
                body: Vec::new(),
                line: 0,
            }],
            note: None,
            acc_descr_block: false,
            direction: None,
            acc_title: None,
            acc_descr: None,
            class_statements: Vec::new(),
        }
    }

    fn body(&mut self) -> &mut Vec<Raw> {
        let last = self.stack.len() - 1;
        &mut self.stack[last].body
    }

    /// Reads one source line.
    fn line(&mut self, raw: &str, line: usize) -> Result<(), ParseError> {
        // `accDescr { … }` swallows everything to its `}`, so it is checked before the `{` that
        // opens a block would be.
        if self.acc_descr_block {
            if let Some(end) = raw.find('}') {
                self.push_acc_descr(&raw[..end]);
                self.acc_descr_block = false;
            } else {
                self.push_acc_descr(raw);
            }
            return Ok(());
        }
        // `<NOTE_TEXT>[\s\S]*?\n\s*"end note"` — the text runs to the line that closes it.
        if let Some(note) = &mut self.note {
            if raw.trim() == "end note" {
                let note = self.note.take().expect("note is open");
                self.attach_note(note);
            } else {
                note.lines.push(raw.trim().to_string());
            }
            return Ok(());
        }

        let text = strip_comment(raw);
        // `accDescr { … }` has to be recognised before the segmenter reads its `{` as the start
        // of a block. mermaid's lexer has the same ordering, as a dedicated start condition.
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
        for seg in segments(&text, line)? {
            match seg {
                Segment::Statement(s) => self.statement(&s, line)?,
                Segment::Open(head) => self.open(&head, line),
                Segment::Close => self.close(),
            }
        }
        Ok(())
    }

    fn push_acc_descr(&mut self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        match &mut self.acc_descr {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(t);
            }
            None => self.acc_descr = Some(t.to_string()),
        }
    }

    /// A `{` was reached; `head` is whatever preceded it on the line.
    fn open(&mut self, head: &str, line: usize) {
        // Anything other than a `state x` in front of the `{` is not something mermaid accepts
        // either. Rather than guess what the author meant, such a frame is *transparent*: its
        // contents stay in the enclosing document and the matching `}` closes nothing. Past
        // [`MAX_DEPTH`] every frame is transparent, so the braces still balance.
        let owner = if self.stack.len() > MAX_DEPTH {
            None
        } else {
            read_state_keyword(head)
        };
        self.stack.push(Frame {
            owner,
            body: Vec::new(),
            line,
        });
    }

    fn close(&mut self) {
        // A `}` with nothing open is not an error worth refusing a diagram over; mermaid's own
        // lexer would read it as part of an id.
        if self.stack.len() < 2 {
            return;
        }
        let frame = self.stack.pop().expect("checked above");
        match frame.owner {
            Some(mut owner) => {
                owner.doc = Some(frame.body);
                self.body().push(Raw::State(owner));
            }
            None => {
                let body = frame.body;
                self.body().extend(body);
            }
        }
    }

    fn finish(&mut self) -> Result<Vec<Raw>, ParseError> {
        if let Some(note) = self.note.take() {
            return Err(ParseError::UnclosedNote { line: note.line });
        }
        if self.stack.len() > 1 {
            let frame = &self.stack[1];
            return Err(ParseError::UnclosedComposite {
                id: frame
                    .owner
                    .as_ref()
                    .map_or_else(|| "{".to_string(), |o| o.id.clone()),
                line: frame.line,
            });
        }
        Ok(std::mem::take(&mut self.stack[0].body))
    }

    /// One statement, already free of `{`, `}` and comments.
    fn statement(&mut self, s: &str, line: usize) -> Result<(), ParseError> {
        let t = s.trim();
        if t.is_empty() {
            return Ok(());
        }
        // A `--` on its own is a concurrency divider. `-->` is not: it is longer, so the exact
        // comparison is what tells them apart. It only means anything *inside* a block: mermaid's
        // lexer returns `CONCURRENT` from its `struct` start condition alone, so a `--` at the
        // top level is a syntax error there. konoma drops it rather than refusing the diagram —
        // but it must not restructure the root document, which has no frame to divide.
        if t == "--" {
            if self.stack.len() > 1 {
                self.body().push(Raw::Divider);
            }
            return Ok(());
        }
        if let Some(dir) = direction_statement(t) {
            // Only the outermost document's direction is the diagram's; see the module docs.
            if self.stack.len() == 1 {
                self.direction = Some(dir);
            }
            return Ok(());
        }
        if let Some(v) = keyword_value(t, "accTitle") {
            self.acc_title = Some(v);
            return Ok(());
        }
        if let Some(v) = keyword_value(t, "accDescr") {
            self.acc_descr = Some(v);
            return Ok(());
        }
        if t == "accDescr" || t.starts_with("accDescr ") {
            // An `accDescr` with neither a `:` nor a `{`. Recognised and dropped, so that the
            // keyword cannot become a state.
            return Ok(());
        }
        // Recognised so it cannot become a phantom state, and then dropped: konoma does not
        // colour states, and `scale`/`click`/`hide empty description` have no geometry.
        if is_ignored_statement(t) {
            self.apply_class_statement(t);
            return Ok(());
        }
        if let Some(rest) = keyword(t, "note") {
            return self.note_statement(rest, line);
        }
        if let Some(rest) = keyword(t, "state") {
            let state = read_state_body(rest);
            self.body().push(Raw::State(state));
            return Ok(());
        }
        if let Some((left, right)) = split_transition(t) {
            let (right, descr) = split_description(&right);
            let from = Endpoint::new(&left);
            let to = Endpoint::new(&right);
            // Half a transition is not a transition. mermaid refuses the whole diagram for
            // `A -->`; konoma drops the statement, which is the smaller refusal — and it must not
            // instead invent a state with no id. A chain (`A --> B --> C`) is dropped for the
            // same reason: it is not syntax mermaid has, and reading the tail as one long id
            // would put a box called `B --> C` in the picture.
            if from.id.is_empty() || to.id.is_empty() || to.id.contains("-->") {
                return Ok(());
            }
            self.body().push(Raw::Relation {
                from,
                to,
                descr: descr.map(|d| decode_label(&d)),
            });
            return Ok(());
        }
        // Plain `s1`, `s1 : text`, `s1:::cls`.
        let (head, descr) = split_description(t);
        let mut state = RawState::new(&head, Kind::Simple);
        if let Some(d) = descr {
            state.descriptions.push(decode_label(&d));
        }
        self.body().push(Raw::State(state));
        Ok(())
    }

    /// `class one,two styleName` — remembered now, applied once every state is known.
    fn apply_class_statement(&mut self, t: &str) {
        let Some(rest) = keyword(t, "class") else {
            return;
        };
        let mut parts = rest.trim().splitn(2, char::is_whitespace);
        let (Some(ids), Some(style)) = (parts.next(), parts.next()) else {
            return;
        };
        let style = style.trim().to_string();
        if style.is_empty() {
            return;
        }
        let ids: Vec<String> = ids
            .split(',')
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty())
            .collect();
        if !ids.is_empty() {
            self.class_statements.push((ids, style));
        }
    }

    /// `note right of x : text`, `note left of x` + `end note`, and the floating `note "t" as n`.
    fn note_statement(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        let r = rest.trim_start();
        // A floating note is parsed and dropped: mermaid's own grammar rule for
        // `note NOTE_TEXT AS ID` has an empty action, so it draws nothing there either.
        if r.starts_with('"') {
            return Ok(());
        }
        let (position, after) = if let Some(a) = r.strip_prefix("right of") {
            (NotePosition::Right, a)
        } else if let Some(a) = r.strip_prefix("left of") {
            (NotePosition::Left, a)
        } else {
            // Not a note form konoma knows. Dropping it is safer than reading `note` as a state.
            return Ok(());
        };
        let (target, text) = split_description(after.trim());
        let target = target.trim().to_string();
        if target.is_empty() {
            return Ok(());
        }
        match text {
            Some(t) => {
                self.attach_note(PendingNote {
                    target,
                    position,
                    lines: vec![t],
                    line,
                });
                Ok(())
            }
            None => {
                self.note = Some(PendingNote {
                    target,
                    position,
                    lines: Vec::new(),
                    line,
                });
                Ok(())
            }
        }
    }

    fn attach_note(&mut self, note: PendingNote) {
        let text = note
            .lines
            .iter()
            .map(|l| decode_label(l))
            .collect::<Vec<_>>()
            .join("\n");
        let text = text.trim_matches('\n').to_string();
        if text.is_empty() {
            return;
        }
        // mermaid attaches the note to the state through `addState`, which reaches the state
        // wherever it was declared. Here it rides on a fresh mention in the current document,
        // which is where the reader wrote it.
        let mut state = RawState::new(&note.target, Kind::Simple);
        state.note = Some((note.position, text));
        self.body().push(Raw::State(state));
    }
}

/// The text after the `{` of an `accDescr {`, when that is what the line is.
fn acc_descr_block_start(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix("accDescr")?;
    let rest = rest.trim_start();
    rest.strip_prefix('{')
}

/// Statements konoma recognises only so that they cannot become states.
fn is_ignored_statement(t: &str) -> bool {
    for kw in ["classDef", "class", "style", "click", "scale"] {
        if keyword(t, kw).is_some() {
            return true;
        }
    }
    t == "hide empty description" || t == "end note"
}

/// `kw` followed by whitespace, returning the rest of the statement.
fn keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(kw)?;
    if rest.is_empty() {
        return None;
    }
    if !rest.starts_with(char::is_whitespace) {
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

/// Splits a statement at the `:` that starts a description, if there is one.
///
/// The colon of `:::` is the style separator, not a description, which is why this cannot be a
/// `split_once(':')`.
fn split_description(s: &str) -> (String, Option<String>) {
    match descr_colon(s) {
        Some(i) => {
            let head = s[..i].trim().to_string();
            let tail = s[i + 1..].trim();
            (head, Some(tail.to_string()))
        }
        None => (s.trim().to_string(), None),
    }
}

/// Byte index of the `:` that starts a description, ignoring quoted text and `:::`.
fn descr_colon(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => quoted = !quoted,
            b':' if !quoted => {
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

/// Splits `a --> b` at the arrow, if the statement has one outside a quoted string.
fn split_transition(s: &str) -> Option<(String, String)> {
    let bytes = s.as_bytes();
    let limit = descr_colon(s).unwrap_or(s.len());
    let mut i = 0;
    let mut quoted = false;
    while i + 3 <= limit {
        match bytes[i] {
            b'"' => quoted = !quoted,
            b'-' if !quoted && s[i..].starts_with("-->") => {
                return Some((s[..i].to_string(), s[i + 3..].to_string()));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `state …` after the keyword, in each of the forms the lexer's `STATE` state accepts.
fn read_state_body(rest: &str) -> RawState {
    let r = rest.trim();
    // `<STATE>.*"<<fork>>"` — the marker is stripped and what precedes it is the id. Both the
    // `<<fork>>` and the `[[fork]]` spellings are in the lexer, in any case.
    for (marker, kind) in [
        ("<<fork>>", Kind::Fork),
        ("<<join>>", Kind::Join),
        ("<<choice>>", Kind::Choice),
        ("[[fork]]", Kind::Fork),
        ("[[join]]", Kind::Join),
        ("[[choice]]", Kind::Choice),
    ] {
        if let Some(i) = rfind_ignore_ascii_case(r, marker) {
            let id = r[..i].trim();
            if !id.is_empty() {
                return RawState::new(id, kind);
            }
        }
    }
    // `state "long description" as id`, possibly with a `: more` after the id.
    if let Some(after_quote) = r.strip_prefix('"') {
        if let Some(end) = after_quote.find('"') {
            let descr = decode_label(&after_quote[..end]);
            let tail = after_quote[end + 1..].trim_start();
            let id_part = keyword(tail, "as").unwrap_or(tail).trim();
            // `if ($3.match(':')) { id = parts[0]; description = [description, parts[1]] }`
            let (id, extra) = split_description(id_part);
            let mut state = RawState::new(&id, Kind::Simple);
            state.descriptions.push(descr);
            if let Some(e) = extra {
                state.descriptions.push(decode_label(&e));
            }
            return state;
        }
    }
    // `<STATE>[^\n\s\{]+` — the id is the first word, and mermaid rejects anything after it.
    // konoma keeps the first word and drops the rest rather than refusing the diagram.
    let (head, descr) = split_description(r);
    let id = head.split_whitespace().next().unwrap_or("");
    let mut state = RawState::new(id, Kind::Simple);
    if let Some(d) = descr {
        state.descriptions.push(decode_label(&d));
    }
    state
}

/// The `state x` / `state "t" as x` in front of a `{`, or `None` when the head is something else.
fn read_state_keyword(head: &str) -> Option<RawState> {
    let rest = keyword(head.trim(), "state")?;
    let state = read_state_body(rest);
    if state.id.is_empty() {
        return None;
    }
    Some(state)
}

fn rfind_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.to_ascii_lowercase();
    // `to_ascii_lowercase` maps ASCII only, so byte offsets in `h` are offsets in `haystack`.
    h.rfind(&needle.to_ascii_lowercase())
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

/// Cuts one line into statements at the `{` and `}` that open and close a block.
///
/// Nothing after the `:` that starts a description is structural: mermaid's `DESCR` token runs
/// to the end of the line and happily contains braces, so `s1 : uses {braces}` is one state with
/// one description rather than a block.
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

/// Removes an end-of-line comment.
///
/// State diagrams have two comment markers where a flowchart has one: the lexer skips both
/// `\%\%(?!\{)[^\n]*` and `\#[^\n]*`, and — unlike a flowchart, where the pattern is anchored to
/// the start of a line — it does so wherever a token could begin. So a comment is recognised at
/// the start of the line or after whitespace, and only *before* the `:` that starts a
/// description: `s1 : 50% done #1` is one description, exactly as mermaid's longest-match lexer
/// reads it.
fn strip_comment(line: &str) -> String {
    let limit = descr_colon(line).unwrap_or(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut quoted = false;
    while i < limit {
        let boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
        match bytes[i] {
            b'"' => quoted = !quoted,
            b'#' if !quoted && boundary => return line[..i].to_string(),
            // `%%{` opens a directive, which `preprocess` has already removed; mermaid's own
            // comment rule excludes it with the same negative lookahead.
            b'%' if !quoted
                && boundary
                && line[i..].starts_with("%%")
                && !line[i..].starts_with("%%{") =>
            {
                return line[..i].to_string();
            }
            _ => {}
        }
        i += 1;
    }
    line.to_string()
}

// ---------------------------------------------------------------------------------------------
// Translation: `[*]`, `--` and nesting resolved (mermaid's `docTranslator`)
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Counters {
    divider: usize,
}

/// mermaid's `[*]`.
const EDGE_STATE: &str = "[*]";

/// Resolves one document, then each block inside it.
///
/// The order is mermaid's and it matters: the divider split happens *before* the `[*]` in the
/// document are named, so an `[*]` written inside a concurrent region belongs to that region.
fn translate(owner: &str, doc: &mut Vec<Raw>, counter: &mut Counters, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    split_dividers(doc, counter);
    for item in doc.iter_mut() {
        match item {
            Raw::Relation { from, to, .. } => {
                rename_edge_state(from, owner, true);
                rename_edge_state(to, owner, false);
            }
            Raw::State(s) => {
                if s.id == EDGE_STATE {
                    s.id = format!("{owner}_start");
                    s.kind = Kind::Start;
                }
            }
            Raw::Divider => {}
        }
    }
    for item in doc.iter_mut() {
        if let Raw::State(s) = item {
            if let Some(inner) = &mut s.doc {
                let id = s.id.clone();
                translate(&id, inner, counter, depth + 1);
            }
        }
    }
}

fn rename_edge_state(ep: &mut Endpoint, owner: &str, first: bool) {
    if ep.id != EDGE_STATE {
        return;
    }
    ep.id = format!("{owner}_{}", if first { "start" } else { "end" });
    ep.kind = if first { Kind::Start } else { Kind::End };
}

/// Turns a document holding `--` dividers into a document of anonymous concurrent regions.
///
/// mermaid only restructures when there is something on *both* sides of the last divider
/// (`if (doc.length > 0 && currentDoc.length > 0)`); a block that merely ends with a `--` is
/// left as it was. konoma keeps that condition and, when it is not met, drops the dividers
/// rather than leaving a node behind that draws as an empty box.
fn split_dividers(doc: &mut Vec<Raw>, counter: &mut Counters) {
    if !doc.iter().any(|r| matches!(r, Raw::Divider)) {
        return;
    }
    let mut groups: Vec<Vec<Raw>> = Vec::new();
    let mut current: Vec<Raw> = Vec::new();
    for item in doc.drain(..) {
        if matches!(item, Raw::Divider) {
            groups.push(std::mem::take(&mut current));
        } else {
            current.push(item);
        }
    }
    if groups.is_empty() || current.is_empty() {
        // No restructuring: put everything back, dividers dropped.
        for g in groups {
            doc.extend(g);
        }
        doc.extend(current);
        return;
    }
    groups.push(current);
    for group in groups {
        counter.divider += 1;
        let mut region =
            RawState::new(&format!("divider-id-{}", counter.divider), Kind::Concurrent);
        region.doc = Some(group);
        doc.push(Raw::State(region));
    }
}

// ---------------------------------------------------------------------------------------------
// Collection: raw tree -> `StateDiagram`
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Collector {
    /// Descriptions per state, in the order they were written.
    descriptions: HashMap<String, Vec<String>>,
    /// Which document first claimed each state — a node can only have one parent.
    claimed: HashMap<String, Option<String>>,
    transitions: usize,
    notes: usize,
}

impl Collector {
    /// Walks one document, returning the ids of its direct members in order.
    fn collect(
        &mut self,
        out: &mut StateDiagram,
        parent: Option<&str>,
        doc: &[Raw],
        depth: usize,
    ) -> Vec<String> {
        let mut members: Vec<String> = Vec::new();
        if depth > MAX_DEPTH {
            return members;
        }
        for item in doc {
            match item {
                Raw::Divider => {}
                Raw::Relation { from, to, descr } => {
                    self.touch(out, from.id.as_str(), from.kind, parent, &mut members);
                    self.classes(out, &from.id, &from.classes);
                    self.touch(out, to.id.as_str(), to.kind, parent, &mut members);
                    self.classes(out, &to.id, &to.classes);
                    if self.transitions < MAX_TRANSITIONS {
                        let id = format!("t{}", self.transitions);
                        self.transitions += 1;
                        out.transitions.push(Transition {
                            id,
                            from: from.id.clone(),
                            to: to.id.clone(),
                            label: descr.clone().filter(|d| !d.trim().is_empty()),
                            is_note_link: false,
                        });
                    }
                }
                Raw::State(s) => {
                    if s.id.is_empty() {
                        continue;
                    }
                    let kind = if s.doc.is_some() && !s.kind.is_block() {
                        Kind::Composite
                    } else {
                        s.kind
                    };
                    self.touch(out, &s.id, kind, parent, &mut members);
                    self.classes(out, &s.id, &s.classes);
                    for d in &s.descriptions {
                        self.descriptions
                            .entry(s.id.clone())
                            .or_default()
                            .push(d.clone());
                    }
                    if let Some((position, text)) = &s.note {
                        self.add_note(out, &s.id, *position, text, parent, &mut members);
                    }
                    if let Some(inner) = &s.doc {
                        let inner_members =
                            self.collect(out, Some(s.id.as_str()), inner, depth + 1);
                        if let Some(state) = out.state_mut(&s.id) {
                            for m in inner_members {
                                if !state.members.contains(&m) {
                                    state.members.push(m);
                                }
                            }
                        }
                    }
                }
            }
        }
        members
    }

    /// Registers a mention of `id` in the document owned by `parent`.
    fn touch(
        &mut self,
        out: &mut StateDiagram,
        id: &str,
        kind: Kind,
        parent: Option<&str>,
        members: &mut Vec<String>,
    ) {
        out.intern(id, kind);
        match self.claimed.get(id) {
            Some(owner) => {
                // Already placed. Only the document that claimed it lists it as a member.
                if owner.as_deref() == parent && !members.iter().any(|m| m == id) {
                    members.push(id.to_string());
                }
            }
            None => {
                self.claimed
                    .insert(id.to_string(), parent.map(str::to_string));
                if let Some(state) = out.state_mut(id) {
                    state.parent = parent.map(str::to_string);
                }
                members.push(id.to_string());
            }
        }
    }

    fn classes(&mut self, out: &mut StateDiagram, id: &str, classes: &[String]) {
        if classes.is_empty() {
            return;
        }
        if let Some(state) = out.state_mut(id) {
            for c in classes {
                if !state.classes.contains(c) {
                    state.classes.push(c.clone());
                }
            }
        }
    }

    /// A note becomes a node of its own plus the line that ties it to its state.
    ///
    /// mermaid's `dataFetcher` does the same, including the direction of the tie: `left of`
    /// points at the state, `right of` points away from it, which is what puts the note on the
    /// requested side of it along the layout's axis.
    fn add_note(
        &mut self,
        out: &mut StateDiagram,
        target: &str,
        position: NotePosition,
        text: &str,
        parent: Option<&str>,
        members: &mut Vec<String>,
    ) {
        self.notes += 1;
        let id = format!("{target}----note-{}", self.notes);
        out.intern(&id, Kind::Note);
        if let Some(state) = out.state_mut(&id) {
            state.label = text.to_string();
            state.note_position = Some(position);
            state.parent = parent.map(str::to_string);
        }
        self.claimed.insert(id.clone(), parent.map(str::to_string));
        members.push(id.clone());
        let (from, to) = match position {
            NotePosition::Right => (target.to_string(), id.clone()),
            NotePosition::Left => (id.clone(), target.to_string()),
        };
        out.transitions.push(Transition {
            id: format!("note-{}", self.notes),
            from,
            to,
            label: None,
            is_note_link: true,
        });
    }

    /// Turns the descriptions gathered along the way into the label each state is drawn with.
    ///
    /// mermaid's rule, from `dataFetcher` plus the fix-up loop at the end of `extract`: a state
    /// with no description of its own is drawn with its id, one description replaces the id, and
    /// two or more become a box with a rule under the first of them
    /// (`SHAPE_STATE_WITH_DESC`). A kind that draws no text at all keeps an empty label.
    fn finish(&mut self, out: &mut StateDiagram) {
        for state in &mut out.states {
            if state.kind == Kind::Note {
                continue;
            }
            let descriptions = self.descriptions.remove(&state.id).unwrap_or_default();
            if state.kind.is_textless() {
                continue;
            }
            state.titled = descriptions.len() >= 2;
            state.label = if descriptions.is_empty() {
                state.id.clone()
            } else {
                descriptions.join("\n")
            };
        }
        // A transition whose endpoint never became a state cannot be drawn.
        let known: Vec<String> = out.states.iter().map(|s| s.id.clone()).collect();
        out.transitions
            .retain(|t| known.iter().any(|k| k == &t.from) && known.iter().any(|k| k == &t.to));
    }
}
