//! Golden-snapshot safety net for the new Markdown block model
//! (`crate::preview::markdown::model`), the same kind of net `md_snapshot_tests` (a sibling module)
//! already keeps over Markdown *rendering*, aimed at the model instead. Reuses that module's own
//! corpus-gathering (`all_cases`), preprocessing (`pre_src_for`), and snapshot-compare machinery
//! (`assert_matches_snapshot`) directly rather than re-implementing any of the three — the whole
//! point of a second net covering the same corpus is that it cannot silently drift onto a
//! different one, or a differently-preprocessed one, from the first.
//!
//! Unlike `md_snapshot_tests`, this dump has no `code_bg`-flavored second variant: the model
//! carries no style/color information at all (see `crate::preview::markdown::model`'s module doc
//! comment on scope), so there is nothing that config value could change here. One file,
//! `snapshots/markdown_model.snap`, covers it.
//!
//! ## Regenerating
//!
//! ```text
//! KONOMA_UPDATE_SNAPSHOTS=1 cargo test --features git markdown_model_snapshot
//! KONOMA_UPDATE_SNAPSHOTS=1 cargo test --no-default-features markdown_model_snapshot
//! ```
//!
//! Review the resulting diff under `snapshots/` like any other generated-file change before
//! committing it.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::PathBuf;

use pulldown_cmark::{Event, Parser, Tag};

use crate::config::Config;
use crate::preview::markdown::model::{
    self, code_body_text, html_body_text, html_body_text_in, is_stray_inline_leaf,
    is_unmodeled_container_tag, Block, BlockKind, Doc,
};

use super::md_snapshot_tests::{all_cases, assert_matches_snapshot, pre_src_for};

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join("markdown_model.snap")
}

/// `{:?}`-escaped, truncated to `CAP` **chars** (not bytes, so this never lands mid-codepoint) —
/// long block bodies (a big fenced block, a whole sample file's lead paragraph) would otherwise
/// make individual dump lines unreadable and swamp a diff with content that isn't the point of the
/// line it's on (the range and the kind are).
fn snippet(s: &str) -> String {
    const CAP: usize = 80;
    if s.chars().count() <= CAP {
        format!("{s:?}")
    } else {
        let truncated: String = s.chars().take(CAP).collect();
        format!("{truncated:?}…")
    }
}

fn fmt_range(r: &Range<usize>) -> String {
    format!("{}..{}", r.start, r.end)
}

