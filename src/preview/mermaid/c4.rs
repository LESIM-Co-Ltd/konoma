//! Reading mermaid's C4 languages — `C4Context`, `C4Container`, `C4Component`, `C4Dynamic` and
//! `C4Deployment`.
//!
//! Read from `c4/parser/c4Diagram.jison` and `c4Db.ts` at tag `mermaid@11.17.2`.
//!
//! # Five keywords, one grammar
//!
//! `graphConfig` differs only in which word starts the diagram; every statement after it is the
//! same. So this module reads all five and records which one it was, and the renderer draws them
//! identically — which is also what upstream does.
//!
//! # An argument list is positional, and the positions differ by statement
//!
//! ```text
//! addPersonOrSystem(type, alias, label, descr, sprite, tags, link)
//! addContainer     (type, alias, label, techn, descr, sprite, tags, link)
//! addRel           (type, from,  to,    label, techn, descr, sprite, tags, link)
//! ```
//!
//! A `Container`'s third argument is its **technology** and a `Person`'s third is its
//! **description**. Getting that wrong puts the words in the wrong line of the box, which reads
//! as a plausible diagram of something else — so the two are separate fields here and separate
//! tests below.
//!
//! # `$key="value"` is an argument too
//!
//! `<attribute>[ ]*[\$]` opens a key-value argument, so `$tags="v1"` is a *named* argument sitting
//! among the positional ones. It does not consume a position, which is the part a positional
//! reader gets wrong: `Person(a, "A", $tags="x", "a description")` still has its description
//! third.
//!
//! # What is parsed and not drawn
//!
//! `UpdateElementStyle`, `UpdateRelStyle`, `UpdateLayoutConfig` — all three are appearance, and
//! §0-1 settles that konoma decides its own. `sprite` and `link` likewise: a terminal has no
//! icons and nothing to click.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, starts_ci, Envelope, ParseError, Preamble, Source, TitleSyntax,
};
use crate::preview::mermaid::flowchart::Direction;

/// The five keywords, longest first so `C4Container` never matches as `C4Cont…`.
pub const KEYWORDS: &[&str] = &[
    "C4Deployment",
    "C4Component",
    "C4Container",
    "C4Context",
    "C4Dynamic",
];

/// What kind of box an element is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// `Person` / `Person_Ext` — a stick figure over a box.
    Person,
    /// `System` / `Container` / `Component` and their `_Ext` forms — a plain box.
    Box,
    /// `SystemDb` / `ContainerDb` / `ComponentDb` — a cylinder.
    Database,
    /// `SystemQueue` / `ContainerQueue` / `ComponentQueue` — a stadium.
    Queue,
}

/// One element.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The alias a relationship names it by.
    pub alias: String,
    /// The name drawn in bold.
    pub label: String,
    /// The `[Person]` / `[Container: Go]` line under the name.
    pub kind_line: String,
    /// The description under that.
    pub descr: String,
    /// What it is drawn as.
    pub shape: ShapeKind,
    /// Whether it is one of the `_Ext` forms, which mermaid greys out.
    pub external: bool,
    /// Which boundary it sits in, if any.
    pub parent: Option<String>,
}

/// One boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct Boundary {
    /// The alias.
    pub alias: String,
    /// The name on the frame.
    pub label: String,
    /// The `[Boundary]` / `[Deployment Node]` line under it.
    pub kind_line: String,
    /// Which boundary it sits in, if any.
    pub parent: Option<String>,
    /// Its direct members, elements and nested boundaries alike, in source order.
    pub members: Vec<String>,
}

/// One relationship.
#[derive(Debug, Clone, PartialEq)]
pub struct Rel {
    /// Where it starts.
    pub from: String,
    /// Where it points.
    pub to: String,
    /// The words on the line.
    pub label: String,
    /// The `[HTTPS]` line under them.
    pub techn: String,
    /// Whether both ends carry an arrowhead (`BiRel`).
    pub bidirectional: bool,
}

