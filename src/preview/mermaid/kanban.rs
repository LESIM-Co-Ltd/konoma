//! Reading mermaid's `kanban` language — columns of cards.
//!
//! Read from `kanban/parser/kanban.jison` and `kanbanDb.ts` at tag `mermaid@11.17.2`.
//!
//! # It is `mindmap`'s lexer with one addition
//!
//! The two `.jison` files are the same file with three differences: the keyword, `NODE_ID` also
//! stops at `@`, and a node may be followed by `@{ … }` of metadata. So the tree is read by
//! [`mindmap::read_tree`] and only the metadata is new.
//!
//! [`mindmap::read_tree`]: super::mindmap::read_tree
//!
//! # The defect this is measured against
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate konoma is replacing parsing
//! `[Create Documentation]` as a card called `[Create`. That is the `NODE` lexer state being
//! skipped: `[` opens a description that runs to `]`, spaces and all. The test
//! [`tests::a_bracketed_card_keeps_all_of_its_words`] is that defect, pinned.
//!
//! # Sections are decided by the *first* node's indentation
//!
//! `kanbanDb.getSection` takes `nodes[0].level` as the section level: every node at that level is
//! a column and everything deeper is a card in the last column. A node **shallower** than the
//! first one is an error upstream ("Items without section detected"), not a new column.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{find_header, first_word, ParseError};
use crate::preview::mermaid::mindmap::{self, Dialect, Mindmap};

/// The keyword.
pub const KEYWORD: &str = "kanban";

/// One card.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Card {
    /// The id, written or generated.
    pub id: String,
    /// The words on the card. `@{ label: … }` overrides what was written in the brackets.
    pub label: String,
    /// `@{ assigned: … }`.
    pub assigned: Option<String>,
    /// `@{ ticket: … }`.
    pub ticket: Option<String>,
    /// `@{ priority: … }` — one of `Very High`, `High`, `Low`, `Very Low`, as written.
    pub priority: Option<String>,
    /// `@{ icon: … }`, read and not drawn.
    pub icon: Option<String>,
}

/// One column.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Column {
    /// The id, written or generated.
    pub id: String,
    /// The words on the column header.
    pub label: String,
    /// `@{ ticket: … }` on the column itself.
    pub ticket: Option<String>,
    /// The cards in it, in source order.
    pub cards: Vec<Card>,
}

/// A parsed kanban board.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kanban {
    /// The columns, left to right in source order.
    pub columns: Vec<Column>,
}

/// `kanban`'s answers to the two questions `mindmap` answers differently.
const DIALECT: Dialect = Dialect {
    kind: KEYWORD,
    single_root: false,
    at_ends_the_id: true,
};

/// Whether this source is a kanban board.
pub fn is_kanban(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid kanban board.
pub fn parse(src: &str) -> Result<Kanban, ParseError> {
    let Some(source) = find_header(src, &[KEYWORD]) else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };

    // The metadata is collected beside the tree: `read_tree` builds the same `Mindmap` a mindmap
    // does, and the `extra` hook is offered whatever followed the node on its line.
    let mut meta: Vec<(String, Meta)> = Vec::new();
    let mut tree = Mindmap::default();
    let mut failure: Option<ParseError> = None;
    mindmap::read_tree(&mut tree, &source, &DIALECT, |node, tail| {
        let Some(body) = shape_data(tail) else {
            return false;
        };
        match read_meta(body) {
            Ok(m) => {
                if let Some(label) = &m.label {
                    node.label = label.clone();
                }
                meta.push((node.id.clone(), m));
                true
            }
            Err(e) => {
                // Consumed — the failure is reported below rather than as "unexpected text",
                // which would name the whole `@{ … }` instead of the field that is wrong.
                failure = Some(e);
                true
            }
        }
    })?;
    if let Some(e) = failure {
        return Err(e);
    }
    if tree.nodes.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "column",
        });
    }

    // **Zero, by construction.** `kanbanDb.getSection` reads `nodes[0].level`, and `read_tree`
    // has already subtracted the first node's own indentation from every level — so the first
    // node is at 0 whatever the board was indented by. Writing `tree.nodes[0].level` here would
    // read as a rule that could give another answer, and it cannot.
    let section_level = 0;
    let mut board = Kanban::default();
    for (i, node) in tree.nodes.iter().enumerate() {
        let m = meta
            .iter()
            .find(|(id, _)| *id == node.id)
            .map(|(_, m)| m.clone())
            .unwrap_or_default();
        if node.level < section_level {
            // Upstream: "Items without section detected".
            return Err(ParseError::Invalid {
                line: 0,
                message: format!("kanban card {:?} sits outside every column", node.label),
            });
        }
        if node.level == section_level {
            board.columns.push(Column {
                id: id_of(node, i),
                label: node.label.clone(),
                ticket: m.ticket,
                cards: Vec::new(),
            });
            continue;
        }
        let Some(column) = board.columns.last_mut() else {
            return Err(ParseError::Invalid {
                line: 0,
                message: format!("kanban card {:?} sits outside every column", node.label),
            });
        };
        column.cards.push(Card {
            id: id_of(node, i),
            label: node.label.clone(),
            assigned: m.assigned,
            ticket: m.ticket,
            priority: m.priority,
            icon: m.icon.or_else(|| node.icon.clone()),
        });
    }
    Ok(board)
}

/// `sanitizeText(id, conf) || 'kbn' + cnt++` — a node written with no id of its own gets one.
fn id_of(node: &mindmap::Node, i: usize) -> String {
    if node.id.trim().is_empty() {
        format!("kbn{i}")
    } else {
        node.id.clone()
    }
}

/// The body of `@{ … }`, or `None` when the tail is not shape data.
fn shape_data(tail: &str) -> Option<&str> {
    let rest = tail.trim().strip_prefix("@{")?;
    Some(rest.strip_suffix('}').unwrap_or(rest))
}

