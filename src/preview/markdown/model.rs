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
    /// that line — not a second classifier of this module's own (see `alert_kind_of`). `alert_title`
    /// is that same call's own second return value, the header's optional Obsidian-style trailing
    /// title (`> [!NOTE] My title` → `"My title"`) — kept as a sibling field rather than folded into
    /// `alert` itself (`Option<(AlertKind, String)>`) so every existing `alert`-only match arm needs
    /// only `..` added, not a payload-shape rewrite. Empty whenever `alert` is `None` (not an alert
    /// at all) or the header carried no title text — `alert_kind_of` never invents one.
    Quote {
        alert: Option<AlertKind>,
        alert_title: String,
    },
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
    /// Note pulldown-cmark does not merge a `<details>`/`<summary>` pair and the later `</details>`
    /// into one block the way `split_details` (the renderer's own peel) does — CommonMark only glues
    /// *consecutive, blank-line-free* HTML-tag lines into one `HtmlBlock`, so `<details>`,
    /// `<summary>...</summary>`, and (once the body paragraph in between closes it) the standalone
    /// `</details>` still parse here as three separate top-level blocks, exactly as this once-flat
    /// doc comment described. `fold_details` (run once, at every `parse_blocks` return point — see
    /// its own doc comment) folds a **well-formed** instance of that shape — a `<details ...>`
    /// opening tag whose closing `</details>` later shows up as its own, independent `Html` leaf
    /// (blank-line-separated on both sides, the shape every real-world `<details>` block in this
    /// codebase's own samples and every diff-harness case that now matches production uses) — into
    /// `BlockKind::Details`, matching `split_details`'s own "one folded block" view without a second
    /// parse. A `Details` block's own `Block::children` is exactly the sibling blocks `fold_details`
    /// found sitting between that open tag and its matching close, in source order — the same
    /// content a caller reading `split_details`'s own `body` string would get, structured instead of
    /// flat.
    ///
    /// A **pathological or malformed** shape a raw-line scanner (`split_details`) can decompose but
    /// this model's own, real-parser-driven segmentation cannot — most commonly, `<details>` and
    /// `<summary>...</summary>` (or a nested `<details>`) glued onto the very same `HtmlBlock` with
    /// **no** blank line anywhere inside the whole construct, so pulldown-cmark itself never reports
    /// a separate sibling block for `fold_details` to search for a close among at all — still parses
    /// as a plain, unfolded `Html` leaf here, exactly as before this variant existed. This is not a
    /// silent content loss: a caller (`render.rs`'s own `contains_unsupported`) that does not know
    /// how to draw a raw `Html` leaf already reports the *whole* enclosing construct as out of scope
    /// rather than rendering something incomplete — see that function's own doc comment.
    ///
    /// `body_spans` names this block's own content the identical way `CodeBlock.body_spans` does —
    /// one byte range per `Event::Html` pulldown-cmark reports, in source order, rather than a single
    /// range into `src` — and for the identical reason: `Block::src` (the whole `HtmlBlock`, fence-
    /// to-fence) is one un-split byte range, so inside a block quote its later lines still carry that
    /// quote's own `>` marker verbatim, while pulldown-cmark's own per-line `Event::Html` ranges do
    /// not (confirmed directly, the same way `CodeBlock.body_spans`'s own doc comment describes for
    /// `Event::Text`: parsing `"> <div>\n> body\n> </div>\n"` reports `Start(HtmlBlock)` at
    /// `2..len`, its slice still `>`-prefixed on every line but the first, while each `Event::Html`
    /// range — `2..23`, `25..43`, `45..52` for that exact input — already excludes the marker). Empty
    /// for a block pulldown-cmark reports as `HtmlBlock` with no `Event::Html` at all (not observed in
    /// practice — every CommonMark HTML-block type has at least one line of content by construction —
    /// but not assumed away either, the same defensive stance `collect_code_body_spans` takes).
    /// `html_body_text(&body_spans, src)` reconstructs the content; see its own doc comment for
    /// exactly what "reconstructs" means here (unlike `code_body_text`, no trailing-newline stripping
    /// — see that function's own doc comment for why the two conventions differ).
    Html {
        tag: Option<String>,
        body_spans: Vec<Range<usize>>,
    },
    /// A `<details>` … `</details>` block, folded from what `Doc::parse` itself would otherwise have
    /// reported for its opening and closing tags (see `Html`'s own doc comment on exactly which
    /// shapes qualify, and `fold_details` for the folding algorithm). Two different shapes fold into
    /// this one variant — see `glued_body`'s own doc comment for the second:
    ///
    /// * **Well-formed** (open tag, body, close tag each its own sibling `Html` leaf, blank-line
    ///   separated): `Block::src` spans from the opening tag's own first byte through the closing
    ///   tag's own last byte (or through the end of input, when unclosed — matching `split_details`'s
    ///   own `close.unwrap_or(lines.len())` fallback), and `Block::children` is the sibling blocks
    ///   that sat between them, in source order — the block's own rendered body, already structured,
    ///   with no second parse of any substring needed to read it. `glued_body` is `None`.
    Details {
        /// The `open` HTML attribute this block's own `<details ...>` tag carried
        /// (`details_open_tag`'s own return value) — the document's own *declared* default, not a
        /// runtime toggle state: whether a given `Details` block is actually expanded or collapsed
        /// on screen is a per-render decision the render pipeline makes elsewhere (see
        /// `crate::preview::markdown::next_details_open`'s own doc comment), which this model has no
        /// opinion about and does not track.
        open_attr: bool,
        /// The `<summary>...</summary>` label text, tags stripped, computed by handing the render
        /// pipeline's own `extract_summary_body` the exact same byte range `split_details` itself
        /// would hand it for identical source text (see `fold_details`'s own doc comment for exactly
        /// which range that is) — not a second, model-side reimplementation of that extraction.
        /// Empty when the block has no `<summary>` tag at all, matching `extract_summary_body`'s own
        /// "no summary" fallback.
        summary: String,
        /// Byte range, into `Doc::parse`'s own whole-document `src`, of this block's own **glued**
        /// body — set only for the second, pathological shape `fold_details` also recognizes (see
        /// `glued_details_fold`'s own doc comment for exactly which glued shapes qualify, and which
        /// stay an ordinary, unfolded `Html` leaf instead): the open tag, its own `<summary>`, and its
        /// own close all landed in the *same*, single, literal `Html` leaf (no blank line anywhere in
        /// the whole construct — see `Html`'s own doc comment), so pulldown-cmark's one whole-document
        /// walk never reported any block-level events for the body at all — an HTML block's own
        /// interior is copied *literally*, per spec, so there is no finer structure in `Doc.events`
        /// for `Block::children` to name a slice of, the way a well-formed `<details>`'s body always
        /// can. `Block::children` is always empty exactly when this is `Some` (and never empty when
        /// this is `None` for anything but a genuinely empty well-formed body) — `render.rs`'s own
        /// `render_details_from_model` is the one reader, and does its own fresh, isolated
        /// `Doc::parse` over `src[glued_body]` to render real Markdown structure for it (see that
        /// function's own doc comment for why this is the one place this file's whole "no second
        /// parse of any substring, ever" promise has a documented exception, and why it is safe: the
        /// isolated re-parse's own output is *rendered* immediately, into plain `Line`s, never spliced
        /// back into this `Doc`'s own `blocks`/`events` — nothing downstream of `Doc::parse` ever
        /// reads a `Block`/index built from that second parse).
        glued_body: Option<Range<usize>>,
    },
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
///
/// A thin wrapper around [`parse_blocks_raw`], the actual event-walking loop: this level's own
/// sibling list is run through [`fold_details`] exactly once, right before it is handed back to
/// whichever caller asked for it — the top-level `Doc::parse`, or `parse_container`'s own
/// `List`/`Item`/`BlockQuote` arms, each of which calls `parse_blocks` (never `parse_blocks_raw`
/// directly), so a `<details>` block gets folded at *every* nesting depth this model builds a fresh
/// sibling list at, not merely the top of the document.
fn parse_blocks(events: &mut Walker, src: &str) -> Vec<Block> {
    fold_details(parse_blocks_raw(events, src), src)
}

