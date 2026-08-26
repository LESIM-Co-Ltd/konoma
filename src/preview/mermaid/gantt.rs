//! Reading mermaid's `gantt` language — dated tasks in sections along a time axis.
//!
//! Read from `gantt/parser/gantt.jison` and `ganttDb.js` at tag `mermaid@11.17.2`.
//!
//! # Why this one has a date library in it
//!
//! Every other language konoma reads is text in and geometry out. A gantt chart is text in,
//! **arithmetic** in the middle, and geometry out: `after b`, `3d`, `until c`, `excludes weekends`
//! are all statements about a calendar, and the picture is wrong — not ugly, *wrong* — if the
//! arithmetic is. `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate konoma is replacing
//! drawing a sixty-day chart on an axis that runs to **2058**, which is what happens when the
//! dates do not parse and the failure is not noticed.
//!
//! So [`time`] is a small civil-calendar module — days-from-civil, a strict `dateFormat` reader
//! and a `d3` axis formatter — and it is checked against dates a person can verify by hand
//! (leap days, month ends, the year 2000 rule) rather than against konoma's own answers.
//!
//! # The three ways a task can say when it happens
//!
//! `parseData` splits the text after `:` on commas, takes the tags off the front, and then reads
//! the rest **by how many fields are left**:
//!
//! | fields | meaning |
//! |---|---|
//! | 1 | starts when the previous task ended; the field is the end |
//! | 2 | start, end |
//! | 3 | **id**, start, end |
//!
//! That is the rule people get wrong, and it is why `des1, 2014-01-06, 2014-01-08` is a task with
//! an *id* of `des1` and `2014-01-06, 2014-01-08` is a task with no id at all.
//!
//! # What is parsed and not drawn
//!
//! `click` / `href` / `call` (a terminal has nowhere to click), `todayMarker off`'s styling
//! arguments, and `topAxis`'s duplicate axis. All are read, because a line the grammar has a rule
//! for and the parser has none for becomes a phantom task (§2-3).

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests and for the renderer look unreachable to rustc until every caller is
// wired. Same reasoning, and the same lint, as the sibling parsers.
#![allow(dead_code)]

use crate::preview::mermaid::chart::{
    find_header, first_word, Envelope, ParseError, Preamble, Source, TitleSyntax,
};

pub mod time;

use time::Instant;

/// The keyword.
pub const KEYWORD: &str = "gantt";

/// The flags a task can carry. `ganttDb`'s `tags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tags {
    /// `active` — in progress.
    pub active: bool,
    /// `done` — finished.
    pub done: bool,
    /// `crit` — on the critical path.
    pub crit: bool,
    /// `milestone` — a moment rather than a span.
    pub milestone: bool,
    /// `vert` — a vertical marker across the whole chart. Read; see the module docs.
    pub vert: bool,
}

/// One task.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// The id another task's `after` / `until` names. Generated when the source gave none.
    pub id: String,
    /// The section it was written under.
    pub section: String,
    /// The words on the bar.
    pub name: String,
    /// When it starts.
    pub start: Instant,
    /// When it ends. Equal to `start` for a milestone.
    pub end: Instant,
    /// Its flags.
    pub tags: Tags,
}

/// A parsed gantt chart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gantt {
    /// Title and accessibility statements.
    pub preamble: Preamble,
    /// The `dateFormat` the source declared, verbatim.
    pub date_format: String,
    /// The `axisFormat` the source declared, verbatim. Empty means the default `%Y-%m-%d`.
    pub axis_format: String,
    /// The `tickInterval` the source declared, verbatim.
    pub tick_interval: String,
    /// `inclusiveEndDates`.
    pub inclusive_end_dates: bool,
    /// `topAxis`. Read and not drawn — see the module docs.
    pub top_axis: bool,
    /// `todayMarker`'s argument. Read and not drawn.
    pub today_marker: String,
    /// `excludes`, lower-cased and split the way `mergeTokens` splits it.
    pub excludes: Vec<String>,
    /// `includes`, likewise.
    pub includes: Vec<String>,
    /// Which day a week starts on, for `tickInterval … week`.
    pub weekday: String,
    /// Which day the weekend starts on, for `excludes weekends`.
    pub weekend: String,
    /// The sections, in declaration order.
    pub sections: Vec<String>,
    /// Every task, in source order.
    pub tasks: Vec<Task>,
}

