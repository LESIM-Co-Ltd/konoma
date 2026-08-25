//! The renderer built directly on the block model (`crate::preview::markdown::model`), rather than on
//! tui-markdown's own rendered output (tui-markdown itself no longer exists — it was dropped entirely
//! once nothing called into it any more; see `markdown.rs`'s own module doc comment). This module is
//! now the **sole** production Markdown renderer: `render_markdown_with_images` calls `render_doc`
//! directly and unconditionally, with no fallback to any other renderer.
//!
//! (Historical note: this doc comment used to describe an earlier, not-yet-wired-in stage of the
//! `md-block-walk` migration, when this renderer's output was diffed against a separate legacy,
//! tui-markdown-based renderer by a dedicated test harness. Both
//! the legacy renderer and that harness were deleted once the migration completed — see `479df2c`,
//! "refactor(markdown): delete the legacy renderer, leaving one renderer". A number of comments
//! throughout the rest of this file still refer to that harness and to a separate "production
//! pipeline" as if they were distinct from this one; read those as describing that now-finished
//! migration, not the current architecture.)
//!
//! ## Scope (this pass)
//!
//! `Heading`, `Paragraph`, `ThematicBreak`, `List`/`ListItem`, `Quote` (plain **or** an alert —
//! `alert: Some(_)`), `Details`, and `CodeBlock` (fenced or indented, at any depth this pass
//! otherwise covers — top level, inside a list item, inside a quote; see `render_code_block`'s own
//! doc comment) — plus their inline content: emphasis, strong, strikethrough, inline code, links,
//! images (alt text only), soft/hard breaks, and super/subscript. A GitHub alert
//! (`Quote { alert: Some(_), .. }`) is drawn via `render_alert_from_model`, which reuses
//! `markdown.rs`'s own `alert_header_line`/`alert_bar` for the header/left-bar decoration, with the
//! *body* rendered from this file's own tree (`Block.children`) rather than a re-parsed string; see
//! that function's own doc comment. A `Details` block is drawn the identical way, via `render_details_from_model` and
//! `markdown.rs`'s own `details_marker_line`/`details_bar`. `Table` and `Html`, the two remaining
//! `BlockKind`s a `Doc` can contain, are drawn too now (`render_table_from_model`/
//! `render_html_block_from_model`), at any nesting depth — top level, inside a list item, inside a
//! quote, included — closing the one gap this pass used to have: `contains_unsupported` (below) no
//! longer finds anything to screen out, so `RenderOut::unsupported` is always empty in practice and
//! `render_doc` never actually skips a subtree any more. The machinery for that skip — `unsupported`
//! itself and `contains_unsupported` — is kept regardless, unused in practice rather than deleted. It
//! used to back a fallback in `render_markdown_with_images` to a separate legacy, tui-markdown-based
//! renderer whenever `unsupported` was non-empty; that legacy renderer, and the fallback branch that
//! called it, were deleted once the migration completed (`479df2c`), so `unsupported` is now read
//! only by this module's own tests, asserting it stays empty; see `contains_unsupported`'s own doc
//! comment for the same point made where the code itself lives.
//!
//! ## Alert/`<details>` interactivity — `BlockCtx.details_interactive`
//!
//! This file draws a `Details` block's summary marker two different ways, depending on *where* it
//! was found: **interactively** (consuming a slot from the document-wide Tab-toggle ordinal
//! sequence, `next_details_open`, and drawing the marker in `details_marker_style()` — the
//! Tab-focus sentinel) only at the single top-level entry point (`render_doc`); **statically** (using
//! the tag's own `open` attribute directly, no ordinal consumed, a plain marker style) everywhere
//! else — inside a GitHub alert's own body or another `Details` block's own body, at any nesting
//! depth — see `render_bar_prefixed_body`'s own doc comment for the full reasoning.
//! `BlockCtx.details_interactive` is this distinction's own carrier, threaded down through every
//! recursive block dispatcher (`render_block`/`render_list`/`render_item`/`render_quote`) alongside
//! `math_here` — `true` only at `render_doc`'s own top-level dispatch, forced `false`, permanently, by
//! `render_alert_from_model`/`render_details_from_model` for their own `children` (never by plain
//! `render_quote`, whose own children are still reached by the *same* top-level, single-parse walk —
//! see `render_quote`'s own doc comment).
//!
//! A list item nested *inside* a code block, table, or HTML block is not a concern the other
//! direction either — those three are leaves with no `Block::children` of their own (see
//! `model::BlockKind`'s own doc comment), so there is no "list inside a code block" shape for
//! `contains_unsupported` to have to reject.
//!
//! A task-list item inside a *loose* list is fully covered too, marker and all, including a visually
//! surprising layout that is nonetheless the specified behavior: its marker lands on its own line,
//! separate from its own bullet/number (see `render_item`'s own doc comment for exactly why, and
//! `Writer::write_task_marker`'s for the one bookkeeping fix that shape needed — `contains_unsupported`
//! used to exclude it entirely, under a synthetic `"LooseTask"` name, while the underlying panic that
//! shape hit was still open).
//!
//! A **bare** (synthetic — `is_real_paragraph` false) `Paragraph` can legitimately appear anywhere in
//! a *tight* list item's own child list, not merely first, once `CodeBlock` support means the item can
//! contain a non-paragraph sibling that interrupts one paragraph and leaves another lazily continuing
//! right after it with no blank line — `render_block`'s own `Paragraph` arm and `render_bare_paragraph`
//! handle this; see either's own doc comment for why a *later* bare paragraph needs different
//! treatment from a *real* one (`render_paragraph`'s own unconditional double blank-line push would be
//! wrong for it) and why this was a latent gap in this file's existing `Heading`/`ThematicBreak`
//! support too, not something `CodeBlock` alone introduced (see `render_bare_paragraph`'s own doc
//! comment for how that was confirmed, directly, before `CodeBlock` support existed at all).
//!
//! ## How inline content is rendered
//!
//! Not by calling into `pulldown-cmark` at all — and, since this file was rewritten for the
//! `md-block-walk` refactor, **not by re-parsing anything either**, with one narrow, deliberate
//! exception (see below): a `Heading`'s or `Paragraph`'s own inline events are read straight out of
//! `Doc.events` (see `model::Doc`'s own doc comment), by the index range `Doc::parse` already
//! recorded for that exact block (`BlockKind::Heading.inline` / `BlockKind::Paragraph.inline` —
//! every inline-content call site's own first argument is exactly that slice). `Doc::parse` walks
//! `src` **once**, as a whole document, so a reference-style link (`[text][ref]`) whose `[ref]: url`
//! definition lives in a *different* top-level block resolves correctly here — the accepted-gap
//! category an earlier, per-block-re-parsing version of this file had to document (and
//! `md_render_diff_tests` had to carve a dedicated, unlisted pin around) no longer exists at all; see
//! `crate::preview::markdown::inline_corpus`'s own doc comment for the structural proof, and
//! `model::Doc.events`'s doc comment for why a single walk gets this "for free".
//!
//! **The one exception**: `render_details_from_model`, for a `<details>` block whose body could not
//! be represented as real `Block::children` in the first place (`model::BlockKind::Details.glued_body`
//! — a `<summary>`/body/close all glued into one raw, literal `Html` leaf by CommonMark's own
//! HTML-block grammar, no blank line anywhere to have split any of it into finer structure for
//! `Doc::parse`'s one walk to have reported). There is no substring of the *already-recorded*
//! `Doc.events` this body's content could be read from — it was never tokenized as Markdown at all,
//! the first time — so this one function does its own fresh, isolated `Doc::parse` over just that raw
//! byte range, renders it immediately into plain `Line`s (via the ordinary `render_bar_prefixed_body`
//! path, the same one a well-formed `<details>`'s real `children` render through), and never touches
//! the *outer* `Doc`/`events` this file's every other function reads from — see that function's own
//! doc comment for the mechanism, and why keeping the isolated parse's own output fully self-contained
//! (rendered lines only, nothing indexed by anything outside itself) is what keeps this exception from
//! reopening the reference-link gap the rest of this section describes closing.
//!
//! `Writer`, below, is the accumulator every block-rendering function in this file writes rendered
//! output into: it owns the growing `Vec<Line>` the whole render produces, walks `model::Block`'s tree
//! (not a raw `pulldown_cmark::Event` stream), and carries the small amount of extra state a block walk
//! needs on top of a plain inline-content walk — the running item number of whichever list is
//! *currently being rendered*, and whether the next block owes the page a blank separator line before
//! it starts. Two further things a naive port of the same idea would have piled onto `Writer` are
//! deliberately *not* there. A blockquote's own `>` prefix/style is not — see
//! `Writer::emit_row_prefixed`'s own doc comment for why a tree walk never needs a stack for that the
//! way a flat event-stream walk would. Nor is how many levels of list nesting are currently open —
//! see `BlockCtx::list_depth`'s own doc comment for why a tree walk's own call stack already carries
//! that, the same principle applied to a second, unrelated stack. See `Writer`'s own doc comment for
//! what each field holds and why (`pending_block_gap`).
//!
//! `Writer`'s row-building methods (`emit_row`/`append_span`/`join_with_space`/`end_row`, and the rest
//! of the small set `Writer`'s own doc comment covers) are derived from `tui-markdown`'s own
//! `TextWriter` (MIT-licensed; `tui-markdown` itself is no longer a Cargo dependency). That license's
//! notice still has to travel with the binary regardless of whether the crate is still linked, so it is
//! reproduced under "Derived code (not a dependency)" in `THIRD-PARTY-LICENSES.md` (kept there by hand
//! in `about.hbs`, since `cargo-about` only walks the dependency graph).
//!
//! `Image` renders only its alt text: the URL is dropped and never appended anywhere, and nothing
//! about the tag itself changes the writer's state (`start_inline_tag`'s `Tag::Image` arm). `Link` appends
//! `" ("` + the URL styled `KonomaStyles::link()` + `")"` right after the label, the exact 4-span
//! shape `app::md_text::collapse_links` looks for (see that function's own doc comment: "Pattern:
//! `[label]` `" ("` `[URL(blue underline)]` `")"`").
//!
//! Once a block's own raw lines are built, `super::decorate_headings_and_extras` turns the heading's
//! leading `#` span into KonomaStyles-decorated hierarchy (full-width rule under H1/H2), the literal
//! `"---"` this file emits for a `ThematicBreak` into the real full-width rule line, and a literal
//! `"- [x] "`/`"[x] "` run (however `Writer::write_task_marker` happened to have produced it — see its
//! own doc comment for the two distinct shapes) into a dedicated, focusable marker span. This is
//! reused, not reimplemented, for the same reason `Doc::parse` itself reuses
//! `parse_alert_header`/`details_open_tag`/... rather than re-deriving their judgments: a second,
//! hand-written copy of "what does a heading/rule/checkbox look like once decorated" is exactly the
//! kind of drift this whole refactor exists to eliminate. One consequence worth naming explicitly: a
//! few of `decorate_headings_and_extras`'s own passes only recognize the shape they are looking for
//! on a line with **no** leading spans of any kind (`heading_level` reads `line.spans.first()`;
//! `decorate_extras`'s thematic-break check joins *every* span, prefix included) — so a heading or
//! rule nested inside a `Quote` does **not** get its usual special treatment (no color, no
//! full-width rule, literal `"> ---"` instead of an expanded line) here either. That is not a bug this
//! file introduces: it is a real, existing property of `decorate_headings_and_extras` itself, shared
//! by every caller of it.
//!
//! `#![allow(dead_code)]`: this comment predates the `md-block-walk` migration completing. `render_doc`
//! itself is no longer dead code — `markdown.rs`'s `render_markdown_with_images` (the sole production
//! Markdown render path) calls it directly and unconditionally now. Whether every other item in this
//! file still needs the `allow` was not re-audited as part of this comment fix; do not assume it still
//! holds without checking.
#![allow(dead_code)]

use std::ops::Range;

use pulldown_cmark::{Alignment, Event, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::model::{
    code_body_text, html_body_text, html_body_text_in, AlertKind, Block, BlockKind, Doc,
    HtmlTableCell, Task,
};
use super::{
    alert_bar, alert_header_line, code_header, decorate_headings_and_extras, details_bar,
    details_marker_line, gutter_span, highlight_body, html_cell_to_markdown, image_loading_line,
    image_placeholder_lines, image_text_fallback, is_mermaid_info, math_placeholder_lines,
    math_raw_lines, math_url, mermaid_diagram_col, mermaid_fence_url, mermaid_placeholder_lines,
    next_details_open, normalize_cell, pad_to_width, parse_cell_segments, prefix_link_icons,
    render_html_block, render_mermaid_block, render_table_cells, scan_inline_math,
    task_prefix_state, BlockAligns, CellAttrs, CellImage, CellSeg, CodeStyle, ColAlign,
    ImagePlacement, ImageSlot, KonomaStyles, MathPart, MathSlot, MermaidSlot, SourceRun,
    TableCells,
};

/// What `render_doc` produced: the decorated lines (same shape `render_markdown_with_images`
/// returns), every image/diagram/equation placement — a standalone `![alt](url)` paragraph, an
/// extracted ```mermaid fence, and lifted LaTeX math (`math_slot` answering anything but
/// `MathSlot::Raw`) all push into this one, shared `Vec`, exactly the way `render_markdown_with_images`
/// itself returns a single, unified `placements` list regardless of which of the three produced each
/// entry — and (`unsupported`, below) the name of every top-level `BlockKind` this pass encountered
/// but could not render. As of this pass covering every `BlockKind` a `Doc` can contain, `unsupported`
/// is always empty in practice (`contains_unsupported`'s own doc comment) — the field, and the
/// `render_markdown_with_images` legacy fallback that reads it, are kept rather than removed for now;
/// see that doc comment for why.
pub(crate) struct RenderOut {
    pub lines: Vec<Line<'static>>,
    pub images: Vec<ImagePlacement>,
    pub unsupported: Vec<&'static str>,
    /// Every `CodeBlock` this pass actually drew a header for (fenced or indented, at any depth this
    /// pass covers — top level, inside a list item, inside a quote, inside an *open* alert/`Details`
    /// body — see `render_code_block`'s own doc comment), in document order, holding the exact source
    /// text `render_code_block` itself reconstructed via `model::code_body_text` to draw it — **never
    /// a fence diverted to a mermaid diagram instead** (that one never reaches `render_code_block` at
    /// all; see `render_code_block_dispatch`). This is not a second derivation of "where are the code
    /// blocks" the way the old, source-line-scanning `code_block_source_locs` was: it is the exact
    /// text this very pass already decided to draw a header for, pushed at the moment it drew it — so
    /// the caller can read a focused code block's own source straight off this list, by ordinal, with
    /// no re-scan and no count-guard reconciliation of any kind (see `app::md_items::focused_code_source`).
    pub code_blocks: Vec<String>,
    /// Every task-list checkbox marker this pass actually drew, in document order: `(state char,
    /// byte offset of that char within `src`)`. GFM states (a space, `x`, or `X`) come straight from
    /// the model's own `model::Task::state_at` — no re-derivation. A custom state (`ui.md_task_states`,
    /// e.g. `/`) is never reported by pulldown-cmark as a task marker at all (see `model::Task`'s own
    /// doc comment), so it is detected the identical way `decorate_extras`'s own text-based
    /// `task_prefix_state` already detects one on a *rendered* line — applied here to the list item's
    /// own `Block::src` (which always starts at the bullet character, regardless of nesting — see
    /// `render_item`'s own call site) instead of a decorated line's text. Either way, an entry here is
    /// pushed at the exact moment `render_item` decided to draw *some* checkbox for that item, so this
    /// is the render pass's own record, not an independent re-scan the caller has to reconcile by
    /// count against what actually ended up on screen (see `app::md_tasks::md_toggle_focused_task`).
    pub tasks: Vec<(char, usize)>,
}

/// Three independent "how deep in the tree am I, and does it still apply" flags every recursive block
/// dispatcher (`render_block`/`render_list`/`render_item`/`render_quote`, and the two container
/// renderers this pass adds for a GitHub alert/`<details>` — `render_alert_from_model`/
/// `render_details_from_model`) has to thread down together — bundled into one small `Copy` struct,
/// rather than three adjacent `bool` parameters: three `bool`s next to each other at a call site is
/// exactly the kind of silent-swap footgun a named field turns into a compile error instead.
#[derive(Clone, Copy)]
struct BlockCtx {
    /// Whether LaTeX math lifting is still active at this position — unchanged in meaning from
    /// before this pass added alert/`<details>` support (see the module doc comment's own, larger
    /// doc comment on `render_doc`): `false` inside *any* `Quote`'s own children, alert or not
    /// (`render_quote` forces it, the same way it already did before `Quote { alert: Some(_), .. }`
    /// existed at all — production's own `structure_mask` excludes a blockquote's own lines from
    /// math lifting regardless of whether it turns out to be a plain quote or an alert), and inside
    /// a `Details` block's own children too (`render_details_from_model` forces it — production
    /// never lifts math out of a `<details>` body either, for the identical `structure_mask` reason).
    ///
    /// **Not the same question as `extract_here`, below, despite sharing an exclusion footprint for
    /// `Quote`/alert/`Details`** — see that field's own doc comment for exactly where the two
    /// diverge (`render_doc`'s own top-level construction, specifically) and why conflating them was
    /// a real, confirmed bug in an earlier version of this pass: `math_here` starts `math.is_some()`
    /// — `false` for the *entire* render whenever the caller passes `math_on: false` (`ui.math =
    /// "text"`, say) — while `extract_here` starts unconditionally `true`. A version of this file
    /// that reused this one field for both questions (a plain rename, no new field) left block-image
    /// and mermaid-fence extraction silently *disabled* the moment `math_on` was `false`, even though
    /// production's own `split_block_parts` extracts a standalone image or a ```mermaid fence
    /// completely independently of whether math lifting is on at all — confirmed directly: a probe
    /// with `math_on: false` rendered a standalone `![alt](url)` paragraph as plain inline alt text,
    /// never calling `slot_of` at all, before this field was split out.
    math_here: bool,
    /// Whether standalone block-image extraction (`![alt](url)` as a paragraph's *entire* content)
    /// and top-level ```mermaid fence extraction are still active at this position. Shares its own
    /// `Quote`/alert/`Details` exclusion footprint with `math_here` (`false` inside either, for the
    /// identical reason — see that field's own doc comment for the shared mechanism), because
    /// production's own flat, whole-document, raw-source scans for each — `split_block_parts`'s
    /// standalone-image line check (`extract_block_image`/`extract_md_img`) and its ```mermaid
    /// fence-line check (`parse_fence`/`is_mermaid_info`) — are defeated by a blockquote's own
    /// literal `>` prefix the identical way `split_math`'s own `structure_mask` exclusion is
    /// (confirmed directly: `extract_md_img`'s own "prefix must be empty or exactly `[`" check
    /// rejects `"> ![a](u)"` the same way `parse_fence`'s own "must open with `` ` `` or `~`" check
    /// rejects `"> ```mermaid"`), and both are skipped, explicitly, inside an alert's or `<details>`'s
    /// own independently-re-parsed body too — see `code_block_source_locs_inner`'s own
    /// `in_alert_body` doc comment ("the block-level extraction pass (images, ```mermaid fences) runs
    /// on the raw source, where an alert's lines still carry their `>` prefix and match nothing — so
    /// inside an alert body it must be skipped here too, and a diagram there really is drawn as an
    /// ordinary code block") and `split_block_parts`'s own module doc comment for the `<details>`-body
    /// half of the same story. `List`/list-item nesting never forces this `false` (matching
    /// `extract_block_image`'s own `.trim()`-based check, which tolerates any amount of *pure
    /// indentation* — no marker character survives on a list item's own continuation lines the way a
    /// quote's `>` does — see `push_item_marker`'s own doc comment for why a list item's marker
    /// itself only ever occupies the item's very first line, never repeated on later ones the way a
    /// blockquote's own prefix is).
    ///
    /// **Diverges from `math_here` at exactly one place**: `render_doc`'s own top-level construction
    /// sets this unconditionally `true` (never `math.is_some()`) — see that field's own doc comment
    /// for the bug this closes. Everywhere else the two are set identically, side by side, in lockstep
    /// (`render_quote`'s `child_ctx`, `render_bar_prefixed_body`'s `body_ctx`).
    extract_here: bool,
    /// Whether a `Details` block reached at this position should consume the document-wide
    /// Tab-toggle ordinal sequence (`super::next_details_open`) — see the module doc comment's own
    /// "Alert/`<details>` interactivity" section for the full contract this mirrors.
    details_interactive: bool,
    /// How many `List`s are currently open around this position — `0` at the top of a fresh render
    /// (or the top of any isolated sub-`Writer`'s own children, e.g. `render_quote`'s/
    /// `render_bar_prefixed_body`'s), incremented by exactly `1` in the `BlockCtx` `render_list`
    /// passes down to its own items and, through them, to every one of an item's own children —
    /// list or not (see `render_list`'s own doc comment for exactly where that `+1` happens).
    /// `push_item_marker` reads this directly to size a marker's own column width; `render_list`
    /// reads it, still at its *incoming* value (before the `+1`), to tell "no list at all is
    /// currently open" apart from "already inside an enclosing list" for its own leading blank-line
    /// decision.
    ///
    /// Used to be a `Vec<Option<u64>>` (`list_counters`) on `Writer` instead — one entry per
    /// currently-open list, pushed by `render_list` and popped again on its way out, with `.len()`
    /// read back for marker width the same way this field is now. That was the right shape for
    /// tracking a list's own running item *number*: it changes on every single item, and the
    /// function actually incrementing it (`push_item_marker`, called once per item by `render_item`)
    /// is nested two calls below the function that owns the list as a whole (`render_list`) — a
    /// plain local variable can't reach that deep without being threaded down explicitly, which is
    /// exactly what happens now: `render_list` keeps that counter as its own local, mutable
    /// `Option<u64>` and passes it to `render_item`/`push_item_marker` by `&mut` reference, one list
    /// at a time, no stack needed (a nested list's own counter is a *different* local variable in a
    /// *different*, later call to `render_list` — never a second entry sharing one `Vec` with the
    /// first).
    ///
    /// *Depth*, by contrast, never changes within a single call to `render_list` at all — it is
    /// fixed for the whole lifetime of one list, decided once, by whichever caller is already
    /// however many levels deep, before that list's own first item ever renders. That is exactly the
    /// shape `BlockCtx`'s other two fields already have (see `math_here`'s/`extract_here`'s own doc
    /// comments), and exactly what a tree walk's own call stack already tracks for free — a
    /// recursive `render_list` call sitting `N` frames below the outermost one *is* the fact that
    /// `N` lists are currently open, with no separate bookkeeping required to also say so. Keeping
    /// depth on `Writer` as a `Vec` alongside the ordinal one used to work, but conflated two
    /// different kinds of state behind one field — the same principle `row_prefixes`/`row_styles`
    /// were retired under (see this struct's own module, `Writer`'s doc comment): a tree walk's
    /// immutable "how deep am I" question belongs to the call stack (here, `BlockCtx`, threaded
    /// through parameters the way `math_here`/`extract_here` already were), and only a genuinely
    /// *mutable*, per-list value like the running item number needs a variable of its own at all.
    list_depth: usize,
}

/// Renders every top-level block in `doc` this file's own scope covers (see the module doc comment),
/// in source order, through a single top-level `Writer` — the same instance a `List`'s own items
/// render into (nesting deeper only ever means a taller `BlockCtx::list_depth`; see that field's own
/// doc comment for why depth lives there, not on `Writer`, now), so that `pending_block_gap`
/// bookkeeping carries correctly across every block boundary in the document, top level included (see
/// `Writer`'s own doc comment). A `Quote`'s own body, by
/// contrast, always renders into its own *isolated* sub-`Writer` first (`render_quote`'s own doc
/// comment covers why) — this top-level instance never sees a quoted child's lines until they come
/// back already fully decorated and `>`-prefixed. A block kind
/// this pass does not cover — whether sitting directly in `doc.blocks` or found anywhere inside a
/// `List`'s or `Quote`'s own subtree by `contains_unsupported` — contributes no lines and no
/// separator at all, and is recorded in `unsupported` instead.
///
/// `src` is the exact text `doc` was parsed from (`Doc::parse(src)`) — needed, now that `List`
/// support means this file must be able to tell a **tight** list item's own paragraph (no wrapping
/// `Tag::Paragraph` at all; `model::collect_stray_inline_run`'s synthetic node — and, since
/// `CodeBlock` support, not necessarily the item's own *first* child either, see `render_block`'s own
/// `Paragraph` arm) apart from a **loose** one's (a real `Tag::Paragraph`, which changes how
/// `render_paragraph`'s own unconditional blank-line push interacts with `Writer::write_task_marker`'s
/// placement — see `render_item`'s own doc comment): the model already draws that exact line, in
/// `Block.src`'s own trailing byte, for anything computed from raw `pulldown_cmark::Event` ranges to
/// read back (see `model::collect_stray_inline_run`'s own doc comment, and its home test,
/// `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one`) — `is_real_paragraph`
/// is exactly that read, not a second parser judgment of any kind.
/// `width`/`code`/`theme`/`icons`/`tasks` are threaded straight through to `w` (`Writer`'s own
/// `width`/`code`/`theme` fields — read by `render_code_block`, the same way `math` is read by
/// `render_math_slot` — and, at the very end here, `super::decorate_headings_and_extras` runs the
/// heading/thematic-break/checkbox post-passes over the whole document once): this file's own
/// `CodeBlock`s are already fully decorated by the time they reach `w.lines` — see `render_code_block`'s
/// own doc comment. `math_slot`/`math_on` are the same
/// two arguments `render_markdown_with_images` itself takes for the identical purpose — see
/// `render_paragraph_math`'s own doc comment for how a `Paragraph`'s own text gets its LaTeX math
/// lifted onto its own placeholder line(s) when `math_on` is set: at the top level, and inside a
/// `List` item, but *not* inside a `Quote` — matching production's own `structure_mask`, which
/// excludes a blockquote's own lines from math lifting altogether (see `render_paragraph_math`'s own
/// doc comment for exactly why, and `math_here`, the `bool` `render_block`/`render_list`/`render_item`
/// thread down to make that call at every nesting depth, forced `false` the moment `render_quote`
/// dispatches to its own children and staying `false` for everything nested inside it from then on,
/// however many more `List`s or `Quote`s that subtree goes on to contain).
#[allow(clippy::too_many_arguments)] // mirrors render_markdown_with_images's own signature (see that function's own identical allow) — one call site (the diff harness)
pub(crate) fn render_doc(
    doc: &Doc<'_>,
    src: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
    slot_of: &dyn Fn(&str, Option<u16>) -> ImageSlot,
    mermaid_slot: &dyn Fn(&str) -> MermaidSlot,
    mermaid_caption: &str,
    alerts: bool,
    math_slot: &dyn Fn(&str, bool) -> MathSlot,
    math_on: bool,
) -> RenderOut {
    render_doc_aligned(
        doc,
        src,
        width,
        code,
        theme,
        icons,
        tasks,
        slot_of,
        mermaid_slot,
        mermaid_caption,
        alerts,
        math_slot,
        math_on,
        BlockAligns::default(),
    )
}

/// [`render_doc`] plus the block alignments this render runs under (`[ui] md_table_align` /
/// `[ui] md_image_align`, resolved by `config::UiConfig::md_block_aligns`). `render_doc` is this
/// function with [`BlockAligns::default`] — konoma's own historical layout (tables left, images and
/// mermaid diagrams centered) — which is what every test in this file and every golden snapshot
/// still goes through unchanged.
///
/// The alignments live on `Writer` (`Writer::aligns`) rather than being threaded through each
/// block-rendering function's own parameters, the same way `width`/`icons`/`alerts` already are:
/// they are fixed for a whole render and are read at exactly three places
/// (`flush_row_group`, `render_mermaid_slot`, `emit_table`).
#[allow(clippy::too_many_arguments)] // as `render_doc` above, plus the alignments
pub(crate) fn render_doc_aligned(
    doc: &Doc<'_>,
    src: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
    slot_of: &dyn Fn(&str, Option<u16>) -> ImageSlot,
    mermaid_slot: &dyn Fn(&str) -> MermaidSlot,
    mermaid_caption: &str,
    alerts: bool,
    math_slot: &dyn Fn(&str, bool) -> MathSlot,
    math_on: bool,
    aligns: BlockAligns,
) -> RenderOut {
    let styles = KonomaStyles { code_bg: code.bg };
    let math = math_on.then_some(MathCtx {
        slot: math_slot,
        width,
    });
    // `fences_on`: the identical probe `render_markdown_with_images` itself runs
    // (`!matches!(mermaid_slot(""), MermaidSlot::Text)`) — whether a ```mermaid fence is extracted to
    // a diagram placement at all, computed once for the whole render rather than re-probed at every
    // fence (`split_block_parts`'s own doc comment: an empty string is indistinguishable from a real,
    // empty fence body, so probing it *once*, up front, is also what keeps a genuinely empty fence
    // from misreading the probe's own answer as its own slot — see `render_mermaid_slot`'s own doc
    // comment for the empty-fence-body case this is not that).
    let mermaid = MermaidCtx {
        slot: mermaid_slot,
        caption: mermaid_caption,
        width,
        fences_on: !matches!(mermaid_slot(""), MermaidSlot::Text),
        ord: 0,
    };
    // Whether math lifting applies at all, at the *top* of the tree — `render_doc`'s own dispatch is
    // never inside a `Quote`, so this is simply "was `math_on` set", the same answer `math.is_some()`
    // would give; computed once, ahead of `w`'s own construction (which moves `math` into it), so
    // nothing below needs to keep re-deriving `w.math.is_some()`. `extract_here: true` is
    // *unconditional* — image/mermaid extraction is never gated by `math_on` in production either
    // (see `BlockCtx.extract_here`'s own doc comment for the bug this distinction closes).
    // `details_interactive: true` is the *other* half of the identical "top of the tree" reasoning —
    // see `BlockCtx`'s own doc comment.
    let ctx = BlockCtx {
        math_here: math.is_some(),
        extract_here: true,
        details_interactive: true,
        list_depth: 0,
    };
    let mut w = Writer {
        lines: Vec::new(),
        inline_styles: Vec::new(),
        link: None,
        styles,
        pending_block_gap: false,
        math,
        mermaid,
        slot_of,
        images: Vec::new(),
        code_blocks: Vec::new(),
        task_marks: Vec::new(),
        after_math: false,
        fresh_boundary: false,
        pending_para_start: None,
        heading_rule_shift: 0,
        code,
        theme: theme.to_string(),
        width,
        icons,
        tasks,
        alerts,
        aligns,
    };
    // ## A raw, un-stripped YAML front-matter block draws directly from `doc.events`
    //
    // `Options::ENABLE_YAML_STYLE_METADATA_BLOCKS` is always on (`model::parse_options`, built on
    // `markdown.rs`'s own `markdown_parse_options` — see that function's own doc comment), so a
    // document whose caller did *not* run `strip_front_matter` first (`[ui] md_frontmatter = false`,
    // the one real, reachable production case — every other caller strips it before any text reaches
    // a parser at all, matching `model::is_unmodeled_container_tag`'s own doc comment) hands
    // `Doc::parse` a leading `Event::Start(Tag::MetadataBlock(_))`/`Event::End(TagEnd::
    // MetadataBlock(_))` pair. This model's own `parse_container` deliberately does not represent
    // that construct as a `Block` at all (`model::is_unmodeled_container_tag`), so there is no
    // top-level entry in `doc.blocks` for the loop below to ever see — unlike a quote-nested
    // `Table`/`Html`, there is nothing here for `contains_unsupported` to have found. `doc.events`
    // still carries every event pulldown-cmark reported for it regardless (`Doc.events`'s own doc
    // comment: "every `Event`/byte-range pair ... for the *whole* document, not merely the ones some
    // `Block` below happens to reference") — `render_metadata_block` is the one further place in this
    // file (besides this function's own top-level dispatch below) that reads that stream directly,
    // rather than through a `Block`'s own `inline`/`body_spans` range, for the identical reason: there
    // is no `Block` to name a range on. A no-op, immediately, for every other caller (front matter
    // already stripped, so `doc.events` never carries a `MetadataBlock` event to find at all — see
    // that function's own doc comment).
    render_metadata_block(&mut w, doc);
    // Nothing pushes into this any more — `contains_unsupported` never returns `Some` (see its own
    // doc comment), and the only other push site left, `BlockKind::ListItem` a few arms down, can
    // never legitimately fire either (a `ListItem` only ever exists as a `List`'s own child, always
    // unwrapped before this loop sees one). `RenderOut::unsupported` and the machinery around it
    // (this `Vec`, `contains_unsupported` itself, `render_markdown_with_images`'s own legacy
    // fallback) are kept, not removed, for this pass — see this file's own module doc comment.
    let mut unsupported: Vec<&'static str> = Vec::new();
    for block in &doc.blocks {
        match &block.kind {
            BlockKind::Heading {
                level,
                inline,
                id,
                classes,
                attrs,
            } => {
                let meta = HeadingMeta {
                    id: id.clone(),
                    classes: classes.clone(),
                    attrs: attrs.clone(),
                };
                render_heading(&mut w, doc, src, *level, inline.clone(), &meta);
            }
            BlockKind::Paragraph { inline } => {
                render_paragraph_dispatch(
                    &mut w,
                    doc,
                    src,
                    inline.clone(),
                    ctx.math_here,
                    ctx.extract_here,
                    block.src.start,
                );
            }
            BlockKind::ThematicBreak => render_rule(&mut w),
            BlockKind::List { start, .. } => match contains_unsupported(&block.children, false) {
                Some(bad) => unsupported.push(bad),
                None => render_list(&mut w, doc, src, *start, &block.children, ctx),
            },
            BlockKind::Quote { alert: None, .. } => {
                match contains_unsupported(&block.children, true) {
                    Some(bad) => unsupported.push(bad),
                    None => render_quote(&mut w, doc, src, &block.children, ctx),
                }
            }
            // A GitHub-alert-headed quote with `w.alerts = false` (`ui.md_alerts`): drawn as an
            // ordinary blockquote, exactly the `alert: None` arm above — see `Writer.alerts`'s own
            // doc comment for why this is read off `w` rather than threaded as a fourth `BlockCtx`
            // field. `block.children` still has the literal `[!TYPE]`
            // header text sitting in its own first paragraph either way (`model::alert_kind_of` only
            // *peeks* at the quote's first line to decide `alert`/`alert_title` — it never strips
            // anything from `children`), so this plain-quote render of an alert-headed block shows
            // that marker text verbatim, matching the legacy path's own "`[!WARNING]` survives as raw
            // text" contract (pinned by `e2e_markdown_autolink_emoji_alerts_toggle_off`). A separate
            // arm from the one above (not merged via `alert.is_none() || !w.alerts`) because a match
            // guard does not count towards exhaustiveness — the compiler cannot tell that arm, on its
            // own, already covers every `alert: None` case.
            BlockKind::Quote { alert: Some(_), .. } if !w.alerts => {
                match contains_unsupported(&block.children, true) {
                    Some(bad) => unsupported.push(bad),
                    None => render_quote(&mut w, doc, src, &block.children, ctx),
                }
            }
            // A GitHub alert — konoma's own `render_alert_from_model`, reusing `markdown.rs`'s own
            // `alert_header_line`/`alert_bar` for the header/bar decoration; see the module doc
            // comment's own "Scope" section. An alert is still, structurally, a `Quote` (pulldown-cmark
            // reports the identical `Tag::BlockQuote` either way — see `model::BlockKind::Quote`'s own
            // doc comment) — so its own children get `in_quote: true` too, the same as a plain quote's.
            // Only reached when `w.alerts` is `true` — the two arms above already caught every
            // `alert: None`, and every `alert: Some(_)` with alerts turned off.
            BlockKind::Quote {
                alert: Some(kind),
                alert_title,
            } => match contains_unsupported(&block.children, true) {
                Some(bad) => unsupported.push(bad),
                None => render_alert_from_model(
                    &mut w,
                    doc,
                    src,
                    *kind,
                    alert_title,
                    &block.children,
                    block.src.start,
                ),
            },
            BlockKind::CodeBlock {
                lang, body_spans, ..
            } => render_code_block_dispatch(
                &mut w,
                src,
                lang.as_deref(),
                body_spans,
                ctx.extract_here,
            ),
            BlockKind::Details {
                open_attr,
                summary,
                glued_body,
            } => match contains_unsupported(&block.children, false) {
                Some(bad) => unsupported.push(bad),
                None => render_details_from_model(
                    &mut w,
                    doc,
                    src,
                    *open_attr,
                    summary,
                    &block.children,
                    glued_body.clone(),
                    ctx,
                ),
            },
            // Never reachable at the top level — a `ListItem` only ever exists as a `List`'s own
            // child (see `model::BlockKind::ListItem`'s doc comment), and `render_list` unwraps every
            // one of those itself before this loop ever sees it. Matched defensively all the same,
            // rather than assumed unreachable — see `render_block`'s own doc comment for why that
            // convention holds throughout this file.
            BlockKind::ListItem { .. } => unsupported.push("ListItem"),
            // A top-level `Table`/`Html` is drawn directly — never reported unsupported, at this or
            // any nesting depth (`contains_unsupported` never returns for either name any more; see
            // its own doc comment for why the check it used to make here is now permanently moot).
            BlockKind::Table { aligns, rows } => render_table_from_model(&mut w, src, aligns, rows),
            BlockKind::Html { body_spans, .. } => {
                render_html_block_from_model(&mut w, src, body_spans)
            }
            BlockKind::HtmlTable { body_spans, rows } => {
                render_html_table_from_model(&mut w, src, body_spans, rows)
            }
        }
    }
    let lines = decorate_headings_and_extras(w.lines, width, icons, tasks);
    RenderOut {
        lines,
        images: w.images,
        unsupported,
        code_blocks: w.code_blocks,
        tasks: w.task_marks,
    }
}

/// **Now a permanent no-op — always returns `None`.** Kept, not deleted or inlined away, because the
/// machinery around it (`RenderOut::unsupported`, `render_markdown_with_images`'s own legacy
/// fallback, this function's own `in_quote` parameter) is not being removed by this pass either; see
/// this file's own module doc comment ("Scope") for the plan that removes both together.
///
/// This last held out for a `Table` found **nested inside a `Quote`**: it used to be reported
/// unsupported whenever `in_quote` (below) was `true` at the point a `Table` turned up, because
/// `render_table_from_model` used to render one from a *naive* whole-block slice of `Block.src`, and
/// a blockquote's own `>` marker survives on every continuation line of that slice (confirmed
/// directly: parsing `"> | a | b |\n> |---|---|\n> | 1 | 2 |\n"` reported the table's own `Block.src`
/// slice as `"| a | b |\n> |---|---|\n> | 1 | 2 |\n"` — the *first* line's marker excluded, every
/// later line's kept), which broke the legacy renderer's own line-based delimiter/cell parsing (the
/// `>`-prefixed delimiter row no longer matched `is_table_delimiter`'s own character set, so the
/// whole table degraded to garbled literal rows — this was production's own real, confirmed
/// limitation too: `split_tables`'s own raw-line scan is equally defeated by the same `>`-prefixed
/// continuation lines, so a quote-nested table rendered as literal wrapped text there as well, not a
/// defect unique to this stage's own reuse choice). `render_table_from_model` no longer takes that
/// slice at all — it builds `markdown.rs`'s own `TableCells` directly from
/// `model::BlockKind::Table.aligns`/`.rows`, whose cell ranges are pulldown-cmark's own per-cell
/// ranges on `src`, already free of the enclosing `>` marker regardless of nesting depth (see that
/// function's own doc comment) — the identical fix `model::BlockKind::Html.body_spans` already made
/// for `Html`'s own, now-closed instance of this same defect, which is why `Html` stopped being
/// reported here first (see the version-control history of this doc comment for that earlier text,
/// if useful — it walked through the two side by side while `Table` was still the one remaining gap).
///
/// `CodeBlock`, `Quote { alert: Some(_), .. }`/`Details`, and a **loose list item with a task marker**
/// were three further names this function used to screen out, at various earlier points — see
/// `render_code_block`'s own doc comment (`CodeBlock`), the module doc comment (alert/`<details>`),
/// and `Writer::write_task_marker`'s own doc comment (the task-marker crash a synthetic `"LooseTask"`
/// name used to work around here) for why each of those closed constructively instead, the same
/// pattern `Table` and `Html` both followed: give the shape a real renderer (or fix the underlying
/// crash), rather than leave this function permanently screening it out.
///
/// `blocks`/`in_quote`: unchanged from before — `blocks` is a `List`'s own item children, a `Quote`'s
/// own body (alert or not), or a `Details`'s own body, walked recursively; `in_quote` still tracks
/// whether the walk is currently inside a `Quote`'s own subtree (`true` the moment `Quote { .. }`,
/// below, recurses into its own `children`), even though nothing left in this function's own body
/// reads it for a decision any more — every call site above still threads it through, and removing
/// the parameter itself is exactly the kind of "this function is dead weight now" cleanup that
/// belongs with removing the rest of the fallback machinery, not with this fix in isolation.
///
/// `clippy::only_used_in_recursion` fires on `in_quote` for exactly that reason — every read of it
/// left is a recursive call passing it along, never a branch — and is silenced below rather than
/// acted on for the identical reason the parameter itself is kept.
#[allow(clippy::only_used_in_recursion)]
fn contains_unsupported(blocks: &[Block], in_quote: bool) -> Option<&'static str> {
    for b in blocks {
        match &b.kind {
            BlockKind::Table { .. } | BlockKind::Html { .. } | BlockKind::HtmlTable { .. } => {}
            BlockKind::Heading { .. }
            | BlockKind::Paragraph { .. }
            | BlockKind::ThematicBreak
            | BlockKind::CodeBlock { .. } => {}
            BlockKind::ListItem { .. } | BlockKind::List { .. } | BlockKind::Details { .. } => {
                if let Some(bad) = contains_unsupported(&b.children, in_quote) {
                    return Some(bad);
                }
            }
            BlockKind::Quote { .. } => {
                if let Some(bad) = contains_unsupported(&b.children, true) {
                    return Some(bad);
                }
            }
        }
    }
    None
}

/// The accumulator every block-rendering function in this file writes into, one instance per render,
/// walking `model::Block`'s tree rather than a raw `pulldown_cmark::Event` stream (see the module doc
/// comment). Besides the growing `Vec<Line>` itself (`lines`, below), it holds the small amount of
/// extra bookkeeping a *block* walk needs on top of a plain inline-content walk:
///
/// Deliberately *not* here — two separate things a naive port of `tui-markdown`'s own `TextWriter`
/// would have carried on `Writer` as a stack, and this file does not.
///
/// The first is anything tracking a currently-open `Quote`'s own `>` prefix/style. An earlier version
/// of this struct held that the way upstream `tui-markdown` did — a `row_prefixes: Vec<Span>`/
/// `row_styles: Vec<Style>` pair of stacks every line-emitting method read from its own top — because
/// that shape was copied wholesale from a renderer that walks a *flat* `pulldown_cmark::Event` stream,
/// where a stack genuinely is the only way to know, while emitting one line deep inside a
/// doubly-quoted paragraph, how many `>` markers are currently "open" around it. This file walks
/// `model::Block`'s tree instead: `render_quote` assembles a whole quoted run of lines in an isolated
/// sub-`Writer`, decorates it, and only *then* prefixes it — as a plain local value, never
/// Writer-owned state — before handing it back to its caller (see that function's own doc comment,
/// and [`Writer::emit_row_prefixed`]'s, for the full mechanism and why it reproduces nested `"> >"`
/// prefixing without ever needing more than depth `1`).
///
/// The second is how many `List`s are currently open — an earlier version of *this* struct held that
/// as `list_counters: Vec<Option<u64>>` too, one entry per open list, `.len()` read back for every
/// item marker's own column width. That, again, is the right shape for a flat event stream with no
/// nesting boundary of its own to hang the depth on, but this file's own tree walk already has one:
/// `BlockCtx::list_depth`, threaded down as an ordinary parameter the same way `math_here`/
/// `extract_here` already were (see that field's own doc comment for the fuller reasoning, and for why
/// the *ordinal* half of the old `Vec<Option<u64>>` — a list's own running item number, genuinely
/// mutable and updated once per item — stays an ordinary local variable in `render_list` instead,
/// passed to `render_item`/`push_item_marker` by `&mut` reference rather than promoted to `BlockCtx`
/// alongside depth: unlike depth, it changes within a single list's own lifetime, so it isn't the kind
/// of thing a `Copy` context struct should carry).
///
/// * `pending_block_gap` — whether the *next* block-level construct owes the page a blank separator line
///   before it starts. Starts `false`. Every *block*-rendering function in this file — `render_heading`/
///   `render_paragraph`/`render_rule`/`render_list`/`render_quote` — sets it back to `true` on its own
///   way out. The one deliberate exception is bare *inline* content with no wrapping block tag at
///   all — `Writer::write_text`/`write_code_span`/`join_with_space`/`end_row`, and the inline-style push/pop
///   pair — none of which ever sets it `true` (see `Writer::write_text`'s own doc comment); this is
///   exactly what a **tight** list item's own content is (see `render_item`'s own doc comment on the
///   tight/loose distinction), so it must not accidentally read as "the page owes a blank separator"
///   once that content finishes. `after_math` (below) is a *further* exception,
///   deliberately narrower than that one: a math lift sets `pending_block_gap` to `false` itself (see
///   `render_math_slot`), and every *container*-closing site that would otherwise unconditionally set
///   it back to `true` (`render_list`/`render_quote`'s own exits) has to check `after_math` first
///   rather than clobber that — see `after_math`'s own doc comment for why.
///
/// Two fields exist only for math lifting (`render_paragraph_math`'s own doc comment covers the
/// mechanism these support in full; this only documents what each field itself is):
///
/// * `math` — the math-lifting context (`None` whenever `render_doc`'s own `math_on` argument is
///   `false`; `Some` for the whole render otherwise, `render_math_slot` reads/pushes to it directly).
///   Owned here, not passed as a separate parameter down every call site, both because it needs to
///   accumulate `ImagePlacement`s across however many expressions the whole document lifts (a single,
///   shared `Vec`, not one per call) and because every function in this file already threads
///   `w: &mut Writer` through everywhere it needs state at all — a second, parallel `Option<&mut
///   MathCtx>` parameter would need its own reborrow at every loop this file already has one of
///   (`render_list`'s own item loop, `render_quote`'s own child loop, ...), for no benefit over simply
///   reading `w.math` directly wherever it is needed. *Whether* a given `Paragraph` actually consults
///   it is a separate, per-call decision — the `math_here: bool` parameter `render_block`/`render_list`/
///   `render_item` thread — not something this field's own presence controls by itself; see
///   `render_paragraph_math`'s own doc comment for why a blockquote's own text is excluded from math
///   lifting even though `math` stays populated the whole way through it.
/// * `after_math` — whether the very last thing pushed onto `self.lines` was a lifted math
///   expression's own placeholder, with nothing textual rendered since. `render_math_slot` is the
///   *only* place that sets it `true` — its own placeholder lines go straight to `self.lines` via
///   `self.lines.extend(..)`, deliberately bypassing `emit_row` (the one narrow exception to that
///   method's own "only place a new `Line` legitimately enters `self.lines`" doc comment — see there),
///   precisely so this flag survives long enough for whatever renders *next* to see it. `emit_row`
///   clears it unconditionally, first thing, the moment anything else renders through the normal
///   machinery — so every block-rendering function that already calls `emit_row` somewhere in its own
///   body before its own exit (every one of them does) needs no `after_math`-awareness of its own at
///   all; only a function whose *exit* can run **without** having called `emit_row` again since a
///   lift — `render_list`/`render_quote`, when their very last item/child ended right on one — has to
///   consult it explicitly, to avoid re-forcing `pending_block_gap` back to `true` and reintroducing the
///   very separator line the lift's own reset was there to suppress. See `render_paragraph_math`'s own
///   doc comment for the "fresh reparse boundary" this reproduces, and `ensure_fresh_after_math`'s own
///   for how the flag gets consumed once something *does* follow a lift within the same paragraph.
struct Writer<'m> {
    lines: Vec<Line<'static>>,
    inline_styles: Vec<Style>,
    link: Option<String>,
    styles: KonomaStyles,
    pending_block_gap: bool,
    math: Option<MathCtx<'m>>,
    /// The mermaid-extraction context — unconditional (not `Option`, unlike `math`), since
    /// production's own `mermaid_slot`/`mermaid_caption` are always-required parameters to
    /// `render_markdown_with_images` too (whether extraction actually *fires* for a given fence is
    /// `mermaid.fences_on`, computed once via the same probe production runs — see `render_doc`'s own
    /// construction site).
    mermaid: MermaidCtx<'m>,
    /// `render_doc`'s own incoming `slot_of` — how the app wants an image rendered; read by
    /// `render_image_group` for a standalone block image (`![alt](url)` as a paragraph's *entire*
    /// content) and by `render_table_from_model`/`render_html_table_from_model` for an image inside a
    /// table cell. Like `mermaid`, unconditional: production's own block-image extraction
    /// (`split_block_parts`) is never gated by anything analogous to `math_on`/mermaid's own
    /// `fences_on` probe — a qualifying paragraph is *always* handed to `slot_of`, whatever it answers.
    ///
    /// The second argument is the caller's own width budget for that one image (see
    /// `super::render_markdown_with_images`'s own doc comment): `None` from the block-image path (the
    /// whole page), `Some(column width)` from a table cell.
    slot_of: &'m dyn Fn(&str, Option<u16>) -> ImageSlot,
    /// Every image/diagram/equation placement this render has produced so far — the one, shared
    /// accumulator `RenderOut::images` reads from at the very end (see that field's own doc comment
    /// on why math/mermaid/block-images all push into the *same* `Vec`, matching production's own
    /// single, unified `placements` list). Owned directly on `Writer`, not nested inside `math` the
    /// way an earlier version of this struct had it — mermaid and block-image placements have no
    /// natural home inside `MathCtx` at all, and duplicating an `images: Vec<ImagePlacement>` field
    /// on three separate context structs would just be three accumulators that all have to get
    /// concatenated back together at the end anyway.
    images: Vec<ImagePlacement>,
    /// `RenderOut::code_blocks`, accumulated as `render_code_block` draws each header — see that
    /// field's own doc comment. `render_bar_prefixed_body` merges its own isolated sub-`Writer`'s copy
    /// back into the outer one's, in document order, the identical way it already does for `images`.
    code_blocks: Vec<String>,
    /// `RenderOut::tasks`, accumulated as `render_item` draws each checkbox marker (GFM or custom
    /// state) — see that field's own doc comment. Named `task_marks`, not `tasks`, to keep it distinct
    /// from `Writer::tasks` (the `&'m [char]` of configured custom states `render_item` reads to
    /// *detect* one in the first place). Merged across `render_bar_prefixed_body`'s isolation boundary
    /// the same way `images`/`code_blocks` are.
    task_marks: Vec<(char, usize)>,
    after_math: bool,
    /// Whether the very last thing pushed onto `self.lines` was the end of a GitHub alert or
    /// `Details` block — `render_alert_from_model`/`render_details_from_model` are the *only* two
    /// places that set this `true`, on their own way out, alongside `pending_block_gap = false` (see
    /// either's own doc comment for why both: whatever renders next is exactly as if a brand-new
    /// `Writer` had just started — an alert's or `<details>`'s own body really is rendered by a
    /// fresh, independent sub-`Writer` (`render_bar_prefixed_body`'s own isolation), the identical
    /// "fresh boundary" `render_math_slot`'s own doc comment already documents in full for a lifted
    /// math expression).
    ///
    /// `pending_block_gap = false` alone reproduces that boundary for every *other* block-rendering
    /// function in this file, all of which gate their own leading blank-line push on it — except
    /// `render_code_block`, whose own leading-blank check (`if !w.lines.is_empty() { .. }`) is
    /// deliberately **not** gated on `pending_block_gap` at all —
    /// see that function's own doc comment for exactly why not (a fence gets its separator even when
    /// `pending_block_gap` happens to be `false` for an unrelated reason, e.g. a tight list item's own
    /// bare content). That same "unconditional as long as *something* already rendered" check is
    /// exactly what mistakes an alert's/`Details`'s own already-rendered lines, still sitting in the
    /// *shared* `w.lines`, for "something precedes this fence within the *same* segment" — an ambiguity
    /// the legacy renderer's own per-segment isolation never had to face at all (confirmed
    /// directly: `code_corpus`'s own "indented block nested inside a closed details is not counted"
    /// and "everything" cases both show a fence immediately after a closed `Details`/an alert with no
    /// blank row between them, exactly the same way a fence right after a lifted math expression, or
    /// as the very first thing in the whole document, gets none). `render_code_block` is the only
    /// site that has to consult this flag for that reason; `emit_row` clears it unconditionally,
    /// first thing, the same way it already clears `after_math` — so, like that flag, no other
    /// block-rendering function needs any awareness of it at all (each already calls `emit_row`
    /// itself before returning).
    fresh_boundary: bool,
    /// A math-lifting paragraph's own leading-blank-line convention, deferred rather than applied
    /// eagerly — `Some(gap_before)` from the moment `render_paragraph_math`/`render_item`
    /// (its own tight/loose-item-first-child special case) starts walking a paragraph whose text
    /// *might* lift math, until whichever of `open_pending_para_start`/`drop_pending_para_start` first
    /// resolves it; `None` once resolved (or when no math-lifting walk is in progress at all).
    ///
    /// ## Why this cannot be applied eagerly, the way `render_paragraph`'s non-math counterpart does
    ///
    /// `render_paragraph` (no math) always opens with its own two-part leading-blank-line push
    /// (`if pending_block_gap { emit_blank_row() }` then an *unconditional* second one)
    /// *before* walking the paragraph's own inline content, because that walk is guaranteed to reach
    /// `Writer::write_text`/`append_span` for its own first content — which *fills* that freshly-opened blank
    /// row rather than adding a new one (`append_span`'s own `self.lines.last_mut()` fallback). A
    /// math-lifting paragraph's walk (`walk_inline_math`) is not guaranteed that: its own very first
    /// content can just as easily be a lifted math expression's own placeholder, which
    /// `render_math_slot` appends via `self.lines.extend(..)` — deliberately bypassing `append_span`
    /// (see `after_math`'s own doc comment for why) — so a blank row opened ahead of time for math to
    /// "fill" never gets filled at all, and is left behind, genuinely empty, forever.
    ///
    /// This is not a hypothetical: it is exactly the shape a lifted expression gets in this file's own
    /// `render_math_slot` — a lifted expression that is the very first (and typically only) content of
    /// its own paragraph gets **no leading blank line of any kind**, not even the separator a genuinely
    /// new top-level block would otherwise owe the page: `render_math_slot` appends its own placeholder
    /// lines directly (`self.lines.extend(math_placeholder_lines(..))`), with **no** paragraph-start
    /// convention run for it at all, because `append_span` is never involved in placing it. Confirmed
    /// directly against the pre-migration production build for the identical shape too (`code_span_corpus`'s
    /// own math cases — a `$$…$$`/`\(…\)`/`\[…\]` expression as a paragraph's own *entire* content,
    /// immediately after another top-level block — an indented code block, in every one of those three
    /// cases): this holds regardless of *what* preceded it — a code block (confirmed: the padding row's
    /// own line is followed immediately by the math line, one line closer than a real separator would
    /// put it), a heading, or another paragraph (confirmed with a hand-traced `"para1\n\n$$y$$\n"`).
    ///
    /// So the convention has to become *lazy*: `render_paragraph_math` records that a paragraph-start
    /// is owed (`Some(w.pending_block_gap)`, capturing the value the eager version would have read
    /// immediately) but does not yet act on it. The first thing that turns out to need a fresh row —
    /// real text, reached via `render_text_with_math`'s own `MathPart::Text` arm, or an inline code
    /// span/soft break/hard break/nested inline tag, reached via `ensure_fresh_after_math` (now the
    /// shared "about to render real content" checkpoint for *both* this and `after_math` — see that
    /// function's own doc comment) — opens it via `open_pending_para_start`, applying the exact same
    /// two-part convention `render_paragraph` still applies eagerly (this struct's own doc comment on
    /// `pending_block_gap` explains why that shape is correct for text: it is literally the same
    /// `start_paragraph` convention, just applied at the point real content is about to fill it rather
    /// than in advance). If a lifted expression turns out to be the very first thing instead,
    /// `render_math_slot` calls `drop_pending_para_start` — clearing the slot **without** ever opening
    /// it — matching production's own "appended directly, no convention at all" behavior exactly.
    pending_para_start: Option<bool>,
    /// Colors/layout for a `CodeBlock`'s own header/body/padding band (`render_code_block`) — the
    /// exact `CodeStyle` `render_doc` itself received, held here (rather than threaded as an extra
    /// parameter through every intermediate block dispatcher — `render_block`/`render_list`/
    /// `render_item`/`render_quote` — down to the one function that actually reads it) for the same
    /// reason `math` is: every one of those functions already threads `w: &mut Writer` through
    /// everywhere it needs state at all. `Copy`, so reading it never needs a reborrow.
    code: CodeStyle,
    /// The syntect theme name `render_code_block` highlights a fence's body against — owned
    /// (`String`, not `&'m str`) purely to sidestep threading a second named lifetime through this
    /// struct for a value that is, in practice, a short string read at most a handful of times per
    /// document; `render_doc`'s own `theme: &str` parameter is cloned into this exactly once.
    theme: String,
    /// `render_doc`'s own incoming column count, unchanged for the whole render — `render_code_block`
    /// reads it directly (see that function's own doc comment on why the *quote-prefix* width still
    /// has to be subtracted from it there, but list nesting never does).
    width: u16,
    /// `render_doc`'s own incoming `icons` flag, unchanged for the whole render — read by
    /// `render_alert_from_model` for its own header line's optional Nerd Font icon (`super::
    /// alert_header_line`'s own `icons` parameter). `Copy`, so reading it never needs a reborrow.
    icons: bool,
    /// `render_doc`'s own incoming `tasks` slice, unchanged for the whole render — read by
    /// `render_bar_prefixed_body`, which needs it to run `super::decorate_headings_and_extras` over
    /// an alert's/`Details`'s own body **before** the bar gets prepended to each line; see that
    /// function's own doc comment for why. Borrowed with this struct's own `'m` lifetime (the same
    /// one `math: Option<MathCtx<'m>>` already uses) rather than owned, since `render_doc`'s own
    /// `tasks: &[char]` parameter already outlives the whole call.
    tasks: &'m [char],
    /// `render_doc`'s own incoming `alerts` flag (`ui.md_alerts`) — unchanged for the whole render,
    /// the same way `icons`/`tasks` are. Read at *both* places a `Quote { alert: Some(_), .. }` can
    /// be reached (`render_doc`'s own top-level dispatch, and `render_block`'s nested one — a
    /// GitHub-alert-headed quote can sit inside a `List`/`Quote`/`Details` too): `false` renders it
    /// as an ordinary blockquote (`render_quote`, exactly the `alert: None` arm's own path) instead of
    /// the colored callout `render_alert_from_model` draws. Held on `Writer` — not threaded through
    /// `BlockCtx` as a fourth field — because, unlike `math_here`/`extract_here`/
    /// `details_interactive`, its value never changes with nesting depth (it is one document-wide
    /// config flag, not a position in the tree): every function that reaches a `Quote` at all already
    /// has `w: &mut Writer` in scope, so no call site needs to remember to propagate it into a fresh
    /// `BlockCtx` the way `render_quote`'s own `child_ctx`/`render_bar_prefixed_body`'s own `body_ctx`
    /// have to for the three fields that *do* vary. `render_bar_prefixed_body`'s own, fully isolated
    /// sub-`Writer` copies this field from the outer one for the identical reason it copies
    /// `icons`/`code`/`theme`: a nested alert discovered *inside* another alert's or a `<details>`
    /// block's own body must honor the same document-wide toggle.
    alerts: bool,
    /// `render_doc_aligned`'s own incoming block alignments (`[ui] md_table_align` /
    /// `[ui] md_image_align`) — one document-wide pair of settings, held here for exactly the reason
    /// `alerts` above is (it never varies with nesting depth), and copied verbatim into every
    /// isolated sub-`Writer` alongside `alerts`/`icons`/`code`/`theme` so a table or an image inside
    /// a quote, an alert or a `<details>` body aligns by the same rule as one at the top level —
    /// within *that* body's own narrowed width, which is exactly what `w.width` already means there.
    ///
    /// Read at three places, and nowhere else: `flush_row_group` (a block image row group's start
    /// column), `render_mermaid_slot` (via `mermaid_diagram_col`, which also feeds the caption indent
    /// and — through `ui::preview` — the focus frame), and `emit_table` (a table's own box shift, and
    /// with it the `col` of every image reserved inside one of its cells).
    aligns: BlockAligns,
    /// How many extra lines `decorate_headings_and_extras`'s own `decorate_headings` pass — run
    /// exactly **once**, over the *whole* document, right at the very end of `render_doc` — will
    /// insert **before** whatever line index `self.lines.len()` currently reports, once that single
    /// pass finally runs. Incremented by exactly `1` by `render_heading`, immediately after pushing
    /// an H1/H2 heading's own line (the only shape `decorate_headings` ever adds a line for: a
    /// full-width rule directly under a level-1/2 heading — see that function's own body).
    ///
    /// This exists because an `ImagePlacement`'s own `line` field has to name a row index in
    /// `RenderOut::lines` — the *fully decorated* array a real preview overlay actually reads
    /// positions from — but every placement is recorded *during* the walk, i.e. against
    /// `self.lines.len()` **before** that single, whole-document decoration pass has run at all
    /// (`render_image_group`/`render_mermaid_slot`/`render_math_slot` all read `self.lines.len()` the
    /// moment their own reserved rows are about to be pushed — the identical instant the legacy
    /// renderer's own `BlockPart::Image`/`BlockPart::Mermaid`/`BlockPart::Math` arms read `out.len()`).
    /// The legacy renderer never had this gap at all: it decorated each Markdown *text* segment (via
    /// its own call to `decorate_headings_and_extras`) the moment that segment finished, immediately, *before*
    /// moving on to the next `BlockPart` — so by the time it reads `out.len()` for the next
    /// image/mermaid/math placement, every heading rule any *earlier* segment might have inserted is
    /// already baked into that count. This file's own `Writer`, by contrast, accumulates the *entire*
    /// document into one
    /// `self.lines` and decorates it **once**, at the very end (`render_doc`'s own final
    /// `decorate_headings_and_extras(w.lines, ..)` call) — so a placement recorded partway through
    /// the walk has no way to know, at push time, how many rule lines *later* decoration will still
    /// go on to insert **before** it, from headings that already rendered earlier in the very same
    /// document. This field is exactly that missing count, tracked incrementally as the one thing
    /// that *does* change `self.lines`'s own eventual length between "when a placement is recorded"
    /// and "when decoration finally runs" (`decorate_extras`'s own thematic-break/checkbox passes
    /// never change line *count* at all, only individual lines' own content — see `render_heading`'s
    /// own doc comment for the one line this field does *not* need to track: a level 3–6 heading,
    /// which `decorate_headings` strips its own leading `#` span from in place, adding no line).
    ///
    /// Not consulted for anything rendered *inside* an alert's or a `<details>`'s own body
    /// (`render_bar_prefixed_body`'s own, fully independent sub-`Writer`, decorated in complete
    /// isolation the moment that body finishes — see that function's own doc comment): a heading
    /// reached there increments *that* sub-`Writer`'s own, freshly-zeroed copy of this field instead,
    /// which is exactly correct, since no image/mermaid/math placement can ever originate inside one
    /// in the first place (`BlockCtx.math_here` is forced `false` for every alert's/`Details`'s own
    /// children unconditionally — see that field's own doc comment) — there is nothing on the
    /// *outer* `Writer`'s own placements this inner counter would ever need to influence.
    heading_rule_shift: usize,
}

impl<'m> Writer<'m> {
    fn cur_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
    }

    /// The single place a new `Line` legitimately enters `self.lines` in the ordinary case — no
    /// prefix, no style patch. `render_math_slot`'s own `self.lines.extend(..)` is the one other,
    /// deliberate exception (see this struct's own doc comment on `after_math` for why). Two callers
    /// need a prefix and/or a style patch applied first — `render_quote` (a blockquote's `>` marker)
    /// and `render_metadata_block` (front matter's own color) — see [`Writer::emit_row_prefixed`],
    /// which this delegates to.
    fn emit_row(&mut self, line: Line<'static>) {
        self.emit_row_prefixed(line, None, None);
    }

    /// The explicit form `emit_row` delegates to. `prefix`, when given, is inserted ahead of the
    /// line's own content together with the single shared space that has always separated a `>`
    /// (or nested `> >`) from what follows it; `style` patches the whole line, the same way
    /// `Line::patch_style` always did.
    ///
    /// This used to be implicit: a `row_prefixes: Vec<Span>`/`row_styles: Vec<Style>` pair of stacks
    /// on `Writer` that every `emit_row` call read from its own top, pushed by whichever
    /// block-rendering function currently had one open and popped on its way out — a shape copied
    /// from upstream `tui-markdown`, which walks a *flat* `pulldown_cmark::Event` stream and
    /// therefore genuinely needs a stack: it has no other way to know, while emitting one line deep
    /// inside a doubly-quoted paragraph, that two `>` markers (or, for metadata, one governing style)
    /// are currently "open" around it, because nothing has assembled that paragraph's lines as a
    /// standalone unit it could prefix all at once.
    ///
    /// konoma's own renderer walks `model::Block`'s tree instead, and every block-rendering function
    /// that needs to wrap a prefix/style around a whole run of lines already assembles that run in
    /// full — via an isolated sub-`Writer`, decorated, *before* handing it back — rather than
    /// interleaving it, line by line, with its own parent's other output (see `render_quote`'s own
    /// doc comment for the isolate-then-decorate-then-prefix shape this produces, and
    /// `render_bar_prefixed_body`'s sibling one for an alert/`<details>` body). By the time either
    /// caller is ready to call this method, it already holds the exact, single prefix/style that run
    /// of lines needs, as a plain local value — there is nothing left for a stack to track: no
    /// caller of this method is ever nested inside another live call to it (confirmed: `render_quote`
    /// only ever prefixes already-fully-decorated lines, never lines a nested call is still
    /// assembling; `render_metadata_block` only ever runs at the very top of a document, never inside
    /// a quote or an alert/`<details>` body). A doubly-quoted line still ends up with two `>` markers
    /// the same way it always did — not from two entries on one stack read at once, but from two
    /// *separate* calls to this method, one per nesting level, each on its own `Writer` instance, each
    /// prefixing what the level below it already finished and handed back.
    fn emit_row_prefixed(
        &mut self,
        line: Line<'static>,
        prefix: Option<&Span<'static>>,
        style: Option<Style>,
    ) {
        self.after_math = false;
        self.fresh_boundary = false;
        let mut line = match style {
            Some(s) => line.patch_style(s),
            None => line,
        };
        if let Some(p) = prefix {
            line.spans.insert(0, Span::raw(" "));
            line.spans.insert(0, p.clone());
        }
        self.lines.push(line);
    }

    /// Appends `span` to the current row, or starts a fresh row with it when there isn't one yet.
    fn append_span(&mut self, span: Span<'static>) {
        match self.lines.last_mut() {
            Some(l) => l.push_span(span),
            None => self.emit_row(Line::from(vec![span])),
        }
    }

    fn emit_blank_row(&mut self) {
        self.emit_row(Line::default());
    }

    /// Consumes a still-pending math-paragraph-start slot (see `pending_para_start`'s own doc
    /// comment), opening it with the same two-part leading-blank-line convention
    /// `render_paragraph` applies unconditionally for a non-math paragraph — called from
    /// `ensure_fresh_after_math`, the shared checkpoint every path that is about to render real
    /// (non-lifted) content already runs through. A no-op once the slot is no longer pending (already
    /// opened this way, or already dropped — unopened — by a leading math lift; see
    /// `drop_pending_para_start`).
    fn open_pending_para_start(&mut self) {
        if let Some(gap_before) = self.pending_para_start.take() {
            if gap_before {
                self.emit_blank_row();
            }
            self.emit_blank_row();
            self.pending_block_gap = false;
        }
    }

    /// Drops a still-pending math-paragraph-start slot **without** opening it — called from
    /// `render_math_slot`, the one place a lifted expression can be the very first content a
    /// math-lifting paragraph ever renders (see `pending_para_start`'s own doc comment for why that
    /// shape gets no leading blank line at all, unlike ordinary text). A no-op once the slot is no
    /// longer pending.
    fn drop_pending_para_start(&mut self) {
        self.pending_para_start = None;
    }

    /// Pushes a new inline style onto the stack, patched on top of whatever is currently active
    /// (`cur_style`) — every span rendered until the matching `pop_inline_style` picks it up.
    fn push_inline_style(&mut self, style: Style) {
        self.inline_styles.push(self.cur_style().patch(style));
    }

    /// Pops the innermost inline style pushed by `push_inline_style`, reverting to whatever was
    /// active before it.
    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    /// Writes a run of inline text. An embedded `\n` inside one `Event::Text` payload — rare, but not
    /// impossible — starts a new physical line for every line after the first. Sets
    /// `pending_block_gap = false` unconditionally on its own way out — **never** `true`, in
    /// contrast to `render_heading`/`render_paragraph`/`render_rule`/`render_list`/`render_quote`,
    /// which all set it `true` when *they* finish. This is what makes a
    /// **tight** list item's own bare inline content (see `render_item`'s own doc comment) genuinely
    /// different from a real paragraph's: nothing in a plain sequence of `text`/`code`/`join_with_space`/
    /// `end_row`/inline-style calls, with no wrapping `end_paragraph`-equivalent at all, ever asks
    /// the *next* construct for a blank separator line.
    fn write_text(&mut self, text: &str) {
        for (i, line) in text.lines().enumerate() {
            if i > 0 {
                self.emit_blank_row();
            }
            let style = self.cur_style();
            self.append_span(Span::styled(line.to_string(), style));
        }
        self.pending_block_gap = false;
    }

    /// Writes an inline code span, styled with `KonomaStyles::code()`.
    fn write_code_span(&mut self, code: &str) {
        self.append_span(Span::styled(code.to_string(), self.styles.code()));
    }

    /// Joins two pieces of inline content with a single space — a soft line break in the source
    /// collapses to this. `walk_inline`, this method's only caller, only ever walks a `Heading`'s or
    /// `Paragraph`'s own inline content (see that function's own doc comment), never a metadata
    /// block's raw text — that one further shape `Doc::parse` never turns into a `Block` at all (see
    /// `model::is_unmodeled_container_tag`'s own doc comment) is read straight out of `doc.events`
    /// instead, by `render_metadata_block`, which calls `end_row` directly for a stray
    /// `Event::SoftBreak` rather than routing through this method (see that function's own doc
    /// comment for why real front matter never actually produces one to route in the first place).
    fn join_with_space(&mut self) {
        self.append_span(Span::raw(" "));
    }

    /// Ends the current row and starts a fresh one — a hard line break in the source.
    fn end_row(&mut self) {
        self.emit_blank_row();
    }

    /// Opens a link: remembers its URL until the matching `leave_link` closes it.
    fn enter_link(&mut self, url: String) {
        self.link = Some(url);
    }

    /// Closes the link opened by `enter_link`, appending its URL after the label — see the module
    /// doc comment's "How inline content is rendered" section for the exact span shape this produces.
    fn leave_link(&mut self) {
        if let Some(link) = self.link.take() {
            self.append_span(Span::raw(" ("));
            self.append_span(Span::styled(link, self.styles.link()));
            self.append_span(Span::raw(")"));
        }
    }

    /// Writes a task-list checkbox marker (`"[x] "` or `"[ ] "`). Two behaviors are konoma's own
    /// specification here, not accidents to be smoothed over:
    ///
    /// * It always writes a lowercase `x`, never `X` — this method only ever receives a `bool`
    ///   (whether the box is checked), which cannot distinguish the two in the first place;
    ///   `checked` here collapses `model::Task::state`'s own `'x'`/`'X'` distinction back down
    ///   to match (`render_item`'s own call site does that collapsing).
    /// * The two shapes it can produce are genuinely different, by design: when the *current*
    ///   line's own first span already reads `"- "` (a **tight**, unordered item — see
    ///   `render_item`'s own doc comment for exactly when that holds), the marker is spliced directly
    ///   into that span's own text, becoming one span reading `"- [x] "`. Every other case — a
    ///   **tight** ordered item's own `"N. "` marker span (one existing span, the marker becomes a
    ///   second one right after it), or **any loose item** (unordered or ordered alike), whose own
    ///   marker always lands on a *fresh, empty* line with **no** existing span at all (`render_item`
    ///   opens that blank line before calling this, precisely to strand the marker there — see its own
    ///   doc comment) — inserts the marker as a **new** span, immediately after whatever is already on
    ///   the line (nothing, for the loose case; the one marker span, for the tight-ordered case).
    ///   `decorate_extras`'s own `replace_task_checkbox`, downstream, recognizes the literal `[x]`/
    ///   `[ ]` text either way, so all shapes decorate identically on screen; the only consequence of
    ///   the split is which byte range the surrounding spans (the parts strictly before/after the
    ///   marker, when there are any) keep their own original styling from.
    ///
    ///   `Vec::insert`'s own index has to stay `<=` the current length — `spans.len().min(1)` is
    ///   exactly "after span 0 if there is one, otherwise at the only position an empty `Vec` has" —
    ///   principle #3 (`CLAUDE.md`), never crash: a fixed literal `1` here panicked on exactly one
    ///   shape (a **loose** item's own task marker, landing on that fresh, empty line with zero spans
    ///   on it) before this existed; see this method's own home tests.
    fn write_task_marker(&mut self, checked: bool) {
        let marker = if checked { 'x' } else { ' ' };
        let marker_span = Span::raw(format!("[{marker}] "));
        if let Some(line) = self.lines.last_mut() {
            if let Some(first_span) = line.spans.first_mut() {
                let content = first_span.content.to_mut();
                if content.ends_with("- ") {
                    let len = content.len();
                    content.truncate(len - 2);
                    content.push_str("- [");
                    content.push(marker);
                    content.push_str("] ");
                    return;
                }
            }
            let idx = line.spans.len().min(1);
            line.spans.insert(idx, marker_span);
        } else {
            self.append_span(marker_span);
        }
    }
}

/// Walks `events`, dispatching every event a `Heading`'s or `Paragraph`'s own inline content can
/// contain to the matching `Writer` method. Recurses one level for every
/// further inline `Start` it meets (`start_inline_tag`), so nested constructs (bold inside a link's
/// label, an image inside a link, ...) balance correctly, the same way `model::skip_inline_to`
/// balances the block model's own walk.
///
/// `stop`, when `Some`, is the `TagEnd` this call should consume and return on — used for every
/// *nested* call (`wrap_styled`, `Link`, `Image`): the caller has just consumed that construct's own
/// `Start`, so its `End` is still somewhere ahead in the very same slice, mixed in with whatever
/// else the construct contains. The *root* call — from `render_heading`/`render_paragraph`, or from
/// `render_item` rendering a tight item's own first (synthetic) paragraph directly — passes `None`
/// instead: their own `events` iterator is already exactly one block's own inline content — no
/// wrapping `Start`/`End` pair at all, on either side (see `BlockKind::Heading.inline`'s own doc
/// comment on that convention) — so there is nothing to stop *for*; the loop simply runs until the
/// iterator itself is exhausted. A bare `Event::End` reaching this loop with `stop == None` cannot
/// legitimately happen for well-formed input either way (nothing in this slice was left unbalanced
/// by `Doc::parse`'s own walk); the catch-all arm below drops it the same defensive way it drops
/// every other construct this file does not represent, rather than assuming it unreachable.
fn walk_inline<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    w: &mut Writer<'_>,
    stop: Option<TagEnd>,
) {
    while let Some(ev) = events.next() {
        match ev {
            Event::End(e) if Some(e) == stop => return,
            Event::Start(tag) => start_inline_tag(events, w, tag),
            Event::Text(t) => w.write_text(&t),
            Event::Code(c) => w.write_code_span(&c),
            Event::SoftBreak => w.join_with_space(),
            Event::HardBreak => w.end_row(),
            // `Html`/`InlineHtml`/`FootnoteReference`/`InlineMath`/`DisplayMath`: none of these produce
            // any output — silently dropped.
            // `TaskListMarker`/`Rule`/a mismatched `Event::End` cannot legitimately occur inside a
            // `Heading`'s or `Paragraph`'s own inline content at all (`Doc::parse`'s own walk already
            // balanced every construct correctly before this slice was ever handed to this file, and
            // a task marker is never folded into any `inline` range in the first place — see
            // `model::Task::state_at`'s own doc comment). Dropped the same defensive way rather than
            // assumed unreachable, so a pulldown-cmark edge case this file has not been tested
            // against still degrades (principle #3) instead of desyncing the walk.
            _ => {}
        }
    }
}

/// One inline `Start(tag)` `walk_inline` has just consumed: pushes/pops the matching style
/// (`Emphasis`/`Strong`/`Strikethrough`/`Subscript`/`Superscript`), threads a `Link`'s destination
/// through `enter_link`/`leave_link`, or — for `Image` — walks its alt-text content with no style change
/// and no destination recorded at all (see the module doc comment: `Image` renders alt text only).
/// Anything else cannot legitimately open inside a `Heading`'s
/// or `Paragraph`'s own inline content — CommonMark's grammar has no block-shaped construct there — so
/// it gets a defensive, balanced consume (`skip_balanced`, mirroring `model::skip_inline_to`) instead
/// of being assumed unreachable.
fn start_inline_tag<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    w: &mut Writer<'_>,
    tag: Tag<'a>,
) {
    match tag {
        Tag::Emphasis => wrap_styled(
            events,
            w,
            TagEnd::Emphasis,
            Style::new().add_modifier(Modifier::ITALIC),
        ),
        Tag::Strong => wrap_styled(
            events,
            w,
            TagEnd::Strong,
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Tag::Strikethrough => wrap_styled(
            events,
            w,
            TagEnd::Strikethrough,
            Style::new().add_modifier(Modifier::CROSSED_OUT),
        ),
        Tag::Subscript => wrap_styled(
            events,
            w,
            TagEnd::Subscript,
            Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
        Tag::Superscript => wrap_styled(
            events,
            w,
            TagEnd::Superscript,
            Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC),
        ),
        Tag::Link { dest_url, .. } => {
            w.enter_link(dest_url.into_string());
            walk_inline(events, w, Some(TagEnd::Link));
            w.leave_link();
        }
        Tag::Image { .. } => walk_inline(events, w, Some(TagEnd::Image)),
        other => skip_balanced(events, other.to_end()),
    }
}

/// `push_inline_style`, walk the tag's own body to `stop`, `pop_inline_style` — the shared shape of
/// every plain style-stacking inline tag (`Emphasis`/`Strong`/`Strikethrough`/`Subscript`/
/// `Superscript`).
fn wrap_styled<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    w: &mut Writer<'_>,
    stop: TagEnd,
    style: Style,
) {
    w.push_inline_style(style);
    walk_inline(events, w, Some(stop));
    w.pop_inline_style();
}

/// Defensive balance for a `Start` this file never expects to see inside a `Heading`'s or
/// `Paragraph`'s own inline content — see `start_inline_tag`'s own doc comment. Mirrors
/// `model::skip_inline_to`'s own balancing exactly, not reused directly: that one is specific to
/// `Peekable<OffsetIter>` (it needs offsets for the block model), while this walk needs neither
/// offsets nor peeking.
fn skip_balanced<'a>(events: &mut impl Iterator<Item = Event<'a>>, end: TagEnd) {
    while let Some(ev) = events.next() {
        match ev {
            Event::Start(t) => skip_balanced(events, t.to_end()),
            Event::End(e) if e == end => return,
            _ => {}
        }
    }
}

/// Heading attribute metadata (`{#id .class key=value}`) collected from `Tag::Heading`'s own fields.
/// `to_suffix` (below) is pure output *formatting* only, not a second judgment about document
/// *structure* (the kind of duplication this refactor's other reused functions — `decorate_headings_and_extras`,
/// `parse_alert_header`, ... — exist to avoid).
struct HeadingMeta {
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, Option<String>)>,
}

impl HeadingMeta {
    fn to_suffix(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(id) = &self.id {
            parts.push(format!("#{id}"));
        }
        for class in &self.classes {
            parts.push(format!(".{class}"));
        }
        for (key, value) in &self.attrs {
            match value {
                Some(v) => parts.push(format!("{key}={v}")),
                None => parts.push(key.clone()),
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(format!(" {{{}}}", parts.join(" ")))
        }
    }
}

/// Clones every `Event` out of `events` (an inline slice, `&doc.events[block_inline_range]`), in
/// order — cheap, not merely convenient: see `model::Walker::next`'s own doc comment for why cloning
/// one of these is never more than a couple of machine words. `walk_inline` and its own recursive
/// helpers all want an owned `Event<'a>` stream, matching `pulldown_cmark::Parser`'s own `Item` type,
/// so this is the one place a `&[(Event<'a>, Range<usize>)]` slice becomes that shape — every other caller in this
/// file goes through it rather than writing `events.iter().map(|(ev, _)| ev.clone())` again itself.
fn events_iter<'a, 'e>(
    events: &'e [(Event<'a>, Range<usize>)],
) -> impl Iterator<Item = Event<'a>> + 'e {
    events.iter().map(|(ev, _)| ev.clone())
}

/// Renders one `Heading` block into `w`, in place. `inline` is `Doc.events[block's own inline range]`,
/// read directly (no second parse — see the module doc comment).
///
/// Also maintains `w.heading_rule_shift` (see that field's own doc comment for the full mechanism):
/// incremented by `1` for every level-1/2 heading — `w` here is always the *un-prefixed* Writer a
/// heading is drawn into (a heading inside a `Quote`/alert/`<details>` renders into that container's
/// own isolated sub-`Writer`, whose `heading_rule_shift` is a fresh, separately-consulted counter —
/// see `render_quote`'s/`render_bar_prefixed_body`'s own doc comments — never `w` at any point where
/// a `>`/bar prefix is actually attached to a line) — exactly the condition under which
/// `decorate_headings`'s own `heading_level` check will, later, actually recognize this line and
/// insert a rule directly under it. A level 3–6 heading never increments this either way —
/// `decorate_headings` only ever adds a line for level 1/2 (see that function's own body); levels 3–6
/// have their own leading `#` span stripped *in place*, changing no line count at all.
fn render_heading(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    level: u8,
    inline: Range<usize>,
    meta: &HeadingMeta,
) {
    if w.pending_block_gap {
        w.emit_blank_row();
    }
    let heading_style = w.styles.heading(level);
    let hashes = format!("{} ", "#".repeat(level as usize));
    w.emit_row(Line::styled(hashes, heading_style));
    w.pending_block_gap = false;
    // `heading_trailing_images` — see its own doc comment — peels a trailing badge row off the
    // title text before drawing it, so a badge's own `dest_url` is not silently discarded the way
    // `walk_inline`'s own generic `Tag::Image` handling always discards one; the ordinary case (no
    // trailing images at all, `None`) draws the *whole* `inline` range exactly as before, with no
    // trailing images to render afterward.
    let (text_range, images) = match heading_trailing_images(doc, src, &inline) {
        Some((text_range, images)) => (text_range, images),
        None => (inline, Vec::new()),
    };
    walk_inline(&mut events_iter(&doc.events[text_range]), w, None);
    if let Some(suffix) = meta.to_suffix() {
        w.append_span(Span::styled(suffix, w.styles.heading_meta()));
    }
    if level <= 2 {
        w.heading_rule_shift += 1;
    }
    w.pending_block_gap = true;
    render_image_group(w, &images);
}

/// Renders one **real** `Paragraph` block into `w`, in place: the leading blank-line convention is a
/// two-part push — a conditional one if `w.pending_block_gap` was already owed, then an
/// *unconditional* second one regardless — before walking the paragraph's own inline content (see
/// `render_item`'s own doc comment for why that second, unconditional push is exactly what strands a
/// **loose** item's task marker and its own first line of text on a line of their own, one row below
/// the item's bullet/number). Never called for a **tight** list item's own first (synthetic)
/// paragraph — see `render_item`, which renders that one's inline content directly via `walk_inline`
/// instead, with no blank-line bookkeeping of its own at all.
fn render_paragraph(w: &mut Writer<'_>, doc: &Doc<'_>, inline: Range<usize>) {
    if w.pending_block_gap {
        w.emit_blank_row();
    }
    w.emit_blank_row();
    w.pending_block_gap = false;
    walk_inline(&mut events_iter(&doc.events[inline]), w, None);
    w.pending_block_gap = true;
}

/// Renders a leading YAML/pluses-style front-matter block straight out of `doc.events`, in place —
/// see `render_doc`'s own "raw, un-stripped YAML front-matter" doc comment for why this reads
/// `doc.events` directly instead of through a `Block`. A no-op — `doc` is left exactly as
/// `render_doc`'s own top-level dispatch found it — when `doc.events` carries no
/// `Event::Start(Tag::MetadataBlock(_))` at all, which is every caller except `[ui] md_frontmatter =
/// false` (see `render_doc`'s own call site).
///
/// Confirmed directly against pulldown-cmark 0.13.4's own event stream for a metadata block
/// (`Parser::new_ext` with `Options::ENABLE_YAML_STYLE_METADATA_BLOCKS`, dumped via `into_offset_iter`):
/// the block's *entire* raw text arrives as a **single** `Event::Text`, its internal line breaks
/// embedded literally rather than reported as separate `Event::SoftBreak`s — `parse_metadata_block`
/// (`firstpass.rs`) appends each raw source line the identical, non-inline-parsed way an indented code
/// block's own body does (`append_code_text`), so there is never a container boundary for pulldown-cmark
/// to have reported a soft break *at* in the first place. The `.lines()` split below is what actually
/// turns each YAML source line into its own screen row here. The `Event::SoftBreak` arm is therefore
/// never reached by real front matter; it stays as a defensive no-op for a shape today's pulldown-cmark
/// does not currently produce, rather than assumed unreachable.
///
/// Builds its own rows locally (`Span`/`Line` directly, `w.emit_row_prefixed(.., Some(style))`)
/// rather than going through `Writer::write_text`/`append_span`, its shared, generic counterparts
/// used by every *other* block-rendering function in this file: those two have no style parameter of
/// their own (they read `w.cur_style()`, the *inline*-emphasis stack `<em>`/`<strong>` push/pop use,
/// which front matter never touches at all), and front matter is the *only* place in the whole
/// document that needs every row it produces patched with one particular color the moment each row is
/// created — the same job `row_styles: Vec<Style>` used to do implicitly, by being read back inside
/// `emit_row` itself. Keeping that one narrow need local, rather than adding a style parameter to two
/// widely shared methods for a single caller, is what let `row_styles` disappear from `Writer`
/// entirely — see [`Writer::emit_row_prefixed`]'s own doc comment for the fuller reasoning.
fn render_metadata_block(w: &mut Writer<'_>, doc: &Doc<'_>) {
    let Some(start) = doc
        .events
        .iter()
        .position(|(ev, _)| matches!(ev, Event::Start(Tag::MetadataBlock(_))))
    else {
        return;
    };
    // Opens the front-matter block: the leading blank-line convention, a styled "---" rule, then a
    // blank row before the body.
    if w.pending_block_gap {
        w.emit_blank_row();
    }
    let style = w.styles.metadata_block();
    w.emit_row_prefixed(Line::from("---"), None, Some(style));
    w.emit_row_prefixed(Line::default(), None, Some(style));
    for ev in events_iter(&doc.events[start + 1..]) {
        match ev {
            Event::End(TagEnd::MetadataBlock(_)) => break,
            // Mirrors `Writer::write_text`'s own `.lines()` split exactly (see this function's own
            // doc comment for why this doesn't just call that method): a fresh row per embedded line
            // after the first, each new row's style patched at creation via `emit_row_prefixed`,
            // every span itself carrying no style of its own (`cur_style()` is always `Style::default()`
            // here — front matter never opens an inline-emphasis span) so it reads back through the
            // row's own patched style, the identical mechanism every other styled row in this file
            // uses.
            Event::Text(t) => {
                for (i, line) in t.lines().enumerate() {
                    if i > 0 {
                        w.emit_row_prefixed(Line::default(), None, Some(style));
                    }
                    let span = Span::styled(line.to_string(), w.cur_style());
                    match w.lines.last_mut() {
                        Some(l) => l.push_span(span),
                        None => w.emit_row_prefixed(Line::from(vec![span]), None, Some(style)),
                    }
                }
                w.pending_block_gap = false;
            }
            // Defensive: see this function's own doc comment for why real front matter never
            // actually reaches this arm.
            Event::SoftBreak => w.emit_row_prefixed(Line::default(), None, Some(style)),
            // Nothing else can legitimately appear inside a metadata block's own raw content (see
            // this function's own doc comment). Dropped the same defensive way `walk_inline`'s own
            // catch-all is, rather than assumed unreachable.
            _ => {}
        }
    }
    // Closes the front-matter block with a second styled "---" rule.
    w.emit_row_prefixed(Line::from("---"), None, Some(style));
    w.pending_block_gap = true;
}

/// `math_slot`/`width` bundled together for `Writer::math`. Owned by `Writer` itself, not
/// passed as a separate parameter — see that struct's own doc comment on `math` for why. Placements
/// `render_math_slot` produces push directly onto `Writer::images` (the one, shared accumulator every
/// image/mermaid/math placement uses — see that field's own doc comment), not a copy held here.
struct MathCtx<'a> {
    slot: &'a dyn Fn(&str, bool) -> MathSlot,
    width: u16,
}

/// `mermaid_slot`/`mermaid_caption`/`width`/`fences_on`/a running fence ordinal, bundled together for
/// `Writer::mermaid` — the mermaid analogue of `MathCtx`. Unlike `math`, this is never `Option`: see
/// `Writer::mermaid`'s own doc comment for why mermaid extraction's own "is it on at all" question is
/// answered by `fences_on` (a probe result, computed once in `render_doc`) rather than by the
/// presence/absence of the context struct itself.
struct MermaidCtx<'a> {
    slot: &'a dyn Fn(&str) -> MermaidSlot,
    /// The pre-translated affordance text (`"Enter: full screen"` or its translated equivalent) —
    /// `render_doc`'s own `mermaid_caption: &str` parameter, forwarded verbatim into
    /// `super::mermaid_placeholder_lines`'s own identically-named parameter (see that function's own
    /// doc comment for why the `"◇ mermaid"` prefix itself stays untranslated, as the Tab-focus
    /// sentinel `is_mermaid_header_span` keys off).
    caption: &'a str,
    width: u16,
    /// Whether extraction is on *at all*, for the whole document — the identical probe production's
    /// own `render_markdown_with_images` runs (`!matches!(mermaid_slot(""), MermaidSlot::Text)`),
    /// computed once by `render_doc` rather than re-probed at every fence (see that function's own
    /// construction site for why: an empty string is otherwise indistinguishable from a real, empty
    /// fence body — probing it once, up front, keeps a genuinely empty fence's own real slot answer
    /// from ever being confused with this probe's).
    fences_on: bool,
    /// The running mermaid-fence ordinal (`ImagePlacement.fence_ord`) — mirrors production's own
    /// `fence_ord` counter in `render_markdown_with_images` exactly: incremented by `1` every time a
    /// fence *eligible* for extraction is encountered (`render_code_block_dispatch`'s own mermaid
    /// branch — gated the identical way production's own `split_block_parts` is, see
    /// `BlockCtx.math_here`'s own doc comment), **before** checking whether its own body is empty —
    /// so an empty mermaid fence (which still falls back to the legacy text diagram, not a code
    /// block) still consumes an ordinal slot, keeping later fences' own numbering in lockstep with
    /// production's identical "increment first, check emptiness after" order (see
    /// `render_mermaid_slot`'s own doc comment). A fence this file never even attempts to extract in
    /// the first place — quote-nested, or reached while `fences_on` is `false` — never touches this
    /// counter at all, exactly mirroring production's own `parse_fence`-based line scan never
    /// recognizing one there either (see `BlockCtx.math_here`'s own doc comment).
    ord: usize,
}

/// Index bounds (`(start, end)`, `end` exclusive, the break event itself in neither side) of every
/// run `events` splits into at a top-level (`depth == 0` — never inside an open tag such as a `Link`'s
/// own span) `SoftBreak`/`HardBreak`. A multi-line paragraph with no blank line separating its
/// physical source lines is exactly this shape: pulldown-cmark reports one `SoftBreak` event per line
/// join, at the paragraph's own top level, so this recovers "one segment per physical source line"
/// without re-parsing anything (see the module doc comment's own "How inline content is rendered"
/// section on why re-parsing is avoided everywhere else in this file). The index form (rather than
/// borrowed slices directly) is what lets `paragraph_trailing_images` reconstruct a single, contiguous
/// `Range<usize>` spanning *several* leading segments — the shape `render_paragraph`/`walk_inline`
/// themselves need — something a `Vec` of already-split slices cannot hand back without re-deriving
/// the same boundaries a second time; `split_top_level_breaks` (below) is the slice-returning form for
/// callers (`paragraph_as_block_images`) that only ever need one segment at a time on its own.
fn top_level_segment_bounds(events: &[(Event<'_>, Range<usize>)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    for (i, (ev, _)) in events.iter().enumerate() {
        match ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            Event::SoftBreak | Event::HardBreak if depth == 0 => {
                out.push((start, i));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push((start, events.len()));
    out
}

/// `top_level_segment_bounds`, sliced against `events` — one borrowed slice per resulting run. See
/// that function's own doc comment for the split rule and why the two forms coexist.
fn split_top_level_breaks<'e, 'd>(
    events: &'e [(Event<'d>, Range<usize>)],
) -> Vec<&'e [(Event<'d>, Range<usize>)]> {
    top_level_segment_bounds(events)
        .into_iter()
        .map(|(s, e)| &events[s..e])
        .collect()
}

/// If `seg` (one line-segment of a paragraph's own inline events — see `split_top_level_breaks`) is
/// *nothing but* a single standalone image, returns its `(alt, url)` — either a Markdown `![alt](url)`
/// (optionally wrapped in a link, `[![alt](url)](href)`; the image's own `dest_url` is used, never the
/// wrapping link's own `href` — matching `markdown.rs`'s own line-based `extract_md_img`'s identical
/// "the link is recognized only to be stepped past" contract), or a raw HTML `<img src=...>` tag
/// (optionally wrapped in layout tags like `<a>`/`<p>` — `super::extract_block_image`, the identical
/// function `render_html_block_from_model`/production's own `split_block_parts_masked` call for the
/// same shape; reconstructed from `src`, never from an event's own `CowStr` payload, since an HTML
/// attribute's entities are not decoded by pulldown-cmark the way real text is).
///
/// **The `image_starts != 1` check below, not merely "is the *first* event `Start(Image)` and the
/// *last* one `End(Image)`", is what tells one image apart from several concatenated ones sharing an
/// outer link/segment** — the exact defect `render_html_block_from_model`'s own doc comment already
/// recounts fixing for the HTML "centered banner" shape (a whole multi-image block naively handed to
/// `extract_block_image` once "finds only the *first* `<img>` tag... and, on a match, rendered *only*
/// that one image..., silently discarding every badge after the first"): a paragraph whose *first*
/// physical line is `[![a](u1)](h1)` and whose *next* line (no blank line between — soft-continued
/// into the *same* Markdown paragraph) is `[![b](u2)](h2)` produces the event sequence
/// `Start(Link) Start(Image) Text(a) End(Image) End(Link) SoftBreak Start(Link) Start(Image) Text(b)
/// End(Image) End(Link)` — whose *first* event is `Start(Link)` and *last* is `End(Link)`, the exact
/// shape the outer link-unwrap below expects for a *single* wrapped image, even though two entirely
/// separate ones sit end to end inside it. Checking only the endpoints (this function's own
/// pre-`md-block-walk` version) unwrapped that "outer link" anyway, found `inner`'s own first/last
/// events were `Start(Image)`/`End(Image)` (the *first* badge's own pair — the *second* badge's own
/// `End(Link)`/`SoftBreak`/`Start(Link)`/`Start(Image)` all end up *inside* `inner` by the same wrong
/// assumption), and rendered one, wrong, merged line: both alt texts concatenated, only the *first*
/// image's own URL kept, the second's silently dropped — confirmed against `~/.cargo/registry`'s own
/// real crate READMEs (the shields.io "badge row" idiom — one badge per line, no blank line between —
/// is extremely common there) rather than merely reasoned about: hundreds of files lost a badge's own
/// URL/label text this way before this function existed. This function is only ever handed one
/// already-`split_top_level_breaks`-separated line-segment at a time, so a *second* image sharing that
/// segment can only mean exactly this "concatenated, not truly one" shape — never a legitimately
/// nested one (Markdown's own alt-text position never permits another `Image` to start inside it).
fn segment_as_block_image(
    src: &str,
    seg: &[(Event<'_>, Range<usize>)],
) -> Option<(String, String)> {
    if seg.is_empty() {
        return None;
    }
    let inner = match seg.first() {
        Some((Event::Start(Tag::Link { .. }), _)) => {
            if !matches!(seg.last(), Some((Event::End(TagEnd::Link), _))) {
                return None;
            }
            &seg[1..seg.len() - 1]
        }
        _ => seg,
    };
    if inner.is_empty() {
        return None;
    }
    let image_starts = inner
        .iter()
        .filter(|(ev, _)| matches!(ev, Event::Start(Tag::Image { .. })))
        .count();
    if image_starts > 0 {
        if image_starts != 1
            || !matches!(inner.first(), Some((Event::Start(Tag::Image { .. }), _)))
            || !matches!(inner.last(), Some((Event::End(TagEnd::Image), _)))
        {
            return None;
        }
        let Some((Event::Start(Tag::Image { dest_url, .. }), _)) = inner.first() else {
            return None;
        };
        let url = dest_url.to_string();
        if url.is_empty() {
            // Matches `extract_md_img`'s own `if url.is_empty() { return None; }` — an image with no
            // destination at all is not extracted in production either.
            return None;
        }
        let alt = image_alt_text(&inner[1..inner.len() - 1]);
        return Some((alt, url));
    }
    // Not a real Markdown `Tag::Image` event at all — try the HTML shape instead: `inner` must be
    // *nothing but* inline HTML (an image event above would already have matched, so this can never
    // double-match one) for `extract_block_image` to even apply the same way it would to a genuine raw
    // source line; any real text, emphasis, or other inline construct mixed in falls through to
    // ordinary paragraph rendering instead, unchanged.
    if !inner
        .iter()
        .all(|(ev, _)| matches!(ev, Event::InlineHtml(_) | Event::Html(_)))
    {
        return None;
    }
    let start = inner.first()?.1.start;
    let end = inner.last()?.1.end;
    super::extract_block_image(&src[start..end])
}

/// Splits `seg` (one line-segment — see `split_top_level_breaks`) into one slice per **top-level**
/// image-or-link-wrapped-image unit, when `seg` is nothing but a run of those separated only by
/// whitespace — `None` for anything else (real text, another inline construct, an HTML-only segment
/// — the latter still falls through to `segment_as_block_image`'s own whole-segment HTML branch,
/// unaffected by this function at all, since neither `Event::Html` nor `Event::InlineHtml` matches
/// either arm below).
///
/// Exists for one real, confirmed shape `segment_as_block_image`'s own single-image contract cannot
/// see at all: **two or more badges sharing one physical source line**, separated only by a literal
/// space rather than each getting its own line — `phf-0.11.3/README.md`'s own `[![CI](https://\
/// github.com/rust-phf/rust-phf/actions/workflows/ci.yml/badge.svg)](https://github.com/rust-phf/\
/// rust-phf/actions/workflows/ci.yml) [![Latest Version](https://img.shields.io/crates/v/phf.svg)]\
/// (https://crates.io/crates/phf)`, both badges on one line. `split_top_level_breaks` only ever
/// splits at a `SoftBreak`/`HardBreak` — a *line join* — so two images on the *same* line, with no
/// line join anywhere between them, land in one shared segment together; `segment_as_block_image`'s
/// own `image_starts != 1` check (deliberately: see that check's own doc comment, "more than one
/// image sharing an outer link/segment") correctly refuses to guess which of the two `dest_url`s is
/// the "real" one and returns `None` for the *whole* segment — which used to mean the *whole
/// paragraph* fell through to ordinary inline rendering, where a bare `Tag::Image` shows only its
/// own alt text (`start_inline_tag`'s `Tag::Image` arm — `walk_inline` on its own children, no
/// `render_image_group` call, no URL recorded anywhere), losing **both** badges' own image URLs
/// entirely (not just the wrapping links' own `href`s, already excluded by design — see
/// `render_image_group`) — confirmed directly against the real registry sweep this function exists to
/// close: `vello_common`/`vello_cpu`/`fearless_simd`/`rangemap`/`atomic`/`base64`/`bit-set`/`bit-vec`
/// and more all write two or more shields.io badges on one shared line this same way.
///
/// A unit is either a bare `Start(Image)..End(Image)` run or a `Start(Link)..End(Link)` run wrapping
/// exactly one (`segment_as_block_image` itself still does the "exactly one, and only one" check —
/// this function only finds each unit's own boundary, matching braces by depth for the identical
/// reason: a
/// `Link` can wrap an `Image` — never the reverse, CommonMark disallows nesting either kind inside
/// itself — so simple depth-tracking on *any* `Start`/`End` pair, stopping at the matching top-level
/// close of *this* unit's own opening kind, can never mismatch which `End` belongs to which `Start`).
/// One top-level image-or-link-wrapped-image unit's own event slice, as [`split_top_level_image_\
/// units`] returns them (`clippy::type_complexity`'s own suggestion — this exists only to name the
/// type, not to change what it means).
type ImageUnits<'e, 'd> = Vec<&'e [(Event<'d>, Range<usize>)]>;

fn split_top_level_image_units<'e, 'd>(
    src: &str,
    seg: &'e [(Event<'d>, Range<usize>)],
) -> Option<ImageUnits<'e, 'd>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < seg.len() {
        if let Event::Text(t) = &seg[i].0 {
            if t.trim().is_empty() {
                i += 1;
                continue;
            }
            return None; // real text alongside an image on this line — not a pure badge row.
        }
        // A trailing (or interspersed) HTML comment on the *same* physical line as a badge —
        // `rangemap-1.7.1/README.md`'s own `[![Rust](…)](…) <!-- Don't forget to update the GitHub
        // actions config… -->` — is harmless the identical way whitespace is: it draws nothing
        // (`render_html_block` on it reduces to empty), so skipping it here does not silently drop
        // any visible content. A **non**-comment (or otherwise non-empty) `Html`/`InlineHtml` still
        // falls through to the `_ => return None` arm below, unchanged — this only ever widens what
        // counts as "harmless", never what counts as an image.
        if let Event::Html(_) | Event::InlineHtml(_) = &seg[i].0 {
            if render_html_block(&src[seg[i].1.clone()]).is_empty() {
                i += 1;
                continue;
            }
            return None;
        }
        let wants_end = match &seg[i].0 {
            Event::Start(Tag::Link { .. }) => TagEnd::Link,
            Event::Start(Tag::Image { .. }) => TagEnd::Image,
            _ => return None, // any other construct — not a pure badge row either.
        };
        let mut depth = 0i32;
        let mut j = i + 1;
        let end_idx = loop {
            if j >= seg.len() {
                return None; // unbalanced within this segment.
            }
            match &seg[j].0 {
                Event::Start(_) => depth += 1,
                Event::End(e) if *e == wants_end && depth == 0 => break j,
                Event::End(_) => depth -= 1,
                _ => {}
            }
            j += 1;
        };
        out.push(&seg[i..=end_idx]);
        i = end_idx + 1;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// [`segment_as_block_image`], generalized to one or more images sharing a single line-segment
/// (`split_top_level_image_units` — see that function's own doc comment for exactly which real
/// shape this closes). Every caller of the old, single-image function goes through this one instead;
/// a segment that is genuinely just one image behaves identically either way (`split_top_level_\
/// image_units` returns a single-unit `Vec`, `segment_as_block_image` on that one unit is the exact
/// same call the old code made directly), and a segment `split_top_level_image_units` cannot make
/// sense of at all (real text, an HTML-only line, ...) falls through to `segment_as_block_image`
/// itself unchanged, wrapped in a one-element `Vec` on a match.
///
/// A genuinely **empty** segment (`seg.is_empty()`) — real, not a defensive-only case:
/// `top_level_segment_bounds` pushes one whenever two break events land back to back with nothing
/// between them, which is exactly what a bare `\` line (`"row one\n\\\nrow two\n"` — a hard line
/// break on a line of its own, the standard Markdown idiom for a visible gap *inside* one paragraph
/// without starting a new one — GitHub renders shields.io "badge row" READMEs with it constantly)
/// produces: pulldown-cmark reports the line above's own trailing newline as one `SoftBreak`, then
/// the `\`-line's own escape-newline as a second, immediately following `HardBreak`, with **no**
/// event of any kind between them — confirmed directly: `vello_common-0.0.9/README.md`'s own six-
/// badge banner, split into two rows of three by exactly this `\` line. Returns `Some(vec![])` for
/// this shape specifically, **not** `None` — an empty segment carries no real text of any kind (the
/// disqualifying condition every other arm here checks for), so it can never be the reason a
/// "nothing but standalone images" paragraph should stop being recognized as one; `segment_as_\
/// block_image`'s own `if seg.is_empty() { return None; }`, reached without this check, is what
/// used to make this whole 3-badges-then-a-gap-then-3-more shape fail as **whole-paragraph** image
/// extraction the instant it reached the gap — losing every badge from the row *before* it too, not
/// merely swallowing the (rendered) `\` line, when the real paragraph fell through to ordinary
/// inline rendering as a single, whole unit (confirmed directly in this module's own development,
/// the same file: three unrelated badges fused onto one merged inline-link line, all three losing
/// their own image `src`, with the *later* row of three — past the point `paragraph_trailing_\
/// images`'s own peel-from-the-end still succeeded — rendered as real images regardless, further
/// confirming the empty segment, not anything about the badges themselves, was the actual defect).
fn segment_as_block_images(
    src: &str,
    seg: &[(Event<'_>, Range<usize>)],
) -> Option<Vec<(String, String)>> {
    if seg.is_empty() {
        return Some(Vec::new());
    }
    if let Some(units) = split_top_level_image_units(src, seg) {
        let mut out = Vec::with_capacity(units.len());
        for unit in units {
            out.push(segment_as_block_image(src, unit)?);
        }
        return Some(out);
    }
    segment_as_block_image(src, seg).map(|img| vec![img])
}

/// If `inline` (a `Paragraph`'s own event-index range into `doc.events` — see `model::Doc.events`'s
/// own doc comment) represents nothing but a sequence of standalone images — one per physical source
/// line (`split_top_level_breaks`), each recognized by `segment_as_block_image` — returns them all, in
/// document order. A paragraph with no `SoftBreak`/`HardBreak` at all (the common case: one image,
/// alone, with blank lines on both sides) is exactly the one-segment instance of this same shape, so
/// this subsumes what a narrower "is the whole paragraph one image" check used to do on its own.
///
/// Reads straight from `doc.events` (only `segment_as_block_image`'s own HTML branch ever reads
/// `src[range]`, and only for a byte range `doc.events` itself already reported) — so, unlike
/// `render_table_from_model`/`render_html_block_from_model`'s own naive whole-block slice, this has no
/// quote-marker-survival concern at all (`Doc.events` never represents a container's own marker as an
/// event to begin with; see `model::Doc.events`'s own doc comment). It is `BlockCtx.math_here`'s own
/// value — not anything this function itself checks — that decides whether extraction is *eligible* at
/// this position: production's own line-based `extract_block_image` still rejects a quote-nested
/// standalone-image line outright (its own leading `>` defeats the "prefix must be empty or exactly
/// `[`" check the same way it defeats `parse_fence`'s own fence-open check — see `BlockCtx.math_here`'s
/// own doc comment for the full reasoning, and the confirmed, documented production behavior it
/// cites), so matching that call site's own eligibility gate is what keeps this file's output aligned
/// with production even though this function *could*, on its own, safely extract a quote-nested one
/// too.
///
/// A paragraph mixing a standalone-image line with a line of *real* text (no blank line between) is
/// **not** extracted here — the text-carrying segment fails `segment_as_block_image` and the whole
/// paragraph falls through to ordinary rendering (each image shown as inline alt-text, matching this
/// file's own `Image` handling in `start_inline_tag`) — still a real, narrow, documented gap against
/// production's own per-*line* `extract_block_image` (which pulls a standalone-image line out
/// regardless of what a lazily-continued neighbor line holds); not exercised by the parity corpus, and
/// not the shape the real-world sweep (`app::md_real_file_sweep_tests`) that motivated this function's
/// own image-count fix above actually found losing content — every affected real file was a run of
/// *consecutive* image-only lines (the "badge row" idiom), which this function now handles in full.
fn paragraph_as_block_images(
    doc: &Doc<'_>,
    src: &str,
    inline: &Range<usize>,
) -> Option<Vec<(String, String)>> {
    let events = &doc.events[inline.clone()];
    if events.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for seg in split_top_level_breaks(events) {
        out.extend(segment_as_block_images(src, seg)?);
    }
    Some(out)
}

/// `(the text-only prefix's own inline range, its trailing images)` — `paragraph_trailing_images`'s
/// own return shape, named so its signature reads plainly instead of tripping clippy's
/// `type_complexity` lint on the raw nested tuple.
type TextPrefixAndTrailingImages = (Range<usize>, Vec<(String, String)>);

/// Peels a **trailing run of standalone image units** off the end of a *heading's* own inline event
/// stream — `render_heading`'s own only caller, and this file's only extraction of any kind for a
/// heading at all: `walk_inline`'s generic `Tag::Image` handling (`start_inline_tag`) shows only an
/// image's own alt text, discarding its `dest_url` entirely, matching every *other* inline position
/// this file draws an image inline (bold/italic text, a list item's own tight paragraph, ...) — but
/// unlike those, a heading is often *itself* a badge row's own home: `rav1e-0.8.1/README.md`'s own
/// `# rav1e [![Actions Status][actions badge]][actions] [![CodeCov][codecov badge]][codecov]`, and a
/// setext heading whose title text is lazily continued by standalone image lines with no blank line
/// anywhere (`ab_glyph-0.2.32/README.md`'s own `ab_glyph\n[![crates.io](…)](…)\n[![Documentation]\
/// (…)](…)\n========` — CommonMark correctly folds the whole thing into one heading's own inline
/// content; a narrower, previously *deliberately left open* gap this function now closes, see this
/// sweep's own `docs`/git history for that earlier decision and the direct confirmation the shape is
/// real and common enough in this same registry to close properly rather than leave open).
///
/// Generalizes `paragraph_trailing_images`'s own "text, then one or more standalone image lines"
/// shape from *physical-line* granularity (`top_level_segment_bounds`, which needs a `SoftBreak`/
/// `HardBreak` between every unit) to *unit* granularity (`split_top_level_image_units`'s own single
/// image-or-link-wrapped-image shape), because — unlike the paragraphs `paragraph_trailing_images`
/// itself was built for — a heading's own badges are just as often written on the **same** physical
/// line as the title text with nothing but a space between (`rav1e`'s own case above has no
/// `SoftBreak`/`HardBreak` anywhere in it at all, so `paragraph_trailing_images`'s own line-based
/// split would find nothing to peel). Kept as a **separate** function from `paragraph_trailing_\
/// images` rather than generalizing that one in place, deliberately: a heading's own real title text
/// is never itself "just images" the way `paragraph_as_block_images`'s own top-level image-only
/// paragraph is, so the two functions' own contracts genuinely differ at the type-of-caller level,
/// and keeping them apart means a future change to either can never risk silently perturbing the
/// other's own, already pinned paragraph behavior (this file's own `paragraph_trailing_images`
/// tests, in `mod tests` below).
///
/// Walks `inline`'s own top-level events **once**, front to back, classifying each top-level item as
/// exactly one of: a whitespace-only `Text` or a comment-reducing `Html`/`InlineHtml` (transparent —
/// neither an image nor real title text, the identical two "harmless" exceptions `split_top_level_\
/// image_units`/`segment_as_block_images` already establish for a *paragraph's* own badge row, for
/// the identical reason: a bare `\` hard-break line between two rows of badges, and an inline HTML
/// comment trailing a badge on the very same physical line, both real, confirmed shapes — see those
/// two functions' own doc comments); a `Start(Link)`/`Start(Image)` run through its own matching
/// `End` at the same top-level depth, classified as an image via `segment_as_block_image` on that
/// exact unit slice if it resolves to exactly one image, real (disqualifying) content otherwise (a
/// link wrapping real text, several images sharing one unit, ...); or anything else, always real.
/// The **longest trailing run of image classifications** — skipping over transparent items, which
/// never break a run — is what gets peeled; `None` when that run is empty (the ordinary heading,
/// unchanged) or spans the *whole* heading (an image-only heading is not a shape this sweep's own
/// real corpus exercises, and falls through to ordinary, pre-existing inline rendering unchanged —
/// not a regression this function introduces).
/// `(start_idx, end_idx_exclusive, image-if-any)` — one entry per non-transparent top-level item
/// [`heading_trailing_images`] classifies its own heading's inline content into, in document order
/// (`clippy::type_complexity`'s own suggestion — this exists only to name the type, not to change
/// what it means).
type HeadingItems = Vec<(usize, usize, Option<(String, String)>)>;

fn heading_trailing_images(
    doc: &Doc<'_>,
    src: &str,
    inline: &Range<usize>,
) -> Option<TextPrefixAndTrailingImages> {
    let events = &doc.events[inline.clone()];
    if events.is_empty() {
        return None;
    }
    // One entry per non-transparent top-level item, in document order; a whitespace-only/comment-
    // reducing item contributes no entry at all.
    let mut items: HeadingItems = Vec::new();
    let mut i = 0usize;
    while i < events.len() {
        match &events[i].0 {
            Event::Text(t) if t.trim().is_empty() => i += 1,
            // A `SoftBreak`/`HardBreak` between two lines of a setext heading's own lazily-continued
            // title (`ab_glyph-0.2.32/README.md`'s own `ab_glyph\n[badge1]\n[badge2]\n========` —
            // pulldown-cmark reports one `SoftBreak` per line join, exactly the same shape
            // `top_level_segment_bounds` itself splits *on* rather than treats as content) is
            // harmless the identical way a plain space between two badges on one physical line is —
            // neither is real title text, and skipping either without disqualifying the run is what
            // lets this function recognize a badge row regardless of whether it is one physical line
            // or several: confirmed directly, this module's own development — without this arm, the
            // very first `SoftBreak` scanning backward from the end stopped the trailing-run search
            // dead, peeling only the *last* line's own badge (or none at all) off a multi-line row.
            Event::SoftBreak | Event::HardBreak => i += 1,
            Event::Html(_) | Event::InlineHtml(_)
                if render_html_block(&src[events[i].1.clone()]).is_empty() =>
            {
                i += 1;
            }
            Event::Start(Tag::Link { .. }) | Event::Start(Tag::Image { .. }) => {
                let wants_end = if matches!(events[i].0, Event::Start(Tag::Link { .. })) {
                    TagEnd::Link
                } else {
                    TagEnd::Image
                };
                let mut depth = 0i32;
                let mut j = i + 1;
                let end_idx = loop {
                    if j >= events.len() {
                        break events.len(); // unbalanced within this heading — treat as real content.
                    }
                    match &events[j].0 {
                        Event::Start(_) => depth += 1,
                        Event::End(e) if *e == wants_end && depth == 0 => break j + 1,
                        Event::End(_) => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                };
                let img = segment_as_block_image(src, &events[i..end_idx]);
                items.push((i, end_idx, img));
                i = end_idx;
            }
            _ => {
                items.push((i, i + 1, None));
                i += 1;
            }
        }
    }
    let mut split = items.len();
    while split > 0 && items[split - 1].2.is_some() {
        split -= 1;
    }
    if split == 0 || split == items.len() {
        return None;
    }
    let images: Vec<(String, String)> = items[split..]
        .iter()
        .map(|(_, _, img)| {
            img.clone()
                .expect("every item past `split` is an image by construction")
        })
        .collect();
    // `items[split].0` is an index *relative to `events`* (`= &doc.events[inline.clone()]`), not an
    // absolute one — `inline` (like `BlockKind::Heading.inline` itself) is an **event-index** range
    // into `doc.events`, not a byte range into `src` (the same distinction `paragraph_trailing_\
    // images`'s own `inline.start + text_end_rel` below already gets right) — `inline.start + …`
    // converts it back to the absolute event index `doc.events[text_range]` needs.
    let text_end_idx = items[split].0;
    let text_range = inline.start..(inline.start + text_end_idx);
    Some((text_range, images))
}

/// If `inline` is a real paragraph of ordinary text **immediately followed**, with no blank line, by
/// one or more standalone image lines (`segment_as_block_image`, one per physical source line — the
/// identical per-line unit `paragraph_as_block_images` itself splits on) — the "intro sentence, then a
/// badge/screenshot below it" idiom, confirmed against real content
/// (`~/.cargo/registry/.../zune-jpeg-*/Benches.md`: `"...of (now defunct?) [Cutefish OS] default \
/// wallpaper.\n![img](benches/images/speed_bench.jpg)\n"`, no blank line between the sentence and the
/// image) — returns `(the text-only prefix's own inline range, the trailing images, in document
/// order)`. `None` when the paragraph has no top-level break at all, when its *last* segment is not an
/// image (nothing to peel off the end), or when *every* segment is an image (that shape is
/// `paragraph_as_block_images`'s own, checked first by every caller — see `try_render_paragraph_as_\
/// image`'s own doc comment — so this function is never even asked about it: reaching this function at
/// all already means that check failed).
///
/// **Does not** handle the mirror shape — one or more standalone image lines *first*, real text lazily
/// continuing right after — `paragraph_as_block_images`'s own pre-`md-block-walk` doc comment already
/// named that a real, narrower gap in this stage's own coverage (not exercised by the parity corpus,
/// and not the shape the real-world sweep motivating *this* function's own trailing-image half
/// actually found losing content — every real instance found was trailing, never leading).
fn paragraph_trailing_images(
    doc: &Doc<'_>,
    src: &str,
    inline: &Range<usize>,
) -> Option<TextPrefixAndTrailingImages> {
    let events = &doc.events[inline.clone()];
    let bounds = top_level_segment_bounds(events);
    if bounds.len() < 2 {
        return None; // no top-level break at all — nothing to peel a trailing run off of.
    }
    let mut images = Vec::new();
    let mut trailing = 0usize;
    for &(s, e) in bounds.iter().rev() {
        match segment_as_block_images(src, &events[s..e]) {
            Some(imgs) => {
                // `imgs` is already in document order (this one segment's own left-to-right
                // reading); pushed here in the *reverse* per-segment order the outer walk visits
                // them in, so the final `images.reverse()` below restores overall document order
                // for a multi-image segment exactly the same way it already does across segments.
                images.extend(imgs.into_iter().rev());
                trailing += 1;
            }
            None => break,
        }
    }
    if trailing == 0 || trailing == bounds.len() {
        return None;
    }
    images.reverse(); // collected innermost-out (last segment first); restore document order.
    let text_end_rel = bounds[bounds.len() - trailing - 1].1;
    let text_range = inline.start..(inline.start + text_end_rel);
    Some((text_range, images))
}

/// The mirror of [`paragraph_trailing_images`]: peels one or more standalone image **lines** off the
/// **front** of a paragraph whose own tail, immediately after (no blank line), is real text —
/// `rusty-fork-0.3.1/README.md`'s own `[![](http://meritbadge.herokuapp.com/rusty-fork)](https://\
/// crates.io/crates/rusty-fork)\nRusty-fork provides a way to "fork" unit tests into separate \
/// processes.\n`, one (empty-alt) badge line immediately followed by the crate's own real intro
/// sentence, no blank line between them. `paragraph_trailing_images`'s own doc comment named this
/// exact shape ("the mirror shape... a real, narrower gap... not exercised by the parity corpus")
/// as deliberately left open at the time it was written; closed here once this sweep confirmed it as
/// real, non-hypothetical content loss (not merely a hypothetical gap) rather than left open a
/// second time.
///
/// Returns `(the leading images, in document order, the text-only suffix's own inline range)`.
/// `None` under the identical three conditions `paragraph_trailing_images` itself returns `None`
/// for, mirrored front-to-back: no top-level break at all, the paragraph's own *first* segment is
/// not an image (nothing to peel off the front), or *every* segment is an image (`paragraph_as_\
/// block_images`'s own shape, already checked first by every caller — see `try_render_paragraph_\
/// as_image`'s own doc comment — so this function is never even asked about it either).
/// `(the leading images, in document order, the text-only suffix's own inline range)` —
/// [`paragraph_leading_images`]'s own return shape, the mirror of `TextPrefixAndTrailingImages`
/// (`clippy::type_complexity`'s own suggestion — this exists only to name the type, not to change
/// what it means).
type LeadingImagesAndTextSuffix = (Vec<(String, String)>, Range<usize>);

fn paragraph_leading_images(
    doc: &Doc<'_>,
    src: &str,
    inline: &Range<usize>,
) -> Option<LeadingImagesAndTextSuffix> {
    let events = &doc.events[inline.clone()];
    let bounds = top_level_segment_bounds(events);
    if bounds.len() < 2 {
        return None; // no top-level break at all — nothing to peel a leading run off of.
    }
    let mut images = Vec::new();
    let mut leading = 0usize;
    for &(s, e) in &bounds {
        match segment_as_block_images(src, &events[s..e]) {
            Some(imgs) => {
                images.extend(imgs); // already document order; segments walked front to back too.
                leading += 1;
            }
            None => break,
        }
    }
    if leading == 0 || leading == bounds.len() {
        return None;
    }
    let text_start_rel = bounds[leading].0;
    let text_range = (inline.start + text_start_rel)..inline.end;
    Some((images, text_range))
}

/// Plain-text reconstruction of one `Image`'s own alt-text content (`events` = everything strictly
/// between its `Start`/`End`) — concatenates every `Event::Text`/`Event::Code` payload it contains
/// (a soft/hard break becomes a single space, matching `Writer::join_with_space`'s own literal-space
/// rendering), dropping any inline styling (bold/italic/...) the alt text happens to carry. Mirrors
/// how `markdown.rs`'s own `extract_md_img` reads alt text back from raw source: a literal substring
/// between `![` and the first `]`, which likewise carries no styling of its own — Markdown image
/// syntax has no mechanism for any.
fn image_alt_text(events: &[(Event<'_>, Range<usize>)]) -> String {
    let mut s = String::new();
    for (ev, _) in events {
        match ev {
            Event::Text(t) => s.push_str(t),
            Event::Code(c) => s.push_str(c),
            Event::SoftBreak | Event::HardBreak => s.push(' '),
            _ => {}
        }
    }
    s
}

/// If `extract_eligible` and `inline` is nothing but one or more standalone block images
/// (`paragraph_as_block_images` — one per physical source line when several sit end to end with no
/// blank line between them, the common "badge row" README idiom), renders them all in document order
/// via `render_image_group` (packed left to right onto shared rows — see that function's own doc
/// comment) and returns `true`; otherwise renders nothing and returns `false`, leaving the caller
/// (`render_paragraph_dispatch`/`render_bare_paragraph`) to fall through to its own ordinary paragraph
/// handling. The one shared checkpoint both of those functions' own `Paragraph` dispatch runs through
/// first, so the "is this paragraph actually just image(s)" decision — and its own eligibility gate —
/// never gets duplicated between them.
fn try_render_paragraph_as_image(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    inline: &Range<usize>,
    extract_eligible: bool,
) -> bool {
    if !extract_eligible {
        return false;
    }
    let Some(images) = paragraph_as_block_images(doc, src, inline) else {
        return false;
    };
    render_image_group(w, &images);
    true
}

/// Renders one run of block-level image placements into `w`, in place, packing consecutive `Inline`
/// slot answers left to right onto shared rows instead of the one-row-per-image stack this file used
/// to draw (the real bug this exists to fix: GitHub/crates.io lay out a `[![a](x)](u1)
/// [![b](y)](u2) [![c](z)](u3)` badge row horizontally; konoma used to draw it as three stacked rows).
/// The block-image analogue of `render_math_slot`/`render_mermaid_slot`, generalized from what used to
/// be a strictly one-image-at-a-time `render_image_slot`: `w.slot_of(url, None)` decides, per image, between
/// a real `Inline` placement (reserved cells, packed into the current row group), a `Loading` line, or
/// the `Unavailable` text fallback (`image_loading_line`/`image_text_fallback`, unchanged — the
/// identical two functions `render_markdown_with_images`'s own `BlockPart::Image` arm calls for those
/// two slots).
///
/// ## Row-group packing
///
/// Consecutive `Inline` answers accumulate into one pending group; each next image is appended if it
/// fits (`current group width + 1-cell gap + this image's own cols <= w.width`), otherwise the pending
/// group is flushed (drawn, centered, as its own row) and a fresh group starts with just that image. A
/// `Loading`/`Unavailable` answer flushes whatever `Inline` group preceded it — its own single dim line
/// is drawn immediately, unconditionally on its own row, exactly as before this change — and starts a
/// *fresh* group for whatever `Inline` image(s) follow (never packed onto a row a text placeholder line
/// already occupies; the simplest rule that changes existing Loading/Unavailable behavior not at all).
///
/// A group of exactly one image reduces to the pre-existing single-image placement byte for byte: see
/// `flush_row_group`'s own doc comment for why.
///
/// No leading-blank-line check of any kind, matching this file's own prior, single-image-at-a-time
/// `render_image_slot`'s unconditional draw — a standalone image (unlike an *ordinary* paragraph) is
/// extracted *before* `render_paragraph` ever sees any surrounding text at all, so
/// nothing here owes the page a leading-blank-line-style separator; see `render_math_slot`'s own doc
/// comment (and `pending_para_start`'s) for the identical reasoning already established for a lifted
/// math expression that is a paragraph's own *entire* content. Sets `pending_block_gap = false`/
/// `after_math = true`/`fresh_boundary = true` on the way out, unconditionally, once for the whole run
/// — the identical trailing state the old per-image `render_image_slot` left after *every* image, so a
/// multi-image run leaves the writer in exactly the state a single-image run always did.
///
/// An **empty** `images` is a true no-op — no lines, no placements, and none of that trailing state
/// touched either. This matters: `render_heading`'s own call site passes `Vec::new()` whenever
/// `heading_trailing_images` found no trailing badges at all, and the old `for (alt, url) in &images
/// { render_image_slot(..) }` loop it replaced never ran its body (and so never touched `w`) in that
/// case — silently forcing `pending_block_gap = false` here instead would stomp the `pending_block_gap = true`
/// `render_heading` had just set for an ordinary (badge-less) heading.
fn render_image_group(w: &mut Writer<'_>, images: &[(String, String)]) {
    if images.is_empty() {
        return;
    }
    // (alt, url, cols, rows) for the `Inline`-slot images currently pending a row.
    let mut group: Vec<(String, String, u16, u16)> = Vec::new();
    for (alt, url) in images {
        match (w.slot_of)(url, None) {
            ImageSlot::Inline { cols, rows } => {
                let would_be =
                    row_group_width(&group) + if group.is_empty() { 0 } else { 1 } + cols as usize;
                if !group.is_empty() && would_be > w.width as usize {
                    flush_row_group(w, &mut group);
                }
                group.push((alt.clone(), url.clone(), cols, rows));
            }
            ImageSlot::Loading => {
                flush_row_group(w, &mut group);
                w.lines.extend(image_loading_line(alt, url, w.width));
            }
            ImageSlot::Unavailable => {
                flush_row_group(w, &mut group);
                w.lines.extend(image_text_fallback(alt, url, w.width));
            }
        }
    }
    flush_row_group(w, &mut group);
    w.pending_block_gap = false;
    w.after_math = true;
    // `render_code_block`'s own leading-blank-line check is gated on `fresh_boundary`, not
    // `pending_block_gap` (see that function's own doc comment on why) — an image placement immediately
    // followed by a fence, with nothing textual between them, needs this set too, or that fence would
    // insert a spurious extra blank row production's own flat accumulation never does — see
    // `render_table_from_model`'s own doc comment for the concrete corpus case (a table, not an
    // image, but the identical mechanism) that caught this before this field was added here.
    w.fresh_boundary = true;
}

/// The total cell width a row group of `Inline` images occupies once packed: every image's own `cols`
/// plus a 1-cell gap between each adjacent pair (`len - 1` gaps for `len` images, `0` for an empty or
/// single-image group).
fn row_group_width(group: &[(String, String, u16, u16)]) -> usize {
    if group.is_empty() {
        return 0;
    }
    let cols_sum: usize = group.iter().map(|(_, _, cols, _)| *cols as usize).sum();
    cols_sum + (group.len() - 1)
}

/// Draws one pending row group of `Inline` images — centers the whole packed row inside `w.width`
/// (mirroring the pre-existing single-image centering formula, `(width - total) / 2`), records each
/// image's own `ImagePlacement` (with its own real `cols`/`rows`, at its own `col` within the row), and
/// reserves rows equal to the group's own **tallest** image (design: a shorter image in the same row
/// group still gets the taller image's full row count reserved beneath it, so the next block never
/// starts inside another image's own reserved band). A no-op on an empty group (both call sites in
/// `render_image_group` call this unconditionally, including once no group is pending).
///
/// A group of exactly one image has `row_group_width == cols` (no gap term) and thus `start_col ==
/// (w.width - cols) / 2` — the *exact* pre-existing single-image centering formula
/// `image_placeholder_lines`'s old signature used to compute inline. `image_placeholder_lines`'s own
/// new, general form draws that one label at that same `col`, so the emitted lines are byte-for-byte
/// identical to before this change (confirmed directly: the golden Markdown snapshots — every existing
/// single-image case in `md_snapshot_tests`/`samples/*.md` — are unchanged by this refactor).
fn flush_row_group(w: &mut Writer<'_>, group: &mut Vec<(String, String, u16, u16)>) {
    if group.is_empty() {
        return;
    }
    let width = w.width;
    let rows = group
        .iter()
        .map(|(_, _, _, rows)| *rows)
        .max()
        .unwrap_or(1)
        .max(1);
    let total = row_group_width(group) as u16;
    // `[ui] md_image_align` (default `center`, i.e. `(width - total) / 2` — the exact expression
    // this line used before the option existed; see `BlockAlign::offset`).
    let start_col = w.aligns.image.offset(width, total);
    let placement_line = w.lines.len() + w.heading_rule_shift;
    let mut items: Vec<(u16, u16, &str)> = Vec::with_capacity(group.len());
    let mut col = start_col;
    for (alt, url, cols, item_rows) in group.iter() {
        items.push((col, *cols, alt.as_str()));
        w.images.push(ImagePlacement {
            url: url.clone(),
            alt: alt.clone(),
            line: placement_line,
            col,
            cols: *cols,
            rows: *item_rows,
            fence_ord: None,
        });
        col += cols + 1; // this image's own width, plus the 1-cell gap before the next one.
    }
    w.lines.extend(image_placeholder_lines(&items, rows));
    group.clear();
}

/// Renders a `Paragraph` block into `w`, in place — first checking whether it is actually a
/// standalone block image (`try_render_paragraph_as_image`, gated by `extract_here`), then choosing
/// between `render_paragraph_math` (lifts any LaTeX math the paragraph's own text contains onto its
/// own placeholder line(s)) and plain `render_paragraph` — the call site `render_doc`'s own top-level
/// dispatch and `render_block`'s generic one both go through for every `Paragraph` in the tree
/// *except* a list item's own first child, which `render_item` special-cases (see that function's own
/// doc comment on why: continuing text directly onto the marker's own row is incompatible with this
/// function's `start_paragraph`/`end_paragraph`-equivalent bookkeeping). That special case *does*
/// still route back through `render_block` — and so through this function — for the one shape where
/// bypassing it would silently drop image extraction entirely: a first child that is nothing but
/// standalone block image(s) (`- ![alt](x)`; found live — see `render_item`'s own doc comment for the
/// bug this closes). Any other first-child paragraph (plain text, or text mixed with a
/// leading/trailing image) still bypasses this function, via `walk_inline`/`walk_inline_math` called
/// directly — a real, narrower gap this fix does not close (only the "entirely image(s)" shape is
/// covered) — so `math_here`/`extract_here` being propagated to `render_item` at all is only ever
/// consulted there for that one narrow re-dispatch decision, not for driving this function's own
/// leading/trailing-image branches on that first child. `math_here`/`extract_here` are
/// `render_block`/`render_list`/`render_item`'s own propagated answers to "is math lifting"/"is
/// image/mermaid extraction" active at this position in the tree (see `BlockCtx`'s own doc comment
/// for why the two are separate fields, not one shared flag); `w.math.is_some()` is `render_doc`'s
/// own `math_on` argument, unconditionally true or false for the *whole* render. Both `math_here` and
/// `w.math.is_some()` have to hold for a math lift — `math_here` alone is not enough on its own,
/// since a caller could (in principle) call this function with `math_here: true` while `w.math` is
/// `None` (`math_on` was never set at all) — though no call site in this file actually does; checked
/// here all the same, once, rather than trusting every caller to have already combined the two
/// correctly. `extract_here` alone decides the image check — there is no analogous "is image
/// extraction on at all" toggle to combine it with (`w.slot_of` is unconditional — see that field's
/// own doc comment).
///
/// `block_start`: the `Paragraph` `Block`'s own `src.start`, forwarded to `render_paragraph_math`
/// (which needs it — see that function's own doc comment and `walk_inline_math`'s) — read, but not
/// otherwise used, whenever the `math_here && w.math.is_some()` check sends this to plain
/// `render_paragraph` instead, which does not need it (a `Paragraph` with no math to lift has no gap
/// for a backslash-math opener to be caught in in the first place).
fn render_paragraph_dispatch(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    inline: Range<usize>,
    math_here: bool,
    extract_here: bool,
    block_start: usize,
) {
    if try_render_paragraph_as_image(w, doc, src, &inline, extract_here) {
        return;
    }
    // A real, ordinary paragraph whose own *tail* is one or more standalone image lines (see
    // `paragraph_trailing_images`'s own doc comment for the confirmed real-content shape this
    // closes) — render the leading text normally, then each trailing image, in order.
    if extract_here {
        if let Some((text_range, images)) = paragraph_trailing_images(doc, src, &inline) {
            if math_here && w.math.is_some() {
                render_paragraph_math(w, doc, src, text_range, block_start);
            } else {
                render_paragraph(w, doc, text_range);
            }
            render_image_group(w, &images);
            return;
        }
        // The mirror shape — see `paragraph_leading_images`'s own doc comment for the confirmed
        // real-content shape this closes: one or more standalone image lines *first*, real text
        // lazily continuing right after with no blank line between them. `render_image_group` itself
        // has no leading-blank-line check of its own (that function's own doc comment) — unlike the
        // trailing-image case above, where `render_paragraph`'s own leading check already ran first,
        // here the *first* thing drawn for this whole paragraph is an image, so this block owns that
        // check itself, the identical one `render_paragraph`/`render_heading` each run at their own
        // start.
        if let Some((images, text_range)) = paragraph_leading_images(doc, src, &inline) {
            if w.pending_block_gap {
                w.emit_blank_row();
            }
            w.pending_block_gap = false;
            render_image_group(w, &images);
            if math_here && w.math.is_some() {
                render_paragraph_math(w, doc, src, text_range, block_start);
            } else {
                render_paragraph(w, doc, text_range);
            }
            return;
        }
    }
    if math_here && w.math.is_some() {
        render_paragraph_math(w, doc, src, inline, block_start);
    } else {
        render_paragraph(w, doc, inline);
    }
}

/// Renders a `Paragraph` block into `w`, in place, lifting any LaTeX math its own text contains onto
/// its own placeholder line(s) — the math analogue of `render_paragraph` above, which
/// `render_text_with_math` delegates back to, verbatim, the moment a line turns out to hold none (see
/// that function's own "no math on this line" fallback: byte-for-byte the same span it would have
/// gotten from a plain `walk_inline`). Only ever called through `render_paragraph_dispatch` (a
/// top-level paragraph, or one reached through a `List` item — never one inside a `Quote`; see that
/// function's own doc comment, and `render_doc`'s, for exactly why).
///
/// ## Why per-`Event::Text`, not per-block, and why that is exactly what the legacy renderer's own
/// splitting did too
///
/// The legacy renderer lifted math by running `split_math` — not on any single block's own text, but
/// on (up to) the **whole remaining document** as one `SourceRun`, *before* any parser ever saw it,
/// splitting the raw source itself at every match and handing each resulting fragment to its own,
/// entirely independent re-parse. This file reads its inline content from a *single*, whole-document
/// parse instead (see the module doc comment on why that is the entire point of this refactor) —
/// re-parsing a fragment here would reintroduce, for math specifically, the exact cross-block
/// reference-link gap eliminating re-parsing closed for everything else (see the module doc comment's
/// "How inline content is rendered" section).
/// What this
/// function does instead is scan each `Paragraph`'s own `Event::Text` payloads for math directly, via
/// `scan_inline_math` (reused verbatim — see the module doc comment on not writing a second math
/// detector) — never `split_math` itself, whose own job (finding a *document-wide* line-by-line split
/// point, honoring `structure_mask`'s blockquote/`<details>`/HTML-block/table exclusions) does not
/// apply to a single, already-known paragraph's own text at all: **at the top level or inside a
/// `List` item**, a `Paragraph`'s own raw text can never itself be `>`-prefixed, inside a `<details>`
/// block, or a table row (if it were, `Doc::parse` would have produced a `Quote`/`Html`/`Table` block
/// instead — see `model::BlockKind`'s own doc comment); **inside a `Quote`**, `structure_mask` would
/// exclude it from lifting anyway, which is exactly why `render_paragraph_dispatch` never routes a
/// quote-nested paragraph here at all rather than this function having to re-derive that exclusion
/// itself from `Block.src`'s own, asymmetric byte range there (a quoted paragraph's first line has its
/// `>` marker stripped from `Block.src`, but a continuation line's own `>` stays in — confirmed
/// directly, not merely inferred, by parsing `"> hello $x$ world\n> line2\n"` and reading the
/// resulting `Paragraph`'s own `src` range back: `"hello $x$ world\n> line2\n"` — so scanning that
/// text for a `>`-prefixed line, the way `structure_mask` itself would, could not reliably tell a
/// quoted paragraph's own lines apart from an unquoted one's). Nor can a `Paragraph` reached this way
/// ever itself be a multi-line `$$…$$`/`\[…\]` opener needing `split_math`'s own multi-line lookahead
/// **within this specific pass's scope** — the parity corpus's own math cases are all single-line/
/// inline-style (`$…$`/`\(…\)`/single-line `$$…$$`), a scope `render_doc`'s own doc comment names
/// explicitly; `scan_inline_math` alone reproduces every one of them exactly, matching production line
/// for line.
///
/// This *is* the same splitting production performs, restricted to one paragraph's own text rather
/// than the whole remaining document: `Event::Code`/`Event::SoftBreak`/`Event::HardBreak`, and any
/// nested inline construct (`Emphasis`/`Strong`/`Link`/...), are never scanned for math at all — math
/// syntax straddling into one of those is out of scope here the same way it already is for
/// production's *own* re-parse-based splitting whenever it would land astride an already-parsed
/// construct (see `Writer::text`'s own per-event granularity for why `Event::Code`'s own payload, in
/// particular, was *already* parsed out of any literal backticks around it — reusing it directly, not
/// rescanning its content, is what keeps a code span's own `$` correctly excluded from math lifting,
/// for free, the same way `structure_mask`'s own code-block exclusion keeps production's).
///
/// ## The "fresh reparse boundary" every lift needs
///
/// Production's own per-fragment independent re-parse has an observable consequence this function has
/// to reproduce deliberately, not merely "insert the placeholder and carry on": every fragment
/// *following* a lifted math expression is treated as starting fresh, with `pending_block_gap` forced
/// `false` — so whatever comes right after a lift never gets the leading blank
/// line it would have gotten had rendering simply continued (see `ensure_fresh_after_math`'s own doc
/// comment for the mechanics, and CommonMark's own paragraph-content trimming, which the now-isolated
/// fragment's own leading/trailing whitespace becomes newly subject to — see `render_text_with_math`'s
/// own trimming). Confirmed against the real production pipeline (not merely inferred) via
/// `code_span_corpus`'s own cases: math sitting at the very end of a paragraph is followed *directly*
/// by whatever top-level block comes next (no separator line at all — e.g. a `ThematicBreak`
/// immediately after, matching a *fresh* `Writer`'s own `pending_block_gap == false` start), and math with
/// real text trailing it within the same original paragraph (`"...$x$ b"`) puts that trailing text on
/// its own, freshly-opened line with **no** leading whitespace, exactly as an independent re-parse of
/// `" b"` alone would produce. This holds even when the paragraph carrying the lift is itself a list
/// item's own content, not a top-level block: the "fragment" a lift's own boundary defines can end a
/// `List` mid-render (its own closing container events fire, invisibly, inside that same independent
/// re-parse — see `render_list`'s own doc comment on `after_math` for why its own exit has to respect
/// this rather than clobber it) just as easily as it can end a top-level paragraph — confirmed
/// directly against `code_span_corpus`'s own "indented four columns inside a list item is not a code
/// block" case, whose trailing math sits inside a (tight) list item's own lazily-continued text and is
/// followed, with no separator line, by a wholly unrelated top-level paragraph next in the same
/// document.
///
/// ## The leading edge: deferred, not eager (`pending_para_start`)
///
/// Unlike `render_paragraph`, this function does **not** open with the two-part leading-blank-line
/// convention before walking the paragraph's own content — it records that one is *owed*
/// (`w.pending_para_start = Some(w.pending_block_gap)`) and lets `walk_inline_math` resolve it, via
/// whichever of `Writer::open_pending_para_start`/`drop_pending_para_start` the paragraph's own very
/// first rendered content turns out to call for (see `pending_para_start`'s own doc comment for the
/// full mechanism, and why an eager push here — this function's own original shape — left an orphaned,
/// permanently-empty row behind whenever that first content turned out to be a lifted math expression
/// rather than real text: confirmed directly via `code_span_corpus`'s own three backslash-math and
/// display-math cases, each an indented code block followed by a paragraph whose *entire* content is a
/// single lifted expression).
///
/// `block_start` is threaded straight through to `walk_inline_math`'s own identical parameter — see
/// that function's own doc comment for the *other* bug this closes (a backslash-math opener that is
/// this same paragraph's own very first event).
fn render_paragraph_math(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    inline: Range<usize>,
    block_start: usize,
) {
    w.pending_para_start = Some(w.pending_block_gap);
    walk_inline_math(
        &mut events_iter_ranged(&doc.events[inline]),
        w,
        src,
        block_start,
    );
    // A paragraph whose own trailing content was math (nothing textual after the last lift) leaves
    // `pending_block_gap == false`, matching a *fresh* `Writer`'s own start state — see this function's
    // own doc comment. Only a paragraph that ends in real text (even text *following* an earlier
    // lift) gets the normal `end_paragraph()`-equivalent `true`.
    if !w.after_math {
        w.pending_block_gap = true;
    }
    // Defensive only (principle #3, `CLAUDE.md`): a well-formed `Paragraph`'s own `inline` range is
    // never empty (`Doc::parse` always records at least one event for real content), so
    // `walk_inline_math`'s own dispatch always resolves the slot just opened above — via either
    // `open_pending_para_start` or `drop_pending_para_start` — before this function returns. Cleared
    // here too regardless, so a `Doc::parse` edge case this file has not been tested against still
    // degrades to "no stray leading blank line" for whatever paragraph renders *next* through this
    // same `Writer`, rather than silently carrying a stale slot from this call into that one.
    w.pending_para_start = None;
}

/// Like `events_iter`, but yields each event *with* its own byte range too. `walk_inline_math`'s own
/// top-level dispatch needs the *gap* between one event's own end and the next one's own start — the
/// range alone exposes it; the event's own (already-escape-processed) payload cannot, by
/// construction (see `backslash_math_opener`'s own doc comment for exactly why that gap matters).
fn events_iter_ranged<'a, 'e>(
    events: &'e [(Event<'a>, Range<usize>)],
) -> impl Iterator<Item = (Event<'a>, Range<usize>)> + 'e {
    events.iter().cloned()
}

/// Walks `events` exactly the way `walk_inline` itself does (see that function's own doc comment for
/// the event-kind coverage and why a mismatched `Event::End`/`TaskListMarker`/`Rule` cannot
/// legitimately reach this loop either), but scans every `Event::Text` for LaTeX math along the way
/// instead of pushing it verbatim — the shared math-aware walk both `render_paragraph_math` (a
/// standalone `Paragraph`) and `render_item` (a list item's own first child, tight or loose, which
/// never goes through `render_paragraph`/`render_paragraph_math` at all — see that function's own doc
/// comment on why) drive their own inline content through. Every non-`Text` branch calls
/// `ensure_fresh_after_math` first — a math lift can be followed directly by any of these (an inline
/// code span right after `"...$x$"`, say), and each one's own underlying `Writer` method
/// (`code`/`join_with_space`/`end_row`, or a nested `start_inline_tag` call) ultimately reaches
/// `Writer::append_span`, whose own fallback (`self.lines.last_mut()` being `Some`) would otherwise
/// append straight onto the math placeholder's own line rather than a fresh one.
///
/// `Event::Text` itself is two-tiered: first, `backslash_math_opener` checks whether the *gap* just
/// before this event, together with its own first character, is `\(`/`\[` split apart by
/// pulldown-cmark's own backslash-escape handling (see that function's own doc comment for exactly
/// why and how) — if so, `render_backslash_math` takes over, scanning ahead across as many further
/// events as it takes to find the matching closer. Otherwise, `render_text_with_math` (reused
/// verbatim — see `render_paragraph_math`'s own doc comment on not writing a second math detector)
/// handles everything `scan_inline_math` alone can see within this one event's own payload
/// (`$…$`/`$$…$$`, both fully self-contained within a single `Event::Text` — see that function's own
/// doc comment for why those never need this same event-spanning treatment).
///
/// ## Why a plain `Text` event's own rendering is *deferred*, not immediate
///
/// A `Text` event that does **not** itself open a backslash-math span still cannot be rendered the
/// moment it is seen — not until this loop also knows what the event *right after* it turns out to be:
/// if that next event opens a backslash-math span that goes on to find a real closer, this text's own
/// trailing whitespace needs the exact same trim any `MathPart::Text` fragment immediately preceding a
/// lift gets (`render_text_with_math`'s own doc comment) — but if the very same "looks like an opener"
/// shape turns out to have no matching closer anywhere ahead (an *ordinary* escaped bracket — see
/// `render_backslash_math`'s own doc comment on why that ambiguity is real, not a corner case this
/// loop invented), the trim must **not** happen: `inline_corpus`'s own "escaped brackets are literal"
/// case is exactly this — `"This is "` sits right before `"\[not a link](nope)."`, whose own `\[`
/// *looks* like an opener but never closes, so `"This is "` keeps its own trailing space unchanged.
/// Deciding this correctly needs to know the *outcome* of the next event's own scan, which is only
/// available once that scan has actually run — so `pending`, this loop's own held-back `Text` event,
/// travels into `render_backslash_math` itself (whichever of its own three exits fires — closer found,
/// ran out of events, or a line-scoping break — flushes `pending` with the trim that exit's own
/// outcome calls for) rather than this loop trying to peek and guess ahead of time.
///
/// ## `block_start`: letting a backslash-math opener be the walk's own very first event
///
/// `backslash_math_opener` needs a *gap* to read — the raw source bytes between the *previous* event's
/// own end and this one's own start (see that function's own doc comment) — but the very first event
/// this loop ever sees has no previous *event*, within this walk, to compute one from. Seeding
/// `prev_end` with `None` for that first iteration (an earlier version of this function did exactly
/// that) means `gap` comes out `None` too — never `Some("\\")` — so `\(`/`\[` can never be detected as
/// an opener when it is the paragraph's own very first character: a genuine, pre-existing bug, not a
/// corner case this loop invented (confirmed directly: `code_span_corpus`'s own backslash-math cases,
/// each an indented code block followed by a paragraph whose *entire* content is `\(y\)`/`\[y\]`, used
/// to fall through as the *literal* text `"(y)"`/`"[y]"`, never lifted).
///
/// `block_start` is the fix: the byte position *conceptually* just before this walk's own first event
/// — `block.src.start`, for whichever `Block` owns the paragraph being walked (see each of this
/// function's own three call sites for where that comes from). For a **real** `Paragraph`
/// (`render_paragraph_math`'s own case, and a loose list item's own first child, in `render_item`),
/// `block.src` is pulldown-cmark's own `Tag::Paragraph` span, which — unlike any individual inline
/// event's own range — still includes a leading backslash its own escape handling stripped out of the
/// first *event's* range (confirmed directly: parsing `"\[y\]\n"` alone reports `Start(Paragraph)`
/// spanning `0..6`, the *whole* source, while its own first inline event, `Text("[y")`, starts at `1`
/// — `src[0..1] == "\\"`, exactly the gap `backslash_math_opener` needs). Seeding `prev_end` with
/// `Some(block_start)` computes that same gap for the walk's own first event too, the same way it
/// already does for every later one, closing the bug for exactly the shape it affects — with **no**
/// risk of a false positive for the ordinary case: whenever there is no leading escape (`block_start ==
/// first_event.start`, or this paragraph does not start with a backslash), the computed gap is simply
/// `""`, which `backslash_math_opener`'s own `gap != Some("\\")` check already rejects, identically to
/// `None`. A **tight** list item's own first child (`render_item`'s *other* branch, a *synthetic*
/// `Paragraph` `model::collect_stray_inline_run` builds rather than one pulldown-cmark itself wrapped)
/// does not carry this same wider span — its own `Block.src` is built purely from consumed events, with
/// no wrapping tag to have recorded one — so `block_start == first_event.start` there even when the
/// item's own content *does* open with a backslash escape; this bug stays open for that one narrow
/// shape (not exercised by the parity corpus, and no worse than before this fix: the gap is still `""`,
/// not a false `Some("\\")`), rather than fabricating a position the model does not actually record.
fn walk_inline_math<'a>(
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    w: &mut Writer<'_>,
    src: &str,
    block_start: usize,
) {
    let mut prev_end: Option<usize> = Some(block_start);
    let mut pending: Option<pulldown_cmark::CowStr<'a>> = None;
    while let Some((ev, range)) = events.next() {
        let gap = prev_end.map(|p| &src[p..range.start]);
        prev_end = Some(range.end);
        match ev {
            Event::Text(t) => {
                if let Some((closer, body_start)) = multiline_math_opener(src, &range) {
                    render_multiline_display_math(
                        w,
                        events,
                        pending.take(),
                        t,
                        body_start,
                        closer,
                        src,
                        &mut prev_end,
                    );
                    continue;
                }
                if let Some((display, closer)) = backslash_math_opener(gap, &t) {
                    render_backslash_math(
                        w,
                        events,
                        pending.take(),
                        t,
                        range.clone(),
                        display,
                        closer,
                        src,
                        &mut prev_end,
                    );
                    continue;
                }
                if dollar_math_still_open(&t) {
                    render_dollar_math_tail(
                        w,
                        events,
                        pending.take(),
                        t,
                        range.clone(),
                        src,
                        &mut prev_end,
                    );
                    continue;
                }
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                pending = Some(t);
            }
            Event::Code(c) => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                ensure_fresh_after_math(w);
                w.write_code_span(&c);
            }
            Event::SoftBreak => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                ensure_fresh_after_math(w);
                w.join_with_space();
            }
            Event::HardBreak => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                ensure_fresh_after_math(w);
                w.end_row();
            }
            Event::Start(tag) => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                ensure_fresh_after_math(w);
                // `start_inline_tag` wants a plain `Iterator<Item = Event<'a>>` (nested content is
                // never math-scanned — see `render_paragraph_math`'s own doc comment on scope), so
                // this drops the range from here on; `prev_end` still has to keep advancing while
                // nested content is consumed, so whatever *follows* the whole nested construct gets
                // its own gap computed correctly, not one measured from before it even started.
                let mut inner = events.by_ref().map(|(ev, r)| {
                    prev_end = Some(r.end);
                    ev
                });
                start_inline_tag(&mut inner, w, tag);
            }
            // Mirrors `walk_inline`'s own defensive catch-all — see that function's own doc comment.
            _ => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
            }
        }
    }
    if let Some(p) = pending.take() {
        render_text_with_math(w, &p, false);
    }
}

/// The byte bounds of the whole physical source **line** containing `range` — from the byte right
/// after the previous `'\n'` (or `0`, at the very start of `src`) through the byte right before the
/// next `'\n'` (or `src.len()`, on the last, possibly unterminated line). Used by
/// `multiline_math_opener`/`render_multiline_display_math` to ask "is this event's own *whole source
/// line* — not merely its own event payload, which a differently-shaped call site could have handed a
/// narrower range for — exactly `$$`/`\[`/`\]`", the identical question `markdown.rs`'s own
/// `split_math` asks of a raw line (`bare.trim()`) before its own dedicated multi-line collection loop
/// runs (see that function's own doc comment).
fn line_bounds(src: &str, range: &Range<usize>) -> Range<usize> {
    let start = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let end = src[range.end..]
        .find('\n')
        .map_or(src.len(), |i| range.end + i);
    start..end
}

/// Whether `range`'s own whole physical source line, trimmed, is exactly `"$$"` or `"\["` — the
/// **multi-line display math** opener `markdown.rs`'s own `split_math` recognizes as a shape distinct
/// from (and checked *before*) its generic, single-line `scan_inline_math` (see that function's own
/// doc comment: `"a line that is exactly $$ or \[ (collect to the matching close)"`). Returns the
/// matching closer (`"$$"`/`"\]"`) and the byte offset, into `src`, right after this opener's own
/// line — where `render_multiline_display_math`'s own body collection starts — mirroring
/// `split_math`'s own `j = i + 1` (the *next* physical line, not this one).
///
/// This is the one shape `dollar_math_still_open`/`render_dollar_math_tail` cannot resolve on their
/// own: `$$`/`\[` alone on a line is immediately followed, in `Doc.events`, by an `Event::SoftBreak`
/// to the next line — and `render_dollar_math_tail`'s own growing-raw-slice loop deliberately treats
/// *any* soft/hard break at `depth == 0` as a give-up boundary (matching `scan_inline_math`'s own,
/// single-physical-line scope for the *generic* `$…$`/`$$…$$` shape it handles — see that function's
/// own doc comment on why a soft break is a real, not merely defensive, stopping point there), so a
/// `$$` that opens a *display* block spanning several physical lines was never reaching a closer at
/// all: `render_dollar_math_tail` gave up on the very first break, replaying `$$` as ordinary literal
/// text — losing the whole expression, not merely rendering it worse (a real regression, to be fixed
/// outright rather than catalogued as an accepted mismatch). Checked
/// *before* `backslash_math_opener`/`dollar_math_still_open` in `walk_inline_math`'s own dispatch —
/// order does not otherwise matter (the three triggers are mutually exclusive: a leading-backslash gap,
/// an unresolved generic dollar-scan, and *this* whole-line-is-just-the-opener shape never overlap on
/// the same event), but checking this one first keeps the narrower, better-matching-production
/// mechanism from ever losing a race to the generic one.
fn multiline_math_opener(src: &str, range: &Range<usize>) -> Option<(&'static str, usize)> {
    let bounds = line_bounds(src, range);
    let closer = match src[bounds.clone()].trim() {
        "$$" => "$$",
        "\\[" => "\\]",
        _ => return None,
    };
    let body_start = if bounds.end < src.len() {
        bounds.end + 1
    } else {
        bounds.end
    };
    Some((closer, body_start))
}

/// Consumes events starting right after `first_text` (whose own whole line `multiline_math_opener`
/// already confirmed is just `$$`/`\[`, in `walk_inline_math`) until a **later** `Event::Text` whose
/// own whole physical line, trimmed, equals `closer` — mirroring `markdown.rs`'s own dedicated
/// multi-line collection loop in `split_math` (`while j < lines.len() { ... if bj.trim() == closer {
/// found = true; break; } body.push_str(lines[j]); j += 1; }`) rather than the generic, single-line
/// `render_dollar_math_tail`/`scan_inline_math` machinery, which cannot see past a soft/hard break at
/// all (see `multiline_math_opener`'s own doc comment for why that gap exists and what this closes).
///
/// Every event this loop sees — `Text`, `Code`, `Start`/`End`, `SoftBreak`/`HardBreak` — is buffered
/// into `fallback` and consumed unconditionally (the identical "just keep consuming, whatever the
/// event kind" shape `render_dollar_math_tail`'s own loop already has, for the identical reason: see
/// that function's own "Never stop mid-construct" section), and `depth` tracks whether the loop is
/// currently inside an unfinished `Start`/`End` pair the same way — a `Text` event reached while
/// `depth > 0` can never be *this* block's own closer line (its content is nested inside some other
/// inline construct — emphasis, a link, ... — not a standalone line of its own), so `is_closer_line`
/// only ever fires at `depth == 0`, the same guard `render_dollar_math_tail`'s own two exits share.
///
/// On success: the raw text strictly between `body_start` and the closer's own line start
/// (`src[body_start..bounds.start]`, trimmed the same way `flush_math`'s own `latex.trim()` is — this
/// function's caller, `render_math_slot`, receives already-trimmed LaTeX every other way it is ever
/// reached too) is rendered as display math via `render_math_slot` — the identical function a
/// single-line `$$…$$` lift already renders through, so a live `Picker` answering with a real raster
/// image, a background render still in flight, or the raw-LaTeX-dimmed fallback all behave identically
/// whether the expression happened to span one physical line or several.
///
/// On failure (events run out before a closer's own line ever turns up): `fallback` — starting from
/// `first_text` itself — replays verbatim via `replay_events`, the identical "give the events back
/// unchanged" fallback `render_dollar_math_tail`'s own doc comment already explains in full: a
/// document that opens a `$$`/`\[` line and never closes it renders exactly as if this function had
/// never intercepted it at all, not as a truncated or corrupted attempt.
#[allow(clippy::too_many_arguments)] // mirrors render_dollar_math_tail's own identical allow — one call site (walk_inline_math), each argument its own distinct piece of state
fn render_multiline_display_math<'a>(
    w: &mut Writer<'_>,
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    pending: Option<pulldown_cmark::CowStr<'a>>,
    first_text: pulldown_cmark::CowStr<'a>,
    body_start: usize,
    closer: &str,
    src: &str,
    prev_end: &mut Option<usize>,
) {
    let mut fallback: Vec<Event<'a>> = vec![Event::Text(first_text)];
    let mut depth: usize = 0;
    loop {
        let Some((ev, range)) = events.next() else {
            if let Some(p) = pending {
                render_text_with_math(w, &p, false);
            }
            replay_events(w, fallback);
            return;
        };
        *prev_end = Some(range.end);
        let bounds = line_bounds(src, &range);
        let is_closer_line =
            depth == 0 && matches!(&ev, Event::Text(_)) && src[bounds.clone()].trim() == closer;
        match &ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        if is_closer_line {
            let body = src[body_start.min(bounds.start)..bounds.start].trim();
            if let Some(p) = pending {
                render_text_with_math(w, &p, true);
            }
            render_math_slot(w, body, true);
            return;
        }
        fallback.push(ev);
    }
}

/// Whether `t` (an `Event::Text` payload) — together with `gap`, the raw source bytes between the
/// *previous* event's own end and this one's own start (`None` for the very first event a walk sees)
/// — opens a backslash-delimited math span (`\(…\)`/`\[…\]`) whose own leading backslash pulldown-cmark's
/// escape handling has already stripped out of *every* event's own range. Confirmed directly, not
/// merely inferred — `Doc::parse("`\\[x\\]` and \\[y\\]\n")`'s own event stream reports `` Text("[y")
/// range=13..15 `` immediately after `` Text(" and ") range=7..12 `` (a one-byte gap, `src[12..13] ==
/// "\\"`, between them), then `` Text("]") range=16..17 `` after *another* one-byte gap
/// (`src[15..16] == "\\"`) — the backslash itself is not part of *any* event's own range at all,
/// unlike a code span's own delimiters (compare `Code("K")`'s own range, which *does* include its
/// surrounding backticks even though its payload does not). Per-event `scan_inline_math`
/// (`render_text_with_math`) cannot see this shape: `\(`/`\[` never appear together in the same
/// event's own payload for it to match against — this is the one gap `render_paragraph_math`'s own
/// doc comment names explicitly. Returns the display flag (`true` for `\[`, `false` for `\(`) and the
/// matching closer character (`]`/`)`) `render_backslash_math` should now scan ahead for.
fn backslash_math_opener(gap: Option<&str>, t: &str) -> Option<(bool, char)> {
    if gap != Some("\\") {
        return None;
    }
    match t.chars().next()? {
        '[' => Some((true, ']')),
        '(' => Some((false, ')')),
        _ => None,
    }
}

/// Accumulates every event from `first_text` (whose own opening bracket/paren `backslash_math_opener`
/// just confirmed, in `walk_inline_math`) up to and including whichever later event's own gap+payload
/// closes it — the mirror image of the opener: a lone-backslash gap immediately followed by `closer`
/// — then lifts the whole accumulated span exactly like any other match (`content.trim()`, matching
/// `flush_math`'s own `latex.trim().to_string()`). `prev_end` is threaded through (not recomputed
/// from `events` after this returns, since this function is the one that actually consumes them) so
/// the caller's own next gap computation is correct once control returns to it.
///
/// ## Why "no closer found" is a *real*, not merely defensive, case — and why the fallback replays
/// events rather than reconstructing text
///
/// `\[`/`\(` are CommonMark's own generic backslash-escape syntax for *any* ASCII punctuation, not a
/// notation this file's own math extension owns — so an escaped bracket that is **not** math at all
/// (used, say, to stop `[text](url)` from parsing as a link — `inline_corpus`'s own "escaped brackets
/// are literal" case, `"This is \[not a link](nope).\n"`) looks *identical*, at the gap+payload level,
/// to a genuine backslash-math opener: both are a lone-backslash gap followed by `[`. The only thing
/// that tells them apart is whether a **matching closer gap** (`\]`) turns up later on the same line —
/// exactly the same way `scan_inline_math`'s own `find_close` decides it, searching raw text for the
/// literal closer string and falling through to "ordinary escaped character" when it is not found (see
/// that function's own doc comment). This function has to make the identical judgment call, but working
/// from events rather than raw text — so it cannot know in advance whether it is looking at a math
/// span or an ordinary escape, and must be prepared to "give the events back" faithfully if no closer
/// ever arrives (buffered here, verbatim, precisely so `replay_events` can push each one through
/// **its own regular dispatch** rather than reconstructing anything from `content`). Confirmed
/// directly, not merely inferred: the legacy renderer's own rendering for `"This is \[not a link](nope).\n"`
/// was `["This is ", "[not a link", "]", "(nope)."]` — **four separate spans**, no backslash anywhere in
/// the output at all (unsurprising once the reason is clear: the legacy renderer's own rendering, like this
/// one, worked from an event stream, and pulldown-cmark's own escape handling had already stripped the
/// backslash — and only the backslash, never the bracket it escapes — out of the event stream entirely
/// before either renderer ever got to see it; see `backslash_math_opener`'s own doc comment for the
/// exact byte ranges that prove this). A hand-rolled literal string (`"\\[" + content`, an earlier
/// version of this function did exactly that) reintroduces a backslash neither renderer's own event
/// walk would ever produce, *and* flattens what the legacy renderer kept as several separate spans into one —
/// two independent bugs from the same shortcut. `scan_inline_math` is also explicitly **line-scoped**
/// (see its own doc comment) — a `SoftBreak`/`HardBreak` encountered mid-scan ends the search the same
/// way running out of events entirely does, buffered and replayed exactly the same way.
///
/// `pending`, when `Some`, is the plain `Text` event `walk_inline_math`'s own loop was holding back
/// (see that function's own doc comment on "Why a plain `Text` event's own rendering is deferred") —
/// this function is what finally decides whether it gets the trailing-whitespace trim that precedes a
/// real lift or not, and flushes it accordingly at whichever of the three exits below actually fires:
/// trimmed (`true`) only on the "closer found, this really was math" exit; untouched (`false`) on
/// either "gave up" exit (ran out of events, or hit a line-scoping break with no closer yet) — both of
/// those mean the opener never actually panned out, so nothing before it should be trimmed either.
///
/// Any text trailing the closer *within that same event's own payload* (`"] more text"`) is handed to
/// `render_text_with_math` unchanged (`trailing_lift: false`) — not exercised by the parity corpus (no
/// case this function's own math-lifting half handles has anything trailing its own closer).
///
/// ## Content: a raw `src` slice, never an accumulation of escape-processed event payloads
///
/// `content_start` — `first_range.start + opener.len_utf8()`, the byte position *right after* the
/// opening bracket/paren, in `src` itself — and, once the closer is found, `range.start - 1` (the
/// closer *character*'s own position, minus the one ASCII byte the `gap == Some("\\")` check just
/// confirmed sits immediately before it — the closer's own escaping backslash, which belongs to the
/// closer delimiter `\)`/`\]` itself, never to the content) bound the math content as one direct
/// `&src[content_start..range.start - 1]` slice. This matters whenever the content itself holds a
/// backslash-escaped ASCII punctuation character (`` \(a\,b\) ``, say — CommonMark treats `\`
/// followed by *any* ASCII punctuation as an escape, not a notation this file's own math extension
/// owns, so this is unremarkable, ordinary LaTeX): an earlier version of this function accumulated
/// `content` by `push_str`-ing each intervening `Event::Text`'s own **already escape-processed**
/// payload instead (`content.push_str(t)`) — correct for the *opener*/*closer* detection itself
/// (`backslash_math_opener`'s own gap-based check does not need the content at all), but silently
/// **dropped every backslash the content's own internal escape(s) had** — `\(a\,b\)` accumulated as
/// `"a,b"`, not `"a\,b"` — a real, confirmed mismatch against production's own `collect_math_exprs`
/// (`code_span_corpus`'s own `math` field, and `math_url`, both computed straight off `src`), never
/// exercised by the parity corpus before it grew a case with an escape *inside* the expression itself
/// (every existing case — `y` — has none). The raw slice sidesteps the whole class of bug the same way
/// `render_dollar_math_tail`'s own doc comment explains for `$…$`/`$$…$$`: **`src` never had the
/// backslash stripped out of it at all** — only the *event stream* pulldown-cmark's own escape
/// handling produced does — so reading the content directly from `src`, bounded by two already-known
/// byte positions, reproduces it byte-for-byte, with no reconstruction (and so no possibility of
/// silently dropping anything) needed. Safe even across an intervening `Event::Code` (an inline code
/// span embedded inside the math content, e.g. `` \(a `x` b\) `` — not exercised by the parity corpus,
/// but not excluded by this function's own detection loop either): `range.start`/`range.end` are raw
/// byte positions regardless of what kind of event they belong to, so the slice still reconstructs the
/// content verbatim, backticks included, exactly as `scan_inline_math`'s own raw-text scan would.
#[allow(clippy::too_many_arguments)] // one call site (walk_inline_math); each argument is its own distinct, already-computed piece of state — bundling any subset into a struct would only rename the same eight pieces of information, not reduce them
fn render_backslash_math<'a>(
    w: &mut Writer<'_>,
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    pending: Option<pulldown_cmark::CowStr<'a>>,
    first_text: pulldown_cmark::CowStr<'a>,
    first_range: Range<usize>,
    display: bool,
    closer: char,
    src: &str,
    prev_end: &mut Option<usize>,
) {
    let opener = if display { '[' } else { '(' };
    let content_start = first_range.start + opener.len_utf8();
    let mut fallback: Vec<Event<'a>> = vec![Event::Text(first_text)];
    // Tracks whether the loop is currently inside a nested `Start`/`End` span (a markdown link, say)
    // sitting *between* the opener and the eventual closer — mirrors `render_multiline_display_math`'s
    // own identical `depth` counter, for the identical reason: a `Start` event's own `range` spans its
    // *entire* construct (matching its own later `End`'s range — pulldown-cmark's convention), so its
    // first child's own `range.start` sits *earlier* than the `Start` event's own `range.end`. Gap-
    // computing (`src[prev_end..range.start]`) against that child while `prev_end` still holds the
    // parent `Start`'s own `range.end` slices with `prev_end > range.start` — an invalid, **panicking**
    // range. Confirmed directly: `"...compilation. \([#519](https://…/pull/519))\n"` (a real
    // CommonMark generic backslash-escape — `\(`, stopping this from parsing as an ordinary open
    // paren — immediately followed by a real markdown link, drawn straight from a `~/.cargo/registry`
    // README's own CHANGELOG.md) panicked exactly this way before this guard existed, crashing the
    // whole preview on ordinary, valid input — a design-principle-#3 violation, not a documented gap.
    let mut depth: i32 = 0;
    loop {
        let Some((ev, range)) = events.next() else {
            if let Some(p) = pending {
                render_text_with_math(w, &p, false);
            }
            replay_events(w, fallback);
            return;
        };
        // Only a *top-level* (`depth == 0`) gap is ever a meaningful closer candidate in the first
        // place — `backslash_math_opener`'s own doc comment: the question is whether a bare `\)`/`\]`
        // gap turns up on the same line, never whether one happens to sit inside some unrelated
        // nested construct's own interior — so skipping it entirely while `depth > 0` changes no
        // real detection, only avoids the invalid slice above.
        let gap = if depth == 0 {
            prev_end.map(|p| &src[p..range.start])
        } else {
            None
        };
        *prev_end = Some(range.end);
        match &ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        let is_break = depth == 0 && matches!(ev, Event::SoftBreak | Event::HardBreak);
        if depth == 0 {
            if let Event::Text(t) = &ev {
                if gap == Some("\\") && t.starts_with(closer) {
                    if let Some(p) = pending {
                        render_text_with_math(w, &p, true);
                    }
                    // `range.start` is the closer *character*'s own position — but the `gap ==
                    // Some("\\")` check just above confirms the one byte immediately before it is
                    // the closer's own escaping backslash (ASCII, always exactly one byte), which
                    // belongs to the closer delimiter `\)`/`\]` itself, never to the content —
                    // excluded here (`- 1`) the same way `opener.len_utf8()` is added, not
                    // subtracted, on `content_start`'s own side.
                    let content = &src[content_start..range.start - 1];
                    render_math_slot(w, content.trim(), display);
                    let rest = &t[closer.len_utf8()..];
                    if !rest.is_empty() {
                        // No lookahead available here for whether *this* trailing text is itself
                        // immediately followed by another backslash-math opener (`trailing_lift:
                        // false`) — not exercised by the parity corpus (see this function's own doc
                        // comment on its own "text trailing the closer" caveat).
                        render_text_with_math(w, rest, false);
                    }
                    return;
                }
            }
        }
        fallback.push(ev);
        if is_break {
            // `scan_inline_math` never looks past a physical line's own end for a closer — see this
            // function's own doc comment.
            if let Some(p) = pending {
                render_text_with_math(w, &p, false);
            }
            replay_events(w, fallback);
            return;
        }
    }
}

/// Dispatches every buffered event through the exact same handling `walk_inline_math`'s own loop
/// would have given it, had `backslash_math_opener` never intercepted the first one — the "give the
/// events back unchanged" half of `render_backslash_math`'s own fallback (see that function's own doc
/// comment for why this exists, and why it cannot be a hand-rolled literal string instead). A `Start`
/// consumes its own matching `End` from `events`'s own remainder here — nested content inside a failed
/// scan (an inline code span, an emphasis run, ...) balances correctly the same way it would have in
/// `walk_inline_math`'s own loop, since `events` is the buffered slice's own full, balanced sub-sequence.
fn replay_events<'a>(w: &mut Writer<'_>, events: Vec<Event<'a>>) {
    let mut it = events.into_iter();
    while let Some(ev) = it.next() {
        match ev {
            // `trailing_lift: false` — this is the "gave up, nothing here was math" path; nothing
            // downstream of it should get math's own trailing-trim treatment.
            Event::Text(t) => render_text_with_math(w, &t, false),
            Event::Code(c) => {
                ensure_fresh_after_math(w);
                w.write_code_span(&c);
            }
            Event::SoftBreak => {
                ensure_fresh_after_math(w);
                w.join_with_space();
            }
            Event::HardBreak => {
                ensure_fresh_after_math(w);
                w.end_row();
            }
            Event::Start(tag) => {
                ensure_fresh_after_math(w);
                start_inline_tag(&mut it, w, tag);
            }
            // Mirrors `walk_inline`'s own defensive catch-all — see that function's own doc comment.
            _ => {}
        }
    }
}

/// Whether `t` (a plain `Event::Text` payload, *not* itself a backslash-math opener —
/// `backslash_math_opener` is always checked first at the one call site, in `walk_inline_math`) still
/// has a dollar-math span open at its own end once `scan_inline_math` has scanned it in full — the
/// dollar analogue of what `backslash_math_opener` answers for `\(`/`\[`, but reusing
/// `scan_inline_math` itself (see the module doc comment's own "no second math detector") rather than
/// a hand-rolled gap check, because `$…$`/`$$…$$` cannot be recognized the same narrow way: the opener
/// is not necessarily this event's own very first character (arbitrary literal text can precede it
/// within the same payload — `` "intro text $$a" `` — see `render_dollar_math_tail`'s own doc comment
/// for how that leading text still renders correctly once the span resolves), so there is no fixed
/// byte offset to gap-check against the way `backslash_math_opener`'s own `t.chars().next()` does.
///
/// `scan_inline_math`'s own accumulation buffer (`buf`, the trailing fragment it hands back once `t`
/// is exhausted) is, provably, an exact byte-for-byte **suffix** of `t` itself: every branch of its own
/// scan either `push_str`s the identical source slice it just consumed or, for the one single-byte
/// fallback (`buf.push('$'); i += 1;`, an attempted `$$`/`$` that failed its own `find_close`/
/// `find_inline_dollar` check), pushes exactly the one byte at the position it just advanced past — so
/// `buf`'s own content, at every point, is *literally* `t`'s own bytes, never a reconstruction — and
/// `flush_math` only ever *clears* `buf` (on a real match), never edits it in place. `buf` containing a
/// live, un-escaped `$` character at all — not merely being non-empty, since `scan_inline_math` already
/// degrades an *ordinary* trailing literal fragment (no `$` in it at all) to a plain, untouched
/// `MathPart::Text` the same way it always has — is the one signal that distinguishes "this event's own
/// text alone gave up on a `$`/`$$` it saw but could not close" (the shape this function exists to
/// catch) from "there is nothing math-shaped in this text at all" (the overwhelmingly common case,
/// where nothing about `walk_inline_math`'s own dispatch should change).
///
/// Whether that trailing `$` is a **genuine** escape-split math opener (`` "$$a" `` from `` $$a\,b$$
/// ``, this function's own reason for existing) or an **ordinary**, never-closing literal one (`` "It
/// costs $50." ``, ordinary currency prose with nothing else remotely math-shaped anywhere near it) is
/// not decided here at all — both look identical from inside `t` alone, the same ambiguity
/// `backslash_math_opener`'s own doc comment already documents for an escaped bracket that turns out
/// not to be math either. `render_dollar_math_tail` is where that gets resolved, the same way
/// `render_backslash_math` resolves its own identical ambiguity: by looking for a closer, and — if one
/// never turns up — giving the events back unchanged rather than guessing.
fn dollar_math_still_open(t: &str) -> bool {
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut mask = Vec::new();
    scan_inline_math(t, &mut parts, &mut buf, &mut mask);
    buf.contains('$')
}

/// The dollar-delimited (`$…$`/`$$…$$`) analogue of `render_backslash_math` — accumulates events
/// starting from `first_text` (whose own trailing `$`/`$$` `dollar_math_still_open` just flagged as
/// unresolved) until either a raw re-scan finds a closer or the attempt is given up on. Mirrors that
/// function's own two-exit shape (closer found; ran out of events, or hit a line-scoping break) and its
/// own `pending`/`fallback`/`replay_events` machinery — see `render_backslash_math`'s own doc comment
/// for the shared reasoning behind each of those (particularly "Why 'no closer found' is a *real*, not
/// merely defensive, case", which applies here identically: an ordinary, never-closing currency `$` is
/// indistinguishable from a genuine opener until a closer either turns up or does not) — but, unlike
/// that function, does **not** gate which events it is willing to consume at all: every event this loop
/// sees (`Text`, `Code`, `Start`/`End`, `SoftBreak`/`HardBreak`) is buffered into `fallback` and folded
/// into the growing raw slice, the identical "just keep consuming, event by event, until a break or the
/// events run out" shape `render_backslash_math`'s own loop already has (it never special-cases
/// `Start`/`Code` either — see that function's own doc comment). What *is* specific to this function is
/// the "closer found" test itself (`Why this cannot be a single gap+char check`, below) and a
/// depth-tracking guard neither `render_backslash_math` nor `walk_inline_math`'s own dispatch needs
/// (`Never stop mid-construct`, below, on the crash a version of this function without it actually hit).
///
/// ## Why this cannot be a single gap+char check the way `backslash_math_opener` is
///
/// `\(`/`\[` are recognized by a **fixed, narrow** shape — gap `"\\"`, immediately followed by a
/// specific opening character, always this event's own very first byte (`backslash_math_opener`'s own
/// doc comment) — so `render_backslash_math` only ever needs to watch, event by event, for the mirror
/// image: gap `"\\"` immediately followed by the specific closer. `$…$`/`$$…$$` has no such fixed
/// shape at either end: the opener can sit anywhere inside `t`'s own text (arbitrary literal content
/// may precede it — `` "intro text $$a" ``), and the closer is not one specific *character* to
/// gap-check for but the same **substring search** `scan_inline_math`'s own `find_close`/
/// `find_inline_dollar` already perform. Re-running `scan_inline_math` itself — on a **raw** (never
/// escape-processed) slice of `src` that grows to cover every event consumed so far, each time through
/// the loop below — is what reuses that existing logic rather than re-deriving a second, narrower
/// version of it: the same "no second math detector" principle `render_paragraph_math`'s own doc
/// comment states for the single-event case, extended across however many event boundaries this one
/// has to cross.
///
/// ## Raw slicing: why the growing slice is always safe, and why it does not over-match
///
/// The slice re-scanned each time through the loop is `&src[first_range.start..end]` — **raw** `src`,
/// never a concatenation of escape-processed event payloads — for the identical reason
/// `render_backslash_math`'s own content is now computed the same way (see that function's own doc
/// comment): the backslash a `` \, ``/`` \_ ``/... escape inside the math content needs to survive is
/// not part of *any* individual event's own range at all, only of `src` itself, between two already-
/// known byte positions. `end` grows monotonically (`end.max(range.end)`, never a bare assignment —
/// see "Never stop mid-construct" for why a bare one is unsound) and the loop checks for resolution
/// (`depth == 0 && !buf.contains('$')`) after *every* event, so it never reads or renders more of `src`
/// than the shortest span that actually closes the dollar span — a later paragraph, or even a later
/// *sentence*, can never leak into this scan by growing past its own real closer. Whether raw slicing
/// itself is ever *unsafe* — leaking a backslash from some *other*, unrelated escape into rendered
/// output — depends entirely on which of the two exits below fires, not on how far growth went to get
/// there: the "closer found" exit renders straight from *this* raw slice (correct — the content genuinely
/// does span everything between the real opener and the real closer, arbitrary intervening markup
/// included, the identical way production's own raw-source `scan_inline_math` already treats such markup
/// as ordinary literal characters); the "give up" exit never renders from the raw slice at all — see
/// that exit's own explanation below for why.
///
/// On the "closer found" exit, the re-scanned `parts` may include real literal text *before* the
/// match too (`` "intro text " `` from `` "intro text $$a\,b$$" ``, say) — `scan_inline_math`'s own
/// `flush_math` produces that the same way it always has, from *this* raw slice, so no separate
/// handling is needed for it: `render_math_parts` renders the whole resolved list — literal fragments
/// and the lift together — in one pass, with the same `idx`-based trim rules `render_text_with_math`
/// itself uses (`render_math_parts`'s own doc comment). Any further trailing fragment `scan_inline_math`
/// leaves behind on *this* pass (now confirmed `$`-free, or it would not have stopped growing) is
/// rendered immediately too (`trailing_lift: false`) rather than deferred as a fresh `pending` for the
/// outer walk — the identical simplification `render_backslash_math`'s own doc comment already accepts
/// for "text trailing the closer within that same event's own payload" (not exercised by the parity
/// corpus either way).
///
/// On "give up" (ran out of events, or hit a top-level line-scoping break — see "Never stop
/// mid-construct" for why *top-level* matters), `fallback` — buffered here the identical way
/// `render_backslash_math`'s own is, starting from `first_text` itself — is replayed verbatim via
/// `replay_events`, so every buffered event (including `first_text`) renders through its own normal,
/// escape-*processed* dispatch, exactly as if `dollar_math_still_open` had never fired at all: this is
/// what keeps an ordinary, never-closing `$` (currency prose, however much unrelated markup or however
/// many unrelated escapes follow it before this loop finally gives up) rendering identically to before
/// this function existed — `replay_events`'s own `Event::Text(t) => render_text_with_math(w, &t,
/// false)` arm renders each buffered event's own **escape-processed** payload, never the raw slice this
/// function scanned to look for a closer, so nothing pulled into a failed growth attempt (an unrelated
/// `` \*escape\* ``, an ordinary `**bold**`, ...) can ever leak a stray literal backslash — or stray
/// markup-as-literal-text — onto the screen.
///
/// ## Never stop mid-construct
///
/// Neither exit above may fire while `depth > 0` — i.e. after this loop has consumed a `Start` but not
/// yet its own matching `End`. Stopping there is a real, confirmed crash this depth guard exists to
/// close, not a merely theoretical concern: `Event::Start`'s own `range` already spans its *entire*
/// nested content (confirmed directly — parsing `"**bold**"` alone reports `Start(Strong)`/
/// `End(Strong)` both spanning the *whole* `0..8`, while the nested `Text("bold")` in between reports
/// its own, narrower `2..6`), so a version of this loop that gave up the instant a non-continuing event
/// turned up (buffering only that one `Start` into `fallback`, then returning) left `Text("bold")`/
/// `End(Strong)` **still sitting, unconsumed, in the very same `events` iterator**
/// `walk_inline_math`'s own outer loop keeps driving once this function returns — whose own next
/// `range.start` is now *behind* `end`/`prev_end`, which this loop had already advanced past via the
/// `Start`'s own wide range alone. `&src[prev_end..next.start]` panics the moment that happens ("byte
/// range starts at X but ends at Y", `X > Y`) — reproduced directly, before this guard existed, by
/// `` "It costs $50 **bold** text.\n" `` (ordinary prose; no escape anywhere near the `Start`/`End` pair
/// at all — `$50` alone is what makes `dollar_math_still_open` trigger, per that function's own doc
/// comment on how broad its own trigger condition deliberately is). Waiting for `depth` to return to
/// `0` mirrors how `render_backslash_math`'s own loop achieves the identical safety *implicitly*, simply
/// by never special-casing `Start`/`End` at all and so never stopping partway through either — the same
/// property this loop now has explicitly, via `depth`, because — unlike `render_backslash_math`, which
/// only ever needs to scan for one specific closer *character* — this loop's own "found a closer" check
/// (a full `scan_inline_math` re-scan) has to run *somewhere*, and running it right after consuming a
/// `Start` but before its `End` is exactly the state this guard rules out.
#[allow(clippy::too_many_arguments)] // mirrors render_backslash_math's own identical allow — one call site (walk_inline_math), each argument its own distinct piece of state
fn render_dollar_math_tail<'a>(
    w: &mut Writer<'_>,
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    pending: Option<pulldown_cmark::CowStr<'a>>,
    first_text: pulldown_cmark::CowStr<'a>,
    first_range: Range<usize>,
    src: &str,
    prev_end: &mut Option<usize>,
) {
    let mut end = first_range.end;
    let mut fallback: Vec<Event<'a>> = vec![Event::Text(first_text)];
    // Tracks whether the event just consumed sits inside an unfinished `Start`/`End` pair this loop
    // has itself already started consuming (never negative — a paragraph's own inline event range is
    // always balanced, by construction; `checked_sub`/`saturating_sub` would silently paper over a
    // real desync instead of surfacing it, so plain arithmetic is used and the loop simply cannot
    // decrement past zero given that invariant). See "Never stop mid-construct" below for why this
    // exists at all.
    let mut depth: usize = 0;
    loop {
        let raw = &src[first_range.start..end];
        let mut parts: Vec<MathPart> = Vec::new();
        let mut buf = String::new();
        let mut mask: Vec<bool> = Vec::new();
        scan_inline_math(raw, &mut parts, &mut buf, &mut mask);
        if depth == 0 && !buf.contains('$') {
            // Resolved: either a real closer turned up somewhere in the growing raw slice, or growth
            // stopped because the (now fully re-scanned) text genuinely has no `$` left in it at all.
            // Either way, `parts`/`buf` are this pass's own correct, final answer — render them. Gated
            // on `depth == 0` for the identical reason the "keep growing" branch below is — see "Never
            // stop mid-construct".
            let pending_trailing_lift = matches!(parts.first(), Some(MathPart::Math { .. }));
            if let Some(p) = pending {
                render_text_with_math(w, &p, pending_trailing_lift);
            }
            if !buf.is_empty() {
                mask.push(false);
                parts.push(MathPart::Text(SourceRun::new(buf, mask)));
            }
            render_math_parts(w, parts, false);
            return;
        }
        let Some((ev, range)) = events.next() else {
            if let Some(p) = pending {
                render_text_with_math(w, &p, false);
            }
            replay_events(w, fallback);
            return;
        };
        *prev_end = Some(range.end);
        // `end` grows to the *widest* extent seen so far, never merely the latest event's own — a
        // `Start`'s own range already spans its *entire* nested content (confirmed directly: parsing
        // `"**bold**"` alone reports `Start(Strong)`/`End(Strong)` both spanning `0..8`, the whole
        // thing, while the nested `Text("bold")` in between reports its own, narrower `2..6`), so the
        // very next event's own `range.end` can be *smaller* than `end` already is — `.max`, not a
        // bare assignment, keeps the raw slice from ever shrinking back and silently dropping bytes it
        // already decided to include.
        end = end.max(range.end);
        let is_break = matches!(ev, Event::SoftBreak | Event::HardBreak);
        match &ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        fallback.push(ev);
        // ## Never stop mid-construct
        //
        // Neither exit above (`depth == 0 && !buf.contains('$')` above; `is_break` here) may fire while
        // `depth > 0` — i.e. after this loop has itself consumed a `Start` but not yet its own matching
        // `End`. Stopping there — the crash this comment exists to explain, not merely guard against —
        // would leave that `End` (and everything between it and the `Start`, however much nested
        // content that is) *still sitting, unconsumed, in the very same `events` iterator*
        // `walk_inline_math`'s own outer loop keeps driving once this function returns: its own next
        // `range.start` would be *behind* `end`/`prev_end`, which this loop has already advanced past
        // via the `Start`'s own wide range — exactly the "byte range starts at X but ends at Y" panic
        // `&src[prev_end..next.start]` produces the moment `X > Y`. Confirmed directly, before this
        // depth guard existed: `"It costs $50 **bold** text.\n"` — ordinary prose, no escape anywhere
        // near the `Start`/`End` pair at all — panicked here, because giving up the instant a `Start`
        // event turned up (this loop's own first version: `extendable = gap == "\\" && matches!(ev,
        // Event::Text(_))`, `false` for `Event::Start` unconditionally) buffered only the lone `Start`
        // into `fallback` and returned, leaving `Text("bold")`/`End(Strong)` orphaned. Waiting for
        // `depth` to return to `0` closes this structurally, not just for `Strong` — any inline
        // construct at all (`Emphasis`, `Link`, nested combinations of either) shares the same
        // wide-`Start`-range shape and so needs the identical protection. (`render_backslash_math`'s
        // own loop never special-cases `Start`/`End` either, so it never stops mid-construct this same
        // way — but it turned out to need its *own*, distinct `depth` guard regardless, for a second,
        // narrower reason this loop's own re-scan-the-growing-slice design happens not to share: gap-
        // *computing* against a child event while `prev_end` still holds its wide-spanning parent
        // `Start`'s own `range.end` is itself an invalid, panicking slice, degenerate `depth > 0`
        // buffering aside — see that function's own doc comment for the confirmed, real-content case.)
        if is_break && depth == 0 {
            if let Some(p) = pending {
                render_text_with_math(w, &p, false);
            }
            replay_events(w, fallback);
            return;
        }
    }
}

/// The shared checkpoint every path in this file that is about to render **real** (non-lifted)
/// content already runs through, ahead of its own normal handling — resolves both of the two states
/// that can leave a math-lifting paragraph's own leading edge unfinished:
///
/// 1. **A still-pending paragraph start** (`w.pending_para_start`) — this is the paragraph's own very
///    first content, and it turns out to be real text/an inline construct rather than a lifted math
///    expression, so the leading-blank-line convention `render_paragraph_math` deferred (see
///    `pending_para_start`'s own doc comment for why it *had* to defer, rather than open eagerly the
///    way `render_paragraph` still does) needs opening now, via `open_pending_para_start`.
/// 2. **The last thing rendered was a lifted math expression** (`w.after_math`) — opens a fresh
///    paragraph-start boundary (the exact same two-part blank-line push, verbatim) before whatever
///    comes next: everything following a lift is treated as starting fresh (see
///    `render_paragraph_math`'s own doc comment) — a boundary, not a second parse, since nothing is
///    actually re-parsed here.
///
/// These two states can never both apply to the very same call: `pending_para_start` is only ever
/// `Some` before *anything* has rendered in this walk, and `after_math` only ever becomes `true`
/// *after* `render_math_slot` has already resolved (opened or dropped) whatever pending slot there
/// was — so checking `pending_para_start` first, unconditionally, then `after_math`, is safe; at most
/// one of the two blocks below ever actually does anything for a given call. Both are no-ops in the
/// overwhelmingly common case (most events in most paragraphs are neither the walk's own first content
/// nor immediately follow a lift), so every call site in `walk_inline_math`'s own dispatch loop calls
/// this unconditionally, ahead of its own normal handling, rather than each having to re-derive either
/// condition itself. The explicit `w.after_math = false` is redundant with `emit_blank_row`'s own
/// side effect (`Writer::emit_row` clears it unconditionally — see that struct's own doc comment) but
/// kept anyway: this function's own contract ("the flag is consumed") should hold by its own reading,
/// not merely as an accident of what its callee happens to also do.
fn ensure_fresh_after_math(w: &mut Writer<'_>) {
    w.open_pending_para_start();
    if w.after_math {
        if w.pending_block_gap {
            w.emit_blank_row();
        }
        w.emit_blank_row();
        w.pending_block_gap = false;
        w.after_math = false;
    }
}

/// Renders one `Event::Text` payload, scanning it for LaTeX math via `scan_inline_math` (reused
/// verbatim — see `render_paragraph_math`'s own doc comment) and lifting every occurrence onto its own
/// placeholder line via `render_math_slot`. Mirrors `Writer::text`'s own multi-physical-line handling
/// (`text.lines().enumerate()`, a blank line between them) — see that method's own doc comment for why
/// this is rare but not impossible — scanning *each* physical line independently, matching
/// `scan_inline_math`'s own line-scoped contract exactly (see its own doc comment).
///
/// A line with no math at all degrades to the exact span `Writer::text` itself would have pushed
/// (`scan_inline_math` reports it back as a single, untouched `MathPart::Text` covering the whole
/// line — see that function's own accumulation via `flush_math`) — this is what lets
/// `walk_inline_math` call this for *every* one of its own `Event::Text` events unconditionally, with
/// no separate "does this text contain math" pre-check of its own: the fallback path already *is*
/// that check, structurally, not an approximation of it.
///
/// ## Trimming: reproducing what an independent re-parse does to a fragment's own edges
///
/// `scan_inline_math`'s own accumulation buffer never trims anything — a fragment sitting strictly
/// *between* two lifts (or before the first / after the last) keeps whatever leading/trailing
/// whitespace the original, unsplit text had right there, because in the *original* text that
/// whitespace was never actually at a paragraph's own edge (there was more content, the math itself,
/// still ahead). Once production lifts the math out and independently re-parses what is left, that
/// same whitespace genuinely *is* now at a fragment's own edge — and CommonMark's own paragraph-content
/// rule discards it (`start_paragraph`'s own trailing-whitespace behavior; verified directly against
/// the real production pipeline, not merely inferred — `code_span_corpus`'s own "no backticks at all"
/// case renders the tail of `"...$x$ b"` as a bare `"b"`, its own leading space gone, and the text
/// immediately before a lift with `"...\`K\` $x$"` as `"...\`K\`"` with no trailing space). This
/// function reproduces exactly that: a `MathPart::Text` fragment gets its **leading** space/tab
/// trimmed whenever a lift immediately precedes it (`idx > 0` — there is no other way for it to have
/// gotten there; `scan_inline_math`/`flush_math` only ever start a new fragment right after a lift) and
/// its **trailing** space/tab trimmed whenever a lift immediately follows it (`idx + 1 < n`, or —
/// `trailing_lift` — a *later* event, right after this whole payload, opens a backslash-math span
/// `scan_inline_math` could not have seen from inside this call at all; see `walk_inline_math`'s own
/// call site for where that gets decided) — matching CommonMark's own "space or tab" definition, not
/// a generic `.trim()` (which would also eat a legitimate blank line marker or non-ASCII space this
/// rule was never meant to touch). A fragment that trims down to nothing contributes no span and does
/// not open a fresh-paragraph boundary either — an entirely-whitespace gap between two adjacent lifts
/// (rare, but handled the same principled way rather than left to guess) is exactly what an
/// independent re-parse of whitespace-only text would produce too: nothing at all.
fn render_text_with_math(w: &mut Writer<'_>, text: &str, trailing_lift: bool) {
    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 {
            w.emit_blank_row();
        }
        let mut parts: Vec<MathPart> = Vec::new();
        let mut buf = String::new();
        let mut mask: Vec<bool> = Vec::new();
        scan_inline_math(line, &mut parts, &mut buf, &mut mask);
        // `scan_inline_math` itself never touches `mask` for the *trailing* fragment it leaves
        // behind in `buf` (only `flush_math`, for a fragment that precedes an actual match, does) —
        // `split_math`'s own caller supplies that missing entry right after every `scan_inline_math`
        // call (`buf.push('\n'); mask.push(false);`, with `false` because that branch is only
        // reached for a non-code line — see `flush_math`'s own doc comment for the identical
        // reasoning). `line` here is already one bare, newline-free physical line (this loop's own
        // `text.lines()` split it out), so `buf` — if non-empty — is always exactly the one line
        // `SourceRun::new`'s own `code.len() == text.lines().count()` invariant expects a single
        // matching `mask` entry for; `false` is correct for the identical reason it is there.
        if !buf.is_empty() {
            mask.push(false);
            parts.push(MathPart::Text(SourceRun::new(buf, mask)));
        }
        let is_last_line = i + 1 == line_count;
        render_math_parts(w, parts, trailing_lift && is_last_line);
    }
}

/// The per-fragment rendering loop `render_text_with_math` itself drives, over one physical line's
/// worth of already-`scan_inline_math`-produced `parts` — extracted so `render_dollar_math_tail` (an
/// event-**spanning** group's own resolved parts, not confined to one `Event::Text`'s own payload) can
/// share the identical trim/dispatch logic rather than a second, hand-copied near-duplicate of it. See
/// `render_text_with_math`'s own doc comment ("Trimming: reproducing what an independent re-parse does
/// to a fragment's own edges") for exactly what `trailing_lift` means and why the `idx`-based trim
/// rules below are correct; unchanged here, verbatim, from that function's own former inline loop.
fn render_math_parts(w: &mut Writer<'_>, parts: Vec<MathPart>, trailing_lift: bool) {
    let n = parts.len();
    for (idx, part) in parts.into_iter().enumerate() {
        match part {
            MathPart::Text(run) => {
                let mut t = run.text().to_string();
                if idx > 0 {
                    t = t.trim_start_matches([' ', '\t']).to_string();
                }
                if idx + 1 < n || (trailing_lift && idx + 1 == n) {
                    t = t.trim_end_matches([' ', '\t']).to_string();
                }
                if t.is_empty() {
                    continue;
                }
                ensure_fresh_after_math(w);
                let style = w.cur_style();
                w.append_span(Span::styled(t, style));
            }
            MathPart::Math { latex, display } => {
                // Unlike a `MathPart::Text` fragment, a lift never opens a fresh-paragraph
                // boundary of its own first — production appends its own placeholder line(s)
                // directly, with no gap check of any kind, regardless of what came immediately
                // before it (verified directly: `code_span_corpus`'s own cases show the math
                // placeholder line landing right after the preceding text line, never a blank
                // line apart from it).
                render_math_slot(w, &latex, display);
            }
        }
    }
}

/// Renders one lifted math expression's own placeholder — the exact three functions
/// `render_markdown_with_images`'s own `BlockPart::Math` arm calls, reused verbatim, so a document
/// that lifts math renders identically whether a live `Picker` answers `math_slot` with a real raster
/// (`MathSlot::Image`), a background render still in flight (`MathSlot::Loading`), or neither
/// (`MathSlot::Raw`, design principle #3's own safe degrade — the raw LaTeX source, dimmed). Sets
/// `w.pending_block_gap = false` and `w.after_math = true` unconditionally on its own way out — see
/// `Writer`'s own doc comment on `after_math` for why every placeholder line goes straight to
/// `self.lines` here rather than through `emit_row` (which would immediately clear the very flag this
/// sets). Reads `w.math` defensively (`let Some(..) = .. else { return }`) rather than assuming it is
/// always populated when this runs — every real call site is reached exclusively through
/// `render_paragraph_dispatch`'s own `w.math.is_some()` check, so this should never actually trigger,
/// but principle #3 (`CLAUDE.md`) applies regardless: degrade to rendering nothing for this one
/// expression rather than panic if that invariant is ever wrong.
///
/// `w.drop_pending_para_start()` runs first, unconditionally — this is the one place a math-lifting
/// paragraph's own still-pending leading-blank-line slot (see `pending_para_start`'s own doc comment)
/// can be resolved by a *lift*, rather than by real text (`ensure_fresh_after_math`'s own
/// `open_pending_para_start` call): a no-op whenever nothing is pending (the overwhelmingly common
/// case — a lift preceded by real text on the same line, or not the walk's own first content at all),
/// but exactly the fix for the shape that motivated this pending-slot mechanism in the first place —
/// a math expression that is a paragraph's own entire, only content — where dropping it here (never
/// opening it) is what keeps that paragraph's own placeholder from being preceded by an orphaned blank
/// row nothing else ever fills.
fn render_math_slot(w: &mut Writer<'_>, latex: &str, display: bool) {
    w.drop_pending_para_start();
    let Some(math) = w.math.as_ref() else {
        return;
    };
    let slot = (math.slot)(latex, display);
    let width = math.width;
    match slot {
        MathSlot::Image { cols, rows } => {
            // `+ w.heading_rule_shift`: converts the *pre*-decoration row index this line is about
            // to occupy into the row it will actually land on in `RenderOut::lines`, once
            // `render_doc`'s own single, whole-document `decorate_headings_and_extras` pass has run
            // — see that field's own doc comment for the full mechanism and why every placement in
            // this file needs it.
            let placement_line = w.lines.len() + w.heading_rule_shift;
            w.lines
                .extend(math_placeholder_lines(cols, rows, width, display));
            // Mirrors `math_placeholder_lines`'s own `pad` exactly (display math is centered, inline
            // math is left-aligned at column 0) — the identical single-item centering formula
            // `flush_row_group` uses for a block image's own `col`.
            let col = if display {
                width.saturating_sub(cols) / 2
            } else {
                0
            };
            w.images.push(ImagePlacement {
                url: math_url(latex, display),
                alt: "math".into(),
                line: placement_line,
                col,
                cols,
                rows,
                fence_ord: None,
            });
        }
        MathSlot::Loading => {
            w.lines
                .extend(image_loading_line("math", "equation", width));
        }
        MathSlot::Raw => {
            w.lines.extend(math_raw_lines(latex, display));
        }
    }
    w.pending_block_gap = false;
    w.after_math = true;
}

/// Renders a `ThematicBreak` into `w`, in place — a literal `Line::from("---")`. `decorate_extras`
/// (called via `decorate_headings_and_extras`, back in `render_doc`) recognizes exactly this shape on an
/// *unprefixed* line and replaces it with the real full-width rule; see the module doc comment for
/// what happens instead when this line is `Quote`-prefixed.
fn render_rule(w: &mut Writer<'_>) {
    if w.pending_block_gap {
        w.emit_blank_row();
    }
    w.emit_row(Line::from("---"));
    w.pending_block_gap = true;
}

/// Whether `lang`, `body_spans`, and `extract_eligible` together mean *this* `CodeBlock` should be
/// diverted to a mermaid diagram placement (`render_mermaid_slot`) instead of being drawn as ordinary,
/// syntax-highlighted code — the one call site both `render_doc`'s own top-level dispatch and
/// `render_block`'s generic one go through, so a `CodeBlock`'s eligibility for extraction never gets
/// decided twice, in two slightly different ways.
///
/// A fence diverts when **all three** hold: `extract_eligible` (`ctx.extract_here`'s own value — see
/// that field's own doc comment for the full reasoning: production's own flat, line-based fence-open
/// check, `parse_fence`, is defeated by a blockquote's own `>` prefix exactly the way
/// `extract_block_image`'s is, and is explicitly skipped inside an alert's/`<details>`'s own
/// independently-re-parsed body too — a mermaid fence reached there really is drawn as ordinary code
/// in production, confirmed directly by `code_block_source_locs_inner`'s own `in_alert_body` doc
/// comment and pinned by `code_block_scanner_counts_exactly_what_the_renderer_draws`'s own "mermaid
/// inside an alert is a code block" case); `w.mermaid.fences_on` (mermaid extraction is on *at all*,
/// for the whole document — the identical probe production runs, see `MermaidCtx::fences_on`'s own
/// doc comment); and `is_mermaid_info(lang)` (the fence's own info string names it a mermaid fence —
/// the identical classifier production's own `split_block_parts_masked` calls). Falls through to
/// ordinary `render_code_block` whenever any one of the three does not hold — including, deliberately,
/// a mermaid fence nested *deeper* than this file's own `Quote` exclusion alone would catch: a mermaid
/// fence at a **list** item's own content column still diverts (list nesting never forces
/// `extract_eligible` off — see `BlockCtx.extract_here`'s own doc comment), matching production's own
/// identical, depth-insensitive `.trim()`-based line recognition for that shape. This same shape
/// (`"a mermaid fence at an ordered item's content column is diverted to a diagram"`) used to
/// *mismatch* the parity harness that once compared this renderer against the legacy,
/// tui-markdown-based one (deleted once the migration completed), for a wholly separate reason:
/// that legacy renderer's own flat, pre-parse fence deletion changed the surrounding list item's own
/// tight/loose shape, while `render_doc` reads the real, loose structure directly.
fn render_code_block_dispatch(
    w: &mut Writer<'_>,
    src: &str,
    lang: Option<&str>,
    body_spans: &[Range<usize>],
    extract_eligible: bool,
) {
    if extract_eligible && w.mermaid.fences_on && is_mermaid_info(lang.unwrap_or("")) {
        render_mermaid_slot(w, src, body_spans);
    } else {
        render_code_block(w, src, lang, body_spans);
    }
}

/// Renders one `CodeBlock` — fenced or indented, at any nesting depth this file covers (top level,
/// inside a list item, inside a quote — `render_code_block_dispatch`, this function's only caller,
/// reached from both `render_block`'s own `CodeBlock` arm and `render_doc`'s own top-level one) —
/// directly from the model's own `lang`/`body_spans`, using `code_header` for the language-badge row,
/// `highlight_body` for the syntect-highlighted, gutter+background body rows, and
/// `gutter_span`/`pad_to_width` for the bottom padding row.
///
/// Unlike the legacy renderer's own line-based fence detection — which had to *re-derive* a code
/// block's own extent from tui-markdown's own *rendered* output (a maximal run of `fg(White)` lines),
/// because that was all it had to go on — this already *has* the block's own exact byte extent,
/// straight from the one whole-document parse `Doc::parse` performed (see the module doc comment:
/// this is the entire point of this refactor). It therefore never mis-detects a block's own boundary
/// the way that line-based approach could — see `code_corpus`'s own "list item fence glued to a
/// following inline-code paragraph" cases, the very shape that motivated this refactor in the first
/// place — every `CodeBlock` this file's own `Doc::parse` reports gets exactly one header, drawn from
/// the model, never zero (contrast `render_bare_paragraph`'s own doc comment for the companion half of
/// that same fix: keeping whatever *follows* this block from gluing onto its own last row).
///
/// ## The blank line above the header
///
/// The leading-blank-line check here is `if !w.lines.is_empty() { emit_row(default) }` — which is
/// **not** the same condition every other block-rendering function in this file uses (`if
/// w.pending_block_gap`). This is deliberate: a fence is *always* preceded by exactly one blank row
/// whenever anything at all has already been rendered — even when the source has *no* blank line
/// before it (`"para\n```\naaa\n```\n"` renders identically to `"para\n\n```\naaa\n```\n"`, one blank
/// row either way — a fence is a leaf block that can interrupt a paragraph without one), even when the
/// immediately preceding content is a **tight** list item's own bare inline text with `pending_block_gap`
/// still `false` (`"- text\n  ```\n  aaa\n  ```\n"` still gets the blank row), and even when the code
/// block is a list item's own very *first* child with no leading text at all (`"- \n  ```\n  aaa\n
/// ```\n"` — the blank row lands right after the bare `"- "` marker row) — but *not* when the code
/// block is the very first thing in the whole document (`w.lines.is_empty()` still true at that
/// point). `w.pending_block_gap` plays no part in this decision at all — using it here instead (matching
/// every *other* block-rendering function's own convention) would skip the blank row for exactly the
/// two shapes above where `pending_block_gap` happens to already be `false` for an unrelated reason (a
/// **tight** item's own bare content, per `Writer::write_text`'s own doc comment, or `render_item`'s own
/// unconditional reset right before dispatching an item's first child) even though a fence still needs
/// one there.
///
/// The one further exception, `w.fresh_boundary` (see that field's own doc comment): a fence
/// directly following a GitHub alert or a `Details` block gets **no** leading blank row, even though
/// `w.lines` is (correctly) non-empty by then — that body was rendered by a fresh, independent
/// sub-`Writer` (`render_bar_prefixed_body`'s own isolation), so nothing precedes the fence *within
/// that sub-`Writer`'s own view* — the identical "fresh boundary" a lifted math expression already
/// reproduces via `after_math`.
///
/// ## The width used for the header/body/padding band
///
/// `w.width` (`render_doc`'s own incoming column count) directly, with no further reduction here —
/// never by list nesting, which (confirmed the same way — see this function's own leading-blank-line
/// evidence above, gathered by rendering real list content and reading the columns back) adds no left
/// margin of its own at all, in production or here. A fence nested inside a `Quote` still ends up
/// narrower, but that narrowing already happened one level up: `render_quote` isolates its own
/// children into a sub-`Writer` constructed with `width: w.width.saturating_sub(2)` (see that
/// function's own doc comment) — this function, called with `w` bound to *that* sub-`Writer`, simply
/// reads the width it was already given, the same way it would for a fence at any other nesting
/// depth. This keeps a quote-nested code block's own full-width background band from overflowing the
/// terminal once `render_quote`'s own re-emit loop prepends the `"> "` prefix on top of it — a
/// genuine judgment call, not something checked against a production precedent, because the legacy
/// renderer never decorated a quote-nested fence at all (its own line-based fence detection never
/// recognized one — the first rendered row read `"> ```"`, not `"```"`). See
/// `model::BlockKind::CodeBlock`'s own doc comment for why this model represents a quote-nested code
/// block like any other regardless ("an intentional superset of what `parser_code_blocks` reports"),
/// and `contains_unsupported`'s own doc comment above for where the old, blanket "any `CodeBlock`
/// anywhere in a `Quote`'s subtree is unsupported" screen used to stop this file from ever reaching
/// one at all.
///
/// ## Content: reconstructed from `body_spans`, never from a rendered run
///
/// `body_spans` is empty precisely when the fence has no content line at all (`model::BlockKind::
/// CodeBlock.body_spans`'s own doc comment) — checked directly, rather than via `body_text.is_empty()`
/// (which cannot tell "no content" apart from "exactly one *blank* content line": `code_body_text`
/// strips at most one trailing newline either way, so both shapes reconstruct to `""`) — so a
/// genuinely empty fence draws no body rows at all (matching `highlight_body`'s own early return for
/// an empty `body: &[String]`), while one blank content line still draws exactly one
/// (highlighted, gutter+background) blank row.
fn render_code_block(
    w: &mut Writer<'_>,
    src: &str,
    lang: Option<&str>,
    body_spans: &[Range<usize>],
) {
    if !w.lines.is_empty() && !w.fresh_boundary {
        w.emit_blank_row();
    }
    let content_w = w.width as usize;
    // The legacy renderer had to strip backticks from its own raw opening-fence text
    // (`opening_trimmed.trim_matches('`').trim()`); the model's own `lang` never carries backticks
    // (those are fence syntax, not part of the info string pulldown-cmark reports), so only the
    // `.trim()` half is still needed here.
    let lang_trimmed = lang.unwrap_or("").trim();
    let label = if lang_trimmed.is_empty() {
        "code"
    } else {
        lang_trimmed
    };
    w.emit_row(code_header(label, content_w, w.code));
    let body_text = code_body_text(body_spans, src);
    // Record the exact text `y c` will copy for this block — the same reconstruction just used to
    // draw it, at the moment its header actually landed on screen (`RenderOut::code_blocks`'s own doc
    // comment: this is the render pass's own record, never a second, source-line-scanning derivation
    // reconciled by count against it afterward). Pushed for an empty fence too (an opening fence
    // immediately followed by a closing one — `body_text` is `""`), matching `parser_code_blocks`'s
    // own identical "still push, even when the block turned out to have no content" contract.
    w.code_blocks.push(body_text.clone());
    let body_lines: Vec<String> = if body_spans.is_empty() {
        Vec::new()
    } else {
        body_text.split('\n').map(str::to_string).collect()
    };
    let theme = w.theme.clone();
    for line in highlight_body(
        &body_lines,
        lang_trimmed,
        content_w,
        w.code.bg,
        &theme,
        w.code.tab_width,
        w.code.wrap,
    ) {
        w.emit_row(line);
    }
    w.emit_row(pad_to_width(
        vec![gutter_span(w.code.bg)],
        content_w,
        w.code.bg,
    ));
    w.pending_block_gap = true;
}

/// Renders one extracted ```mermaid fence into `w`, in place — the mermaid analogue of
/// `render_math_slot`/`render_image_group`, mirroring production's own `BlockPart::Mermaid` handling in
/// `render_markdown_with_images` almost exactly. `code` (below) is this fence's own body,
/// reconstructed via `model::code_body_text(body_spans, src)` — the identical reconstruction
/// `render_code_block` itself already uses for an *ordinary* fence's own body, so a quote-nested
/// mermaid fence's own per-line `>` markers are excluded the same way (see
/// `model::BlockKind::CodeBlock.body_spans`'s own doc comment) — not that a quote-nested fence can
/// actually reach this function at all today: `render_code_block_dispatch`'s own `extract_eligible`
/// gate (`ctx.extract_here`) already keeps one from ever diverting here in the first place (see that
/// function's own doc comment), matching production's *own* documented "mermaid inside an alert is a
/// code block" behavior; `code_body_text`'s own quote-safety is simply not the reason this function
/// itself needs to worry about it.
///
/// One byte-level detail this function has to correct for explicitly before hashing: `code_body_text`
/// strips *at most one* trailing `\n` from the reconstructed body (see that function's own doc
/// comment — its own contract, not incidental), while production's own `fence` variable
/// (`split_block_parts_masked`'s own `mermaid: Option<String>` accumulator) keeps every line's own
/// trailing `\n`, including the last — every content line the accumulator ever pushes comes from
/// `src.split_inclusive('\n')`, and a mermaid fence's content lines are, by construction, never the
/// last line of the document (the closing ` ``` ` line always follows), so a non-empty, successfully
/// closed fence's own `fence` string *always* ends with exactly one `\n`. That makes "strip at most
/// one, re-add exactly one" a lossless round trip, not a guess: `code` below is `code_body_text`'s own
/// output with that one `\n` put back (`code_for_url`), confirmed directly against
/// `collect_mermaid_fences`'s own real extraction for both a single-line body (`"A-->B\n"`) and one
/// ending in several blank lines (`"A-->B\n\n\n"`) — see `mermaid_fence_url_matches_production_for_a_
/// top_level_fence`'s own doc comment for the exact cases and why a *list-nested* fence is
/// deliberately not among them (a different, pre-existing divergence — the list item's own content-
/// column indentation, stripped from `body_spans` by the real parser but retained verbatim by
/// production's own raw-line scan — that this fix does not, and structurally cannot, close; that
/// specific corpus case is a known, accepted divergence for an unrelated, `lines`-level reason of its
/// own, so its own URL never needing to match separately does not go unnoticed). `render_mermaid_block` itself already normalizes the *rendered lines*'
/// trailing-`\n` difference away internally (`code.trim_end_matches('\n')`, its own first statement),
/// unaffected by this fix either way — `code` (unchanged, still `code_body_text`'s own trimmed
/// output) is what still gets passed there for every rendering exit (the empty-body fallback above,
/// and the `Text` arm below). `w.mermaid.slot` itself, however, is handed the corrected, `\n`-restored
/// `hashed` string, not `code` — see this function's own body for why: the one real, stateful slot
/// closure (`app::md_render::build_decorated`'s own `mermaid_slot`) has no independent re-add step of
/// its own, so it has to be handed the fence's literal, hash-ready bytes directly, the same shape
/// `markdown.rs`'s own `BlockPart::Mermaid` carries elsewhere in this codebase (a `\n`-terminated
/// fence body).
///
/// Increments `w.mermaid.ord` — the running fence ordinal `ImagePlacement.fence_ord` records —
/// **before** checking whether `code` is empty (`let ord = fence_ord; fence_ord += 1; if
/// fence.trim().is_empty() { .. }`, in that order): a half-written ```mermaid fence
/// with no body still consumes an ordinal slot, so a *later*, real fence's own numbering never shifts
/// depending on whether an earlier, empty one happened to slot successfully (see `MermaidCtx::ord`'s
/// own doc comment).
///
/// No leading-blank-line check, matching `render_image_group`'s own (see that function's own doc
/// comment for why): a mermaid fence is extracted the identical way, before this file's own
/// `render_paragraph` ever sees any surrounding text, so nothing here owes the page a separator either.
/// Sets `pending_block_gap = false`/`after_math = true`/`fresh_boundary = true` on the way out, same
/// reasoning (see `render_image_group`'s own doc comment on why `fresh_boundary` specifically matters
/// too — `render_code_block`'s own leading-blank check reads it, not `pending_block_gap`), for every one
/// of its own four exits (empty-fence fallback included).
fn render_mermaid_slot(w: &mut Writer<'_>, src: &str, body_spans: &[Range<usize>]) {
    let ord = w.mermaid.ord;
    w.mermaid.ord += 1;
    let code = code_body_text(body_spans, src);
    let width = w.mermaid.width;
    if code.trim().is_empty() {
        w.lines.extend(render_mermaid_block(&code, width));
        w.pending_block_gap = false;
        w.after_math = true;
        w.fresh_boundary = true;
        return;
    }
    // `code` is `code_body_text`'s own output — one trailing `\n` short of what a fence's own raw
    // body actually reads as (see this function's own doc comment for why that gap is always
    // exactly one `\n`, never more or less, for a non-empty fence reaching this arm at all).
    // `hashed` puts it back — and, critically, is what gets passed to `w.mermaid.slot` itself, not
    // the shorter `code`: the slot closure (`app::md_render::build_decorated`'s own `mermaid_slot`)
    // computes its own cache-lookup key by hashing whatever string it is handed. Handing it `code`
    // (short by one `\n`) here, while `extraction_targets`'s own probe closure independently
    // re-added the missing `\n` before recording it, used to make the two recorded keys disagree
    // with each other and with this function's own URL — the closure (which has no such
    // independent re-add step of its own; it simply hashes whatever `code` it receives) silently
    // looked up the wrong cache key, leaving every mermaid diagram stuck on `Loading` forever.
    // Confirmed directly: `app::tests::empty_mermaid_fence_does_not_stick_on_loading`
    // synchronously renders and caches the diagram, then still shows `🖼 mermaid — loading…` on
    // rebuild, because the closure's own lookup key (`mermaid_fence_url(code)`, "graph LR\n  A -->
    // B" — no trailing `\n` at all) never matched the key the diagram was actually cached under
    // (`mermaid_fence_url(&hashed)`, "graph LR\n  A --> B\n"). Passing `hashed` here instead
    // matches the convention `extraction_targets`'s own probe closure already uses (the fence's own
    // literal bytes, trailing `\n` included), so the slot closure's own cache key and this
    // function's own URL agree; `code` (still `code_body_text`'s own trimmed output) remains what
    // gets rendered on every other exit (`render_mermaid_block(&code, ..)` below, and above for the
    // empty-body case).
    let hashed = format!("{code}\n");
    match (w.mermaid.slot)(&hashed) {
        MermaidSlot::Image { cols, rows } => {
            // Still calls production's own `mermaid_fence_url` directly — never a second,
            // hand-rolled copy of its FNV-1a computation — on the identical bytes just handed to
            // the slot closure above, so this URL and whatever cache key that closure itself
            // looked up under can never independently drift apart.
            let url = mermaid_fence_url(&hashed);
            // The one derivation of this diagram's horizontal position: `[ui] md_image_align`
            // resolved against the pane by `mermaid_diagram_col` (default `center` = the exact
            // `(width - cols) / 2` this line used before the option existed). The same value is
            // handed to `mermaid_placeholder_lines` as the caption's indent — that function no
            // longer computes a centering formula of its own — and is what `ui::preview` positions
            // the cyan focus frame from (`mermaid_focus_border_x`), so the diagram, its caption and
            // its frame cannot drift apart by a column.
            let col = mermaid_diagram_col(w.aligns.image, width, cols);
            let mut ls = mermaid_placeholder_lines(col, rows, width, w.mermaid.caption);
            // The first entry is the caption line (a Tab-focus sentinel) — the reserved rows the
            // placement overlays start right after it, matching production's own identical split.
            w.lines.push(ls.remove(0));
            let placement_line = w.lines.len() + w.heading_rule_shift;
            w.images.push(ImagePlacement {
                url,
                alt: "mermaid".into(),
                line: placement_line,
                col,
                cols,
                rows,
                fence_ord: Some(ord),
            });
            w.lines.extend(ls);
        }
        MermaidSlot::Loading => w
            .lines
            .extend(image_loading_line("mermaid", "diagram", width)),
        MermaidSlot::Text => w.lines.extend(render_mermaid_block(&code, width)),
    }
    w.pending_block_gap = false;
    w.after_math = true;
    // See `render_image_group`'s own doc comment on why `fresh_boundary` (not just `pending_block_gap`)
    // has to be set too — `render_code_block`'s own leading-blank check reads that flag specifically.
    w.fresh_boundary = true;
}

/// Renders a `Table` block into `w`, in place, by building `markdown.rs`'s own `TableCells` directly
/// from the model's `BlockKind::Table.aligns`/`.rows`, then handing it to `render_table_cells` — the
/// layout/wrapping/box-drawing phase `markdown.rs` hosts (cell links, inline styling, alignment
/// colons, CJK width all already correct there, not reimplemented). **Not** a slice of `Block.src`
/// fed to a raw-line-scanning phase the way the legacy renderer's table support used to work: each
/// cell is a `Range<usize>` directly on `src`
/// (`model::BlockKind::Table.rows`'s own doc comment — "the pipe-delimited span including its padding
/// spaces"), already free of the enclosing `Quote`'s own `>` marker regardless of nesting depth — the
/// identical reason `model::BlockKind::Html.body_spans` fixed the same defect for `Html` (see
/// `contains_unsupported`'s own doc comment for the full, confirmed-by-dump asymmetry between "a
/// whole-block raw slice" and "pulldown-cmark's own per-element ranges" this rewrite closes for
/// `Table` too). `normalize_cell` resolves the `\|` escape and trims padding off each raw range
/// before `parse_cell_segments` parses it into inline segments.
///
/// `aligns`: `Alignment::None`/`Alignment::Left` both map to `ColAlign::Left` — the identical default
/// the legacy renderer's own table-alignment parsing fell back to for a delimiter cell with no `:`
/// colon at all (`_ => ColAlign::Left`) — so this mapping is total, not a lossy narrowing of some case
/// `ColAlign` cannot represent; `pulldown_cmark::Alignment` simply has no "unspecified" *variant*
/// distinct from `None`, the same distinction `model::BlockKind::Table`'s own doc comment already
/// draws between the two types.
///
/// `header_rows`: `rows[0]` is always the header row when `rows` is non-empty (a GFM table has
/// exactly one header row by construction — `model::BlockKind::Table.rows`'s own doc comment), so
/// this is `1` whenever there is anything to draw at all, `0` for the pathological empty case
/// `render_table_cells`'s own `rows.is_empty()` guard already handles by drawing nothing.
///
/// `w.width` directly, with no further reduction here — mirrors `render_code_block`'s own identical
/// choice (see that function's own doc comment for the fuller reasoning this borrows): a quote-nested
/// `Table` renders into `render_quote`'s own already-narrowed sub-`Writer` (`width:
/// w.width.saturating_sub(2)`), one level up, so by the time this function reads `w.width` the
/// reduction has already happened — not something this function needs to redo, or can even see
/// happening, since `render_quote`'s own `>` prefix is only ever attached to a line *after* this
/// function has already finished producing it (see `Writer::emit_row_prefixed`'s own doc comment on
/// why no block-rendering function ever runs while a prefix is actually "open").
///
/// No leading-blank-line check and a "fresh boundary" exit (`pending_block_gap = false; after_math
/// = true; fresh_boundary = true`), matching `render_image_group`'s/`render_mermaid_slot`'s own (see
/// either's doc comment for why): a table is drawn as its own, self-contained band, with no separator
/// logic of any kind on either side of it.
/// `fresh_boundary`, specifically, is what keeps a fence directly following a table (no blank line
/// between them) from getting a spurious extra blank row of its own — confirmed directly:
/// `task_corpus`'s own "everything" case (a GFM table immediately followed by a fence with no blank
/// line) showed exactly this extra row before this field was set here.
fn render_table_from_model(
    w: &mut Writer<'_>,
    src: &str,
    aligns: &[Alignment],
    rows: &[Vec<Range<usize>>],
) {
    let cells: Vec<Vec<Vec<CellSeg>>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|range| {
                    let mut segs = parse_cell_segments(&normalize_cell(&src[range.clone()]));
                    prefix_link_icons(&mut segs, w.icons);
                    segs
                })
                .collect()
        })
        .collect();
    let col_aligns: Vec<ColAlign> = aligns
        .iter()
        .map(|a| match a {
            Alignment::Center => ColAlign::Center,
            Alignment::Right => ColAlign::Right,
            Alignment::None | Alignment::Left => ColAlign::Left,
        })
        .collect();
    let header_rows = if rows.is_empty() { 0 } else { 1 };
    let table = TableCells {
        rows: cells,
        header_rows,
        aligns: col_aligns,
        // A GFM cell carries no alignment or header state of its own — its column's delimiter row
        // carries both — so there is nothing to override here; see `TableCells::cell_attrs`.
        cell_attrs: Vec::new(),
    };
    let content_w = w.width as usize;
    emit_table(w, &table, content_w as u16);
    w.pending_block_gap = false;
    w.after_math = true;
    w.fresh_boundary = true;
}

/// Draws one already-built `TableCells` into `w` and records an `ImagePlacement` for every real-pixel
/// image `render_table_cells` reserved a rectangle for — the single tail both table paths
/// (`render_table_from_model` above, `render_html_table_from_model` below) share, so a cell image
/// lands at the same document row/column whichever kind of table it came from.
///
/// The row conversion is `render_image_group`'s own, verbatim: `w.lines.len()` is where this table's
/// first line is about to land, `+ w.heading_rule_shift` converts that pre-decoration index into the
/// fully-decorated one an `ImagePlacement.line` has to name (see that field's own doc comment), and
/// `CellImage.row` is the offset within the table. It is read **before** the emit loop, since every
/// row pushed moves `w.lines.len()` on.
///
/// `CellImage.col` is already measured from column 0 of the table's own lines, and a table line is
/// emitted with `emit_row` (no prefix of any kind — see `Writer::emit_row_prefixed`'s own doc comment
/// on why no block-rendering function ever runs while a prefix is open), so it is also the offset
/// from `w`'s own left edge, which is exactly what `ImagePlacement.col` means — **plus** whatever
/// indent `[ui] md_table_align` put in front of that line, which is why the shift is added to both
/// in the same breath, from the same local, below.
fn emit_table(w: &mut Writer<'_>, table: &TableCells, width: u16) {
    let base = w.lines.len() + w.heading_rule_shift;
    let (lines, cell_images, drawn_width) = render_table_cells(table, width, w.slot_of);
    // `[ui] md_table_align`. `drawn_width` is `render_table_cells`'s own answer for how wide the box
    // it just drew actually is (derived from the very `col_w` its border rules and its
    // `CellImage.col`s were built from — never re-measured here), so the indent below and the
    // `+ shift` on each cell image are two uses of *one* number. Getting those two out of step is
    // exactly how a cell's picture would end up drawn outside the box's own right border.
    // Default `left` = `0`, i.e. no indent span at all and `col` untouched — byte for byte what this
    // function did before the option existed.
    let shift = w.aligns.table.offset(width, drawn_width);
    for line in lines {
        // Inserted the same way `Writer::emit_row_prefixed` inserts a quote's own `>` marker: a
        // leading raw span, which every downstream pass (`collapse_links`, the code/task sentinel
        // scans, `md_content_anchor_line`, which skips blank spans) already tolerates.
        let line = if shift == 0 {
            line
        } else {
            let mut line = line;
            line.spans.insert(0, Span::raw(" ".repeat(shift as usize)));
            line
        };
        w.emit_row(line);
    }
    for CellImage {
        url,
        alt,
        row,
        col,
        cols,
        rows,
    } in cell_images
    {
        w.images.push(ImagePlacement {
            url,
            alt,
            line: base + row,
            col: col + shift,
            cols,
            rows,
            // Not a ```mermaid fence — the ordinal is that extraction's own ID (see the field's own
            // doc comment), and a cell image is an ordinary `![alt](url)`/`<img>`.
            fence_ord: None,
        });
    }
}

/// Draws a `BlockKind::HtmlTable` — an HTML `<table>` the model was able to read as a real grid (see
/// that variant's own doc comment for exactly which blocks qualify and what it deliberately does not
/// model) — through the **same** `TableCells`/`render_table_cells` pair a GFM `BlockKind::Table` goes
/// through (`render_table_from_model`, right above). There is no second table renderer here: column
/// measurement, width shaving, per-cell wrapping and the box drawing are all that one function's,
/// unchanged.
///
/// ## Cell text
///
/// Read through the block's own `body_spans` (`html_body_text_in`), never `&src[cell.inner]` — a
/// quote-nested table's continuation lines still carry their `>` marker in `src` but not in those
/// spans, the identical asymmetry `render_html_block_from_model` reads `body_spans` for. The raw HTML
/// that comes back goes through `html_cell_to_markdown` (which substitutes the handful of inline tags
/// that have a Markdown spelling and hands the rest to the pipeline's one tag stripper,
/// `render_html_block`) and then the identical `normalize_cell` -> `parse_cell_segments` ->
/// `prefix_link_icons` chain the GFM path uses — so a cell's `[label](url)`, `**bold**`, `` `code` ``
/// and `![alt](url)` all mean exactly what they mean in a GFM cell — including an `<img>`, which
/// `html_cell_to_markdown` rewrites to `![alt](url)` and which therefore gets a real reserved
/// rectangle and a real `ImagePlacement` (`emit_table`) on both paths alike, falling back to the
/// `🖼 alt` label on either whenever the caller has no pixels to give (`CellSeg::Image`).
///
/// ## Alignment and header rows
///
/// `aligns` (the **column** alignments) is left empty on purpose: an HTML table has no delimiter row,
/// so there is no column-level alignment to read, and `render_table_cells` reads a missing entry as
/// `ColAlign::Left` — the same default a GFM column with no `:` gets. Every alignment an HTML table
/// *does* declare is a **cell** attribute (`align="center"` on one `<td>`), which travels in
/// `cell_attrs` and wins over the column for that one cell only; a cell that declares none falls back
/// to its column, so nothing here changes what a GFM table draws.
///
/// `header_rows` is the leading run of rows whose cells are *all* `<th>` — the shape a `<thead>` (or a
/// hand-written first row of `<th>`) produces, and the only one a horizontal header rule can describe,
/// since that rule sits between whole rows. A `<th>` anywhere else — one leading column of them, a
/// stray one mid-table — cannot be expressed as a rule and is drawn as that one cell's own header
/// styling instead (`CellAttrs::header`), which is why this is a *count of rows* rather than a search
/// for every `<th>`.
///
/// Exit state (`pending_block_gap = false`, `after_math = true`, `fresh_boundary = true`) and the
/// absence of any leading-blank-line check are `render_table_from_model`'s own, for the reasons its
/// doc comment gives: a table is a self-contained band with no separator logic on either side.
fn render_html_table_from_model(
    w: &mut Writer<'_>,
    src: &str,
    body_spans: &[Range<usize>],
    rows: &[Vec<HtmlTableCell>],
) {
    let cells: Vec<Vec<Vec<CellSeg>>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| {
                    let raw = html_body_text_in(body_spans, src, &cell.inner);
                    let mut segs =
                        parse_cell_segments(&normalize_cell(&html_cell_to_markdown(&raw)));
                    prefix_link_icons(&mut segs, w.icons);
                    segs
                })
                .collect()
        })
        .collect();
    let cell_attrs: Vec<Vec<CellAttrs>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| CellAttrs {
                    align: cell.align.map(|a| match a {
                        Alignment::Center => ColAlign::Center,
                        Alignment::Right => ColAlign::Right,
                        // `html_align_attr` never reports `Alignment::None` (it maps an absent or
                        // unrecognized `align=` to `None` at the `Option` level instead), but the
                        // match is kept total rather than assumed — principle #3.
                        Alignment::None | Alignment::Left => ColAlign::Left,
                    }),
                    header: cell.header,
                })
                .collect()
        })
        .collect();
    let header_rows = rows
        .iter()
        .take_while(|r| !r.is_empty() && r.iter().all(|c| c.header))
        .count();
    let table = TableCells {
        rows: cells,
        header_rows,
        aligns: Vec::new(),
        cell_attrs,
    };
    emit_table(w, &table, w.width);
    w.pending_block_gap = false;
    w.after_math = true;
    w.fresh_boundary = true;
}

/// Renders an `Html` block's own content directly into `w`, in place — mirroring production's own
/// two-pass split line by line, rather than treating the block as one opaque blob. Reads that content
/// from `body_spans` (`model::BlockKind::Html.body_spans`), not a single `&src[block_src]` slice: a
/// block quote's own `>` marker survives on every continuation line of `Block::src` (`contains_unsupported`'s
/// own doc comment covers this in full for why a naive whole-block slice cannot be used here), but not
/// on any individual `Event::Html` range `body_spans` is built from — see that field's own doc comment
/// for the confirmed dump. This is what lets this function draw a quote-nested `Html` block correctly
/// (production silently *drops* one entirely — this was, until this field existed, an unfixed defect
/// this file reproduced too by falling the whole enclosing `Quote` back to the legacy renderer via
/// `contains_unsupported`'s `Html { .. } if in_quote` arm; that arm no longer fires for `Html` at all —
/// see its own doc comment).
///
/// ## Every standalone-image line is its own image, not just the block's first `<img>`
///
/// Production's own `split_block_parts_masked` walks the *whole document* one physical source line at a
/// time, pulling every standalone-image line (`super::extract_block_image`) out into its own
/// `BlockPart::Image` *before* `split_html_blocks` ever groups the surviving lines into an
/// `HtmlPart::Html` blob at all (see that pass's own module doc comment: image extraction happens first
/// in production's pipeline). A banner with several badge `<img>` lines — `<div align="center">` wrapping
/// three or four of them, the shape at the top of a great many crate READMEs — therefore never reaches
/// production's own `render_html_block` as one chunk in the first place: each image line is peeled off
/// individually, and only the tag-only lines left between them (`<div>`/`<a>`/`</a>`/`<br>`/`</div>`) are
/// what `render_html_block` ever tag-strips.
///
/// This file has no equivalent two-pass ordering (`Doc::parse` reads `Html`/`Image` block-vs-inline
/// structure straight off the real parser, in one walk — see the module doc comment's own "How inline
/// content is rendered" section): by the time this function runs, `Doc::parse` has already folded an
/// entire multi-image banner into a single `Html`-kind block's own raw source, so reproducing
/// production's result means re-running its own per-line split *here*, not calling
/// `super::extract_block_image` once on the whole block. (This function used to do exactly that —
/// `extract_block_image(raw)` on the whole multi-line slice finds only the *first* `<img>` tag anywhere
/// in the block, `extract_html_img`'s own scan having no notion of "stop at the first one versus find
/// them all" — and, on a match, rendered *only* that one image and returned immediately, silently
/// discarding every badge after the first and all the surrounding tag-only markup too. Real crate
/// READMEs — `static_assertions`/`raw-window-metal`/`tinytemplate`, the corpus cases this closes — hit
/// this constantly.)
///
/// Walks `raw`'s own physical lines (`split_inclusive('\n')`, so a final line with no trailing newline
/// — `html_body_text` need not produce one — is handled the same as every other). A line that is itself a
/// standalone image (`super::extract_block_image` on that one line, trimmed — the identical per-line
/// check `split_block_parts_masked` runs) first flushes whatever tag-only/text lines have accumulated so
/// far through `render_html_block` (so an image mid-banner does not end up sandwiched inside the *next*
/// image's own tag-stripped run), then renders via `render_image_group` — the same block-image renderer a
/// standalone Markdown `![alt](url)` paragraph already goes through (`try_render_paragraph_as_image`).
/// Every other line accumulates into that buffer; whatever is left over at the end (the common case —
/// most `Html` blocks carry no image line at all — and a banner's own tail, e.g. its closing `</div>`) is
/// flushed the same way. A block with no image lines at all behaves exactly as before this change: the
/// buffer never flushes early, so `render_html_block` still sees the whole block in one call.
///
/// A genuinely multi-line `<img …>` tag — its own attributes wrapped across two or more physical source
/// lines, with no single line holding a complete `<...>` — is not recognized by this per-line walk (each
/// half fails `extract_block_image` on its own). Production has the identical gap: `split_block_parts_masked`
/// is exactly as line-bound. Not a regression relative to the whole-block call this replaced, either: an
/// `<img>` tag that *is* alone on its own physical line — however many tag-only lines wrap it on
/// neighbouring lines of their own — extracts identically either way, since that one line alone already
/// satisfies `extract_block_image` on its own; a `<div>` opened and closed on separate physical lines
/// with nothing but such an `<img>` line between them still extracts exactly the same way a single-line
/// block does.
///
/// No leading-blank-line check, matching `render_table_from_model`'s own (see that function's own doc
/// comment) — `render_html_block`'s own output already ends with its own trailing blank line
/// (`render_html_block`'s own final `if !out.is_empty() { out.push(Line::from("")) }`), so a second,
/// separately-pushed one here would be a real, visible double blank row production never shows.
/// `render_image_group`'s own trailing state (`pending_block_gap = false`/`after_math = true`/
/// `fresh_boundary = true`) already matches this function's own unconditional exit state exactly (see
/// both fields' own doc comments below), so interleaving it with `render_html_block` flushes leaves
/// nothing to reconcile between calls.
/// `pending_block_gap = false`/`fresh_boundary = true` on the way out means whatever renders *next* does
/// not add a leading blank line of its own either (`render_code_block`'s own leading-blank check
/// reads `fresh_boundary` specifically, not `pending_block_gap` — see `render_table_from_model`'s own doc
/// comment for the concrete case that caught this), matching that same already-baked-in trailing
/// blank exactly.
fn render_html_block_from_model(w: &mut Writer<'_>, src: &str, body_spans: &[Range<usize>]) {
    let raw = html_body_text(body_spans, src);
    let mut buf = String::new();
    let mut pending_images: Vec<(String, String)> = Vec::new();
    let flush_text = |w: &mut Writer<'_>, buf: &mut String| {
        if !buf.is_empty() {
            for l in render_html_block(buf) {
                w.emit_row(l);
            }
            buf.clear();
        }
    };
    // `render_image_group` (see its own doc comment) packs a *run* of images onto shared rows —
    // consecutive `<img>` lines with no intervening tag-only/text line between them (a plain
    // `<img/><img/><img/>` banner, with no per-badge `<a>` wrapper) now share a row exactly like the
    // Markdown badge-row idiom does. An `<img>` line individually wrapped in its own `<a>…</a>` (the
    // common real-README shape, confirmed against `static_assertions`/`raw-window-metal`/
    // `tinytemplate` by the sweep `render_html_block_from_model`'s own doc comment above cites) still
    // renders one image per group — the `</a>` line right after it is *not* an image line, so it
    // flushes that group of one before the next `<img>` line even starts a new one — reproducing this
    // function's own pre-existing per-image output exactly.
    for line in raw.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        match super::extract_block_image(bare) {
            Some((alt, url)) => {
                flush_text(w, &mut buf);
                pending_images.push((alt, url));
            }
            None => {
                render_image_group(w, &pending_images);
                pending_images.clear();
                buf.push_str(line);
            }
        }
    }
    render_image_group(w, &pending_images);
    flush_text(w, &mut buf);
    w.pending_block_gap = false;
    w.after_math = true;
    w.fresh_boundary = true;
}

/// The generic block dispatcher used for every position in this file's block tree that is **not** a
/// `List`'s own item sequence (which `render_list` walks itself, since each one needs a marker built
/// first — see `render_item`) or an item's own *first* child (which `render_item` special-cases for
/// exactly the tight/loose distinction just described): a `Quote`'s own body, and every child of a
/// list item after the first — the top-level dispatch for every block kind this file covers.
///
/// A quote-nested `Table`/`Html` reaches this function too, exactly like a `List`/`Details`-nested
/// one — `contains_unsupported` no longer screens either out at any nesting depth (see that
/// function's own doc comment) — rendered by the identical
/// `render_table_from_model`/`render_html_block_from_model` pair `render_doc`'s own top-level
/// dispatch calls, both of which read a block's own `model`-derived ranges rather than a naive
/// `Block.src` slice, so neither cares whether `ctx`'s own enclosing chain happens to include a
/// `Quote`. `ListItem` never legitimately reaches this function — it only ever exists as a `List`'s
/// own child (see `model::BlockKind::ListItem`'s doc
/// comment), and `render_list` unwraps every one of those itself. Matched defensively (silently
/// rendering nothing further) rather than assumed unreachable all the same, so a bug elsewhere
/// degrades to "this one block goes missing" instead of a panic — principle #3 (`CLAUDE.md`).
/// `CodeBlock`, `Quote { alert: Some(_), .. }`, and `Details` *do* legitimately reach here
/// (`contains_unsupported` no longer screens any of the three out — see that function's own doc
/// comment) — `render_code_block_dispatch`/`render_alert_from_model`/`render_details_from_model` draw
/// them.
///
/// The `Paragraph` arm is not a single, unconditional dispatch the way it looks from `render_doc`'s
/// own top-level loop (where a `Paragraph` is *always* real — see `is_real_paragraph`'s own doc
/// comment on why that only ever fails for a list item's own content): a `Paragraph` reached *through*
/// this function can be **bare** — `is_real_paragraph` decides which, per call, and routes to
/// `render_paragraph_dispatch` (a real one — unchanged) or `render_bare_paragraph` (a bare one — see
/// its own doc comment for why treating it as real would be wrong, and for the concrete shape that
/// exercises this: a tight list item's own *later* child, once a `CodeBlock`, `Heading`, or
/// `ThematicBreak` interrupts one bare paragraph with another lazily continuing right after it, no
/// blank line either side).
///
/// `ctx` is simply threaded through to whichever of `render_paragraph_dispatch`/
/// `render_bare_paragraph`/`render_list`/`render_details_from_model` this block turns out to need it
/// — see `BlockCtx`'s own doc comment for what each of its two fields means, and `render_quote`'s own
/// call site below (and `render_alert_from_model`/`render_details_from_model`'s own bodies) for where
/// a caller does *not* pass its own value through unchanged.
fn render_block(block: &Block, w: &mut Writer<'_>, doc: &Doc<'_>, src: &str, ctx: BlockCtx) {
    match &block.kind {
        BlockKind::Heading {
            level,
            inline,
            id,
            classes,
            attrs,
        } => {
            let meta = HeadingMeta {
                id: id.clone(),
                classes: classes.clone(),
                attrs: attrs.clone(),
            };
            render_heading(w, doc, src, *level, inline.clone(), &meta);
        }
        BlockKind::Paragraph { inline } => {
            if is_real_paragraph(src, block) {
                render_paragraph_dispatch(
                    w,
                    doc,
                    src,
                    inline.clone(),
                    ctx.math_here,
                    ctx.extract_here,
                    block.src.start,
                );
            } else {
                render_bare_paragraph(
                    w,
                    doc,
                    src,
                    inline.clone(),
                    ctx.math_here,
                    ctx.extract_here,
                    block.src.start,
                );
            }
        }
        BlockKind::ThematicBreak => render_rule(w),
        BlockKind::List { start, .. } => render_list(w, doc, src, *start, &block.children, ctx),
        BlockKind::Quote { alert: None, .. } => render_quote(w, doc, src, &block.children, ctx),
        // A GitHub-alert-headed quote with `w.alerts = false` — see the identical pair of arms in
        // `render_doc`'s own top-level dispatch, above, for the full reasoning (`Writer.alerts`'s own
        // doc comment for why this is read off `w` rather than a fourth `BlockCtx` field, and why this
        // is a separate arm from `alert: None` rather than one merged guard — a match guard does not
        // count towards exhaustiveness).
        BlockKind::Quote { alert: Some(_), .. } if !w.alerts => {
            render_quote(w, doc, src, &block.children, ctx)
        }
        BlockKind::Quote {
            alert: Some(kind),
            alert_title,
        } => render_alert_from_model(
            w,
            doc,
            src,
            *kind,
            alert_title,
            &block.children,
            block.src.start,
        ),
        BlockKind::CodeBlock {
            lang, body_spans, ..
        } => render_code_block_dispatch(w, src, lang.as_deref(), body_spans, ctx.extract_here),
        BlockKind::Details {
            open_attr,
            summary,
            glued_body,
        } => render_details_from_model(
            w,
            doc,
            src,
            *open_attr,
            summary,
            &block.children,
            glued_body.clone(),
            ctx,
        ),
        BlockKind::Table { aligns, rows } => render_table_from_model(w, src, aligns, rows),
        BlockKind::Html { body_spans, .. } => render_html_block_from_model(w, src, body_spans),
        BlockKind::HtmlTable { body_spans, rows } => {
            render_html_table_from_model(w, src, body_spans, rows)
        }
        // Never reachable — see the doc comment above.
        BlockKind::ListItem { .. } => {}
    }
}

/// Renders a `List`'s own items into `w`, in place — opens the list's own running item counter as a
/// local variable, calls `render_item` once per item (threading that counter down by `&mut`
/// reference, along with a `ctx` one level deeper than the one this function itself received), then
/// lets the counter drop when the loop ends; there is no push/pop pair of any kind, on `w` or
/// otherwise, because nothing outside this one call needs to see this list's own counter at all (see
/// `BlockCtx::list_depth`'s own doc comment for the fuller reasoning on why depth and the running
/// counter ended up split across two different kinds of state this way). `start` is
/// `BlockKind::List.start` verbatim (`None` for an unordered list, `Some(n)` for an ordered one's own
/// first number) — the counter's own initial value, and the exact value it keeps reading back as
/// `None` for as long as this list stays unordered. Not gated on `BlockKind::List.ordered` — that
/// field is redundant with `start.is_some()` by construction (see `model::BlockKind::List`'s own doc
/// comment on how `parse_container` computes it), so there is nothing `ordered` could tell this
/// function that `start` does not already say.
///
/// The leading-blank-line check reads `ctx.list_depth` — the value *this* call received, before the
/// `+1` below — rather than `list_ctx`'s: `0` means no list at all is currently open around this
/// position (this is a top-level list, or the first list found inside an isolated sub-`Writer`'s own
/// children), the identical question the old `w.list_counters.is_empty()` check answered by reading
/// `Writer`-owned state instead of a parameter.
///
/// The exit's own `pending_block_gap = true` is *conditional* on `!w.after_math` — unlike every other
/// container-closing site in this file, this one genuinely can run with the flag still set: the very
/// last item this loop rendered may have ended in a lifted math expression with nothing textual after
/// it (see `render_paragraph_math`'s own doc comment for why that is exactly the shape a whole `List`
/// can legitimately end on, and `Writer`'s own doc comment on `after_math` for why unconditionally
/// re-forcing `pending_block_gap` back to `true` here would reintroduce the very separator line the
/// lift's own reset exists to suppress).
fn render_list(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    start: Option<u64>,
    items: &[Block],
    ctx: BlockCtx,
) {
    if ctx.list_depth == 0 && w.pending_block_gap {
        w.emit_blank_row();
    }
    let list_ctx = BlockCtx {
        list_depth: ctx.list_depth + 1,
        ..ctx
    };
    let mut counter = start;
    for item in items {
        if let BlockKind::ListItem { task } = &item.kind {
            render_item(
                w,
                doc,
                src,
                *task,
                &item.children,
                list_ctx,
                item.src.start,
                &mut counter,
            );
        }
        // A `List`'s own children are always `ListItem`s (see `model::parse_container`'s own
        // `Tag::List` arm) — the `if let` above is not a lossy filter of some other shape, only the
        // same defensive-match convention this whole file follows rather than an unchecked `panic!`.
    }
    if !w.after_math {
        w.pending_block_gap = true;
    }
}

/// Renders one list item into `w`, in place — the marker, then, where the item has one, the task
/// marker, then, for the item's own remaining content, whichever of
/// `render_paragraph`/`render_block`/`walk_inline` its own first child calls for (see below).
///
/// ## The tight/loose distinction, and why it decides everything here
///
/// Pulldown-cmark never reports a `Start(Paragraph)` event for a **tight** item's own content at all —
/// it hands the item's inline events directly, right after `Start(Item)`; `model`
/// still gives that content somewhere to live, a *synthetic* `Paragraph` `collect_stray_inline_run`
/// builds on the fly (see that function's own doc comment), but there was never a real `Tag::Paragraph`
/// for it. A **loose** item's own first paragraph, by contrast, is real — pulldown-cmark reports an
/// ordinary `Start(Paragraph)`/`End(Paragraph)` pair around it, indistinguishable in `Doc.events` from
/// any other paragraph in the document. `is_real_paragraph` reads `src` to tell the two apart
/// (see `render_doc`'s own doc comment for exactly which byte settles it), because the two cases
/// produce genuinely different output:
///
/// * **Tight**: no blank-line push happens at all — the item's inline content continues directly on
///   the very same line the marker row this function opened just started (the marker's own), and (if
///   the item has one) a task marker lands on that same line too, spliced into the `"- "` bullet span
///   itself.
/// * **Loose**: `render_item` pushes a **second**, fresh, empty row right after the marker's own
///   (`loose_first` below) — the identical two-part convention `render_paragraph` applies for an
///   ordinary top-level paragraph, applied here directly rather than by calling that function — before
///   either the task marker (if the item has one) or the paragraph's own first line of text is
///   written. The net effect: a loose item's own bullet/number sits alone, on its own row, with the
///   marker and text one row below it — a deliberate property of this layout, not a bug to "fix".
///
/// The task marker, when present, is written **after** that decision is settled either way (mirroring
/// pulldown-cmark's own event order: a loose item's `Event::TaskListMarker` is reported as the item's
/// own first *paragraph*'s first inline event, strictly after `Start(Paragraph)` — see
/// `model::parse_item_task_and_children`'s own doc comment) — so it always lands on whichever line is
/// current at that point: the marker's own row for a tight item, or the freshly-opened blank row for
/// a loose one.
///
/// An item whose own first child is not a `Paragraph` at all (real or synthetic) — a bare bullet
/// immediately followed by a nested `List`/`Quote` with no inline text of its own, e.g. `- - nested`
/// — falls through to the generic dispatcher (`render_block`) for that first child too, the same as
/// every child after it; `is_real_paragraph` reports `false` for a non-`Paragraph` block, so no
/// spurious blank line gets inserted ahead of it.
///
/// `math_here` decides which of `walk_inline`/`walk_inline_math` drives the item's own first child's
/// content when that child *is* a `Paragraph` — see `render_doc`'s own doc comment for where this
/// value comes from and `walk_inline_math`'s for why both walks share the exact same event-kind
/// dispatch, math-scanning aside. `w.pending_block_gap = loose_first && !w.after_math` folds `after_math`
/// into the tight/loose decision the surrounding comment already documents: for a **tight** item
/// (`loose_first == false`) the right-hand side of the `&&` never matters, so behavior is unchanged
/// from before math support existed; for a **loose** one, this is exactly `render_paragraph_math`'s
/// own "only a paragraph that does *not* end in trailing math gets the normal
/// `end_paragraph()`-equivalent `true`" rule, applied here since a loose item's own first child is a
/// *real* paragraph in every sense that matters to that rule — it simply never goes through
/// `render_paragraph`/`render_paragraph_math` themselves (see this function's own doc comment on the
/// tight/loose distinction for why).
///
/// `first.src.start` (below, at the `math_here` call site) closes `walk_inline_math`'s own
/// backslash-math-as-first-event gap-detection bug for a **loose** item's own first child the same way
/// it does for `render_paragraph_math` (see that function's own doc comment, and `walk_inline_math`'s)
/// — a loose item's first child is a *real*, pulldown-cmark-wrapped `Paragraph`, so its own `Block.src`
/// carries the same wider span a top-level paragraph's does. A **tight** item's synthetic first child
/// does not (`model::collect_stray_inline_run`'s own doc comment: no wrapping tag to have recorded a
/// wider span in the first place), so the gap this closes stays `""` there regardless — safe, not a
/// regression, just not a fix for that one narrower shape (see `walk_inline_math`'s own doc comment for
/// the identical caveat).
///
/// This function does **not**, however, reuse `render_paragraph_math`'s *other* fix — the deferred,
/// `pending_para_start`-based leading-blank-line convention (see that field's own doc comment) — for
/// its own `loose_first` branch below, on purpose: a loose item's own task marker (`w.write_task_marker`,
/// called unconditionally right after `loose_first`'s own eager blank-line push, task or no task) needs
/// that row to already exist the moment it runs, *regardless* of whether the item's first child's own
/// first rendered content later turns out to be math — deferring the push here would either strand the
/// marker on the wrong (marker's own) row when a task is present, or require re-deriving "does this
/// item have a task" a second time inside the pending-slot machinery for no benefit, since a loose
/// list item beginning directly with a lifted math expression is not a shape any corpus case exercises
/// (nor one this task's own scope — closing three top-level math mismatches against the legacy
/// renderer — asked for). Left as the pre-existing eager push, unchanged, rather than risked on an
/// unverified guess at what the legacy renderer would even do there.
#[allow(clippy::too_many_arguments)] // one call site (render_list); each argument its own distinct piece of state — `counter` is the enclosing list's own running item number, threaded down by `&mut` reference rather than folded into `ctx` because (unlike `ctx`'s own fields) it mutates within this one list's lifetime — see `BlockCtx::list_depth`'s own doc comment
fn render_item(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    task: Option<Task>,
    children: &[Block],
    ctx: BlockCtx,
    item_src_start: usize,
    counter: &mut Option<u64>,
) {
    // start_item(): a fresh physical line, unconditionally, carrying the item's own marker.
    w.emit_row(Line::default());
    push_item_marker(w, ctx.list_depth, counter);
    w.pending_block_gap = false;

    let loose_first = children.first().is_some_and(|b| is_real_paragraph(src, b));
    // pulldown-cmark never reports a *custom* state (`ui.md_task_states`, e.g. `/`) as a task marker
    // event at all — `[/] foo` parses as an item with no `task`, its literal text becoming ordinary
    // inline content (see `model::Task`'s own doc comment and
    // `custom_task_state_char_is_not_recognized_as_a_task_by_pulldown_cmark`). Detected here the
    // identical way `decorate_extras`'s own `replace_task_checkbox` detects one on a *rendered*
    // line's text (`task_prefix_state`) — but applied to this item's own source range instead:
    // `item_src_start` is always the item's own bullet character (`- `/`* `/`+ `), regardless of
    // nesting — a list item's own marker is never repeated on a continuation line the way a block
    // quote's `>` is, and never carries a parent list's own indentation either (both confirmed
    // directly against the model: `item.src` for a nested item, or one inside a block quote, starts
    // exactly at its own bullet byte, indentation and any `>` prefix already excluded — see this
    // file's own module doc comment's citation of `model.rs`'s equivalent tests) — so
    // `task_prefix_state`'s own "first byte must be a bullet" check is safe to run directly against
    // `src[item_src_start..]` with no line re-slicing of any kind on this file's own part.
    //
    // Only the item's own **first physical line** is a candidate (a checkbox's own trailing `]` must
    // be followed by whitespace or end-of-line, never a literal `\n` — the "any text after it" case
    // `task_prefix_state`'s own doc comment covers is about the *rest of that one line*, not the
    // item's own later, unrelated content), so the search is bounded to it explicitly rather than
    // handed the item's own, possibly multi-line, `Block::src` whole. Computed once, *before*
    // `task`'s own match below, both because it decides `checkbox_shares_the_marker_row`
    // (`loose_first`'s own gap has to be settled before anything writes to `w.lines`) and because
    // the `None` arm below reuses this exact value instead of re-deriving it a second time.
    let custom_task = if task.is_none() {
        let first_line_end = src[item_src_start..]
            .find('\n')
            .map_or(src.len(), |off| item_src_start + off);
        let first_line = &src[item_src_start..first_line_end];
        task_prefix_state(first_line, w.tasks)
    } else {
        None
    };
    // A task item's own checkbox always collapses onto the marker's own row — tight or loose list
    // alike — rather than following `loose_first`'s usual "marker alone, text one row below" shape.
    //
    // Root cause this closes (found live, comparing this renderer's output for `"- [ ] one\n\n- [ ]
    // two\n"` against the pre-refactor production build): real pulldown-cmark reports a *loose*
    // item's own `Event::TaskListMarker` strictly *after* `Start(Paragraph)` (see
    // `model::parse_item_task_and_children`'s own doc comment) — real tui-markdown's own task-marker
    // method therefore landed the checkbox on the fresh, *empty* row its own two-part blank-line push
    // just opened, not the marker's own row, via
    // an unconditional `line.spans.insert(1, ..)` with **no bounds check at all** — which panics on
    // exactly that empty row (0 spans, `insert(1, ..)` needs at least 1). The legacy renderer's own
    // panic-isolation caught that crash and re-split the document at the nearest blank line, and a
    // `"- [ ] one\n"` fragment on its own — a single item, no blank line *inside* it — reparsed as a
    // **tight** one-item list, which is what actually reached the screen: the legacy renderer's
    // "tight-looking" task list was an accidental side effect of crash recovery, not a deliberate
    // rendering choice (confirmed directly: a *custom*-state loose item right after a real-state one
    // — no `Event::TaskListMarker` involved for that item at all — still showed up "tight-looking"
    // there, purely because the crash-recovery split happened to isolate it onto its own,
    // trivially-tight single-item fragment too).
    //
    // This file's own `Writer::write_task_marker` fixed the underlying panic (`spans.len().min(1)`
    // instead of a bare `1`) but, before this fix, still *placed* the marker exactly where real
    // tui-markdown's crashing version would have — the empty row — faithfully reproducing the
    // *position* a bug in the reference implementation would have produced, while no longer
    // reproducing the crash that used to make that position invisible. Confirmed directly: without
    // this change, `"- [ ] one\n\n- [ ] two\n"` rendered `["- ", "[ ] one", "- ", "[ ] two"]` — the
    // marker and the raw, never-decorated `"[ ] "` text stranded on separate lines, so
    // `decorate_extras`'s own `replace_task_checkbox` (which requires the *same* line to both start
    // with the bullet and hold the checkbox — `task_prefix_state`'s own "first byte must be a
    // bullet" contract) never recognized either line as a checkbox at all, leaving the literal
    // brackets on screen and no `task_marker_style()` sentinel for Tab-focus/`Space` to key off.
    //
    // The model already carries `task: Option<Task>` as a plain field on `ListItem`, decoupled from
    // pulldown-cmark's own event *order* — unlike real tui-markdown (and this file's own inline
    // walk, which only ever sees a raw `Event` stream), `render_item` is free to draw the checkbox
    // wherever makes sense on screen, with no obligation to reproduce a upstream ordering quirk that
    // exists only because of *how* the reference implementation happens to consume its own event
    // stream. Suppressing the loose-only blank-line gap for either a real (`task.is_some()`) or a
    // custom (`custom_task.is_some()`) checkbox is what makes that row line up with the marker's
    // own: for a real task, via the same splice `Writer::write_task_marker`'s own
    // `content.ends_with("- ")` fast path already performs for a **tight** item, reused here
    // unchanged; for a custom one, there is no dedicated splice call at all — the marker's own row
    // simply stays the *current* line, so the item's own ordinary paragraph walk below (which
    // reads the literal `"[/] foo"` text as plain inline content — the whole reason no splice is
    // needed) appends directly onto it, the identical mechanism a **tight** item's own custom-state
    // checkbox already relies on. A loose item with no task at all keeps its own separate row
    // exactly as before (this is not a general "loose items always render tight" change).
    let checkbox_shares_the_marker_row = task.is_some() || custom_task.is_some();
    if loose_first && !checkbox_shares_the_marker_row {
        w.emit_blank_row();
        w.pending_block_gap = false;
    }
    match task {
        Some(t) => {
            w.write_task_marker(matches!(t.state, 'x' | 'X'));
            // A GFM state (a space, `x`, or `X` — the only three pulldown-cmark itself ever reports
            // via `Event::TaskListMarker`; see `model::Task`'s own doc comment) — `state_at` is
            // already an absolute byte offset into `src`, read straight off the model, no detection
            // needed.
            //
            // Only pushed when *this* item's own list is unordered (`counter.is_none()`) — the
            // exact same "is this list's own marker a bullet" test `push_item_marker`, just above,
            // already used (via this same `counter` reference) to decide between writing a literal
            // `"- "` or a `"N. "` marker span — `counter` only ever holds `None` for a list that
            // started `None`, never flips to `Some` partway through (`push_item_marker`'s own `None`
            // arm never touches it), so reading it back here, after that call already ran, still
            // answers the identical question. `decorate_extras`'s own `replace_task_checkbox`
            // (downstream, over the *rendered* text) only ever recognizes a checkbox whose line, after
            // trimming, starts with a bullet byte (`task_prefix_state`'s own "first byte must be
            // `-`/`*`/`+`" contract) — an ordered item's own `"N. "` marker line starts with a digit,
            // so real GFM task syntax on an *ordered* list item (pulldown-cmark reports
            // `Event::TaskListMarker` for either kind — see `model::Task`'s own doc comment) still
            // writes the literal `"[ ] "`/`"[x] "` text here (unchanged — matching production, which
            // never decorates an ordered item's own checkbox either) but is never turned into a
            // sentinel span, so `build_md_items`'s "Nth sentinel on screen == task_marks[N]" ordinal
            // pairing (see its own doc comment) must never count that entry at all — pushing it
            // regardless (this file's pre-existing bug, found live: a document mixing an ordered
            // task list ahead of an unordered one toggled the *ordered* item's own, invisible,
            // never-decorated checkbox when `Space` was pressed on the *visible*, unordered one)
            // silently shifted every later unordered task's own ordinal down by one per ordered task
            // ahead of it in the document, pairing the focused sentinel with a completely different
            // checkbox's byte offset instead of degrading to `None` the way a genuine count mismatch
            // does (principle #3) — a wrong answer is worse than a safe refusal.
            if counter.is_none() {
                w.task_marks.push((t.state, t.state_at));
            }
        }
        None => {
            if let Some((state, off)) = custom_task {
                // No equivalent gate needed here: `custom_task` (above) already requires
                // `src[item_src_start..]`'s own first byte to be a bullet char (mirroring
                // `task_prefix_state`'s identical check on the *rendered* line) — an ordered item's
                // own source starts with a digit, so `custom_task` is already `None` for one, with
                // nothing left for a second gate to filter out.
                w.task_marks.push((state, item_src_start + off));
            }
        }
    }

    let mut rest = children;
    if let Some(first) = children.first() {
        if let BlockKind::Paragraph { inline } = &first.kind {
            // A first-child paragraph that is nothing but standalone block image(s) — `- ![alt](x)`,
            // the common "one image per list item" idiom — must never take the marker-row splice
            // below. That splice's whole premise is "continue on whichever line is already open"; an
            // image is a multi-row block (`render_image_group`/`flush_row_group` reserve and draw a
            // whole band of rows), fundamentally incompatible with continuing text onto a single
            // shared row. Route it through the exact same dispatcher every *later* sibling paragraph
            // in this same item already goes through instead (`render_block`'s own `Paragraph` arm,
            // which sends a real paragraph to `render_paragraph_dispatch` and a tight item's bare one
            // to `render_bare_paragraph` — both try `try_render_paragraph_as_image` before anything
            // else). Confirmed against `tight_list_bare_paragraph_that_is_only_an_image_returns_early`
            // (a *later* bare-paragraph sibling that is only an image, already covered): that path
            // draws the marker's own row first (unchanged — nothing here touches `push_item_marker`'s
            // own call, above), then the image band on the row(s) directly below it, exactly what this
            // arm now reuses. Before this fix, neither `try_render_paragraph_as_image` nor
            // `paragraph_trailing_images`/`paragraph_leading_images` was ever reachable from an item's
            // own *first* child at all — `walk_inline`, below, renders an `Event::Start(Image)` as
            // nothing but its own literal alt text (see `Writer`'s inline walk), so a first-child-only
            // image silently degraded to bare alt text while every later paragraph in the identical
            // list rendered the real thing — the exact "1 item 1 image, only the first fails"
            // inconsistency this arm closes. Gated on `ctx.extract_here` for the same reason every
            // other image-extraction call site in this file is (see `BlockCtx.extract_here`'s own doc
            // comment) — mirrors `try_render_paragraph_as_image`'s own eligibility gate exactly, so
            // this decision and that function's are never out of sync with each other.
            if ctx.extract_here && paragraph_as_block_images(doc, src, inline).is_some() {
                render_block(first, w, doc, src, ctx);
            } else {
                // Tight or loose, an ordinary (non-image-only) first paragraph's content continues
                // directly on whichever line is already open — the marker's own (tight), or the blank
                // one `loose_first` just opened above (loose) — never a *second* blank-line push of
                // its own; that is exactly what distinguishes this from `render_paragraph`, which this
                // call deliberately bypasses for the item's own first child.
                if ctx.math_here {
                    walk_inline_math(
                        &mut events_iter_ranged(&doc.events[inline.clone()]),
                        w,
                        src,
                        first.src.start,
                    );
                } else {
                    walk_inline(&mut events_iter(&doc.events[inline.clone()]), w, None);
                }
                // `loose_first == true`: this was a *real* paragraph — `end_paragraph()`'s own
                // unconditional `pending_block_gap = true` applies, exactly as it would via
                // `render_paragraph`, *unless* its own trailing content was a math lift (`w.after_math`
                // — see this function's own doc comment). `loose_first == false` (tight): there was no
                // wrapping paragraph tag at all, so no `end_paragraph()` ever ran — `walk_inline`'s own
                // inline-only calls (`Writer::text` chief among them; see its own doc comment) leave
                // `pending_block_gap` exactly as `false` as `start_item` set it. Setting this unconditionally
                // to `true` here — an earlier version of this function did — inserted a spurious blank
                // line before any `Quote`/`Heading`/`ThematicBreak` directly interrupting a tight item's
                // own paragraph with no blank line in the source; see `list_corpus`'s own "block quote
                // can interrupt a tight item's own paragraph with no blank line" case, which is exactly
                // what caught this. `checkbox_shares_the_marker_row`: whenever a task collapsed this
                // item onto the marker's own row (see that binding's own doc comment, above), no
                // `start_paragraph()`-equivalent blank line ever opened for it either — behaviorally
                // tight, regardless of `loose_first`'s own value — so the exit state has to match.
                w.pending_block_gap =
                    loose_first && !checkbox_shares_the_marker_row && !w.after_math;
            }
        } else {
            render_block(first, w, doc, src, ctx);
        }
        rest = &children[1..];
    }
    for child in rest {
        render_block(child, w, doc, src, ctx);
    }
}

/// Whether `block` is a **real** `Paragraph` — pulldown-cmark's own `Tag::Paragraph`, reported for a
/// loose list item's own content, or (always, regardless of tight/loose) for one reached any other
/// way this file's own `Doc::parse` can produce a `Paragraph` (top level, or a `Quote`'s own body —
/// see `render_block`'s own doc comment for why the reverse — "reached through `render_block` but
/// still bare" — can only ever happen for a list item's own content) — as opposed to the *synthetic*
/// one `collect_stray_inline_run` builds for a **tight** item's (see `render_item`'s own doc comment
/// for what that distinction changes for an item's own *first* child, and `render_bare_paragraph`'s
/// for a *later* one). Despite the name (kept from this function's original, narrower use —
/// `render_item`'s own first-child special case, still its only *other* caller), nothing about this
/// check is first-child-specific: it purely inspects `block`'s own byte range, a property of *how
/// pulldown-cmark represented that one block*, independent of its position among its own siblings —
/// which is exactly what lets `render_block`'s own `Paragraph` arm reuse it unchanged for *every*
/// position, not just an item's first child.
///
/// The two shapes are told apart by `Block.src`'s own trailing byte in `src`: a real `Paragraph`'s
/// range always runs through its own trailing newline (`"a\n"`), the way pulldown-cmark reports every
/// `Tag::Paragraph`'s range; the synthetic one never does (`"a"`) — there is no wrapping tag for
/// pulldown-cmark to have assigned that wider range to in the first place, only the leaf events
/// themselves, whose own ranges stop at the run's last one. This is not a byte-range heuristic this
/// file invented: it is the model's own, explicitly documented and unit-tested contract for reading
/// the distinction back (see `model::collect_stray_inline_run`'s own doc comment, and
/// `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one`, the test that pins
/// exactly this) — confirmed to hold for a **loose** list's own *later* paragraphs too, not merely its
/// first (`Doc::parse` reports a trailing `"\n"` on *every* real paragraph in a loose list, even one
/// with no blank line directly in front of it, e.g. right after a fence's own closing marker — the
/// CommonMark rule is "loose or tight" per *list*, not per individual blank line).
fn is_real_paragraph(src: &str, block: &Block) -> bool {
    matches!(&block.kind, BlockKind::Paragraph { .. }) && src[block.src.clone()].ends_with('\n')
}

/// Renders a **bare** (synthetic — `is_real_paragraph` false) `Paragraph`'s own inline content
/// directly into `w`, with no `start_paragraph`/`end_paragraph`-equivalent bookkeeping of any kind —
/// used both for a **tight** list item's own first child (`render_item`'s own first-child special
/// case calls `walk_inline`/`walk_inline_math` itself, inline, for reasons specific to that position —
/// see its own doc comment) and for any *later* sibling in the same item that turns out, on inspection
/// (`is_real_paragraph`), to be equally bare (`render_block`'s own `Paragraph` arm, this function's
/// only caller). A bare paragraph can appear anywhere in a tight item's own child list, not merely
/// first, once the item contains an interrupting non-paragraph sibling (a `CodeBlock`, a `Heading`, a
/// `ThematicBreak`, a nested `List`/`Quote`) with no blank line around it — CommonMark's own lazy
/// continuation only absorbs plain text back into an *already-open* paragraph (`render_quote`'s own
/// call site above already handles that shape correctly, since it never creates a second sibling
/// block at all — confirmed directly: `"- a\n  > quoted\n  after\n"` parses `"after"` as part of the
/// quote's own single paragraph, joined by a soft break, not a new block), but a fence, heading, or
/// thematic break always *starts a new block*, so a lazily-continued line right after one becomes a
/// genuinely separate sibling `Paragraph` — real if the enclosing list is loose, bare if it is tight.
///
/// The one piece of bookkeeping this function *does* still need — even though there is no wrapping
/// paragraph-start/paragraph-end pair to supply it — is consuming a *pending* `w.pending_block_gap`
/// left by whatever sibling came immediately before this one, so this content starts on its own fresh
/// row rather than gluing onto that sibling's own last row. This reproduces the effect of a leading-
/// row guard (`if self.pending_block_gap { emit_row(default); pending_block_gap = false }`), but
/// applied **once**, up front, here, rather than repeated inside every individual `Writer` method the
/// way tui-markdown's own reference implementation repeated an equivalent guard inside each of its own
/// event-handling methods.
///
/// ## A deliberate, documented departure from tui-markdown's own (buggy) behavior
///
/// Confirmed directly against real tui-markdown: only its own `text` method carried that guard — its
/// `code`/`soft_break`/`hard_break` counterparts never consulted the equivalent state at all, so when a bare paragraph's
/// own *first* inline event happens to be something other than `Event::Text` (most commonly
/// `Event::Code`, an inline code span — e.g. `` `inline` after `` immediately following a list-nested
/// fence's own closing marker with no blank line — `code_corpus`'s own "KNOWN FAILURE: list item fence
/// glued to a following inline-code paragraph" cases), real tui-markdown glued that content directly
/// onto whatever row was currently open. The legacy renderer's own consequence of this — its own
/// "last row must be fence characters only" check failing, dropping that code block's header entirely,
/// cancelling `y c` for the *whole document* (the count guard is document-wide) — is exactly the
/// defect those corpus cases pin, and exactly the shape that motivated this refactor's own "single
/// source of truth for block structure" premise (see the parent module's own doc comment). This
/// function does **not** reproduce tui-markdown's own inconsistency here: the guard runs once, up
/// front, regardless of which kind of event the paragraph's own content happens to start with, so a
/// bare paragraph *always* lands on its own fresh row when one is owed — never glued onto a preceding
/// sibling's own last row. This is a deliberate improvement, not an oversight: this file draws a
/// `CodeBlock`'s own header directly from the model (`render_code_block`) and so can never lose one to
/// begin with, which is exactly what makes fixing the *visual* glue here safe — there is no downstream
/// count-guard left to desynchronize the way the legacy renderer's own was. This is a documented,
/// intentional improvement over the legacy renderer's own behavior, not a gap.
///
/// ## Not a `CodeBlock`-specific fix
///
/// Confirmed, before any `CodeBlock` support existed, that `render_block`'s *previous* unconditional
/// "every `Paragraph` is real" dispatch already mishandled this shape for the two block kinds this
/// file already covered — `"- a\n  # heading\n  after\n"` and `"- a\n  ---\n  after\n"` (a tight
/// item's own paragraph interrupted by a `Heading`/`ThematicBreak`, with a bare paragraph lazily
/// continuing right after) — both rendered an extra, spurious blank line before `"after"` that real
/// production never shows (`render_paragraph`'s own unconditional *second* blank-line push, wrong for
/// a bare paragraph, right only for a real one). Nothing in the existing parity corpus happened to
/// exercise a tight item with a *third*, later child at all, so this stayed a latent bug — undetected,
/// not previously absent — until `CodeBlock` support made it observable via the very corpus cases the
/// task this function was added for asks to fix. Fixing it generically here (rather than narrowly,
/// only for a paragraph that happens to follow a `CodeBlock`) closes the `Heading`/`ThematicBreak`
/// cases too, for free.
///
/// `extract_here` (added alongside `math_here`, not folded into it — see `BlockCtx`'s own doc
/// comment): whether this bare paragraph's own raw line, having no container marker on it either (a
/// bare paragraph is, by definition, never a list item's own *first* child — see this function's own
/// doc comment above), is eligible for standalone block-image extraction the same way any other
/// non-first paragraph is.
fn render_bare_paragraph(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    inline: Range<usize>,
    math_here: bool,
    extract_here: bool,
    block_start: usize,
) {
    if w.pending_block_gap {
        w.emit_blank_row();
        w.pending_block_gap = false;
    }
    if try_render_paragraph_as_image(w, doc, src, &inline, extract_here) {
        return;
    }
    // See the identical branch in `render_paragraph_dispatch` — `paragraph_trailing_images`'s own
    // doc comment for the shape this closes.
    if extract_here {
        if let Some((text_range, images)) = paragraph_trailing_images(doc, src, &inline) {
            if math_here {
                walk_inline_math(
                    &mut events_iter_ranged(&doc.events[text_range]),
                    w,
                    src,
                    block_start,
                );
            } else {
                walk_inline(&mut events_iter(&doc.events[text_range]), w, None);
            }
            render_image_group(w, &images);
            return;
        }
        // See the identical branch in `render_paragraph_dispatch` — `paragraph_leading_images`'s own
        // doc comment for the shape this closes. No extra leading-blank-line handling needed here
        // (unlike that other call site): this function's own `if w.pending_block_gap { .. }` at its very
        // start, above, already ran before any dispatch decision was made.
        if let Some((images, text_range)) = paragraph_leading_images(doc, src, &inline) {
            render_image_group(w, &images);
            if math_here {
                walk_inline_math(
                    &mut events_iter_ranged(&doc.events[text_range]),
                    w,
                    src,
                    block_start,
                );
            } else {
                walk_inline(&mut events_iter(&doc.events[text_range]), w, None);
            }
            return;
        }
    }
    if math_here {
        walk_inline_math(
            &mut events_iter_ranged(&doc.events[inline]),
            w,
            src,
            block_start,
        );
    } else {
        walk_inline(&mut events_iter(&doc.events[inline]), w, None);
    }
}

/// Builds and pushes the current list item's own marker span onto whatever line `render_item` just
/// opened — the width formula, the unstyled `"<spaces>- "` for an unordered item, the
/// `LightBlue`-styled, right-aligned `"N. "` for an ordered one, incrementing `*counter` as it goes.
///
/// `depth` and `counter` are `render_item`'s own `ctx.list_depth`/`counter` parameters, passed
/// straight through unchanged — this function reads/writes neither `Writer` state nor anything of its
/// own; every input it needs arrives as a plain parameter. `depth` is always `>= 1` here by
/// construction (`render_list` always calls `render_item` with a `ctx` whose `list_depth` is its own
/// incoming depth `+ 1` — see that function's own doc comment — so there is always at least one list
/// open by the time any item's marker renders), unlike the `Vec<Option<u64>>` this used to read
/// (`list_counters.len()`), which could be observed empty by a direct, low-level test call bypassing
/// `render_list` entirely (see `BlockCtx::list_depth`'s own doc comment for the fuller history) —
/// there is no equivalent empty state a plain `usize` parameter can be "caught" in, so the defensive
/// `let ... else` an empty `Vec` used to require is gone along with the `Vec` itself.
fn push_item_marker(w: &mut Writer<'_>, depth: usize, counter: &mut Option<u64>) {
    let width = depth.saturating_mul(4).saturating_sub(3);
    let span = match counter {
        None => Span::raw(" ".repeat(width - 1) + "- "),
        Some(index) => {
            *index += 1;
            Span::styled(
                format!("{:width$}. ", *index - 1),
                Style::new().fg(Color::LightBlue),
            )
        }
    };
    w.append_span(span);
}

/// Renders a **plain** `Quote { alert: None, .. }` block's own body into `w`, in place — walks the
/// quote's own children into an isolated sub-`Writer`, decorates that output as a whole, then applies
/// a `>` prefix/style to every one of its (already-decorated) lines explicitly, via
/// `w.emit_row_prefixed` — the quote's own body can contain any of `Heading`/`Paragraph`/
/// `ThematicBreak`/`List`/a further nested `Quote`/`Details`, all of which get the same `>`-prefix
/// treatment this way. A nested `Quote` reached this way (`>>`) simply recurses into this same
/// function again — see this function's own body, below, for how a *second* `>` ends up on a doubly-
/// quoted line without either level ever needing to know about the other's prefix. A GitHub alert
/// (`Quote { alert: Some(_), .. }`) never reaches this function at all — see
/// `render_alert_from_model`, this function's own sibling for that shape.
///
/// Always calls `render_block` with `math_here: false, extract_here: false`, regardless of whatever
/// values were in `ctx` for *this* quote — production never lifts math out of a blockquote's own text
/// at all (`structure_mask`'s own exclusion; see `render_paragraph_math`'s own doc comment), and never
/// extracts a standalone image or a ```mermaid fence out of one either (`extract_block_image`'s/
/// `parse_fence`'s own `>`-prefix rejection — see `BlockCtx.extract_here`'s own doc comment), so
/// nothing this function dispatches to, however many more `List`s or nested `Quote`s that subtree goes
/// on to contain, should ever end up scanning for either. Because of that, `w.after_math` can never
/// legitimately be `true` by the time this function's own exit runs (nothing in its own subtree can
/// have set it) — so, unlike `render_list`'s own exit, this one stays the plain, unconditional
/// `pending_block_gap = true`, with no `after_math` check needed.
///
/// `details_interactive`, by contrast, is **propagated unchanged** to `children` (not forced) — a
/// plain quote's own body is still reached through the *same* top-level, single-parse walk `Doc::parse`
/// already performs (unlike a GitHub alert's or a `Details` block's own body, each of which gets its
/// own, independent re-parse through `render_bar_prefixed_body` — see `BlockCtx.details_interactive`'s
/// own doc comment): a `Details` block nested inside a plain quote
/// (reached at the *top* level, i.e. `ctx.details_interactive` still `true` coming in) is exactly as
/// interactive as one sitting directly at the top level.
fn render_quote(w: &mut Writer<'_>, doc: &Doc<'_>, src: &str, children: &[Block], ctx: BlockCtx) {
    if w.pending_block_gap {
        w.emit_blank_row();
        w.pending_block_gap = false;
    }
    // Renders `children` into an **isolated** sub-`Writer` first, and decorates *that* raw output
    // (`decorate_headings_and_extras`) before this quote's own `>` prefix ever gets applied to a
    // single line of it. Mirrors `render_bar_prefixed_body`'s own "decorate, then prepend the
    // container's own bar" shape exactly (see that function's own doc comment for the fuller
    // reasoning this borrows) — the identical, already-established pattern this file already uses for
    // a GitHub alert's or a `<details>` block's own body, not a new mechanism invented for quotes
    // specifically.
    //
    // Root cause this closes (a *pre-existing* gap, not a regression — confirmed directly against
    // the shipped, pre-refactor production build: `"> - [ ] x\n"` shows the same undecorated,
    // unfocusable `[ ] x` there too): an *earlier* version of this function rendered every child
    // straight into the same, single `w` with no isolation at all — so by the time `render_doc`'s
    // own, single, whole-document `decorate_headings_and_extras` pass finally ran (once, at the very
    // end, over every line this whole render produced), a quoted child's own lines already carried
    // their `>` prefix span, permanently. `decorate_extras`'s own `replace_task_checkbox`/
    // `task_prefix_state` (and `decorate_headings`'s own heading-rule recognition) both require a
    // line's own **first** span to carry no prefix at all — the module doc comment's own "How inline
    // content is rendered" section already names this as a known, *accepted* limitation for a quoted
    // heading/rule (matching production's own, equally `>`-blind `decorate_headings_and_extras`
    // there) — but a checkbox failing to become interactive is a real, user-visible loss (no Tab
    // focus, no `Space` toggle, no icon), exactly the shape `render_bar_prefixed_body` was already
    // built to avoid for an alert's/`<details>`'s own body. This function had simply never been
    // updated to use that same shape.
    //
    // The actual `>`-prepending step (the loop just below, after decoration) calls
    // `w.emit_row_prefixed` directly, on the **outer** `w` — not `inner` — with a single, explicit
    // `Span::raw(">")`/`w.styles.blockquote()` pair built locally right there, rather than mutating
    // any state on either `Writer`. This is what produces the identical `N` bare `">"` spans
    // (adjacent, no space between them) followed by exactly *one* shared space, then content,
    // regardless of nesting depth, that the pre-migration renderer always drew — not from one method
    // reading `N` entries off a stack in a single call, but from `N` *separate* calls to this method,
    // one per nesting level, each on its own `Writer` instance: a doubly-quoted line (`"> > x"`)
    // reaches its final shape because the *inner* quote's own `render_quote` call already prefixed it
    // with one `"> "` before handing it back (as an ordinary, already-decorated line sitting in
    // `inner.lines`), and *this* call prefixes it with a second `"> "` on top of that — never because
    // `inner.row_prefixes.len()` reached `2` inside one shared stack. `inner` itself never receives
    // any prefix/style of its own at construction (there is no `row_prefixes`/`row_styles` field left
    // on `Writer` to set) — a quote's own `>` is applied only once, by whichever call is about to hand
    // its own fully-decorated output back to its parent.
    let mut inner = Writer {
        lines: Vec::new(),
        inline_styles: Vec::new(),
        link: None,
        styles: w.styles,
        pending_block_gap: false,
        math: None,
        mermaid: MermaidCtx {
            slot: w.mermaid.slot,
            caption: w.mermaid.caption,
            // Same `- 2` reduction `render_bar_prefixed_body` uses for an alert's/`<details>`'s own
            // body, applied here for the identical reason (a code block or table directly inside
            // this quote must not overflow once the `>` prefix lands on top of it — see
            // `render_code_block`'s own doc comment on how this narrowed width reaches it). Not
            // exact for a **doubly**-nested quote specifically (each isolated level only ever knows
            // its own immediate parent's *un-prefixed* width, not how many quote levels already sit
            // above that — a real, accepted imprecision, not a silent one: see the width field's own
            // comment just below for the full reasoning and why erring narrow, never wide, is the
            // safe direction).
            width: w.width.saturating_sub(2),
            fences_on: w.mermaid.fences_on,
            ord: 0,
        },
        slot_of: w.slot_of,
        images: Vec::new(),
        code_blocks: Vec::new(),
        task_marks: Vec::new(),
        after_math: false,
        fresh_boundary: false,
        pending_para_start: None,
        heading_rule_shift: 0,
        code: w.code,
        theme: w.theme.clone(),
        // `w.width - 2`, always — not some tighter reduction computed from how many quote levels
        // actually sit above this one (unavailable here; see `mermaid.width`'s own comment above for
        // why). Every quote reached through *this* file's own recursion isolates independently, so a
        // doubly-quoted code block ends up narrower than the theoretically tightest fit by a column
        // or two, never wider — the direction that keeps `render_code_block`'s own
        // overflow-prevention guarantee intact even where the reduction is not pixel-perfect.
        width: w.width.saturating_sub(2),
        icons: w.icons,
        tasks: w.tasks,
        alerts: w.alerts,
        // One document-wide pair, copied verbatim for the same reason `alerts`/`icons` are: a
        // table or an image nested in this body aligns by the same rule as one at the top level,
        // inside this body's own already-narrowed `width`.
        aligns: w.aligns,
    };
    // Unchanged from before this fix: math lifting/image/mermaid extraction never fire inside a
    // blockquote at any depth (`structure_mask`'s own exclusion — see `BlockCtx.math_here`'s own doc
    // comment) — only `details_interactive` is still propagated, not forced, exactly as before (see
    // this function's own doc comment above for why: a plain quote's own body is reached through the
    // *same* top-level interactive scan, unlike an alert's/`<details>`'s own independently re-parsed
    // one). `list_depth: 0`, always, regardless of `ctx.list_depth` — `inner` is a fresh, isolated
    // sub-`Writer` (see this function's own doc comment for why), so a `List` found anywhere in this
    // quote's own body starts counting nesting from scratch, exactly as if it were the first list in
    // a brand new document; it has no way to see, nor any reason to see, how many lists (if any) were
    // already open in whatever `w` this quote itself sits inside of.
    let child_ctx = BlockCtx {
        math_here: false,
        extract_here: false,
        details_interactive: ctx.details_interactive,
        list_depth: 0,
    };
    for child in children {
        render_block(child, &mut inner, doc, src, child_ctx);
    }
    let decorated =
        decorate_headings_and_extras(inner.lines, inner.width, inner.icons, inner.tasks);
    let prefix = Span::raw(">");
    let style = w.styles.blockquote();
    // Where `inner`'s own first line is about to land in `w`, and how far every one of its own
    // columns is about to move right — read before the emit loop, which moves `w.lines.len()` on.
    // See `rebase_nested_images` for what these two numbers are for.
    let base = w.lines.len() + w.heading_rule_shift;
    for line in decorated {
        w.emit_row_prefixed(line, Some(&prefix), Some(style));
    }
    // Merged back the identical way `render_bar_prefixed_body` merges its own `inner`'s
    // accumulators. `images` used to be unconditionally empty here (`extract_here: false`) and was
    // merged purely defensively; a table inside this quote whose cells hold real-pixel images now
    // fills it for real (`emit_table` — cell images are not gated by `extract_here`, since nothing
    // about a cell image mirrors the legacy renderer's own block-image extraction), which is what
    // `rebase_nested_images` is for: an `inner` placement's row and column are `inner`-relative, and
    // extending them onto `w` unchanged would draw the image at the top of the document.
    w.images
        .extend(rebase_nested_images(inner.images, base, QUOTE_PREFIX_COLS));
    w.code_blocks.extend(inner.code_blocks);
    w.task_marks.extend(inner.task_marks);
    // Because of the isolation above, `w.after_math` can never legitimately be `true` here either —
    // nothing inside a quote's own subtree can have set it (`math_here: false`, throughout) — so this
    // stays the plain, unconditional `pending_block_gap = true`, exactly as before this fix.
    w.pending_block_gap = true;
}

/// Display width of the `">"` + shared space `render_quote` prefixes every one of its own body lines
/// with (`Writer::emit_row_prefixed` inserts both), and so the distance every column inside that body
/// moves right when it is merged into the enclosing `Writer`.
const QUOTE_PREFIX_COLS: u16 = 2;

/// The same, for the `"▌ "` bar `render_bar_prefixed_body` puts in front of an alert's or a
/// `<details>`'s own body lines (`super::alert_bar`/`details_bar` — both two cells wide).
const BAR_PREFIX_COLS: u16 = 2;

/// Move an isolated sub-`Writer`'s own image placements onto the enclosing `Writer`'s coordinates:
/// `base` rows down (where the sub-writer's first line lands, already converted past any heading
/// rules the outer decoration will insert ahead of it) and `prefix_cols` columns right (the `>` or
/// `▌ ` every one of those lines gains on the way out).
///
/// Both nesting containers isolate: they render their children into a fresh `Writer` whose `lines`
/// start at index 0 and whose column 0 is *inside* the prefix that has not been attached yet, so
/// every placement recorded in there is relative to that private origin. Until table cells could hold
/// real pixels this never mattered, because nothing inside either container ever recorded a placement
/// at all — the two `w.images.extend(inner.images)` calls were documented as defensive no-ops. They
/// are no longer no-ops, so the coordinates have to be translated rather than merely concatenated,
/// which is what this does.
///
/// The column translation is exact for a body one level deep, which is the only depth it can be exact
/// for: a doubly-quoted body's own sub-`Writer` is rebased onto its parent quote's coordinates here,
/// and that parent is then rebased onto *its* parent by this same function, so each level adds its own
/// prefix in turn.
fn rebase_nested_images(
    images: Vec<ImagePlacement>,
    base: usize,
    prefix_cols: u16,
) -> impl Iterator<Item = ImagePlacement> {
    images.into_iter().map(move |mut p| {
        p.line += base;
        p.col += prefix_cols;
        p
    })
}

/// Renders a GitHub alert (`Quote { alert: Some(kind), alert_title }`) into `w`, in place — the
/// header line via `super::alert_header_line` (a `markdown.rs`-hosted helper, so the color/icon/label
/// decision lives in exactly one place rather than being re-derived here), the body via
/// [`render_bar_prefixed_body`] with `super::alert_bar(kind.color())` as the bar. The header line is
/// appended with no leading-blank-line check of any kind, regardless of what `pending_block_gap`-
/// equivalent state the *previous* construct happened to end in — so this function does not check
/// `w.pending_block_gap` before pushing the header either, and leaves it `false` on the way out (not
/// `true`): matching that same "fresh boundary" contract `render_math_slot`'s own doc comment already
/// documents in full for a lifted math expression — whatever renders *next* is exactly as if a
/// brand-new `Writer` had just started, the same "fresh, independent sub-`Writer`" boundary
/// `render_bar_prefixed_body`'s own isolation produces.
///
/// Does **not** take a `BlockCtx`: nothing about this function's *own* rendering depends on either of
/// its two fields, and `children` always gets a freshly forced one regardless of what the caller had
/// in scope — see [`render_bar_prefixed_body`]'s own doc comment for exactly what gets forced and
/// why (both `math_here` and `details_interactive`, unconditionally — `render_bar_prefixed_body` is
/// the single body-rendering entry point both an alert's and a `Details` block's own body funnel
/// through).
///
/// ## The glued-header case — the one place *this* function re-parses anything
///
/// `> [!NOTE] title` is not real CommonMark syntax (see `trim_leading_header`'s own doc comment): it
/// is ordinary blockquote text that just happens to look like a header, so CommonMark's own lazy-
/// continuation rule can glue it into the *same* `Paragraph` as whatever non-blank line follows it —
/// `"> [!IMPORTANT]\n> Crate | Note\n> ------|------\n> ndk   | x\n"` parses as **one** `Paragraph`
/// spanning all four lines, not a `Paragraph` (the header) followed by a `Table` (the rows), because a
/// GFM table can never start mid-paragraph — CommonMark only recognizes one at the *top* of a block.
/// `trim_leading_header` copes with this by trimming the header's own leading `Event`s off that one
/// straddling `Paragraph` and rendering what is left of it as a paragraph — correct as far as it goes,
/// but it can never turn those trailing lines into a `Table`, `List`, or anything else a real,
/// block-level parse would have found, because nothing here re-derives block structure from raw text.
/// A **blank line** after the header avoids this entirely (the header becomes its own, separate,
/// complete `Paragraph`, and the table starts fresh right after it — CommonMark's normal case, the one
/// `trim_leading_header`'s "starts cleanly after the header" branch already handles without any
/// reparse) — this whole mechanism exists only for the no-blank-line shape.
///
/// `glued_alert_body` detects exactly this straddle (mirroring `trim_leading_header`'s own boundary
/// scan) and, when it fires, hands back one raw byte range: from right after the header line through
/// the **last** child's own `src.end` — the straddling block's own trailing text *and* every child
/// after it, not just the one straddling block. Everything from that point on is re-parsed and
/// re-rendered as a single unit rather than split at the straddle boundary — an earlier version of
/// this function split there (isolated `Doc::parse` of just the straddling block, then a second,
/// separate `render_bar_prefixed_body` call for the untouched children after it) and a real regression
/// caught that: `code_corpus: indented block nested inside an open alert` (`"> [!NOTE]\n> para\n>\n>
/// indented in alert\n"`) lost the blank line between `para` and the indented block that followed it,
/// because each `render_bar_prefixed_body` call starts its own brand-new sub-`Writer` (`pending_block_gap:
/// false`), and the blank-line-before-a-block-boundary spacing `emit_row` inserts depends on *that*
/// state carrying over within one continuous `Writer` session — which two separate calls, by
/// construction, cannot give it. Reparsing everything from the header on as one text run and handing
/// it to `render_bar_prefixed_body` exactly once restores that continuity (the identical reason
/// `render_details_from_model`'s own `glued_body` reparses a `<details>` block's *entire* raw body in
/// one shot, never split at whatever sibling boundary the pathological gluing happened to fall on).
///
/// This function then does the identical thing `render_details_from_model` already does for a glued
/// `<details>` body (`model::BlockKind::Details.glued_body`): copies that raw range out, strips its own
/// per-line blockquote marker (`dequote_alert_body` — every physical line in that range still carries
/// its own literal `>`, since `Block::src` is a contiguous byte range into the *whole* document, not a
/// marker-free reconstruction; see that function's own doc comment), and runs a **fresh, isolated**
/// `Doc::parse` over the result — a real block-level parse this time, since a bare "table" written as
/// three lines of plain pipes-and-dashes text (no leading `>` at all) is exactly the input a real
/// parser needs to recognize a `Table`. The isolated parse's own `blocks`/`events` are rendered
/// immediately through `render_bar_prefixed_body` and never touched again, the same narrow, self-
/// contained exception `render_details_from_model`'s own doc comment already carves out of this file's
/// "never re-parses anything" promise — not a second instance of that exception, the *same* one,
/// reused for a second call site with the identical safety argument.
///
/// When `glued_alert_body` finds no straddle (children is empty, entirely header, or already starts
/// cleanly after it) this function's behavior is **byte-for-byte unchanged** from before this case
/// existed: `trim_leading_header` still runs, and `render_bar_prefixed_body` is still called exactly
/// once, on its output — the fast path this mechanism intentionally leaves alone.
fn render_alert_from_model(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    kind: AlertKind,
    title: &str,
    children: &[Block],
    quote_src_start: usize,
) {
    w.emit_row(alert_header_line(kind, title, w.icons));
    let header_end = line_after(src, quote_src_start);
    let bar = alert_bar(kind.color());
    match glued_alert_body(children, header_end) {
        Some(glued_range) => {
            // Owned, not borrowed from `src`: `Doc::parse` needs a string that outlives this whole
            // call — mirrors `render_details_from_model`'s own identical `body_src` for
            // `glued_body`, see that function's own doc comment for why an isolated copy, not a
            // slice of `src`, is the right shape here.
            let base = glued_range.start;
            let (body_src, line_map) = dequote_alert_body(&src[glued_range]);
            let nested = Doc::parse(&body_src);
            // `render_bar_prefixed_body` merges its own inner `Writer`'s `task_marks` onto `w`'s
            // (`w.task_marks.extend(inner.task_marks)`) — always appended, never inserted — so the
            // slice pushed by *this* call is exactly the tail starting at `marks_before`. Every one of
            // those offsets was computed against `body_src` (`Writer::render_item`'s own `t.state_at`/
            // `item_src_start + off`, both "absolute byte offset into `src`" for whichever `src` this
            // particular call was handed — `body_src` here, not the real file) — see
            // `remap_glued_alert_offset`'s own doc comment for why translating them back is not just
            // "add `base`": `dequote_alert_body` does not merely slice, it strips a different number of
            // marker bytes off every physical line, so no single constant offset holds across a line
            // boundary. Left unmapped, `Space`/`Enter` on a real task inside a glued alert body writes
            // its new state character at the *wrong* byte in the user's actual file — confirmed
            // directly: `app::tests::md_task_toggle_is_byte_exact_across_the_corpus` failed on exactly
            // this shape (`task_corpus: task-lookalike inside an indented block nested in an alert
            // stays literal`) before this remap existed, "別の位置を書き換えた" against a real temp
            // file, not merely a mismatched assertion in isolation.
            let marks_before = w.task_marks.len();
            render_bar_prefixed_body(w, &nested, &body_src, &nested.blocks, bar);
            for mark in &mut w.task_marks[marks_before..] {
                mark.1 = base + remap_glued_alert_offset(mark.1, &line_map);
            }
        }
        None => {
            let trimmed = trim_leading_header(children, doc, header_end);
            render_bar_prefixed_body(w, doc, src, &trimmed, bar);
        }
    }
    w.pending_block_gap = false;
    w.fresh_boundary = true;
}

/// Detects the "header glued into the same `Paragraph` as its own body" shape
/// `render_alert_from_model`'s own doc comment describes in full, mirroring `trim_leading_header`'s
/// own boundary scan exactly (same three shapes, same order) but reporting the straddle instead of
/// papering over it: `None` for the two shapes `trim_leading_header` already handles correctly on its
/// own (nothing before `header_end` needs a reparse, or nothing straddles it at all), `Some(range)`
/// only for the one shape it cannot — a `Paragraph` whose own `src` starts before `header_end` and ends
/// after it. `range` runs from `header_end` through the **last** child's own `src.end` (not just the
/// straddling block's) — see `render_alert_from_model`'s own doc comment for why a split there
/// regressed a real corpus case. A non-`Paragraph` straddle is impossible by construction (every other
/// block kind always starts fresh at its own line — see `trim_leading_header`'s own doc comment) but
/// matched defensively all the same, `None`, falling back to `trim_leading_header`'s own identical
/// defensive branch (principle #3, `CLAUDE.md`): keeping the block unmodified is safer than a reparse
/// this shape was never actually tested against.
fn glued_alert_body(children: &[Block], header_end: usize) -> Option<Range<usize>> {
    let b = children.iter().find(|b| b.src.end > header_end)?;
    if b.src.start >= header_end {
        return None; // starts cleanly after the header — trim_leading_header's fast path applies
    }
    if !matches!(b.kind, BlockKind::Paragraph { .. }) {
        return None; // can't legitimately straddle — defensive fallback, matches trim_leading_header
    }
    // `children` is in non-decreasing source order (`model::Doc::parse`'s own invariant), so the last
    // child's own `src.end` is this alert body's own last byte — always `>= b.src.end`, `b` itself
    // included when it happens to be the only child.
    Some(
        header_end
            ..children
                .last()
                .expect("b came from this slice, so it is non-empty")
                .src
                .end,
    )
}

/// Strips one level of blockquote marker (`markdown.rs`'s own `strip_blockquote`/`is_blockquote_line`
/// pair, reused rather than re-derived) from every physical line of `raw`. Needed because `raw`
/// (`glued_alert_body`'s own `range`, a raw slice of the whole document's `src`) is *not* marker-free:
/// `Block::src` is a contiguous byte range into the full document, so only the straddling paragraph's
/// own **first** line has its leading `> ` excluded (`header_end` — see `glued_alert_body`'s own doc
/// comment — always lands exactly on the *next* physical line's own leading `>`, since that is where
/// the header's line ends); every line after that still carries its own literal `>` verbatim, the same
/// way a `<details>` block's `glued_body` never needed this step at all (an HTML block's own raw
/// interior has no quote marker to begin with). Feeding `raw` to `Doc::parse` unstripped would parse
/// every continuation line as its own nested block quote instead of the table/list/etc. it actually
/// is — confirmed directly (an isolated reparse of an un-dequoted `"> a\n> b\n"`-shaped slice reports a
/// second, spurious `BlockQuote` around everything past the first line).
///
/// Also returns a `line_map`, `(dequoted_line_start, raw_line_start)` pairs in source order, one per
/// physical line — `remap_glued_alert_offset`'s own input, and the reason this function does *not*
/// just return `String` the way an earlier version did: unlike `render_details_from_model`'s own
/// `glued_body` (whose `body_src` is a plain, unmodified slice of `src`, so `range.start` alone is
/// already the right correction for anything the isolated re-render reports), this function *removes*
/// bytes — a different number per line (a bare `>` strips one, `> ` strips two, an already-marker-free
/// line strips zero) — so no single constant offset can translate a `body_src`-relative position back
/// into `raw`'s own coordinate space; only a per-line table can. `raw`'s own EOL sequence (`\n` or a
/// CRLF document's own `\r\n`) is preserved byte-for-byte on the dequoted side too, both so a CRLF
/// alert body's own isolated reparse sees exactly the line endings a real `Doc::parse` of a top-level
/// CRLF document would, and so `raw_pos` below (this function's own running position in `raw`) advances
/// by each line's *actual* raw byte length, not an assumed constant.
fn dequote_alert_body(raw: &str) -> (String, Vec<(usize, usize)>) {
    let mut out = String::with_capacity(raw.len());
    let mut line_map = Vec::new();
    let mut raw_pos = 0usize;
    for line in raw.split_inclusive('\n') {
        let (text, term) = match line.strip_suffix("\r\n") {
            Some(t) => (t, "\r\n"),
            None => match line.strip_suffix('\n') {
                Some(t) => (t, "\n"),
                None => (line, ""), // final line, no trailing newline at all
            },
        };
        let stripped = super::strip_blockquote(text);
        let marker_len = text.len() - stripped.len();
        line_map.push((out.len(), raw_pos + marker_len));
        out.push_str(&stripped);
        out.push_str(term);
        raw_pos += line.len();
    }
    (out, line_map)
}

/// Translates `body_offset` — an absolute byte offset into `dequote_alert_body`'s own dequoted output
/// — back into that same call's `raw` argument's own coordinate space, via `line_map` (that function's
/// own second return value; see its doc comment for why a per-line table, not a constant, is needed at
/// all). Finds the physical line `body_offset` falls on (the last entry whose own dequoted-side start
/// is `<= body_offset` — `line_map` is in non-decreasing source order by construction, one entry per
/// line in the order `dequote_alert_body`'s own loop visited them, so `partition_point` applies) and
/// carries the same within-line distance over to the raw side: the dequoted and raw text are
/// byte-identical from the marker's own end through that line's own terminator, so an offset `d` bytes
/// into the dequoted line is also `d` bytes into the raw one. `render_alert_from_model`'s own call site
/// adds `glued_range.start` on top of this function's own result — `line_map`'s "raw" side is relative
/// to `raw` (`dequote_alert_body`'s own parameter, itself already `src[glued_range]`), not to the whole
/// document, so this function alone cannot produce an absolute `src` offset on its own.
fn remap_glued_alert_offset(body_offset: usize, line_map: &[(usize, usize)]) -> usize {
    let idx = line_map
        .partition_point(|&(body_start, _)| body_start <= body_offset)
        .saturating_sub(1);
    let (body_start, raw_start) = line_map[idx];
    raw_start + (body_offset - body_start)
}

/// The byte position right after the next `'\n'` at or after `start` — `src.len()` if there is
/// none. Mirrors `model::line_after` exactly (not reused directly: that one is private to the
/// model module, and this file's own use is a one-liner) — used by `render_alert_from_model` to
/// find where a GitHub alert's own header line (`"> [!NOTE] title\n"`, or, once a `Quote`'s own `>`
/// markers are logically stripped, just `"[!NOTE] title\n"`) ends within `src`, so
/// `trim_leading_header` knows which of the quote's own parsed `children` — or which *part* of
/// one — is actually the header line's own text, not the alert's real body.
fn line_after(src: &str, start: usize) -> usize {
    match src[start..].find('\n') {
        Some(off) => start + off + 1,
        None => src.len(),
    }
}

/// Trims a GitHub alert's own header-line text out of its `Quote`'s already-parsed `children` —
/// needed because, unlike `<details>` (a real CommonMark HTML block, whose own opening tag is a
/// *separate* sibling `Doc::parse` never folds into the body — see `model::fold_details`'s own doc
/// comment), `> [!NOTE] title` is not real CommonMark syntax at all: pulldown-cmark parses a
/// `Quote`'s body exactly the way it would any other blockquote, with the literal text `"[!NOTE]
/// title"` becoming part of (or all of) that body's own **first** block, right alongside whatever
/// real content follows it. Without this trim, that literal header text renders a second time,
/// verbatim, as the alert's own first body line, directly underneath the *already*-decorated header
/// `alert_header_line` just drew — confirmed directly against every alert case in the parity corpus
/// before this function existed (`task_corpus: alert NOTE` and its siblings, each showing e.g.
/// `["[", "!NOTE", "]"]` as a spurious extra line).
///
/// `header_end` is the byte position right after the header's own physical source line (see
/// `line_after`) — everything in `children` **before** that position is the header's own text, not
/// body content. Three shapes, in the order this function checks for them:
///
/// * A child whose own `src.end <= header_end` is **entirely** the header line (a common shape: the
///   header interrupts nothing and forms its own complete leading `Paragraph`, e.g. `"[!NOTE]"`
///   followed by a `List` that CommonMark's own "a list can interrupt a paragraph" rule starts
///   fresh right after it) — dropped outright.
/// * A child whose own `src.start >= header_end` starts cleanly *after* the header line — kept
///   unmodified, and so is everything after it (nothing earlier in `children` could ever need
///   trimming once one child has already cleared the boundary, since blocks are in non-decreasing
///   source order).
/// * A child that straddles the boundary — only a `Paragraph` can (CommonMark's lazy-continuation
///   rule joins the header's own text and the next line into *one* paragraph, via a `SoftBreak`,
///   whenever nothing about that next line starts a new block on its own — e.g. `"[!NOTE]\nSome
///   prose."` with no blank line between them) — has its own `inline` event range trimmed to start
///   at the first event landing at or after `header_end`, additionally skipping any leading
///   `SoftBreak`/`HardBreak` right at that position so the body's own first line doesn't open with
///   a stray leading space. A non-`Paragraph` child cannot legitimately straddle this boundary at
///   all (every other block kind always starts fresh at its own line's beginning) — kept unmodified
///   rather than dropped, as a defensive fallback (principle #3, `CLAUDE.md`): degrading to "renders
///   the header's own text once more than it should" is safer than discarding real content this
///   function was never actually tested against losing.
fn trim_leading_header(children: &[Block], doc: &Doc<'_>, header_end: usize) -> Vec<Block> {
    for (i, b) in children.iter().enumerate() {
        if b.src.end <= header_end {
            continue; // entirely the header line — drop it, keep scanning for the real body
        }
        if b.src.start >= header_end {
            return children[i..].to_vec(); // starts cleanly after the header — keep as-is
        }
        // Straddles the boundary.
        let mut out = Vec::with_capacity(children.len() - i);
        if let BlockKind::Paragraph { inline } = &b.kind {
            let mut start = inline.start;
            while start < inline.end {
                let (ev, r) = &doc.events[start];
                if r.start < header_end || matches!(ev, Event::SoftBreak | Event::HardBreak) {
                    start += 1;
                } else {
                    break;
                }
            }
            out.push(Block {
                kind: BlockKind::Paragraph {
                    inline: start..inline.end,
                },
                src: header_end..b.src.end,
                children: Vec::new(),
            });
        } else {
            out.push(b.clone());
        }
        out.extend_from_slice(&children[i + 1..]);
        return out;
    }
    Vec::new()
}

/// Renders a `Details` block into `w`, in place — the summary marker line via
/// `super::details_marker_line`, the body (when open) via [`render_bar_prefixed_body`] with
/// `super::details_bar()` as the bar. Mirrors `render_alert_from_model`'s own doc comment on why
/// there is no leading-blank-line check here either, and why `w.pending_block_gap` ends up `false`, not
/// `true`, on the way out (the identical "fresh boundary" `render_alert_from_model` produces, this
/// time for a `<details>` block instead of an alert).
///
/// `ctx.details_interactive` decides, for *this* block specifically (not its children, which
/// `render_bar_prefixed_body` always forces `false` regardless — see that function's own doc
/// comment): whether it consumes a slot from the document-wide Tab-toggle ordinal sequence
/// (`super::next_details_open(open_attr)`, and draws the marker in the Tab-focus sentinel style) or
/// simply uses its own `open_attr` directly (no ordinal consumed, a plain marker style) — passed
/// straight through to `super::details_marker_line`'s own identical `interactive` parameter.
///
/// ## `glued_body` — the one place this file re-parses anything
///
/// `children` is always empty for a **glued** `<details>` block (`model::BlockKind::Details.glued_body`
/// — see that field's own doc comment for exactly which shape this is): there was no separate sibling
/// `Block` for `fold_details` to have found the body in, in the first place, because CommonMark's own
/// HTML-block grammar glued the whole construct — open tag, `<summary>`, body, close tag — into one
/// raw, literal `Html` leaf, with no blank line anywhere to have split any of it into finer structure
/// for `Doc::parse`'s one whole-document walk to have reported. There is, structurally, no substring
/// of the *already-recorded* `doc.events` this body's content could be read from (an HTML block's own
/// interior is copied literally, per spec — no inline events were ever emitted for it at all), so
/// `children.is_empty() && glued_body.is_some()` takes a different path: a **fresh, isolated**
/// `Doc::parse` over `src[glued_body]` (an owned copy of that raw text — nothing here borrows from the
/// *outer* `src`, so there is no lifetime entanglement with `doc.events`, this call's own arguments),
/// rendered immediately through the ordinary `render_bar_prefixed_body` path — the same one a
/// well-formed `<details>`'s own real `children` render through — with that isolated `Doc`/its own raw
/// text standing in for `doc`/`src` for this **one** call. The isolated parse's own `blocks`/`events`
/// are never touched again after this — nothing downstream of it is indexed by anything outside
/// itself — so this stays a narrow, self-contained exception to the rest of this file's "never
/// re-parses anything" promise (see the module doc comment's own note on this), not a reopening of it:
/// a reference-style link whose definition happens to live *outside* this one glued body would not
/// resolve inside the isolated re-parse (the identical gap a per-block re-parse used to have
/// everywhere, before this file's own single-whole-document-walk rewrite — see the module doc
/// comment's "How inline content is rendered" section), an acceptable, honestly-scoped cost for a
/// shape CommonMark itself gives no finer structure to read in the first place.
#[allow(clippy::too_many_arguments)] // mirrors render_dollar_math_tail's own identical allow — one call site (render_block's Details dispatch), each argument its own distinct piece of state
fn render_details_from_model(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    open_attr: bool,
    summary: &str,
    children: &[Block],
    glued_body: Option<Range<usize>>,
    ctx: BlockCtx,
) {
    let open = if ctx.details_interactive {
        next_details_open(open_attr)
    } else {
        open_attr
    };
    w.emit_row(details_marker_line(open, summary, ctx.details_interactive));
    if open {
        if !children.is_empty() {
            render_bar_prefixed_body(w, doc, src, children, details_bar());
        } else if let Some(range) = glued_body {
            // Owned, not borrowed from `src`: `Doc::parse` needs a string that outlives this whole
            // call, and this one's own text has nothing to do with the outer document's lifetime —
            // see this function's own doc comment on why an isolated copy, not a slice of `src`, is
            // the right shape here.
            let body_src = src[range].to_string();
            let nested = Doc::parse(&body_src);
            render_bar_prefixed_body(w, &nested, &body_src, &nested.blocks, details_bar());
        }
    }
    w.pending_block_gap = false;
    w.fresh_boundary = true;
}

/// Shared body-rendering half of `render_alert_from_model`/`render_details_from_model`: renders
/// `children` into a **fresh, independent** sub-`Writer`, computing an alert's/`Details`'s own body
/// in complete isolation — a fresh `Vec<Line>`, a fresh `Writer` underneath it — before splicing a
/// bar prefix onto every resulting line and appending the result to the *caller's* own output.
///
/// The sub-`Writer` starts with every piece of *positional* state reset (`lines`/`pending_block_gap`/
/// `pending_para_start`/`after_math`, all fresh or `false`) — a body is a standalone construct, not a
/// continuation of whatever came before the alert/`<details>` — but **inherits** `w`'s own
/// `styles`/`code`/`theme`/`icons` (display configuration, not position) and renders at `w.width - 2`
/// (the two columns `render_alert_from_model`/`render_details_from_model` reserve for the bar).
/// `math: None`: math is never lifted inside either body (`structure_mask`'s own exclusion — the same
/// reason `render_quote` forces `math_here: false` for a plain quote's own children), so there is
/// nothing to share across the isolation boundary; `body_ctx` below (`math_here: false,
/// extract_here: false, details_interactive: false, list_depth: 0`) is what `children` actually
/// renders with, unconditionally, regardless of what was in scope at the call site — see
/// `render_details_from_model`'s own doc comment for why *this* block's own interactivity is a
/// separate question from its children's, and `render_quote`'s `child_ctx` for why `list_depth`
/// resets to `0` here for the identical "fresh, isolated sub-`Writer`" reason.
///
/// Once the sub-`Writer` finishes, its own resulting lines are run through
/// `super::decorate_headings_and_extras` — **before** `bar` gets prepended to any of them — the same
/// pass `render_doc`'s own top-level dispatch runs over every other text segment. Task-checkbox
/// recognition specifically (`replace_task_checkbox` →
/// `task_prefix_state`) only ever matches a line whose *own, trimmed* text starts with a bullet —
/// a `▌ `/`▏ ` bar prepended first would permanently hide every checkbox inside an alert/`<details>`
/// body from that check (confirmed directly: without this call here, `render_doc`'s own single,
/// top-level `decorate_headings_and_extras` pass — which only ever runs *after* every bar is already
/// in place — never converts one). Heading decoration is not at risk of running twice because of
/// this *extra* call: `decorate_headings`'s own `heading_level` reads `line.spans.first()` and
/// requires no leading span at all (see the module doc comment's own note on this), so the *later*,
/// outer pass (`render_doc`'s own, over the *already* bar-prefixed `w.lines`) simply never matches
/// these lines a second time — this call is the only chance heading decoration (or task-checkbox
/// conversion) ever gets for content reached this way.
///
/// Each (now fully decorated) line then gets `bar` prepended onto its spans (its own style preserved
/// via `Line::from(spans).style(bl.style)`) and is pushed into
/// the **outer**, shared `w` via `w.emit_row` — not a direct `w.lines.push` — so a nested
/// alert/`<details>` (this function called recursively, through `render_block`'s own dispatch, from
/// *inside* another alert's/`<details>`'s own body) gets its own bar layered on top of the inner
/// one's the same way a real, doubly-nested `> >` quote gets two `>` spans (one bar prepended per
/// nesting level, from the inside out, each time this function recurses) — and so that an
/// alert/`<details>` nested inside a **plain** `Quote` still gets that quote's own `>` prefix applied
/// on top: this function's own bar-prefixed lines land in `w.lines` like any other content this
/// quote's children produce, so the enclosing `render_quote`'s own re-emit loop (see that function's
/// own doc comment) prepends its `>` to them the identical way it does to everything else in
/// `inner.lines` once every child has finished.
fn render_bar_prefixed_body(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    children: &[Block],
    bar: Span<'static>,
) {
    let mut inner = Writer {
        lines: Vec::new(),
        inline_styles: Vec::new(),
        link: None,
        styles: w.styles,
        pending_block_gap: false,
        math: None,
        // `body_ctx.extract_here: false` (below) keeps both mermaid-fence extraction
        // (`render_code_block_dispatch`) and standalone-image extraction
        // (`try_render_paragraph_as_image`) from ever actually consulting `mermaid`/`slot_of` at all
        // inside an alert's/`<details>`'s own body (production never extracts either there either —
        // see `BlockCtx.extract_here`'s own doc comment) — so the exact values here are never read;
        // `w.mermaid`'s own fields are simply copied over (cheap: two `&dyn Fn` references and a
        // `bool`/`u16`) rather than inventing new, unused ones, and `ord: 0` needs no special value
        // for the identical reason.
        mermaid: MermaidCtx {
            slot: w.mermaid.slot,
            caption: w.mermaid.caption,
            width: w.width.saturating_sub(2),
            fences_on: w.mermaid.fences_on,
            ord: 0,
        },
        slot_of: w.slot_of,
        images: Vec::new(),
        code_blocks: Vec::new(),
        task_marks: Vec::new(),
        after_math: false,
        fresh_boundary: false,
        pending_para_start: None,
        heading_rule_shift: 0,
        code: w.code,
        theme: w.theme.clone(),
        width: w.width.saturating_sub(2),
        icons: w.icons,
        tasks: w.tasks,
        // Copied from the outer `Writer`, matching `icons`/`code`/`theme` just above — a nested
        // alert discovered *inside* this body (an alert nested inside a `Details` block, or inside
        // another alert) has to honor the same document-wide `ui.md_alerts` toggle the outer render
        // is using; see `Writer.alerts`'s own doc comment.
        alerts: w.alerts,
        // One document-wide pair, copied verbatim for the same reason `alerts`/`icons` are: a
        // table or an image nested in this body aligns by the same rule as one at the top level,
        // inside this body's own already-narrowed `width`.
        aligns: w.aligns,
    };
    let body_ctx = BlockCtx {
        math_here: false,
        extract_here: false,
        details_interactive: false,
        list_depth: 0,
    };
    for child in children {
        render_block(child, &mut inner, doc, src, body_ctx);
    }
    let decorated =
        decorate_headings_and_extras(inner.lines, inner.width, inner.icons, inner.tasks);
    // Read before the emit loop below moves `w.lines.len()` on — see `rebase_nested_images`.
    let base = w.lines.len() + w.heading_rule_shift;
    for line in decorated {
        let style = line.style;
        let mut spans = vec![bar.clone()];
        spans.extend(line.spans);
        w.emit_row(Line::from(spans).style(style));
    }
    // No longer always empty: `body_ctx.extract_here: false` still keeps standalone-image and
    // mermaid extraction from firing anywhere in this body, but a table inside an alert or a
    // `<details>` whose cells hold real-pixel images records placements of its own (`emit_table`,
    // which cell images reach regardless of `extract_here`). Rebased onto `w`'s own coordinates for
    // the same reason `render_quote` rebases its own — see `rebase_nested_images`.
    w.images
        .extend(rebase_nested_images(inner.images, base, BAR_PREFIX_COLS));
    // Unlike `images`, these are **not** always empty: a code block or a task-list checkbox inside an
    // alert's/`<details>`'s own body is drawn (`render_code_block`/`render_item`, reached the normal
    // way via `render_block`'s dispatch above) and is a real Tab/`y c` target on screen — merged back
    // in document order, at the exact point this alert/`<details>` sits in the *outer* walk, matching
    // where its own bar-prefixed lines just landed in `w.lines` two statements up.
    w.code_blocks.extend(inner.code_blocks);
    w.task_marks.extend(inner.task_marks);
}

#[cfg(test)]
mod tests {
    use super::*;
    // `Span::dim()` (the `Stylize` trait) — needed by the two math regression tests below to build
    // the exact same dimmed placeholder `math_raw_lines` itself constructs (see that function's own
    // `Span::from(text).dim()`), for a byte-for-byte `Line` equality check against `render_doc`'s
    // own output.
    use ratatui::style::Stylize;

    /// The no-op image slot every test below that has no images of its own to exercise reaches for —
    /// `ImageSlot::Unavailable` (matching the diff harness's own `md_snapshot_tests::render_case`
    /// choice for the identical reason: no live `Picker` exists in a unit test).
    fn no_images(_: &str, _: Option<u16>) -> ImageSlot {
        ImageSlot::Unavailable
    }

    /// The no-op mermaid slot every test below that has no mermaid fences of its own to exercise
    /// reaches for — `MermaidSlot::Text` disables extraction entirely (`fences_on` reads `false`),
    /// matching this file's own pre-mermaid-support behavior (a mermaid-tagged fence draws as
    /// ordinary code) for every test that predates this pass.
    fn no_mermaid(_: &str) -> MermaidSlot {
        MermaidSlot::Text
    }

    fn fresh_writer() -> Writer<'static> {
        Writer {
            lines: Vec::new(),
            inline_styles: Vec::new(),
            link: None,
            styles: KonomaStyles::default(),
            pending_block_gap: false,
            math: None,
            mermaid: MermaidCtx {
                slot: &no_mermaid,
                caption: "Enter: full screen",
                width: 80,
                fences_on: false,
                ord: 0,
            },
            slot_of: &no_images,
            images: Vec::new(),
            code_blocks: Vec::new(),
            task_marks: Vec::new(),
            after_math: false,
            fresh_boundary: false,
            pending_para_start: None,
            heading_rule_shift: 0,
            code: CodeStyle::default(),
            theme: String::new(),
            width: 80,
            icons: false,
            tasks: super::super::DEFAULT_TASK_STATES,
            // Matches `ui.md_alerts`'s own default (`true`) — every existing test below that reaches
            // a `Quote { alert: Some(_), .. }` through this helper expects the colored-callout path
            // unless its own doc comment says otherwise.
            alerts: true,
            // Matches production's own default (`BlockAligns::default`) — tables left, images and
            // mermaid diagrams centered — so every existing test below renders exactly as it did
            // before these options existed.
            aligns: BlockAligns::default(),
        }
    }

    /// `BlockCtx` with math off, extraction on, `Details` interactive, and no list open — the shape
    /// a fresh top-level render started with `math_on: false` has (see `render_doc`'s own
    /// construction of `ctx`: `math_here: math.is_some()`, but `extract_here: true` unconditionally,
    /// regardless of `math_on` — see `BlockCtx.extract_here`'s own doc comment for why the two are
    /// not the same question); most of this module's own tests have no math/image/mermaid/`<details>`
    /// case to exercise any of these flags' *other* value, so this is the default every test below
    /// reaches for unless a test's own doc comment says otherwise. A test that needs to simulate
    /// already being inside a list (`render_item`/`push_item_marker`, direct, low-level calls) builds
    /// its own `BlockCtx { list_depth: 1, ..fresh_ctx() }` rather than reaching for a different helper.
    fn fresh_ctx() -> BlockCtx {
        BlockCtx {
            math_here: false,
            extract_here: true,
            details_interactive: true,
            list_depth: 0,
        }
    }

    /// Regression for the `line.spans.insert(1, marker_span)` panic
    /// ("insertion index (is 1) should be <= len (is 0)") — a fresh, *empty* line (`spans.len() ==
    /// 0`) is exactly the shape `render_item` leaves behind for a **loose** item's own task marker
    /// (see `write_task_marker`'s own doc comment): `spans.first_mut()` returns `None`, so the
    /// early-return splice never fires, and a fixed `insert(1, ..)` used to panic against the empty
    /// `Vec` — confirmed to actually happen (both directly on `Writer`, and reached through
    /// `render_item` on `task_corpus`'s own "a real task at a list item's own indentation is still a
    /// real task" case, `"- [ ] outer\n\n  - [ ] nested at matching indent\n"`) before this fix; see
    /// `write_task_marker`'s own doc comment for why `spans.len().min(1)` is the constructive fix
    /// rather than a special-cased early return. Before the fix, this test itself panicked with
    /// exactly that message.
    #[test]
    fn write_task_marker_on_a_fresh_empty_line_does_not_panic() {
        let mut w = fresh_writer();
        w.lines.push(Line::default());
        w.write_task_marker(false);
        assert_eq!(w.lines.len(), 1);
        assert_eq!(
            w.lines[0].spans.len(),
            1,
            "the marker becomes the line's only span"
        );
        assert_eq!(w.lines[0].spans[0].content.as_ref(), "[ ] ");
    }

    /// Pins the exact pulldown-cmark contract `backslash_math_opener`'s own doc comment cites as
    /// evidence: a backslash-escaped bracket/paren never survives as part of *any* event's own byte
    /// range at all — not even the escaped character's own event (unlike a code span's own
    /// delimiters, which *do* stay part of the range — see the `Code` assertion below). If a future
    /// pulldown-cmark upgrade ever changed this, `backslash_math_opener`'s whole detection strategy
    /// (reading the *gap* between two events) would silently stop working — this test exists so that
    /// change shows up here first, as a failing assertion, rather than as a re-broken math case.
    #[test]
    fn escape_handling_drops_the_backslash_from_every_event_range() {
        let src = "`\\[x\\]` and \\[y\\]\n";
        let doc = Doc::parse(src);
        let events: Vec<(Event<'_>, Range<usize>)> = doc.events;
        assert_eq!(events.len(), 6, "events: {events:#?}");
        assert!(
            matches!(&events[1].0, Event::Code(c) if c.as_ref() == "\\[x\\]"),
            "a code span's own delimiters keep their content literal — events: {events:#?}"
        );
        assert_eq!(events[1].1, 0..7);
        assert_eq!(&src[events[1].1.clone()], "`\\[x\\]`");
        assert!(matches!(&events[2].0, Event::Text(t) if t.as_ref() == " and "));
        assert_eq!(events[2].1, 7..12);
        assert!(matches!(&events[3].0, Event::Text(t) if t.as_ref() == "[y"));
        assert_eq!(
            events[3].1,
            13..15,
            "the opening backslash (position 12) is not part of this event's own range at all"
        );
        assert_eq!(&src[12..13], "\\");
        assert!(matches!(&events[4].0, Event::Text(t) if t.as_ref() == "]"));
        assert_eq!(
            events[4].1,
            16..17,
            "the closing backslash (position 15) is not part of this event's own range either"
        );
        assert_eq!(&src[15..16], "\\");
    }

    /// Pins the exact shape `render_backslash_math`'s own doc comment relies on for the "no closer
    /// found" fallback: an escaped bracket used to stop `[text](url)` from parsing as a link produces
    /// the *same* gap="\\" + payload-starts-with-"[" pattern a genuine backslash-math opener does,
    /// with no matching "\\]" anywhere on the line — the real ambiguity `backslash_math_opener` alone
    /// cannot resolve without scanning ahead.
    #[test]
    fn escaped_bracket_with_no_matching_closer_has_no_gap_before_its_own_closing_bracket() {
        let src = "This is \\[not a link](nope).\n";
        let doc = Doc::parse(src);
        let events: Vec<(Event<'_>, Range<usize>)> = doc.events;
        assert_eq!(events.len(), 6, "events: {events:#?}");
        assert!(matches!(&events[1].0, Event::Text(t) if t.as_ref() == "This is "));
        assert_eq!(events[1].1, 0..8);
        assert!(matches!(&events[2].0, Event::Text(t) if t.as_ref() == "[not a link"));
        assert_eq!(
            events[2].1,
            9..20,
            "the escaping backslash (position 8) is dropped from the range, same as the math case"
        );
        assert_eq!(&src[8..9], "\\");
        assert!(matches!(&events[3].0, Event::Text(t) if t.as_ref() == "]"));
        assert_eq!(
            events[3].1,
            20..21,
            "no gap before the closing bracket — it is not itself escaped, so this can never look like a closer"
        );
        assert_eq!(&src[20..20], "", "zero-width gap");
    }

    /// `spans.len().min(1)` must not regress the *non-empty* line case it already handled correctly
    /// (a tight ordered item's own `"N. "` marker span, checked) — the marker still lands as a
    /// second, separate span right after it, not before it.
    #[test]
    fn write_task_marker_on_a_line_with_one_existing_span_inserts_after_it() {
        let mut w = fresh_writer();
        w.lines.push(Line::from(vec![Span::raw("1. ")]));
        w.write_task_marker(true);
        assert_eq!(w.lines.len(), 1);
        let spans: Vec<&str> = w.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(spans, vec!["1. ", "[x] "]);
    }

    /// Regression, through the real block-model walk (not just the isolated `Writer` call above):
    /// `task_corpus`'s own `"LooseTask"` case renders without panicking now that
    /// `contains_unsupported` no longer excludes it (see that function's own doc comment). Loosely
    /// pins the resulting shape too — both items' own markers land on their own, `render_item`-opened
    /// blank line (`"[ ] outer"` / `"[ ] nested..."`), matching the module doc comment's own
    /// description of a loose item's marker never sharing its bullet's line.
    #[test]
    fn loose_task_item_renders_without_panicking_through_render_item() {
        let src = "- [ ] outer\n\n  - [ ] nested at matching indent\n";
        let doc = Doc::parse(src);
        assert!(
            contains_unsupported(&doc.blocks[0].children, false).is_none(),
            "the LooseTask exclusion should be gone"
        );
        let mut w = fresh_writer();
        let BlockKind::List { .. } = &doc.blocks[0].kind else {
            panic!("expected a top-level list");
        };
        let BlockKind::ListItem { task } = &doc.blocks[0].children[0].kind else {
            panic!("expected the outer list item");
        };
        let mut counter = None;
        render_item(
            &mut w,
            &doc,
            src,
            *task,
            &doc.blocks[0].children[0].children,
            BlockCtx {
                list_depth: 1,
                ..fresh_ctx()
            },
            doc.blocks[0].children[0].src.start,
            &mut counter,
        );
        let rendered: Vec<String> = w
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            rendered.iter().any(|l| l.contains("[ ] outer")),
            "rendered lines: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|l| l.contains("[ ] nested")),
            "rendered lines: {rendered:?}"
        );
    }

    /// Regression for `Writer::pending_para_start`'s own bug (see that field's own doc comment): a
    /// paragraph whose own first rendered content is a lifted math expression must render to *exactly*
    /// its own placeholder line(s) — never preceded by an orphaned, permanently-empty blank row that
    /// nothing ever fills (the old, eager `render_paragraph_math` unconditionally opened one, assuming
    /// the paragraph's own first content would always reach `w.lines` via `append_span`, which a lift
    /// never does — see `render_math_slot`'s own `self.lines.extend(..)`).
    ///
    /// Two shapes, both confirmed directly against the real production pipeline (the legacy,
    /// now-deleted parity harness, before this fix, listed all three `code_span_corpus` math cases as
    /// a known mismatch for exactly this reason):
    ///
    /// * Math as the *whole* document — nothing precedes it, so `w.pending_block_gap` is `false` on entry
    ///   and only the eager code's own *unconditional* second push would have fired.
    /// * Math as a paragraph immediately following an indented code block (`code_span_corpus`'s own
    ///   `"display math ($$...$$) inside an indented code block and outside"` case, `"    $$x$$\n\n$$y$$\n"`)
    ///   — `w.pending_block_gap` is `true` on entry here, so the eager code's *conditional* leading push
    ///   would have fired too, on top of the unconditional one (two orphaned blank lines, not one).
    ///
    /// Reverting `render_paragraph_math`/`Writer::open_pending_para_start`/`drop_pending_para_start`
    /// back to the pre-fix eager two-part push makes both assertions below fail: the first case renders
    /// two lines (`[Line::default(), "$$ y $$".dim()]`) instead of one, and the second renders six lines
    /// (two orphaned `Line::default()`s ahead of the math line) instead of four.
    #[test]
    fn math_lift_that_is_a_paragraphs_entire_content_gets_no_leading_blank_line() {
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let width = 80;

        let src = "$$y$$\n";
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            width,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            true,
        );
        assert!(out.unsupported.is_empty(), "src: {src:?}");
        assert_eq!(
            out.lines,
            vec![Line::from(Span::from("$$ y $$").dim())],
            "a paragraph whose entire content is lifted math must render to exactly its own \
             placeholder line — no leading blank row ahead of it: {:?}",
            out.lines
        );

        let src = "    $$x$$\n\n$$y$$\n";
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            width,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            true,
        );
        assert!(out.unsupported.is_empty(), "src: {src:?}");
        assert_eq!(
            out.lines.len(),
            4,
            "expected exactly [header, body, padding, math] — no orphaned blank line(s) between the \
             code block's own padding row and the math placeholder: {:?}",
            out.lines
        );
        assert_eq!(
            out.lines.last(),
            Some(&Line::from(Span::from("$$ y $$").dim())),
            "the math placeholder must be the render's own last line: {:?}",
            out.lines
        );
    }

    /// Regression for `backslash_math_opener`'s own gap-detection bug (see `walk_inline_math`'s own doc
    /// comment, "`block_start`: letting a backslash-math opener be the walk's own very first event"): a
    /// backslash-delimited math expression (`\(…\)`/`\[…\]`) that is a paragraph's own very first inline
    /// event must still be detected and lifted, not fall through as literal, unlifted text (`"(y)"`/
    /// `"[y]"`) for lack of a *previous* event, within that paragraph's own local walk, to compute a gap
    /// from. Confirmed directly against the real production pipeline before this fix (the legacy,
    /// now-deleted parity harness carried two backslash-math entries as a known mismatch for exactly
    /// this reason).
    ///
    /// Reverting `walk_inline_math`'s own `block_start` seeding back to the pre-fix `prev_end: None`
    /// makes both assertions below fail: each source renders its own escaped bracket/paren as four
    /// plain text spans (`["(", "y", "(y" ...]`-shaped — see `backslash_math_opener`'s own doc comment
    /// for the exact shape a failed detection falls back to) instead of one dimmed math placeholder.
    #[test]
    fn backslash_math_opener_detects_a_paragraphs_own_first_event() {
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let width = 80;

        for (src, expected_dim) in [("\\(y\\)\n", "$y$"), ("\\[y\\]\n", "$$ y $$")] {
            let doc = Doc::parse(src);
            let out = render_doc(
                &doc,
                src,
                width,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "Enter: full screen",
                true,
                &math_slot,
                true,
            );
            assert!(out.unsupported.is_empty(), "src: {src:?}");
            assert_eq!(
                out.lines,
                vec![Line::from(Span::from(expected_dim).dim())],
                "backslash-delimited math that is a paragraph's own very first event must still be \
                 detected and lifted, not fall through as literal text — src: {src:?}, lines: {:?}",
                out.lines
            );
        }
    }

    /// Direct assertion on `render_doc`'s own output for a GitHub alert — header (bar, icon, color,
    /// label with its Obsidian-style title) and a **recursively rendered** body (a tight list, not a
    /// literal string), every body line carrying the alert's own colored bar.
    #[test]
    fn alert_header_bar_icon_label_and_recursive_body_render_from_the_model() {
        crate::preview::markdown::set_details_open(Vec::new());
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let src = "> [!TIP] Hey\n> - a\n> - b\n";
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            true, // icons on, so the header's own Nerd Font glyph is checked too
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let color = Color::Green; // AlertKind::Tip's own color
        assert_eq!(
            out.lines.first(),
            Some(&Line::from(vec![
                Span::styled("▌ ".to_string(), Style::new().fg(color)),
                Span::styled("\u{f0eb} ".to_string(), Style::new().fg(color)),
                Span::styled(
                    "Tip — Hey".to_string(),
                    Style::new().fg(color).add_modifier(Modifier::BOLD)
                ),
            ])),
            "header line (bar/icon/color/label+title): {:?}",
            out.lines
        );
        assert_eq!(
            out.lines.len(),
            3,
            "header + two recursively rendered list-item body lines: {:?}",
            out.lines
        );
        let bar = Span::styled("▌ ".to_string(), Style::new().fg(color));
        for (i, want_tail) in [(1, "- a"), (2, "- b")] {
            assert_eq!(
                out.lines[i].spans.first(),
                Some(&bar),
                "line {i} does not open with the alert's own bar: {:?}",
                out.lines[i]
            );
            let joined: String = out.lines[i]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(
                joined,
                format!("▌ {want_tail}"),
                "line {i}: the body's own list item was not recursively rendered: {:?}",
                out.lines[i]
            );
        }
    }

    /// Direct assertion on `render_doc`'s own output for a `Details` block — marker line (`▾`/`▸`,
    /// the Tab-focus sentinel style, the summary label), and (when open) a **recursively rendered**
    /// body, every body line carrying the `▏` bar in `TABLE_BORDER_FG`. Mirrors the alert test above
    /// (see its own doc comment for why direct assertions here, not just the parity harness, matter
    /// for the *shared* decoration functions).
    #[test]
    fn details_marker_style_bar_and_recursive_body_render_from_the_model() {
        crate::preview::markdown::set_details_open(Vec::new());
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let src = "<details open>\n<summary>More</summary>\n\n- x\n- y\n\n</details>\n";
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        // The sentinel style is hardcoded here, deliberately, rather than obtained by calling
        // `super::details_marker_style()` — that would make this assertion compare the shared
        // function's own output against itself, silently passing even if that function's own
        // color/modifiers were wrong (confirmed directly: this test still passed when
        // `details_marker_style`'s own color was corrupted to `Magenta`, before this fix — the
        // exact "shared code, both sides identical" blind spot this test otherwise exists to close).
        let sentinel_style = Style::new()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::ITALIC);
        assert_eq!(
            out.lines.first(),
            Some(&Line::from(vec![
                Span::styled("▾ ".to_string(), sentinel_style),
                Span::styled(
                    "More".to_string(),
                    Style::new().add_modifier(Modifier::BOLD)
                ),
            ])),
            "marker line (arrow/sentinel style/summary label): {:?}",
            out.lines
        );
        assert_eq!(
            out.lines.len(),
            3,
            "marker + two recursively rendered list-item body lines: {:?}",
            out.lines
        );
        let bar = Span::styled("▏ ".to_string(), Style::new().fg(Color::Rgb(90, 98, 120)));
        for (i, want_tail) in [(1, "- x"), (2, "- y")] {
            assert_eq!(
                out.lines[i].spans.first(),
                Some(&bar),
                "line {i} does not open with the details block's own bar: {:?}",
                out.lines[i]
            );
            let joined: String = out.lines[i]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(
                joined,
                format!("▏ {want_tail}"),
                "line {i}: the body's own list item was not recursively rendered: {:?}",
                out.lines[i]
            );
        }
    }

    // ---- HTML `<table>` (`render_html_table_from_model`) ----

    /// Renders `src` through the whole top-level dispatch, at `width`, and returns each line's own
    /// visible text — the shorthand every HTML-table test below reaches for. `icons` is off so a
    /// link's own Nerd Font prefix does not shift the column widths these tests read.
    fn html_table_lines(src: &str, width: u16) -> Vec<String> {
        crate::preview::markdown::set_details_open(Vec::new());
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            width,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        out.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// The inner width of every column of the drawn box, read off its top border rule
    /// (`"┌───┬──┐"` -> `[3, 2]`). Each number counts the column's own content width **plus** the one
    /// space of padding on either side, so a one-cell-wide column reads as `3`.
    fn column_rule_widths(lines: &[String]) -> Vec<usize> {
        let top = lines
            .iter()
            .find(|l| l.starts_with('┌'))
            .unwrap_or_else(|| panic!("no table was drawn at all: {lines:?}"));
        top.trim_start_matches('┌')
            .trim_end_matches('┐')
            .split('┬')
            .map(|seg| seg.chars().count())
            .collect()
    }

    /// A column whose cells are all empty is still **one cell wide** — `render_table_cells` seeds
    /// every column at `1`, not at `0`, before widening it to fit its content.
    ///
    /// A zero-wide column draws as `│  │`: two spaces of padding with nothing between them, a box
    /// whose column is visibly narrower than every other one and which no source ever asks for. The
    /// floor is also what the shaving loop stops at (`if mw <= 1 { break }`), so seeding at `0` would
    /// additionally let a squeezed table shave a column below the width that loop refuses to go under.
    ///
    /// Why this needed a test of its own: exactly one corpus case in the tree
    /// (`html_table_corpus: "empty cells"`) has an all-empty column, so dropping the floor moved
    /// three characters in the goldens and nothing anywhere else went red. The cases below are that
    /// shape spread across its type — the empty column first, in the middle, alone, every cell in the
    /// table empty, a `<th>` rather than a `<td>`, and the GFM path as well as the HTML one, since
    /// the seeding is shared code both reach.
    #[test]
    fn every_column_is_at_least_one_cell_wide_however_empty_its_cells_are() {
        for (why, src, want) in [
            (
                "an empty cell in the first column",
                "<table>\n<tr><td></td><td>b</td></tr>\n</table>\n",
                vec!["┌───┬───┐", "│   │ b │", "└───┴───┘"],
            ),
            (
                "every cell in the table empty",
                "<table>\n<tr><td></td><td></td></tr>\n</table>\n",
                vec!["┌───┬───┐", "│   │   │", "└───┴───┘"],
            ),
            (
                "an empty column between two that have content",
                "<table>\n<tr><td>a</td><td></td><td>c</td></tr>\n</table>\n",
                vec!["┌───┬───┬───┐", "│ a │   │ c │", "└───┴───┴───┘"],
            ),
            (
                "a lone empty header cell",
                "<table>\n<tr><th></th></tr>\n</table>\n",
                // The lone `<th>` row is a header, so the divider rule follows it.
                vec!["┌───┐", "│   │", "├───┤", "└───┘"],
            ),
            (
                "the same, through the GFM table path",
                "| a |  | c |\n|---|---|---|\n| 1 |  | 3 |\n",
                vec![
                    "┌───┬───┬───┐",
                    "│ a │   │ c │",
                    "├───┼───┼───┤",
                    "│ 1 │   │ 3 │",
                    "└───┴───┴───┘",
                ],
            ),
            (
                "a GFM table with nothing in it at all",
                "|   |\n|---|\n|   |\n",
                vec!["┌───┐", "│   │", "├───┤", "│   │", "└───┘"],
            ),
        ] {
            let lines = html_table_lines(src, 40);
            assert_eq!(lines, want, "{why}");
            assert!(
                column_rule_widths(&lines).iter().all(|&w| w >= 3),
                "{why}: 内容 1 桁 + 左右のパディングに満たない列がある: {:?}",
                column_rule_widths(&lines)
            );
        }
    }

    /// A ragged table draws as many columns as its **widest** row has — every row is padded out to
    /// that count, and no cell anywhere is dropped.
    ///
    /// `render_table_cells` takes the column count as the `max` over the rows' own lengths precisely
    /// so that the short rows are the ones adjusted. Taking the `min` instead silently deletes every
    /// cell past the shortest row's length — an author's real content, gone from the screen with no
    /// sign that anything was cut — and it does so *only* for a ragged table, which is exactly the
    /// shape a hand-written HTML table (or a `colspan`, which this renderer draws as one plain cell)
    /// keeps producing.
    ///
    /// Why this needed a test of its own: three corpus cases have the shape, and all three of them
    /// are goldens-only — dropping cells shrank three snapshot blocks and nothing else in the tree
    /// noticed. The cases below cover the type: the long row first, the long row last, a `colspan`
    /// and a `rowspan` (the two ways real markup produces a ragged table without meaning to), and the
    /// GFM path, where a body row shorter than the header row is the ordinary way to write one.
    #[test]
    fn a_ragged_table_draws_as_many_columns_as_its_widest_row_has() {
        for (why, src, want) in [
            (
                "the widest row first",
                "<table>\n<tr><td>a</td><td>b</td><td>c</td></tr>\n<tr><td>d</td></tr>\n</table>\n",
                vec!["┌───┬───┬───┐", "│ a │ b │ c │", "│ d │   │   │", "└───┴───┴───┘"],
            ),
            (
                "the widest row last",
                "<table>\n<tr><td>d</td></tr>\n<tr><td>a</td><td>b</td><td>c</td></tr>\n</table>\n",
                vec!["┌───┬───┬───┐", "│ d │   │   │", "│ a │ b │ c │", "└───┴───┴───┘"],
            ),
            (
                "a colspan cell, drawn as one plain cell in a row of its own",
                "<table>\n<tr><td colspan=\"2\">spanning</td></tr>\n<tr><td>a</td><td>b</td></tr>\n</table>\n",
                vec![
                    "┌──────────┬───┐",
                    "│ spanning │   │",
                    "│ a        │ b │",
                    "└──────────┴───┘",
                ],
            ),
            (
                "a body row shorter than the header row, through the GFM path",
                "| a | b | c |\n|---|---|---|\n| d |\n",
                vec![
                    "┌───┬───┬───┐",
                    "│ a │ b │ c │",
                    "├───┼───┼───┤",
                    "│ d │   │   │",
                    "└───┴───┴───┘",
                ],
            ),
        ] {
            let lines = html_table_lines(src, 40);
            assert_eq!(lines, want, "{why}");
        }
    }

    /// The shape this feature exists for: the README screenshot grid comes out as a real 2x2 box-drawn
    /// table, not two loose images followed by two loose caption lines.
    #[test]
    fn html_table_renders_a_box_drawn_grid() {
        let src = "<table>\n  <tr>\n    <td><img src=\"a.png\" alt=\"A\"></td>\n    <td><img src=\"b.png\" alt=\"B\"></td>\n  </tr>\n  <tr>\n    <td align=\"center\"><b>One</b></td>\n    <td align=\"center\"><b>Two</b></td>\n  </tr>\n</table>\n";
        let lines = html_table_lines(src, 60);
        assert_eq!(lines.len(), 4, "top rule, two rows, bottom rule: {lines:?}");
        assert!(
            lines[0].starts_with('┌') && lines[0].ends_with('┐'),
            "{lines:?}"
        );
        assert!(
            lines[3].starts_with('└') && lines[3].ends_with('┘'),
            "{lines:?}"
        );
        assert!(
            lines[1].contains("🖼 A") && lines[1].contains("🖼 B"),
            "{lines:?}"
        );
        assert!(
            lines[2].contains("One") && lines[2].contains("Two"),
            "{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("<td") || l.contains("<img")),
            "no raw tag survives: {lines:?}"
        );
    }

    /// A cell's own `align=` wins over its column, per cell — the requirement a "round the whole
    /// column to one alignment" shortcut would have failed. Checked by padding, since that is the
    /// only observable difference: the same column holds a left-, a centered- and a right-aligned
    /// cell at once.
    #[test]
    fn html_table_cell_align_beats_the_column_one_cell_at_a_time() {
        let src = "<table>\n<tr><td align=\"left\">x</td></tr>\n<tr><td align=\"center\">x</td></tr>\n<tr><td align=\"right\">x</td></tr>\n<tr><td>wwwwwwwww</td></tr>\n</table>\n";
        let lines = html_table_lines(src, 60);
        // Column is 9 wide (`wwwwwwwww`), each cell padded to ` <9 cols> `.
        assert_eq!(lines[1], "│ x         │", "left: {lines:?}");
        assert_eq!(lines[2], "│     x     │", "center: {lines:?}");
        assert_eq!(lines[3], "│         x │", "right: {lines:?}");
        assert_eq!(
            lines[4], "│ wwwwwwwww │",
            "unspecified falls back to the column: {lines:?}"
        );
    }

    /// A leading run of all-`<th>` rows becomes real header rows — the horizontal rule sits under the
    /// *last* of them, not under each.
    #[test]
    fn html_table_leading_th_rows_get_one_header_rule() {
        let one = html_table_lines(
            "<table>\n<tr><th>H</th></tr>\n<tr><td>a</td></tr>\n<tr><td>b</td></tr>\n</table>\n",
            40,
        );
        assert!(
            one[2].starts_with('├'),
            "rule under the single header row: {one:?}"
        );
        let two = html_table_lines(
            "<table>\n<tr><th>H</th></tr>\n<tr><th>I</th></tr>\n<tr><td>a</td></tr>\n</table>\n",
            40,
        );
        assert!(
            two[3].starts_with('├'),
            "one rule, under the second header row: {two:?}"
        );
        assert_eq!(
            two.iter().filter(|l| l.starts_with('├')).count(),
            1,
            "exactly one header rule: {two:?}"
        );
    }

    /// A `<th>` that is *not* part of a leading all-`<th>` row cannot be a header *row* (a rule spans
    /// the whole width), so it is drawn as that one cell's own header styling instead — and draws no
    /// rule at all.
    #[test]
    fn html_table_stray_th_is_styled_without_a_header_rule() {
        let src =
            "<table>\n<tr><td>a</td><th>b</th></tr>\n<tr><td>c</td><td>d</td></tr>\n</table>\n";
        crate::preview::markdown::set_details_open(Vec::new());
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            40,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        let text: Vec<String> = out
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            !text.iter().any(|l| l.starts_with('├')),
            "a stray <th> draws no header rule: {text:?}"
        );
        let bold_texts: Vec<String> = out.lines[1]
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            bold_texts.iter().any(|t| t == "b"),
            "the <th> cell itself is bold: {bold_texts:?}"
        );
        assert!(
            !bold_texts.iter().any(|t| t == "a"),
            "its <td> neighbour is not: {bold_texts:?}"
        );
    }

    /// A quote-nested table draws inside the quote's own bar, with its cell text free of the `>`
    /// markers `Block::src` still carries — the reason cells are read back through `body_spans`.
    #[test]
    fn html_table_inside_a_quote_draws_inside_the_quote_bar() {
        let src = "> <table>\n> <tr><td>\n> first\n> second\n> </td></tr>\n> </table>\n";
        let lines = html_table_lines(src, 60);
        assert!(
            lines.iter().all(|l| l.starts_with("> ")),
            "every line stays inside the quote: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("first second")),
            "the multi-line cell reads without quote markers: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l[2..].contains('>')),
            "no stray quote marker inside a cell: {lines:?}"
        );
    }

    /// A GFM table in the same document is untouched by any of this — its columns still take their
    /// alignment from the delimiter row, and no cell overrides anything.
    #[test]
    fn a_gfm_table_still_takes_its_alignment_from_the_delimiter_row() {
        let lines = html_table_lines("| a | b |\n|:--|--:|\n| x | y |\n| wwwww | wwwww |\n", 60);
        assert_eq!(
            lines[3], "│ x     │     y │",
            "left / right columns: {lines:?}"
        );
    }

    /// A **closed** `Details` block (no `open` attribute, and `set_details_open` seeds nothing —
    /// see `render_details_from_model`'s own doc comment for `open`'s two possible sources) renders
    /// *only* its own marker line — its body must not appear at all, the same information-disclosure
    /// concern `markdown.rs`'s own `alert_inside_closed_details_is_not_leaked` test guards for the
    /// string-based pipeline.
    #[test]
    fn details_closed_state_hides_its_body() {
        crate::preview::markdown::set_details_open(Vec::new());
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let src = "<details>\n<summary>S</summary>\n\nSECRET\n\n</details>\n";
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.lines.len(),
            1,
            "a closed details block draws only its own marker line: {:?}",
            out.lines
        );
        let joined: String = out.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            !joined.contains("SECRET"),
            "closed details leaked its body: {joined:?}"
        );
    }

    /// A `Details` block reached only through a GitHub alert's own body renders **statically** — it
    /// does not consume a slot from the document-wide Tab-toggle ordinal sequence
    /// (`super::next_details_open`), so a *real*, top-level `Details` block that follows it still
    /// gets the ordinal slot `set_details_open` actually meant for it. Mirrors
    /// `markdown.rs`'s own `nested_details_do_not_drift_later_block_open_state` test, this time
    /// checking `render_doc`'s own output rather than `collect_details_open`'s count.
    #[test]
    fn details_nested_inside_an_alert_does_not_consume_the_top_level_ordinal() {
        // Slot 0 is the *only* real top-level `Details` block below (the alert's own nested one is
        // not supposed to be counted at all) — seeded closed, opposite its own `open` attribute, so
        // a wrongly-consumed slot would be directly observable as the wrong body showing up.
        crate::preview::markdown::set_details_open(vec![false]);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let src = "> [!NOTE]\n> <details open>\n> <summary>Nested</summary>\n>\n> shown\n>\n> </details>\n\n\
                   <details open>\n<summary>Real</summary>\n\nSECRET\n\n</details>\n";
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let joined_all: Vec<String> = out
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            joined_all.iter().any(|l| l.contains("shown")),
            "the alert-nested details is rendered statically, honoring its own `open` attribute \
             directly (never consuming an ordinal slot) — it must still show its body: {joined_all:?}"
        );
        assert!(
            !joined_all.iter().any(|l| l.contains("SECRET")),
            "the one real top-level details block must land on ordinal slot 0 (seeded closed) — \
             if the alert-nested one had wrongly consumed that slot instead, this block would fall \
             back to its own `open` attribute (`true`) and leak SECRET: {joined_all:?}"
        );
    }

    // ---- Table/HTML/image/mermaid — direct assertions on render_doc's own output -----------------
    //
    // Mirrors the alert/details tests above: checked here, directly, because `render_table_cells`/
    // `render_html_block` are *shared*, `markdown.rs`-hosted helpers (reused, not reimplemented —
    // see `render_table_from_model`'s own doc comment) — a regression in either would break every
    // caller identically. These tests pin known-good output directly, independent of anything else,
    // so they can actually catch such a regression.

    use super::super::TABLE_BORDER_FG;

    /// Table structure straight from the model: border characters, the header row's own
    /// cyan+bold style, and per-column alignment (`:---` left / `:---:` center / `---:` right) — a
    /// three-column table whose body row (`"1"`/`"2"`/`"3"`) is narrower than its own header
    /// (`"a"`/`"bb"`/`"ccc"`) in the center/right columns specifically, so the padding distribution
    /// is actually observable (a column with no padding at all would pass this test even with the
    /// alignment logic entirely broken).
    #[test]
    fn table_renders_borders_header_style_and_column_alignment_from_the_model() {
        let src = "| a | bb | ccc |\n|:---|:---:|---:|\n| 1 | 2 | 3 |\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let border = Style::new().fg(TABLE_BORDER_FG);
        let head = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
        assert_eq!(
            out.lines,
            vec![
                Line::from(Span::styled("┌───┬────┬─────┐", border)),
                Line::from(vec![
                    Span::styled("│", border),
                    Span::styled(" ", head),
                    Span::styled("a", head),
                    Span::styled(" ", head),
                    Span::styled("│", border),
                    Span::styled(" ", head),
                    Span::styled("bb", head),
                    Span::styled(" ", head),
                    Span::styled("│", border),
                    Span::styled(" ", head),
                    Span::styled("ccc", head),
                    Span::styled(" ", head),
                    Span::styled("│", border),
                ]),
                Line::from(Span::styled("├───┼────┼─────┤", border)),
                Line::from(vec![
                    Span::styled("│", border),
                    Span::raw(" "),
                    Span::raw("1"),
                    Span::raw(" "),
                    Span::styled("│", border),
                    Span::raw(" "), // left half of "2"'s own center padding (pad=1, lp=0)
                    Span::raw("2"),
                    Span::raw("  "), // right half (rp=1) + the cell's own trailing space
                    Span::styled("│", border),
                    Span::raw("   "), // "3"'s own right-align padding (pad=2, lp=2) + leading space
                    Span::raw("3"),
                    Span::raw(" "),
                    Span::styled("│", border),
                ]),
                Line::from(Span::styled("└───┴────┴─────┘", border)),
            ],
            "table structure (borders/header style/alignment) did not match: {:?}",
            out.lines
        );
    }

    /// A `Table` nested inside a `List` item renders correctly (list indentation never breaks
    /// `render_table_from_model`'s own model-based parsing — see that function's own doc comment on
    /// why list nesting is safe but quote nesting is not) — the *other* half of
    /// `contains_unsupported`'s own `in_quote` distinction, alongside the quote-nested case below.
    #[test]
    fn table_nested_in_a_list_item_renders_correctly() {
        let src = "- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let has_border = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.as_ref() == "┌───┬───┐"));
        assert!(
            has_border,
            "expected a real, box-drawn table: {:?}",
            out.lines
        );
        let has_row = out.lines.iter().any(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined.contains('1') && joined.contains('2')
        });
        assert!(
            has_row,
            "expected the table's own data row: {:?}",
            out.lines
        );
    }

    /// Neither a `Table` nor an `Html` block nested inside a `Quote` is reported unsupported any
    /// more — `contains_unsupported` is a permanent no-op now (see its own doc comment). This test
    /// used to pin the opposite for `Table` (`quote_nested_table_and_html_are_reported_unsupported`,
    /// then `quote_nested_table_is_reported_unsupported_html_is_not` once `Html`'s own instance of
    /// the same defect closed first — see either name's own git history for that intermediate state)
    /// — kept, renamed and re-pointed at `None` for both, as the safety net for *this* fix: a
    /// regression that quietly started reporting a quote-nested `Table` unsupported again would show
    /// up as a silently skipped subtree, not as a clean failure here.
    #[test]
    fn quote_nested_table_and_html_are_neither_reported_unsupported() {
        let table_src = "> | a | b |\n> |---|---|\n> | 1 | 2 |\n";
        let doc = Doc::parse(table_src);
        assert_eq!(
            contains_unsupported(&doc.blocks[0].children, true),
            None,
            "src: {table_src:?}"
        );
        let html_src = "> <div>x</div>\n> more\n";
        let doc = Doc::parse(html_src);
        assert_eq!(
            contains_unsupported(&doc.blocks[0].children, true),
            None,
            "src: {html_src:?}"
        );
    }

    /// An `Html` block's tags are stripped, comments dropped, and entities decoded straight from the
    /// model (`render_html_block`, reused verbatim) — a nested `<b>` disappears (not merely its own
    /// tags: `render_html_block`'s own scan has no concept of nested structure at all, everything
    /// between `<` and `>` is dropped uniformly), and the trailing blank line
    /// `render_html_block_from_model` never adds a second one of its own on top of.
    ///
    /// A **quote-nested** `Html` block used to be a total content loss: the pre-`body_spans` legacy
    /// fallback (the legacy renderer's own `Event::Html` handling — see `model::BlockKind::Html.body_spans`'s
    /// own doc comment for the confirmed dump this pins) simply could not draw this shape at all, and
    /// `contains_unsupported` used to fall the whole enclosing `Quote` back to it whenever an `Html`
    /// block showed up nested inside one — so the block's content vanished from the screen outright,
    /// not merely mis-rendered. `body_spans` is what lets this pass draw it directly instead: pins
    /// both halves of the fix — `unsupported` stays empty (no more legacy fallback for this shape),
    /// and the block's own text (`HTML-INSIDE-QUOTE`) actually reaches the output, `>`-quoted like any
    /// other content inside this `Quote`.
    #[test]
    fn html_block_nested_inside_a_quote_renders_its_content_instead_of_vanishing() {
        let src = "> <div align=\"center\">\n> HTML-INSIDE-QUOTE\n> </div>\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let rendered: Vec<String> = out
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // Exact match, not a mere `.contains("HTML-INSIDE-QUOTE")` substring check: a naive
        // `&src[block_src]` slice (this test's own regression target — see `render_html_block_from_model`'s
        // own doc comment) still leaves that substring *somewhere* in the output — it doubles the `>`
        // marker onto a second, stray copy of the quote prefix (`"> > HTML-INSIDE-QUOTE"`, confirmed
        // directly by temporarily reverting the fix and running this exact test) rather than dropping
        // the text outright — so only an exact-line assertion, not a substring one, actually catches
        // that regression.
        assert_eq!(
            rendered,
            vec!["> HTML-INSIDE-QUOTE".to_string(), "> ".to_string()],
            "the quote-nested HTML block did not render cleanly (no doubled `>` marker, no dropped \
             content): {rendered:#?}"
        );
    }

    /// The HTML `<img>` path (`render_html_block_from_model`) is routed through the identical
    /// `render_image_group` packer a Markdown `![alt](url)` badge row goes through — design point 5
    /// of the row-packing change (see `render_html_block_from_model`'s own doc comment for exactly how
    /// it batches a *run* of consecutive `<img>` lines). Three bare `<img>` tags with no per-badge
    /// `<a>` wrapper between them (unlike the real-README shape every other HTML-image test/corpus
    /// case here uses) are the one shape where that batching is actually observable: nothing but
    /// whitespace-only lines separates them, so all three should land on the identical `line`,
    /// packed left to right, exactly like the Markdown badge-row tests above.
    #[test]
    fn html_img_tags_with_no_intervening_tag_lines_pack_onto_one_row() {
        let src = "<div>\n<img src=\"a.svg\" alt=\"a\">\n<img src=\"b.svg\" alt=\"b\">\n\
                   <img src=\"c.svg\" alt=\"c\">\n</div>\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 5, rows: 1 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let lines: Vec<usize> = out.images.iter().map(|p| p.line).collect();
        assert_eq!(
            lines,
            vec![0, 0, 0],
            "three bare <img> lines with nothing but blank tag-only content between them must pack \
             onto one shared row via the same `render_image_group` a Markdown badge row uses — not \
             three stacked rows: {:?}",
            out.images
        );
        let cols: Vec<u16> = out.images.iter().map(|p| p.col).collect();
        assert_eq!(
            cols,
            vec![11, 17, 23],
            "cols must increase strictly left to right, same as the Markdown-only badge-row tests: \
             {:?}",
            out.images
        );
    }

    #[test]
    fn html_block_strips_tags_drops_comments_and_decodes_entities_from_the_model() {
        let src = "<div class=\"x\">\n<!-- hidden -->\nHello <b>world</b> &amp; friends\n</div>\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.lines,
            vec![Line::from("Hello world & friends"), Line::from("")],
            "tag-stripped/comment-dropped/entity-decoded output did not match: {:?}",
            out.lines
        );
    }

    /// A standalone block image (`![alt](url)` as a paragraph's *entire* content) renders through
    /// `slot_of` exactly like production's own `BlockPart::Image` handling — an `Inline` slot reserves
    /// rows and records an `ImagePlacement`; this also pins `ImagePlacement.cols`/`.rows` verbatim
    /// from whatever `slot_of` answered (items 5/6 of this pass's own detection-power check: a wrong
    /// `cols`/`rows` here is exactly what a real preview overlay would size incorrectly on screen).
    ///
    /// `math_on: false` is deliberate, not an oversight: production's own image extraction is never
    /// gated by whether math lifting is on (`split_block_parts` runs unconditionally) — see
    /// `BlockCtx.extract_here`'s own doc comment for the real, confirmed bug this pins a regression
    /// test for (an earlier version of this pass reused `ctx.math_here` for image eligibility too,
    /// which left image extraction silently disabled whenever `math_on` was `false`).
    #[test]
    fn standalone_block_image_renders_and_is_not_gated_by_math_on() {
        let src = "![a cat](cat.png)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 7, rows: 3 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false, // math_on: false — extraction must still fire (see this test's own doc comment)
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.images,
            vec![ImagePlacement {
                url: "cat.png".to_string(),
                alt: "a cat".to_string(),
                line: 0,
                col: 16, // (width 40 - cols 7) / 2, the single-image group centering formula
                cols: 7,
                rows: 3,
                fence_ord: None,
            }],
            "images: {:?}",
            out.images
        );
        assert_eq!(
            out.lines.len(),
            3,
            "3 reserved rows for rows: 3: {:?}",
            out.lines
        );
    }

    /// `ImagePlacement.line` accounts for `decorate_headings_and_extras`'s own H1/H2 rule-line
    /// insertion — a heading *before* the image shifts every later line index by one per level-1/2
    /// heading, but that shift only happens once, at the very end of `render_doc` (see
    /// `Writer.heading_rule_shift`'s own doc comment for why a placement recorded mid-walk cannot see
    /// it directly). Two headings (one H1, one H2) precede the image here specifically so a
    /// off-by-one in the shift's own accumulation (counting only one of the two, say) is
    /// distinguishable from counting neither or both correctly.
    #[test]
    fn block_image_placement_line_accounts_for_heading_decoration_shift() {
        let src = "# One\n\n## Two\n\npara\n\n![a](u.png)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 4, rows: 2 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        let placement = &out.images[0];
        // `out.lines[placement.line]` must be the image's own reserved placeholder row (the one
        // carrying its "🖼 alt" label — `image_placeholder_lines`'s own first line) — not merely *a*
        // line somewhere nearby: if `heading_rule_shift` under- or over-counts by even one (missing
        // one of the two preceding headings' own rule lines, say), this indexes the wrong row
        // entirely (one short would land on "para" or a rule line; one over would land on a blank
        // row past the image), and this assertion catches either directly.
        let at_placement = out.lines.get(placement.line).map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        });
        assert_eq!(
            at_placement.as_deref().map(str::trim),
            Some("🖼 a"),
            "ImagePlacement.line ({}) must index the image's own reserved row in the fully \
             decorated `out.lines` — two headings (one H1, one H2) precede it, each contributing its \
             own rule line: {:?}",
            placement.line,
            out.lines
        );
        // And the row immediately before it must be the paragraph's own trailing blank separator,
        // not a stray rule line the shift double-counted.
        assert_eq!(
            out.lines.get(placement.line - 1),
            Some(&Line::from("para")),
            "the line right before the image placement must be \"para\", the real paragraph \
             immediately preceding it in the source — a wrong shift would land here instead: {:?}",
            out.lines
        );
    }

    /// Regression for a real, confirmed content-loss bug — the `app::md_real_file_sweep_tests` sweep
    /// over every `.md` file under `~/.cargo/registry/src` found hundreds of real crate READMEs (the
    /// shields.io "badge row" idiom: one `[![alt](url)](href)` badge per physical line, no blank line
    /// between consecutive badges — a single Markdown paragraph, several images long) silently losing
    /// every badge's own URL past the first one, before `paragraph_as_block_images`'s own `image_starts
    /// != 1` count check existed (see that function's own doc comment for the exact, confirmed
    /// mechanism: checking only the first/last event of a naively "outer-link-unwrapped" range treated
    /// two entirely separate badges sharing one paragraph as if they were one, nested one — rendering a
    /// single, wrong, merged line with both alt texts concatenated and only the *first* image's own
    /// URL kept). Two badges here, each independently link-wrapped, one per line — the minimal shape
    /// that reproduces it — must extract as **two separate placements**, each with its own real URL,
    /// not as one merged placement or one placement with the second badge's own information dropped.
    ///
    /// Both placements share `line: 0` here — `render_image_group`'s own row-packing (added after this
    /// test was first written; see that function's own doc comment) lays every image `paragraph_as_\
    /// block_images` returns for one paragraph onto shared rows, left to right, exactly the way GitHub/
    /// crates.io render this same badge-row idiom. `col` (14, then 14 + 5 + 1 gap = 20) is the other
    /// half of that fix: the two placements no longer just avoid data loss, they no longer stack
    /// vertically either.
    #[test]
    fn consecutive_badge_lines_extract_every_image_not_just_the_first() {
        let src = "[![Build Status](build.svg)](https://ci.example/build)\n\
                   [![Docs](docs.svg)](https://docs.example)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 5, rows: 1 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.images,
            vec![
                ImagePlacement {
                    url: "build.svg".to_string(),
                    alt: "Build Status".to_string(),
                    line: 0,
                    col: 14, // (width 40 - (cols 5 + gap 1 + cols 5)) / 2
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
                ImagePlacement {
                    url: "docs.svg".to_string(),
                    alt: "Docs".to_string(),
                    line: 0,
                    col: 20, // 14 + cols 5 + 1-cell gap
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
            ],
            "both badges must extract as separate placements, each with its own URL, packed onto the \
             same row left to right — not one merged placement missing the second badge's own URL, \
             and not stacked one row below the other: {:?}",
            out.images
        );
    }

    /// The `image_starts != 1` count check's own, narrower target directly, isolated from
    /// `paragraph_as_block_images`'s own per-physical-line segmentation (the sibling test above puts
    /// its two badges on *separate* lines — each already lands in its own segment before the count
    /// check is ever consulted, so that test alone does not exercise this specific guard): two badges
    /// on the *same* physical line (a bare space between them, no `SoftBreak` at all — confirmed
    /// against real content too, `~/.cargo/registry/.../phf-*/README.md`'s own two-badge row) share
    /// one segment, with two `Start(Image)`/`End(Image)` pairs inside it.
    ///
    /// `segment_as_block_images`/`split_top_level_image_units` (added for `md_real_file_sweep_\
    /// tests`'s own real-registry sweep) now recognizes this exact shape and extracts **both**
    /// badges correctly, as two separate placements, the same way `consecutive_badge_lines_extract_\
    /// every_image_not_just_the_first` above already establishes for the *separate*-line case — not
    /// "must not be extracted at all," this test's own earlier version (see `git log` for the
    /// version this replaced): that version deliberately rejected the whole segment and fell through
    /// to ordinary rendering, a safe-but-lossy degrade written specifically to avoid a real, worse
    /// bug this function's own pre-`md-block-walk` version had — reverting the count check back to
    /// "only look at the first/last event" reproduces it exactly: `events.first()` is badge *a*'s
    /// own `Start(Link)`, `events.last()` happens to be badge *b*'s own `End(Link)`, so the naive
    /// unwrap treats the *whole* two-badge span as one wrapped image — `inner.first()`/`inner.\
    /// last()` then resolve to badge *a*'s `Start(Image)` and badge *b*'s `End(Image)` respectively,
    /// both checks pass, and the result is one wrong, merged placement: alt texts concatenated
    /// (`"a b"`), only badge *a*'s own URL (`u1`) kept, badge *b*'s own URL (`u2`) silently dropped.
    /// The safe-degrade was the best available fix at the time; it is not anymore, now that
    /// `split_top_level_image_units` can tell "two separate units sharing one line" apart from "one
    /// truly ambiguous merge" (still `segment_as_block_image`'s own `image_starts != 1` guard, for a
    /// shape like two images glued inside one *un-split-able* wrapper) precisely.
    ///
    /// Both placements share `line: 0` here (`col` 14/20 — see `render_image_group`'s own doc comment
    /// and `consecutive_badge_lines_extract_every_image_not_just_the_first`'s identical note above,
    /// which computes the same two numbers for the same `cols`/gap/width): row-packing lays every
    /// image one paragraph's own `paragraph_as_block_images` call returns onto shared rows, so two
    /// badges already sharing one physical *source* line now also share one rendered row, not two.
    #[test]
    fn two_images_sharing_one_line_segment_both_extract_as_separate_placements() {
        let src = "[![a](u1)](h1) [![b](u2)](h2)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 5, rows: 1 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.images,
            vec![
                ImagePlacement {
                    url: "u1".to_string(),
                    alt: "a".to_string(),
                    line: 0,
                    col: 14, // (width 40 - (cols 5 + gap 1 + cols 5)) / 2
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
                ImagePlacement {
                    url: "u2".to_string(),
                    alt: "b".to_string(),
                    line: 0,
                    col: 20, // 14 + cols 5 + 1-cell gap
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
            ],
            "both badges sharing one physical line must extract as separate placements, each with \
             its own URL, packed onto the same row left to right — not rejected outright, and not \
             merged into one with the second badge's own information dropped: {:?}",
            out.images
        );
    }

    /// Three badges on one physical source line (bare spaces between them, no `SoftBreak` at all) —
    /// the shape `README.md`'s own "CI · crates.io · License" badge row is written as in the wild.
    /// GitHub/crates.io lay these out horizontally; before `render_image_group`'s own row-packing
    /// existed, konoma stacked each one on its own row instead (the real bug this whole change fixes).
    /// All three must land on the *same* `line`, in left-to-right `col` order, each `col` the previous
    /// one's own `col + cols + 1` (the packer's 1-cell gap) — not three separate `line`s.
    #[test]
    fn three_badges_on_one_line_pack_onto_one_row_left_to_right() {
        let src = "[![a](x.svg)](u1) [![b](y.svg)](u2) [![c](z.svg)](u3)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 5, rows: 1 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.images,
            vec![
                ImagePlacement {
                    url: "x.svg".to_string(),
                    alt: "a".to_string(),
                    line: 0,
                    col: 11, // (width 40 - (5+1+5+1+5)) / 2 = (40-17)/2 = 11
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
                ImagePlacement {
                    url: "y.svg".to_string(),
                    alt: "b".to_string(),
                    line: 0,
                    col: 17, // 11 + 5 + 1
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
                ImagePlacement {
                    url: "z.svg".to_string(),
                    alt: "c".to_string(),
                    line: 0,
                    col: 23, // 17 + 5 + 1
                    cols: 5,
                    rows: 1,
                    fence_ord: None,
                },
            ],
            "three badges sharing one physical source line must pack onto one shared row, left to \
             right — not stack one per row: {:?}",
            out.images
        );
        assert_eq!(
            out.lines.len(),
            1,
            "one shared row for all three badges (rows: 1 each) — three stacked rows would be a \
             regression to the pre-row-packing bug: {:?}",
            out.lines
        );
    }

    /// The README idiom itself: three badges on three *separate* physical source lines with no blank
    /// line between them (a lazy-continuation paragraph, `SoftBreak`-joined) — the exact shape
    /// `consecutive_badge_lines_extract_every_image_not_just_the_first` already covers for two badges;
    /// this pins the same row-packing for three, confirming the fix generalizes past the two-image
    /// case (a group-of-3 walks a different, less-exercised branch of `render_image_group`'s own
    /// width-accumulation loop than a group-of-2 does).
    #[test]
    fn three_badges_on_separate_lines_still_pack_onto_one_row() {
        let src = "[![a](x.svg)](u1)\n[![b](y.svg)](u2)\n[![c](z.svg)](u3)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 5, rows: 1 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let lines: Vec<usize> = out.images.iter().map(|p| p.line).collect();
        assert_eq!(
            lines,
            vec![0, 0, 0],
            "three badges on separate source lines, joined lazily with no blank line, must still \
             pack onto one shared rendered row (the README badge-row idiom): {:?}",
            out.images
        );
        let cols: Vec<u16> = out.images.iter().map(|p| p.col).collect();
        assert_eq!(
            cols,
            vec![11, 17, 23],
            "cols must increase strictly left to right by cols(5) + gap(1) each step: {:?}",
            out.images
        );
    }

    /// A row group that does not fit `w.width` wraps to a fresh row-group instead of overflowing —
    /// the "otherwise flush the pending group and start a new one" rule `render_image_group`'s own
    /// packing loop applies whenever the running width would exceed `w.width`. Four 5-cell-wide
    /// images (each `+ 1` gap) need `5*4 + 3 = 23` cells; at `width: 13` only two fit on a row
    /// (`5+1+5 = 11 <= 13`; a third would need `11+1+5 = 17 > 13`), so this must wrap into two row
    /// groups of two images each (`[a, b]`, then `[c, d]` — the same two-image-per-row capacity
    /// applies again once the second group starts fresh). Assert the exact `line` grouping (not just
    /// "some wrap happened") so a capacity miscount is caught, not just total absence of wrapping.
    #[test]
    fn a_row_group_that_does_not_fit_wraps_to_a_new_line() {
        let src = "[![a](a.svg)](u1) [![b](b.svg)](u2) [![c](c.svg)](u3) [![d](d.svg)](u4)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 5, rows: 1 };
        let out = render_doc(
            &doc,
            src,
            13,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        let lines: Vec<usize> = out.images.iter().map(|p| p.line).collect();
        assert_eq!(
            lines,
            vec![0, 0, 1, 1],
            "at width 13, two 5-cell images (11 <= 13) share row 0; the third no longer fits \
             (11 + 1 + 5 = 17 > 13) and wraps to row 1 along with the fourth: {:?}",
            out.images
        );
        let cols: Vec<u16> = out.images.iter().map(|p| p.col).collect();
        assert_eq!(
            cols,
            vec![1, 7, 1, 7],
            "each row group is independently centered — both rows hold two 5-cell images with a \
             1-cell gap, so both rows compute the identical (13 - 11) / 2 = 1 start column: {:?}",
            out.images
        );
        assert_eq!(
            out.lines.len(),
            2,
            "two reserved rows (one per row group, rows: 1 each) — a group that fit everything onto \
             one row, or overflowed past width without wrapping, would both show up here as a wrong \
             row count: {:?}",
            out.lines
        );
    }

    /// Regression for a second, real, confirmed content-loss bug the same sweep found: a real sentence
    /// directly followed, with no blank line, by a standalone image line
    /// (`~/.cargo/registry/.../zune-jpeg-*/Benches.md`: `"...of (now defunct?) [Cutefish OS] default \
    /// wallpaper.\n![img](benches/images/speed_bench.jpg)\n"`) lost the image's own URL entirely before
    /// `paragraph_trailing_images` existed — the whole paragraph (sentence *and* image line, joined by
    /// `paragraph_as_block_images`'s own "every segment must be an image" gate failing on the sentence)
    /// fell through to ordinary rendering, where the image showed only as inline alt-text with no URL,
    /// squished onto the end of the sentence's own line. Both the sentence's own text *and* the image's
    /// own real placement must survive intact.
    #[test]
    fn sentence_directly_followed_by_a_standalone_image_line_still_extracts_the_image() {
        let src = "Some intro sentence.\n![a cat](cat.png)\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 7, rows: 3 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.images,
            vec![ImagePlacement {
                url: "cat.png".to_string(),
                alt: "a cat".to_string(),
                line: 1,
                col: 16, // (width 40 - cols 7) / 2, the single-image group centering formula
                cols: 7,
                rows: 3,
                fence_ord: None,
            }],
            "the trailing image must still extract, with its own real URL, not fall back to \
             URL-less inline alt-text: {:?}",
            out.images
        );
        assert_eq!(
            out.lines.first(),
            Some(&Line::from("Some intro sentence.")),
            "the leading sentence's own text must survive intact, on its own line: {:?}",
            out.lines
        );
    }

    /// Mermaid fence extraction and its own running ordinal (`ImagePlacement.fence_ord`) — three
    /// fences, only the outer two of which are mermaid (the middle one is an ordinary fence, drawn as
    /// code and never touching the ordinal counter at all), pinning both the *values* `fence_ord`
    /// takes (`0`, then `1` — not `0`/`0`, which a broken "always 0" counter would produce, and not
    /// `0`/`2`, which counting the plain fence too would produce) and the ordinary fence's own
    /// untouched code rendering in between.
    #[test]
    fn mermaid_fence_ordinal_and_placement_render_from_the_model() {
        let src = "```mermaid\nA\n```\n\n```\nplain\n```\n\n```mermaid\nB\n```\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let mermaid_slot = |_: &str| MermaidSlot::Image { cols: 6, rows: 2 };
        let out = render_doc(
            &doc,
            src,
            40,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &mermaid_slot,
            "mermaid",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "src: {src:?}, unsupported: {:?}",
            out.unsupported
        );
        assert_eq!(
            out.images.iter().map(|p| p.fence_ord).collect::<Vec<_>>(),
            vec![Some(0), Some(1)],
            "two mermaid fences, ordinal 0 then 1 (the plain fence between them must not consume a \
             slot): {:?}",
            out.images
        );
        assert_eq!(
            out.images
                .iter()
                .map(|p| (p.cols, p.rows))
                .collect::<Vec<_>>(),
            vec![(6, 2), (6, 2)],
            "images: {:?}",
            out.images
        );
        let has_plain_code = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.as_ref() == "plain"));
        assert!(
            has_plain_code,
            "the ordinary fence between the two mermaid ones must still render as plain, \
             highlighted code: {:?}",
            out.lines
        );
    }

    /// `render_mermaid_slot`'s own `ImagePlacement.url` must be **byte-for-byte identical** to what
    /// production's own `mermaid_fence_url` computes for the same fence — not merely well-formed:
    /// it is the literal cache key `md_image_cache`/`ensure_mermaid_fence_render` look the rendered
    /// diagram up by (see `mermaid_fence_url`'s own doc comment), so a URL even one byte off from
    /// what production would compute for the identical source resolves to nothing once a real
    /// preview path tries to look it up — the diagram just never renders, silently, with every
    /// rendered *line* (`RenderOut::lines`, the only thing the legacy, now-deleted parity harness
    /// compared before this pass) still looking exactly right. Pins the regression `render_mermaid_slot`'s own doc
    /// comment documents in full (`code_body_text` strips one trailing `\n` from the reconstructed
    /// body; production's own `fence` accumulator never does).
    ///
    /// Two cases, not one, each a **top-level** fence (not list-/quote-/details-nested — see
    /// `render_mermaid_slot`'s own doc comment for why a list-nested fence is a separate, deliberately
    /// still-open gap, already a known, accepted divergence for an unrelated reason of its own): a
    /// single content line, and a body ending in *several* blank lines — `code_body_text` only ever
    /// strips exactly one trailing `\n` regardless of how many precede it (see that function's own
    /// doc comment), so a fix that put back "however many newlines look right" rather than exactly
    /// one would still be caught wrong by the second case alone. Compares against
    /// `collect_mermaid_fences` — production's own real extraction function, called directly, never a
    /// hand-copied re-derivation of what its own `fence` accumulator would produce.
    #[test]
    fn mermaid_fence_url_matches_production_for_a_top_level_fence() {
        for src in ["```mermaid\nA-->B\n```\n", "```mermaid\nA-->B\n\n\n```\n"] {
            let prod_fences = crate::preview::markdown::collect_mermaid_fences(src);
            assert_eq!(
                prod_fences.len(),
                1,
                "src: {src:?}, fences: {prod_fences:?}"
            );
            let want = crate::preview::markdown::mermaid_fence_url(&prod_fences[0]);

            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Raw;
            let mermaid_slot = |_: &str| MermaidSlot::Image { cols: 6, rows: 2 };
            let out = render_doc(
                &doc,
                src,
                40,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &mermaid_slot,
                "mermaid",
                true,
                &math_slot,
                false,
            );
            assert!(
                out.unsupported.is_empty(),
                "src: {src:?}, unsupported: {:?}",
                out.unsupported
            );
            assert_eq!(
                out.images.len(),
                1,
                "src: {src:?}, images: {:?}",
                out.images
            );
            assert_eq!(
                out.images[0].url, want,
                "src: {src:?} — render_doc's own mermaid URL ({:?}) must match production's \
                 mermaid_fence_url(&fence) ({want:?}) exactly, byte for byte",
                out.images[0].url
            );
        }
    }

    /// The math analogue of `mermaid_fence_url_matches_production_for_a_top_level_fence` —
    /// `render_math_slot`'s own `ImagePlacement.url` must match production's own `math_url(latex,
    /// display)` exactly, for both an inline (`$…$`) and a display (`$$…$$`) expression. Unlike the
    /// mermaid case, this pass never reconstructs `latex` from a byte range at all — `walk_inline_math`
    /// reuses `scan_inline_math` verbatim, the same production function, so `latex` is production's
    /// own raw capture already (see the module doc comment's own "no second math detector") — but the
    /// URL is still checked directly here, not merely assumed correct from that reuse alone, so a
    /// future change that quietly starts diverging (a normalization step added to one side but not
    /// the other, say) does not go unnoticed just because `RenderOut::lines` still happens to render
    /// correctly (see the module doc comment's own opening paragraph on why `images` needs its own,
    /// independent comparison in the first place). Compares against `collect_math_exprs` — production's
    /// own real extraction, called directly, never a hand-copied re-derivation.
    ///
    /// **Includes** a backslash-escaped ASCII punctuation character inside the expression (`` $$a\_b$$
    /// ``/`` $$a\,b$$ ``, `` \(a\,b\) ``) — this **used to be** a real, separate gap this test
    /// deliberately did not exercise: CommonMark's own inline grammar treats `\` + ASCII punctuation as
    /// an escape, so pulldown-cmark reports the escaped character as its own, separate `Event::Text`
    /// (splitting what `scan_inline_math`'s own line-based, pre-parse `collect_math_exprs` sees as one
    /// unbroken run of characters into several inline events) — `walk_inline_math`'s own per-event drive
    /// of that same scanner used to lose the still-open `$$` across that split (confirmed directly, at
    /// the time: `collect_math_exprs` still found one expression; `render_doc`'s own `out.images` came
    /// back empty, `out.unsupported` still empty too — the expression silently left as ordinary,
    /// unlifted text, not flagged unsupported), and separately, `\(…\)`/`\[…\]`'s own content —
    /// accumulated back then by `push_str`-ing each intervening event's own **already escape-processed**
    /// payload — silently dropped the escape's own backslash entirely (`\(a\,b\)` became `"a,b"`, not
    /// `"a\,b"`). Both closed constructively in `render_dollar_math_tail`/`render_backslash_math` — see
    /// each function's own doc comment for the fix (event-spanning re-scan over a growing **raw** `src`
    /// slice, never an accumulation of escape-processed event payloads) — and pinned end to end, not just
    /// at the URL level, by `code_span_corpus`'s own escaped-math cases.
    #[test]
    fn math_url_matches_production_for_the_same_expression() {
        for (src, display) in [
            ("inline $x^2$ math\n", false),
            ("$$\\int_0^1 x dx$$\n", true),
            ("$$a\\,b$$\n", true),
            ("$$a\\_b$$\n", true),
            ("$a\\%b$\n", false),
            ("\\(a\\,b\\)\n", false),
            ("\\[a\\,b\\]\n", true),
        ] {
            let prod = crate::preview::markdown::collect_math_exprs(src);
            assert_eq!(prod.len(), 1, "src: {src:?}, exprs: {prod:?}");
            let (latex, prod_display) = &prod[0];
            assert_eq!(*prod_display, display, "src: {src:?}, exprs: {prod:?}");
            let want = crate::preview::markdown::math_url(latex, *prod_display);

            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Image { cols: 6, rows: 2 };
            let out = render_doc(
                &doc,
                src,
                40,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "mermaid",
                true,
                &math_slot,
                true,
            );
            assert!(
                out.unsupported.is_empty(),
                "src: {src:?}, unsupported: {:?}",
                out.unsupported
            );
            assert_eq!(
                out.images.len(),
                1,
                "src: {src:?}, images: {:?}",
                out.images
            );
            assert_eq!(
                out.images[0].url, want,
                "src: {src:?} — render_doc's own math URL ({:?}) must match production's \
                 math_url(latex, display) ({want:?}) exactly, byte for byte",
                out.images[0].url
            );
        }
    }

    /// Regression for the two bugs `math_url_matches_production_for_the_same_expression`'s own doc
    /// comment now records as fixed — but checked here at the `lines`/`unsupported` level instead of
    /// just `url`, since a wrong `url` and a *dropped* lift look identical from that test's own
    /// `images.len() == 1` assertion alone (both would have failed it, for different reasons, before
    /// either fix — this test pins the *shape* of the fix, not merely its end result).
    ///
    /// `$$a\,b$$` alone (the module doc comment's own minimal repro): before `render_dollar_math_tail`
    /// existed, this rendered as two *literal* spans — `"$$a"` and `",b$$"` — one per
    /// escape-split `Event::Text`, `out.images` empty, `out.unsupported` still empty (silently wrong,
    /// not flagged). Reverting `render_dollar_math_tail`'s own "still open" trigger
    /// (`walk_inline_math`'s `if dollar_math_still_open(&t) { ... }` arm) back out — the smallest change
    /// that reproduces the pre-fix behavior — makes this assertion fail exactly that way.
    #[test]
    fn escaped_dollar_math_lifts_instead_of_rendering_as_two_literal_fragments() {
        let src = "$$a\\,b$$\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "mermaid",
            true,
            &math_slot,
            true,
        );
        assert!(out.unsupported.is_empty(), "src: {src:?}");
        assert_eq!(
            out.lines,
            vec![Line::from(Span::from("$$ a\\,b $$").dim())],
            "an escape-split `$$…$$` must still lift as one placeholder, not fall through as two \
             literal text fragments (`\"$$a\"`/`\",b$$\"`) around the event boundary the escape causes: \
             {:?}",
            out.lines
        );
    }

    /// The backslash-math analogue: before `render_backslash_math`'s own content was computed as a raw
    /// `src` slice, `\(a\,b\)` *did* lift (the opener/closer detection never depended on content at
    /// all), but with the wrong content — `"a,b"`, the escape's own backslash silently dropped by the
    /// old `content.push_str(t)` accumulation (`t` being pulldown-cmark's own **already
    /// escape-processed** event payload, which never had the backslash to begin with). Reverting
    /// `content_start`/`&src[content_start..range.start - 1]` back to that old accumulation makes this
    /// assertion fail with `"a,b"` instead.
    #[test]
    fn escaped_backslash_math_content_keeps_the_escapes_own_backslash() {
        for (src, want_display) in [("\\(a\\,b\\)\n", false), ("\\[a\\,b\\]\n", true)] {
            let prod = crate::preview::markdown::collect_math_exprs(src);
            assert_eq!(
                prod,
                vec![("a\\,b".to_string(), want_display)],
                "src: {src:?}"
            );

            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Raw;
            let out = render_doc(
                &doc,
                src,
                80,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "mermaid",
                true,
                &math_slot,
                true,
            );
            assert!(out.unsupported.is_empty(), "src: {src:?}");
            let want_dim = if want_display {
                "$$ a\\,b $$"
            } else {
                "$a\\,b$"
            };
            assert_eq!(
                out.lines,
                vec![Line::from(Span::from(want_dim).dim())],
                "src: {src:?} — the lifted expression's own content must keep the internal escape's \
                 backslash, matching collect_math_exprs's own raw-source extraction exactly: {:?}",
                out.lines
            );
        }
    }

    /// Detection-power/no-regression pair for `dollar_math_still_open`'s own trigger condition, which
    /// is deliberately *broader* than "this is actually math" (any leftover, un-escaped `$` at all —
    /// including one that will never close, like ordinary currency prose) — see that function's own doc
    /// comment. Both cases below already rendered correctly *before* `render_dollar_math_tail` existed
    /// (there was nothing to fix here); what this test actually pins is that routing them through the
    /// new "maybe extend" path anyway does not introduce a regression — in particular, that the "give
    /// up" exit replays each buffered event through its own normal, escape-*processed* dispatch
    /// (`replay_events`), never rendering the growing **raw** slice it used to look for a closer in,
    /// which would otherwise leak a stray literal backslash from an unrelated escape pulled into the
    /// same escape-joined chain purely because the gap before it happened to be `"\\"` too.
    #[test]
    fn a_dollar_that_never_closes_still_renders_literally_even_with_unrelated_escapes_nearby() {
        for src in [
            // No other escape in the document at all — the simplest "give up" shape.
            "It costs $50 for the widget.\n",
            // The escape that keeps `dollar_math_still_open` triggering (`\$`, itself never an opener —
            // `scan_inline_math`'s own escape branch never re-examines the `$` it is attached to) sits
            // right next to a second, wholly unrelated escape (`\*`) chained onto the very same
            // "still open" growth attempt — the shape most likely to leak a stray backslash if the
            // give-up path ever rendered from the raw slice instead of replaying the original events.
            "cost \\$5 and \\$10 for \\*not italic\\*.\n",
        ] {
            let prod = crate::preview::markdown::collect_math_exprs(src);
            assert!(prod.is_empty(), "src: {src:?}, exprs: {prod:?}");

            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Raw;
            let out = render_doc(
                &doc,
                src,
                80,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "mermaid",
                true,
                &math_slot,
                true,
            );
            assert!(out.unsupported.is_empty(), "src: {src:?}");
            assert!(
                out.images.is_empty(),
                "src: {src:?}, images: {:?}",
                out.images
            );
            let rendered: String = out
                .lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .map(|s| s.content.as_ref())
                .collect();
            assert!(
                !rendered.contains('\\'),
                "src: {src:?} — no backslash should ever reach the screen for an escape that is not \
                 part of a real lift, whether the escape sits *inside* the never-closing dollar span's \
                 own reach or somewhere else entirely: {rendered:?}"
            );
        }
    }

    /// Regression for a real, confirmed panic `render_dollar_math_tail`'s own "Never stop mid-construct"
    /// doc comment records the discovery of: `dollar_math_still_open`'s own trigger condition fires for
    /// **any** ordinary, never-closing `$` (currency prose, no escape needed at all — unlike the previous
    /// test, which specifically targets the escape-chain angle) — and an early version of this loop gave
    /// up the instant a non-continuing event turned up, which crashes the moment that event is a
    /// `Start(tag)` whose own `range` spans further than the *individual* nested events
    /// `walk_inline_math`'s own outer loop still has left to consume once this function returns
    /// (`"byte range starts at X but ends at Y"`, confirmed directly for `` "It costs $50 **bold**
    /// text.\n" `` — ordinary prose, no escape anywhere near the `Start`/`End` pair at all). Reverting
    /// the `depth`-tracked "never stop mid-construct" guard back to unconditional exits — the smallest
    /// change that reproduces the pre-fix behavior — panics on every case below.
    ///
    /// Each case's own expected `lines` is exactly what `render_doc` already produces for the identical
    /// text with its `$…` prefix removed (confirmed by hand, not merely asserted): this pins not just
    /// "does not panic" but "renders identically to as if `dollar_math_still_open` had never fired at
    /// all" — bold/link/emphasis styling intact, an interior `SoftBreak` still rendered as one, no
    /// backslash or dropped span anywhere.
    #[test]
    fn dollar_that_never_closes_does_not_panic_when_unrelated_markup_follows() {
        for (src, want) in [
            (
                "It costs $50 **bold** text.\n",
                vec![Line::from_iter([
                    Span::from("It costs $50 "),
                    Span::from("bold").bold(),
                    Span::from(" text."),
                ])],
            ),
            (
                "It costs $50 *italic\nbreak* more.\n",
                vec![Line::from_iter([
                    Span::from("It costs $50 "),
                    Span::from("italic").italic(),
                    Span::from(" "),
                    Span::from("break").italic(),
                    Span::from(" more."),
                ])],
            ),
            (
                "It costs $50 [a link](url) more.\n",
                vec![Line::from_iter([
                    Span::from("It costs $50 "),
                    Span::from("a link"),
                    Span::from(" ("),
                    Span::from("url").blue().underlined(),
                    Span::from(")"),
                    Span::from(" more."),
                ])],
            ),
        ] {
            let prod = crate::preview::markdown::collect_math_exprs(src);
            assert!(prod.is_empty(), "src: {src:?}, exprs: {prod:?}");

            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Raw;
            let out = render_doc(
                &doc,
                src,
                80,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "mermaid",
                true,
                &math_slot,
                true,
            );
            assert!(out.unsupported.is_empty(), "src: {src:?}");
            assert!(
                out.images.is_empty(),
                "src: {src:?}, images: {:?}",
                out.images
            );
            assert_eq!(
                out.lines, want,
                "src: {src:?} — must render exactly as if the never-closing `$` had never triggered \
                 `render_dollar_math_tail` at all: {:?}",
                out.lines
            );
        }
    }

    /// Regression for a real, confirmed panic — the `render_backslash_math` analogue of the
    /// `render_dollar_math_tail` one just above, found by `app::md_real_file_sweep_tests`'s own sweep
    /// over every `.md` file under `~/.cargo/registry/src`, not merely reasoned about:
    /// `proptest-1.11.0/CHANGELOG.md` panics rendering `"...compilation. \([#519](https://…/pull/\
    /// 519))\n"` — an ordinary CommonMark generic backslash-escape (`\(`, stopping this from parsing
    /// as an open paren at all — not LaTeX math) immediately followed by a real markdown link, whose
    /// closing `)` is never itself escaped. `render_backslash_math`'s own loop, before this fix,
    /// computed a gap (`src[prev_end..range.start]`) against *every* event unconditionally, including
    /// a `Start(Link)`'s own first child — but a `Start` event's own `range` spans its *entire*
    /// construct (matching its later `End`'s), so that child's own `range.start` sits *earlier* than
    /// the `Start`'s own `range.end` `prev_end` had just been set to: `src[prev_end..range.start]`
    /// with `prev_end > range.start` panics ("byte range starts at X but ends at Y"). Reverting the
    /// `depth == 0` gate on gap-computation/closer-detection/`is_break` back to unconditional (the
    /// smallest change that reproduces the pre-fix behavior) panics on every case below.
    ///
    /// Each case's own expected `lines` was confirmed against the fixed renderer directly (not merely
    /// asserted): the opener replays as a literal `"("`/`"["` (no lift — no matching *escaped* closer
    /// ever turns up), whatever nested link/emphasis sits between it and the closer renders exactly as
    /// it would have outside any math context at all, and the closer/trailing text survive intact —
    /// pinning "does not panic" *and* "renders as if `backslash_math_opener` had never matched at all",
    /// the identical double bar `dollar_that_never_closes_does_not_panic_when_unrelated_markup_follows`
    /// holds its own cases to.
    #[test]
    fn backslash_math_that_never_closes_does_not_panic_when_a_link_follows() {
        for (src, want) in [
            (
                "See \\([a link](http://x)) here.\n",
                vec![Line::from_iter([
                    Span::from("See "),
                    Span::from("("),
                    Span::from("a link"),
                    Span::from(" ("),
                    Span::from("http://x").blue().underlined(),
                    Span::from(")"),
                    Span::from(") here."),
                ])],
            ),
            (
                "See \\[[a link](http://x)] here.\n",
                vec![Line::from_iter([
                    Span::from("See "),
                    Span::from("["),
                    Span::from("a link"),
                    Span::from(" ("),
                    Span::from("http://x").blue().underlined(),
                    Span::from(")"),
                    Span::from("]"),
                    Span::from(" here."),
                ])],
            ),
            (
                "\\([a **bold** link](http://x)) works.\n",
                vec![Line::from_iter([
                    Span::from("("),
                    Span::from("a "),
                    Span::from("bold").bold(),
                    Span::from(" link"),
                    Span::from(" ("),
                    Span::from("http://x").blue().underlined(),
                    Span::from(")"),
                    Span::from(") works."),
                ])],
            ),
        ] {
            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Raw;
            let out = render_doc(
                &doc,
                src,
                80,
                code,
                "TwoDark",
                // icons: `false` — matching the sibling `$…` regression test above (`render_doc`'s
                // own raw Link rendering is icon-blind regardless; icon substitution is
                // `collapse_links`'s own app-level post-process, exercised separately).
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "mermaid",
                true,
                &math_slot,
                true,
            );
            assert!(out.unsupported.is_empty(), "src: {src:?}");
            assert!(
                out.images.is_empty(),
                "src: {src:?}, images: {:?}",
                out.images
            );
            assert_eq!(
                out.lines, want,
                "src: {src:?} — must render exactly as if `backslash_math_opener` had never matched \
                 at all: {:?}",
                out.lines
            );
        }
    }

    /// The flip side of the panic regression above: once `render_dollar_math_tail` no longer gives up
    /// the instant a non-`Text` event turns up, a dollar-math span whose real closer sits *past* an
    /// intervening inline code span or nested markup construct now resolves correctly too — matching
    /// production's own raw-source `collect_math_exprs`, which has always treated such markup as
    /// ordinary literal characters (it runs directly on raw source, before any parser interprets any of
    /// it). Not required
    /// by the escape-splitting bug this task set out to fix, but a direct, confirmed consequence of the
    /// crash fix above, pinned here so a future regression back to the narrower (and crash-prone)
    /// "only ever grow across an escape-joined `Event::Text`" condition shows up as a real test failure,
    /// not merely as this doc comment's own unverified claim.
    #[test]
    fn dollar_math_resolves_across_an_intervening_code_span_or_nested_markup() {
        for src in ["$$a\\,b `x` c\\_d$$\n", "$$a\\,b **c** d\\_e$$\n"] {
            let prod = crate::preview::markdown::collect_math_exprs(src);
            assert_eq!(prod.len(), 1, "src: {src:?}, exprs: {prod:?}");
            let (latex, display) = &prod[0];
            let want_url = crate::preview::markdown::math_url(latex, *display);

            let doc = Doc::parse(src);
            let code = CodeStyle::default();
            let math_slot = |_: &str, _: bool| MathSlot::Image { cols: 6, rows: 2 };
            let out = render_doc(
                &doc,
                src,
                80,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "mermaid",
                true,
                &math_slot,
                true,
            );
            assert!(out.unsupported.is_empty(), "src: {src:?}");
            assert_eq!(
                out.images.len(),
                1,
                "src: {src:?}, images: {:?}",
                out.images
            );
            assert_eq!(
                out.images[0].url, want_url,
                "src: {src:?} — must resolve to production's own math_url exactly, byte for byte, \
                 even though a code span/nested markup construct sits between the escape and the real \
                 closer: {:?}",
                out.images[0].url
            );
        }
    }

    /// Regression: a **loose** task list (items separated by a blank line —
    /// `"- [ ] one\n\n- [ ] two\n\n- [ ] three\n"`) used to render each item's marker and its own
    /// checkbox+text on two *separate* rows (`["- ", "[ ] one", "- ", "[ ] two", ...]`), with the
    /// checkbox left as literal, undecorated `"[ ] "` text — a real regression against the
    /// pre-refactor, legacy tui-markdown-based renderer, which showed every item as one row, checkbox
    /// properly symbolized (`"- [ ] one"`, `"- [ ] two"`, ...), found live by comparing this
    /// renderer's own output against the legacy renderer's for the identical input.
    ///
    /// The root cause (see `render_item`'s own `checkbox_shares_the_marker_row` doc comment, right
    /// above where this test's own fix lives): the legacy renderer's *apparent* "tight-looking" loose
    /// task list was not a deliberate design at all — it was its own panic-recovery pass silently
    /// re-parsing each crashed item as its own isolated, single-line (and so genuinely tight) list,
    /// after real tui-markdown's `TaskListMarker` handling panicked on the identical "loose item's own
    /// fresh, empty row" shape this file used to place the marker on, unpanicking but unchanged in
    /// *position*. This file has no such crash (and no such recovery pass) to hide behind —
    /// `render_item` now decides the checkbox's own row directly, matching what the legacy renderer
    /// only ever *showed* by accident.
    #[test]
    fn loose_task_list_collapses_checkbox_onto_the_marker_row_like_production_does() {
        let src = "- [ ] one\n\n- [ ] two\n\n- [ ] three\n";
        let out = render(src, true, false);
        assert_eq!(
            lines_text(&out),
            vec!["- [ ] one", "- [ ] two", "- [ ] three"]
        );
        // The checkbox span must carry `task_marker_style()` — the exact Tab-focus/`Space`-toggle
        // sentinel `app::md_items`/`app::md_tasks` key off — not survive as plain, undecorated text
        // (the actual, reported symptom: before this fix, `decorate_extras`'s own
        // `replace_task_checkbox` never recognized either malformed line as a checkbox at all, so
        // this style was never applied).
        let checkbox_style = super::super::task_marker_style();
        for line in &out.lines {
            let checkbox = line
                .spans
                .iter()
                .find(|s| s.style == checkbox_style)
                .unwrap_or_else(|| panic!("no task_marker_style() span on line {line:?}"));
            assert_eq!(checkbox.content.as_ref(), "[ ] ");
        }
        // `task_marks` (what `y c`/write-back/Space-toggle read the byte offset from) must still
        // point at the state character itself, one entry per item, in document order — unaffected
        // by the row the marker happens to render on, but pinned here as part of the same
        // regression's own fix.
        assert_eq!(out.tasks.len(), 3, "tasks: {:?}", out.tasks);
        for (state, off) in &out.tasks {
            assert_eq!(*state, ' ');
            assert_eq!(
                &src[*off..*off + 1],
                " ",
                "byte {off} must be the state char itself"
            );
        }
    }

    /// The identical loose-task-list shape, but with a **checked** item mixed in and a **custom**
    /// task state (`ui.md_task_states` — never reported by pulldown-cmark as `Event::TaskListMarker`
    /// at all, so it takes the `None` arm's own `task_prefix_state` detection instead, not
    /// `Writer::write_task_marker`) — confirms the fix does not regress either of those two other
    /// task-item shapes for a loose list.
    #[test]
    fn loose_task_list_checked_and_custom_states_also_collapse_onto_the_marker_row() {
        let src = "- [x] done\n\n- [/] custom\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        // `'/'` must be in the configured task-state alphabet for `task_prefix_state` to recognize
        // it at all — the `render`/`render_with_images` helpers above only ever configure `[' ',
        // 'x']` (GFM's own two), so this calls `render_doc` directly with a widened set.
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x', '/'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        let texts = lines_text(&out);
        assert_eq!(texts, vec!["- [x] done", "- [/] custom"]);
        assert_eq!(
            out.tasks,
            vec![('x', 3), ('/', 15)],
            "tasks: {:?}",
            out.tasks
        );
    }

    /// A **tight** task list (no blank line between items) must render exactly as it always has —
    /// `checkbox_shares_the_marker_row` only ever *skips* a blank-line push that a tight item's own
    /// `loose_first == false` already made a no-op; nothing here should change for it.
    #[test]
    fn tight_task_list_is_unaffected_by_the_loose_task_fix() {
        let out = render("- [ ] one\n- [ ] two\n", true, false);
        assert_eq!(lines_text(&out), vec!["- [ ] one", "- [ ] two"]);
    }

    /// A **loose** list item with **no** task at all must keep its own pre-existing two-row shape
    /// (marker alone, text one row below) — `checkbox_shares_the_marker_row` is `false` whenever
    /// `task.is_none()`, so this is not a general "loose lists always collapse" change.
    #[test]
    fn loose_list_with_no_task_still_gets_its_own_separate_row() {
        let out = render("- one\n\n- two\n", true, false);
        assert_eq!(lines_text(&out), vec!["- ", "one", "- ", "two"]);
    }

    /// Companion fix, same underlying principle, requested separately once confirmed *not* a
    /// regression (the shipped production build shows the identical, undecorated gap for this
    /// shape): a checkbox inside a **plain** (non-alert) blockquote — `"> - [ ] x\n"` — used to
    /// render its `[ ] x` text with the `>` prefix already baked into `w.lines` (`render_quote`'s
    /// own *previous* shared-`Writer` design), so `render_doc`'s own single, whole-document
    /// `decorate_headings_and_extras` pass — which requires a line's own **first** span to carry no
    /// prefix at all (`task_prefix_state`'s "first byte must be a bullet" contract) — never
    /// recognized it. `render_quote` now decorates its own children in an isolated sub-`Writer`
    /// *before* the `>` prefix is ever applied (mirroring `render_bar_prefixed_body`'s own,
    /// already-established pattern for an alert's/`<details>`'s own body — see `render_quote`'s own
    /// doc comment for the full mechanism and why it calls `emit_row_prefixed` directly, with an
    /// explicit `>` span, rather than a per-level bar-span prepend).
    #[test]
    fn checkbox_inside_a_plain_quote_is_decorated_and_recorded() {
        let out = render("> - [ ] quoted task\n", true, false);
        assert_eq!(lines_text(&out), vec!["> - [ ] quoted task"]);
        let checkbox_style = super::super::task_marker_style();
        let checkbox = out.lines[0]
            .spans
            .iter()
            .find(|s| s.style == checkbox_style)
            .expect("the checkbox span must carry the Tab-focus sentinel style");
        assert_eq!(checkbox.content.as_ref(), "[ ] ");
        assert_eq!(out.tasks, vec![(' ', 5)], "tasks: {:?}", out.tasks);
    }

    /// **Nested** quotes (`> > x`) must keep rendering byte-for-byte as before this fix — `N` bare
    /// `>` spans, no space between them, exactly one shared space before the content, regardless of
    /// depth (`Writer::emit_row_prefixed`'s own doc comment covers how `N` separate calls, one per
    /// nesting level, reproduce this without a shared stack) — proving the isolate-then-re-prefix
    /// shape works correctly rather than reproducing an alert-bar-style "space after every level"
    /// shape.
    #[test]
    fn nested_quotes_keep_the_same_prefix_shape_as_before() {
        let out = render("> > nested\n", true, false);
        assert_eq!(lines_text(&out), vec!["> > nested"]);
    }

    /// A checkbox inside a **doubly**-nested quote (`> > - [ ] x`) — the recursive case
    /// `nested_quotes_keep_the_same_prefix_shape_as_before` doesn't exercise on its own — confirms
    /// the fix generalizes past one level: each level isolates, decorates, and re-prefixes in turn,
    /// from the innermost quote outward.
    #[test]
    fn checkbox_inside_a_doubly_nested_quote_is_also_decorated() {
        let out = render("> > - [ ] x\n", true, false);
        assert_eq!(lines_text(&out), vec!["> > - [ ] x"]);
        let checkbox_style = super::super::task_marker_style();
        assert!(
            out.lines[0].spans.iter().any(|s| s.style == checkbox_style),
            "lines: {:?}",
            out.lines[0]
        );
    }

    /// A heading directly inside a plain quote — a documented, *accepted* limitation before this
    /// fix (the module doc comment's own "How inline content is rendered" section: "a heading or
    /// rule nested inside a `Quote` does **not** get its usual special treatment"), closed as a
    /// side effect of the same isolate-before-prefix rewrite (`decorate_headings` now sees the
    /// heading's own line with no leading span, exactly like a top-level one). Pinned here as a
    /// deliberate behavior change, not merely inferred from the checkbox fix.
    #[test]
    fn heading_inside_a_plain_quote_now_gets_the_full_width_rule_too() {
        let out = render("> # H\n", true, false);
        let texts = lines_text(&out);
        assert_eq!(texts.len(), 2, "lines: {texts:?}");
        assert!(texts[0].starts_with("> "), "heading line: {:?}", texts[0]);
        assert!(texts[0].contains('H'), "heading line: {:?}", texts[0]);
        assert!(
            texts[1].starts_with("> ─") || texts[1].starts_with('>'),
            "the full-width rule under an H1/H2 must also be quote-prefixed: {texts:?}"
        );
    }

    /// A code block directly inside a plain quote — the shape whose own *width* accounting this
    /// fix's own doc comment flags as an accepted imprecision for deep nesting — still gets its
    /// background band correctly narrowed for the **common**, single-level case (never wider than
    /// before, matching `render_code_block`'s own overflow-prevention contract): every line's own
    /// rendered width, quote prefix included, must not exceed the requested column count.
    #[test]
    fn code_block_inside_a_plain_quote_still_fits_the_requested_width() {
        let src = "> ```rust\n> fn a() {}\n> ```\n";
        let doc = Doc::parse(src);
        let code = CodeStyle {
            bg: Some(Color::Rgb(10, 10, 10)),
            ..CodeStyle::default()
        };
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let width = 40u16;
        let out = render_doc(
            &doc,
            src,
            width,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        for l in &out.lines {
            let cols: usize = l
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(
                cols <= width as usize,
                "line {cols} cols wide, requested width was {width}: {:?}",
                line_text(l)
            );
        }
    }

    // =========================================================================================
    // Coverage-hardening pass (pre-release audit). Everything below closes the ~125-line gap
    // `cargo llvm-cov` reported for this file before this pass, plus a batch of mutation-testing
    // and adversarial-input regressions written alongside it. Grouped by the area of `render_doc`
    // each targets; every group's own header names the line ranges (as of the audit) it closes.
    // Several tests reach a genuinely defensive branch directly (bypassing `Doc::parse` and
    // `render_doc` entirely, the same way `write_task_marker_on_a_fresh_empty_line_does_not_panic`
    // above already does) — each such test says so, and why real parsing cannot reach it.

    /// A blockquote isolates into its own sub-`Writer`, so a list found inside one starts counting
    /// its nesting **from scratch** — a `> - a` written inside a list item indents exactly like a
    /// `> - a` at the top of the document. Carrying the enclosing list's depth into the quote
    /// instead pushes the inner marker right by four columns per level already open outside, which
    /// looks like a deeply nested list that nothing in the source asked for.
    ///
    /// The three-level shape this needs (list → quote → list) existed nowhere: the corpus has
    /// list → quote → *table* and quote → list, but never a list inside a quote inside a list.
    #[test]
    fn a_quoted_list_inside_a_list_item_starts_its_nesting_over() {
        for (why, src, want) in [
            (
                "the quote sits in a one-level list",
                "- item\n  > - a\n  > - b\n",
                vec!["- item", "> - a", "> - b"],
            ),
            (
                "the quote sits in an ordered list",
                "1. item\n   > - a\n",
                vec!["1. item", "> - a"],
            ),
            (
                "the quote sits two list levels deep",
                "- one\n  - two\n    > - a\n",
                vec!["- one", "    - two", "> - a"],
            ),
            (
                "an ordered list inside the quote",
                "- item\n  > 1. a\n  > 2. b\n",
                vec!["- item", "> 1. a", "> 2. b"],
            ),
            (
                "a doubly nested quote inside a list item",
                "- item\n  > > - a\n",
                vec!["- item", "> > - a"],
            ),
            (
                // The other half of the same rule: nesting that happens *inside* the quote is still
                // counted, so this is not "quoted lists never indent".
                "a list nested inside the quoted list",
                "- item\n  > - a\n  >   - deep\n",
                vec!["- item", "> - a", ">     - deep"],
            ),
            (
                // The control: with no list open outside, both the correct rule and the broken one
                // produce this, so a test built only on this shape proves nothing.
                "the same quote with no list around it at all",
                "> - a\n",
                vec!["> - a"],
            ),
        ] {
            let out = render(src, true, false);
            assert_eq!(lines_text(&out), want, "{why}");
        }
    }

    /// A table asked to draw into a pane narrower than its own frame (the border bars plus each
    /// column's two padding spaces) falls back to **one column per column** — the narrowest grid
    /// that still has a cell in it. The box is then `1 + 4*ncol` wide whatever the pane says, which
    /// is deliberately wider than a very narrow pane: `budget`'s `.max(ncol)` floor is what stops
    /// the shaving loop from grinding columns away to nothing, and one column of content is the
    /// least a table can be drawn as. Pinned as the current, chosen behavior — not endorsed as
    /// pretty — because the alternative (a floor of two columns per column) is silently *wider*
    /// still, and nothing anywhere drew a table into a pane this narrow before.
    #[test]
    fn a_table_in_a_pane_narrower_than_its_own_frame_keeps_one_column_per_column() {
        for ncol in 1..=4usize {
            let head: Vec<String> = (0..ncol).map(|c| format!("aaaa{c}")).collect();
            let src = format!(
                "| {} |\n|{}|\n| {} |\n",
                head.join(" | "),
                vec!["---"; ncol].join("|"),
                head.join(" | ")
            );
            let want = 1 + 4 * ncol;
            for width in 1..=(want as u16) {
                let lines = html_table_lines(&src, width);
                let box_lines: Vec<&String> = lines
                    .iter()
                    .filter(|l| {
                        l.starts_with('┌')
                            || l.starts_with('│')
                            || l.starts_with('├')
                            || l.starts_with('└')
                    })
                    .collect();
                assert!(
                    !box_lines.is_empty(),
                    "ncol={ncol} width={width}: 表が描かれていない: {lines:?}"
                );
                for l in box_lines {
                    assert_eq!(
                        unicode_width::UnicodeWidthStr::width(l.as_str()),
                        want,
                        "ncol={ncol} width={width}: 列が1桁に潰れていない: |{l}|"
                    );
                }
            }
        }
    }

    /// The concrete shape of the same rule, spelled out: three columns in a 12-column pane draw a
    /// 13-column box — one column wider than the pane. That overflow is the documented consequence
    /// of the `.max(ncol)` floor above, and it is pinned here so a later change to the floor shows
    /// up as this number moving rather than as a silent reflow.
    #[test]
    fn three_columns_in_a_twelve_column_pane_draw_a_thirteen_column_box() {
        let lines = html_table_lines("| aaaa | bbbb | cccc |\n|---|---|---|\n| 1 | 2 | 3 |\n", 12);
        assert_eq!(
            lines
                .iter()
                .map(|l| unicode_width::UnicodeWidthStr::width(l.as_str()))
                .collect::<Vec<_>>(),
            // Eight lines: the two rules, the header's four wrapped rows (`aaaa` one column at a
            // time), the divider, and the body row.
            vec![13; 8],
            "{lines:?}"
        );
    }

    /// A convenience `render_doc` caller for this whole section: same defaults `fresh_writer`/
    /// `fresh_ctx` establish (80 columns, default `CodeStyle`, icons off, GFM task states,
    /// `no_images`/`no_mermaid`, `MathSlot::Raw`), `alerts`/`math_on` as explicit arguments since
    /// most of the tests below need to vary at least one of them. Not a second render path: this
    /// is exactly the `render_doc(&doc, src, 80, code, "TwoDark", false, &[' ', 'x'], &no_images,
    /// &no_mermaid, "Enter: full screen", alerts, &math_slot, math_on)` call every test below would
    /// otherwise repeat verbatim.
    fn render(src: &str, alerts: bool, math_on: bool) -> RenderOut {
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            alerts,
            &math_slot,
            math_on,
        )
    }

    /// Like `render`, but with a `slot_of` that answers `ImageSlot::Inline` for every URL — needed
    /// by every test in this section that has to observe a real `ImagePlacement` (the `no_images`
    /// helper always answers `Unavailable`, which never records one).
    fn render_with_images(
        src: &str,
        alerts: bool,
        math_on: bool,
        cols: u16,
        rows: u16,
    ) -> RenderOut {
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols, rows };
        render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &slot_of,
            &no_mermaid,
            "Enter: full screen",
            alerts,
            &math_slot,
            math_on,
        )
    }

    /// Joins one decorated `Line`'s own spans back into plain text, for assertions that only care
    /// about the words on screen, not which spans/styles carried them.
    fn line_text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn lines_text(out: &RenderOut) -> Vec<String> {
        out.lines.iter().map(line_text).collect()
    }

    /// A **nested** list never opens with a blank row of its own; a top-level one does.
    ///
    /// `render_list` asks `ctx.list_depth == 0 && w.pending_block_gap` before emitting that row, and
    /// both halves are load-bearing. The `pending_block_gap` half is what puts one blank row between
    /// a paragraph and the list that follows it. The `list_depth == 0` half is what keeps that same
    /// row *out* of a nested list: a **loose** outer item ends its own paragraph with the gap flag
    /// still set, so a nested list opening right after it would otherwise be pushed down by a blank
    /// row that has no counterpart in the source — the outer item's bullet, its text, and then a
    /// hole before the sub-item.
    ///
    /// Why this needed a test of its own: exactly one corpus case in the whole tree
    /// (`task_corpus: "an indented paragraph inside a list item is not a task even if it starts
    /// with a dash"`) happens to have this shape, so dropping the depth check moved a single line in
    /// the goldens and nothing else anywhere went red. Every case below is a *loose* outer list —
    /// the only kind that reaches `render_list` with the flag still set — across the ways a nested
    /// list can be reached: straight after the item's own paragraph, after a second paragraph, three
    /// levels deep, ordered, carrying a task marker, and inside a block quote. The last entry is the
    /// counter-case that pins the other half: a top-level list *keeps* its blank row.
    #[test]
    fn a_nested_list_never_opens_with_a_blank_row_however_loose_its_parent_is() {
        for (why, src, want) in [
            (
                "a nested list right after a loose item's own paragraph",
                "- a\n\n  - b\n",
                vec!["- ", "a", "    - b"],
            ),
            (
                "a nested list after a second paragraph in the same item",
                "- a\n\n  para\n\n  - b\n",
                vec!["- ", "a", "", "para", "    - b"],
            ),
            (
                "three levels, every one of them loose",
                "- a\n\n  - b\n\n    - c\n",
                vec!["- ", "a", "    - ", "b", "        - c"],
            ),
            (
                "an ordered list nested in a loose ordered item",
                "1. a\n\n   1. b\n",
                vec!["1. ", "a", "    1. b"],
            ),
            (
                "a nested list whose item carries a task marker",
                "- item\n\n  - [ ] looks like a task\n",
                vec!["- ", "item", "    - [ ] looks like a task"],
            ),
            (
                "the same shape inside a block quote",
                "> - a\n>\n>   - b\n",
                vec!["> - ", "> a", ">     - b"],
            ),
            (
                "counter-case: a top-level list still gets its blank row",
                "before.\n\n- a\n- b\n",
                vec!["before.", "", "- a", "- b"],
            ),
        ] {
            assert_eq!(lines_text(&render(src, true, false)), want, "{why}");
        }
    }

    /// Whatever follows a table, an **HTML** table is separated from it exactly the way a **GFM**
    /// table is — because `render_html_table_from_model` clears `pending_block_gap` after drawing,
    /// the same thing the GFM table path does.
    ///
    /// Leaving the flag set instead puts a blank row between the table and everything that follows
    /// it, and only for HTML tables — which is both a difference between two things that draw the
    /// same box and, on a source that has no blank line there at all, a row invented out of nothing.
    ///
    /// Written as an equivalence rather than as a list of expected lines: the point is not what the
    /// gap *is* (that is the goldens' job) but that the two table paths cannot drift apart, so a
    /// later change to the shared rule moves both sides together and this test keeps holding. Each
    /// side's own leading table box is dropped — everything through the first `└` — and the tails
    /// are compared.
    #[test]
    fn an_html_table_is_separated_from_what_follows_exactly_like_a_gfm_table() {
        // Everything after the first table's own bottom border.
        fn tail(src: &str) -> Vec<String> {
            let lines = lines_text(&render(src, true, false));
            let bottom = lines
                .iter()
                .position(|l| l.starts_with('└'))
                .unwrap_or_else(|| panic!("no table was drawn at all for {src:?}: {lines:?}"));
            lines[bottom + 1..].to_vec()
        }
        for follower in [
            "para\n",
            "# H\n",
            "- item\n",
            "1. item\n",
            "```\nfence\n```\n",
            "> quote\n",
            "---\n",
            "<table><tr><td>z</td></tr></table>\n",
            "| q |\n|---|\n| r |\n",
            "", // nothing at all follows
        ] {
            let html = tail(&format!("<table><tr><td>a</td></tr></table>\n\n{follower}"));
            let gfm = tail(&format!("| a |\n|---|\n| x |\n\n{follower}"));
            assert_eq!(
                html, gfm,
                "HTML 表と GFM 表で、後に続く {follower:?} との間の空け方が違う"
            );
        }
    }

    // ---- `contains_unsupported`/`render_doc` top-level dispatch: every position a Table/Html
    // nested inside a Quote, somewhere in a List's/alert's/Details's own subtree, can be reached
    // from — all four render correctly now (`contains_unsupported` is a permanent no-op; see its own
    // doc comment), not reported unsupported the way they used to be. ----

    /// Whether `out` drew a table's own header/body divider rule (`├───┼───┤`) anywhere —
    /// `render_table_cells`'s own `if header_rows > 0 && ri + 1 == header_rows` arm, which only
    /// fires once `is_table_delimiter` actually recognized the delimiter row. This is a stronger
    /// check than "does *any* box-drawing glyph (`┌`) appear" for the four tests below, and
    /// deliberately so: reverting `render_table_from_model` to the pre-fix naive `&src[block_src]`
    /// slice (confirmed directly, by making that exact change and rerunning this group) still draws
    /// *a* box — any parsed set of cells does, delimiter recognized or not — because the quote's own
    /// `>` marker surviving on the delimiter row's own continuation line defeats
    /// `is_table_delimiter`'s own character set (no `>` in it), so `header_rows` stays `0` and the
    /// corrupted delimiter row is folded in as a bogus extra *data* row instead of drawing this
    /// divider — a regression a bare "`┌` appears somewhere" assertion does not catch, but this one
    /// does.
    fn has_table_divider(out: &RenderOut) -> bool {
        out.lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('├')))
    }

    /// A `List` whose only item's own content is a plain `Quote` containing a GFM table: the whole
    /// document renders — the outer `List` arm, the enclosing `Quote` arm, and `render_table_from_model`
    /// itself for the table's own body — with a real, box-drawn table, `>`-quoted like the rest of
    /// the quote's own body.
    #[test]
    fn list_with_a_quote_nested_table_renders_a_real_table() {
        let src = "- item\n  > | a | b |\n  > |---|---|\n  > | 1 | 2 |\n";
        let out = render(src, true, false);
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        let has_border = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('┌')));
        assert!(
            has_border,
            "expected a real, box-drawn table: {:?}",
            out.lines
        );
        assert!(
            has_table_divider(&out),
            "expected the header/body divider rule (proof the delimiter row itself parsed \
             correctly, not as a bogus extra data row): {:?}",
            out.lines
        );
    }

    /// A `<details>` block whose body is a plain `Quote` containing a table — the identical nested
    /// shape as above, but reached via `render_doc`'s own `Details` arm instead of `List`'s.
    #[test]
    fn details_with_a_quote_nested_table_renders_a_real_table() {
        // `open` — otherwise the body (the table this test exists to check) never renders at all,
        // closed being every `<details>` block's own default state (`model::BlockKind::Details`'s
        // own `open_attr`), for a reason that has nothing to do with this test's own subject.
        let src = "<details open>\n<summary>s</summary>\n\n\
                    > | a | b |\n> |---|---|\n> | 1 | 2 |\n\n\
                    </details>\n";
        let out = render(src, true, false);
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        let has_border = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('┌')));
        assert!(
            has_border,
            "expected a real, box-drawn table: {:?}",
            out.lines
        );
        assert!(
            has_table_divider(&out),
            "expected the header/body divider rule (proof the delimiter row itself parsed \
             correctly, not as a bogus extra data row): {:?}",
            out.lines
        );
    }

    /// A GitHub-alert-headed quote (`> [!NOTE]`) whose own body directly contains a table, with
    /// `alerts: false`: reaches `render_doc`'s own "`Quote { alert: Some(_) }` but `!w.alerts`" arm,
    /// not the interactive-alert one below — the table still renders for real either way.
    #[test]
    fn alert_headed_quote_with_alerts_off_and_a_nested_table_renders_a_real_table() {
        let src = "> [!NOTE]\n> | a | b |\n> |---|---|\n> | 1 | 2 |\n";
        let out = render(src, false, false);
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        let has_border = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('┌')));
        assert!(
            has_border,
            "expected a real, box-drawn table: {:?}",
            out.lines
        );
        assert!(
            has_table_divider(&out),
            "expected the header/body divider rule (proof the delimiter row itself parsed \
             correctly, not as a bogus extra data row): {:?}",
            out.lines
        );
    }

    /// The identical document, `alerts: true` this time — reaches `render_doc`'s own real,
    /// interactive alert arm instead (`render_alert_from_model`), which draws the table the
    /// identical way through the same `render_table_from_model`.
    #[test]
    fn alert_headed_quote_with_alerts_on_and_a_nested_table_renders_a_real_table() {
        let src = "> [!NOTE]\n> | a | b |\n> |---|---|\n> | 1 | 2 |\n";
        let out = render(src, true, false);
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        let has_border = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('┌')));
        assert!(
            has_border,
            "expected a real, box-drawn table: {:?}",
            out.lines
        );
        assert!(
            has_table_divider(&out),
            "expected the header/body divider rule (proof the delimiter row itself parsed \
             correctly, not as a bogus extra data row): {:?}",
            out.lines
        );
    }

    /// The two tests above use `| a | b |`-style rows, whose *leading pipe* makes pulldown-cmark
    /// treat the table as able to interrupt the header's own paragraph on its own — `Doc::parse`
    /// already reports two separate sibling blocks (`Paragraph` then `Table`) with no glue for
    /// `render_alert_from_model` to cope with at all (confirmed directly, by dumping
    /// `Parser::new_ext(..).into_offset_iter()` for this exact source: `Start(Paragraph)`..
    /// `End(Paragraph)` covering only `"[!NOTE]"`, then `Start(Table(..))` starting fresh right
    /// after it). A real-world GFM table (an `ndk` README's own alert box, the report that prompted
    /// this fix) is written *without* a leading/trailing pipe — `Crate | Note` / `------|------` —
    /// and that shape genuinely does glue: pulldown-cmark reports **one** `Paragraph` spanning the
    /// header and all three table lines, no `Table` event anywhere, because a table can never start
    /// mid-paragraph and this row shape gives pulldown-cmark no earlier signal to split on. Before
    /// `glued_alert_body`/`dequote_alert_body` existed, `render_alert_from_model` had no way to
    /// recover a `Table` from that one straddling `Paragraph` — it rendered the header's own
    /// left-over text as a single, wrapped plain-text line (`Crate | Note ------|------ ndk   | x`),
    /// not a table at all: a real regression, since the *legacy* renderer's own line-based
    /// `split_tables` never needed block-level structure to find this same table in the first place
    /// and drew a real one. Checks **both** `┌` (any box at all) **and** `has_table_divider`'s own
    /// `├` (the header/delimiter rule specifically) for the identical reason `has_table_divider`'s
    /// own doc comment already gives for the tests above: a `┌`-only check cannot tell a real table
    /// from one whose header row count came out `0` (every row folded in as a bogus data row) —
    /// confirmed directly by reverting `glued_alert_body` to always return `None` (the pre-fix
    /// behavior) and rerunning this test, which fails on the `┌` assertion first, before ever
    /// reaching the `├` one — the shape this test exists to pin.
    #[test]
    fn alert_headed_quote_with_a_table_glued_to_the_header_no_blank_line_renders_a_real_table() {
        let src = "> [!IMPORTANT]\n> Crate | Note\n> ------|------\n> ndk   | x\n";
        let out = render(src, true, false);
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        let has_border = out
            .lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('┌')));
        assert!(
            has_border,
            "expected a real, box-drawn table even though the header line glued into the same \
             paragraph as the table's own raw text: {:?}",
            out.lines
        );
        assert!(
            has_table_divider(&out),
            "expected the header/body divider rule (proof the delimiter row itself parsed \
             correctly, not as a bogus extra data row): {:?}",
            out.lines
        );
    }

    /// `render_doc`'s own top-level `BlockKind::ListItem { .. } => unsupported.push("ListItem")`
    /// arm (line 525) — the doc comment above it says this can never legitimately fire (a `List`
    /// always unwraps its own item children before this loop ever sees one); no real `Doc::parse`
    /// output can produce a top-level `ListItem`, so this constructs one directly (`Doc`/`Block`
    /// fields are all `pub` within the crate — the same "call the private surface directly" this
    /// whole file's own defensive-branch tests already do, e.g. `write_task_marker`'s own low-level
    /// tests above) and confirms `render_doc` degrades to reporting it, not panicking.
    #[test]
    fn a_top_level_list_item_that_could_never_really_happen_is_reported_unsupported_not_a_panic() {
        let doc = Doc {
            events: Vec::new(),
            blocks: vec![Block {
                kind: BlockKind::ListItem { task: None },
                src: 0..0,
                children: Vec::new(),
            }],
        };
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            "",
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert_eq!(out.unsupported, vec!["ListItem"]);
    }

    // ---- `Writer` low-level bookkeeping: lines 963, 1020, 1110-1112 ----

    /// `Writer::append_span`'s own `None` fallback (`self.lines.last_mut()` finds nothing —
    /// `self.lines` is completely empty). Every full-document test above always has
    /// *something* already on `w.lines` (a heading rule, a marker row, ...) by the time any inline
    /// content pushes a span, so this is a direct, low-level call on a genuinely fresh `Writer`.
    #[test]
    fn append_span_on_a_totally_empty_writer_opens_the_first_line_itself() {
        let mut w = fresh_writer();
        assert!(w.lines.is_empty());
        w.append_span(Span::raw("hi"));
        assert_eq!(w.lines, vec![Line::from(vec![Span::raw("hi")])]);
    }

    /// `Writer::write_text`'s own `if i > 0 { emit_blank_row() }` — a `Text` payload with an
    /// embedded `\n`, the "rare but not impossible" shape that method's own doc comment names.
    #[test]
    fn text_with_an_embedded_newline_starts_a_fresh_line_for_the_second_half() {
        let mut w = fresh_writer();
        w.write_text("line1\nline2");
        assert_eq!(
            lines_text(&RenderOut {
                lines: w.lines.clone(),
                images: Vec::new(),
                unsupported: Vec::new(),
                code_blocks: Vec::new(),
                tasks: Vec::new(),
            }),
            vec!["line1", "line2"]
        );
    }

    /// `Writer::write_task_marker`'s own `else` arm (`self.lines` is `None` —
    /// nothing has been pushed at all yet, unlike the existing regression test above which pushes
    /// an empty `Line` first) — falls back to `append_span`, which itself opens the very first line
    /// (the `None` branch this same section already covers on its own).
    #[test]
    fn write_task_marker_with_no_lines_at_all_falls_back_to_append_span() {
        let mut w = fresh_writer();
        assert!(w.lines.is_empty());
        w.write_task_marker(true);
        assert_eq!(w.lines.len(), 1);
        assert_eq!(w.lines[0].spans.len(), 1);
        assert_eq!(w.lines[0].spans[0].content.as_ref(), "[x] ");
    }

    // ---- Inline dispatch: subscript/superscript, and the defensive fallbacks around them
    // (lines 1195-1205, 1213, 1236-1244) ----

    /// `Tag::Superscript`'s own `wrap_styled` arm (lines 1201-1206) — `^text^`, real
    /// `Doc::parse` output (`Options::ENABLE_SUPERSCRIPT` is on — see `model::parse_options`).
    /// Note (an honest finding, not asserted here): pulldown-cmark 0.13's superscript/subscript
    /// delimiters follow the *underscore*-style "no intraword use" flanking rule — `x^2^` (no
    /// space around the delimiters) never opens one at all, only `x ^2^ y` does; `inline_corpus`'s
    /// own "superscript"/"subscript" cases (`"x^2^ is x squared.\n"`/`"H~2~O is water.\n"`) are
    /// mid-word and so never reach this arm through real parsing either — confirmed directly via a
    /// throwaway event dump before writing this test, not merely inferred.
    #[test]
    fn superscript_renders_through_the_real_tag_dispatch() {
        let out = render("a ^super^ b\n", true, false);
        assert_eq!(lines_text(&out), vec!["a super b"]);
        // Mutation-testing gap closed: text-content-only assertions above did not catch a mutant
        // that dropped `Modifier::ITALIC` from the superscript style, leaving only `DIM` — the span
        // itself has to carry the exact `DIM | ITALIC` combination `start_inline_tag`'s own
        // `Tag::Superscript` arm sets, not merely render the right characters.
        let span = out.lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "super")
            .expect("superscript span");
        assert!(span.style.add_modifier.contains(Modifier::DIM));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }

    /// `Tag::Subscript`'s own arm (lines 1195-1200) — the mirror case, `~text~`.
    #[test]
    fn subscript_renders_through_the_real_tag_dispatch() {
        let out = render("a ~sub~ b\n", true, false);
        assert_eq!(lines_text(&out), vec!["a sub b"]);
        // Mutation-testing gap closed: same reasoning as the superscript test above, this time for
        // a mutant that dropped `Modifier::DIM`, leaving only `ITALIC`.
        let span = out.lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "sub")
            .expect("subscript span");
        assert!(span.style.add_modifier.contains(Modifier::DIM));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }

    /// Mutation-testing gap: a strikethrough span's own style must carry `Modifier::CROSSED_OUT`
    /// specifically — a mutant that swapped it for `BOLD` rendered identical *text* (this file's own
    /// existing strikethrough coverage, in the production-diff-parity tests elsewhere in this
    /// module, only ever compares rendered text/line shape against production, never inspects the
    /// modifier bits directly) and stayed green.
    #[test]
    fn strikethrough_span_carries_the_crossed_out_modifier() {
        let out = render("This was ~~deleted~~ text.\n", true, false);
        let span = out.lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "deleted")
            .expect("strikethrough span");
        assert!(span.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    /// Mutation-testing gap: `push_item_marker`'s own ordered-item marker span must be styled
    /// `Color::LightBlue` specifically — a mutant that swapped it for `Yellow` stayed green, since no
    /// existing test inspected the marker span's own color.
    #[test]
    fn ordered_list_marker_is_styled_light_blue() {
        let out = render("1. first\n2. second\n", true, false);
        let marker = out.lines[0].spans.first().expect("marker span");
        assert!(
            marker.content.as_ref().contains('1'),
            "marker: {:?}",
            marker.content
        );
        assert_eq!(marker.style.fg, Some(Color::LightBlue));
    }

    /// Mutation-testing gap: `render_code_block`'s own `label` fallback for a fence/indented block
    /// with no language info string must be the literal `"code"` — a mutant that swapped it for
    /// `"text"` stayed green, since no existing test in this file reads the header badge's own text
    /// back for a *lang-less* block specifically (every other code-block test in this module names a
    /// real language).
    #[test]
    fn code_block_with_no_language_labels_itself_code() {
        let out = render("```\nplain\n```\n", true, false);
        let header = out
            .lines
            .iter()
            .find(|l| l.spans.iter().any(super::super::is_code_header_span))
            .expect("a header line");
        assert!(
            line_text(header).contains("code"),
            "header: {:?}",
            line_text(header)
        );
    }

    /// Mutation-testing gap: `multiline_math_opener`'s own `"\\[" => "\\]"` mapping specifically —
    /// the existing direct test for this function only exercises the `"$$" => "$$"` arm; a mutant
    /// that had the `\[` opener look for the **wrong** closer (`"$$"` instead of `"\]"`) stayed
    /// green.
    #[test]
    fn multiline_math_opener_maps_backslash_bracket_to_its_own_closer() {
        let (closer, _) = multiline_math_opener("\\[\n", &(0..2)).expect("opener recognized");
        assert_eq!(closer, "\\]");
    }

    /// Mutation-testing gap: `render_rule`'s own literal `"---"` push — the *pre-decoration* text
    /// `decorate_extras` later recognizes and turns into the real full-width rule. A mutant that
    /// swapped it for `"***"` stayed green through the *full* `render_doc` pipeline (`decorate_extras`
    /// treats `"---"`/`"***"`/`"___"` identically — an accepted equivalence, not a gap: see the
    /// direct, pre-decoration check below, which *does* distinguish them, confirming `render_rule`
    /// itself still emits the specific literal its own doc comment says it does).
    #[test]
    fn render_rule_pushes_the_literal_dashes_line() {
        let mut w = fresh_writer();
        render_rule(&mut w);
        assert_eq!(w.lines, vec![Line::from("---")]);
    }

    /// Mutation-testing gap: `render_mermaid_slot`'s own `if code.trim().is_empty()` branch — an
    /// empty mermaid fence body must degrade to the legacy text-diagram fallback (`render_mermaid_
    /// block`) without ever consulting `w.mermaid.slot` at all. No existing test in this file drives
    /// an *empty* fence body through a `fences_on: true` mermaid slot (every existing mermaid test
    /// either has real content or uses `no_mermaid`, which disables extraction before this branch is
    /// ever reached) — a mutant that neutered the check (`&& false`) stayed green.
    ///
    /// The slot closure below has to answer the one-time `fences_on` probe (`render_doc`'s own
    /// `mermaid_slot("")` call, always literally empty) with something *other* than `MermaidSlot::
    /// Text` — answering `Text` there would itself disable extraction (`fences_on: !matches!(.., Text)`)
    /// and mask the mutant by routing through `render_code_block` instead, never reaching
    /// `render_mermaid_slot` at all; confirmed directly, the hard way (this test's own first version
    /// did exactly that and the mutation went right on passing).
    #[test]
    fn empty_mermaid_fence_uses_the_text_fallback_without_consulting_the_slot() {
        let src = "```mermaid\n```\n";
        let doc = Doc::parse(src);
        let code = CodeStyle::default();
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let mermaid_slot = |s: &str| -> MermaidSlot {
            if s.is_empty() {
                MermaidSlot::Image { cols: 1, rows: 1 } // the fences_on probe — keep extraction on
            } else {
                panic!("the empty-fence-body branch must never consult the slot: {s:?}")
            }
        };
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &mermaid_slot,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(out.images.is_empty(), "no placement for an empty fence");
    }

    /// `start_inline_tag`'s own defensive `other => skip_balanced(..)` arm (line 1213) and
    /// `skip_balanced`'s own recursive `Start`/matching-`End`/catch-all branches (lines 1236-1241) —
    /// a `Tag::List` can never legitimately open *inside* a `Heading`/`Paragraph`'s own inline
    /// content (CommonMark has no such construct there), so this feeds `walk_inline` a hand-built
    /// event stream directly, the same low-level pattern the file's own existing
    /// `escape_handling_drops_the_backslash_from_every_event_range` test already uses for
    /// `Doc::parse`'s own event stream. Includes a nested `Emphasis` *inside* the stray `List`, so
    /// `skip_balanced`'s own recursive `Event::Start(t) => skip_balanced(events, t.to_end())` arm
    /// (not merely its own top-level dispatch) gets exercised too, and a plain `Text` while
    /// skipping, so its own catch-all (`_ => {}`, line 1241) fires as well.
    #[test]
    fn an_unexpected_inline_start_tag_is_skipped_balanced_not_rendered() {
        let mut w = fresh_writer();
        let events: Vec<Event<'_>> = vec![
            Event::Start(Tag::List(None)),
            Event::Text("skip me".into()),
            Event::Start(Tag::Emphasis),
            Event::Text("nested".into()),
            Event::End(TagEnd::Emphasis),
            Event::End(TagEnd::List(false)),
            Event::Text("after".into()),
        ];
        walk_inline(&mut events.into_iter(), &mut w, None);
        assert_eq!(w.lines, vec![Line::from(vec![Span::raw("after")])]);
    }

    /// `skip_balanced`'s own "ran out of events before the matching `End` ever turned up" path —
    /// the `while let Some(ev) = events.next()` loop simply exhausting (no real `Doc::parse` output
    /// can be unbalanced this way; this calls the private helper directly, as above).
    #[test]
    fn skip_balanced_on_an_unbalanced_stream_just_runs_out_without_panicking() {
        let events: Vec<Event<'_>> = vec![
            Event::Start(Tag::Emphasis),
            Event::Text("never closes".into()),
        ];
        // No assertion beyond "returns" — the whole point is that this does not loop forever or
        // panic reading past the end.
        skip_balanced(&mut events.into_iter(), TagEnd::List(false));
    }

    // ---- `HeadingMeta::to_suffix`: line 1270 ----

    /// A bare heading-attribute key with no `=value` (`{lang}`, inside `{#id .cls lang}`) — the
    /// `None => parts.push(key.clone())` arm every other heading-attribute test in this file's own
    /// production-diff corpus happens to avoid (they all use `key=value` pairs).
    #[test]
    fn heading_attribute_with_no_value_renders_the_bare_key() {
        let out = render("## Heading {#id .cls lang}\n", true, false);
        assert!(
            lines_text(&out)[0].contains("lang"),
            "lines: {:?}",
            lines_text(&out)
        );
        assert!(
            !lines_text(&out)[0].contains("lang="),
            "no value should be appended"
        );
    }

    // ---- Block-image extraction: `segment_as_block_image`/`split_top_level_image_units`
    // (lines 1488, 1500, 1520, 1604-1605, 1616) ----

    /// `segment_as_block_image`'s own `if seg.is_empty() { return None; }` (line 1488) — direct,
    /// low-level: no real caller ever hands it a literally empty slice (`segment_as_block_images`'s
    /// own empty-segment shortcut, `Some(Vec::new())`, intercepts that case first — see that
    /// function's own doc comment on the bare-`\` "gap" shape this exists for).
    #[test]
    fn segment_as_block_image_on_an_empty_segment_is_none() {
        assert_eq!(segment_as_block_image("", &[]), None);
    }

    /// `segment_as_block_image`'s own "link wraps nothing at all" branch (line 1500) — `seg` is
    /// exactly `Start(Link)`, `End(Link)`, back to back, so `inner = &seg[1..1]` is empty. Direct,
    /// low-level construction (real parsing never wraps an empty link body around *nothing* —
    /// `[](url)` has no `Start(Image)`/text inside the link at all, so this exercises the same
    /// shape without needing to find a source string that produces it).
    #[test]
    fn segment_as_block_image_where_a_link_wraps_nothing_is_none() {
        let link_tag = Tag::Link {
            link_type: pulldown_cmark::LinkType::Inline,
            dest_url: "h".into(),
            title: "".into(),
            id: "".into(),
        };
        let events: Vec<(Event<'_>, Range<usize>)> = vec![
            (Event::Start(link_tag), 0..1),
            (Event::End(TagEnd::Link), 1..2),
        ];
        assert_eq!(segment_as_block_image("xx", &events), None);
    }

    /// `segment_as_block_image`'s own `if url.is_empty() { return None; }` (line 1520) — a
    /// standalone image whose destination is the empty string (`![alt]()`), reached through the
    /// real `paragraph_as_block_images` path: since the extraction fails, the whole paragraph falls
    /// through to ordinary inline rendering, showing only the alt text (matching production's own
    /// "an image with no destination is not extracted" contract, cited in this function's own doc
    /// comment).
    #[test]
    fn standalone_image_with_an_empty_url_is_not_extracted_as_a_block_image() {
        let out = render("![a]()\n", true, false);
        assert!(out.images.is_empty());
        assert_eq!(lines_text(&out), vec!["a"]);
    }

    /// `split_top_level_image_units`'s own "a non-comment HTML/InlineHtml event sits alongside the
    /// image — not a pure badge row" branch (lines 1600-1605): a CDATA section containing a literal
    /// `>` before its own real close (`<![CDATA[a>b]]>`) is reported by pulldown-cmark as **one**
    /// `InlineHtml` event spanning the whole thing, but `render_html_block`'s own naive
    /// first-`>`-wins tag stripper reads only `<![CDATA[a` as "the tag" and leaves `b]]>` behind as
    /// visible text — confirmed non-empty, so this is not treated as a harmless comment, and the
    /// whole segment falls through to ordinary paragraph rendering (an `Unavailable`-slot image
    /// placeholder plus the literal fallback text, never a block-image placement).
    #[test]
    fn image_next_to_non_comment_html_is_not_folded_into_a_badge_row() {
        let out = render("![a](u.png) <![CDATA[a>b]]>\n", true, false);
        assert!(
            out.images.is_empty(),
            "html with real visible content should block badge-row extraction: {:?}",
            out.images
        );
    }

    /// `split_top_level_image_units`'s own "unbalanced within this segment" (line 1616) — an
    /// `Image` `Start` with no matching `End` anywhere in the slice. Direct, low-level (real
    /// `Doc::parse` output is always balanced).
    #[test]
    fn split_top_level_image_units_on_an_unbalanced_image_is_none() {
        let image_tag = Tag::Image {
            link_type: pulldown_cmark::LinkType::Inline,
            dest_url: "u".into(),
            title: "".into(),
            id: "".into(),
        };
        let events: Vec<(Event<'_>, Range<usize>)> = vec![
            (Event::Start(image_tag), 0..3),
            (Event::Text("a".into()), 1..2),
        ];
        assert_eq!(split_top_level_image_units("src", &events), None);
    }

    // ---- `paragraph_as_block_images`/`heading_trailing_images`: empty ranges and an unbalanced
    // heading badge (lines 1720, 1791, 1811-1815, 1820, 1826) ----

    /// `paragraph_as_block_images`'s own `if events.is_empty() { return None; }` (line 1720) —
    /// direct, low-level: an empty inline range never actually reaches this function through
    /// `render_paragraph_dispatch` (a real `Paragraph` always has at least one inline event).
    #[test]
    fn paragraph_as_block_images_on_an_empty_range_is_none() {
        let doc = Doc::parse("");
        assert_eq!(paragraph_as_block_images(&doc, "", &(0..0)), None);
    }

    /// `heading_trailing_images`'s own identical empty-range guard (line 1791).
    #[test]
    fn heading_trailing_images_on_an_empty_range_is_none() {
        let doc = Doc::parse("");
        assert_eq!(heading_trailing_images(&doc, "", &(0..0)), None);
    }

    /// A heading whose title is followed by a **bare** (not link-wrapped) badge image and then a
    /// trailing HTML comment on the same line — `rav1e`-style trailing badges, but with an inline
    /// comment after the last one too (the shields.io README idiom `heading_trailing_images`'s own
    /// doc comment cites). Exercises the HTML-comment-is-transparent skip (lines 1811-1815, `i +=
    /// 1`, not disqualifying the trailing run) and the bare-`Image` (as opposed to `Link`-wrapped)
    /// `wants_end` arm (line 1820) together: the badge still peels off, the heading's own title text
    /// is what is left.
    #[test]
    fn heading_badge_followed_by_a_comment_still_peels_as_a_trailing_image() {
        let out = render_with_images(
            "## Heading ![`code` alt](u.png) <!-- c -->\n",
            true,
            false,
            5,
            2,
        );
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].url, "u.png");
        assert_eq!(
            lines_text(&out)[0],
            "Heading ",
            "the comment and the badge must not survive as literal title text: {:?}",
            lines_text(&out)
        );
    }

    /// `heading_trailing_images`'s own `Event::Html(_) | Event::InlineHtml(_) if
    /// render_html_block(..).is_empty()` match *guard* evaluating to **false** — the sibling of the
    /// test just above, which only ever exercised the guard's *true* half (a comment, which strips
    /// to nothing). A CDATA section holding a literal `>` before its own real close
    /// (`<![CDATA[a>b]]>` — the identical shape `image_next_to_non_comment_html_is_not_folded_into_
    /// a_badge_row` above already confirms `render_html_block` does not strip cleanly) is reported
    /// by pulldown-cmark as one `InlineHtml` event; failing the guard, it falls through to the
    /// catch-all "real content" arm instead of being skipped as transparent — disqualifying the
    /// badge immediately before it from ever counting as part of a trailing run at all (`split`
    /// never moves off `items.len()`, so nothing peels).
    #[test]
    fn heading_badge_followed_by_non_comment_html_is_not_peeled_as_trailing() {
        let out = render_with_images(
            "## Heading ![badge](u.png) <![CDATA[a>b]]>\n",
            true,
            false,
            5,
            2,
        );
        assert!(
            out.images.is_empty(),
            "the CDATA content disqualifies the badge from the trailing run: {:?}",
            out.images
        );
    }

    /// `heading_trailing_images`'s own `Event::Html(_) | Event::InlineHtml(_) if
    /// render_html_block(..).is_empty()` match guard evaluating to **false**, isolated as a direct,
    /// low-level call — the sibling test just above exercises this same guard through real parsing
    /// (a heading badge followed by non-comment HTML), but that document *also* has to satisfy the
    /// `Event::Start(Tag::Image { .. })`/text-classification machinery around it, which turned out to
    /// leave this one specific line's own region still unattributed by `cargo llvm-cov`'s per-region
    /// tracking. A bare, single-event heading (just the CDATA `InlineHtml`, nothing else) pins the
    /// guard's `false` outcome on its own, unambiguously.
    #[test]
    fn heading_trailing_images_html_guard_false_branch_is_reached_directly() {
        let src = "<![CDATA[a>b]]>";
        assert!(
            !super::super::render_html_block(src).is_empty(),
            "fixture must actually disqualify — otherwise this test proves nothing"
        );
        let events: Vec<(Event<'_>, Range<usize>)> =
            vec![(Event::InlineHtml(src.into()), 0..src.len())];
        let doc = Doc {
            events,
            blocks: Vec::new(),
        };
        assert_eq!(heading_trailing_images(&doc, src, &(0..1)), None);
    }

    /// `heading_trailing_images`'s own "unbalanced within this heading — treat as real content"
    /// fallback (line 1826) — an `Image` `Start` with no matching `End` anywhere in the heading's
    /// own inline range. Direct, low-level construction (real parsing is always balanced): confirms
    /// this degrades to "not a peelable trailing run" (`None`) rather than reading past the end of
    /// `events`.
    #[test]
    fn heading_trailing_images_on_an_unbalanced_heading_gives_up_without_panicking() {
        let image_tag = Tag::Image {
            link_type: pulldown_cmark::LinkType::Inline,
            dest_url: "u".into(),
            title: "".into(),
            id: "".into(),
        };
        let events: Vec<(Event<'_>, Range<usize>)> = vec![
            (Event::Start(image_tag), 0..5),
            (Event::Text("ab".into()), 1..3),
        ];
        let doc = Doc {
            events,
            blocks: Vec::new(),
        };
        assert_eq!(heading_trailing_images(&doc, "abcde", &(0..2)), None);
    }

    // ---- `image_alt_text`: lines 1986-1988 ----

    /// Alt text containing an inline code span (`Event::Code`, line 1986) and a hard break
    /// (`Event::HardBreak`, line 1987; `Event::Text`/`Event::SoftBreak` are already covered
    /// elsewhere) — both concatenated with the `Event::Text` payloads around them, a hard break
    /// becoming a single literal space, mirroring `Writer::join_with_space`'s own rendering (see this
    /// function's own doc comment).
    #[test]
    fn image_alt_text_joins_code_spans_and_hard_breaks_as_plain_text() {
        let out = render_with_images("## Heading ![`code` alt](u.png)\n", true, false, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].alt, "code alt");

        let out2 = render("![line1\\\nline2](u.png)\n", true, false);
        assert!(
            out2.images.is_empty(),
            "no live slot in this call — Unavailable fallback"
        );
        assert!(
            lines_text(&out2)[0].contains("line1 line2"),
            "hard break inside alt text should become one literal space: {:?}",
            lines_text(&out2)
        );
    }

    /// `image_alt_text`'s own catch-all (`_ => {}`, line 1988) — a nested inline construct (bold)
    /// inside a standalone image's own alt text contributes its own `Start`/`End(Strong)` events,
    /// silently dropped (matching this function's own doc comment: alt text carries no styling of
    /// its own, only the plain text/code/breaks inside it).
    #[test]
    fn image_alt_text_drops_nested_style_tags() {
        let out = render_with_images("![**bold** alt](u.png)\n", true, false, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].alt, "bold alt");
    }

    // ---- `render_paragraph_dispatch`/`render_bare_paragraph`: the math-lifting branches of the
    // leading/trailing standalone-image split (lines 2110, 2129, 2138-2139, 4168, 4174-4187,
    // 4194-4207) — and `render_image_group`'s `Unavailable` fallback (`no_images`, lines 4640-4642)
    // ----

    /// A top-level paragraph whose text lifts inline math, immediately followed (no blank line) by
    /// a standalone trailing image, with `math_on: true` — `paragraph_trailing_images` peels the
    /// image, and since `math_here && w.math.is_some()`, the leading text renders through
    /// `render_paragraph_math` (line 2110), not plain `render_paragraph`.
    #[test]
    fn paragraph_with_math_and_a_trailing_image_lifts_the_math_first() {
        let out = render_with_images("$x$ intro\n![badge](u.png)\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].url, "u.png");
        assert!(
            lines_text(&out).iter().any(|l| l.contains('$')),
            "the math must have been lifted (a dimmed raw placeholder), not left as plain text: {:?}",
            lines_text(&out)
        );
    }

    /// A heading (leaving `w.pending_block_gap: true`), then a paragraph whose text is a **leading**
    /// standalone image followed by real text, `math_on: false` — `paragraph_leading_images` peels
    /// the image; since `w.pending_block_gap` is true entering, the leading-blank-line push (line 2129)
    /// fires; since `math_here` is false, the trailing text renders through plain `render_paragraph`
    /// (line 2138-2139), not `render_paragraph_math`.
    #[test]
    fn heading_then_a_leading_image_paragraph_gets_its_own_blank_line_and_plain_text() {
        let out = render_with_images("# T\n\n![badge](u.png)\noutro\n", false, false, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        let texts = lines_text(&out);
        assert_eq!(texts.last().map(String::as_str), Some("outro"));
        // Between the heading's own rule and the image there must be a blank row — proof
        // `emit_blank_row` actually fired for the leading-image branch, not just the heading's own.
        let rule_idx = texts.iter().position(|l| l.starts_with('━')).unwrap();
        assert_eq!(texts[rule_idx + 1], "", "lines: {texts:?}");
    }

    /// A **tight** list item's own *later* (bare, interrupted-by-a-heading) child paragraph whose
    /// entire content is a single standalone image — `try_render_paragraph_as_image` inside
    /// `render_bare_paragraph` returns early (line 4168), before either the trailing- or
    /// leading-image branch below it ever runs.
    #[test]
    fn tight_list_bare_paragraph_that_is_only_an_image_returns_early() {
        let out = render_with_images("- a\n  # h\n  ![badge](u.png)\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].url, "u.png");
    }

    /// **The bug this task exists to fix**: a tight list item's own *first* child paragraph, when it
    /// is nothing but a standalone image (`- ![alt](x)` — the common "one image per bullet" idiom),
    /// used to bypass image extraction entirely. `render_item`'s own marker-row splice (drawing the
    /// item's inline content directly onto the same row as `- `) never called
    /// `try_render_paragraph_as_image`/`paragraph_trailing_images`/`paragraph_leading_images` at
    /// all — only a *later* sibling paragraph in the same item did (see
    /// `tight_list_bare_paragraph_that_is_only_an_image_returns_early`, immediately above), by going
    /// through the ordinary `render_block` dispatch. Before the fix, this rendered nothing but the
    /// literal alt text on the marker's own row (`"- alt"`) and `out.images` stayed empty; pins that
    /// `render_doc` now returns a real `ImagePlacement` for this exact shape instead.
    #[test]
    fn tight_list_first_child_that_is_only_an_image_becomes_a_real_placement() {
        let out = render_with_images("- ![alt](u.png)\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].url, "u.png");
        assert_eq!(out.images[0].alt, "alt");
    }

    /// The identical shape as above, but with **two** consecutive items, each solely an image — pins
    /// that the fix applies uniformly to every item in the list, not just the one first encountered.
    /// `render_item` runs its own marker-row-splice decision fresh for every item (`render_list`'s
    /// own per-item loop), so a fix that only happened to patch some list-level, once-per-list piece
    /// of bookkeeping rather than the splice itself could plausibly repair the first item and leave
    /// every later one broken — this is exactly the "same list, only the first item's image fails"
    /// inconsistency the bug report described, fixed for both positions at once.
    #[test]
    fn two_tight_list_items_each_only_an_image_both_become_real_placements() {
        let out = render_with_images("- ![a](x.png)\n- ![b](y.png)\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 2, "images: {:?}", out.images);
        assert_eq!(out.images[0].url, "x.png");
        assert_eq!(out.images[1].url, "y.png");
    }

    /// The same "first child is only an image" shape, but in a **loose** list (`loose_first == true`,
    /// a real `Tag::Paragraph`, reached through `render_block`'s `is_real_paragraph` arm —
    /// `render_paragraph_dispatch`, not `render_bare_paragraph`) — pins that the fix's `ctx.extract_\
    /// here && paragraph_as_block_images(..).is_some()` re-dispatch check in `render_item` covers
    /// both the tight and loose paths through `render_block`'s own `Paragraph` arm, not just the
    /// tight (bare-paragraph) one every other new test here exercises.
    #[test]
    fn loose_list_first_child_that_is_only_an_image_becomes_a_real_placement() {
        let out = render_with_images("- ![alt](u.png)\n\n- b\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert_eq!(out.images[0].url, "u.png");
    }

    /// The same tight-list, bare-paragraph shape, but with leading real text before the trailing
    /// image, `math_on: true` — `render_bare_paragraph`'s own trailing-image branch's `math_here`
    /// arm (lines 4174-4187, `walk_inline_math` instead of plain `walk_inline`).
    #[test]
    fn tight_list_bare_paragraph_trailing_image_with_math_on_uses_the_math_walk() {
        let out = render_with_images("- a\n  # h\n  intro\n  ![badge](u.png)\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert!(lines_text(&out).iter().any(|l| l == "intro"));
    }

    /// The mirror shape — leading image, trailing real text — `render_bare_paragraph`'s own
    /// leading-image branch's `math_here` arm (lines 4193-4207).
    #[test]
    fn tight_list_bare_paragraph_leading_image_with_math_on_uses_the_math_walk() {
        let out = render_with_images("- a\n  # h\n  ![badge](u.png)\n  outro\n", true, true, 5, 2);
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert!(lines_text(&out).iter().any(|l| l == "outro"));
    }

    /// The identical trailing-image bare-paragraph shape as above, but with `math_on: false` — the
    /// `else` half of `render_bare_paragraph`'s own trailing-image branch (plain `walk_inline`
    /// instead of `walk_inline_math`, line 4182).
    #[test]
    fn tight_list_bare_paragraph_trailing_image_with_math_off_uses_the_plain_walk() {
        let out = render_with_images(
            "- a\n  # h\n  intro\n  ![badge](u.png)\n",
            true,
            false,
            5,
            2,
        );
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert!(lines_text(&out).iter().any(|l| l == "intro"));
    }

    /// The identical leading-image bare-paragraph shape, `math_on: false` — the `else` half of
    /// `render_bare_paragraph`'s own leading-image branch (line 4205).
    #[test]
    fn tight_list_bare_paragraph_leading_image_with_math_off_uses_the_plain_walk() {
        let out = render_with_images(
            "- a\n  # h\n  ![badge](u.png)\n  outro\n",
            true,
            false,
            5,
            2,
        );
        assert_eq!(out.images.len(), 1, "images: {:?}", out.images);
        assert!(lines_text(&out).iter().any(|l| l == "outro"));
    }

    /// `no_images` itself (lines 4640-4642) — every other test in this file that has no image of
    /// its own to exercise reaches for `&no_images` as a filler argument, but none of them ever
    /// actually *render* a real standalone image through it (all of `render.rs`'s own pre-existing
    /// image tests use a live `ImageSlot::Inline`-returning closure instead — see e.g. the
    /// `slot_of` closures a few hundred lines above). A standalone image rendered with `no_images`
    /// as the real slot answer is what finally calls its body, and confirms `render_image_group`'s
    /// own `Unavailable` arm degrades to the one-line text fallback rather than reserving any rows.
    #[test]
    fn a_standalone_image_with_no_live_slot_degrades_to_the_text_fallback() {
        let out = render("![alt](u.png)\n", true, false);
        assert!(out.images.is_empty());
        assert_eq!(lines_text(&out).len(), 1);
        assert!(
            lines_text(&out)[0].contains("alt") && lines_text(&out)[0].contains("u.png"),
            "lines: {:?}",
            lines_text(&out)
        );
    }

    // ---- Math walk: multi-line display-math bounds, growing-slice resolution/give-up exits with
    // a preceding `pending` text (lines 2512, 2563, 2774-2778, 2797-2814, 2994, 3053) ----

    /// `multiline_math_opener`'s own "this opener's own line is the very last line of `src`, with
    /// no trailing newline at all" branch (line 2511-2512, `bounds.end` unchanged rather than `+
    /// 1`) — direct, low-level: the private helper takes only `src`/a byte range, no `Doc` needed.
    #[test]
    fn multiline_math_opener_at_the_very_end_of_input_with_no_trailing_newline() {
        let src = "abc\n$$";
        let range = 4..6; // the "$$" itself
        assert_eq!(&src[range.clone()], "$$");
        let (closer, body_start) = multiline_math_opener(src, &range).expect("opener recognized");
        assert_eq!(closer, "$$");
        assert_eq!(
            body_start,
            src.len(),
            "no `+ 1`: there is no byte after this line at all"
        );
    }

    /// `render_multiline_display_math`'s own "ran out of events" exit (lines 2560-2567), with a
    /// non-empty `pending` text carried in (line 2562-2563 specifically — the sibling test for the
    /// *same* exit with `pending: None` is already covered by an existing regression elsewhere in
    /// this file). Direct, low-level call: getting `pending` to survive into this exact call via
    /// real parsing needs two back-to-back `Event::Text`s with nothing between them (the shape a
    /// caret/tilde delimiter ambiguity sometimes produces — see the superscript/subscript note
    /// above) whose *second* one is, byte for byte, a whole physical line reading exactly `"$$"` —
    /// a combination real documents essentially never produce; calling the function directly proves
    /// the code path itself is sound regardless.
    #[test]
    fn render_multiline_display_math_ran_out_of_events_flushes_a_pending_text_first() {
        let mut w = fresh_writer();
        let mut events = std::iter::empty();
        let mut prev_end = Some(0usize);
        render_multiline_display_math(
            &mut w,
            &mut events,
            Some(pulldown_cmark::CowStr::from("lead ")),
            pulldown_cmark::CowStr::from("$$"),
            2,
            "$$",
            "$$\n",
            &mut prev_end,
        );
        assert_eq!(
            lines_text(&RenderOut {
                lines: w.lines,
                images: Vec::new(),
                unsupported: Vec::new(),
                code_blocks: Vec::new(),
                tasks: Vec::new(),
            }),
            vec!["lead $$"],
            "both the flushed pending text and the un-closed opener replay onto one line"
        );
    }

    /// `render_multiline_display_math`'s own **success** exit (`is_closer_line`, lines 2577-2584)
    /// with a non-empty `pending` text (line 2579-2580) — the sibling of the "ran out of events"
    /// test just above: here the closer line genuinely turns up, so `pending` flushes with
    /// `trailing_lift: true` before the display-math placeholder itself renders (via
    /// `render_math_slot`, which degrades to nothing here since `w.math` is `None` — already
    /// covered on its own above; what this test is actually proving is that the pending flush
    /// happens at all). Direct, low-level construction, for the identical reason the "ran out of
    /// events" sibling test is: real parsing essentially never produces two adjacent `Text` events
    /// where the second is a whole physical line reading exactly `"$$"`.
    #[test]
    fn render_multiline_display_math_closer_found_flushes_a_pending_text_first() {
        let mut w = fresh_writer();
        let src = "$$\nbody\n$$\n";
        let events: Vec<(Event<'_>, Range<usize>)> = vec![
            (Event::Text("body".into()), 3..7),
            (Event::Text("$$".into()), 8..10),
        ];
        let mut it = events.into_iter();
        let mut prev_end = Some(0usize);
        render_multiline_display_math(
            &mut w,
            &mut it,
            Some(pulldown_cmark::CowStr::from("lead ")),
            pulldown_cmark::CowStr::from("$$"),
            3,
            "$$",
            src,
            &mut prev_end,
        );
        assert_eq!(
            w.lines,
            // `trailing_lift: true` (a lift immediately follows) trims the trailing space —
            // `render_math_parts`'s own trim rule, exercised here too.
            vec![Line::from(vec![Span::raw("lead")])],
            "the pending text must flush before the (here, no-op) math slot renders"
        );
    }

    /// `render_backslash_math`'s own `is_break` exit (lines 2771-2779) with a non-empty `pending`
    /// text (lines 2774-2775) — a backslash-math opener (`\(`) that never finds its own closer
    /// before a hard line break, preceded by ordinary text on the *same* physical line (the
    /// preceding-adjacent-`Text`-event shape a backslash escape reliably produces — see
    /// `backslash_math_opener`'s own doc comment). The buffered fallback replayed through
    /// `replay_events` also carries an inline code span (`Event::Code`, lines 2797-2800), a raw
    /// inline HTML tag (dropped by `replay_events`'s own catch-all, line 2814), and the triggering
    /// `HardBreak` itself (lines 2805-2808) — so this one document exercises `replay_events`'s
    /// entire event-kind coverage in a single pass.
    #[test]
    fn backslash_math_opener_that_never_closes_replays_everything_it_buffered() {
        let out = render("lead \\(a `code` <b> more  \nend\n", true, true);
        assert!(
            out.images.is_empty(),
            "nothing here should have been recognized as math: {:?}",
            out.images
        );
        let texts = lines_text(&out);
        assert_eq!(texts.len(), 2, "lines: {texts:?}");
        assert_eq!(texts[0], "lead (a code  more");
        assert_eq!(texts[1], "end");
    }

    /// `render_dollar_math_tail`'s own "resolved" exit (lines 2986-3001) with a non-empty `pending`
    /// text (line 2993-2994) — a dollar-math span that only closes after growing *past* a nested
    /// `Strong` span (so the growing-raw-slice re-scan, not a single-event scan, is what actually
    /// resolves it — see this function's own "Never stop mid-construct" doc comment), preceded by
    /// ordinary text on the same physical line via the identical backslash-escape adjacency trick
    /// used above.
    #[test]
    fn dollar_math_that_resolves_across_a_nested_strong_span_flushes_a_pending_text_first() {
        let out = render("lead \\*$a **b** c$ tail\n", true, true);
        let texts = lines_text(&out);
        assert_eq!(texts.len(), 3, "lines: {texts:?}");
        assert_eq!(
            texts[0], "lead *",
            "the pending text must have flushed onto its own line"
        );
        assert!(texts[1].contains("$a **b** c$"), "lines: {texts:?}");
        assert_eq!(texts[2], "tail");
    }

    /// `render_dollar_math_tail`'s own `is_break` exit (lines 3051-3057) with a non-empty `pending`
    /// text (line 3052-3053) — the mirror of the backslash-math case above, but for the
    /// currency-shaped `$…` opener that `dollar_math_still_open` cannot resolve within one event:
    /// preceded by ordinary text on the same physical line, and given up on at the very next
    /// `SoftBreak` (never even reaching the unrelated `end` text on the following line — confirmed
    /// by the fact that everything still renders on one row, `SoftBreak` degrading to a literal
    /// space, not two).
    #[test]
    fn dollar_math_that_never_closes_gives_up_at_the_next_soft_break() {
        let out = render("lead \\*$50 more\nend\n", true, true);
        assert!(
            out.images.is_empty(),
            "nothing should have been lifted: {:?}",
            out.images
        );
        let texts = lines_text(&out);
        assert_eq!(texts, vec!["lead *$50 more end"]);
    }

    // ---- `render_math_slot`/`ensure_fresh_after_math` defensive fallbacks: lines 3092, 3145, 3234
    // ----

    /// `render_math_slot`'s own `let Some(math) = w.math.as_ref() else { return; }` (line 3233-3235)
    /// — every real call site only ever reaches this with `w.math.is_some()` already true (gated by
    /// `render_paragraph_dispatch`'s own check), so this calls it directly on a fresh `Writer`
    /// (`math: None` by construction — see `fresh_writer`'s own doc comment) to confirm the
    /// defensive fallback degrades to "render nothing" rather than panicking on the `unwrap` this
    /// function's own doc comment explains it deliberately does not use.
    #[test]
    fn render_math_slot_with_no_math_context_at_all_renders_nothing() {
        let mut w = fresh_writer();
        render_math_slot(&mut w, "x", false);
        assert!(w.lines.is_empty());
    }

    /// `render_text_with_math`'s own `if i > 0 { emit_blank_row() }` (line 3144-3146) — the
    /// math-aware analogue of `Writer::text`'s identical multi-physical-line handling (already
    /// covered above for the plain, non-math case); direct, low-level call with an embedded `\n`.
    #[test]
    fn render_text_with_math_on_an_embedded_newline_starts_a_fresh_line() {
        let mut w = fresh_writer();
        render_text_with_math(&mut w, "line1\nline2", false);
        assert_eq!(
            lines_text(&RenderOut {
                lines: w.lines,
                images: Vec::new(),
                unsupported: Vec::new(),
                code_blocks: Vec::new(),
                tasks: Vec::new(),
            }),
            vec!["line1", "line2"]
        );
    }

    /// `ensure_fresh_after_math`'s own **separate** `if w.after_math { if w.pending_block_gap { .. } }`
    /// branch (lines 3090-3096), with `pending_block_gap` *and* `after_math` both already `true` on
    /// entry, and `pending_para_start` already `None` — the one state-combination this function's
    /// own doc comment says can legitimately reach it (`pending_para_start`/`after_math` are stated
    /// to be mutually exclusive in practice for real documents: whichever produces `after_math:
    /// true` always resolves `pending_para_start` in the very same step — see `render_math_slot`'s
    /// own unconditional `w.drop_pending_para_start()`). A real document that lands in exactly this
    /// shape (an empty `Quote` — the one construct whose own exit sets `pending_block_gap = true`
    /// *unconditionally*, without going through `emit_row` — immediately following a lift, with a
    /// second math-lifting paragraph after it) was not found; this is a direct, low-level
    /// construction of the state instead, which is honest about that (see this section's own header
    /// comment on why several tests here take this shape).
    #[test]
    fn ensure_fresh_after_math_with_needs_newline_and_after_math_both_set_pushes_two_blanks() {
        let mut w = fresh_writer();
        w.lines.push(Line::from("prior"));
        w.pending_block_gap = true;
        w.after_math = true;
        w.pending_para_start = None;
        ensure_fresh_after_math(&mut w);
        assert_eq!(
            w.lines,
            vec![Line::from("prior"), Line::default(), Line::default()],
            "the inner (pending_block_gap) push and the unconditional second push both fired"
        );
        assert!(!w.after_math, "the flag must be consumed");
        assert!(!w.pending_block_gap);
    }

    // ---- `render_block`/`trim_leading_header`: lines 3797-3798, 3832, 4342, 4408-4410, 4414 ----

    /// `render_block`'s own generic `BlockKind::Quote { alert: Some(_), .. } if !w.alerts` arm
    /// (lines 3797-3798) — the *nested* sibling of the identical top-level-dispatch arm already
    /// covered above (line 466-471): an alert-headed quote reached through a **list item**'s own
    /// content, with `alerts: false`, so it renders as a plain blockquote (the literal `[!NOTE]`
    /// marker text surviving, matching `Writer.alerts`'s own doc comment) rather than the colored
    /// callout box.
    #[test]
    fn alert_headed_quote_nested_inside_a_list_item_with_alerts_off_renders_as_a_plain_quote() {
        let out = render("- item\n\n  > [!NOTE]\n  > text\n", false, false);
        assert!(out.unsupported.is_empty());
        let joined = lines_text(&out).join("\n");
        assert!(joined.contains("[!NOTE]"), "lines: {:?}", lines_text(&out));
        assert!(
            !joined.contains('▌'),
            "no alert bar — this must be a plain quote: {joined:?}"
        );
    }

    /// `render_block`'s own defensive `BlockKind::ListItem { .. } => {}` catch-all (line 3832) —
    /// like the top-level equivalent above, real parsing can never hand this function a raw
    /// `ListItem` (`render_list` always unwraps one first); direct, low-level call confirms it
    /// renders nothing rather than panicking.
    #[test]
    fn render_block_on_a_bare_list_item_that_could_never_really_happen_renders_nothing() {
        let doc = Doc::parse("");
        let mut w = fresh_writer();
        let block = Block {
            kind: BlockKind::ListItem { task: None },
            src: 0..0,
            children: Vec::new(),
        };
        render_block(&block, &mut w, &doc, "", fresh_ctx());
        assert!(w.lines.is_empty());
    }

    /// `line_after`'s own `None => src.len()` fallback (line 4341-4342) — an alert header that is
    /// the very last line of the whole document, with no trailing newline at all.
    #[test]
    fn alert_header_with_no_trailing_newline_at_all_does_not_panic() {
        let out = render("> [!NOTE]", true, false);
        assert!(out.unsupported.is_empty());
        assert!(
            lines_text(&out)[0].contains("Note"),
            "lines: {:?}",
            lines_text(&out)
        );
    }

    /// `trim_leading_header`'s own "straddles the boundary, but is not a `Paragraph`" defensive
    /// fallback (lines 4408-4410, `out.push(b.clone())` — kept unmodified rather than dropped) and
    /// its own final `Vec::new()` for an empty/all-header `children` slice (line 4414). This
    /// function's own doc comment states the straddling-non-`Paragraph` shape "cannot legitimately
    /// straddle this boundary at all" for real parsing (every other block kind starts fresh at its
    /// own line) — direct, low-level calls for both.
    #[test]
    fn trim_leading_header_keeps_a_straddling_non_paragraph_block_unmodified() {
        let doc = Doc::parse("");
        let children = vec![Block {
            kind: BlockKind::ThematicBreak,
            src: 0..10,
            children: Vec::new(),
        }];
        let out = trim_leading_header(&children, &doc, 5);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].kind, BlockKind::ThematicBreak));
        assert_eq!(
            out[0].src,
            0..10,
            "a non-Paragraph straddler is kept byte-for-byte unchanged"
        );
    }

    #[test]
    fn trim_leading_header_on_no_children_at_all_is_empty() {
        let doc = Doc::parse("");
        assert_eq!(trim_leading_header(&[], &doc, 0), Vec::<Block>::new());
    }

    // =========================================================================================
    // Extreme/adversarial input sweep: `render_doc` must never panic, on any of the shapes below.
    // Each case is rendered at several widths (including pathologically narrow ones) and with both
    // `code.wrap` settings; a handful of specific cases from konoma's own history (cited inline) are
    // also pinned individually above the sweep.

    /// Every genuinely adversarial/extreme document this pass collected, run through `render_doc`
    /// at a handful of widths (1, 2, 3, 10, 80, 4000) and both `code.wrap` settings, with math/
    /// alerts both on — the combination most likely to trip an internal invariant. No output is
    /// asserted (the documents are deliberately meaningless); the only contract under test is "does
    /// not panic, for any of these, at any of these widths".
    #[test]
    fn render_doc_never_panics_on_adversarial_input() {
        let cases: Vec<String> = vec![
            // Deeply nested containers (10 levels) of every kind this file covers.
            "> ".repeat(10) + "deep quote\n",
            "- ".repeat(10) + "deep list\n",
            {
                let mut s = String::new();
                for _ in 0..10 {
                    s.push_str("- a\n");
                }
                s
            },
            // Unclosed fences, quotes, emphasis, links, alerts, details — at every depth this file
            // itself dispatches through.
            "```rust\nfn a() {}\n".to_string(),
            "> unclosed quote with no closing anything".to_string(),
            "**unclosed bold".to_string(),
            "*unclosed italic".to_string(),
            "~~unclosed strike".to_string(),
            "[unclosed link(".to_string(),
            "![unclosed image(".to_string(),
            "> [!NOTE] unclosed alert body".to_string(),
            "<details>\n<summary>unclosed".to_string(),
            "<details>\n<summary>s</summary>\nbody with no close at all\n".to_string(),
            "$$unclosed display math".to_string(),
            "\\(unclosed backslash math".to_string(),
            "$unclosed dollar".to_string(),
            // A single, enormous line — 300,000 bytes, no newline anywhere (stresses width-based
            // wrapping/highlighting at extreme scale without allocating hundreds of MB).
            "x".repeat(300_000),
            // Thousands of blank lines back to back.
            "\n".repeat(5_000),
            // CJK, emoji, combining marks, zero-width characters, mixed into every construct.
            "# 見出し ✅🇯🇵\u{200B}\u{0301}\n".to_string(),
            "これは*強調*と**太字**と`コード`です。\n".to_string(),
            "- 項目1\n- 項目2\u{200D}\u{FE0F}\n".to_string(),
            "> 引用文\u{0301}です\n".to_string(),
            "![代替テキスト](画像.png)\n".to_string(),
            "```rust\nlet 変数 = \"文字列\u{200B}\";\n```\n".to_string(),
            // CRLF and a leading UTF-8 BOM.
            "line one\r\nline two\r\n\r\n# heading\r\n".to_string(),
            "\u{FEFF}# heading with a BOM\n".to_string(),
            // Control characters and literal tabs mixed into ordinary text and code fences.
            "text with a \u{0007} bell and a \u{001B} escape\n".to_string(),
            "\tindented with a real tab\nnot indented\n".to_string(),
            "```\n\tfenced code with a tab\n```\n".to_string(),
            // Every `\`-escapable character from the module's own escape corpus, immediately
            // followed by a multibyte character each time — the exact shape that has panicked this
            // file before on a byte-boundary slice (`escape then CJK`/`escape then emoji`).
            "\\*あ \\_い \\[う \\]え \\(お \\)か \\$き \\\\く\n".to_string(),
            "\\*🎉 \\_🎉 \\[🎉 \\]🎉 \\(🎉 \\)🎉 \\$🎉 \\\\🎉\n".to_string(),
            "\\あ \\🎉 \\\u{200B}\n".to_string(),
            // A currency `$` immediately followed by bold text, on multiple lines and inside a
            // list item — the exact shape `render_dollar_math_tail`'s own doc comment cites as a
            // real, confirmed pre-fix panic (`"It costs $50 **bold** text.\n"`).
            "It costs $50 **bold** text.\n".to_string(),
            "- It costs $50 **bold** text.\n".to_string(),
            "> It costs $50 **bold** text.\n".to_string(),
            // A backslash-escaped bracket immediately followed by a real link — the confirmed
            // pre-fix panic `render_backslash_math`'s own doc comment cites.
            "See \\([#519](https://example.com/pull/519))\n".to_string(),
            // Table with ragged/empty rows and CJK cells, immediately touching a fence with no
            // blank line.
            "| a | b |\n|---|---|\n| | 日本語 |\n```\ncode\n```\n".to_string(),
            // A checkbox marker as the very first byte of the document, and a custom task state.
            "- [ ] first\n".to_string(),
            "- [x] first\n".to_string(),
            "- [/] custom\n".to_string(),
            // Every inline construct packed onto a single line together.
            "# H *em* **st** ~~s~~ `c` [l](u) ![i](u) ^sup^ ~sub~ \\* end\n".to_string(),
        ];
        let code_wrap_true = CodeStyle {
            wrap: true,
            ..CodeStyle::default()
        };
        let code_wrap_false = CodeStyle {
            wrap: false,
            ..CodeStyle::default()
        };
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 4, rows: 2 };
        for src in &cases {
            let doc = Doc::parse(src);
            for width in [1u16, 2, 3, 10, 80, 4000] {
                for code in [code_wrap_true, code_wrap_false] {
                    let _ = render_doc(
                        &doc,
                        src,
                        width,
                        code,
                        "TwoDark",
                        true,
                        &[' ', 'x', '/'],
                        &slot_of,
                        &no_mermaid,
                        "Enter: full screen",
                        true,
                        &math_slot,
                        true,
                    );
                }
            }
        }
    }

    /// `code.tab_width` swept across its own extremes (0 = disabled, and a pathologically large
    /// value) against a fenced code block whose body actually contains tab characters — a separate,
    /// focused sweep from the general adversarial one above because a bad `tab_width` interacts with
    /// `highlight_body`'s own column math specifically, not with parsing.
    #[test]
    fn render_doc_never_panics_across_extreme_tab_widths() {
        let src = "```rust\n\tfn a() {\n\t\tb();\n\t}\n```\n";
        let doc = Doc::parse(src);
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        for tab_width in [0usize, 1, 100] {
            let code = CodeStyle {
                tab_width,
                ..CodeStyle::default()
            };
            let _ = render_doc(
                &doc,
                src,
                80,
                code,
                "TwoDark",
                false,
                &[' ', 'x'],
                &no_images,
                &no_mermaid,
                "Enter: full screen",
                true,
                &math_slot,
                false,
            );
        }
    }

    /// Deeply nested inline emphasis (10 levels of `*`/`_`/`~~` interleaved) — a shape aimed
    /// squarely at `walk_inline`'s own recursive `Start`/`End` balancing rather than at block-level
    /// nesting (already covered by the adversarial sweep above).
    #[test]
    fn render_doc_never_panics_on_deeply_nested_inline_emphasis() {
        let mut src = String::new();
        for i in 0..10 {
            src.push_str(if i % 2 == 0 { "*" } else { "_" });
        }
        src.push_str("center");
        for i in (0..10).rev() {
            src.push_str(if i % 2 == 0 { "*" } else { "_" });
        }
        src.push('\n');
        let out = render(&src, true, true);
        assert!(!out.lines.is_empty());
    }

    /// Fixture for the two front-matter tests below: an **un-stripped** YAML metadata block — the
    /// exact shape `render_metadata_block` exists for (`[ui] md_frontmatter = false` hands
    /// `Doc::parse` this source unmodified; see `render_doc`'s own "raw, un-stripped YAML front-matter"
    /// doc comment). Multi-line YAML (exercises `Writer::text`'s own `.lines()` split inside the
    /// block), no blank source line between the closing `---` and the very next block, then an
    /// ordinary heading and paragraph — closely mirrors `e2e_tests::FRONT_MATTER_BODY`'s own shape
    /// (the fixture `e2e_markdown_front_matter_off_leaves_body_intact` — the acceptance test this
    /// change has to keep passing — exercises), so both tests below and that e2e test are really
    /// checking the identical handoff from `render_metadata_block` into the ordinary block loop.
    const FRONT_MATTER_SRC: &str = concat!(
        "---\n",
        "title: My Doc\n",
        "author: Me\n",
        "---\n",
        "body text\n",
        "\n",
        "# Heading\n",
        "\n",
        "more text\n",
    );

    /// `render_doc` no longer reports a leading, un-stripped metadata block as `Unsupported` — see the
    /// module doc comment's own "raw, un-stripped YAML front-matter" section and `render_metadata_block`'s
    /// own doc comment. Before that change, this exact fixture's `unsupported` was `vec!["MetadataBlock"]`
    /// and `render_markdown_with_images` (`markdown.rs`) fell back to the legacy renderer for the *whole*
    /// document whenever `[ui] md_frontmatter = false`; the legacy, now-deleted parity harness's own
    /// corpus check (`unsupported: 0` across the whole corpus) was the whole-corpus version of this
    /// same assertion — this is the narrow, single-fixture pin for it.
    #[test]
    fn render_doc_front_matter_is_no_longer_unsupported() {
        let doc = Doc::parse(FRONT_MATTER_SRC);
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            FRONT_MATTER_SRC,
            80,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &no_images,
            &no_mermaid,
            "Enter: full screen",
            true,
            &math_slot,
            true,
        );
        assert!(
            out.unsupported.is_empty(),
            "a leading metadata block should render directly, not fall back: {:?}",
            out.unsupported
        );
        // Sanity: the metadata block's own content actually made it into `out.lines` (not silently
        // dropped along with `unsupported` — the exact failure mode this change fixed; see
        // `render_doc`'s own doc comment on the bug this closed).
        assert!(
            out.lines.iter().any(|l| l
                .spans
                .iter()
                .any(|s| s.content.as_ref() == "title: My Doc")),
            "front matter content missing from render_doc's own output: {:?}",
            out.lines
        );
    }
}

/// Real pixels inside a table cell — the reserved rectangle, the `ImagePlacement` recorded over it,
/// and every way that reservation can be forced to fall back to the `🖼 alt` label instead.
///
/// A sibling of `mod tests` rather than a section inside it: these all drive one purpose-built
/// `slot_of` (`slot`, below) that answers all three `ImageSlot` variants and re-fits to the width it
/// is asked for, whereas `mod tests`'s own helpers are deliberately built around a `slot_of` that
/// never draws anything (`no_images`), and every table test in there depends on that.
#[cfg(test)]
mod cell_image_tests {
    use super::*;
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    /// The natural width, in cells, `slot` reports for a drawable image when nothing constrains it.
    const NAT_COLS: u16 = 40;

    /// Stands in for the app's own image lookup (`app::md_render`'s closure over
    /// `app::md_image_cells`) with all three answers reachable from the URL alone: `https://…` is
    /// still downloading (`Loading`), anything named `missing` cannot be read (`Unavailable`), and
    /// everything else is a drawable 5:1 picture fitted to whatever width the caller asks for —
    /// downscaled, never upscaled, exactly like `md_image_cells`.
    fn slot(url: &str, max_cols: Option<u16>) -> ImageSlot {
        if url.starts_with("https://") {
            return ImageSlot::Loading;
        }
        if url.contains("missing") {
            return ImageSlot::Unavailable;
        }
        let cols = max_cols.unwrap_or(NAT_COLS).clamp(1, NAT_COLS);
        ImageSlot::Inline {
            cols,
            rows: (cols as u32).div_ceil(5) as u16,
        }
    }

    fn render(src: &str, width: u16) -> RenderOut {
        render_with(src, width, &slot)
    }

    /// `render`, with the image lookup left to the caller — the one thing `slot` cannot express is a
    /// document whose images are **different sizes from one another** (it answers `NAT_COLS` for
    /// every URL alike), which is exactly what a cell holding several pictures needs. `sized_slot`
    /// is that caller; nothing else about the render changes.
    fn render_with(
        src: &str,
        width: u16,
        slot_of: &dyn Fn(&str, Option<u16>) -> ImageSlot,
    ) -> RenderOut {
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let doc = Doc::parse(src);
        let out = render_doc(
            &doc,
            src,
            width,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            slot_of,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert!(
            out.unsupported.is_empty(),
            "unsupported: {:?}",
            out.unsupported
        );
        out
    }

    /// Like `slot`, but the natural width is read off the URL itself — `w20.png` is 20 columns wide
    /// standing on its own, `w10.png` is 10 — so one cell can hold pictures of **different** sizes.
    /// Same 5:1 aspect and the same downscale-never-upscale re-fit `slot` performs, and `missing…`
    /// still answers `Unavailable`, so a cell can mix drawable and undrawable images.
    fn sized_slot(url: &str, max_cols: Option<u16>) -> ImageSlot {
        if url.contains("missing") {
            return ImageSlot::Unavailable;
        }
        let nat: u16 = url
            .strip_prefix('w')
            .and_then(|s| s.split('.').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(NAT_COLS)
            .max(1);
        let cols = max_cols.unwrap_or(nat).clamp(1, nat);
        ImageSlot::Inline {
            cols,
            rows: (cols as u32).div_ceil(5) as u16,
        }
    }

    /// Every rendered line's own visible text.
    fn texts(out: &RenderOut) -> Vec<String> {
        out.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// The `len` display columns of `s` starting at column `from` (CJK/emoji counted as 2, the same
    /// way every width in the table layout is) — how these tests read the cells a placement claims.
    fn slice_cols(s: &str, from: usize, len: usize) -> String {
        let mut out = String::new();
        let mut col = 0usize;
        for ch in s.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col >= from && col + w <= from + len {
                out.push(ch);
            }
            col += w;
        }
        out
    }

    /// The invariant every cell image has to satisfy, whatever produced it: the rectangle the
    /// renderer *reported* is the rectangle it actually *drew*. For each placement — the reserved
    /// rows all exist, the top one carries the `🖼` label at exactly the reported column, every row
    /// below it is blank across the reported width, each of those rows still starts and ends on the
    /// table's own border bar, and the rectangle fits inside the line.
    ///
    /// This is what catches a column-shaving or padding miscount: a rectangle reported one cell off
    /// lands on a border bar or on a neighbouring cell's text, and both show up here.
    fn assert_reserved(out: &RenderOut) {
        let lines = texts(out);
        assert!(!out.images.is_empty(), "配置が 1 件も無い: {lines:?}");
        for p in &out.images {
            let (col, cols, rows) = (p.col as usize, p.cols as usize, p.rows as usize);
            assert!(
                p.line + rows <= lines.len(),
                "予約行が行数を超えている {p:?}: {lines:?}"
            );
            let top = &lines[p.line];
            assert!(
                slice_cols(top, col, cols).starts_with('🖼'),
                "予約矩形の左上に 🖼 ラベルが無い {p:?}: |{top}|"
            );
            for r in 0..rows {
                let line = &lines[p.line + r];
                assert!(
                    UnicodeWidthStr::width(line.as_str()) >= col + cols,
                    "行が予約矩形より狭い {p:?}: |{line}|"
                );
                assert!(
                    line.starts_with('│') && line.ends_with('│'),
                    "予約行の罫線が壊れている {p:?}: |{line}|"
                );
                if r > 0 {
                    assert_eq!(
                        slice_cols(line, col, cols),
                        " ".repeat(cols),
                        "予約矩形の 2 行目以降は空白でなければならない {p:?}: |{line}|"
                    );
                }
            }
        }
    }

    /// Every line of the drawn table is the same display width — the check that a reserved rectangle
    /// has not silently pushed a row wider (or narrower) than its neighbours.
    fn assert_rectangular(out: &RenderOut) {
        let lines: Vec<String> = texts(out)
            .into_iter()
            .filter(|l| l.starts_with('│') || l.starts_with('┌') || l.starts_with('└'))
            .collect();
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| UnicodeWidthStr::width(l.as_str()))
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "表の行幅が揃っていない {widths:?}: {lines:?}"
        );
    }

    // ---- The two table paths ----

    #[test]
    fn a_gfm_cell_image_reserves_a_rectangle_and_records_a_placement() {
        let src = "| name | image |\n|---|---|\n| png | ![alt text](a.png) |\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        assert_eq!(p.url, "a.png", "セル画像の URL がそのまま渡る");
        assert_eq!(p.alt, "alt text");
        assert_eq!(p.fence_ord, None, "セル画像は mermaid フェンスではない");
        assert_eq!((p.cols, p.rows), (NAT_COLS, 8), "自然サイズのまま入る");
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    #[test]
    fn an_html_cell_image_reserves_a_rectangle_and_records_a_placement() {
        let src = "<table>\n  <tr><td><img src=\"a.png\" alt=\"A\"></td></tr>\n</table>\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        assert_eq!(out.images[0].url, "a.png");
        assert_eq!(out.images[0].alt, "A");
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    /// The shape this whole feature exists for (konoma's own README): two screenshots side by side,
    /// captions underneath. Both cells get their own rectangle, on the same rows, at different
    /// columns — and the caption row below is ordinary text, not swallowed by the reservation.
    #[test]
    fn two_image_cells_in_one_row_sit_side_by_side_on_the_same_rows() {
        let src = "<table>\n  <tr>\n    <td><img src=\"tree.png\" alt=\"Tree\"></td>\n    <td><img src=\"graph.png\" alt=\"Graph\"></td>\n  </tr>\n  <tr><td align=\"center\">Tree view</td><td align=\"center\">Git graph</td></tr>\n</table>\n";
        let out = render(src, 90);
        assert_eq!(out.images.len(), 2, "{:?}", out.images);
        let (a, b) = (&out.images[0], &out.images[1]);
        assert_eq!(a.line, b.line, "同じ行に並ぶ: {:?}", out.images);
        assert!(
            a.col + a.cols <= b.col,
            "2 枚の矩形が重なっている: {:?}",
            out.images
        );
        assert_reserved(&out);
        assert_rectangular(&out);
        let lines = texts(&out);
        let caption = lines
            .iter()
            .find(|l| l.contains("Tree view"))
            .unwrap_or_else(|| panic!("キャプション行が無い: {lines:?}"));
        assert!(
            caption.contains("Git graph"),
            "キャプション行が 1 行に揃っていない: |{caption}|"
        );
    }

    #[test]
    fn an_image_cell_and_a_text_cell_share_the_rows_the_taller_one_needs() {
        let src = "| a | b |\n|---|---|\n| ![x](a.png) | just text |\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        let lines = texts(&out);
        // The text cell's own content sits on the image's *first* row (both start at the top of the
        // shared row band), and the rows the image still needs below it are drawn as ordinary,
        // fully-bordered cell rows.
        assert!(
            lines[p.line].contains("just text"),
            "テキストセルが行の先頭に無い: |{}|",
            lines[p.line]
        );
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    /// `<td><img …> caption</td>` — a cell holding both. They stack, image first (a cell has no way
    /// to flow text beside a picture), and the text is still drawn.
    #[test]
    fn an_image_and_text_in_one_cell_stack_vertically_image_first() {
        let src =
            "<table>\n  <tr><td><img src=\"a.png\" alt=\"A\"> caption here</td></tr>\n</table>\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        let lines = texts(&out);
        let caption_row = lines
            .iter()
            .position(|l| l.contains("caption here"))
            .unwrap_or_else(|| panic!("キャプションが描かれていない: {lines:?}"));
        assert!(
            caption_row >= p.line + p.rows as usize,
            "キャプションが予約矩形の中に入り込んでいる (行 {caption_row}, 矩形 {p:?})"
        );
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    // ---- Widths ----

    /// The trap the width-budget shaving sets: when the columns are squeezed, the rectangle has to be
    /// recomputed at the width the column actually ended up with. A placement still reporting the
    /// natural width would overhang the border.
    #[test]
    fn a_shaved_column_places_the_image_at_the_final_width_not_the_natural_one() {
        let src = "| a | b |\n|---|---|\n| ![x](a.png) | a fairly long stretch of text |\n";
        let out = render(src, 40);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        assert!(p.cols < NAT_COLS, "狭い幅なのに自然幅のまま: {p:?}");
        assert_eq!(
            p.rows,
            (p.cols as u32).div_ceil(5) as u16,
            "再フィットで縦横比が保たれていない: {p:?}"
        );
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    /// An image is re-fit **only** when it is wider than the column it landed in — an image exactly
    /// as wide as its column already fits and is used as it stands.
    ///
    /// `plan_cell` asks that with `(nat_cols as usize) <= w`, and the `=` is the whole of this test.
    /// Losing it sends every image whose natural width happens to equal its column's final width
    /// back through `slot_of(url, Some(w))` for a second answer — and the contract there is explicit
    /// that a second answer which is not `Inline` degrades the image back to its `🖼 {alt}` label.
    /// So the boundary case is not an optimization: it decides whether that picture is drawn at all
    /// when the app's own lookup can answer differently the second time (an evicted decode answers
    /// `Unavailable`, an in-flight download answers `Loading`).
    ///
    /// The boundary is reached by the ordinary path, not a contrived one: a column with a single
    /// image in it and enough room to hold it is sized *by that image*, so `w == nat_cols` is what
    /// a one-image column normally produces. Both neighbours are checked too — a column widened past
    /// the image by other content (no re-fit) and a column shaved below it (re-fit) — so the test
    /// pins the comparison from both sides rather than only the point itself.
    #[test]
    fn an_image_is_re_fit_only_when_it_is_wider_than_its_column() {
        // `(why, src, pane width, does the renderer have to ask a second time?)`
        let cases = [
            (
                "the column is sized by the image itself, so it fits exactly",
                "| i |\n|---|\n| ![A](w20.png) |\n",
                70u16,
                false,
            ),
            (
                "a header wider than the image leaves the column with room to spare",
                "| a header much wider than the picture |\n|---|\n| ![A](w20.png) |\n",
                70,
                false,
            ),
            (
                "the pane squeezes the column below the image's natural width",
                "| i |\n|---|\n| ![A](w40.png) |\n",
                20,
                true,
            ),
        ];
        for (why, src, width, must_refit) in cases {
            // Every `max_cols` the render asked for, in order. `sized_slot`'s own answers, so the
            // layout is exactly the one every other test in this module renders under.
            let asked = std::cell::RefCell::new(Vec::<Option<u16>>::new());
            let recording = |url: &str, max_cols: Option<u16>| {
                asked.borrow_mut().push(max_cols);
                sized_slot(url, max_cols)
            };
            let out = render_with(src, width, &recording);
            let refits: Vec<u16> = asked.borrow().iter().flatten().copied().collect();
            assert_eq!(
                !refits.is_empty(),
                must_refit,
                "{why}: 再フィットの問い合わせ {refits:?} が想定と違う"
            );
            assert_eq!(out.images.len(), 1, "{why}: {:?}", out.images);
            assert_reserved(&out);
            assert_rectangular(&out);

            // The same render with a lookup that refuses to answer a *second* time. An image that
            // is never re-asked is unaffected; one that is re-asked loses its rectangle and falls
            // back to its `🖼 {alt}` label — which is what makes the boundary observable on screen
            // rather than only in the call log.
            let once = |url: &str, max_cols: Option<u16>| match max_cols {
                None => sized_slot(url, None),
                Some(_) => ImageSlot::Unavailable,
            };
            let out = render_with(src, width, &once);
            assert_eq!(
                out.images.len(),
                usize::from(!must_refit),
                "{why}: 2 回目に答えない lookup での配置数が想定と違う: {:?}",
                out.images
            );
        }
    }

    /// A cell image never widens the table past the pane: the shaving still runs, unchanged.
    #[test]
    fn a_cell_image_never_pushes_the_table_past_the_pane_width() {
        for width in [20u16, 30, 45, 70, 120] {
            let src = "| a | b |\n|---|---|\n| ![x](a.png) | ![y](b.png) |\n";
            let out = render(src, width);
            for line in texts(&out) {
                assert!(
                    UnicodeWidthStr::width(line.as_str()) <= width as usize,
                    "幅 {width} を超えた行がある: |{line}|"
                );
            }
            assert_reserved(&out);
            assert_rectangular(&out);
        }
    }

    /// A cell that declares `align="center"` centers the **rectangle**, not merely text — so the
    /// alignment padding has to be counted into the reported column. Reporting the column without it
    /// would draw every centered README screenshot flush against its left border while the layout
    /// underneath said otherwise.
    #[test]
    fn a_centered_cell_image_is_reported_at_the_column_it_was_centered_to() {
        let src = "<table>\n  <tr><td align=\"center\"><img src=\"a.png\" alt=\"A\"></td></tr>\n  <tr><td align=\"center\">a caption that runs quite a lot wider than the image</td></tr>\n</table>\n";
        let out = render(src, 80);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        assert!(
            p.col > 2,
            "中央寄せの余白が桁に反映されていない (左端は 2): {p:?}"
        );
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    /// The GFM spelling of the same thing (`:---:` on the delimiter row), which reaches the identical
    /// padding through a *column* alignment rather than a cell one.
    #[test]
    fn a_center_aligned_gfm_column_centers_its_cell_image_too() {
        let src = "| shot |\n|:---:|\n| ![x](a.png) |\n| a caption that runs quite a lot wider than the image |\n";
        let out = render(src, 80);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        assert!(out.images[0].col > 2, "{:?}", out.images);
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    // ---- Falling back to the label (principle #3) ----

    #[test]
    fn a_missing_image_keeps_its_label_and_records_no_placement() {
        let src = "| a |\n|---|\n| ![gone](missing.png) |\n";
        let out = render(src, 70);
        assert!(
            out.images.is_empty(),
            "描けない画像に矩形を予約している: {:?}",
            out.images
        );
        let lines = texts(&out);
        assert!(
            lines.iter().any(|l| l.contains("🖼 gone")),
            "ラベルへの退行が消えている: {lines:?}"
        );
    }

    #[test]
    fn a_remote_image_still_fetching_keeps_its_label_and_records_no_placement() {
        let src = "| a |\n|---|\n| ![badge](https://example.com/b.svg) |\n";
        let out = render(src, 70);
        assert!(out.images.is_empty(), "{:?}", out.images);
        let lines = texts(&out);
        assert!(
            lines.iter().any(|l| l.contains("🖼 badge")),
            "取得前のラベルが出ていない: {lines:?}"
        );
    }

    /// The whole table with nothing drawable is byte-for-byte the pre-feature rendering — the
    /// property the golden snapshots (which render every case with an `Unavailable` slot) rest on.
    #[test]
    fn a_table_with_nothing_drawable_renders_exactly_as_it_did_before() {
        let src = "| a | b |\n|---|---|\n| ![gone](missing.png) | text |\n";
        let with_slot = texts(&render(src, 70));
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let doc = Doc::parse(src);
        let plain = render_doc(
            &doc,
            src,
            70,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str, _: Option<u16>| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &math_slot,
            false,
        );
        assert_eq!(
            with_slot,
            texts(&plain),
            "描けない画像しか無い表の描画が変わっている"
        );
    }

    /// A badge written as a link wrapping an image (`[![alt](img)](href)`, the README badge-row
    /// idiom) is a *link*, not a picture — `parse_cell_segments` folds it into one `Link` segment
    /// before this feature ever sees it, and it must stay that way: turning badge tables into rows
    /// of reserved rectangles would be a regression, not a fix.
    #[test]
    fn a_link_wrapped_badge_stays_a_link_and_reserves_nothing() {
        let src = "| ci |\n|---|\n| [![build](b.svg)](https://ci.example.com) |\n";
        let out = render(src, 70);
        assert!(
            out.images.is_empty(),
            "バッジのリンクを画像として予約している: {:?}",
            out.images
        );
        let lines = texts(&out);
        assert!(
            lines.iter().any(|l| l.contains("🖼 build")),
            "バッジのラベルが消えている: {lines:?}"
        );
    }

    // ---- Shapes the corpus would otherwise not cover ----

    #[test]
    fn an_svg_cell_image_is_drawn_like_any_other() {
        let src = "| logo |\n|---|\n| ![logo](logo.svg) |\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        assert_eq!(out.images[0].url, "logo.svg");
        assert_reserved(&out);
    }

    /// Two cells naming the *same* file each get their own placement (they are two rectangles on
    /// screen, in two different places). Both are the same size here because both columns end up the
    /// same width — which is what keeps `App::md_image_cache`, whose entry holds a single encoded
    /// protocol per path, from being asked for two different encodes of one file at once. See
    /// `CellSeg::Image`'s own doc comment.
    #[test]
    fn the_same_image_in_two_cells_gets_one_placement_each() {
        let src = "| a | b |\n|---|---|\n| ![x](same.png) | ![x](same.png) |\n";
        let out = render(src, 90);
        assert_eq!(out.images.len(), 2, "{:?}", out.images);
        assert_eq!(out.images[0].url, out.images[1].url);
        assert_ne!(
            out.images[0].col, out.images[1].col,
            "同じ画像の 2 枚が同じ桁に重なっている: {:?}",
            out.images
        );
        assert_eq!(
            (out.images[0].cols, out.images[0].rows),
            (out.images[1].cols, out.images[1].rows),
            "等幅の列に入った同じ画像のサイズが食い違っている: {:?}",
            out.images
        );
        assert_reserved(&out);
    }

    #[test]
    fn a_header_cell_image_is_drawn_too() {
        let src = "| ![h](head.png) | b |\n|---|---|\n| x | y |\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        assert_reserved(&out);
        assert_rectangular(&out);
        let lines = texts(&out);
        assert!(
            lines.iter().any(|l| l.starts_with('├')),
            "ヘッダ罫線が消えている: {lines:?}"
        );
    }

    #[test]
    fn a_one_by_one_image_only_table_is_all_rectangle() {
        let src = "<table><tr><td><img src=\"only.png\" alt=\"Only\"></td></tr></table>\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        assert_eq!(out.images[0].rows, 8, "{:?}", out.images);
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    // ---- Nesting: the placement has to be rebased onto the outer writer ----

    /// A table inside a `<details>` renders into an isolated sub-`Writer` whose rows start at 0 and
    /// whose columns start inside the `▏ ` bar. `rebase_nested_images` moves the placement onto the
    /// outer writer's own coordinates; without it the image would be drawn at the top of the document
    /// and two columns to the left. `assert_reserved` proves the rebase landed on the real rectangle.
    #[test]
    fn a_table_inside_details_places_its_cell_image_where_it_was_drawn() {
        crate::preview::markdown::set_details_open(vec![true]);
        let src = "intro\n\n<details open>\n<summary>S</summary>\n\n| a |\n|---|\n| ![x](a.png) |\n\n</details>\n";
        let out = render(src, 70);
        crate::preview::markdown::set_details_open(Vec::new());
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        assert!(p.line > 1, "文書先頭に貼り付いている: {p:?}");
        let lines = texts(&out);
        let top = &lines[p.line];
        assert!(
            slice_cols(top, p.col as usize, p.cols as usize).starts_with('🖼'),
            "入れ子の桁補正が効いていない {p:?}: |{top}|"
        );
        for r in 1..p.rows as usize {
            let line = &lines[p.line + r];
            assert_eq!(
                slice_cols(line, p.col as usize, p.cols as usize),
                " ".repeat(p.cols as usize),
                "入れ子の予約矩形が空白でない {p:?}: |{line}|"
            );
        }
    }

    /// The same, for a plain blockquote (`>` prefix). Its own bar is two cells wide too, so the same
    /// rebase applies — checked separately because the two containers merge their sub-writer through
    /// entirely different code (`render_quote` vs `render_bar_prefixed_body`).
    #[test]
    fn a_table_inside_a_quote_places_its_cell_image_where_it_was_drawn() {
        let src = "intro\n\n> | a |\n> |---|\n> | ![x](a.png) |\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        assert!(p.line > 1, "文書先頭に貼り付いている: {p:?}");
        let lines = texts(&out);
        let top = &lines[p.line];
        assert!(top.starts_with('>'), "引用の中の表になっていない: |{top}|");
        assert!(
            slice_cols(top, p.col as usize, p.cols as usize).starts_with('🖼'),
            "引用の桁補正が効いていない {p:?}: |{top}|"
        );
        for r in 1..p.rows as usize {
            let line = &lines[p.line + r];
            assert_eq!(
                slice_cols(line, p.col as usize, p.cols as usize),
                " ".repeat(p.cols as usize),
                "引用の予約矩形が空白でない {p:?}: |{line}|"
            );
        }
    }

    /// A cell image is not a heading rule, but it lands *after* headings whose rules the final
    /// decoration pass inserts — the same `heading_rule_shift` conversion every other placement
    /// needs. Without it the rectangle would be reported one row short per preceding level-1/2
    /// heading.
    #[test]
    fn a_cell_image_after_headings_accounts_for_the_rules_decoration_inserts() {
        let src = "# One\n\n## Two\n\n| a |\n|---|\n| ![x](a.png) |\n";
        let out = render(src, 70);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        assert_reserved(&out);
    }

    // ---- The label belongs to the rectangle, not to the alt text ----

    /// The dim `🖼 {alt}` label on a reserved rectangle's top row is cut to **the rectangle's own
    /// width**, not written out at whatever length the alt text happens to be. The two only differ
    /// when a long alt lands in a column the width budget has shaved — every other cell-image case
    /// pairs a short alt with a column wide enough to hold it whole, which is why the truncation
    /// could be deleted and the rest of this module would still pass. Without it the label runs past
    /// its own cell, the row grows wider than every other row of the box (`assert_rectangular`), and
    /// the table spills out of the pane.
    ///
    /// Note this is a **different** code path from the one an undrawable image's label takes: that
    /// one is an ordinary `CellSeg::Image` and is wrapped by `wrap_segments` like any other text,
    /// so no label test that goes through the fallback can stand in for this one.
    #[test]
    fn a_reserved_rectangles_label_is_cut_to_the_rectangle_not_to_the_alt_text() {
        for (why, src, width) in [
            (
                "long alt, and a neighbour long enough to shave the column",
                "| ![a very long alternative text indeed](a.png) | some fairly long prose that eats the width |\n|---|---|\n| x | y |\n",
                60u16,
            ),
            (
                "the same, in a much narrower pane",
                "| ![a very long alternative text indeed](a.png) | some fairly long prose that eats the width |\n|---|---|\n| x | y |\n",
                40,
            ),
            (
                "a CJK alt, whose characters are two columns each",
                "| ![とても長い日本語の代替テキストです](a.png) | some fairly long prose that eats the width |\n|---|---|\n| x | y |\n",
                50,
            ),
            (
                "an HTML cell, which reaches the identical rectangle through the other parser",
                "<table>\n<tr><td><img src=\"a.png\" alt=\"a very long alternative text indeed\"></td><td>some fairly long prose that eats the width</td></tr>\n</table>\n",
                60,
            ),
            (
                "two long-alt image cells at once, so both labels have to be cut",
                "| ![a very long alternative text indeed](a.png) | ![another equally long alternative text](b.png) |\n|---|---|\n| x | y |\n",
                50,
            ),
        ] {
            let out = render(src, width);
            assert!(!out.images.is_empty(), "{why}: 画像が配置されていない");
            for p in &out.images {
                // Vacuity guard: unless the untruncated label really is wider than the rectangle it
                // sits on, this case would pass with the truncation deleted.
                let label_w =
                    UnicodeWidthStr::width(super::super::cell_image_label(&p.alt).as_str());
                assert!(
                    label_w > p.cols as usize,
                    "{why}: ラベル({label_w}桁)が矩形({}桁)より広くない=この形では切り詰めを検査できない",
                    p.cols
                );
            }
            for line in texts(&out) {
                assert!(
                    UnicodeWidthStr::width(line.as_str()) <= width as usize,
                    "{why}: 幅 {width} を超えた行がある: |{line}|"
                );
            }
            assert_reserved(&out);
            assert_rectangular(&out);
        }
    }

    // ---- Text and pictures in one cell keep their source order ----

    /// A cell is a vertical stack of bands in **source order**: text written before a picture stays
    /// above it, text written between two pictures stays between them. `render_table_cells`'s own
    /// doc comment states exactly this, but every case that existed put the image first
    /// (`<img> caption`), so the flush that closes the pending text run *before* an image band is
    /// pushed was never exercised — delete it and every "text first" cell silently re-orders itself
    /// to "picture first, text last".
    #[test]
    fn text_and_images_in_one_cell_keep_their_source_order() {
        // `(why, src, the text to locate, how many bands must sit above it, how many below)`.
        for (why, src, needle, above, below) in [
            (
                "text then image",
                "| Shot: ![x](a.png) |\n|---|\n| y |\n",
                "Shot:",
                0usize,
                1usize,
            ),
            (
                "image then text",
                "| ![x](a.png) caption |\n|---|\n| y |\n",
                "caption",
                1,
                0,
            ),
            (
                "text, image, text",
                "| lead ![x](a.png) tail |\n|---|\n| y |\n",
                "lead",
                0,
                1,
            ),
            (
                "image, text, image",
                "| ![a](a.png) middle ![b](b.png) |\n|---|\n| y |\n",
                "middle",
                1,
                1,
            ),
            (
                "the HTML spelling of text-then-image",
                "<table>\n<tr><td>Shot: <img src=\"a.png\" alt=\"x\"></td></tr>\n</table>\n",
                "Shot:",
                0,
                1,
            ),
        ] {
            let out = render(src, 70);
            let lines = texts(&out);
            let row = lines
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{why}: 「{needle}」が描かれていない: {lines:?}"));
            let n_above = out
                .images
                .iter()
                .filter(|p| p.line + p.rows as usize <= row)
                .count();
            let n_below = out.images.iter().filter(|p| p.line > row).count();
            assert_eq!(
                (n_above, n_below),
                (above, below),
                "{why}: 「{needle}」(行 {row}) の上下にある矩形の数が違う: {:?}\n{lines:?}",
                out.images
            );
        }
    }

    /// The same ordering rule with the text long enough to wrap: **every** one of its physical rows
    /// stays on its own side of the picture, not just the first.
    #[test]
    fn a_wrapped_text_run_before_an_image_keeps_all_of_its_rows_above_it() {
        let src =
            "| one two three four five six seven eight nine ten ![x](a.png) |\n|---|\n| y |\n";
        let out = render(src, 40);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        let lines = texts(&out);
        let rows: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("one") || l.contains("ten"))
            .map(|(i, _)| i)
            .collect();
        assert!(rows.len() >= 2, "折り返していない: {lines:?}");
        for r in rows {
            assert!(
                r < p.line,
                "折り返した本文の一部が矩形の下に落ちた (行 {r}, 矩形 {p:?}): {lines:?}"
            );
        }
        assert_reserved(&out);
        assert_rectangular(&out);
    }

    // ---- A cell's own width: bands are stacked, so they are `max`ed, never summed ----

    /// The width of the drawn box, read off its own top rule.
    fn box_width(out: &RenderOut) -> usize {
        let top = texts(out)
            .into_iter()
            .find(|l| l.trim_start().starts_with('┌'))
            .expect("表が描かれていない");
        UnicodeWidthStr::width(top.trim_start())
    }

    /// A cell holding a picture *and* text is exactly as wide as the wider of the two bands — they
    /// sit above/below each other, so summing them would reserve a column ~one whole picture wider
    /// than anything ever drawn in it (dead space on every row). Measured against a reference render
    /// of the same table with the narrower band removed: same box, to the column.
    #[test]
    fn a_cells_width_is_the_wider_band_not_the_sum_of_its_bands() {
        // `(why, with both bands, the reference holding only the wider one, width)`
        for (why, both, wider_alone, width) in [
            (
                "the caption is narrower than the picture",
                "| ![x](a.png) a caption | b |\n|---|---|\n| 1 | 2 |\n",
                "| ![x](a.png) | b |\n|---|---|\n| 1 | 2 |\n",
                90u16,
            ),
            (
                "a CJK caption, still narrower than the picture",
                "| ![x](a.png) 図の説明 | b |\n|---|---|\n| 1 | 2 |\n",
                "| ![x](a.png) | b |\n|---|---|\n| 1 | 2 |\n",
                90,
            ),
            (
                // Written with no space between the caption and the picture, so the reference's own
                // text band is the identical string (a separating space would make the reference one
                // column narrower for a reason that has nothing to do with the rule under test).
                "the caption is the wider band",
                "| a caption that is a good deal wider than the picture it explains![x](a.png) | b |\n|---|---|\n| 1 | 2 |\n",
                "| a caption that is a good deal wider than the picture it explains | b |\n|---|---|\n| 1 | 2 |\n",
                90,
            ),
            (
                "the HTML spelling",
                "<table>\n<tr><td><img src=\"a.png\" alt=\"x\"> a caption</td><td>b</td></tr>\n</table>\n",
                "<table>\n<tr><td><img src=\"a.png\" alt=\"x\"></td><td>b</td></tr>\n</table>\n",
                90,
            ),
        ] {
            let out = render(both, width);
            let reference = render(wider_alone, width);
            assert_eq!(
                box_width(&out),
                box_width(&reference),
                "{why}: 帯の和で測って列が膨らんでいる\n{:?}\n{:?}",
                texts(&out),
                texts(&reference)
            );
            assert_rectangular(&out);
        }
    }

    /// The same rule, seen from inside the cell: when the picture *is* the widest band, its
    /// rectangle fills its column edge to edge — one padding space, the rectangle, one padding
    /// space, the border bar. A column measured as "text + picture" leaves the difference as blank
    /// dead space on that row, which this catches without knowing any of the numbers involved.
    #[test]
    fn the_widest_band_leaves_no_dead_space_in_its_column() {
        let src = "| ![x](a.png) a caption | b |\n|---|---|\n| 1 | 2 |\n";
        let out = render(src, 90);
        assert_eq!(out.images.len(), 1, "{:?}", out.images);
        let p = &out.images[0];
        let lines = texts(&out);
        let top = &lines[p.line];
        assert_eq!(
            slice_cols(top, p.col as usize + p.cols as usize, 2),
            " │",
            "矩形の右に死んだ余白がある {p:?}: |{top}|"
        );
        assert_eq!(
            slice_cols(top, p.col as usize - 2, 2),
            "│ ",
            "矩形の左に死んだ余白がある {p:?}: |{top}|"
        );
    }

    /// Several pictures in one cell: the column is measured by the **widest** of them, not by
    /// whichever one happened to be scanned last. Measuring by the last one silently re-fits a wide
    /// picture down to a narrow neighbour's size — the picture still draws, just smaller, with
    /// nothing on screen to say it was cut. No case anywhere put two images in one cell before this.
    #[test]
    fn a_cell_with_several_images_is_measured_by_the_widest_of_them() {
        // `(why, src, the natural width of each image in source order)`
        for (why, src, want) in [
            (
                "widest first",
                "| ![a](w20.png) ![b](w10.png) | z |\n|---|---|\n| 1 | 2 |\n",
                vec![20u16, 10],
            ),
            (
                "widest last",
                "| ![a](w10.png) ![b](w20.png) | z |\n|---|---|\n| 1 | 2 |\n",
                vec![10, 20],
            ),
            (
                "widest in the middle of three",
                "| ![a](w10.png) ![b](w30.png) ![c](w20.png) | z |\n|---|---|\n| 1 | 2 |\n",
                vec![10, 30, 20],
            ),
            (
                "three, widest first",
                "| ![a](w30.png) ![b](w10.png) ![c](w20.png) | z |\n|---|---|\n| 1 | 2 |\n",
                vec![30, 10, 20],
            ),
            (
                "an undrawable one between two drawable ones",
                "| ![a](w20.png) ![g](missing.png) ![b](w10.png) | z |\n|---|---|\n| 1 | 2 |\n",
                vec![20, 10],
            ),
            (
                "the HTML spelling, widest first",
                "<table>\n<tr><td><img src=\"w20.png\" alt=\"a\"> <img src=\"w10.png\" alt=\"b\"></td><td>z</td></tr>\n</table>\n",
                vec![20, 10],
            ),
        ] {
            let out = render_with(src, 90, &sized_slot);
            let got: Vec<u16> = out.images.iter().map(|p| p.cols).collect();
            assert_eq!(
                got, want,
                "{why}: どれかの絵が黙って縮んでいる: {:?}\n{:?}",
                out.images,
                texts(&out)
            );
            // Each band still stacks on its own rows, and the box stays rectangular around them.
            for w in out.images.windows(2) {
                assert!(
                    w[0].line + w[0].rows as usize <= w[1].line,
                    "{why}: 同じセルの矩形どうしが重なっている: {:?}",
                    out.images
                );
            }
            assert_reserved(&out);
            assert_rectangular(&out);
        }
    }

    /// The widest picture in a cell of several is what the column is sized to, so it is the one that
    /// keeps its natural size — read as a box width against a reference holding that picture alone.
    #[test]
    fn several_images_in_a_cell_size_the_column_like_the_widest_one_alone() {
        let several = render_with(
            "| ![a](w30.png) ![b](w10.png) ![c](w20.png) | z |\n|---|---|\n| 1 | 2 |\n",
            90,
            &sized_slot,
        );
        let widest_alone = render_with(
            "| ![a](w30.png) | z |\n|---|---|\n| 1 | 2 |\n",
            90,
            &sized_slot,
        );
        assert_eq!(
            box_width(&several),
            box_width(&widest_alone),
            "複数枚のセルの列幅が、いちばん広い1枚だけの場合と違う\n{:?}\n{:?}",
            texts(&several),
            texts(&widest_alone)
        );
    }
}