/// The event-walking loop `parse_blocks` wraps — same signature, same contract (see that function's
/// own doc comment for what a "sibling list" here means and how a tight list item's own bare inline
/// content is handled) — split out purely so `parse_blocks` itself can run [`fold_details`] over the
/// result at a single, guaranteed exit point (this function still returns early, via `Event::End(_)
/// => return out`, for a `List`/`Item`/`BlockQuote`'s own children — `parse_blocks`'s own fold runs
/// *after* either exit, uniformly).
fn parse_blocks_raw(events: &mut Walker, src: &str) -> Vec<Block> {
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

/// Folds a `<details>` opening `Html` leaf and the sibling blocks that follow it, up to (and
/// swallowing) the first later sibling that is itself a `</details>`-closing `Html` leaf, into one
/// `BlockKind::Details` container — reshaping the *already-parsed* sibling list one `parse_blocks`
/// call just built (`blocks`), never re-parsing `src`: `Doc::parse` still performs exactly one
/// `Parser::new_ext` call for the whole document (see the module doc comment).
///
/// ## Why "first close wins", not depth-aware nesting
///
/// This deliberately reproduces `split_details`'s own greedy contract, not a properly nesting-aware
/// one: scanning forward from an open tag, the *first* sibling whose own first line reads
/// `</details>` is taken as *this* block's close, however many further `<details>` opens sit between
/// them. A `<details>` reached only by being swallowed into another one's body this way is not
/// folded a second time here — it stays an ordinary, unfolded `Html` leaf inside `children`, exactly
/// the shape `contains_unsupported` (`render.rs`) already knows how to react to for a `Table` or
/// plain `Html` block found anywhere inside a `List`/`Quote`'s own subtree: the *whole* enclosing
/// construct is out of this stage's scope, not silently rendered with a gap in it. Matching
/// `collect_details_open`'s own contract (its doc comment: "a `<details>` nested inside another …
/// is swallowed and not counted separately") is the whole reason for choosing this over a
/// depth-aware match — see that function's own doc comment, and `BlockKind::Details.open_attr`'s,
/// for why the two must never disagree about which blocks the document-wide Tab-toggle ordinal
/// sequence counts.
///
/// ## Why this only ever succeeds for the well-formed shape
///
/// A close is only ever found among *sibling* blocks — pulldown-cmark's own HTML-block grammar
/// glues any run of consecutive, blank-line-free HTML-tag lines into a single `HtmlBlock` (see
/// `BlockKind::Html`'s own doc comment), so a `<details>`/`<summary>`/`</details>` written with no
/// blank line anywhere inside the whole construct arrives here as *one* `Html` leaf already, with no
/// separate sibling for this function to have found a close among in the first place — it is left
/// exactly as before this function existed, an unfolded `Html` leaf. This is not a narrower
/// approximation of `split_details`'s own boundary-finding that risks disagreeing with it on content
/// that *does* fold: for the shape this function does fold (open tag, body, close tag, each
/// separated from the next by at least one blank line — the shape every real `<details>` block in
/// this codebase's own samples uses), pulldown-cmark's block segmentation and `split_details`'s own
/// raw-line scan land on the identical boundaries.
///
/// ## The summary text
///
/// `extract_summary_body` — the render pipeline's own — is handed the exact same slice
/// `split_details` itself would compute for identical source text: from the byte right after the
/// open tag's own **first source line** (`line_after`, matching `lines[start + 1]`'s own start
/// offset) through the close tag's own `src.start` (matching `lines[body_end]`'s own start offset)
/// — or, when unclosed, through the end of the last child this function actually collected (**not**
/// `src.len()` for the whole document: unlike `split_details`, which re-parses a *fresh substring*
/// per recursion level and so can safely use "the end of what I was handed" as its own fallback,
/// this model's `src` is always the one, whole-document text at every nesting depth — bounding the
/// unclosed case to the children actually gathered, rather than to the document's own end, is what
/// keeps this from scanning into unrelated, later document content it was never asked about).
fn fold_details(blocks: Vec<Block>, src: &str) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    let mut rest: std::collections::VecDeque<Block> = blocks.into();
    while let Some(b) = rest.pop_front() {
        let open_attr = match &b.kind {
            BlockKind::Html { tag: Some(t), .. } if t == "details" => {
                super::details_open_tag(first_source_line(src, &b.src))
            }
            _ => None,
        };
        let Some(open_attr) = open_attr else {
            out.push(b);
            continue;
        };
        if rest.is_empty() || html_leaf_contains_its_own_close(src, &b) {
            // Either nothing follows this open tag at this sibling level at all, or this leaf's own
            // *raw text* already runs past a `</details>` line of its own — pulldown-cmark glued
            // everything (a `<summary>`, a nested `<details>`, its own close, ...) into this one
            // `Html` leaf, with no blank line anywhere inside the construct to have split any of it
            // into separate siblings for this function to have found (`b.src` can run well past its
            // own first line even though there is exactly one sibling here — confirmed directly:
            // `"<details>\n<summary>A</summary>\n<details>\n<summary>Nested</summary>\n</details>\n\
            // </details>\n"` parses as a *single* 89-byte `HtmlBlock`, not six). There is no
            // `children` this function could build here the normal, no-second-parse way — but
            // `glued_details_fold` (below) still recognizes the *unambiguous* instance of this shape
            // (see its own doc comment for exactly which one) and folds it anyway, storing the raw
            // body's own byte range for `render.rs`'s own isolated re-parse instead of a `children`
            // list (`BlockKind::Details.glued_body`). Anything more ambiguous — a nested `<details>`
            // glued into the same leaf, or unrelated content following the found close within it —
            // stays an ordinary, unfolded `Html` leaf, exactly as before either variant existed; see
            // `Html`'s own doc comment on exactly this shape.
            //
            // The second half of this check (`html_leaf_contains_its_own_close`) matters even when
            // `rest` is *not* empty: a glued block like the one above can still be followed by
            // further, wholly unrelated siblings (a trailing paragraph, a footnote section
            // `process_footnotes` appended, ...) — without this check, `close_pos` below would never
            // find this leaf's own (already-consumed) `</details>` among `rest` at all, and the
            // "unclosed" fallback would swallow every one of those unrelated siblings as if they were
            // this block's own body (confirmed directly: `code_span_corpus`'s own "details/summary
            // immediately followed by a fenced code block, no blank line" case, whose `src` appends a
            // footnote section after an already-self-closed, glued `<details>` block — and, since
            // 2026-08, one `glued_details_fold` *does* fold, its trailing blank line keeping that
            // footnote section correctly outside the found close's own line).
            if let Some((summary, glued_body)) = glued_details_fold(src, &b) {
                out.push(Block {
                    kind: BlockKind::Details {
                        open_attr,
                        summary,
                        glued_body: Some(glued_body),
                    },
                    src: b.src.clone(),
                    children: Vec::new(),
                });
                continue;
            }
            out.push(b);
            continue;
        }
        let close_pos = rest.iter().position(|c| is_details_close_block(c, src));
        let body_start = line_after(src, b.src.start);
        let (mut children, block_end, summary_end) = match close_pos {
            Some(pos) => {
                let mut children = Vec::with_capacity(pos);
                for _ in 0..pos {
                    children.push(rest.pop_front().expect("position() found this many ahead"));
                }
                let close = rest
                    .pop_front()
                    .expect("position() found a close at this offset");
                (children, close.src.end, close.src.start)
            }
            None => {
                // `rest` is non-empty here (checked above) and `close_pos` found nothing in it, so
                // every remaining sibling becomes a child — `children` is never empty in this arm.
                let children: Vec<Block> = rest.drain(..).collect();
                let end = children.last().map_or(b.src.end, |c| c.src.end);
                (children, end, end)
            }
        };
        // Defensive clamp (principle #3, `CLAUDE.md`): every real input keeps `body_start <=
        // summary_end` (a later sibling's own range never starts before this block's own first
        // line ends), but this guards against a slice panic outright if some future input this
        // function has not been tested against ever violated it.
        let inner_start = body_start.min(summary_end);
        let inner = &src[inner_start..summary_end];
        let (summary, _body) = super::extract_summary_body(inner);
        // A **no-blank-line-after-the-open-tag** rescue: `b.src` (the open tag's own leaf) can run
        // past its own first line even on this "normal" (not `glued_details_fold`) path — CommonMark's
        // HTML-block grammar keeps absorbing whatever *non-blank* lines follow `<details ...>`
        // (`alpha` in `<details open>\nalpha\n<br>\nbeta\n</details>\n`, confirmed directly: this
        // exact source's own `Html` leaf reports `src: 0..21`, covering the open tag's own line *and*
        // `alpha`'s) until the block genuinely ends at a blank line, and `alpha` here is no more a
        // `<summary>` than it is a `</details>` — it is ordinary body text that just happened to have
        // no blank line of its own separating it from the tag above. `super::split_details`'s own
        // raw-line body scan has no such gluing to begin with (it treats `body_start..summary_end` as
        // one flat span regardless of blank lines, then hands the whole thing to a fresh Markdown
        // parse), so production loses nothing here — this rescues the identical text on this file's
        // own path (found 2026-08, rendering every "no blank line under an unseparated `<details>`"
        // shape in the parity corpus: `alpha` above vanished outright, kept out of both `children`
        // — `fold_details` never sees it as a sibling `Block` at all, since it was never a `Doc.events`
        // block of its own — *and* the `extract_summary_body` call above, whose own `_body` return
        // value has always been discarded here (`inner` deliberately reaches past `b.src.end`, into
        // whatever real sibling children/close tag follow, purely so a *glued* `<summary>` — see
        // below — is still found; the well-formed "summary and body are separate siblings" shape
        // never had a leftover for `_body` to carry in the first place, which is why nothing here
        // needed it before now).
        //
        // The rescued range is only ever the slice `super::extract_summary_body` has *not* already
        // read as the summary tag (`super::summary_tag_end`, the identical boundary that function's
        // own second return value is sliced from) — clamped to `b.src.end`, this leaf's own extent, so
        // a `<summary>` glued onto the same line as the open tag (the everyday, blank-line-separated-
        // body idiom `<details>\n<summary>S</summary>\n\nbody\n</details>\n` glues *these two* lines
        // into one `Html` leaf too, confirmed directly) is never rescued a second time as raw body
        // text alongside its own already-extracted `summary` field: `real_body_start` lands at (or
        // past) `b.src.end` there, so the `if` below never fires, and this stays a pure no-op on
        // every shape that does not have this exact gap. Rendered via `render_html_block_from_model`
        // like any other `Html` block (`tag: None`, matching what `super::html_tag_name` would report
        // for a line that does not start with `<` at all) — reusing the one function this file already
        // trusts to turn arbitrary glued raw text into `Line`s, rather than a second, narrower
        // reimplementation of it here.
        //
        // Guarded on the leftover being more than whitespace: the everyday, blank-line-separated-body
        // idiom (`<details>\n<summary>S</summary>\n\nbody\n</details>\n`) glues the open tag and its
        // own `<summary>` into one `Html` leaf too (confirmed directly — no blank line separates
        // *those* two lines either), and, once rounded up to a line boundary (below), `real_body_start`
        // there lands *at* `b.src.end`: there is nothing left in the leaf past the summary line at all.
        // Rescuing an empty range as a `Block` on every well-formed `<details>` would still be a
        // *structural* regression `render.rs`'s own `contains_unsupported` can observe even though the
        // screen never changes: a quote-nested alert's own `<details>` reaches this exact shape
        // (`> <details open>\n> <summary>Nested</summary>\n`, `md_render_diff_tests`'s own
        // `details_nested_inside_an_alert_does_not_consume_the_top_level_ordinal` corpus case) with
        // `in_quote: true` already set, and a bare `BlockKind::Html` child there is reported
        // unsupported unconditionally (`contains_unsupported`'s own `Html { .. } if in_quote` arm) —
        // falling the *whole* alert back to the legacy renderer over one phantom, invisible child. The
        // `.trim()` check keeps this rescue narrowly targeted at the one shape it exists for (real body
        // text CommonMark genuinely glued to the open tag, confirmed non-whitespace) rather than firing
        // on every well-formed block regardless of whether there is anything left to rescue at all.
        //
        // Rounded up to the *next line* — via `line_after`, not used byte-exact — when a `<summary>`
        // was actually found and consumed: `super::summary_tag_end`'s own boundary lands mid-line,
        // right after `</summary>`'s own `>`, but pulldown-cmark's own `Event::Html` sub-events are
        // always whole, `'\n'`-inclusive physical lines (confirmed directly: `Doc::parse`'s own event
        // stream for a glued leaf reports one `Event::Html` per source line, trailing newline and all —
        // see the module doc comment's "How inline content is rendered" section). Splitting a leaf
        // mid-line here would leave neither the tag-line sliver `render.rs`'s own completeness check
        // (`app::md_model_snapshot_tests::model_covers_every_inline_event_across_the_full_corpus`)
        // computes from `b.src.start..first_child.src.start`, nor this rescued leaf itself, covering
        // that one summary line's own trailing `'\n'` in full — found the same way as the leaf-count
        // regression above, by running the full test suite after adding the byte-exact version first.
        // No such rounding when no `<summary>` was found at all (`summary_tag_end` returns `None`):
        // `inner_start` is already a clean line start there (`line_after(src, b.src.start)`, the
        // position right after the open tag's own first line) — rounding again would skip the very
        // first real body line (`alpha`, in the doc comment above) whole.
        let real_body_start = match super::summary_tag_end(inner) {
            Some(off) => line_after(src, inner_start + off),
            None => inner_start,
        }
        .min(b.src.end);
        if real_body_start < b.src.end && !src[real_body_start..b.src.end].trim().is_empty() {
            // `b` (the open tag's own leaf) is itself a `Html` block, and its own `body_spans`
            // already cover this whole leaf's content, one clean range per physical source line
            // (`real_body_start` is line-rounded, via `line_after` above, to exactly one of those
            // lines' own start) — this rescued child's own `body_spans` is just the tail of that
            // same list, not a fresh scan: correct inside a block quote too, the same way `b`'s own
            // were, with no separate `>`-marker handling needed here.
            let rescued_body_spans = match &b.kind {
                BlockKind::Html { body_spans, .. } => body_spans
                    .iter()
                    .filter(|r| r.start >= real_body_start)
                    .cloned()
                    .collect(),
                // Not reachable: `open_attr` above is only `Some` when `b.kind` already matched
                // `Html { tag: Some(t) } if t == "details"` — kept as a defensive fallback rather
                // than assumed away, per `CLAUDE.md` principle #3 (never crash).
                _ => Vec::new(),
            };
            children.insert(
                0,
                Block {
                    kind: BlockKind::Html {
                        tag: None,
                        body_spans: rescued_body_spans,
                    },
                    src: real_body_start..b.src.end,
                    children: Vec::new(),
                },
            );
        }
        out.push(Block {
            kind: BlockKind::Details {
                open_attr,
                summary,
                glued_body: None,
            },
            src: b.src.start..block_end,
            children,
        });
    }
    out
}

/// The first physical line of `src[r]`, trailing-whitespace-trimmed — mirrors `html_tag_of`'s own
/// identical read exactly (kept as a separate one-liner rather than shared with it, since that
/// function reads off a `body_spans` slice instead of a raw `Block::src` range — the two agree here
/// only because `r` is already a whole, already-parsed block's own range, not a fresh one this
/// function would need `events` to build). Used by `fold_details` to decide, for a `Html { tag:
/// Some("details") }` leaf, whether its own first line is an opening or a closing details tag.
fn first_source_line<'a>(src: &'a str, r: &Range<usize>) -> &'a str {
    src[r.clone()].lines().next().unwrap_or("").trim_end()
}

/// Whether `c` is a `Html { tag: Some("details") }` leaf whose own first source line is a
/// `</details>`-closing tag (as opposed to an opening `<details ...>` one — both share the same
/// `tag` value, see `html_tag_of`'s own doc comment on why: this is the one place that tells
/// them apart, by re-checking `is_details_close` on the block's own first line, the identical
/// predicate `split_details` itself uses).
fn is_details_close_block(c: &Block, src: &str) -> bool {
    matches!(&c.kind, BlockKind::Html { tag: Some(t), .. } if t == "details")
        && super::is_details_close(first_source_line(src, &c.src))
}