impl Gantt {
    /// The earliest start and the latest end over every task.
    pub fn extent(&self) -> Option<(Instant, Instant)> {
        let first = self.tasks.first()?;
        let mut lo = first.start;
        let mut hi = first.end;
        for t in &self.tasks {
            lo = lo.min(t.start);
            hi = hi.max(t.end);
        }
        Some((lo, hi))
    }
}

/// Whether this source is a gantt chart.
pub fn is_gantt(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid gantt chart.
pub fn parse(src: &str) -> Result<Gantt, ParseError> {
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

    let mut gantt = Gantt {
        weekday: "sunday".to_string(),
        weekend: "saturday".to_string(),
        ..Gantt::default()
    };
    let mut env = Envelope::new(front_matter_title);
    let mut section = String::new();
    // The raw task rows, kept until every setting has been read: `dateFormat` may legally come
    // after a task upstream too, because dates are compiled in a second pass (`compileTasks`).
    let mut rows: Vec<(usize, String, String, String)> = Vec::new();

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
        // The settings, each of which is one lexer rule of the form `"keyword"\s[^#\n;]+`.
        if let Some(v) = value_of(t, "dateFormat") {
            gantt.date_format = v.to_string();
            continue;
        }
        if let Some(v) = value_of(t, "axisFormat") {
            gantt.axis_format = v.to_string();
            continue;
        }
        if let Some(v) = value_of(t, "tickInterval") {
            gantt.tick_interval = v.to_string();
            continue;
        }
        if let Some(v) = value_of(t, "excludes") {
            merge_tokens(&mut gantt.excludes, v);
            continue;
        }
        if let Some(v) = value_of(t, "includes") {
            merge_tokens(&mut gantt.includes, v);
            continue;
        }
        if let Some(v) = value_of(t, "todayMarker") {
            gantt.today_marker = v.to_string();
            continue;
        }
        if let Some(v) = value_of(t, "section") {
            section = v.trim().to_string();
            gantt.sections.push(section.clone());
            continue;
        }
        if let Some(v) = value_of(t, "weekday") {
            gantt.weekday = v.trim().to_ascii_lowercase();
            continue;
        }
        if let Some(v) = value_of(t, "weekend") {
            gantt.weekend = v.trim().to_ascii_lowercase();
            continue;
        }
        if eq_ci(t, "inclusiveEndDates") {
            gantt.inclusive_end_dates = true;
            continue;
        }
        if eq_ci(t, "topAxis") {
            gantt.top_axis = true;
            continue;
        }
        // `click <id> href "…"` / `call fn(args)`. Read and dropped; a terminal has nothing to
        // click, and leaving it unread would make the line a task called `click des1 href`.
        if value_of(t, "click").is_some() {
            continue;
        }

        // `taskTxt taskData` — `[^:\n]+` then `":"[^#\n;]+`.
        let Some(colon) = t.find(':') else {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            });
        };
        let name = t[..colon].trim().to_string();
        let data: String = t[colon + 1..]
            .split(['#', ';'])
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            });
        }
        rows.push((number, section.clone(), name, data));
    }

    gantt.preamble = env.preamble;
    compile(&mut gantt, &rows)?;
    if gantt.tasks.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "task",
        });
    }
    Ok(gantt)
}

