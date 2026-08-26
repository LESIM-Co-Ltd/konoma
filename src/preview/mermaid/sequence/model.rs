//! The intermediate representation the sequence parser produces.
//!
//! Structure, no geometry — the same contract the other four parsers have. The shape is mermaid's
//! own (`sequenceDb.ts`): a list of participants in the order they first appeared, the boxes that
//! group them, and **one flat stream of events in source order**. Nesting is not a tree here
//! because it is not a tree upstream either: a `loop` is a `BlockStart` event and an `end` is a
//! `BlockEnd` event, and everything between them is simply what came in between. That is what
//! makes "a message's y is greater than the previous message's" a statement about a list rather
//! than a walk, and it is why the renderer can lay the diagram out with one pass and a stack.

use std::collections::HashMap;
use std::fmt;

/// How a participant is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ParticipantKind {
    /// `participant A` — a labelled box.
    #[default]
    Participant,
    /// `actor A` — a stick figure over the name. See [`render::sequence`] for why konoma draws
    /// the figure where the crate it replaces draws another box.
    ///
    /// [`render::sequence`]: crate::preview::mermaid::render::sequence
    Actor,
}

/// One participant, in the order it first appeared.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Participant {
    /// The name messages refer to it by.
    pub id: String,
    /// The text drawn: the alias when `participant A as Alice` gave one, else the id.
    pub label: String,
    /// Box or stick figure.
    pub kind: ParticipantKind,
    /// Index into [`SequenceDiagram::events`] of the `create` that declared it, if any. A created
    /// participant is not drawn until the message that creates it.
    pub created_at: Option<usize>,
    /// Index into [`SequenceDiagram::events`] of the `destroy` that ended it, if any.
    pub destroyed_at: Option<usize>,
    /// The `box` that holds it, if any.
    pub box_id: Option<String>,
}

/// One `box` — a group of participants drawn as a vertical band.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticipantBox {
    /// Generated: `box<n>`.
    pub id: String,
    /// The text on the band. Empty for `box` with no title.
    pub title: String,
    /// The colour the author asked for, kept and **not drawn** — see [`SequenceDiagram`].
    pub color: Option<String>,
    /// The participants inside, in declaration order.
    pub members: Vec<String>,
}

/// Whether a message's shaft is drawn solid or dashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Line {
    /// One dash: `->`, `->>`, `-x`, `-)`.
    #[default]
    Solid,
    /// Two dashes: `-->`, `-->>`, `--x`, `--)`. A reply, by convention.
    Dotted,
}

/// What is drawn at one end of a message.
///
/// Four kinds, which is what mermaid's marker set comes to once the sixteen half-arrow spellings
/// are folded together (see [`super::parser`]): nothing, a filled triangle, a cross, and the
/// concave chevron mermaid calls `filled-head` and its documentation calls "an open arrow
/// (async)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Head {
    /// `->` and `-->`: mermaid's documentation calls these "solid/dotted line **without** arrow",
    /// and its renderer really does attach no marker to them.
    #[default]
    None,
    /// `->>`, `-->>`, and both ends of `<<->>` / `<<-->>`.
    Arrow,
    /// `-x` and `--x`.
    Cross,
    /// `-)` and `--)` — an asynchronous message.
    Async,
}

/// A message's line style and the mark at each of its ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Signal {
    /// Solid or dashed.
    pub line: Line,
    /// The mark at the sending end. Only `<<->>` / `<<-->>` and the reverse half-arrows put one
    /// here.
    pub start: Head,
    /// The mark at the receiving end.
    pub end: Head,
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    /// Sender.
    pub from: String,
    /// Receiver.
    pub to: String,
    /// The text after the `:`, with `<br>` already turned into newlines.
    pub text: String,
    /// What is drawn.
    pub signal: Signal,
    /// 1-based source line, for diagnostics.
    pub line: usize,
}

/// Which side of a participant a note is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Placement {
    /// `note left of A`.
    LeftOf,
    /// `note right of A`.
    RightOf,
    /// `note over A` and `note over A,B`.
    #[default]
    Over,
}

/// One note.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    /// One participant, or the two of a `note over A,B`.
    pub actors: Vec<String>,
    /// Which side.
    pub placement: Placement,
    /// The text after the `:`.
    pub text: String,
}