/// Whether an open-tag `Html { tag: Some("details") }` leaf's own **raw text** already runs past a
/// `</details>`-closing line of its own — i.e., whether pulldown-cmark glued this open tag and its
/// own matching close into the *same* `HtmlBlock` (no blank line anywhere between them) rather than
/// reporting the close as a separate sibling `fold_details` could find via `is_details_close_block`.
/// Scans every line of `b.src` *after* its own first (the `<details ...>` tag's own line, already
/// known not to be a close) for one matching `is_details_close` — used by `fold_details` to tell
/// "this open tag is genuinely unclosed, or closed only by a later sibling" apart from "this leaf
/// is already fully self-contained, whatever *else* happens to follow it at this sibling level" —
/// see that function's own call site for exactly why the distinction matters.
fn html_leaf_contains_its_own_close(src: &str, b: &Block) -> bool {
    let text = &src[b.src.clone()];
    text.lines()
        .skip(1)
        .any(|line| super::is_details_close(line.trim_end()))
}

/// Whether `b` — a glued `<details>` leaf `html_leaf_contains_its_own_close` has already confirmed
/// contains its own close — is the **unambiguous** instance of that shape: the summary and
/// `render.rs`'s own byte range for its isolated re-parse (`BlockKind::Details.glued_body`), when it
/// is. `None` when it is not, leaving `fold_details` on its previous, safe fallback (an ordinary,
/// unfolded `Html` leaf — see that function's own call site).
///
/// "Unambiguous" means both of the following hold, checked by scanning `b`'s own raw text one line at
/// a time, starting right after its own first line (the `<details ...>` open tag itself):
///
/// * The **first** line matching `is_details_close` is also `b`'s own **last** line (mod a single
///   trailing `'\n'`, or nothing at all — see the `after_close` check below) — no further, unrelated
///   content follows the found close within the same glued leaf. Without this, truncating
///   `Block::src` to end at that close (as this function's caller does) would silently orphan
///   whatever bytes come after it — there is no sibling slot left to carry them.
/// * No line strictly between the open tag and that close is **itself** a `<details ...>` open tag —
///   no nested `<details>` glued into the same leaf. Without this, "first close wins" would hand the
///   *inner* `<details>`'s own close to the *outer* tag (`</details>` on the line right after
///   `<summary>Nested</summary>` in `"<details>\n<summary>A</summary>\n<details>\n\
///   <summary>Nested</summary>\n</details>\n</details>\n"`, say — matching `split_details`'s own
///   line-scan contract exactly, since it does not track nesting either), which this function
///   deliberately does not attempt to resolve: see `glued_details_with_no_blank_line_anywhere_stays_\
///   an_unfolded_html_leaf`, still pinned exactly this way, for why an ambiguous glued shape like that
///   one stays an unfolded `Html` leaf rather than folding into something that would misrepresent
///   which `</details>` actually belongs to which `<details>`.
///
/// Both checks are read straight off a literal line-by-line scan of `b.src`'s own raw text, the
/// identical predicates (`super::is_details_close`/`super::details_open_tag`) `html_leaf_contains_\
/// its_own_close`/`fold_details`'s own well-formed branch already use — not a third, competing
/// classifier.
///
/// The returned range excludes the `<summary>...</summary>` tag itself (found the same way
/// `super::extract_summary_body` finds it, via `super::summary_tag_end` — the offset-returning
/// sibling of the same search, so the two can never disagree about where the summary ends and the
/// body begins) — a caller re-parsing `src[range]` in isolation must never see that tag a second
/// time, or the summary text would render twice (once as this block's own fold marker, again as the
/// re-parsed body's first line of literal text). Unlike `extract_summary_body`'s own `body` (its
/// second return value), this range is **not** blank-line-trimmed: a leading or trailing blank line
/// left inside it changes nothing about the block-level structure a fresh `Doc::parse` reads back —
/// CommonMark itself already treats leading/trailing blank lines as no content at all, the same way
/// `trim_blank_lines` removes them for `extract_summary_body`'s own, differently-purposed `body`
/// string — so there is no byte-offset equivalent of that trim to compute here.
fn glued_details_fold(src: &str, b: &Block) -> Option<(String, Range<usize>)> {
    let body_start = line_after(src, b.src.start);
    let scan = &src[body_start..b.src.end];
    let mut close: Option<(usize, usize)> = None; // (start, end) absolute into `src`; `end` excludes any trailing '\n'
    let mut nested = false;
    let mut pos = 0usize; // offset within `scan`
    loop {
        let rel_end = scan[pos..].find('\n').map_or(scan.len(), |i| pos + i);
        let line = &scan[pos..rel_end];
        if super::is_details_close(line) {
            close = Some((body_start + pos, body_start + rel_end));
            break;
        }
        if super::details_open_tag(line).is_some() {
            nested = true;
        }
        if rel_end >= scan.len() {
            break; // ran off the end of `b.src` with no close found
        }
        pos = rel_end + 1;
    }
    let (close_start, close_end) = close?;
    if nested {
        return None;
    }
    // Nothing but whitespace (a lone trailing '\n', most commonly, or nothing at all) may follow the
    // found close within this same leaf — anything else is unrelated trailing content this function
    // has no sibling slot to hand back to `fold_details`'s own caller.
    if !src[close_end..b.src.end].trim().is_empty() {
        return None;
    }
    let inner = &src[body_start..close_start];
    let (summary, _body) = super::extract_summary_body(inner);
    let body_off = super::summary_tag_end(inner).unwrap_or(0);
    Some((summary, (body_start + body_off)..close_start))
}

/// The byte position right after the next `'\n'` at or after `start` — `src.len()` if there is
/// none. Always a valid `char` boundary (`'\n'` is one ASCII byte, and `str::find` never returns a
/// non-boundary index), so a caller slicing `&src[line_after(src, x)..]` never risks a panic on that
/// account. Used by `fold_details` to compute "the start of the source line right after this
/// block's own first one" — the same position `lines[start + 1]` names in `split_details`'s own,
/// raw-line-indexed world (see that function's own doc comment) — without needing a `Vec<&str>` of
/// this model's own to index into.
fn line_after(src: &str, start: usize) -> usize {
    match src[start..].find('\n') {
        Some(off) => start + off + 1,
        None => src.len(),
    }
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
            let (alert, alert_title) = alert_kind_of(src, &range);
            Some(Block {
                kind: BlockKind::Quote { alert, alert_title },
                src: range,
                children,
            })
        }
        Tag::Table(aligns) => {
            let rows = collect_table_rows(events);
            Some(leaf(BlockKind::Table { aligns, rows }, range))
        }
        Tag::HtmlBlock => {
            let body_spans = collect_html_body_spans(events);
            let tag = html_tag_of(&body_spans, src);
            Some(leaf(BlockKind::Html { tag, body_spans }, range))
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

/// Collects an `HtmlBlock`'s content as one byte range per `Event::Html` pulldown-cmark reports, in
/// order — mirrors `collect_code_body_spans` exactly (see that function's own doc comment for the
/// walk itself), for the identical reason: see `BlockKind::Html.body_spans`'s own doc comment for why
/// a list of ranges, not one, is needed here and what each range does and does not include. Consumes
/// up to and including the block's own `End(HtmlBlock)`.
fn collect_html_body_spans(events: &mut Walker) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    while let Some((ev, range)) = events.next() {
        match ev {
            Event::Html(_) => spans.push(range),
            Event::End(TagEnd::HtmlBlock) => break,
            // Not reachable for well-formed input (an HTML block's content is never itself a
            // container), balanced defensively rather than assumed away — see `parse_blocks`'s doc
            // comment on never desyncing the stream.
            Event::Start(t) => skip_inline_to(events, t.to_end()),
            _ => {}
        }
    }
    spans
}

/// Reconstructs an HTML block's content from its `body_spans` — concatenating each range's slice of
/// `src`, in order — with **no** trailing-newline stripping, unlike `code_body_text`: every
/// `Event::Html` range already includes its own physical line's trailing `\n` verbatim (confirmed by
/// the same dump `BlockKind::Html.body_spans`'s own doc comment quotes), and `render_html_block_from_model`
/// (`render.rs`) walks the result back apart one physical line at a time (`split_inclusive('\n')`) —
/// the identical way it walked `&src[block_src]` before this field existed — so stripping a trailing
/// newline here would silently glue the block's last two lines into one for that walk. A block whose
/// very last line in the source carries no trailing `\n` at all (end of file) is reproduced exactly
/// that way too, since nothing here invents one.
pub(crate) fn html_body_text(body_spans: &[Range<usize>], src: &str) -> String {
    let mut s = String::new();
    for r in body_spans {
        s.push_str(&src[r.clone()]);
    }
    s
}