/// Turns the raw rows into dated tasks — `ganttDb`'s `parseData` plus `compileTasks`.
fn compile(gantt: &mut Gantt, rows: &[(usize, String, String, String)]) -> Result<(), ParseError> {
    let format = gantt.date_format.trim().to_string();
    let mut auto = 0usize;
    // Two passes, as upstream does: `after x` may name a task written later.
    struct Raw {
        number: usize,
        section: String,
        name: String,
        id: String,
        tags: Tags,
        start: String,
        end: String,
        previous: Option<String>,
    }
    impl HasId for Raw {
        fn id(&self) -> &str {
            &self.id
        }
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut previous: Option<String> = None;
    for (number, section, name, data) in rows {
        let mut fields: Vec<String> = data.split(',').map(|s| s.trim().to_string()).collect();
        let tags = take_tags(&mut fields);
        let (id, start, end) = match fields.len() {
            1 => {
                auto += 1;
                (format!("task{auto}"), String::new(), fields[0].clone())
            }
            2 => {
                auto += 1;
                (format!("task{auto}"), fields[0].clone(), fields[1].clone())
            }
            3 => (fields[0].clone(), fields[1].clone(), fields[2].clone()),
            _ => {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: *number,
                    text: data.trim().to_string(),
                })
            }
        };
        raws.push(Raw {
            number: *number,
            section: section.clone(),
            name: name.clone(),
            id: id.clone(),
            tags,
            start,
            end,
            previous: previous.clone(),
        });
        previous = Some(id);
    }

    // Resolve, repeating while something moved — `getTasks` iterates `compileTasks` up to ten
    // times so that a chain of `after` settles whichever order it was written in.
    let mut resolved: Vec<Option<(Instant, Instant)>> = vec![None; raws.len()];
    for _ in 0..=raws.len().min(10) {
        let mut moved = false;
        for i in 0..raws.len() {
            let r = &raws[i];
            let start = if r.start.is_empty() {
                // "starts when the previous task ended".
                let Some(prev) = r.previous.as_deref() else {
                    continue;
                };
                let Some(j) = raws.iter().position(|x| x.id == prev) else {
                    continue;
                };
                match resolved[j] {
                    Some((_, end)) => end,
                    None => continue,
                }
            } else {
                match start_of(&r.start, &format, &raws, &resolved) {
                    Some(s) => s,
                    None => {
                        if r.start
                            .trim_start()
                            .to_ascii_lowercase()
                            .starts_with("after")
                        {
                            continue;
                        }
                        return Err(ParseError::Invalid {
                            line: r.number,
                            message: format!("`{}` is not a date this chart can read", r.start),
                        });
                    }
                }
            };
            let end = match end_of(
                &r.end,
                start,
                &format,
                gantt.inclusive_end_dates,
                &raws,
                &resolved,
            ) {
                Some(e) => e,
                None => {
                    if r.end.trim_start().to_ascii_lowercase().starts_with("until") {
                        continue;
                    }
                    return Err(ParseError::Invalid {
                        line: r.number,
                        message: format!(
                            "`{}` is neither a date nor a duration this chart can read",
                            r.end
                        ),
                    });
                }
            };
            let end = if r.tags.milestone { start } else { end };
            let end = fix_end(start, end, gantt);
            if resolved[i] != Some((start, end)) {
                resolved[i] = Some((start, end));
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }

    for (i, r) in raws.iter().enumerate() {
        let Some((start, end)) = resolved[i] else {
            return Err(ParseError::Invalid {
                line: r.number,
                message: format!("task `{}` has no date that can be worked out", r.name),
            });
        };
        gantt.tasks.push(Task {
            id: r.id.clone(),
            section: r.section.clone(),
            name: r.name.clone(),
            start,
            end: end.max(start),
            tags: r.tags,
        });
    }
    Ok(())
}

/// `getStartDate`.
fn start_of(
    text: &str,
    format: &str,
    raws: &[impl HasId],
    resolved: &[Option<(Instant, Instant)>],
) -> Option<Instant> {
    let text = text.trim();
    if (format == "x" || format == "X")
        && text.chars().all(|c| c.is_ascii_digit())
        && !text.is_empty()
    {
        return Some(Instant::from_millis(text.parse::<i64>().ok()?));
    }
    if let Some(ids) = after_ids(text, "after") {
        // "check all after ids and take the latest".
        let mut latest: Option<Instant> = None;
        for id in ids {
            if let Some(j) = raws.iter().position(|r| r.id() == id) {
                if let Some((_, end)) = resolved[j] {
                    latest = Some(match latest {
                        Some(l) if l >= end => l,
                        _ => end,
                    });
                }
            }
        }
        return latest;
    }
    time::parse(text, format)
}

/// `getEndDate`.
fn end_of(
    text: &str,
    start: Instant,
    format: &str,
    inclusive: bool,
    raws: &[impl HasId],
    resolved: &[Option<(Instant, Instant)>],
) -> Option<Instant> {
    let text = text.trim();
    if let Some(ids) = after_ids(text, "until") {
        // "check all until ids and take the earliest".
        let mut earliest: Option<Instant> = None;
        for id in ids {
            if let Some(j) = raws.iter().position(|r| r.id() == id) {
                if let Some((s, _)) = resolved[j] {
                    earliest = Some(match earliest {
                        Some(e) if e <= s => e,
                        _ => s,
                    });
                }
            }
        }
        return earliest;
    }
    if let Some(d) = time::parse(text, format) {
        return Some(if inclusive {
            d.add(1, time::Unit::Day)
        } else {
            d
        });
    }
    let (value, unit) = time::parse_duration(text)?;
    Some(start.add_f64(value, unit))
}

/// `^(after|until)\s+(?<ids>[\d\w- ]+)` — the ids are split on a space.
fn after_ids<'a>(text: &'a str, keyword: &str) -> Option<Vec<&'a str>> {
    let head = text.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &text[keyword.len()..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start();
    // The character class stops at anything that is not a word character, a digit, `-` or a space.
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == ' '))
        .unwrap_or(rest.len());
    Some(rest[..end].split(' ').filter(|s| !s.is_empty()).collect())
}

