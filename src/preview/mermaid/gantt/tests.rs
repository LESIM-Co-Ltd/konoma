//! What the gantt reader is checked with.
//!
//! The arithmetic is checked in [`super::time`], against dates a person can verify. What is
//! checked here is the *language*: which field means what, and what a task inherits when it does
//! not say.

use super::time::Instant;
use super::*;

fn ok(src: &str) -> Gantt {
    parse(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn day(t: Instant) -> (i64, u32, u32) {
    let (y, m, d, ..) = t.civil();
    (y, m, d)
}

/// mermaid's own first example on the gantt page, whole.
#[test]
fn the_documented_example_reads_as_dated_tasks_in_sections() {
    let g = ok("gantt\n    title A Gantt Diagram\n    dateFormat YYYY-MM-DD\n    section Section\n        A task          :a1, 2014-01-01, 30d\n        Another task    :after a1, 20d\n    section Another\n        Task in Another :2014-01-12, 12d\n        another task    :24d\n");
    assert_eq!(g.preamble.title.as_deref(), Some("A Gantt Diagram"));
    assert_eq!(g.date_format, "YYYY-MM-DD");
    assert_eq!(g.sections, ["Section", "Another"]);
    assert_eq!(g.tasks.len(), 4);

    assert_eq!(g.tasks[0].id, "a1");
    assert_eq!(day(g.tasks[0].start), (2014, 1, 1));
    assert_eq!(day(g.tasks[0].end), (2014, 1, 31));

    // `after a1, 20d` — two fields, so no id of its own, and it starts where a1 ended.
    assert_eq!(day(g.tasks[1].start), (2014, 1, 31));
    assert_eq!(day(g.tasks[1].end), (2014, 2, 20));

    assert_eq!(g.tasks[2].section, "Another");
    assert_eq!(day(g.tasks[2].start), (2014, 1, 12));

    // One field: it starts when the task before it ended.
    assert_eq!(day(g.tasks[3].start), day(g.tasks[2].end));
}

/// The rule people get wrong: the *number of fields* decides whether the first one is an id.
#[test]
fn the_field_count_decides_whether_the_first_field_is_an_id() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  three :des1, 2014-01-06, 2014-01-08\n  two :2014-01-09, 2d\n  one :3d\n");
    assert_eq!(g.tasks[0].id, "des1");
    assert_eq!(day(g.tasks[0].start), (2014, 1, 6));
    assert_eq!(day(g.tasks[0].end), (2014, 1, 8));

    assert_ne!(g.tasks[1].id, "2014-01-09", "two fields means no id");
    assert_eq!(day(g.tasks[1].start), (2014, 1, 9));
    assert_eq!(day(g.tasks[1].end), (2014, 1, 11));

    assert_eq!(day(g.tasks[2].start), (2014, 1, 11));
    assert_eq!(day(g.tasks[2].end), (2014, 1, 14));
}

#[test]
fn every_documented_tag_is_read_off_the_front_of_the_data() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  a :done, des1, 2014-01-06, 2014-01-08\n  b :active, des2, 2014-01-09, 3d\n  c :crit, done, des3, 2014-01-06, 24h\n  d :milestone, m1, 2014-01-25, 0d\n  e :vert, v1, 2014-01-26, 0d\n");
    assert!(g.tasks[0].tags.done);
    assert!(g.tasks[1].tags.active);
    assert!(g.tasks[2].tags.crit && g.tasks[2].tags.done);
    assert!(g.tasks[3].tags.milestone);
    assert!(g.tasks[4].tags.vert);
    // A milestone is a moment, so it has no width at all.
    assert_eq!(g.tasks[3].start, g.tasks[3].end);
}

/// `after a b c` takes the **latest** of the ends it names; `until` takes the **earliest** start.
#[test]
fn after_takes_the_latest_and_until_takes_the_earliest() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  a :a, 2014-01-01, 5d\n  b :b, 2014-01-01, 10d\n  c :after a b, 1d\n  d :d, 2014-02-01, 1d\n  e :e, 2014-03-01, 1d\n  f :2014-01-20, until d e\n");
    assert_eq!(
        day(g.tasks[2].start),
        (2014, 1, 11),
        "the later of the two ends"
    );
    assert_eq!(
        day(g.tasks[5].end),
        (2014, 2, 1),
        "the earlier of the two starts"
    );
}

/// An `after` that names a task written **later** still resolves — upstream compiles in a loop
/// for exactly this reason, and a one-pass reader draws the task at the wrong place.
#[test]
fn after_can_name_a_task_that_has_not_been_written_yet() {
    let g = ok(
        "gantt\n  dateFormat YYYY-MM-DD\n  late :after early, 2d\n  early :early, 2014-05-01, 3d\n",
    );
    assert_eq!(day(g.tasks[0].start), (2014, 5, 4));
}

#[test]
fn inclusive_end_dates_pushes_an_explicit_end_out_by_a_day() {
    let plain = ok("gantt\n  dateFormat YYYY-MM-DD\n  a :2014-01-01, 2014-01-03\n");
    let inclusive =
        ok("gantt\n  dateFormat YYYY-MM-DD\n  inclusiveEndDates\n  a :2014-01-01, 2014-01-03\n");
    assert_eq!(day(plain.tasks[0].end), (2014, 1, 3));
    assert_eq!(day(inclusive.tasks[0].end), (2014, 1, 4));
}