/// A block statement that opens a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    /// `loop`.
    Loop,
    /// `alt`, which `else` divides.
    Alt,
    /// `opt`.
    Opt,
    /// `par`, which `and` divides.
    Par,
    /// `par_over` — a `par` mermaid draws with its sections overlapping. konoma draws it as a
    /// `par`; see [`super::parser`].
    ParOver,
    /// `critical`, which `option` divides.
    Critical,
    /// `break`.
    Break,
    /// `rect` — a background highlight. The colour is parsed and not drawn.
    Rect,
}

impl BlockKind {
    /// The keyword, as it is written and as it is drawn on the frame.
    pub fn keyword(self) -> &'static str {
        match self {
            BlockKind::Loop => "loop",
            BlockKind::Alt => "alt",
            BlockKind::Opt => "opt",
            BlockKind::Par | BlockKind::ParOver => "par",
            BlockKind::Critical => "critical",
            BlockKind::Break => "break",
            BlockKind::Rect => "rect",
        }
    }
}

/// A statement that divides an open block into sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionKind {
    /// `else`, inside `alt`.
    Else,
    /// `and`, inside `par`.
    And,
    /// `option`, inside `critical`.
    Option,
}

impl SectionKind {
    /// The keyword, as written and as drawn.
    pub fn keyword(self) -> &'static str {
        match self {
            SectionKind::Else => "else",
            SectionKind::And => "and",
            SectionKind::Option => "option",
        }
    }
}

/// What `autonumber` asked for.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AutoNumber {
    /// `autonumber`, `autonumber <start>`, `autonumber <start> <step>`.
    On {
        /// The number the next message gets.
        start: f64,
        /// How much each message after it adds.
        step: f64,
    },
    /// `autonumber off`.
    #[default]
    Off,
}

/// One thing that happened, in source order.
///
/// The stream is flat on purpose — see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A message from one participant to another.
    Message(Message),
    /// A note beside or over a participant.
    Note(Note),
    /// `activate A`, or the `+` on a message (which lands here immediately after it).
    Activate(String),
    /// `deactivate A`, or the `-` on a message.
    Deactivate(String),
    /// `loop` / `alt` / `opt` / `par` / `critical` / `break` / `rect`.
    BlockStart {
        /// Which keyword opened it.
        kind: BlockKind,
        /// The rest of the line.
        title: String,
    },
    /// `else` / `and` / `option`.
    Section {
        /// Which keyword.
        kind: SectionKind,
        /// The rest of the line.
        title: String,
    },
    /// `end`.
    BlockEnd,
    /// `autonumber` in any of its four forms.
    AutoNumber(AutoNumber),
    /// `create participant X` / `create actor X`. Carried as an event so the renderer knows *when*
    /// the participant appears.
    Create(String),
    /// `destroy X`.
    Destroy(String),
}

/// A parsed `sequenceDiagram`.
///
/// # What is parsed and deliberately not drawn
///
/// * `links` / `link` / `properties` / `details` declare an actor popup menu. There is nothing to
///   click in a terminal, so the payload is dropped — but the statement is still *parsed*, because
///   a statement a parser fails to understand becomes a phantom participant (§2-3).
/// * a `box`'s colour, and a `rect`'s. konoma composites onto the terminal (§1), so an opaque
///   highlight is a grey slab rather than a tint.
/// * `accTitle` / `accDescr`, which are for screen readers.
/// * the `wrap:` / `nowrap:` prefixes, because there is no automatic wrapping at all (§2-5).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SequenceDiagram {
    /// `title X`, `title: X`, or `title:` from a YAML front matter block. Drawn above everything.
    pub title: Option<String>,
    /// `accTitle:` — not drawn.
    pub acc_title: Option<String>,
    /// `accDescr:` in either form — not drawn.
    pub acc_descr: Option<String>,
    /// Every participant, in the order it first appeared.
    pub participants: Vec<Participant>,
    /// Every `box`, in source order.
    pub boxes: Vec<ParticipantBox>,
    /// Everything that happened, in source order.
    pub events: Vec<Event>,
    /// id -> index into [`SequenceDiagram::participants`]. Maintained by [`SequenceDiagram::ensure`]
    /// so that "has this participant been seen?" is one lookup rather than a scan.
    pub(super) index: HashMap<String, usize>,
}

impl SequenceDiagram {
    /// Looks a participant up by id.
    pub fn participant(&self, id: &str) -> Option<&Participant> {
        self.index.get(id).map(|i| &self.participants[*i])
    }

