//! A renderer built directly on the block model (`crate::preview::markdown::model`), rather than on
//! tui-markdown's own rendered output. **Not wired into any preview path yet** — the only caller is
//! the diff harness in `src/app/md_render_diff_tests.rs`, which measures how closely this renderer's
//! output matches the production pipeline's (`render_markdown_with_images`) for the block kinds it
//! currently covers, over the full parity corpus. See that module's own doc comment for the numbers
//! from the last run.
//!
//! ## Scope (this pass)
//!
//! `Heading`, `Paragraph`, `ThematicBreak`, `List`/`ListItem`, `Quote` (plain **or** an alert —
//! `alert: Some(_)`), `Details`, and `CodeBlock` (fenced or indented, at any depth this pass
//! otherwise covers — top level, inside a list item, inside a quote; see `render_code_block`'s own
//! doc comment) — plus their inline content: emphasis, strong, strikethrough, inline code, links,
//! images (alt text only), soft/hard breaks, and super/subscript. A GitHub alert
//! (`Quote { alert: Some(_), .. }`) is drawn via `render_alert_from_model`, which reuses
//! `markdown.rs`'s own `alert_header_line`/`alert_bar` for the header/left-bar decoration — the same
//! callout box `render_alert` draws, just with the *body* rendered from this file's own tree
//! (`Block.children`) instead of a re-parsed string; see that function's own doc comment. A
//! `Details` block is drawn the identical way, via `render_details_from_model` and
//! `markdown.rs`'s own `details_marker_line`/`details_bar`. The one remaining `BlockKind` a `Doc`
//! can contain (`Table`/`Html`) is recorded in `RenderOut::unsupported` and produces **no output at
//! all** — not even its own inline content, if a block this pass *does* cover happens to nest one of
//! them (a table three levels down inside a list item, say). Rendering only the *rest* of that
//! container, working around the one nested piece it cannot draw, would produce something the
//! production renderer never draws (a list with a hole in the middle of it), and the diff harness
//! would have no way to tell that apart from an actual match — so `contains_unsupported` walks a
//! `List`/`Quote`/`Details`'s *entire* subtree before rendering a single line of it, and the whole
//! thing is reported unsupported (under whichever nested kind's own name it actually found) the
//! moment a `Table`/`Html` turns up anywhere inside it, at any depth. Reporting the container as
//! unsupported and skipping the whole subtree is the honest choice for a skeleton stage; `render_doc`
//! therefore only ever draws lines for a `List`/`Quote`/`Details` whose entire contents this pass
//! already knows how to render.
//!
//! ## Alert/`<details>` interactivity — `BlockCtx.details_interactive`
//!
//! `markdown.rs`'s own pipeline draws a `Details` block's summary marker two different ways
//! (`render_details`'s own `interactive` parameter) depending on *where* it was found:
//! **interactively** (consuming a slot from the document-wide Tab-toggle ordinal sequence,
//! `next_details_open`, and drawing the marker in `details_marker_style()` — the Tab-focus sentinel)
//! only at the single top-level entry point (`render_md_text`); **statically** (using the tag's own
//! `open` attribute directly, no ordinal consumed, a plain marker style) everywhere else — inside a
//! GitHub alert's own body or another `Details` block's own body, at any nesting depth — see
//! `render_md_body_nested`'s own doc comment for the full reasoning. `BlockCtx.details_interactive`
//! is this file's own mirror of that same distinction, threaded down through every recursive block
//! dispatcher (`render_block`/`render_list`/`render_item`/`render_quote`) alongside `math_here` —
//! `true` only at `render_doc`'s own top-level dispatch, forced `false`, permanently, by
//! `render_alert_from_model`/`render_details_from_model` for their own `children` (never by plain
//! `render_quote`, whose own children are still reached by the *same* top-level scan production's
//! `split_details` performs — see `render_quote`'s own doc comment).
//!
//! A list item nested *inside* a code block, table, or HTML block is not a concern the other
//! direction either — those three are leaves with no `Block::children` of their own (see
//! `model::BlockKind`'s own doc comment), so there is no "list inside a code block" shape for
//! `contains_unsupported` to have to reject.
//!
//! A task-list item inside a *loose* list is fully covered too, marker and all, including the
//! visually surprising (but faithful — see the module doc comment on reproducing whatever the real
//! renderer does) layout that shape gets: its marker lands on its own line, separate from its own
//! bullet/number (see `render_item`'s own doc comment for exactly why, and `Writer::task_list_marker`'s
//! for the one bookkeeping fix that shape needed — `contains_unsupported` used to exclude it entirely,
//! under a synthetic `"LooseTask"` name, while the underlying panic that shape hit was still open).
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
//! `md-block-walk` refactor, **not by re-parsing anything either**: a `Heading`'s or `Paragraph`'s
//! own inline events are read straight out of `Doc.events` (see `model::Doc`'s own doc comment), by
//! the index range `Doc::parse` already recorded for that exact block (`BlockKind::Heading.inline` /
//! `BlockKind::Paragraph.inline` — every inline-content call site's own first argument is exactly
//! that slice). `Doc::parse` walks `src` **once**, as a whole document, so a reference-style link
//! (`[text][ref]`) whose `[ref]: url` definition lives in a *different* top-level block resolves
//! correctly here — the accepted-gap category an earlier, per-block-re-parsing version of this file
//! had to document (and `md_render_diff_tests` had to carve a dedicated, unlisted pin around) no
//! longer exists at all; see `crate::preview::markdown::inline_corpus`'s own doc comment for the
//! structural proof, and `model::Doc.events`'s doc comment for why a single walk gets this "for
//! free".
//!
//! What this file *does* reimplement is the handful of `tui_markdown::TextWriter` methods that
//! matter for the block kinds this pass covers (see the pinned `tui-markdown = "0.3.7"` dependency's
//! own `src/lib.rs`, `impl<I, S> TextWriter<'_, I, S>`) — `Writer`, below, is that reimplementation,
//! one method per `TextWriter` method it mirrors, walking `model::Block`'s tree instead of a raw
//! `pulldown_cmark::Event` stream. See `Writer`'s own doc comment for the shape of that
//! correspondence and why it has to hold onto more state than a plain inline-content walk needed
//! (`list_indices`/`line_prefixes`/`line_styles`/`needs_newline` — all four exist in `TextWriter`
//! itself, under the identical names, for the identical reasons).
//!
//! `Image` renders only its alt text — tui-markdown's own `Tag::Image` arm is a no-op `warn!`, so the
//! URL is dropped and never appended anywhere, and nothing about the tag itself changes the writer's
//! state; this file mirrors that exactly (`start_inline_tag`'s `Tag::Image` arm). `Link` appends
//! `" ("` + the URL styled `KonomaStyles::link()` + `")"` right after the label, the exact 4-span
//! shape `app::md_text::collapse_links` looks for (see that function's own doc comment: "Pattern:
//! `[label]` `" ("` `[URL(blue underline)]` `")"`").
//!
//! Once a block's own raw lines are built, `super::decorate_md_lines` — the exact function
//! `render_text_block_safe` calls on tui-markdown's own output — turns the heading's leading `#`
//! span into KonomaStyles-decorated hierarchy (full-width rule under H1/H2), the literal `"---"`
//! this file emits for a `ThematicBreak` into the real full-width rule line, and a literal
//! `"- [x] "`/`"[x] "` run (however `Writer::task_list_marker` happened to have produced it — see its
//! own doc comment for the two distinct shapes) into a dedicated, focusable marker span. This is
//! reused, not reimplemented, for the same reason `Doc::parse` itself reuses
//! `parse_alert_header`/`details_open_tag`/... rather than re-deriving their judgments: a second,
//! hand-written copy of "what does a heading/rule/checkbox look like once decorated" is exactly the
//! kind of drift this whole refactor exists to eliminate. One consequence worth naming explicitly: a
//! few of `decorate_md_lines`'s own passes only recognize the shape they are looking for on a line
//! with **no** leading spans of any kind (`heading_level` reads `line.spans.first()`;
//! `decorate_extras`'s thematic-break check joins *every* span, prefix included) — so a heading or
//! rule nested inside a `Quote` does **not** get its usual special treatment (no color, no
//! full-width rule, literal `"> ---"` instead of an expanded line) here either. That is not a new gap
//! this file introduces: it is a real, existing property of `decorate_md_lines` itself, and since
//! both this renderer and the production one funnel every line through that exact same function, a
//! quoted heading or rule is expected to render identically on both sides — a match, not a mismatch —
//! precisely *because* neither side gets the special treatment.
//!
//! `#![allow(dead_code)]`: like `model.rs` (see its own module doc comment), nothing outside
//! `md_render_diff_tests` (`#[cfg(test)]`, in `src/app/`) calls into this module yet — a plain,
//! non-test build has no path from any renderer to `render_doc` at all.
#![allow(dead_code)]

use std::ops::Range;

use pulldown_cmark::{Event, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::StyleSheet;

use super::model::{code_body_text, AlertKind, Block, BlockKind, Doc, Task};
use super::{
    alert_bar, alert_header_line, code_header, decorate_headings_and_extras, details_bar,
    details_marker_line, gutter_span, highlight_body, image_loading_line, math_placeholder_lines,
    math_raw_lines, math_url, next_details_open, pad_to_width, scan_inline_math, CodeStyle,
    ImagePlacement, KonomaStyles, MathPart, MathSlot, SourceRun,
};

/// What `render_doc` produced: the decorated lines (same shape `render_markdown_with_images`
/// returns), any inline images placed (populated only by lifted LaTeX math whose `math_slot` answers
/// `MathSlot::Image` — see `render_paragraph_math`; still always empty for block-level `![alt](url)`
/// images, out of scope for this stage — see the module doc comment on scope), and the name of every
/// top-level `BlockKind` this pass encountered but does not yet render (see `contains_unsupported`'s
/// own doc comment for why a name recorded here can come from several levels deep inside a `List` or
/// `Quote`, not only from a block sitting directly in `doc.blocks`).
pub(crate) struct RenderOut {
    pub lines: Vec<Line<'static>>,
    pub images: Vec<ImagePlacement>,
    pub unsupported: Vec<&'static str>,
}

/// Two independent "how deep in the tree am I, and does it still apply" flags every recursive block
/// dispatcher (`render_block`/`render_list`/`render_item`/`render_quote`, and the two container
/// renderers this pass adds for a GitHub alert/`<details>` — `render_alert_from_model`/
/// `render_details_from_model`) has to thread down together — bundled into one small `Copy` struct,
/// rather than two adjacent `bool` parameters, for the same reason `markdown.rs`'s own `MdRenderCtx`
/// bundles same-typed positional arguments: two `bool`s next to each other at a call site is exactly
/// the kind of silent-swap footgun a named field turns into a compile error instead.
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
    math_here: bool,
    /// Whether a `Details` block reached at this position should consume the document-wide
    /// Tab-toggle ordinal sequence (`super::next_details_open`) — see the module doc comment's own
    /// "Alert/`<details>` interactivity" section for the full contract this mirrors.
    details_interactive: bool,
}

