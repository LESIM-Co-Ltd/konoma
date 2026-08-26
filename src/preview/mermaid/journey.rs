//! Reading mermaid's `journey` language — a user journey, in sections, with a score per task.
//!
//! Read from `user-journey/parser/journey.jison` and `journeyDb.js` at tag `mermaid@11.17.2`,
//! not recalled: the rule stages 2, 3 and 5a arrived at after recall produced nine real parser
//! bugs between them.
//!
//! # The grammar is four statements long, and the whole of it is in the lexer
//!
//! ```text
//! "section"\s[^#:\n;]+    return 'section';
//! [^#:\n;]+               return 'taskName';
//! ":"[^#\n;]+             return 'taskData';
//! ```
//!
//! Three consequences a hand-written reader gets wrong if it works from the shape of the examples
//! rather than from those three lines, and each is pinned by a test:
//!
//! 1. **A task name may not contain `#`, `:` or `;`** — the character class stops at all three.
//!    `Sign in: 5: Me` therefore splits at the *first* colon and never at a later one.
//! 2. **The task data may contain further colons but not `;` or `#`.** That is what makes
//!    `Do: 5: Me, You` one task with two people rather than a syntax error, and it is why the
//!    people list is split off the *second* colon by `journeyDb`, not by the lexer.
//! 3. **`#` starts a comment anywhere on a line** (`\#[^\n]*` is skipped), unlike every other
//!    mermaid language, where only `%%` does. A `#` in a task name silently truncates it, and
//!    konoma reproduces that rather than "fixing" it, because the alternative is a picture that
//!    disagrees with every other renderer.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, Envelope, ParseError, Preamble, Source, TitleSyntax,
};

/// The keyword.
pub const KEYWORD: &str = "journey";

/// One task on the journey.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// The section it was written under. Empty before the first `section`.
    pub section: String,
    /// The words on the task.
    pub name: String,
    /// How it felt, 1–5. `journeyDb` takes `Number(...)`, so a missing or unreadable score is
    /// `NaN` there; konoma keeps it as `None` so a test can say which of the two happened.
    pub score: Option<f64>,
    /// Who was involved, in the order written.
    pub people: Vec<String>,
}

/// A parsed user journey.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Journey {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// The sections, in the order they were declared.
    pub sections: Vec<String>,
    /// Every task, in source order.
    pub tasks: Vec<Task>,
}

impl Journey {
    /// Everyone named on any task, in first-appearance order — `journeyDb`'s `getActors`.
    pub fn actors(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in &self.tasks {
            for p in &t.people {
                if !out.iter().any(|a| a == p) {
                    out.push(p.clone());
                }
            }
        }
        out
    }
}

/// Whether this source is a user journey.
pub fn is_journey(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid user journey.
pub fn parse(src: &str) -> Result<Journey, ParseError> {
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

    let mut journey = Journey::default();
    let mut env = Envelope::new(front_matter_title);
    if !header_rest.is_empty() {
        read_line(&mut journey, &mut env, &header_rest, header_index + 1)?;
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        read_line(&mut journey, &mut env, line, i + 1)?;
    }
    journey.preamble = env.preamble;
    if journey.tasks.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "task",
        });
    }
    Ok(journey)
}

/// `\#[^\n]*` — the lexer skips from the first `#` to the end of the line.
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn read_line(
    journey: &mut Journey,
    env: &mut Envelope,
    line: &str,
    number: usize,
) -> Result<(), ParseError> {
    if env.read(line, TitleSyntax::RawRestOfLine) {
        return Ok(());
    }
    let line = strip_comment(line);
    let t = line.trim();
    if t.is_empty() {
        return Ok(());
    }
    // `"section"\s[^#:\n;]+` — the keyword needs whitespace after it, and the name stops at a
    // colon. `section: x` is therefore *not* a section, which is the sort of thing only the
    // grammar says.
    if let Some(rest) = strip_keyword(t, "section") {
        let name = rest
            .split([':', ';'])
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        journey.sections.push(name);
        return Ok(());
    }

    let Some(colon) = t.find(':') else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    };
    let name = t[..colon].trim().to_string();
    if name.is_empty() || name.contains(';') {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    }
    // `journeyDb.addTask`: drop the leading `:`, split the rest on `:`; field 0 is the score and
    // field 1 — if there is one — is a comma-separated list of people.
    let data = &t[colon + 1..];
    let mut pieces = data.split(':');
    let score_text = pieces.next().unwrap_or("").trim();
    let people: Vec<String> = match pieces.next() {
        Some(p) => p
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    };
    journey.tasks.push(Task {
        section: journey.sections.last().cloned().unwrap_or_default(),
        name,
        score: score_text.parse::<f64>().ok().filter(|v| v.is_finite()),
        people,
    });
    Ok(())
}