/// A parsed C4 diagram.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct C4 {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// Which of the five keywords started it.
    pub kind: String,
    /// The axis the ranks run along.
    pub direction: Direction,
    /// The elements, in source order.
    pub elements: Vec<Element>,
    /// The boundaries, in source order.
    pub boundaries: Vec<Boundary>,
    /// The relationships, in source order.
    pub rels: Vec<Rel>,
}

/// Whether this source is one of the five C4 diagrams.
pub fn is_c4(src: &str) -> bool {
    find_header(src, KEYWORDS).is_some()
}

/// Parses a mermaid C4 diagram.
pub fn parse(src: &str) -> Result<C4, ParseError> {
    let Some(Source {
        lines,
        header_index,
        header_rest,
        front_matter_title,
    }) = find_header(src, KEYWORDS)
    else {
        return Err(ParseError::NotThisChart {
            expected: "C4 diagram",
            header: first_word(src),
        });
    };

    // Which keyword it was: `find_header` matched one of the five and left the rest of the line.
    let kind: String = lines[header_index]
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect();
    let mut c4 = C4 {
        kind,
        ..C4::default()
    };
    let mut env = Envelope::new(front_matter_title);
    let mut stack: Vec<String> = Vec::new();

    let mut all: Vec<(usize, &str)> = Vec::new();
    if !header_rest.is_empty() {
        all.push((header_index + 1, header_rest.as_str()));
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        all.push((i + 1, line.as_str()));
    }

    for (number, line) in all {
        if env.read(line, TitleSyntax::RawRestOfLine) {
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // `boundaryStart NEWLINE LBRACE` — the brace may sit on its own line, and the boundary
        // was already opened when its keyword was read.
        if t == "{" {
            continue;
        }
        if t == "}" {
            if stack.pop().is_none() {
                return Err(ParseError::Unexpected {
                    kind: "C4 diagram",
                    line: number,
                    text: t.to_string(),
                });
            }
            continue;
        }
        if let Some(d) = direction_of(t) {
            c4.direction = d;
            continue;
        }
        // `accDescription` has no start state; it is one lexer rule taking the rest of the line.
        if starts_ci(t, "accDescription") {
            continue;
        }
        read_statement(&mut c4, &mut stack, t, number)?;
    }

    c4.preamble = env.preamble;
    if c4.elements.is_empty() && c4.boundaries.is_empty() {
        return Err(ParseError::NoData {
            kind: "C4 diagram",
            wanted: "element",
        });
    }
    Ok(c4)
}

/// One row of the element table: the keyword, the shape, whether it is external, and how many
/// positional arguments come before the description.
struct ElementRule {
    word: &'static str,
    kind_line: &'static str,
    shape: ShapeKind,
    external: bool,
    /// Whether the third positional argument is a technology rather than the description.
    has_techn: bool,
}

/// Every element keyword, **longest first**: `Person_Ext` has to be tried before `Person`, and
/// `SystemQueue_Ext` before `SystemQueue` before `System`.
const ELEMENTS: &[ElementRule] = &[
    rule("Person_Ext", "Person", ShapeKind::Person, true, false),
    rule("Person", "Person", ShapeKind::Person, false, false),
    rule("SystemQueue_Ext", "System", ShapeKind::Queue, true, false),
    rule("SystemDb_Ext", "System", ShapeKind::Database, true, false),
    rule("System_Ext", "System", ShapeKind::Box, true, false),
    rule("SystemQueue", "System", ShapeKind::Queue, false, false),
    rule("SystemDb", "System", ShapeKind::Database, false, false),
    rule("System", "System", ShapeKind::Box, false, false),
    rule(
        "ContainerQueue_Ext",
        "Container",
        ShapeKind::Queue,
        true,
        true,
    ),
    rule(
        "ContainerDb_Ext",
        "Container",
        ShapeKind::Database,
        true,
        true,
    ),
    rule("Container_Ext", "Container", ShapeKind::Box, true, true),
    rule("ContainerQueue", "Container", ShapeKind::Queue, false, true),
    rule("ContainerDb", "Container", ShapeKind::Database, false, true),
    rule("Container", "Container", ShapeKind::Box, false, true),
    rule(
        "ComponentQueue_Ext",
        "Component",
        ShapeKind::Queue,
        true,
        true,
    ),
    rule(
        "ComponentDb_Ext",
        "Component",
        ShapeKind::Database,
        true,
        true,
    ),
    rule("Component_Ext", "Component", ShapeKind::Box, true, true),
    rule("ComponentQueue", "Component", ShapeKind::Queue, false, true),
    rule("ComponentDb", "Component", ShapeKind::Database, false, true),
    rule("Component", "Component", ShapeKind::Box, false, true),
];

const fn rule(
    word: &'static str,
    kind_line: &'static str,
    shape: ShapeKind,
    external: bool,
    has_techn: bool,
) -> ElementRule {
    ElementRule {
        word,
        kind_line,
        shape,
        external,
        has_techn,
    }
}

/// Every boundary keyword, longest first, with the `[…]` line each one defaults to.
const BOUNDARIES: &[(&str, &str)] = &[
    ("Enterprise_Boundary", "Enterprise"),
    ("System_Boundary", "System"),
    ("Container_Boundary", "Container"),
    ("Boundary", ""),
    ("Deployment_Node", "Deployment Node"),
    ("Node_L", "Deployment Node"),
    ("Node_R", "Deployment Node"),
    ("Node", "Deployment Node"),
];

/// Every relationship keyword, longest first, and whether it draws two arrowheads.
const RELS: &[(&str, bool)] = &[
    ("RelIndex", false),
    ("Rel_Up", false),
    ("Rel_Down", false),
    ("Rel_Left", false),
    ("Rel_Right", false),
    ("Rel_Back", false),
    ("Rel_U", false),
    ("Rel_D", false),
    ("Rel_L", false),
    ("Rel_R", false),
    ("Rel_B", false),
    ("BiRel", true),
    ("Rel", false),
];

fn read_statement(
    c4: &mut C4,
    stack: &mut Vec<String>,
    t: &str,
    number: usize,
) -> Result<(), ParseError> {
    // Appearance, read and dropped.
    for word in ["UpdateElementStyle", "UpdateRelStyle", "UpdateLayoutConfig"] {
        if call_of(t, word).is_some() {
            return Ok(());
        }
    }

    for r in ELEMENTS {
        let Some((args, tail)) = call_of(t, r.word) else {
            continue;
        };
        expect_nothing_after(tail, t, number)?;
        let alias = args.positional(0).unwrap_or_default();
        let label = args.positional(1).unwrap_or_default();
        let (techn, descr) = if r.has_techn {
            (
                args.positional(2).unwrap_or_default(),
                args.positional(3).unwrap_or_default(),
            )
        } else {
            (String::new(), args.positional(2).unwrap_or_default())
        };
        if alias.is_empty() {
            return Err(ParseError::Unexpected {
                kind: "C4 diagram",
                line: number,
                text: t.to_string(),
            });
        }
        let kind_line = if techn.is_empty() {
            r.kind_line.to_string()
        } else {
            format!("{}: {techn}", r.kind_line)
        };
        push_member(c4, stack, &alias);
        c4.elements.push(Element {
            alias,
            label,
            kind_line,
            descr,
            shape: r.shape,
            external: r.external,
            parent: stack.last().cloned(),
        });
        return Ok(());
    }

    for (word, default_line) in BOUNDARIES {
        let Some((args, tail)) = call_of(t, word) else {
            continue;
        };
        // A boundary opens a block, either on this line or on the next.
        let tail = tail.trim();
        if !tail.is_empty() && tail != "{" {
            return Err(ParseError::Unexpected {
                kind: "C4 diagram",
                line: number,
                text: t.to_string(),
            });
        }
        let alias = args.positional(0).unwrap_or_default();
        let label = args.positional(1).unwrap_or_default();
        let written = args.positional(2).unwrap_or_default();
        if alias.is_empty() {
            return Err(ParseError::Unexpected {
                kind: "C4 diagram",
                line: number,
                text: t.to_string(),
            });
        }
        let kind_line = if written.is_empty() {
            default_line.to_string()
        } else {
            written
        };
        push_member(c4, stack, &alias);
        c4.boundaries.push(Boundary {
            alias: alias.clone(),
            label,
            kind_line,
            parent: stack.last().cloned(),
            members: Vec::new(),
        });
        stack.push(alias);
        return Ok(());
    }

    for (word, bidirectional) in RELS {
        let Some((args, tail)) = call_of(t, word) else {
            continue;
        };
        expect_nothing_after(tail, t, number)?;
        // `RelIndex` puts a number first and `$2.splice(0, 1)` throws it away.
        let base = usize::from(*word == "RelIndex");
        let from = args.positional(base).unwrap_or_default();
        let to = args.positional(base + 1).unwrap_or_default();
        let label = args.positional(base + 2).unwrap_or_default();
        let techn = args.positional(base + 3).unwrap_or_default();
        if from.is_empty() || to.is_empty() {
            return Err(ParseError::Unexpected {
                kind: "C4 diagram",
                line: number,
                text: t.to_string(),
            });
        }
        c4.rels.push(Rel {
            from,
            to,
            label,
            techn,
            bidirectional: *bidirectional,
        });
        return Ok(());
    }

    Err(ParseError::Unexpected {
        kind: "C4 diagram",
        line: number,
        text: t.to_string(),
    })
}

/// Records `alias` as a member of whatever boundary is open.
fn push_member(c4: &mut C4, stack: &[String], alias: &str) {
    let Some(parent) = stack.last() else { return };
    if let Some(b) = c4.boundaries.iter_mut().find(|b| b.alias == *parent) {
        b.members.push(alias.to_string());
    }
}

fn expect_nothing_after(tail: &str, t: &str, number: usize) -> Result<(), ParseError> {
    if tail.trim().is_empty() {
        return Ok(());
    }
    Err(ParseError::Unexpected {
        kind: "C4 diagram",
        line: number,
        text: t.to_string(),
    })
}

/// The arguments of one `Keyword(…)` call.
struct Args {
    /// The positional arguments, in order. A `$key="value"` does not take a place.
    values: Vec<String>,
}

impl Args {
    fn positional(&self, i: usize) -> Option<String> {
        self.values.get(i).cloned().filter(|v| !v.is_empty())
    }
}

/// Splits `Keyword(a, "b", $k="v")` into its arguments and whatever follows the `)`.
fn call_of<'a>(t: &'a str, word: &str) -> Option<(Args, &'a str)> {
    if !t
        .get(..word.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(word))
    {
        return None;
    }
    let rest = t[word.len()..].trim_start();
    let inner = rest.strip_prefix('(')?;
    // The lexer pops both states on `)`, and a `)` inside a string is inside `<string>` where the
    // rule does not apply — so the closing bracket is the first one outside quotation marks.
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut end = None;
    for (i, c) in inner.char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"') => quote = Some('"'),
            (None, '(') => depth += 1,
            (None, ')') => {
                if depth == 0 {
                    end = Some(i);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
        let _ = i;
    }
    let end = end?;
    Some((read_args(&inner[..end]), &inner[end + 1..]))
}

/// The argument list itself.
fn read_args(body: &str) -> Args {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut quoted = false;
    let mut named = false;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_string => in_string = false,
            '"' if !in_string => {
                in_string = true;
                quoted = true;
            }
            ',' if !in_string => {
                push_arg(&mut values, &mut current, quoted, named);
                quoted = false;
                named = false;
            }
            '$' if !in_string && current.trim().is_empty() => {
                named = true;
                current.clear();
            }
            _ => current.push(c),
        }
        let _ = chars.peek();
    }
    push_arg(&mut values, &mut current, quoted, named);
    Args { values }
}