/// Renders every top-level block in `doc` this file's own scope covers (see the module doc comment),
/// in source order, through a single `Writer` shared across the whole walk — the same instance a
/// `List`'s own items, a `Quote`'s own body, and any further nesting of either all render into, so
/// that `needs_newline`/`list_indices`/`line_prefixes` bookkeeping carries correctly across every
/// block boundary in the document, top level included (see `Writer`'s own doc comment). A block kind
/// this pass does not cover — whether sitting directly in `doc.blocks` or found anywhere inside a
/// `List`'s or `Quote`'s own subtree by `contains_unsupported` — contributes no lines and no
/// separator at all, and is recorded in `unsupported` instead.
///
/// `src` is the exact text `doc` was parsed from (`Doc::parse(src)`) — needed, now that `List`
/// support means this file must be able to tell a **tight** list item's own paragraph (no wrapping
/// `Tag::Paragraph` at all; `model::collect_stray_inline_run`'s synthetic node — and, since
/// `CodeBlock` support, not necessarily the item's own *first* child either, see `render_block`'s own
/// `Paragraph` arm) apart from a **loose** one's (a real `Tag::Paragraph`, which changes how
/// `render_paragraph`'s own unconditional blank-line push interacts with `Writer::task_list_marker`'s
/// placement — see `render_item`'s own doc comment): the model already draws that exact line, in
/// `Block.src`'s own trailing byte, for anything computed from raw `pulldown_cmark::Event` ranges to
/// read back (see `model::collect_stray_inline_run`'s own doc comment, and its home test,
/// `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one`) — `is_real_paragraph`
/// is exactly that read, not a second parser judgment of any kind.
/// `width`/`code`/`theme`/`icons`/`tasks` are threaded straight through to `w` (`Writer`'s own
/// `width`/`code`/`theme` fields — read by `render_code_block`, the same way `math` is read by
/// `render_math_slot` — and, at the very end here, `super::decorate_headings_and_extras`, the same
/// heading/thematic-break/checkbox post-passes `render_text_block_safe` itself runs on tui-markdown's
/// own output — matching `render_markdown_with_images`'s own call to `decorate_md_lines` via that
/// function *minus* its own leading `decorate_code_blocks` pass: this file's own `CodeBlock`s are
/// already fully decorated by the time they reach `w.lines`, so running that pass a second time here
/// would be redundant — see `render_code_block`'s own doc comment, and this file's own
/// `decorate_code_blocks_is_idempotent_on_render_docs_own_output` test (in `mod tests` below) for
/// proof it would be no *more* than redundant, not a source of double-decoration). `math_slot`/
/// `math_on` are the same
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
    math_slot: &dyn Fn(&str, bool) -> MathSlot,
    math_on: bool,
) -> RenderOut {
    let styles = KonomaStyles { code_bg: code.bg };
    let math = math_on.then(|| MathCtx {
        slot: math_slot,
        width,
        images: Vec::new(),
    });
    // Whether math lifting applies at all, at the *top* of the tree — `render_doc`'s own dispatch is
    // never inside a `Quote`, so this is simply "was `math_on` set", the same answer `math.is_some()`
    // would give; computed once, ahead of `w`'s own construction (which moves `math` into it), so
    // nothing below needs to keep re-deriving `w.math.is_some()`. `details_interactive: true` is the
    // *other* half of the identical "top of the tree" reasoning — see `BlockCtx`'s own doc comment.
    let ctx = BlockCtx {
        math_here: math.is_some(),
        details_interactive: true,
    };
    let mut w = Writer {
        lines: Vec::new(),
        inline_styles: Vec::new(),
        link: None,
        styles,
        list_indices: Vec::new(),
        line_prefixes: Vec::new(),
        line_styles: Vec::new(),
        needs_newline: false,
        math,
        after_math: false,
        fresh_boundary: false,
        pending_para_start: None,
        code,
        theme: theme.to_string(),
        width,
        icons,
        tasks,
    };
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
                render_heading(&mut w, doc, *level, inline.clone(), &meta);
            }
            BlockKind::Paragraph { inline } => {
                render_paragraph_dispatch(
                    &mut w,
                    doc,
                    src,
                    inline.clone(),
                    ctx.math_here,
                    block.src.start,
                );
            }
            BlockKind::ThematicBreak => render_rule(&mut w),
            BlockKind::List { start, .. } => match contains_unsupported(&block.children) {
                Some(bad) => unsupported.push(bad),
                None => render_list(&mut w, doc, src, *start, &block.children, ctx),
            },
            BlockKind::Quote { alert: None, .. } => match contains_unsupported(&block.children) {
                Some(bad) => unsupported.push(bad),
                None => render_quote(&mut w, doc, src, &block.children, ctx),
            },
            // A GitHub alert — konoma's own `render_alert_from_model`, reusing `markdown.rs`'s own
            // `alert_header_line`/`alert_bar` for the header/bar decoration; see the module doc
            // comment's own "Scope" section.
            BlockKind::Quote {
                alert: Some(kind),
                alert_title,
            } => match contains_unsupported(&block.children) {
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
            } => render_code_block(&mut w, src, lang.as_deref(), body_spans),
            BlockKind::Details { open_attr, summary } => {
                match contains_unsupported(&block.children) {
                    Some(bad) => unsupported.push(bad),
                    None => render_details_from_model(
                        &mut w,
                        doc,
                        src,
                        *open_attr,
                        summary,
                        &block.children,
                        ctx,
                    ),
                }
            }
            // Never reachable at the top level — a `ListItem` only ever exists as a `List`'s own
            // child (see `model::BlockKind::ListItem`'s doc comment), and `render_list` unwraps every
            // one of those itself before this loop ever sees it. Matched defensively all the same,
            // rather than assumed unreachable — see `render_block`'s own doc comment for why that
            // convention holds throughout this file.
            BlockKind::ListItem { .. } => unsupported.push("ListItem"),
            // Exhaustive on purpose (matching this crate's own convention — see e.g.
            // `md_snapshot_tests::fmt_item_kind`'s doc comment): a new `BlockKind` variant must be
            // taught to this match rather than silently falling through as "unsupported" under a
            // name nobody chose on purpose.
            BlockKind::Table { .. } => unsupported.push("Table"),
            BlockKind::Html { .. } => unsupported.push("Html"),
        }
    }
    let lines = decorate_headings_and_extras(w.lines, width, icons, tasks);
    let images = w.math.map(|m| m.images).unwrap_or_default();
    RenderOut {
        lines,
        images,
        unsupported,
    }
}

/// Whether `blocks` — a `List`'s own item children, a `Quote`'s own body (alert or not), a
/// `Details`'s own body, or (recursively) any of those nested arbitrarily deep inside more of the
/// same — contains, anywhere inside it, a block kind this pass does not render at all: a `Table` or
/// an `Html` block (the one remaining `BlockKind` a `Doc` can contain that has no renderer here — see
/// the module doc comment's own "Scope" section). `CodeBlock`, and (as of this pass) `Quote { alert:
/// Some(_), .. }`/`Details`, used to be names this function screened out too — see
/// `render_code_block`'s own doc comment for why a `CodeBlock` no longer needs this treatment at all,
/// at any nesting depth, quote included, and the module doc comment for the identical reasoning now
/// extended to an alert/`<details>`: both have real renderers now, so they recurse into their own
/// `children` here (checking for a *still*-unsupported `Table`/`Html` nested inside *them*) the same
/// way `List`/plain-`Quote` already do, rather than being reported unsupported outright.
///
/// Returns the *first* such kind's name found, in document order, for the caller (`render_doc`'s own
/// top-level dispatch, or a nested recursive call from this same function walking back up) to record
/// in `RenderOut::unsupported` — the exact same reporting a *directly* unsupported top-level block
/// already gets, just discovered one or more levels deeper. This is what keeps `render_doc`'s own
/// "report the container as unsupported and skip the whole subtree" choice (see the module doc
/// comment) honest for `List`/`Quote`/`Details`: a three-item list with a table buried in the third
/// item's own nested sublist is not partially rendered with a hole where that block would have gone —
/// the whole list, all three items, is skipped, called out under whichever nested kind's own name
/// `contains_unsupported` actually found, exactly like a bare top-level `Table` always has been.
///
/// A **loose list item that has a task marker** used to be excluded here too, under a synthetic
/// `"LooseTask"` name — not a real `BlockKind` variant at all — because that exact shape crashed
/// `Writer::task_list_marker` (`line.spans.insert(1, ..)` against an empty `Vec`; see that method's
/// own doc comment for the fix). Principle #3 (`CLAUDE.md`) is "never crash", not "never render a
/// shape this stage happens to mishandle" — so once the underlying panic was fixed constructively,
/// screening the shape out here stopped being necessary and was removed: a loose task item now
/// renders (matching tui-markdown's own real, if visually surprising, layout for it — see
/// `render_item`'s own doc comment) instead of being reported unsupported.
///
/// A leaf kind with no `Block::children` of its own (`Heading`/`Paragraph`/`ThematicBreak`/
/// `CodeBlock`, and the two `BlockKind` variants this function itself is checking for) never
/// recurses — there is nothing further to walk under any of them (see `model::Block.children`'s own
/// doc comment: "Empty for every leaf kind").
fn contains_unsupported(blocks: &[Block]) -> Option<&'static str> {
    for b in blocks {
        match &b.kind {
            BlockKind::Table { .. } => return Some("Table"),
            BlockKind::Html { .. } => return Some("Html"),
            BlockKind::Heading { .. }
            | BlockKind::Paragraph { .. }
            | BlockKind::ThematicBreak
            | BlockKind::CodeBlock { .. } => {}
            BlockKind::ListItem { .. }
            | BlockKind::List { .. }
            | BlockKind::Quote { .. }
            | BlockKind::Details { .. } => {
                if let Some(bad) = contains_unsupported(&b.children) {
                    return Some(bad);
                }
            }
        }
    }
    None
}

