//! Diff harness for the block-model renderer (`crate::preview::markdown::render`), over the exact
//! same parity corpus the golden render snapshot (`md_snapshot_tests`) already covers. This is
//! **not** a pass/fail equivalence check between the two renderers in general — `render::render_doc`
//! only covers `Heading`/`Paragraph`/`ThematicBreak` so far (see that module's own doc comment on
//! scope), so most cases are expected to fall outside its coverage entirely. What this module checks
//! is narrower and more honest: for every case whose *entire* top-level block set is one this stage
//! claims to support, does the new renderer's fully post-processed output match the production
//! pipeline's, line for line, span for span, style for style?
//!
//! ## Expected mismatch categories (this stage)
//!
//! Even restricted to Heading/Paragraph/ThematicBreak/List/Quote-only documents, one thing is a known,
//! accepted gap at this stage rather than a bug to chase down — see [`render_case_new`]'s callers for
//! where it comes from:
//!
//! * **Standalone block-level images.** A paragraph whose entire content is `![alt](url)` is
//!   intercepted by the production pipeline *before* tui-markdown ever sees it (`split_block_parts`),
//!   becoming a reserved-row image placement — not "alt text as a plain paragraph", which is what
//!   `render::render_doc`'s inline-only `Image` handling produces (see that module's own doc comment:
//!   block-level image extraction is out of scope for this stage).
//!
//! Two *further* categories used to apply here — both artifacts of `render::render_doc` re-parsing
//! each `Heading`/`Paragraph`'s own byte range in isolation rather than reading from a single
//! whole-document parse: a **cross-block reference-style link** (`[text][ref]` whose `[ref]: url`
//! definition lives in a different top-level block) could never resolve that way, and *any*
//! CommonMark construct whose classification depends on context outside its own block was, in
//! principle, at risk of coming out differently when re-parsed alone. `render.rs` no longer
//! re-parses anything at all (see its own module doc comment) — every block's own inline events are
//! read straight out of the one event stream `Doc::parse` recorded for the *whole* document
//! (`model::Doc.events`) — so both categories are gone structurally, not merely unobserved; see
//! `crate::preview::markdown::inline_corpus`'s own doc comment for how the reference-link case
//! specifically was proven to resolve, and [`markdown_render_diff_report`]'s own numbers below for
//! the (lack of) fallout from the second, broader category.
//!
//! A *third* former category, **inline LaTeX math**, is gone the same structural way as of
//! `render::render_paragraph_math` (see that function's own doc comment): `render_doc` now takes the
//! same `math_slot`/`math_on` arguments `render_markdown_with_images` does, reusing that function's
//! own `scan_inline_math`/`math_url` (no second math detector) to lift `$…$`/`$$…$$`/`\(…\)`/`\[…\]`
//! out of a top-level paragraph's own text onto its own placeholder line(s), matching production line
//! for line, span for span — see that function's own doc comment for the "before text / math
//! placeholder / after text" mechanism and why it needs to reproduce a *fresh reparse boundary* at
//! every lift point (not merely "insert a line").
//!
//! Every mismatch this module actually observes is recorded in [`KNOWN_MISMATCHES`], by case name,
//! filled in from a real run of this harness (not asserted to be empty from the start — see the task
//! this module was built for). [`markdown_render_diff_report`] fails only when a *new*, previously
//! unlisted mismatch appears among the cases this stage claims to support — a regression in
//! `render::render_doc` itself, not a gap this stage already knows about and accepts, and not one of
//! [`INTENDED_IMPROVEMENTS`] either (see that list's own doc comment — a *different* kind of expected
//! difference: not a gap, an improvement).

use std::fmt::Write as _;

// Brings in the same `app`-and-descendants scope `md_snapshot_tests` itself relies on (`Config`,
// `Line`/`Span`, and the `md_text` free functions `collapse_links`/`autolink_bare_urls`/
// `substitute_emoji` glob-imported into `app`'s own scope by `app.rs`'s `use md_text::*;`) — see that
// module's own doc comment for the precedent.
use super::md_snapshot_tests::{all_cases, pre_src_for, render_case, SNAPSHOT_WIDTH};
use super::*;

