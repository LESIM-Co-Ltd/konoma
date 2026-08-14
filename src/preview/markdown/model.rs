//! A structural block model of a preprocessed Markdown document, built by walking
//! `pulldown-cmark`'s own event stream **exactly once** — the first step of a planned rendering
//! refactor (`md-block-walk`). **This module is not wired into any *production* renderer yet**:
//! `render_*` in the parent module still derives block structure the way it always has (from
//! tui-markdown's *rendered output*, re-parsed by half a dozen ad-hoc line scanners —
//! `code_block_mask`, `structure_mask`, `code_block_source_locs`, `task_source_locs`, ...). Those
//! keep working exactly as before. `crate::preview::markdown::render` — this module's own sibling —
//! *is* built on `Doc`, and reads every leaf block's inline content straight out of the single event
//! stream `Doc::parse` records here (`Doc.events`), with no second parse of any kind; see that
//! module's own doc comment. That renderer is itself not wired into any *preview* path yet (see its
//! own module doc comment for the one caller it has today), so none of this changes any observable
//! behavior of the running app.
//!
//! ## Why this exists
//!
//! Every one of the "renderer vs. scanner disagree" bugs this file's neighbors document at length
//! (`parser_code_blocks`'s doc comment alone lists this as the *seventh* recurrence of the same
//! shape) comes from the same root cause: konoma currently has no single source of truth for "what
//! block is this text inside of". The render path infers it from styled output; the copy/toggle
//! scanners infer it by re-implementing CommonMark's own block rules by hand. `Doc`/`Block` here is
//! meant to become *that* single source of truth — built once, by asking the real parser, with
//! byte ranges precise enough that a future `y c` or checkbox-toggle can slice the input directly
//! instead of re-deriving "which block is the Nth one on screen" via a count-guard.
//!
//! "Built once" is not just a byte-range promise: `Doc::parse` calls `pulldown_cmark::Parser::new_ext`
//! exactly one time, over the *whole* document, and never re-parses any substring of it for any
//! reason — not for a block's own inline content, not for a tight list item's stray inline run,
//! nowhere. An earlier version of `render.rs` re-parsed each `Heading`/`Paragraph`'s own byte range
//! in isolation instead, which meant a reference-style link (`[text][ref]`) whose `[ref]: url`
//! definition lived in a different top-level block could never resolve (the definition and the
//! reference never shared a re-parsed slice) — see `Doc.events`'s own doc comment for how a leaf
//! block gets at its inline content without that cost today.
//!
//! ## What text this model expects
//!
//! `Doc::parse` takes whatever string it is given and walks it directly — it does **not** strip
//! front matter, renumber footnotes, or convert inline HTML tags (`<kbd>`, `<br>`, ...) itself.
//! Those are konoma's own source pre-passes (`strip_front_matter`, `process_footnotes`,
//! `process_inline_html`, all in the parent module), run once, in that order, before the render
//! pipeline ever reaches tui-markdown — the result is `MdCache::pre_src` in `App`. A caller that
//! wants this model to agree with what the screen shows must hand it that same preprocessed text,
//! not the raw file. `md_model_snapshot_tests` (in `src/app/`) does exactly this, via
//! `md_snapshot_tests::pre_src_for` — the same helper the rendering golden-snapshot test uses, so
//! the two can never quietly drift onto different inputs.
//!
//! ## Scope
//!
//! Block-level structure only — this file never builds an inline-content *tree* (no `Emphasis`/
//! `Link`/`Code` node types here at all). Inline content nested *inside* a `TableCell` (emphasis,
//! links, code spans, images, ...) is walked *past* and recorded nowhere — see `skip_inline_to`. A
//! `Heading`'s or `Paragraph`'s own inline content is the one place this model *does* keep something:
//! not a tree, just *where* that content sits in the single, whole-document event stream every block
//! here was parsed from (`Doc.events`) — an index range (`BlockKind::Heading.inline` /
//! `BlockKind::Paragraph.inline`), the same way `BlockKind::CodeBlock.body_spans` names a code
//! block's own content by *byte* range into `src` instead. Either way, a caller wanting the actual
//! content still has to walk raw `pulldown_cmark::Event`s itself (`render.rs`'s own `walk_inline`
//! does exactly this) — this model draws the boundary, not the picture.
//!
//! `mermaid`-tagged fences are represented as an ordinary `CodeBlock { lang: Some("mermaid"), .. }`,
//! exactly like any other fenced block; the parent module's own choice to render some of those as
//! diagrams instead of code is a rendering decision, and this is a structural model of the document,
//! not of the screen.
//!
//! One exception, not a violation of the above: a **tight** list item's own inline text, which
//! pulldown-cmark reports with no enclosing `Paragraph` at all (unlike a loose item's), is still
//! coalesced into a synthetic one-off `Paragraph` node (`collect_stray_inline_run`), rather than
//! silently dropped — see `BlockKind::ListItem`'s own doc comment for why leaving it unrepresented
//! was a real gap, not a scope choice: without *some* leaf block covering that text, there would be
//! nothing for a future renderer built on `Doc` to read the item's own content from at all. That
//! synthetic `Paragraph` gets an `inline` range the same way a real one does — see
//! `collect_stray_inline_run`'s own doc comment for exactly which events it spans.
//!
//! `#![allow(dead_code)]`: nothing outside this module's own `#[cfg(test)]` tests, the equally
//! `#[cfg(test)]`-gated `md_model_snapshot_tests` (in `src/app/`, a sibling module that reuses this
//! one's `pub(crate)` items — `parse_options`, `is_stray_inline_leaf`, `is_unmodeled_container_tag`
//! — for its own independent completeness cross-check), or `render.rs` (see above — itself unwired
//! into any *preview* path), calls into any of this yet — a plain, non-test build has no path from
//! any preview to `Doc::parse`, so every item here is legitimately dead code in that configuration.
//! Removing this allow is part of whichever future change actually wires a preview caller in.
#![allow(dead_code)]

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A parsed document: its top-level blocks, in source order, plus the single event stream
/// `Doc::parse` walked to build them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Doc<'a> {
    /// The parser's own event stream, in source order, exactly as `Doc::parse`'s one walk consumed
    /// it (see `Walker::next`) — every `Event`/byte-range pair pulldown-cmark reported for the
    /// *whole* document, not merely the ones some `Block` below happens to reference. A leaf block
    /// whose own content is purely inline (`BlockKind::Heading`/`BlockKind::Paragraph`) names its
    /// own slice of this by *index* range (`inline`) rather than holding a copy of it, the same way
    /// `BlockKind::CodeBlock` names its content by *byte* range (`body_spans`) into `src` instead of
    /// copying it — `doc.events[block_inline_range]` is exactly the slice a renderer needs, with no
    /// second parse of any kind (see the module doc comment's "Why this exists" section for why that
    /// matters: a per-block re-parse cannot resolve a reference-style link whose definition lives in
    /// a different block, among other things a whole-document parse gets right "for free").
    pub events: Vec<(Event<'a>, Range<usize>)>,
    pub blocks: Vec<Block>,
}

/// One block-level construct and its extent in the source `Doc::parse` was given.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Block {
    pub kind: BlockKind,
    /// Byte range of the **entire** block in the input string — for a fenced code block this
    /// includes the fence lines and the info string; for a list item it includes the marker and
    /// every line the item owns, nested content included. Always a `pulldown-cmark`-reported
    /// range, so its endpoints are guaranteed to land on UTF-8 char boundaries.
    pub src: Range<usize>,
    /// Nested blocks (a list's items, an item's own content, a quote's body). Empty for every
    /// leaf kind (`Heading`, `Paragraph`, `CodeBlock`, `Table`, `Html`, `ThematicBreak`).
    pub children: Vec<Block>,
}

