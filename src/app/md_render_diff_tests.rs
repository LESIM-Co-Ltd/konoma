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
//! Even restricted to Heading/Paragraph/ThematicBreak-only documents, a couple of things are known,
//! accepted gaps at this stage rather than bugs to chase down — see [`render_case_new`]'s callers for
//! where each one comes from:
//!
//! * **Standalone block-level images.** A paragraph whose entire content is `![alt](url)` is
//!   intercepted by the production pipeline *before* tui-markdown ever sees it (`split_block_parts`),
//!   becoming a reserved-row image placement — not "alt text as a plain paragraph", which is what
//!   `render::render_doc`'s inline-only `Image` handling produces (see that module's own doc comment:
//!   block-level image extraction is out of scope for this stage).
//! * **Inline LaTeX math.** `md_snapshot_tests::render_case` always renders with math extraction "on"
//!   (`math_on: true`), lifting `$…$`/`$$…$$` onto their own reserved-row placeholder before
//!   tui-markdown ever sees them. The block model has no concept of math extraction at all (it is not
//!   a `BlockKind`), so `render::render_doc` renders a `$…$` run as literal text.
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
//! Every mismatch this module actually observes is recorded in [`KNOWN_MISMATCHES`], by case name,
//! filled in from a real run of this harness (not asserted to be empty from the start — see the task
//! this module was built for). [`markdown_render_diff_report`] fails only when a *new*, previously
//! unlisted mismatch appears among the cases this stage claims to support — a regression in
//! `render::render_doc` itself, not a gap this stage already knows about and accepts.

use std::fmt::Write as _;

// Brings in the same `app`-and-descendants scope `md_snapshot_tests` itself relies on (`Config`,
// `Line`/`Span`, and the `md_text` free functions `collapse_links`/`autolink_bare_urls`/
// `substitute_emoji` glob-imported into `app`'s own scope by `app.rs`'s `use md_text::*;`) — see that
// module's own doc comment for the precedent.
use super::md_snapshot_tests::{all_cases, pre_src_for, render_case, SNAPSHOT_WIDTH};
use super::*;

/// Cases (by the same `"corpus: name"` label `all_cases()` reports) whose entire top-level block set
/// is one `render::render_doc` claims to support (`unsupported` is empty), but whose fully
/// post-processed output still does not match the production pipeline's — see the module doc comment
/// for the categories these fall into. Filled in from an actual run of
/// [`markdown_render_diff_report`] (see its own `--nocapture` output), not asserted empty from the
/// start: most of this stage's job *is* measuring how many of these there are, honestly, not hiding
/// them behind a passing test.
const KNOWN_MISMATCHES: &[&str] = &[
    // All three are the "inline LaTeX math" gap the module doc comment names: `md_snapshot_tests`
    // renders with math extraction always on, lifting `$$x$$`/`\(x\)`/`\[x\]` onto their own
    // reserved-row placeholder before tui-markdown ever sees the run; `render::render_doc` has no
    // concept of math extraction at all (it is not a `BlockKind`), so it renders the second
    // occurrence in each of these cases (the one *outside* the leading code span) as literal text
    // glued onto the same line instead of a placeholder on its own row.
    "code_span_corpus: bracket math inside and outside",
    "code_span_corpus: display math inside and outside",
    "code_span_corpus: paren math inside and outside",
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

    let pre_src = pre_src_for(cfg, src);
    let doc = crate::preview::markdown::model::Doc::parse(&pre_src);
    let out = crate::preview::markdown::render::render_doc(
        &doc,
        SNAPSHOT_WIDTH,
        code,
        theme,
        icons,
        &tasks,
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
/// when a case not already in [`KNOWN_MISMATCHES`] mismatches; a case *in* `KNOWN_MISMATCHES` that
/// now matches is reported (via `--nocapture`) as a stale entry, not a failure — this stage's job is
/// to measure honestly, and a shrinking mismatch list is progress, not something to guard against.
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

    for (name, src) in &cases {
        let (outcome, _old, _new) = run_case(&cfg, src);
        match outcome {
            Outcome::Unsupported => unsupported += 1,
            Outcome::Match => {
                if KNOWN_MISMATCHES.contains(&name.as_str()) {
                    seen_known.push(
                        KNOWN_MISMATCHES
                            .iter()
                            .find(|k| **k == name.as_str())
                            .unwrap(),
                    );
                }
                matched += 1;
            }
            Outcome::Mismatch { first_diff } => mismatched.push((name.clone(), first_diff)),
        }
    }

    let supported = cases.len() - unsupported;

    eprintln!("=== new-renderer diff report ===");
    eprintln!("  total cases: {}", cases.len());
    eprintln!("  unsupported (outside this stage's block-kind coverage): {unsupported}");
    eprintln!("  supported (this stage claims to cover): {supported}");
    eprintln!("    exact match: {matched}");
    eprintln!("    mismatch: {}", mismatched.len());
    if !mismatched.is_empty() {
        eprintln!("  mismatches (case, first differing line):");
        for (name, diag) in &mismatched {
            let known = if KNOWN_MISMATCHES.contains(&name.as_str()) {
                "known"
            } else {
                "NEW"
            };
            eprintln!("  - [{known}] {name}");
            eprint!("{diag}");
        }
    }
    let stale: Vec<&&str> = KNOWN_MISMATCHES
        .iter()
        .filter(|k| !seen_known.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale.is_empty() {
        eprintln!("  stale KNOWN_MISMATCHES entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale {
            eprintln!("  - {k}");
        }
    }
    eprintln!("=== end report ===");

    let new_mismatches: Vec<&str> = mismatched
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| !KNOWN_MISMATCHES.contains(n))
        .collect();
    assert!(
        new_mismatches.is_empty(),
        "{} new (unlisted) mismatch(es) between render::render_doc and the production renderer: {:?}\n\
         If this is an intentional, understood gap (see this module's own doc comment for the \
         accepted categories), add the case name(s) to KNOWN_MISMATCHES. If it is a real bug in \
         render_doc, fix it instead of listing it.",
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
