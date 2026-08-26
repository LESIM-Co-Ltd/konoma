//! Reading mermaid's `requirementDiagram` language — requirements, elements and the links
//! between them.
//!
//! Read from `requirement/parser/requirementDiagram.jison` and `requirementDb.ts` at tag
//! `mermaid@11.17.2`.
//!
//! # A relationship's arrow says which end is the *source*
//!
//! ```text
//! relationshipDef
//!   : id END_ARROW_L relationship LINE id   { yy.addRelationship($3, $5, $1) }
//!   | id LINE relationship END_ARROW_R id   { yy.addRelationship($3, $1, $5) }
//! ```
//!
//! `A - satisfies -> B` is A satisfying B; `A <- satisfies - B` is **B** satisfying A. The two
//! spellings put the source at opposite ends, which is the one thing about this language a reader
//! written from the examples gets backwards — the examples only ever show the first form.
//!
//! # The body is `key: value`, and an unknown key is a syntax error
//!
//! `requirementBody` lists exactly four keys and `elementBody` exactly two. Upstream's own
//! documentation warns that "the parser will fail if another keyword is detected", so konoma
//! refuses an unknown key rather than dropping it: a requirement missing its risk looks exactly
//! like a requirement that never declared one.
//!
//! # What is parsed and not drawn
//!
//! `style`, `classDef`, `class` and `:::` — colour rules, and §4-4 records `classDef`'s `color:`
//! having no effect in konoma's label model anyway.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, Envelope, ParseError, Preamble, Source, TitleSyntax,
};
use crate::preview::mermaid::flowchart::Direction;

/// The keyword.
pub const KEYWORD: &str = "requirementDiagram";

/// Which kind of requirement a box is. The six the grammar names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// `requirement`
    #[default]
    Requirement,
    /// `functionalRequirement`
    Functional,
    /// `interfaceRequirement`
    Interface,
    /// `performanceRequirement`
    Performance,
    /// `physicalRequirement`
    Physical,
    /// `designConstraint`
    DesignConstraint,
}

impl Kind {
    /// The word drawn in the box's top compartment, as mermaid draws it.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Requirement => "Requirement",
            Kind::Functional => "Functional Requirement",
            Kind::Interface => "Interface Requirement",
            Kind::Performance => "Performance Requirement",
            Kind::Physical => "Physical Requirement",
            Kind::DesignConstraint => "Design Constraint",
        }
    }
}

/// The seven relationship verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// `contains`
    Contains,
    /// `copies`
    Copies,
    /// `derives`
    Derives,
    /// `satisfies`
    Satisfies,
    /// `verifies`
    Verifies,
    /// `refines`
    Refines,
    /// `traces`
    Traces,
}

impl Relation {
    /// The word drawn on the line.
    pub fn word(self) -> &'static str {
        match self {
            Relation::Contains => "contains",
            Relation::Copies => "copies",
            Relation::Derives => "derives",
            Relation::Satisfies => "satisfies",
            Relation::Verifies => "verifies",
            Relation::Refines => "refines",
            Relation::Traces => "traces",
        }
    }
}

/// One requirement.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Requirement {
    /// The name, which is what a relationship refers to it by.
    pub name: String,
    /// Which kind it is.
    pub kind: Kind,
    /// `id:`
    pub id: String,
    /// `text:`
    pub text: String,
    /// `risk:` — `Low`, `Medium` or `High`, as the grammar spells them.
    pub risk: String,
    /// `verifyMethod:` — `Analysis`, `Demonstration`, `Inspection` or `Test`.
    pub verify_method: String,
    /// `:::class`, read and not drawn.
    pub classes: Vec<String>,
}

/// One element.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// The name.
    pub name: String,
    /// `type:`
    pub kind: String,
    /// `docRef:`
    pub doc_ref: String,
    /// `:::class`, read and not drawn.
    pub classes: Vec<String>,
}

/// One relationship.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// The verb.
    pub relation: Relation,
    /// Where the arrow starts.
    pub src: String,
    /// Where it points.
    pub dst: String,
}

/// A parsed requirement diagram.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequirementDiagram {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// The axis the ranks run along.
    pub direction: Direction,
    /// The requirements, in declaration order.
    pub requirements: Vec<Requirement>,
    /// The elements, in declaration order.
    pub elements: Vec<Element>,
    /// The relationships, in source order.
    pub links: Vec<Link>,
}