/// What kind of block-level construct a [`Block`] is, and the kind-specific data attached to it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BlockKind {
    Heading {
        level: u8,
        /// Index range into `Doc.events` for this heading's own inline content: every event
        /// strictly *between* its `Start(Heading)` and matching `End(Heading)`, in source order,
        /// with **neither** endpoint included — there is nothing left in this slice for a reader to
        /// skip past; it is exactly what a fresh inline walk of this heading's own text would
        /// report (see `render.rs`'s `render_heading`, the one caller today). Always contained in
        /// `0..Doc.events.len()` and never decreasing; empty for a heading with no inline content at
        /// all. Computed by `parse_container`'s own `Tag::Heading` arm — see that call site for the
        /// exact bookkeeping (`Walker::pos`, read once before and once after the content is
        /// consumed).
        inline: Range<usize>,
        /// `{#id .class key=value}` heading-attribute syntax
        /// (`Options::ENABLE_HEADING_ATTRIBUTES`), read straight off `Tag::Heading`'s own fields and
        /// converted to owned `String`s so `BlockKind` itself never needs `Doc`'s own lifetime —
        /// `None`/empty exactly when the source used none, matching pulldown-cmark's own convention.
        id: Option<String>,
        classes: Vec<String>,
        /// The first item of each tuple is the attribute name, the second (if present) its value —
        /// matching `Tag::Heading.attrs`'s own shape exactly.
        attrs: Vec<(String, Option<String>)>,
    },
    /// A paragraph of inline content — real (pulldown-cmark reports one for a block quote's own
    /// text, a *loose* list item's, ...) or synthetic (`collect_stray_inline_run`'s one-off node for
    /// a *tight* list item's own inline text, which pulldown-cmark reports with no enclosing
    /// `Paragraph` at all — see that function's own doc comment, and `BlockKind::ListItem`'s).
    Paragraph {
        /// Same convention as `Heading.inline` above for a real paragraph — the events strictly
        /// between its `Start`/matching `End`, excluding both, and (for a loose list item's *first*
        /// paragraph specifically) excluding its own leading `Event::TaskListMarker` too, if it has
        /// one (see `parse_item_task_and_children`'s own doc comment for why that marker is
        /// recorded on `ListItem.task` instead, and never inside any paragraph's own `inline` range).
        /// For the synthetic case, exactly the events `collect_stray_inline_run` itself consumed —
        /// there is no wrapping `Start`/`End` pair to exclude in the first place, since
        /// pulldown-cmark never reported one.
        inline: Range<usize>,
    },
    /// A fenced (```` ``` ```` / `~~~`) or indented (4+ column) code block.
    CodeBlock {
        /// The fence's info string verbatim (e.g. `"rust"`, or `"rust {.line-numbers}"`), matching
        /// the render pipeline's own convention of using the *whole* trimmed info string as the
        /// language label rather than splitting it at whitespace (see `flush_code_run`'s `lang`).
        /// Always `None` for an indented block (CommonMark gives those no info string at all).
        lang: Option<String>,
        fenced: bool,
        /// Byte ranges of the block's **content only**, one per `Event::Text` pulldown-cmark
        /// reports between this block's `Start`/`End(CodeBlock)`, in source order — the fence
        /// lines, the info string, and any leading indentation the block's own container (a list
        /// item, a block quote) imposes on every line are never part of any of these ranges,
        /// because pulldown-cmark never reports them as text in the first place.
        ///
        /// ## Why a list of ranges, not one
        ///
        /// A single contiguous `Range<usize>` cannot represent a code block's content exactly
        /// whenever pulldown-cmark's own text excludes bytes that sit *between* two `Event::Text`
        /// spans — a continuation line's own share of a list item's or block quote's baseline
        /// indentation; a literal `\r` consumed by line-ending normalization. Those bytes still lie
        /// inside `first_text.start..last_text.end`, so a single range spanning that whole interval
        /// would include them even though pulldown-cmark's own reading of "the content" does not.
        /// This happens for: any indented block of 2+ lines; any fenced or indented block of 2+
        /// lines nested inside a list item or block quote; two indented chunks CommonMark glues
        /// across a blank line; and *any* code block at all in a CRLF-encoded file, even a single
        /// line. Recording each `Event::Text`'s own range instead of merging them skips exactly
        /// those gaps — there is no interval that both names only content bytes *and* has a hole in
        /// the middle of it, so representing this exactly needs a list.
        ///
        /// ## What concatenating them gives you
        ///
        /// `code_body_text(&body_spans, src)` — join every range's slice, in that order, then strip
        /// at most one trailing `\n` — reconstructs the block's content byte for byte, matching what
        /// `parser_code_blocks` computes by joining the same events' `Event::Text` *payloads*
        /// directly, for **every** code block in the corpus outside a block quote, not just the ones
        /// whose content happens to arrive as a single span; see
        /// `code_body_text_matches_parser_code_blocks_for_every_non_quote_code_block_in_the_corpus`.
        ///
        /// An empty code block (an opening fence immediately followed by a closing one — no content
        /// line at all) has no `Event::Text` to report, so its `body_spans` is simply empty;
        /// `code_body_text` on an empty slice returns `""`, matching that (empty) content.
        ///
        /// ## Code blocks inside a block quote
        ///
        /// This model does not special-case block-quote nesting at all: a code block whose nearest
        /// container is (or is nested inside) a `Quote` is represented exactly like any other one —
        /// present in `Block::children`, with a `body_spans` that reconstructs its content the same
        /// way. `parser_code_blocks`, the render pipeline's own scanner this field is cross-checked
        /// against, deliberately does **not** count these — its `quote_depth > 0` gate exists only
        /// because nothing downstream of it currently offers a "copy this code block" affordance for
        /// quoted text, not because a quoted code block fails to structurally exist. This model is
        /// therefore an intentional superset of what `parser_code_blocks` reports; a future renderer
        /// built on top of `Doc` gains the ability to handle quoted code blocks as a byproduct of
        /// switching to it, without this model itself needing to change.
        body_spans: Vec<Range<usize>>,
    },
    List {
        ordered: bool,
        /// The first item's number, for an ordered list (`Tag::List(Some(n))`). `None` for a
        /// bullet list, and also `None` for an ordered list that happens to start at a number
        /// pulldown-cmark did not report (never happens in practice — kept `Option` only because
        /// the tag itself is).
        start: Option<u64>,
    },
    /// A list item. Its checkbox marker, if it has one — GFM task-list syntax only recognizes a
    /// literal space, `x`, or `X` between the brackets (see `Task`'s doc comment) — is `task`; its
    /// own content is `Block::children`: a `Paragraph` (plus whatever follows) for a loose item, or
    /// nested lists/code/quotes for a tight one, if it has any of those. A tight item's own inline
    /// text — which pulldown-cmark emits with **no** enclosing `Paragraph` at all, unlike a loose
    /// item's — is still represented here, as a synthetic `Paragraph` `parse_blocks` builds on the
    /// fly (`collect_stray_inline_run`) spanning exactly that inline run, not simply absent the way
    /// an earlier version of this model left it; see that function's own doc comment.
    ListItem { task: Option<Task> },
    /// A block quote. `alert` is `Some` when the quote's first line matches GitHub's `> [!TYPE]`
    /// alert-header syntax, decided by calling the render pipeline's own `parse_alert_header` on
    /// that line — not a second classifier of this module's own (see `alert_kind_of`).
    Quote { alert: Option<AlertKind> },
    /// A GFM table. `aligns` is the delimiter row's per-column alignment, straight from
    /// pulldown-cmark (`Alignment::None` for a column with no `:`, matching CommonMark — *not*
    /// the render pipeline's own `ColAlign`, which is a display-time choice with no "unspecified"
    /// state and defaults it to `Left`; see this variant's construction site for why the two are
    /// deliberately not the same type). `rows` is every row's cells, in source order, header row
    /// first (`rows[0]`) followed by the body rows (`rows[1..]`) — pulldown-cmark's own event
    /// stream keeps no other marker distinguishing them, since a GFM table has exactly one header
    /// row by construction. Each cell is its own byte range (the pipe-delimited span including its
    /// padding spaces); cell content is inline, so — like `Paragraph`/`Heading` — it is not
    /// represented as a nested `Block`.
    Table {
        aligns: Vec<Alignment>,
        rows: Vec<Vec<Range<usize>>>,
    },
    /// An HTML block (one of CommonMark's six HTML-block types — a `<div>`, a comment, a
    /// `<!DOCTYPE>`, ...). `tag` is a best-effort tag name read off the block's own opening line,
    /// `None` for a block that does not open with a plain `<name ...>`/`</name>` tag (a comment, a
    /// processing instruction, a declaration, a CDATA section). `<details>`/`</details>` are
    /// identified via the render pipeline's own `details_open_tag`/`is_details_close` predicates
    /// rather than the generic name scan, so this can never drift from what the renderer itself
    /// considers a details tag; everything else falls back to the generic scan (`html_tag_name`),
    /// which is not a competing *judgment* about anything the renderer decides — it is a plain
    /// "what's the first tag's name" read with no bearing on any other pass's behavior.
    ///
    /// Note pulldown-cmark does not merge a `<details>`/`<summary>` pair and the later
    /// `</details>` into one block the way `split_details` (the renderer's own peel) does — CommonMark
    /// only glues *consecutive, blank-line-free* HTML-tag lines into one `HtmlBlock`, so `<details>`,
    /// `<summary>...</summary>`, and (once the body paragraph in between closes it) the standalone
    /// `</details>` are three separate top-level blocks here. Reconstructing `split_details`'s
    /// higher-level "one folded block" view from these, if a future caller needs it, is a
    /// consumer-side concern — see the module doc comment on scope.
    Html { tag: Option<String> },
    /// A thematic break (`---` / `***` / `___`).
    ThematicBreak,
}

/// A task-list checkbox marker (`- [ ]` / `- [x]` / `- [X]`).
///
/// GFM task-list syntax (`pulldown_cmark::Options::ENABLE_TASKLISTS`, the only extension this
/// module uses to recognize one at all) accepts exactly one character between the brackets: a
/// space, `x`, or `X` — nothing else, including any of konoma's own custom task states (`/`, `-`,
/// ..., configured via `ui.md_task_states`), is emitted as `Event::TaskListMarker` by the parser at
/// all; a line like `- [/] in progress` parses as an ordinary list item whose text happens to start
/// with `[/]`. A future integration that wants custom states recognized here needs to read the
/// bracket contents itself rather than rely on this event — see the task report this module's
/// commit was reviewed against for the measurement backing this note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Task {
    /// The character currently written between the brackets in the source (`' '`, `'x'`, or
    /// `'X'`) — read directly off the byte at `state_at`, not inferred from
    /// `Event::TaskListMarker`'s own `bool` payload, which collapses `x` and `X` into the same
    /// "checked" value and so cannot tell them apart.
    pub state: char,
    /// Byte offset of that character within the string `Doc::parse` was given. `src[state_at]` is
    /// always `[state]`'s own byte; `src.as_bytes()[state_at - 1] == b'['` and
    /// `src.as_bytes()[state_at + 1] == b']'` always hold — `check_invariants` checks exactly this,
    /// over the whole corpus (via `model_invariants_hold_across_the_full_parity_corpus` and
    /// `model_invariants_hold_across_the_sample_md_files`), not just the handful of cases
    /// `task_marker_state_and_position_are_exact` spells out by hand — so a future in-place toggle
    /// can overwrite this one byte with no further bookkeeping.
    ///
    /// Unlike `BlockKind::CodeBlock.body_spans`, this needs no analogous "list of ranges" fix:
    /// pulldown-cmark reports `Event::TaskListMarker` as a single event whose own range is always
    /// exactly the three bytes `[`, the state character, `]` — never wider (confirmed against the
    /// scanner's own source, `scan_task_list_marker`) — regardless of how much whitespace precedes
    /// the bracket, what container the item is nested in, or CRLF line endings elsewhere on the
    /// line, because none of those bytes are *inside* the marker the way a continuation line's
    /// indentation sits *inside* a code block's `first_text.start..last_text.end`. There is no gap
    /// for a single contiguous range to fail to skip.
    pub state_at: usize,
}

/// One of the five GitHub alert types a block quote's header can declare
/// (`> [!NOTE]`/`[!TIP]`/`[!IMPORTANT]`/`[!WARNING]`/`[!CAUTION]`, plus the render pipeline's own
/// Obsidian aliases — see `AlertKind::parse`). Re-exported here (rather than a second enum with the
/// same variants) so `BlockKind::Quote.alert` is always in lockstep with what the renderer draws as
/// a callout box: adding a sixth alert type only ever means editing one enum.
pub(crate) use super::AlertKind;

/// The single walk `Doc::parse` performs over `src` (see the module doc comment: exactly one
/// `Parser::new_ext` call for the whole document, never a second one for any substring of it).
/// Wraps pulldown-cmark's own `Peekable<OffsetIter>` — `peek`/`next` behave identically to that
/// type's own methods from every call site's point of view, which is why every block-parsing
/// function below barely changed shape when this replaced a plain `Peekable<OffsetIter>` alias —
/// and additionally records, in `events`, a clone of every event `next()` actually consumes (never
/// one merely `peek`ed at and left alone). `events` becomes `Doc.events` once parsing finishes.
/// `pos()`, read once before and once after a leaf block's own inline content is consumed, is how
/// `BlockKind::Heading`/`BlockKind::Paragraph` — and the tight-list-item stray-run case, see
/// `collect_stray_inline_run` — each compute which slice of it is theirs; see those construction
/// sites for the exact convention each one uses (which markers, if any, a block's own `inline`
/// range does or does not include).
struct Walker<'a> {
    iter: std::iter::Peekable<pulldown_cmark::OffsetIter<'a>>,
    events: Vec<(Event<'a>, Range<usize>)>,
}

impl<'a> Walker<'a> {
    fn peek(&mut self) -> Option<&(Event<'a>, Range<usize>)> {
        self.iter.peek()
    }

    /// Consumes and returns the next event — identical, from the caller's point of view, to
    /// `Peekable::next` — but first appends a clone of it to `self.events`. Cheap, not merely
    /// convenient: `Event`/`Range<usize>` are both `Clone`, and every event this walk ever sees
    /// borrows from `src` via `CowStr::Borrowed` (a fresh `Parser`, never a second one over a
    /// substring — see the module doc comment), so cloning one copies a couple of machine words,
    /// never allocates.
    fn next(&mut self) -> Option<(Event<'a>, Range<usize>)> {
        let item = self.iter.next()?;
        self.events.push(item.clone());
        Some(item)
    }

    /// The index the *next* `next()` call will record its event at — equivalently, how many events
    /// have been consumed (and recorded) so far. See the struct's own doc comment for how a leaf
    /// block uses two readings of this (one before, one after consuming its own inline content) to
    /// compute an `inline: Range<usize>` into `Doc.events`.
    fn pos(&self) -> usize {
        self.events.len()
    }
}

