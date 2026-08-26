//! Reading mermaid's `timeline` language — periods along an axis, each with its events.
//!
//! Read from `timeline/parser/timeline.jison` and `timelineDb.js` at tag `mermaid@11.17.2`.
//!
//! # The one rule the examples do not show
//!
//! An event is lexed as
//!
//! ```text
//! ":"\s(?:[^:\n]|":"(?!\s))+        return 'event';
//! ```
//!
//! — a colon, **then whitespace**, then characters that may include a colon *only when the colon
//! is not followed by whitespace*. Three things fall out of that, and a reader written from the
//! examples gets all three wrong:
//!
//! 1. `2004 : Facebook : Google` is **one period and two events**, because the second `: ` ends
//!    the first event and starts another. The examples show it on one line and on two, and both
//!    mean the same thing.
//! 2. `12:30` inside an event stays inside it — the colon is not followed by whitespace.
//! 3. `2004:Facebook` (no space after the colon) is **not** an event. The whole line is a period,
//!    colon and all, because `period` is `[^#:\n]+`… which stops at the colon, leaving `:Facebook`
//!    to match nothing. Upstream fails the diagram; so does konoma, rather than guessing.
//!
//! # `#` is a comment here too
//!
//! `\#[^\n]*` is skipped, exactly as in `journey`. A `#` in a period name truncates it.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, Envelope, ParseError, Preamble, Source, TitleSyntax,
};

/// The keyword.
pub const KEYWORD: &str = "timeline";

/// Which way the axis runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// `timeline` and `timeline LR` — periods run left to right. mermaid's default.
    #[default]
    LeftToRight,
    /// `timeline TD` — periods run down the page (`timelineRendererVertical`).
    TopToBottom,
}

/// One period on the axis, with the events written under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    /// The section it was written under. Empty before the first `section`.
    pub section: String,
    /// The words naming the period — a year, a quarter, a phase.
    pub name: String,
    /// The events, in source order.
    pub events: Vec<String>,
}

/// A parsed timeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timeline {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// Which way the axis runs.
    pub direction: Direction,
    /// The sections, in declaration order.
    pub sections: Vec<String>,
    /// The periods, in source order.
    pub periods: Vec<Period>,
}

/// Whether this source is a timeline.
pub fn is_timeline(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid timeline.
pub fn parse(src: &str) -> Result<Timeline, ParseError> {
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

    let mut timeline = Timeline::default();
    let mut env = Envelope::new(front_matter_title);

    // `"timeline"[ \t]+LR` / `[ \t]+TD` are their own tokens, so the direction has to sit on the
    // keyword's line with nothing else in between.
    let mut rest = header_rest.trim();
    if let Some(after) = strip_word(rest, "LR") {
        timeline.direction = Direction::LeftToRight;
        rest = after;
    } else if let Some(after) = strip_word(rest, "TD") {
        timeline.direction = Direction::TopToBottom;
        rest = after;
    }
    if !rest.is_empty() {
        read_line(&mut timeline, &mut env, rest, header_index + 1)?;
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        read_line(&mut timeline, &mut env, line, i + 1)?;
    }
    timeline.preamble = env.preamble;
    if timeline.periods.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "period",
        });
    }
    Ok(timeline)
}

/// `word` as a whole word at the start of `rest`, case-insensitively.
fn strip_word<'a>(rest: &'a str, word: &str) -> Option<&'a str> {
    let head = rest.get(..word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    let after = &rest[word.len()..];
    (after.is_empty() || after.starts_with([' ', '\t'])).then(|| after.trim_start())
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn read_line(
    timeline: &mut Timeline,
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
    // `"section"\s[^:\n]+` — unlike `journey`, a section name here *may* contain `;` and `#`
    // (well, `#` is eaten by the comment rule first), and stops only at a colon.
    if let Some(rest) = strip_keyword(t, "section") {
        let name = rest.split(':').next().unwrap_or("").trim().to_string();
        timeline.sections.push(name);
        return Ok(());
    }

    let mut cursor = t;
    // A line beginning with `: ` continues the previous period.
    if !starts_event(cursor) {
        let end = cursor.find(':').unwrap_or(cursor.len());
        let name = cursor[..end].trim().to_string();
        if name.is_empty() {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            });
        }
        timeline.periods.push(Period {
            section: timeline.sections.last().cloned().unwrap_or_default(),
            name,
            events: Vec::new(),
        });
        cursor = cursor[end..].trim_start_matches(' ');
    }

    while !cursor.is_empty() {
        if !starts_event(cursor) {
            // A colon with no space after it matches neither `event` nor `period`.
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: cursor.trim().to_string(),
            });
        }
        let body = &cursor[1..];
        let end = next_event_start(body);
        let text = body[..end].trim().to_string();
        match timeline.periods.last_mut() {
            Some(p) => p.events.push(text),
            None => {
                // `addEvent` looks the current task up by id; with no task at all it throws.
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: t.to_string(),
                });
            }
        }
        cursor = body[end..].trim_start_matches(' ');
    }
    Ok(())
}