    /// Whether a participant with this id has been declared.
    pub fn has(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    /// Registers a participant if it is new, and returns its index either way.
    ///
    /// mermaid's `addActor`: **a message, a note, a `links` and a `link` all bring a participant
    /// into existence**, in the order they mention it, while `activate` and `destroy` do not (the
    /// grammar drops the `addParticipant` object those two produce). Getting that wrong reorders
    /// the columns.
    pub fn ensure(&mut self, id: &str, kind: ParticipantKind) -> usize {
        if let Some(i) = self.index.get(id) {
            return *i;
        }
        let i = self.participants.len();
        self.participants.push(Participant {
            id: id.to_string(),
            label: id.to_string(),
            kind,
            ..Participant::default()
        });
        self.index.insert(id.to_string(), i);
        i
    }

    /// Mutable access by id, for the statements that refine a participant already registered.
    pub fn participant_mut(&mut self, id: &str) -> Option<&mut Participant> {
        self.index
            .get(id)
            .copied()
            .map(|i| &mut self.participants[i])
    }

    /// Every message, in source order. The renderer walks the events; this is for the tests.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.events.iter().filter_map(|e| match e {
            Event::Message(m) => Some(m),
            _ => None,
        })
    }

    /// Every note, in source order.
    pub fn notes(&self) -> impl Iterator<Item = &Note> {
        self.events.iter().filter_map(|e| match e {
            Event::Note(n) => Some(n),
            _ => None,
        })
    }
}

/// Why a source could not be read.
///
/// Every variant is a case where mermaid also refuses, except that konoma keeps the reason
/// (§8: the crate throws its messages away). Refusing means the fence degrades to the Unicode
/// text diagram — it is **never** handed to the crate for a second opinion (§1, stage 1's rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The source does not begin with `sequenceDiagram`.
    NotASequenceDiagram {
        /// The first word that was there instead, for the message.
        header: String,
    },
    /// Nothing but blank lines and comments.
    Empty,
    /// A `sequenceDiagram` header with no participant under it.
    ///
    /// Refused rather than drawn: an empty diagram rasterises to a transparent rectangle that the
    /// pane then stretches, which is exactly the failure §6 records the current crate having.
    NoParticipants,
    /// An `end` with no block open.
    UnmatchedEnd {
        /// 1-based source line.
        line: usize,
    },
    /// A block that never met its `end`.
    UnclosedBlock {
        /// The keyword that opened it.
        keyword: &'static str,
        /// 1-based source line of that keyword.
        line: usize,
    },
    /// An `else` / `and` / `option` outside any block.
    SectionOutsideBlock {
        /// The keyword.
        keyword: &'static str,
        /// 1-based source line.
        line: usize,
    },
    /// `deactivate X` (or a `-` suffix) where `X` has nothing to deactivate. mermaid throws
    /// "Trying to inactivate an inactive participant".
    NotActive {
        /// The participant named.
        name: String,
        /// 1-based source line.
        line: usize,
    },
    /// `create participant X` where `X` already exists. mermaid throws, because the two would be
    /// one column.
    DuplicateCreate {
        /// The participant named.
        name: String,
        /// 1-based source line.
        line: usize,
    },
    /// A statement that needs a participant and was given none, e.g. a bare `note over`.
    MissingActor {
        /// The keyword that needed one.
        keyword: &'static str,
        /// 1-based source line.
        line: usize,
    },
    /// More messages than the renderer will draw. mermaid has the same kind of ceiling
    /// (`maxEdges`); without one, a generated file can hang the worker thread.
    TooManyMessages {
        /// The ceiling.
        limit: usize,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::NotASequenceDiagram { header } => {
                write!(f, "not a sequence diagram: `{header}`")
            }
            ParseError::Empty => write!(f, "sequence diagram is empty"),
            ParseError::NoParticipants => write!(f, "sequence diagram declares no participants"),
            ParseError::UnmatchedEnd { line } => {
                write!(f, "`end` with no open block at line {line}")
            }
            ParseError::UnclosedBlock { keyword, line } => {
                write!(f, "`{keyword}` at line {line} was never closed with `end`")
            }
            ParseError::SectionOutsideBlock { keyword, line } => {
                write!(f, "`{keyword}` outside any block at line {line}")
            }
            ParseError::NotActive { name, line } => {
                write!(f, "`{name}` is not active at line {line}")
            }
            ParseError::DuplicateCreate { name, line } => {
                write!(
                    f,
                    "`{name}` already exists and cannot be created at line {line}"
                )
            }
            ParseError::MissingActor { keyword, line } => {
                write!(f, "`{keyword}` names no participant at line {line}")
            }
            ParseError::TooManyMessages { limit } => {
                write!(f, "sequence diagram has more than {limit} messages")
            }
        }
    }
}

impl std::error::Error for ParseError {}