/// A minimal reimplementation of `tui_markdown::TextWriter` itself (see the module doc comment),
/// scoped to the block kinds this pass covers — not merely its *inline*-event handling any more (an
/// earlier version of this file, before `List`/`Quote` support, only ever needed that half; see this
/// struct's own git history if a comparison is ever useful). Every field below exists in the real
/// `TextWriter` too, under the identical name, tracking the identical thing:
///
/// * `list_indices` — one entry per currently-open `List`, `None` for an unordered one or `Some(n)`
///   for an ordered one's own running item counter (see `render_list`/`push_item_marker`). Nesting
///   depth (`list_indices.len()`) is what sets every item marker's own column width, *regardless* of
///   how many spaces the source itself used for that level's indentation — konoma's own hand-picked
///   4-columns-per-level convention, read back unconditionally the same way both here and in the real
///   `TextWriter`.
/// * `line_prefixes`/`line_styles` — one entry per currently-open `Quote`, its leading `>` span and
///   its `KonomaStyles::blockquote()` style. `push_line` applies *every* currently-open prefix/style
///   to *every* line it builds, list markers and headings and rules included — not merely paragraph
///   text — which is exactly how a `Quote` containing a `List` ends up with each item's own marker
///   line `>`-prefixed too (see `render_doc`'s module doc comment for the flip side of that same
///   mechanism: a heading or rule *inside* a `Quote`, prefixed this same way, no longer matches the
///   narrow, no-leading-span shape `decorate_md_lines`'s own passes look for).
/// * `needs_newline` — whether the *next* block-level construct owes the page a blank separator line
///   before it starts. Starts `false`. Every *block*-rendering function in this file — `render_heading`/
///   `render_paragraph`/`render_rule`/`render_list`/`render_quote` — sets it back to `true` on its own
///   way out, the same way every one of `TextWriter`'s own `end_*`/`rule`/`end_list`/`end_blockquote`
///   methods does. The one deliberate exception is bare *inline* content with no wrapping block tag at
///   all — `Writer::text`/`code`/`soft_break`/`hard_break`, and the inline-style push/pop pair — none
///   of which ever sets it `true` in the real `TextWriter` either (see `Writer::text`'s own doc
///   comment); this is exactly what a **tight** list item's own content is (see `render_item`'s own
///   doc comment on the tight/loose distinction), so it must not accidentally read as "the page owes a
///   blank separator" once that content finishes. `after_math` (below) is a *further* exception,
///   deliberately narrower than that one: a math lift sets `needs_newline` to `false` itself (see
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
///   `self.lines.extend(..)`, deliberately bypassing `push_line` (the one narrow exception to that
///   method's own "only place a new `Line` legitimately enters `self.lines`" doc comment — see there),
///   precisely so this flag survives long enough for whatever renders *next* to see it. `push_line`
///   clears it unconditionally, first thing, the moment anything else renders through the normal
///   machinery — so every block-rendering function that already calls `push_line` somewhere in its own
///   body before its own exit (every one of them does) needs no `after_math`-awareness of its own at
///   all; only a function whose *exit* can run **without** having called `push_line` again since a
///   lift — `render_list`/`render_quote`, when their very last item/child ended right on one — has to
///   consult it explicitly, to avoid re-forcing `needs_newline` back to `true` and reintroducing the
///   very separator line the lift's own reset was there to suppress. See `render_paragraph_math`'s own
///   doc comment for the "fresh reparse boundary" this reproduces, and `ensure_fresh_after_math`'s own
///   for how the flag gets consumed once something *does* follow a lift within the same paragraph.
struct Writer<'m> {
    lines: Vec<Line<'static>>,
    inline_styles: Vec<Style>,
    link: Option<String>,
    styles: KonomaStyles,
    list_indices: Vec<Option<u64>>,
    line_prefixes: Vec<Span<'static>>,
    line_styles: Vec<Style>,
    needs_newline: bool,
    math: Option<MathCtx<'m>>,
    after_math: bool,
    /// Whether the very last thing pushed onto `self.lines` was the end of a GitHub alert or
    /// `Details` block — `render_alert_from_model`/`render_details_from_model` are the *only* two
    /// places that set this `true`, on their own way out, alongside `needs_newline = false` (see
    /// either's own doc comment for why both: whatever renders next is exactly as if a brand-new
    /// `Writer` had just started, since production's own next fragment, if any, really is rendered
    /// by a brand-new `tui_markdown::TextWriter` — the identical "fresh reparse boundary"
    /// `render_math_slot`'s own doc comment already documents in full for a lifted math expression).
    ///
    /// `needs_newline = false` alone reproduces that boundary for every *other* block-rendering
    /// function in this file, all of which gate their own leading blank-line push on it — except
    /// `render_code_block`, whose own leading-blank check (`if !w.lines.is_empty() { .. }`, mirroring
    /// real `TextWriter::start_codeblock`) is deliberately **not** gated on `needs_newline` at all —
    /// see that function's own doc comment for exactly why not (a fence gets its separator even when
    /// `needs_newline` happens to be `false` for an unrelated reason, e.g. a tight list item's own
    /// bare content). That same "unconditional as long as *something* already rendered" check is
    /// exactly what mistakes an alert's/`Details`'s own already-rendered lines, still sitting in the
    /// *shared* `w.lines`, for "something precedes this fence within the *same* segment" — which
    /// production's own, `w.lines`-per-segment tui-markdown reality never sees at all (confirmed
    /// directly: `code_corpus`'s own "indented block nested inside a closed details is not counted"
    /// and "everything" cases both show a fence immediately after a closed `Details`/an alert with no
    /// blank row between them, exactly the same way a fence right after a lifted math expression, or
    /// as the very first thing in the whole document, gets none). `render_code_block` is the only
    /// site that has to consult this flag for that reason; `push_line` clears it unconditionally,
    /// first thing, the same way it already clears `after_math` — so, like that flag, no other
    /// block-rendering function needs any awareness of it at all (each already calls `push_line`
    /// itself before returning).
    fresh_boundary: bool,
    /// A math-lifting paragraph's own leading-blank-line convention, deferred rather than applied
    /// eagerly — `Some(needs_newline_before)` from the moment `render_paragraph_math`/`render_item`
    /// (its own tight/loose-item-first-child special case) starts walking a paragraph whose text
    /// *might* lift math, until whichever of `open_pending_para_start`/`drop_pending_para_start` first
    /// resolves it; `None` once resolved (or when no math-lifting walk is in progress at all).
    ///
    /// ## Why this cannot be applied eagerly, the way `render_paragraph`'s non-math counterpart does
    ///
    /// `render_paragraph` (no math) always opens with `TextWriter::start_paragraph`'s own two-part
    /// blank-line push (`if needs_newline { push_blank_line() }` then an *unconditional* second one)
    /// *before* walking the paragraph's own inline content, because that walk is guaranteed to reach
    /// `Writer::text`/`push_span` for its own first content — which *fills* that freshly-opened blank
    /// row rather than adding a new one (`push_span`'s own `self.lines.last_mut()` fallback). A
    /// math-lifting paragraph's walk (`walk_inline_math`) is not guaranteed that: its own very first
    /// content can just as easily be a lifted math expression's own placeholder, which
    /// `render_math_slot` appends via `self.lines.extend(..)` — deliberately bypassing `push_span`
    /// (see `after_math`'s own doc comment for why) — so a blank row opened ahead of time for math to
    /// "fill" never gets filled at all, and is left behind, genuinely empty, forever.
    ///
    /// This is not a hypothetical: it is exactly what production's own `render_markdown_with_images`
    /// does for the identical shape, confirmed directly (not merely inferred) against
    /// `code_span_corpus`'s own math cases (a `$$…$$`/`\(…\)`/`\[…\]` expression as a paragraph's own
    /// *entire* content, immediately after another top-level block — an indented code block, in every
    /// one of those three cases) — a lifted expression that is the very first (and typically only)
    /// content of its own paragraph gets **no leading blank line of any kind**, not even the separator
    /// a genuinely new top-level block would otherwise owe the page: `BlockPart::Math`'s own handling
    /// (`render_markdown_with_images`'s own match arm) is `out.extend(math_placeholder_lines(..))` —
    /// appended directly, with **no** paragraph-start convention run for it at all, because a lifted
    /// expression is never fed through tui-markdown (or, here, `Writer::push_span`) in the first
    /// place. This holds regardless of *what* preceded it — a code block (confirmed: the padding row's
    /// own line is followed immediately by the math line, one line closer than a real separator would
    /// put it), a heading, or another paragraph (confirmed with a hand-traced `"para1\n\n$$y$$\n"`) —
    /// because `split_math`'s own document-wide split means the leading text run and the lifted
    /// expression are never rendered by the same, blank-line-aware machinery at all; the split simply
    /// discards whatever separator state might otherwise have applied.
    ///
    /// So the convention has to become *lazy*: `render_paragraph_math` records that a paragraph-start
    /// is owed (`Some(w.needs_newline)`, capturing the value the eager version would have read
    /// immediately) but does not yet act on it. The first thing that turns out to need a fresh row —
    /// real text, reached via `render_text_with_math`'s own `MathPart::Text` arm, or an inline code
    /// span/soft break/hard break/nested inline tag, reached via `ensure_fresh_after_math` (now the
    /// shared "about to render real content" checkpoint for *both* this and `after_math` — see that
    /// function's own doc comment) — opens it via `open_pending_para_start`, applying the exact same
    /// two-part convention `render_paragraph` still applies eagerly (this struct's own doc comment on
    /// `needs_newline` explains why that shape is correct for text: it is literally the same
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
    /// alert_header_line`'s own `icons` parameter), the identical `ctx.icons` `markdown.rs`'s
    /// `render_alert` reads off `MdRenderCtx`. `Copy`, so reading it never needs a reborrow.
    icons: bool,
    /// `render_doc`'s own incoming `tasks` slice, unchanged for the whole render — read by
    /// `render_bar_prefixed_body`, which needs it to run `super::decorate_headings_and_extras` over
    /// an alert's/`Details`'s own body **before** the bar gets prepended to each line; see that
    /// function's own doc comment for why. Borrowed with this struct's own `'m` lifetime (the same
    /// one `math: Option<MathCtx<'m>>` already uses) rather than owned, since `render_doc`'s own
    /// `tasks: &[char]` parameter already outlives the whole call.
    tasks: &'m [char],
}