/// Whether this source is a requirement diagram.
pub fn is_requirement_diagram(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid requirement diagram.
pub fn parse(src: &str) -> Result<RequirementDiagram, ParseError> {
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

    let mut diagram = RequirementDiagram::default();
    let mut env = Envelope::new(front_matter_title);
    let mut open: Option<Body> = None;

    let mut all: Vec<(usize, &str)> = Vec::new();
    if !header_rest.is_empty() {
        all.push((header_index + 1, header_rest.as_str()));
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        all.push((i + 1, line.as_str()));
    }

    for (number, line) in all {
        if open.is_none() && env.read(line, TitleSyntax::RawRestOfLine) {
            continue;
        }
        // `\#[^\n]*` and `\%%[^\n]*` are both skipped by the lexer.
        let t = strip_comment(line).trim();
        if t.is_empty() {
            continue;
        }

        if let Some(body) = open.as_mut() {
            if t == "}" {
                close_body(&mut diagram, open.take().expect("open"));
                continue;
            }
            read_body_line(body, t, number)?;
            continue;
        }

        if let Some(d) = direction_of(t) {
            diagram.direction = d;
            continue;
        }
        // Colour rules, read and dropped. Checked before a definition so `class a b` is not read
        // as an element called `class`.
        if starts_word(t, "style") || starts_word(t, "classDef") || starts_word(t, "class") {
            continue;
        }
        if let Some(link) = read_relationship(t) {
            diagram.links.push(link);
            continue;
        }
        if let Some(body) = read_definition(t, number)? {
            open = Some(body);
            continue;
        }
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    }
    if let Some(body) = open {
        // Upstream's parser has no rule for a body that never closes.
        return Err(ParseError::Invalid {
            line: body.line,
            message: format!("`{}` was opened with `{{` and never closed", body.name),
        });
    }

    diagram.preamble = env.preamble;
    if diagram.requirements.is_empty() && diagram.elements.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "requirement",
        });
    }
    Ok(diagram)
}

/// A `{ … }` block being read.
struct Body {
    line: usize,
    name: String,
    classes: Vec<String>,
    /// `Some` for a requirement, `None` for an element.
    kind: Option<Kind>,
    requirement: Requirement,
    element: Element,
}

fn close_body(diagram: &mut RequirementDiagram, body: Body) {
    match body.kind {
        Some(kind) => {
            // `addRequirement` keeps the **first** definition of a repeated name.
            if diagram.requirements.iter().any(|r| r.name == body.name) {
                return;
            }
            diagram.requirements.push(Requirement {
                name: body.name,
                kind,
                classes: body.classes,
                ..body.requirement
            });
        }
        None => {
            if diagram.elements.iter().any(|e| e.name == body.name) {
                return;
            }
            diagram.elements.push(Element {
                name: body.name,
                classes: body.classes,
                ..body.element
            });
        }
    }
}

/// `<type> name {` or `element name {`, with an optional `:::class` between the two.
fn read_definition(t: &str, number: usize) -> Result<Option<Body>, ParseError> {
    const TYPES: &[(&str, Kind)] = &[
        ("functionalRequirement", Kind::Functional),
        ("interfaceRequirement", Kind::Interface),
        ("performanceRequirement", Kind::Performance),
        ("physicalRequirement", Kind::Physical),
        ("designConstraint", Kind::DesignConstraint),
        ("requirement", Kind::Requirement),
    ];
    let (kind, rest) = match TYPES
        .iter()
        .find_map(|(word, k)| word_rest(t, word).map(|r| (Some(*k), r)))
    {
        Some(hit) => hit,
        None => match word_rest(t, "element") {
            Some(r) => (None, r),
            None => return Ok(None),
        },
    };
    let Some(brace) = rest.rfind('{') else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    };
    if !rest[brace + 1..].trim().is_empty() {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    }
    let head = rest[..brace].trim();
    let (name, classes) = match head.split_once(":::") {
        Some((n, c)) => (
            n.trim(),
            c.split(',').map(|s| s.trim().to_string()).collect(),
        ),
        None => (head, Vec::new()),
    };
    let name = unquote(name);
    if name.is_empty() {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    }
    Ok(Some(Body {
        line: number,
        name,
        classes,
        kind,
        requirement: Requirement::default(),
        element: Element::default(),
    }))
}