/// What `@{ … }` can say.
#[derive(Debug, Clone, Default, PartialEq)]
struct Meta {
    label: Option<String>,
    assigned: Option<String>,
    ticket: Option<String>,
    priority: Option<String>,
    icon: Option<String>,
    shape: Option<String>,
}

/// Reads `@{ … }`.
///
/// Upstream wraps the text in braces and hands it to `js-yaml` with the JSON schema, which for the
/// one-line form people actually write is `key: value` pairs separated by commas. konoma reads
/// that form and **refuses anything else**, rather than half-reading a YAML document: a card whose
/// metadata was silently dropped looks exactly like a card that never had any.
fn read_meta(body: &str) -> Result<Meta, ParseError> {
    let mut meta = Meta::default();
    for field in split_fields(body) {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let Some((key, value)) = field.split_once(':') else {
            return Err(ParseError::Invalid {
                line: 0,
                message: format!("kanban metadata `{field}` is not `key: value`"),
            });
        };
        let value = unquote(value.trim());
        match key.trim().to_ascii_lowercase().as_str() {
            "label" => meta.label = Some(value),
            "assigned" => meta.assigned = Some(value),
            "ticket" => meta.ticket = Some(value),
            "priority" => meta.priority = Some(value),
            "icon" => meta.icon = Some(value),
            // Upstream raises for a shape name that is not lowercase, and only honours
            // `kanbanItem`, which is the shape a card already has.
            "shape" => meta.shape = Some(value),
            other => {
                return Err(ParseError::Invalid {
                    line: 0,
                    message: format!("kanban metadata has no `{other}` field"),
                })
            }
        }
    }
    Ok(meta)
}

/// Splits on commas that are not inside quotation marks.
fn split_fields(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in body.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => {
                quote = None;
                current.push(c);
            }
            (Some(_), _) => current.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(c);
                current.push(c);
            }
            (None, ',') => out.push(std::mem::take(&mut current)),
            (None, '\n') => current.push(' '),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Kanban {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    /// **The crate's defect, pinned.** `[Create Documentation]` is one card, not `[Create`.
    #[test]
    fn a_bracketed_card_keeps_all_of_its_words() {
        let b = ok("kanban\n  Todo\n    [Create Documentation]\n");
        assert_eq!(b.columns[0].cards[0].label, "Create Documentation");
    }

    #[test]
    fn the_documented_example_reads_as_columns_of_cards() {
        let b = ok(
            "kanban\n  Todo\n    [Create Documentation]\n    docs[Create Blog about the new diagram]\n  \
             id7[In progress]\n    id6[Create renderer so that it works in all cases]\n",
        );
        assert_eq!(b.columns.len(), 2);
        assert_eq!(b.columns[0].label, "Todo");
        assert_eq!(b.columns[0].cards.len(), 2);
        assert_eq!(b.columns[1].label, "In progress");
        assert_eq!(b.columns[1].id, "id7");
        assert_eq!(b.columns[1].cards[0].id, "id6");
    }

    #[test]
    fn metadata_is_read_off_the_card_it_follows() {
        let b = ok(
            "kanban\n  Backlog\n    id1[Ticket]@{ assigned: 'knsv', ticket: MC-2038, priority: 'Very High' }\n",
        );
        let card = &b.columns[0].cards[0];
        assert_eq!(card.assigned.as_deref(), Some("knsv"));
        assert_eq!(card.ticket.as_deref(), Some("MC-2038"));
        assert_eq!(card.priority.as_deref(), Some("Very High"));
    }

    #[test]
    fn metadata_label_overrides_the_bracketed_words() {
        let b = ok("kanban\n  Todo\n    id1@{ label: 'From the metadata' }\n");
        assert_eq!(b.columns[0].cards[0].label, "From the metadata");
    }

    #[test]
    fn an_unknown_metadata_field_is_refused_rather_than_dropped() {
        assert!(matches!(
            parse("kanban\n  Todo\n    a@{ colour: red }\n"),
            Err(ParseError::Invalid { .. })
        ));
    }

    #[test]
    fn a_card_shallower_than_the_first_node_is_refused() {
        assert!(matches!(
            parse("kanban\n    Todo\n  Loose\n"),
            Err(ParseError::Invalid { .. })
        ));
    }

    /// A card written `[]` has no words, so `nodeWithoutId` gives it no name either — and
    /// `kanbanDb` falls back to `kbn` plus a counter. **Two of them have to get two ids**: the
    /// counter is the whole of what tells them apart, and a mutation that dropped it survived a
    /// whole campaign because no case had a second nameless card.
    #[test]
    fn two_nameless_cards_are_given_two_different_ids() {
        let b = ok("kanban\n  Todo\n    []\n    []\n");
        assert_eq!(b.columns[0].cards.len(), 2);
        assert_ne!(
            b.columns[0].cards[0].id, b.columns[0].cards[1].id,
            "two cards with no name of their own were given one id between them"
        );
        assert!(b.columns[0].cards[0].id.starts_with("kbn"));
    }

    #[test]
    fn a_column_with_no_cards_is_still_a_column() {
        let b = ok("kanban\n  Todo\n  Doing\n    a\n");
        assert_eq!(b.columns.len(), 2);
        assert!(b.columns[0].cards.is_empty());
    }

    #[test]
    fn a_header_with_no_columns_is_refused() {
        assert!(matches!(parse("kanban\n"), Err(ParseError::NoData { .. })));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "kanban\n  Todo\n    a",
            "KANBAN\n  Todo\n    a",
            "kanban",
            "kanbans\n  a",
            "mindmap\n  a",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_kanban(src), !refused, "{src:?}");
        }
    }
}