impl<'m> Writer<'m> {
    fn cur_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
    }

    /// Display-column width `push_line` currently adds to *every* line via `line_prefixes` — one
    /// column per currently-open quote's own `">"` span, plus the one further column `push_line`
    /// itself inserts (a single leading space) ahead of the innermost prefix, whenever at least one
    /// quote is open; `0` when none is. Used by `render_code_block` to keep a quote-nested code
    /// block's own full-width background band from overflowing the terminal once that prefix is
    /// prepended on top of it — see that function's own doc comment.
    fn prefix_width(&self) -> usize {
        if self.line_prefixes.is_empty() {
            0
        } else {
            self.line_prefixes.len() + 1
        }
    }

    /// `TextWriter::push_line`: applies every currently-open blockquote prefix/style — see this
    /// struct's own doc comment on `line_prefixes`/`line_styles` — to `line`, then appends it. The
    /// only other place a new `Line` legitimately enters `self.lines` in this whole file is
    /// `render_math_slot`'s own `self.lines.extend(..)` — deliberately bypassing this method, so that
    /// call *doesn't* clear `after_math` the way every other new line does (see this struct's own doc
    /// comment on that field for why). Every other method below that starts a fresh line
    /// (`push_blank_line`, `push_span`'s own fallback) goes through this one rather than pushing to
    /// `self.lines` directly, so a prefix can never be accidentally skipped for one particular line
    /// shape.
    fn push_line(&mut self, line: Line<'static>) {
        self.after_math = false;
        self.fresh_boundary = false;
        let style = self.line_styles.last().copied().unwrap_or_default();
        let mut line = line.patch_style(style);
        if !self.line_prefixes.is_empty() {
            line.spans.insert(0, Span::raw(" "));
        }
        for prefix in self.line_prefixes.iter().rev().cloned() {
            line.spans.insert(0, prefix);
        }
        self.lines.push(line);
    }

    /// `TextWriter::push_span`.
    fn push_span(&mut self, span: Span<'static>) {
        match self.lines.last_mut() {
            Some(l) => l.push_span(span),
            None => self.push_line(Line::from(vec![span])),
        }
    }

    fn push_blank_line(&mut self) {
        self.push_line(Line::default());
    }

    /// Consumes a still-pending math-paragraph-start slot (see `pending_para_start`'s own doc
    /// comment), opening it with the normal two-part leading-blank-line convention
    /// `render_paragraph`/real `TextWriter::start_paragraph` both use unconditionally — called from
    /// `ensure_fresh_after_math`, the shared checkpoint every path that is about to render real
    /// (non-lifted) content already runs through. A no-op once the slot is no longer pending (already
    /// opened this way, or already dropped — unopened — by a leading math lift; see
    /// `drop_pending_para_start`).
    fn open_pending_para_start(&mut self) {
        if let Some(needs_newline_before) = self.pending_para_start.take() {
            if needs_newline_before {
                self.push_blank_line();
            }
            self.push_blank_line();
            self.needs_newline = false;
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

    /// `TextWriter::push_inline_style`.
    fn push_inline_style(&mut self, style: Style) {
        self.inline_styles.push(self.cur_style().patch(style));
    }

    /// `TextWriter::pop_inline_style`.
    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    /// `TextWriter::text`: an embedded `\n` inside one `Event::Text` payload — rare, but not
    /// impossible; see that method's own `text.lines().with_position()` loop — starts a new physical
    /// line for every line after the first. Sets `needs_newline = false` unconditionally on its own
    /// way out (matching the real `TextWriter::text`'s own final statement, outside its own loop) —
    /// **never** `true`, in contrast to `render_heading`/`render_paragraph`/`render_rule`/
    /// `render_list`/`render_quote`, which all set it `true` when *they* finish. This is what makes a
    /// **tight** list item's own bare inline content (see `render_item`'s own doc comment) genuinely
    /// different from a real paragraph's: nothing in a plain sequence of `text`/`code`/`soft_break`/
    /// `hard_break`/inline-style calls, with no wrapping `end_paragraph`-equivalent at all, ever asks
    /// the *next* construct for a blank separator line.
    fn text(&mut self, text: &str) {
        for (i, line) in text.lines().enumerate() {
            if i > 0 {
                self.push_blank_line();
            }
            let style = self.cur_style();
            self.push_span(Span::styled(line.to_string(), style));
        }
        self.needs_newline = false;
    }

    /// `TextWriter::code`.
    fn code(&mut self, code: &str) {
        self.push_span(Span::styled(code.to_string(), self.styles.code()));
    }

    /// `TextWriter::soft_break` — the `in_metadata_block` branch there never applies here: this walk
    /// never sees `Tag::MetadataBlock` at all, front matter having already been stripped from `src`
    /// before `Doc::parse` ever ran (see the model module's own doc comment on what text it expects).
    fn soft_break(&mut self) {
        self.push_span(Span::raw(" "));
    }

    /// `TextWriter::hard_break`.
    fn hard_break(&mut self) {
        self.push_blank_line();
    }

    /// `TextWriter::push_link`.
    fn push_link(&mut self, url: String) {
        self.link = Some(url);
    }

    /// `TextWriter::pop_link`.
    fn pop_link(&mut self) {
        if let Some(link) = self.link.take() {
            self.push_span(Span::raw(" ("));
            self.push_span(Span::styled(link, self.styles.link()));
            self.push_span(Span::raw(")"));
        }
    }

    /// `TextWriter::task_list_marker` — verbatim, including its own two quirks, neither of which this
    /// file tries to smooth over (matching whatever the real one produces, bugs included, is the
    /// entire point; see the module doc comment):
    ///
    /// * It always writes a lowercase `x`, never `X` — `TextWriter` itself only ever receives a
    ///   `bool` (`Event::TaskListMarker(checked)`), which cannot distinguish the two in the first
    ///   place; `checked` here collapses `model::Task::state`'s own `'x'`/`'X'` distinction back down
    ///   to match (`render_item`'s own call site does that collapsing).
    /// * The two shapes it can produce are genuinely different — not a bug in this reimplementation,
    ///   a property of `TextWriter`'s own algorithm this file must reproduce: when the *current*
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
    ///   principle #3 (`CLAUDE.md`), never crash, applies to a reimplementation of a real crate's
    ///   real behavior exactly as much as to code this file invented itself: reproducing tui-markdown's
    ///   real, if buggy, *rendering* for a shape it happens to mishandle (see the module doc comment)
    ///   is not license to reproduce an *unrelated* panic this file's own bookkeeping introduces on
    ///   top of that — a fixed literal `1` here panicked on exactly this shape (a **loose** item's own
    ///   task marker, landing on that fresh, empty line with zero spans on it) before this existed; see
    ///   `contains_unsupported`'s own former `"LooseTask"` exclusion, now removed now that this no
    ///   longer needs it, and this method's home tests.
    fn task_list_marker(&mut self, checked: bool) {
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
            self.push_span(marker_span);
        }
    }
}

/// Walks `events`, dispatching every event a `Heading`'s or `Paragraph`'s own inline content can
/// contain to the matching `Writer` method — mirroring `TextWriter::handle_event`/`start_tag`/
/// `end_tag` exactly for those events (see the module doc comment). Recurses one level for every
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
            Event::Text(t) => w.text(&t),
            Event::Code(c) => w.code(&c),
            Event::SoftBreak => w.soft_break(),
            Event::HardBreak => w.hard_break(),
            // `Html`/`InlineHtml`/`FootnoteReference`/`InlineMath`/`DisplayMath`: tui-markdown's own
            // `handle_event` only ever `warn!`s and drops these, producing no output (see the crate
            // citation in the module doc comment) — matched here the same way, silently.
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
/// through `push_link`/`pop_link`, or — for `Image` — walks its alt-text content with no style change
/// and no destination recorded at all (mirroring tui-markdown's own `Tag::Image` arm; see the module
/// doc comment on why the URL is dropped). Anything else cannot legitimately open inside a `Heading`'s
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
            w.push_link(dest_url.into_string());
            walk_inline(events, w, Some(TagEnd::Link));
            w.pop_link();
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

/// Heading attribute metadata (`{#id .class key=value}`) collected from `Tag::Heading`'s own fields,
/// formatted exactly the way `tui_markdown`'s own (crate-private) `HeadingMeta::to_suffix` does — see
/// that type in the pinned `tui-markdown-0.3.7` source; it is not exported by the crate, so this is a
/// small reimplementation of pure output *formatting*, not a second judgment about document
/// *structure* (the kind of duplication this refactor's other reused functions — `decorate_md_lines`,
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
/// helpers all want an owned `Event<'a>` stream (mirroring `tui_markdown::TextWriter`'s own
/// `Iterator<Item = Event<'a>>` bound, matching a real `Parser`'s own `Item` type), so this is the
/// one place a `&[(Event<'a>, Range<usize>)]` slice becomes that shape — every other caller in this
/// file goes through it rather than writing `events.iter().map(|(ev, _)| ev.clone())` again itself.
fn events_iter<'a, 'e>(
    events: &'e [(Event<'a>, Range<usize>)],
) -> impl Iterator<Item = Event<'a>> + 'e {
    events.iter().map(|(ev, _)| ev.clone())
}

/// Renders one `Heading` block into `w`, in place — mirrors `TextWriter::start_heading`/
/// `end_heading`. `inline` is `Doc.events[block's own inline range]`, read directly (no second parse
/// — see the module doc comment).
fn render_heading(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    level: u8,
    inline: Range<usize>,
    meta: &HeadingMeta,
) {
    if w.needs_newline {
        w.push_blank_line();
    }
    let heading_style = w.styles.heading(level);
    let hashes = format!("{} ", "#".repeat(level as usize));
    w.push_line(Line::styled(hashes, heading_style));
    w.needs_newline = false;
    walk_inline(&mut events_iter(&doc.events[inline]), w, None);
    if let Some(suffix) = meta.to_suffix() {
        w.push_span(Span::styled(suffix, w.styles.heading_meta()));
    }
    w.needs_newline = true;
}

/// Renders one **real** `Paragraph` block into `w`, in place — mirrors `TextWriter::start_paragraph`/
/// `end_paragraph` (the *unconditional* second blank-line push and all — see `TextWriter::
/// start_paragraph`'s own two-part body, and `render_item`'s own doc comment for why that second,
/// unconditional push is exactly what strands a **loose** item's task marker and its own first line
/// of text on a line of their own, one row below the item's bullet/number). Never called for a
/// **tight** list item's own first (synthetic) paragraph — see `render_item`, which renders that
/// one's inline content directly via `walk_inline` instead, with no blank-line bookkeeping of its own
/// at all, matching `TextWriter` never seeing a `Start(Paragraph)` event for one in the first place.
fn render_paragraph(w: &mut Writer<'_>, doc: &Doc<'_>, inline: Range<usize>) {
    if w.needs_newline {
        w.push_blank_line();
    }
    w.push_blank_line();
    w.needs_newline = false;
    walk_inline(&mut events_iter(&doc.events[inline]), w, None);
    w.needs_newline = true;
}

/// `math_slot`/`images`/`width` bundled together for `Writer::math` — the math analogue of
/// `MdRenderCtx` (`markdown.rs`'s own bundling of same-shaped trailing arguments): keeps the
/// `math_slot` closure and `width` next to the one, shared `images` accumulator `render_math_slot`
/// pushes to, so `RenderOut::images` can collect it once `render_doc`'s own walk finishes, no matter
/// how many math expressions the document lifts or how deep in a `List` a given one sits. Owned by
/// `Writer` itself, not passed as a separate parameter — see that struct's own doc comment on `math`
/// for why.
struct MathCtx<'a> {
    slot: &'a dyn Fn(&str, bool) -> MathSlot,
    width: u16,
    images: Vec<ImagePlacement>,
}

