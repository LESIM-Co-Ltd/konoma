//! A renderer built directly on the block model (`crate::preview::markdown::model`), rather than on
//! tui-markdown's own rendered output. **Not wired into any preview path yet** — the only caller is
//! the diff harness in `src/app/md_render_diff_tests.rs`, which measures how closely this renderer's
//! output matches the production pipeline's (`render_markdown_with_images`) for the block kinds it
//! currently covers, over the full parity corpus. See that module's own doc comment for the numbers
//! from the last run.
//!
//! ## Scope (this pass)
//!
//! `Heading`, `Paragraph`, and `ThematicBreak` — plus their inline content: emphasis, strong,
//! strikethrough, inline code, links, images (alt text only), soft/hard breaks, and
//! super/subscript. Every other `BlockKind` a `Doc` can contain (`List`/`ListItem`/`Quote`/
//! `CodeBlock`/`Table`/`Html`) is recorded in `RenderOut::unsupported` and produces **no output at
//! all** — not even its own inline content, if it happens to nest one of the three kinds above (a
//! paragraph inside a list item, say). Rendering only that fragment, out of its enclosing
//! container's own layout (a list's indentation, a block quote's `>` prefix, ...), would draw
//! something the production renderer never draws, and the diff harness would have no way to tell
//! that apart from an actual match. Reporting the container as unsupported and skipping the whole
//! subtree is the honest choice for a skeleton stage; `render_doc` therefore only ever looks at
//! `doc.blocks` — the top level — and never recurses into a block's own `children`.
//!
//! ## How inline content is rendered
//!
//! Not by calling into the `tui-markdown` crate at all — this file reimplements the handful of
//! `tui_markdown::TextWriter` methods that matter for the three block kinds above (see the pinned
//! `tui-markdown = "0.3.7"` dependency's own `src/lib.rs`, `impl<I, S> TextWriter<'_, I, S>`):
//! `text`/`code`/`soft_break`/`hard_break`, the `push_inline_style`/`pop_inline_style` stack for
//! `Emphasis`/`Strong`/`Strikethrough`/`Subscript`/`Superscript`, and `push_link`/`pop_link` for
//! `Link` — which appends `" ("` + the URL styled `KonomaStyles::link()` + `")"` right after the
//! label, the exact 4-span shape `app::md_text::collapse_links` looks for (see that function's own
//! doc comment: "Pattern: `[label]` `" ("` `[URL(blue underline)]` `")"`"). `Image` renders only its
//! alt text — tui-markdown's own `Tag::Image` arm is a no-op `warn!`, so the URL is dropped and
//! never appended anywhere, and nothing about the tag itself changes the writer's state; this file
//! mirrors that exactly (`start_inline_tag`'s `Tag::Image` arm).
//!
//! Each `Heading`/`Paragraph` block's own inline events come from **re-parsing that block's own
//! byte range alone**, with `tui_markdown_parse_options()` — the exact option set the `tui-markdown`
//! crate hardcodes internally (see `from_str_with_options`), so this walk sees exactly the events
//! tui-markdown itself would for the same text — rather than being threaded through from
//! `Doc::parse`'s own walk (`model::parse_blocks` intentionally never keeps a block's inline events;
//! see that module's own doc comment on scope). Re-parsing a *slice* rather than the whole document
//! has one known, accepted cost, documented rather than hidden: a reference-style link
//! (`[text][ref]`) whose definition lives in a *different* top-level block does not resolve in this
//! block-local re-parse the way it would in a whole-document parse — pulldown-cmark falls back to
//! literal bracket text. `md_render_diff_tests` reports this as an ordinary mismatch, not a special
//! case; see that module's own doc comment for the full list of mismatch categories expected at this
//! stage.
//!
//! Once a block's own raw lines are built, `super::decorate_md_lines` — the exact function
//! `render_text_block_safe` calls on tui-markdown's own output — turns the heading's leading `#`
//! span into KonomaStyles-decorated hierarchy (full-width rule under H1/H2) and the literal `"---"`
//! this file emits for a `ThematicBreak` into the real full-width rule line. Reused, not
//! reimplemented, for the same reason `Doc::parse` itself reuses `parse_alert_header`/
//! `details_open_tag`/... rather than re-deriving their judgments: a second, hand-written copy of
//! "what does a heading/rule look like once decorated" is exactly the kind of drift this whole
//! refactor exists to eliminate.
//!
//! `#![allow(dead_code)]`: like `model.rs` (see its own module doc comment), nothing outside
//! `md_render_diff_tests` (`#[cfg(test)]`, in `src/app/`) calls into this module yet — a plain,
//! non-test build has no path from any renderer to `render_doc` at all.
#![allow(dead_code)]

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::StyleSheet;