impl<'a> Doc<'a> {
    /// Parses `src` — which the caller is responsible for having already run through konoma's own
    /// source pre-passes if it wants the result to match what the screen shows; see the module doc
    /// comment — into a block tree, walking pulldown-cmark's own event stream for the whole document
    /// exactly once (`Walker`). Every leaf block whose own content is purely inline
    /// (`BlockKind::Heading`/`BlockKind::Paragraph`) records where its share of that single stream
    /// sits, by index range, rather than being handed a copy of it or (as an earlier version of the
    /// renderer built on this model did) needing to re-parse its own byte range separately later.
    pub(crate) fn parse(src: &'a str) -> Doc<'a> {
        let mut w = Walker {
            iter: Parser::new_ext(src, parse_options())
                .into_offset_iter()
                .peekable(),
            events: Vec::new(),
        };
        let blocks = parse_blocks(&mut w, src);
        Doc {
            events: w.events,
            blocks,
        }
    }
}

/// The `pulldown-cmark` options this model's walk uses. Built from the render pipeline's own
/// `tui_markdown_parse_options` — by calling it, not by copying its six `insert` calls — so the two
/// option sets can never independently drift the way that function's own doc comment warns two
/// *different* option sets reading the same document eventually do; this model asks about
/// everything the renderer's own scanners already ask about, plus exactly one extension:
///
/// * `ENABLE_TABLES` — the render pipeline turns this **off** (see
///   `tui_markdown_parse_options`'s doc comment: tui-markdown collapses a GFM table into a single
///   line, so konoma's own hand-written table renderer, `split_tables`, intercepts table text
///   *before* it ever reaches tui-markdown, and turning this on would let tui-markdown see a table
///   it isn't meant to render). This model has no such renderer standing between it and the table —
///   it is the thing a future renderer would *read* structure from — so it needs pulldown-cmark's
///   real `Table`/`TableHead`/`TableRow`/`TableCell` events to fill in `BlockKind::Table`.
///
/// Two extensions that would add whole new *event kinds* stay off, on purpose, because konoma
/// already has a hand-written source pre-pass for each, run on `src` *before* it reaches any
/// parser — turning the parser's own equivalent on here would not add anything this model uses, it
/// would just be a second, silently-discarded reading of text those pre-passes already consumed:
///
/// * `ENABLE_MATH` — `$...$`/`$$...$$` are extracted by `split_math`, replaced with synthetic image
///   placements, before the text a Markdown parser ever sees is finalized.
/// * `ENABLE_FOOTNOTES` — `[^id]`/`[^id]: ...` are rewritten by `process_footnotes` into plain
///   numbered references (and a trailing rendered list) as one of the very pre-passes that produces
///   the `pre_src` this model expects (see the module doc comment) — by the time text reaches here,
///   a real footnote reference has already become plain text, not `Event::FootnoteReference`.
///
/// `ENABLE_GFM` (which, among other things, would make pulldown-cmark itself recognize `> [!NOTE]`
/// and report a `BlockQuoteKind`) is left off for a third, different reason: konoma already has a
/// hand-written alert-header parser, `parse_alert_header`, and `BlockKind::Quote.alert` is filled
/// in by *calling that function* on the quote's own first line (`alert_kind_of`) rather than by a
/// second, competing classifier reading pulldown-cmark's own opinion.
///
/// `pub(crate)`: `md_model_snapshot_tests` (in `src/app/`) drives its own independent,
/// tree-free reading of the same event stream — `scan_reference_events` — off *this exact*
/// function too, for the same reason `Doc::parse` itself calls `tui_markdown_parse_options`
/// instead of copying it: a completeness check built on a second, hand-copied option set could
/// pass or fail for reasons that have nothing to do with whether `Doc::parse` actually covers
/// everything the real parser reports.
pub(crate) fn parse_options() -> Options {
    let mut o = super::tui_markdown_parse_options();
    o.insert(Options::ENABLE_TABLES);
    o
}

/// Parses the children of the container whose `Start` event `events` has already consumed (or, at
/// the top level, the whole document) — every top-level block-shaped event, recursing one level for
/// each, until either the iterator is exhausted (top level) or an `Event::End` is reached, which
/// this function consumes and returns after (so the caller never needs to consume its own
/// terminator separately; see `parse_container`'s `List`/`Item`/`BlockQuote` arms).
///
/// Every construct pulldown-cmark can emit at this level either gets a specific `BlockKind` (see
/// `parse_container`), is silently skipped, balanced, via `skip_inline_to` (a construct this model
/// never represents at all — a footnote definition, a metadata block, ...; see the module doc
/// comment and `is_unmodeled_container_tag`), or — the one shape this loop does not simply hand off
/// to `parse_container` — is a **tight list item's own inline content, reaching this level with no
/// enclosing `Paragraph` at all**. Pulldown-cmark's tight/loose convention (`Task`'s doc comment
/// documents the identical shift for a task marker) omits the wrapping tag entirely for a tight
/// item, so the very next event here can legitimately be a plain `Text`/`Code`/... or even the
/// `Start` of an inline construct like `Strong`/`Link` (`is_inline_start_tag`) — either shape starts
/// a run `collect_stray_inline_run` coalesces into one synthetic `Paragraph`, rather than the
/// content being silently dropped the way an earlier version of this function left it (see the
/// completeness invariants in `md_model_snapshot_tests` that gap was caught by, and
/// `BlockKind::ListItem`'s own doc comment). Nothing here can desync the event stream either way:
/// every `Start` this loop sees — whether routed to `parse_container` or folded into a stray run —
/// is fully consumed (recursively, down to its own matching `End`) before the loop looks at the
/// next event.
fn parse_blocks(events: &mut Walker, src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    while let Some((ev, range)) = events.next() {
        match ev {
            Event::End(_) => return out,
            Event::Start(tag) if is_inline_start_tag(&tag) => {
                // The `next()` call above already recorded this `Start(tag)` — it is the stray
                // run's own first event, so `events.pos() - 1` (its index) is where the run's
                // `inline` range starts; see `collect_stray_inline_run`'s own doc comment.
                let inline_start = events.pos() - 1;
                let run = collect_stray_inline_run((Event::Start(tag), range), events);
                let inline = inline_start..events.pos();
                out.push(leaf(BlockKind::Paragraph { inline }, run));
            }
            Event::Start(tag) => {
                if let Some(block) = parse_container(tag, range, events, src) {
                    out.push(block);
                }
            }
            Event::Rule => out.push(Block {
                kind: BlockKind::ThematicBreak,
                src: range,
                children: Vec::new(),
            }),
            other if is_stray_inline_leaf(&other) => {
                let inline_start = events.pos() - 1;
                let run = collect_stray_inline_run((other, range), events);
                let inline = inline_start..events.pos();
                out.push(leaf(BlockKind::Paragraph { inline }, run));
            }
            // `Event::TaskListMarker` reaching this loop at all (rather than being consumed by
            // `parse_item_task_and_children`, the only place that ever looks for one, at one of
            // the two positions it can legitimately appear) is not reachable for well-formed
            // input — a marker only ever exists as the first event of an item's own content.
            // Ignored rather than treated as an error: never crashing on any input pulldown-cmark
            // accepts is principle #3 (see `CLAUDE.md`), and there is no well-defined block to
            // build from three stray bytes that were supposed to be a list item's own marker.
            _ => {}
        }
    }
    out
}

/// Builds the `Block` for one container whose `Start(tag)` (at `range`) `parse_blocks` has just
/// consumed, then consumes everything up to and including its matching `End` — so control returns
/// to `parse_blocks` exactly one event past this construct's own extent, every time. Returns `None`
/// for a construct this model does not represent (see the module doc comment on scope) after still
/// fully consuming and discarding its subtree, so the caller's stream position is unaffected either
/// way.
fn parse_container(tag: Tag, range: Range<usize>, events: &mut Walker, src: &str) -> Option<Block> {
    match tag {
        Tag::Paragraph => {
            let inline_start = events.pos();
            skip_inline_to(events, TagEnd::Paragraph);
            // `skip_inline_to` always returns having just consumed the matching `End` as the very
            // last event (see its own doc comment) — `events.pos() - 1` is that `End`'s own index,
            // so this excludes it from the paragraph's own `inline` range.
            let inline = inline_start..events.pos() - 1;
            Some(leaf(BlockKind::Paragraph { inline }, range))
        }
        Tag::Heading {
            level,
            id,
            classes,
            attrs,
        } => {
            let inline_start = events.pos();
            skip_inline_to(events, TagEnd::Heading(level));
            let inline = inline_start..events.pos() - 1;
            Some(leaf(
                BlockKind::Heading {
                    level: level as u8,
                    inline,
                    id: id.map(|c| c.into_string()),
                    classes: classes.into_iter().map(|c| c.into_string()).collect(),
                    attrs: attrs
                        .into_iter()
                        .map(|(k, v)| (k.into_string(), v.map(|v| v.into_string())))
                        .collect(),
                },
                range,
            ))
        }
        Tag::CodeBlock(kind) => {
            let (fenced, lang) = match kind {
                CodeBlockKind::Fenced(info) => (
                    true,
                    if info.trim().is_empty() {
                        None
                    } else {
                        Some(info.into_string())
                    },
                ),
                CodeBlockKind::Indented => (false, None),
            };
            let body_spans = collect_code_body_spans(events);
            Some(leaf(
                BlockKind::CodeBlock {
                    lang,
                    fenced,
                    body_spans,
                },
                range,
            ))
        }
        Tag::List(start) => {
            let children = parse_blocks(events, src); // also consumes End(List(_))
            Some(Block {
                kind: BlockKind::List {
                    ordered: start.is_some(),
                    start,
                },
                src: range,
                children,
            })
        }
        Tag::Item => {
            let (task, children) = parse_item_task_and_children(events, src);
            Some(Block {
                kind: BlockKind::ListItem { task },
                src: range,
                children,
            })
        }
        Tag::BlockQuote(_) => {
            let children = parse_blocks(events, src); // also consumes End(BlockQuote(_))
            let alert = alert_kind_of(src, &range);
            Some(Block {
                kind: BlockKind::Quote { alert },
                src: range,
                children,
            })
        }
        Tag::Table(aligns) => {
            let rows = collect_table_rows(events);
            Some(leaf(BlockKind::Table { aligns, rows }, range))
        }
        Tag::HtmlBlock => {
            let tag = collect_html_tag(events, &range, src);
            Some(leaf(BlockKind::Html { tag }, range))
        }
        // A YAML/`+++`-style metadata block (only reachable if the caller did not strip front
        // matter first — see the module doc comment), a footnote definition, or a definition list:
        // none has a `BlockKind`. Consumed and discarded rather than left half-read, so a caller
        // that *does* hand this a raw, un-preprocessed file still gets a well-formed (if partial)
        // tree instead of a desynced one.
        other => {
            debug_assert!(
                is_unmodeled_container_tag(&other),
                "parse_container's own exhaustive match and is_unmodeled_container_tag must \
                 agree on which Tag variants land here — the latter is a second, independent \
                 reading of the exact same set for md_model_snapshot_tests's completeness cross-\
                 check (see its own doc comment); a new Tag variant landing in *only one* of the \
                 two would let that check pass without actually exercising the gap it exists to \
                 catch."
            );
            skip_inline_to(events, other.to_end());
            None
        }
    }
}

fn leaf(kind: BlockKind, src: Range<usize>) -> Block {
    Block {
        kind,
        src,
        children: Vec::new(),
    }
}

/// Whether `tag` is one of the handful of constructs this model never represents at all —
/// `parse_container`'s own `other` arm, named as a predicate rather than left as an implicit "every
/// `Tag` variant not matched above" so it can be reused (and checked in lockstep with that match via
/// a `debug_assert!` right there) by `md_model_snapshot_tests`'s completeness cross-check, which
/// needs to know which parts of a raw event stream the model *deliberately* does not promise to
/// cover before it can flag anything else as a genuine gap. `FootnoteDefinition` and
/// `MetadataBlock` are reachable only when the caller hands `Doc::parse` un-preprocessed text
/// (konoma's own source pre-passes strip front matter and rewrite footnote definitions away before
/// any text reaches a parser at all — see the module doc comment); `Options::ENABLE_DEFINITION_LIST`
/// is never turned on by `parse_options` in the first place, so `Tag::DefinitionList*` can never
/// actually be emitted at all today, but is still named here for the same reason `parse_container`'s
/// own match still spells it out explicitly — an exhaustive match, not one relying on what today's
/// `parse_options` happens to enable.
///
/// `pub(crate)`: reused, unchanged, by `md_model_snapshot_tests`'s completeness cross-check
/// (`scan_reference_events`) — see `parse_options`'s own doc comment for why sharing a classifier
/// like this, rather than a second hand-copied list of the same `Tag` variants, is the whole point.
pub(crate) fn is_unmodeled_container_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
    )
}