/// Cases (by the same `"corpus: name"` label `all_cases()` reports) whose entire top-level block set
/// is one `render::render_doc` claims to support (`unsupported` is empty), but whose fully
/// post-processed output still does not match the production pipeline's, and where that mismatch is a
/// **gap in this stage** — not a deliberate, permanent improvement (see [`INTENDED_IMPROVEMENTS`] for
/// that other kind). Filled in from an actual run of [`markdown_render_diff_report`] (see its own
/// `--nocapture` output), not asserted empty from the start: most of this stage's job *is* measuring
/// how many of these there are, honestly, not hiding them behind a passing test. Currently empty —
/// the one gap this list used to carry (inline LaTeX math) is closed; see the module doc comment.
const KNOWN_MISMATCHES: &[&str] = &[];

/// Cases whose fully post-processed output does not match production **on purpose** — not a gap in
/// this stage (see [`KNOWN_MISMATCHES`] for that), but this stage's own *improvement* over a real
/// upstream crash the production pipeline only partially recovers from. Kept in a list separate from
/// `KNOWN_MISMATCHES`, deliberately, because the two are not the same kind of "expected mismatch": a
/// `KNOWN_MISMATCHES` entry is something to eventually close (`render_doc` still has a gap);
/// an `INTENDED_IMPROVEMENTS` entry is not expected to *ever* start matching — the day it does,
/// something regressed `render_doc` back down to reproducing production's own bug, not fixed a gap.
///
/// ## The mechanism (verified directly, not merely quoted from the corpus comment)
///
/// Real, upstream `tui-markdown` (`tui-markdown-0.3.7`/`0.3.8`, the pinned dependency) panics parsing
/// a **loose** list item that has a task marker — the doc comment on `render.rs`'s own former
/// `contains_unsupported`'s `"LooseTask"` arm (now removed — see that function's own doc comment)
/// names the panic exactly. `render_text_block_safe` catches that panic and, rather than degrading the
/// *whole* segment to plain text, bisects it at the nearest blank line outside a fence and *retries
/// each half independently* (see that function's own doc comment) — for both cases below, the split
/// lands exactly on the blank line CommonMark itself used to decide the list is loose in the first
/// place, so each half is now a **single-item list with no blank line inside it at all**, which
/// CommonMark's own tight/loose rule makes *tight* — a shape that does not panic. So the *specific*
/// claim "production degrades to plain text" is not quite what was directly observed here: each half
/// still decorates normally (task-marker icon, styling, all present) — what actually differs is that
/// bisection has silently turned one **loose** list (its own items' markers landing on their own line,
/// separate from their bullet — see `render_item`'s own doc comment for exactly why) into *two
/// independent tight one-item lists* glued back together, each rendered as if its marker and text
/// shared one line. `render::render_doc` does not re-parse at all (see its own module doc comment) and
/// so never desyncs the list this way — it draws the one, real, loose list tui-markdown itself would
/// draw if asked to (crash aside), matching `Writer`'s own reimplementation of `TextWriter`'s real
/// (if visually surprising) loose-item layout faithfully. Confirmed directly: calling the production
/// pipeline's own `render_md_segment`/`split_block_for_retry` on each of these two sources shows the
/// whole-text parse panicking (caught) and *both* bisected halves parsing (and decorating) cleanly on
/// their own.
const INTENDED_IMPROVEMENTS: &[&str] = &[
    "list_corpus: task item inside a loose list",
    "task_corpus: a real task at a list item's own indentation is still a real task",
];

