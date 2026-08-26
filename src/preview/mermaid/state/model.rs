//! The intermediate representation the state-diagram parser produces.
//!
//! Stage 2's whole output: structure, no geometry. Nothing here knows about pixels, fonts or
//! SVG — the renderer reads this and decides all of that, exactly as it does for
//! [`flowchart`](crate::preview::mermaid::flowchart).
//!
//! The shape of the model mirrors what mermaid's own `StateDB` ends up holding *after* its
//! `docTranslator` pass, not what its grammar produces:
//!
//! * `[*]` has already become a named node — `root_start`, `Active_end`, … — because that
//!   renaming is the only description of "which `[*]` is which" that is not a guess
//!   (`stateDb.ts`'s `docTranslator`);
//! * a `--` concurrency divider has already turned the block it lives in into a set of
//!   anonymous sub-blocks;
//! * a `note` has already become a node of its own plus the link that ties it to its state.
//!
//! Doing all three in the parser means the renderer sees one uniform thing: states, links, and
//! a nesting relation. That is what lets stage 2 reuse stage 1's layout instead of growing a
//! second one (`docs/FEATURE-MERMAID-RENDERER.md` §7).

use std::collections::HashMap;
use std::fmt;

pub use crate::preview::mermaid::flowchart::Direction;

/// What a state is drawn as.
///
/// mermaid keeps this in two places at once — `StateStmt::type` for the `<<…>>` kinds and a
/// separate `start: boolean` for `[*]` — and then folds them into one `shape` string in
/// `dataFetcher`. This enum is that fold, done once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Kind {
    /// An ordinary state: `s1`, `state "text" as s1`, `s1 : text`.
    #[default]
    Simple,
    /// The `[*]` an arrow *leaves*: the filled dot a reader starts at.
    Start,
    /// The `[*]` an arrow *enters*: the ringed dot a reader stops at.
    End,
    /// `state x <<choice>>` — a branch with no text of its own.
    Choice,
    /// `state x <<fork>>` — one thread becomes several.
    Fork,
    /// `state x <<join>>` — several threads become one.
    Join,
    /// `state x { … }` — a state with a diagram inside it. Drawn as a frame, never as a box.
    Composite,
    /// One side of a `--` divider: an anonymous block holding one concurrent region.
    Concurrent,
    /// A `note left of` / `note right of`, lifted into a node of its own.
    Note,
}

impl Kind {
    /// Whether this kind is drawn as a frame around other states rather than as a box.
    pub fn is_block(self) -> bool {
        matches!(self, Kind::Composite | Kind::Concurrent)
    }

    /// Whether this kind carries no text at all, however the source described it.
    ///
    /// mermaid draws nothing inside `[*]`, a choice diamond or a fork bar — `stateStart.ts`,
    /// `choice.ts` and `forkJoin.ts` emit a shape and no `<text>` — so a description written on
    /// one is not lost by konoma, it was never drawn by anybody.
    pub fn is_textless(self) -> bool {
        matches!(
            self,
            Kind::Start | Kind::End | Kind::Choice | Kind::Fork | Kind::Join | Kind::Concurrent
        )
    }
}

/// Where a note sits relative to the state it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotePosition {
    /// `note left of s1`
    Left,
    /// `note right of s1`
    Right,
}

/// One state of the diagram — including the ones the source never named, which the parser
/// invented for `[*]`, for a `--` region and for a `note`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The identifier this state is referred to by. Generated for `[*]`, for a divider region
    /// and for a note.
    pub id: String,
    /// The text drawn on it, already decoded. Empty when the state has no text of its own —
    /// which for a [`Kind::is_textless`] kind is always.
    pub label: String,
    /// How it is drawn.
    pub kind: Kind,
    /// The block that contains it, if any.
    pub parent: Option<String>,
    /// Direct members, in declaration order, for a block. Empty for anything else. A nested
    /// block appears here by its own id, so the tree is read by following ids — the same shape
    /// as [`flowchart::Subgraph::members`](crate::preview::mermaid::flowchart::Subgraph).
    pub members: Vec<String>,
    /// Class names from `:::name` or a `class` statement, in the order applied. konoma does not
    /// colour states yet; the names are carried so that it can.
    pub classes: Vec<String>,
    /// Where the note sits, for [`Kind::Note`]. `None` for everything else.
    pub note_position: Option<NotePosition>,
    /// Whether the label came from more than one `s1 : …` line, which mermaid draws as a box
    /// with a rule under its first line (`SHAPE_STATE_WITH_DESC`).
    pub titled: bool,
}

/// One transition, `s1 --> s2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// Generated: `t<n>` for a real transition, `<state>-<note>` for a note's tie-line.
    pub id: String,
    /// Source state id.
    pub from: String,
    /// Target state id.
    pub to: String,
    /// The text on the arrow, from `s1 --> s2 : text`.
    pub label: Option<String>,
    /// True for the line that ties a note to its state, which is drawn without an arrow head.
    pub is_note_link: bool,
}