/// Renders a `Paragraph` block into `w`, in place, choosing between `render_paragraph_math` (lifts any
/// LaTeX math the paragraph's own text contains onto its own placeholder line(s)) and plain
/// `render_paragraph` — the one call site every `Paragraph`-rendering path in this file (`render_doc`'s
/// own top-level dispatch, `render_block`'s generic one, and `render_item`'s first-child special case
/// — see `walk_inline_math`, its own analogous split) goes through, so the decision itself never gets
/// duplicated. `math_here` is `render_block`/`render_list`/`render_item`'s own propagated answer to
/// "is math lifting active at this position in the tree" (see `render_doc`'s own doc comment for how
/// it starts `true` and gets forced `false` the moment a `Quote` is entered); `w.math.is_some()` is
/// `render_doc`'s own `math_on` argument, unconditionally true or false for the *whole* render. Both
/// have to hold — `math_here` alone is not enough on its own, since a caller could (in principle) call
/// this function with `math_here: true` while `w.math` is `None` (`math_on` was never set at all) —
/// though no call site in this file actually does; checked here all the same, once, rather than
/// trusting every caller to have already combined the two correctly.
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
    block_start: usize,
) {
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
/// ## Why per-`Event::Text`, not per-block, and why that is exactly production's own splitting
///
/// Production (`render_markdown_with_images`) lifts math by running `split_math` — not on any single
/// block's own text, but on (up to) the **whole remaining document** as one `SourceRun`, *before*
/// tui-markdown (or this file's own `Doc::parse`) ever sees it, splitting the raw source itself at
/// every match and handing each resulting fragment to its own, entirely independent re-parse. This
/// file reads its inline content from a *single*, whole-document parse instead (see the module doc
/// comment on why that is the entire point of this refactor) — re-parsing a fragment here would
/// reintroduce, for math specifically, the exact cross-block reference-link gap eliminating re-parsing
/// closed for everything else (see `md_render_diff_tests`'s own module doc comment). What this
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
/// *following* a lifted math expression starts a **brand new** `tui_markdown::TextWriter`, whose own
/// `needs_newline` starts `false` — so whatever comes right after a lift never gets the leading blank
/// line it would have gotten had rendering simply continued (see `ensure_fresh_after_math`'s own doc
/// comment for the mechanics, and CommonMark's own paragraph-content trimming, which the now-isolated
/// fragment's own leading/trailing whitespace becomes newly subject to — see `render_text_with_math`'s
/// own trimming). Confirmed against the real production pipeline (not merely inferred) via
/// `code_span_corpus`'s own cases: math sitting at the very end of a paragraph is followed *directly*
/// by whatever top-level block comes next (no separator line at all — e.g. a `ThematicBreak`
/// immediately after, matching a *fresh* `Writer`'s own `needs_newline == false` start), and math with
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
/// (`w.pending_para_start = Some(w.needs_newline)`) and lets `walk_inline_math` resolve it, via
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
    w.pending_para_start = Some(w.needs_newline);
    walk_inline_math(
        &mut events_iter_ranged(&doc.events[inline]),
        w,
        src,
        block_start,
    );
    // A paragraph whose own trailing content was math (nothing textual after the last lift) leaves
    // `needs_newline == false`, matching a *fresh* `Writer`'s own start state — see this function's
    // own doc comment. Only a paragraph that ends in real text (even text *following* an earlier
    // lift) gets the normal `end_paragraph()`-equivalent `true`.
    if !w.after_math {
        w.needs_newline = true;
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
/// (`code`/`soft_break`/`hard_break`, or a nested `start_inline_tag` call) ultimately reaches
/// `Writer::push_span`, whose own fallback (`self.lines.last_mut()` being `Some`) would otherwise
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
                if let Some((display, closer)) = backslash_math_opener(gap, &t) {
                    render_backslash_math(
                        w,
                        events,
                        pending.take(),
                        t,
                        display,
                        closer,
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
                w.code(&c);
            }
            Event::SoftBreak => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                ensure_fresh_after_math(w);
                w.soft_break();
            }
            Event::HardBreak => {
                if let Some(p) = pending.take() {
                    render_text_with_math(w, &p, false);
                }
                ensure_fresh_after_math(w);
                w.hard_break();
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
/// directly, not merely inferred: production's own rendering for `"This is \[not a link](nope).\n"` is
/// `["This is ", "[not a link", "]", "(nope)."]` — **four separate spans**, no backslash anywhere in
/// the output at all (unsurprising once the reason is clear: tui-markdown's own rendering, like this
/// one, works from `Doc.events`, and pulldown-cmark's own escape handling had already stripped the
/// backslash — and only the backslash, never the bracket it escapes — out of the event stream entirely
/// before either renderer ever got to see it; see `backslash_math_opener`'s own doc comment for the
/// exact byte ranges that prove this). A hand-rolled literal string (`"\\[" + content`, an earlier
/// version of this function did exactly that) reintroduces a backslash neither renderer's own event
/// walk would ever produce, *and* flattens what production keeps as several separate spans into one —
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
#[allow(clippy::too_many_arguments)] // one call site (walk_inline_math); each argument is its own distinct, already-computed piece of state — bundling any subset into a struct would only rename the same eight pieces of information, not reduce them
fn render_backslash_math<'a>(
    w: &mut Writer<'_>,
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    pending: Option<pulldown_cmark::CowStr<'a>>,
    first_text: pulldown_cmark::CowStr<'a>,
    display: bool,
    closer: char,
    src: &str,
    prev_end: &mut Option<usize>,
) {
    let opener = if display { '[' } else { '(' };
    let mut content = first_text[opener.len_utf8()..].to_string();
    let mut fallback: Vec<Event<'a>> = vec![Event::Text(first_text)];
    loop {
        let Some((ev, range)) = events.next() else {
            if let Some(p) = pending {
                render_text_with_math(w, &p, false);
            }
            replay_events(w, fallback);
            return;
        };
        let gap = prev_end.map(|p| &src[p..range.start]);
        *prev_end = Some(range.end);
        let is_break = matches!(ev, Event::SoftBreak | Event::HardBreak);
        if let Event::Text(t) = &ev {
            if gap == Some("\\") && t.starts_with(closer) {
                if let Some(p) = pending {
                    render_text_with_math(w, &p, true);
                }
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
            content.push_str(t);
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
                w.code(&c);
            }
            Event::SoftBreak => {
                ensure_fresh_after_math(w);
                w.soft_break();
            }
            Event::HardBreak => {
                ensure_fresh_after_math(w);
                w.hard_break();
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
///    comes next, mirroring production's own "every fragment following a lift is its own brand-new
///    `TextWriter`" behavior (see `render_paragraph_math`'s own doc comment) without actually
///    re-parsing: the *boundary*, not a second parse, is the only thing that needs reproducing.
///
/// These two states can never both apply to the very same call: `pending_para_start` is only ever
/// `Some` before *anything* has rendered in this walk, and `after_math` only ever becomes `true`
/// *after* `render_math_slot` has already resolved (opened or dropped) whatever pending slot there
/// was — so checking `pending_para_start` first, unconditionally, then `after_math`, is safe; at most
/// one of the two blocks below ever actually does anything for a given call. Both are no-ops in the
/// overwhelmingly common case (most events in most paragraphs are neither the walk's own first content
/// nor immediately follow a lift), so every call site in `walk_inline_math`'s own dispatch loop calls
/// this unconditionally, ahead of its own normal handling, rather than each having to re-derive either
/// condition itself. The explicit `w.after_math = false` is redundant with `push_blank_line`'s own
/// side effect (`Writer::push_line` clears it unconditionally — see that struct's own doc comment) but
/// kept anyway: this function's own contract ("the flag is consumed") should hold by its own reading,
/// not merely as an accident of what its callee happens to also do.
fn ensure_fresh_after_math(w: &mut Writer<'_>) {
    w.open_pending_para_start();
    if w.after_math {
        if w.needs_newline {
            w.push_blank_line();
        }
        w.push_blank_line();
        w.needs_newline = false;
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
/// (rare, not exercised by the parity corpus, but handled the same principled way rather than left to
/// guess) is exactly what an independent re-parse of whitespace-only text produces too: nothing at all
/// (`render_markdown_tasks_opts`'s own `text.trim().is_empty()` skip, right where it dispatches each
/// split fragment).
fn render_text_with_math(w: &mut Writer<'_>, text: &str, trailing_lift: bool) {
    let lines: Vec<&str> = text.lines().collect();
    let line_count = lines.len();
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 {
            w.push_blank_line();
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
        let n = parts.len();
        let is_last_line = i + 1 == line_count;
        for (idx, part) in parts.into_iter().enumerate() {
            match part {
                MathPart::Text(run) => {
                    let mut t = run.text().to_string();
                    if idx > 0 {
                        t = t.trim_start_matches([' ', '\t']).to_string();
                    }
                    let last_frag_on_last_line = is_last_line && idx + 1 == n;
                    if idx + 1 < n || (trailing_lift && last_frag_on_last_line) {
                        t = t.trim_end_matches([' ', '\t']).to_string();
                    }
                    if t.is_empty() {
                        continue;
                    }
                    ensure_fresh_after_math(w);
                    let style = w.cur_style();
                    w.push_span(Span::styled(t, style));
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
}

/// Renders one lifted math expression's own placeholder — the exact three functions
/// `render_markdown_with_images`'s own `BlockPart::Math` arm calls, reused verbatim, so a document
/// that lifts math renders identically whether a live `Picker` answers `math_slot` with a real raster
/// (`MathSlot::Image`), a background render still in flight (`MathSlot::Loading`), or neither
/// (`MathSlot::Raw`, design principle #3's own safe degrade — the raw LaTeX source, dimmed). Sets
/// `w.needs_newline = false` and `w.after_math = true` unconditionally on its own way out — see
/// `Writer`'s own doc comment on `after_math` for why every placeholder line goes straight to
/// `self.lines` here rather than through `push_line` (which would immediately clear the very flag this
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
            let placement_line = w.lines.len();
            w.lines
                .extend(math_placeholder_lines(cols, rows, width, display));
            if let Some(math) = w.math.as_mut() {
                math.images.push(ImagePlacement {
                    url: math_url(latex, display),
                    alt: "math".into(),
                    line: placement_line,
                    cols,
                    rows,
                    fence_ord: None,
                });
            }
        }
        MathSlot::Loading => {
            w.lines
                .extend(image_loading_line("math", "equation", width));
        }
        MathSlot::Raw => {
            w.lines.extend(math_raw_lines(latex, display));
        }
    }
    w.needs_newline = false;
    w.after_math = true;
}

/// Renders a `ThematicBreak` into `w`, in place — mirrors `TextWriter::rule`. The literal text
/// tui-markdown's own `rule()` pushes (`Line::from("---")`) — `decorate_extras` (called via
/// `decorate_md_lines`, back in `render_doc`) recognizes exactly this shape on an *unprefixed* line
/// and replaces it with the real full-width rule; see the module doc comment for what happens instead
/// when this line is `Quote`-prefixed.
fn render_rule(w: &mut Writer<'_>) {
    if w.needs_newline {
        w.push_blank_line();
    }
    w.push_line(Line::from("---"));
    w.needs_newline = true;
}

/// Renders one `CodeBlock` — fenced or indented, at any nesting depth this file covers (top level,
/// inside a list item, inside a quote — `render_block`'s own `CodeBlock` arm, and `render_doc`'s own
/// top-level one, are this function's only two callers) — directly from the model's own
/// `lang`/`body_spans`, reusing the exact building blocks production's own `flush_code_run` calls for
/// the identical shape: `code_header` for the language-badge row, `highlight_body` for the
/// syntect-highlighted, gutter+background body rows, and `gutter_span`/`pad_to_width` for the bottom
/// padding row.
///
/// Unlike `flush_code_run` — which has to *re-derive* a code block's own extent from tui-markdown's
/// own *rendered* output (`is_code_block_line`, a maximal run of `fg(White)` lines), because that is
/// all production has to go on — this already *has* the block's own exact byte extent, straight from
/// the one whole-document parse `Doc::parse` performed (see the module doc comment: this is the entire
/// point of this refactor). It therefore never mis-detects a block's own boundary the way
/// `flush_code_run` can — see that function's own doc comment, and `code_corpus`'s own "list item
/// fence glued to a following inline-code paragraph" cases, the very shape that motivated this
/// refactor in the first place — every `CodeBlock` this file's own `Doc::parse` reports gets exactly
/// one header, drawn from the model, never zero (contrast `render_bare_paragraph`'s own doc comment
/// for the companion half of that same fix: keeping whatever *follows* this block from gluing onto its
/// own last row).
///
/// ## The blank line above the header
///
/// Mirrors real `TextWriter::start_codeblock`'s own leading blank-line check *exactly* — `if
/// !self.text.lines.is_empty() { push_line(default) }` — which is **not** the same condition every
/// other block-rendering function in this file uses (`if w.needs_newline`). Confirmed directly against
/// real tui-markdown (not merely inferred): a fence is *always* preceded by exactly one blank row
/// whenever anything at all has already been rendered — even when the source has *no* blank line
/// before it (`"para\n```\naaa\n```\n"` renders identically to `"para\n\n```\naaa\n```\n"`, one blank
/// row either way — a fence is a leaf block that can interrupt a paragraph without one), even when the
/// immediately preceding content is a **tight** list item's own bare inline text with `needs_newline`
/// still `false` (`"- text\n  ```\n  aaa\n  ```\n"` still gets the blank row), and even when the code
/// block is a list item's own very *first* child with no leading text at all (`"- \n  ```\n  aaa\n
/// ```\n"` — the blank row lands right after the bare `"- "` marker row) — but *not* when the code
/// block is the very first thing in the whole document (`w.lines.is_empty()` still true at that
/// point). `w.needs_newline` plays no part in this decision at all — using it here instead (matching
/// every *other* block-rendering function's own convention) would skip the blank row for exactly the
/// two shapes above where `needs_newline` happens to already be `false` for an unrelated reason (a
/// **tight** item's own bare content, per `Writer::text`'s own doc comment, or `render_item`'s own
/// unconditional reset right before dispatching an item's first child) even though real tui-markdown
/// still draws one.
///
/// The one further exception, `w.fresh_boundary` (see that field's own doc comment): a fence
/// directly following a GitHub alert or a `Details` block gets **no** leading blank row, even though
/// `w.lines` is (correctly) non-empty by then — production's own next fragment there is a brand-new,
/// independent `tui_markdown::TextWriter` whose own `!self.text.lines.is_empty()` reads `false`
/// (nothing precedes the fence *within that fragment*), the identical "fresh reparse boundary" a
/// lifted math expression already reproduces via `after_math`.
///
/// ## The width used for the header/body/padding band
///
/// `w.width` (`render_doc`'s own incoming column count), reduced by whatever quote-prefix width is
/// currently open (`w.prefix_width()`) — never by list nesting, which (confirmed the same way — see
/// this function's own leading-blank-line evidence above, gathered by rendering real list content and
/// reading the columns back) adds no left margin of its own at all, in production or here. Reducing by
/// the quote-prefix width keeps a quote-nested code block's own full-width background band from
/// overflowing the terminal once `push_line` prepends the `"> "` prefix on top of it — a genuine
/// judgment call, not something checked against a production precedent, because production does not
/// decorate a quote-nested fence at all (its own `flush_code_run` never even recognizes one — the
/// first rendered row reads `"> ```"`, not `"```"` — see that function's own doc comment). See
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
/// genuinely empty fence draws no body rows at all (matching `flush_code_run`'s own `highlight_body`
/// early return for an empty `body: &[String]`), while one blank content line still draws exactly one
/// (highlighted, gutter+background) blank row.
fn render_code_block(
    w: &mut Writer<'_>,
    src: &str,
    lang: Option<&str>,
    body_spans: &[Range<usize>],
) {
    if !w.lines.is_empty() && !w.fresh_boundary {
        w.push_blank_line();
    }
    let content_w = (w.width as usize).saturating_sub(w.prefix_width());
    // Mirrors `flush_code_run`'s own `let lang = opening_trimmed.trim_matches('`').trim().to_string();`
    // — the model's own `lang` never carries backticks (those are fence syntax, not part of the info
    // string pulldown-cmark reports), so only the `.trim()` half is still needed here.
    let lang_trimmed = lang.unwrap_or("").trim();
    let label = if lang_trimmed.is_empty() {
        "code"
    } else {
        lang_trimmed
    };
    w.push_line(code_header(label, content_w, w.code));
    let body_text = code_body_text(body_spans, src);
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
        w.push_line(line);
    }
    w.push_line(pad_to_width(
        vec![gutter_span(w.code.bg)],
        content_w,
        w.code.bg,
    ));
    w.needs_newline = true;
}

/// The generic block dispatcher used for every position in this file's block tree that is **not** a
/// `List`'s own item sequence (which `render_list` walks itself, since each one needs a marker built
/// first — see `render_item`) or an item's own *first* child (which `render_item` special-cases for
/// exactly the tight/loose distinction just described): a `Quote`'s own body, and every child of a
/// list item after the first. Mirrors `TextWriter::start_tag`'s own top-level dispatch for the block
/// kinds this pass covers.
///
/// `Table`/`Html` never legitimately reach this function: every caller that walks into a
/// `List`/`Quote`/`Details`'s own children only does so after `contains_unsupported` has already
/// confirmed neither appears anywhere in that subtree (see `render_doc`'s own dispatch, and that
/// function's own doc comment) — so encountering one here would mean a bug upstream of this file,
/// not merely input a smaller local check failed to reject. `ListItem` never legitimately reaches
/// this function either — it only ever exists as a `List`'s own child (see
/// `model::BlockKind::ListItem`'s doc comment), and `render_list` unwraps every one of those itself.
/// Matched defensively (silently rendering nothing further) rather than assumed unreachable all the
/// same, so a bug elsewhere degrades to "this one block goes missing" instead of a panic — principle
/// #3 (`CLAUDE.md`). `CodeBlock`, `Quote { alert: Some(_), .. }`, and `Details` *do* legitimately
/// reach here (`contains_unsupported` no longer screens any of the three out — see that function's
/// own doc comment) — `render_code_block`/`render_alert_from_model`/`render_details_from_model` draw
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
            render_heading(w, doc, *level, inline.clone(), &meta);
        }
        BlockKind::Paragraph { inline } => {
            if is_real_paragraph(src, block) {
                render_paragraph_dispatch(
                    w,
                    doc,
                    src,
                    inline.clone(),
                    ctx.math_here,
                    block.src.start,
                );
            } else {
                render_bare_paragraph(w, doc, src, inline.clone(), ctx.math_here, block.src.start);
            }
        }
        BlockKind::ThematicBreak => render_rule(w),
        BlockKind::List { start, .. } => render_list(w, doc, src, *start, &block.children, ctx),
        BlockKind::Quote { alert: None, .. } => render_quote(w, doc, src, &block.children, ctx),
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
        } => render_code_block(w, src, lang.as_deref(), body_spans),
        BlockKind::Details { open_attr, summary } => {
            render_details_from_model(w, doc, src, *open_attr, summary, &block.children, ctx)
        }
        BlockKind::Table { .. } | BlockKind::Html { .. } => {}
        // Never reachable — see the doc comment above.
        BlockKind::ListItem { .. } => {}
    }
}

/// Renders a `List`'s own items into `w`, in place — mirrors `TextWriter::start_list`/`end_list`
/// wrapped around one call to `render_item` per item. `start` is `BlockKind::List.start` verbatim
/// (`None` for an unordered list, `Some(n)` for an ordered one's own first number) — the exact value
/// `list_indices` itself holds for this list for as long as it stays open, matching `TextWriter`'s own
/// `list_indices.push(index)` in `start_list` (which receives that same `Tag::List(start_index)`
/// value directly). Not gated on `BlockKind::List.ordered` — that field is redundant with
/// `start.is_some()` by construction (see `model::BlockKind::List`'s own doc comment on how
/// `parse_container` computes it), so there is nothing `ordered` could tell this function that `start`
/// does not already say.
///
/// The exit's own `needs_newline = true` is *conditional* on `!w.after_math` — unlike every other
/// container-closing site in this file, this one genuinely can run with the flag still set: the very
/// last item this loop rendered may have ended in a lifted math expression with nothing textual after
/// it (see `render_paragraph_math`'s own doc comment for why that is exactly the shape a whole `List`
/// can legitimately end on, and `Writer`'s own doc comment on `after_math` for why re-forcing
/// `needs_newline` back to `true` here — the way `TextWriter::end_list` always does — would reintroduce
/// the very separator line the lift's own reset exists to suppress).
fn render_list(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    start: Option<u64>,
    items: &[Block],
    ctx: BlockCtx,
) {
    if w.list_indices.is_empty() && w.needs_newline {
        w.push_blank_line();
    }
    w.list_indices.push(start);
    for item in items {
        if let BlockKind::ListItem { task } = &item.kind {
            render_item(w, doc, src, *task, &item.children, ctx);
        }
        // A `List`'s own children are always `ListItem`s (see `model::parse_container`'s own
        // `Tag::List` arm) — the `if let` above is not a lossy filter of some other shape, only the
        // same defensive-match convention this whole file follows rather than an unchecked `panic!`.
    }
    w.list_indices.pop();
    if !w.after_math {
        w.needs_newline = true;
    }
}

/// Renders one list item into `w`, in place — mirrors `TextWriter::start_item` (the marker) plus,
/// where the item has one, `TextWriter::task_list_marker` — and, for the item's own remaining
/// content, whichever of `render_paragraph`/`render_block`/`walk_inline` its own first child calls
/// for (see below).
///
/// ## The tight/loose distinction, and why it decides everything here
///
/// `TextWriter` never emits a `Start(Paragraph)` event for a **tight** item's own content at all —
/// pulldown-cmark hands it the item's inline events directly, right after `Start(Item)`; `model`
/// still gives that content somewhere to live, a *synthetic* `Paragraph` `collect_stray_inline_run`
/// builds on the fly (see that function's own doc comment), but there was never a real `Tag::Paragraph`
/// for it. A **loose** item's own first paragraph, by contrast, is real — pulldown-cmark reports an
/// ordinary `Start(Paragraph)`/`End(Paragraph)` pair around it, indistinguishable in `Doc.events` from
/// any other paragraph in the document. `is_real_paragraph` reads `src` to tell the two apart
/// (see `render_doc`'s own doc comment for exactly which byte settles it), because the two cases
/// produce genuinely different output:
///
/// * **Tight**: no `start_paragraph()` call happens at all, so its own unconditional blank-line push
///   never fires — the item's inline content continues directly on the very same line `start_item`
///   just opened (the marker's own), and (if the item has one) a task marker lands on that same line
///   too, spliced into the `"- "` bullet span itself.
/// * **Loose**: `start_paragraph()` *does* fire, unconditionally pushing a **second**, fresh, empty
///   line right after the marker's own — before either the task marker (if the item has one) or the
///   paragraph's own first line of text is written. The net effect: a loose item's own bullet/number
///   sits alone, on its own row, with the marker and text one row below it — a real (if perhaps
///   surprising) property of `TextWriter`'s own algorithm, not a bug this file introduces or should
///   try to "fix"; see the module doc comment on faithfully reproducing whatever the real renderer
///   does, quirks included.
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
/// dispatch, math-scanning aside. `w.needs_newline = loose_first && !w.after_math` folds `after_math`
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
/// its own `loose_first` branch below, on purpose: a loose item's own task marker (`w.task_list_marker`,
/// called unconditionally right after `loose_first`'s own eager blank-line push, task or no task) needs
/// that row to already exist the moment it runs, *regardless* of whether the item's first child's own
/// first rendered content later turns out to be math — deferring the push here would either strand the
/// marker on the wrong (marker's own) row when a task is present, or require re-deriving "does this
/// item have a task" a second time inside the pending-slot machinery for no benefit, since a loose
/// list item beginning directly with a lifted math expression is not a shape any corpus case exercises
/// (nor one this task's own scope — closing `KNOWN_MISMATCHES`'s three top-level math entries — asks
/// for). Left as the pre-existing eager push, unchanged, rather than risked on an unverified guess at
/// what production would even do there.
fn render_item(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    task: Option<Task>,
    children: &[Block],
    ctx: BlockCtx,
) {
    // start_item(): a fresh physical line, unconditionally, carrying the item's own marker.
    w.push_line(Line::default());
    push_item_marker(w);
    w.needs_newline = false;

    let loose_first = children.first().is_some_and(|b| is_real_paragraph(src, b));
    if loose_first {
        w.push_blank_line();
        w.needs_newline = false;
    }
    if let Some(t) = task {
        w.task_list_marker(matches!(t.state, 'x' | 'X'));
    }

    let mut rest = children;
    if let Some(first) = children.first() {
        if let BlockKind::Paragraph { inline } = &first.kind {
            // Tight or loose, the item's own first paragraph's content continues directly on
            // whichever line is already open — the marker's own (tight), or the blank one
            // `loose_first` just opened above (loose) — never a *second* blank-line push of its
            // own; that is exactly what distinguishes this from `render_paragraph`, which this
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
            // unconditional `needs_newline = true` applies, exactly as it would via
            // `render_paragraph`, *unless* its own trailing content was a math lift (`w.after_math`
            // — see this function's own doc comment). `loose_first == false` (tight): there was no
            // wrapping paragraph tag at all, so no `end_paragraph()` ever ran — `walk_inline`'s own
            // inline-only calls (`Writer::text` chief among them; see its own doc comment) leave
            // `needs_newline` exactly as `false` as `start_item` set it. Setting this unconditionally
            // to `true` here — an earlier version of this function did — inserted a spurious blank
            // line before any `Quote`/`Heading`/`ThematicBreak` directly interrupting a tight item's
            // own paragraph with no blank line in the source; see `list_corpus`'s own "block quote
            // can interrupt a tight item's own paragraph with no blank line" case, which is exactly
            // what caught this.
            w.needs_newline = loose_first && !w.after_math;
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
/// `start_paragraph`/`end_paragraph` pair to supply it — is consuming a *pending* `w.needs_newline`
/// left by whatever sibling came immediately before this one, so this content starts on its own fresh
/// row rather than gluing onto that sibling's own last row. Mirrors real `TextWriter::text`'s own
/// per-event guard (`if self.needs_newline { push_line(default); needs_newline = false }`), but
/// applied **once**, up front, rather than repeated inside every individual `Writer` method the way
/// the real `text`'s own copy is.
///
/// ## A deliberate, documented departure from tui-markdown's own (buggy) behavior
///
/// Confirmed directly against real tui-markdown: only `TextWriter::text` carries this guard —
/// `code`/`soft_break`/`hard_break` do not consult `needs_newline` at all, so when a bare paragraph's
/// own *first* inline event happens to be something other than `Event::Text` (most commonly
/// `Event::Code`, an inline code span — e.g. `` `inline` after `` immediately following a list-nested
/// fence's own closing marker with no blank line — `code_corpus`'s own "KNOWN FAILURE: list item fence
/// glued to a following inline-code paragraph" cases), real tui-markdown glues that content directly
/// onto whatever row is currently open. Production's own consequence of this — `flush_code_run`'s own
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
/// count-guard left to desynchronize the way `flush_code_run`'s is. See `md_render_diff_tests`'s own
/// `INTENDED_IMPROVEMENTS` list for where this shows up as a *documented* mismatch against production,
/// not a gap.
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
fn render_bare_paragraph(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    inline: Range<usize>,
    math_here: bool,
    block_start: usize,
) {
    if w.needs_newline {
        w.push_blank_line();
        w.needs_newline = false;
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
/// opened — mirrors `TextWriter::start_item`'s own marker half exactly (the width formula, the
/// unstyled `"<spaces>- "` for an unordered item, the `LightBlue`-styled, right-aligned `"N. "` for
/// an ordered one, incrementing `w.list_indices`'s own top entry as it goes). Always called with
/// `w.list_indices` non-empty (`render_list` pushes this list's own entry before rendering a single
/// item), so the `let... else` below is defensive only, never actually reached.
fn push_item_marker(w: &mut Writer<'_>) {
    let width = w.list_indices.len() * 4 - 3;
    let Some(last_index) = w.list_indices.last_mut() else {
        return;
    };
    let span = match last_index {
        None => Span::raw(" ".repeat(width - 1) + "- "),
        Some(index) => {
            *index += 1;
            Span::styled(
                format!("{:width$}. ", *index - 1),
                Style::new().fg(Color::LightBlue),
            )
        }
    };
    w.push_span(span);
}

/// Renders a **plain** `Quote { alert: None, .. }` block's own body into `w`, in place — mirrors
/// `TextWriter::start_blockquote`/`end_blockquote` wrapped around a normal walk of the quote's own
/// children (`render_block`, the same generic dispatcher a list item's non-first children go
/// through — a quote's own body can contain any of `Heading`/`Paragraph`/`ThematicBreak`/`List`/
/// a further nested `Quote`/`Details`, all of which get the same `>`-prefix treatment via
/// `Writer::push_line`; see this module's own `Writer` doc comment). A nested `Quote` reached this
/// way (`>>`) simply recurses into this same function again, pushing a *second* `>` onto
/// `w.line_prefixes`. A GitHub alert (`Quote { alert: Some(_), .. }`) never reaches this function at
/// all — see `render_alert_from_model`, this function's own sibling for that shape.
///
/// Always calls `render_block` with `math_here: false`, regardless of whatever value was in `ctx` for
/// *this* quote — production never lifts math out of a blockquote's own text at all (`structure_mask`'s
/// own exclusion; see `render_paragraph_math`'s own doc comment), so nothing this function dispatches
/// to, however many more `List`s or nested `Quote`s that subtree goes on to contain, should ever end
/// up scanning for it either. Because of that, `w.after_math` can never legitimately be `true` by the
/// time this function's own exit runs (nothing in its own subtree can have set it) — so, unlike
/// `render_list`'s own exit, this one stays the plain, unconditional `needs_newline = true` `TextWriter`
/// itself always uses for `end_blockquote`.
///
/// `details_interactive`, by contrast, is **propagated unchanged** to `children` (not forced) — a
/// plain quote's own body is still reached through the *same* top-level scan production's
/// `split_details` performs on the whole segment (unlike a GitHub alert's or a `Details` block's own
/// body, each of which gets its own, independent re-parse through `render_md_body_nested` — see
/// `BlockCtx.details_interactive`'s own doc comment): a `Details` block nested inside a plain quote
/// (reached at the *top* level, i.e. `ctx.details_interactive` still `true` coming in) is exactly as
/// interactive as one sitting directly at the top level.
fn render_quote(w: &mut Writer<'_>, doc: &Doc<'_>, src: &str, children: &[Block], ctx: BlockCtx) {
    if w.needs_newline {
        w.push_blank_line();
        w.needs_newline = false;
    }
    w.line_prefixes.push(Span::raw(">"));
    w.line_styles.push(w.styles.blockquote());
    let child_ctx = BlockCtx {
        math_here: false,
        details_interactive: ctx.details_interactive,
    };
    for child in children {
        render_block(child, w, doc, src, child_ctx);
    }
    w.line_prefixes.pop();
    w.line_styles.pop();
    w.needs_newline = true;
}

/// Renders a GitHub alert (`Quote { alert: Some(kind), alert_title }`) into `w`, in place — the
/// header line via `super::alert_header_line` (identical color/icon/label decision `markdown.rs`'s
/// own `render_alert` uses, see that function's own doc comment for why this is shared rather than
/// re-derived), the body via [`render_bar_prefixed_body`] with `super::alert_bar(kind.color())` as
/// the bar. `render_alert`'s own header line, in production, is simply appended directly onto
/// whatever `Vec<Line>` the caller handed it (`render_markdown_tasks_opts`'s own
/// `out.extend(render_alert(..))`) — no leading-blank-line check of any kind, regardless of what
/// `needs_newline`-equivalent state the *previous* segment happened to end in (`split_alerts` peels
/// the alert into its own, independent fragment; nothing carries across that boundary) — so this
/// function does not check `w.needs_newline` before pushing the header either, and leaves it `false`
/// on the way out (not `true`): matching that same "fresh reparse boundary" contract
/// `render_math_slot`'s own doc comment already documents in full for a lifted math expression —
/// whatever renders *next* is exactly as if a brand-new `Writer` had just started, since production's
/// own next fragment (if any) is rendered by a brand-new `tui_markdown::TextWriter` too.
///
/// Does **not** take a `BlockCtx`: nothing about this function's *own* rendering depends on either of
/// its two fields, and `children` always gets a freshly forced one regardless of what the caller had
/// in scope — see [`render_bar_prefixed_body`]'s own doc comment for exactly what gets forced and
/// why (both `math_here` and `details_interactive`, unconditionally, matching production's own
/// `render_md_body_nested` — the single body-rendering entry point both an alert's and a `Details`
/// block's own body funnel through).
fn render_alert_from_model(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    kind: AlertKind,
    title: &str,
    children: &[Block],
    quote_src_start: usize,
) {
    w.push_line(alert_header_line(kind, title, w.icons));
    let header_end = line_after(src, quote_src_start);
    let trimmed = trim_leading_header(children, doc, header_end);
    render_bar_prefixed_body(w, doc, src, &trimmed, alert_bar(kind.color()));
    w.needs_newline = false;
    w.fresh_boundary = true;
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
/// there is no leading-blank-line check here either, and why `w.needs_newline` ends up `false`, not
/// `true`, on the way out (the identical "fresh reparse boundary" production's own
/// `split_details`/`render_details` produce, this time for a `<details>` block instead of an alert).
///
/// `ctx.details_interactive` decides, for *this* block specifically (not its children, which
/// `render_bar_prefixed_body` always forces `false` regardless — see that function's own doc
/// comment): whether it consumes a slot from the document-wide Tab-toggle ordinal sequence
/// (`super::next_details_open(open_attr)`, and draws the marker in the Tab-focus sentinel style) or
/// simply uses its own `open_attr` directly (no ordinal consumed, a plain marker style) — the exact
/// `interactive` distinction `super::render_details`'s own doc comment documents in full, mirrored
/// here via `super::details_marker_line`'s own identical parameter.
fn render_details_from_model(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    open_attr: bool,
    summary: &str,
    children: &[Block],
    ctx: BlockCtx,
) {
    let open = if ctx.details_interactive {
        next_details_open(open_attr)
    } else {
        open_attr
    };
    w.push_line(details_marker_line(open, summary, ctx.details_interactive));
    if open && !children.is_empty() {
        render_bar_prefixed_body(w, doc, src, children, details_bar());
    }
    w.needs_newline = false;
    w.fresh_boundary = true;
}

/// Shared body-rendering half of `render_alert_from_model`/`render_details_from_model`: renders
/// `children` into a **fresh, independent** sub-`Writer` — mirroring production's own
/// `render_md_body_nested`, which computes an alert's/`Details`'s own body in complete isolation
/// (a fresh `Vec<Line>`, a fresh `tui_markdown::TextWriter` underneath it) before splicing a bar
/// prefix onto every resulting line and appending the result to the *caller's* own output — see
/// `render_alert`/`render_details`'s own bodies in `markdown.rs` for the exact shape this mirrors:
/// `let mut body_lines = Vec::new(); render_md_body_nested(&mut body_lines, .., &inner_ctx); for bl
/// in body_lines { let mut spans = vec![bar()]; spans.extend(bl.spans); out.push(..) }`.
///
/// The sub-`Writer` starts with every piece of *positional* state reset (`lines`/`list_indices`/
/// `line_prefixes`/`line_styles`/`needs_newline`/`pending_para_start`/`after_math`, all fresh or
/// `false`) — a body is a standalone construct, not a continuation of whatever came before the
/// alert/`<details>` — but **inherits** `w`'s own `styles`/`code`/`theme`/`icons` (display
/// configuration, not position) and renders at `w.width - 2` (the same two columns
/// `render_alert`/`render_details` reserve for the bar). `math: None`: production never lifts math
/// inside either body (`structure_mask`'s own exclusion — the same reason `render_quote` forces
/// `math_here: false` for a plain quote's own children), so there is nothing to share across the
/// isolation boundary; `BlockCtx { math_here: false, details_interactive: false }` is what `children`
/// actually renders with, unconditionally, regardless of what was in scope at the call site — see
/// `render_details_from_model`'s own doc comment for why *this* block's own interactivity is a
/// separate question from its children's.
///
/// Once the sub-`Writer` finishes, its own resulting lines are run through
/// `super::decorate_headings_and_extras` — **before** `bar` gets prepended to any of them — the
/// identical `decorate_md_lines` call `render_text_block_safe` (in `markdown.rs`) already makes
/// for *every* text segment, including one that turns out to be an alert's/`<details>`'s own body:
/// `render_md_body_nested` is only ever reached from inside `render_alert`/`render_details`, and it
/// still funnels a plain-text run through `render_md_text_inner` → `render_text_block_safe`, which
/// decorates that segment's own lines *before* `render_alert`/`render_details`'s own caller-side
/// loop ever prepends the bar to them. Task-checkbox recognition specifically (`replace_task_checkbox`
/// → `task_prefix_state`) only ever matches a line whose *own, trimmed* text starts with a bullet —
/// a `▌ `/`▏ ` bar prepended first would permanently hide every checkbox inside an alert/`<details>`
/// body from that check (confirmed directly: without this call here, `render_doc`'s own single,
/// top-level `decorate_headings_and_extras` pass — which only ever runs *after* every bar is already
/// in place — never converts one). Heading decoration is not at risk of running twice because of
/// this *extra* call: `decorate_headings`'s own `heading_level` reads `line.spans.first()` and
/// requires no leading span at all (see the module doc comment's own note on this), so the *later*,
/// outer pass (`render_doc`'s own, over the *already* bar-prefixed `w.lines`) simply never matches
/// these lines a second time — this call is the only chance heading decoration (or task-checkbox
/// conversion) ever gets for content reached this way, exactly mirroring why `render_alert`/
/// `render_details` decorate their own body *before* bar-prefixing it too.
///
/// Each (now fully decorated) line then gets `bar` prepended onto its spans (its own style preserved
/// via `Line::from(spans).style(bl.style)`, exactly like production's own loop) and is pushed into
/// the **outer**, shared `w` via `w.push_line` — not a direct `w.lines.push` — so a nested
/// alert/`<details>` (this function called recursively, through `render_block`'s own dispatch, from
/// *inside* another alert's/`<details>`'s own body) gets its own bar layered on top of the inner
/// one's the same way a real, doubly-nested `> >` quote gets two `>` spans (production's own
/// recursive `render_alert`/`render_details` produce the identical layering, one bar prepended per
/// nesting level, from the inside out) — and so that an alert/`<details>` nested inside a **plain**
/// `Quote` still gets that quote's own `>` prefix applied on top, via the outer `w`'s own
/// `line_prefixes`.
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
        list_indices: Vec::new(),
        line_prefixes: Vec::new(),
        line_styles: Vec::new(),
        needs_newline: false,
        math: None,
        after_math: false,
        fresh_boundary: false,
        pending_para_start: None,
        code: w.code,
        theme: w.theme.clone(),
        width: w.width.saturating_sub(2),
        icons: w.icons,
        tasks: w.tasks,
    };
    let body_ctx = BlockCtx {
        math_here: false,
        details_interactive: false,
    };
    for child in children {
        render_block(child, &mut inner, doc, src, body_ctx);
    }
    let decorated =
        decorate_headings_and_extras(inner.lines, inner.width, inner.icons, inner.tasks);
    for line in decorated {
        let style = line.style;
        let mut spans = vec![bar.clone()];
        spans.extend(line.spans);
        w.push_line(Line::from(spans).style(style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `#[cfg(test)]`-local, not added to the file's own top-level `use super::{..}` import: neither
    // is read outside `decorate_code_blocks_is_idempotent_on_render_docs_own_output`, and a plain,
    // non-test build already never calls `render_doc` at all (see the module doc comment) — importing
    // them unconditionally would just be an `unused_imports` warning waiting to happen there.
    use super::super::{decorate_code_blocks, is_code_header_span};
    // `Span::dim()` (the `Stylize` trait) — needed by the two math regression tests below to build
    // the exact same dimmed placeholder `math_raw_lines` itself constructs (see that function's own
    // `Span::from(text).dim()`), for a byte-for-byte `Line` equality check against `render_doc`'s
    // own output.
    use ratatui::style::Stylize;

    fn fresh_writer() -> Writer<'static> {
        Writer {
            lines: Vec::new(),
            inline_styles: Vec::new(),
            link: None,
            styles: KonomaStyles::default(),
            list_indices: Vec::new(),
            line_prefixes: Vec::new(),
            line_styles: Vec::new(),
            needs_newline: false,
            math: None,
            after_math: false,
            fresh_boundary: false,
            pending_para_start: None,
            code: CodeStyle::default(),
            theme: String::new(),
            width: 80,
            icons: false,
            tasks: super::super::DEFAULT_TASK_STATES,
        }
    }

    /// `BlockCtx` with math off and `Details` interactive — the shape a fresh top-level render
    /// starts with (see `render_doc`'s own construction of `ctx`); most of this module's own tests
    /// have no math or `<details>` case to exercise either flag's *other* value, so this is the
    /// default every test below reaches for unless a test's own doc comment says otherwise.
    fn fresh_ctx() -> BlockCtx {
        BlockCtx {
            math_here: false,
            details_interactive: true,
        }
    }

    /// Regression for the `line.spans.insert(1, marker_span)` panic
    /// ("insertion index (is 1) should be <= len (is 0)") — a fresh, *empty* line (`spans.len() ==
    /// 0`) is exactly the shape `render_item` leaves behind for a **loose** item's own task marker
    /// (see `task_list_marker`'s own doc comment): `spans.first_mut()` returns `None`, so the
    /// early-return splice never fires, and a fixed `insert(1, ..)` used to panic against the empty
    /// `Vec` — confirmed to actually happen (both directly on `Writer`, and reached through
    /// `render_item` on `task_corpus`'s own "a real task at a list item's own indentation is still a
    /// real task" case, `"- [ ] outer\n\n  - [ ] nested at matching indent\n"`) before this fix; see
    /// `task_list_marker`'s own doc comment for why `spans.len().min(1)` is the constructive fix
    /// rather than a special-cased early return. Before the fix, this test itself panicked with
    /// exactly that message.
    #[test]
    fn task_list_marker_on_a_fresh_empty_line_does_not_panic() {
        let mut w = fresh_writer();
        w.lines.push(Line::default());
        w.task_list_marker(false);
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
    fn task_list_marker_on_a_line_with_one_existing_span_inserts_after_it() {
        let mut w = fresh_writer();
        w.lines.push(Line::from(vec![Span::raw("1. ")]));
        w.task_list_marker(true);
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
            contains_unsupported(&doc.blocks[0].children).is_none(),
            "the LooseTask exclusion should be gone"
        );
        let mut w = fresh_writer();
        w.list_indices.push(None);
        let BlockKind::List { .. } = &doc.blocks[0].kind else {
            panic!("expected a top-level list");
        };
        let BlockKind::ListItem { task } = &doc.blocks[0].children[0].kind else {
            panic!("expected the outer list item");
        };
        render_item(
            &mut w,
            &doc,
            src,
            *task,
            &doc.blocks[0].children[0].children,
            fresh_ctx(),
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

    /// Structural proof (not merely an assumption) that `decorate_md_lines`'s split into
    /// `decorate_code_blocks` + `decorate_headings_and_extras` (see that function's own doc comment
    /// in the parent module) is safe for `render_doc` to skip the first half of: running
    /// `decorate_code_blocks` a *second* time over `render_doc`'s own already-fully-decorated output
    /// changes nothing at all. Exercises every position `render_code_block` covers in one document —
    /// top level, inside a list item, inside a quote, and an indented (non-fenced) block — plus a
    /// heading and a plain paragraph as neighbors, so a regression that made *any* of
    /// `render_code_block`'s own output lines start matching `is_code_block_line`'s `fg(White)`
    /// signal (the one thing that would make a second pass over them do anything at all — see
    /// `decorate_code_blocks`'s own doc comment) would show up here as a genuine diff, not silently
    /// pass because the corpus happened not to exercise that shape.
    #[test]
    fn decorate_code_blocks_is_idempotent_on_render_docs_own_output() {
        let src = "# Heading\n\npara text\n\n```rust\nfn a() {}\n```\n\n\
                    - item\n\n  ```sh\n  echo hi\n  ```\n\n\
                    > quoted\n>\n> ```\n> quoted code\n> ```\n\n    indented one\n    indented two\n";
        let doc = Doc::parse(src);
        let code = CodeStyle {
            bg: Some(Color::Rgb(43, 48, 59)),
            label_bg: Some(Color::Rgb(70, 78, 99)),
            label_right: true,
            tab_width: 4,
            wrap: true,
        };
        let math_slot = |_: &str, _: bool| MathSlot::Raw;
        let out = render_doc(
            &doc,
            src,
            80,
            code,
            "TwoDark",
            false,
            &[' ', 'x'],
            &math_slot,
            true,
        );
        assert!(
            out.unsupported.is_empty(),
            "fixture should be fully supported: {:?}",
            out.unsupported
        );
        // Sanity: the fixture really does exercise `render_code_block` more than once (one header
        // per code block — a run over an *empty* document would trivially "pass" this test for the
        // wrong reason).
        let header_count = out
            .lines
            .iter()
            .filter(|l| l.spans.iter().any(is_code_header_span))
            .count();
        assert_eq!(
            header_count, 4,
            "fixture doesn't exercise all four render_code_block positions: {:?}",
            out.lines
        );
        let redecorated = decorate_code_blocks(out.lines.clone(), 80, code, "TwoDark");
        assert_eq!(
            out.lines, redecorated,
            "a second decorate_code_blocks pass changed render_doc's own output — \
             render_code_block's own output lines must never match is_code_block_line"
        );
    }

    /// Regression for `Writer::pending_para_start`'s own bug (see that field's own doc comment): a
    /// paragraph whose own first rendered content is a lifted math expression must render to *exactly*
    /// its own placeholder line(s) — never preceded by an orphaned, permanently-empty blank row that
    /// nothing ever fills (the old, eager `render_paragraph_math` unconditionally opened one, assuming
    /// the paragraph's own first content would always reach `w.lines` via `push_span`, which a lift
    /// never does — see `render_math_slot`'s own `self.lines.extend(..)`).
    ///
    /// Two shapes, both confirmed directly against the real production pipeline
    /// (`md_render_diff_tests`'s own `markdown_render_diff_report`, before this fix, listed all three
    /// `code_span_corpus` math cases in `KNOWN_MISMATCHES` for exactly this reason):
    ///
    /// * Math as the *whole* document — nothing precedes it, so `w.needs_newline` is `false` on entry
    ///   and only the eager code's own *unconditional* second push would have fired.
    /// * Math as a paragraph immediately following an indented code block (`code_span_corpus`'s own
    ///   `"display math ($$...$$) inside an indented code block and outside"` case, `"    $$x$$\n\n$$y$$\n"`)
    ///   — `w.needs_newline` is `true` on entry here, so the eager code's *conditional* leading push
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
    /// from. Confirmed directly against the real production pipeline before this fix
    /// (`md_render_diff_tests`'s own two backslash-math `KNOWN_MISMATCHES` entries).
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
    /// literal string), every body line carrying the alert's own colored bar. Deliberately checked
    /// here, on `render_doc`'s own output, rather than only through `md_render_diff_tests`'s parity
    /// harness: `alert_header_line`/`alert_bar` are *shared* with `markdown.rs`'s own `render_alert`
    /// (see `render_alert_from_model`'s own doc comment for why), so a wrong color/icon/label/bar
    /// there would break both renderers identically and the parity harness would keep reporting a
    /// match — this test is what actually exercises this file's own call site.
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
}