use super::model::{BlockKind, Doc};
use super::{CodeStyle, ImagePlacement, KonomaStyles};

/// What `render_doc` produced: the decorated lines (same shape `render_markdown_with_images`
/// returns), any inline images placed (always empty at this stage — see the module doc comment on
/// scope; a future stage that models block-level images would fill this in), and the name of every
/// top-level `BlockKind` this pass encountered but does not yet render.
pub(crate) struct RenderOut {
    pub lines: Vec<Line<'static>>,
    pub images: Vec<ImagePlacement>,
    pub unsupported: Vec<&'static str>,
}

/// Renders every top-level block in `doc` this file's own scope covers (`Heading`/`Paragraph`/
/// `ThematicBreak`), in source order, with a single blank line between any two consecutive rendered
/// blocks — matching `tui_markdown::TextWriter`'s own `needs_newline` bookkeeping exactly for a
/// document built *only* from those three kinds: `start_paragraph`/`start_heading`/`rule` all push a
/// blank line first exactly when `needs_newline` is true, and every one of the three sets
/// `needs_newline = true` on its own way out — so, restricted to this file's scope, there is always
/// exactly one blank separator line between two consecutive blocks, never one before the first block
/// or after the last (`needs_newline` starts `false`). Every other block kind is recorded in
/// `unsupported`, in the order encountered, and contributes no lines and no separator at all.
///
/// `src` must be the exact string `doc` was parsed from (`Doc::parse(src)`) — every `Block::src`
/// range is sliced out of it directly. `width`/`theme`/`icons`/`tasks` are threaded straight through
/// to `super::decorate_md_lines`, matching `render_markdown_with_images`'s own call to it via
/// `render_text_block_safe`; `theme` and `tasks` are not yet read by anything *this* pass renders (no
/// code blocks, and no task-list item can be represented in a Heading/Paragraph/ThematicBreak-only
/// document — a task marker only ever exists inside a `ListItem`, which is unsupported here), kept
/// for signature parity with `render_markdown_with_images` so a later stage that adds `CodeBlock`/
/// `List` support does not have to change every call site's argument list again.
pub(crate) fn render_doc(
    src: &str,
    doc: &Doc,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) -> RenderOut {
    let styles = KonomaStyles { code_bg: code.bg };
    let mut raw: Vec<Line<'static>> = Vec::new();
    let mut unsupported: Vec<&'static str> = Vec::new();
    for block in &doc.blocks {
        match &block.kind {
            BlockKind::Heading { level } => {
                if !raw.is_empty() {
                    raw.push(Line::default());
                }
                raw.extend(render_heading(&src[block.src.clone()], *level, styles));
            }
            BlockKind::Paragraph => {
                if !raw.is_empty() {
                    raw.push(Line::default());
                }
                raw.extend(render_paragraph(&src[block.src.clone()], styles));
            }
            BlockKind::ThematicBreak => {
                if !raw.is_empty() {
                    raw.push(Line::default());
                }
                // The literal text tui-markdown's own `rule()` pushes (`Line::from("---")`) —
                // `decorate_extras` (called via `decorate_md_lines` below) recognizes exactly this
                // shape and replaces it with the real full-width rule.
                raw.push(Line::from("---"));
            }
            // Exhaustive on purpose (matching this crate's own convention — see e.g.
            // `md_snapshot_tests::fmt_item_kind`'s doc comment): a new `BlockKind` variant must be
            // taught to this match rather than silently falling through as "unsupported" under a
            // name nobody chose on purpose.
            BlockKind::CodeBlock { .. } => unsupported.push("CodeBlock"),
            BlockKind::List { .. } => unsupported.push("List"),
            BlockKind::ListItem { .. } => unsupported.push("ListItem"),
            BlockKind::Quote { .. } => unsupported.push("Quote"),
            BlockKind::Table { .. } => unsupported.push("Table"),
            BlockKind::Html { .. } => unsupported.push("Html"),
        }
    }
    let lines = super::decorate_md_lines(raw, width, code, theme, icons, tasks);
    RenderOut {
        lines,
        images: Vec::new(),
        unsupported,
    }
}

