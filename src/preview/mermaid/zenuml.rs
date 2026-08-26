//! Reading mermaid's `zenuml` language — a sequence-diagram dialect written as code.
//!
//! # This is the one language mermaid does not define
//!
//! `packages/mermaid-zenuml/src/parser.ts` is, verbatim, "a dummy parser that satisfies the
//! mermaid API logic": mermaid hands the source to `@zenuml/core`, whose grammar lives in
//! `sequenceParser.g4` in a different repository and is an **expression language** — arithmetic,
//! boolean operators, typed declarations, method chains. There is no `.jison` in mermaid to read,
//! and implementing ANTLR's `expr` is not what stage 5b is for.
//!
//! So konoma reads the **statement layer** — everything mermaid's own `zenuml.md` documents, and
//! nothing else — and **refuses anything it does not recognise**, which degrades to the Unicode
//! text diagram. That is the rule the whole of this work runs on: a diagram konoma cannot read is
//! not one it guesses at. The crate being replaced does guess, and
//! `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the result — a participant named
//! `OrderController.post(payload) {`, which is a *brace* drawn as a name.
//!
//! # It becomes a sequence diagram, not a twelfth layout
//!
//! Everything here builds a [`SequenceDiagram`] and hands it to stage 4's renderer. A ZenUML
//! source and a `sequenceDiagram` source say the same kinds of thing — participants, messages,
//! nesting, alternatives, loops — so a second layout would be a second way to be wrong about the
//! same geometry.
//!
//! | ZenUML | becomes |
//! |---|---|
//! | `A->B: text` | a solid message with no arrowhead, which is what `->` is in `sequenceDiagram` too |
//! | `A.method()` | a message **to** `A` from whoever is calling, with an activation bar on `A` |
//! | `A.method() { … }` | the same, and everything inside is called *by* `A` |
//! | `x = A.method()` | the call, then a dotted reply labelled `x` |
//! | `return x` | a dotted reply from the current callee to its caller |
//! | `new A(…)` | `create participant A` and a message to it |
//! | `if / else if / else` | `alt` / `else` |
//! | `while (c)` | `loop c` |
//! | `par` / `opt` | `par` / `opt` |
//! | `try / catch / finally` | a `try` frame with `catch` and `finally` sections |
//!
//! # The implicit starter
//!
//! `A.method()` at the top level is called by *something*, and ZenUML draws that something as a
//! lifeline of its own. When the source does not name it with `@Starter(name)`, konoma labels it
//! **`Starter`** — the word ZenUML's own grammar uses (`starterExp`) — rather than drawing an
//! unlabelled box, which reads as a rendering fault rather than as "the caller is outside this
//! diagram".

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{find_header, first_word, ParseError, Source};
use crate::preview::mermaid::sequence::{
    BlockKind, Event, Head, Line, Message, Participant, ParticipantKind, SectionKind,
    SequenceDiagram, Signal,
};

/// The keyword.
pub const KEYWORD: &str = "zenuml";

/// What the implicit caller is called when the source does not name one.
pub const STARTER: &str = "Starter";

/// Whether this source is a ZenUML diagram.
pub fn is_zenuml(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Reads a ZenUML source into the sequence model stage 4 already draws.
pub fn parse(src: &str) -> Result<SequenceDiagram, ParseError> {
    let Some(Source {
        lines,
        header_index,
        header_rest,
        front_matter_title,
    }) = find_header(src, &[KEYWORD])
    else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };

    let mut diagram = SequenceDiagram::default();
    diagram.title = front_matter_title;
    let mut b = Builder {
        diagram,
        callers: Vec::new(),
        frames: Vec::new(),
        reply_next: false,
    };

    let mut all: Vec<(usize, &str)> = Vec::new();
    if !header_rest.is_empty() {
        all.push((header_index + 1, header_rest.as_str()));
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        all.push((i + 1, line.as_str()));
    }
    for (number, line) in all {
        b.read(line, number)?;
    }
    if let Some(frame) = b.frames.last() {
        return Err(ParseError::Invalid {
            line: frame.line(),
            message: "a `{` was opened and never closed".to_string(),
        });
    }
    if b.diagram.participants.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "participant",
        });
    }
    Ok(b.diagram)
}

