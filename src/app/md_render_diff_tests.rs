//! Diff harness for the block-model renderer (`crate::preview::markdown::render`), over the exact
//! same parity corpus the golden render snapshot (`md_snapshot_tests`) already covers. This is
//! **not** a pass/fail equivalence check between the two renderers in general — `render::render_doc`
//! only covers `Heading`/`Paragraph`/`ThematicBreak`/`List`/`ListItem`/`Quote{alert: None}`/
//! `CodeBlock` so far (see that module's own doc comment on scope), so most cases are expected to
//! fall outside its coverage entirely. What this module checks is narrower and more honest: for every
//! case whose *entire* top-level block set is one this stage claims to support, does the new
//! renderer's fully post-processed output match the production pipeline's, line for line, span for
//! span, style for style?
//!
//! ## Three kinds of "expected mismatch" — and the one kind that is not
//!
//! A mismatch this module observes among the cases this stage claims to support always falls into
//! exactly one of three tracked lists, or it is a **real bug in `render_doc`** to be fixed, not
//! catalogued. The three lists are not interchangeable — mixing them together would hide the actual
//! reason production doesn't match, which is the whole point of separating them:
//!
//! * [`KNOWN_MISMATCHES`] — **this stage genuinely has not implemented the shape yet.** The gap is in
//!   `render_doc` itself; a future stage closes it, and the day it does, this list shrinks. **Never**
//!   a place to list something `render_doc` gets *worse* than production — a mismatch where `render_doc`
//!   produces something visibly broken (an empty line where content belongs, dropped structure, a
//!   panic) compared to what production already shows is not "a gap in this stage's coverage," it is a
//!   bug, and belongs fixed, not catalogued (see this section's own math history below for exactly this
//!   mistake, caught and corrected).
//! * [`INTENDED_IMPROVEMENTS`] — **`render_doc` is *better* than production on purpose**, and is not
//!   expected to ever start matching it: production has a real, confirmed bug (an upstream crash, a
//!   glued-together row that drops a code block's own header, ...) that `render_doc`'s own architecture
//!   structurally cannot reproduce even if it tried. See that list's own doc comment for the mechanism
//!   behind each entry.
//! * [`BEHAVIOR_DECISIONS`] — **the two sides disagree about what CommonMark itself says**, and neither
//!   is a bug: `render_doc` follows the real grammar (`Doc::parse`, straight off pulldown-cmark), while
//!   production's own hand-written pre-pass is deliberately *more permissive* than the spec, for real,
//!   still-useful reasons (see that list's own doc comment). Closing this gap is a **product decision**
//!   for whoever wires `render_doc` into a real preview path — not something to resolve inside this
//!   harness by picking a side.
//!
//! ## Standalone block-level images and mermaid fences (`KNOWN_MISMATCHES`)
//!
//! Two shapes this stage's own scope (see `render.rs`'s own module doc comment) does not cover at all
//! yet — see [`render_case_new`]'s callers for where the first one comes from:
//!
//! * **Standalone block-level images.** A paragraph whose entire content is `![alt](url)` is
//!   intercepted by the production pipeline *before* tui-markdown ever sees it (`split_block_parts`),
//!   becoming a reserved-row image placement — not "alt text as a plain paragraph", which is what
//!   `render::render_doc`'s inline-only `Image` handling produces (see that module's own doc comment:
//!   block-level image extraction is out of scope for this stage).
//! * **A fenced `mermaid` code block.** Production intercepts one *before* tui-markdown ever sees it
//!   too (the same `split_segments`-family interception the previous bullet describes, just for a
//!   different tag), substituting a diagram placement; `render_doc`'s `render_code_block` has no such
//!   interception at all — a `mermaid`-tagged fence is drawn as an ordinary, highlighted code block,
//!   exactly matching `model::BlockKind::CodeBlock`'s own doc comment ("the parent module's own choice
//!   to render some of those as diagrams instead of code is a rendering decision, and this is a
//!   structural model of the document, not of the screen"). Only when the diagram substitution
//!   actually *fires* in production is this a mismatch — a mermaid fence indented past its own
//!   container's content column, which production therefore also draws as plain code, still matches
//!   (module scope, not a gap in `render_code_block`'s own content reconstruction). Both of these are
//!   closed the same way, by a **future** stage: the one that teaches `render_doc` to draw konoma's own
//!   remaining *structural* extras straight from the model — tables, block images, mermaid/math
//!   diagrams — the same family `markdown.rs`'s own `split_segments`/`split_tables`/`split_math`
//!   intercepts today, ahead of tui-markdown, for production. (GitHub alerts and `<details>` blocks,
//!   the other two members of that family, are no longer in this category — see `render.rs`'s own
//!   module doc comment for `render::render_alert_from_model`/`render::render_details_from_model`.)
//!
//! ## Math (closed — history, not a live gap)
//!
//! Lifted LaTeX math (`$…$`/`$$…$$`/`\(…\)`/`\[…\]`) used to be [`KNOWN_MISMATCHES`] for exactly the
//! three `code_span_corpus` cases that combine it with an indented code block — **not** because
//! `render_doc`'s own math-lifting architecture was incomplete, but because of two real, pre-existing
//! bugs in `render::render_paragraph_math`/`walk_inline_math`, present since that function was added
//! (`c39dfe3`) and confirmed with **no `CodeBlock` involved at all** (minimal repro: `"\[y\]\n"` alone,
//! and `"$$y$$\n"` alone) — simply never exercised by this harness before those three cases existed,
//! because every corpus case combining math with an indented code block was previously `Unsupported`
//! outright:
//!
//! 1. `backslash_math_opener`'s own detection read the *gap* between the previous event's end and this
//!    one's start — but `walk_inline_math` started each paragraph's own walk with `prev_end: None`, so a
//!    backslash-math expression (`\(…\)`/`\[…\]`) that is the very *first* event of its own paragraph had
//!    no previous event, in that paragraph's own local walk, to compute a gap from — it could never be
//!    detected as an opener at all and fell through as literal text (`"[y]"`, not lifted).
//! 2. `render_paragraph_math`'s own leading blank-line convention (conditional-if-`needs_newline` +
//!    unconditional, mirroring `start_paragraph`) assumed the paragraph's own first content would reach
//!    `w.lines` via `push_span` (which fills the row that convention just opened) — true for plain text,
//!    but a lift goes straight to `self.lines.extend(...)` instead (`render_math_slot`, deliberately
//!    bypassing `push_line`/`push_span` — see `Writer`'s own doc comment on `after_math`), so that opened
//!    row was left behind, genuinely empty, forever. Confirmed even for `$$…$$` math (not affected by
//!    bug 1 at all, since `scan_inline_math` finds it by substring, not by gap) — `"$$y$$\n"` alone
//!    rendered with one *extra*, orphaned blank row ahead of the placeholder that production never shows.
//!
//! **Both are real bugs**, not gaps in this stage's own scope — `render_doc`'s math-lifting output was
//! genuinely *worse* than production's (an empty line where a rendered expression belongs; a lifted
//! backslash expression left as unrendered literal text), which is precisely the shape
//! [`KNOWN_MISMATCHES`]'s own doc comment now warns against cataloguing rather than fixing. Both are
//! fixed, constructively, in `render.rs` itself:
//!
//! 1. `walk_inline_math` now takes a `block_start: usize` parameter — the byte position *conceptually*
//!    just before the walk's own first event (the owning `Block`'s own `src.start`) — and seeds
//!    `prev_end` with it instead of `None`, so a leading backslash escape computes the same non-empty
//!    gap for the walk's own first event that it already did for every later one (see that parameter's
//!    own doc comment on `walk_inline_math` for the exact mechanism, and why this closes the bug for a
//!    top-level or loose-list paragraph but not a tight list item's own synthetic first child, safely).
//! 2. The leading-blank-line convention is now **deferred**, not eager: `render_paragraph_math` records
//!    that a paragraph-start is *owed* (`Writer::pending_para_start`) rather than opening it up front,
//!    and whichever of the paragraph's own first rendered content resolves it — `open_pending_para_start`
//!    for real text (opens the same two-part convention, now lazily), `drop_pending_para_start` for a
//!    lifted expression (drops it, unopened) — matching production's own confirmed behavior exactly: a
//!    lifted expression that is a paragraph's own entire, only content gets **no** leading blank line at
//!    all, regardless of what preceded it (see `pending_para_start`'s own doc comment for the full
//!    mechanism and the production traces that pin this).
//!
//! All three `code_span_corpus` math cases match production exactly now (regression tests: `render.rs`'s
//! own `mod tests`, `math_lift_that_is_a_paragraphs_entire_content_gets_no_leading_blank_line`/
//! `backslash_math_opener_detects_a_paragraphs_own_first_event`), so this category no longer appears in
//! [`KNOWN_MISMATCHES`] at all — recorded here as history (why these bugs existed, how they were closed)
//! rather than removed outright, since a *future* regression back to the old, buggy shape should read as
//! "this got worse," not as a mystery.
//!
//! ## Inline HTML that interrupts a paragraph (`BEHAVIOR_DECISIONS`)
//!
//! **Inline HTML that interrupts a paragraph in a way production's own hand-written `split_html_blocks`
//! pre-pass recognizes, but CommonMark's real HTML-block grammar (and so `Doc::parse`) does not.**
//! `<span>x</span>` on its own line, directly under a paragraph with no blank line, is *not* one of
//! CommonMark's HTML-block-starting tags (not in the type-6 list, and a type-7 block explicitly cannot
//! interrupt a paragraph) — pulldown-cmark reports it as ordinary `Event::InlineHtml` *inside* that same
//! paragraph (confirmed directly against the raw event stream), which is exactly what `Doc::parse` sees
//! too — `render_doc` is CommonMark-compliant here. Production's *own*, more permissive, hand-written
//! `split_html_blocks` pre-pass intercepts it anyway, *before* the text ever reaches tui-markdown,
//! rendering it as its own separate (tag-stripped) line via `render_html_block` — a whole extra
//! pre-processing pass `render.rs`'s `Doc::parse`/`render_doc` has no equivalent of at all (nor of
//! `split_tables`/`split_alerts`/`split_details`/`split_math`, the same family — see `markdown.rs`'s own
//! `split_segments`).
//!
//! This is **not** "a gap in this stage" the way the images/mermaid section above is: `render_doc` is
//! not missing an implementation of CommonMark's own HTML-block grammar here, it is *already correct*
//! against that grammar — production is the one that deviates, on purpose, and for a real reason:
//! `split_html_blocks`'s own permissiveness is exactly what lets konoma rescue a `<details>` block or a
//! centered `<div>` banner that a strict CommonMark reading would otherwise leave as raw, tag-cluttered
//! text (see `markdown.rs`'s own doc comment on that pre-pass for the motivating cases). Picking either
//! side inside this harness would be wrong: matching production exactly would mean deliberately
//! re-implementing its *deviation* from the spec, not closing a gap; matching the spec exactly would
//! change how real `<details>`/centered-banner Markdown (README files included) actually renders once
//! `render_doc` is wired into a real preview path. That choice — and whether `render_doc` needs its own
//! `split_html_blocks`-equivalent pass before it can be, or ships as a deliberate, documented behavior
//! change — belongs to whoever makes that wiring decision, not to this diff harness; tracked in
//! [`BEHAVIOR_DECISIONS`] rather than [`KNOWN_MISMATCHES`] for exactly that reason.
//!
//! ## A GitHub alert's own flat, pre-parse extraction can change a surrounding list's tight/loose
//! ## shape (`BEHAVIOR_DECISIONS`)
//!
//! **A GitHub alert (`> [!TYPE]`) sitting inside a list item, alongside other sibling content in
//! that same item, can make production render the item *tight* while `render_doc` — following real
//! CommonMark structure — renders it *loose*, and neither side is a bug.** `markdown.rs`'s own
//! `split_alerts` runs over the **whole** top-level segment's raw text, line by line, *before*
//! tui-markdown (or `Doc::parse`) ever sees any of it — and it has no concept of list-item nesting at
//! all: it simply deletes the `>`-prefixed alert lines from the middle of the text, wherever they
//! happen to sit. For `"- item\n\n  > [!NOTE]\n  > ```sh\n  > echo hi\n  > ```\n"`, that leaves
//! tui-markdown with effectively `"- item\n\n"` to parse — a **single-item list whose only block is
//! one paragraph**, which CommonMark's own tight/loose rule makes tight (no second, blank-line-separated
//! block remains inside the item once the alert is gone) — so `"item"`'s own text lands on the very
//! same row as the `"- "` marker (confirmed directly: `code_corpus`'s own "an alert inside a list
//! item" case, `old[0] = ["- ", "item"]`, one line). `Doc::parse` never performs this kind of
//! string-level deletion at all (see the module doc comment's own "no re-parsing" invariant) — it
//! reads the **real** source structure, where the item genuinely *does* contain two block-level
//! children (the paragraph and the alert quote) separated by a blank line, which CommonMark's own
//! rule makes **loose** — so `render_item`'s own loose-first convention puts `"item"` on its own row,
//! one below the marker.
//!
//! This is not "a gap in this stage" (see [`KNOWN_MISMATCHES`]: `render_doc` already draws the alert
//! itself correctly, via `render::render_alert_from_model` — the *list item surrounding it* is what
//! differs) and not an improvement either (see [`INTENDED_IMPROVEMENTS`]: production's own shape here
//! is not a bug, it is simply a side effect of extracting the alert before any list parsing happens
//! at all) — it is the identical kind of disagreement the "Inline HTML that interrupts a paragraph"
//! section above already tracks here, just triggered by `split_alerts`'s own flat extraction instead
//! of `split_html_blocks`'s own permissive one. Resolving it — teaching `render_doc` to mimic
//! production's own pre-parse deletion (which would mean building a *second*, string-level pass that
//! violates this file's whole "read structure once, from the real parser" premise) or leaving
//! production to catch up to real CommonMark structure instead — is a product decision, not something
//! this harness can settle by picking a side.
//!
//! ## Two categories closed structurally, not merely observed
//!
//! Two categories used to apply here — both artifacts of `render::render_doc` re-parsing each
//! `Heading`/`Paragraph`'s own byte range in isolation rather than reading from a single whole-document
//! parse: a **cross-block reference-style link** (`[text][ref]` whose `[ref]: url` definition lives in a
//! different top-level block) could never resolve that way, and *any* CommonMark construct whose
//! classification depends on context outside its own block was, in principle, at risk of coming out
//! differently when re-parsed alone. `render.rs` no longer re-parses anything at all (see its own module
//! doc comment) — every block's own inline events are read straight out of the one event stream
//! `Doc::parse` recorded for the *whole* document (`model::Doc.events`) — so both categories are gone
//! structurally, not merely unobserved; see `crate::preview::markdown::inline_corpus`'s own doc comment
//! for how the reference-link case specifically was proven to resolve, and
//! [`markdown_render_diff_report`]'s own numbers below for the (lack of) fallout from the second,
//! broader category.
//!
//! A *third* former category, **inline LaTeX math**, is gone the same structural way as of
//! `render::render_paragraph_math` (see that function's own doc comment): `render_doc` now takes the
//! same `math_slot`/`math_on` arguments `render_markdown_with_images` does, reusing that function's own
//! `scan_inline_math`/`math_url` (no second math detector) to lift `$…$`/`$$…$$`/`\(…\)`/`\[…\]` out of a
//! top-level paragraph's own text onto its own placeholder line(s), matching production line for line,
//! span for span — see that function's own doc comment for the "before text / math placeholder / after
//! text" mechanism and why it needs to reproduce a *fresh reparse boundary* at every lift point (not
//! merely "insert a line"); the two bugs the "Math" section above documents were narrower gaps *within*
//! that otherwise-working mechanism, now closed, not a regression of this category as a whole.
//!
//! Every mismatch this module actually observes is recorded in [`KNOWN_MISMATCHES`]/
//! [`INTENDED_IMPROVEMENTS`]/[`BEHAVIOR_DECISIONS`], by case name, filled in from a real run of this
//! harness (not asserted to be empty from the start — see the task this module was built for).
//! [`markdown_render_diff_report`] fails only when a *new*, previously unlisted mismatch appears among
//! the cases this stage claims to support — a regression in `render::render_doc` itself, not a gap this
//! stage already knows about and accepts, and not one of the other two lists either (see each one's own
//! doc comment — three genuinely different kinds of expected difference: a gap, an improvement, and an
//! unresolved spec-vs-production disagreement).