/// A minimal reimplementation of `tui_markdown::TextWriter`'s inline-event handling (see the module
/// doc comment), scoped to a single already-open block: `render_heading`/`render_paragraph` seed
/// `lines` with the block's own pre-existing content — a blank line for a paragraph
/// (`start_paragraph`'s own unconditional `push_line(Line::default())`), or the heading's own
/// hash-marker line (`start_heading`) — and every method below mirrors one `TextWriter` method
/// exactly, restricted to what can occur inside a `Heading`'s or `Paragraph`'s own inline content.
struct InlineWriter {
    lines: Vec<Line<'static>>,
    inline_styles: Vec<Style>,
    link: Option<String>,
    styles: KonomaStyles,
}

impl InlineWriter {
    fn cur_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
    }

    /// `TextWriter::push_span` (its `line_prefixes`/`line_styles` handling never applies here — see
    /// the module doc comment: those exist only for blockquote nesting, which is unsupported).
    fn push_span(&mut self, span: Span<'static>) {
        match self.lines.last_mut() {
            Some(l) => l.push_span(span),
            None => self.lines.push(Line::from(vec![span])),
        }
    }

    fn push_blank_line(&mut self) {
        self.lines.push(Line::default());
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
    /// line for every line after the first.
    fn text(&mut self, text: &str) {
        for (i, line) in text.lines().enumerate() {
            if i > 0 {
                self.push_blank_line();
            }
            let style = self.cur_style();
            self.push_span(Span::styled(line.to_string(), style));
        }
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
}

/// Walks `events` until (and consuming) the `Event::End` matching `stop`, dispatching every event a
/// `Heading`'s or `Paragraph`'s own inline content can contain to the matching `InlineWriter` method
/// — mirroring `TextWriter::handle_event`/`start_tag`/`end_tag` exactly for those events (see the
/// module doc comment). Recurses one level for every further inline `Start` it meets
/// (`start_inline_tag`), so nested constructs (bold inside a link's label, an image inside a link,
/// ...) balance correctly, the same way `model::skip_inline_to` balances the block model's own walk.
fn walk_inline<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    w: &mut InlineWriter,
    stop: TagEnd,
) {
    while let Some(ev) = events.next() {
        match ev {
            Event::End(e) if e == stop => return,
            Event::Start(tag) => start_inline_tag(events, w, tag),
            Event::Text(t) => w.text(&t),
            Event::Code(c) => w.code(&c),
            Event::SoftBreak => w.soft_break(),
            Event::HardBreak => w.hard_break(),
            // `Html`/`InlineHtml`/`FootnoteReference`/`InlineMath`/`DisplayMath`: tui-markdown's own
            // `handle_event` only ever `warn!`s and drops these, producing no output (see the crate
            // citation in the module doc comment) — matched here the same way, silently.
            // `TaskListMarker`/`Rule`/a mismatched `Event::End` cannot legitimately occur inside a
            // `Heading`'s or `Paragraph`'s own inline content at all (both this walk's re-parse and
            // the whole-document parse `Doc::parse` itself did use the same option set —
            // `tui_markdown_parse_options()` — so they see the same events for the same text).
            // Dropped the same defensive way rather than assumed unreachable, so a pulldown-cmark
            // edge case this file has not been tested against still degrades (principle #3) instead
            // of desyncing the walk.
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
    w: &mut InlineWriter,
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
            walk_inline(events, w, TagEnd::Link);
            w.pop_link();
        }
        Tag::Image { .. } => walk_inline(events, w, TagEnd::Image),
        other => skip_balanced(events, other.to_end()),
    }
}

