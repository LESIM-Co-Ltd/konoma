//! A renderer built directly on the block model (`crate::preview::markdown::model`), rather than on
//! tui-markdown's own rendered output. **Not wired into any preview path yet** — the only caller is
//! the diff harness in `src/app/md_render_diff_tests.rs`, which measures how closely this renderer's
//! output matches the production pipeline's (`render_markdown_with_images`) for the block kinds it
//! currently covers, over the full parity corpus. See that module's own doc comment for the numbers
//! from the last run.
//!
//! ## Scope (this pass)
//!
//! `Heading`, `Paragraph`, `ThematicBreak`, `List`/`ListItem`, and `Quote{alert: None}` — plus their
//! inline content: emphasis, strong, strikethrough, inline code, links, images (alt text only),
//! soft/hard breaks, and super/subscript. `Quote{alert: Some(_)}` (a GitHub alert — konoma's own
//! `render_alert`, not tui-markdown's, draws those; see `render_markdown_tasks_opts`'s own `alerts`
//! branch) and every remaining `BlockKind` a `Doc` can contain (`CodeBlock`/`Table`/`Html`) are
//! recorded in `RenderOut::unsupported` and produce **no output at all** — not even their own inline
//! content, if a block this pass *does* cover happens to nest one of them (a fenced code block three
//! levels down inside a list item, say). Rendering only the *rest* of that container, working around
//! the one nested piece it cannot draw, would produce something the production renderer never draws
//! (a list with a hole in the middle of it), and the diff harness would have no way to tell that
//! apart from an actual match — so `contains_unsupported` walks a `List`'s or `Quote`'s *entire*
//! subtree before rendering a single line of it, and the whole thing is reported unsupported (under
//! whichever nested kind's name it actually found) the moment any of those three turns up anywhere
//! inside it, at any depth. Reporting the container as unsupported and skipping the whole subtree is
//! the honest choice for a skeleton stage; `render_doc` therefore only ever draws lines for a
//! `List`/`Quote` whose entire contents this pass already knows how to render.
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

use super::model::{Block, BlockKind, Doc, Task};
use super::{
    image_loading_line, math_placeholder_lines, math_raw_lines, math_url, scan_inline_math,
    CodeStyle, ImagePlacement, KonomaStyles, MathPart, MathSlot, SourceRun,
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
/// support means this file must be able to tell a **tight** list item's own first paragraph (no
/// wrapping `Tag::Paragraph` at all; `model::collect_stray_inline_run`'s synthetic node) apart from a
/// **loose** one's (a real `Tag::Paragraph`, which changes how `render_paragraph`'s own unconditional
/// blank-line push interacts with `Writer::task_list_marker`'s placement — see `render_item`'s own doc
/// comment): the model already draws that exact line, in `Block.src`'s own trailing byte, for anything
/// computed from raw `pulldown_cmark::Event` ranges to read back (see `model::collect_stray_inline_run`'s
/// own doc comment, and its home test,
/// `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one`) —
/// `is_real_first_paragraph` is exactly that read, not a second parser judgment of any kind.
/// `width`/`theme`/`icons`/`tasks` are
/// threaded straight through to `super::decorate_md_lines`, matching `render_markdown_with_images`'s
/// own call to it via `render_text_block_safe`; `theme` is not yet read by anything this pass renders
/// (no code blocks reach this file at all — see the module doc comment on scope), kept for signature
/// parity with `render_markdown_with_images` so a later stage that adds `CodeBlock`/`Table` support
/// does not have to change every call site's argument list again. `math_slot`/`math_on` are the same
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
    // nothing below needs to keep re-deriving `w.math.is_some()`.
    let math_here = math.is_some();
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
                render_paragraph_dispatch(&mut w, doc, src, inline.clone(), math_here);
            }
            BlockKind::ThematicBreak => render_rule(&mut w),
            BlockKind::List { start, .. } => match contains_unsupported(&block.children) {
                Some(bad) => unsupported.push(bad),
                None => render_list(&mut w, doc, src, *start, &block.children, math_here),
            },
            BlockKind::Quote { alert: None } => match contains_unsupported(&block.children) {
                Some(bad) => unsupported.push(bad),
                None => render_quote(&mut w, doc, src, &block.children),
            },
            // GitHub alerts (`> [!NOTE]` …) are konoma's own `render_alert`, not tui-markdown's — see
            // the module doc comment. Left unsupported rather than drawn as a plain quote: an alert's
            // colored callout box is a wholly different shape from anything `Writer` produces, so
            // rendering the alert's body as an ordinary blockquote here would be a *third*, silently
            // wrong rendering, not a faithful stand-in for either side.
            BlockKind::Quote { alert: Some(_) } => unsupported.push("Quote"),
            // Exhaustive on purpose (matching this crate's own convention — see e.g.
            // `md_snapshot_tests::fmt_item_kind`'s doc comment): a new `BlockKind` variant must be
            // taught to this match rather than silently falling through as "unsupported" under a
            // name nobody chose on purpose.
            BlockKind::CodeBlock { .. } => unsupported.push("CodeBlock"),
            // Never reachable at the top level — a `ListItem` only ever exists as a `List`'s own
            // child (see `model::BlockKind::ListItem`'s doc comment), and `render_list` unwraps every
            // one of those itself before this loop ever sees it. Matched defensively all the same,
            // rather than assumed unreachable — see `render_block`'s own doc comment for why that
            // convention holds throughout this file.
            BlockKind::ListItem { .. } => unsupported.push("ListItem"),
            BlockKind::Table { .. } => unsupported.push("Table"),
            BlockKind::Html { .. } => unsupported.push("Html"),
        }
    }
    let lines = super::decorate_md_lines(w.lines, width, code, theme, icons, tasks);
    let images = w.math.map(|m| m.images).unwrap_or_default();
    RenderOut {
        lines,
        images,
        unsupported,
    }
}