fn read_body_line(body: &mut Body, t: &str, number: usize) -> Result<(), ParseError> {
    let Some((key, value)) = t.split_once(':') else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    };
    let value = unquote(value.trim());
    let key = key.trim().to_ascii_lowercase();
    let is_requirement = body.kind.is_some();
    match (is_requirement, key.as_str()) {
        (true, "id") => body.requirement.id = value,
        (true, "text") => body.requirement.text = value,
        (true, "risk") => {
            body.requirement.risk = match value.to_ascii_lowercase().as_str() {
                "low" => "Low",
                "medium" => "Medium",
                "high" => "High",
                _ => {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!("`{value}` is not one of low, medium, high"),
                    })
                }
            }
            .to_string()
        }
        (true, "verifymethod") => {
            body.requirement.verify_method = match value.to_ascii_lowercase().as_str() {
                "analysis" => "Analysis",
                "demonstration" => "Demonstration",
                "inspection" => "Inspection",
                "test" => "Test",
                _ => {
                    return Err(ParseError::Invalid {
                        line: number,
                        message: format!(
                            "`{value}` is not one of analysis, demonstration, inspection, test"
                        ),
                    })
                }
            }
            .to_string()
        }
        (false, "type") => body.element.kind = value,
        (false, "docref") => body.element.doc_ref = value,
        _ => {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            })
        }
    }
    Ok(())
}

/// `A - verb -> B` and `A <- verb - B`.
fn read_relationship(t: &str) -> Option<Link> {
    const VERBS: &[(&str, Relation)] = &[
        ("contains", Relation::Contains),
        ("copies", Relation::Copies),
        ("derives", Relation::Derives),
        ("satisfies", Relation::Satisfies),
        ("verifies", Relation::Verifies),
        ("refines", Relation::Refines),
        ("traces", Relation::Traces),
    ];
    // `->` and `<-` are their own tokens, so the shape is decided by which one is present.
    if let Some((left, rest)) = t.split_once("<-") {
        let (verb, right) = rest.split_once('-')?;
        let relation = VERBS
            .iter()
            .find(|(w, _)| verb.trim().eq_ignore_ascii_case(w))
            .map(|(_, r)| *r)?;
        return Some(Link {
            relation,
            // `addRelationship($3, $5, $1)` — the right-hand id is the source.
            src: unquote(right.trim()),
            dst: unquote(left.trim()),
        });
    }
    let (left, rest) = t.split_once('-')?;
    let (verb, right) = rest.split_once("->")?;
    let relation = VERBS
        .iter()
        .find(|(w, _)| verb.trim().eq_ignore_ascii_case(w))
        .map(|(_, r)| *r)?;
    Some(Link {
        relation,
        src: unquote(left.trim()),
        dst: unquote(right.trim()),
    })
}

/// `.*direction\s+TB[^\n]*` and its three siblings.
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

fn strip_comment(line: &str) -> &str {
    let mut end = line.len();
    if let Some(i) = line.find('#') {
        end = end.min(i);
    }
    if let Some(i) = line.find("%%") {
        end = end.min(i);
    }
    &line[..end]
}

/// `word` at the start of `t`, followed by whitespace. Returns the rest.
///
/// `get(..n)` rather than `[..n]`: a byte index that lands inside a multi-byte character
/// **panics** on a slice and returns `None` here, which is the right answer — every keyword is
/// ASCII, so a character straddling the boundary cannot be one.
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

