//! The intermediate representation the ER parser produces.
//!
//! Structure, no geometry — the same contract the other three parsers have. What is here mirrors
//! mermaid's `ErDB` after parsing: entities with their attributes, relationships with a
//! cardinality at each end, and the subgraphs that group them.

use std::collections::HashMap;
use std::fmt;

pub use crate::preview::mermaid::flowchart::Direction;

/// How many of one entity take part in a relationship, at one end of it.
///
/// mermaid's `Cardinality`. Each has several spellings — `||`, `only one` and `1` are the same
/// thing — which is why the parser maps them all onto this rather than keeping the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Cardinality {
    /// `||`, `one`, `only one`, `1`
    #[default]
    OnlyOne,
    /// `|o`, `o|`, `zero or one`, `one or zero`
    ZeroOrOne,
    /// `}o`, `o{`, `zero or more`, `zero or many`, `many`, `many(0)`, `0+`
    ZeroOrMore,
    /// `}|`, `|{`, `one or more`, `one or many`, `many(1)`, `1+`
    OneOrMore,
    /// `u` — undocumented, and drawn as a plain end. Kept so it cannot become part of a name.
    MdParent,
}

/// Whether the child's identity depends on the parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Identification {
    /// `--`, `to` — a solid line.
    #[default]
    Identifying,
    /// `..`, `.-`, `-.`, `optionally to` — a dashed line.
    NonIdentifying,
}

/// One row of an entity's attribute block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attribute {
    /// The type: `string`, `int`, `string[]`, `string(99)`, `string?`.
    pub kind: String,
    /// The attribute's name.
    pub name: String,
    /// `PK` / `FK` / `UK`, in the order written. Empty when there are none.
    pub keys: Vec<String>,
    /// The quoted comment. Empty when there is none.
    pub comment: String,
}

/// One entity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Entity {
    /// The name relationships refer to it by.
    pub id: String,
    /// The text drawn in the name band: the alias when `p["Person"]` gave one, else the id.
    pub label: String,
    /// The rows of the attribute block, in the order written.
    pub attributes: Vec<Attribute>,
    /// The subgraph that holds it, if any.
    pub parent: Option<String>,
    /// Names from `:::name` or a `class` statement. konoma does not colour entities; the names
    /// are carried so that it can.
    pub css_classes: Vec<String>,
}

/// One relationship, `CUSTOMER ||--o{ ORDER : places`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Relationship {
    /// Generated: `r<n>`.
    pub id: String,
    /// The entity (or subgraph) written on the left.
    pub from: String,
    /// The entity (or subgraph) written on the right.
    pub to: String,
    /// The cardinality written next to `from`.
    pub from_cardinality: Cardinality,
    /// The cardinality written next to `to`.
    pub to_cardinality: Cardinality,
    /// Solid or dashed.
    pub identification: Identification,
    /// The role, from the `: role` that every relationship must carry.
    pub label: Option<String>,
}

/// One `subgraph`, drawn as a frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubGraph {
    /// The id an edge may name as an endpoint.
    pub id: String,
    /// The text on the frame.
    pub title: String,
    /// Direct members by id, in declaration order — entities and nested subgraphs alike.
    pub members: Vec<String>,
    /// The subgraph that holds this one.
    pub parent: Option<String>,
}

/// A parsed `erDiagram`.
#[derive(Debug, Clone, Default)]
pub struct ErDiagram {
    /// Layout direction. `TB` unless a top-level `direction` said otherwise.
    pub direction: Direction,
    /// Every entity, in the order it first appeared.
    pub entities: Vec<Entity>,
    /// Every relationship, in source order.
    pub relationships: Vec<Relationship>,
    /// Every subgraph, outermost first.
    pub subgraphs: Vec<SubGraph>,
    /// `title:` from a YAML front matter block.
    pub title: Option<String>,
    /// `accTitle:` — not drawn.
    pub acc_title: Option<String>,
    /// `accDescr:` in either form — not drawn.
    pub acc_descr: Option<String>,
    /// id -> index into [`ErDiagram::entities`].
    index: HashMap<String, usize>,
}