/// What a `}` closes.
enum Frame {
    /// A `{ … }` on a sync call: the callee is active for the length of it.
    Call { who: String, line: usize },
    /// A block that draws a frame: `if`, `while`, `par`, `opt`, `try`.
    Block { kind: BlockKind, line: usize },
}

impl Frame {
    fn line(&self) -> usize {
        match self {
            Frame::Call { line, .. } | Frame::Block { line, .. } => *line,
        }
    }
}

struct Builder {
    diagram: SequenceDiagram,
    /// The open sync calls, innermost last, as `(callee, caller)`.
    ///
    /// The **caller** is carried rather than derived from the frame below, because
    /// `Client->A.method() { … }` names a caller that is nowhere on the stack: without it, a
    /// `return` at the end of that block replies to the starter instead of to `Client`.
    callers: Vec<(String, String)>,
    frames: Vec<Frame>,
    /// Whether `@return` / `@reply` was written on the line before.
    reply_next: bool,
}

impl Frame {
    fn kind(&self) -> Option<BlockKind> {
        match self {
            Frame::Block { kind, .. } => Some(*kind),
            Frame::Call { .. } => None,
        }
    }
}

impl Builder {
    fn read(&mut self, line: &str, number: usize) -> Result<(), ParseError> {
        // `//` runs to the end of the line, and is the only comment ZenUML has.
        let t = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        }
        .trim();
        if t.is_empty() {
            return Ok(());
        }
        let unexpected = || ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        };

        // --- closing, and the keywords that close and reopen in one line -----------------------
        if let Some(rest) = t.strip_prefix('}') {
            let rest = rest.trim();
            let Some(frame) = self.frames.pop() else {
                return Err(unexpected());
            };
            if rest.is_empty() {
                self.close(frame);
                return Ok(());
            }
            // `} else {`, `} else if (c) {`, `} catch {`, `} catch (E) {`, `} finally {`
            let Some(kind) = frame.kind() else {
                return Err(unexpected());
            };
            // Every one of these reopens the block, so the trailing `{` belongs to the keyword
            // and not to the condition.
            let Some(head) = rest.strip_suffix('{').map(str::trim) else {
                return Err(unexpected());
            };
            let (section, title) = if let Some(after) = word_rest_or_bracket(head, "else") {
                match word_rest_or_bracket(after, "if") {
                    Some(cond) => (SectionKind::Else, format!("if {}", condition(cond)?)),
                    None if after.is_empty() => (SectionKind::Else, String::new()),
                    None => return Err(unexpected()),
                }
            } else if let Some(after) = word_rest_or_bracket(head, "catch") {
                (SectionKind::Catch, condition(after)?)
            } else if head.eq_ignore_ascii_case("finally") {
                (SectionKind::Finally, String::new())
            } else {
                return Err(unexpected());
            };
            let allowed = match section {
                SectionKind::Else => kind == BlockKind::Alt,
                SectionKind::Catch | SectionKind::Finally => kind == BlockKind::Try,
                SectionKind::And | SectionKind::Option => false,
            };
            if !allowed {
                return Err(unexpected());
            }
            self.diagram.events.push(Event::Section {
                kind: section,
                title,
            });
            self.frames.push(Frame::Block { kind, line: number });
            return Ok(());
        }

        // --- the envelope ------------------------------------------------------------------
        if let Some(rest) = word_rest(t, "title") {
            self.diagram.title = Some(rest.trim().to_string());
            return Ok(());
        }

        // --- annotators ---------------------------------------------------------------------
        if let Some(rest) = t.strip_prefix('@') {
            return self.annotator(rest.trim(), number, &unexpected);
        }

        // --- blocks -------------------------------------------------------------------------
        for (word, kind) in [
            ("if", BlockKind::Alt),
            ("while", BlockKind::Loop),
            ("par", BlockKind::Par),
            ("opt", BlockKind::Opt),
            ("try", BlockKind::Try),
        ] {
            let Some(rest) = word_rest_or_bracket(t, word) else {
                continue;
            };
            let Some(body) = rest.strip_suffix('{') else {
                return Err(unexpected());
            };
            let title = condition(body.trim())?;
            self.diagram.events.push(Event::BlockStart { kind, title });
            self.frames.push(Frame::Block { kind, line: number });
            return Ok(());
        }

        // --- return -------------------------------------------------------------------------
        if let Some(rest) = word_rest_or_end(t, "return") {
            let who = self.callers.last().map(|(callee, _)| callee.clone());
            let (Some(callee), Some(caller)) = (who, self.caller_of_current()) else {
                return Err(ParseError::Invalid {
                    line: number,
                    message: "`return` outside a call has nothing to reply to".to_string(),
                });
            };
            self.reply(&callee, &caller, rest.trim());
            return Ok(());
        }

        // --- messages -----------------------------------------------------------------------
        self.statement(t, number, &unexpected)
    }

    fn close(&mut self, frame: Frame) {
        match frame {
            Frame::Block { .. } => self.diagram.events.push(Event::BlockEnd),
            Frame::Call { who, .. } => {
                self.diagram.events.push(Event::Deactivate(who));
                self.callers.pop();
            }
        }
    }

    /// Who the innermost callee's own caller is.
    fn caller_of_current(&self) -> Option<String> {
        self.callers.last().map(|(_, caller)| caller.clone())
    }

    fn annotator(
        &mut self,
        rest: &str,
        number: usize,
        unexpected: &dyn Fn() -> ParseError,
    ) -> Result<(), ParseError> {
        // `@return` / `@reply` mark the *next* async message as a reply.
        if rest.eq_ignore_ascii_case("return") || rest.eq_ignore_ascii_case("reply") {
            self.reply_next = true;
            return Ok(());
        }
        if let Some(after) = word_rest_or_bracket(rest, "Starter") {
            let name = after.trim().trim_matches(['(', ')']).trim();
            let name = if name.is_empty() { STARTER } else { name };
            // The starter has to be the first lifeline, which is what it means.
            self.diagram.push_front(Participant {
                id: name.to_string(),
                label: name.to_string(),
                kind: ParticipantKind::Actor,
                ..Participant::default()
            });
            return Ok(());
        }
        // `@Actor Alice`, `@Database Bob`, and the rest of the annotator set. konoma draws the
        // two it has shapes for and every other one as a plain participant.
        let mut words = rest.split_whitespace();
        let Some(annotator) = words.next() else {
            return Err(unexpected());
        };
        let Some(name) = words.next() else {
            return Err(unexpected());
        };
        if words.next().is_some() {
            return Err(unexpected());
        }
        let kind = if annotator.eq_ignore_ascii_case("Actor") {
            ParticipantKind::Actor
        } else {
            ParticipantKind::Participant
        };
        let _ = number;
        self.declare_with(name, name, kind);
        Ok(())
    }

    fn statement(
        &mut self,
        t: &str,
        number: usize,
        unexpected: &dyn Fn() -> ParseError,
    ) -> Result<(), ParseError> {
        // An assignment in front of a call: `a = A.m()` or `SomeType a = A.m()`.
        let (assign, body) = match split_assignment(t) {
            Some((name, rest)) => (Some(name), rest),
            None => (None, t),
        };
        let (body, opens) = match body.strip_suffix('{') {
            Some(head) => (head.trim(), true),
            None => (body, false),
        };

        // `new A` / `new A(args)` — a creation message.
        if let Some(rest) = word_rest(body, "new") {
            let (name, args) = split_call(rest.trim());
            if name.is_empty() {
                return Err(unexpected());
            }
            let from = self.current_caller();
            self.declare(&name);
            self.diagram.events.push(Event::Create(name.clone()));
            let text = match args {
                Some(a) if !a.trim().is_empty() => format!("new({})", a.trim()),
                _ => "new".to_string(),
            };
            self.send(&from, &name, &text, false);
            if opens {
                self.diagram.events.push(Event::Activate(name.clone()));
                self.callers.push((name.clone(), from.clone()));
                self.frames.push(Frame::Call {
                    who: name,
                    line: number,
                });
            }
            return Ok(());
        }

        // `A->B: text` and `A->B.method()`.
        if let Some((left, right)) = body.split_once("->") {
            let from = left.trim();
            if from.is_empty() || from.contains(char::is_whitespace) {
                return Err(unexpected());
            }
            let right = right.trim();
            if let Some((to, text)) = right.split_once(':') {
                if opens {
                    return Err(unexpected());
                }
                let (to, text) = (to.trim(), text.trim());
                if to.is_empty() || to.contains('.') {
                    return Err(unexpected());
                }
                self.declare(from);
                self.declare(to);
                let reply = std::mem::take(&mut self.reply_next);
                self.send(from, to, text, reply);
                return Ok(());
            }
            // `Client->A.method() { … }` — a sync call whose caller is written.
            let Some((to, method)) = split_method(right) else {
                return Err(unexpected());
            };
            self.declare(from);
            self.declare(&to);
            self.send(from, &to, &method, false);
            self.after_call(&to, assign.as_deref(), opens, number, from);
            return Ok(());
        }

        // `A.method()` / `A.method`.
        if let Some((to, method)) = split_method(body) {
            let from = self.current_caller();
            self.declare(&to);
            self.send(&from, &to, &method, false);
            self.after_call(&to, assign.as_deref(), opens, number, &from);
            return Ok(());
        }

        if opens || assign.is_some() {
            return Err(unexpected());
        }
        // A bare name is a participant declaration; `A as Alice` gives it a label.
        if let Some((id, label)) = split_alias(body) {
            self.declare_with(&id, &label, ParticipantKind::Participant);
            return Ok(());
        }
        if body.split_whitespace().count() == 1 && is_name(body) {
            self.declare(body);
            return Ok(());
        }
        Err(unexpected())
    }

    /// What follows a sync call: an activation and a frame if it opened one, a reply if the call
    /// was assigned to a variable.
    fn after_call(
        &mut self,
        to: &str,
        assign: Option<&str>,
        opens: bool,
        number: usize,
        from: &str,
    ) {
        if opens {
            self.diagram.events.push(Event::Activate(to.to_string()));
            self.callers.push((to.to_string(), from.to_string()));
            self.frames.push(Frame::Call {
                who: to.to_string(),
                line: number,
            });
            return;
        }
        if let Some(name) = assign {
            self.reply(to, from, name);
        }
    }

    /// Whoever is calling right now.
    fn current_caller(&mut self) -> String {
        if let Some((callee, _)) = self.callers.last() {
            return callee.clone();
        }
        self.starter_id()
    }

    /// The participant a top-level call comes from, declared the first time one is needed.
    ///
    /// `@Starter(name)` puts it at the front of the participant list, so an existing first
    /// participant drawn as an actor **is** the starter; otherwise one is made, because a message
    /// has to leave from somewhere and drawing it from the first ordinary participant would say
    /// that participant called itself.
    fn starter_id(&mut self) -> String {
        if let Some(p) = self
            .diagram
            .participants
            .first()
            .filter(|p| p.kind == ParticipantKind::Actor)
        {
            return p.id.clone();
        }
        let id = STARTER.to_string();
        self.diagram.push_front(Participant {
            id: id.clone(),
            label: id.clone(),
            kind: ParticipantKind::Actor,
            ..Participant::default()
        });
        id
    }

    fn declare(&mut self, id: &str) {
        self.declare_with(id, id, ParticipantKind::Participant);
    }

    fn declare_with(&mut self, id: &str, label: &str, kind: ParticipantKind) {
        // `ensure` is the model's own registrar; it keeps the id index that `participant()`
        // reads, which a bare `push` would leave stale.
        let i = self.diagram.ensure(id, kind);
        let p = &mut self.diagram.participants[i];
        if p.label == p.id && label != id {
            p.label = label.to_string();
        }
        if kind != ParticipantKind::Participant {
            p.kind = kind;
        }
    }

    fn send(&mut self, from: &str, to: &str, text: &str, dotted: bool) {
        self.diagram.events.push(Event::Message(Message {
            from: from.to_string(),
            to: to.to_string(),
            text: text.to_string(),
            signal: Signal {
                line: if dotted { Line::Dotted } else { Line::Solid },
                start: Head::None,
                end: if dotted { Head::None } else { Head::Arrow },
            },
            line: 0,
        }));
    }

    fn reply(&mut self, from: &str, to: &str, text: &str) {
        self.declare(from);
        self.declare(to);
        self.diagram.events.push(Event::Message(Message {
            from: from.to_string(),
            to: to.to_string(),
            text: text.to_string(),
            signal: Signal {
                line: Line::Dotted,
                start: Head::None,
                end: Head::Arrow,
            },
            line: 0,
        }));
    }
}