fn push_arg(values: &mut Vec<String>, current: &mut String, quoted: bool, named: bool) {
    let text = std::mem::take(current);
    if named {
        // `$tags="v1"` is a named argument and takes no position — the part a positional reader
        // gets wrong, which would shift every later argument by one.
        return;
    }
    // A quoted argument and a bare one are trimmed the same way: `<attribute>[ ]*["]` eats the
    // spaces before the quotation mark and `<attribute>[^,]+` keeps them, so trimming both leaves
    // one spelling of the same value.
    let _ = quoted;
    values.push(text.trim().to_string());
}

fn direction_of(t: &str) -> Option<Direction> {
    let at = t.find("direction")?;
    let rest = t[at + "direction".len()..].trim_start();
    let word = rest.split_whitespace().next()?;
    match word.to_ascii_uppercase().as_str() {
        "TB" => Some(Direction::TopToBottom),
        "BT" => Some(Direction::BottomToTop),
        "RL" => Some(Direction::RightToLeft),
        "LR" => Some(Direction::LeftToRight),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> C4 {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_context_example_reads_as_people_systems_and_relationships() {
        let c = ok("C4Context\n  title System Context diagram for Internet Banking System\n  Enterprise_Boundary(b0, \"BankBoundary0\") {\n    Person(customerA, \"Banking Customer A\", \"A customer of the bank, with personal bank accounts.\")\n    System(SystemAA, \"Internet Banking System\", \"Allows customers to view information.\")\n  }\n  System_Ext(SystemC, \"E-mail system\", \"The internal Microsoft Exchange e-mail system.\")\n  Rel(customerA, SystemAA, \"Uses\")\n  Rel(SystemAA, SystemC, \"Sends e-mails\", \"SMTP\")\n");
        assert_eq!(c.kind, "C4Context");
        assert_eq!(
            c.preamble.title.as_deref(),
            Some("System Context diagram for Internet Banking System")
        );
        assert_eq!(c.boundaries.len(), 1);
        assert_eq!(c.boundaries[0].members, ["customerA", "SystemAA"]);
        assert_eq!(c.elements[0].alias, "customerA");
        assert_eq!(c.elements[0].shape, ShapeKind::Person);
        assert_eq!(c.elements[0].kind_line, "Person");
        assert_eq!(c.elements[0].parent.as_deref(), Some("b0"));
        assert!(c.elements[2].external);
        assert_eq!(c.elements[2].parent, None);
        assert_eq!(c.rels[1].techn, "SMTP");
    }

    /// A `Container`'s third argument is its technology; a `Person`'s is its description. Putting
    /// them in the same slot draws a plausible diagram of something else.
    #[test]
    fn a_containers_third_argument_is_its_technology_and_a_persons_is_its_description() {
        let c = ok("C4Container\n  Person(a, \"A\", \"the description\")\n  Container(b, \"B\", \"Go\", \"the description\")\n");
        assert_eq!(c.elements[0].descr, "the description");
        assert_eq!(c.elements[0].kind_line, "Person");
        assert_eq!(c.elements[1].kind_line, "Container: Go");
        assert_eq!(c.elements[1].descr, "the description");
    }

    #[test]
    fn every_documented_shape_keyword_is_read_as_its_own_shape() {
        for (word, shape, external) in [
            ("Person", ShapeKind::Person, false),
            ("Person_Ext", ShapeKind::Person, true),
            ("System", ShapeKind::Box, false),
            ("SystemDb", ShapeKind::Database, false),
            ("SystemQueue", ShapeKind::Queue, false),
            ("System_Ext", ShapeKind::Box, true),
            ("SystemDb_Ext", ShapeKind::Database, true),
            ("SystemQueue_Ext", ShapeKind::Queue, true),
            ("Container", ShapeKind::Box, false),
            ("ContainerDb", ShapeKind::Database, false),
            ("ContainerQueue", ShapeKind::Queue, false),
            ("Container_Ext", ShapeKind::Box, true),
            ("Component", ShapeKind::Box, false),
            ("ComponentDb", ShapeKind::Database, false),
            ("ComponentQueue", ShapeKind::Queue, false),
            ("Component_Ext", ShapeKind::Box, true),
        ] {
            let c = ok(&format!("C4Container\n  {word}(a, \"A\")\n"));
            assert_eq!(c.elements[0].shape, shape, "{word}");
            assert_eq!(c.elements[0].external, external, "{word}");
        }
    }

    /// `$key="value"` does not take a positional slot.
    #[test]
    fn a_named_argument_does_not_shift_the_positional_ones() {
        let c = ok("C4Context\n  Person(a, \"A\", $tags=\"v1\", \"the description\")\n");
        assert_eq!(c.elements[0].descr, "the description");
    }

    #[test]
    fn nested_boundaries_record_their_members_and_their_parents() {
        let c = ok("C4Deployment\n  Deployment_Node(outer, \"Outer\", \"DC\") {\n    Deployment_Node(inner, \"Inner\") {\n      Container(app, \"App\", \"Go\")\n    }\n  }\n");
        assert_eq!(c.boundaries[0].alias, "outer");
        assert_eq!(c.boundaries[0].kind_line, "DC");
        assert_eq!(c.boundaries[0].members, ["inner"]);
        assert_eq!(c.boundaries[1].parent.as_deref(), Some("outer"));
        assert_eq!(c.boundaries[1].kind_line, "Deployment Node");
        assert_eq!(c.elements[0].parent.as_deref(), Some("inner"));
    }

    #[test]
    fn a_boundary_may_open_its_block_on_the_next_line() {
        let c = ok("C4Context\n  System_Boundary(b, \"B\")\n  {\n    System(s, \"S\")\n  }\n");
        assert_eq!(c.boundaries[0].members, ["s"]);
    }

    #[test]
    fn every_documented_relationship_keyword_is_read() {
        for word in [
            "Rel",
            "BiRel",
            "Rel_U",
            "Rel_Up",
            "Rel_D",
            "Rel_Down",
            "Rel_L",
            "Rel_Left",
            "Rel_R",
            "Rel_Right",
            "Rel_Back",
            "Rel_B",
        ] {
            let c = ok(&format!(
                "C4Context\n  System(a, \"A\")\n  System(b, \"B\")\n  {word}(a, b, \"x\")\n"
            ));
            assert_eq!(c.rels.len(), 1, "{word}");
            assert_eq!(c.rels[0].from, "a", "{word}");
            assert_eq!(c.rels[0].bidirectional, word == "BiRel", "{word}");
        }
        // `RelIndex` throws its first argument away.
        let c = ok(
            "C4Dynamic\n  System(a, \"A\")\n  System(b, \"B\")\n  RelIndex(\"1\", a, b, \"x\")\n",
        );
        assert_eq!(c.rels[0].from, "a");
        assert_eq!(c.rels[0].label, "x");
    }

    #[test]
    fn the_appearance_statements_are_read_and_do_not_become_elements() {
        let c = ok("C4Context\n  System(a, \"A\")\n  UpdateElementStyle(a, $fontColor=\"red\")\n  UpdateRelStyle(a, a, $offsetY=\"10\")\n  UpdateLayoutConfig($c4ShapeInRow=\"3\")\n");
        assert_eq!(c.elements.len(), 1);
        assert!(c.rels.is_empty());
    }

    #[test]
    fn direction_is_read() {
        assert_eq!(
            ok("C4Context\n  direction LR\n  System(a, \"A\")\n").direction,
            Direction::LeftToRight
        );
    }

    #[test]
    fn a_line_the_grammar_has_no_rule_for_is_refused_rather_than_becoming_a_phantom_box() {
        assert!(matches!(
            parse("C4Context\n  System(a, \"A\")\n  Sytem(b, \"B\")\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    #[test]
    fn a_header_with_nothing_in_it_is_refused() {
        assert!(matches!(
            parse("C4Context\n"),
            Err(ParseError::NoData { .. })
        ));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "C4Context\n  System(a, \"A\")",
            "C4Container\n  Container(a, \"A\")",
            "C4Component\n  Component(a, \"A\")",
            "C4Dynamic\n  System(a, \"A\")",
            "C4Deployment\n  Node(a, \"A\") {\n  }",
            "C4Context",
            "C4Contexts\n  a",
            "gantt\n  title G",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_c4(src), !refused, "{src:?}");
        }
    }
}