/// A weekend inside a task pushes the end out, one excluded day at a time.
#[test]
fn excludes_weekends_lengthens_a_task_that_spans_one() {
    // 2024-01-01 is a Monday; five working days ends on Friday the 5th, but a `5d` task from
    // Monday would naturally end on Saturday the 6th, so nothing moves. Seven days does.
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  excludes weekends\n  a :2024-01-01, 7d\n");
    assert_eq!(
        day(g.tasks[0].end),
        (2024, 1, 10),
        "the Saturday and Sunday inside the span push the end out by two"
    );
}

#[test]
fn an_included_date_wins_over_excluded_weekends() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  excludes weekends\n  includes 2024-01-06 2024-01-07\n  a :2024-01-01, 7d\n");
    assert_eq!(day(g.tasks[0].end), (2024, 1, 8));
}

#[test]
fn the_settings_are_read_verbatim_where_they_are_drawn_and_lower_cased_where_they_are_matched() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  axisFormat %d/%m\n  tickInterval 1week\n  todayMarker off\n  weekday monday\n  weekend friday\n  excludes Weekends, 2024-01-01\n  topAxis\n  a :2024-02-01, 1d\n");
    assert_eq!(g.axis_format, "%d/%m");
    assert_eq!(g.tick_interval, "1week");
    assert_eq!(g.today_marker, "off");
    assert_eq!(g.weekday, "monday");
    assert_eq!(g.weekend, "friday");
    assert_eq!(g.excludes, ["weekends", "2024-01-01"]);
    assert!(g.top_axis);
}

/// `"section"\s[^\n]+` takes the **whole** rest of the line, colons and semicolons included —
/// unlike every other setting, which stops at `#` or `;`.
#[test]
fn a_section_name_may_contain_a_colon() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  section Phase 1: design\n  a :2024-01-01, 1d\n");
    assert_eq!(g.sections, ["Phase 1: design"]);
    assert_eq!(g.tasks[0].section, "Phase 1: design");
}

/// A `click` line is read and dropped. Leaving it unread would make it a task called
/// `click des1 href` with `//x.test/` as its dates.
#[test]
fn a_click_line_is_read_and_is_not_a_task() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  a :des1, 2024-01-01, 1d\n  click des1 href \"https://x.test/\"\n");
    assert_eq!(g.tasks.len(), 1);
}

#[test]
fn a_date_that_cannot_be_read_is_refused_rather_than_drawn_somewhere_wrong() {
    // This is the failure `docs/FEATURE-MERMAID-RENDERER.md` §6-A records the crate turning into
    // an axis that runs to 2058.
    assert!(matches!(
        parse("gantt\n  dateFormat YYYY-MM-DD\n  a :not-a-date, 3d\n"),
        Err(ParseError::Invalid { .. })
    ));
    assert!(matches!(
        parse("gantt\n  dateFormat YYYY-MM-DD\n  a :2024-01-01, three days\n"),
        Err(ParseError::Invalid { .. })
    ));
}

#[test]
fn a_header_with_no_tasks_is_refused() {
    assert!(matches!(
        parse("gantt\n  title G\n"),
        Err(ParseError::NoData { .. })
    ));
}

#[test]
fn a_task_line_with_no_colon_is_refused() {
    assert!(matches!(
        parse("gantt\n  dateFormat YYYY-MM-DD\n  just words\n"),
        Err(ParseError::Unexpected { .. })
    ));
}

#[test]
fn the_routing_predicate_and_the_parser_agree() {
    for src in [
        "gantt\n  a :2024-01-01, 1d",
        "GANTT\n  a :2024-01-01, 1d",
        "gantt",
        "gantts\n  a",
        "journey\n  a: 1",
        "",
        "%% only a comment\n",
    ] {
        let refused = matches!(parse(src), Err(ParseError::NotThisChart { .. }));
        assert_eq!(is_gantt(src), !refused, "{src:?}");
    }
}

#[test]
fn the_extent_spans_every_task() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  a :2024-03-01, 2d\n  b :2024-01-01, 2d\n  c :2024-06-01, 2d\n");
    let (lo, hi) = g.extent().expect("tasks");
    assert_eq!(day(lo), (2024, 1, 1));
    assert_eq!(day(hi), (2024, 6, 3));
}

/// **A milestone is a moment, whatever duration the source gave it.**
///
/// `ganttDb.compileTasks` sets `endTime = startTime` for a milestone unconditionally, and the
/// documentation's examples always write `0d` — which makes that rule and *no rule at all* the
/// same answer. A mutation that deleted it therefore survived a whole campaign: every milestone
/// the corpus could reach it with was zero-length already.
#[test]
fn a_milestone_ends_when_it_starts_even_when_a_duration_was_written() {
    let g = ok("gantt\n  dateFormat YYYY-MM-DD\n  m :milestone, m1, 2024-01-01, 5d\n");
    assert_eq!(g.tasks[0].start, g.tasks[0].end, "a milestone took time");
    assert_eq!(day(g.tasks[0].end), (2024, 1, 1));
    // …and the chart's own extent does not stretch to cover a duration it never had.
    let (_, hi) = g.extent().expect("tasks");
    assert_eq!(day(hi), (2024, 1, 1));
}