/// A parsed `stateDiagram` / `stateDiagram-v2`.
#[derive(Debug, Clone, Default)]
pub struct StateDiagram {
    /// Layout direction. `TB` unless a top-level `direction` statement said otherwise.
    ///
    /// Unlike a flowchart — where mermaid pushes a top-level `direction` onto an array nothing
    /// reads — a state diagram really does honour it (`stateDb.ts`'s `setDirection` /
    /// `getDirection`), and the documentation shows it being used that way.
    pub direction: Direction,
    /// Every state, in the order it first appears.
    pub states: Vec<State>,
    /// Every transition, in source order.
    pub transitions: Vec<Transition>,
    /// Ids of the top-level members, in order.
    pub roots: Vec<String>,
    /// `title:` from a YAML front matter block.
    pub title: Option<String>,
    /// `accTitle:` — an accessibility title. Not drawn.
    pub acc_title: Option<String>,
    /// `accDescr:` in either spelling. Not drawn.
    pub acc_descr: Option<String>,
    /// id -> index into [`StateDiagram::states`].
    index: HashMap<String, usize>,
}

impl StateDiagram {
    /// An empty diagram with a direction and (optionally) a front matter title. Used by the
    /// parser; the id index is private, so a struct literal cannot be written from outside.
    pub(super) fn new(direction: Direction, title: Option<String>) -> StateDiagram {
        StateDiagram {
            direction,
            title,
            ..StateDiagram::default()
        }
    }

    /// Looks a state up by id.
    pub fn state(&self, id: &str) -> Option<&State> {
        self.index.get(id).and_then(|i| self.states.get(*i))
    }

    /// The ids of every state, in declaration order.
    pub fn state_ids(&self) -> Vec<&str> {
        self.states.iter().map(|s| s.id.as_str()).collect()
    }

    /// Every state that is drawn as a box rather than as a frame.
    pub fn boxes(&self) -> impl Iterator<Item = &State> {
        self.states.iter().filter(|s| !s.kind.is_block())
    }

    /// Every state that is drawn as a frame.
    pub fn blocks(&self) -> impl Iterator<Item = &State> {
        self.states.iter().filter(|s| s.kind.is_block())
    }

    /// Adds a state, or returns the index of the one that already has this id. Parser only.
    pub(super) fn intern(&mut self, id: &str, kind: Kind) -> usize {
        if let Some(i) = self.index.get(id) {
            let i = *i;
            // mermaid's `addState`: a later mention only *fills in* a kind, never replaces one
            // that is already more specific than `default`.
            if self.states[i].kind == Kind::Simple && kind != Kind::Simple {
                self.states[i].kind = kind;
            }
            return i;
        }
        let i = self.states.len();
        self.states.push(State {
            id: id.to_string(),
            label: String::new(),
            kind,
            parent: None,
            members: Vec::new(),
            classes: Vec::new(),
            note_position: None,
            titled: false,
        });
        self.index.insert(id.to_string(), i);
        i
    }

    pub(super) fn state_mut(&mut self, id: &str) -> Option<&mut State> {
        let i = *self.index.get(id)?;
        self.states.get_mut(i)
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }
}

/// Why a source could not be read as a state diagram.
///
/// Same contract as the flowchart parser (`docs/FEATURE-MERMAID-RENDERER.md` §1): a failure
/// means *do not draw*, and the caller degrades to the Unicode text diagram. So every variant
/// here is a case where drawing something would be worse than drawing nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The source held no statements at all.
    Empty,
    /// The first keyword was not `stateDiagram` or `stateDiagram-v2`.
    NotAStateDiagram {
        /// The keyword that was found, for the message.
        header: String,
    },
    /// The header was a state diagram but the body declared no state.
    NoStates,
    /// A `state x {` was opened and never closed, so where the block ends is a guess.
    UnclosedComposite {
        /// The block's id.
        id: String,
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `"` was opened and never closed. mermaid swallows the rest of the diagram here.
    UnclosedString {
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `note … of x` with no text after it and no `end note` to close it.
    UnclosedNote {
        /// 1-based line in the original source.
        line: usize,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty diagram"),
            ParseError::NotAStateDiagram { header } => {
                write!(f, "not a state diagram: diagram starts with `{header}`")
            }
            ParseError::NoStates => write!(f, "state diagram declares no states"),
            ParseError::UnclosedComposite { id, line } => {
                write!(f, "unclosed `state {id} {{` opened at line {line}")
            }
            ParseError::UnclosedString { line } => write!(f, "unclosed `\"` at line {line}"),
            ParseError::UnclosedNote { line } => {
                write!(
                    f,
                    "unclosed `note` at line {line} (needs `: text` or `end note`)"
                )
            }
        }
    }
}

impl std::error::Error for ParseError {}