/// Whether `tag` opens one of pulldown-cmark's own *span-level* constructs — its own source groups
/// these under a `// span-level tags` comment, directly below the block-level ones — as opposed to
/// something block-shaped (`Paragraph`, `Heading`, `List`, ...). Needed because, for a tight list
/// item, either kind of `Start` can legitimately be the very next event `parse_blocks` sees (see its
/// own doc comment): this tells the two apart so the block-shaped ones still go to
/// `parse_container`, while the span-level ones start a stray inline run instead
/// (`collect_stray_inline_run`).
///
/// Not `pub(crate)` like its two siblings (`is_stray_inline_leaf`, `is_unmodeled_container_tag`):
/// `md_model_snapshot_tests`'s completeness cross-check has no runs of its own to group, only raw
/// event positions to record, so it never needs this particular distinction — see
/// `scan_reference_events`'s own doc comment there.
fn is_inline_start_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

/// Whether `ev` is a leaf inline-content event this model can fold into a stray run's synthetic
/// `Paragraph` (see `parse_blocks`'s doc comment) — every `Event` variant that carries literal
/// document content and is neither a container (`Start`/`End`), `Rule` (its own leaf `BlockKind`),
/// nor `TaskListMarker`. `TaskListMarker` is deliberately excluded even though it is, in every other
/// sense, a leaf event this model's own `parse_options` can produce: it is recorded on
/// `ListItem.task` instead of folded into a `Paragraph`'s range, because its own three bytes (`[`,
/// the state character, `]`) sit in the item's own marker text — *outside* whatever `Paragraph`
/// (real or synthetic) the item's content becomes — not inside it; see `Task::state_at`'s doc
/// comment. Every genuine call site consumes a `TaskListMarker` before `parse_blocks` ever sees one
/// (`parse_item_task_and_children`); this function simply does not claim it as "content" in case
/// that assumption is ever wrong for some input this model has not been tested against, so it still
/// degrades to `parse_blocks`'s own final, silent catch-all rather than corrupting a `Paragraph`'s
/// range with marker bytes it never claimed to cover.
///
/// `pub(crate)`: reused, unchanged, by `md_model_snapshot_tests`'s completeness cross-check
/// (`scan_reference_events`) — see `parse_options`'s own doc comment for why sharing a classifier
/// like this, rather than a second hand-copied list of the same `Event` variants, is the whole
/// point.
pub(crate) fn is_stray_inline_leaf(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::SoftBreak
            | Event::HardBreak
    )
}

/// Builds a "stray inline run"'s byte range — see `parse_blocks`'s own doc comment for when one
/// starts — given `first`, the run's own first event, **already consumed** by `parse_blocks`'s
/// loop (so this never re-peeks it; `first.1.start` is the run's own starting offset). The run's
/// `inline` *event-index* range (the synthetic `Paragraph`'s own share of `Doc.events`) is computed
/// by the caller instead, from `Walker::pos()` read before and after this call — not here, since
/// `first` was already consumed (and recorded) before this function was ever entered, so this
/// function alone cannot see where the run's own first index actually was. Balances
/// `first` exactly the way the loop below balances every further event it consumes itself: a leaf
/// (`is_stray_inline_leaf`) needs nothing further, but the `Start` of a further inline construct
/// (`is_inline_start_tag` — `**bold**`, a link, an image, ...) still has its own body sitting
/// unconsumed in `events`, so `skip_inline_to` drains it, down to and including its matching `End`,
/// before this can safely look at what comes next — an earlier version of this function forgot this
/// step for `first` specifically (handling it only for events the loop consumed on its own), which
/// left the very case this function exists for — a tight item beginning with `**bold**` rather than
/// plain text — desyncing the event stream one token in, corrupting every range downstream; see the
/// `emphasis in task` case in `task_corpus` and the parity corpus's own overlap invariant, which is
/// what actually caught it.
///
/// Drains every further event that belongs to the same run the identical way, advancing `end` to
/// each one's own range end — relying on the same pulldown-cmark behavior `parse_container` already
/// leans on throughout this file, that a `Start`'s reported range always equals its construct's
/// *whole* span, matching its `End`'s range exactly — so no separate bookkeeping is needed to find
/// where a nested `Start(Strong)`/`Start(Link)`/... really ends; its own `Start` event's range
/// already says. Stops, **without** consuming it, the moment the next event is not part of the run:
/// the `Event::End` that closes the *container* this content belongs to, or any block-level `Start`
/// (a nested list, a code block, a following loose paragraph, ...) — both are left for
/// `parse_blocks`'s own loop to see next, exactly like any other block boundary.
///
/// One real (not a bug) difference from a *real* `Paragraph`'s own range: pulldown-cmark reports a
/// `Tag::Paragraph`'s range as running through its trailing newline (`"a\n"` for a one-line
/// paragraph), while a synthetic one built here stops at the run's last inline event's own end
/// (`"a"`, no newline) — there is no wrapping tag here for pulldown-cmark to have assigned that
/// wider range to in the first place, only the leaf events themselves, whose own ranges never
/// include it; see `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one`.
fn collect_stray_inline_run(first: (Event<'_>, Range<usize>), events: &mut Walker) -> Range<usize> {
    let (first_ev, first_range) = first;
    let start = first_range.start;
    let mut end = first_range.end;
    if let Event::Start(tag) = first_ev {
        skip_inline_to(events, tag.to_end());
    }
    loop {
        let continues = match events.peek() {
            Some((Event::Start(tag), _)) => is_inline_start_tag(tag),
            Some((ev, _)) => is_stray_inline_leaf(ev),
            None => false,
        };
        if !continues {
            return start..end;
        }
        let (ev, range) = events.next().expect("peeked Some above");
        end = range.end;
        if let Event::Start(tag) = ev {
            skip_inline_to(events, tag.to_end());
        }
    }
}

/// Consumes events up to and including the given `end`, for a tag whose `Start` the caller has
/// already consumed — recursing into any further `Start` it meets along the way so a *nested*
/// inline construct (a link containing emphasis, an image inside a link's title, ...) cannot be
/// mistaken for the enclosing one's own close. Used for every construct whose CommonMark content is
/// purely inline and this model does not represent as nested blocks: a heading's or paragraph's
/// text, and a table cell's.
fn skip_inline_to(events: &mut Walker, end: TagEnd) {
    while let Some((ev, _)) = events.next() {
        match ev {
            Event::Start(t) => skip_inline_to(events, t.to_end()),
            Event::End(e) if e == end => return,
            _ => {}
        }
    }
}

/// Parses one list item's task marker and its own children together — the two cannot be handled as
/// two independent steps (`take_task_marker` then `parse_blocks`) the way an earlier version of this
/// model did, because *where* the marker sits in the event stream depends on whether the item is
/// tight or loose (see `Task`'s own doc comment for the identical tight/loose distinction):
///
/// * **Tight item** (`- [ ] a`): `Event::TaskListMarker`, if present, is the very first event of the
///   item's own content — `take_task_marker` handles this directly, exactly as before.
/// * **Loose item** (`- [ ] a\n\n- b`): the marker instead sits *inside* the item's own first
///   `Paragraph` — pulldown-cmark still omits nothing, but wraps the item's text in a `Paragraph`
///   (as any loose item's is) and reports the marker as that paragraph's own first inline event,
///   ahead of its `Text`. Seeing it therefore means consuming `Start(Paragraph)` first — `Walker` is
///   only one-deep peekable, so there is no way to look two events ahead without consuming the
///   first — which means this function has to build that first `Paragraph` block itself (identically
///   to `parse_container`'s own `Tag::Paragraph` arm) rather than being able to hand `Start`
///   `(Paragraph)` back to `parse_blocks` the way it would for any *later* paragraph in the item.
///
/// Only the item's own *first* child is ever special-cased this way; every other one is reached
/// through the trailing `parse_blocks` call below, unchanged, and a nested item (inside a sublist
/// reached the same way) parses its own marker through its own, independent call to this same
/// function — so there is no path by which an outer item could mistake an inner one's marker, or an
/// inner one's first paragraph, for its own.
fn parse_item_task_and_children(events: &mut Walker, src: &str) -> (Option<Task>, Vec<Block>) {
    if matches!(events.peek(), Some((Event::Start(Tag::Paragraph), _))) {
        let (_, para_range) = events.next().expect("peeked Some above");
        let task = take_task_marker(events, src);
        // `pos()` is read *after* `take_task_marker` — a loose item's own leading marker, if any,
        // has already been consumed and excluded by this point, matching `Task::state_at`'s own
        // doc comment: those bytes sit in the item's marker text, not inside its paragraph's
        // content.
        let inline_start = events.pos();
        skip_inline_to(events, TagEnd::Paragraph);
        let inline = inline_start..events.pos() - 1;
        let mut children = vec![leaf(BlockKind::Paragraph { inline }, para_range)];
        children.extend(parse_blocks(events, src)); // also consumes End(Item)
        return (task, children);
    }
    let task = take_task_marker(events, src);
    let children = parse_blocks(events, src); // also consumes End(Item)
    (task, children)
}

/// If the very next event in `events` is `Event::TaskListMarker`, consumes it and returns the
/// `Task` it describes; otherwise leaves `events` untouched. Called from the two positions
/// `parse_item_task_and_children` documents — right after `Item`'s own `Start` (a tight item) and
/// right after the item's own first `Paragraph`'s `Start` (a loose item) — because a marker, when
/// present at all, is always the very next event at whichever of those two the item's own shape
/// calls for; see that function's own doc comment for why the two positions exist, and `Task`'s doc
/// comment for why a marker can only ever appear at one of them, never both, never neither.
fn take_task_marker(events: &mut Walker, src: &str) -> Option<Task> {
    if !matches!(events.peek(), Some((Event::TaskListMarker(_), _))) {
        return None;
    }
    let (_, range) = events.next().expect("peeked Some above");
    // `[` at `range.start`, the state char at `range.start + 1`, `]` at `range.start + 2` — see
    // `scan_task_list_marker` in pulldown-cmark's own scanner, which accepts nothing but a single
    // ASCII byte there, so this index always lands on (and this is always a one-byte) char.
    let state_at = range.start + 1;
    let state = src[state_at..]
        .chars()
        .next()
        .expect("a TaskListMarker range always has a state byte following '['");
    Some(Task { state, state_at })
}

/// Collects a code block's content as one byte range per `Event::Text` pulldown-cmark reports, in
/// order — see `BlockKind::CodeBlock.body_spans`'s doc comment for exactly what these ranges do and
/// do not guarantee and how to turn them back into content — by walking every such event up to the
/// block's own `End(CodeBlock)`, which this also consumes. If the stream is exhausted first (a
/// malformed/truncated caller input), this simply stops with whatever spans it collected so far. An
/// empty code block emits no `Event::Text` at all, so this returns an empty `Vec` for one — there is
/// no position to anchor a placeholder range on that would mean anything once the content is a list
/// rather than a single interval, so none is invented.
fn collect_code_body_spans(events: &mut Walker) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    while let Some((ev, range)) = events.next() {
        match ev {
            Event::Text(_) => spans.push(range),
            Event::End(TagEnd::CodeBlock) => break,
            // Not reachable for well-formed input (a code block's content is never itself a
            // container), balanced defensively rather than assumed away — see `parse_blocks`'s
            // doc comment on never desyncing the stream.
            Event::Start(t) => skip_inline_to(events, t.to_end()),
            _ => {}
        }
    }
    spans
}