/// Concatenates every `Event::Text`/`Event::Code`/`Event::SoftBreak`(→ `" "`)/`Event::HardBreak`(→
/// `"\n"`) payload in `events[inline]`, in order — the same minimal, style-blind reconstruction
/// `model::tests::inline_plain_text` uses at the unit-test level, kept here as an independent copy
/// rather than reused directly: that one is `#[cfg(test)]`-private to `model.rs`'s own `mod tests`,
/// and this dump has no path to it. Folded into `fmt_kind`'s own `Heading`/`Paragraph` output the
/// same way `code_body_text` is folded into its `CodeBlock` output — a snapshot of *only* the
/// `inline` range's own numbers, with no reconstructed text alongside it, would leave a silent
/// content regression (the range pointing at the *wrong* slice of `Doc.events`, while still
/// well-formed) invisible to this dump.
fn inline_text(events: &[(Event<'_>, Range<usize>)], inline: &Range<usize>) -> String {
    let mut s = String::new();
    for (ev, _) in &events[inline.clone()] {
        match ev {
            Event::Text(t) => s.push_str(t),
            Event::Code(c) => s.push_str(c),
            Event::SoftBreak => s.push(' '),
            Event::HardBreak => s.push('\n'),
            _ => {}
        }
    }
    s
}

/// Exhaustive on purpose (matching `md_snapshot_tests::fmt_item_kind`'s own precedent): a new
/// `BlockKind` variant must be taught to this dump rather than silently rendering as nothing.
/// `events` is `Doc.events` for the whole document this block came from — every `Heading`/
/// `Paragraph`'s own `inline` field indexes into that same single `Vec`, at any nesting depth.
fn fmt_kind(k: &BlockKind, src: &str, events: &[(Event<'_>, Range<usize>)]) -> String {
    match k {
        BlockKind::Heading {
            level,
            inline,
            id,
            classes,
            attrs,
        } => format!(
            "Heading{{level={level}, inline={}:{}, id={id:?}, classes={classes:?}, attrs={attrs:?}}}",
            fmt_range(inline),
            snippet(&inline_text(events, inline)),
        ),
        BlockKind::Paragraph { inline } => format!(
            "Paragraph{{inline={}:{}}}",
            fmt_range(inline),
            snippet(&inline_text(events, inline)),
        ),
        BlockKind::CodeBlock {
            lang,
            fenced,
            body_spans,
        } => {
            let spans: Vec<String> = body_spans
                .iter()
                .map(|r| format!("{}:{}", fmt_range(r), snippet(&src[r.clone()])))
                .collect();
            format!(
                "CodeBlock{{lang={lang:?}, fenced={fenced}, body_spans=[{}], text={}}}",
                spans.join(", "),
                snippet(&code_body_text(body_spans, src))
            )
        }
        BlockKind::List { ordered, start } => {
            format!("List{{ordered={ordered}, start={start:?}}}")
        }
        BlockKind::ListItem { task: None } => "ListItem{task=None}".to_string(),
        BlockKind::ListItem { task: Some(t) } => format!(
            "ListItem{{task=Some(state={:?}, state_at={})}}",
            t.state, t.state_at
        ),
        BlockKind::Quote { alert, alert_title } => {
            format!("Quote{{alert={alert:?}, alert_title={alert_title:?}}}")
        }
        BlockKind::Table { aligns, rows } => {
            let mut s = format!("Table{{aligns={aligns:?}, rows=[");
            for (ri, row) in rows.iter().enumerate() {
                if ri > 0 {
                    s.push_str(", ");
                }
                s.push('[');
                for (ci, cell) in row.iter().enumerate() {
                    if ci > 0 {
                        s.push_str(", ");
                    }
                    write!(s, "{}:{}", fmt_range(cell), snippet(&src[cell.clone()])).unwrap();
                }
                s.push(']');
            }
            s.push_str("]}");
            s
        }
        BlockKind::Html { tag, body_spans } => {
            let spans: Vec<String> = body_spans
                .iter()
                .map(|r| format!("{}:{}", fmt_range(r), snippet(&src[r.clone()])))
                .collect();
            format!(
                "Html{{tag={tag:?}, body_spans=[{}], text={}}}",
                spans.join(", "),
                snippet(&html_body_text(body_spans, src))
            )
        }
        BlockKind::HtmlTable { body_spans, rows } => {
            let spans: Vec<String> = body_spans
                .iter()
                .map(|r| format!("{}:{}", fmt_range(r), snippet(&src[r.clone()])))
                .collect();
            let mut s = format!("HtmlTable{{body_spans=[{}], rows=[", spans.join(", "));
            for (ri, row) in rows.iter().enumerate() {
                if ri > 0 {
                    s.push_str(", ");
                }
                s.push('[');
                for (ci, cell) in row.iter().enumerate() {
                    if ci > 0 {
                        s.push_str(", ");
                    }
                    write!(
                        s,
                        "{}:{}(header={}, align={:?})",
                        fmt_range(&cell.inner),
                        snippet(&html_body_text_in(body_spans, src, &cell.inner)),
                        cell.header,
                        cell.align,
                    )
                    .unwrap();
                }
                s.push(']');
            }
            s.push_str("]}");
            s
        }
        BlockKind::Details {
            open_attr,
            summary,
            glued_body,
        } => {
            let gb = match glued_body {
                Some(r) => format!("Some({}:{})", fmt_range(r), snippet(&src[r.clone()])),
                None => "None".to_string(),
            };
            format!(
                "Details{{open_attr={open_attr}, summary={summary:?}, glued_body={gb}}}"
            )
        }
        BlockKind::ThematicBreak => "ThematicBreak".to_string(),
    }
}

/// Dumps one block and (recursively, indented) its children — the block's own kind, source range,
/// and a truncated preview of its source text (a `CodeBlock`'s `body` snippet, and a
/// `Heading`/`Paragraph`'s own reconstructed inline text, are folded into `fmt_kind` already, since
/// those are the fields the model's whole value proposition rests on; see
/// `crate::preview::markdown::model::BlockKind::CodeBlock`'s doc comment). `events` is `Doc.events`
/// for the whole document `b` came from, threaded through unchanged at every depth.
fn dump_block(
    b: &Block,
    src: &str,
    events: &[(Event<'_>, Range<usize>)],
    depth: usize,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    writeln!(
        out,
        "{indent}{} src={} text={}",
        fmt_kind(&b.kind, src, events),
        fmt_range(&b.src),
        snippet(&src[b.src.clone()])
    )
    .unwrap();
    for c in &b.children {
        dump_block(c, src, events, depth + 1, out);
    }
}

fn dump_case(cfg: &Config, name: &str, raw_src: &str, out: &mut String) {
    let pre_src = pre_src_for(cfg, raw_src);
    let doc = Doc::parse(&pre_src);
    writeln!(out, "=== {name} ===").unwrap();
    writeln!(out, "-- BLOCKS ({}) --", doc.blocks.len()).unwrap();
    for b in &doc.blocks {
        dump_block(b, &pre_src, &doc.events, 0, out);
    }
    writeln!(out).unwrap();
}

/// The full dump for `cfg`, over every case `all_cases()` reports, in that (sorted) order — the
/// exact same corpus/order `md_snapshot_tests`'s own dump covers (see the module doc comment).
fn dump_all(cfg: &Config) -> String {
    let mut out = String::new();
    for (name, src) in all_cases() {
        dump_case(cfg, &name, &src, &mut out);
    }
    out
}

/// The golden snapshot of `Doc::parse`'s output over the full parity corpus, under the default
/// config's preprocessing (front matter/footnotes/inline HTML all on — `pre_src_for`'s gating). See
/// the module doc comment for what this covers and how to regenerate it after an intentional
/// change to the model.
#[test]
fn markdown_model_snapshot_default() {
    assert_matches_snapshot(snapshot_path(), "model", &dump_all(&Config::default()));
}

/// Permanent regression guard, mirroring `md_snapshot_tests`'s own: rendering (here, parsing) the
/// same corpus twice within one process must produce byte-identical output.
#[test]
fn markdown_model_snapshot_is_deterministic_within_a_process() {
    let cfg = Config::default();
    let a = dump_all(&cfg);
    let b = dump_all(&cfg);
    assert_eq!(
        a, b,
        "parsing the golden-snapshot corpus twice in the same process produced different output \
         — some part of Doc::parse is not deterministic"
    );
}

/// Guards `all_cases()` itself, mirroring `md_snapshot_tests`'s own: if the shared corpus-gathering
/// call silently broke, both snapshot tests would still "pass" by comparing an empty dump against
/// an empty snapshot.
#[test]
fn markdown_model_snapshot_corpus_is_not_empty() {
    let cases = all_cases();
    assert!(
        cases.len() > 100,
        "the golden snapshot corpus looks suspiciously small ({} cases) — \
         a corpus-gathering call in `all_cases` probably broke",
        cases.len()
    );
}

// ============================================================================================
// Completeness invariants
// ============================================================================================
//
// Everything above this line checks that `Doc::parse`'s output looks the way it looked last time
// (the golden snapshot) and is internally deterministic — neither of which says anything about
// whether the *content* is actually all there. Two real gaps shipped past both of those, plus the
// model's own `check_invariants` (which only checks range containment/ordering/char-boundary
// well-formedness, not coverage): a loose task-list item's own `Event::TaskListMarker` was silently
// dropped (it sits inside the item's first `Paragraph`, not directly after `Start(Item)`, so the
// original marker lookup never saw it), and a *tight* list item's own inline text — which
// pulldown-cmark reports with no enclosing `Paragraph` at all — produced no `Block` whatsoever, so
// there was nothing downstream could ever read that content from.
//
// The three checks below close that hole structurally: for every one of the 347 cases
// `all_cases()` covers, they read the *real* parser's own event stream independently
// (`scan_reference_events`, driven by `model::parse_options()` — the exact function `Doc::parse`
// itself calls, not a second hand-copied option set) and confirm every real inline-content event,
// task marker, and code-block start the parser reports is actually represented somewhere in
// `Doc::parse`'s own tree. A future change that drops content again — for *any* construct, not just
// the two these two exact gaps happened to involve — fails one of these three, with the specific
// case name, event range, and a short source excerpt, rather than silently shipping.

/// An independent, tree-free reading of the exact same event stream `Doc::parse` walks — computed
/// directly from a fresh `Parser`/`model::parse_options()`, never by inspecting `Doc`'s own output
/// — so the completeness checks below cannot pass merely because a bug in `Doc::parse` and a bug in
/// this scan happen to agree. Reuses the model's own event classifiers (`is_stray_inline_leaf`,
/// `is_unmodeled_container_tag`) rather than a second, hand-copied list of the same `Event`/`Tag`
/// variants, for the same reason `Doc::parse` itself does: two independently maintained
/// classifications of the same event stream eventually drift onto disagreeing about the same event.
/// (`is_inline_start_tag`, the model's third classifier, is not needed here: this scan only ever
/// needs to know whether a `Start`/`End` pair is suppressed, never whether it is specifically
/// *span-level* — the run-grouping distinction that classifier exists for is a `parse_blocks`-only
/// concern, this scan has no runs to group.)
struct ReferenceEvents {
    /// Every leaf inline-content event's own range (`Text`, `Code`, a soft/hard break, ...).
    /// `Event::TaskListMarker` is deliberately **not** included here — see `tasks` below for why.
    inline: Vec<Range<usize>>,
    /// Every `Event::TaskListMarker`'s own range, checked separately from `inline` (rather than
    /// folded into it): a *correctly* modeled document never covers a task marker's own bytes
    /// inside any leaf `Block`'s range at all — they sit in the item's own marker text, outside
    /// whichever `Paragraph` (real or synthetic) its content becomes; see
    /// `crate::preview::markdown::model::Task::state_at`'s doc comment.
    /// `model_covers_every_task_marker_across_the_full_corpus` checks these against
    /// `ListItem.task` instead of leaf-range containment.
    tasks: Vec<Range<usize>>,
    /// Every `Tag::CodeBlock`'s own `Start` range — which, like every block-level tag this file
    /// leans on elsewhere (see `collect_stray_inline_run`'s own doc comment in the model), equals
    /// the construct's whole span, matching its `End`'s range too.
    code_blocks: Vec<Range<usize>>,
}

/// Walks `src` with the model's own parse options, excluding anything nested inside a construct
/// `Doc::parse` deliberately never represents at all — a footnote definition, a definition list, a
/// metadata block (`is_unmodeled_container_tag`, mirroring `parse_container`'s own `other` arm in
/// the model exactly) — precisely as `Doc::parse` itself discards that content. This holds the
/// completeness checks below to what the model actually promises to cover, not to a stricter
/// contract nobody signed up for; in practice, for the always-preprocessed 347-case corpus these
/// checks run over, none of the three ever actually appears (footnotes/definition lists are never
/// enabled by `parse_options`, and front matter is stripped by `pre_src_for` before this ever runs)
/// — this exists for robustness against a corpus case, or a future one, that manages to contain a
/// mid-document YAML-style metadata block anyway (`ENABLE_YAML_STYLE_METADATA_BLOCKS` *is* on).
fn scan_reference_events(src: &str) -> ReferenceEvents {
    let mut out = ReferenceEvents {
        inline: Vec::new(),
        tasks: Vec::new(),
        code_blocks: Vec::new(),
    };
    // `stack.last()` is `true` exactly when the tag most recently opened (and not yet closed) is
    // itself `is_unmodeled_container_tag`, or is nested inside one that is. `Start` pushes one
    // entry per open tag, its matching `End` pops it — mirroring the model's own nesting exactly —
    // so this needs no bookkeeping about *which* tag started the suppression, only whether one did.
    let mut stack: Vec<bool> = Vec::new();
    for (ev, range) in Parser::new_ext(src, model::parse_options()).into_offset_iter() {
        let suppressed_here = stack.last().copied().unwrap_or(false);
        match &ev {
            Event::Start(tag) => {
                if matches!(tag, Tag::CodeBlock(_)) && !suppressed_here {
                    out.code_blocks.push(range.clone());
                }
                stack.push(suppressed_here || is_unmodeled_container_tag(tag));
                continue;
            }
            Event::End(_) => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        if suppressed_here {
            continue;
        }
        match &ev {
            Event::TaskListMarker(_) => out.tasks.push(range),
            other if is_stray_inline_leaf(other) => out.inline.push(range),
            _ => {}
        }
    }
    out
}

/// Every leaf `Block`'s own range in `blocks`, gathered recursively from the whole tree —
/// `BlockKind::Heading`/`Paragraph`/`CodeBlock`/`Table`/`Html`/`ThematicBreak`, matching `Block`'s
/// own doc comment on which kinds have no children ("empty for every leaf kind"). Non-leaf ranges
/// (`List`, `ListItem`, `Quote`) are deliberately excluded — checking against those instead would
/// make `model_covers_every_inline_event_across_the_full_corpus` pass trivially, since a
/// container's own range always spans its descendant content by construction (it has to, to
/// contain its children) *whether or not* the model actually built a leaf block for any of it —
/// exactly the shape of both real gaps this invariant was written to catch.
///
/// `Details` is a container too (it has `children`), but is not excluded the same blanket way:
/// unlike `List`/`Quote` — whose own `.src` is *exactly* the union of their children's ranges, with
/// nothing left over — a `Details` block's own `.src` also covers its opening `<details ...>` tag
/// line (and, when closed, the standalone `</details>` line too), neither of which is represented by
/// any child at all (see `model::fold_details`'s own doc comment: that text becomes `summary`/
/// `open_attr` instead, consumed rather than kept as a modeled leaf). Those tag lines still report
/// real `Event::Html` events this check has to account for, so `details_tag_ranges` reports exactly
/// the two slivers of `.src` outside whatever `children` spans — never the *whole* `.src` (which
/// would reintroduce the same "trivially passes" hole this function's own doc comment above warns
/// against, for the *body* portion `children` is supposed to be covering on its own).
fn leaf_ranges(blocks: &[Block], out: &mut Vec<Range<usize>>) {
    for b in blocks {
        let is_leaf = matches!(
            b.kind,
            BlockKind::Heading { .. }
                | BlockKind::Paragraph { .. }
                | BlockKind::CodeBlock { .. }
                | BlockKind::Table { .. }
                | BlockKind::Html { .. }
                | BlockKind::HtmlTable { .. }
                | BlockKind::ThematicBreak
        );
        if is_leaf {
            out.push(b.src.clone());
        }
        if matches!(b.kind, BlockKind::Details { .. }) {
            out.extend(details_tag_ranges(b));
        }
        leaf_ranges(&b.children, out);
    }
}

/// The parts of a `Details` block's own `.src` that its `children` do **not** span — its opening
/// `<details ...>` tag's own line(s), and, when closed, the standalone `</details>` line — see
/// `leaf_ranges`'s own doc comment for why these need reporting here at all. Relies on `children`
/// being in non-decreasing, non-overlapping source order (a model invariant, checked elsewhere by
/// `model::tests::check_invariants`) to compute the two slivers as plain range subtraction, without
/// needing to know line boundaries itself.
fn details_tag_ranges(b: &Block) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    match (b.children.first(), b.children.last()) {
        (Some(first), Some(last)) => {
            if b.src.start < first.src.start {
                out.push(b.src.start..first.src.start);
            }
            if last.src.end < b.src.end {
                out.push(last.src.end..b.src.end);
            }
        }
        // No children at all (an empty, self-closing-ish `<details></details>` with nothing in
        // between, or one whose own tag line already carried everything — see `fold_details`'s own
        // "nothing follows this open tag" branch) — the *whole* range is tag text.
        _ => out.push(b.src.clone()),
    }
    out
}

/// Every `Task::state_at` recorded anywhere in `blocks`, gathered recursively.
fn task_state_positions(blocks: &[Block], out: &mut Vec<usize>) {
    for b in blocks {
        if let BlockKind::ListItem { task: Some(t) } = &b.kind {
            out.push(t.state_at);
        }
        task_state_positions(&b.children, out);
    }
}

/// Every `CodeBlock`'s own `src` range recorded anywhere in `blocks`, gathered recursively —
/// including one nested inside a `Quote` (`BlockKind::CodeBlock.body_spans`'s own doc comment in
/// the model documents this as a deliberate superset of `parser_code_blocks`; this completeness
/// check holds `Doc::parse` to that same, wider promise, not to `parser_code_blocks`'s narrower
/// one).
fn code_block_srcs(blocks: &[Block], out: &mut Vec<Range<usize>>) {
    for b in blocks {
        if let BlockKind::CodeBlock { .. } = &b.kind {
            out.push(b.src.clone());
        }
        code_block_srcs(&b.children, out);
    }
}

/// A short, char-boundary-safe excerpt of `src[r]` for a violation message — long spans (a whole
/// paragraph, a big code block) would otherwise make an assertion failure unreadable.
fn excerpt(src: &str, r: &Range<usize>) -> String {
    const CAP: usize = 60;
    let slice = &src[r.clone()];
    if slice.chars().count() <= CAP {
        format!("{slice:?}")
    } else {
        let truncated: String = slice.chars().take(CAP).collect();
        format!("{truncated:?}…")
    }
}

/// (i) Inline exhaustiveness: every leaf inline-content event the real parser reports for a case
/// (via `scan_reference_events`) has its byte range contained in some *leaf* `Block`'s own range
/// (`leaf_ranges`) — not merely inside some `Block` in the tree, which (see `leaf_ranges`'s own doc
/// comment) any container's range already trivially satisfies whether or not its content actually
/// became a modeled leaf. This is the check that catches gap B: a tight list item's own inline text
/// used to produce no leaf `Block` at all, so its `Text`/`Code`/... events landed inside the (non-
/// leaf) `ListItem`'s own range but nowhere else — exactly the shape this check is built to reject.
#[test]
fn model_covers_every_inline_event_across_the_full_corpus() {
    let cfg = Config::default();
    let mut violations = Vec::new();
    let mut total_checked = 0usize;
    for (name, raw) in all_cases() {
        let pre_src = pre_src_for(&cfg, &raw);
        let doc = Doc::parse(&pre_src);
        let mut leaves = Vec::new();
        leaf_ranges(&doc.blocks, &mut leaves);
        let reference = scan_reference_events(&pre_src);
        for r in &reference.inline {
            total_checked += 1;
            let covered = leaves.iter().any(|l| l.start <= r.start && r.end <= l.end);
            if !covered {
                violations.push(format!(
                    "{name}: inline event {r:?} {} is not covered by any leaf block",
                    excerpt(&pre_src, r)
                ));
            }
        }
    }
    assert!(
        total_checked > 500,
        "the corpus's inline-event count looks suspiciously small ({total_checked}) — \
         a corpus-gathering call probably broke"
    );
    assert!(
        violations.is_empty(),
        "{} inline event(s) not covered by any leaf block:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// (ii) Task exhaustiveness: every `Event::TaskListMarker` the real parser reports for a case has a
/// matching `Task` recorded on some `ListItem` in the model, at the exact same position (compared
/// as sorted `Vec`s of `state_at`, so this also flags a spurious *extra* marker the model invented,
/// not just a missing one). This is the check that catches gap A: a loose task item's marker used
/// to be silently dropped because the original marker lookup only ever peeked directly after
/// `Start(Item)`, never inside the item's own first `Paragraph`, which is where a loose item's
/// marker actually sits.
#[test]
fn model_covers_every_task_marker_across_the_full_corpus() {
    let cfg = Config::default();
    let mut violations = Vec::new();
    let mut total_checked = 0usize;
    for (name, raw) in all_cases() {
        let pre_src = pre_src_for(&cfg, &raw);
        let doc = Doc::parse(&pre_src);
        let mut recorded = Vec::new();
        task_state_positions(&doc.blocks, &mut recorded);
        recorded.sort_unstable();
        // `[` at `r.start`, the state char at `r.start + 1`, `]` at `r.start + 2` — see
        // `Task::state_at`'s own doc comment for why this is exact, not an approximation.
        let mut expected: Vec<usize> = scan_reference_events(&pre_src)
            .tasks
            .iter()
            .map(|r| r.start + 1)
            .collect();
        expected.sort_unstable();
        total_checked += expected.len();
        if recorded != expected {
            violations.push(format!(
                "{name}: expected task marker positions {expected:?}, model recorded {recorded:?}"
            ));
        }
    }
    assert!(
        total_checked > 20,
        "the corpus's task marker count looks suspiciously small ({total_checked}) — \
         a corpus-gathering call probably broke"
    );
    assert!(
        violations.is_empty(),
        "{} case(s) with missing or extra task marker(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// (iii) Code-block exhaustiveness: every `Tag::CodeBlock`'s own `Start` the real parser reports
/// for a case is represented as a `CodeBlock` block somewhere in the model, at a range containing
/// it (`code_block_srcs`) — including one nested inside a block quote (a deliberate superset of
/// what the render pipeline's own `parser_code_blocks` counts; see `code_block_srcs`'s own doc
/// comment). No known gap involves code blocks specifically — both real gaps in this module's
/// history were about `Paragraph`/`Task` coverage — but this closes the same class of hole for the
/// third leaf kind this model's own doc comment singles out as its whole reason for existing
/// (`BlockKind::CodeBlock.body_spans`'s doc comment), so a future regression there gets caught the
/// same structural way rather than only by the golden snapshot noticing *something* changed.
#[test]
fn model_covers_every_code_block_start_across_the_full_corpus() {
    let cfg = Config::default();
    let mut violations = Vec::new();
    let mut total_checked = 0usize;
    for (name, raw) in all_cases() {
        let pre_src = pre_src_for(&cfg, &raw);
        let doc = Doc::parse(&pre_src);
        let mut modeled = Vec::new();
        code_block_srcs(&doc.blocks, &mut modeled);
        let reference = scan_reference_events(&pre_src);
        for r in &reference.code_blocks {
            total_checked += 1;
            let covered = modeled.iter().any(|m| m.start <= r.start && r.end <= m.end);
            if !covered {
                violations.push(format!(
                    "{name}: code block start {r:?} {} is not modeled",
                    excerpt(&pre_src, r)
                ));
            }
        }
    }
    assert!(
        total_checked > 20,
        "the corpus's code block count looks suspiciously small ({total_checked}) — \
         a corpus-gathering call probably broke"
    );
    assert!(
        violations.is_empty(),
        "{} code block(s) not modeled:\n{}",
        violations.len(),
        violations.join("\n")
    );
}