impl ErDiagram {
    /// An empty diagram. The id index is private, so a struct literal cannot be written outside.
    pub(super) fn new(direction: Direction, title: Option<String>) -> ErDiagram {
        ErDiagram {
            direction,
            title,
            ..ErDiagram::default()
        }
    }

    /// Looks an entity up by id.
    pub fn entity(&self, id: &str) -> Option<&Entity> {
        self.index.get(id).and_then(|i| self.entities.get(*i))
    }

    /// The ids of every entity, in declaration order.
    pub fn entity_ids(&self) -> Vec<&str> {
        self.entities.iter().map(|e| e.id.as_str()).collect()
    }

    /// mermaid's `addEntity`: adds the entity if it is new, and fills in an alias that arrived
    /// later. Returns its index either way.
    pub(super) fn intern(&mut self, id: &str, alias: Option<&str>) -> usize {
        if let Some(i) = self.index.get(id) {
            let i = *i;
            if let Some(alias) = alias {
                if !alias.is_empty() && self.entities[i].label == self.entities[i].id {
                    self.entities[i].label = alias.to_string();
                }
            }
            return i;
        }
        let i = self.entities.len();
        self.entities.push(Entity {
            id: id.to_string(),
            label: alias.filter(|a| !a.is_empty()).unwrap_or(id).to_string(),
            ..Entity::default()
        });
        self.index.insert(id.to_string(), i);
        i
    }

    pub(super) fn entity_mut(&mut self, id: &str) -> Option<&mut Entity> {
        let i = *self.index.get(id)?;
        self.entities.get_mut(i)
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }
}

/// Why a source could not be read as an ER diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The source held no statements at all.
    Empty,
    /// The first keyword was not `erDiagram`.
    NotAnErDiagram {
        /// The keyword that was found, for the message.
        header: String,
    },
    /// The header was an ER diagram but the body declared no entity.
    NoEntities,
    /// An attribute block `{` was opened and never closed.
    UnclosedBlock {
        /// The entity the block belonged to.
        id: String,
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `subgraph` was opened and never closed with `end`.
    UnclosedSubgraph {
        /// The subgraph's id.
        id: String,
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `"` was opened and never closed.
    UnclosedString {
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `[` was opened and never closed. `SQS` with no `SQE` is a syntax error upstream.
    UnclosedBracket {
        /// The statement, for the message.
        text: String,
        /// 1-based line in the original source.
        line: usize,
    },
    /// A `[…]` was followed by something that cannot be part of a declaration.
    ///
    /// `SQS entityName SQE` appears in `erDiagram.jison` only in the entity declaration rules and
    /// in a `subgraph` header, so a bracket on a relationship — or a second bracket pair on the
    /// same line — is a line mermaid throws on. Refusing lets the fence fall back to the text
    /// diagram; the alternative was to draw an entity named after the rest of the statement.
    BracketIsNotADeclaration {
        /// The statement, for the message.
        text: String,
        /// 1-based line in the original source.
        line: usize,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty diagram"),
            ParseError::NotAnErDiagram { header } => {
                write!(f, "not an ER diagram: diagram starts with `{header}`")
            }
            ParseError::NoEntities => write!(f, "ER diagram declares no entities"),
            ParseError::UnclosedBlock { id, line } => {
                write!(f, "unclosed `{id} {{` opened at line {line}")
            }
            ParseError::UnclosedSubgraph { id, line } => {
                write!(f, "unclosed `subgraph {id}` opened at line {line}")
            }
            ParseError::UnclosedString { line } => write!(f, "unclosed `\"` at line {line}"),
            ParseError::UnclosedBracket { text, line } => {
                write!(f, "unclosed `[` at line {line}: `{text}`")
            }
            ParseError::BracketIsNotADeclaration { text, line } => write!(
                f,
                "line {line}: `{text}` — `[…]` names an entity's alias and only `:::classes` or \
                 an attribute block may follow it"
            ),
        }
    }
}

impl std::error::Error for ParseError {}
