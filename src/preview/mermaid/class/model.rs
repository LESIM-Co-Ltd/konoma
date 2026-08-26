//! The intermediate representation the class-diagram parser produces.
//!
//! Structure, no geometry — the same contract [`flowchart::model`] and [`state::model`] have.
//! Nothing here knows about pixels, fonts or SVG; [`render::class`] reads this and decides all of
//! that.
//!
//! [`flowchart::model`]: crate::preview::mermaid::flowchart::model
//! [`state::model`]: crate::preview::mermaid::state::model
//! [`render::class`]: crate::preview::mermaid::render::class
//!
//! The shape of the model follows what mermaid's `ClassDB` holds after parsing rather than what
//! its grammar produces, for the same reason stage 2's did: the renderer should receive one
//! uniform thing.
//!
//! * a class's **generic** has already been split off its id (`splitClassNameAndType`), because
//!   `Square~Shape~` and `Square` are the *same class* as far as a relationship is concerned;
//! * a **member** has already been taken apart into visibility, name, parameters, return type and
//!   classifier (`ClassMember.parseMember`), because that is the only description of "what does
//!   `+deposit(amount) bool$` mean" that is not a guess;
//! * a **note** is a node of its own with an optional target, which is what `getData` makes of it.

use std::collections::HashMap;
use std::fmt;

pub use crate::preview::mermaid::flowchart::Direction;

/// What is drawn at one end of a relationship.
///
/// mermaid's `relationType` (`classDb.ts`), which is a number 0–4 there. The names are UML's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Marker {
    /// Nothing at this end.
    #[default]
    None,
    /// `<|` / `|>` — inheritance, or (on a dotted line) realization.
    Extension,
    /// `*` — composition.
    Composition,
    /// `o` — aggregation.
    Aggregation,
    /// `>` / `<` — association, and (on a dotted line) dependency. mermaid draws one mark for
    /// both and lets the line style tell them apart.
    Dependency,
    /// `()` — a provided/required interface.
    Lollipop,
}

/// How a relationship's line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Line {
    /// `--`
    #[default]
    Solid,
    /// `..`
    Dotted,
}

/// Whether a member is a field or a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MemberKind {
    /// `+String owner`
    #[default]
    Attribute,
    /// `+deposit(amount)` — anything with a `)` in it past the first character.
    Method,
}

/// One line inside a class's body, taken apart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Member {
    /// `+` public, `-` private, `#` protected, `~` package. `None` when none was written.
    pub visibility: Option<char>,
    /// The member's name, with `~T~` already turned into `<T>`.
    pub name: String,
    /// Field or function.
    pub kind: MemberKind,
    /// What was between the brackets, for a method. Empty otherwise.
    pub parameters: String,
    /// What followed the brackets, for a method. Empty otherwise.
    pub return_type: String,
    /// `*` abstract or `$` static, written at the very end of the line.
    pub classifier: Option<char>,
}

impl Member {
    /// The line as it is drawn.
    ///
    /// mermaid's `getDisplayDetails`, plus one deliberate difference: the **classifier is kept as
    /// a character** at the end of the line. Upstream turns `*` into italics and `$` into an
    /// underline, which konoma cannot do without depending on the terminal's font set having an
    /// italic face — and a member that silently loses its `$` reads as an instance member. Keeping
    /// the character the author wrote loses nothing and needs no font (§0-1).
    pub fn display(&self) -> String {
        let mut out = String::new();
        if let Some(v) = self.visibility {
            out.push(v);
        }
        out.push_str(&self.name);
        if self.kind == MemberKind::Method {
            out.push('(');
            out.push_str(&self.parameters);
            out.push(')');
            if !self.return_type.is_empty() {
                out.push_str(" : ");
                out.push_str(&self.return_type);
            }
        }
        if let Some(c) = self.classifier {
            out.push(c);
        }
        out
    }
}

/// One class.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Class {
    /// The name relationships refer to it by, generic parameter removed.
    pub id: String,
    /// The generic parameter, if the source wrote one: `Shape` for `Square~Shape~`. Empty
    /// otherwise.
    pub generic: String,
    /// The text drawn at the top of the box. The id (plus `<generic>`) unless a
    /// `class X["label"]` said otherwise.
    pub label: String,
    /// `<<interface>>`, `<<enumeration>>`, … in the order they were written.
    pub annotations: Vec<String>,
    /// Fields, in the order they were written.
    pub attributes: Vec<Member>,
    /// Methods, in the order they were written.
    pub methods: Vec<Member>,
    /// Names from `:::name` or a `cssClass` statement. konoma does not colour classes; the names
    /// are carried so that it can.
    pub css_classes: Vec<String>,
    /// The namespace that holds it, if any.
    pub parent: Option<String>,
}