use std::fmt::Write as _;

// Brings in the same `app`-and-descendants scope `md_snapshot_tests` itself relies on (`Config`,
// `Line`/`Span`, and the `md_text` free functions `collapse_links`/`autolink_bare_urls`/
// `substitute_emoji` glob-imported into `app`'s own scope by `app.rs`'s `use md_text::*;`) — see that
// module's own doc comment for the precedent.
use super::md_snapshot_tests::{all_cases, pre_src_for, render_case, SNAPSHOT_WIDTH};
use super::*;

/// **This stage genuinely has not implemented the shape yet.** Cases (by the same `"corpus: name"`
/// label `all_cases()` reports) whose entire top-level block set is one `render::render_doc` claims to
/// support (`unsupported` is empty), but whose fully post-processed output still does not match the
/// production pipeline's, and where that mismatch is a real, still-open **gap in this stage's own
/// implementation** — never a place to list `render_doc` producing something visibly *worse* than
/// production (that is a bug to fix, not a mismatch to catalogue; see the module doc comment's own
/// "Math (closed — history, not a live gap)" section for a concrete case that used to be miscategorized
/// here exactly that way, before the underlying bugs were fixed constructively). Not the same kind of
/// expected difference as [`INTENDED_IMPROVEMENTS`] (`render_doc` deliberately, permanently *better*
/// than production) or [`BEHAVIOR_DECISIONS`] (the two sides disagree about what CommonMark itself
/// says, and neither is wrong) — see each list's own doc comment. Filled in from an actual run of
/// [`markdown_render_diff_report`] (see its own `--nocapture` output), not asserted empty from the
/// start: most of this stage's job *is* measuring how many of these there are, honestly, not hiding
/// them behind a passing test.
///
/// The one remaining entry falls into the mermaid-substitution gap the module doc comment's own
/// "Standalone block-level images and mermaid fences" section documents in full — closed by a future
/// stage, the one that teaches `render_doc` to draw konoma's own structural extras (tables, alerts,
/// `<details>`, images, mermaid/math diagrams) straight from the model.
const KNOWN_MISMATCHES: &[&str] = &[
    // Mermaid substitution (production intercepts the fence before tui-markdown ever sees it;
    // `render_code_block` has no such interception — draws it as an ordinary code block instead).
    "code_corpus: a mermaid fence at an ordered item's content column is diverted to a diagram",
];