/// `(cond)` or `cond`, with the brackets taken off. Anything else is a syntax error.
fn condition(text: &str) -> Result<String, ParseError> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    let Some(inner) = t.strip_prefix('(') else {
        return Ok(t.to_string());
    };
    let Some(inner) = inner.strip_suffix(')') else {
        return Err(ParseError::Invalid {
            line: 0,
            message: format!("`{t}` has an unclosed `(`"),
        });
    };
    Ok(inner.trim().to_string())
}

/// `x = …` / `Type x = …`, if the statement starts with one.
fn split_assignment(t: &str) -> Option<(String, &str)> {
    let eq = t.find('=')?;
    // `==` is a divider and `>=` is an operator; neither is an assignment.
    if t.as_bytes().get(eq + 1) == Some(&b'=')
        || matches!(
            t.as_bytes().get(eq.wrapping_sub(1)),
            Some(b'<' | b'>' | b'!' | b'=')
        )
    {
        return None;
    }
    let head = t[..eq].trim();
    let words: Vec<&str> = head.split_whitespace().collect();
    let name = match words.len() {
        1 => words[0],
        2 => words[1],
        _ => return None,
    };
    if !is_name(name) {
        return None;
    }
    Some((name.to_string(), t[eq + 1..].trim()))
}

/// `A.method(args)` / `A.method` → the participant and the text drawn on the arrow.
fn split_method(body: &str) -> Option<(String, String)> {
    let dot = body.find('.')?;
    let to = body[..dot].trim();
    let method = body[dot + 1..].trim();
    if to.is_empty() || method.is_empty() || !is_name(to) {
        return None;
    }
    // A chain (`A.b().c()`) is expression syntax, and reading only its first link would draw a
    // message that is not the one written.
    let (name, args) = split_call(method);
    if !is_name(&name) {
        return None;
    }
    match args {
        Some(a) if a.contains('(') => return None,
        _ => {}
    }
    Some((to.to_string(), method.to_string()))
}