/// `push_inline_style`, walk the tag's own body to `stop`, `pop_inline_style` — the shared shape of
/// every plain style-stacking inline tag (`Emphasis`/`Strong`/`Strikethrough`/`Subscript`/
/// `Superscript`).
fn wrap_styled<'a>(
    events: &mut impl Iterator<Item = Event<'a>>,
    w: &mut InlineWriter,
    stop: TagEnd,
    style: Style,
) {
    w.push_inline_style(style);
    walk_inline(events, w, stop);
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

/// Renders one `Heading` block's own raw lines (before `decorate_md_lines`): the hash-marker span
/// tui-markdown itself starts a heading line with — `decorate_headings`, downstream, recognizes and
/// strips exactly this shape (see `heading_level`) — the heading's own inline content, and, if the
/// source used `{#id ...}` heading-attribute syntax, the metadata suffix. `text` is the block's own
/// `src` slice (e.g. `"## Title\n"`); re-parsed here with `tui_markdown_parse_options()` rather than
/// threaded from `Doc::parse`'s own walk — see the module doc comment for why, and for the one
/// accepted gap (cross-block reference links) that choice costs. `model_level` is `Doc::parse`'s own
/// reading of this same heading (`BlockKind::Heading::level`), used only as a cross-check
/// (`debug_assert_eq!`) — the actual rendered level comes from this function's own re-parse, so a
/// mismatch (if this file's assumptions about re-parsing a slice are ever wrong for some input) is
/// caught in a debug build rather than silently rendering the wrong hierarchy.
fn render_heading(text: &str, model_level: u8, styles: KonomaStyles) -> Vec<Line<'static>> {
    let mut events = Parser::new_ext(text, super::tui_markdown_parse_options());
    let Some(Event::Start(Tag::Heading {
        level: parsed_level,
        id,
        classes,
        attrs,
    })) = events.next()
    else {
        // The model classified this block as a Heading from the exact same source slice, using an
        // option set (`model::parse_options`) that only ever *adds* `ENABLE_TABLES` on top of this
        // same base — see that function's own doc comment — which cannot change how a heading itself
        // parses. Re-parsing it alone should therefore always reproduce the same tag; degrading to no
        // output (principle #3) rather than panicking if that assumption is ever wrong for some input
        // this file has not been tested against. The diff harness reports it as a mismatch either way.
        return Vec::new();
    };
    debug_assert_eq!(
        parsed_level as u8, model_level,
        "re-parsing a Heading block's own src slice should reproduce the level Doc::parse itself saw"
    );
    let level = parsed_level as u8;
    let heading_style = styles.heading(level);
    let hashes = format!("{} ", "#".repeat(level as usize));
    let mut w = InlineWriter {
        lines: vec![Line::styled(hashes, heading_style)],
        inline_styles: Vec::new(),
        link: None,
        styles,
    };
    walk_inline(&mut events, &mut w, TagEnd::Heading(parsed_level));
    let meta = HeadingMeta {
        id: id.map(|c| c.into_string()),
        classes: classes.into_iter().map(|c| c.into_string()).collect(),
        attrs: attrs
            .into_iter()
            .map(|(k, v)| (k.into_string(), v.map(|v| v.into_string())))
            .collect(),
    };
    if let Some(suffix) = meta.to_suffix() {
        w.push_span(Span::styled(suffix, styles.heading_meta()));
    }
    w.lines
}

/// Renders one `Paragraph` block's own raw lines (before `decorate_md_lines`) — see `render_heading`
/// for the shared re-parse rationale and its accepted gap.
fn render_paragraph(text: &str, styles: KonomaStyles) -> Vec<Line<'static>> {
    let mut events = Parser::new_ext(text, super::tui_markdown_parse_options());
    if !matches!(events.next(), Some(Event::Start(Tag::Paragraph))) {
        return Vec::new(); // see render_heading's doc comment on this degrade
    }
    let mut w = InlineWriter {
        lines: vec![Line::default()],
        inline_styles: Vec::new(),
        link: None,
        styles,
    };
    walk_inline(&mut events, &mut w, TagEnd::Paragraph);
    w.lines
}