/// `keyword` followed by whitespace, as the lexer's `"section"\s` requires. Returns the rest.
///
/// `get(..n)` rather than `[..n]`: a byte index inside a multi-byte character panics on a slice
/// and is `None` here, which is the right answer — every keyword is ASCII.
fn strip_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line
        .get(..keyword.len())
        .filter(|h| h.eq_ignore_ascii_case(keyword))?;
    let _ = rest;
    let after = &line[keyword.len()..];
    after.starts_with([' ', '\t']).then(|| after.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Journey {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_example_reads_as_three_sections_of_tasks() {
        let j = ok("journey\n    title My working day\n    section Go to work\n      Make tea: 5: Me\n      Go upstairs: 3: Me\n      Do work: 1: Me, Cat\n    section Go home\n      Go downstairs: 5: Me\n      Sit down: 5: Me\n");
        assert_eq!(j.preamble.title.as_deref(), Some("My working day"));
        assert_eq!(j.sections, ["Go to work", "Go home"]);
        assert_eq!(j.tasks.len(), 5);
        assert_eq!(j.tasks[2].name, "Do work");
        assert_eq!(j.tasks[2].score, Some(1.0));
        assert_eq!(j.tasks[2].people, ["Me", "Cat"]);
        assert_eq!(j.tasks[3].section, "Go home");
        assert_eq!(j.actors(), ["Me", "Cat"]);
    }

    /// The lexer skips `\#[^\n]*` anywhere on a line — not only at the start, and not only `%%`.
    #[test]
    fn a_hash_truncates_the_rest_of_the_line() {
        let j = ok("journey\n  Make tea: 5: Me # first thing\n");
        assert_eq!(
            j.tasks[0].people,
            ["Me"],
            "the comment must not become a person"
        );
    }

    /// The other half of the same rule, and the one that surprises: a `#` **before** the colon
    /// takes the task data with it, and a `taskName` with no `taskData` is not a statement in this
    /// grammar. Upstream fails the whole diagram; so does konoma.
    #[test]
    fn a_hash_before_the_colon_makes_the_line_a_syntax_error() {
        assert!(matches!(
            parse("journey\n  Task # not part of the name: 3: Me\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    #[test]
    fn a_task_with_no_people_is_a_task_with_no_people() {
        let j = ok("journey\n  Alone: 2\n");
        assert_eq!(j.tasks[0].score, Some(2.0));
        assert!(j.tasks[0].people.is_empty());
    }

    /// `Number('')` is `NaN` upstream. konoma keeps the distinction rather than defaulting to 0,
    /// because a score of zero is a face a reader would believe.
    #[test]
    fn an_unreadable_score_is_absent_rather_than_zero() {
        let j = ok("journey\n  Hmm: high: Me\n");
        assert_eq!(j.tasks[0].score, None);
        assert_eq!(j.tasks[0].people, ["Me"]);
    }

    #[test]
    fn a_section_keyword_needs_whitespace_after_it() {
        // `section:` has no whitespace, so it is a *task* called `section` — the grammar's answer.
        let j = ok("journey\n  section: 3: Me\n");
        assert!(j.sections.is_empty());
        assert_eq!(j.tasks[0].name, "section");
    }

    #[test]
    fn a_task_before_any_section_belongs_to_no_section() {
        let j = ok("journey\n  First: 3: Me\n  section S\n  Second: 3: Me\n");
        assert_eq!(j.tasks[0].section, "");
        assert_eq!(j.tasks[1].section, "S");
    }

    #[test]
    fn a_line_with_no_colon_is_refused_rather_than_drawn_as_a_scoreless_task() {
        assert!(matches!(
            parse("journey\n  just some words\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    #[test]
    fn a_header_with_no_tasks_is_refused() {
        assert!(matches!(
            parse("journey\n  title T\n"),
            Err(ParseError::NoData { .. })
        ));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "journey\n  A: 3: Me",
            "JOURNEY\n  A: 3: Me",
            "journey",
            "journeys\n  A: 3",
            "gantt\n  title G",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_journey(src), !refused, "{src:?}");
        }
    }

    #[test]
    fn front_matter_and_accessibility_statements_are_read_and_not_drawn() {
        let j = ok("---\ntitle: FM\n---\njourney\n  accTitle: a\n  accDescr: b\n  A: 3: Me\n");
        assert_eq!(j.preamble.title.as_deref(), Some("FM"));
        assert_eq!(j.preamble.acc_title.as_deref(), Some("a"));
        assert_eq!(j.preamble.acc_descr.as_deref(), Some("b"));
        assert_eq!(j.tasks.len(), 1);
    }
}