fn starts_word(t: &str, word: &str) -> bool {
    word_rest(t, word).is_some()
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> RequirementDiagram {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_example_reads_as_a_requirement_an_element_and_a_link() {
        let d = ok("requirementDiagram\n\n    requirement test_req {\n    id: 1\n    text: the test text.\n    risk: high\n    verifymethod: test\n    }\n\n    element test_entity {\n    type: simulation\n    }\n\n    test_entity - satisfies -> test_req\n");
        assert_eq!(d.requirements.len(), 1);
        let r = &d.requirements[0];
        assert_eq!(r.name, "test_req");
        assert_eq!(r.kind, Kind::Requirement);
        assert_eq!(r.id, "1");
        assert_eq!(r.text, "the test text.");
        assert_eq!(r.risk, "High");
        assert_eq!(r.verify_method, "Test");
        assert_eq!(d.elements[0].name, "test_entity");
        assert_eq!(d.elements[0].kind, "simulation");
        assert_eq!(
            d.links[0],
            Link {
                relation: Relation::Satisfies,
                src: "test_entity".to_string(),
                dst: "test_req".to_string(),
            }
        );
    }

    /// The rule the examples never show: `<-` puts the source on the **right**.
    #[test]
    fn the_backwards_arrow_swaps_the_source_and_the_destination() {
        let d = ok("requirementDiagram\n  requirement a {\n  }\n  requirement b {\n  }\n  a <- satisfies - b\n");
        assert_eq!(d.links[0].src, "b");
        assert_eq!(d.links[0].dst, "a");
    }

    #[test]
    fn every_documented_requirement_type_is_read() {
        for (word, want) in [
            ("requirement", Kind::Requirement),
            ("functionalRequirement", Kind::Functional),
            ("interfaceRequirement", Kind::Interface),
            ("performanceRequirement", Kind::Performance),
            ("physicalRequirement", Kind::Physical),
            ("designConstraint", Kind::DesignConstraint),
        ] {
            let d = ok(&format!(
                "requirementDiagram\n  {word} x {{\n  id: 1\n  }}\n"
            ));
            assert_eq!(d.requirements[0].kind, want, "{word}");
        }
    }

    #[test]
    fn every_documented_risk_and_verify_method_is_read() {
        for (risk, want) in [("low", "Low"), ("medium", "Medium"), ("high", "High")] {
            let d = ok(&format!(
                "requirementDiagram\n  requirement x {{\n  risk: {risk}\n  }}\n"
            ));
            assert_eq!(d.requirements[0].risk, want);
        }
        for (m, want) in [
            ("analysis", "Analysis"),
            ("demonstration", "Demonstration"),
            ("inspection", "Inspection"),
            ("test", "Test"),
        ] {
            let d = ok(&format!(
                "requirementDiagram\n  requirement x {{\n  verifyMethod: {m}\n  }}\n"
            ));
            assert_eq!(d.requirements[0].verify_method, want);
        }
    }

    #[test]
    fn a_quoted_name_and_a_quoted_value_lose_their_quotes() {
        let d = ok("requirementDiagram\n  requirement \"__test_req__\" {\n  id: 1\n  text: \"*italicized text* **bold text**\"\n  }\n");
        assert_eq!(d.requirements[0].name, "__test_req__");
        assert_eq!(d.requirements[0].text, "*italicized text* **bold text**");
    }

    #[test]
    fn an_unknown_body_key_is_refused_rather_than_dropped() {
        assert!(matches!(
            parse("requirementDiagram\n  requirement x {\n  colour: red\n  }\n"),
            Err(ParseError::Unexpected { .. })
        ));
        assert!(matches!(
            parse("requirementDiagram\n  requirement x {\n  risk: catastrophic\n  }\n"),
            Err(ParseError::Invalid { .. })
        ));
    }

    #[test]
    fn a_body_that_is_never_closed_is_refused() {
        assert!(matches!(
            parse("requirementDiagram\n  requirement x {\n  id: 1\n"),
            Err(ParseError::Invalid { .. })
        ));
    }

    #[test]
    fn direction_and_the_colour_statements_are_read_and_do_not_become_nodes() {
        let d = ok("requirementDiagram\n  direction LR\n  classDef hot fill:#f00\n  style x fill:#0f0\n  requirement x {\n  id: 1\n  }\n  class x hot\n");
        assert_eq!(d.direction, Direction::LeftToRight);
        assert_eq!(d.requirements.len(), 1, "no phantom nodes");
    }

    #[test]
    fn a_style_separator_on_the_definition_is_read_as_classes() {
        let d = ok("requirementDiagram\n  requirement x:::hot {\n  id: 1\n  }\n");
        assert_eq!(d.requirements[0].name, "x");
        assert_eq!(d.requirements[0].classes, ["hot"]);
    }

    #[test]
    fn a_header_with_nothing_in_it_is_refused() {
        assert!(matches!(
            parse("requirementDiagram\n"),
            Err(ParseError::NoData { .. })
        ));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "requirementDiagram\n  requirement x {\n  }",
            "REQUIREMENTDIAGRAM\n  element x {\n  }",
            "requirementDiagram",
            "requirementDiagrams\n  a",
            "gantt\n  title G",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_requirement_diagram(src), !refused, "{src:?}");
        }
    }
}
