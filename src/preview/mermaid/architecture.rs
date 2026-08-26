//! Reading mermaid's `architecture-beta` language — services, groups, junctions and ported edges.
//!
//! Read from `packages/parser/src/language/architecture/{architecture,arch}.langium` at tag
//! `mermaid@11.17.2`.
//!
//! # An edge names a **side** at each end, and that is the layout
//!
//! ```text
//! Arrow: ':'lhsDir ARROW_INTO? ('--' | '-' title '-') ARROW_INTO? rhsDir':'
//! ```
//!
//! `db:R --> L:server` says "leave `db` by its right side and arrive at `server`'s left" — which
//! is a statement about *where the two boxes are*, not only about how the line is drawn. It is
//! the whole reason this language exists rather than being a flowchart, and it is why konoma's
//! renderer places these on a grid instead of handing them to dagre: a layered layout has no way
//! to honour "B is to the right of A".
//!
//! # `L R T B` and nothing else
//!
//! `terminal ARROW_DIRECTION: 'L' | 'R' | 'T' | 'B'`. Not `left`, not lower case — the terminal is
//! four literal capitals, and `db:right -- L:x` is a syntax error upstream.
//!
//! # What is parsed and not drawn
//!
//! `(icon)` and `("emoji")` — konoma ships no icon font, and §2-5 already puts `icon` shapes out
//! of scope. `{group}` on an edge end, which upstream uses to attach the line to a group's border
//! rather than to a service's: the line still joins the same two things, and konoma draws it to
//! the service. Both are read, because an unread token becomes part of the id.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, Envelope, ParseError, Preamble, Source, TitleSyntax,
};

/// The keyword.
pub const KEYWORD: &str = "architecture-beta";

/// Which side of a box an edge leaves or arrives at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// `L`
    Left,
    /// `R`
    Right,
    /// `T`
    Top,
    /// `B`
    Bottom,
}

impl Side {
    /// The step this side takes on the grid: right is `(1, 0)`, top is `(0, -1)`.
    pub fn step(self) -> (i32, i32) {
        match self {
            Side::Left => (-1, 0),
            Side::Right => (1, 0),
            Side::Top => (0, -1),
            Side::Bottom => (0, 1),
        }
    }
}

/// One service or junction.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    /// The id an edge names it by.
    pub id: String,
    /// The words in `[…]`. Empty for a junction and for a service written without one.
    pub title: String,
    /// The `(icon)` or `("emoji")`, read and not drawn.
    pub icon: Option<String>,
    /// Which group it is `in`, if any.
    pub group: Option<String>,
    /// Whether it is a `junction` — a corner with no box and no words.
    pub junction: bool,
}

/// One group.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The id.
    pub id: String,
    /// The words on the frame.
    pub title: String,
    /// The `(icon)`, read and not drawn.
    pub icon: Option<String>,
    /// Which group it is `in`, if any.
    pub parent: Option<String>,
}

/// One edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// The id at the left of the statement.
    pub from: String,
    /// Which side of it the line leaves by.
    pub from_side: Side,
    /// Whether that end carries an arrowhead.
    pub from_arrow: bool,
    /// The id at the right.
    pub to: String,
    /// Which side of it the line arrives at.
    pub to_side: Side,
    /// Whether that end carries an arrowhead.
    pub to_arrow: bool,
    /// The words in `-[…]-`, if any.
    pub title: String,
}

/// One `align`.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    /// `row` or `column`.
    pub row: bool,
    /// The ids to line up.
    pub members: Vec<String>,
}

/// A parsed architecture diagram.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Architecture {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// The groups, in declaration order.
    pub groups: Vec<Group>,
    /// The services and junctions, in declaration order.
    pub services: Vec<Service>,
    /// The edges, in source order.
    pub edges: Vec<Edge>,
    /// The alignments, in source order.
    pub alignments: Vec<Alignment>,
}