/// **The two sides disagree about what CommonMark itself says, and neither is a bug.** Cases whose
/// fully post-processed output does not match production because `render_doc` follows the real
/// CommonMark grammar (straight off `Doc::parse`/pulldown-cmark) while production's own hand-written
/// pre-pass is *deliberately* more permissive than the spec, for a real, still-useful reason of its
/// own — not a gap in this stage (see [`KNOWN_MISMATCHES`] for that: something `render_doc` has not
/// implemented yet) and not an improvement either (see [`INTENDED_IMPROVEMENTS`]: production having a
/// confirmed bug `render_doc`'s own architecture cannot reproduce). Resolving an entry here is a
/// **product decision**, not something this harness can settle by picking a side — see each entry's
/// own doc comment for exactly what production would lose by matching the spec exactly, and vice
/// versa. Kept separate from both other lists for the identical reason they are kept separate from each
/// other: conflating "not implemented yet" with "the spec itself is ambiguous about which side is
/// right" would hide which of the two questions actually needs answering before `render_doc` could ever
/// replace production.
const BEHAVIOR_DECISIONS: &[&str] = &[
    // `split_html_blocks` interception (production's own hand-written pre-pass recognizes
    // `<span>x</span>` as its own segment, rescuing constructs like `<details>` or a centered `<div>`
    // banner that a strict CommonMark reading would otherwise leave as raw, tag-cluttered text;
    // CommonMark's real grammar — and so `Doc::parse` — does not, since a type-7 HTML block can never
    // interrupt a paragraph). See the module doc comment's own "Inline HTML that interrupts a
    // paragraph" section for the full trade-off.
    "code_corpus: an indented line swallowed by an inline-tag block that interrupts a paragraph",
    // `split_alerts`'s own flat, pre-parse deletion of a `> [!TYPE]` alert's lines changes the
    // surrounding list item's own tight/loose shape before tui-markdown ever sees it (production:
    // tight, one block left in the item; `render_doc`: loose, the real two-block structure). See the
    // module doc comment's own "A GitHub alert's own flat, pre-parse extraction..." section.
    "code_corpus: an alert inside a list item",
];