/// A row that has an id, so `start_of` / `end_of` can look one up without knowing the row type.
trait HasId {
    fn id(&self) -> &str;
}

/// `checkTaskDates` + `fixTaskDates`: push the end out past every excluded day inside the task.
fn fix_end(start: Instant, end: Instant, gantt: &Gantt) -> Instant {
    if gantt.excludes.is_empty() {
        return end;
    }
    let mut cursor = start.add(1, time::Unit::Day);
    let mut end = end;
    let limit = end.add(10_000, time::Unit::Day);
    while cursor <= end {
        if time::is_excluded(cursor, &gantt.excludes, &gantt.includes, &gantt.weekend) {
            end = end.add(1, time::Unit::Day);
            if end > limit {
                break;
            }
        }
        cursor = cursor.add(1, time::Unit::Day);
    }
    end
}

/// `getTaskTags` — the leading fields, while they are one of the five tag words.
fn take_tags(fields: &mut Vec<String>) -> Tags {
    let mut tags = Tags::default();
    while let Some(first) = fields.first().map(|f| f.trim().to_ascii_lowercase()) {
        let hit = match first.as_str() {
            "active" => &mut tags.active,
            "done" => &mut tags.done,
            "crit" => &mut tags.crit,
            "milestone" => &mut tags.milestone,
            "vert" => &mut tags.vert,
            _ => break,
        };
        *hit = true;
        fields.remove(0);
    }
    tags
}

/// `mergeTokens`: lower-case, split on whitespace and commas, keep the set.
fn merge_tokens(into: &mut Vec<String>, text: &str) {
    for token in text
        .to_ascii_lowercase()
        .split([' ', '\t', ','])
        .filter(|t| !t.is_empty())
    {
        if !into.iter().any(|x| x == token) {
            into.push(token.to_string());
        }
    }
}

/// `"keyword"\s[^#\n;]+` — the keyword, whitespace, then the rest of the line up to `#` or `;`.
fn value_of<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let head = line.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &line[keyword.len()..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let value = rest.trim_start();
    if value.is_empty() {
        return None;
    }
    // `section` and `title` take the whole rest of the line; the others stop at `#`/`;`. Taking
    // the narrower rule for all of them would cut a section named `Q1; Q2` in half, so the two
    // that upstream spells `[^\n]+` are excluded here.
    if keyword.eq_ignore_ascii_case("section") {
        return Some(value);
    }
    Some(value.split(['#', ';']).next().unwrap_or(value).trim_end())
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[cfg(test)]
mod tests;