/// Whether this source is an architecture diagram.
pub fn is_architecture(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid architecture diagram.
pub fn parse(src: &str) -> Result<Architecture, ParseError> {
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

    let mut arch = Architecture::default();
    let mut env = Envelope::new(front_matter_title);

    let mut all: Vec<(usize, &str)> = Vec::new();
    if !header_rest.is_empty() {
        all.push((header_index + 1, header_rest.as_str()));
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        all.push((i + 1, line.as_str()));
    }

    for (number, line) in all {
        if env.read(line, TitleSyntax::Terminal) {
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = word_rest(t, "group") {
            let (id, icon, title, parent) = read_declaration(rest, number)?;
            arch.groups.push(Group {
                id,
                title,
                icon,
                parent,
            });
            continue;
        }
        if let Some(rest) = word_rest(t, "service") {
            let (id, icon, title, group) = read_declaration(rest, number)?;
            arch.services.push(Service {
                id,
                title,
                icon,
                group,
                junction: false,
            });
            continue;
        }
        if let Some(rest) = word_rest(t, "junction") {
            let (id, icon, title, group) = read_declaration(rest, number)?;
            if icon.is_some() || !title.is_empty() {
                // `Junction: 'junction' id=ID ('in' in=ID)? EOL` — no icon, no title.
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: t.to_string(),
                });
            }
            arch.services.push(Service {
                id,
                title,
                icon,
                group,
                junction: true,
            });
            continue;
        }
        if let Some(rest) = word_rest(t, "align") {
            let mut words = rest.split_whitespace();
            let row = match words.next() {
                Some(w) if w.eq_ignore_ascii_case("row") => true,
                Some(w) if w.eq_ignore_ascii_case("column") => false,
                _ => {
                    return Err(ParseError::Unexpected {
                        kind: KEYWORD,
                        line: number,
                        text: t.to_string(),
                    })
                }
            };
            let members: Vec<String> = words.map(str::to_string).collect();
            // `members+=ID (members+=ID)+` — two is the minimum.
            if members.len() < 2 {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: t.to_string(),
                });
            }
            arch.alignments.push(Alignment { row, members });
            continue;
        }
        arch.edges.push(read_edge(t, number)?);
    }

    arch.preamble = env.preamble;
    if arch.services.is_empty() && arch.groups.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "service",
        });
    }
    Ok(arch)
}

/// `id (icon)? [title]? (in parent)?` — the tail shared by `group`, `service` and `junction`.
fn read_declaration(
    rest: &str,
    number: usize,
) -> Result<(String, Option<String>, String, Option<String>), ParseError> {
    let mut s = rest.trim();
    let id_end = s
        .find(|c: char| c == '(' || c == '[' || c.is_whitespace())
        .unwrap_or(s.len());
    let id = s[..id_end].to_string();
    if id.is_empty() {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: rest.trim().to_string(),
        });
    }
    s = s[id_end..].trim_start();

    let mut icon = None;
    if let Some(after) = s.strip_prefix('(') {
        let Some(end) = after.find(')') else {
            return Err(ParseError::Invalid {
                line: number,
                message: "unclosed `(`".to_string(),
            });
        };
        icon = Some(after[..end].to_string());
        s = after[end + 1..].trim_start();
    }

    let mut title = String::new();
    if let Some(after) = s.strip_prefix('[') {
        let Some(end) = after.rfind(']') else {
            return Err(ParseError::Invalid {
                line: number,
                message: "unclosed `[`".to_string(),
            });
        };
        title = unquote(&after[..end]);
        s = after[end + 1..].trim_start();
    }

    let mut group = None;
    if let Some(after) = word_rest(s, "in") {
        let name = after.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: rest.trim().to_string(),
            });
        }
        group = Some(name);
        s = after[after.find(char::is_whitespace).unwrap_or(after.len())..].trim_start();
    }
    if !s.is_empty() {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: s.to_string(),
        });
    }
    Ok((id, icon, title, group))
}

/// `lhsId{group}?:L <-- R:rhsId{group}?`, with an optional `-[title]-` in the middle.
fn read_edge(t: &str, number: usize) -> Result<Edge, ParseError> {
    let unexpected = || ParseError::Unexpected {
        kind: KEYWORD,
        line: number,
        text: t.to_string(),
    };
    let (left, rest) = t.split_once(':').ok_or_else(unexpected)?;
    let from = strip_group(left.trim()).to_string();
    if from.is_empty() {
        return Err(unexpected());
    }
    let rest = rest.trim_start();
    let (from_side, rest) = read_side(rest).ok_or_else(unexpected)?;
    let rest = rest.trim_start();

    let (from_arrow, rest) = match rest.strip_prefix('<') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    // `'--'` or `'-' title '-'`; the title is an `ARCH_TITLE`, brackets and all.
    let rest = rest.strip_prefix('-').ok_or_else(unexpected)?;
    let (title, rest) = if let Some(after) = rest.strip_prefix('[') {
        let end = after.find(']').ok_or_else(unexpected)?;
        let title = unquote(&after[..end]);
        (
            title,
            after[end + 1..].strip_prefix('-').ok_or_else(unexpected)?,
        )
    } else {
        (
            String::new(),
            rest.strip_prefix('-').ok_or_else(unexpected)?,
        )
    };
    let (to_arrow, rest) = match rest.strip_prefix('>') {
        Some(r) => (true, r),
        None => (false, rest),
    };

    let rest = rest.trim_start();
    let (to_side, rest) = read_side(rest).ok_or_else(unexpected)?;
    let rest = rest.strip_prefix(':').ok_or_else(unexpected)?;
    let to = strip_group(rest.trim()).to_string();
    if to.is_empty() {
        return Err(unexpected());
    }
    Ok(Edge {
        from,
        from_side,
        from_arrow,
        to,
        to_side,
        to_arrow,
        title,
    })
}