/// **`render_doc` is better than production on purpose, and is not expected to ever start matching
/// it.** Cases whose fully post-processed output does not match production **on purpose** — not a gap
/// in this stage (see [`KNOWN_MISMATCHES`] for that: something `render_doc` has not implemented yet)
/// and not an unresolved spec-vs-production disagreement either (see [`BEHAVIOR_DECISIONS`]: neither
/// side is a bug) — but this stage's own *improvement* over a real upstream crash the production
/// pipeline only partially recovers from. Kept in a list separate from `KNOWN_MISMATCHES`, deliberately,
/// because the two are not the same kind of "expected mismatch": a `KNOWN_MISMATCHES` entry is something
/// to eventually close (`render_doc` still has a gap); an `INTENDED_IMPROVEMENTS` entry is not expected
/// to *ever* start matching — the day it does, something regressed `render_doc` back down to
/// reproducing production's own bug, not fixed a gap.
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
///
/// ## `render_code_block`'s own two improvements (added for this task's own `CodeBlock` support)
///
/// Every remaining entry below falls into one of two further categories, both real, upstream defects
/// in tui-markdown's own per-`Event::Text` rendering that `render_code_block` structurally cannot
/// reproduce (nor should it — see each one's own explanation) because it never asks tui-markdown to
/// render a code block at all, drawing it straight from the model instead
/// (`model::code_body_text(body_spans, src)`; see that function's own doc comment).
///
/// **Category 1 — a code block's own multi-line content, correctly kept one row per source line.**
/// Real tui-markdown's `text()` only inserts a fresh row *within* the physical lines of a single
/// `Event::Text` payload (`text.lines().with_position()`) — it does **not** insert one *between* two
/// separate `Event::Text` calls unless `needs_newline` happens to be `true` going in, which it never is
/// mid-code-block (`end_codeblock` alone sets it, and only once, at the very end). Whenever
/// pulldown-cmark reports a code block's content as more than one `Event::Text` — a continuation line's
/// own share of a list item's or block quote's indentation sits *between* two spans it never reports as
/// text at all (confirmed via `model::BlockKind::CodeBlock.body_spans`'s own doc comment, the same
/// reasoning `body_spans` itself is a `Vec<Range<usize>>` rather than one `Range` for), a CRLF line
/// ending, two indented chunks CommonMark glues across a blank line, or (the shape that actually
/// motivated this whole refactor) a fence nested inside a list item at all — every event after the
/// first lands glued onto the *previous* event's own row (`"line oneline two"`, `"aaabbb"`, even
/// `` "```rustfn a(){}```" `` — a fence-lookalike's own literal backtick text glued the identical way).
/// `render_code_block` reconstructs the block's own content directly from `body_spans` and splits it
/// on `'\n'` (see that function's own doc comment on "Content: reconstructed from `body_spans`, never
/// from a rendered run") — one row per source line, always, matching what `y c` (which copies the
/// file's own bytes, not the rendered screen) already gave a reader, and what a human reading the
/// *source* would expect.
///
/// One entry in this category is not, on its own, a multi-line body — `"KNOWN FAILURE: list item fence
/// glued to a following inline-code paragraph"` and its two siblings are the *header-dropping*
/// consequence `code_corpus`'s own doc comment (next to those three cases) describes in full: gluing a
/// **following** paragraph's own leading inline code span onto the fence's closing row breaks
/// `flush_code_run`'s "last row is fence characters only" check, so production draws *no header at all*
/// for that block. `render_code_block` draws the header unconditionally, straight from the model, with
/// no such check to break — and `render_bare_paragraph`'s own fix (see that function's own doc comment)
/// keeps the following paragraph itself from gluing onto the block's own last row either. Two defects,
/// one shared root cause, one shared fix.
///
/// **Category 2 — a code block nested inside a `Quote`, now drawn instead of left raw.** Production's
/// own `flush_code_run` looks for a *literal* `` "```" `` (optionally indented) as a run's own first
/// rendered row (`opening_trimmed.starts_with("```")`) — but tui-markdown's real blockquote handling
/// prepends `"> "` to *every* line while a quote is open, fence markers included, so a quote-nested
/// fence's own opening row reads `` "> ```" ``, never matches, and the whole run — opener, body, and
/// closer — is handed back completely undecorated (see `flush_code_run`'s own doc comment, and the
/// `code_corpus` comment directly above its "a plain (non-alert) block quote wrapping a fence" case).
/// `render_code_block` has no rendered-text boundary to mis-detect in the first place — it draws
/// directly from a `model::BlockKind::CodeBlock` node regardless of how many `Quote`s it sits inside,
/// which is exactly the "intentional superset" that field's own doc comment describes, made real now
/// that `contains_unsupported` no longer excludes `CodeBlock` from a `Quote`'s own subtree (see that
/// function's own doc comment). The quote's own `"> "` prefix still applies, correctly, to every row of
/// the resulting header/body/padding band (`Writer::push_line`, unchanged) — production simply never
/// gets far enough to draw a band there at all.
const INTENDED_IMPROVEMENTS: &[&str] = &[
    "list_corpus: task item inside a loose list",
    "task_corpus: a real task at a list item's own indentation is still a real task",
    // ---- Category 1: multi-line code-block content, one row per source line (see above) ----
    "code_corpus: KNOWN FAILURE: list item fence glued to a following inline-code paragraph (no blank line) drops the header",
    "code_corpus: KNOWN FAILURE: same glue in a bullet list item",
    "code_corpus: KNOWN FAILURE: same glue, with an unrelated real fence following later in the document",
    "code_corpus: a checkbox after a fence whose closing line has trailing text (list ended by a paragraph)",
    "code_corpus: a checkbox after a fence whose closing line has trailing text (tight list)",
    "code_corpus: a details block after a fence whose closing line has trailing text",
    "code_corpus: a fence-lookalike indented 4 columns is the literal content of an indented code block",
    "code_corpus: a heading and a real fence after a fence whose closing line has trailing text",
    "code_corpus: an alert after a fence whose closing line has trailing text",
    "code_corpus: a mermaid fence indented 4 columns is left in the text and drawn as ordinary code",
    "code_corpus: a shorter closing line does not close a longer fence",
    "code_corpus: a tilde closing line does not close a backtick fence",
    "code_corpus: crlf indented block",
    "code_corpus: fence-lookalike 4 columns past a bullet item's content column is indented code",
    "code_corpus: indented block at doc start",
    "code_corpus: indented block whose content is a GFM table stays literal code",
    "code_corpus: indented block whose content is an alert header stays literal code",
    "code_corpus: indented block whose first line is plain and a later line is tag-shaped",
    "code_corpus: indented code inside a bullet item (content column + 4)",
    "code_corpus: list item fence with a two-line body (count parity only — content is checked elsewhere)",
    "code_corpus: standalone indented block",
    "code_corpus: two chunks glued by a blank separator into one code block",
    "code_span_corpus: indented code block opening with a closing tag",
    "code_span_corpus: indented code block opening with a rewritable tag",
    "code_span_corpus: indented code block opening with a tag with attributes",
    "code_span_corpus: indented code block opening with an HTML comment",
    "code_span_corpus: indented code block opening with an autolink",
    "code_span_corpus: indented code block opening with cjk in angle brackets",
    "code_span_corpus: two indented chunks separated by a blank line stay one literal block",
    "preprocess_corpus: def carrying a fence",
    "preprocess_corpus: def carrying a list",
    "task_corpus: task-lookalike inside a fence-lookalike top-level indented code block",
    // ---- Category 2: a code block nested inside a Quote, now decorated (see above) ----
    "code_corpus: a plain (non-alert) block quote wrapping a fence is not detected by either side",
    "code_corpus: block quote nested inside a list item, wrapping a fence",
    "code_corpus: block quote nested inside a list item, wrapping an indented block",
    "code_corpus: fence inside a plain block quote draws no header (tui-markdown prefixes every line)",
    "code_corpus: list item fence nested inside a plain block quote, with an inline-code continuation — masked by the pre-existing block-quote non-detection, not the glue defect",
    "code_span_corpus: indented code block inside a blockquote",
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
/// when a case not already in [`KNOWN_MISMATCHES`], [`INTENDED_IMPROVEMENTS`], *or*
/// [`BEHAVIOR_DECISIONS`] mismatches; a case in any of the three that now matches is reported (via
/// `--nocapture`) as a stale entry, not a failure — this stage's job is to measure honestly, and a
/// shrinking mismatch list is progress, not something to guard against. All three lists are tracked
/// (and reported) separately throughout — see each one's own doc comment for why conflating them would
/// hide the distinction that is the whole point of having three: a gap to close, an improvement never
/// expected to close, and a spec-vs-production disagreement that needs a human decision, not code, to
/// close.
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
    let mut seen_behavior: Vec<&'static str> = Vec::new();

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
                if let Some(k) = BEHAVIOR_DECISIONS.iter().find(|k| **k == name.as_str()) {
                    seen_behavior.push(k);
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
    let behavior_count = mismatched
        .iter()
        .filter(|(n, _)| BEHAVIOR_DECISIONS.contains(&n.as_str()))
        .count();
    let new_count = mismatched.len() - known_count - improvement_count - behavior_count;

    eprintln!("=== new-renderer diff report ===");
    eprintln!("  total cases: {}", cases.len());
    eprintln!("  unsupported (outside this stage's block-kind coverage): {unsupported}");
    eprintln!("  supported (this stage claims to cover): {supported}");
    eprintln!("    exact match: {matched}");
    eprintln!(
        "    mismatch: {} (known gaps: {known_count}, intended improvements: {improvement_count}, \
         behavior decisions: {behavior_count}, NEW: {new_count})",
        mismatched.len()
    );
    if !mismatched.is_empty() {
        eprintln!("  mismatches (case, first differing line):");
        for (name, diag) in &mismatched {
            let label = if KNOWN_MISMATCHES.contains(&name.as_str()) {
                "known"
            } else if INTENDED_IMPROVEMENTS.contains(&name.as_str()) {
                "improvement"
            } else if BEHAVIOR_DECISIONS.contains(&name.as_str()) {
                "behavior"
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
    let stale_behavior: Vec<&&str> = BEHAVIOR_DECISIONS
        .iter()
        .filter(|k| !seen_behavior.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale_behavior.is_empty() {
        eprintln!("  stale BEHAVIOR_DECISIONS entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale_behavior {
            eprintln!("  - {k}");
        }
    }
    eprintln!("=== end report ===");

    let new_mismatches: Vec<&str> = mismatched
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| {
            !KNOWN_MISMATCHES.contains(n)
                && !INTENDED_IMPROVEMENTS.contains(n)
                && !BEHAVIOR_DECISIONS.contains(n)
        })
        .collect();
    assert!(
        new_mismatches.is_empty(),
        "{} new (unlisted) mismatch(es) between render::render_doc and the production renderer: {:?}\n\
         If this is an intentional, understood gap this stage has not implemented yet (see this \
         module's own doc comment for the accepted categories), add the case name(s) to \
         KNOWN_MISMATCHES; if it is a deliberate improvement over a real upstream bug (see \
         INTENDED_IMPROVEMENTS's own doc comment), add it there instead; if the two sides disagree \
         about what CommonMark itself says and neither is wrong (see BEHAVIOR_DECISIONS's own doc \
         comment), add it there instead. If render_doc is genuinely worse than production — an empty \
         line, dropped structure, a panic — that is a real bug: fix it, don't list it anywhere.",
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