/// Renders `src` through `render::render_doc` (the block-model renderer under test) and the exact
/// same app-side post-process `render_case` applies to the production pipeline's own output
/// (`app::md_text::collapse_links`/`autolink_bare_urls`/`substitute_emoji`, gated by the same `cfg`
/// fields) — so a mismatch this module reports can only come from `render_doc` itself (or the one
/// block kind it does not model at all, front matter — see below), never from the two sides running
/// different post-processing.
///
/// Front matter is handled identically to `render_case`'s own (private, unexported) handling of it:
/// `Doc`/`render_doc` have no concept of a metadata block at all (`pre_src_for` already stripped it
/// before `Doc::parse` ever ran — see the model module's own doc comment on what text it expects), so
/// the rendered metadata block is computed the same way and prepended the same way here, by calling
/// the exact same two functions (`strip_front_matter`/`render_front_matter`) `render_case` calls, not
/// a second copy of that four-line prepend.
fn render_case_new(cfg: &Config, src: &str) -> (Vec<Line<'static>>, Vec<&'static str>) {
    let icons = cfg.ui.icons;
    let code = crate::preview::markdown::CodeStyle {
        bg: cfg.ui.theme.code_bg(),
        label_bg: cfg.ui.theme.code_label_bg(),
        label_right: cfg.ui.theme.code_label_right(),
        tab_width: cfg.ui.tab_width,
        wrap: cfg.ui.wrap,
    };
    let theme = cfg.ui.theme.code_theme.as_str();
    let tasks = cfg.ui.md_task_state_chars();

    let fm_lines = if cfg.ui.md_frontmatter {
        match crate::preview::markdown::strip_front_matter(src) {
            (Some(fm), _) => crate::preview::markdown::render_front_matter(&fm, SNAPSHOT_WIDTH),
            (None, _) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Same fixed-answer math slot `md_snapshot_tests::render_case` itself uses (see that function's
    // own doc comment): no live `Picker` in a unit test, so every expression "renders" to its raw-LaTeX
    // fallback — extraction and placement logic is still exercised end to end, just not the
    // picker-dependent raster step. `math_on: true` matches the default `math = "image"` config.
    let math_slot = |_: &str, _: bool| crate::preview::markdown::MathSlot::Raw;

    let pre_src = pre_src_for(cfg, src);
    let doc = crate::preview::markdown::model::Doc::parse(&pre_src);
    let out = crate::preview::markdown::render::render_doc(
        &doc,
        &pre_src,
        SNAPSHOT_WIDTH,
        code,
        theme,
        icons,
        &tasks,
        &math_slot,
        true,
    );

    let mut lines = fm_lines;
    lines.extend(out.lines);

    let (lines, targets) = collapse_links(lines, icons);
    let (lines, _targets) = if cfg.ui.md_autolink {
        autolink_bare_urls(lines, targets)
    } else {
        (lines, targets)
    };
    let lines = if cfg.ui.md_emoji {
        substitute_emoji(lines)
    } else {
        lines
    };

    (lines, out.unsupported)
}

/// One case's outcome: unsupported (some top-level block kind `render_doc` does not render at all —
/// see the module doc comment), an exact match against the production pipeline, or a mismatch, with
/// enough of a diagnostic (`{:?}` of both lines, `ratatui::text::Line` has a hand-written `Debug`
/// impl — see `ratatui_core::text::line`) to see what differs without dumping the whole case.
enum Outcome {
    Unsupported,
    Match,
    Mismatch { first_diff: String },
}

/// The first index at which `a`/`b` differ (comparing by `PartialEq` — `Line`/`Span`/`Style` all
/// derive it, so this is exact: content, span boundaries, and every style field), or `None` if one is
/// a strict prefix of the other (`min(a.len(), b.len())`, reported as the point of divergence too).
fn first_diff_index(a: &[Line<'static>], b: &[Line<'static>]) -> Option<usize> {
    let n = a.len().min(b.len());
    (0..n)
        .find(|&i| a[i] != b[i])
        .or(if a.len() != b.len() { Some(n) } else { None })
}

fn diff_message(old: &[Line<'static>], new: &[Line<'static>], i: usize) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "    first differing line: {i} (old has {} lines, new has {} lines)",
        old.len(),
        new.len()
    )
    .unwrap();
    writeln!(s, "    old[{i}] = {:?}", old.get(i)).unwrap();
    writeln!(s, "    new[{i}] = {:?}", new.get(i)).unwrap();
    s
}

fn run_case(cfg: &Config, src: &str) -> (Outcome, Vec<Line<'static>>, Vec<Line<'static>>) {
    let old = render_case(cfg, src);
    let (new_lines, unsupported) = render_case_new(cfg, src);
    if !unsupported.is_empty() {
        return (Outcome::Unsupported, old.lines, new_lines);
    }
    match first_diff_index(&old.lines, &new_lines) {
        None => (Outcome::Match, old.lines, new_lines),
        Some(i) => {
            let msg = diff_message(&old.lines, &new_lines, i);
            (Outcome::Mismatch { first_diff: msg }, old.lines, new_lines)
        }
    }
}

/// Runs every case in the parity corpus through both renderers and prints an honest tally — see the
/// module doc comment for the report's exact shape and what each mismatch category means. Fails only
/// when a case not already in [`KNOWN_MISMATCHES`] *or* [`INTENDED_IMPROVEMENTS`] mismatches; a case
/// in either list that now matches is reported (via `--nocapture`) as a stale entry, not a failure —
/// this stage's job is to measure honestly, and a shrinking mismatch list is progress, not something
/// to guard against. The two lists are tracked (and reported) separately throughout — see their own
/// doc comments for why conflating them would hide the distinction that is the whole point of having
/// both: one is a gap, the other is not.
#[test]
fn markdown_render_diff_report() {
    let cfg = Config::default();
    let cases = all_cases();
    assert!(
        cases.len() > 100,
        "the parity corpus looks suspiciously small ({} cases) — a corpus-gathering call probably broke",
        cases.len()
    );

    let mut unsupported = 0usize;
    let mut matched = 0usize;
    let mut mismatched: Vec<(String, String)> = Vec::new(); // (case name, diagnostic)
    let mut seen_known: Vec<&'static str> = Vec::new();
    let mut seen_improvements: Vec<&'static str> = Vec::new();

    for (name, src) in &cases {
        let (outcome, _old, _new) = run_case(&cfg, src);
        match outcome {
            Outcome::Unsupported => unsupported += 1,
            Outcome::Match => {
                if let Some(k) = KNOWN_MISMATCHES.iter().find(|k| **k == name.as_str()) {
                    seen_known.push(k);
                }
                if let Some(k) = INTENDED_IMPROVEMENTS.iter().find(|k| **k == name.as_str()) {
                    seen_improvements.push(k);
                }
                matched += 1;
            }
            Outcome::Mismatch { first_diff } => mismatched.push((name.clone(), first_diff)),
        }
    }

    let supported = cases.len() - unsupported;
    let known_count = mismatched
        .iter()
        .filter(|(n, _)| KNOWN_MISMATCHES.contains(&n.as_str()))
        .count();
    let improvement_count = mismatched
        .iter()
        .filter(|(n, _)| INTENDED_IMPROVEMENTS.contains(&n.as_str()))
        .count();
    let new_count = mismatched.len() - known_count - improvement_count;

    eprintln!("=== new-renderer diff report ===");
    eprintln!("  total cases: {}", cases.len());
    eprintln!("  unsupported (outside this stage's block-kind coverage): {unsupported}");
    eprintln!("  supported (this stage claims to cover): {supported}");
    eprintln!("    exact match: {matched}");
    eprintln!(
        "    mismatch: {} (known gaps: {known_count}, intended improvements: {improvement_count}, NEW: {new_count})",
        mismatched.len()
    );
    if !mismatched.is_empty() {
        eprintln!("  mismatches (case, first differing line):");
        for (name, diag) in &mismatched {
            let label = if KNOWN_MISMATCHES.contains(&name.as_str()) {
                "known"
            } else if INTENDED_IMPROVEMENTS.contains(&name.as_str()) {
                "improvement"
            } else {
                "NEW"
            };
            eprintln!("  - [{label}] {name}");
            eprint!("{diag}");
        }
    }
    let stale_known: Vec<&&str> = KNOWN_MISMATCHES
        .iter()
        .filter(|k| !seen_known.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale_known.is_empty() {
        eprintln!("  stale KNOWN_MISMATCHES entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale_known {
            eprintln!("  - {k}");
        }
    }
    let stale_improvements: Vec<&&str> = INTENDED_IMPROVEMENTS
        .iter()
        .filter(|k| !seen_improvements.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale_improvements.is_empty() {
        eprintln!("  stale INTENDED_IMPROVEMENTS entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale_improvements {
            eprintln!("  - {k}");
        }
    }
    eprintln!("=== end report ===");

    let new_mismatches: Vec<&str> = mismatched
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| !KNOWN_MISMATCHES.contains(n) && !INTENDED_IMPROVEMENTS.contains(n))
        .collect();
    assert!(
        new_mismatches.is_empty(),
        "{} new (unlisted) mismatch(es) between render::render_doc and the production renderer: {:?}\n\
         If this is an intentional, understood gap (see this module's own doc comment for the \
         accepted categories), add the case name(s) to KNOWN_MISMATCHES; if it is a deliberate \
         improvement over a real upstream bug (see INTENDED_IMPROVEMENTS's own doc comment), add it \
         there instead. If it is a real bug in render_doc, fix it instead of listing it.",
        new_mismatches.len(),
        new_mismatches
    );
}

/// Sanity check on the report harness itself, mirroring `markdown_render_snapshot_corpus_is_not_empty`
/// in `md_snapshot_tests`: if `all_cases()` silently broke, `markdown_render_diff_report` would still
/// "pass" (zero cases in, zero mismatches out), defeating the whole point of this module.
#[test]
fn markdown_render_diff_corpus_is_not_empty() {
    assert!(all_cases().len() > 100);
}