/// `name(args)` → the name and the argument text, if there is a bracket.
fn split_call(text: &str) -> (String, Option<String>) {
    match text.find('(') {
        Some(i) => {
            let name = text[..i].trim().to_string();
            let rest = &text[i + 1..];
            let args = rest.strip_suffix(')').map(|a| a.to_string());
            (name, args.or_else(|| Some(rest.to_string())))
        }
        None => (text.trim().to_string(), None),
    }
}

/// `A as Alice`.
fn split_alias(body: &str) -> Option<(String, String)> {
    let words: Vec<&str> = body.split_whitespace().collect();
    if words.len() != 3 || !words[1].eq_ignore_ascii_case("as") {
        return None;
    }
    if !is_name(words[0]) {
        return None;
    }
    Some((words[0].to_string(), words[2].to_string()))
}

/// Whether `s` is an identifier this dialect can use as a participant name.
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// `word` followed by whitespace.
fn word_rest<'a>(t: &'a str, word: &str) -> Option<&'a str> {
    if !t
        .get(..word.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(word))
    {
        return None;
    }
    let after = &t[word.len()..];
    after.starts_with([' ', '\t']).then(|| after.trim_start())
}

/// The same, also accepting `word(` with no space — `if(cond)`, `while(true)`.
fn word_rest_or_bracket<'a>(t: &'a str, word: &str) -> Option<&'a str> {
    if !t
        .get(..word.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(word))
    {
        return None;
    }
    let after = &t[word.len()..];
    // The word may end the text (`} else {` once its brace is off), or be followed by its
    // condition — with or without a space, since `if(a)` and `if (a)` are both written.
    if after.is_empty() || after.starts_with(['(', ' ', '\t', '{']) {
        return Some(after.trim_start());
    }
    None
}

/// The same, also accepting the word alone on the line — `return`.
fn word_rest_or_end<'a>(t: &'a str, word: &str) -> Option<&'a str> {
    if t.eq_ignore_ascii_case(word) {
        return Some("");
    }
    word_rest(t, word)
}

#[cfg(test)]
mod tests;