/// `":"\s` — the two characters that begin an event.
fn starts_event(s: &str) -> bool {
    let mut it = s.chars();
    it.next() == Some(':') && it.next().is_some_and(|c| c == ' ' || c == '\t')
}

/// Where the next event begins inside an event's body: the first `:` **followed by whitespace**.
fn next_event_start(body: &str) -> usize {
    let bytes = body.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b':' && matches!(bytes.get(i + 1), Some(b' ') | Some(b'\t')) {
            return i;
        }
    }
    body.len()
}

/// `keyword` followed by whitespace, as the lexer's `"section"\s` requires.
fn strip_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    if !line
        .get(..keyword.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(keyword))
    {
        return None;
    }
    let after = &line[keyword.len()..];
    after.starts_with([' ', '\t']).then(|| after.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Timeline {
        parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
    }

    #[test]
    fn the_documented_social_media_example_reads_as_four_periods() {
        let t = ok("timeline\n    title History of Social Media Platform\n    2002 : LinkedIn\n    2004 : Facebook\n         : Google\n    2005 : YouTube\n    2006 : Twitter\n");
        assert_eq!(
            t.preamble.title.as_deref(),
            Some("History of Social Media Platform")
        );
        assert_eq!(t.periods.len(), 4);
        assert_eq!(t.periods[1].name, "2004");
        assert_eq!(t.periods[1].events, ["Facebook", "Google"]);
    }

    /// The same diagram written on one line means the same thing — this is the rule the lexer's
    /// `":"(?!\s)` lookahead exists for.
    #[test]
    fn two_events_on_one_line_are_two_events() {
        let t = ok("timeline\n  2004 : Facebook : Google\n");
        assert_eq!(t.periods.len(), 1);
        assert_eq!(t.periods[0].events, ["Facebook", "Google"]);
    }

    /// …and a colon with nothing after it stays inside the event.
    #[test]
    fn a_colon_not_followed_by_a_space_stays_inside_the_event() {
        let t = ok("timeline\n  Day 1 : Stand-up at 09:30 sharp\n");
        assert_eq!(t.periods[0].events, ["Stand-up at 09:30 sharp"]);
    }

    #[test]
    fn sections_group_the_periods_under_them() {
        let t = ok("timeline\n  section 17th-20th century\n    Industry 1.0 : Steam\n  section 21st century\n    Industry 4.0 : Internet\n");
        assert_eq!(t.sections, ["17th-20th century", "21st century"]);
        assert_eq!(t.periods[0].section, "17th-20th century");
        assert_eq!(t.periods[1].section, "21st century");
    }

    #[test]
    fn the_direction_is_read_from_the_keyword_line() {
        assert_eq!(
            ok("timeline TD\n  a : b\n").direction,
            Direction::TopToBottom
        );
        assert_eq!(
            ok("timeline LR\n  a : b\n").direction,
            Direction::LeftToRight
        );
        assert_eq!(ok("timeline\n  a : b\n").direction, Direction::LeftToRight);
    }

    /// `timeline TDX` is not a direction: the token is `"timeline"[ \t]+TD`, so what follows has
    /// to end the word. Without this, a period called `TDX` would vanish.
    #[test]
    fn a_word_beginning_with_td_is_not_the_direction() {
        let t = ok("timeline TDX : first\n");
        assert_eq!(t.direction, Direction::LeftToRight);
        assert_eq!(t.periods[0].name, "TDX");
    }

    #[test]
    fn a_colon_with_no_space_after_it_is_refused_rather_than_guessed() {
        assert!(matches!(
            parse("timeline\n  2004:Facebook\n"),
            Err(ParseError::Unexpected { .. })
        ));
    }

    #[test]
    fn a_hash_truncates_the_rest_of_the_line() {
        let t = ok("timeline\n  2002 : LinkedIn # the first one\n");
        assert_eq!(t.periods[0].events, ["LinkedIn"]);
    }

    #[test]
    fn a_period_with_no_events_is_still_a_period() {
        let t = ok("timeline\n  Prehistory\n  2002 : LinkedIn\n");
        assert_eq!(t.periods.len(), 2);
        assert!(t.periods[0].events.is_empty());
    }

    #[test]
    fn a_header_with_no_periods_is_refused() {
        assert!(matches!(
            parse("timeline\n  title T\n"),
            Err(ParseError::NoData { .. })
        ));
    }

    #[test]
    fn the_routing_predicate_and_the_parser_agree() {
        for src in [
            "timeline\n  a : b",
            "TIMELINE\n  a : b",
            "timeline TD\n  a : b",
            "timeline",
            "timelines\n  a",
            "journey\n  a: 1",
            "",
            "%% only a comment\n",
        ] {
            let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
            assert_eq!(is_timeline(src), !refused, "{src:?}");
        }
    }
}