/// One relationship, `A <|-- B`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Relation {
    /// Generated: `r<n>`.
    pub id: String,
    /// The class written on the left.
    pub from: String,
    /// The class written on the right.
    pub to: String,
    /// The mark at the `from` end.
    pub start: Marker,
    /// The mark at the `to` end.
    pub end: Marker,
    /// Solid or dotted.
    pub line: Line,
    /// The text after the `:`.
    pub label: Option<String>,
    /// The quoted string written next to `from`: `"1"`, `"0..*"`, `"many"`.
    pub start_cardinality: Option<String>,
    /// The quoted string written next to `to`.
    pub end_cardinality: Option<String>,
}

/// One `note` — floating, or attached to a class.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    /// Generated: `note<n>`, which is also mermaid's spelling.
    pub id: String,
    /// The text between the quotes.
    pub text: String,
    /// The class a `note for X` belongs to. `None` for a floating note.
    pub target: Option<String>,
    /// The namespace the note was written in, if any.
    pub parent: Option<String>,
}

/// One `namespace`, drawn as a frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Namespace {
    /// The fully qualified id: `Company.Engineering` for a `namespace Engineering` written inside
    /// `namespace Company`, and for `namespace Company.Engineering` written at the top level.
    pub id: String,
    /// The text on the frame.
    pub label: String,
    /// Direct members by id, in declaration order — classes, notes and nested namespaces alike.
    pub members: Vec<String>,
    /// The namespace that holds this one.
    pub parent: Option<String>,
}

/// A parsed `classDiagram` / `classDiagram-v2`.
#[derive(Debug, Clone, Default)]
pub struct ClassDiagram {
    /// Layout direction. `TB` unless a top-level `direction` said otherwise.
    pub direction: Direction,
    /// Every class, in the order it first appeared.
    pub classes: Vec<Class>,
    /// Every relationship, in source order.
    pub relations: Vec<Relation>,
    /// Every note, in source order.
    pub notes: Vec<Note>,
    /// Every namespace, outermost first.
    pub namespaces: Vec<Namespace>,
    /// `title:` from a YAML front matter block.
    pub title: Option<String>,
    /// `accTitle:` — not drawn.
    pub acc_title: Option<String>,
    /// `accDescr:` in either form — not drawn.
    pub acc_descr: Option<String>,
    /// id -> index into [`ClassDiagram::classes`].
    index: HashMap<String, usize>,
}

impl ClassDiagram {
    /// An empty diagram. The id index is private, so a struct literal cannot be written outside.
    pub(super) fn new(direction: Direction, title: Option<String>) -> ClassDiagram {
        ClassDiagram {
            direction,
            title,
            ..ClassDiagram::default()
        }
    }

    /// Looks a class up by id.
    pub fn class(&self, id: &str) -> Option<&Class> {
        self.index.get(id).and_then(|i| self.classes.get(*i))
    }

    /// The ids of every class, in declaration order.
    pub fn class_ids(&self) -> Vec<&str> {
        self.classes.iter().map(|c| c.id.as_str()).collect()
    }

    /// Adds a class if it is new, and returns its index either way. mermaid's `addClass`, which
    /// is likewise a no-op for a name it already knows.
    pub(super) fn intern(&mut self, id: &str, generic: &str) -> usize {
        if let Some(i) = self.index.get(id) {
            return *i;
        }
        let i = self.classes.len();
        let label = if generic.is_empty() {
            id.to_string()
        } else {
            format!("{id}<{generic}>")
        };
        self.classes.push(Class {
            id: id.to_string(),
            generic: generic.to_string(),
            label,
            ..Class::default()
        });
        self.index.insert(id.to_string(), i);
        i
    }

    pub(super) fn class_mut(&mut self, id: &str) -> Option<&mut Class> {
        let i = *self.index.get(id)?;
        self.classes.get_mut(i)
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }
}

/// Why a source could not be read as a class diagram.
///
/// Same contract as the other two parsers (§1): a failure means *do not draw*, and the caller
/// degrades to the Unicode text diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The source held no statements at all.
    Empty,
    /// The first keyword was not `classDiagram` or `classDiagram-v2`.
    NotAClassDiagram {
        /// The keyword that was found, for the message.
        header: String,
    },
    /// The header was a class diagram but nothing in the body draws.
    NoClasses,
    /// A `class X {` or `namespace X {` was opened and never closed.
    UnclosedBody {
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
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty diagram"),
            ParseError::NotAClassDiagram { header } => {
                write!(f, "not a class diagram: diagram starts with `{header}`")
            }
            ParseError::NoClasses => write!(f, "class diagram declares no classes"),
            ParseError::UnclosedBody { id, line } => {
                write!(f, "unclosed `{id} {{` opened at line {line}")
            }
            ParseError::UnclosedString { line } => write!(f, "unclosed `\"` at line {line}"),
        }
    }
}

impl std::error::Error for ParseError {}
