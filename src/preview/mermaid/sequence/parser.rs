//! mermaid `sequenceDiagram` source in, [`SequenceDiagram`] out.
//!
//! # Reference, and where konoma leaves it
//!
//! Read out of `sequenceDiagram.jison` and `sequenceDb.ts` at `mermaid@11.17.2`. Six rules decide
//! what a reader sees, and not one of them is guessable from the documentation:
//!
//! * **`#` starts a comment**, in almost every lexer state, and `;` is a statement separator. No
//!   other mermaid grammar konoma implements works this way, and it is why upstream's own
//!   documentation warns that a `#rrggbb` colour cannot be written in a `box`.
//! * **A message, a note and a `links`/`link` bring a participant into existence; `activate` and
//!   `destroy` do not.** The grammar makes the difference by returning the actor object from some
//!   productions and dropping it in others, and getting it wrong reorders — or invents — columns.
//! * **`+` activates the *receiver*; `-` deactivates the *sender*.** They are not a pair around
//!   one participant. `A->>+B` opens B's bar and `B-->>-A` closes B's.
//! * **`->` and `-->` draw no arrowhead at all.** They are "open" only in the sense of having no
//!   head; the head-like mark the documentation calls "an open arrow (async)" belongs to `-)`.
//! * **The lexer is `%options case-insensitive`**, like the ER one and unlike the other two.
//! * **`autonumber`'s numbers may carry two decimals** (`autonumber 1.5 0.25`), because its `NUM`
//!   token is `[0-9]+(\.[0-9]{1,2})?`.
//!
//! # Deliberate divergences, each pinned by a test
//!
//! 1. **A keyword is only a keyword at the start of a statement, followed by a word boundary.**
//!    jison-lex without `%options flex` takes the *first* rule that matches rather than the
//!    longest, so upstream a participant called `looper` lexes as `loop` + `er`, and mermaid's own
//!    documentation warns that "the word `end` could potentially break the diagram". konoma reads
//!    `looper->>B: hi` as a message from `looper`. This can only turn a diagram that upstream
//!    breaks on into one that draws, never the other way round.
//! 2. **`#nnn;` and `#name;` are entity codes, not comments.** Upstream's `#` rule sits ahead of
//!    its `TXT` rule, so the `A->>B: I #9829; you!` in its own documentation does not survive its
//!    lexer. konoma cuts a comment at `#` **unless** the `#` opens an entity code, which is what
//!    makes the documented example read as it is documented to read.
//! 3. **A message with no `: text` is a message with no text.** Upstream's `signal` production
//!    requires the `TXT` token, so `Alice->>John` alone is a syntax error and the whole diagram
//!    refuses to draw. Refusing a page of correct messages over one missing colon is the wrong
//!    trade under §0-1.
//! 4. **The sixteen half-arrow spellings fold onto a full arrowhead**, and the four `_REVERSE`
//!    families put that head at the sending end. A half-barb is a few pixels wide at the size a
//!    terminal shows a diagram; the direction it points is the part that carries meaning.
//! 5. **`()` central connections are parsed and drawn as ordinary messages**, and `par_over` is
//!    drawn as a `par`. Both are v11.12.3 spellings whose difference is a drawing style, not a
//!    different statement.
//! 6. **A participant's `@{ … }` block is read for `type` and `alias` only**, and every `type`
//!    other than `actor` is drawn as a plain participant. §2-4 settles the same thing for a
//!    flowchart's fifty-three shapes: recognise every name, draw a few.

use super::model::{
    AutoNumber, BlockKind, Event, Head, Line, Message, Note, ParseError, Participant,
    ParticipantBox, ParticipantKind, Placement, SectionKind, SequenceDiagram, Signal,
};
use crate::preview::mermaid::flowchart::preprocess::preprocess;
use crate::preview::mermaid::flowchart::text::decode_label;

/// Ceiling on how many messages one diagram may declare, mirroring mermaid's `maxEdges`.
const MAX_MESSAGES: usize = 500;

/// [`MAX_MESSAGES`], for the test that checks the ceiling really holds.
#[cfg(test)]
pub const MAX_MESSAGES_FOR_TESTS: usize = MAX_MESSAGES;