/// The four capitals, and nothing else.
fn read_side(s: &str) -> Option<(Side, &str)> {
    let side = match s.chars().next()? {
        'L' => Side::Left,
        'R' => Side::Right,
        'T' => Side::Top,
        'B' => Side::Bottom,
        _ => return None,
    };
    Some((side, &s[1..]))
}

/// `{group}` on an edge end. Read and dropped — see the module docs.
fn strip_group(s: &str) -> &str {
    s.strip_suffix("{group}")
        .unwrap_or_else(|| s.strip_prefix("{group}").unwrap_or(s))
        .trim()
}

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

    fn ok(src: &str) -> Architecture {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_example_reads_as_a_group_four_services_and_three_edges() {
        let a = ok("architecture-beta\n    group api(cloud)[API]\n\n    service db(database)[Database] in api\n    service disk1(disk)[Storage] in api\n    service disk2(disk)[Storage] in api\n    service server(server)[Server] in api\n\n    db:L -- R:server\n    disk1:T -- B:server\n    disk2:T -- B:db\n");
        assert_eq!(a.groups[0].id, "api");
        assert_eq!(a.groups[0].title, "API");
        assert_eq!(a.groups[0].icon.as_deref(), Some("cloud"));
        assert_eq!(a.services.len(), 4);
        assert_eq!(a.services[0].group.as_deref(), Some("api"));
        assert_eq!(a.edges.len(), 3);
        assert_eq!(a.edges[0].from, "db");
        assert_eq!(a.edges[0].from_side, Side::Left);
        assert_eq!(a.edges[0].to_side, Side::Right);
        assert!(!a.edges[0].from_arrow && !a.edges[0].to_arrow);
    }

    #[test]
    fn an_arrowhead_on_either_end_is_read() {
        let a = ok("architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  a:R --> L:b\n  a:B <-- T:b\n  a:T <--> B:b\n");
        assert!(!a.edges[0].from_arrow && a.edges[0].to_arrow);
        assert!(a.edges[1].from_arrow && !a.edges[1].to_arrow);
        assert!(a.edges[2].from_arrow && a.edges[2].to_arrow);
    }

    #[test]
    fn an_edge_may_carry_a_title() {
        let a =
            ok("architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  a:R -[reads]- L:b\n");
        assert_eq!(a.edges[0].title, "reads");
    }

    #[test]
    fn a_junction_has_no_title_and_no_icon() {
        let a = ok("architecture-beta\n  service a(x)[A]\n  junction j\n  a:R -- L:j\n");
        assert!(a.services[1].junction);
        assert_eq!(a.services[1].title, "");
        assert!(matches!(
            parse("architecture-beta\n  junction j[Nope]\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    #[test]
    fn align_needs_a_direction_and_at_least_two_members() {
        let a = ok("architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  align column a b\n");
        assert!(!a.alignments[0].row);
        assert_eq!(a.alignments[0].members, ["a", "b"]);
        assert!(matches!(
            parse("architecture-beta\n  service a(x)[A]\n  align column a\n"),
            Err(ParseError::Unexpected { .. })
        ));
        assert!(matches!(
            parse("architecture-beta\n  service a(x)[A]\n  align sideways a b\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    /// `ARROW_DIRECTION` is four literal capitals.
    #[test]
    fn a_lower_case_or_spelled_out_side_is_refused() {
        assert!(matches!(
            parse("architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  a:r -- l:b\n"),
            Err(ParseError::Unexpected { .. })
        ));
        assert!(matches!(
            parse("architecture-beta\n  service a(x)[A]\n  service b(x)[B]\n  a:right -- left:b\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    #[test]
    fn a_group_edge_end_is_read_and_names_the_same_service() {
        let a = ok("architecture-beta\n  group g(cloud)[G]\n  service a(x)[A] in g\n  service b(x)[B]\n  a{group}:R --> L:b\n");
        assert_eq!(a.edges[0].from, "a");
    }

    #[test]
    fn a_nested_group_records_its_parent() {
        let a = ok("architecture-beta\n  group outer(cloud)[Outer]\n  group inner(cloud)[Inner] in outer\n  service s(x)[S] in inner\n");
        assert_eq!(a.groups[1].parent.as_deref(), Some("outer"));
    }

    #[test]
    fn a_service_may_carry_a_quoted_icon_instead_of_a_named_one() {
        let a = ok("architecture-beta\n  service a(\"🚀\")[Rocket]\n");
        assert_eq!(a.services[0].icon.as_deref(), Some("\"🚀\""));
        assert_eq!(a.services[0].title, "Rocket");
    }

    #[test]
    fn a_header_with_nothing_in_it_is_refused() {
        assert!(matches!(
            parse("architecture-beta\n"),
            Err(ParseError::NoData { .. })
        ));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "architecture-beta\n  service a(x)[A]",
            "ARCHITECTURE-BETA\n  service a(x)[A]",
            "architecture-beta",
            "architecture\n  service a",
            "gantt\n  title G",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_architecture(src), !refused, "{src:?}");
        }
    }
}