/// Whether `blocks` — a `List`'s own item children, a `Quote`'s own body, or (recursively) either of
/// those nested arbitrarily deep inside more `List`/`Quote` nesting — contains, anywhere inside it, a
/// block kind this pass does not render at all: a fenced or indented `CodeBlock`, a `Table`, an
/// `Html` block, or a GitHub-alert `Quote` (`alert: Some(_)`; see `render_doc`'s own `Quote` arm for
/// why that one specifically is out of scope here too, not merely deferred like the other three).
/// Returns the *first* such kind's name found, in document order, for the caller (`render_doc`'s own
/// top-level dispatch, or a nested recursive call from this same function walking back up) to record
/// in `RenderOut::unsupported` — the exact same reporting a *directly* unsupported top-level block
/// already gets, just discovered one or more levels deeper. This is what keeps `render_doc`'s own
/// "report the container as unsupported and skip the whole subtree" choice (see the module doc
/// comment) honest for `List`/`Quote`: a three-item list with a fenced code block buried in the third
/// item's own nested sublist is not partially rendered with a hole where that block would have gone —
/// the whole list, all three items, is skipped, called out under whichever nested kind's own name
/// `contains_unsupported` actually found, exactly like a bare top-level `CodeBlock` always has been.
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
/// A leaf kind with no `Block::children` of its own (`Heading`/`Paragraph`/`ThematicBreak`, and the
/// three `BlockKind` variants this function itself is checking for) never recurses — there is nothing
/// further to walk under any of them (see `model::Block.children`'s own doc comment: "Empty for every
/// leaf kind").
fn contains_unsupported(blocks: &[Block]) -> Option<&'static str> {
    for b in blocks {
        match &b.kind {
            BlockKind::CodeBlock { .. } => return Some("CodeBlock"),
            BlockKind::Table { .. } => return Some("Table"),
            BlockKind::Html { .. } => return Some("Html"),
            BlockKind::Quote { alert: Some(_) } => return Some("Quote"),
            BlockKind::Heading { .. } | BlockKind::Paragraph { .. } | BlockKind::ThematicBreak => {}
            BlockKind::ListItem { .. }
            | BlockKind::List { .. }
            | BlockKind::Quote { alert: None } => {
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
}

impl<'m> Writer<'m> {
    fn cur_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
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
fn render_paragraph_dispatch(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    inline: Range<usize>,
    math_here: bool,
) {
    if math_here && w.math.is_some() {
        render_paragraph_math(w, doc, src, inline);
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
fn render_paragraph_math(w: &mut Writer<'_>, doc: &Doc<'_>, src: &str, inline: Range<usize>) {
    if w.needs_newline {
        w.push_blank_line();
    }
    w.push_blank_line();
    w.needs_newline = false;
    walk_inline_math(&mut events_iter_ranged(&doc.events[inline]), w, src);
    // A paragraph whose own trailing content was math (nothing textual after the last lift) leaves
    // `needs_newline == false`, matching a *fresh* `Writer`'s own start state — see this function's
    // own doc comment. Only a paragraph that ends in real text (even text *following* an earlier
    // lift) gets the normal `end_paragraph()`-equivalent `true`.
    if !w.after_math {
        w.needs_newline = true;
    }
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
fn walk_inline_math<'a>(
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
    w: &mut Writer<'_>,
    src: &str,
) {
    let mut prev_end: Option<usize> = None;
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

/// If the last thing rendered was a lifted math expression (`w.after_math`), opens a fresh
/// paragraph-start boundary (`render_paragraph`'s own two-part blank-line push, verbatim) before
/// whatever comes next — mirrors production's own "every fragment following a lift is its own
/// brand-new `TextWriter`" behavior (see `render_paragraph_math`'s own doc comment) without actually
/// re-parsing: the *boundary*, not a second parse, is the only thing that needs reproducing. A no-op
/// whenever `after_math` is already `false` (the overwhelmingly common case — most events in most
/// paragraphs have no math anywhere near them), so every call site in `walk_inline_math`'s own
/// dispatch loop can call this unconditionally, ahead of its own normal handling, rather than each
/// having to re-derive "is this event adjacent to a lift" itself. The explicit `w.after_math = false`
/// at the end is redundant with `push_blank_line`'s own side effect (`Writer::push_line` clears it
/// unconditionally — see that struct's own doc comment) but kept anyway: this function's own contract
/// ("the flag is consumed") should hold by its own reading, not merely as an accident of what its
/// callee happens to also do.
fn ensure_fresh_after_math(w: &mut Writer<'_>) {
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
fn render_math_slot(w: &mut Writer<'_>, latex: &str, display: bool) {
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

/// The generic block dispatcher used for every position in this file's block tree that is **not** a
/// `List`'s own item sequence (which `render_list` walks itself, since each one needs a marker built
/// first — see `render_item`) or an item's own *first* child (which `render_item` special-cases for
/// exactly the tight/loose distinction just described): a `Quote`'s own body, and every child of a
/// list item after the first. Mirrors `TextWriter::start_tag`'s own top-level dispatch for the block
/// kinds this pass covers.
///
/// `Quote{alert: Some(_)}`/`CodeBlock`/`Table`/`Html` never legitimately reach this function: every
/// caller that walks into a `List`'s or `Quote`'s own children only does so after `contains_unsupported`
/// has already confirmed none of those four appear anywhere in that subtree (see `render_doc`'s own
/// dispatch, and that function's own doc comment) — so encountering one here would mean a bug
/// upstream of this file, not merely input a smaller local check failed to reject. `ListItem` never
/// legitimately reaches this function either — it only ever exists as a `List`'s own child (see
/// `model::BlockKind::ListItem`'s doc comment), and `render_list` unwraps every one of those itself.
/// Matched defensively (silently rendering nothing further) rather than assumed unreachable all the
/// same, so a bug elsewhere degrades to "this one block goes missing" instead of a panic — principle
/// #3 (`CLAUDE.md`).
///
/// `math_here` is simply threaded through to whichever of `render_paragraph_dispatch`/`render_list`
/// this block turns out to need it — see `render_doc`'s own doc comment for what it means and
/// `render_quote`'s own call site below for the one place a caller of this function does *not* pass
/// its own value through unchanged (a `Quote`'s own children never get it, regardless of what this
/// function itself received).
fn render_block(block: &Block, w: &mut Writer<'_>, doc: &Doc<'_>, src: &str, math_here: bool) {
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
            render_paragraph_dispatch(w, doc, src, inline.clone(), math_here);
        }
        BlockKind::ThematicBreak => render_rule(w),
        BlockKind::List { start, .. } => {
            render_list(w, doc, src, *start, &block.children, math_here)
        }
        BlockKind::Quote { alert: None } => render_quote(w, doc, src, &block.children),
        BlockKind::Quote { alert: Some(_) }
        | BlockKind::CodeBlock { .. }
        | BlockKind::Table { .. }
        | BlockKind::Html { .. }
        | BlockKind::ListItem { .. } => {}
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
    math_here: bool,
) {
    if w.list_indices.is_empty() && w.needs_newline {
        w.push_blank_line();
    }
    w.list_indices.push(start);
    for item in items {
        if let BlockKind::ListItem { task } = &item.kind {
            render_item(w, doc, src, *task, &item.children, math_here);
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
/// any other paragraph in the document. `is_real_first_paragraph` reads `src` to tell the two apart
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
/// every child after it; `is_real_first_paragraph` reports `false` for a non-`Paragraph` block, so no
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
fn render_item(
    w: &mut Writer<'_>,
    doc: &Doc<'_>,
    src: &str,
    task: Option<Task>,
    children: &[Block],
    math_here: bool,
) {
    // start_item(): a fresh physical line, unconditionally, carrying the item's own marker.
    w.push_line(Line::default());
    push_item_marker(w);
    w.needs_newline = false;

    let loose_first = children
        .first()
        .is_some_and(|b| is_real_first_paragraph(src, b));
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
            if math_here {
                walk_inline_math(&mut events_iter_ranged(&doc.events[inline.clone()]), w, src);
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
            render_block(first, w, doc, src, math_here);
        }
        rest = &children[1..];
    }
    for child in rest {
        render_block(child, w, doc, src, math_here);
    }
}

/// Whether `block` is a **real** `Paragraph` — pulldown-cmark's own `Tag::Paragraph`, reported for a
/// loose list item's own content — as opposed to the *synthetic* one `collect_stray_inline_run`
/// builds for a tight item's (see `render_item`'s own doc comment for what that distinction changes).
/// The two are told apart by `Block.src`'s own trailing byte in `src`: a real `Paragraph`'s range
/// always runs through its own trailing newline (`"a\n"`), the way pulldown-cmark reports every
/// `Tag::Paragraph`'s range; the synthetic one never does (`"a"`) — there is no wrapping tag for
/// pulldown-cmark to have assigned that wider range to in the first place, only the leaf events
/// themselves, whose own ranges stop at the run's last one. This is not a byte-range heuristic this
/// file invented: it is the model's own, explicitly documented and unit-tested contract for reading
/// the distinction back (see `model::collect_stray_inline_run`'s own doc comment, and
/// `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one`, the test that pins
/// exactly this).
fn is_real_first_paragraph(src: &str, block: &Block) -> bool {
    matches!(&block.kind, BlockKind::Paragraph { .. }) && src[block.src.clone()].ends_with('\n')
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

/// Renders a `Quote{alert: None}` block's own body into `w`, in place — mirrors
/// `TextWriter::start_blockquote`/`end_blockquote` wrapped around a normal walk of the quote's own
/// children (`render_block`, the same generic dispatcher a list item's non-first children go
/// through — a quote's own body can contain any of `Heading`/`Paragraph`/`ThematicBreak`/`List`/
/// a further nested `Quote`, all of which get the same `>`-prefix treatment via `Writer::push_line`;
/// see this module's own `Writer` doc comment). A nested `Quote` reached this way (`>>`) simply
/// recurses into this same function again, pushing a *second* `>` onto `w.line_prefixes`.
///
/// Always calls `render_block` with `math_here: false`, regardless of whatever value was in scope for
/// *this* quote — production never lifts math out of a blockquote's own text at all (`structure_mask`'s
/// own exclusion; see `render_paragraph_math`'s own doc comment), so nothing this function dispatches
/// to, however many more `List`s or nested `Quote`s that subtree goes on to contain, should ever end
/// up scanning for it either. Because of that, `w.after_math` can never legitimately be `true` by the
/// time this function's own exit runs (nothing in its own subtree can have set it) — so, unlike
/// `render_list`'s own exit, this one stays the plain, unconditional `needs_newline = true` `TextWriter`
/// itself always uses for `end_blockquote`.
fn render_quote(w: &mut Writer<'_>, doc: &Doc<'_>, src: &str, children: &[Block]) {
    if w.needs_newline {
        w.push_blank_line();
        w.needs_newline = false;
    }
    w.line_prefixes.push(Span::raw(">"));
    w.line_styles.push(w.styles.blockquote());
    for child in children {
        render_block(child, w, doc, src, false);
    }
    w.line_prefixes.pop();
    w.line_styles.pop();
    w.needs_newline = true;
}

#[cfg(test)]
mod tests {
    use super::*;

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
            false,
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
}