/// Reconstructs a code block's content, byte for byte, from its `body_spans` — concatenating each
/// range's slice of `src`, in order, then stripping at most one trailing `\n` — matching
/// `parser_code_blocks`'s own "join every `Event::Text`, strip one trailing newline" convention
/// exactly. See `BlockKind::CodeBlock.body_spans`'s doc comment for what bytes the spans themselves
/// necessarily skip (a continuation line's own share of container indentation; a `\r` consumed by
/// line-ending normalization) and why concatenating them still gets the content right regardless.
pub(crate) fn code_body_text(body_spans: &[Range<usize>], src: &str) -> String {
    let mut s = String::new();
    for r in body_spans {
        s.push_str(&src[r.clone()]);
    }
    match s.strip_suffix('\n') {
        Some(t) => t.to_string(),
        None => s,
    }
}

/// Consumes a `Table`'s `TableHead`/`TableRow` children up to and including its own `End(Table)`,
/// collecting each row's cells. `rows[0]` is always the header row — GFM defines a table as
/// exactly one header row followed by zero or more body rows, and pulldown-cmark's own event
/// stream (one `TableHead`, then any number of `TableRow`s, all inside one `Table`) mirrors that
/// directly, so no extra bookkeeping is needed to tell them apart.
fn collect_table_rows(events: &mut Walker) -> Vec<Vec<Range<usize>>> {
    let mut rows = Vec::new();
    while let Some((ev, _)) = events.next() {
        match ev {
            Event::Start(Tag::TableHead) => rows.push(collect_row_cells(events, TagEnd::TableHead)),
            Event::Start(Tag::TableRow) => rows.push(collect_row_cells(events, TagEnd::TableRow)),
            Event::End(TagEnd::Table) => break,
            Event::Start(t) => skip_inline_to(events, t.to_end()), // defensive; not reachable
            _ => {}
        }
    }
    rows
}

/// Consumes one table row's `TableCell`s up to and including its own `end` (`TableHead` or
/// `TableRow`), collecting each cell's byte range. Cell content is inline (see `BlockKind::Table`'s
/// doc comment), so — like a paragraph or heading — it is walked past via `skip_inline_to`, not
/// turned into a nested `Block`.
fn collect_row_cells(events: &mut Walker, end: TagEnd) -> Vec<Range<usize>> {
    let mut cells = Vec::new();
    while let Some((ev, range)) = events.next() {
        match ev {
            Event::Start(Tag::TableCell) => {
                skip_inline_to(events, TagEnd::TableCell);
                cells.push(range);
            }
            Event::End(e) if e == end => break,
            Event::Start(t) => skip_inline_to(events, t.to_end()), // defensive; not reachable
            _ => {}
        }
    }
    cells
}

/// Consumes an `HtmlBlock`'s `Event::Html` lines up to and including its own `End(HtmlBlock)`,
/// then reads a best-effort tag name off the block's own opening line — see `BlockKind::Html`'s
/// doc comment for exactly what this does and does not distinguish.
fn collect_html_tag(events: &mut Walker, block_range: &Range<usize>, src: &str) -> Option<String> {
    while let Some((ev, _)) = events.next() {
        match ev {
            Event::End(TagEnd::HtmlBlock) => break,
            Event::Start(t) => skip_inline_to(events, t.to_end()), // defensive; not reachable
            _ => {}
        }
    }
    let first_line = src[block_range.clone()]
        .lines()
        .next()
        .unwrap_or("")
        .trim_end();
    if super::details_open_tag(first_line).is_some() || super::is_details_close(first_line) {
        return Some("details".to_string());
    }
    html_tag_name(first_line)
}