/// Reads a best-effort tag name off an `HtmlBlock`'s own opening line — see `BlockKind::Html`'s doc
/// comment for exactly what this does and does not distinguish. `body_spans` must be this same
/// block's own `collect_html_body_spans` result: its first entry is exactly the block's first
/// physical line, `>`-marker-free even inside a block quote (see that function's own doc comment),
/// which is what makes reading the tag name off it — rather than off `src[block_range]` directly —
/// correct inside a block quote too, not merely for the top-level case where the two happen to
/// already agree (a block's *first* line is never `>`-marked even inside a quote; only its
/// *continuation* lines are — see `BlockKind::Html.body_spans`'s own doc comment).
fn html_tag_of(body_spans: &[Range<usize>], src: &str) -> Option<String> {
    let first_line = body_spans
        .first()
        .map(|r| src[r.clone()].lines().next().unwrap_or(""))
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
/// render pipeline's real `parse_alert_header`, not a second parser of this module's own. Returns
/// that same call's own title text too (`String::new()` when there is no alert, or the header
/// carried none) — see `BlockKind::Quote.alert_title`'s own doc comment for why it travels
/// alongside `alert` as a sibling field rather than folded into it.
fn alert_kind_of(src: &str, range: &Range<usize>) -> (Option<AlertKind>, String) {
    let first_line = src[range.clone()].lines().next().unwrap_or("");
    match super::parse_alert_header(first_line) {
        Some((kind, title)) => (Some(kind), title),
        None => (None, String::new()),
    }
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
        let BlockKind::Quote { alert, alert_title } = doc.blocks[0].kind.clone() else {
            panic!("expected a quote")
        };
        assert_eq!(alert, Some(AlertKind::Warning));
        assert_eq!(
            alert_title, "Heads up",
            "the header's own Obsidian-style trailing title travels alongside alert"
        );
    }

    #[test]
    fn plain_block_quote_has_no_alert_kind() {
        let doc = Doc::parse("> just a quote\n");
        let BlockKind::Quote { alert, alert_title } = doc.blocks[0].kind.clone() else {
            panic!("expected a quote")
        };
        assert_eq!(alert, None);
        assert_eq!(alert_title, "", "no alert header means no title either");
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
        let BlockKind::Html { tag, .. } = &doc.blocks[0].kind else {
            panic!("expected an html block")
        };
        assert_eq!(tag.as_deref(), Some("div"));
    }

    #[test]
    fn html_comment_block_has_no_tag_name() {
        let doc = Doc::parse("<!-- a comment -->\n");
        let BlockKind::Html { tag, .. } = &doc.blocks[0].kind else {
            panic!("expected an html block")
        };
        assert_eq!(*tag, None);
    }

    #[test]
    fn well_formed_details_folds_into_one_container_with_summary_and_children() {
        let src = "<details>\n<summary>S</summary>\n\nbody\n\n</details>\n";
        let doc = Doc::parse(src);
        // pulldown-cmark still groups `<details>`/`<summary>...</summary>` into one HtmlBlock (no
        // blank line between them) and reports the paragraph and the standalone `</details>` as two
        // further siblings — three raw blocks total, same as before `fold_details` existed (see
        // `BlockKind::Html`'s own doc comment) — but `parse_blocks` now folds all three into one
        // `Details` container before handing the sibling list back to any caller.
        assert_eq!(doc.blocks.len(), 1, "folded into a single Details block");
        let BlockKind::Details {
            open_attr, summary, ..
        } = &doc.blocks[0].kind
        else {
            panic!("expected a Details block: {:?}", doc.blocks[0].kind)
        };
        assert!(!open_attr, "no `open` attribute on the tag");
        assert_eq!(summary, "S");
        assert_eq!(
            &src[doc.blocks[0].src.clone()],
            src,
            "spans the whole construct"
        );
        assert_eq!(doc.blocks[0].children.len(), 1, "just the body paragraph");
        let BlockKind::Paragraph { inline } = &doc.blocks[0].children[0].kind else {
            panic!("expected the body paragraph")
        };
        assert_eq!(inline_plain_text(&doc, inline), "body");
    }

    #[test]
    fn details_open_attribute_is_captured() {
        let doc = Doc::parse("<details open>\n<summary>S</summary>\n\nbody\n\n</details>\n");
        let BlockKind::Details { open_attr, .. } = &doc.blocks[0].kind else {
            panic!("expected a Details block")
        };
        assert!(open_attr);
    }

    #[test]
    fn details_with_no_summary_tag_reports_an_empty_summary() {
        let doc = Doc::parse("<details>\n\nbody\n\n</details>\n");
        let BlockKind::Details { summary, .. } = &doc.blocks[0].kind else {
            panic!("expected a Details block")
        };
        assert_eq!(summary, "");
    }

    #[test]
    fn unclosed_details_folds_every_remaining_sibling_as_its_body() {
        let src = "<details>\n<summary>S</summary>\n\nbody one\n\nbody two\n";
        let doc = Doc::parse(src);
        assert_eq!(doc.blocks.len(), 1);
        let BlockKind::Details {
            open_attr, summary, ..
        } = &doc.blocks[0].kind
        else {
            panic!("expected a Details block")
        };
        assert!(!open_attr);
        assert_eq!(summary, "S");
        assert_eq!(
            doc.blocks[0].children.len(),
            2,
            "both trailing paragraphs, with no closing tag to stop at"
        );
        assert_eq!(
            doc.blocks[0].src.end,
            src.len(),
            "an unclosed block runs to the end of input"
        );
    }

    #[test]
    fn nested_details_swallows_the_inner_close_first_matching_split_details() {
        // Mirrors `markdown.rs`'s own `nested_details_do_not_drift_later_block_open_state`
        // (`split_details`'s own greedy, non-nesting-aware contract), but with the outer/inner tags
        // separated by blank lines so pulldown-cmark itself reports each one as its own sibling
        // block (rather than gluing everything into one opaque `Html` leaf) — exercising
        // `fold_details`'s own "first close wins" search, not merely `split_details`'s line scan.
        let src = "<details>\n<summary>Outer</summary>\n\n\
                   <details open>\n<summary>Inner</summary>\n\n\
                   inner body\n\n\
                   </details>\n\n\
                   outer body after inner\n\n\
                   </details>\n";
        let doc = Doc::parse(src);
        // The outer's own search finds the *inner's* close first, swallowing it — its own trailing
        // `</details>` (and the paragraph right before it) are left as top-level siblings, not
        // children of anything.
        assert_eq!(
            doc.blocks.len(),
            3,
            "outer Details, the leftover paragraph, and the leftover close tag: {:?}",
            doc.blocks
        );
        let BlockKind::Details {
            open_attr, summary, ..
        } = &doc.blocks[0].kind
        else {
            panic!("expected the outer Details block")
        };
        assert!(!open_attr);
        assert_eq!(summary, "Outer");
        // The inner `<details>` is swallowed into the outer's own body as a raw, unfolded `Html`
        // leaf — not itself recognized as a nested `Details` container (see `fold_details`'s own
        // doc comment on why this is not recursive).
        assert_eq!(doc.blocks[0].children.len(), 2);
        assert!(matches!(
            doc.blocks[0].children[0].kind,
            BlockKind::Html { tag: Some(ref t), .. } if t == "details"
        ));
        let BlockKind::Paragraph { inline } = &doc.blocks[0].children[1].kind else {
            panic!("expected the inner body paragraph")
        };
        assert_eq!(inline_plain_text(&doc, inline), "inner body");
        // Leftover siblings after the outer's own (stolen) close.
        assert!(matches!(doc.blocks[1].kind, BlockKind::Paragraph { .. }));
        assert!(matches!(
            doc.blocks[2].kind,
            BlockKind::Html { tag: Some(ref t), .. } if t == "details"
        ));
    }

    #[test]
    fn glued_details_with_no_blank_line_anywhere_stays_an_unfolded_html_leaf() {
        // No blank line separates the open tag, the nested block, or either close — pulldown-cmark
        // merges the whole thing into a single `HtmlBlock`, so there is no separate sibling for
        // `fold_details` to have found a close among at all (see `BlockKind::Html`'s own doc
        // comment on exactly this shape) — unchanged from before `Details` existed.
        let src = "<details>\n<summary>A</summary>\n<details>\n<summary>Nested</summary>\n</details>\n</details>\n";
        let doc = Doc::parse(src);
        assert_eq!(doc.blocks.len(), 1);
        assert!(matches!(
            doc.blocks[0].kind,
            BlockKind::Html { tag: Some(ref t), .. } if t == "details"
        ));
    }

    #[test]
    fn details_nested_inside_a_list_item_folds_at_that_level_too() {
        let src = "- item\n\n  <details>\n  <summary>S</summary>\n\n  body\n\n  </details>\n";
        let doc = Doc::parse(src);
        let item = &doc.blocks[0].children[0];
        let details = item
            .children
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Details { .. }))
            .expect("expected a folded Details block inside the list item");
        let BlockKind::Details { summary, .. } = &details.kind else {
            unreachable!()
        };
        assert_eq!(summary, "S");
    }

    #[test]
    fn details_nested_inside_a_quote_folds_at_that_level_too() {
        let src = "> <details>\n> <summary>S</summary>\n>\n> body\n>\n> </details>\n";
        let doc = Doc::parse(src);
        let quote = &doc.blocks[0];
        assert!(matches!(quote.kind, BlockKind::Quote { .. }));
        let details = quote
            .children
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Details { .. }))
            .expect("expected a folded Details block inside the quote");
        let BlockKind::Details { summary, .. } = &details.kind else {
            unreachable!()
        };
        assert_eq!(summary, "S");
    }

    #[test]
    fn a_tag_name_prefix_is_not_confused_with_details_itself() {
        let doc = Doc::parse("<detailsx>\nhi\n</detailsx>\n");
        let BlockKind::Html { tag, .. } = &doc.blocks[0].kind else {
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

    // =============================================================================================
    // Test-hardening pass (pre-release audit): everything from here down was added to close two
    // kinds of gap found by measuring line coverage on this file (`cargo llvm-cov`):
    //
    //   1. `check_invariants` itself (above) had 100% *function* coverage but was missing ~85 *line*
    //      lines — every one of its "report a violation" branches, the whole reason the function
    //      exists, had never actually fired in any test run. A checker nobody has ever seen catch
    //      anything is not yet proven to catch anything — see `invariant_violations` below, which
    //      builds a deliberately-broken `Block`/`Task` for each violation kind `check_invariants`
    //      claims to detect and asserts it is actually reported.
    //   2. A handful of defensive branches (a handful of `Event::Start` arms marked "not reachable
    //      for well-formed input", `parse_blocks_raw`'s own stray-`TaskListMarker` catch-all,
    //      `collect_stray_inline_run`'s exhausted-stream arm, `glued_details_fold`'s "ran off the
    //      end with no close" and "found a close but something unrelated follows it" arms,
    //      `html_tag_name`'s empty-name arm) are real code, but only reachable either through a
    //      *malformed* — not merely unusual — event stream, or through document shapes this model's
    //      own doc comments already anticipated (`<details>` with no close anywhere, glued to
    //      trailing content, ending mid-line with no trailing newline). Reached below either by a
    //      genuinely adversarial `Doc::parse` input, or — where no real document reaches a purely
    //      defensive branch at all — by calling the private helper directly with a hand-built
    //      `Walker`/`Task`/`Block`, exactly as this module's own contract (private items are visible
    //      to this same file's own `#[cfg(test)] mod tests`) allows.
    //
    // Three "completeness proxy" tests round this out for the one class of gap line coverage cannot
    // measure at all: the render pipeline's own completeness cross-check
    // (`app::md_model_snapshot_tests::model_covers_every_inline_event_across_the_full_corpus`, in
    // `src/app/`) is out of scope for this file to touch or duplicate — instead, three small,
    // self-contained scans (over this same `Doc`) are built here, purely to *prove* — by corrupting a
    // real, correctly-parsed `Doc` one way at a time and showing the scan notices — that "an inline
    // event belongs to no leaf" / "a `TaskListMarker` was dropped" / "a `CodeBlock` was dropped" are
    // all things *some* check can catch, without claiming these three toy scans are a substitute for
    // the real one.

    // ---- adversarial coverage of `check_invariants` itself: prove each violation kind is caught ---
    //
    // Every test below builds a `Block`/`Task` value that breaks exactly one invariant
    // `check_invariants` is documented to catch, then asserts the violation is actually reported.
    // `check_invariants` is called directly (never through `Doc::parse`) — nothing here needs a
    // document real parsing could ever produce, only a `Block` tree shaped the way the checker is
    // supposed to reject. `src`/`events_len` are likewise whatever this test needs them to be, not
    // values a real `Doc::parse` call produced — `check_invariants` takes them as plain parameters
    // and has no way to tell the difference.

    /// Runs `check_invariants` over `blocks` at the top level (`parent: None`) and returns whatever
    /// it reports — a thin wrapper purely to avoid repeating the same four parameters at every call
    /// site below.
    fn violations_of(blocks: &[Block], src: &str, events_len: usize) -> Vec<String> {
        let mut out = Vec::new();
        check_invariants(blocks, src, events_len, None, "root", &mut out);
        out
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)] // deliberately backwards: this is the violation under test
    fn check_invariants_catches_src_start_after_end() {
        let bad = leaf(BlockKind::ThematicBreak, 5..2);
        let out = violations_of(std::slice::from_ref(&bad), "0123456789", 0);
        assert!(
            out.iter().any(|m| m.contains("src.start > src.end")),
            "expected a start>end violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_src_end_past_input_length() {
        let src = "abc";
        let bad = leaf(BlockKind::ThematicBreak, 0..99);
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter().any(|m| m.contains("past the input's length")),
            "expected an end-past-length violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_a_range_that_splits_a_multibyte_char() {
        let src = "あ"; // one 3-byte character: valid boundaries are only 0 and 3
        let bad = leaf(BlockKind::ThematicBreak, 0..1); // ends mid-character
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter().any(|m| m.contains("is not on a char boundary")),
            "expected a char-boundary violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_a_child_escaping_its_parent() {
        let src = "0123456789";
        let child = leaf(BlockKind::ThematicBreak, 6..8); // outside the parent's own 0..5
        let parent = Block {
            kind: BlockKind::List {
                ordered: false,
                start: None,
            },
            src: 0..5,
            children: vec![child],
        };
        let out = violations_of(std::slice::from_ref(&parent), src, 0);
        assert!(
            out.iter().any(|m| m.contains("escapes its parent")),
            "expected a parent-escape violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_overlapping_siblings() {
        let src = "0123456789";
        let a = leaf(BlockKind::ThematicBreak, 0..5);
        let b = leaf(BlockKind::ThematicBreak, 3..8); // starts before `a` ends
        let out = violations_of(&[a, b], src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("overlaps the previous sibling")),
            "expected an overlap violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_siblings_out_of_source_order() {
        let src = "0123456789";
        // Not overlapping in the visual sense either sibling's own text occupies, but the second
        // one's own start (0) sits *before* the first one's own end (9) — `check_invariants` only
        // has one guard for "siblings must be in non-decreasing source order" (the same `start < pe`
        // check `check_invariants_catches_overlapping_siblings` exercises above), so a genuinely
        // reversed pair trips the identical branch — see this module's own report on why "siblings
        // overlap" and "siblings are out of order" are, in this checker, the same check.
        let a = leaf(BlockKind::ThematicBreak, 5..9);
        let b = leaf(BlockKind::ThematicBreak, 0..2);
        let out = violations_of(&[a, b], src, 0);
        assert!(
            !out.is_empty(),
            "expected a violation for an out-of-source-order sibling pair, got none"
        );
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges, clippy::single_range_in_vec_init)] // deliberately backwards single-span body_spans: this is the violation under test
    fn check_invariants_catches_a_code_block_span_start_after_end() {
        let src = "0123456789";
        let bad = leaf(
            BlockKind::CodeBlock {
                lang: None,
                fenced: true,
                body_spans: vec![5..2],
            },
            0..10,
        );
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("body_spans") && m.contains("start > end")),
            "expected a body_spans start>end violation, got {out:?}"
        );
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a real body_spans is a Vec<Range<usize>>; one span is the normal case
    fn check_invariants_catches_a_code_block_span_past_input_length() {
        let src = "0123456789";
        let bad = leaf(
            BlockKind::CodeBlock {
                lang: None,
                fenced: true,
                body_spans: vec![0..99],
            },
            0..10,
        );
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("body_spans") && m.contains("past the input's length")),
            "expected a body_spans length violation, got {out:?}"
        );
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a real body_spans is a Vec<Range<usize>>; one span is the normal case
    fn check_invariants_catches_a_code_block_span_splitting_a_multibyte_char() {
        let src = "0あ23456789"; // "あ" occupies bytes 1..4
        let bad = leaf(
            BlockKind::CodeBlock {
                lang: None,
                fenced: true,
                body_spans: vec![0..2], // ends mid-character
            },
            0..src.len(),
        );
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("body_spans") && m.contains("is not on a char boundary")),
            "expected a body_spans char-boundary violation, got {out:?}"
        );
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a real body_spans is a Vec<Range<usize>>; one span is the normal case
    fn check_invariants_catches_a_code_block_span_escaping_its_own_block() {
        let src = "0123456789";
        let bad = leaf(
            BlockKind::CodeBlock {
                lang: None,
                fenced: true,
                body_spans: vec![6..8], // the block's own src (below) ends at 5
            },
            0..5,
        );
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("body_spans") && m.contains("escapes its own block's src")),
            "expected a body_spans own-block-escape violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_overlapping_code_block_spans() {
        let src = "0123456789";
        let bad = leaf(
            BlockKind::CodeBlock {
                lang: None,
                fenced: true,
                body_spans: vec![0..5, 3..8], // second starts before the first ends
            },
            0..10,
        );
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("body_spans") && m.contains("overlaps the previous span")),
            "expected an overlapping-spans violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_a_task_state_at_splitting_a_multibyte_char() {
        let src = "[あ]"; // '[' at byte 0, "あ" at bytes 1..4, ']' at byte 4
        let bad = Block {
            kind: BlockKind::ListItem {
                task: Some(Task {
                    state: 'x',
                    state_at: 2, // mid-character
                }),
            },
            src: 0..src.len(),
            children: Vec::new(),
        };
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("task.state_at") && m.contains("is not on a char boundary")),
            "expected a task char-boundary violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_a_task_state_that_does_not_match_the_byte_at_state_at() {
        let src = "[y]";
        let bad = Block {
            kind: BlockKind::ListItem {
                task: Some(Task {
                    state: 'x', // the byte at state_at is actually 'y'
                    state_at: 1,
                }),
            },
            src: 0..src.len(),
            children: Vec::new(),
        };
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter()
                .any(|m| m.contains("does not point at task.state")),
            "expected a task state-mismatch violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_a_task_state_at_not_bracketed() {
        let src = "(x)"; // parens, not square brackets
        let bad = Block {
            kind: BlockKind::ListItem {
                task: Some(Task {
                    state: 'x',
                    state_at: 1,
                }),
            },
            src: 0..src.len(),
            children: Vec::new(),
        };
        let out = violations_of(std::slice::from_ref(&bad), src, 0);
        assert!(
            out.iter().any(|m| m.contains("is not bracketed by")),
            "expected a task bracket violation, got {out:?}"
        );
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)] // deliberately backwards: this is the violation under test
    fn check_invariants_catches_a_headings_inline_start_after_end() {
        let src = "# hi\n";
        let bad = leaf(
            BlockKind::Heading {
                level: 1,
                inline: 5..2,
                id: None,
                classes: Vec::new(),
                attrs: Vec::new(),
            },
            0..src.len(),
        );
        let out = violations_of(std::slice::from_ref(&bad), src, 10);
        assert!(
            out.iter().any(|m| m.contains("inline.start > inline.end")),
            "expected an inline start>end violation, got {out:?}"
        );
    }

    #[test]
    fn check_invariants_catches_a_paragraphs_inline_end_past_events_len() {
        let src = "hi\n";
        let bad = leaf(BlockKind::Paragraph { inline: 0..99 }, 0..src.len());
        let out = violations_of(std::slice::from_ref(&bad), src, 3); // only 3 real events exist
        assert!(
            out.iter()
                .any(|m| m.contains("inline.end") && m.contains("past events_len")),
            "expected an inline-past-events_len violation, got {out:?}"
        );
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)] // deliberately backwards: this is one of the two violations under test
    fn check_invariants_reports_every_violation_at_once_not_just_the_first() {
        // `check_invariants`'s own doc comment promises this ("Returns every violation found ...
        // rather than asserting inline, so one test run reports everything wrong at once") — pin it
        // directly: two independently-broken top-level siblings should both show up in `out`, not
        // just whichever is encountered first.
        let src = "0123456789";
        let a = leaf(BlockKind::ThematicBreak, 5..2); // start > end
        let b = leaf(BlockKind::ThematicBreak, 0..99); // past input length
        let out = violations_of(&[a, b], src, 0);
        assert!(
            out.iter().any(|m| m.contains("src.start > src.end")),
            "{out:?}"
        );
        assert!(
            out.iter().any(|m| m.contains("past the input's length")),
            "{out:?}"
        );
    }

    // ---- completeness proxies: prove "a piece of the raw stream went unrepresented" is catchable -
    //
    // The render pipeline's own completeness cross-check (`app::md_model_snapshot_tests`, in
    // `src/app/`) is out of scope here — these three tests instead build small, local, self-contained
    // scans over a `Doc` this file already trusts (real output of `Doc::parse`), then *corrupt a
    // clone* of that same `Doc` one way at a time and show the scan's own count changes. This proves
    // each described gap is detectable by *some* check, without claiming these scans are a substitute
    // for the real one.

    /// Marks every event index claimed by some `Heading`/`Paragraph` leaf's own `inline` range,
    /// recursively. A narrow proxy for "every inline-content event belongs to some leaf" — real
    /// completeness also has to account for `CodeBlock`/`Table` content (byte ranges, not event
    /// indices) and container `Start`/`End` events, none of which this toy scan attempts; see the
    /// section doc comment above.
    fn events_claimed_by_a_leaf(blocks: &[Block], claimed: &mut [bool]) {
        for b in blocks {
            let inline = match &b.kind {
                BlockKind::Heading { inline, .. } => Some(inline),
                BlockKind::Paragraph { inline } => Some(inline),
                _ => None,
            };
            if let Some(inline) = inline {
                for i in inline.clone() {
                    if let Some(slot) = claimed.get_mut(i) {
                        *slot = true;
                    }
                }
            }
            events_claimed_by_a_leaf(&b.children, claimed);
        }
    }

    #[test]
    fn completeness_proxy_detects_an_inline_event_dropped_from_every_leaf() {
        // A heading, a paragraph, *and* a thematic break — so `events_claimed_by_a_leaf`'s own match
        // exercises all three of its arms (`Heading`, `Paragraph`, and the `_ => None` fallback for
        // every other `BlockKind`, `ThematicBreak` here), not just the one this proxy happens to
        // corrupt below.
        let src = "# Title\n\nhello world\n\n---\n";
        let doc = Doc::parse(src);
        assert_eq!(
            doc.blocks.len(),
            3,
            "heading, paragraph, thematic break: {:?}",
            doc.blocks
        );
        let mut claimed = vec![false; doc.events.len()];
        events_claimed_by_a_leaf(&doc.blocks, &mut claimed);
        assert!(
            claimed.iter().any(|&c| c),
            "sanity: the real model should claim at least one event"
        );

        // Corrupt a copy: truncate the paragraph's own `inline` range so it drops its own trailing
        // content — exactly what "an inline event belongs to no leaf" looks like.
        let mut corrupted = doc.blocks.clone();
        let BlockKind::Paragraph { inline } = &mut corrupted[1].kind else {
            panic!("expected the paragraph at index 1: {:?}", corrupted[1].kind)
        };
        inline.end = inline.start;
        let mut claimed_after = vec![false; doc.events.len()];
        events_claimed_by_a_leaf(&corrupted, &mut claimed_after);
        assert!(
            claimed
                .iter()
                .zip(claimed_after.iter())
                .any(|(&before, &after)| before && !after),
            "corrupting the paragraph's own inline range should un-claim at least one event the \
             real model claims"
        );
    }

    fn count_task_list_markers_in_events(doc: &Doc<'_>) -> usize {
        doc.events
            .iter()
            .filter(|(ev, _)| matches!(ev, Event::TaskListMarker(_)))
            .count()
    }

    fn count_tasks_recorded_in_tree(blocks: &[Block]) -> usize {
        let mut n = 0;
        for b in blocks {
            if let BlockKind::ListItem { task: Some(_) } = &b.kind {
                n += 1;
            }
            n += count_tasks_recorded_in_tree(&b.children);
        }
        n
    }

    #[test]
    fn completeness_proxy_detects_a_task_list_marker_dropped_from_the_tree() {
        let src = "- [ ] a\n- [x] b\n";
        let doc = Doc::parse(src);
        assert_eq!(
            count_task_list_markers_in_events(&doc),
            2,
            "sanity: two markers in the raw event stream"
        );
        assert_eq!(
            count_tasks_recorded_in_tree(&doc.blocks),
            2,
            "sanity: the real model records both"
        );

        // Corrupt a copy: drop one item's own recorded task, as if `take_task_marker` had missed it.
        let mut corrupted = doc.blocks.clone();
        let BlockKind::ListItem { task } = &mut corrupted[0].children[0].kind else {
            panic!("expected the first list item")
        };
        *task = None;
        assert_eq!(
            count_tasks_recorded_in_tree(&corrupted),
            1,
            "the corrupted copy under-counts relative to the raw stream's own marker count"
        );
        assert_ne!(
            count_task_list_markers_in_events(&doc),
            count_tasks_recorded_in_tree(&corrupted),
            "a TaskListMarker present in the raw stream but absent from every ListItem.task is \
             exactly the completeness gap this proxy exists to catch"
        );
    }

    fn count_code_block_starts_in_raw_events(src: &str) -> usize {
        Parser::new_ext(src, parse_options())
            .into_offset_iter()
            .filter(|(ev, _)| matches!(ev, Event::Start(Tag::CodeBlock(_))))
            .count()
    }

    fn count_code_blocks_recorded_in_tree(blocks: &[Block]) -> usize {
        let mut n = 0;
        for b in blocks {
            if matches!(b.kind, BlockKind::CodeBlock { .. }) {
                n += 1;
            }
            n += count_code_blocks_recorded_in_tree(&b.children);
        }
        n
    }

    #[test]
    fn completeness_proxy_detects_a_code_block_dropped_from_the_tree() {
        let src = "```rust\nfn a() {}\n```\n\npara\n\n```\nplain\n```\n";
        let doc = Doc::parse(src);
        let raw = count_code_block_starts_in_raw_events(src);
        assert_eq!(
            raw, 2,
            "sanity: two Start(CodeBlock) events in the raw stream"
        );
        assert_eq!(
            count_code_blocks_recorded_in_tree(&doc.blocks),
            2,
            "sanity: the real model records both"
        );

        // Corrupt a copy: replace one CodeBlock with an unrelated leaf kind, as if
        // `parse_container`'s own `Tag::CodeBlock` arm had been skipped instead of building a
        // `Block` for it.
        let mut corrupted = doc.blocks.clone();
        corrupted[0].kind = BlockKind::ThematicBreak;
        assert_eq!(
            count_code_blocks_recorded_in_tree(&corrupted),
            1,
            "the corrupted copy under-counts relative to the raw stream's own CodeBlock count"
        );
        assert_ne!(
            raw,
            count_code_blocks_recorded_in_tree(&corrupted),
            "a Start(CodeBlock) present in the raw stream but represented by no \
             BlockKind::CodeBlock anywhere in the tree is exactly the completeness gap this proxy \
             exists to catch"
        );
    }

    // ---- defensive branches reachable only through a malformed event stream, poked directly ------
    //
    // Every function below is documented, at its own definition, as having a branch "not reachable
    // for well-formed input" — a real `Doc::parse` call can never desync its own event stream enough
    // to trip these, because pulldown-cmark's own event stream is always well-formed (every opened
    // tag is eventually closed). They exist purely as principle-#3 ("never crash on any input
    // pulldown-cmark accepts") insurance. Reaching them at all means calling the private helper
    // directly, with a hand-built `Walker` deliberately positioned somewhere the function does not
    // expect — legitimate here because `Walker`/every helper below is private to this file, and
    // `#[cfg(test)] mod tests` is a descendant module of it (same privacy rule the rest of this file
    // already relies on to construct `Block`/`Task` values by hand above).

    fn walker_for(src: &str) -> Walker<'_> {
        Walker {
            iter: Parser::new_ext(src, parse_options())
                .into_offset_iter()
                .peekable(),
            events: Vec::new(),
        }
    }

    #[test]
    fn parse_blocks_raw_silently_ignores_a_top_level_task_list_marker() {
        // `parse_item_task_and_children`/`take_task_marker` are the *only* real callers that ever
        // look for a `TaskListMarker` — every genuine one is consumed by one of them before
        // `parse_blocks_raw`'s own loop ever sees it. Advancing a walker past `Start(List)` and
        // `Start(Item)` by hand, then handing it straight to `parse_blocks_raw` (bypassing
        // `parse_item_task_and_children` entirely), puts the marker exactly where its own doc
        // comment says it can never legitimately be: the very next event that loop's own `match`
        // sees. No panic, no desync — the marker is dropped (its own three bytes never claimed by
        // anything, matching `is_stray_inline_leaf`'s own doc comment on why `TaskListMarker` is
        // excluded from a stray run), and parsing continues normally past it.
        let src = "- [ ] a\n";
        let mut w = walker_for(src);
        let _ = w.next(); // Start(List)
        let _ = w.next(); // Start(Item)
        let out = parse_blocks_raw(&mut w, src);
        assert_eq!(
            out.len(),
            1,
            "the item's own text still becomes a block: {out:?}"
        );
        let BlockKind::Paragraph { inline } = &out[0].kind else {
            panic!("expected a synthetic paragraph: {:?}", out[0].kind)
        };
        assert_eq!(
            inline_plain_text(
                &Doc {
                    events: w.events.clone(),
                    blocks: Vec::new()
                },
                inline
            ),
            "a"
        );
    }

    #[test]
    fn collect_stray_inline_run_handles_an_exhausted_event_stream() {
        // Reachable only through a manufactured stream: a genuine tight-list-item stray run is
        // always followed by at least an `End(Item)` (pulldown-cmark closes every tag it opens), so
        // `events.peek()` never actually returns `None` mid-run for real input. Feeding this
        // function a walker over an *already-exhausted* parser (an empty document has zero events)
        // exercises the `None => false` arm directly — the run still ends cleanly, at the one event
        // handed in as `first`, with no panic.
        let mut w = walker_for("");
        let range = collect_stray_inline_run((Event::Text("x".into()), 3..4), &mut w);
        assert_eq!(
            range,
            3..4,
            "an exhausted stream ends the run at just its own first event"
        );
    }

    #[test]
    fn collect_code_body_spans_defensively_skips_an_unexpected_start_event() {
        // Called on a walker that never consumed the `Start(Paragraph)` opening this text — its own
        // first `next()` sees that `Start` directly, not a `Start(CodeBlock)`'s own content.
        let src = "**bold** x\n";
        let mut w = walker_for(src);
        let spans = collect_code_body_spans(&mut w);
        assert!(
            spans.is_empty(),
            "no Event::Text was ever seen at code-block level: {spans:?}"
        );
    }

    #[test]
    fn collect_code_body_spans_defensively_skips_an_event_that_is_neither_text_nor_end() {
        // Same idea as the sibling test above, but positioned to reach the function's own final
        // `_ => {}` catch-all specifically (unlike a `Start`, a bare `SoftBreak`/`End(Paragraph)` —
        // neither `Event::Text` nor `Event::End(TagEnd::CodeBlock)` nor a `Start` — has no arm of its
        // own at all): consume the opening `Start(Paragraph)` by hand first, so the walker handed to
        // `collect_code_body_spans` starts mid-paragraph, at `Text("a")`/`SoftBreak`/`Text("b")`/
        // `End(Paragraph)` directly. Two of those are genuine `Event::Text` (collected, however
        // meaningless the resulting "spans" are outside any real code block — this function has no
        // way to know it was handed the wrong context), and the other two both land on `_ => {}`.
        let src = "a\nb\n"; // a single-newline soft break inside one paragraph
        let mut w = walker_for(src);
        let _ = w.next(); // Start(Paragraph)
        let spans = collect_code_body_spans(&mut w);
        assert_eq!(
            spans.len(),
            2,
            "both Text(\"a\") and Text(\"b\") still get collected: {spans:?}"
        );
    }

    #[test]
    fn collect_table_rows_defensively_skips_an_unexpected_start_event() {
        let src = "**bold** x\n";
        let mut w = walker_for(src);
        let rows = collect_table_rows(&mut w);
        assert!(
            rows.is_empty(),
            "no TableHead/TableRow was ever seen: {rows:?}"
        );
    }

    #[test]
    fn collect_table_rows_defensively_skips_an_event_that_is_neither_a_row_kind_nor_end() {
        // Reaches `collect_table_rows`'s own final `_ => {}` the same way as
        // `collect_code_body_spans_defensively_skips_an_event_that_is_neither_text_nor_end` above.
        let src = "a\nb\n";
        let mut w = walker_for(src);
        let _ = w.next(); // Start(Paragraph)
        let rows = collect_table_rows(&mut w);
        assert!(
            rows.is_empty(),
            "no TableHead/TableRow/End(Table) event ever appears in this stream: {rows:?}"
        );
    }

    #[test]
    fn collect_row_cells_defensively_skips_an_unexpected_start_event() {
        let src = "**bold** x\n";
        let mut w = walker_for(src);
        let cells = collect_row_cells(&mut w, TagEnd::TableRow);
        assert!(cells.is_empty(), "no TableCell was ever seen: {cells:?}");
    }

    #[test]
    fn collect_row_cells_defensively_skips_an_event_that_is_neither_a_cell_nor_its_own_end() {
        // Reaches `collect_row_cells`'s own final `_ => {}` the same way as the two tests above —
        // `end` is `TagEnd::TableRow`, which the stray `End(Paragraph)` here does not match either.
        let src = "a\nb\n";
        let mut w = walker_for(src);
        let _ = w.next(); // Start(Paragraph)
        let cells = collect_row_cells(&mut w, TagEnd::TableRow);
        assert!(cells.is_empty(), "no TableCell was ever seen: {cells:?}");
    }

    #[test]
    fn collect_html_body_spans_defensively_skips_an_unexpected_start_event() {
        // Mirrors `collect_code_body_spans_defensively_skips_an_unexpected_start_event` exactly —
        // this stream is not an `HtmlBlock`'s at all (`walker_for` hands back raw events starting at
        // `Start(Paragraph)`), so the first event hits `collect_html_body_spans`'s own defensive
        // `Event::Start(t) => skip_inline_to` branch, and no `End(HtmlBlock)` ever follows to break
        // on — this pins that the walk simply stops with whatever it collected (nothing) instead of
        // panicking or looping.
        let src = "**bold** x\n";
        let mut w = walker_for(src);
        let spans = collect_html_body_spans(&mut w);
        assert!(spans.is_empty(), "no Event::Html was ever seen: {spans:?}");
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // a real body_spans is a Vec<Range<usize>>; one span is the normal case
    fn html_tag_of_reports_none_for_a_body_that_does_not_start_with_a_tag() {
        // `html_tag_of` reads no events at all (unlike the old, since-split `collect_html_tag`) — it
        // is a pure read off `body_spans`/`src` — so this pins its own scope directly: a first line
        // starting with `*`, not `<`, reports `None` via `html_tag_name`'s own early `?` (a different
        // branch than the one this test targets — see
        // `html_tag_name_reports_none_when_no_name_characters_follow_the_opening_bracket` below for
        // that one directly).
        let src = "**bold** x\n";
        let tag = html_tag_of(&[0..src.len()], src);
        assert_eq!(tag, None);
    }

    #[test]
    fn html_tag_name_reports_none_when_no_name_characters_follow_the_opening_bracket() {
        // `html_tag_name` is a pure function of a line, callable directly with no `Doc`/`Walker` at
        // all — real CommonMark HTML-block recognition never hands it a line shaped like these (a
        // "complete" open/close tag requires a valid name right after `<`/`</`), so its own
        // `if name.is_empty()` branch is otherwise unreachable through any document that actually
        // parses as an `HtmlBlock` in the first place.
        assert_eq!(
            html_tag_name("< foo>"),
            None,
            "space right after '<' leaves no name chars"
        );
        assert_eq!(html_tag_name("<"), None, "nothing at all after '<'");
        assert_eq!(
            html_tag_name("</"),
            None,
            "nothing after the closing-tag slash either"
        );
        // Controls: the ordinary, well-formed cases this function exists to handle still work.
        assert_eq!(html_tag_name("<div>"), Some("div".to_string()));
        assert_eq!(html_tag_name("</div>"), Some("div".to_string()));
        assert_eq!(html_tag_name(""), None, "doesn't even start with '<'");
        assert_eq!(
            html_tag_name("<!doctype>"),
            None,
            "excluded by the '!' guard, not this branch"
        );
    }

    // ---- `glued_details_fold`'s own two "give up" arms, reached through real `Doc::parse` input ---
    //
    // Unlike the previous section, every case below *is* reachable through ordinary `Doc::parse` —
    // no hand-built `Walker` needed — because pulldown-cmark really does glue an unclosed or
    // trailing-content `<details>` block into one literal `HtmlBlock`; `fold_details` really does
    // call `glued_details_fold` on it (whenever it is the last sibling at its own level, whether or
    // not it happens to also contain its own close — see `fold_details`'s own doc comment on why
    // `rest.is_empty()` alone is enough to try).

    #[test]
    fn unclosed_glued_details_with_no_close_anywhere_in_the_document_stays_unfolded() {
        // No `</details>` anywhere in the whole (single-block) document — `glued_details_fold`'s own
        // scan runs off the end of `b.src` having found nothing, returns `None`, and `fold_details`
        // falls back to leaving the raw `Html` leaf exactly as it was.
        let src = "<details>\n<summary>S</summary>\nno close anywhere\n";
        let doc = Doc::parse(src);
        assert_eq!(doc.blocks.len(), 1);
        assert!(matches!(
            doc.blocks[0].kind,
            BlockKind::Html { tag: Some(ref t), .. } if t == "details"
        ));
    }

    #[test]
    fn glued_details_with_unrelated_content_after_the_close_stays_unfolded() {
        // A close *is* found this time (`</details>` on line 3), but the same glued `HtmlBlock`
        // keeps running past it (no blank line separates "extra content here" from the close
        // either) — `glued_details_fold` refuses to silently drop that trailing text, and
        // `fold_details` leaves the whole thing as one unfolded `Html` leaf rather than truncating
        // `Block::src` and orphaning bytes with no sibling slot to carry them.
        let src = "<details>\n<summary>A</summary>\n</details>\nextra content here\n";
        let doc = Doc::parse(src);
        assert_eq!(doc.blocks.len(), 1);
        assert!(matches!(
            doc.blocks[0].kind,
            BlockKind::Html { tag: Some(ref t), .. } if t == "details"
        ));
        assert_eq!(
            &src[doc.blocks[0].src.clone()],
            src,
            "the whole glued leaf is kept intact"
        );
    }

    #[test]
    fn unclosed_details_with_no_trailing_newline_stays_unfolded() {
        // The open tag is *also* the entire input, with no trailing `'\n'` at all — `line_after`'s
        // own "no '\n' found" fallback (`src.len()`) and `glued_details_fold`'s own "ran off the
        // end" arm both fire from this one case.
        let src = "<details>";
        let doc = Doc::parse(src);
        assert_eq!(doc.blocks.len(), 1);
        assert!(matches!(
            doc.blocks[0].kind,
            BlockKind::Html { tag: Some(ref t), .. } if t == "details"
        ));
        assert_eq!(doc.blocks[0].src, 0..src.len());
    }

    // ---- Details ordinal contract vs. `collect_details_open` (markdown.rs) ------------------------

    /// Depth-first, source-order collection of every `BlockKind::Details`'s own `open_attr` — the
    /// model's own analogue of `collect_details_open`'s return value, used only by the two tests
    /// below to compare the two.
    fn collect_model_details_open(blocks: &[Block], out: &mut Vec<bool>) {
        for b in blocks {
            if let BlockKind::Details { open_attr, .. } = &b.kind {
                out.push(*open_attr);
            }
            collect_model_details_open(&b.children, out);
        }
    }

    #[test]
    fn model_details_ordinal_matches_collect_details_open_for_well_formed_non_quote_nesting() {
        // `collect_details_open` (`markdown.rs`) is the render pipeline's own scanner for seeding a
        // `<details>` block's default open/closed toggle state — see its own doc comment. This
        // model's `BlockKind::Details` sequence (walked depth-first, in source order) must agree
        // with it for every shape the two are documented, or empirically confirmed, to fold
        // identically: well-formed and blank-line-separated (at the top level, nested inside a list
        // item, several deep), unclosed, and "first close wins" swallowing of an inner `<details>`
        // glued into an *outer* one's own well-formed body.
        //
        // Two shapes are deliberately **excluded** here — not because they are hard to write, but
        // because the two sides are already known to disagree about them (found empirically while
        // writing this test, not merely inferred from a doc comment):
        //
        //   * a `<details>` nested inside a **block quote** — see
        //     `model_details_ordinal_diverges_from_collect_details_open_for_a_quote_nested_details`
        //     below, which pins the divergence directly and explains it.
        //   * a `<details>` glued (no blank line anywhere) to a *further nested* `<details>`, itself
        //     also glued with no blank line: this model deliberately leaves the whole construct as
        //     one unfolded `Html` leaf (see
        //     `glued_details_with_no_blank_line_anywhere_stays_an_unfolded_html_leaf`, above) — there
        //     is no separate sibling boundary for `fold_details`'s "first close wins" search to have
        //     found in the first place — while `split_details`'s own, differently-shaped raw-line
        //     scan folds it anyway. A pre-existing, intentional divergence for this one pathological
        //     shape (confirmed directly: `model` reports `[]`, `collect_details_open` reports
        //     `[false]`, for `"<details>\n<summary>A</summary>\n<details>\n<summary>Nested\
        //     </summary>\n</details>\n</details>\n"`), not something this test asserts parity on.
        let cases = [
            "<details>\n<summary>S</summary>\n\nbody\n\n</details>\n",
            "<details>\n<summary>A</summary>\n\na\n\n</details>\n\n\
             <details open>\n<summary>B</summary>\n\nb\n\n</details>\n",
            "- item\n\n  <details>\n  <summary>S</summary>\n\n  body\n\n  </details>\n",
            "<details>\n<summary>S</summary>\n\nbody one\n\nbody two\n",
            "<details>\n<summary>Outer</summary>\n\n\
             <details open>\n<summary>Inner</summary>\n\ninner body\n\n</details>\n\n\
             outer body after inner\n\n</details>\n",
            "- item\n\n  <details>\n  <summary>Outer</summary>\n\n  body\n\n  </details>\n\n\
             - item2\n\n  <details open>\n  <summary>Second</summary>\n\n  body2\n\n  </details>\n",
            "<details open>\n<summary>A</summary>\n\na\n\n</details>\n\n\
             <details>\n<summary>B</summary>\n\nb\n\n</details>\n\n\
             <details open>\n<summary>C</summary>\n\nc\n\n</details>\n",
        ];
        for src in cases {
            let doc = Doc::parse(src);
            let mut model_open: Vec<bool> = Vec::new();
            collect_model_details_open(&doc.blocks, &mut model_open);
            let split_open = super::super::collect_details_open(src);
            assert_eq!(model_open, split_open, "case {src:?}");
        }
    }

    #[test]
    fn model_details_ordinal_diverges_from_collect_details_open_for_a_quote_nested_details() {
        // Documents, rather than papers over, a real gap: `collect_details_open`'s own doc comment
        // explains that a `<details>` reachable only through a GitHub *alert*'s body is invisible to
        // it, "because `split_details` sees the raw, not-yet-alert-stripped source, where such a
        // block's opening tag is still `>`-prefixed and so does not match `details_open_tag`". The
        // identical mechanism — the raw line still carries a `>` prefix — applies just as much to a
        // *plain*, non-alert block quote, which that doc comment does not separately call out. This
        // model still folds it correctly (pulldown-cmark's own event stream has already stripped the
        // `>` by the time this model ever sees the text), so the two genuinely disagree here — worth
        // flagging as a discovered gap in `collect_details_open`'s own documented contract, not
        // something this file can fix (it lives in `markdown.rs`).
        let src = "> <details>\n> <summary>S</summary>\n>\n> body\n>\n> </details>\n";
        let doc = Doc::parse(src);
        let mut model_open: Vec<bool> = Vec::new();
        collect_model_details_open(&doc.blocks, &mut model_open);
        assert_eq!(
            model_open,
            vec![false],
            "the model itself still folds a quote-nested details"
        );
        assert_eq!(
            super::super::collect_details_open(src),
            Vec::<bool>::new(),
            "collect_details_open misses it — the same '>'-prefix mechanism its own doc comment \
             documents for alerts specifically also applies to a plain quote"
        );
    }

    // ---- extreme / adversarial input: `Doc::parse` must never panic, and invariants must hold ----
    //
    // Every case below is real Markdown source text handed straight to `Doc::parse` — not a
    // hand-built `Block` tree — covering shapes principle #3 (`CLAUDE.md`: "never crash on any input
    // pulldown-cmark accepts") says this model must survive: unclosed constructs, deep nesting, huge
    // single lines, huge sibling counts, CJK/emoji/combining/zero-width/RTL text, CRLF/no-trailing-
    // newline/BOM/NUL/control-byte encodings, tab/full-width-space/mixed indentation, empty/
    // whitespace-only input, and every backslash-escape form this model's own module doc comment
    // discusses (`\*` `\_` `\[` `\]` `\(` `\)` `\$` `\\`), including immediately before a multi-byte
    // character — the exact shape that has caused byte-boundary panics elsewhere in this codebase's
    // own history (see this file's own CJK/multibyte tests above, and `markdown.rs`'s own
    // `scan_inline_math`/`find_inline_dollar` fixes for the same class of bug). `Doc::parse` is run
    // through `super::super::catch_silent` (the project's own shared panic-catching helper, already
    // `pub(crate)`) so one panicking case does not stop the rest of the corpus from being checked —
    // every failure (a panic, or an invariant violation) is collected and reported together, in the
    // same "report everything wrong, not just the first" style `check_invariants` itself follows.

    /// Every extreme/adversarial `(name, source)` pair this section checks. Kept as a single,
    /// reusable list so both `doc_parse_never_panics_and_invariants_hold_across_extreme_inputs`
    /// (below) and any future addition to this corpus benefit from the same collection.
    fn extreme_input_corpus() -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        let mut push = |name: &str, src: String| v.push((name.to_string(), src));

        // -- unclosed constructs --
        push("unclosed fence", "```rust\nfn a() {}\n".to_string());
        push("unclosed fence, no lang", "```\nplain\n".to_string());
        push(
            "unclosed fence nested in a list item",
            "- item\n\n  ```rust\n  fn a() {}\n".to_string(),
        );
        push(
            "unclosed details, well-formed open/summary",
            "<details>\n<summary>S</summary>\n\nbody\n".to_string(),
        );
        push(
            "unclosed details, no summary tag",
            "<details>\nbody only\n".to_string(),
        );
        push(
            "unclosed html comment",
            "<!-- never closes\nmore text\nstill in the comment\n".to_string(),
        );
        push(
            "unclosed emphasis (asterisk)",
            "*never closes\nmore text\n".to_string(),
        );
        push(
            "unclosed emphasis (underscore)",
            "_never closes\nmore text\n".to_string(),
        );
        push("unclosed strong", "**never closes\nmore text\n".to_string());
        push(
            "unclosed link, no matching bracket",
            "[never closes (no matching bracket\n".to_string(),
        );
        push(
            "unclosed link, no matching paren",
            "[text](never closes\n".to_string(),
        );
        push(
            "unclosed inline code",
            "`never closes\nmore text\n".to_string(),
        );
        push(
            "unclosed strikethrough",
            "~~never closes\nmore text\n".to_string(),
        );
        push(
            "table header with no body rows",
            "| a | b |\n|---|---|\n".to_string(),
        );
        push(
            "table header row with no delimiter row at all",
            "| a | b |\nnot a delimiter row\n".to_string(),
        );

        // -- deep nesting --
        push("deeply nested bullet list (10 levels)", {
            let mut s = String::new();
            for i in 0..10 {
                s.push_str(&"  ".repeat(i));
                s.push_str("- level\n");
            }
            s
        });
        push("deeply nested block quote (10 levels)", {
            let mut s = "> ".repeat(10);
            s.push_str("deep quote\n");
            s
        });
        push(
            "list inside quote inside list",
            "- outer\n\n  > quoted\n  >\n  > - inner list\n  >   - inner inner\n".to_string(),
        );
        push("alternating quote/list nesting (8 levels)", {
            let mut s = String::new();
            for i in 0..8 {
                if i % 2 == 0 {
                    s.push_str(&"> ".repeat(i / 2 + 1));
                } else {
                    s.push_str(&"  ".repeat(i));
                    s.push_str("- ");
                }
            }
            s.push_str("bottom\n");
            s
        });

        // -- huge single line / many blank lines / many siblings --
        push("huge single line, no newline (~1MB)", "x".repeat(1_000_000));
        push("huge single line, unclosed emphasis (~1MB)", {
            let mut s = String::from("*");
            s.push_str(&"a".repeat(1_000_000));
            s
        });
        push("many blank lines (5000)", "\n".repeat(5000));
        push("many sibling paragraphs (2000)", {
            let mut s = String::new();
            for i in 0..2000 {
                s.push_str(&format!("para {i}\n\n"));
            }
            s
        });
        push("many sibling list items (3000)", {
            let mut s = String::new();
            for i in 0..3000 {
                s.push_str(&format!("- item {i}\n"));
            }
            s
        });

        // -- CJK / emoji / combining / zero-width / RTL --
        push(
            "cjk heavy",
            "# 見出しタイトル\n\n本文は日本語です。**強調**も*斜体*も入ります。\n\n\
             - 項目一\n- 項目二\n\n> 引用も日本語\n"
                .to_string(),
        );
        push(
            "emoji throughout",
            "# 🎉 Title 🚀\n\nHello 👋 world 🌍! `code 💻` and **bold 🔥**.\n\n- [ ] 🧪 test\n"
                .to_string(),
        );
        push(
            "combining marks stacked",
            "e\u{0301}\u{0301}\u{0301}\u{0301}\u{0301} five combining acutes on one base\n"
                .to_string(),
        );
        push(
            "zero width characters",
            "zero\u{200B}width\u{200C}space\u{200D}joiner\u{FEFF}mixed\n".to_string(),
        );
        push(
            "rtl text mixed with ltr",
            "\u{202B}שלום עולם\u{202C} mixed with English text\n".to_string(),
        );
        push(
            "rtl heading",
            "# \u{0645}\u{0631}\u{062D}\u{0628}\u{0627} Arabic heading\n".to_string(),
        );
        push(
            "cjk task list and code fence",
            "- [ ] 日本語のタスク\n- [x] 完了したタスク\n\n```rust\nfn 関数() -> 文字列 {}\n```\n"
                .to_string(),
        );

        // -- CRLF / no trailing newline / BOM / NUL / control chars --
        push(
            "crlf throughout, mixed constructs",
            "# Title\r\n\r\nBody line one\r\nBody line two\r\n\r\n\
             - item one\r\n- item two\r\n\r\n```rust\r\nfn a() {}\r\n```\r\n\r\n\
             > quoted\r\n> line two\r\n"
                .to_string(),
        );
        push(
            "no trailing newline, plain text",
            "just text, no newline at all".to_string(),
        );
        push(
            "no trailing newline, heading",
            "# Title, no newline".to_string(),
        );
        push(
            "no trailing newline, fenced code",
            "```\ncode\n```".to_string(),
        );
        push("no trailing newline, list item", "- one\n- two".to_string());
        push(
            "bom prefixed document",
            "\u{FEFF}# Title\n\nBody\n".to_string(),
        );
        push("nul byte embedded", "before\u{0}after\n".to_string());
        push(
            "assorted c0 control chars",
            "col1\u{1}col2\u{2}col3\u{7}bell\u{8}backspace\n".to_string(),
        );

        // -- tab / full-width space / mixed indentation --
        push(
            "tab-indented code block",
            "para\n\n\tline one\n\tline two\n".to_string(),
        );
        push(
            "tab after a list marker",
            "-\ttab then item text\n".to_string(),
        );
        push(
            "fullwidth space, not real indentation",
            "　　- looks indented but is U+3000, not ascii space\n".to_string(),
        );
        push(
            "mixed tab and space indentation",
            "- item\n \t - mixed indent nested item\n".to_string(),
        );

        // -- empty / whitespace-only --
        push("empty string", String::new());
        push("single space", " ".to_string());
        push(
            "whitespace only (spaces and tabs)",
            "   \t   \t\t  ".to_string(),
        );
        push("single newline only", "\n".to_string());
        push("many newlines only", "\n\n\n\n\n\n\n\n".to_string());

        // -- escapes: every form this model's own module doc comment discusses --
        for esc in ["\\*", "\\_", "\\[", "\\]", "\\(", "\\)", "\\$", "\\\\"] {
            push(
                &format!("escape {esc:?} mid-text"),
                format!("text {esc} more text\n"),
            );
            push(
                &format!("escape {esc:?} at start of line"),
                format!("{esc} more text\n"),
            );
            push(
                &format!("escape {esc:?} at end of input, no newline"),
                format!("text {esc}"),
            );
        }
        // The one class of bug this codebase has actually hit before (see `CLAUDE.md`'s own record
        // of a `\` + multibyte byte-boundary panic in a *different* scanner, `scan_inline_math`):
        // a backslash immediately followed by a multi-byte character.
        push(
            "backslash immediately before cjk",
            "before \\あ after\n".to_string(),
        );
        push(
            "backslash immediately before emoji",
            "before \\🎉 after\n".to_string(),
        );
        push(
            "backslash immediately before combining mark",
            "before \\\u{0301} after\n".to_string(),
        );
        push(
            "input ends with a lone trailing backslash",
            "text ends with backslash\\".to_string(),
        );
        push(
            "currency dollar, not math (ENABLE_MATH is off anyway)",
            "price is \\$5 not math\n".to_string(),
        );
        push(
            "raw dollar-delimited text, unprocessed",
            "inline $x + y$ and $$z$$ display\n".to_string(),
        );
        push(
            "raw footnote-shaped text, unprocessed",
            "See [^1] here.\n\n[^1]: definition\n".to_string(),
        );

        v
    }

    #[test]
    fn doc_parse_never_panics_and_invariants_hold_across_extreme_inputs() {
        let mut failures: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for (name, src) in extreme_input_corpus() {
            checked += 1;
            // Not a `move` closure: `name`/`src` are only ever borrowed here, so both are still
            // usable afterward to build a failure message — `catch_silent`'s own `f: impl FnOnce()
            // -> T` has no `'static` bound, so a borrowing closure is fine.
            let result = super::super::catch_silent(|| {
                let doc = Doc::parse(&src);
                let mut out = Vec::new();
                check_invariants(&doc.blocks, &src, doc.events.len(), None, &name, &mut out);
                out
            });
            match result {
                None => failures.push(format!("{name:?}: PANICKED")),
                Some(violations) if !violations.is_empty() => {
                    failures.push(format!(
                        "{name:?}: {} invariant violation(s): {violations:?}",
                        violations.len()
                    ));
                }
                Some(_) => {}
            }
        }
        assert!(
            checked > 60,
            "the extreme-input corpus looks suspiciously small ({checked}) — did a `push` call \
             break?"
        );
        assert!(
            failures.is_empty(),
            "{} case(s) failed out of {checked}:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn doc_parse_never_panics_on_deeply_nested_alternating_containers() {
        // A single, especially adversarial case pulled out on its own (rather than folded silently
        // into the corpus above) so a regression here fails with an obviously specific test name:
        // 15 levels of quote-wrapping-list-wrapping-quote, ending in a task item, a fenced code
        // block, and a table, none of them closed.
        let mut src = String::new();
        for i in 0..15 {
            if i % 2 == 0 {
                src.push_str("> ");
            } else {
                src.push_str("- ");
            }
        }
        src.push_str("- [ ] deep task\n");
        for i in 0..15 {
            let prefix = if i % 2 == 0 { "> " } else { "  " };
            src.push_str(prefix);
        }
        src.push_str("```rust\nfn deep() {}\n");
        let doc = super::super::catch_silent(|| Doc::parse(&src));
        let doc = doc.expect("Doc::parse must not panic on deep, unclosed, alternating nesting");
        let mut out = Vec::new();
        check_invariants(
            &doc.blocks,
            &src,
            doc.events.len(),
            None,
            "deeply_nested_alternating",
            &mut out,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    // ---- `CodeBlock.body_spans` and `Task.state_at` contract, hardened across extreme containers --
    //
    // The two fields a future in-place "copy this code block" / "toggle this checkbox" feature would
    // slice the input directly from — see `BlockKind::CodeBlock.body_spans`'s and `Task::state_at`'s
    // own doc comments. Both are checked here across every combination their own doc comments call
    // out as needing a *list* of spans (rather than one contiguous range) in the first place: fenced
    // and indented, nested inside a list item / block quote / GFM alert / `<details>` body, under
    // CRLF line endings, with a literal tab as *content* (inside a fence, a tab is just a content
    // byte, not indentation), and with CJK content. Every expected reconstruction below was confirmed
    // by first printing `Doc::parse`'s own actual output for that exact source (not merely assumed)
    // before being pinned as an assertion — the reconstruction rules for quote/list continuation-line
    // indentation and CRLF normalization are exactly the subtle, easy-to-get-wrong shape this whole
    // model exists to get right once, in one place (see the module doc comment's own "Why this
    // exists" section).

    #[test]
    fn code_block_body_spans_reconstruct_byte_exact_content_across_extreme_containers() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "fenced code nested inside a block quote",
                "> ```rust\n> fn a() {}\n> ```\n",
                "fn a() {}",
            ),
            (
                "fenced code nested inside a GFM alert",
                "> [!WARNING]\n> ```\n> code in alert\n> ```\n",
                "code in alert",
            ),
            (
                "fenced code nested inside a details body",
                "<details>\n<summary>S</summary>\n\n```rust\nfn a() {}\n```\n\n</details>\n",
                "fn a() {}",
            ),
            (
                "fenced code nested inside a list item, CRLF throughout",
                "- item\r\n\r\n  ```rust\r\n  fn a() {}\r\n  ```\r\n",
                "fn a() {}",
            ),
            (
                "indented code, CRLF throughout",
                "para\r\n\r\n    line one\r\n    line two\r\n",
                "line one\nline two",
            ),
            (
                "indented code nested inside a block quote",
                "> para\n>\n>     indented code\n>     more code\n",
                "indented code\nmore code",
            ),
            (
                "cjk code content",
                "```rust\nfn 関数() -> 文字列 {}\n```\n",
                "fn 関数() -> 文字列 {}",
            ),
            (
                "a literal tab as fenced code content, not indentation",
                "```\n\tindented with tab inside fence\n```\n",
                "\tindented with tab inside fence",
            ),
            (
                "empty fenced code nested inside a list item",
                "- item\n\n  ```\n  ```\n",
                "",
            ),
        ];
        for (name, src, expected) in cases {
            let doc = Doc::parse(src);
            let mut spans_by_block: Vec<Vec<Range<usize>>> = Vec::new();
            code_block_bodies_ignoring_quote_flag(&doc.blocks, &mut spans_by_block);
            assert_eq!(
                spans_by_block.len(),
                1,
                "case {name:?}: expected exactly one code block"
            );
            let actual = code_body_text(&spans_by_block[0], src);
            assert_eq!(actual, *expected, "case {name:?}");
            // Every reconstructed span also has to satisfy the checker's own invariants — belt and
            // suspenders on top of the byte-exact content match above.
            let mut violations = Vec::new();
            check_invariants(
                &doc.blocks,
                src,
                doc.events.len(),
                None,
                name,
                &mut violations,
            );
            assert!(violations.is_empty(), "case {name:?}: {violations:?}");
        }
    }

    /// Like `code_block_bodies` above, but collecting only the `body_spans` themselves (not the
    /// quote-nesting flag) — used by the extreme-containers test above, which deliberately *does*
    /// want quote-nested code blocks included (unlike `parser_code_blocks`'s own convention).
    fn code_block_bodies_ignoring_quote_flag(blocks: &[Block], out: &mut Vec<Vec<Range<usize>>>) {
        for b in blocks {
            if let BlockKind::CodeBlock { body_spans, .. } = &b.kind {
                out.push(body_spans.clone());
            }
            code_block_bodies_ignoring_quote_flag(&b.children, out);
        }
    }

    #[test]
    fn task_state_at_contract_holds_across_extreme_containers() {
        let cases = [
            (
                "task list nested inside a block quote",
                "> - [ ] a\n> - [x] b\n> - [X] c\n",
            ),
            (
                "task list nested inside a GFM alert",
                "> [!NOTE]\n> - [ ] a\n> - [x] b\n> - [X] c\n",
            ),
            (
                "task list nested inside a details body",
                "<details>\n<summary>S</summary>\n\n- [ ] a\n- [x] b\n- [X] c\n\n</details>\n",
            ),
            (
                "task list, CRLF throughout",
                "- [ ] a\r\n- [x] b\r\n- [X] c\r\n",
            ),
            (
                "task list, cjk label text",
                "- [ ] 日本語のタスク\n- [x] 完了したタスク\n",
            ),
            (
                "task list nested two list levels deep",
                "- outer\n  - [ ] nested a\n  - [x] nested b\n",
            ),
        ];
        for (name, src) in cases {
            let doc = Doc::parse(src);
            let mut tasks: Vec<Task> = Vec::new();
            collect_tasks(&doc.blocks, &mut tasks);
            assert!(
                !tasks.is_empty(),
                "case {name:?}: expected at least one task item"
            );
            for t in &tasks {
                assert!(
                    src.is_char_boundary(t.state_at),
                    "case {name:?}: state_at {} not on a char boundary",
                    t.state_at
                );
                assert!(
                    src[t.state_at..].starts_with(t.state),
                    "case {name:?}: byte at state_at does not match state {:?}",
                    t.state
                );
                assert_eq!(
                    src.as_bytes().get(t.state_at - 1),
                    Some(&b'['),
                    "case {name:?}: byte before state_at is not '['"
                );
                assert_eq!(
                    src.as_bytes().get(t.state_at + 1),
                    Some(&b']'),
                    "case {name:?}: byte after state_at is not ']'"
                );
            }
            // Same belt-and-suspenders as the code-block test above: `check_invariants` itself must
            // also see the whole tree as clean.
            let mut violations = Vec::new();
            check_invariants(
                &doc.blocks,
                src,
                doc.events.len(),
                None,
                name,
                &mut violations,
            );
            assert!(violations.is_empty(), "case {name:?}: {violations:?}");
        }
    }

    fn collect_tasks(blocks: &[Block], out: &mut Vec<Task>) {
        for b in blocks {
            if let BlockKind::ListItem { task: Some(t) } = &b.kind {
                out.push(*t);
            }
            collect_tasks(&b.children, out);
        }
    }
}