/// Parses a mermaid sequence diagram.
///
/// # Errors
///
/// Only where drawing would be worse than not drawing — see [`ParseError`].
pub fn parse(src: &str) -> Result<SequenceDiagram, ParseError> {
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

/// Whether this source is a sequence diagram.
///
/// The routing predicate `preview::markdown::mermaid_to_svg` asks this **before either renderer
/// runs** (§1, stage 1's rule): a diagram konoma owns and then refuses degrades to the text
/// diagram rather than being redrawn by the crate. Classifying therefore reads only the header,
/// so that a source of some other kind never reaches this grammar at all.
pub fn is_sequence_diagram(src: &str) -> bool {
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
        if starts_ci(t, "sequenceDiagram") {
            let rest = &t[15..];
            if !rest.chars().next().is_some_and(is_id_char) {
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
        return Err(ParseError::NotASequenceDiagram { header });
    }
    Err(ParseError::Empty)
}

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `s` starts with `kw`, ignoring ASCII case, on a character boundary.
///
/// Written out rather than slicing because `s[..kw.len()]` panics the moment `s` starts with a
/// multi-byte character, and this lexer is case-insensitive so the comparison happens everywhere.
fn starts_ci(s: &str, kw: &str) -> bool {
    s.len() >= kw.len() && s.is_char_boundary(kw.len()) && s[..kw.len()].eq_ignore_ascii_case(kw)
}

// ---------------------------------------------------------------------------------------------
// Splitting a line into statements
// ---------------------------------------------------------------------------------------------

/// An entity code — `#` then digits or letters then `;` — starting at `i`, and its end.
///
/// This is the whole of divergence 2: the `#` and the `;` of `#9829;` are neither a comment nor a
/// statement separator, so both scanners have to be able to step over one atomically.
fn entity_at(chars: &[char], i: usize) -> Option<usize> {
    if chars.get(i) != Some(&'#') {
        return None;
    }
    let mut p = i + 1;
    let digits = chars.get(p).is_some_and(|c| c.is_ascii_digit());
    while chars.get(p).is_some_and(|c| {
        if digits {
            c.is_ascii_digit()
        } else {
            c.is_ascii_alphabetic()
        }
    }) {
        p += 1;
    }
    if p == i + 1 || chars.get(p) != Some(&';') {
        return None;
    }
    Some(p + 1)
}

/// Splits one source line into statements: `;` separates, `#` starts a comment, and an entity code
/// is one atom that contains neither.
fn split_statements(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = entity_at(&chars, i) {
            current.extend(&chars[i..end]);
            i = end;
            continue;
        }
        match chars[i] {
            '#' => break,
            ';' => {
                out.push(std::mem::take(&mut current));
                i += 1;
            }
            c => {
                current.push(c);
                i += 1;
            }
        }
    }
    out.push(current);
    out
}

/// The first word of a statement, lowercased, and what follows it.
///
/// Divergence 1 lives here: a keyword is the *whole* first word. `looper->>B` has first word
/// `looper->>B`, which is no keyword, so it stays a message.
fn head_word(stmt: &str) -> (String, &str) {
    let t = stmt.trim_start();
    let end = t.find(char::is_whitespace).unwrap_or(t.len());
    (t[..end].to_ascii_lowercase(), t[end..].trim_start())
}

/// mermaid's `extractWrap`: `wrap:` / `nowrap:` in front of a `restOfLine` or a message text sets
/// automatic wrapping, which konoma does not implement at all (§2-5). The prefix is removed so it
/// does not end up drawn.
fn strip_wrap(s: &str) -> &str {
    let t = s.strip_prefix(':').unwrap_or(s);
    for prefix in ["nowrap:", "wrap:"] {
        if starts_ci(t, prefix) {
            return t[prefix.len()..].trim_start();
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Arrows
// ---------------------------------------------------------------------------------------------

/// Every arrow spelling the grammar has, longest first.
///
/// Longest-first is what makes the scan below correct: `-->>` has to be tried before `-->`, and
/// `<<-->>` before `<<->>`. The sixteen half-arrows are here because a spelling this table does
/// not know becomes part of a participant's name — a phantom column — rather than a message.
const ARROWS: &[(&str, Line, Head, Head)] = &[
    ("<<-->>", Line::Dotted, Head::Arrow, Head::Arrow),
    ("<<->>", Line::Solid, Head::Arrow, Head::Arrow),
    // Half-arrows, dotted. Folded onto a full head (divergence 4); `_REVERSE` moves it to the
    // sending end, which is the half that carries meaning.
    ("--|\\", Line::Dotted, Head::None, Head::Arrow),
    ("--|/", Line::Dotted, Head::None, Head::Arrow),
    ("--\\\\", Line::Dotted, Head::None, Head::Arrow),
    ("--//", Line::Dotted, Head::None, Head::Arrow),
    ("/|--", Line::Dotted, Head::Arrow, Head::None),
    ("\\|--", Line::Dotted, Head::Arrow, Head::None),
    ("//--", Line::Dotted, Head::Arrow, Head::None),
    ("\\\\--", Line::Dotted, Head::Arrow, Head::None),
    ("-->>", Line::Dotted, Head::None, Head::Arrow),
    ("-->", Line::Dotted, Head::None, Head::None),
    ("--x", Line::Dotted, Head::None, Head::Cross),
    ("--)", Line::Dotted, Head::None, Head::Async),
    // Half-arrows, solid.
    ("-|\\", Line::Solid, Head::None, Head::Arrow),
    ("-|/", Line::Solid, Head::None, Head::Arrow),
    ("-\\\\", Line::Solid, Head::None, Head::Arrow),
    ("-//", Line::Solid, Head::None, Head::Arrow),
    ("/|-", Line::Solid, Head::Arrow, Head::None),
    ("\\|-", Line::Solid, Head::Arrow, Head::None),
    ("//-", Line::Solid, Head::Arrow, Head::None),
    ("\\\\-", Line::Solid, Head::Arrow, Head::None),
    ("->>", Line::Solid, Head::None, Head::Arrow),
    ("->", Line::Solid, Head::None, Head::None),
    ("-x", Line::Solid, Head::None, Head::Cross),
    ("-)", Line::Solid, Head::None, Head::Async),
];

/// Finds the arrow in a statement: the leftmost position where any spelling matches, taking the
/// longest spelling there.
///
/// Scanning left to right is what lets a participant keep a `-` in its name — `A-B->>C` finds the
/// arrow at the `->>` and not at the `-B`, because no spelling matches at the earlier `-`.
fn find_arrow(stmt: &str) -> Option<(usize, usize, Signal)> {
    let bytes = stmt.as_bytes();
    for i in 0..bytes.len() {
        if !stmt.is_char_boundary(i) {
            continue;
        }
        for (spelling, line, start, end) in ARROWS {
            if stmt[i..].starts_with(spelling) {
                // A `:` before the arrow means the arrow is inside a message's text, not the
                // message's own arrow — `A->>B: see A-->C` must not be split at the second one.
                if stmt[..i].contains(':') {
                    return None;
                }
                return Some((
                    i,
                    i + spelling.len(),
                    Signal {
                        line: *line,
                        start: *start,
                        end: *end,
                    },
                ));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------------------------

/// One thing that is open and waiting for its `end`.
struct Open {
    /// `None` for a `box`, which is a different kind of grouping and cannot nest.
    block: Option<BlockKind>,
    keyword: &'static str,
    line: usize,
}

struct Scanner {
    diagram: SequenceDiagram,
    open: Vec<Open>,
    /// How many times each participant has been activated and not yet deactivated. mermaid's
    /// `activationCount`, kept as a running total rather than recomputed per statement.
    active: Vec<(String, i32)>,
    /// Set by `create`; the next message has to be the one that creates it (mermaid's rule).
    last_created: Option<String>,
    last_destroyed: Option<String>,
    /// Inside an `accDescr { … }` block.
    in_acc_descr: bool,
    acc_descr_buf: String,
    messages: usize,
}

impl Scanner {
    fn new(front_matter_title: Option<String>) -> Scanner {
        Scanner {
            diagram: SequenceDiagram {
                title: front_matter_title,
                ..SequenceDiagram::default()
            },
            open: Vec::new(),
            active: Vec::new(),
            last_created: None,
            last_destroyed: None,
            in_acc_descr: false,
            acc_descr_buf: String::new(),
            messages: 0,
        }
    }

    fn line(&mut self, raw: &str, number: usize) -> Result<(), ParseError> {
        if self.in_acc_descr {
            // `<acc_descr_multiline>[\}]` closes it; everything else is content.
            match raw.find('}') {
                Some(i) => {
                    self.acc_descr_buf.push_str(&raw[..i]);
                    self.in_acc_descr = false;
                    let text = std::mem::take(&mut self.acc_descr_buf);
                    self.diagram.acc_descr = Some(text.trim().to_string());
                }
                None => {
                    self.acc_descr_buf.push_str(raw);
                    self.acc_descr_buf.push('\n');
                }
            }
            return Ok(());
        }
        for stmt in split_statements(raw) {
            self.statement(&stmt, number)?;
        }
        Ok(())
    }

    fn statement(&mut self, stmt: &str, line: usize) -> Result<(), ParseError> {
        let t = stmt.trim();
        if t.is_empty() {
            return Ok(());
        }
        let (word, rest) = head_word(t);
        match word.as_str() {
            "participant" => self.participant(rest, ParticipantKind::Participant, line, false),
            "actor" => self.participant(rest, ParticipantKind::Actor, line, false),
            "create" => {
                let (w2, rest2) = head_word(rest);
                match w2.as_str() {
                    "participant" => {
                        self.participant(rest2, ParticipantKind::Participant, line, true)
                    }
                    "actor" => self.participant(rest2, ParticipantKind::Actor, line, true),
                    // Upstream's grammar has no `create <id>` production; reading it as one is
                    // forgiving in the direction that draws more, never less.
                    _ => self.participant(rest, ParticipantKind::Participant, line, true),
                }
            }
            "destroy" => self.destroy(rest, line),
            "box" => self.open_box(rest, line),
            "end" => self.close(line),
            "loop" => self.open_block(BlockKind::Loop, "loop", rest, line),
            // A `rect`'s `restOfLine` is a **colour**, not a title: `rect rgb(0, 255, 0)`. konoma
            // does not paint with it (§1), and drawing it as the frame's caption would put
            // `rgb(191, 223, 255)` across the top of the highlight it marks.
            "rect" => {
                let (_color, title) = split_box_data(strip_wrap(rest));
                self.open_block(BlockKind::Rect, "rect", &title, line)
            }
            "opt" => self.open_block(BlockKind::Opt, "opt", rest, line),
            "alt" => self.open_block(BlockKind::Alt, "alt", rest, line),
            "par" => self.open_block(BlockKind::Par, "par", rest, line),
            "par_over" => self.open_block(BlockKind::ParOver, "par_over", rest, line),
            "critical" => self.open_block(BlockKind::Critical, "critical", rest, line),
            "break" => self.open_block(BlockKind::Break, "break", rest, line),
            "else" => self.section(SectionKind::Else, rest, line),
            "and" => self.section(SectionKind::And, rest, line),
            "option" => self.section(SectionKind::Option, rest, line),
            "note" => self.note(rest, line),
            // Actor menus. §2-3's rule applies with force: there is nothing to click in a
            // terminal, but a statement the parser cannot read becomes a phantom participant, and
            // these do declare a real one.
            "links" | "link" | "properties" | "details" => {
                let (name, _payload) = split_once_colon(rest);
                let name = name.trim();
                if name.is_empty() {
                    return Err(ParseError::MissingActor {
                        keyword: "link",
                        line,
                    });
                }
                self.diagram.ensure(name, ParticipantKind::Participant);
                Ok(())
            }
            "activate" => self.activate(rest, line),
            "deactivate" => self.deactivate(rest, line),
            "autonumber" => self.autonumber(rest),
            "title" => {
                self.diagram.title = non_empty(decode_label(rest));
                Ok(())
            }
            _ => {
                if let Some(v) = after_colon_keyword(t, "title") {
                    self.diagram.title = non_empty(decode_label(&v));
                    return Ok(());
                }
                if let Some(v) = after_colon_keyword(t, "accTitle") {
                    self.diagram.acc_title = non_empty(decode_label(&v));
                    return Ok(());
                }
                if starts_ci(t, "accDescr") {
                    let after = t[8..].trim_start();
                    if let Some(body) = after.strip_prefix('{') {
                        match body.find('}') {
                            Some(i) => {
                                self.diagram.acc_descr = non_empty(body[..i].trim().to_string());
                            }
                            None => {
                                self.in_acc_descr = true;
                                self.acc_descr_buf = format!("{body}\n");
                            }
                        }
                        return Ok(());
                    }
                    if let Some(v) = after.strip_prefix(':') {
                        self.diagram.acc_descr = non_empty(decode_label(v));
                        return Ok(());
                    }
                }
                self.message(t, line)
            }
        }
    }

    /// `participant A`, `actor A`, either with `as Alias`, either with an `@{ … }` block, and
    /// either behind `create`.
    fn participant(
        &mut self,
        rest: &str,
        kind: ParticipantKind,
        line: usize,
        created: bool,
    ) -> Result<(), ParseError> {
        let (head, alias) = split_as(rest);
        let (id, config) = split_config(head);
        let id = id.trim();
        if id.is_empty() {
            return Err(ParseError::MissingActor {
                keyword: "participant",
                line,
            });
        }
        if created && self.diagram.has(id) {
            return Err(ParseError::DuplicateCreate {
                name: id.to_string(),
                line,
            });
        }
        // `@{ "type": "database" }`: every type but `actor` is drawn as a plain participant
        // (divergence 6), and an inline `"alias"` loses to an external `as` one — which is
        // upstream's own precedence rule, stated in its documentation.
        let mut kind = kind;
        let mut label: Option<String> = alias.map(|a| decode_label(strip_wrap(a)));
        if let Some(config) = config {
            if config_value(config, "type").as_deref() == Some("actor") {
                kind = ParticipantKind::Actor;
            }
            if label.is_none() {
                label = config_value(config, "alias").map(|a| decode_label(&a));
            }
        }

        let existed = self.diagram.has(id);
        let i = self.diagram.ensure(id, kind);
        let p = &mut self.diagram.participants[i];
        // A second `participant A` never nulls a label the first one set (upstream's
        // "don't allow description nulling"), and never demotes an actor back to a box.
        if let Some(label) = label {
            p.label = label;
        }
        if kind == ParticipantKind::Actor {
            p.kind = ParticipantKind::Actor;
        }
        if created && !existed {
            p.created_at = Some(self.diagram.events.len());
            self.diagram.events.push(Event::Create(id.to_string()));
            self.last_created = Some(id.to_string());
        }
        if let Some(open) = self.open.last() {
            if open.block.is_none() {
                let box_id = self
                    .diagram
                    .boxes
                    .last()
                    .map(|b| b.id.clone())
                    .unwrap_or_default();
                if let Some(b) = self.diagram.boxes.last_mut() {
                    if !b.members.iter().any(|m| m == id) {
                        b.members.push(id.to_string());
                    }
                }
                let p = &mut self.diagram.participants[i];
                if p.box_id.is_none() {
                    p.box_id = Some(box_id);
                }
            }
        }
        Ok(())
    }

    /// `destroy A`. Does **not** bring a participant into existence — the grammar changes the
    /// statement's type and never calls `addActor`.
    fn destroy(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        let id = rest.trim();
        if id.is_empty() {
            return Err(ParseError::MissingActor {
                keyword: "destroy",
                line,
            });
        }
        let at = self.diagram.events.len();
        if let Some(p) = self.diagram.participant_mut(id) {
            p.destroyed_at = Some(at);
        }
        self.diagram.events.push(Event::Destroy(id.to_string()));
        self.last_destroyed = Some(id.to_string());
        Ok(())
    }

    fn open_box(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        let (color, title) = split_box_data(strip_wrap(rest));
        let id = format!("box{}", self.diagram.boxes.len());
        self.diagram.boxes.push(ParticipantBox {
            id,
            title: decode_label(&title),
            color,
            members: Vec::new(),
        });
        self.open.push(Open {
            block: None,
            keyword: "box",
            line,
        });
        Ok(())
    }

    fn open_block(
        &mut self,
        kind: BlockKind,
        keyword: &'static str,
        rest: &str,
        line: usize,
    ) -> Result<(), ParseError> {
        self.diagram.events.push(Event::BlockStart {
            kind,
            title: decode_label(strip_wrap(rest)),
        });
        self.open.push(Open {
            block: Some(kind),
            keyword,
            line,
        });
        Ok(())
    }

    fn section(&mut self, kind: SectionKind, rest: &str, line: usize) -> Result<(), ParseError> {
        if !self.open.iter().any(|o| o.block.is_some()) {
            return Err(ParseError::SectionOutsideBlock {
                keyword: kind.keyword(),
                line,
            });
        }
        self.diagram.events.push(Event::Section {
            kind,
            title: decode_label(strip_wrap(rest)),
        });
        Ok(())
    }

    fn close(&mut self, line: usize) -> Result<(), ParseError> {
        let Some(open) = self.open.pop() else {
            return Err(ParseError::UnmatchedEnd { line });
        };
        if open.block.is_some() {
            self.diagram.events.push(Event::BlockEnd);
        }
        Ok(())
    }

    /// `note left of A: t`, `note right of A: t`, `note over A: t`, `note over A,B: t`.
    fn note(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        let (placement, after) = if starts_ci(rest, "left of") {
            (Placement::LeftOf, rest[7..].trim_start())
        } else if starts_ci(rest, "right of") {
            (Placement::RightOf, rest[8..].trim_start())
        } else if starts_ci(rest, "over") {
            (Placement::Over, rest[4..].trim_start())
        } else {
            return Err(ParseError::MissingActor {
                keyword: "note",
                line,
            });
        };
        let (names, text) = split_once_colon(after);
        let actors: Vec<String> = names
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if actors.is_empty() {
            return Err(ParseError::MissingActor {
                keyword: "note",
                line,
            });
        }
        // A note declares its participants, exactly as a message does.
        for a in &actors {
            self.diagram.ensure(a, ParticipantKind::Participant);
        }
        // `note over A,B,C` is `[].concat($3,$3).slice(0,2)` upstream: only the first two count.
        let actors = actors.into_iter().take(2).collect();
        self.diagram.events.push(Event::Note(Note {
            actors,
            placement,
            text: decode_label(strip_wrap(&text)),
        }));
        Ok(())
    }

    fn activate(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        let id = rest.trim();
        if id.is_empty() {
            return Err(ParseError::MissingActor {
                keyword: "activate",
                line,
            });
        }
        self.push_activate(id);
        Ok(())
    }

    fn deactivate(&mut self, rest: &str, line: usize) -> Result<(), ParseError> {
        let id = rest.trim();
        if id.is_empty() {
            return Err(ParseError::MissingActor {
                keyword: "deactivate",
                line,
            });
        }
        self.push_deactivate(id, line)
    }

    fn push_activate(&mut self, id: &str) {
        match self.active.iter_mut().find(|(n, _)| n == id) {
            Some((_, n)) => *n += 1,
            None => self.active.push((id.to_string(), 1)),
        }
        self.diagram.events.push(Event::Activate(id.to_string()));
    }

    fn push_deactivate(&mut self, id: &str, line: usize) -> Result<(), ParseError> {
        let count = self
            .active
            .iter_mut()
            .find(|(n, _)| n == id)
            .map(|(_, n)| n)
            .filter(|n| **n > 0);
        let Some(count) = count else {
            return Err(ParseError::NotActive {
                name: id.to_string(),
                line,
            });
        };
        *count -= 1;
        self.diagram.events.push(Event::Deactivate(id.to_string()));
        Ok(())
    }

    /// `autonumber`, `autonumber off`, `autonumber <start>`, `autonumber <start> <step>`.
    fn autonumber(&mut self, rest: &str) -> Result<(), ParseError> {
        let words: Vec<&str> = rest.split_whitespace().collect();
        let cmd = match words.first().map(|w| w.to_ascii_lowercase()) {
            Some(w) if w == "off" => AutoNumber::Off,
            _ => {
                let start = words.first().and_then(|w| w.parse::<f64>().ok());
                let step = words.get(1).and_then(|w| w.parse::<f64>().ok());
                AutoNumber::On {
                    start: start.filter(|v| v.is_finite()).unwrap_or(1.0),
                    step: step.filter(|v| v.is_finite()).unwrap_or(1.0),
                }
            }
        };
        self.diagram.events.push(Event::AutoNumber(cmd));
        Ok(())
    }

    fn message(&mut self, stmt: &str, line: usize) -> Result<(), ParseError> {
        let Some((start, end, signal)) = find_arrow(stmt) else {
            // Not a message and not a keyword: upstream lexes it as `INVALID` and its `line`
            // production throws the token away. So does konoma — a line nobody can read must not
            // become a participant.
            return Ok(());
        };
        let mut from = stmt[..start].trim();
        let mut rest = stmt[end..].trim_start();
        // `A()->>B` / `A->>()B` / `A()->>()B` — a central connection. Parsed so the `()` cannot
        // become part of a name; drawn as an ordinary message (divergence 5).
        if let Some(f) = from.strip_suffix("()") {
            from = f.trim_end();
        }
        if let Some(r) = rest.strip_prefix("()") {
            rest = r.trim_start();
        }
        let mut activate_to = false;
        let mut deactivate_from = false;
        if let Some(r) = rest.strip_prefix('+') {
            activate_to = true;
            rest = r.trim_start();
        } else if let Some(r) = rest.strip_prefix('-') {
            deactivate_from = true;
            rest = r.trim_start();
        }
        if let Some(r) = rest.strip_prefix("()") {
            rest = r.trim_start();
        }
        let (to, text) = split_once_colon(rest);
        let (from, to) = (from.trim().to_string(), to.trim().to_string());
        if from.is_empty() || to.is_empty() {
            return Ok(());
        }
        self.messages += 1;
        if self.messages > MAX_MESSAGES {
            return Err(ParseError::TooManyMessages {
                limit: MAX_MESSAGES,
            });
        }

        // Upstream's `signal` production returns `[$1, $3, {addMessage…}]`, so the two endpoints
        // are registered — in this order — before the message itself.
        self.diagram.ensure(&from, ParticipantKind::Participant);
        self.diagram.ensure(&to, ParticipantKind::Participant);
        self.last_created = None;
        self.last_destroyed = None;
        self.diagram.events.push(Event::Message(Message {
            from: from.clone(),
            to: to.clone(),
            text: decode_label(strip_wrap(&text)),
            signal,
            line,
        }));
        // `+` opens the **receiver**'s bar; `-` closes the **sender**'s. They are not a pair.
        if activate_to {
            self.push_activate(&to);
        }
        if deactivate_from {
            self.push_deactivate(&from, line)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<SequenceDiagram, ParseError> {
        if let Some(open) = self.open.first() {
            return Err(ParseError::UnclosedBlock {
                keyword: open.keyword,
                line: open.line,
            });
        }
        if self.in_acc_descr {
            let text = std::mem::take(&mut self.acc_descr_buf);
            self.diagram.acc_descr = non_empty(text.trim().to_string());
        }
        if self.diagram.participants.is_empty() {
            return Err(ParseError::NoParticipants);
        }
        Ok(self.diagram)
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Splits at the first `:`, returning the part before it and the part after.
///
/// The part after keeps neither the `:` nor a `wrap:` prefix. A statement with no `:` yields an
/// empty second half, which is divergence 3.
fn split_once_colon(s: &str) -> (String, String) {
    match s.find(':') {
        Some(i) => (s[..i].to_string(), strip_wrap(&s[i + 1..]).to_string()),
        None => (s.to_string(), String::new()),
    }
}

/// Splits `A as Alias` at the ` as ` — a whole word, case-insensitively, as the `ALIAS` lexer
/// state requires (`(?=\s+as\s)`).
fn split_as(s: &str) -> (&str, Option<&str>) {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (n, (i, c)) in chars.iter().enumerate() {
        if !c.is_whitespace() {
            continue;
        }
        let rest = &s[*i..];
        let trimmed = rest.trim_start();
        if !starts_ci(trimmed, "as") {
            continue;
        }
        let after = &trimmed[2..];
        if !after.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let _ = n;
        return (s[..*i].trim_end(), Some(after.trim_start()));
    }
    (s.trim(), None)
}

/// Splits `Alice@{ "type": "boundary" }` into the id and the block's contents.
fn split_config(s: &str) -> (&str, Option<&str>) {
    let Some(at) = s.find("@{") else {
        return (s, None);
    };
    let Some(close) = s[at..].find('}') else {
        return (s, None);
    };
    (s[..at].trim_end(), Some(&s[at + 2..at + close]))
}

/// Reads one `"key": "value"` out of a participant's `@{ … }` block.
///
/// Deliberately not a YAML parser. The block's documented contents are `type` and `alias`, both
/// scalar strings, and every other key configures something konoma does not draw — so the same
/// reasoning as `front_matter_title` applies: read the two keys that matter and ignore the rest,
/// rather than take on a YAML dependency for a two-field object.
fn config_value(config: &str, key: &str) -> Option<String> {
    for part in config.split(',') {
        let (k, v) = part.split_once(':')?;
        let k = k.trim().trim_matches(['"', '\'']).trim();
        if !k.eq_ignore_ascii_case(key) {
            continue;
        }
        let v = v.trim().trim_matches(['"', '\'']).trim();
        if v.is_empty() {
            return None;
        }
        return Some(v.to_string());
    }
    None
}

/// mermaid's `parseBoxData`: the first word is the colour if it looks like one, and the rest is
/// the title. `box transparent Aqua` is the documented escape hatch for a title that *is* a
/// colour name.
///
/// konoma keeps the colour and does not paint with it (see [`SequenceDiagram`]), so the only job
/// this does that a reader can see is keeping a colour word out of the title.
fn split_box_data(s: &str) -> (Option<String>, String) {
    let t = s.trim();
    if t.is_empty() {
        return (None, String::new());
    }
    // `rgb(…)` / `rgba(…)` / `hsl(…)` / `hsla(…)` run to their closing bracket.
    for f in ["rgba", "rgb", "hsla", "hsl"] {
        if starts_ci(t, f) {
            let after = &t[f.len()..];
            if after.trim_start().starts_with('(') {
                if let Some(close) = t.find(')') {
                    return (
                        Some(t[..=close].to_string()),
                        t[close + 1..].trim().to_string(),
                    );
                }
            }
        }
    }
    let (first, rest) = head_word(t);
    if is_color_name(&first) {
        return (Some(first), rest.to_string());
    }
    (None, t.to_string())
}

/// The CSS colour names mermaid's `window.CSS.supports('color', …)` accepts, as far as a box
/// title needs to care.
///
/// The full list is 148 names. Testing membership of all of them would buy nothing a reader can
/// see — konoma does not paint the box — so this holds the ones the documentation uses plus the
/// obvious primaries, and anything else stays part of the title. That is the safe direction: a
/// colour word left in a title is visible and harmless, a title word eaten as a colour is lost.
fn is_color_name(word: &str) -> bool {
    const NAMES: &[&str] = &[
        "transparent",
        "aqua",
        "aquamarine",
        "black",
        "blue",
        "brown",
        "coral",
        "cyan",
        "gold",
        "gray",
        "green",
        "grey",
        "indigo",
        "khaki",
        "lavender",
        "lime",
        "magenta",
        "maroon",
        "navy",
        "olive",
        "orange",
        "pink",
        "purple",
        "red",
        "salmon",
        "silver",
        "tan",
        "teal",
        "violet",
        "wheat",
        "white",
        "yellow",
    ];
    NAMES.contains(&word)
}

/// `accTitle: X` and the legacy `title: X`, where the keyword and the `:` are one token upstream.
fn after_colon_keyword(stmt: &str, keyword: &str) -> Option<String> {
    if !starts_ci(stmt, keyword) {
        return None;
    }
    let after = stmt[keyword.len()..].trim_start();
    Some(after.strip_prefix(':')?.trim().to_string())
}

/// Whether the participant is drawn as a stick figure. Used by the renderer and the tests.
pub fn is_actor(p: &Participant) -> bool {
    p.kind == ParticipantKind::Actor
}