/// A permissive, best-effort HTML tag name off the start of `line`: `<name`/`</name`, taking every
/// following ASCII alphanumeric-or-hyphen character as part of the name (so `<detailsx>` reads as
/// `"detailsx"`, not a truncated match on `"details"` — the full run is compared, not a prefix).
/// `None` for anything that is not a plain opening/closing tag at all — a comment (`<!--`), a
/// declaration (`<!DOCTYPE`), or a processing instruction (`<?php`) — since none of those has a
/// "tag name" in the sense this field means.
fn html_tag_name(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('<')?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if rest.starts_with(['!', '?']) {
        return None;
    }
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

/// Whether the block quote at `range` opens with a GitHub alert header (`> [!NOTE]`, optionally
/// followed by an Obsidian-style title) — decided by handing the quote's own first line to the
/// render pipeline's real `parse_alert_header`, not a second parser of this module's own.
fn alert_kind_of(src: &str, range: &Range<usize>) -> Option<AlertKind> {
    let first_line = src[range.clone()].lines().next().unwrap_or("");
    super::parse_alert_header(first_line).map(|(kind, _title)| kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the exact, asymmetric byte range `render.rs`'s own `render_paragraph_math` doc comment
    /// cites as the reason a quoted paragraph's text is never scanned for math directly from
    /// `Block.src`: a quoted `Paragraph`'s own range excludes the `>` marker on its *first* line
    /// (`Paragraph`'s own `src.start` lands right after it) but *includes* it on every continuation
    /// line (the range simply runs to the end of whatever pulldown-cmark last consumed, markers and
    /// all) — so a naive line-based `>`-prefix check over that one range would misjudge the first
    /// line every time.
    #[test]
    fn quoted_paragraph_src_range_excludes_the_first_lines_marker_but_not_a_continuation_lines() {
        let src = "> hello $x$ world\n> line2\n";
        let doc = Doc::parse(src);
        let BlockKind::Quote { .. } = &doc.blocks[0].kind else {
            panic!("expected a top-level quote: {:?}", doc.blocks[0].kind);
        };
        assert_eq!(doc.blocks[0].children.len(), 1);
        let child = &doc.blocks[0].children[0];
        assert!(matches!(&child.kind, BlockKind::Paragraph { .. }));
        assert_eq!(&src[child.src.clone()], "hello $x$ world\n> line2\n");
    }

    /// Pins the tight list item counterpart: unlike a quote, a list marker is never repeated on a
    /// continuation line, so a **synthetic** first paragraph's own range (`collect_stray_inline_run`)
    /// never includes any container marker at all — no asymmetry to guard against there, which is
    /// exactly why `render.rs`'s own `walk_inline_math` is safe to scan a list item's first child
    /// directly (see that module's own doc comment on `render_doc`'s `math_here` for the contrast).
    #[test]
    fn tight_list_items_synthetic_first_paragraph_never_includes_the_bullet_marker() {
        let src = "- item $x$ here\n- second\n";
        let doc = Doc::parse(src);
        let BlockKind::List { .. } = &doc.blocks[0].kind else {
            panic!("expected a top-level list: {:?}", doc.blocks[0].kind);
        };
        assert_eq!(doc.blocks[0].children.len(), 2);
        let BlockKind::ListItem { .. } = &doc.blocks[0].children[0].kind else {
            panic!("expected the first list item");
        };
        let first_child = &doc.blocks[0].children[0].children[0];
        assert!(matches!(&first_child.kind, BlockKind::Paragraph { .. }));
        assert_eq!(&src[first_child.src.clone()], "item $x$ here");
    }

    /// Concatenates every `Event::Text`/`Event::Code`/`Event::SoftBreak`(→ `" "`)/
    /// `Event::HardBreak`(→ `"\n"`) payload in `doc.events[inline]`, in order — a minimal,
    /// style-blind reconstruction, good enough for a unit test to confirm a leaf's own `inline`
    /// range names the right slice of `Doc.events` (and, implicitly, that it excludes whatever it
    /// is supposed to — a task marker, the block's own wrapping `Start`/`End` — since any of those
    /// leaking in would show up here as extra text). `render.rs`'s own diff harness is the
    /// exhaustive, style-aware check on top of this; this helper only needs to catch "wrong slice
    /// entirely" at the unit-test level.
    fn inline_plain_text(doc: &Doc<'_>, inline: &Range<usize>) -> String {
        let mut s = String::new();
        for (ev, _) in &doc.events[inline.clone()] {
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

    // ---- construction sanity: one focused case per construct -------------------------------

    #[test]
    fn heading_and_paragraph_are_leaves_with_no_children() {
        let doc = Doc::parse("# Title\n\nbody\n");
        assert_eq!(doc.blocks.len(), 2);
        let BlockKind::Heading {
            level: 1, inline, ..
        } = &doc.blocks[0].kind
        else {
            panic!("expected a level-1 heading")
        };
        assert_eq!(inline_plain_text(&doc, inline), "Title");
        assert!(doc.blocks[0].children.is_empty());
        assert_eq!(doc.blocks[0].src, 0..8);
        let BlockKind::Paragraph { inline } = &doc.blocks[1].kind else {
            panic!("expected a paragraph")
        };
        assert_eq!(inline_plain_text(&doc, inline), "body");
        assert_eq!(doc.blocks[1].src, 9..14);
    }

    #[test]
    fn heading_level_matches_the_number_of_hashes() {
        for (n, marker) in [
            (1, "#"),
            (2, "##"),
            (3, "###"),
            (4, "####"),
            (5, "#####"),
            (6, "######"),
        ] {
            let src = format!("{marker} h\n");
            let doc = Doc::parse(&src);
            let BlockKind::Heading { level, inline, .. } = &doc.blocks[0].kind else {
                panic!("expected a heading for {marker:?}")
            };
            assert_eq!(*level, n, "marker {marker:?}");
            assert_eq!(inline_plain_text(&doc, inline), "h");
        }
    }

    #[test]
    fn heading_attributes_are_captured_as_owned_data() {
        // `{#id .class key=value}` heading-attribute syntax used to only ever reach `render.rs` by
        // re-parsing the heading's own byte range a second time (`Tag::Heading`'s `id`/`classes`/
        // `attrs` fields) — `Doc::parse` now captures the exact same data itself, converted to owned
        // `String`s, so a caller never needs a second parse to read it.
        let doc = Doc::parse("## Custom Heading {#custom-id .note lang=en}\n");
        let BlockKind::Heading {
            id,
            classes,
            attrs,
            inline,
            ..
        } = &doc.blocks[0].kind
        else {
            panic!("expected a heading")
        };
        assert_eq!(id.as_deref(), Some("custom-id"));
        assert_eq!(classes, &["note".to_string()]);
        assert_eq!(attrs, &[("lang".to_string(), Some("en".to_string()))]);
        // The attribute block itself is not part of the heading's own inline *content* — it is a
        // separate field pulldown-cmark reports on `Tag::Heading` itself, not an inline event.
        assert_eq!(inline_plain_text(&doc, inline), "Custom Heading");
    }

    #[test]
    fn heading_with_no_attribute_syntax_has_none_and_empty() {
        let doc = Doc::parse("## Plain\n");
        let BlockKind::Heading {
            id, classes, attrs, ..
        } = &doc.blocks[0].kind
        else {
            panic!("expected a heading")
        };
        assert_eq!(*id, None);
        assert!(classes.is_empty());
        assert!(attrs.is_empty());
    }

    #[test]
    fn reference_link_with_definition_in_a_different_block_resolves_across_the_whole_document() {
        // The whole point of `Doc::parse` walking the document once instead of block-by-block: a
        // resolved reference link's label becomes an `Event::Text` inside a real `Tag::Link`, not
        // literal bracket text, only if the parser has already seen the `[ref]: url` definition
        // (here, in the *next* top-level block) by the time it reaches this paragraph — something a
        // per-block re-parse (an earlier version of `render.rs`) could never see, because a link
        // reference definition contributes no events of its own to any block's range at all (see
        // `md_render_diff_tests`'s own module doc comment).
        let doc = Doc::parse("See [the docs][ref] here.\n\n[ref]: https://example.com/docs\n");
        let BlockKind::Paragraph { inline } = &doc.blocks[0].kind else {
            panic!("expected a paragraph")
        };
        assert_eq!(inline_plain_text(&doc, inline), "See the docs here.");

        // Control: a dangling reference (no definition anywhere) reports the literal bracket text
        // instead of a `Link`'s label — this reconstruction shows exactly that difference, rather
        // than merely asserting the two cases differ.
        let dangling = Doc::parse("See [the docs][missing] here.\n");
        let BlockKind::Paragraph { inline } = &dangling.blocks[0].kind else {
            panic!("expected a paragraph")
        };
        assert_eq!(
            inline_plain_text(&dangling, inline),
            "See [the docs][missing] here."
        );
    }

    #[test]
    fn fenced_code_block_body_excludes_fence_and_info_string() {
        let src = "```rust\nfn a() {}\nfn b() {}\n```\n";
        let doc = Doc::parse(src);
        let BlockKind::CodeBlock {
            lang,
            fenced,
            body_spans,
        } = &doc.blocks[0].kind
        else {
            panic!("expected a code block")
        };
        assert_eq!(lang.as_deref(), Some("rust"));
        assert!(fenced);
        assert_eq!(
            code_body_text(body_spans, src),
            "fn a() {}\nfn b() {}",
            "content, with the one trailing newline stripped"
        );
    }

    #[test]
    fn fenced_code_block_with_no_info_string_has_no_lang() {
        let doc = Doc::parse("```\nplain\n```\n");
        let BlockKind::CodeBlock { lang, fenced, .. } = &doc.blocks[0].kind else {
            panic!("expected a code block")
        };
        assert_eq!(*lang, None);
        assert!(fenced);
    }

    #[test]
    fn indented_code_block_is_not_fenced_and_has_no_lang() {
        let doc = Doc::parse("para\n\n    line one\n    line two\n");
        let BlockKind::CodeBlock { lang, fenced, .. } = &doc.blocks[1].kind else {
            panic!("expected a code block")
        };
        assert_eq!(*lang, None);
        assert!(!fenced);
    }

    #[test]
    fn empty_fenced_code_block_has_no_body_spans() {
        let src = "```\n```\n";
        let doc = Doc::parse(src);
        let BlockKind::CodeBlock { body_spans, .. } = &doc.blocks[0].kind else {
            panic!("expected a code block")
        };
        assert_eq!(
            body_spans,
            &Vec::<Range<usize>>::new(),
            "an empty code block reports no Event::Text spans at all"
        );
        assert_eq!(code_body_text(body_spans, src), "");
    }

    #[test]
    fn list_children_are_items_and_ordered_lists_report_their_start() {
        let doc = Doc::parse("5. a\n6. b\n");
        assert_eq!(doc.blocks.len(), 1);
        let BlockKind::List { ordered, start } = doc.blocks[0].kind else {
            panic!("expected a list")
        };
        assert!(ordered);
        assert_eq!(start, Some(5));
        assert_eq!(doc.blocks[0].children.len(), 2);
        for item in &doc.blocks[0].children {
            assert!(matches!(item.kind, BlockKind::ListItem { task: None }));
        }
    }

    #[test]
    fn bullet_list_is_unordered_with_no_start() {
        let doc = Doc::parse("- a\n- b\n");
        let BlockKind::List { ordered, start } = doc.blocks[0].kind else {
            panic!("expected a list")
        };
        assert!(!ordered);
        assert_eq!(start, None);
    }

    #[test]
    fn tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one() {
        // Tight item: pulldown-cmark emits the item's own text with no enclosing `Paragraph` at
        // all — `parse_blocks` still gives the model somewhere to represent it, coalescing the
        // stray inline run into a synthetic `Paragraph` (`collect_stray_inline_run`) rather than
        // leaving the item with no children at all (an earlier version of this model did exactly
        // that; see `BlockKind::ListItem`'s doc comment).
        let src = "- a\n- b\n";
        let tight = Doc::parse(src);
        assert_eq!(tight.blocks[0].children[0].children.len(), 1);
        let synthetic = &tight.blocks[0].children[0].children[0];
        let BlockKind::Paragraph { inline } = &synthetic.kind else {
            panic!("expected a synthetic paragraph")
        };
        assert_eq!(inline_plain_text(&tight, inline), "a");
        assert_eq!(&src[synthetic.src.clone()], "a");

        // Loose item: pulldown-cmark itself wraps the text in a real `Paragraph`, so there is no
        // coalescing to do — the model just represents what the parser already reported. Note the
        // real `Paragraph`'s own range includes the trailing newline ("a\n") the way pulldown-cmark
        // always reports a `Paragraph`'s range, while the synthetic one above does not (it is built
        // purely from `Event::Text`'s own, narrower range) — a real difference in convention between
        // the two, not a bug; see `collect_stray_inline_run`'s doc comment.
        let loose_src = "- a\n\n- b\n";
        let loose = Doc::parse(loose_src);
        assert_eq!(loose.blocks[0].children[0].children.len(), 1);
        let real = &loose.blocks[0].children[0].children[0];
        let BlockKind::Paragraph { inline } = &real.kind else {
            panic!("expected a real paragraph")
        };
        assert_eq!(inline_plain_text(&loose, inline), "a");
        assert_eq!(&loose_src[real.src.clone()], "a\n");
    }

    #[test]
    fn task_marker_state_and_position_are_exact() {
        let src = "- [ ] a\n- [x] b\n- [X] c\n";
        let doc = Doc::parse(src);
        let states: Vec<(char, usize)> = doc.blocks[0]
            .children
            .iter()
            .map(|item| {
                let BlockKind::ListItem { task } = &item.kind else {
                    panic!("expected a list item")
                };
                let t = task.expect("expected a task marker");
                (t.state, t.state_at)
            })
            .collect();
        assert_eq!(states, vec![(' ', 3), ('x', 11), ('X', 19)]);
        for (_, at) in &states {
            assert_eq!(src.as_bytes()[at - 1], b'[');
            assert_eq!(src.as_bytes()[at + 1], b']');
        }
    }

    #[test]
    fn loose_task_item_task_marker_is_still_captured() {
        // Gap A: a *loose* item's `Event::TaskListMarker` sits inside the item's own first
        // `Paragraph`, not directly after `Start(Item)` the way a tight item's does — see
        // `parse_item_task_and_children`'s doc comment. An earlier version of this model only ever
        // peeked right after `Start(Item)`, so a loose task item's marker was silently dropped
        // (`task: None`) even though pulldown-cmark reported it.
        let src = "- [ ] outer\n\n  - [ ] nested at matching indent\n";
        let doc = Doc::parse(src);
        let outer = &doc.blocks[0].children[0];
        let BlockKind::ListItem { task } = &outer.kind else {
            panic!("expected a list item")
        };
        let t = task.expect("expected the outer (loose) item's task marker to be captured");
        assert_eq!((t.state, t.state_at), (' ', 3));
        // The item's own first child is still its real `Paragraph` (loose items get one) — the
        // marker being captured on the side does not change that, and the marker's own three bytes
        // (`[`, the state char, `]`) must not leak into the paragraph's own `inline` range either.
        let BlockKind::Paragraph { inline } = &outer.children[0].kind else {
            panic!("expected the loose item's own paragraph")
        };
        assert_eq!(inline_plain_text(&doc, inline), "outer");

        // The nested item's own marker (necessarily a *tight* one — a lone item can't itself be
        // loose) is captured too, with no leakage between the outer item's own marker lookup and
        // the inner one's independent call to the same function.
        let nested_list = &outer.children[1];
        let nested_item = &nested_list.children[0];
        let BlockKind::ListItem {
            task: nested_task, ..
        } = &nested_item.kind
        else {
            panic!("expected the nested list item")
        };
        let nested = nested_task.expect("expected the nested item's own marker to be captured");
        assert_eq!(nested.state, ' ');
    }

    #[test]
    fn tight_list_item_starting_with_inline_formatting_is_still_captured() {
        // Gap B, the harder case: a tight item whose content does not start with plain text at all
        // — pulldown-cmark can hand `parse_blocks` the `Start` of an inline construct
        // (`Start(Strong)`/`Start(Link)`/...) directly, with no leading `Text` event to notice
        // first. `collect_stray_inline_run` has to balance that construct's own subtree
        // (`skip_inline_to`) for real, not just record its range — an earlier version of this
        // function forgot to do that for the *first* event specifically, desyncing the event
        // stream on exactly this shape (see its own doc comment): every block appearing anywhere
        // after this item in that earlier version came out with corrupted, overlapping ranges.
        let src = "- **bold** and *em* and `code` end\n";
        let doc = Doc::parse(src);
        let item = &doc.blocks[0].children[0];
        assert_eq!(item.children.len(), 1);
        let synthetic = &item.children[0];
        let BlockKind::Paragraph { inline } = &synthetic.kind else {
            panic!("expected a synthetic paragraph")
        };
        assert_eq!(
            &src[synthetic.src.clone()],
            "**bold** and *em* and `code` end"
        );
        // `collect_stray_inline_run`'s own event-desync bug (see above) would have corrupted this
        // range too — the plain-text reconstruction from `doc.events[inline]` is a second, coarser
        // proof the run's own events line up correctly, on top of the byte-range check above.
        assert_eq!(inline_plain_text(&doc, inline), "bold and em and code end");
    }

    #[test]
    fn non_task_list_item_has_no_task() {
        let doc = Doc::parse("- plain\n");
        let BlockKind::ListItem { task } = doc.blocks[0].children[0].kind else {
            panic!("expected a list item")
        };
        assert_eq!(task, None);
    }

    #[test]
    fn custom_task_state_char_is_not_recognized_as_a_task_by_pulldown_cmark() {
        // Documents the limitation `Task`'s doc comment describes: `[/]` is not GFM task syntax,
        // so no `Event::TaskListMarker` fires and this model reports no task at all for it.
        let doc = Doc::parse("- [/] in progress\n");
        let BlockKind::ListItem { task } = doc.blocks[0].children[0].kind else {
            panic!("expected a list item")
        };
        assert_eq!(
            task, None,
            "pulldown-cmark does not emit TaskListMarker for a custom state"
        );
    }

    #[test]
    fn block_quote_alert_kind_uses_the_real_alert_header_parser() {
        let doc = Doc::parse("> [!WARNING] Heads up\n> body\n");
        let BlockKind::Quote { alert } = doc.blocks[0].kind else {
            panic!("expected a quote")
        };
        assert_eq!(alert, Some(AlertKind::Warning));
    }

    #[test]
    fn plain_block_quote_has_no_alert_kind() {
        let doc = Doc::parse("> just a quote\n");
        let BlockKind::Quote { alert } = doc.blocks[0].kind else {
            panic!("expected a quote")
        };
        assert_eq!(alert, None);
    }

    #[test]
    fn table_rows_have_header_first_then_body_with_correct_alignment() {
        let src = "| a | b |\n|:--|--:|\n| 1 | 2 |\n| 3 | 4 |\n";
        let doc = Doc::parse(src);
        let BlockKind::Table { aligns, rows } = &doc.blocks[0].kind else {
            panic!("expected a table")
        };
        assert_eq!(aligns, &[Alignment::Left, Alignment::Right]);
        assert_eq!(rows.len(), 3, "1 header row + 2 body rows");
        assert_eq!(&src[rows[0][0].clone()], " a ");
        assert_eq!(&src[rows[0][1].clone()], " b ");
        assert_eq!(&src[rows[1][0].clone()], " 1 ");
        assert_eq!(&src[rows[2][1].clone()], " 4 ");
    }

    #[test]
    fn table_with_no_alignment_colons_reports_none_not_left() {
        // Distinguishes this from the render pipeline's own `ColAlign`, which has no "unspecified"
        // state and would default this to `Left` for display — the model keeps CommonMark's real
        // answer (see `BlockKind::Table`'s doc comment).
        let doc = Doc::parse("| a |\n|---|\n| 1 |\n");
        let BlockKind::Table { aligns, .. } = &doc.blocks[0].kind else {
            panic!("expected a table")
        };
        assert_eq!(aligns, &[Alignment::None]);
    }

    #[test]
    fn html_block_tag_name_is_read_from_the_opening_line() {
        let doc = Doc::parse("<div class=\"x\">\nhello\n</div>\n");
        let BlockKind::Html { tag } = &doc.blocks[0].kind else {
            panic!("expected an html block")
        };
        assert_eq!(tag.as_deref(), Some("div"));
    }

    #[test]
    fn html_comment_block_has_no_tag_name() {
        let doc = Doc::parse("<!-- a comment -->\n");
        let BlockKind::Html { tag } = &doc.blocks[0].kind else {
            panic!("expected an html block")
        };
        assert_eq!(*tag, None);
    }

    #[test]
    fn details_and_summary_and_closing_details_are_all_identified_via_the_shared_predicates() {
        let doc = Doc::parse("<details>\n<summary>S</summary>\n\nbody\n\n</details>\n");
        // pulldown-cmark groups the two opening lines into one HtmlBlock (see `BlockKind::Html`'s
        // doc comment on why this differs from `split_details`'s own "one folded block" view).
        assert_eq!(
            doc.blocks.len(),
            3,
            "opening html block, paragraph, closing html block"
        );
        let BlockKind::Html { tag } = &doc.blocks[0].kind else {
            panic!("expected an html block")
        };
        assert_eq!(tag.as_deref(), Some("details"));
        let BlockKind::Html { tag } = &doc.blocks[2].kind else {
            panic!("expected an html block")
        };
        assert_eq!(
            tag.as_deref(),
            Some("details"),
            "the standalone </details> line too"
        );
    }

    #[test]
    fn a_tag_name_prefix_is_not_confused_with_details_itself() {
        let doc = Doc::parse("<detailsx>\nhi\n</detailsx>\n");
        let BlockKind::Html { tag } = &doc.blocks[0].kind else {
            panic!("expected an html block")
        };
        assert_eq!(
            tag.as_deref(),
            Some("detailsx"),
            "not truncated down to \"details\""
        );
    }

    #[test]
    fn thematic_break_is_reported_with_its_own_source_range() {
        let src = "a\n\n---\n\nb\n";
        let doc = Doc::parse(src);
        assert!(matches!(doc.blocks[1].kind, BlockKind::ThematicBreak));
        assert_eq!(&src[doc.blocks[1].src.clone()], "---\n");
    }

    #[test]
    fn nested_list_inside_a_list_item_is_a_child_block_not_flattened() {
        let doc = Doc::parse("- a\n  - b\n");
        let outer_item = &doc.blocks[0].children[0];
        // The outer item's own tight inline text ("a") is a synthetic `Paragraph` ahead of the
        // nested list — see
        // `tight_list_item_gets_a_synthetic_paragraph_child_loose_item_gets_a_real_one` — not
        // flattened into, or dropped in favor of, the nested list either way.
        assert_eq!(outer_item.children.len(), 2);
        assert!(matches!(
            outer_item.children[0].kind,
            BlockKind::Paragraph { .. }
        ));
        assert!(matches!(
            outer_item.children[1].kind,
            BlockKind::List { .. }
        ));
    }

    #[test]
    fn code_block_nested_inside_a_list_item_is_a_real_child_block() {
        // The render pipeline's `parser_code_blocks` deliberately does *not* count this one (its
        // `quote_depth` gate only excludes block-quote nesting, list nesting is fine for it) — this
        // just pins that the model represents it as a genuine nested CodeBlock either way.
        let doc = Doc::parse("- item\n\n  ```rust\n  code\n  ```\n");
        let item = &doc.blocks[0].children[0];
        let code = item
            .children
            .iter()
            .find(|b| matches!(b.kind, BlockKind::CodeBlock { .. }))
            .expect("expected a nested code block");
        assert!(matches!(
            code.kind,
            BlockKind::CodeBlock { fenced: true, .. }
        ));
    }

    #[test]
    fn cjk_ranges_land_on_char_boundaries_and_slice_correctly() {
        let src = "# 見出し\n\n```rust\nfn 関数() {}\n```\n";
        let doc = Doc::parse(src);
        assert_eq!(&src[doc.blocks[0].src.clone()], "# 見出し\n");
        let BlockKind::Heading { inline, .. } = &doc.blocks[0].kind else {
            panic!("expected a heading")
        };
        assert_eq!(inline_plain_text(&doc, inline), "見出し");
        let BlockKind::CodeBlock { body_spans, .. } = &doc.blocks[1].kind else {
            panic!("expected a code block")
        };
        for r in body_spans {
            assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end));
        }
        assert_eq!(code_body_text(body_spans, src), "fn 関数() {}");
    }

    #[test]
    fn empty_document_has_no_blocks() {
        assert_eq!(Doc::parse("").blocks, Vec::new());
    }

    // ---- invariants, checked over the full parity corpus ------------------------------------

    /// Recursively checks, for every block in `blocks` (whose own extent, if any, is `parent`):
    /// every range lands on char boundaries and inside the input; siblings are in non-decreasing,
    /// non-overlapping source order; every child's range is contained in its parent's; a
    /// `CodeBlock`'s `body_spans` are each contained in its own `src`, land on char boundaries, and
    /// are themselves in non-decreasing, non-overlapping order; a `Heading`/`Paragraph`'s own
    /// `inline` index range is well-formed and inside `[0, events_len]` (`events_len` is
    /// `Doc.events.len()` for the whole document this block came from — every `inline` range, at any
    /// nesting depth, indexes into that same single `Vec`, not a per-block one); and a `ListItem`'s
    /// `Task`, if any, has its bracket invariant (`state_at`'s doc comment) hold. Returns every
    /// violation found (empty on success) rather than asserting inline, so one test run reports
    /// everything wrong at once instead of stopping at the first failure.
    fn check_invariants(
        blocks: &[Block],
        src: &str,
        events_len: usize,
        parent: Option<&Range<usize>>,
        path: &str,
        out: &mut Vec<String>,
    ) {
        let mut prev_end: Option<usize> = None;
        for (i, b) in blocks.iter().enumerate() {
            let here = format!("{path}[{i}]");
            if b.src.start > b.src.end {
                out.push(format!("{here}: src.start > src.end ({:?})", b.src));
            }
            if b.src.end > src.len() {
                out.push(format!(
                    "{here}: src.end past the input's length ({:?})",
                    b.src
                ));
            } else if !src.is_char_boundary(b.src.start) || !src.is_char_boundary(b.src.end) {
                out.push(format!("{here}: src {:?} is not on a char boundary", b.src));
            }
            if let Some(pe) = prev_end {
                if b.src.start < pe {
                    out.push(format!(
                        "{here}: overlaps the previous sibling (starts at {} before it ends at {pe})",
                        b.src.start
                    ));
                }
            }
            if let Some(p) = parent {
                if b.src.start < p.start || b.src.end > p.end {
                    out.push(format!("{here}: src {:?} escapes its parent {p:?}", b.src));
                }
            }
            if let BlockKind::CodeBlock { body_spans, .. } = &b.kind {
                let mut prev_span_end: Option<usize> = None;
                for (si, span) in body_spans.iter().enumerate() {
                    let here = format!("{here}.body_spans[{si}]");
                    if span.start > span.end {
                        out.push(format!("{here}: start > end ({span:?})"));
                    } else if span.end > src.len() {
                        out.push(format!("{here}: end past the input's length ({span:?})"));
                    } else if !src.is_char_boundary(span.start) || !src.is_char_boundary(span.end) {
                        out.push(format!("{here}: {span:?} is not on a char boundary"));
                    } else if span.start < b.src.start || span.end > b.src.end {
                        out.push(format!(
                            "{here}: {span:?} escapes its own block's src {:?}",
                            b.src
                        ));
                    }
                    if let Some(pe) = prev_span_end {
                        if span.start < pe {
                            out.push(format!(
                                "{here}: overlaps the previous span (starts at {} before it ends at {pe})",
                                span.start
                            ));
                        }
                    }
                    prev_span_end = Some(span.end);
                }
            }
            if let BlockKind::ListItem { task: Some(t) } = &b.kind {
                if !src.is_char_boundary(t.state_at) {
                    out.push(format!(
                        "{here}: task.state_at {} is not on a char boundary",
                        t.state_at
                    ));
                } else if !src[t.state_at..].starts_with(t.state) {
                    out.push(format!(
                        "{here}: task.state_at {} does not point at task.state {:?}",
                        t.state_at, t.state
                    ));
                } else if src.as_bytes().get(t.state_at.wrapping_sub(1)) != Some(&b'[')
                    || src.as_bytes().get(t.state_at + 1) != Some(&b']')
                {
                    out.push(format!(
                        "{here}: task.state_at {} is not bracketed by '[' and ']'",
                        t.state_at
                    ));
                }
            }
            let inline = match &b.kind {
                BlockKind::Heading { inline, .. } => Some(inline),
                BlockKind::Paragraph { inline } => Some(inline),
                _ => None,
            };
            if let Some(inline) = inline {
                if inline.start > inline.end {
                    out.push(format!("{here}: inline.start > inline.end ({inline:?})"));
                } else if inline.end > events_len {
                    out.push(format!(
                        "{here}: inline.end {} past events_len {events_len}",
                        inline.end
                    ));
                }
            }
            check_invariants(&b.children, src, events_len, Some(&b.src), &here, out);
            prev_end = Some(b.src.end);
        }
    }

    /// Every raw and preprocessed case in the same parity corpus `md_snapshot_tests` covers
    /// (`task_corpus`/`code_corpus`/`code_span_corpus`/`preprocess_corpus`), both as written and
    /// after the same front-matter/footnote/inline-HTML pre-passes the render pipeline itself
    /// applies before any of this text reaches a parser. Checking both is deliberate: the raw form
    /// exercises constructs the pre-passes remove entirely (front matter, footnote definitions)
    /// that would otherwise never be modeled at all, and the preprocessed form is what this
    /// module's own contract (see the module doc comment) actually promises to handle correctly.
    fn all_corpus_texts() -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        for (name, src) in super::super::task_corpus::cases() {
            v.push((format!("task_corpus: {name}"), src.to_string()));
        }
        for (name, src) in super::super::code_corpus::cases() {
            v.push((format!("code_corpus: {name}"), src.to_string()));
        }
        for case in super::super::code_span_corpus::cases() {
            v.push((format!("code_span_corpus: {}", case.name), case.src));
        }
        for (name, src) in super::super::preprocess_corpus::cases() {
            v.push((format!("preprocess_corpus: {name}"), src.to_string()));
        }
        let mut with_preprocessing: Vec<(String, String)> = Vec::new();
        for (name, raw) in &v {
            with_preprocessing.push((format!("{name} [preprocessed]"), preprocess_default(raw)));
        }
        v.extend(with_preprocessing);
        v
    }

    /// The default-config equivalent of `md_snapshot_tests::pre_src_for` — front matter stripped,
    /// then footnotes, then inline HTML, unconditionally (matching `Config::default()`, under which
    /// all three are on) — reimplemented here, rather than depending on `crate::app`/`Config` from
    /// this module's own test suite, so this file's tests stay self-contained within
    /// `preview::markdown`. `md_model_snapshot_tests` (in `src/app/`) uses the real
    /// `pre_src_for` directly instead, and the two are required to agree — see that module's doc
    /// comment.
    fn preprocess_default(src: &str) -> String {
        let (_front_matter, body) = super::super::strip_front_matter(src);
        let origin = super::super::identity_origin(&body);
        let (s, origin) = super::super::process_footnotes_traced(&body, &origin);
        let (pre_src, _origin) = super::super::process_inline_html_traced(&s, &origin);
        pre_src
    }

    #[test]
    fn model_invariants_hold_across_the_full_parity_corpus() {
        let mut violations = Vec::new();
        for (name, src) in all_corpus_texts() {
            let doc = Doc::parse(&src);
            check_invariants(
                &doc.blocks,
                &src,
                doc.events.len(),
                None,
                &name,
                &mut violations,
            );
        }
        assert!(
            violations.is_empty(),
            "{} invariant violation(s):\n{}",
            violations.len(),
            violations.join("\n")
        );
    }

    #[test]
    fn model_invariants_hold_across_the_sample_md_files() {
        // `samples/` is excluded from the published crate (see `Cargo.toml`) — degrade to "skip"
        // rather than fail a build from an extracted tarball, mirroring
        // `md_snapshot_tests::sample_src`'s own contract.
        let mut violations = Vec::new();
        let mut checked = 0usize;
        for name in [
            "images.md",
            "links.md",
            "links.ja.md",
            "markdown.md",
            "markdown.ja.md",
            "README.md",
            "tutorial.md",
            "tutorial.ja.md",
        ] {
            let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("samples")
                .join(name);
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            checked += 1;
            let pre = preprocess_default(&raw);
            let doc = Doc::parse(&pre);
            check_invariants(
                &doc.blocks,
                &pre,
                doc.events.len(),
                None,
                name,
                &mut violations,
            );
        }
        if checked == 0 {
            eprintln!("model_invariants_hold_across_the_sample_md_files: no samples/*.md found (published-crate build?) — skipping");
            return;
        }
        assert!(
            violations.is_empty(),
            "{} invariant violation(s) across {checked} sample file(s):\n{}",
            violations.len(),
            violations.join("\n")
        );
    }

    // ---- cross-check against `parser_code_blocks` --------------------------------------------

    /// Recursively collects `(is_inside_a_quote, body_spans)` for every `CodeBlock` in `blocks`, in
    /// source order — `inside_quote` true for any block whose nearest ancestor is (or is nested
    /// inside) a `BlockKind::Quote`, matching `parser_code_blocks`'s own `quote_depth > 0` gate
    /// exactly (both increment/decrement, respectively track, at precisely the same `BlockQuote`
    /// `Start`/`End` events — this model via nesting, `parser_code_blocks` via a counter). See
    /// `BlockKind::CodeBlock.body_spans`'s doc comment for why this model deliberately does **not**
    /// filter these out itself the way `parser_code_blocks` does — `inside_quote` is reported here
    /// precisely so the tests below can apply that same filter before comparing.
    fn code_block_bodies(
        blocks: &[Block],
        inside_quote: bool,
        out: &mut Vec<(bool, Vec<Range<usize>>)>,
    ) {
        for b in blocks {
            match &b.kind {
                BlockKind::CodeBlock { body_spans, .. } => {
                    out.push((inside_quote, body_spans.clone()))
                }
                BlockKind::Quote { .. } => code_block_bodies(&b.children, true, out),
                _ => code_block_bodies(&b.children, inside_quote, out),
            }
        }
    }

    /// An independent walk of the same event stream `Doc::parse` reads, joining each code block's
    /// `Event::Text` *contents* (rather than deriving a `Range` from their positions) and tagging
    /// block-quote nesting exactly like `code_block_bodies` above — the same shape as
    /// `parser_code_blocks` itself (same options, same `Start`/`End(CodeBlock)` bracketing, same
    /// join, same trailing-`\n` strip), except it reports quote nesting instead of filtering by it.
    /// Existing on purpose as *test* code, not production: its whole job is to be a second,
    /// differently-shaped computation of the same thing, used two ways below —
    /// `code_blocks_match_parser_code_blocks_outside_block_quotes_across_the_corpus` filters its own
    /// results by `!inside_quote` and checks that reproduces `parser_code_blocks`'s real output
    /// exactly (confirming `parser_code_blocks` itself behaves as documented, independent of
    /// `Doc::parse` entirely), and `model_body_ranges_are_internally_consistent_with_parser_code_blocks_quote_filtering`
    /// cross-checks `Doc::parse`'s tree against it on block *count* and quote nesting, not content.
    /// The exhaustive content check
    /// (`code_body_text_matches_parser_code_blocks_for_every_non_quote_code_block_in_the_corpus`)
    /// compares `Doc::parse`'s reconstructed `body_spans` directly against `parser_code_blocks`
    /// instead, without going through this function at all.
    fn joined_code_block_texts(src: &str) -> Vec<(bool, String)> {
        let mut out = Vec::new();
        let mut quote_depth = 0usize;
        let mut in_quote_at_open = false;
        let mut body: Option<String> = None;
        for (ev, _) in Parser::new_ext(src, parse_options()).into_offset_iter() {
            match ev {
                Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
                Event::End(TagEnd::BlockQuote(_)) => quote_depth = quote_depth.saturating_sub(1),
                Event::Start(Tag::CodeBlock(_)) => {
                    in_quote_at_open = quote_depth > 0;
                    body = Some(String::new());
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some(b) = body.take() {
                        let trimmed = b.strip_suffix('\n').unwrap_or(&b).to_string();
                        out.push((in_quote_at_open, trimmed));
                    }
                }
                Event::Text(t) => {
                    if let Some(b) = body.as_mut() {
                        b.push_str(&t);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A parity check on `parser_code_blocks` itself, independent of `Doc::parse`: over the whole
    /// corpus, joining `Event::Text` straight off a fresh event walk (`joined_code_block_texts`,
    /// filtered to non-quote-nested blocks) reproduces exactly what `parser_code_blocks` computes —
    /// confirming the render pipeline's own scanner behaves as documented, before the tests below
    /// ask whether `Doc::parse`'s tree agrees with it too.
    #[test]
    fn code_blocks_match_parser_code_blocks_outside_block_quotes_across_the_corpus() {
        let mut total_checked = 0usize;
        for (name, src) in all_corpus_texts() {
            let mut expected = Vec::new();
            super::super::parser_code_blocks(&src, &mut expected);
            let actual: Vec<String> = joined_code_block_texts(&src)
                .into_iter()
                .filter(|(in_quote, _)| !in_quote)
                .map(|(_, text)| text)
                .collect();
            assert_eq!(actual, expected, "case {name:?}");
            total_checked += expected.len();
        }
        assert!(
            total_checked > 20,
            "the corpus's non-quote-nested code block count looks suspiciously small \
             ({total_checked}) — a corpus-gathering call probably broke"
        );
    }

    /// `Doc::parse`'s own tree — walked via `code_block_bodies` above — finds the same *number* of
    /// code blocks as a fresh, independent event walk (`joined_code_block_texts`), and agrees with
    /// it on which ones are block-quote-nested, for every case in the corpus, regardless of how many
    /// `Event::Text` spans any individual block took to build. A count/nesting check, not a content
    /// one — see the next test for the exhaustive content comparison against `parser_code_blocks`
    /// itself.
    #[test]
    fn model_body_ranges_are_internally_consistent_with_parser_code_blocks_quote_filtering() {
        for (name, src) in all_corpus_texts() {
            let doc = Doc::parse(&src);
            let mut model = Vec::new();
            code_block_bodies(&doc.blocks, false, &mut model);
            let joined = joined_code_block_texts(&src);
            assert_eq!(
                model.len(),
                joined.len(),
                "case {name:?}: model found {} code blocks, the independent walk found {}",
                model.len(),
                joined.len()
            );
            for (i, ((model_in_quote, _), (joined_in_quote, _))) in
                model.iter().zip(joined.iter()).enumerate()
            {
                assert_eq!(
                    model_in_quote, joined_in_quote,
                    "case {name:?} block #{i}: model and the independent walk disagree about quote nesting"
                );
            }
        }
    }

    /// The primary parity check this module exists for: over the whole corpus, reconstructing every
    /// **non-quote-nested** code block's content from `Doc::parse`'s own `body_spans` (via
    /// `code_body_text`) matches `parser_code_blocks` — the render pipeline's real write-back
    /// scanner — byte for byte. No exceptions: unlike an earlier version of this test, this does not
    /// carve out a "single `Event::Text` span" case and merely tally the rest; every code block in
    /// the corpus, however many spans its content took to build (an indented block, a block nested
    /// inside a list item or block quote, a block in a CRLF file, ...), is checked the same way. See
    /// `BlockKind::CodeBlock.body_spans`'s doc comment for why quote-nested blocks are excluded here
    /// specifically (`parser_code_blocks` itself never counts them — this model still models them,
    /// deliberately, as a superset) rather than a limitation of the reconstruction being tested.
    #[test]
    fn code_body_text_matches_parser_code_blocks_for_every_non_quote_code_block_in_the_corpus() {
        let mut total_checked = 0usize;
        for (name, src) in all_corpus_texts() {
            let mut expected = Vec::new();
            super::super::parser_code_blocks(&src, &mut expected);

            let doc = Doc::parse(&src);
            let mut model = Vec::new();
            code_block_bodies(&doc.blocks, false, &mut model);
            let actual: Vec<String> = model
                .into_iter()
                .filter(|(in_quote, _)| !in_quote)
                .map(|(_, spans)| code_body_text(&spans, &src))
                .collect();

            assert_eq!(actual, expected, "case {name:?}");
            total_checked += expected.len();
        }
        assert!(
            total_checked > 20,
            "the corpus's non-quote-nested code block count looks suspiciously small \
             ({total_checked}) — a corpus-gathering call probably broke"
        );
    }

    // ---- determinism --------------------------------------------------------------------------

    #[test]
    fn parsing_the_same_text_twice_gives_the_same_tree() {
        for (_, src) in all_corpus_texts() {
            assert_eq!(Doc::parse(&src), Doc::parse(&src));
        }
    }
}
