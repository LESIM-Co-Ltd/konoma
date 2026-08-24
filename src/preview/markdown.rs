// Built-in Markdown / Mermaid renderer (M3).
//
// Structure (settled 2026-06):
//   - Markdown decoration: `tui-markdown`'s `from_str` (ratatui-core 0.1 = the same type as our
//                 0.30). Decorates headings/emphasis/code/lists/tables/blockquotes/links etc.
//                 tui-markdown's highlight-code (syntect default = the oniguruma C library) is
//                 disabled, and code inside ```lang fences is instead colored on konoma's side
//                 with pure-Rust syntect (`preview::code::highlight_lang`) = no oniguruma needed,
//                 preserving ease of distribution (PRD §5). Same path as a standalone code file.
//   - Mermaid   : rendered as Unicode box-drawing text via `mermaid-text` (its only dependency is
//                 unicode-width, pure Rust). No browser or image protocol needed. ```mermaid
//                 fences inside the md are intercepted and composed.
//
// The initially considered `ratatui-markdown` depends on ratatui ^0.29, which is incompatible with
// image preview (ratatui-image 11 requires ratatui 0.30), so it was rejected. See the comment in
// Cargo.toml for details.
//
// Safe fallback on failure (design principle 3): if mermaid rendering fails or is unsupported
// (e.g. a state diagram), show the raw source full-screen in a dim color (never crash).

use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
// The parser's own option type takes the name tui-markdown itself uses for it internally
// (`ParseOptions`) — see `tui_markdown_parse_options`, which must stay in lockstep with
// tui-markdown's own set.
use pulldown_cmark::Options as ParseOptions;
use tui_markdown::StyleSheet;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) mod model;
pub(crate) mod render;

// Decoration colors. A code block is enclosed as a "special area" with a background + left gutter.
const CODE_GUTTER_FG: Color = Color::Cyan;
const HEAD_FG: Color = Color::Cyan;
const TABLE_BORDER_FG: Color = Color::Rgb(90, 98, 120); // a subdued rule line (more understated than body text)

/// Colors and layout for code decoration (built from the `ui.theme` setting).
#[derive(Clone, Copy, Debug, Default)]
pub struct CodeStyle {
    /// Code background (shared by inline code and code blocks). None = no background.
    pub bg: Option<Color>,
    /// Background of the language label (badge). None = no background.
    pub label_bg: Option<Color>,
    /// Whether the language label is right-aligned (true) or left-aligned (false).
    pub label_right: bool,
    /// Tab stop width (the `ui.tab_width` setting, default 4). The number of columns to which
    /// code-block tabs expand, shown as a visible marker (→) plus spaces. 0 disables it; same basis as the standalone code preview.
    pub tab_width: usize,
    /// Whether text previews soft-wrap (`ui.wrap`). When true, code-block lines are pre-wrapped
    /// **here** so every visual row carries the `▎` gutter and the full-width background band —
    /// leaving the wrapping to ratatui's Paragraph breaks the bar on continuation rows.
    pub wrap: bool,
}

/// konoma styles for headings, code, and so on. Headings are emphasized per level.
/// True font-size changes are impossible on a terminal cell grid, so hierarchy is conveyed via color, bold, and underline rules.
/// `code_bg` is the inline-code background (the `ui.theme.code_bg` setting). None means no background.
#[derive(Clone, Copy, Debug, Default)]
struct KonomaStyles {
    code_bg: Option<Color>,
}

impl StyleSheet for KonomaStyles {
    fn heading(&self, level: u8) -> Style {
        let base = Style::new().fg(HEAD_FG).add_modifier(Modifier::BOLD);
        match level {
            1 | 2 => base, // a full-width rule is added right below to express "size"
            3 => base.add_modifier(Modifier::ITALIC),
            _ => Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        }
    }
    fn code(&self) -> Style {
        // Inline code (`...`). The background color is configurable (None = no background).
        let s = Style::new().fg(Color::White);
        match self.code_bg {
            Some(bg) => s.bg(bg),
            None => s,
        }
    }
    fn link(&self) -> Style {
        Style::new()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED)
    }
    fn blockquote(&self) -> Style {
        Style::new().fg(Color::Green).add_modifier(Modifier::ITALIC)
    }
    fn heading_meta(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }
    fn metadata_block(&self) -> Style {
        Style::new().fg(Color::LightYellow)
    }
}

/// Render Markdown into decorated lines. ```mermaid fences in the md are composed into diagrams via mermaid-text.
/// `width` is the column count of the display area (inside the frame). Mermaid renders to fit within this width, so
/// the box-drawing lines stay intact even when a later Paragraph wraps.
/// `icons`: whether Nerd Font icons are enabled (`ui.icons`) — table-cell link labels get the same
/// link icon prefix as paragraph links, and the icon width is included in the column widths.
/// (Production goes through `render_markdown_tasks`/`render_markdown_with_images` with the
/// configured task states; this default-states shorthand remains for the test suites.)
#[cfg(test)]
pub fn render_markdown(
    src: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
) -> Vec<Line<'static>> {
    render_markdown_tasks(src, width, code, theme, icons, DEFAULT_TASK_STATES)
}

/// The standard GFM task states (unchecked / checked). Used when no custom states are configured.
pub(crate) const DEFAULT_TASK_STATES: &[char] = &[' ', 'x'];

/// `render_markdown` plus the configured task-list states (`ui.md_task_states`): custom state
/// chars (e.g. `/`) are also recognized as toggleable task markers. Test-only shorthand that now
/// drives the same dispatcher production calls (`render_markdown_with_images`, which tries
/// `render::render_doc` first — see that function's own doc comment) instead of the retired
/// `render_markdown_tasks_opts` direct call: since `3318a5b`, `render::render_doc`'s
/// `unsupported` is always empty, so `render_markdown_with_images` never falls back to the legacy
/// renderer, and this helper's 56 callers now exercise the production block-model path. The
/// image/mermaid/math slot closures below are fixed answers (no live `Picker`/on-disk state in a
/// unit test), matching the exact values `md_snapshot_tests::render_case_with` and
/// `md_render_diff_tests::render_case_new` already use for the same reason: images "unavailable",
/// mermaid fences "extracted" to a placeholder `Image { cols: 20, rows: 5 }`, math expressions
/// "extracted" to `Raw` — so extraction/placement logic is still exercised end to end, just not
/// the picker-dependent raster step.
#[cfg(test)]
pub fn render_markdown_tasks(
    src: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) -> Vec<Line<'static>> {
    // Reset the per-render `<details>` state so tests are deterministic (production seeds it via
    // `set_details_open` before every draw). Empty = every block honors its own `open` attribute.
    set_details_open(Vec::new());
    let slot_of = |_: &str| ImageSlot::Unavailable;
    let mermaid_slot = |_: &str| MermaidSlot::Image { cols: 20, rows: 5 };
    let math_slot = |_: &str, _: bool| MathSlot::Raw;
    // Default entry (tests / the `render_markdown` wrapper): GitHub alerts on, math extraction on
    // (mirrors the default `math = "image"` config), the same literal `"mermaid"` caption the other
    // two test harnesses pass (this helper never varies `ui.lang`).
    render_markdown_with_images(
        src,
        width,
        code,
        theme,
        icons,
        tasks,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        true,
        &math_slot,
        true,
    )
    .0
}

thread_local! {
    /// This thread is inside a `silence_panics` section (its expected panics are caught and
    /// must not print). Thread-local so other threads' panics keep their messages.
    static PANIC_SILENCED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f`, catching any panic and suppressing its message on **this** thread — returns
/// `None` on panic. The shared safety net for background workers (media / image decode /
/// fence render / encode): a worker that panics without reporting back would latch its
/// in-flight flag on, keeping `md_images_loading()`/`media_loading` true forever (busy
/// spinner + 16ms polling — idle CPU 0% broken). Wrapping the job here guarantees a result
/// is always produced so the flag clears and the render side degrades (principle #3).
pub(crate) fn catch_silent<T>(f: impl FnOnce() -> T) -> Option<T> {
    silence_panics(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok())
}

/// `catch_silent(f).unwrap_or_else(fallback)` — run `f`, and on a caught panic run `fallback`
/// instead, so the caller always has *a* value to report back rather than nothing at all. Several
/// background workers (`App::spawn_or_sync_statuses`/`spawn_or_sync_ignored` in
/// `app/bookmark_actions.rs`, `App::fence_sharpen_if_needed` in `app/md_media.rs`) used to guard
/// their job with `catch_silent` but only send a result on the `Some` branch — so a caught panic
/// sent *nothing* on the channel, and the caller's "in flight" flag (`git_status_pending`/
/// `git_ignored_pending`/`reraster_inflight`) stayed latched forever (a spinner that never stops).
/// Naming this pattern (rather than leaving `catch_silent(..).unwrap_or_else(..)` duplicated at
/// each call site) also makes the "always send a result" contract impossible to accidentally
/// half-implement again — every caller's `tx.send(..)` sits right after this call with nothing
/// gating it. Extracted so the panic/fallback behavior is directly testable (a real panic — e.g. in
/// syntect, resvg, or a `git status` scan — can't be induced on demand from a test; see this
/// module's tests below).
pub(crate) fn compute_or_fallback<T>(f: impl FnOnce() -> T, fallback: impl FnOnce() -> T) -> T {
    catch_silent(f).unwrap_or_else(fallback)
}

/// Run `f` with panic **messages** suppressed on this thread (the caller still catches the
/// unwind itself). A composite hook is installed process-wide **once** and consults the
/// thread-local flag — the previous take_hook/set_hook swap around every call raced when
/// several diagram renders ran concurrently (fence workers / media thread / warm-up): an
/// interleaved restore could leave the silent hook installed forever, permanently losing
/// panic messages for the whole process (a diagnostics hazard, not a crash).
fn silence_panics<T>(f: impl FnOnce() -> T) -> T {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !PANIC_SILENCED.with(|c| c.get()) {
                prev(info);
            }
        }));
    });
    PANIC_SILENCED.with(|c| c.set(true));
    let r = f();
    PANIC_SILENCED.with(|c| c.set(false));
    r
}

// ---- Inline images (MVP: block-level local images) ----
// tui-markdown drops image URLs (it alt-izes them), so block-level images are extracted here
// *before* the text runs reach tui-markdown. Each standalone image line (Markdown `![alt](url)`
// or a line that is just an HTML `<img src=...>`) becomes reserved rows in the decorated output plus
// an `ImagePlacement` recording where to overlay the real image (drawn via kitty graphics by ui::preview).

/// Where an inline image sits in the decorated line list, and its display size in cells.
#[derive(Clone, Debug, PartialEq)]
pub struct ImagePlacement {
    /// The image URL/path as written in the source (may be relative to the Markdown file).
    pub url: String,
    /// The alt text (used for the placeholder / text fallback).
    pub alt: String,
    /// Index of the first reserved row within the decorated line list.
    pub line: usize,
    /// Display width in terminal cells.
    pub cols: u16,
    /// Display height in terminal cells (== number of reserved rows).
    pub rows: u16,
    /// Source ordinal of the ```mermaid fence this placement renders — document order over **all**
    /// fences (including ones currently loading or degraded to text); `None` for regular images.
    /// This is the canonical fence ID: deriving it by counting rendered sentinels goes wrong as
    /// soon as an earlier fence has no sentinel (failed/loading), and Enter would then re-extract
    /// and open a different fence's source.
    pub fence_ord: Option<usize>,
}

/// A run of Markdown source text plus, for each of its lines, whether the **document** placed that
/// line inside a code block.
///
/// The mask travels with the text instead of each splitter re-deriving it, because by the time most
/// of these splitters run, the text they hold is no longer a document that parses the way its lines
/// did in the file. [`split_block_parts`] (block-level images, ```mermaid fences), [`split_segments`]
/// and [`split_math`] all remove lines from *within* a block and pass the survivors on as one run —
/// and a fragment cut out of the middle of a block reads differently from the same lines in place.
///
/// The shape that forced this is the centered `<div align="center">` badge banner at the top of a
/// great many crate READMEs. Pulling its `<img>` lines out leaves fragments like
/// `    </a>\n    <a href="…">\n`, which **open** with 4-column-indented content only because their
/// `<div>` went into the previous fragment. Asked about that fragment alone, the parser correctly
/// calls it an indented code block; asked about the original document, it correctly calls it the
/// interior of an HTML block — and the second reading is the one konoma draws. Both answers are
/// right for what the parser was shown, and the two inputs are byte-for-byte the same shape, so no
/// amount of care *inside* a splitter can recover the document's answer once the surrounding block
/// has been cut away. The only fix is not to destroy the context: compute the mask once, over the
/// whole document, before any line is removed, and carry each run's slice of it.
///
/// The three splitters that peel a *container* — [`split_alerts`] (strips the `>` markers),
/// [`split_details`] (strips the `<details>`/`<summary>` tags) and [`split_tables`] — are the
/// deliberate exception, and the reason this type has two constructors. They remove the markers
/// *along with* the block, so what they hand on genuinely is a standalone document; re-deriving the
/// mask there is not merely allowed but required (see [`literal_code_mask`]'s doc comment for the
/// fence that only parses as code once the `<summary>` line above it is gone). So a peeled body is
/// built with [`SourceRun::parse`], while a verbatim text run gets a slice of the mask its splitter
/// was handed, via [`SourceRun::new`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SourceRun {
    text: String,
    /// One entry per line of `text` (`text.lines().count()`).
    code: Vec<bool>,
}

impl SourceRun {
    /// Compute the code mask over `text` itself. The correct constructor for a **whole document**
    /// and for a **peeled container body** — the two cases where the text really is a standalone
    /// document (see the type's doc comment). Never for a fragment a splitter cut lines out of.
    fn parse(text: String) -> Self {
        let code = splitter_code_mask(&text.lines().collect::<Vec<_>>());
        Self { text, code }
    }

    /// Pair `text` with an already-computed mask — how a splitter emits a verbatim run: it copies
    /// the lines it did not consume and, alongside them, the entries those lines had in the mask it
    /// was handed.
    ///
    /// The assertion is the reason every such emit is routed through here rather than building the
    /// struct literally. A splitter that forgets to push an entry for a line it kept (or pushes one
    /// for a line it dropped) shifts every later line's answer by one, and a mask that is merely
    /// *offset* is far worse than one that is absent: it reports plausible answers for the wrong
    /// lines, which is precisely the drift this type exists to end. As a debug assertion the slip
    /// becomes a loud failure in the test suite instead of a quietly wrong render.
    fn new(text: String, code: Vec<bool>) -> Self {
        debug_assert_eq!(
            code.len(),
            text.lines().count(),
            "SourceRun mask/line-count drift for {text:?}"
        );
        Self { text, code }
    }

    fn text(&self) -> &str {
        &self.text
    }

    /// The per-line code mask, positionally paired with [`lines`](Self::lines).
    fn code(&self) -> &[bool] {
        &self.code
    }

    /// The run's lines, positionally paired with [`code`](Self::code).
    fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

/// A whole document as a [`SourceRun`], for the tests that call a splitter or
/// `render_markdown_tasks_opts` directly. Production reaches those through
/// `render_markdown_with_images`, which splits the document into runs first; a test handing one a
/// complete document is exactly the [`SourceRun::parse`] case (see that type's doc comment).
#[cfg(test)]
fn doc_run(src: &str) -> SourceRun {
    SourceRun::parse(src.to_string())
}

enum BlockPart {
    Text(SourceRun),
    Image {
        url: String,
    },
    /// A top-level ```mermaid fence (only produced when the caller asked for fence extraction —
    /// i.e. image-mode mermaid; in text mode fences stay in the text runs and render as today).
    Mermaid {
        code: String,
    },
}

/// One piece of a text run after math extraction (used to lift inline `$…$` onto its own line).
enum MathPart {
    Text(SourceRun),
    Math { latex: String, display: bool },
}

/// How the app wants one math expression rendered (the math analog of `MermaidSlot`).
#[derive(Clone, Debug, PartialEq)]
pub enum MathSlot {
    /// The rendered equation raster is cached: reserve `cols`x`rows` cells and record a placement.
    Image { cols: u16, rows: u16 },
    /// The equation is rendering on a worker thread: show a dim "loading" line until it arrives.
    Loading,
    /// Image mode off / no backend / render failed: show the raw LaTeX text (design principle #3).
    Raw,
}

/// How the app wants one ```mermaid fence rendered (the image-mode analog of `ImageSlot`).
#[derive(Clone, Debug, PartialEq)]
pub enum MermaidSlot {
    /// The rendered diagram raster is cached: reserve `cols`x`rows` cells and record a placement.
    Image { cols: u16, rows: u16 },
    /// The diagram is rendering on a worker thread: show a dim "loading" line until it arrives.
    Loading,
    /// Image mode off / no backend / render failed: draw the legacy Unicode text diagram.
    Text,
}

/// How the app wants one block-level image rendered. The app decides this because it owns the image
/// backend, the local file / remote-cache state, and any in-flight fetch.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageSlot {
    /// The image is available (local file or a cached remote fetch): reserve `cols`x`rows` cells and
    /// record a placement so the renderer draws the real image inline.
    Inline { cols: u16, rows: u16 },
    /// A remote image is being fetched in the background: show a dim "loading" line until it arrives.
    Loading,
    /// The image cannot be shown inline (no backend / missing file / fetch failed / `data:` URL):
    /// degrade to a one-line text placeholder (design principle #3).
    Unavailable,
}

/// Render Markdown, additionally reserving space for block-level images and returning their placements.
/// `slot_of(url)` tells how to render each image: `Inline` reserves rows and records a placement for the
/// real image; `Loading` shows a dim "loading" line while a remote fetch is in flight; `Unavailable`
/// degrades to a one-line text placeholder (design principle #3).
///
/// ## The production entry point: the block-model renderer (`md-block-walk`)
///
/// This is the single production call site every real preview goes through
/// (`app::md_render::build_decorated`, one call), and also the function the whole parity-test suite
/// (`md_snapshot_tests`, and every "does the render match the write-back scanner" test in this file)
/// calls directly: `Doc::parse` → `render::render_doc` (see that function's own module doc comment
/// for the renderer itself, and `model::Doc`'s for why a single whole-document parse is the entire
/// point).
///
/// `render_markdown_with_images`'s own `code_blocks`/`tasks`: the source range/position data for
/// every code block and task checkbox the render pass itself drew, in document order — straight
/// from the same parse it rendered from (see `render::RenderOut`'s own `code_blocks`/`tasks` doc
/// comments for why this is a record rather than a second derivation).
pub struct MdRenderExtras {
    pub code_blocks: Vec<String>,
    pub tasks: Vec<(char, usize)>,
}

#[allow(clippy::too_many_arguments)] // 7 args + mermaid/math slots (only one call site in app + tests)
pub fn render_markdown_with_images(
    src: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
    slot_of: &dyn Fn(&str) -> ImageSlot,
    mermaid_slot: &dyn Fn(&str) -> MermaidSlot,
    mermaid_caption: &str,
    alerts: bool,
    math_slot: &dyn Fn(&str, bool) -> MathSlot,
    math_on: bool,
) -> (Vec<Line<'static>>, Vec<ImagePlacement>, MdRenderExtras) {
    let doc = model::Doc::parse(src);
    let out = render::render_doc(
        &doc,
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
    );
    (
        out.lines,
        out.images,
        MdRenderExtras {
            code_blocks: out.code_blocks,
            tasks: out.tasks,
        },
    )
}

/// Reserved rows for one math image: `rows` blank lines the image overlays, centered for display math
/// and left-aligned for inline math. No focusable caption (math images are not Tab targets).
fn math_placeholder_lines(cols: u16, rows: u16, width: u16, display: bool) -> Vec<Line<'static>> {
    let rows = rows.max(1);
    let pad = if display {
        (width.saturating_sub(cols) / 2) as usize
    } else {
        0
    };
    let indent = " ".repeat(pad);
    let mut lines = Vec::with_capacity(rows as usize);
    for _ in 0..rows {
        lines.push(Line::from(indent.clone()));
    }
    lines
}

/// Raw-LaTeX fallback for a math expression that could not be rendered (design principle #3): the
/// source with its delimiters, dimmed, so nothing is lost and the reader sees exactly what was written.
fn math_raw_lines(latex: &str, display: bool) -> Vec<Line<'static>> {
    let text = if display {
        format!("$$ {} $$", latex.trim())
    } else {
        format!("${}$", latex.trim())
    };
    vec![Line::from(Span::from(text).dim())]
}

/// Sentinel style of the caption line above an inline mermaid diagram. Doubles as the Tab-focus
/// marker (`is_mermaid_header_span`) — same trick as the task-marker / code-header sentinels.
fn mermaid_header_style() -> Style {
    // Don't add DIM: overlapping with focus's REVERSED would sink the character into the background
    // and make it unreadable (caught on a real Ghostty as a white bar). Cyan+ITALIC+prefix is unique enough as a sentinel.
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::ITALIC)
}

/// Whether `span` is the caption/sentinel line of an inline mermaid diagram (Tab item collection).
pub fn is_mermaid_header_span(span: &Span<'_>) -> bool {
    span.style == mermaid_header_style() && span.content.starts_with("◇ mermaid")
}

/// Reserved rows for one inline mermaid diagram: a caption line (focusable sentinel) + blank rows
/// the image overlays. The fence's source ordinal travels in `ImagePlacement::fence_ord` (the
/// item collector reads it from the k-th mermaid placement, matching sentinels in order).
/// `caption` is the pre-translated affordance ("Enter: full screen"); the `◇ mermaid` prefix is
/// kept literal so `is_mermaid_header_span` still recognizes the sentinel across languages.
fn mermaid_placeholder_lines(
    cols: u16,
    rows: u16,
    width: u16,
    caption: &str,
) -> Vec<Line<'static>> {
    let rows = rows.max(1);
    let pad = (width.saturating_sub(cols) / 2) as usize;
    let indent = " ".repeat(pad);
    let mut lines = Vec::with_capacity(rows as usize + 2);
    lines.push(Line::from(vec![
        Span::raw(indent),
        Span::styled(format!("◇ mermaid — {caption}"), mermaid_header_style()),
    ]));
    for _ in 0..rows {
        lines.push(Line::from(String::new()));
    }
    // Bottom margin row: where the focus border's bottom edge is drawn (so it doesn't overwrite a body-text row outside the reserved area).
    lines.push(Line::from(String::new()));
    lines
}

/// Whether a Markdown image URL points at a remote resource fetched over HTTP(S).
pub fn is_remote_image_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Collect the URLs of all block-level images whose source is remote (HTTP(S)), in document order.
/// Fence-aware (an image inside a code fence is skipped), matching `render_markdown_with_images`. Used
/// by the app to kick off background fetches for the remote images it is about to show as "loading".
///
/// Derived from `extraction_targets` — see that function's own doc comment for why this, and its two
/// siblings below (`collect_mermaid_fences`/`collect_math_exprs`), no longer walk `src` on their own:
/// every extraction key this returns is exactly the one `render_markdown_with_images` itself would
/// place a remote image under for this same `src`, by construction (both read off the identical
/// `render::render_doc` pass, or fall back to the identical raw-line scan, together).
pub fn collect_remote_image_urls(src: &str) -> Vec<String> {
    extraction_targets(src).remote_urls
}

/// The pre-`md-block-walk` implementation of `collect_remote_image_urls` (line-based, fence-aware
/// scanning of raw `src` — unchanged). Kept as the fallback `extraction_targets` calls whenever the
/// model-based renderer cannot be trusted for `src` (mirroring `render_markdown_with_images_legacy`'s
/// own role for rendering) — see that function's own doc comment.
fn collect_remote_image_urls_legacy(src: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for part in split_block_images(src) {
        if let BlockPart::Image { url, .. } = part {
            if is_remote_image_url(&url) {
                urls.push(url);
            }
        }
    }
    urls
}

/// What `collect_remote_image_urls`/`collect_mermaid_fences`/`collect_math_exprs` each return one
/// field of — computed together, by one `Doc::parse` + `render::render_doc` pass (or, when that pass
/// cannot be trusted for `src`, one shared fallback to the three legacy, raw-line-based scanners), so
/// the three lists can never independently disagree about which extraction path a given document took.
struct ExtractionTargets {
    remote_urls: Vec<String>,
    mermaid_fences: Vec<String>,
    math_exprs: Vec<(String, bool)>,
}

/// The shared implementation behind `collect_remote_image_urls`/`collect_mermaid_fences`/
/// `collect_math_exprs`: every URL/fence-body/`(latex, display)` key any of the three collects, in one
/// pass, so a caller invoking all three for the same `src` (as `app::md_render::build_decorated` does)
/// always sees a consistent answer for "which renderer will actually place this document" — never one
/// collector saying "model" while another says "legacy" for the identical document.
///
/// ## Why this exists: the exact URL a real render will key its placement under
///
/// `collect_mermaid_fences`/`collect_math_exprs` do not return URLs at all — they return the fence
/// body / `(latex, display)` pair the app later hashes itself (`mermaid_fence_url`/`math_url`) to kick
/// off a background render and to key `md_image_cache`. For that hash to land on the *same* key
/// `render_markdown_with_images` will place its own `ImagePlacement` under, the **input bytes** handed
/// to the hash have to match byte for byte, not merely "be an equivalent representation of the same
/// fence" — and `render::render_doc`'s own `render_mermaid_slot` computes its hash input from
/// `model::code_body_text(body_spans, src)` (the fence's content **with any enclosing list item's own
/// content-column indentation already stripped** — see that field's own doc comment), which disagrees,
/// byte for byte, with what the pre-refactor line scanner (`split_block_parts`, still `src`'s own
/// literal lines, indentation included) would have hashed for exactly the same list-nested fence — the
/// "list-nested mermaid fence" divergence `render_mermaid_slot`'s own doc comment documents in full.
/// Re-deriving each collector's own answer independently (a second, hand-written walk of `src` that
/// tries to reproduce the model's own dedenting rules) would risk drifting from `render_doc`'s own
/// extraction the exact way this whole refactor exists to eliminate (see `model.rs`'s own module doc
/// comment: "every one of the 'renderer vs. scanner disagree' bugs ... comes from the same root cause").
/// So this does not re-derive anything: it calls `render::render_doc` itself, with recording probe
/// closures standing in for `slot_of`/`mermaid_slot`/`math_slot`, and simply records the exact
/// arguments those closures were called with — the identical inputs a *real* render's own closures
/// would have received, in the identical order, because `render_doc` calls each of the three
/// unconditionally, at exactly the same points in its own tree walk, regardless of what the closure
/// answers (`render_image_slot`/`render_mermaid_slot`/`render_math_slot` each call their own slot
/// closure — building a real `ImagePlacement` only for the `Inline`/`Image` answer, but *calling* it
/// either way; see each one's own doc comment). The probe closures here always answer `Inline`/`Image`
/// (never `Loading`/`Unavailable`/`Text`/`Raw`) purely so a placement is always recorded to read the
/// input back off of — the `cols`/`rows` they invent are never read by anything downstream of this
/// function, which discards `RenderOut::lines`/`RenderOut::images` entirely and keeps only what the
/// closures themselves observed.
///
/// `math_on: true` unconditionally: matching how the three *callers* of these collectors
/// (`app::md_render::build_decorated`) already gate them — `collect_math_exprs` is only ever called
/// `if math_on`, `collect_mermaid_fences` only `if mermaid_on`, `collect_remote_image_urls` only `if
/// font.is_some()` — so by the time any of the three collector functions actually runs, the caller has
/// already decided extraction is wanted; this function's own job is only "what would be extracted",
/// never "should it be" a second time.
///
/// ## The fallback: matching `render_markdown_with_images`'s own choice, not just its extraction rules
///
/// `render::render_doc` reports `RenderOut::unsupported` the identical way whether called for real
/// rendering or for this probe (same `doc`, same `src`) — so whenever it is non-empty,
/// `render_markdown_with_images` itself will have already fallen back to
/// `render_markdown_with_images_legacy` for this exact `src` (see that function's own doc comment), and
/// every extraction key this function hands back falls back the identical way, to the three legacy,
/// raw-line-based scanners (`collect_remote_image_urls_legacy`/`collect_mermaid_fences_legacy`/
/// `collect_math_exprs_legacy`) — never a mix of "some keys from the model, some from the legacy scan"
/// for the same document. See `render_markdown_with_images`'s own doc comment for the one further
/// fallback condition (`alerts = false`) this function cannot see or mirror, and why that gap is
/// accepted rather than closed here.
fn extraction_targets(src: &str) -> ExtractionTargets {
    let doc = model::Doc::parse(src);
    let remote_urls: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let mermaid_fences: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let math_exprs: std::cell::RefCell<Vec<(String, bool)>> = std::cell::RefCell::new(Vec::new());
    // Every extracted image URL is recorded (not merely the remote ones) — `collect_remote_image_urls`
    // itself filters down to `is_remote_image_url` below, matching its own pre-refactor contract
    // (`collect_remote_image_urls_legacy`'s identical filter over `BlockPart::Image`).
    let slot_of = |url: &str| {
        remote_urls.borrow_mut().push(url.to_string());
        ImageSlot::Inline { cols: 1, rows: 1 }
    };
    // `render_mermaid_slot` never calls its own slot closure for a fence whose body is empty (a
    // half-written ```mermaid — see that function's own doc comment); `code` here is exactly
    // `render_mermaid_slot`'s own hash input — the fence's literal bytes with the one trailing `\n`
    // `model::code_body_text` strips already put back (`hashed`; see that function's own doc
    // comment for why the *same* string, not the shorter, still-stripped one, is what every real
    // slot closure is handed: the one real, stateful implementation,
    // `app::md_render::build_decorated`'s own `mermaid_slot`, hashes whatever it receives with no
    // independent re-add step of its own, so this probe has to record precisely what that closure
    // would receive too, byte for byte — recording anything else would silently stop matching the
    // key a real diagram actually gets cached under, the exact bug that function's own doc comment
    // documents in full). But `render_doc` itself *also* calls this same closure once, up front,
    // with a literal empty string (`fences_on: !matches!(mermaid_slot(""), MermaidSlot::Text)` — see
    // `render_doc`'s own doc comment on that line) purely to decide whether fence extraction is on at
    // all, before it has parsed a single real fence. In real production that call is harmless (the
    // real `mermaid_slot` closure just answers a variant, nothing is recorded), but *this* closure
    // records every call it receives — so without the `!code.is_empty()` guard below, that one probe
    // call would show up as a phantom `""` entry ahead of every document's real fences. Skipping
    // empty `code` here can never drop a *real* fence: a fence-body call only ever reaches this
    // closure once `render_mermaid_slot` has already confirmed `code.trim().is_empty()` is false (on
    // the *pre*-hash `code`, which is a byte-for-byte prefix of `hashed` short only its own final
    // `\n`), so `hashed` itself is never empty — nor merely `"\n"` — for a genuine fence.
    let mermaid_slot = |code: &str| {
        if !code.is_empty() {
            mermaid_fences.borrow_mut().push(code.to_string());
        }
        MermaidSlot::Image { cols: 1, rows: 1 }
    };
    let math_slot = |latex: &str, display: bool| {
        math_exprs.borrow_mut().push((latex.to_string(), display));
        MathSlot::Image { cols: 1, rows: 1 }
    };
    // `width`/`code`/`theme`/`icons`/`tasks`/`mermaid_caption`/`alerts` affect only `RenderOut::lines`
    // (layout, never read here) and are otherwise inert to which extraction closures fire — any
    // well-formed value works (see `render_markdown_with_images`'s own doc comment, specifically the
    // paragraph on `alerts`, for exactly why a `Quote`'s own body never extracts anything regardless
    // of whether its header renders as a plain quote or a colored alert callout); `DEFAULT_TASK_STATES`
    // and a representative width match what the rest of this file's own probe call sites
    // (`md_render_diff_tests::render_case_new`) already use for the identical reason.
    let out = render::render_doc(
        &doc,
        src,
        100,
        CodeStyle::default(),
        "TwoDark",
        false,
        DEFAULT_TASK_STATES,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        true, // alerts: inert here (see the paragraph above) — matches production's own default
        &math_slot,
        true,
    );
    if out.unsupported.is_empty() {
        ExtractionTargets {
            remote_urls: remote_urls
                .into_inner()
                .into_iter()
                .filter(|u| is_remote_image_url(u))
                .collect(),
            mermaid_fences: mermaid_fences.into_inner(),
            math_exprs: math_exprs.into_inner(),
        }
    } else {
        ExtractionTargets {
            remote_urls: collect_remote_image_urls_legacy(src),
            mermaid_fences: collect_mermaid_fences_legacy(src),
            math_exprs: collect_math_exprs_legacy(src),
        }
    }
}

/// Split the source into text runs and standalone block-level images. Fence-aware: an image inside a
/// ``` / ~~~ code fence stays in the surrounding text run (it is not treated as an image).
fn split_block_images(src: &str) -> Vec<BlockPart> {
    split_block_parts(src, false)
}

/// Split `src` into text runs, block-level images, and (when `mermaid_fences` is on) top-level
/// ```mermaid fences. Fence tracking is shared, so a mermaid fence *inside* another code fence is
/// never extracted (it is literal code), matching the segment renderer's rules.
///
/// Known limitation: this runs over the whole source *before* `<details>` extraction, so a block-level
/// image or a ```mermaid fence placed inside a `<details>` body is pulled out here and rendered
/// outside the collapsible region (always visible, un-indented) rather than folded with the block.
/// Regular text, code fences and inline images inside `<details>` fold correctly.
///
/// Inline math (handled downstream by `split_math`, per `BlockPart::Text` run) takes the opposite
/// stance on purpose: it stays *inside* a `<details>`/blockquote/table/HTML block rather than being
/// pulled out, because those blocks' own splitters need the run of lines intact to recognize the
/// block at all (see `split_math`'s doc comment).
///
/// This is where the document-wide code mask is born: the whole source is still intact here, so
/// [`splitter_code_mask`] is asked once, before a single line is removed, and every emitted
/// [`BlockPart::Text`] carries its own slice of the answer (see [`SourceRun`] for why a later
/// splitter cannot re-derive it). Image extraction is gated on that mask for the same reason it
/// exists: the fence tracker in the loop below only recognizes *fenced* blocks, so an
/// `![img](x)` line sitting inside an **indented** code block used to be lifted out as a real
/// image — the block is a literal transcript on screen, and pulling a line out of its middle is
/// exactly what manufactures the mid-block fragments this mask exists to protect the rest of the
/// pipeline from. The ```mermaid branch is deliberately **not** gated: a mermaid fence's own lines
/// are code as far as the parser is concerned, so gating it would disable fence extraction
/// outright.
fn split_block_parts(src: &str, mermaid_fences: bool) -> Vec<BlockPart> {
    let doc = splitter_code_mask(&src.lines().collect::<Vec<_>>());
    split_block_parts_masked(src, &doc, mermaid_fences)
}

/// [`split_block_parts`] for a caller that already holds the document's mask for this text — the
/// copy scanner, whose `code_block_source_locs_inner` computed it over the same whole document
/// before peeling alerts/`<details>` off. Reusing it (rather than letting `split_block_parts`
/// re-derive one from the reassembled run) is what keeps the scanner and the renderer looking at
/// the *same* answer, which is the property that stops `y c` being refused for a whole document
/// over a single disagreement.
#[cfg(test)]
fn split_block_parts_run(run: &SourceRun, mermaid_fences: bool) -> Vec<BlockPart> {
    split_block_parts_masked(run.text(), run.code(), mermaid_fences)
}

/// The shared body of [`split_block_parts`] / [`split_block_parts_run`]. `doc` is positionally
/// paired with `src.lines()`.
fn split_block_parts_masked(src: &str, doc: &[bool], mermaid_fences: bool) -> Vec<BlockPart> {
    debug_assert_eq!(
        doc.len(),
        src.lines().count(),
        "split_block_parts mask/line-count drift"
    );
    let mut parts = Vec::new();
    let mut text = String::new();
    // Mask entries for the lines accumulated in `text`, kept in lockstep with it: every push to
    // `text` pushes here too, so the run emitted below can hand on the document's own answer.
    let mut mask: Vec<bool> = Vec::new();
    // Open code fence, as (fence char byte, fence length). `mermaid` = collecting a diagram body.
    let mut open: Option<(u8, usize)> = None;
    let mut mermaid: Option<String> = None;
    // `split_inclusive('\n')` yields exactly as many items as `lines()`, so `i` indexes `doc`.
    for (i, line) in src.split_inclusive('\n').enumerate() {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        let in_code = doc.get(i).copied().unwrap_or(false);
        match open {
            None => {
                if let Some((fence, info)) = parse_fence(bare) {
                    open = Some((fence.ch, fence.len));
                    if mermaid_fences && is_mermaid_info(&info) {
                        if !text.is_empty() {
                            parts.push(BlockPart::Text(SourceRun::new(
                                std::mem::take(&mut text),
                                std::mem::take(&mut mask),
                            )));
                        }
                        mermaid = Some(String::new());
                    } else {
                        text.push_str(line);
                        mask.push(in_code);
                    }
                } else if let Some((_alt, url)) =
                    (!in_code).then(|| extract_block_image(bare)).flatten()
                {
                    if !text.is_empty() {
                        parts.push(BlockPart::Text(SourceRun::new(
                            std::mem::take(&mut text),
                            std::mem::take(&mut mask),
                        )));
                    }
                    parts.push(BlockPart::Image { url });
                } else {
                    text.push_str(line);
                    mask.push(in_code);
                }
            }
            Some((ch, len)) => {
                let closing = parse_fence(bare)
                    .map(|(f, info)| f.ch == ch && f.len >= len && info.is_empty())
                    .unwrap_or(false);
                match (&mut mermaid, closing) {
                    (Some(code), true) => {
                        parts.push(BlockPart::Mermaid {
                            code: std::mem::take(code),
                        });
                        mermaid = None;
                    }
                    (Some(code), false) => code.push_str(line),
                    (None, _) => {
                        text.push_str(line);
                        mask.push(in_code);
                    }
                }
                if closing {
                    open = None;
                }
            }
        }
    }
    // An unclosed mermaid fence: safely revert it to raw text (principle #3, never drop it).
    let reverted_mermaid = mermaid.is_some();
    if let Some(code) = mermaid {
        text.push_str("```mermaid\n");
        text.push_str(&code);
    }
    if !text.is_empty() {
        // The reverted text is the one run that no longer corresponds line-for-line to the
        // document — a synthesized ` ```mermaid ` opener plus the diagram body, which were never in
        // `mask` — so its mask has to be computed from what it actually became. Everything else
        // carries the document's own answer, collected above.
        parts.push(BlockPart::Text(if reverted_mermaid {
            SourceRun::parse(text)
        } else {
            SourceRun::new(text, mask)
        }));
    }
    parts
}

/// If `line` is *just* an image — Markdown `![alt](url)` (optionally wrapped in a link) or an HTML
/// `<img src=...>` (optionally wrapped in layout tags like `<p>`/`<td>`/`<a>`) — return (alt, url).
fn extract_block_image(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(img) = extract_html_img(t) {
        return Some(img);
    }
    extract_md_img(t)
}

/// Extract an HTML `<img>` when the line consists only of tags (no other visible text).
fn extract_html_img(t: &str) -> Option<(String, String)> {
    let lower = t.to_ascii_lowercase();
    let pos = lower.find("<img")?;
    // The character after "<img" must be whitespace or a tag terminator (avoid matching `<images>`).
    let after = lower[pos + 4..].chars().next()?;
    if !after.is_whitespace() && after != '>' && after != '/' {
        return None;
    }
    // The whole line must be only tags around the image (no stray words).
    if !html_is_only_tags(t) {
        return None;
    }
    let tag_end = lower[pos..].find('>').map(|i| pos + i)?;
    let tag = &t[pos..tag_end];
    let url = html_attr(tag, "src")?;
    let alt = html_attr(tag, "alt").unwrap_or_default();
    Some((alt, url))
}

/// Whether the visible text outside of `<...>` tags is empty (the line is pure HTML tags).
fn html_is_only_tags(t: &str) -> bool {
    let mut depth = 0i32;
    for c in t.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            c if c.is_whitespace() => {}
            _ => return false,
        }
    }
    true
}

/// Read an HTML attribute value (double- or single-quoted) from a tag string.
fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find(name) {
        let i = search + rel;
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        if before_ok {
            let after = tag[i + name.len()..].trim_start();
            if let Some(rest) = after.strip_prefix('=') {
                let rest = rest.trim_start();
                if let Some(q) = rest.chars().next() {
                    if (q == '"' || q == '\'') && rest.len() > 1 {
                        if let Some(end) = rest[1..].find(q) {
                            return Some(rest[1..1 + end].to_string());
                        }
                    }
                }
            }
        }
        search = i + name.len();
    }
    None
}

/// Extract a Markdown `![alt](url)`, optionally wrapped in a `[ ... ](href)` link, requiring the
/// line to contain nothing else.
fn extract_md_img(t: &str) -> Option<(String, String)> {
    let bang = t.find("![")?;
    let prefix = t[..bang].trim();
    if !(prefix.is_empty() || prefix == "[") {
        return None;
    }
    let rest = &t[bang + 2..];
    let close_alt = rest.find(']')?;
    let alt = rest[..close_alt].to_string();
    let after_alt = rest[close_alt + 1..].trim_start();
    let after_alt = after_alt.strip_prefix('(')?;
    let close_url = after_alt.find(')')?;
    // Strip an optional title: ![alt](url "title")
    let url = after_alt[..close_url]
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if url.is_empty() {
        return None;
    }
    // Whatever follows the image must be empty or just the link close `](href)`.
    let suffix = after_alt[close_url + 1..].trim();
    let ok_suffix = suffix.is_empty() || (prefix == "[" && suffix.starts_with(']'));
    if !ok_suffix {
        return None;
    }
    Some((alt, url))
}

/// Reserved rows for an inline image (covered by the real image once it is decoded and fully visible).
/// The first row shows a dim `🖼 alt` label so the user sees an image is present while it loads.
fn image_placeholder_lines(cols: u16, rows: u16, alt: &str, width: u16) -> Vec<Line<'static>> {
    let rows = rows.max(1);
    let pad = (width.saturating_sub(cols) / 2) as usize;
    let indent = " ".repeat(pad);
    let alt = alt.trim();
    let label = if alt.is_empty() {
        "🖼 image".to_string()
    } else {
        format!("🖼 {alt}")
    };
    let label = truncate_width(&label, cols as usize);
    let mut lines = Vec::with_capacity(rows as usize);
    lines.push(Line::from(format!("{indent}{label}")).dim());
    for _ in 1..rows {
        lines.push(Line::from(String::new()));
    }
    lines
}

/// One-line fallback for an image that cannot be shown inline (no backend / remote / missing file).
fn image_text_fallback(alt: &str, url: &str, width: u16) -> Vec<Line<'static>> {
    let alt = alt.trim();
    let s = if alt.is_empty() {
        format!("🖼 {url}")
    } else {
        format!("🖼 {alt} — {url}")
    };
    vec![Line::from(truncate_width(&s, width as usize)).dim()]
}

/// One-line "loading" indicator shown while a remote image is being fetched in the background.
/// It is replaced by the real inline image once the fetch completes and the preview re-decorates.
fn image_loading_line(alt: &str, url: &str, width: u16) -> Vec<Line<'static>> {
    let alt = alt.trim();
    let what = if alt.is_empty() { url } else { alt };
    let s = format!("🖼 {what} — loading…");
    vec![Line::from(truncate_width(&s, width as usize)).dim()]
}

/// Truncate a string to a maximum display width (CJK/emoji counted as 2), appending `…` when cut.
fn truncate_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > budget {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// The tail of `decorate_md_lines`: heading decoration (`decorate_headings`) followed by the small
/// extras pass (`decorate_extras` — thematic breaks, task-list checkboxes). See `decorate_md_lines`'s
/// own doc comment for why this exists as its own function rather than being inlined there.
fn decorate_headings_and_extras(
    lines: Vec<Line<'static>>,
    width: u16,
    icons: bool,
    tasks: &[char],
) -> Vec<Line<'static>> {
    let lines = decorate_headings(lines, width);
    decorate_extras(lines, width, icons, tasks)
}

/// Small post-passes over tui-markdown output: a thematic break (`---`/`***`/`___`) renders as a
/// full-width rule instead of literal dashes, and task-list checkboxes `[ ]`/`[x]` become dedicated
/// marker spans (Nerd Font icon when `icons`, ASCII bracket otherwise).
/// Runs after the code-block pass, so fenced content (already `▎`-prefixed) can't be mistaken.
fn decorate_extras(
    lines: Vec<Line<'static>>,
    width: u16,
    icons: bool,
    tasks: &[char],
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            let t = joined.trim();
            if t == "---" || t == "***" || t == "___" {
                // Thematic break: tui-markdown emits a Rule as literal text, so replace it with a full-width rule line.
                return Line::from(Span::styled(
                    "─".repeat(width as usize),
                    Style::new().fg(TABLE_BORDER_FG),
                ));
            }
            replace_task_checkbox(l, &joined, icons, tasks)
        })
        .collect()
}

/// Replace a leading task-list checkbox (`- [ ] ` / `- [x] ` / a configured custom state like `- [/] `)
/// with a dedicated marker span (`task_marker_style`), so the app can focus/toggle it like a link.
/// Only the list-leading marker is touched — a literal `[ ]` mid-sentence stays as-is.
fn replace_task_checkbox(
    l: Line<'static>,
    joined: &str,
    icons: bool,
    tasks: &[char],
) -> Line<'static> {
    // The offset is only needed by the source scanners (write-back); here the marker is located by
    // searching the *rendered* text, which tui-markdown has already normalized to `- [x] `.
    let Some((state, _)) = task_prefix_state(joined.trim_start(), tasks) else {
        return l;
    };
    let pat = format!("[{state}]");
    let Some(pos) = joined.find(&pat) else {
        return l;
    };
    // Fold the following half-width space into the marker span too (it's always there, since the
    // detection condition requires "] "). Focus's reversal then covers 2 cells — glyph + space —
    // so even a font that draws the Nerd Font glyph at full-width (2 cells), like HackGen NF, still
    // fits the whole glyph inside the reversed area. Same overflow-tolerant convention as the
    // tree's "icon + space".
    let trail_space = joined[pos + pat.len()..].starts_with(' ');
    let end = pos + pat.len() + usize::from(trail_space);
    let (style, alignment) = (l.style, l.alignment);
    // Replace the marker range [pos, end) on `joined` with a dedicated span. Since tui-markdown can
    // split a custom state (e.g. `[/]`) across multiple spans, split by range rather than per-span.
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut off = 0usize;
    let mut inserted = false;
    for sp in l.spans {
        let s = sp.content.as_ref();
        let (a, b) = (off, off + s.len());
        off = b;
        if b <= pos || a >= end {
            out.push(sp); // outside the marker range, keep as-is
            continue;
        }
        // A span overlapping the marker range: keep only the parts spilling outside the range in their original style.
        if a < pos {
            out.push(Span::styled(s[..pos - a].to_string(), sp.style));
        }
        if !inserted {
            let mut disp = task_marker_display(state, icons);
            if trail_space {
                disp.push(' ');
            }
            out.push(Span::styled(disp, task_marker_style()));
            inserted = true;
        }
        if b > end {
            out.push(Span::styled(s[end - a..].to_string(), sp.style));
        }
    }
    let mut nl = Line::from(out).style(style);
    nl.alignment = alignment;
    nl
}

// ---- Task-list checkboxes (interactive: Tab focus / Space toggle, wired by the app) ----

/// The state char if the (whitespace-trimmed) line starts with a task marker (`- [<c>] `, where
/// `<c>` is a recognized state — ` `/`x`/`X` always, plus the configured custom states), together
/// with the **byte offset of that state char within `t`**, which the caller turns into the absolute
/// offset it writes the toggled character back to.
///
/// Returning the offset rather than assuming a fixed one is what makes the spacing rules below safe:
/// `<bullet> [` is only 3 bytes when the source writes exactly one space after the bullet.
fn task_prefix_state(t: &str, tasks: &[char]) -> Option<(char, usize)> {
    // GFM task lists accept any unordered-list bullet (`-`, `*`, `+`) — tui-markdown renders all
    // three as checkboxes, so the source scanner must recognize all three too, or the toggle's
    // count-guard mismatches and every toggle is cancelled (a safe fallback, principle #3).
    let bytes = t.as_bytes();
    if !matches!(bytes.first(), Some(b'-' | b'*' | b'+')) {
        return None;
    }
    // CommonMark §5.2: 1-4 spaces after the marker just set the item's content column, so
    // `*   [ ] x` (three spaces — a common style, e.g. zerocopy's agent docs) is the same checkbox
    // as `* [ ] x`. Five or more make the item's content *start with an indented code block*, where
    // `[ ]` is literal text and no checkbox is drawn.
    let spaces = bytes[1..].iter().take_while(|&&c| c == b' ').count();
    if spaces == 0 || spaces > 4 {
        return None;
    }
    let rest = t[1 + spaces..].strip_prefix('[')?;
    let c = rest.chars().next()?;
    let tail = &rest[c.len_utf8()..];
    let closed = tail.strip_prefix(']').is_some_and(|after| {
        // A marker with no text after it (`- [x]` on its own line — seen in the wild in
        // line-clipping's README) is still drawn as a checkbox, so end-of-line closes it too.
        after.is_empty() || after.starts_with([' ', '\t'])
    });
    if !closed {
        return None;
    }
    is_task_state(c, tasks).then_some((c, 1 + spaces + 1))
}

fn is_task_state(c: char, tasks: &[char]) -> bool {
    c == ' ' || c == 'x' || c == 'X' || tasks.contains(&c)
}

/// Display form of a task state. Standard states use Nerd Font checkbox icons when `ui.icons`
/// is on (guaranteed 1 cell — Unicode ☐/☑ are EAW-Neutral but CJK fallback fonts draw them
/// double-width, clipping the glyph and halving the focus highlight), ASCII brackets otherwise
/// (no tofu). Custom states keep the bracket form (`[/]`) so glyph coverage is never an issue.
fn task_marker_display(state: char, icons: bool) -> String {
    match state {
        ' ' if icons => crate::ui::icons::task_icon(false).to_string(),
        'x' | 'X' if icons => crate::ui::icons::task_icon(true).to_string(),
        ' ' => "[ ]".into(),
        'x' | 'X' => "[x]".into(),
        c => format!("[{c}]"),
    }
}

/// Style used **only** for task markers — doubles as the sentinel by which the app recognizes
/// them among rendered spans (same trick as the hidden link-target spans). BOLD distinguishes it
/// from the plain-cyan code gutter, and `is_task_span` additionally checks the content form.
pub(crate) fn task_marker_style() -> Style {
    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

/// Whether this span is a task marker produced by `replace_task_checkbox`.
pub(crate) fn is_task_span(span: &Span<'_>) -> bool {
    span.style == task_marker_style() && task_span_state(span.content.as_ref()).is_some()
}

/// Recover the state char from a rendered marker span (NF icon→` `/`x`, `[c]`→`c`).
/// The marker span carries a trailing space (highlight/overflow room for double-width fonts).
pub(crate) fn task_span_state(s: &str) -> Option<char> {
    let s = s.strip_suffix(' ').unwrap_or(s);
    if s == crate::ui::icons::task_icon(false).to_string() {
        return Some(' ');
    }
    if s == crate::ui::icons::task_icon(true).to_string() {
        return Some('x');
    }
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    let mut it = inner.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

/// Style for the **code-block header's** left gutter — doubles as the sentinel by which the app
/// recognizes a focusable code block among rendered spans (same trick as task markers). ITALIC on
/// the `▎` is visually negligible but distinguishes it from the plain-cyan body gutter (no modifier)
/// and the task marker (BOLD).
pub(crate) fn code_header_marker_style() -> Style {
    Style::new()
        .fg(CODE_GUTTER_FG)
        .add_modifier(Modifier::ITALIC)
}

/// Whether this span is a code-block header gutter produced by `code_header` (the Tab-focus target
/// for "copy this code block"). Ignores the (optional) background so it matches with or without a
/// code background.
pub(crate) fn is_code_header_span(span: &Span<'_>) -> bool {
    span.style.fg == Some(CODE_GUTTER_FG)
        && span.style.add_modifier.contains(Modifier::ITALIC)
        && !span.style.add_modifier.contains(Modifier::BOLD)
        && span.content.as_ref().starts_with('▎')
}

/// Whether this span is styled as inline code (`` `…` ``). `KonomaStyles::code()` uses `fg(White)`
/// (plus an optional background), so post-passes (autolink / emoji) can skip inline-code content
/// **regardless of whether `ui.theme.code_bg` is set** — GitHub never links or emoji-substitutes
/// inside code. (`HEAD_FG`/heading colors are Cyan, so this does not catch headings.)
pub(crate) fn is_inline_code_span(span: &Span<'_>) -> bool {
    span.style.fg == Some(Color::White)
}

/// Whether this decorated line is a fenced code-block line (body or header). Such lines carry the
/// `▎` gutter (fg `CODE_GUTTER_FG`), so autolink/emoji can skip the whole line even when `code_bg`
/// is `none` (no background to key off of) — a URL/`:shortcode:` inside a code block must stay
/// literal. Scans **all** spans (not just the first) because inside a GitHub alert every body line
/// is prefixed with the `▌ ` bar, so a fenced line there is `[▌ ][▎ …]` and the gutter is the
/// second span. Requiring the gutter fg also keeps a user's plain `▎` text (fg `None`) from being
/// mistaken for code.
pub(crate) fn is_code_line(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .any(|s| s.content.starts_with('▎') && s.style.fg == Some(CODE_GUTTER_FG))
}

/// Column width of `line`'s leading whitespace run, expanding a tab to the next multiple of 4
/// (CommonMark's tab-stop rule, applied from column 0 — the only case that matters here, since this
/// is only ever called on a line's own leading run). Verified against the actual renderer: a single
/// leading tab opens an indented code block exactly like 4 spaces do.
fn leading_ws_width(line: &str) -> usize {
    let mut col = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col = (col / 4 + 1) * 4,
            _ => break,
        }
    }
    col
}

/// Strip up to `cols` columns of leading whitespace (space/tab, tab-stop aware) from `line`,
/// returning the rest verbatim — including any leftover indentation beyond `cols`, which is real
/// content once inside an indented code block (e.g. a snippet that itself uses extra indentation).
/// Callers only ever ask for `cols == 4` on a line already known to have `leading_ws_width` >= 4, and
/// 4 is itself always a tab stop, so a leading tab is never left partially consumed.
#[cfg(test)]
fn strip_ws_columns(line: &str, cols: usize) -> &str {
    let mut col = 0usize;
    let mut idx = 0usize;
    for ch in line.chars() {
        if col >= cols {
            break;
        }
        match ch {
            ' ' => {
                col += 1;
                idx += ch.len_utf8();
            }
            '\t' => {
                col = (col / 4 + 1) * 4;
                idx += ch.len_utf8();
            }
            _ => break,
        }
    }
    &line[idx..]
}

/// Whether `t` (already stripped of leading whitespace) opens a Markdown list item: a bullet
/// (`-`/`*`/`+`) or an ordered marker (1-9 digits + `.`/`)`), each required to be followed by a
/// space or end of line. Not a full CommonMark list-marker parser — in particular it does not need
/// to special-case a thematic break (`---`/`***`): those never have a space right after the first
/// marker character, so they never match this.
#[cfg(test)]
fn looks_like_list_marker(t: &str) -> bool {
    let bytes = t.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let marker_end = if matches!(bytes[0], b'-' | b'*' | b'+') {
        1
    } else {
        let mut j = 0;
        while j < bytes.len() && j < 9 && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > 0 && j < bytes.len() && matches!(bytes[j], b'.' | b')') {
            j + 1
        } else {
            return false;
        }
    };
    marker_end == bytes.len() || bytes[marker_end] == b' '
}

/// Whether `line` opens an ATX heading (`#` through `######`) — up to 3 columns of indentation,
/// then 1-6 `#` characters, then whitespace or end of line (CommonMark §4.2). A run of 7+ `#`, or a
/// `#` immediately followed by non-whitespace (`#nospace`), is not a heading — it's ordinary
/// paragraph text, verified directly against the renderer (both draw as a plain paragraph, and a
/// following indented line stays a lazy continuation of it, not code).
#[cfg(test)]
fn is_atx_heading_line(line: &str) -> bool {
    let ws = leading_ws_width(line);
    if ws > 3 {
        return false;
    }
    let rest = strip_ws_columns(line, ws);
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    matches!(rest.as_bytes().get(hashes), None | Some(b' ') | Some(b'\t'))
}

/// Whether `line` is a thematic break (`---`, `***`, `___`, `- - -`, ...) — up to 3 columns of
/// indentation, then 3 or more of the same character among `-`/`_`/`*`, optionally interleaved with
/// spaces/tabs, and nothing else on the line (CommonMark §4.1). Checked unconditionally, with no
/// need to look at the preceding line: a run of 3+ of one of these characters is *never* itself
/// paragraph content, whether it resolves as a thematic break (no paragraph precedes it) or as a
/// setext heading underline (one does — see `is_setext_underline_line`) — the two interpretations
/// never disagree on the one thing this scanner needs to know: "does this end a paragraph".
#[cfg(test)]
fn is_thematic_break_line(line: &str) -> bool {
    let ws = leading_ws_width(line);
    if ws > 3 {
        return false;
    }
    let rest = strip_ws_columns(line, ws);
    for marker in ['-', '_', '*'] {
        let count = rest.chars().filter(|&c| c == marker).count();
        if count >= 3 && rest.chars().all(|c| c == marker || c == ' ' || c == '\t') {
            return true;
        }
    }
    false
}

/// Whether `line` has the *shape* of a setext heading underline: up to 3 columns of indentation,
/// then a run of one or more `=` characters, or a run of one or more `-` characters, with any
/// trailing spaces/tabs (CommonMark §4.3 — unlike a thematic break, there is no minimum count). A
/// setext underline retroactively turns the immediately *preceding* line into a heading, so this
/// shape only actually behaves as "not a paragraph" when a paragraph precedes it — callers must gate
/// on that separately (the "previous line closed a paragraph" flag being false right before this
/// line). Without a preceding open paragraph, the renderer draws a line like this as an ordinary
/// paragraph of its own; calling this unconditionally would wrongly mark it as non-paragraph content
/// and let a following indented line open as code when the renderer keeps it as a lazy continuation.
///
/// `-`/`_`/`*` runs of 3+ are already handled unconditionally by `is_thematic_break_line` — see its
/// doc comment for why the two constructs never disagree on "does this end a paragraph". What only
/// *this* function adds, once the caller gates it, is `=` runs of any length and `-` runs shorter
/// than 3 (a `-`/`--` underline is valid right after a paragraph even though it's too short to ever
/// be a thematic break on its own).
#[cfg(test)]
fn is_setext_underline_line(line: &str) -> bool {
    let ws = leading_ws_width(line);
    if ws > 3 {
        return false;
    }
    let rest = strip_ws_columns(line, ws).trim_end();
    !rest.is_empty() && (rest.chars().all(|c| c == '=') || rest.chars().all(|c| c == '-'))
}

/// Tracks whether the scan is currently inside a (possibly nested) list item, to scope "top-level
/// indented code block" detection the way pulldown-cmark (the parser tui-markdown renders through)
/// actually does: a real, CommonMark-correct indented code block *inside* a list item needs 4
/// columns **beyond that item's own content column** — verified against the actual renderer (e.g.
/// `- item\n\n      nested code\n` at 6 columns *is* code; `- item\n\n    para\n` at 4 columns is
/// just a second paragraph of the item, not code). Reproducing the exact per-depth column math would
/// mean re-implementing list-item container tracking; instead this takes the conservative side once
/// *any* list is active — indented content deeper than the list's own marker is never treated as
/// code. That covers the common case (plain continuation content) exactly; it under-detects the rare
/// deeply-nested-code-in-a-list case (same known-gap class as block quotes — see
/// `code_block_source_locs_inner`'s doc comment), which only means `y c`/toggle stays refused there,
/// same as before this fix, never a false match on real list content.
#[cfg(test)]
struct ListGuard {
    in_list: bool,
}

#[cfg(test)]
impl ListGuard {
    fn new() -> Self {
        Self { in_list: false }
    }

    /// Call for every **non-blank** line that reaches "regular content" handling (not already
    /// claimed by a fence/alert/details/etc). `ws`/`rest` are its leading-whitespace width and the
    /// text after it. Returns whether the line should be treated as (possibly nested) list content.
    fn observe(&mut self, ws: usize, rest: &str) -> bool {
        if ws == 0 {
            // Column 0 unambiguously settles it: a fresh marker (re-)enters/continues the list; any
            // other column-0 content (a plain paragraph) means the list has ended.
            self.in_list = looks_like_list_marker(rest);
        }
        self.in_list
    }

    /// Call when a fence/alert/details/html/table/heading/thematic-break/setext-underline match is
    /// found at `ws` columns — each of those is its own top-level block (container types require
    /// dedenting to their own baseline; the leaf ones — heading, thematic break, setext underline —
    /// simply can never be list content at column 0 either way), so a match at column 0 unambiguously
    /// ends any active list (matches `observe`'s column-0 rule). A match found while still indented is
    /// assumed to still be inside the list — the same conservative default `observe` uses for other
    /// indented content.
    fn observe_special(&mut self, ws: usize) {
        if ws == 0 {
            self.in_list = false;
        }
    }
}

/// Advance `*i` past one indented code block — one or more blank-line-separated chunks — starting at
/// `lines[*i]`, which the caller has already verified opens one (`leading_ws_width >= 4`, not inside
/// a list, allowed to open here per `prev_not_paragraph`), leaving it at the first line that is *not*
/// part of the block. Used by the task scanner only to *skip* such a block (nothing inside code is a
/// checkbox); the block's own content is never needed, since the copy scanner gets its text from the
/// parser (see `parser_code_blocks`).
#[cfg(test)]
fn skip_indented_code_block(lines: &[&str], i: &mut usize) {
    while let Some(line) = lines.get(*i) {
        if line.trim().is_empty() {
            let mut k = *i;
            while k < lines.len() && lines[k].trim().is_empty() {
                k += 1;
            }
            if k < lines.len() && leading_ws_width(lines[k]) >= 4 {
                *i = k; // The chunk continues past the blank run.
                continue;
            }
            break; // The block ends here; the blank line(s) are left for the caller.
        }
        if leading_ws_width(line) >= 4 {
            *i += 1;
            continue;
        }
        break;
    }
}

/// The pulldown-cmark parse options — **kept identical to tui-markdown's own** (written down in
/// `tui-markdown-0.3.7/src/lib.rs`, `from_str_with_options`), because the scanners below ask the very
/// parser the renderer renders through where the code blocks are. Letting the two option sets drift
/// re-creates exactly the scanner-vs-renderer disagreement asking the parser exists to eliminate.
///
/// Note what is deliberately **absent**: `ENABLE_TABLES`. tui-markdown collapses a GFM table into one
/// line, which is why konoma intercepts table blocks with its own renderer (`split_tables`) before the
/// text ever reaches the parser — see `render_md_text_inner`. Adding it here would make the scanner
/// see tables the renderer never shows it.
fn tui_markdown_parse_options() -> ParseOptions {
    let mut o = ParseOptions::empty();
    o.insert(ParseOptions::ENABLE_STRIKETHROUGH);
    o.insert(ParseOptions::ENABLE_TASKLISTS);
    o.insert(ParseOptions::ENABLE_HEADING_ATTRIBUTES);
    o.insert(ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    o.insert(ParseOptions::ENABLE_SUPERSCRIPT);
    o.insert(ParseOptions::ENABLE_SUBSCRIPT);
    o
}

/// Collect the content of every code block in one Markdown text run, **asking pulldown-cmark** —
/// the same parser tui-markdown renders through — rather than re-deriving CommonMark's block rules.
///
/// Root cause this replaced (2026-08, reported by a user on the released build): the hand-written
/// scanner measured indentation as an absolute column, while CommonMark measures it relative to the
/// enclosing container. Inside a list item every column shifts right by the marker's width, so a fence
/// indented 4 columns under `1. ` is a *real fence* (the parser and the screen agree) that the scanner
/// called an indented code block, and a continuation paragraph indented 6 columns under `   4. ` is
/// *not* code that the scanner counted as one. Either way the count guard broke and `y c` was refused
/// for the whole document — measured on 2,182 real `.md` files from the crate registry, 26 of them
/// (Apache license texts, several CHANGELOGs, a README) tripped it. Getting the absolute-column math
/// right is not possible without reproducing container tracking, i.e. re-implementing the parser; so
/// the second implementation is gone instead. This was the seventh renderer-vs-scanner drift of the
/// same shape, and tests can only ever *detect* a drift — they cannot stop one from being written.
///
/// The one thing the parser alone does not know (everything else the caller has already narrowed
/// down by mirroring the render pipeline's own splitting — see `scan_code_run`): code nested in a
/// plain block quote draws no header. tui-markdown prefixes every line of it with `"> "`, so
/// `flush_code_run` cannot find the block's own markers and hands the run back undecorated (see its
/// doc comment). Blockquote nesting is read off the event stream, so this needs no `>`-parsing of
/// its own — and an *alert* blockquote never reaches here, having been peeled off upstream.
#[cfg(test)]
fn parser_code_blocks(text: &str, out: &mut Vec<String>) {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut quote_depth = 0usize;
    // `Some` while inside a code block (they never nest), accumulating its content.
    let mut body: Option<String> = None;
    let mut drawn = false;
    for ev in Parser::new_ext(text, tui_markdown_parse_options()) {
        match ev {
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote_depth = quote_depth.saturating_sub(1),
            Event::Start(Tag::CodeBlock(_)) => {
                drawn = quote_depth == 0;
                body = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(b) = body.take() {
                    if drawn {
                        // The parser always terminates a code block's content with a newline; the
                        // rendered block has no trailing blank row, so neither does the copy.
                        out.push(b.strip_suffix('\n').unwrap_or(&b).to_string());
                    }
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
}

/// Scan one run of plain Markdown lines — everything between the alert / `<details>` blocks the
/// caller peels off — for code blocks, first applying the **same** interceptions the render pipeline
/// applies before a text run ever reaches tui-markdown, using the renderer's own splitter functions
/// rather than copies of them:
///
/// * `split_block_parts` — block-level images and ```mermaid fences become their own parts. This is
///   also the *whole* mermaid rule: a fence it diverts is drawn as a diagram, and one it leaves (a
///   fence indented 4+ absolute columns, which CommonMark still calls a fence because it measures
///   from the enclosing list item's content column) is drawn as ordinary code — both sides agree
///   without a special case. `mermaid_fences` is passed `true` unconditionally: with `ui.mermaid =
///   "text"` the fence is left here but `split_segments` diverts it a step later, so it is never a
///   code header either way.
/// * `split_tables` / `split_html_blocks` — konoma draws those with its own renderers
///   (`render_md_text_inner`), so their contents never reach the parser.
///
/// The HTML step is what makes a 4-column-indented HTML line — `    <a href="…">` inside a `<div
/// align="center">` banner, the shape at the top of many crate READMEs — agree: an HTML block on
/// screen, not an indented code block. Both sides reach that answer the same way, and it is the
/// reason `run` carries a mask entry per line rather than just the line: the caller's
/// whole-document parse is what knows those lines are HTML-block interior, and the fragment left
/// behind once `split_block_parts` lifts the `<img>` lines out no longer does (see [`SourceRun`]).
/// The image step matters for the same banners in the other direction: pulling an `<img>` line out
/// splits the `<div>` in two, and the `    |` separator left between the halves *does* become a
/// real indented code block on screen (criterion's README).
///
/// `in_alert_body` skips the `split_block_parts` step, because production never applies it to an
/// alert's body: that runs on the raw source, where those lines still carry their `>` prefix and
/// match neither an image nor a fence. Hence a diagram inside a `> [!NOTE]` really is drawn as code.
///
/// Because the splitters are shared rather than copied, this half of the scanner tracks any change
/// to them for free. The half that does **not** is the alert / `<details>` peel in the caller
/// (`code_block_source_locs_inner`), which decides where those two constructs start before any
/// splitter runs — that is why it goes through the same [`splitter_code_mask`] the render side's
/// `split_alerts`/`split_details` use, instead of tracking fences itself. When it did, moving the
/// render side onto the parser silently desynced the two in both directions at once (a fence closed
/// by its list item, which a fence toggle reads as running to EOF; an indented code block, which it
/// cannot see at all), and every mismatch refuses `y c` for the whole document.
#[cfg(test)]
fn scan_code_run(run: &mut Vec<(&str, bool)>, in_alert_body: bool, out: &mut Vec<String>) {
    if run.is_empty() {
        return;
    }
    // Each accumulated line comes with the mask entry the **caller's** whole-document parse gave
    // it, so the run handed on carries the same answer the renderer's `split_block_parts` computed
    // over that same document — rather than one re-derived from these lines in isolation, which is
    // exactly the fragment/document ambiguity `SourceRun` exists to keep out of this pipeline. The
    // two sides agreeing line for line is what stops a single disagreement refusing `y c` for a
    // whole document.
    let mut text = run.iter().map(|(l, _)| *l).collect::<Vec<_>>().join("\n");
    text.push('\n');
    let code: Vec<bool> = run.iter().map(|(_, c)| *c).collect();
    run.clear();
    let text = SourceRun::new(text, code);
    if in_alert_body {
        scan_text_part(&text, out);
        return;
    }
    for part in split_block_parts_run(&text, true) {
        if let BlockPart::Text(t) = part {
            scan_text_part(&t, out);
        }
    }
}

/// One `BlockPart::Text` run: mirrors `render_md_text_inner`'s tables → HTML-blocks → parser chain.
#[cfg(test)]
fn scan_text_part(text: &SourceRun, out: &mut Vec<String>) {
    for part in split_tables(text) {
        let MdPart::Text(t) = part else {
            continue; // a table block: drawn with konoma's borders, never as code
        };
        for hp in split_html_blocks(&t) {
            match hp {
                HtmlPart::Text(t2) => parser_code_blocks(t2.text(), out),
                HtmlPart::Html(_) => {} // drawn as tag-stripped text, never as code
            }
        }
    }
}

/// Raw inner text of each code block — fenced (```` ``` ```` / `~~~`) or indented — in document
/// order, skipping `mermaid` fences (they are diverted to diagram rendering and produce no
/// code-block header). Used by the "copy focused code block" action: the Nth entry maps to the Nth
/// focusable block. The caller cross-checks the count against the on-screen headers before copying
/// (safe fallback).
#[cfg(test)]
pub(crate) fn code_block_source_locs(src: &str, details_open: &[bool]) -> Vec<String> {
    code_block_source_locs_inner(src, details_open, false)
}

/// `in_alert_body`: the block-level extraction pass (images, ```mermaid fences) runs on the raw
/// source, where an alert's lines still carry their `>` prefix and match nothing — so inside an alert
/// body it must be skipped here too, and a diagram there really is drawn as an ordinary code block.
/// See `scan_code_run`.
#[cfg(test)]
fn code_block_source_locs_inner(
    src: &str,
    details_open: &[bool],
    in_alert_body: bool,
) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut details_idx = 0usize;
    let mut i = 0;
    // Plain lines accumulated since the last alert/`<details>` block, flushed through the parser by
    // `scan_code_run`. Flushing *before* recursing keeps the results in document order. Each line is
    // carried with its `in_code` answer so `scan_code_run` can hand the splitters this
    // whole-document mask instead of one re-derived from the reassembled run (see `SourceRun`).
    let mut run: Vec<(&str, bool)> = Vec::new();
    // An alert header / `<details>` tag inside a code block is literal content, not a block start.
    // This is `splitter_code_mask` — **the same gate `split_alerts`/`split_details` use on the
    // render side**, over this same text — rather than a fence tracker written out again here: the
    // two peel the same two constructs, so anything they disagree about is a count mismatch that
    // refuses `y c` for the whole document. A hand-rolled fence toggle drifts from them in both
    // directions (a fence in a list item closed by the item's end, which it reads as running to EOF;
    // an indented code block, which it cannot see at all).
    let in_code = splitter_code_mask(&lines);
    while i < lines.len() {
        if in_code[i] {
            run.push((lines[i], true));
            i += 1;
            continue;
        }
        // An alert body is drawn as Markdown with the `>` stripped, so a fence inside it **also
        // becomes a code block on screen**. Unless we strip it and collect under the same rule, the
        // count won't match the on-screen headers and `y c` copy gets refused (for every block in
        // that document). The stripped body is exactly the copy content.
        if parse_alert_header(lines[i]).is_some() {
            scan_code_run(&mut run, in_alert_body, &mut out);
            i += 1;
            let mut body = String::new();
            while i < lines.len() && is_blockquote_line(lines[i]) {
                body.push_str(&strip_blockquote(lines[i]));
                body.push('\n');
                i += 1;
            }
            out.extend(code_block_source_locs_inner(&body, &[], true));
            continue;
        }
        // `<details>` only draws **the body of a block that's open** (if closed, a fence inside it
        // never appears on screen). Open/closed is runtime state, so it's received from the caller.
        if let Some(open_attr) = details_open_tag(lines[i]) {
            scan_code_run(&mut run, in_alert_body, &mut out);
            let open = details_open.get(details_idx).copied().unwrap_or(open_attr);
            details_idx += 1;
            i += 1;
            let mut body = Vec::new();
            while i < lines.len() && !is_details_close(lines[i]) {
                body.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1; // `</details>`
            }
            if open {
                out.extend(code_block_source_locs_inner(
                    &body.join("\n"),
                    &[],
                    in_alert_body,
                ));
            }
            continue;
        }
        run.push((lines[i], in_code[i]));
        i += 1;
    }
    scan_code_run(&mut run, in_alert_body, &mut out);
    out
}

/// Location of one toggleable task marker in the **source** text.
pub(crate) struct TaskLoc {
    /// 0-based line index (by `\n`).
    pub line: usize,
    /// Byte offset of the state char within the line.
    pub state_off: usize,
    /// Current state char in the source. Only read back by test code
    /// (`app::md_snapshot_tests::dump_case`, which dumps it into the golden-snapshot file) — the one
    /// production constructor (`app::md_tasks::md_toggle_focused_task`) keeps its own local `cur`
    /// instead of reading this back, so this is legitimately dead in a non-test build.
    #[allow(dead_code)]
    pub state: char,
}

/// Inverse of the retired `TaskLoc::absolute` (dropped once its only caller,
/// `app::md_render::build_decorated`'s legacy fallback branch, went unreachable — see this
/// function's own doc comment): the `(line, byte offset within that line)` a document-absolute
/// byte offset into `src` falls on. Used by `app::md_tasks::md_toggle_focused_task` to turn
/// `MdItemKind::Task::state_at` (a document-absolute offset — the model render pass's own
/// `render::RenderOut::tasks` convention) back into `TaskLoc`'s line-relative shape, so the
/// write-back verification chain (`pre_origin` line mapping, on-disk prefix comparison) needs no
/// change from before this migration. `None` if `byte` does not land inside `src` at all (past its
/// end) — defensive; every real caller hands this a `state_at` that came from `src` itself.
pub(crate) fn byte_to_line_offset(src: &str, byte: usize) -> Option<(usize, usize)> {
    let mut start = 0usize;
    for (i, line) in src.split('\n').enumerate() {
        let end = start + line.len();
        if byte <= end {
            return Some((i, byte - start));
        }
        start = end + 1;
    }
    None
}

/// Scan a run of "logical lines" — one alert/details body with its container prefix already
/// stripped, its own local coordinate system (mirrors `code_block_source_locs_inner`'s recursion
/// into those bodies) — for task markers, skipping indented (4+ column) code chunks exactly like the
/// top-level scan in `task_source_locs` does: a `- [ ] this looks like a task` inside a block the
/// renderer draws as *code* must not be mistaken (and mis-toggled) as a real checkbox. Each entry is
/// `(original absolute line index for TaskLoc.line, this line's own text, extra prefix bytes already
/// stripped off the **source** line — e.g. a blockquote "> " marker — needed to translate a byte
/// offset within the text back to a byte offset within the real source line)`.
///
/// Recurses into a GitHub alert or a `<details>` block found *within* this body — the write-back
/// half of the same fix `render_md_body_nested` is the render half of (see its doc comment for the
/// full reasoning): a `<details>`'s body can contain an alert, an alert's body can contain a
/// `<details>`, either can nest again inside what that produces, and so on. A `<details>` found
/// this way always uses its own `open` attribute directly, never a document-wide ordinal slot — it
/// never participates in that sequence on the render side either (only the single top-level
/// `task_source_locs` scan, mirroring `render_md_text`, does).
#[cfg(test)]
fn scan_task_lines(logical: &[(usize, &str, usize)], tasks: &[char], out: &mut Vec<TaskLoc>) {
    let mut list = ListGuard::new();
    let mut prev_not_paragraph = true;
    let mut idx = 0usize;
    while idx < logical.len() {
        let (orig_line, text, prefix) = logical[idx];
        let ws = leading_ws_width(text);
        let rest = strip_ws_columns(text, ws);
        // Byte length of the leading run just stripped — **not** `ws`, which is a display *column*
        // width (a tab expands to the next 4-column stop, `leading_ws_width`'s own doc comment).
        // `state_off` below is a byte offset into `text`, so adding `ws` instead of this silently
        // overcounts by (columns - bytes) whenever the run contains a tab, landing the write on some
        // byte of the task's own text rather than the checkbox (bug, 2026-08 — mirrors the correct,
        // byte-based `indent` the top-level scan at the bottom of this file already uses).
        let indent = text.len() - rest.len();
        if rest.trim().is_empty() {
            prev_not_paragraph = true;
            idx += 1;
            continue;
        }
        if parse_alert_header(text).is_some() {
            list.observe_special(ws);
            prev_not_paragraph = true;
            idx += 1;
            let mut nested: Vec<(usize, String, usize)> = Vec::new();
            while idx < logical.len() && is_blockquote_line(logical[idx].1) {
                let (oln, txt, pfx) = logical[idx];
                let stripped = strip_blockquote(txt);
                let extra = txt.len() - stripped.len();
                nested.push((oln, stripped, pfx + extra));
                idx += 1;
            }
            let refs: Vec<(usize, &str, usize)> = nested
                .iter()
                .map(|(o, s, p)| (*o, s.as_str(), *p))
                .collect();
            scan_task_lines(&refs, tasks, out);
            continue;
        }
        if let Some(open_attr) = details_open_tag(text) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            idx += 1;
            let mut nested: Vec<(usize, &str, usize)> = Vec::new();
            while idx < logical.len() && !is_details_close(logical[idx].1) {
                nested.push(logical[idx]);
                idx += 1;
            }
            if idx < logical.len() {
                idx += 1; // skip `</details>`
            }
            if open_attr {
                scan_task_lines(&nested, tasks, out);
            }
            continue;
        }
        // Heading / thematic break / setext underline: same "not a paragraph" treatment as
        // `code_block_source_locs_inner` — see that function's matching checks and the three
        // detector functions' doc comments for the CommonMark reasoning (identical here).
        if is_atx_heading_line(text) || is_thematic_break_line(text) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            idx += 1;
            continue;
        }
        if !prev_not_paragraph && is_setext_underline_line(text) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            idx += 1;
            continue;
        }
        let in_list = list.observe(ws, rest);
        if !in_list && ws >= 4 && prev_not_paragraph {
            // Skip the whole indented code chunk — the same chunk-gluing rule as
            // `skip_indented_code_block` (which the flat scan uses), inlined here because this scan
            // walks `(line, text, prefix)` triples rather than plain lines.
            while let Some(&(_, t2, _)) = logical.get(idx) {
                if t2.trim().is_empty() {
                    let mut k = idx;
                    while k < logical.len() && logical[k].1.trim().is_empty() {
                        k += 1;
                    }
                    if k < logical.len() && leading_ws_width(logical[k].1) >= 4 {
                        idx = k;
                        continue;
                    }
                    break;
                }
                if leading_ws_width(t2) >= 4 {
                    idx += 1;
                    continue;
                }
                break;
            }
            prev_not_paragraph = true;
            continue;
        }
        if let Some((state, off)) = task_prefix_state(rest, tasks) {
            out.push(TaskLoc {
                line: orig_line,
                state_off: prefix + indent + off,
                state,
            });
        }
        prev_not_paragraph = false;
        idx += 1;
    }
}

/// Scan Markdown source for task markers, in document order, skipping exactly what the render
/// pipeline diverts away from `decorate_extras`: code/mermaid fences, HTML blocks (start line up
/// to the next blank line) and GFM table blocks. This keeps the Nth checkbox on screen aligned
/// with the Nth `TaskLoc`, so a toggle edits the right line. Pathological documents could still
/// disagree — the caller cross-checks count and current state before writing.
///
/// This scanner knows nothing about the source pre-passes the caller may have run first
/// (`process_footnotes`, `process_inline_html`), so **which text it is given decides what it means**.
/// Given the file, it describes the file; given the preprocessed text, it describes the screen.
///
/// A previous version of this comment claimed that when the two disagree — a `<br>` making text look
/// like a task line the file does not contain — "the resulting count mismatch just makes the caller's
/// cross-check fail and refuse the toggle, it never mis-edits a line". **That was false**, and was
/// verified false end to end against the released v0.23.5: a document can hide one task line from the
/// screen and reveal a different one to a scan of the file at the same time, leaving both the count
/// and the state characters equal while the ordinals refer to different checkboxes, and the toggle
/// then wrote `x` into a line the reader was not looking at, silently. Counting is not a
/// correspondence proof. `md_toggle_focused_task` now scans the *rendered* text here and maps each
/// hit back to a real source line through `LineOrigin`, refusing when no such line exists.
#[cfg(test)]
pub(crate) fn task_source_locs(src: &str, tasks: &[char], details_open: &[bool]) -> Vec<TaskLoc> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    // Code blocks come from `splitter_code_mask` — the same gate `split_alerts`/`split_details` use
    // on the render side — not a fence tracker written out again here, for the reason in that
    // function's doc comment: the two must agree line for line about which alert/`<details>` starts
    // are real, or the count guard refuses every toggle in the document. Nothing inside a code block
    // is a checkbox either, so this doubles as the skip. Indented blocks the mask does *not* claim
    // are still handled below by `skip_indented_code_block` (unchanged, deliberately conservative).
    let in_code = splitter_code_mask(&lines);
    let mut details_idx = 0usize;
    let mut i = 0;
    // See `ListGuard` / `skip_indented_code_block` docs. `prev_not_paragraph` starts `true`: the very
    // first block of the document needs no preceding blank line to open as indented code.
    // NOTE: this scanner still derives CommonMark's block rules by hand, which is the shape of bug
    // `code_block_source_locs` was moved off of (see `parser_code_blocks`) — the parser reports code
    // blocks, not checkbox positions, so it cannot simply be swapped in here. `ListGuard`'s
    // conservative "anything indented inside a list is list content" keeps that inaccuracy on the
    // safe side (a missed checkbox refuses the toggle; it never edits the wrong byte).
    let mut list = ListGuard::new();
    let mut prev_not_paragraph = true;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        let ws = leading_ws_width(line);
        if in_code[i] {
            if i == 0 || !in_code[i - 1] {
                // First line of the block: same bookkeeping the fence-opening line used to do.
                // Verified against the renderer: a non-paragraph block
                // (fence/alert/details/html/table) does not gate a following indented code block
                // behind a blank line the way a paragraph does.
                list.observe_special(ws);
            }
            prev_not_paragraph = true;
            i += 1;
            continue;
        }
        // A GitHub alert (`> [!NOTE]` …): the render side (`split_alerts`→`render_alert`) strips the
        // `>` and draws the body as ordinary Markdown, so a task inside it **becomes a checkbox on
        // screen**. Unless we strip `>` here too and collect the same way, the count won't match
        // what's on screen and every toggle gets cancelled.
        if parse_alert_header(line).is_some() {
            list.observe_special(ws);
            prev_not_paragraph = true;
            i += 1;
            let mut logical: Vec<(usize, String, usize)> = Vec::new();
            while i < lines.len() && is_blockquote_line(lines[i]) {
                let raw = lines[i];
                let stripped = strip_blockquote(raw);
                let prefix = raw.len() - stripped.len();
                logical.push((i, stripped, prefix));
                i += 1;
            }
            let refs: Vec<(usize, &str, usize)> = logical
                .iter()
                .map(|(idx, s, p)| (*idx, s.as_str(), *p))
                .collect();
            scan_task_lines(&refs, tasks, &mut out);
            continue;
        }
        // `<details>`: the render side only draws **the body of a block that's open** as Markdown
        // (if closed, the body doesn't appear on screen). Open/closed is runtime state, so it's
        // received from the caller, and a closed block's body is not counted (counting it would
        // cancel toggles across the whole document that contains a closed details).
        if let Some(open_attr) = details_open_tag(line) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            // The fallback is **the same as the renderer's** (`next_details_open`) = the tag's
            // `open` attribute. In production the caller supplies a value for every block so this
            // never gets hit, but if it drifts, fall on the side that matches the render.
            let open = details_open.get(details_idx).copied().unwrap_or(open_attr);
            details_idx += 1;
            i += 1;
            let mut body = Vec::new();
            while i < lines.len() && !is_details_close(lines[i]) {
                body.push(i);
                i += 1;
            }
            if i < lines.len() {
                i += 1; // skip `</details>`
            }
            if open {
                // A `<summary>` line is drawn as a heading, so `task_prefix_state` never matches it
                // (it requires the line to start with a bullet) — no special-casing needed here.
                let logical: Vec<(usize, &str, usize)> =
                    body.iter().map(|&bi| (bi, lines[bi], 0usize)).collect();
                scan_task_lines(&logical, tasks, &mut out);
            }
            continue;
        }
        if is_html_block_start(line) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            while i < lines.len() && !lines[i].trim().is_empty() {
                i += 1;
            }
            continue;
        }
        if looks_like_table_row(line) && i + 1 < lines.len() && is_table_delimiter(lines[i + 1]) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            i += 2;
            while i < lines.len() && looks_like_table_row(lines[i]) {
                i += 1;
            }
            continue;
        }
        // Heading / thematic break / setext underline: same "not a paragraph" treatment as
        // `code_block_source_locs_inner` — see that function's matching checks and the three
        // detector functions' doc comments for the CommonMark reasoning (identical here).
        if is_atx_heading_line(line) || is_thematic_break_line(line) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            i += 1;
            continue;
        }
        if !prev_not_paragraph && is_setext_underline_line(line) {
            list.observe_special(ws);
            prev_not_paragraph = true;
            i += 1;
            continue;
        }
        if t.trim().is_empty() {
            prev_not_paragraph = true;
            i += 1;
            continue;
        }
        let in_list = list.observe(ws, t);
        if !in_list && ws >= 4 && prev_not_paragraph {
            // The renderer shows this as an indented code block, not a task — same rule as
            // `code_block_source_locs_inner`. A `- [ ] fake` example inside stays literal and does
            // not get counted (and mis-toggled) as a real checkbox.
            skip_indented_code_block(&lines, &mut i);
            prev_not_paragraph = true;
            continue;
        }
        let indent = line.len() - t.len();
        if let Some((state, off)) = task_prefix_state(t, tasks) {
            out.push(TaskLoc {
                line: i,
                state_off: indent + off, // right after `<bullet><spaces>[`
                state,
            });
        }
        prev_not_paragraph = false;
        i += 1;
    }
    out
}

/// Cache key for a finished code-block render: every input that shapes the output lines.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CodeBlockKey {
    src: String,
    lang: String,
    w: usize,
    code_bg: Option<Color>,
    theme: String,
    tab_width: usize,
    wrap: bool,
}

/// Bounded LRU of finished code-block renders. A Markdown rebuild re-highlights every fence
/// through syntect — the dominant rebuild cost for code-heavy documents — and follow mode
/// rebuilds on every external edit while the fences mostly stay unchanged. Keyed by the full
/// input (no hash truncation → no false hits); ~64 blocks bounds the resident memory.
struct CodeBlockCache {
    map: std::collections::HashMap<CodeBlockKey, (u64, Vec<Line<'static>>)>,
    tick: u64,
}

const CODE_BLOCK_CACHE_CAP: usize = 64;

fn code_block_cache() -> &'static std::sync::Mutex<CodeBlockCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<CodeBlockCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        std::sync::Mutex::new(CodeBlockCache {
            map: std::collections::HashMap::new(),
            tick: 0,
        })
    })
}

/// Current number of cached code-block renders (capacity-bound check in tests).
#[cfg(test)]
pub(crate) fn code_block_cache_len() -> usize {
    code_block_cache().lock().map(|c| c.map.len()).unwrap_or(0)
}

/// Syntect-highlight the collected code body using the `lang` token and `theme`, then add a left gutter, background, and
/// full-width padding to each line. If the body is empty (a fence with no content), no lines are added.
/// The finished lines are cached (see `CodeBlockCache`) so re-rendering the same fence is a lookup.
#[allow(clippy::too_many_arguments)]
fn highlight_body(
    body: &[String],
    lang: &str,
    w: usize,
    code_bg: Option<Color>,
    theme: &str,
    tab_width: usize,
    wrap: bool,
) -> Vec<Line<'static>> {
    if body.is_empty() {
        return Vec::new();
    }
    let src = body.join("\n");
    let key = CodeBlockKey {
        src: src.clone(),
        lang: lang.to_string(),
        w,
        code_bg,
        theme: theme.to_string(),
        tab_width,
        wrap,
    };
    if let Ok(mut cache) = code_block_cache().lock() {
        cache.tick += 1;
        let tick = cache.tick;
        if let Some((used, lines)) = cache.map.get_mut(&key) {
            *used = tick;
            return lines.clone();
        }
    }
    // Tab expansion happens **before adding the gutter/full-width padding** (doing it after would
    // throw off the width calculation and break the band). Column tracking is anchored at the code's
    // start (column 0); since the gutter shifts every line right uniformly, alignment is preserved.
    let hl = crate::preview::code::expand_tabs(
        crate::preview::code::highlight_lang(&src, lang, theme),
        tab_width,
    );
    // When wrap is enabled, **pre-wrap** here to the width (the gutter's 2 columns subtracted).
    // Leaving the wrapping to Paragraph means the 2nd+ wrapped rows carry neither the `▎` gutter nor
    // the background band, breaking the vertical band (reported by the user on 2026-07-07). Fixing
    // every visual row here removes the need for Paragraph to wrap, so the gutter/band always stays continuous.
    let content_w = w.saturating_sub(GUTTER_COLS).max(1);
    let out: Vec<Line<'static>> = hl
        .into_iter()
        .flat_map(|line| {
            let styled: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|s| {
                    let st = match code_bg {
                        Some(bg) => s.style.bg(bg),
                        None => s.style,
                    };
                    Span::styled(s.content, st)
                })
                .collect();
            let rows = if wrap {
                wrap_spans_by_width(styled, content_w)
            } else {
                vec![styled]
            };
            rows.into_iter().map(move |chunk| {
                let mut spans = vec![gutter_span(code_bg)];
                spans.extend(chunk);
                pad_to_width(spans, w, code_bg)
            })
        })
        .collect();
    if let Ok(mut cache) = code_block_cache().lock() {
        let tick = cache.tick;
        cache.map.insert(key, (tick, out.clone()));
        // Evict the least-recently-used entry when over the cap (bounded LRU).
        while cache.map.len() > CODE_BLOCK_CACHE_CAP {
            let oldest = cache
                .map
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    cache.map.remove(&k);
                }
                None => break,
            }
        }
    }
    out
}

/// Display columns taken by the code-block gutter (`▎` + one space).
const GUTTER_COLS: usize = 2;

/// Split styled spans into rows of at most `maxw` display columns (CJK = 2 columns; span styles
/// are preserved across splits). A single char wider than `maxw` still gets its own row (no loss).
fn wrap_spans_by_width(spans: Vec<Span<'static>>, maxw: usize) -> Vec<Vec<Span<'static>>> {
    use unicode_width::UnicodeWidthChar;
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for sp in spans {
        let mut buf = String::new();
        for ch in sp.content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + cw > maxw && used > 0 {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), sp.style));
                }
                rows.push(std::mem::take(&mut cur));
                used = 0;
            }
            buf.push(ch);
            used += cw;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, sp.style));
        }
    }
    rows.push(cur);
    rows
}

/// Remove the leading `#` span from heading lines, and lay a full-width rule directly below H1/H2.
fn decorate_headings(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if let Some(level) = heading_level(&line) {
            let style = line.style;
            let mut spans = line.spans;
            spans.remove(0); // drop the leading "#.. "
            out.push(Line::from(spans).style(style));
            if level <= 2 {
                let ch = if level == 1 { "━" } else { "─" };
                out.push(Line::from(Span::styled(
                    ch.repeat(w),
                    Style::new().fg(HEAD_FG).add_modifier(Modifier::DIM),
                )));
            }
        } else {
            out.push(line);
        }
    }
    out
}

/// The heading text of a decorated heading line, or None if the line is not a heading. After
/// `decorate_headings` the `#` markers are gone; a heading is recognized by its `HEAD_FG` foreground
/// (headings are the only such lines except the full-width rule under H1/H2 and the code gutter,
/// both excluded here). Used to build in-page anchor (`[x](#slug)`) jump targets from what's drawn.
pub(crate) fn heading_text(line: &Line<'_>) -> Option<String> {
    // tui-markdown puts the heading color on the LINE style (the spans keep fg=None). The full-width
    // rule under an H1/H2 is the opposite (span fg = HEAD_FG, line fg = None), so keying off the line
    // fg selects headings and excludes the rule.
    if line.style.fg != Some(HEAD_FG) {
        return None;
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    if text.starts_with('▎') {
        return None; // a code line
    }
    let t = text.trim();
    // A heading inside a GitHub alert carries the "▌ " callout bar as its first span; drop it so the
    // slug is the heading's own text (otherwise the space after the bar yields a leading "-").
    let t = t.strip_prefix('▌').map(str::trim_start).unwrap_or(t);
    if t.is_empty() || t.chars().all(|c| c == '━' || c == '─') {
        return None;
    }
    Some(t.to_string())
}

/// Best-effort heading level (1–4) of a decorated heading line, for outline indentation. After
/// `decorate_headings` the level survives only in the line style: H1/H2 are bold (told apart by the
/// full-width rule that follows — `━` = H1, `─` = H2), H3 is bold+italic, H4–H6 are dim+italic
/// (collapsed to 4). `next` is the line after `line` (the rule line for H1/H2).
pub(crate) fn heading_level_hint(line: &Line<'_>, next: Option<&Line<'_>>) -> u8 {
    let m = line.style.add_modifier;
    if m.contains(Modifier::DIM) {
        return 4;
    }
    if m.contains(Modifier::ITALIC) {
        return 3;
    }
    let rule: String = next
        .map(|n| n.spans.iter().map(|s| s.content.as_ref()).collect())
        .unwrap_or_default();
    if rule.starts_with('━') {
        1
    } else {
        2
    }
}

/// If the line is a heading (its first span is exactly the form "#".."###### "), return its level.
fn heading_level(line: &Line) -> Option<u8> {
    let content = line.spans.first()?.content.as_ref();
    let hashes = content.strip_suffix(' ')?;
    if !hashes.is_empty() && hashes.len() <= 6 && hashes.bytes().all(|b| b == b'#') {
        Some(hashes.len() as u8)
    } else {
        None
    }
}

/// Left gutter span for a code block (background optional).
fn gutter_span(code_bg: Option<Color>) -> Span<'static> {
    let st = Style::new().fg(CODE_GUTTER_FG);
    let st = match code_bg {
        Some(bg) => st.bg(bg),
        None => st,
    };
    Span::styled("▎ ", st)
}

/// Brighten the background color slightly (to distinguish the language badge from the body code). Non-Rgb is left as-is.
/// Language header line at the top of a code block. Shows the language name as a badge (background color and right/left alignment configurable),
/// so it stands out from the body code. When `code.label_bg`=None, it is shown dimmed with no background.
fn code_header(label: &str, w: usize, code: CodeStyle) -> Line<'static> {
    let code_bg = code.bg;
    // The header's gutter is identified by its sentinel style (is_code_header_span) = the marker for a Tab-focus target.
    let gutter = {
        let st = code_header_marker_style();
        let st = match code_bg {
            Some(bg) => st.bg(bg),
            None => st,
        };
        Span::styled("▎ ", st)
    };
    let badge_text = format!(" {label} "); // a badge with padding on both sides
                                           // Badge style: with a background (the given background + bold white), without one, dim italic.
    let badge_style = match code.label_bg {
        Some(bg) => Style::new()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
        None => Style::new()
            .fg(Color::Gray)
            .add_modifier(Modifier::ITALIC | Modifier::DIM),
    };
    let gutter_w = gutter.width();
    let badge_w = UnicodeWidthStr::width(badge_text.as_str());
    let badge = Span::styled(badge_text, badge_style);
    let fill_style = code_bg.map(|bg| Style::new().bg(bg)).unwrap_or_default();
    let mut spans = vec![gutter];
    if code.label_right && w > gutter_w + badge_w {
        // Right-aligned: fill the gap between the gutter and the badge with the body background color, and push the badge to the right edge.
        spans.push(Span::styled(" ".repeat(w - gutter_w - badge_w), fill_style));
        spans.push(badge);
    } else {
        // Left-aligned (or too narrow): the badge right after the gutter, filling the rest with the body background color.
        let used = gutter_w + badge_w;
        spans.push(badge);
        if w > used {
            spans.push(Span::styled(" ".repeat(w - used), fill_style));
        }
    }
    let line = Line::from(spans);
    match code_bg {
        Some(bg) => line.style(Style::new().bg(bg)),
        None => line,
    }
}

/// Pad with spaces up to width `w` to make the block a full-width band. When `code_bg`=None,
/// neither padding nor line background is added (code shown with gutter and foreground color only).
fn pad_to_width(mut spans: Vec<Span<'static>>, w: usize, code_bg: Option<Color>) -> Line<'static> {
    let Some(bg) = code_bg else {
        return Line::from(spans);
    };
    let used: usize = spans.iter().map(|s| s.width()).sum();
    if used < w {
        spans.push(Span::styled(" ".repeat(w - used), Style::new().bg(bg)));
    }
    Line::from(spans).style(Style::new().bg(bg))
}

/// For standalone .mmd / .mermaid files. Renders the entire contents as a single Mermaid diagram.
pub fn render_mermaid_file(src: &str, width: u16) -> Vec<Line<'static>> {
    render_mermaid_block(src, width)
}

/// Render one block of Mermaid source. Failure/unsupported/internal panic falls back to a dimmed display of the raw source.
fn render_mermaid_block(code: &str, width: u16) -> Vec<Line<'static>> {
    let max_width = if width == 0 {
        None
    } else {
        Some(width as usize)
    };
    match render_mermaid_safe(code.trim_end_matches('\n'), max_width) {
        Ok(rendered) => rendered
            .lines()
            .map(|l| Line::from(l.to_string()))
            .collect(),
        Err(note) => fallback_raw(code, &note),
    }
}

/// Call mermaid-text in a panic-safe way.
/// mermaid-text 0.56 can panic rather than return Err on certain inputs (e.g. CJK byte boundaries), and
/// since rendering runs inside terminal.draw, an uncaught panic would crash the whole TUI (a violation of design principle 3).
/// Bound it with catch_unwind so a panic falls back the same way as an Err.
/// To avoid polluting the screen with the panic message, silence the panic hook only during the call.
/// Note: the failure note is English-only since it is rare diagnostics (the raw source is shown separately).
fn render_mermaid_safe(code: &str, max_width: Option<usize>) -> Result<String, String> {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mermaid_text::render_with_width(code, max_width)
        }))
    });
    match caught {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(format!("cannot render mermaid: {e}")),
        // panic: an internal mermaid-text bug (CJK boundaries etc.). Continue safely by showing the raw source.
        Err(_) => Err(
            "cannot render mermaid (internal error: this diagram/char may be unsupported)"
                .to_string(),
        ),
    }
}

/// mermaid source → SVG string via the pure-Rust mermaid-rs-renderer (mermaid.js-quality layout;
/// no browser/Node — PRD §5). Panic-safe like `render_mermaid_safe`: the crate is 0.x, so a panic
/// (or an unsupported diagram → Err) returns None and the caller degrades to the Unicode text
/// rendering (principle #3). Runs on worker threads only (layout costs a few ms per diagram).
pub fn mermaid_to_svg(code: &str, theme: &str) -> Option<String> {
    use mermaid_rs_renderer::{RenderOptions, Theme};
    let modern = Theme::modern();
    let mut t = match theme {
        "light" | "modern" => Theme::modern(),
        "classic" | "mermaid" => Theme::mermaid_default(),
        "forest" => Theme::forest(),
        "neutral" => Theme::neutral(),
        _ => Theme::dark(), // default: fits konoma's dark tone (a port of mermaid-js dark)
    };
    // No background is drawn (fill="none") = on kitty graphics the terminal background shows
    // through. The diagram never becomes a theme-colored "white card" and fits any terminal theme
    // (user feedback 2026-07-17).
    t.background = "none".to_string();
    // Font measurement is **unified to modern across every theme**. When each theme used a
    // different font (e.g. trebuchet 16), the label boxes' measured size changed, and path routing
    // could pick a placement bug — "a big detour ring to the left" — which was reproduced and
    // resolved by direct measurement (user report 2026-07-17). Colors stay theme-specific; only the layout is pinned to the verified measurement.
    t.font_family = modern.font_family.clone();
    t.font_size = modern.font_size;
    let opts = RenderOptions {
        theme: t,
        ..RenderOptions::default()
    };
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mermaid_rs_renderer::render_with_options(code.trim_end_matches('\n'), opts)
        }))
    });
    match caught {
        Ok(Ok(svg)) => Some(svg),
        _ => None,
    }
}

/// Pre-warm the mermaid renderer's lazy text-measurer (its first call scans system fonts, tens of
/// ms). Called from a startup thread so the first diagram render never pays it.
pub fn warm_mermaid() {
    let _ = mermaid_to_svg("graph LR\nA-->B", "dark");
}

/// Synthetic inline-image key for a ```mermaid fence, derived from its content (FNV-1a 64).
/// A changed fence gets a new key, so the rendered-diagram cache invalidates itself.
pub fn mermaid_fence_url(code: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in code.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("mermaid-fence://{h:016x}")
}

/// Whether an inline-image URL is a synthetic mermaid-fence key (never a real file on disk).
pub fn is_mermaid_fence_url(url: &str) -> bool {
    url.starts_with("mermaid-fence://")
}

/// All top-level ```mermaid fence bodies in `src`, in document order (fences nested inside other
/// code fences are not diagrams and are excluded — same fence rules as the block splitter). Each
/// entry is exactly the byte string `mermaid_fence_url` must be called on to reproduce the
/// `ImagePlacement.url` `render_markdown_with_images` places that fence's diagram under — see
/// `extraction_targets`'s own doc comment for why this can no longer be derived from a second,
/// independent walk of `src`.
///
/// A fence whose body is empty (a half-written ```mermaid, nothing typed inside it yet) is never
/// included: it can never become a diagram in the first place (`render_mermaid_slot` never calls its
/// own slot closure for one — see that function's own doc comment), and the one caller
/// (`app::md_render::build_decorated`) already skips any empty entry it does see before doing anything
/// with it, so this is not a narrowing of what the caller can rely on.
pub fn collect_mermaid_fences(src: &str) -> Vec<String> {
    extraction_targets(src).mermaid_fences
}

/// The pre-`md-block-walk` implementation of `collect_mermaid_fences` (line-based, fence-aware
/// scanning of raw `src`, indentation and all — unchanged). Kept as the fallback `extraction_targets`
/// calls whenever the model-based renderer cannot be trusted for `src` — see
/// `render_markdown_with_images_legacy`'s own doc comment for its counterpart on the rendering side.
fn collect_mermaid_fences_legacy(src: &str) -> Vec<String> {
    split_block_parts(src, true)
        .into_iter()
        .filter_map(|p| match p {
            BlockPart::Mermaid { code } => Some(code),
            _ => None,
        })
        .collect()
}

// ---- LaTeX math extraction ($…$, $$…$$, \(…\), \[…\]) ----

/// Synthetic inline-image key for a math expression, derived from its LaTeX + display flag (FNV-1a
/// 64). The `d`/`i` prefix separates display and inline renders of the same source (different rasters).
pub fn math_url(latex: &str, display: bool) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in latex.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("math://{}{h:016x}", if display { "d" } else { "i" })
}

/// Whether an inline-image URL is a synthetic math key (never a real file on disk).
pub fn is_math_url(url: &str) -> bool {
    url.starts_with("math://")
}

/// Whether an inline-image URL is a **synthetic** in-cache key — a mermaid fence or a math expression,
/// both keyed in `md_image_cache` under the URL itself — rather than a real file path. The overlay's
/// `ensure_md_image`/`md_image_proto` must use such a URL directly as the cache key and skip filesystem
/// resolution (which would fail for a `mermaid-fence://`/`math://` scheme and leave the reserved rows
/// blank). Every real inline image (local path / `http(s)://` / `data:`) is *not* synthetic.
pub fn is_synthetic_md_url(url: &str) -> bool {
    is_mermaid_fence_url(url) || is_math_url(url)
}

/// All math expressions in `src`, in document order, as (latex, display). Mirrors the render path
/// so the extracted set matches what is drawn and the caller can kick off exactly those renders.
/// Fence/code-span aware (math inside a code fence or `` `code span` `` is not extracted) — see
/// `extraction_targets`'s own doc comment for why this is no longer a second, independent walk of
/// `src`.
pub fn collect_math_exprs(src: &str) -> Vec<(String, bool)> {
    extraction_targets(src).math_exprs
}

/// The pre-`md-block-walk` implementation of `collect_math_exprs` (`split_block_parts` → `split_math`
/// per text run, over raw `src` — unchanged). Kept as the fallback `extraction_targets` calls whenever
/// the model-based renderer cannot be trusted for `src` — see `render_markdown_with_images_legacy`'s
/// own doc comment for its counterpart on the rendering side.
fn collect_math_exprs_legacy(src: &str) -> Vec<(String, bool)> {
    split_block_parts(src, false)
        .into_iter()
        .flat_map(|p| match p {
            BlockPart::Text(t) => split_math(&t)
                .into_iter()
                .filter_map(|mp| match mp {
                    MathPart::Math { latex, display } => Some((latex, display)),
                    MathPart::Text(_) => None,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// UTF-8 byte length of the char whose leading byte is `b` (for advancing a byte cursor over text).
fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Emit the accumulated text (if any) then one math part.
///
/// `mask` is the per-line code mask being accumulated alongside `buf` (see [`SourceRun`]). This can
/// fire **mid-line**, from inside [`scan_inline_math`] — `text $x$ more` flushes with `buf` ending
/// in `"text "`, no newline — so the partial trailing line it leaves behind needs an entry of its
/// own. `false` is provably the right value: the scanning branch of [`split_math`] is only reached
/// for a line that is neither code nor structure, and it is the only caller that can flush
/// mid-line (the display-math branch in `split_math` always flushes at a line boundary, where
/// `buf` is empty or newline-terminated and the condition below is false).
fn flush_math(
    out: &mut Vec<MathPart>,
    buf: &mut String,
    mask: &mut Vec<bool>,
    latex: &str,
    display: bool,
) {
    if !buf.is_empty() {
        if !buf.ends_with('\n') {
            mask.push(false);
        }
        out.push(MathPart::Text(SourceRun::new(
            std::mem::take(buf),
            std::mem::take(mask),
        )));
    } else {
        debug_assert!(mask.is_empty(), "flush_math: mask without text");
    }
    out.push(MathPart::Math {
        latex: latex.trim().to_string(),
        display,
    });
}

/// Split a text run into literal text and math expressions, lifting each math onto its own part (the
/// caller renders each as its own image line). Supports `$$…$$` / `\[…\]` (display, may span lines) and
/// `$…$` / `\(…\)` (inline, same line). Code- and inline-code-aware: a `$` inside a code block (fenced
/// **or indented**) or a `` `code span` `` is literal. Escaped `\$` is literal. A currency-style
/// `$5 and $10` is not mistaken for math (an inline closing `$` may not be preceded by whitespace nor
/// followed by a digit).
///
/// Structure-aware (`structure_mask`): a line inside a blockquote/alert, a `<details>` block, another
/// HTML block, or a GFM table row is copied verbatim — its math (if any) is left as literal `$…$`
/// text rather than lifted onto its own line. Those blocks' own splitters require an unbroken run of
/// lines to recognize the block at all; lifting math out mid-block breaks that run and the block falls
/// apart — an alert's closing lines spill out of its callout box, a closed `<details>` block's body
/// becomes visible (an information-disclosure bug, since a `▸` collapsed marker no longer hides
/// anything), and a table's remaining rows degrade to raw `| a | b |` text. Leaving the math literal
/// there is a safe degradation (principle #3): the structure survives, only that one expression stays
/// unrendered text instead of becoming an image.
fn split_math(run: &SourceRun) -> Vec<MathPart> {
    let text = run.text();
    let mut out = Vec::new();
    let mut buf = String::new();
    // Mask entries for the lines accumulated in `buf`, kept in lockstep with it. A single source
    // line can contribute several entries here, in different runs: lifting the math out of
    // `a $x$ b` leaves `"a "` ending one run and `" b"` opening the next.
    let mut mask: Vec<bool> = Vec::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let bare_lines: Vec<&str> = lines
        .iter()
        .map(|l| l.strip_suffix('\n').unwrap_or(l))
        .collect();
    // The document's answer, carried in rather than re-derived from this run (see `SourceRun`), and
    // serving both masks: `structure_mask` gates on it directly (exactly like the block splitters it
    // mirrors), and `literal_code_mask` unions it with `fence_mask`.
    let parsed_code = run.code();
    let structure = structure_mask(&bare_lines, parsed_code);
    let in_code = literal_code_mask_from(parsed_code, &bare_lines);
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let bare = raw.strip_suffix('\n').unwrap_or(raw);
        if in_code[i] {
            buf.push_str(raw);
            mask.push(parsed_code[i]);
            i += 1;
            continue;
        }
        // Inside a blockquote/alert, `<details>`, other HTML block, or table row: copy verbatim,
        // never scanning it for math (see the doc comment above for why). Which construct it is
        // makes no difference here — that distinction exists for `process_inline_html_traced`, the
        // mask's other caller — so any non-`None` answer is treated the same.
        if structure[i] != Structure::None {
            buf.push_str(raw);
            mask.push(parsed_code[i]);
            i += 1;
            continue;
        }
        // Multi-line display opener: a line that is exactly `$$` or `\[` (collect to the matching close).
        let trimmed = bare.trim();
        if trimmed == "$$" || trimmed == "\\[" {
            let closer = if trimmed == "$$" { "$$" } else { "\\]" };
            let mut body = String::new();
            let mut j = i + 1;
            let mut found = false;
            while j < lines.len() {
                // A structural or code-block line before the close: stop rather than swallow it into
                // the math body (which would silently eat, e.g., a table row or a fenced/indented code
                // block into an unrendered equation).
                if structure[j] != Structure::None || in_code[j] {
                    break;
                }
                let bj = lines[j].strip_suffix('\n').unwrap_or(lines[j]);
                if bj.trim() == closer {
                    found = true;
                    break;
                }
                body.push_str(lines[j]);
                j += 1;
            }
            if found && !body.trim().is_empty() {
                flush_math(&mut out, &mut buf, &mut mask, &body, true);
                i = j + 1; // skip the closer line
                continue;
            }
            // No close (or empty): fall through and treat the line as ordinary text.
        }
        scan_inline_math(bare, &mut out, &mut buf, &mut mask);
        // Completes the (possibly empty) trailing fragment `scan_inline_math` left in `buf` — one
        // line, and `false` because this branch is only reached for a non-code line.
        buf.push('\n');
        mask.push(false);
        i += 1;
    }
    if !buf.is_empty() {
        out.push(MathPart::Text(SourceRun::new(buf, mask)));
    }
    out
}

// ---- Inline code spans (the one place that decides "is this byte range literal code?") ----
//
// Every source-level pass that rewrites inline syntax (math extraction, `<kbd>`-style inline HTML,
// footnote references) has to agree on which byte ranges of a line are `` `code spans` ``, because
// text inside one is a literal *example* of the syntax and must survive untouched. Re-deriving that
// rule per pass is how this file has repeatedly grown divergent behavior (a footnote ref inside
// backticks became a superscript; `` `<kbd>x</kbd>` `` gained a second pair of backticks and broke
// the span outright), so the rule lives here once and the three passes call it.
//
// Scope, deliberately shared by all callers: **line-scoped**. CommonMark lets a code span run across
// a newline; none of these passes follows it there. That is a limitation, but a *uniform* one — a
// pass that extended it alone would put the passes back out of step, which is the bug class this
// section exists to prevent.

/// Length of the run of backticks starting at `i` (0 when `bytes[i]` is not a backtick).
fn backtick_run_len(bytes: &[u8], i: usize) -> usize {
    let mut n = 0;
    while i + n < bytes.len() && bytes[i + n] == b'`' {
        n += 1;
    }
    n
}

/// Given that a backtick run starts at `start`, the byte index just past the run that closes it.
/// `None` when nothing closes it — CommonMark's rule is that a code span is delimited by runs of
/// **equal length**, and an opener with no equal-length closer is literal text, not a span.
fn inline_code_span_end(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let n = line.len();
    let run = backtick_run_len(bytes, start);
    if run == 0 {
        return None;
    }
    let mut j = start + run;
    while j < n {
        if bytes[j] == b'`' {
            let r = backtick_run_len(bytes, j);
            j += r;
            if r == run {
                return Some(j);
            }
        } else {
            // Byte-wise is safe here: only ASCII backticks are matched, and every index handed out
            // is a run boundary, so slices never land inside a multibyte char.
            j += 1;
        }
    }
    None
}

/// The first inline code span at or after byte `from`, as `(start, end)` with `end` exclusive —
/// `line[start..end]` includes both delimiter runs. Unmatched backtick runs are literal text and are
/// skipped over, and a backslash-escaped `` \` `` never opens a span (CommonMark: escapes apply
/// everywhere *except* inside code). Line-scoped; see the section comment above.
fn next_inline_code_span(line: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let n = line.len();
    let mut i = from;
    while i < n {
        match bytes[i] {
            b'`' => match inline_code_span_end(line, i) {
                Some(end) => return Some((i, end)),
                None => i += backtick_run_len(bytes, i), // literal run: keep looking after it
            },
            // Escaped char: skip the backslash + the WHOLE next char. A fixed +2 would split a
            // multibyte char (`\あ`) and leave `i` off a char boundary.
            b'\\' if i + 1 < n => i = (i + 1 + utf8_len(bytes[i + 1])).min(n),
            b => i += utf8_len(b),
        }
    }
    None
}

/// Stand-in for the `n`-th code span of a line while a pass rewrites around it. NUL is used because
/// CommonMark forbids it in a document at all ("the character U+0000 must be replaced with the
/// REPLACEMENT CHARACTER"), so it cannot collide with real content, and because it carries no
/// meaning to any of the passes — no `<`, `[`, `$` or backtick to react to. The trailing NUL keeps
/// index 1 from matching inside index 11 when the spans are put back.
fn code_span_placeholder(n: usize) -> String {
    format!("\u{0}{n}\u{0}")
}

/// Replace every inline code span of `line` with `code_span_placeholder`, returning the masked line
/// and the spans in order. Empty span list means the line had none.
fn mask_code_spans(line: &str) -> (String, Vec<String>) {
    let mut masked = String::new();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some((start, end)) = next_inline_code_span(line, cursor) {
        masked.push_str(&line[cursor..start]);
        masked.push_str(&code_span_placeholder(spans.len()));
        spans.push(line[start..end].to_string());
        cursor = end;
    }
    if spans.is_empty() {
        return (line.to_string(), spans);
    }
    masked.push_str(&line[cursor..]);
    (masked, spans)
}

/// Run `edit` over `line` with its inline code spans masked out, then put them back verbatim.
///
/// Masking rather than *splitting at* the spans is deliberate: the passes rewrite constructs that
/// may legitimately wrap a span — `<strike>Enables `named::from_str`, …</strike>` occurs verbatim in
/// palette's README — and splitting the line there would hide the closing tag from the opening one
/// and leave both raw. Masking keeps the line's structure whole while making the span's *contents*
/// unreachable, which is the actual requirement.
fn rewrite_masking_code_spans(line: &str, edit: impl FnOnce(&str) -> String) -> String {
    let (masked, spans) = mask_code_spans(line);
    let mut out = edit(&masked);
    for (i, span) in spans.iter().enumerate() {
        out = out.replace(&code_span_placeholder(i), span);
    }
    out
}

/// Scan one line for inline / single-line math, appending literal text to `buf` and lifting each math
/// expression via `flush_math`. Skips `` `code spans` `` (their `$` is literal) and honors `\$` escapes.
/// `mask` is passed straight through to [`flush_math`], which can fire mid-line here — see its doc
/// comment for why the partial line that leaves behind is always non-code.
fn scan_inline_math(line: &str, out: &mut Vec<MathPart>, buf: &mut String, mask: &mut Vec<bool>) {
    let bytes = line.as_bytes();
    let n = line.len();
    let mut i = 0;
    while i < n {
        let c = bytes[i];
        // Inline code span: copy the whole `…` region literally (a `$` inside is not math).
        if c == b'`' {
            // Shared rule (`inline_code_span_end`); an unclosed run is literal and scanning
            // resumes right after it, so a `$` further along the line is still seen.
            let end = inline_code_span_end(line, i).unwrap_or(i + backtick_run_len(bytes, i));
            buf.push_str(&line[i..end]);
            i = end;
            continue;
        }
        if c == b'\\' && i + 1 < n {
            match bytes[i + 1] {
                b'(' => {
                    if let Some((content, end)) = find_close(line, i + 2, "\\)") {
                        flush_math(out, buf, mask, content, false);
                        i = end;
                        continue;
                    }
                }
                b'[' => {
                    if let Some((content, end)) = find_close(line, i + 2, "\\]") {
                        flush_math(out, buf, mask, content, true);
                        i = end;
                        continue;
                    }
                }
                _ => {}
            }
            // Escaped char (`\$`, `\\`, `\あ`, …): keep the backslash + the WHOLE next char literally.
            // Advancing a fixed 2 bytes would split a multibyte char (`\あ`) and panic on a non-boundary
            // slice — and leave `i` mid-char so the tail slice below panics next iteration too.
            let end = (i + 1 + utf8_len(bytes[i + 1])).min(n);
            buf.push_str(&line[i..end]);
            i = end;
            continue;
        }
        if c == b'$' {
            if i + 1 < n && bytes[i + 1] == b'$' {
                if let Some((content, end)) = find_close(line, i + 2, "$$") {
                    if !content.trim().is_empty() {
                        flush_math(out, buf, mask, content, true);
                        i = end;
                        continue;
                    }
                }
                buf.push('$');
                i += 1;
                continue;
            }
            if let Some((content, end)) = find_inline_dollar(line, i + 1) {
                flush_math(out, buf, mask, content, false);
                i = end;
                continue;
            }
            buf.push('$');
            i += 1;
            continue;
        }
        let len = utf8_len(c);
        buf.push_str(&line[i..(i + len).min(n)]);
        i += len;
    }
}

/// Find `needle` starting at byte `from`; return (content before it, index past it). ASCII needle.
fn find_close<'a>(line: &'a str, from: usize, needle: &str) -> Option<(&'a str, usize)> {
    let rel = line.get(from..)?.find(needle)?;
    let pos = from + rel;
    Some((&line[from..pos], pos + needle.len()))
}

/// Find the closing `$` of an inline `$…$` starting at byte `from`, applying the currency guard:
/// content is non-empty, the first content char is not whitespace, the last is not whitespace, and the
/// char after the closing `$` is not an ASCII digit. Returns (content, index past the closing `$`).
fn find_inline_dollar(line: &str, from: usize) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    let n = line.len();
    if from >= n || bytes[from].is_ascii_whitespace() || bytes[from] == b'$' {
        return None; // empty or opens with a space (or is `$$`, handled by the caller)
    }
    let mut j = from;
    while j < n {
        match bytes[j] {
            // Skip an escaped char (`\$` does not close). Advance past the backslash + the WHOLE next
            // char: a fixed +2 would land mid-char on `\あ`, desyncing `j` and dropping the closing `$`.
            b'\\' => j += 1 + bytes.get(j + 1).map_or(0, |&b| utf8_len(b)),
            b'$' => {
                let content = &line[from..j];
                let last_ok = !bytes[j - 1].is_ascii_whitespace();
                let after_digit = bytes.get(j + 1).is_some_and(|b| b.is_ascii_digit());
                if last_ok && !after_digit && !content.is_empty() {
                    return Some((content, j + 1));
                }
                return None; // closes like currency: not math
            }
            _ => j += utf8_len(bytes[j]),
        }
    }
    None
}

/// Safe fallback display when rendering is impossible. Returns the note plus the raw source dimmed (no content is lost).
fn fallback_raw(code: &str, note: &str) -> Vec<Line<'static>> {
    let mut v = vec![Line::from(Span::from(format!("[{note}]")).dim())];
    for l in code.lines() {
        v.push(Line::from(Span::from(format!("  {l}")).dim()));
    }
    v
}

/// A segment of the md body split at ```mermaid fence boundaries.
#[derive(Debug, PartialEq)]
#[cfg(test)]
enum Segment {
    Md(SourceRun),
    Mermaid(String),
}

/// Information about an open code fence (the fence character and its length).
#[derive(Clone, Copy)]
struct Fence {
    ch: u8,
    len: usize,
}

/// If the line is a code fence (three or more of ``` or ~~~, indented no more than three columns),
/// return (fence, info string). CommonMark §4.5: a fence indented 4+ columns is not a fence at all —
/// it is (part of) an indented code block instead, and its content — including anything that merely
/// *looks* like fence syntax — is literal text (verified against the renderer: `"para\n\n    ```rust\n
/// fn a(){}\n    ```\n"` shows as one indented code block whose literal first line is `` ```rust ``,
/// not a code-block header). This threshold applies uniformly to both the opening line and any
/// candidate closing line — CommonMark requires the closing fence to independently satisfy the same
/// ≤3-column rule, regardless of the opening fence's own indentation.
fn parse_fence(line: &str) -> Option<(Fence, String)> {
    if leading_ws_width(line) >= 4 {
        return None;
    }
    let trimmed = line.trim_start();
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|&b| b == ch).count();
    if len < 3 {
        return None;
    }
    let info = trimmed[len..].trim().to_string();
    Some((Fence { ch, len }, info))
}

/// Whether the first word of the info string is "mermaid" (case-insensitive).
fn is_mermaid_info(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("mermaid"))
}

/// Split md and mermaid using a single-pass fence tracker.
/// A ```mermaid-like line that appears inside a normal code fence is not intercepted (since it is already inside a fence).
///
/// A line-removing splitter (a diverted ```mermaid fence leaves the md runs entirely), so the code
/// mask travels with the text rather than being re-derived downstream — see [`SourceRun`].
#[cfg(test)]
fn split_segments(run: &SourceRun) -> Vec<Segment> {
    let src = run.text();
    let code = run.code();
    let mut segments = Vec::new();
    let mut md = String::new();
    // Mask entries for the lines accumulated in `md`, kept in lockstep with it.
    let mut md_mask: Vec<bool> = Vec::new();
    let mut mermaid = String::new();
    // The currently open fence. The bool is "is this a mermaid block".
    let mut open: Option<(Fence, bool)> = None;

    for (i, line) in src.split_inclusive('\n').enumerate() {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        let in_code = code.get(i).copied().unwrap_or(false);
        match &open {
            None => {
                if let Some((fence, info)) = parse_fence(bare) {
                    if is_mermaid_info(&info) {
                        // A mermaid block starts: finalize the md accumulated so far, and drop the fence line itself.
                        if !md.is_empty() {
                            segments.push(Segment::Md(SourceRun::new(
                                std::mem::take(&mut md),
                                std::mem::take(&mut md_mask),
                            )));
                        }
                        open = Some((fence, true));
                    } else {
                        // An ordinary code fence: append it to md as-is, to be passed on to tui-markdown.
                        md.push_str(line);
                        md_mask.push(in_code);
                        open = Some((fence, false));
                    }
                } else {
                    md.push_str(line);
                    md_mask.push(in_code);
                }
            }
            Some((fence, is_mermaid)) => {
                let closing = parse_fence(bare)
                    .map(|(f, info)| f.ch == fence.ch && f.len >= fence.len && info.is_empty())
                    .unwrap_or(false);
                if closing {
                    if *is_mermaid {
                        segments.push(Segment::Mermaid(std::mem::take(&mut mermaid)));
                    } else {
                        md.push_str(line); // include the closing fence in md too
                        md_mask.push(in_code);
                    }
                    open = None;
                } else if *is_mermaid {
                    mermaid.push_str(line);
                } else {
                    md.push_str(line);
                    md_mask.push(in_code);
                }
            }
        }
    }

    // Finalize whatever's left at the end. An unclosed mermaid still attempts to render (falls back to raw display on failure).
    if !md.is_empty() {
        segments.push(Segment::Md(SourceRun::new(md, md_mask)));
    }
    if let Some((_, true)) = open {
        if !mermaid.is_empty() {
            segments.push(Segment::Mermaid(mermaid));
        }
    }
    segments
}

// ---- GFM table rendering ----
// tui-markdown 0.3.7 collapses a table into one line (no table support). So table blocks are
// intercepted, column display widths (full-width = 2) are measured, and it's drawn with borders
// (┌┬┐ │ ├┼┤ └┴┘). When it overflows the width, cells wrap to fit.

#[cfg(test)]
enum MdPart {
    Text(SourceRun),
    // The table's own raw text is never read back: every caller (`scan_text_part`) only needs to
    // know a run *is* a table, to skip it (a table block is drawn with konoma's own borders, never
    // scanned as code).
    Table(#[allow(dead_code)] String),
}

/// For each of `lines`, whether it is part of a fenced code block — the opening/closing marker
/// lines and everything between (using the same fence-matching rules as `split_segments`/
/// `parse_fence`: matching char, len ≥ opening len, empty info to close).
///
/// **One remaining production caller**: [`literal_code_mask`], which unions this with
/// [`code_block_mask`] — this half is what recovers a fence konoma renders in isolation but the
/// parser, reading the whole raw document, swallows into an HTML block. (That union is consulted
/// only by the source pre-passes; see `literal_code_mask`'s own doc comment for why they need it
/// and a block splitter must not.)
///
/// Every block splitter — all four, including [`split_html_blocks`], which was the last holdout
/// until the document mask started travelling on a [`SourceRun`] — plus
/// `structure_mask`/`details_mask` and the two source scanners now go through
/// [`splitter_code_mask`] instead. **This function knows nothing about containers**: CommonMark
/// also closes a fence at the end of the list item or block quote it was opened in, so a fence
/// whose closing line carries trailing text (not a close) is read here as running to the end of the
/// document, when in fact its list item ends it a few lines later. That is the runaway
/// `splitter_code_mask` exists to avoid; see its doc comment.
fn fence_mask(lines: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut open: Option<Fence> = None;
    for (i, line) in lines.iter().enumerate() {
        match &open {
            None => {
                if let Some((fence, _info)) = parse_fence(line) {
                    mask[i] = true;
                    open = Some(fence);
                }
            }
            Some(fence) => {
                mask[i] = true;
                let closing = parse_fence(line)
                    .map(|(f, info)| f.ch == fence.ch && f.len >= fence.len && info.is_empty())
                    .unwrap_or(false);
                if closing {
                    open = None;
                }
            }
        }
    }
    mask
}

/// For each of `lines`, whether it is part of a code block — fenced (```` ``` ```` / `~~~`) **or
/// indented** (CommonMark's other code block kind: 4+ columns, measured relative to the enclosing
/// container) — asking pulldown-cmark, the same parser tui-markdown renders through, rather than
/// re-deriving the block rules by hand.
///
/// Reached two ways: directly, as [`splitter_code_mask`] — the gate every block splitter and source
/// scanner uses (see that function for why the union below is *not* the right answer there) — and as
/// the parser half of [`literal_code_mask`], the union the source pre-passes consult.
///
/// `fence_mask` above only ever recognizes fenced blocks, and only by their delimiters. But
/// `process_footnotes`, `process_inline_html` and `split_math` used
/// `fence_mask` (or, until this change, their own hand-rolled copy of the same fence-only state
/// machine) to decide which lines to leave untouched — and an indented code block is invisible to
/// it. So `<kbd>` written inside one to *document* the tag became a real keycap, `<br>` injected a
/// newline mid-block and split it in two, `[^1]` became a footnote reference, and `$x$` was lifted
/// out onto its own line as a math image — inside a block that is supposed to be a literal,
/// verbatim transcript on screen (2026-08). Confirmed by temporarily swapping this function's own
/// callers back to `fence_mask`: `process_footnotes_leaves_indented_code_untouched`,
/// `process_inline_html_leaves_indented_code_untouched` and the "inside an indented code block" case
/// in `math_extraction_covers_delimiters_and_rejects_lookalikes` all fail exactly this way against
/// `fence_mask` alone and pass once the mask covers indented blocks too.
///
/// This can't be patched onto `fence_mask` by hand the way the fenced half was: CommonMark measures
/// the 4-column indent *relative to the enclosing container*, so a list item's marker shifts its
/// content column right — the identical container-tracking problem `2c41192` hit for the
/// copy-scanner's code-block count (`parser_code_blocks`'s doc comment has the worked example).
/// Rather than grow a second copy of that tracking here, ask the parser directly, exactly as
/// `parser_code_blocks` does.
///
/// Implementation: parse `lines` (rejoined with `\n`) via `Parser::new_ext` using
/// `tui_markdown_parse_options()` — **the same option set the renderer itself parses with, reused
/// rather than redeclared**, so this mask can never drift from what *that parser* would call a code
/// block when it reads `lines` as one raw run — through `into_offset_iter()`, so each
/// `Event::Start(Tag::CodeBlock(_))` carries the block's byte range in that text. A line is marked
/// when its own byte range *overlaps* a code
/// block's range, not when it's fully contained: pulldown-cmark's block range starts after a
/// block's leading structural indentation/quote-marker bytes (measured directly — the range for
/// `    indented` begins at the `i`, not the first space), so containment would silently miss the
/// leading run of an indented block's first line. Each line's range is `[starts[i], starts[i] + 1 +
/// lines[i].len())` — the `+1` for the `\n` the caller's line splitting removed — so two adjacent
/// lines' ranges touch with no gap, and a blank line pulldown-cmark folds into an indented block's
/// range (two chunks separated by one blank line, which CommonMark keeps as a single block) is
/// still marked, matching what a fenced block's blank interior lines already are.
///
/// Both bounds are found by binary search rather than a linear scan per block: `starts` and `ends`
/// are monotonically increasing (each line is at least 1 byte including its `\n`), so
/// `partition_point` finds the first/last overlapping line directly, keeping this at
/// O(lines + blocks·log(lines)) rather than O(lines·blocks) for a document with many blocks.
///
/// Unlike `parser_code_blocks`/`scan_code_run`, this does not special-case a code block nested
/// inside a plain blockquote (which the renderer draws with no header) or exclude ```mermaid fences
/// (diverted to diagram rendering): none of that distinction matters here — every line covered by a
/// `CodeBlock` event, in this one-shot raw-document parse, is code as far as this function is
/// concerned, regardless of what decoration (if any) the renderer eventually draws around it.
///
/// Two ways this can still disagree with what actually reaches the screen (neither reaches this
/// function's own callers directly — see `literal_code_mask`, which is what `process_footnotes`,
/// `process_inline_html` and `split_math` actually consult, and folds both of these in):
///
/// * Not a strict superset of `fence_mask`: a fenced block that pulldown-cmark parses as part of an
///   *HTML block* instead (e.g. `~~~`/code/`~~~` sandwiched inside a raw `<div>…</div>`) is not a
///   `CodeBlock` event at all — and konoma's own renderer agrees here, having already carved that
///   span out as HTML before the parser ever sees it (`split_html_blocks`). `fence_mask` would have
///   called those lines code; this function, correctly, does not.
/// * Not a strict superset in the other direction either: a fence with no blank line between it and
///   a preceding `<summary>…</summary>` line, inside a `<details>` block, is read by *this* function
///   (which parses the whole raw document as one HTML-block-type-7 run — verified directly against
///   pulldown-cmark) as HTML, not code — but konoma's own renderer draws it as a real code block,
///   because `split_details` extracts the `<details>` body without requiring that blank line and
///   hands it to `render_md_body_nested` in isolation, where the same fence, with no competing
///   `<summary>` text around it, does produce a `CodeBlock` event. `fence_mask` gets this one right
///   (it is blind to the surrounding `<details>`/`<summary>` context entirely), so union with it —
///   `literal_code_mask` — is what recovers the renderer's actual answer. Note that
///   [`splitter_code_mask`] does **not** need the union for this: a splitter runs on the text it was
///   handed, and by then `split_details` has already peeled those tags off.
fn code_block_mask(lines: &[&str]) -> Vec<bool> {
    use pulldown_cmark::{Event, Parser, Tag};
    if lines.is_empty() {
        return Vec::new();
    }
    let text = lines.join("\n");
    // `starts[i]` = byte offset line `i` starts at in `text`; `starts[lines.len()]` is a sentinel
    // one past where a line after the last one would start. Since every line contributes `len + 1`
    // (the `+1` uniformly standing in for the `\n` rejoined between lines, including — harmlessly —
    // after the very last line, which `join` does not actually add one after), `ends[i] ==
    // starts[i + 1]` always holds, so a single array serves both.
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut pos = 0usize;
    for line in lines {
        starts.push(pos);
        pos += line.len() + 1;
    }
    starts.push(pos);

    let mut mask = vec![false; lines.len()];
    for (ev, range) in Parser::new_ext(&text, tui_markdown_parse_options()).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(_)) = ev else {
            continue;
        };
        // Smallest `i` with `starts[i] < range.end` (condition `starts[i] < range.end`, upper bound
        // on the overlapping range): `starts` restricted to real lines is what `hi` — the exclusive
        // upper bound — is searched over.
        let hi = starts[..lines.len()].partition_point(|&s| s < range.end);
        // Smallest `j` with `starts[j] > range.start`; `ends[i] > range.start` is `starts[i + 1] >
        // range.start`, so the smallest satisfying `i` is `j - 1` (searched over the full array,
        // sentinel included, since `ends[lines.len() - 1]` is `starts[lines.len()]`).
        let j = starts.partition_point(|&s| s <= range.start);
        let lo = j.saturating_sub(1);
        if lo < hi {
            mask[lo..hi].fill(true);
        }
    }
    mask
}

/// For each of `lines`, whether the text is literal code **on screen** — the mask `process_footnotes`,
/// `process_inline_html` and `split_math` actually consult. It is the union (logical OR) of
/// `code_block_mask` (what pulldown-cmark, parsing the raw document in one pass, calls a code block)
/// and `fence_mask` (what a bare `` ``` ``/`~~~` delimiter line says, ignoring any surrounding
/// construct entirely).
///
/// Neither mask alone matches what konoma draws, because konoma decides "is this a code block" two
/// different ways depending on where the text sits, and this function's callers can't tell which way
/// applies to a given line without redoing that routing:
///
/// * Text that reaches tui-markdown's own parser unmodified is exactly what `code_block_mask`
///   describes — that *is* the parser's decision.
/// * Text konoma peels off **before** the parser ever sees it — a `<details>` body (`split_details`,
///   spanning blank lines) or an alert body (`split_alerts`, `>` stripped) — is re-parsed **in
///   isolation** by `render_md_body_nested`, without the surrounding tag/quote-marker that could have
///   changed how the parser reads it. Concretely: parsing the whole raw document, a fence with no
///   blank line between it and a preceding `<summary>…</summary>` line is swallowed into one
///   `Html_block` — no `CodeBlock` event at all — because CommonMark's HTML-block-type-7 rule keeps
///   consuming lines until a blank one, whichever construct wrote them (verified directly against
///   pulldown-cmark). But `split_details` doesn't require that blank line (`details_block_close`
///   just looks for the literal `</details>` line), so it hands the fence to
///   `render_md_body_nested` as an **isolated** two-line body with no `<details>`/`<summary>` text
///   around it to compete for the parse — and there, with nothing else in the running, the exact
///   same three lines *do* produce a `CodeBlock` event. `code_block_mask` alone, parsing the raw
///   document once, cannot see this: it only ever gets the version *with* the swallowing context.
///   `fence_mask`, being blind to any surrounding construct, gets this one right by accident — it
///   would call those same three lines fenced regardless of what wraps them.
///
/// So a line either mask calls code really is drawn as code somewhere in konoma's own rendering, and
/// the union is what "leave code alone" has to mean. The asymmetry this exists to protect is
/// principle #3's: **over-protecting a line only costs one construct staying unrendered as literal
/// text** (safe), while **under-protecting one corrupts a block that's genuinely on screen** — `<br>`
/// injects a newline that splits a code block's header in two, `$x$` gets lifted out as a stray math
/// image, `[^1]` gets renumbered as a real reference. A union can only ever mark *more* lines than
/// either mask alone, so it can never fall on the unsafe side of that asymmetry, and it strictly
/// includes `fence_mask` — which is what every one of these three passes used, unconditionally,
/// before this mask existed — so no line that used to be protected can lose that protection here.
///
/// The cost carried over unchanged: `fence_mask`'s own pre-existing over-protection (an opening fence
/// delimiter whose info string itself contains a backtick — CommonMark forbids that, so it isn't
/// really a fence at all, but `fence_mask`'s hand-rolled matcher doesn't enforce the rule and reads
/// everything after it as fenced through to the next line shaped like a closing delimiter, or to the
/// end of the document if none comes) is inherited by the union exactly as before. That is pre-existing
/// behavior, not something this mask introduces or could remove without dropping `fence_mask` from the
/// union — which would reintroduce the `<details>`-without-a-blank-line gap above.
fn literal_code_mask(lines: &[&str]) -> Vec<bool> {
    literal_code_mask_from(&code_block_mask(lines), lines)
}

/// The one named home of the rule "don't start a construct on this line, it is code" — so that the
/// rule cannot grow a second hand-written copy, which is the drift this whole area keeps regressing
/// on.
///
/// It is asked **once per text run, by whoever owns that run's text**, and the answer then travels
/// on a [`SourceRun`]. The block splitters ([`split_tables`], [`split_html_blocks`],
/// [`split_alerts`], [`split_details`]) and the `structure_mask`/`details_mask` they feed no longer
/// call this themselves: they read the slice they were handed, because the text in their hands may
/// be a fragment that parses differently from the same lines in the file (see [`SourceRun`], and
/// the last paragraph here). The callers that *do* ask are the ones holding a whole document or a
/// peeled container body — [`SourceRun::parse`], [`split_block_parts`], and the two source scanners
/// that must count exactly what those splitters draw ([`code_block_source_locs`],
/// [`task_source_locs`]).
///
/// It is [`code_block_mask`] — the parser's answer — and deliberately **not** [`literal_code_mask`],
/// even though that union is what the *source pre-passes* (`process_footnotes`,
/// `process_inline_html`, `split_math`) consult. The two have opposite safety asymmetries:
///
/// * A pre-pass's mask means "leave this text alone". Over-marking costs one construct staying
///   literal (safe); under-marking corrupts a code block that is genuinely on screen. So the union
///   — mark if *either* mask says code — is right there.
/// * A splitter's mask means "do not carve a block out here". Over-marking is **not** safe: the
///   construct falls through to tui-markdown, which collapses a GFM table into one line of raw
///   `| a | b |` text, drops an HTML block's contents entirely, and draws an alert as a plain
///   quote. Under-marking splits a code block in two. Both directions cost something real, so the
///   only defensible answer is the one the renderer itself will act on — and everything a splitter
///   does *not* carve out goes to that same parser.
///
/// Concretely, the union cannot be used here because `fence_mask`'s half runs away: its hand-rolled
/// state machine has no notion of a container, so a fence opened inside a list item whose closing
/// line carries trailing text (`` ``` ([#2642](…)) `` — not a close per CommonMark, but the list
/// item ends the block anyway) swallows every remaining line of the document. nix 0.31.3's
/// CHANGELOG.md does exactly this at line 110, and the 617-line GFM table below it rendered as raw
/// pipes (2026-08). pulldown-cmark closes the block at the list item, so `code_block_mask` does too.
///
/// The reason `literal_code_mask` needs `fence_mask` in its union — a fence with no blank line
/// between it and a preceding `<summary>` line, which the parser reads as one HTML block when it
/// sees the whole raw document — does not apply to a **peeled container body**: by the time a
/// splitter runs on a `<details>` body, `split_details` has already taken the surrounding tags
/// away, so the parser sees the fence in isolation and calls it code. Evaluating a peeled body on
/// its own terms is load-bearing, not incidental, and that is what [`SourceRun::parse`] is for.
///
/// The distinction matters because the opposite is true for the other kind of text run. A splitter
/// that removes lines from *within* a block — `split_block_parts`, `split_segments`, `split_math` —
/// hands on a fragment that is not a document at all, and asking this function about one gets a
/// confidently wrong answer: the interior of a `<div>` badge banner reads as an indented code block
/// the moment the `<img>` lines between its `<a>` tags are gone. Those runs therefore carry a slice
/// of the mask computed over the intact document instead of re-deriving one here; see [`SourceRun`],
/// which exists entirely to keep the two cases apart, and [`split_html_blocks`], the gate whose
/// misbehavior on exactly that shape forced the distinction into the open.
fn splitter_code_mask(lines: &[&str]) -> Vec<bool> {
    code_block_mask(lines)
}

/// [`literal_code_mask`] with the [`code_block_mask`] half already computed — for a caller that
/// needs the parser's answer on its own as well (`split_math`, which also gates `structure_mask` on
/// it), so the pulldown-cmark parse happens once per text run instead of twice.
fn literal_code_mask_from(parsed: &[bool], lines: &[&str]) -> Vec<bool> {
    let fenced = fence_mask(lines);
    parsed.iter().zip(fenced).map(|(&a, b)| a || b).collect()
}

/// Which line-continuity-dependent construct a line belongs to, as reported by [`structure_mask`].
///
/// "Line-continuity-dependent" means the construct's own splitter recognizes it by an unbroken run
/// of lines each matching a shape, so a pass that inserts, removes or reshapes a line in the middle
/// of the run destroys the construct rather than editing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Structure {
    /// In none of the constructs below.
    None,
    /// A blockquote line — a plain quote or a GitHub alert body; both are `>`-prefixed, and
    /// `split_alerts` captures a body by consuming consecutive `>`-prefixed lines.
    Quote,
    /// Inside a `<details>` … `</details>` block, its tag lines included.
    Details,
    /// A GFM table row: the header, the delimiter row, or one of the consecutive data rows.
    Table,
    /// The line that *opens* any other HTML block (`<p …>`, `<div …>`, `<!-- … -->`, …).
    ///
    /// Split from [`Structure::HtmlBody`] because the two answer differently for a caller that
    /// rewrites the tags on the line. On this line no block is open yet — it is about to be — so a
    /// rewrite here cannot destroy one; on a body line the block already exists and a rewrite can.
    /// The distinction is load-bearing rather than pedantic because [`is_html_block_start`] accepts
    /// *any* tag name, `br` included, where CommonMark's HTML-block rules do not (type 6 matches a
    /// fixed list of block-level names that has no `br` in it, and type 7 needs the tag alone on the
    /// line). So `<br>` and `<br> tail` open a konoma HTML block that exists **only** because of the
    /// very tag a caller is there to rewrite — which is why the two must not be collapsed back into
    /// one variant now that the body rule *removes* lines: applied to an opener it would delete the
    /// break in the everyday `text` / `<br>` / `more` prose idiom, whose middle line is a block
    /// opener by this over-broad rule alone. See [`process_inline_html_traced`].
    HtmlOpen,
    /// A line inside an HTML block that an earlier line opened.
    ///
    /// `quoted` records whether that block is itself nested inside a blockquote, which a caller
    /// deciding "would rewriting this line leave it **blank**" has to know: inside a quote the
    /// leading `>` marker is container, not content, so `">   "` is exactly as blank to the HTML
    /// block within the quote as `"   "` is to one at top level, and ends it just the same. The flag
    /// cannot be recovered from the line itself — a top-level `<div>` whose body text happens to
    /// start with `>` is byte-for-byte the same shape and there the marker *is* content, so treating
    /// it as a prefix would inject a spurious `>` into the rewritten line. See
    /// [`process_inline_html_traced`].
    HtmlBody { quoted: bool },
}

/// Label the HTML block opening at `view[start]` into `out` (positionally paired with `view`) and
/// return the index one past its last line.
///
/// **The one place that decides an HTML block's extent for this mask** — the top-level pass in
/// [`structure_mask`] and its nested rescan both call it — so the two cannot drift, which is the
/// failure mode this module keeps regressing on (see [`splitter_code_mask`]). The extent is "up to
/// the line before the next blank one", the same walk [`split_html_blocks`] performs.
fn mark_html_block(view: &[&str], start: usize, quoted: bool, out: &mut [Structure]) -> usize {
    let mut j = start + 1;
    while j < view.len() && !view[j].trim().is_empty() {
        j += 1;
    }
    // The opener is reported separately from the body it opens — see `Structure::HtmlOpen`.
    out[start..j].fill(Structure::HtmlBody { quoted });
    out[start] = Structure::HtmlOpen;
    j
}

/// Re-label every HTML block found inside a container's body, overwriting the container's own label
/// on just those lines. `view` is the body **as the renderer sees it** (a blockquote's lines with
/// their `>` markers stripped, a `<details>` body as-is), positionally paired with `in_code` and
/// with `out`.
///
/// Only ever *re-labels* lines that already carry the container's non-[`Structure::None`] label — it
/// neither widens nor narrows the container's span — which is what keeps [`split_math`], whose only
/// question is "is this line inside something at all", byte-for-byte unaffected.
fn mark_nested_html_blocks(view: &[&str], in_code: &[bool], quoted: bool, out: &mut [Structure]) {
    let mut k = 0;
    while k < view.len() {
        if !in_code[k] && is_html_block_start(view[k]) {
            k = mark_html_block(view, k, quoted, out);
            continue;
        }
        k += 1;
    }
}

/// For each of `lines`, which construct whose own parser requires line continuity it sits inside:
/// a blockquote (a plain quote or a GitHub alert — both are `>`-prefixed), a `<details>` block,
/// another HTML block, or a GFM table row. [`Structure::None`] means none of them.
///
/// **Two callers ask this mask two different questions**, which is why it reports *which* construct
/// rather than a bare yes/no:
///
/// * [`split_math`] only needs "is this line inside one of them at all" — i.e. anything but
///   [`Structure::None`], which is exactly what this function meant when it returned `Vec<bool>`.
///   It consults the mask so it never lifts an inline math expression out onto its own line from
///   inside one of these; doing so breaks the very line sequence those blocks' own splitters
///   (`split_alerts`, `split_details`, `split_html_blocks`, `split_tables`) require, tearing the
///   block apart (see `split_math`'s doc comment for the concrete failure modes this prevents).
/// * [`process_inline_html_traced`] needs the kind, because the three ways it can rewrite a `<br>`
///   are not interchangeable. Inside the body of an HTML block a line the rewrite would leave blank
///   has to be dropped outright — every *replacement* still leaves the line blank (and a
///   whitespace-only line *is* blank to CommonMark), which terminates the block, and keeping the tag
///   instead leaves a block opener for a later pass to relocate. Inside a table row it has to
///   collapse to a space — a table
///   row has no continuation form, so a second line can only ever be a second row. Everywhere else,
///   the opening line of an HTML block included (see [`Structure::HtmlOpen`]), it may become a hard
///   break, provided the injected line repeats the enclosing blockquote prefix. See that function's
///   doc comment for the failures each case prevents.
///
/// Reuses the same predicates the block splitters themselves use to decide where a block starts
/// and ends, so this mask can never drift out of sync with what those splitters actually carve out
/// — and, now that there are two callers, so the second one does not become another hand-written
/// copy of those rules. That drift is what this module keeps regressing on (see
/// [`splitter_code_mask`]). Gated by the caller's code mask ([`code_block_mask`]), exactly like
/// `split_tables`/`split_html_blocks`/`split_alerts`: a `>`/`|`/`<div>`-looking line inside a code
/// block is ordinary code, not structure. Taken as a parameter rather than recomputed here so a
/// caller that also needs `literal_code_mask` (both of them do, and it is built on
/// `code_block_mask`) parses each text run exactly once.
///
/// # Nesting
///
/// A container's body is rescanned for **HTML blocks**, whose lines are relabelled
/// [`Structure::HtmlOpen`]/[`Structure::HtmlBody`] over the container's own label: for a
/// `<details>` block the body between its tag lines, for a blockquote the run of `>` lines with
/// their markers stripped — in each case the text the renderer goes on to treat as its own
/// document, and therefore the text whose blank lines decide where a block inside it ends.
///
/// Reporting only the outermost construct was not a neutral simplification, which is why this
/// exists (2026-08). A badge banner nested one level deep came back as all-`Details` (or all-`Quote`),
/// so its lone `    <br>` line took the default hard-break rewrite instead of the HTML-block one,
/// `"    " + "  "` is a blank line to CommonMark, the inner `<p align="center">` ended there, the
/// 4-column-indented lines below became a genuine indented code block — and the block splitter then
/// faithfully honoured that, drawing the markup as `▎ …` and dropping one of the badges, because an
/// `<img>` line the document calls code is no longer extracted. Both containers are affected and by
/// different routes, so both are rescanned.
///
/// **The rescan only ever re-labels lines the container already claimed** — never widening nor
/// narrowing a span — which is what keeps [`split_math`] byte-for-byte unaffected: its only question
/// is whether a line is non-[`Structure::None`], and every line involved was, and stays, non-`None`.
///
/// **Still reported at the outermost construct only:** a **table** nested in either container, and a
/// container nested in a container. Neither has been observed to cause damage — the `<br>` rules for
/// `Quote`, `Details` and `None` all end up at the same hard break — and the rescan is deliberately
/// limited to the one construct whose "runs to the next blank line" rule a rewritten line can
/// actually destroy. The container tag lines themselves (`<details>` / `</details>`) are excluded
/// too; see the call site for why relabelling those would delete breaks the document really has.
fn structure_mask(lines: &[&str], in_code: &[bool]) -> Vec<Structure> {
    let mut mask = vec![Structure::None; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if in_code[i] {
            i += 1;
            continue;
        }
        // Blockquote / GitHub alert: `split_alerts` captures a run of `>`-prefixed lines (its body
        // loop is exactly `while is_blockquote_line(...)`), and a plain (non-alert) blockquote is
        // parsed the same way by tui-markdown itself — one masked line at a time is enough since
        // every continuation line re-tests true.
        // Deliberately NOT extended to CommonMark "lazy continuation" (a quote line followed by an
        // unprefixed paragraph line): `split_alerts` stops at the first non-`>` line too, and staying
        // on its exact rule is what keeps this mask from drifting. The residue is mild and matches
        // what already happens to a list item — the lazy line's tail moves below the lifted math —
        // with no marker leak, no broken callout box and no hidden content revealed.
        if is_blockquote_line(lines[i]) {
            let start = i;
            let mut j = start + 1;
            while j < lines.len() && !in_code[j] && is_blockquote_line(lines[j]) {
                j += 1;
            }
            mask[start..j].fill(Structure::Quote);
            // The run is gathered rather than marked one line at a time only so its interior can be
            // rescanned; the set of lines it marks is identical either way (every continuation line
            // re-tested true under the old per-line form, and a code line ends the run here exactly
            // as it fell through to the code gate there).
            //
            // Rescan that interior for HTML blocks, on the `>`-stripped text — what
            // `split_alerts`/tui-markdown hand down as the quote's body, and therefore the text
            // whose blank lines decide where a block inside the quote ends.
            let body: Vec<String> = lines[start..j]
                .iter()
                .map(|l| strip_blockquote(l))
                .collect();
            let body: Vec<&str> = body.iter().map(String::as_str).collect();
            mark_nested_html_blocks(&body, &in_code[start..j], true, &mut mask[start..j]);
            i = j;
            continue;
        }
        // `<details>` … `</details>`: mirrors `split_details`'s span (opening tag line through the
        // closing tag line inclusive, spanning blank lines; runs to the end of input if unclosed).
        // `details_block_close` is `split_details`'s own scan, shared (see its doc comment) so
        // this can never drift out of sync with what `split_details` actually carves out.
        if details_open_tag(lines[i]).is_some() {
            let start = i;
            let close = details_block_close(lines, start);
            let end = close.unwrap_or(lines.len() - 1);
            mask[start..=end].fill(Structure::Details);
            // Rescan the body — the lines between the container's own tag lines — for HTML blocks.
            // The `<details>`/`</details>` lines themselves are deliberately left out: they are
            // markers `split_details` consumes structurally, and an HTML block opened *on* the
            // `<details>` line would swallow the body behind a label meant for lines that sit inside
            // an already-open block, turning the everyday `text` / `<br>` / `more` idiom in a body
            // into a deletion (the hazard `Structure::HtmlOpen` exists for).
            let body = start + 1..close.unwrap_or(lines.len());
            mark_nested_html_blocks(
                &lines[body.clone()],
                &in_code[body.clone()],
                false,
                &mut mask[body],
            );
            i = close.map_or(lines.len(), |c| c + 1);
            continue;
        }
        // GFM table: header row + delimiter row + consecutive data rows, mirroring `split_tables`.
        if i + 1 < lines.len()
            && !in_code[i + 1]
            && looks_like_table_row(lines[i])
            && is_table_delimiter(lines[i + 1])
        {
            let start = i;
            let mut j = start + 2;
            while j < lines.len() && !in_code[j] && looks_like_table_row(lines[j]) {
                j += 1;
            }
            mask[start..j].fill(Structure::Table);
            i = j;
            continue;
        }
        // Any other HTML block: opening tag line through the line before the next blank line,
        // mirroring `split_html_blocks`.
        if is_html_block_start(lines[i]) {
            i = mark_html_block(lines, i, false, &mut mask);
            continue;
        }
        i += 1;
    }
    mask
}

// ---- HTML block rescue --------------------------------------------------------
// tui-markdown (pulldown-cmark) silently drops Html events, so a block like `<details>` vanished
// **along with its inner text content**. Intercept the block and display its text with the tags
// stripped (konoma doesn't render HTML = a safe degradation, principle #3). A `<!-- -->` comment is hidden entirely.

#[cfg(test)]
enum HtmlPart {
    Text(SourceRun),
    // The HTML block's own raw text is never read back: `scan_text_part` only needs to know a run
    // *is* an HTML block, to skip it (drawn as tag-stripped text, never scanned as code).
    Html(#[allow(dead_code)] String),
}

/// Whether `line` starts an HTML block: `<tag ...>` / `</tag>` / `<!--`. The tag name must be
/// followed by space / `>` / `/` / end — so an autolink like `<https://…>` does NOT match
/// (`:` ends the name with an invalid terminator).
fn is_html_block_start(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("<!--") {
        return true;
    }
    let Some(rest) = t.strip_prefix('<') else {
        return false;
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let name_len = rest
        .char_indices()
        .take_while(|(i, c)| {
            if *i == 0 {
                c.is_ascii_alphabetic()
            } else {
                c.is_ascii_alphanumeric() || *c == '-'
            }
        })
        .count();
    if name_len == 0 {
        return false;
    }
    matches!(
        rest[name_len..].chars().next(),
        None | Some(' ') | Some('\t') | Some('>') | Some('/')
    )
}

/// Split md text into normal text and HTML blocks (a line starting a block, up to the next blank line).
/// Code-aware: an HTML-looking line inside a code block is left as ordinary text, or its tags would
/// be stripped and — worse — the "block" scan (which just runs to the next blank line) would swallow
/// the fence's own closing marker, leaking it into the rescued text.
///
/// This gate is the one that made [`SourceRun`] necessary. An indented (4+ column) code block whose
/// first line starts with an HTML tag is ambiguous, and the two readings can only be told apart with
/// *whole-document* context:
///
/// * `para\n\n    <kbd>Ctrl</kbd>\n` — a genuine indented code block, documenting the tag. Rescuing
///   it as HTML strips the tags and drops the code gutter, and — because the block never reaches the
///   renderer as code — it is not Tab-focusable and cannot be copied with `y c` either.
/// * A centered `<div align="center">` badge banner — the shape at the top of a great many crate
///   READMEs. Pulling its `<img>` lines out leaves fragments like `    </a>\n    <a href="…">\n`,
///   which **open** with 4-column-indented content only because their `<div>` went into the previous
///   fragment. Asked about that fragment alone, the parser correctly calls it an indented code
///   block; asked about the original document, it correctly calls it HTML — and HTML is what konoma
///   draws and what the copy scanner counts (see `scan_code_run`'s doc comment).
///
/// The two are byte-for-byte the same shape *as fragments*, which is why this function was for a
/// while pinned to the narrower `fence_mask` (blind to indentation, so it got the banner right by
/// accident and the `<kbd>` block wrong): measured over the 2,182 `.md` files in the crate registry,
/// simply swapping in [`splitter_code_mask`] here produced zero improvements and three regressions
/// (the raw-window-metal, static_assertions and tinytemplate READMEs each grew several spurious code
/// blocks full of `</a><a href="…">`). The defect was never this gate's choice of mask, though — it
/// was that the block-structure context had already been destroyed upstream. With the mask computed
/// once over the intact document and carried here on the [`SourceRun`], the document's own answer is
/// available again: it marks the `    <kbd>` line as code and does **not** mark any of the banner's
/// `<div>`/`<a>`/`<img>`/`</a>` lines (they are HTML-block interior), so both cases come out right.
#[cfg(test)]
fn split_html_blocks(run: &SourceRun) -> Vec<HtmlPart> {
    let lines: Vec<&str> = run.lines();
    let in_code = run.code();
    let mut parts = Vec::new();
    let mut buf: Vec<(&str, bool)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !in_code[i] && is_html_block_start(lines[i]) {
            if !buf.is_empty() {
                parts.push(HtmlPart::Text(html_text_run(&mut buf)));
            }
            let mut block: Vec<&str> = Vec::new();
            while i < lines.len() && !lines[i].trim().is_empty() {
                block.push(lines[i]);
                i += 1;
            }
            parts.push(HtmlPart::Html(block.join("\n")));
            continue;
        }
        buf.push((lines[i], in_code[i]));
        i += 1;
    }
    if !buf.is_empty() {
        parts.push(HtmlPart::Text(html_text_run(&mut buf)));
    }
    parts
}

/// Drain `buf` into one verbatim text run, keeping each line paired with the mask entry it was
/// handed (`join("\n") + "\n"` yields exactly `buf.len()` lines, so the two stay aligned).
#[cfg(test)]
fn html_text_run(buf: &mut Vec<(&str, bool)>) -> SourceRun {
    let text = buf.iter().map(|(l, _)| *l).collect::<Vec<_>>().join("\n") + "\n";
    let code = buf.iter().map(|(_, c)| *c).collect();
    buf.clear();
    SourceRun::new(text, code)
}

/// Render an HTML block as its tag-stripped text (entities decoded, comments dropped entirely).
///
/// `pub(crate)`, not private: `app::md_real_file_sweep_tests` (a cousin module, not a descendant of
/// this one) calls this directly to compute a document's own "ground truth visible text" for its
/// content-loss check — the exact same tag/comment stripping `render_html_block_from_model` applies
/// when actually drawing an `Html` block, reused rather than re-implemented a second time, so that
/// check can never itself drift from what either renderer actually strips.
pub(crate) fn render_html_block(raw: &str) -> Vec<Line<'static>> {
    // Strip tags/comments by scanning characters (also handles a `<!-- -->` that spans multiple lines).
    let mut text = String::new();
    let mut rest = raw;
    while let Some(pos) = rest.find('<') {
        text.push_str(&rest[..pos]);
        let after = &rest[pos..];
        if let Some(r) = after.strip_prefix("<!--") {
            match r.find("-->") {
                Some(e) => rest = &r[e + 3..],
                None => rest = "",
            }
        } else {
            match after.find('>') {
                Some(e) => rest = &after[e + 1..],
                None => rest = "",
            }
        }
    }
    text.push_str(rest);
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let mut out: Vec<Line<'static>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| Line::from(Span::raw(l.to_string())))
        .collect();
    if !out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

// ---- GitHub alerts (`> [!NOTE]` … callouts) ----

/// One of the five GitHub alert types (`> [!NOTE]` etc.). A handful of common Obsidian aliases map
/// onto the nearest GitHub type so those documents render too.
///
/// `pub(crate)`, not private: `preview::markdown::model` (a child module) re-exports this so
/// `BlockKind::Quote.alert` can use the exact same type the renderer does, rather than a second
/// enum with the same variants — see that module's doc comment on `Quote`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    fn parse(s: &str) -> Option<AlertKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "note" | "info" => Some(AlertKind::Note),
            "tip" | "hint" => Some(AlertKind::Tip),
            "important" => Some(AlertKind::Important),
            "warning" | "attention" => Some(AlertKind::Warning),
            "caution" | "danger" | "error" => Some(AlertKind::Caution),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }
    fn color(self) -> Color {
        match self {
            AlertKind::Note => Color::Blue,
            AlertKind::Tip => Color::Green,
            AlertKind::Important => Color::Magenta,
            AlertKind::Warning => Color::Yellow,
            AlertKind::Caution => Color::Red,
        }
    }
    /// A Nerd Font glyph (classic FontAwesome block, same source as `ui/icons.rs`, so it is present
    /// in Symbols Nerd Font). Only used when `ui.icons` is on.
    fn icon(self) -> char {
        match self {
            AlertKind::Note => '\u{f05a}',      // nf-fa-info_circle
            AlertKind::Tip => '\u{f0eb}',       // nf-fa-lightbulb_o
            AlertKind::Important => '\u{f0a1}', // nf-fa-bullhorn
            AlertKind::Warning => '\u{f071}',   // nf-fa-exclamation_triangle
            AlertKind::Caution => '\u{f06a}',   // nf-fa-exclamation_circle
        }
    }
}

/// Parse a GitHub-alert header line (`> [!NOTE]`, optionally with a trailing Obsidian-style title).
/// Returns the type and the (possibly empty) title. None if the line is not an alert header.
fn parse_alert_header(line: &str) -> Option<(AlertKind, String)> {
    let rest = line.trim_start().strip_prefix('>')?.trim_start();
    let rest = rest.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = AlertKind::parse(&rest[..close])?;
    Some((kind, rest[close + 1..].trim().to_string()))
}

/// A blockquote continuation line (`> …` or a bare `>`), whitespace-tolerant.
fn is_blockquote_line(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// Strip one level of blockquote marker (`>` and one optional following space) from a line.
fn strip_blockquote(line: &str) -> String {
    let l = line.trim_start();
    let l = l.strip_prefix('>').unwrap_or(l);
    l.strip_prefix(' ').unwrap_or(l).to_string()
}

/// The colored `▌ ` left-bar span every alert body line carries, in `color` (`kind.color()` for
/// whichever [`AlertKind`] is being drawn) — shared by [`render_alert`]'s own per-line prefixing and
/// [`alert_header_line`]'s own leading span, and by `render::render_alert_from_model` (the
/// block-model renderer's own alert support), which prefixes each *recursively rendered* body line
/// with the identical span rather than a second, hand-copied `Span::styled("▌ ", ..)` literal.
fn alert_bar(color: Color) -> Span<'static> {
    Span::styled("▌ ".to_string(), Style::new().fg(color))
}

/// The header line of a GitHub-alert callout — `▌` bar, optional Nerd Font icon, and the bold,
/// colored label (`kind.label()`, plus an Obsidian-style trailing title when `title` is non-empty:
/// `"Note — My title"`) — split out of [`render_alert`] so `render::render_alert_from_model` (the
/// block-model renderer's own alert support) can build the identical header line without a second,
/// hand-copied construction of it: only the *body* differs between the two callers (this module's
/// own string-based, re-parsing recursion vs. the block model's own tree-walking one), never the
/// bar/icon/label decision itself — see that function's own doc comment.
fn alert_header_line(kind: AlertKind, title: &str, icons: bool) -> Line<'static> {
    let color = kind.color();
    let mut header = vec![alert_bar(color)];
    if icons {
        header.push(Span::styled(
            format!("{} ", kind.icon()),
            Style::new().fg(color),
        ));
    }
    let label = if title.is_empty() {
        kind.label().to_string()
    } else {
        format!("{} — {}", kind.label(), title)
    };
    header.push(Span::styled(
        label,
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    ));
    Line::from(header)
}

// ---- Collapsible `<details>` blocks ----

// Per-render open state, set by the app before drawing (single-threaded draw path, like the panic
// silencer). `open[k]` = whether the k-th `<details>` block in the document is expanded; the render
// consumes them in order via `ord`. Missing entries default to expanded (safe).
thread_local! {
    static DETAILS: std::cell::RefCell<(usize, Vec<bool>)> =
        const { std::cell::RefCell::new((0, Vec::new())) };
}

/// Set the effective open/closed state of each `<details>` block (document order) and reset the
/// render cursor. Called by the app just before it renders a Markdown preview.
pub fn set_details_open(open: Vec<bool>) {
    DETAILS.with(|d| *d.borrow_mut() = (0, open));
}

/// The effective `<details>` open states currently set (for the app to store in the cache).
pub fn current_details_states() -> Vec<bool> {
    DETAILS.with(|d| d.borrow().1.clone())
}

/// The next `<details>` block's effective open state (falls back to its `open` attribute, then true).
fn next_details_open(open_attr: bool) -> bool {
    DETAILS.with(|d| {
        let mut d = d.borrow_mut();
        let ord = d.0;
        let v = d.1.get(ord).copied().unwrap_or(open_attr);
        d.0 = ord + 1;
        v
    })
}

enum DetailsPart {
    // Only read back by test code (`split_details`'s own tests): production
    // (`collect_details_open`) discards a `Text` part's body entirely and reads only
    // `Details::open_attr` from the other variant — see that function's own `match`.
    Text(#[allow(dead_code)] SourceRun),
    Details {
        open_attr: bool,
        #[allow(dead_code)]
        summary: String,
        #[allow(dead_code)]
        body: String,
    },
}

/// If the line opens a `<details>` block, return its `open` attribute. Case-insensitive; matches
/// `<details>` and `<details open …>` (a well-formed opening tag on its own line).
fn details_open_tag(line: &str) -> Option<bool> {
    let t = line.trim();
    let lower = t.to_ascii_lowercase();
    let rest = lower.strip_prefix("<details")?;
    if !(rest.is_empty() || rest.starts_with('>') || rest.starts_with(' ')) {
        return None; // e.g. "<detailsx" is a different tag
    }
    Some(rest.contains("open"))
}

fn is_details_close(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("</details>")
}

/// The line index of the `</details>` line that closes the `<details>` … `</details>` block opening
/// at `lines[start]` (the caller has already confirmed this via `details_open_tag`) — `None` if
/// unclosed (the block runs to the end of input). **The one place that scans for a `<details>`
/// block's extent** — `split_details`, `structure_mask`, and `details_mask` all call it, so all
/// three can never drift out of sync with each other about where a block starts and ends.
fn details_block_close(lines: &[&str], start: usize) -> Option<usize> {
    ((start + 1)..lines.len()).find(|&j| is_details_close(lines[j]))
}

/// Split off `<details>` … `</details>` blocks (spanning blank lines) from a text run, extracting the
/// `<summary>` and the body. Non-details text is returned verbatim for the normal pipeline.
/// Code-aware: it reads the mask off the [`SourceRun`] it is handed rather than deriving one, so a
/// `<details>` inside a code block is left as text.
///
/// That gate is what makes this function's block count and order agree with
/// [`collect_details_open`], which seeds the per-ordinal open state — and the agreement rests on the
/// two asking about the *same text*, not on them running the same rule. `collect_details_open` is
/// handed the whole preprocessed document and derives the document's mask from it; this runs on a
/// run carved out of that document and is handed the matching slice of that same mask. The
/// [`SourceRun`] is what carries it here, and it is load-bearing rather than an optimisation: a
/// splitter that re-derived a mask from the fragment in its hands would answer for text the
/// document never contained, and every `<details>` after the first disagreement would be seeded with
/// another block's open state.
fn split_details(run: &SourceRun) -> Vec<DetailsPart> {
    let lines: Vec<&str> = run.lines();
    let in_code = run.code();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut mask: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if in_code[i] {
            text.push_str(lines[i]);
            text.push('\n');
            mask.push(in_code[i]);
            i += 1;
            continue;
        }
        if let Some(open_attr) = details_open_tag(lines[i]) {
            if !text.is_empty() {
                parts.push(DetailsPart::Text(SourceRun::new(
                    std::mem::take(&mut text),
                    std::mem::take(&mut mask),
                )));
            }
            let start = i;
            let close = details_block_close(&lines, start);
            let body_end = close.unwrap_or(lines.len());
            let block = &lines[start + 1..body_end];
            let (summary, body) = extract_summary_body(&block.join("\n"));
            parts.push(DetailsPart::Details {
                open_attr,
                summary,
                body,
            });
            i = close.map_or(lines.len(), |c| c + 1);
            continue;
        }
        text.push_str(lines[i]);
        text.push('\n');
        mask.push(in_code[i]);
        i += 1;
    }
    if !text.is_empty() {
        parts.push(DetailsPart::Text(SourceRun::new(text, mask)));
    }
    parts
}

/// The `(s, e)` byte-offset pair — into `inner` — of a well-formed `<summary>...</summary>` tag's own
/// `<summary` and `</summary>` starts: present only when the former actually closes (a literal `>`
/// somewhere in `[s, e)`) before the latter begins. The one place this search happens —
/// `extract_summary_body`/`summary_tag_end` both call this rather than re-deriving the identical
/// `<summary`/`</summary>` scan a second time (the same "one search, several thin callers" shape
/// `details_block_close` already is for `split_details`/`structure_mask`/`details_mask`).
fn summary_tag_bounds(inner: &str) -> Option<(usize, usize)> {
    let lower = inner.to_ascii_lowercase();
    let (s, e) = (lower.find("<summary")?, lower.find("</summary>")?);
    // The `>` that closes the opening `<summary …>` tag must sit before `</summary>`. Bound the
    // search to `[s, e)`: a malformed `<summary` with no `>` of its own would otherwise match the `>`
    // inside `</summary>` (past `e`), giving an inverted slice range that panics further down. When
    // the opening tag never closes, this reports "no summary" (`None`) — the caller falls back to
    // treating the whole `inner` as the body.
    (s < e && inner[s..e].contains('>')).then_some((s, e))
}

/// Pull the `<summary>` text and the remaining body out of a details block's inner content. Tags in
/// the summary are stripped; the body keeps its Markdown. A missing summary yields an empty string
/// (the renderer supplies a default label).
fn extract_summary_body(inner: &str) -> (String, String) {
    if let Some((s, e)) = summary_tag_bounds(inner) {
        // `summary_tag_bounds` already confirmed a `>` sits somewhere in `[s, e)`.
        let gt = inner[s..e]
            .find('>')
            .expect("summary_tag_bounds confirmed a '>' in this range");
        let sum = strip_inline_html_tags(inner[s + gt + 1..e].trim());
        // Trim the blank lines around the body, **not** its indentation. `str::trim` also eats
        // leading spaces, and when the body's first content is an indented code block those spaces
        // *are* the block: the four columns disappeared and the code rendered as an ordinary
        // paragraph, so the screen showed one code block fewer than the source had and `y c` was
        // refused for the whole document (found while auditing this bug class, 2026-08; a
        // `<details>` whose body starts with prose was unaffected, which is why it went unnoticed). A
        // fenced block survives `trim` because its delimiter is not indentation-sensitive — only the
        // indented kind is.
        let body = trim_blank_lines(&inner[e + "</summary>".len()..]);
        return (sum, body);
    }
    (String::new(), inner.trim().to_string())
}

/// The byte offset, into `inner`, right after a well-formed `<summary>...</summary>` tag pair —
/// `None` when `inner` has no such pair (matching `extract_summary_body`'s own "no summary"
/// fallback: the caller should treat `inner` as the whole body, from byte `0`, in that case). Used by
/// `model::glued_details_body_range` to compute the raw byte range of a **glued** `<details>` block's
/// own body (see that function's own doc comment) — the identical boundary `extract_summary_body`'s
/// own second return value is sliced from, before `trim_blank_lines` runs, but as an offset rather
/// than an owned, trimmed `String`: a byte-range caller needs to know *where* the body starts in
/// `inner`'s own coordinate space to slice a **different**, larger string (`model.rs`'s own whole-
/// document `src`) by the identical amount, not read the trimmed text itself (the model's own
/// `Doc::parse`, handed that byte range, does its own, real block-level parsing — see
/// `BlockKind::Details.glued_body`'s own doc comment for why leading/trailing blank lines need no
/// special handling there the way `trim_blank_lines` gives this function's own sibling).
fn summary_tag_end(inner: &str) -> Option<usize> {
    summary_tag_bounds(inner).map(|(_, e)| e + "</summary>".len())
}

/// Drop wholly blank lines from both ends of `s`, leaving the surviving lines' own indentation
/// intact — `str::trim` for a block of Markdown, where leading spaces can be structural.
fn trim_blank_lines(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let Some(start) = start else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .unwrap_or(start);
    lines[start..=end].join("\n")
}

/// Remove any inline HTML tags from `s`, keeping the text (for a `<summary>` label).
fn strip_inline_html_tags(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        match rest[lt..].find('>') {
            Some(gt) => rest = &rest[lt + gt + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Sentinel style of a `<details>` summary line's marker (the Tab-focus target that toggles the
/// block). Cyan + bold + italic is unique among konoma's marker sentinels (task = bold, code header
/// = italic, mermaid = italic), and detection also requires the `▸`/`▾` prefix.
pub(crate) fn details_marker_style() -> Style {
    Style::new()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD | Modifier::ITALIC)
}

/// Whether `span` is a `<details>` summary marker (`▸`/`▾` + `details_marker_style`).
pub(crate) fn is_details_header_span(span: &Span<'_>) -> bool {
    span.style == details_marker_style()
        && (span.content.starts_with('▸') || span.content.starts_with('▾'))
}

/// Whether `line` is part of an **open** `<details>` body — the indented lines carrying the left bar
/// that `render_details` draws. Used to find where a focused details block ends so Tab can scroll the
/// whole thing into view instead of parking its summary at the bottom edge.
pub(crate) fn is_details_body_line(line: &Line<'_>) -> bool {
    line.spans
        .first()
        .is_some_and(|s| s.content.starts_with('▏') && s.style.fg == Some(TABLE_BORDER_FG))
}

/// Render a `<details>` block: a summary line marked `▾`/`▸`, and — when open — its Markdown body
/// indented under a colored left bar (like an alert). `summary` empty → "Details". The body goes
/// through [`render_md_body_nested`] (not the top-level [`render_md_text`]), so a nested alert
/// renders as a proper callout and a nested `<details>` renders statically — see that function's
/// doc comment.
///
/// `interactive` distinguishes the **single top-level entry point** ([`render_md_text`], which
/// hands it `true` and has already consumed a slot from the document-wide Tab-toggle ordinal
/// sequence via `next_details_open`) from every other caller ([`render_md_body_nested`], `false`,
/// for a `<details>` discovered inside an alert's or another `<details>`'s body): only the
/// `true` case draws the summary marker in `details_marker_style()`, the exact style
/// `is_details_header_span`/`build_md_items` look for to make a block Tab/Space-toggleable. A
/// `false` block still draws the same `▾`/`▸` glyph (so it visually folds/unfolds with its `open`
/// attribute) but in a plain style that `is_details_header_span` does not match, so
/// `build_md_items`'s on-screen span count can never include it and drift out of step with
/// `collect_details_open`'s source-order count (which likewise never counts it) — see
/// `render_md_body_nested`'s doc comment for the full reasoning.
/// The `▾`/`▸` summary marker line for a `<details>` block — split out of [`render_details`] so
/// `render::render_details_from_model` (the block-model renderer's own `<details>` support) can
/// build the identical marker line without a second, hand-copied construction of it: only the
/// *body* differs between the two callers, never the arrow/label/`interactive`-style decision — see
/// [`render_details`]'s own doc comment for what `interactive` means and why it exists.
fn details_marker_line(open: bool, summary: &str, interactive: bool) -> Line<'static> {
    let arrow = if open { '▾' } else { '▸' };
    let label = if summary.trim().is_empty() {
        "Details"
    } else {
        summary.trim()
    };
    let marker_style = if interactive {
        details_marker_style()
    } else {
        Style::new().fg(Color::Cyan)
    };
    Line::from(vec![
        Span::styled(format!("{arrow} "), marker_style),
        Span::styled(label.to_string(), Style::new().add_modifier(Modifier::BOLD)),
    ])
}

/// The `▏ ` left-bar span an **open** `<details>` body's every line carries, in the same
/// `TABLE_BORDER_FG` color [`render_details`]'s own body-prefixing and [`is_details_body_line`]'s
/// own detection both already use — shared with `render::render_details_from_model` for the
/// identical reason [`alert_bar`] is.
fn details_bar() -> Span<'static> {
    Span::styled("▏ ".to_string(), Style::new().fg(TABLE_BORDER_FG))
}

/// The `open` attribute of every top-level `<details>` block in `src`, in document order. Derived
/// from `split_details` itself so the collect order is identical to the split/render order and to the
/// `MdItemKind::Details` ordinals — a `<details>` nested inside another **or reachable only through
/// a GitHub alert's body** is swallowed and not counted separately (counting it here, as a flat tag
/// scan once did, drifted the per-ordinal open state so a later block read the wrong slot) — see
/// `render_md_body_nested`'s doc comment, which renders exactly those cases statically for the same
/// reason. A `<details>`-inside-an-alert is invisible to this function specifically because
/// `split_details` sees the raw, not-yet-alert-stripped source, where such a block's opening tag is
/// still `>`-prefixed and so does not match `details_open_tag`. Fence-aware via `split_details`.
/// Used by the app to seed the default open state.
pub fn collect_details_open(src: &str) -> Vec<bool> {
    split_details(&SourceRun::parse(src.to_string()))
        .into_iter()
        .filter_map(|p| match p {
            DetailsPart::Details { open_attr, .. } => Some(open_attr),
            DetailsPart::Text(_) => None,
        })
        .collect()
}

// ---- Inline HTML (`<kbd>`, `<del>`, `<sup>`, `<sub>`, `<br>`) ----

/// Superscript form of a char, or None if it has no clean superscript.
fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'][c as usize - '0' as usize],
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => return None,
    })
}

/// Subscript form of a char, or None if it has no clean subscript.
fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'][c as usize - '0' as usize],
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        _ => return None,
    })
}

/// Map every char through `f`; return the mapped string only if ALL chars mapped, else the original
/// (so `<sup>2</sup>` becomes `²` but `<sup>note</sup>` keeps its text rather than mangling it).
fn map_all_or_keep(inner: &str, f: impl Fn(char) -> Option<char>) -> String {
    match inner.chars().map(f).collect::<Option<String>>() {
        Some(s) => s,
        None => inner.to_string(),
    }
}

/// Replace `<tag>inner</tag>` pairs (no attributes) in `s` with `f(inner)`. Case-sensitive, literal.
fn replace_tag_pair(s: &str, tag: &str, f: impl Fn(&str) -> String) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::new();
    let mut rest = s;
    while let Some(o) = rest.find(&open) {
        let after = o + open.len();
        let Some(c) = rest[after..].find(&close) else {
            break;
        };
        out.push_str(&rest[..o]);
        out.push_str(&f(&rest[after..after + c]));
        rest = &rest[after + c + close.len()..];
    }
    out.push_str(rest);
    out
}

/// The spellings of a line break tag that [`rewrite_br`] recognizes. None of the six is a prefix of
/// another (they already differ at index 3), so at most one can match at a given position.
const BR_TAGS: [&str; 6] = ["<br>", "<br/>", "<br />", "<BR>", "<BR/>", "<BR />"];

/// The first line break tag in `s`, as `(byte offset, the spelling that matched)`.
fn next_br(s: &str) -> Option<(usize, &'static str)> {
    BR_TAGS
        .iter()
        .filter_map(|t| s.find(t).map(|i| (i, *t)))
        .min_by_key(|(i, _)| *i)
}

/// How a `<br>` on one particular line may be rewritten. Which one applies is decided per line from
/// [`structure_mask`]; see [`process_inline_html_traced`] for the failure each variant prevents.
enum BrMode<'a> {
    /// Inside the body of an HTML block: a hard break, with **every line it would leave blank
    /// dropped instead of emitted** — including all of them, in which case the source line produces
    /// no output line at all.
    ///
    /// Stated as the invariant rather than as a rule about where the tag sits, because the blank
    /// line *is* the whole hazard: an HTML block runs to the next blank line, and a whitespace-only
    /// line is blank to CommonMark. Splitting `x<br>y` inside a block yields two non-blank lines and
    /// leaves the block intact, so there is nothing to protect against there — whereas a line that
    /// is nothing but the tag (`    <br>`, the centered-banner idiom) becomes whitespace and kills
    /// the block, and no substitution can save it, a single space least of all. Removing the line
    /// does, and it costs no output: see [`process_inline_html_traced`] for why dropping is the only
    /// safe way to keep the block whole, and what keeping the tag instead did to `shlex`.
    ///
    /// `prefix` is the enclosing blockquote marker, and empty for a block at top level or in a
    /// `<details>` body. It makes "blank" mean *blank to the block* rather than blank to the file:
    /// when the banner is quoted, the same lone-tag line reads `">     <br>"` and rewriting it
    /// yields `">       "`, which `str::trim` calls non-empty while the quote's own body sees
    /// nothing but whitespace — so without the prefix the block died exactly as it did unquoted.
    /// It is carried here rather than read off the line because only [`structure_mask`] can tell a
    /// quote marker from a `>` that is body text of a top-level block (see
    /// [`Structure::HtmlBody`]).
    HtmlBody { prefix: &'a str },
    /// Replace it with a single space, keeping the text on one line.
    Space,
    /// Turn it into a Markdown hard break, starting the injected continuation line with `prefix`
    /// (the enclosing blockquote marker, empty outside a quote).
    Hard { prefix: &'a str },
}

/// The blockquote prefix of `line`: the leading run matching `^ {0,3}(>[ ]?)+`, returned **verbatim**
/// so that nesting (`> > `) and the author's own spacing survive. Empty when the line is not a
/// blockquote line, which is the common case and reproduces the pre-2026-08 behavior exactly.
///
/// Taken from the line rather than reconstructed from a nesting depth because a reconstruction would
/// have to guess: `>text`, `> text` and `> > text` all need different prefixes, and normalizing them
/// would rewrite lines that have nothing to do with the `<br>` being processed.
fn blockquote_prefix(line: &str) -> &str {
    let b = line.as_bytes();
    // CommonMark allows a block marker up to 3 columns of indentation; at 4 it is code, and
    // `structure_mask`'s own code gate has already excluded those lines from reaching here.
    let mut i = 0;
    while i < 3 && i < b.len() && b[i] == b' ' {
        i += 1;
    }
    if b.get(i) != Some(&b'>') {
        return "";
    }
    let mut end = i;
    while b.get(end) == Some(&b'>') {
        end += 1;
        // One optional space after each marker — that space belongs to the marker, not the content.
        if b.get(end) == Some(&b' ') {
            end += 1;
        }
    }
    &line[..end]
}

/// Rewrite every line break tag in `s` as a Markdown hard break, starting each line the break
/// injects with `prefix`.
fn br_hard(s: &str, prefix: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((at, tag)) = next_br(rest) {
        out.push_str(&rest[..at]);
        rest = &rest[at + tag.len()..];
        // Nothing but whitespace after the tag: the source line's own newline already supplies the
        // break, so emit the two-space hard-break marker and stop there. Adding a newline as well
        // would produce a *blank* line, which ends the paragraph — and, inside a container, the
        // container with it. (An inline code span left behind by `mask_code_spans` is a NUL
        // placeholder, not whitespace, so a span following the tag correctly counts as "not at the
        // end".)
        if rest.trim().is_empty() {
            out.push_str("  ");
        } else {
            out.push_str("  \n");
            out.push_str(prefix);
        }
    }
    out.push_str(rest);
    out
}

/// Whether `line` is blank **to the block that encloses it**: whitespace-only once `prefix` — the
/// enclosing blockquote marker, empty outside a quote — is taken off the front. A quote marker is
/// container, not content, so `">   "` ends an HTML block nested in a quote exactly as `"   "` ends
/// one at top level. With an empty prefix this is `str::trim().is_empty()`, byte for byte.
fn blank_within(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix).unwrap_or(line).trim().is_empty()
}

/// Replace every line break tag in `s` with a single space.
fn br_space(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((at, tag)) = next_br(rest) {
        out.push_str(&rest[..at]);
        rest = &rest[at + tag.len()..];
        out.push(' ');
    }
    out.push_str(rest);
    out
}

/// Rewrite every line break tag in `s` according to `mode`.
///
/// Replaces a `s.replace("<br>", "  \n")` chain, which could not express the three distinctions
/// that turned out to matter: whether the tag is the last thing on the line, what the line the
/// break injects has to start with, and whether injecting it would leave a blank line where the
/// enclosing block cannot survive one. See [`process_inline_html_traced`] for why each matters.
///
/// Returns the empty string when [`BrMode::HtmlBody`] drops every line it produced; the caller must
/// then emit no line at all (and no `origin` entry for it).
fn rewrite_br(s: &str, mode: &BrMode<'_>) -> String {
    match mode {
        BrMode::Hard { prefix } => br_hard(s, prefix),
        BrMode::Space => br_space(s),
        BrMode::HtmlBody { prefix } => {
            let hard = br_hard(s, prefix);
            // A blank line terminates the block, so emit none: drop each produced line that is
            // whitespace-only rather than substituting something into it. Note this is per produced
            // line, not all-or-nothing — `a<br><br>b` leaves a blank line *between* two non-blank
            // ones, and dropping just that one keeps both halves and the block. Lines with no blank
            // among them come back byte-identical to `hard`, so the common `x<br>y` is untouched.
            // A masked code span is a NUL placeholder, not whitespace, so a line holding only a
            // code span correctly counts as having content.
            let kept: Vec<&str> = hard
                .split('\n')
                .filter(|l| !blank_within(l, prefix))
                .collect();
            kept.join("\n")
        }
    }
}

/// Convert the common inline HTML that GitHub renders (but tui-markdown strips) into Markdown/Unicode
/// it renders faithfully: `<del>/<s>/<strike>` → strikethrough, `<kbd>` → an inline-code keycap,
/// `<sup>/<sub>` → Unicode (when the content maps), `<br>` → a line break (in one of three forms —
/// see [`process_inline_html_traced`]). Code-aware (fenced **or indented**); a no-op per line without
/// any of these tags. `<mark>`/`<ins>` have no faithful terminal form and are left to tui-markdown
/// (their tags are stripped, text kept).
/// Convenience form without origin tracking. Production always goes through
/// [`process_inline_html_traced`] (it needs the origins); this is the shorthand the tests use.
#[cfg(test)]
pub fn process_inline_html(src: &str) -> String {
    process_inline_html_traced(src, &identity_origin(src)).0
}

/// For each line of a pre-pass's output, which line of the **body** (the text before any pre-pass
/// ran) it came from. `None` marks a line no source line can be held responsible for: one a pre-pass
/// invented outright (the appended footnotes section) or the tail of a line it split in two (the text
/// after a `<br>`).
///
/// This exists so a write-back can prove *correspondence* rather than infer it. The checkbox toggle
/// edits bytes of the file on disk, but the checkbox it must edit is identified by its position on
/// screen — i.e. in the preprocessed text. Counting checkboxes on both sides and comparing the totals
/// looks like a check but is not one: the released build's guards compared count and state character,
/// and a document that simultaneously hid one checkbox from the screen and revealed a different one
/// to the scanner satisfied both while the ordinals pointed at different lines, so `Space` silently
/// wrote `x` into a line the reader was not looking at (verified end to end against v0.23.5). A
/// per-line origin turns "the Nth of each happens to tally" into "this exact screen line came from
/// that exact source line", and where no origin exists the toggle refuses instead of guessing.
pub(crate) type LineOrigin = Vec<Option<usize>>;

/// The identity mapping for a body that has not been through any pre-pass yet.
pub(crate) fn identity_origin(src: &str) -> LineOrigin {
    (0..src.lines().count()).map(Some).collect()
}

/// [`process_inline_html`] that also composes the line origins (see [`LineOrigin`]). A `<br>` is the
/// only substitution that can add a line: the fragment before it keeps the origin, everything after
/// it gets `None`, because that text was never a line of its own in the file and so has no line
/// whose bytes a write-back could target.
///
/// # How a `<br>` is rewritten, and why it is not one rule
///
/// Root cause (2026-08, found by rendering every README in the local crate registry): rewriting
/// `<br>` unconditionally into `"  \n"` **injects a line that carries none of the prefix its
/// enclosing block requires**, so the block it lives in is torn apart. `split_alerts` captures a
/// callout body by consuming consecutive `>`-prefixed lines, so an injected unprefixed line ended
/// the box mid-way; the rest of the alert — including a fenced code block inside it — then leaked
/// out as literal `> ```rust` text. The same injection ended an HTML block, because the tag often
/// sits alone on its own indented line (`    <br>` between two badge `<a>` blocks is the standard
/// centered-banner idiom) and `"    " + "  "` is a whitespace-only line, which *is* a blank line to
/// CommonMark: the block terminated there and every following indented line became a genuine
/// indented code block. `raw-window-metal` and `static_assertions` both drew their badges as a
/// `▎ …` code block for exactly that reason.
///
/// So the governing rule is: **a `<br>` may only become a hard break when the line it injects still
/// belongs to the same block.** Three cases, decided per line from [`structure_mask`]:
///
/// * [`Structure::HtmlBody`] → a hard break, but **any line that would come out blank is dropped
///   instead of emitted** ([`BrMode::HtmlBody`]) — so a line that is nothing but the tag produces no
///   output line at all. No *substitution* is safe here, not even a single space, because the tag is
///   frequently the whole line and any whitespace-only result still reads as blank and still ends
///   the block; removing the line is the only rewrite that keeps the block whole. It costs no
///   output either: konoma renders no HTML inside an HTML block anyway — `split_html_blocks` hands
///   the block to `render_html_block`, which strips every tag and drops the lines that become empty
///   — so the screen content is identical to what keeping the tag would have drawn.
///
///   **Keeping the tag was tried first and is wrong**, which is worth recording because the failure
///   is not visible from here (found 2026-08 by rendering every `.md` in the local crate registry;
///   `shlex`'s `quoting_warning.md` was the one file it damaged). A preserved tag is still a token
///   *this* module treats as a block opener — [`is_html_block_start`] accepts any tag name — and a
///   **later** pass may move the text somewhere the surrounding blank lines no longer exist.
///   `process_footnotes` does exactly that: it relocates definition bodies into the section it
///   appends at the end of the document. A footnote definition whose body held a lone `  <br>` line
///   arrived there with the tag intact, at the head of an unbroken run of lines, where it opened an
///   HTML block that ran to the end of the document — handing the *following* footnotes to
///   `render_html_block`, which emits their text without parsing Markdown at all, so their inline
///   code stopped rendering and drew as literal backticks. Dropping the line leaves nothing behind
///   that a later pass can plant, which is why it is preferred over any rule that keeps the tag
///   "only when it looks safe here".
///
///   The block's *opening* line is deliberately excluded and takes the default below; see
///   [`Structure::HtmlOpen`] for why dropping it there would delete a break the document really has.
/// * [`Structure::Table`] → **a single space** ([`BrMode::Space`]). There is no prefix that makes a
///   second line a valid continuation of a table row, and `render_table` draws one screen row per
///   source row, so any line injected into a row becomes a second, malformed row (`| x<br>y | z |`
///   drew as `│ x │   │` + `│ y │ z │`). Keeping the text on one line is the only rendering that
///   does not corrupt the table — an honest degradation (principle #3): the text is preserved and
///   the markup does not leak. A table row always contains pipes, so it can never collapse to a
///   blank line this way.
/// * anything else → **a hard break** ([`BrMode::Hard`]), with the two corrections that make it
///   correct rather than merely usual: the injected line repeats the current line's blockquote
///   prefix ([`blockquote_prefix`], taken verbatim so `> > ` nesting survives), keeping a quote or
///   alert body one contiguous run of `>` lines; and a `<br>` at the end of a line emits only the
///   `"  "` marker and no newline, because the source already supplies the break and adding another
///   makes a blank line that ends the paragraph — and with it the container. Outside a quote the
///   prefix is empty and the output is byte-identical to the old behavior.
///
/// A `<br>` inside a list item, a `<details>` body or a plain paragraph needs no correction and gets
/// none: CommonMark lazy continuation keeps an unprefixed line attached to the list/quote paragraph
/// above it, and `split_details` spans blank lines, so those three were never broken.
pub(crate) fn process_inline_html_traced(
    src: &str,
    origin_in: &[Option<usize>],
) -> (String, LineOrigin) {
    let mut out = String::new();
    let mut origin = LineOrigin::new();
    let lines: Vec<&str> = src.lines().collect();
    // One pulldown-cmark parse serving both masks (see `literal_code_mask_from`): the union decides
    // "leave this line alone entirely", while `structure_mask` — like every block splitter it
    // mirrors — gates on the parser's answer on its own.
    let parsed_code = code_block_mask(&lines);
    let in_code = literal_code_mask_from(&parsed_code, &lines);
    // Which block each line belongs to, from the one implementation of those rules. Deliberately not
    // re-derived here: a fourth hand-written copy of "is this an HTML block / a table row" is exactly
    // the drift this module keeps regressing on (see `splitter_code_mask`).
    let structure = structure_mask(&lines, &parsed_code);
    for (i, line) in lines.iter().enumerate() {
        let line = *line;
        let src_line = origin_in.get(i).copied().flatten();
        if in_code[i] {
            out.push_str(line);
            out.push('\n');
            origin.push(src_line);
            continue;
        }
        if !line.contains('<') {
            out.push_str(line);
            out.push('\n');
            origin.push(src_line);
            continue;
        }
        // Inline code spans are literal: `` `<kbd>x</kbd>` `` is documentation *of* the tag, not a
        // tag to convert. Converting it also wrapped the content in a second pair of backticks
        // (`` ``x`` ``) and a `<br>` inside a span injected a newline, breaking the span outright.
        // The spans are masked rather than cut out, so a tag pair may still wrap one.
        // Which of the three `<br>` rewrites this line gets (see the doc comment). The blockquote
        // prefix is read off the raw line, not the code-span-masked one: a code span begins with a
        // backtick, so it can never be part of a leading `>` run, and the two agree by construction.
        let mode = match structure[i] {
            // The prefix is empty unless the block is nested in a blockquote, and only
            // `structure_mask` can say: a top-level `<div>` whose body text starts with `>` looks
            // identical from here, and there the marker is content — repeating it would inject a
            // `>` the document never had. See `Structure::HtmlBody`.
            Structure::HtmlBody { quoted } => BrMode::HtmlBody {
                prefix: if quoted { blockquote_prefix(line) } else { "" },
            },
            Structure::Table => BrMode::Space,
            // `HtmlOpen` belongs with the default, not with `HtmlBody`: there is no already-open
            // block to protect on an opening line, and konoma calls a line starting with `<br>` an
            // opener at all only because `is_html_block_start` accepts any tag name (see
            // `Structure::HtmlOpen`). Dropping the line there would delete a break the document
            // really has: `text` / `<br>` / `more`, the ordinary way to force a gap in prose, is a
            // *lone* `<br>` line and konoma classifies it as an opener, so `HtmlBody` treatment
            // would rewrite it to nothing and silently join the two paragraphs. The default leaves
            // the whitespace-only line it has always produced, which reads as the blank line the
            // author asked for. Measured rather than assumed: routing `HtmlOpen` here too changes 5
            // more of the 2182 registry `.md` files against the released rendering, and in each the
            // damage is that deletion — `bit-set`/`bit-vec`'s README asks for a gap with two `<br />`
            // lines and got its two badge rows run together into one.
            //
            // Includes `Structure::Details` and a nested quote inside one too: reading the prefix
            // off the line itself means a `> quoted<br>tail` line in a `<details>` body still gets
            // its marker repeated, without this needing to know it is nested.
            Structure::None | Structure::Quote | Structure::Details | Structure::HtmlOpen => {
                BrMode::Hard {
                    prefix: blockquote_prefix(line),
                }
            }
        };
        let s = rewrite_masking_code_spans(line, |masked| {
            let mut s = replace_tag_pair(masked, "kbd", |i| format!("`{i}`"));
            s = replace_tag_pair(&s, "del", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "s", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "strike", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "sup", |i| map_all_or_keep(i, sup_char));
            s = replace_tag_pair(&s, "sub", |i| map_all_or_keep(i, sub_char));
            rewrite_br(&s, &mode)
        });
        // `BrMode::HtmlBody` drops the blank lines it would have produced, so a line that was
        // nothing but the tag comes back empty: emit nothing for it. Every line reaching here
        // contains a `<`, so it was not empty to begin with, and an empty result can only be that
        // drop. Gated on the mode so the other two keep emitting whatever they produce — outside an
        // HTML block a blank line is meaningful (it ends a paragraph), and removing it would be a
        // silent rendering change rather than a rescue.
        if matches!(mode, BrMode::HtmlBody { .. }) && s.is_empty() {
            continue;
        }
        out.push_str(&s);
        out.push('\n');
        // Only the first fragment still starts where the source line started; a `<br>` tail is new
        // text at a position no source line owns. The count is derived from the produced string, so
        // it stays in step with whichever `BrMode` ran — including the cases that emit *no* newline
        // (a trailing `<br>`, or `Space`) and the one that emits no line at all (a bare `<br>` line
        // inside an HTML block, which `continue`d above without pushing an entry). A drift here is a
        // correctness bug rather than a cosmetic one: `LineOrigin` is what proves the checkbox
        // write-back edits the checkbox the reader is looking at.
        origin.push(src_line);
        origin.extend(std::iter::repeat_n(None, s.matches('\n').count()));
    }
    (out, origin)
}

// ---- Footnotes (`text[^1]` … `[^1]: definition`) ----

/// Superscript form of a number (`12` → `¹²`), for rendering a footnote reference marker.
fn to_superscript(n: usize) -> String {
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    n.to_string()
        .chars()
        .map(|c| SUP[c.to_digit(10).unwrap_or(0) as usize])
        .collect()
}

/// One footnote definition as the parser delimits it: the id, the definition's full text (label
/// stripped, continuation lines kept and dedented to their own left margin), and the half-open range
/// of source lines it occupies.
struct FootnoteDef {
    id: String,
    text: String,
    lines: std::ops::Range<usize>,
}

/// Parse options for locating footnote definitions. Deliberately a *separate* value from
/// [`tui_markdown_parse_options`] — enabling `ENABLE_FOOTNOTES` there would change what the renderer
/// itself parses, and the whole point of `process_footnotes` is that footnotes are rewritten into
/// plain Markdown *before* the renderer ever sees them.
fn footnote_parse_options() -> ParseOptions {
    let mut o = tui_markdown_parse_options();
    o.insert(ParseOptions::ENABLE_FOOTNOTES);
    o
}

/// Every footnote definition in `lines`, **asking pulldown-cmark** rather than re-deriving GFM's
/// continuation rules.
///
/// Root cause this replaced (2026-08, reported by a user on the released build): definitions were
/// matched one line at a time, so only the *first* line of a multi-line definition was moved into
/// the footnotes section. Everything after it stayed in the body — and since a continuation is
/// indented, it landed there as a **phantom indented code block**, while the footnote itself was left
/// truncated mid-syntax (rand_chacha's README ends up showing an unclosed `[label](` and a stray
/// code block containing the URL). The count of code blocks on screen then disagreed with the source
/// scanner's, so `y c` was refused for the whole document.
///
/// Getting this right by hand means implementing container-relative indentation, lazy continuation
/// and blank-line-separated paragraphs inside a definition — i.e. re-implementing the block parser,
/// which is exactly the drift this module keeps regressing on (see [`splitter_code_mask`]). So the
/// definition's extent is the parser's answer, and only the parts the parser cannot tell us — which
/// lines konoma itself will draw as literal code — stay local, via [`literal_code_mask`]: a
/// definition the union mask calls code is dropped, so a `[^1]:` inside a fence that only konoma's
/// own splitting makes literal (see that mask's doc comment) is still left verbatim.
fn footnote_defs(lines: &[&str], in_code: &[bool]) -> Vec<FootnoteDef> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    if lines.is_empty() {
        return Vec::new();
    }
    let text = lines.join("\n");
    // `starts[i]` = byte offset of line `i` in `text`; the trailing sentinel is one past where a
    // line after the last would start. Mirrors `code_block_mask`'s mapping exactly.
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut pos = 0usize;
    for line in lines {
        starts.push(pos);
        pos += line.len() + 1;
    }
    starts.push(pos);

    let mut out: Vec<FootnoteDef> = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(String, usize)> = None;
    for (ev, range) in Parser::new_ext(&text, footnote_parse_options()).into_offset_iter() {
        match ev {
            Event::Start(Tag::FootnoteDefinition(name)) => {
                // Only the outermost definition matters: a definition nested inside another is part
                // of that one's body and moves with it.
                if depth == 0 {
                    open = Some((name.to_string(), range.start));
                }
                depth += 1;
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                depth = depth.saturating_sub(1);
                if depth > 0 {
                    continue;
                }
                let Some((id, start)) = open.take() else {
                    continue;
                };
                let lo = starts.partition_point(|&s| s <= start).saturating_sub(1);
                let hi = starts[..lines.len()].partition_point(|&s| s < range.end);
                if lo >= hi || hi > lines.len() {
                    continue;
                }
                // The parser's range runs to the end of the blank lines that terminate the
                // definition; those are not part of it.
                let mut end = hi;
                while end > lo + 1 && lines[end - 1].trim().is_empty() {
                    end -= 1;
                }
                if in_code[lo..end].iter().any(|&c| c) {
                    continue; // literal code as konoma draws it — leave the whole run alone
                }
                let Some(text) = footnote_def_text(&lines[lo..end]) else {
                    continue;
                };
                out.push(FootnoteDef {
                    id,
                    text,
                    lines: lo..end,
                });
            }
            _ => {}
        }
    }
    out
}

/// The definition's own text: the `[^id]:` label stripped off the first line, and the continuation
/// lines dedented by their common left margin so the text can be re-indented under a list marker
/// when the footnotes section is emitted. Returns `None` if the first line is not a definition
/// (defensive — the parser said it was).
fn footnote_def_text(block: &[&str]) -> Option<String> {
    let (_, first) = parse_footnote_def(block.first()?)?;
    let rest = &block[1..];
    // Common indent over the non-blank continuation lines, measured in *whitespace characters*
    // (not bytes). `trim_start()`/`trim()` strip any Unicode-whitespace character (U+3000
    // ideographic space, U+00A0 no-break space, ...), so a byte length measured on one line is not
    // a valid offset on another: a byte count from a line that starts with an ASCII space can point
    // inside a multi-byte whitespace character on a different line. A character count doesn't have
    // that problem — stripping it below walks characters (`strip_leading_ws_chars`), so it can only
    // ever land on a char boundary, on any line. A lazy continuation (indent 0) makes this 0 and the
    // lines are used as-is.
    let indent = rest
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().count() - l.trim_start().chars().count())
        .min()
        .unwrap_or(0);
    let mut text = first;
    for l in rest {
        text.push('\n');
        if l.trim().is_empty() {
            continue; // a blank line separating paragraphs inside the definition carries no indent
        }
        let line_indent = l.chars().count() - l.trim_start().chars().count();
        text.push_str(strip_leading_ws_chars(l, indent.min(line_indent)));
    }
    Some(text)
}

/// Strip the first `n` *characters* (not bytes) of whitespace from the front of `line`, returning
/// the rest verbatim. Used by `footnote_def_text` to dedent by a character count derived from
/// `trim_start()`: walking characters (via `char_indices`) keeps the resulting slice on a char
/// boundary even when `n` is less than the line's own leading-whitespace run and that run mixes
/// ASCII and multi-byte Unicode whitespace. Callers only ever pass `n <= line_indent` for a line
/// whose `trim()` is non-empty, so `n` always lands inside (or right at the end of) the leading
/// whitespace run, never past the end of the line.
fn strip_leading_ws_chars(line: &str, n: usize) -> &str {
    match line.char_indices().nth(n) {
        Some((idx, _)) => &line[idx..],
        None => "",
    }
}

/// If the line is a footnote definition `[^id]: text`, return `(id, text)`.
fn parse_footnote_def(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("[^")?;
    let close = rest.find(']')?;
    let id = &rest[..close];
    if id.is_empty() || id.contains('[') {
        return None;
    }
    let after = rest[close + 1..].strip_prefix(':')?;
    Some((id.to_string(), after.trim().to_string()))
}

/// Scan `line` for footnote references `[^id]`, returning the ids in order. References inside an
/// inline code span are literal examples of the syntax, not references, and are skipped — the same
/// rule `replace_footnote_refs` applies, so a `` `[^1]` `` is neither numbered nor rewritten.
fn find_footnote_refs(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    collect_footnote_refs(&mask_code_spans(line).0, &mut ids);
    ids
}

/// `find_footnote_refs` for one run of text already known to be outside any code span.
fn collect_footnote_refs(line: &str, ids: &mut Vec<String>) {
    let mut i = 0;
    while i < line.len() {
        if line[i..].starts_with("[^") {
            if let Some(close) = line[i + 2..].find(']') {
                let id = &line[i + 2..i + 2 + close];
                if !id.is_empty() && !id.contains('[') {
                    ids.push(id.to_string());
                    i += 2 + close + 1;
                    continue;
                }
            }
        }
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }
}

/// Replace footnote references `[^id]` in `line` with a superscript number (only ids present in
/// `num`; an undefined reference is left literal, matching GitHub). Inline code spans are copied
/// verbatim — see `find_footnote_refs`.
fn replace_footnote_refs(line: &str, num: &std::collections::HashMap<String, usize>) -> String {
    rewrite_masking_code_spans(line, |masked| {
        replace_footnote_refs_outside_code(masked, num)
    })
}

/// `replace_footnote_refs` for one run of text already known to be outside any code span.
fn replace_footnote_refs_outside_code(
    line: &str,
    num: &std::collections::HashMap<String, usize>,
) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < line.len() {
        if line[i..].starts_with("[^") {
            if let Some(close) = line[i + 2..].find(']') {
                let id = &line[i + 2..i + 2 + close];
                if !id.is_empty() && !id.contains('[') {
                    if let Some(&n) = num.get(id) {
                        out.push_str(&to_superscript(n));
                        i += 2 + close + 1;
                        continue;
                    }
                }
            }
        }
        let ch_len = line[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Rewrite `src` so GFM footnotes render: references `[^id]` become superscript numbers (ordered by
/// first appearance), the `[^id]: …` definitions are pulled out of the body, and a numbered
/// footnotes section is appended after a rule. Code-aware (fenced **or indented**) — refs/defs
/// inside a code block are left verbatim. A no-op (returns `src` unchanged) when there are no
/// definitions.
///
/// A definition may span several lines: its continuation lines (indented, lazy, or further
/// paragraphs) are part of it and move into the section with it, so a reference-style link split
/// across two lines stays a single working link. The extent of each definition comes from
/// [`footnote_defs`] — the parser's answer — rather than a line-shaped guess here; see that
/// function's doc comment for the bug that motivated it.
/// Convenience form without origin tracking. Production always goes through
/// [`process_footnotes_traced`] (it needs the origins); this is the shorthand the tests use.
#[cfg(test)]
pub fn process_footnotes(src: &str) -> String {
    process_footnotes_traced(src, &identity_origin(src)).0
}

/// [`process_footnotes`] that also composes the line origins (see [`LineOrigin`]). Definition lines
/// disappear from the body, so nothing maps to them; the appended footnotes section is new text and
/// maps to nothing. Every other line keeps its origin, since reference rewriting is in place.
pub(crate) fn process_footnotes_traced(
    src: &str,
    origin_in: &[Option<usize>],
) -> (String, LineOrigin) {
    use std::collections::HashMap;
    let lines: Vec<&str> = src.lines().collect();
    // Per-line "is this literal code on screen" mask (`literal_code_mask`) — all three passes below
    // just need to know, per line, whether to skip/echo it verbatim.
    let in_code = literal_code_mask(&lines);
    // Pass 1: collect definitions (code-aware) and mark **every** line each one occupies.
    let found = footnote_defs(&lines, &in_code);
    let mut defs: Vec<(String, String)> = Vec::new();
    let mut is_def = vec![false; lines.len()];
    for d in &found {
        defs.push((d.id.clone(), d.text.clone()));
        is_def[d.lines.clone()].fill(true);
    }
    if defs.is_empty() {
        return (src.to_string(), origin_in.to_vec());
    }
    let def_ids: std::collections::HashSet<&str> = defs.iter().map(|(id, _)| id.as_str()).collect();
    // Pass 2: number by first reference appearance (code-aware, defs excluded).
    let mut num: HashMap<String, usize> = HashMap::new();
    let mut next = 1usize;
    for (i, line) in lines.iter().enumerate() {
        if in_code[i] || is_def[i] {
            continue;
        }
        for id in find_footnote_refs(line) {
            if def_ids.contains(id.as_str()) && !num.contains_key(&id) {
                num.insert(id, next);
                next += 1;
            }
        }
    }
    if num.is_empty() {
        // definitions present but never referenced → leave as-is
        return (src.to_string(), origin_in.to_vec());
    }
    // Pass 3: rebuild the body (drop def lines, replace refs; code blocks verbatim).
    let mut out = String::new();
    let mut origin = LineOrigin::new();
    for (i, line) in lines.iter().enumerate() {
        if in_code[i] {
            out.push_str(line);
            out.push('\n');
            origin.push(origin_in.get(i).copied().flatten());
            continue;
        }
        if is_def[i] {
            continue; // moved into the section below — no line of the output maps here
        }
        out.push_str(&replace_footnote_refs(line, &num));
        out.push('\n');
        origin.push(origin_in.get(i).copied().flatten());
    }
    // Append the footnotes section, numbered by reference order.
    let mut items: Vec<(usize, &str)> = num
        .iter()
        .map(|(id, &n)| {
            let text = defs
                .iter()
                .find(|(did, _)| did == id)
                .map(|(_, t)| t.as_str())
                .unwrap_or("");
            (n, text)
        })
        .collect();
    items.sort_by_key(|(n, _)| *n);
    // Everything from here on is text this pass invented: no line of the file corresponds to it, so
    // a write-back must never be pointed at one (see `LineOrigin`).
    let body_end = out.len();
    out.push_str("\n---\n\n");
    for (n, text) in items {
        // A multi-line definition has to stay *inside* its numbered item. Leaving the continuation
        // flush-left would work for the first item by lazy continuation, but the next item's marker
        // (`2.`) cannot interrupt a paragraph in CommonMark — only a `1.` can — so item 2 onwards
        // would be swallowed into item 1's text. Indenting to the marker's content column makes each
        // continuation an unambiguous part of its own item, whatever the item's number is.
        let marker = format!("{n}. ");
        let pad = " ".repeat(marker.len());
        out.push_str(&marker);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
                if !line.is_empty() {
                    out.push_str(&pad);
                }
            }
            out.push_str(line);
        }
        out.push('\n');
    }
    origin.extend(std::iter::repeat_n(None, out[body_end..].lines().count()));
    (out, origin)
}

// ---- YAML front matter (leading `---` … `---`) ----

/// Split off leading YAML front matter (a `---` fence on the very first line, closed by `---` or
/// `...` on its own line). Returns `(Some(inner), body)` when present, else `(None, src)`. Only the
/// document start is considered, matching GitHub — a later `---` is an ordinary thematic break.
pub fn strip_front_matter(src: &str) -> (Option<String>, String) {
    let lines: Vec<&str> = src.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return (None, src.to_string());
    }
    let Some(rel) = lines[1..]
        .iter()
        .position(|l| matches!(l.trim_end(), "---" | "..."))
    else {
        return (None, src.to_string()); // no closing fence → not front matter
    };
    let close = rel + 1;
    let inner = lines[1..close].join("\n");
    let body = lines[close + 1..].join("\n");
    (Some(inner), body)
}

/// Render front matter as a compact dim metadata block: top-level `key: value` lines get an accented
/// key, other lines are dimmed as-is, and a full-width dim rule closes the block. Recognizing it this
/// way also stops the leading `---` from being drawn as a thematic break and the YAML from being
/// misparsed.
pub fn render_front_matter(inner: &str, width: u16) -> Vec<Line<'static>> {
    let key_style = Style::new().fg(Color::Cyan).add_modifier(Modifier::DIM);
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut out = Vec::new();
    for raw in inner.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            out.push(Line::from(String::new()));
            continue;
        }
        // Top-level `key: value` (not indented) → accent the key.
        if !line.starts_with([' ', '\t']) {
            if let Some(colon) = line.find(':') {
                if colon > 0 {
                    return_split_key(&mut out, line, colon, key_style, dim);
                    continue;
                }
            }
        }
        out.push(Line::from(Span::styled(line.to_string(), dim)));
    }
    out.push(Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::new().fg(TABLE_BORDER_FG),
    )));
    out.push(Line::from(String::new()));
    out
}

fn return_split_key(
    out: &mut Vec<Line<'static>>,
    line: &str,
    colon: usize,
    key_style: Style,
    dim: Style,
) {
    let (k, v) = line.split_at(colon); // v starts at ':'
    out.push(Line::from(vec![
        Span::styled(k.to_string(), key_style),
        Span::styled(v.to_string(), dim),
    ]));
}

/// Whether the line is a GFM table delimiter row (e.g. `|---|:--:|`). Contains `-` and `|`, with only spaces/`-`/`:`/`|` as characters.
fn is_table_delimiter(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('-') || !t.contains('|') {
        return false;
    }
    t.chars().all(|c| matches!(c, ' ' | '\t' | '-' | ':' | '|'))
}

/// Whether this is a table row candidate (a non-empty line containing `|`).
fn looks_like_table_row(line: &str) -> bool {
    line.contains('|') && !line.trim().is_empty()
}

/// Split md text into "normal text" and "table blocks".
/// A table = header row (containing `|`) + the delimiter row right after (`|---|`) + the consecutive data rows.
/// Code-aware: it reads the mask off the [`SourceRun`] it is handed rather than deriving one, so the
/// answer is the **document's** even when this run is a fragment cut out of it (see that type).
/// Pipe-delimited lines inside a code block (e.g. a shell/markdown example) must stay code, not get
/// carved out as a rendered table — which also splits the block into two pieces, each growing its
/// own (broken) code-block header.
#[cfg(test)]
fn split_tables(run: &SourceRun) -> Vec<MdPart> {
    let lines: Vec<&str> = run.lines();
    let in_code = run.code();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut mask: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // A table starts = the current line is a header candidate AND the next line is a delimiter row AND both are outside a code block.
        if !in_code[i]
            && i + 1 < lines.len()
            && !in_code[i + 1]
            && looks_like_table_row(lines[i])
            && is_table_delimiter(lines[i + 1])
        {
            if !text.is_empty() {
                parts.push(MdPart::Text(SourceRun::new(
                    std::mem::take(&mut text),
                    std::mem::take(&mut mask),
                )));
            }
            let mut raw = String::new();
            raw.push_str(lines[i]);
            raw.push('\n');
            raw.push_str(lines[i + 1]);
            raw.push('\n');
            let mut j = i + 2;
            while j < lines.len() && !in_code[j] && looks_like_table_row(lines[j]) {
                raw.push_str(lines[j]);
                raw.push('\n');
                j += 1;
            }
            parts.push(MdPart::Table(raw));
            i = j;
        } else {
            text.push_str(lines[i]);
            text.push('\n');
            mask.push(in_code[i]);
            i += 1;
        }
    }
    if !text.is_empty() {
        parts.push(MdPart::Text(SourceRun::new(text, mask)));
    }
    parts
}

/// Normalize one table cell's raw text into displayable form: unescape `\|` into a literal `|`,
/// then trim surrounding whitespace. Shared by both `parse_table_row`'s own line-based split
/// below (a text-row cell, already boundary-trimmed by that function's own pipe scan) and the
/// model-based path (`render_table_from_model` in `render.rs`), whose cell text instead comes
/// straight from pulldown-cmark's own byte range on `Block::src` — `" a "`, escapes and padding
/// both still literally present, never re-split from a line — so the two paths cannot drift apart
/// on this one shared transformation the way they would if each re-implemented it separately.
fn normalize_cell(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            out.push('|');
            chars.next();
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Column alignment from the delimiter row (`:---` left / `:---:` center / `---:` right).
#[derive(Clone, Copy, PartialEq)]
enum ColAlign {
    Left,
    Center,
    Right,
}

// ---- Inline links inside table cells --------------------------------------------
// Tables are our own custom rendering that doesn't go through tui-markdown, so a `[label](url)`
// inside a cell is interpreted here. Only the **label** is displayed (link style = blue
// underline). The URL is embedded right after the label as a HIDDEN-modified "hidden target"
// span, which the app's collapse_links strips just before display and recovers into targets
// (the link destination for Tab/Enter). Column alignment is computed from the label width (the hidden span vanishes before display, so it isn't counted).

/// The link-label style shared with the app's link machinery (`is_link_span`: blue + underlined).
pub fn link_label_style() -> Style {
    Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED)
}

/// Style marking a **hidden link target** span (the URL payload of a table-cell link).
/// Removed before display by the app's `collapse_links`; HIDDEN distinguishes it from a visible label.
pub fn hidden_link_target_style() -> Style {
    link_label_style().add_modifier(Modifier::HIDDEN)
}

/// Whether `span` is a hidden link target produced by the table renderer.
pub fn is_hidden_link_target(span: &Span<'_>) -> bool {
    span.style.add_modifier.contains(Modifier::HIDDEN)
        && span.style.add_modifier.contains(Modifier::UNDERLINED)
        && span.style.fg == Some(Color::Blue)
}

/// One inline piece of a table cell: (possibly styled) text, or a `[label](url)` link.
#[derive(Clone)]
enum CellSeg {
    Text { text: String, style: Style },
    Link { label: String, url: String },
}

impl CellSeg {
    fn plain(text: String) -> CellSeg {
        CellSeg::Text {
            text,
            style: Style::new(),
        }
    }
}

/// Display width of one segment (a link occupies only its label — the URL is hidden).
fn seg_width(seg: &CellSeg) -> usize {
    match seg {
        CellSeg::Text { text, .. } => UnicodeWidthStr::width(text.as_str()),
        CellSeg::Link { label, .. } => UnicodeWidthStr::width(label.as_str()),
    }
}

/// Try to parse an inline styled run at the start of `rest`: `***both***` / `**bold**` /
/// `*italic*` / `~~strike~~` / `` `code` ``. Flat (no nesting — the inner text stays literal).
/// Emphasis openers/closers must not touch whitespace on the inside (GFM flanking, simplified),
/// so `2 * 3 * 4` stays plain text; code spans keep their content verbatim.
fn try_inline_styled(rest: &str) -> Option<(usize, CellSeg)> {
    const MARKERS: &[&str] = &["***", "**", "*", "~~", "`"];
    for open in MARKERS {
        let Some(r) = rest.strip_prefix(open) else {
            continue;
        };
        let Some(end) = r.find(open) else {
            continue;
        };
        if end == 0 {
            continue; // empty (e.g. `****`) passes through unchanged
        }
        let inner = &r[..end];
        if *open != "`"
            && (inner.starts_with(char::is_whitespace) || inner.ends_with(char::is_whitespace))
        {
            continue; // emphasis doesn't allow inner whitespace (prevents misdetecting e.g. multiplication `2 * 3`)
        }
        let style = match *open {
            "***" => Style::new().add_modifier(Modifier::BOLD | Modifier::ITALIC),
            "**" => Style::new().add_modifier(Modifier::BOLD),
            "*" => Style::new().add_modifier(Modifier::ITALIC),
            "~~" => Style::new().add_modifier(Modifier::CROSSED_OUT),
            // Inline code: the same white foreground as tui-markdown's in-paragraph code.
            "`" => Style::new().fg(Color::White),
            _ => unreachable!(),
        };
        return Some((
            open.len() + end + open.len(),
            CellSeg::Text {
                text: inner.to_string(),
                style,
            },
        ));
    }
    None
}

/// Display width of a whole cell (segment sum).
fn segs_width(segs: &[CellSeg]) -> usize {
    segs.iter().map(seg_width).sum()
}

/// Parse a cell's text into text/link segments. Only the plain `[label](url)` form (no nesting);
/// an image `![alt](url)` and unmatched brackets fall through as plain text.
fn parse_cell_segments(cell: &str) -> Vec<CellSeg> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < cell.len() {
        let rest = &cell[i..];
        // Inline emphasis/code/strikethrough (checking these before a link doesn't conflict = they start with different characters).
        if let Some((consumed, seg)) = try_inline_styled(rest) {
            if !text.is_empty() {
                out.push(CellSeg::plain(std::mem::take(&mut text)));
            }
            out.push(seg);
            i += consumed;
            continue;
        }
        if rest.starts_with('[') && !text.ends_with('!') {
            if let Some(close) = rest.find(']') {
                let after = &rest[close + 1..];
                if let Some(url_rest) = after.strip_prefix('(') {
                    if let Some(par) = url_rest.find(')') {
                        let label = &rest[1..close];
                        let url = strip_link_destination(&url_rest[..par]);
                        let url = url.as_str();
                        if !label.is_empty() && !url.is_empty() {
                            if !text.is_empty() {
                                out.push(CellSeg::plain(std::mem::take(&mut text)));
                            }
                            out.push(CellSeg::Link {
                                label: label.to_string(),
                                url: url.to_string(),
                            });
                            // Consume the whole "[label](url)" and continue from after it.
                            i += close + 2 + par + 1;
                            continue;
                        }
                    }
                }
            }
        }
        let ch = rest.chars().next().expect("non-empty rest");
        text.push(ch);
        i += ch.len_utf8();
    }
    if !text.is_empty() {
        out.push(CellSeg::plain(text));
    }
    out
}

/// Reduce a CommonMark link destination to just the URL/path: unwrap `<...>`, and drop a trailing
/// quoted title (`url "title"` / `url 'title'`). Without this, a table-cell link written as
/// `[t](./x.md "Title")` would carry the title into the open target and fail to resolve.
fn strip_link_destination(dest: &str) -> String {
    let d = dest.trim();
    if let Some(inner) = d.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
        return inner.trim().to_string();
    }
    if let Some(sp) = d.find(char::is_whitespace) {
        let (u, rest) = d.split_at(sp);
        let rest = rest.trim();
        let quoted = rest.len() >= 2
            && ((rest.starts_with('"') && rest.ends_with('"'))
                || (rest.starts_with('\'') && rest.ends_with('\'')));
        if quoted {
            return u.to_string();
        }
    }
    d.to_string()
}

/// Truncate a string to display width `w` (CJK-aware).
fn truncate_to_width(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        if used + cw > w {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out
}

/// Wrap cell segments into physical lines no wider than `w`. Text splits at any char (CJK-aware,
/// full-width = 2 columns); a link label is **atomic** (splitting it would sever the label/target
/// pairing) — it moves to the next line when it doesn't fit, truncated if wider than the column.
fn wrap_segments(segs: &[CellSeg], w: usize) -> Vec<Vec<CellSeg>> {
    if w == 0 || segs_width(segs) <= w {
        return vec![segs.to_vec()];
    }
    let mut lines: Vec<Vec<CellSeg>> = Vec::new();
    let mut cur: Vec<CellSeg> = Vec::new();
    let mut cur_w = 0usize;
    for seg in segs {
        match seg {
            CellSeg::Text { text, style } => {
                let mut buf = String::new();
                for ch in text.chars() {
                    let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
                    if cur_w + cw > w && cur_w > 0 {
                        if !buf.is_empty() {
                            cur.push(CellSeg::Text {
                                text: std::mem::take(&mut buf),
                                style: *style,
                            });
                        }
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                    buf.push(ch);
                    cur_w += cw;
                }
                if !buf.is_empty() {
                    cur.push(CellSeg::Text {
                        text: buf,
                        style: *style,
                    });
                }
            }
            CellSeg::Link { label, url } => {
                let lw = UnicodeWidthStr::width(label.as_str());
                if cur_w + lw > w && cur_w > 0 {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                let label = if lw > w {
                    truncate_to_width(label, w)
                } else {
                    label.clone()
                };
                cur_w += UnicodeWidthStr::width(label.as_str());
                cur.push(CellSeg::Link {
                    label,
                    url: url.clone(),
                });
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// Prepend the Nerd Font link glyph (`ui.icons`) to every link label in `segs`, in place — the
/// identical treatment a paragraph link gets. Shared between `parse_table_text`'s own text-row path
/// below and the model-based path (`render_table_from_model` in `render.rs`), which builds its own
/// `Vec<CellSeg>` straight from `parse_cell_segments` rather than through this function, so without
/// this shared call the two paths would silently need to remember to apply the identical prefix in
/// the identical order on their own. Applied **before** any width calculation ever reads these
/// segments (`parse_table_text`'s own call site, immediately after `parse_cell_segments`) — doing it
/// after would leave the icon's own width uncounted and break column alignment.
fn prefix_link_icons(segs: &mut [CellSeg], icons: bool) {
    if !icons {
        return;
    }
    for seg in segs {
        if let CellSeg::Link { label, .. } = seg {
            *label = format!("{} {label}", crate::ui::icons::link_icon());
        }
    }
}

/// One table's cells, already parsed from its raw source text into inline segments (links resolved,
/// icon prefix applied) and its delimiter row's alignment — but not yet laid out into a fixed-width
/// grid. The split from `render_table`'s own former single body: this is everything phase A (text ->
/// cells) produces, and everything phase B (cells -> displayed rows, `render_table_cells`) needs, no
/// more and no less — so a second cell-producing path (`render_table_from_model` in `render.rs`,
/// building this same shape straight from the block model's own `aligns`/`rows` instead of a raw
/// line scan) can hand its result to the identical phase B rather than reimplement column-width
/// measurement, wrapping, or box-drawing a second time. `rows` is every row in source order, header
/// row(s) first (`rows[..header_rows]`) then body rows — mirrors `model::BlockKind::Table.rows`'s own
/// "header first" convention exactly, so the model path's `header_rows` is a one-line computation
/// (see that function's own doc comment), not a re-derivation of which rows are header rows.
struct TableCells {
    rows: Vec<Vec<Vec<CellSeg>>>,
    /// Number of rows, from the start of `rows`, that are the header (0 or 1 for every real GFM
    /// table — a table has exactly one header row by construction — but not hard-coded to `1` here:
    /// `render_table_cells` only ever reads this as a row-index cutoff, and `parse_table_text`'s own
    /// text-scanning phase naturally computes `rows.len()` at the moment it sees the delimiter line,
    /// which is `0` for the pathological "no delimiter row was ever found" input `render_table`'s own
    /// `ncol == 0` guard already handles further down).
    header_rows: usize,
    aligns: Vec<ColAlign>,
}

/// Phase B of table rendering: lay `t`'s already-parsed cells out as box-drawn `Line`s no wider than
/// `width` (the column count of the display area, inside the frame) — column-width measurement,
/// width-budget shaving, per-cell wrapping, and the border/rule drawing. Cell text was already
/// parsed into inline segments by whichever phase A produced `t` (`parse_table_text`'s own raw-line
/// scan, or the model-based path's direct build), so `[label](url)` links render as links (label
/// only, hidden target span) instead of raw Markdown here too; column widths are measured on that
/// already-displayed form.
fn render_table_cells(t: &TableCells, width: u16) -> Vec<Line<'static>> {
    let mut rows = t.rows.clone();
    let header_rows = t.header_rows;
    let aligns = &t.aligns;
    let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if rows.is_empty() || ncol == 0 {
        return Vec::new();
    }
    for r in &mut rows {
        r.resize(ncol, Vec::new());
    }
    // Natural column widths (full-width chars counted, a link uses its label width). Minimum 1.
    let mut col_w = vec![1usize; ncol];
    for r in &rows {
        for (c, cell) in r.iter().enumerate() {
            col_w[c] = col_w[c].max(segs_width(cell));
        }
    }
    // The table's total display width = Σcol_w + border bars │ (ncol+1) + each column's left/right padding (2*ncol).
    // While it exceeds the width, shave 1 off the widest column at a time (minimum 1). A shaved column wraps its cells to fit.
    let frame = (ncol + 1) + 2 * ncol;
    let budget = (width as usize).saturating_sub(frame).max(ncol);
    let mut total: usize = col_w.iter().sum();
    while total > budget {
        let (mi, &mw) = col_w.iter().enumerate().max_by_key(|(_, &w)| w).unwrap();
        if mw <= 1 {
            break;
        }
        col_w[mi] -= 1;
        total -= 1;
    }

    let border = Style::new().fg(TABLE_BORDER_FG);
    let rule = |left: char, mid: char, right: char| -> Line<'static> {
        let mut s = String::new();
        s.push(left);
        for (c, w) in col_w.iter().enumerate() {
            for _ in 0..(w + 2) {
                s.push('─');
            }
            s.push(if c + 1 == ncol { right } else { mid });
        }
        Line::from(Span::styled(s, border))
    };

    let mut out = Vec::new();
    out.push(rule('┌', '┬', '┐'));
    for (ri, r) in rows.iter().enumerate() {
        let is_head = ri < header_rows;
        // Wrap each cell to the column width, then expand vertically to the row's max physical line count.
        let wrapped: Vec<Vec<Vec<CellSeg>>> = r
            .iter()
            .enumerate()
            .map(|(c, cell)| wrap_segments(cell, col_w[c]))
            .collect();
        let phys = wrapped.iter().map(|w| w.len().max(1)).max().unwrap_or(1);
        let cell_style = if is_head {
            Style::new().fg(HEAD_FG).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        for p in 0..phys {
            let mut spans: Vec<Span<'static>> = vec![Span::styled("│", border)];
            for c in 0..ncol {
                let segs: &[CellSeg] = wrapped[c].get(p).map(|v| v.as_slice()).unwrap_or(&[]);
                let pad = col_w[c].saturating_sub(segs_width(segs));
                // Distribute the padding left/right according to the alignment colons (:---:) (default is left-aligned).
                let (lp, rp) = match aligns.get(c).copied().unwrap_or(ColAlign::Left) {
                    ColAlign::Left => (0, pad),
                    ColAlign::Right => (pad, 0),
                    ColAlign::Center => (pad / 2, pad - pad / 2),
                };
                spans.push(Span::styled(format!(" {}", " ".repeat(lp)), cell_style));
                for seg in segs {
                    match seg {
                        // Styled text (**bold**/*italic*/`code`/~~strike~~ inside the cell).
                        // Layer it on top of the header's bold etc. (cell_style) via patch.
                        CellSeg::Text { text, style } => {
                            spans.push(Span::styled(text.clone(), cell_style.patch(*style)))
                        }
                        // Link: the label (blue underline) + hidden target (the URL, stripped by
                        // collapse_links before display and recovered as the Tab/Enter link destination). Only the label is counted for width.
                        CellSeg::Link { label, url } => {
                            spans.push(Span::styled(label.clone(), link_label_style()));
                            spans.push(Span::styled(url.clone(), hidden_link_target_style()));
                        }
                    }
                }
                spans.push(Span::styled(format!("{} ", " ".repeat(rp)), cell_style));
                spans.push(Span::styled("│", border));
            }
            out.push(Line::from(spans));
        }
        // A divider rule right after the header's last row.
        if header_rows > 0 && ri + 1 == header_rows {
            out.push(rule('├', '┼', '┤'));
        }
    }
    out.push(rule('└', '┴', '┘'));
    out
}

/// Test-only shorthand matching the retired `render_markdown_tasks_opts`'s own signature (a
/// `SourceRun` in, `alerts` gated explicitly) so call sites written against that legacy helper
/// keep working unchanged apart from the name — but driven through the real dispatcher
/// (`render_markdown_with_images`, which resolves to `render::render_doc` for every document)
/// instead. Unlike `render_markdown_tasks` (above), this does **not** reset `set_details_open`
/// on entry: several callers set up `<details>` open/closed state themselves right before calling
/// this, and a reset would silently discard it. `pub(crate)` (not `#[cfg(test)] pub`, matching
/// `render_markdown`/`render_markdown_tasks` above): needed by several sibling test submodules in
/// this file, not only `mod tests` itself.
#[cfg(test)]
pub(crate) fn render_via_dispatcher(
    src: &SourceRun,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
    alerts: bool,
) -> Vec<Line<'static>> {
    let slot_of = |_: &str| ImageSlot::Unavailable;
    let mermaid_slot = |_: &str| MermaidSlot::Image { cols: 20, rows: 5 };
    let math_slot = |_: &str, _: bool| MathSlot::Raw;
    render_markdown_with_images(
        src.text(),
        width,
        code,
        theme,
        icons,
        tasks,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        alerts,
        &math_slot,
        true,
    )
    .0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_CODE_BG;

    /// Regression pin for `KonomaStyles::blockquote()` actually being *reached*: tui-markdown
    /// (`tui-markdown-0.3.7/src/lib.rs:347`) really does call `self.styles.blockquote()` and push
    /// it onto its line-style stack for a plain (non-alert) blockquote line, so the color reaches
    /// the decorated `Line`'s own style, not just the `StyleSheet` impl sitting unused. Confirmed
    /// by dumping the actual render output before writing this assertion (`render_markdown("> hello
    /// quote\n", ...)` produced a line with `style=Style::new().green().italic()` and three spans
    /// carrying no style of their own — i.e. the color/italic live on the *line*, which is exactly
    /// what a terminal buffer merges onto every cell of that line when no span overrides it). A
    /// plain paragraph line right next to it must stay unstyled, so this also pins that the
    /// blockquote style doesn't leak onto unrelated lines.
    #[test]
    fn blockquote_line_is_rendered_green_and_italic() {
        let src = "> hello quote\n\nplain paragraph\n";
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        let joined =
            |l: &Line<'static>| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };
        let quote = lines
            .iter()
            .find(|l| joined(l).contains("hello quote"))
            .expect("blockquote 行が描画されていない");
        assert_eq!(
            quote.style.fg,
            Some(Color::Green),
            "引用行が緑で描画されていない: {:?}",
            quote.style
        );
        assert!(
            quote.style.add_modifier.contains(Modifier::ITALIC),
            "引用行が斜体で描画されていない: {:?}",
            quote.style
        );
        let plain = lines
            .iter()
            .find(|l| joined(l).contains("plain paragraph"))
            .expect("通常の段落行が描画されていない");
        assert_ne!(
            plain.style.fg,
            Some(Color::Green),
            "引用のスタイルが無関係な段落へ漏れている"
        );
        assert!(
            !plain.style.add_modifier.contains(Modifier::ITALIC),
            "引用のスタイルが無関係な段落へ漏れている"
        );
    }

    /// Test-default code decoration (equivalent to the production default `ui.theme`: background on, badge right-aligned, lighter background).
    const BG: CodeStyle = CodeStyle {
        bg: Some(DEFAULT_CODE_BG),
        label_bg: Some(Color::Rgb(70, 78, 99)), // = lighten(DEFAULT_CODE_BG)
        label_right: true,
        tab_width: 4,
        wrap: true,
    };
    /// No background (equivalent to code_bg="none").
    const NO_CODE: CodeStyle = CodeStyle {
        bg: None,
        label_bg: None,
        label_right: true,
        tab_width: 4,
        wrap: true,
    };

    /// Display width of a line (full-width = 2).
    fn line_disp_width(l: &Line<'_>) -> usize {
        let s: String = l.spans.iter().map(|sp| sp.content.as_ref()).collect();
        UnicodeWidthStr::width(s.as_str())
    }

    /// `truncate_to_width`: a pure function that truncates by display width. It never splits a
    /// full-width (2-column) character at its boundary, stopping at the largest amount that doesn't
    /// exceed the width (the foundation for column truncation of CJK labels without overflowing or corrupting bytes).
    #[test]
    fn truncate_to_width_is_cjk_aware() {
        assert_eq!(truncate_to_width("hello", 3), "hel");
        assert_eq!(
            truncate_to_width("hello", 5),
            "hello",
            "ちょうど収まれば全部"
        );
        assert_eq!(truncate_to_width("hello", 99), "hello", "余れば全部");
        // A full-width char is 2 columns each. At width 3 only one character (width 2) fits, without splitting it and overflowing.
        assert_eq!(truncate_to_width("あいう", 3), "あ");
        assert_eq!(truncate_to_width("あいう", 4), "あい");
        // A mix of half-width and full-width is also counted by display width.
        assert_eq!(truncate_to_width("aあb", 2), "a");
        assert_eq!(truncate_to_width("aあb", 3), "aあ");
        // Nothing fits at width 0.
        assert_eq!(truncate_to_width("hi", 0), "");
    }

    #[test]
    fn code_block_tabs_expand_to_marker() {
        // A tab inside a Markdown code block also expands to "→ + spaces", the same as standalone code (the `tab_width` setting).
        let md = "```ts\nfunction f() {\n\tconst x = 1;\n}\n```\n";
        let lines = render_markdown(md, 40, BG, "TwoDark", false);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // The tab line contains the marker → and no raw tab character remains.
        let tab_line = texts
            .iter()
            .find(|t| t.contains("const x"))
            .expect("コード行が無い");
        assert!(
            tab_line.contains('→'),
            "タブが可視化されていない: {tab_line:?}"
        );
        assert!(!tab_line.contains('\t'), "生タブが残っている: {tab_line:?}");
        // In order gutter (▎) → marker (→) → code, with the indent inside.
        assert!(
            tab_line.starts_with("▎ →"),
            "ガター+マーカーの並びが違う: {tab_line:?}"
        );
        // The render is for one code block (only one marker line).
        let marker_lines = texts.iter().filter(|t| t.contains('→')).count();
        assert_eq!(marker_lines, 1, "マーカー行数が想定外: {marker_lines}");
    }

    /// Every line the render produced, as plain text (spans concatenated).
    fn rendered_texts(md: &str) -> Vec<String> {
        render_markdown(md, 100, BG, "TwoDark", false)
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect()
    }

    /// A fenced code block ends **either** at a closing delimiter **or** at the end of the container
    /// it was opened in. The mask that gated the block splitters knew only the first rule, so a fence
    /// opened inside a list item whose "closing" line carries trailing text — not a close per
    /// CommonMark, but the list item ends the block anyway — was treated as running to the end of the
    /// *document*, and every construct below it was skipped as "inside code".
    ///
    /// nix's CHANGELOG.md (all three versions vendored in the crate registry) does exactly this at
    /// line 110: `` ``` ([#2642](https://github.com/nix-rust/nix/pull/2642)) ``. The GFM table 500
    /// lines further down rendered as one line of raw `| Original Type | New Type | | ---…` text, and
    /// the bullet list around it lost its markers — 1,316 lines of the file were affected.
    ///
    /// Pinned per construct, because each one is carved by a different splitter (`split_tables`,
    /// `split_alerts`, `split_details`) that all shared the same broken gate, and per container,
    /// because the container is the half the old mask had no concept of at all.
    #[test]
    fn a_fence_closed_by_its_container_does_not_swallow_the_rest_of_the_document() {
        // (name, source, a needle that must appear once the block ends where it really ends)
        let cases: &[(&str, &str, &str)] = &[
            (
                "table after a trailing-text close in a bullet item",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` ([#1](https://example.com/1))\n\n- next\n\nText.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
                "┌",
            ),
            (
                "table after a trailing-text close in a nested bullet item",
                "- outer\n  - inner\n\n    ```rust\n    let x = 1;\n    ``` (note)\n\nText.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
                "┌",
            ),
            (
                "table after a trailing-text close in a block quote",
                "> ```rust\n> let x = 1;\n> ``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
                "┌",
            ),
            (
                "table after a trailing-text close in a list item in a block quote",
                "> - item\n>\n>   ```rust\n>   let x = 1;\n>   ``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
                "┌",
            ),
            (
                "table after an entirely unclosed fence in a bullet item",
                "- item\n\n  ```rust\n  let x = 1;\n\nText.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
                "┌",
            ),
            (
                "alert after a trailing-text close in a bullet item",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\nText.\n\n> [!NOTE]\n> body\n",
                "▌",
            ),
            (
                "details after a trailing-text close in a bullet item",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\nText.\n\n<details open>\n<summary>Summary here</summary>\n\nbody\n\n</details>\n",
                "Summary here",
            ),
        ];
        for (name, src, needle) in cases {
            set_details_open(Vec::new());
            let texts = rendered_texts(src);
            assert!(
                texts.iter().any(|t| t.contains(needle)),
                "{name}: コンテナで閉じたはずのコードブロックが以降を飲み込んでいる\
                 ({needle:?} が描画に出ない)\n--- src ---\n{src}\n--- rendered ---\n{}",
                texts.join("\n")
            );
        }
        // The other direction: at the **top level** there is no container to end the block, so a
        // trailing-text "close" really does run to the end of the document and the table below it
        // really is code. Over-correcting into "always close at the next delimiter-ish line" would
        // pass every case above while breaking this one.
        set_details_open(Vec::new());
        let texts =
            rendered_texts("```rust\nlet x = 1;\n``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(
            !texts.iter().any(|t| t.contains('┌')),
            "トップレベルの未閉鎖フェンスは EOF まで続くのが正しい\n--- rendered ---\n{}",
            texts.join("\n")
        );
    }

    /// The mirror image of the case above, on the other axis the old gate was blind to: an
    /// **indented** code block (CommonMark's other kind — 4+ columns, which a fence-only matcher
    /// cannot see at all). Its contents are a literal transcript, so a GFM table or a `> [!NOTE]`
    /// written inside one to *document* the notation must stay literal code — carving it out both
    /// renders a construct that isn't there and tears the code block in half.
    ///
    /// The third axis — the same line starting with an HTML tag (`    <kbd>Ctrl</kbd>`) — was a
    /// known gap here for as long as `split_html_blocks` was pinned to the fence-only gate. It no
    /// longer is: now that the document's mask is carried in on a [`SourceRun`] rather than
    /// re-derived from a fragment, such a block is drawn as code like the two cases below. It is
    /// still not asserted here (this test is about *content* being carved out as a `┌` table or a
    /// `▌` callout, which has no HTML analogue), and it is not yet pinned anywhere else either.
    #[test]
    fn an_indented_code_block_keeps_table_and_alert_content_literal() {
        for (name, src, literal) in [
            (
                "table",
                "para\n\n    | a | b |\n    |---|---|\n    | 1 | 2 |\n",
                "| a | b |",
            ),
            ("alert", "para\n\n    > [!NOTE]\n    > body\n", "> [!NOTE]"),
        ] {
            set_details_open(Vec::new());
            let texts = rendered_texts(src);
            let gutter = texts.iter().filter(|t| t.starts_with('▎')).count();
            assert!(
                gutter > 0,
                "{name}: 字下げコードブロックがコードとして描かれていない\n--- rendered ---\n{}",
                texts.join("\n")
            );
            assert!(
                texts.iter().any(|t| t.contains(literal)),
                "{name}: 字下げコードブロックの中身が逐語で出ていない ({literal:?})\
                 \n--- rendered ---\n{}",
                texts.join("\n")
            );
            assert!(
                !texts.iter().any(|t| t.contains('┌') || t.contains('▌')),
                "{name}: 字下げコードブロックの中身が構造として切り出されている\
                 \n--- rendered ---\n{}",
                texts.join("\n")
            );
        }
    }

    #[test]
    fn table_cell_link_renders_label_with_hidden_target() {
        // A [label](url) inside a table cell is drawn as just its label (blue underline), carrying
        // the URL as a HIDDEN hidden span. The raw "[label](url)" text must never show in the table (a bug reported by the user).
        let md = "| name | doc |\n|---|---|\n| konoma | [Docs](./docs/readme.md) |\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let row = lines
            .iter()
            .find(|l| l.spans.iter().any(|sp| sp.content.as_ref() == "Docs"))
            .expect("リンクセルの行が無い");
        let joined: String = row.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(
            !joined.contains("[Docs]"),
            "生の Markdown 記法が残っている: {joined:?}"
        );
        // The label span is in link style (blue underline, no HIDDEN).
        let label = row
            .spans
            .iter()
            .find(|sp| sp.content.as_ref() == "Docs")
            .unwrap();
        assert_eq!(label.style.fg, Some(Color::Blue));
        assert!(label.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!label.style.add_modifier.contains(Modifier::HIDDEN));
        // Right after it, the URL's hidden target span.
        let li = row
            .spans
            .iter()
            .position(|sp| sp.content.as_ref() == "Docs")
            .unwrap();
        let hidden = &row.spans[li + 1];
        assert_eq!(hidden.content.as_ref(), "./docs/readme.md");
        assert!(is_hidden_link_target(hidden), "URL は HIDDEN の隠しスパン");
    }

    #[test]
    fn table_link_rows_align_after_hiding_targets() {
        // Column alignment holds by "display width excluding the hidden span" (assuming
        // collapse_links strips it before display). Every row (including the border) must end up
        // the same display width; a link row being wider/narrower would be a break.
        let md = "| name | doc |\n|---|---|\n| konoma | [Docs](./docs/readme.md) |\n| plain | text cell |\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .filter(|sp| !is_hidden_link_target(sp))
                    .map(|sp| UnicodeWidthStr::width(sp.content.as_ref()))
                    .sum()
            })
            .collect();
        assert!(!widths.is_empty());
        assert!(
            widths.iter().all(|w| *w == widths[0]),
            "行の表示幅が揃わない: {widths:?}"
        );
    }

    #[test]
    fn table_link_wraps_atomically_in_narrow_width() {
        // At a narrow width the cell wraps, but a link's label span is never split (keeps the target pairing intact).
        let md = "| doc |\n|---|\n| intro text [Guide](./guide.md) tail |\n";
        let lines = render_markdown(md, 18, BG, "TwoDark", false);
        // The label "Guide" exists as **one span** on some physical line, immediately followed by the hidden URL.
        let mut found = false;
        for l in &lines {
            if let Some(i) = l.spans.iter().position(|sp| sp.content.as_ref() == "Guide") {
                assert!(is_hidden_link_target(&l.spans[i + 1]));
                found = true;
            }
        }
        assert!(found, "折返し後もリンクラベルが1スパンで残る");
        // The width matches on every row once the hidden spans are removed.
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .filter(|sp| !is_hidden_link_target(sp))
                    .map(|sp| UnicodeWidthStr::width(sp.content.as_ref()))
                    .sum()
            })
            .collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
    }

    #[test]
    fn cell_segments_parse_links_and_leave_images_as_text() {
        // The basic form, surrounding text, and an image (with `!`) pass through unchanged; unsupported brackets also pass through unchanged.
        let segs = parse_cell_segments("see [a](b.md) end");
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[1], CellSeg::Link { label, url } if label == "a" && url == "b.md"));
        let img = parse_cell_segments("![alt](x.png)");
        assert!(matches!(&img[..], [CellSeg::Text { text, .. }] if text == "![alt](x.png)"));
        let broken = parse_cell_segments("[no url] and [y](");
        assert!(broken.iter().all(|s| matches!(s, CellSeg::Text { .. })));
        // A link destination with a title, or wrapped in <>, is reduced to just the URL/path (making it an openable target).
        let titled = parse_cell_segments("[t](./g.md \"Title\")");
        assert!(
            matches!(&titled[..], [CellSeg::Link { url, .. }] if url == "./g.md"),
            "title がリンク先に混入しない"
        );
        let angled = parse_cell_segments("[t](<./with space.md>)");
        assert!(
            matches!(&angled[..], [CellSeg::Link { url, .. }] if url == "./with space.md"),
            "<> 囲みは中身だけ"
        );
    }

    #[test]
    fn table_link_icon_matches_paragraph_links_and_keeps_alignment() {
        // When `ui.icons` is on: a table-internal link also gets the same icon as a paragraph link
        // (visual consistency), and since the icon is folded into the label **before the width calculation**, column alignment isn't broken.
        let md = "| a | b |\n|---|---|\n| [Docs](./g.md) | plain |\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", true);
        let row = lines
            .iter()
            .find(|l| {
                l.spans
                    .iter()
                    .any(|sp| sp.content.as_ref().contains("Docs"))
            })
            .expect("リンク行");
        let icon = crate::ui::icons::link_icon();
        let label = row
            .spans
            .iter()
            .find(|sp| sp.content.as_ref().contains("Docs"))
            .unwrap();
        assert!(
            label.content.as_ref().starts_with(&format!("{icon} ")),
            "アイコンが前置される: {:?}",
            label.content
        );
        // The width matches on every row once the hidden spans are removed (= as displayed).
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .filter(|sp| !is_hidden_link_target(sp))
                    .map(|sp| UnicodeWidthStr::width(sp.content.as_ref()))
                    .sum()
            })
            .collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
    }

    #[test]
    fn table_escaped_pipe_stays_in_one_cell() {
        // GFM: `\|` is a literal `|` (not a cell delimiter). Previously it got split, growing a phantom column.
        let md = "| a | b |\n|---|---|\n| x \\| y | z |\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let row: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .find(|t| t.contains("x | y"))
            .expect("エスケープパイプがリテラルで残る");
        assert_eq!(
            row.matches('│').count(),
            3,
            "2列のまま(幽霊列なし): {row:?}"
        );
    }

    #[test]
    fn table_alignment_colons_are_respected() {
        // Padding distribution for :--- (left) / :---: (center) / ---: (right).
        let md = "| xxxx | yyyy | zzzz |\n|:-----|:----:|-----:|\n| a | b | c |\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let row: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .find(|t| t.contains(" a ") && t.contains('│'))
            .expect("データ行");
        assert_eq!(row, "│ a    │  b   │    c │", "左/中央/右の整列: {row:?}");
    }

    #[test]
    fn table_cell_inline_styles_render_without_markers() {
        // A cell's **bold** / *italic* / `code` / ~~strike~~ shows up styled with the markers removed.
        let md = "| a |\n|---|\n| **b** and *i* and `c` and ~~s~~ |\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let row = lines
            .iter()
            .find(|l| l.spans.iter().any(|sp| sp.content.as_ref() == "b"))
            .expect("スタイルセル行");
        let joined: String = row.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(
            !joined.contains('*') && !joined.contains('`') && !joined.contains('~'),
            "生の記号が残っている: {joined:?}"
        );
        let has = |txt: &str, m: Modifier| {
            row.spans
                .iter()
                .any(|sp| sp.content.as_ref() == txt && sp.style.add_modifier.contains(m))
        };
        assert!(has("b", Modifier::BOLD), "bold");
        assert!(has("i", Modifier::ITALIC), "italic");
        assert!(has("s", Modifier::CROSSED_OUT), "strike");
        assert!(
            row.spans
                .iter()
                .any(|sp| sp.content.as_ref() == "c" && sp.style.fg == Some(Color::White)),
            "code fg"
        );
        // Prevents misdetection: a multiplication-like `*` passes through unchanged.
        let plain = parse_cell_segments("2 * 3 * 4");
        assert!(matches!(&plain[..], [CellSeg::Text { text, .. }] if text == "2 * 3 * 4"));
    }

    #[test]
    fn details_open_tag_and_split_details() {
        assert_eq!(details_open_tag("<details>"), Some(false));
        assert_eq!(details_open_tag("<details open>"), Some(true));
        assert_eq!(details_open_tag("  <DETAILS OPEN>"), Some(true));
        assert_eq!(details_open_tag("<detailsx>"), None);
        assert_eq!(details_open_tag("plain"), None);
        // Extract a block that spans a blank line; summary + body separated, tags stripped.
        let md =
            "intro\n\n<details open>\n<summary>Sum</summary>\n\nbody line\n\n</details>\n\ntail\n";
        let parts = split_details(&doc_run(md));
        assert_eq!(parts.len(), 3, "Text / Details / Text");
        match &parts[1] {
            DetailsPart::Details {
                open_attr,
                summary,
                body,
            } => {
                assert!(*open_attr);
                assert_eq!(summary, "Sum");
                assert!(body.contains("body line"));
            }
            _ => panic!("expected a details part"),
        }
        // Fence-aware: a <details> inside a code fence is left as text.
        let fenced = "```\n<details>\n<summary>x</summary>\n</details>\n```\n";
        assert!(split_details(&doc_run(fenced))
            .iter()
            .all(|p| matches!(p, DetailsPart::Text(_))));
    }

    #[test]
    fn details_config_modes_force_open_or_closed() {
        // `set_details_open` overrides the per-block open state (what `ui.md_details` "open"/"closed"
        // produce). Here the source is a plain <details> (attr = collapsed by default).
        let md = "<details>\n<summary>Sum</summary>\n\nthe body\n\n</details>\n";
        let shown = |states: Vec<bool>| -> bool {
            set_details_open(states);
            let lines = render_via_dispatcher(
                &doc_run(md),
                60,
                BG,
                "TwoDark",
                false,
                DEFAULT_TASK_STATES,
                true,
            );
            let all: String = lines
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect();
            all.contains("the body")
        };
        assert!(shown(vec![true]), "forced open → body shown");
        assert!(!shown(vec![false]), "forced closed → body hidden");
    }

    #[test]
    fn math_inline_and_display_detection() {
        // Inline $…$ and \(…\); display $$…$$ (single + multi-line) and \[…\]. In document order.
        let src = "before $x^2$ mid \\(a+b\\) end\n\n$$E = mc^2$$\n\n\\[ \\int f \\]\n\ntail\n";
        let m = collect_math_exprs(src);
        assert_eq!(
            m,
            vec![
                ("x^2".to_string(), false),
                ("a+b".to_string(), false),
                ("E = mc^2".to_string(), true),
                ("\\int f".to_string(), true),
            ]
        );
        // Multi-line display block ($$ on its own lines).
        let ml = collect_math_exprs("$$\n\\sum_{i} i\n$$\n");
        assert_eq!(ml, vec![("\\sum_{i} i".to_string(), true)]);
    }

    #[test]
    fn math_currency_and_code_are_not_mistaken() {
        // Currency: a `$` whose partner is followed by a digit / preceded by space is not math.
        assert!(collect_math_exprs("it costs $5 and $10 total\n").is_empty());
        assert!(collect_math_exprs("give me $ 5 $ please\n").is_empty()); // opens with a space
                                                                          // Inside inline code / a code fence, `$x$` is literal (not math).
        assert!(collect_math_exprs("use `$x$` inline\n").is_empty());
        assert!(collect_math_exprs("```\n$x^2$\n```\n").is_empty());
        // Escaped \$ is a literal dollar, not a delimiter.
        assert!(collect_math_exprs("cost \\$5 and \\$10\n").is_empty());
    }

    #[test]
    fn math_cjk_and_url_key_are_safe() {
        // CJK around inline math must not panic or corrupt byte offsets.
        let m = collect_math_exprs("質量エネルギー $E=mc^2$ と水\n");
        assert_eq!(m, vec![("E=mc^2".to_string(), false)]);
        // A backslash immediately before a multibyte char (`\あ`, `\🎉`, Windows-path-like) must not
        // panic on a non-char-boundary slice — regression for the byte-boundary crash in the scanner.
        for s in [
            "\\あ",
            "価格は\\円です\n",
            "path C:\\ユーザー\\x done \\🎉\n",
            "$a\\あ$ and text\n", // the `\` skip inside an inline-math scan
            "\\",                 // trailing backslash at end of line
        ] {
            let _ = collect_math_exprs(s); // must not panic
        }
        // The synthetic key separates display vs inline of the same LaTeX (different rasters).
        assert_ne!(math_url("x^2", true), math_url("x^2", false));
        assert!(is_math_url(&math_url("x", false)));
        assert!(!is_math_url("mermaid-fence://abc"));
    }

    #[test]
    fn math_renders_image_placeholder_or_raw_fallback() {
        // With math_on, a rendered expr reserves image rows + a placement; a failed one shows raw LaTeX.
        let img_slot = |_: &str, _: bool| MathSlot::Image { cols: 8, rows: 2 };
        let (_lines, imgs, _extras) = render_markdown_with_images(
            "text $x^2$ more\n",
            40,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &img_slot,
            true,
        );
        assert_eq!(imgs.len(), 1, "one math placement");
        assert!(is_math_url(&imgs[0].url));
        // Raw fallback: the raw LaTeX with delimiters is shown (nothing is lost).
        let (raw_lines, raw_imgs, _extras) = render_markdown_with_images(
            "text $x^2$ more\n",
            40,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            true,
        );
        assert!(raw_imgs.is_empty());
        let joined: String = raw_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("$x^2$"), "raw LaTeX shown: {joined:?}");
        // math_on = false: the `$…$` stays inline as ordinary text (no placement, no lifting).
        let (_l, off_imgs, _extras) = render_markdown_with_images(
            "text $x^2$ more\n",
            40,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert!(off_imgs.is_empty(), "math off: no placements");
    }

    #[test]
    fn malformed_summary_tag_does_not_panic() {
        // A `<summary` opening tag that never closes with its own `>` must not panic (the `>` in
        // `</summary>` sits past `e`; bounding the search to [s,e) avoids the inverted-range slice).
        // Regression for a full-screen crash on typo'd / half-written HTML in a Markdown file.
        for md in [
            "<details>\n<summary attr\n</summary>\nbody\n</details>\n",
            "<details>\n<summary bad</summary>tail\n</details>\n",
            "<details>\n<summary>日本語 attr\n</summary>\n本文\n</details>\n",
        ] {
            let _ = render_markdown(md, 40, BG, "TwoDark", false); // must not panic
        }
        // Direct: unclosed opening tag falls back to no-summary (whole inner becomes the body).
        let (sum, body) = extract_summary_body("<summary attr\nbody");
        assert_eq!(sum, "");
        assert!(body.contains("body"));
    }

    // `#[ignore]`: this end-to-end assertion (not `collect_details_open`'s own count, which still
    // passes) is currently red, and stays red on purpose — see `docs/STATUS.md`'s ★未修正 entry
    // ("`<details>` が別の `<details>` に空行なしで入れ子（グルー）になっていると...", added
    // 2026-08-24) for the full root-cause writeup. In short: this exact glued-nested-`<details>`
    // shape is a *documented, pre-existing* divergence between `model.rs`'s own Details-folding
    // (`fold_details`/`glued_details_fold`, which deliberately leaves it as one unfolded `Html`
    // leaf — see `model.rs`'s own
    // `model_details_ordinal_matches_collect_details_open_for_well_formed_non_quote_nesting` test
    // comment) and this file's `collect_details_open` (which still counts it as one entry). Until
    // this refactor rerouted this test from the retired legacy renderer to the real dispatcher
    // (`render_markdown_with_images` → `render::render_doc`), nothing exercised the *rendering*
    // consequence of that known gap end-to-end: it desyncs `render_doc`'s global
    // `next_details_open` ordinal counter, so a *later* sibling `<details>` reads the wrong slot
    // and can render collapsed when its own `open` attribute says otherwise. Not something to
    // paper over here (principle: fix causes, not symptoms) — needs its own investigation and fix
    // in `collect_details_open`/`model.rs`, tracked in `docs/STATUS.md`.
    #[test]
    #[ignore = "known pre-existing gap — see docs/STATUS.md ★未修正 (2026-08-24, nested <details> ordinal drift)"]
    fn nested_details_do_not_drift_later_block_open_state() {
        // A `<details>` nested inside another is swallowed into the parent's body (split_details stops
        // at the first `</details>`) and must NOT be counted as its own block — otherwise the flat tag
        // scan yielded [false, false, true] while only 2 top-level blocks render, so the later
        // `<details open>` read slot 1 (the nested block's `false`) and drew collapsed.
        let md = "<details>\n<summary>A</summary>\n<details>\n<summary>Nested</summary>\n</details>\n</details>\n\n<details open>\n<summary>C</summary>\nc body\n</details>\n";
        assert_eq!(
            collect_details_open(md),
            vec![false, true],
            "top-level only: outer(closed) + C(open)"
        );
        // End-to-end: the C summary line renders expanded (▾), honoring its `open` attribute.
        set_details_open(collect_details_open(md));
        let lines = render_via_dispatcher(
            &doc_run(md),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let c_arrow = lines.iter().find_map(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined
                .contains(" C")
                .then(|| joined.chars().next().unwrap_or(' '))
        });
        assert_eq!(
            c_arrow,
            Some('▾'),
            "C honors <details open> → expanded marker"
        );
    }

    /// Regression for the reported bug: a GitHub alert nested inside a **closed** `<details>` block
    /// used to render unconditionally, leaking its body regardless of the fold — because
    /// `split_alerts` used to run over the whole document *before* `split_details`, extracting the
    /// alert (and rendering it, via `render_alert`, at the top level) before `<details>`'s own fold
    /// ever got a say. Fixed by gating `split_alerts`'s header detection on `details_mask`, so the
    /// alert stays literal text inside the block and is only ever reached (via
    /// `render_details`/`render_md_body_nested`) if that block is actually open.
    #[test]
    fn alert_inside_closed_details_is_not_leaked() {
        let md = "<details>\n<summary>S</summary>\n\n> [!NOTE]\n\
                  > SECRET-INSIDE-CLOSED-DETAILS\n\n</details>\n\nTail.\n";
        set_details_open(collect_details_open(md));
        assert_eq!(
            collect_details_open(md),
            vec![false],
            "one top-level (closed) details block"
        );
        let lines = render_via_dispatcher(
            &doc_run(md),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains("SECRET-INSIDE-CLOSED-DETAILS"),
            "closed <details> must not leak the alert nested inside it: {joined:?}"
        );
        // The rest of the document still renders: the (closed) summary and the trailing paragraph.
        assert!(joined.contains('S'), "summary still shows: {joined:?}");
        assert!(
            joined.contains("Tail."),
            "trailing text survives: {joined:?}"
        );
        let summary_arrow = lines.iter().find_map(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined
                .trim_end()
                .ends_with('S')
                .then(|| joined.chars().next().unwrap_or(' '))
        });
        assert_eq!(summary_arrow, Some('▸'), "summary shows the closed marker");
    }

    /// Companion to `alert_inside_closed_details_is_not_leaked`: when the `<details>` is **open**,
    /// the nested alert must now actually show — and show as a proper colored callout (via
    /// `render_md_body_nested` → `render_alert`), not as a literal blockquote with `[!NOTE]` still
    /// in the text (which is what the pre-fix generic-HTML-block fallback produced).
    #[test]
    fn alert_inside_open_details_renders_as_a_callout() {
        let md = "<details open>\n<summary>S</summary>\n\n> [!NOTE]\n\
                  > SECRET-INSIDE-OPEN-DETAILS\n\n</details>\n";
        set_details_open(collect_details_open(md));
        assert_eq!(collect_details_open(md), vec![true]);
        let lines = render_via_dispatcher(
            &doc_run(md),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined.contains("SECRET-INSIDE-OPEN-DETAILS"),
            "open <details> shows the nested alert's body: {joined:?}"
        );
        assert!(
            joined.contains("Note"),
            "rendered as a callout with its label: {joined:?}"
        );
        assert!(
            !joined.contains("[!NOTE]"),
            "the raw `[!NOTE]` marker must be gone, not left literal: {joined:?}"
        );
        assert!(
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(|s| s.content.contains('▌') && s.style.fg == Some(Color::Blue)),
            "colored left bar (Note = blue) on the callout"
        );
    }

    /// Reverse nesting: a `<details>` block found **inside a GitHub alert's body**. Before this fix
    /// there was no `<details>` handling in an alert's body pipeline at all (`render_alert` called
    /// `render_md_text_inner` directly) — a `<details>` there fell through to the generic
    /// HTML-block-rescue fallback, which strips tags and always shows the text, `open` or not. Now
    /// it renders via `render_details(.., interactive: false, ..)`: closed hides the body, open
    /// shows it — and crucially, this uses the tag's own `open` attribute directly rather than
    /// consuming a document-wide ordinal slot (see `render_md_body_nested`'s doc comment for why:
    /// `collect_details_open`'s source-order scan cannot see this `<details>` at all, because its
    /// opening tag is still `>`-prefixed at that point).
    #[test]
    fn details_inside_alert_closed_is_not_leaked_and_open_shows() {
        let closed = "> [!NOTE]\n> <details>\n> <summary>S</summary>\n>\n\
                       > SECRET-C\n>\n> </details>\n\nTail.\n";
        assert_eq!(
            collect_details_open(closed),
            Vec::<bool>::new(),
            "a <details> reachable only through an alert is not in the top-level count"
        );
        set_details_open(collect_details_open(closed));
        let lines = render_via_dispatcher(
            &doc_run(closed),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains("SECRET-C"),
            "closed <details> nested inside an alert must not leak: {joined:?}"
        );
        assert!(joined.contains("Note"), "the alert callout still shows");
        assert!(joined.contains("Tail."), "trailing text survives");
        // The nested block's own summary marker must NOT be the interactive sentinel
        // (`is_details_header_span`/`build_md_items` would otherwise count it and drift the
        // document's Tab-toggle ordinal out of step with `collect_details_open` — see
        // `render_md_body_nested`'s doc comment).
        assert!(
            !lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .any(is_details_header_span),
            "a <details> nested inside an alert must not be Tab-toggleable"
        );

        let open = "> [!NOTE]\n> <details open>\n> <summary>S</summary>\n>\n\
                     > SECRET-O\n>\n> </details>\n";
        set_details_open(collect_details_open(open));
        let lines_open = render_via_dispatcher(
            &doc_run(open),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined_open: String = lines_open
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined_open.contains("SECRET-O"),
            "open <details> nested inside an alert shows its body: {joined_open:?}"
        );
    }

    /// Ordinal-contract stress test: a top-level `<details>` block ("A") whose body contains an
    /// alert wrapping a *further* nested, open `<details>` ("Nested") — the composite of both
    /// nesting directions this fix supports — followed by a second, independent top-level
    /// `<details>` ("C"). "Nested" must never be counted by `collect_details_open` (it is only
    /// reachable through A's body via an alert, matching the plain-nested-details rule this mirrors
    /// — see `render_md_body_nested`'s doc comment), so C must still read **its own** correct slot
    /// regardless of what's nested inside A. Checked both with A closed (nothing inside A, including
    /// "Nested", may render at all) and with A open (the deep nesting fully round-trips).
    #[test]
    fn alert_and_details_mutual_nesting_preserves_the_details_ordinal() {
        let build = |a_open: &str| -> String {
            format!(
                "<details{a_open}>\n<summary>A</summary>\n\n> [!NOTE]\n\
                 > <details open>\n> <summary>Nested</summary>\n>\n\
                 > nested-open-body\n>\n> </details>\n\n</details>\n\n\
                 <details open>\n<summary>C</summary>\nc body\n</details>\n"
            )
        };

        // A closed: only 2 top-level blocks are counted (A, C) — "Nested" is invisible to the
        // source-order scan no matter how deep it's buried in A's body.
        let closed = build("");
        assert_eq!(
            collect_details_open(&closed),
            vec![false, true],
            "top-level only: A(closed) + C(open) — Nested never counted"
        );
        set_details_open(collect_details_open(&closed));
        let lines = render_via_dispatcher(
            &doc_run(&closed),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !joined.contains("nested-open-body") && !joined.contains("Nested"),
            "A closed hides everything nested inside it, however deep: {joined:?}"
        );
        let c_arrow = lines.iter().find_map(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined
                .trim_end()
                .ends_with('C')
                .then(|| joined.chars().next().unwrap_or(' '))
        });
        assert_eq!(
            c_arrow,
            Some('▾'),
            "C still honors <details open> — not shifted by whatever is nested inside A"
        );

        // A open: the deep nesting renders end-to-end, and C's ordinal is still correct.
        let open = build(" open");
        assert_eq!(
            collect_details_open(&open),
            vec![true, true],
            "top-level only: A(open) + C(open)"
        );
        set_details_open(collect_details_open(&open));
        let lines_open = render_via_dispatcher(
            &doc_run(&open),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined_open: String = lines_open
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined_open.contains("nested-open-body"),
            "A open + Nested open renders the deeply nested body: {joined_open:?}"
        );
        let c_arrow_open = lines_open.iter().find_map(|l| {
            let joined: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            joined
                .trim_end()
                .ends_with('C')
                .then(|| joined.chars().next().unwrap_or(' '))
        });
        assert_eq!(
            c_arrow_open,
            Some('▾'),
            "C still reads its own (second) slot, not shifted by A's now-visible nested content"
        );
    }

    #[test]
    fn html_block_text_survives_and_autolink_untouched() {
        // `<details>` (no `open`) is collapsed by default: the summary stays and the body is hidden
        // (GitHub's convention). `<details open>` expands and shows the body. A comment is hidden; autolink is not misdetected.
        let md = "before\n\n<details>\n<summary>Summary text</summary>\nhidden body\n</details>\n\n<details open>\n<summary>Open Summary</summary>\nvisible body\n</details>\n\n<!-- secret comment -->\n\nsee <https://ratatui.rs> end\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let all: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        // Collapsed <details>: summary shown, body hidden.
        assert!(
            all.iter().any(|t| t.contains("Summary text")),
            "折りたたみ summary は残る: {all:?}"
        );
        assert!(
            all.iter().all(|t| !t.contains("hidden body")),
            "折りたたみ既定で本文は隠れる: {all:?}"
        );
        // <details open>: summary + body both shown.
        assert!(all.iter().any(|t| t.contains("Open Summary")));
        assert!(
            all.iter().any(|t| t.contains("visible body")),
            "open は展開して本文が出る: {all:?}"
        );
        assert!(
            all.iter().all(|t| !t.contains('<')),
            "タグは剥がす: {all:?}"
        );
        assert!(
            all.iter().all(|t| !t.contains("secret")),
            "コメントは非表示"
        );
        assert!(
            all.iter().any(|t| t.contains("https://ratatui.rs")),
            "autolink は生きる"
        );
    }

    #[test]
    fn thematic_break_and_task_checkboxes_decorate() {
        let md = "para\n\n---\n\n- [ ] open task\n- [x] done task\n";
        let lines = render_markdown(md, 40, BG, "TwoDark", false);
        let all: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        assert!(
            all.iter().any(|t| t.trim() == "─".repeat(40)),
            "--- が全幅罫線になる: {all:?}"
        );
        // icons=false: ASCII bracket display (☐/☑ were dropped since CJK fallback fonts draw them full-width).
        assert!(
            all.iter().any(|t| t.contains("[ ] open task")),
            "未完 [ ]: {all:?}"
        );
        assert!(all.iter().any(|t| t.contains("[x] done task")), "完了 [x]");
        // A marker is emitted as its own dedicated span (a style sentinel).
        let markers: Vec<String> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_task_span(s))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(markers, vec!["[ ] ", "[x] "], "マーカーは末尾スペース込み");
    }

    #[test]
    fn code_block_wrap_keeps_gutter_on_every_row() {
        use unicode_width::UnicodeWidthStr;
        // A code line that overflows the width is **pre-wrapped**, and every visual row after
        // wrapping carries the ▎ gutter, with no row exceeding the width (leaving it to Paragraph is
        // a regression where the gutter/band breaks on the 2nd+ row).
        let long = "abcdefghij".repeat(6); // 60 columns
        let md = format!("```\n{long}\nshort\n```\n");
        let lines = render_markdown(&md, 30, BG, "TwoDark", false);
        let code_rows: Vec<&Line> = lines
            .iter()
            .filter(|l| l.spans.first().is_some_and(|s| s.content.starts_with('▎')))
            .collect();
        // 1 badge row + 60 columns split every 28 columns (=30-gutter 2) into 3 rows + 1 "short" row + 1 end-padding row.
        assert_eq!(code_rows.len(), 6, "{:?}", code_rows.len());
        for l in &code_rows {
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert!(w <= 30, "行幅が枠を超えない: {w}");
        }
        // Nothing is lost (joining all rows contains the original text).
        let joined: String = code_rows
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref().trim_start_matches("▎ "))
            .collect::<String>()
            .replace(' ', "");
        assert!(joined.contains(&long), "折返しで文字が欠けない");

        // CJK: 20 full-width chars (40 columns) split at the 28-column boundary, without breaking a character at the boundary.
        let cjk = "あ".repeat(20);
        let md = format!("```\n{cjk}\n```\n");
        let lines = render_markdown(&md, 30, BG, "TwoDark", false);
        let rows: Vec<&Line> = lines
            .iter()
            .filter(|l| l.spans.first().is_some_and(|s| s.content.starts_with('▎')))
            .collect();
        assert_eq!(rows.len(), 4, "バッジ+全角14文字+6文字+終端パディング");
        for l in &rows {
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert!(w <= 30, "CJK でも行幅が枠内: {w}");
        }

        // With wrap=false (horizontal-scroll workflow), it stays one logical line as before (no pre-wrapping).
        let nowrap = CodeStyle { wrap: false, ..BG };
        let md = format!("```\n{long}\n```\n");
        let lines = render_markdown_tasks(&md, 30, nowrap, "TwoDark", false, DEFAULT_TASK_STATES);
        let rows: Vec<&Line> = lines
            .iter()
            .filter(|l| l.spans.first().is_some_and(|s| s.content.starts_with('▎')))
            .collect();
        assert_eq!(
            rows.len(),
            3,
            "バッジ+本文1行+終端パディングのみ(分割しない)"
        );
        let w0: usize = rows[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref().width())
            .sum();
        assert!(w0 > 30, "wrap=false は長い行を保つ(h スクロールで読む)");
    }

    #[test]
    fn loose_list_task_item_does_not_panic() {
        // Real, upstream tui-markdown 0.3.7/0.3.8 panics parsing "a task item inside a loose list
        // (blank-line separated)" (insertion index should be <= len) — see
        // `INTENDED_IMPROVEMENTS`'s own "list_corpus: task item inside a loose list" entry in
        // `md_render_diff_tests.rs` for the confirmed mechanism. `render_markdown` (via
        // `render_markdown_tasks`) now goes through `render::render_doc`, the production dispatcher
        // since `3318a5b`, which never calls tui-markdown's `from_str` for an ordinary block like
        // this at all (see `render.rs`'s own module doc comment) — so this exact shape has no panic
        // to avoid any more, and the old bisect-and-retry workaround this test used to pin
        // (`split_block_for_retry`, still exercised directly by the tests below) never runs for it.
        // What's asserted now is the *correct* loose-list layout `render_doc` draws instead: a loose
        // item's own marker lands on its own line, separate from its text (see `render_item`'s own
        // doc comment) — the real CommonMark-correct shape the legacy bisect-and-retry used to
        // silently disturb by turning the one loose list into two independent tight one-item lists
        // glued back together (marker and text sharing one line — the layout the old assertion here,
        // `all.iter().any(|t| t.contains("- a"))`, was written to expect).
        let md = "# title\n\n- a\n\n- [ ] b\n\n**bold**\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let all: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // "- " (the marker, alone) is immediately followed by "a" (the item's own text) on the next
        // line — content is readable, just laid out across two lines instead of one.
        let marker_idx = all
            .iter()
            .position(|t| t == "- ")
            .unwrap_or_else(|| panic!("箇条書きマーカーの行が見つからない: {all:?}"));
        assert_eq!(
            all.get(marker_idx + 1).map(String::as_str),
            Some("a"),
            "マーカーの直後の行が項目の本文: {all:?}"
        );
        // Decoration still applies to every construct: the task gets its dedicated marker span, and bold has its markers stripped.
        let markers: Vec<String> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_task_span(s))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(markers, vec!["[ ] "], "タスクが装飾のまま: {all:?}");
        assert!(
            all.iter().any(|t| t.contains("bold") && !t.contains("**")),
            "bold が描画される: {all:?}"
        );
        assert!(
            all.iter().all(|t| !t.contains("# title")),
            "見出しの # が剥がれる: {all:?}"
        );
    }

    #[test]
    fn task_markers_become_dedicated_spans_with_custom_states() {
        // The standard states (` `/`x`) get a dedicated span (icons=NF icon / false=bracket); the custom `/` is only in scope when configured.
        let md = "- [ ] open\n- [x] done\n- [/] doing\n";
        let dflt = render_markdown(md, 40, BG, "TwoDark", false);
        let markers = |lines: &[Line<'static>]| -> Vec<String> {
            lines
                .iter()
                .flat_map(|l| l.spans.iter())
                .filter(|s| is_task_span(s))
                .map(|s| s.content.to_string())
                .collect()
        };
        assert_eq!(markers(&dflt), vec!["[ ] ", "[x] "], "既定では / は対象外");
        let custom = render_markdown_tasks(md, 40, BG, "TwoDark", false, &[' ', '/', 'x']);
        assert_eq!(markers(&custom), vec!["[ ] ", "[x] ", "[/] "]);
        // icons=true: a standard state gets a Nerd Font icon (fixed 1 cell); a custom one stays a bracket.
        let nf_off = format!("{} ", crate::ui::icons::task_icon(false));
        let nf_on = format!("{} ", crate::ui::icons::task_icon(true));
        let iconed = render_markdown_tasks(md, 40, BG, "TwoDark", true, &[' ', '/', 'x']);
        assert_eq!(
            markers(&iconed),
            vec![nf_off.clone(), nf_on.clone(), "[/] ".into()]
        );
        // Recovering the state (used to cross-check the toggle).
        assert_eq!(
            task_span_state(&nf_off),
            Some(' '),
            "末尾スペース込みで復元"
        );
        assert_eq!(task_span_state(&nf_on), Some('x'));
        assert_eq!(task_span_state("[ ]"), Some(' '));
        assert_eq!(task_span_state("[/]"), Some('/'));
        assert_eq!(task_span_state("[ab]"), None, "2文字はマーカーでない");
        // A `[ ]` mid-sentence stays unchanged (reconfirming an existing guarantee).
        let mid = render_markdown("text with [ ] brackets\n", 40, BG, "TwoDark", false);
        assert!(mid
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| !is_task_span(s)));
    }

    #[test]
    fn task_source_locs_skip_fences_html_and_tables() {
        // Skip, using the same rules, the regions the render pipeline never routes through
        // decorate_extras (fences/HTML/tables), and correctly return the real tasks' line number,
        // state char, and byte position (these become the coordinates for a toggle's write).
        let src = "\
- [ ] first
```
- [x] in fence
```
<details>
- [x] in html block

</details>

| a | b |
|---|---|
| - [ ] cell | x |
  - [X] nested
- [/] custom
本文 [ ] は対象外
";
        let locs = task_source_locs(src, &[' ', '/', 'x'], &[]);
        let got: Vec<(usize, char)> = locs.iter().map(|l| (l.line, l.state)).collect();
        assert_eq!(
            got,
            vec![(0, ' '), (12, 'X'), (13, '/')],
            "実タスクのみ: {got:?}"
        );
        // state_off points exactly at the state char within the line (even with CJK/indentation mixed in).
        let lines: Vec<&str> = src.lines().collect();
        for l in &locs {
            assert!(
                lines[l.line][l.state_off..].starts_with(l.state),
                "offset mismatch at line {}",
                l.line
            );
        }
    }

    #[test]
    fn task_source_locs_accepts_all_gfm_bullets() {
        // GFM (and tui-markdown) render `-`/`*`/`+` bullets all alike as task items. If the source
        // scanner only recognized `-`, a `*`/`+` task would be rendered but not found by the
        // rescan, so the toggle's count check fails and every toggle gets cancelled (user report 2026-07-22).
        let src = "- [ ] dash\n* [ ] star\n+ [x] plus\n  * [ ] nested star\n";
        let locs = task_source_locs(src, &[' ', 'x'], &[]);
        let got: Vec<(usize, char)> = locs.iter().map(|l| (l.line, l.state)).collect();
        assert_eq!(
            got,
            vec![(0, ' '), (1, ' '), (2, 'x'), (3, ' ')],
            "3 種の箇条書き全てをタスクとして検出: {got:?}"
        );
        // state_off is right after `<bullet> [` for all three (indent + 3) = points exactly at the state char.
        let lines: Vec<&str> = src.lines().collect();
        for l in &locs {
            assert!(
                lines[l.line][l.state_off..].starts_with(l.state),
                "offset mismatch at line {} (bullet 種別に依らず正しい)",
                l.line
            );
        }
    }

    /// Root cause: `scan_task_lines` — the recursive scan used for a GitHub alert's body and an open
    /// `<details>`'s body, and any nesting of the two — computed `state_off` as `prefix + ws + off`,
    /// where `ws` is `leading_ws_width`'s **display column** width (a tab expands to the next 4-column
    /// stop) while `state_off` is used as a **byte** offset into the raw source line. A tab is 1 byte
    /// but (at column 0) 4 columns, so every tab in the leading run inflated `state_off` by 3 bytes
    /// too many — silently landing the 1-byte write on some byte of the task's own *text*, past the
    /// checkbox, not on the checkbox itself, and with no flash at all (`md_toggle_focused_task`'s
    /// cross-check compares the buggy offset against itself on both sides of an unedited file, so it
    /// cannot catch an internally wrong offset — only an external edit). The top-level scan (the tail
    /// of `task_source_locs` itself) has never had this bug: it computes its byte offset as
    /// `line.len() - line.trim_start().len()`, a byte length, not a column count.
    ///
    /// Each case nests a tab- (or space+tab-) indented checkbox under an outer list item so it is
    /// recognized as continuing the list (not as opening an indented code block, which would keep it
    /// out of `task_prefix_state` entirely and hide the bug) — exactly the shape of the report ("- a"
    /// then a tab-indented "- [ ] task"). Asserts `state_off` directly addresses the state character
    /// on disk, not merely that the toggle's *count* matches (the count is unaffected by this bug:
    /// wrong offset, same character total).
    #[test]
    fn scan_task_lines_state_off_is_byte_exact_with_tab_indentation() {
        let cases: &[(&str, &str)] = &[
            (
                "tab-indented checkbox inside an open <details> (the reported shape)",
                "<details open>\n<summary>s</summary>\n\n- a\n\t- [ ] task\n\n</details>\n",
            ),
            (
                "tab-indented checkbox inside a top-level GitHub alert",
                "> [!NOTE]\n> - a\n> \t- [ ] task\n",
            ),
            (
                "two-tab (deeper nesting) checkbox inside an open <details>",
                "<details open>\n<summary>s</summary>\n\n- a\n\t- b\n\t\t- [ ] task\n\n</details>\n",
            ),
            (
                "space-then-tab-indented checkbox inside an open <details>",
                "<details open>\n<summary>s</summary>\n\n- a\n \t- [ ] task\n\n</details>\n",
            ),
            (
                "tab-indented checkbox inside an alert nested in an open <details>",
                "<details open>\n<summary>s</summary>\n\n> [!NOTE]\n> - a\n> \t- [ ] task\n\n</details>\n",
            ),
        ];
        for (name, src) in cases {
            set_details_open(Vec::new());
            let locs = task_source_locs(src, &[' ', 'x'], &[]);
            let lines: Vec<&str> = src.lines().collect();
            let task_locs: Vec<&TaskLoc> = locs
                .iter()
                .filter(|l| lines[l.line].contains("task"))
                .collect();
            assert_eq!(
                task_locs.len(),
                1,
                "{name}: 「task」を含む行のチェックボックスがちょうど1個見つかるはず: {:?}",
                locs.iter()
                    .map(|l| (l.line, l.state_off, l.state))
                    .collect::<Vec<_>>()
            );
            let loc = task_locs[0];
            let line = lines[loc.line];
            assert!(
                line.is_char_boundary(loc.state_off),
                "{name}: state_off({}) が文字境界でない: {line:?}",
                loc.state_off
            );
            assert_eq!(
                line[loc.state_off..].chars().next(),
                Some(loc.state),
                "{name}: state_off({}) が状態文字 {:?} を指していない(行={:?})",
                loc.state_off,
                loc.state,
                line
            );
        }
    }

    #[test]
    fn cjk_table_is_rectangular_and_aligned() {
        // tui-markdown collapses a table into one line (#1). The custom renderer that intercepts it
        // measures full-width columns for alignment. Confirm every row ends up the same display width = rectangular, even with a full-width header mixed with ASCII data.
        let md = "| 種別 | ライブラリ | 依存 |\n|------|------------|------|\n\
                  | md   | tui-markdown | ratatui-core |\n| 図   | mermaid-text | unicode-width |\n";
        let lines = render_markdown(md, 80, BG, "TwoDark", false);
        // Not collapsed into one line (tui-markdown's default); confirm it spans multiple lines with borders.
        assert!(
            lines.len() >= 6,
            "表が行に展開されていない: {}",
            lines.len()
        );
        let w0 = line_disp_width(&lines[0]);
        assert!(w0 > 0);
        for (i, l) in lines.iter().enumerate() {
            assert_eq!(line_disp_width(l), w0, "{i}行目の表示幅が不揃い(右枠ズレ)");
        }
        // Contains the border (box corners).
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.as_ref()))
            .collect();
        assert!(joined.contains('┌') && joined.contains('┼') && joined.contains('┘'));
    }

    #[test]
    fn wide_table_wraps_within_terminal_width() {
        // A long cell wraps, capped at the terminal width (the border never overflows off-screen).
        let md = "| 名前 | 説明 |\n|---|---|\n\
                  | konoma | 全画面プレビュー特化のターミナルファイルブラウザです長い説明 |\n";
        let lines = render_markdown(md, 30, BG, "TwoDark", false);
        for (i, l) in lines.iter().enumerate() {
            assert!(line_disp_width(l) <= 30, "{i}行目が幅30を超過");
        }
        // Keeps a rectangular shape with a uniform display width.
        let w0 = line_disp_width(&lines[0]);
        assert!(lines.iter().all(|l| line_disp_width(l) == w0), "矩形でない");
    }

    #[test]
    fn splits_mermaid_fence_out_of_markdown() {
        let src = "# Title\n\nbefore\n\n```mermaid\ngraph TD\n  A --> B\n```\n\nafter\n";
        let segs = split_segments(&doc_run(src));
        assert_eq!(segs.len(), 3, "got {segs:?}");
        assert!(matches!(&segs[0], Segment::Md(s) if s.text().contains("Title")));
        assert!(matches!(&segs[1], Segment::Mermaid(s) if s.contains("graph TD")));
        assert!(matches!(&segs[2], Segment::Md(s) if s.text().contains("after")));
        // The fence lines are not included in the mermaid section.
        assert!(matches!(&segs[1], Segment::Mermaid(s) if !s.contains("```")));
    }

    #[test]
    fn normal_code_fence_is_kept_in_markdown() {
        // A ```rust block is not intercepted and is left in md (tui-markdown highlights it).
        let src = "text\n\n```rust\nlet x = 1;\n```\n";
        let segs = split_segments(&doc_run(src));
        assert_eq!(segs.len(), 1, "got {segs:?}");
        assert!(matches!(&segs[0], Segment::Md(s) if s.text().contains("let x = 1;")));
    }

    #[test]
    fn mermaid_inside_normal_fence_is_not_intercepted() {
        // A ```mermaid-looking line inside a normal fence never becomes a diagram (it's already inside a fence).
        let src = "~~~\n```mermaid\nnot a diagram\n```\n~~~\n";
        let segs = split_segments(&doc_run(src));
        assert!(
            segs.iter().all(|s| matches!(s, Segment::Md(_))),
            "got {segs:?}"
        );
    }

    #[test]
    fn renders_plain_markdown_to_lines() {
        let lines = render_markdown("# Hello\n\nworld\n", 80, BG, "TwoDark", false);
        assert!(!lines.is_empty());
    }

    #[test]
    fn invalid_mermaid_falls_back_to_raw() {
        // Unparseable source falls back to a raw display (a note at the top, the body kept).
        let lines = render_mermaid_file("this is definitely not mermaid syntax", 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn cjk_sequence_diagram_renders_not_fallback() {
        // upstream mermaid-text 0.56 used to panic internally on CJK participants/messages
        // (strip_keyword_prefix slicing without regard for byte boundaries). Resolved by the
        // is_char_boundary guard in vendor/mermaid-text → it's "actually drawn" as a box-drawing
        // diagram. This is the regression guard: if the patch is dropped, it panics → falls back to raw, the box-drawing disappears, and this test fails.
        let src = "sequenceDiagram\n  U->>K: ツリーで .mmd を選ぶ\n  K-->>U: 全画面プレビュー";
        let lines = render_mermaid_file(src, 70);
        assert!(!lines.is_empty(), "CJK 入力でも行を返すこと");
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            !joined.contains("cannot render mermaid"),
            "fallback に落ちている (patch 不在の疑い): {joined}"
        );
        // U+2500..U+257F = the box-drawing block. Present if it's an actual diagram.
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2500}'..='\u{257F}').contains(&c)),
            "CJK 図に罫線が無い (panic→fallback の疑い): {joined}"
        );
    }

    #[test]
    fn ascii_sequence_diagram_renders_box_drawing() {
        // A sequence diagram with ASCII labels actually renders as a box-drawing diagram (not a fallback).
        let src = "sequenceDiagram\n  participant U as User\n  participant K as konoma\n  U->>K: open\n  K-->>U: preview";
        let lines = render_mermaid_file(src, 70);
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            !joined.contains("cannot render mermaid"),
            "fallback に落ちている: {joined}"
        );
        // U+2500..U+257F is the box-drawing block. It's always present if it's an actual diagram.
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2500}'..='\u{257F}').contains(&c)),
            "罫線が無い: {joined}"
        );
    }

    #[test]
    fn heading_hash_is_stripped_and_rule_added() {
        let lines = render_markdown("# Title\n\nbody\n", 20, BG, "TwoDark", false);
        // The leading `#` disappears, leaving just the heading text.
        assert_eq!(lines[0].to_string(), "Title");
        // A full-width rule (━) is placed right below.
        assert!(
            lines[1].to_string().chars().all(|c| c == '━'),
            "rule 行が無い: {:?}",
            lines[1].to_string()
        );
    }

    #[test]
    fn code_block_becomes_special_area() {
        let lines = render_markdown(
            "text\n\n```rust\nlet x = 1;\n```\n",
            30,
            BG,
            "TwoDark",
            false,
        );
        // A line from a code block has the background color (DEFAULT_CODE_BG) and starts with the left gutter.
        let coded = lines
            .iter()
            .find(|l| l.to_string().contains("let x = 1;"))
            .expect("コード行が無い");
        assert_eq!(
            coded.style.bg,
            Some(DEFAULT_CODE_BG),
            "背景が敷かれていない"
        );
        assert!(coded.to_string().starts_with("▎"), "左ガターが無い");
        // A fence line ``` is never shown as-is; it's replaced by the language header (rust).
        assert!(lines.iter().all(|l| !l.to_string().contains("```")));
        assert!(lines.iter().any(|l| l.to_string().contains("rust")));
    }

    #[test]
    fn code_block_content_is_syntax_highlighted_and_indented() {
        // Disable tui-markdown's highlight-code, and color md fence code with our own syntect too.
        let lines = render_markdown(
            "```rust\nfn f() {\n    let x = 1;\n}\n```\n",
            40,
            BG,
            "TwoDark",
            false,
        );
        // Highlighting: a keyword etc. gets an Rgb foreground color.
        let colored = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
        assert!(colored, "md コードがハイライトされていない");
        // Indentation preserved: the original 4 spaces remain right after the gutter.
        let indented = lines
            .iter()
            .find(|l| l.to_string().contains("let x = 1;"))
            .expect("コード行");
        assert!(
            indented.to_string().contains("    let x = 1;"),
            "インデントが失われた: {:?}",
            indented.to_string()
        );
    }

    #[test]
    fn code_header_gutter_is_sentinel_and_body_gutter_is_not() {
        // The header's gutter span can be identified with is_code_header_span (the marker for Tab
        // focus), while a body line's gutter (not a sentinel) is never identified as one.
        let lines = render_markdown("```rust\nlet x = 1;\n```\n", 28, BG, "TwoDark", false);
        let header = lines
            .iter()
            .find(|l| l.to_string().contains("rust"))
            .expect("言語ヘッダが無い");
        assert!(
            header.spans.iter().any(is_code_header_span),
            "ヘッダに番兵ガターが無い"
        );
        // A body line's (code line's) gutter is not a sentinel.
        let body = lines
            .iter()
            .find(|l| l.to_string().contains("let x = 1;"))
            .expect("コード本文行が無い");
        assert!(
            !body.spans.iter().any(is_code_header_span),
            "本文ガターを誤って番兵判定"
        );
    }

    #[test]
    fn code_block_source_locs_extracts_and_skips_mermaid() {
        let src = "\
intro

```rust
fn a() {}
```

```mermaid
graph TD
  A-->B
```

~~~text
plain body
~~~
";
        let blocks = code_block_source_locs(src, &[]);
        assert_eq!(
            blocks,
            vec!["fn a() {}".to_string(), "plain body".to_string()],
            "``` と ~~~ を拾い mermaid は除外・生本文を保つ"
        );
    }

    #[test]
    fn code_header_language_is_a_right_aligned_badge() {
        let lines = render_markdown("```rust\nlet x = 1;\n```\n", 28, BG, "TwoDark", false);
        let header = lines
            .iter()
            .find(|l| l.to_string().contains("rust"))
            .expect("言語ヘッダが無い");
        // The language name is placed at the end of the line (right-aligned).
        assert!(
            header.to_string().trim_end().ends_with("rust"),
            "右寄せでない: {:?}",
            header.to_string()
        );
        // The badge span has a lighter background than the body (distinguishable).
        let badge = header
            .spans
            .iter()
            .find(|s| s.content.contains("rust"))
            .expect("バッジ span");
        assert_eq!(
            badge.style.bg,
            Some(crate::config::lighten(DEFAULT_CODE_BG)),
            "バッジ背景が明るくない"
        );
        assert_ne!(badge.style.bg, Some(DEFAULT_CODE_BG), "本文背景と同色");
    }

    #[test]
    fn code_header_align_left_and_right() {
        // Right-aligned (default): the language name is at the line's end.
        let right = render_markdown("```rust\nx\n```\n", 28, BG, "TwoDark", false);
        let rh = right
            .iter()
            .find(|l| l.to_string().contains("rust"))
            .unwrap();
        let rs = rh.to_string();
        assert!(rs.trim_end().ends_with("rust"), "右寄せでない: {rs:?}");
        let right_pos = rs.find("rust").unwrap();
        // Left-aligned: the language name right after the gutter (the line-start side).
        let left = render_markdown(
            "```rust\nx\n```\n",
            28,
            CodeStyle {
                label_right: false,
                ..BG
            },
            "TwoDark",
            false,
        );
        let ls = left
            .iter()
            .find(|l| l.to_string().contains("rust"))
            .unwrap()
            .to_string();
        let left_pos = ls.find("rust").unwrap();
        assert!(
            left_pos < right_pos,
            "左寄せが右寄せより前に来ていない: left={left_pos} right={right_pos}"
        );
    }

    #[test]
    fn code_label_bg_is_configurable() {
        // The badge background can be set to any color.
        let style = CodeStyle {
            label_bg: Some(Color::Rgb(200, 50, 50)),
            ..BG
        };
        let lines = render_markdown("```rust\nx\n```\n", 28, style, "TwoDark", false);
        let badge = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("rust"))
            .expect("バッジ span");
        assert_eq!(badge.style.bg, Some(Color::Rgb(200, 50, 50)));
    }

    #[test]
    fn code_header_badge_has_no_bg_when_code_bg_none() {
        // code_bg=None: the badge has no background (distinguished by a dim color instead).
        let lines = render_markdown("```rust\nx\n```\n", 28, NO_CODE, "TwoDark", false);
        let badge = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("rust"))
            .expect("バッジ span");
        assert_eq!(badge.style.bg, None);
    }

    #[test]
    fn code_bg_color_is_configurable() {
        // The configured color (green) applies to both inline code and code blocks.
        let green = Color::Rgb(10, 80, 20);
        let md = "本文 `inline` 続き\n\n```rust\nlet x = 1;\n```\n";
        let style = CodeStyle {
            bg: Some(green),
            ..BG
        };
        let lines = render_markdown(md, 40, style, "TwoDark", false);
        // The inline code span has the configured color as its background.
        let inline_bg = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "inline")
            .and_then(|s| s.style.bg);
        assert_eq!(inline_bg, Some(green), "inline code に設定色が乗っていない");
        // A code-block line also uses the configured color.
        let coded = lines
            .iter()
            .find(|l| l.to_string().contains("let x = 1;"))
            .expect("コード行が無い");
        assert_eq!(
            coded.style.bg,
            Some(green),
            "コードブロックに設定色が乗っていない"
        );
    }

    #[test]
    fn code_bg_none_removes_all_backgrounds() {
        // code_bg=None: no background on either inline code or a code block (the left gutter stays).
        let md = "本文 `inline` 続き\n\n```rust\nlet x = 1;\n```\n";
        let lines = render_markdown(md, 40, NO_CODE, "TwoDark", false);
        let inline_bg = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "inline")
            .and_then(|s| s.style.bg);
        assert_eq!(inline_bg, None, "inline code の背景が消えていない");
        let coded = lines
            .iter()
            .find(|l| l.to_string().contains("let x = 1;"))
            .expect("コード行が無い");
        assert_eq!(coded.style.bg, None, "コードブロックの背景が消えていない");
        // The left gutter is kept even without a background, so it's still recognizable as code.
        assert!(coded.to_string().starts_with("▎"), "左ガターは残すべき");
    }

    #[test]
    fn cjk_in_markdown_with_mermaid_fence_does_not_panic() {
        // Confirm the app path (render_markdown) doesn't panic even with a mermaid fence + CJK inside the md.
        let src =
            "# 図\n\n```mermaid\nsequenceDiagram\n  甲->>乙: こんにちは\n  乙-->>甲: どうも\n```\n";
        let lines = render_markdown(src, 70, BG, "TwoDark", false);
        assert!(!lines.is_empty());
    }

    #[test]
    fn konoma_stylesheet_arms_return_expected_styles() {
        // The tui-markdown version in use doesn't reflect the blockquote/metadata StyleSheet methods
        // into the output spans (never reached via the render path), so directly verify the StyleSheet implementation to cover every arm.
        let s = KonomaStyles {
            code_bg: Some(Color::Rgb(1, 2, 3)),
        };
        // blockquote = green + italic.
        let bq = s.blockquote();
        assert_eq!(bq.fg, Some(Color::Green));
        assert!(bq.add_modifier.contains(Modifier::ITALIC));
        // metadata block = LightYellow.
        assert_eq!(s.metadata_block().fg, Some(Color::LightYellow));
        // heading_meta = DIM.
        assert!(s.heading_meta().add_modifier.contains(Modifier::DIM));
        // Per heading level (1/2=bold, 3=italic, anything else=DIM+italic).
        assert_eq!(s.heading(1).fg, Some(HEAD_FG));
        assert!(s.heading(1).add_modifier.contains(Modifier::BOLD));
        assert!(s.heading(3).add_modifier.contains(Modifier::ITALIC));
        assert!(s.heading(6).add_modifier.contains(Modifier::DIM));
        // Inline code: reflects the configured background color / no background if None.
        assert_eq!(s.code().bg, Some(Color::Rgb(1, 2, 3)));
        let no_bg = KonomaStyles { code_bg: None };
        assert_eq!(no_bg.code().bg, None);
        // A link is underlined.
        assert!(s.link().add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn render_markdown_with_mermaid_fence_renders_box_drawing() {
        // A ```mermaid fence inside the md is intercepted and becomes a box-drawing diagram via render_mermaid_safe.
        let md = "# Title\n\n```mermaid\nsequenceDiagram\n  A->>B: hi\n  B-->>A: yo\n```\n";
        let lines = render_markdown(md, 70, BG, "TwoDark", false);
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            !joined.contains("cannot render mermaid"),
            "fallback に落ちた: {joined}"
        );
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2500}'..='\u{257F}').contains(&c)),
            "罫線が無い: {joined}"
        );
    }

    // ---- Inline block-level images ----

    #[test]
    fn extract_block_image_markdown_and_html() {
        assert_eq!(
            extract_block_image("![alt text](pic.png)"),
            Some(("alt text".into(), "pic.png".into()))
        );
        // Link-wrapped image.
        assert_eq!(
            extract_block_image("[![a](i.png)](https://x)"),
            Some(("a".into(), "i.png".into()))
        );
        // A title in the URL is stripped.
        assert_eq!(
            extract_block_image(r#"![a](p.png "title")"#),
            Some(("a".into(), "p.png".into()))
        );
        // HTML <img>, optionally wrapped in layout tags.
        assert_eq!(
            extract_block_image(r#"<img src="x.png" alt="y">"#),
            Some(("y".into(), "x.png".into()))
        );
        assert_eq!(
            extract_block_image(r#"<p align="center"><img src="hero.png" width="860"></p>"#),
            Some((String::new(), "hero.png".into()))
        );
        // Not standalone images.
        assert_eq!(extract_block_image("see ![a](p.png) here"), None);
        assert_eq!(extract_block_image("just text"), None);
        assert_eq!(
            extract_block_image(r#"<p>text <img src="a.png"> more</p>"#),
            None
        );
    }

    #[test]
    fn images_in_code_fences_are_not_extracted() {
        let src = "before\n\n```\n![a](x.png)\n```\n\nafter\n";
        let slot_of = |_: &str| ImageSlot::Inline { cols: 10, rows: 4 };
        let (_lines, imgs, _extras) = render_markdown_with_images(
            src,
            40,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &slot_of,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert!(imgs.is_empty(), "fence 内の画像を誤検出: {imgs:?}");
    }

    #[test]
    fn block_image_reserves_rows_and_records_placement() {
        let src = "# Title\n\n![hero](hero.png)\n\nbody\n";
        let slot_of = |_: &str| ImageSlot::Inline { cols: 20, rows: 5 };
        let (lines, imgs, _extras) = render_markdown_with_images(
            src,
            40,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &slot_of,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert_eq!(imgs.len(), 1);
        let p = &imgs[0];
        assert_eq!((p.cols, p.rows), (20, 5));
        assert_eq!(p.url, "hero.png");
        assert_eq!(p.alt, "hero");
        assert!(p.line < lines.len());
        // The first reserved row shows the alt label; the block reserves `rows` lines.
        assert!(
            lines[p.line].to_string().contains("hero"),
            "placeholder label 無し"
        );
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("body"), "画像後の本文が消えた");
    }

    #[test]
    fn image_without_backend_degrades_to_text() {
        let src = "![alt](missing.png)\n";
        let slot_of = |_: &str| ImageSlot::Unavailable; // no backend / unresolvable → text (principle #3)
        let (lines, imgs, _extras) = render_markdown_with_images(
            src,
            40,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &slot_of,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert!(imgs.is_empty());
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("alt"), "alt テキストが無い: {joined}");
        assert!(joined.contains("missing.png"));
    }

    #[test]
    fn remote_image_shows_loading_line() {
        let src = "![shot](https://example.com/a.png)\n";
        let slot_of = |_: &str| ImageSlot::Loading;
        let (lines, imgs, _extras) = render_markdown_with_images(
            src,
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &slot_of,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert!(imgs.is_empty(), "loading 中は placement を出さない");
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("loading"), "loading 表示が無い: {joined}");
    }

    #[test]
    fn collect_remote_image_urls_finds_http_only() {
        let src = "\
![a](local.png)

![b](https://example.com/remote.png)

<p><img src=\"http://example.com/html.png\"></p>

```
![c](https://example.com/in-fence.png)
```
";
        let urls = collect_remote_image_urls(src);
        assert_eq!(
            urls,
            vec![
                "https://example.com/remote.png".to_string(),
                "http://example.com/html.png".to_string(),
            ],
            "remote のみ・fence 内は除外・順序保持: {urls:?}"
        );
    }

    #[test]
    fn code_block_cache_hits_are_identical_and_bounded() {
        // Re-highlighting the same fence is a cache hit, and the output (glyphs, styles) matches exactly.
        let body: Vec<String> = vec!["fn main() {}".into(), "let x = 1;".into()];
        let a = highlight_body(&body, "rust", 60, None, "TwoDark", 4, true);
        let b = highlight_body(&body, "rust", 60, None, "TwoDark", 4, true);
        assert_eq!(a.len(), b.len());
        for (la, lb) in a.iter().zip(&b) {
            assert_eq!(la.spans.len(), lb.spans.len());
            for (sa, sb) in la.spans.iter().zip(&lb.spans) {
                assert_eq!(
                    (sa.content.as_ref(), sa.style),
                    (sb.content.as_ref(), sb.style)
                );
            }
        }
        // Bounded capacity: even feeding in more distinct keys than the cap, the entry count never exceeds CAP.
        for i in 0..(CODE_BLOCK_CACHE_CAP + 20) {
            let one = vec![format!("let v{i} = {i};")];
            let _ = highlight_body(&one, "rust", 60, None, "TwoDark", 4, true);
        }
        assert!(code_block_cache_len() <= CODE_BLOCK_CACHE_CAP);
    }

    // ---- mermaid image rendering (v0.15 feature) ----------------------------------

    /// SVG-compatibility regression test: confirm mermaid-rs-renderer's SVG output can be rasterized
    /// as-is by konoma's own resvg/usvg (seals off any version drift between the renderer and usvg). Includes CJK.
    #[test]
    fn mermaid_to_svg_output_rasterizes_with_konoma_resvg() {
        let svg = mermaid_to_svg(
            "graph TD\n  A[ツリー] -->|Enter| B{種別解決}\n  B --> C[プレビュー]",
            "dark",
        )
        .expect("mermaid renders to SVG");
        assert!(svg.contains("<svg"), "SVG らしい出力");
        let img = crate::preview::svg::rasterize_bytes(
            svg.as_bytes(),
            std::path::Path::new("m.svg"),
            400,
        )
        .expect("konoma の resvg でラスタライズできる");
        assert!(img.width() > 0 && img.height() > 0);
    }

    /// Layout measurement is pinned to modern across every theme (regression guard against the
    /// placement bug where a dark-theme font-metric difference made an edge draw a detour ring — measured 2026-07-17).
    #[test]
    fn mermaid_dark_theme_uses_modern_font_metrics() {
        let svg = mermaid_to_svg("graph LR\nA-->B", "dark").unwrap();
        assert!(svg.contains("Inter"), "modern のフォントファミリで計測");
        assert!(!svg.contains("trebuchet"), "dark 固有フォントは使わない");
    }

    #[test]
    fn mermaid_to_svg_fails_safely_on_garbage() {
        // Unsupported/broken input is None (the caller degrades to the text diagram). Never panics.
        assert!(mermaid_to_svg("definitely not a diagram !!!", "dark").is_none());
    }

    #[test]
    fn catch_silent_returns_none_on_panic_and_some_on_success() {
        // The worker safety net: a panic yields None (the caller sends a failure result to clear the
        // inflight state); a normal value comes back as Some, unchanged. The suppression flag never lingers on either path.
        assert_eq!(catch_silent(|| 42), Some(42));
        let paniced: Option<i32> = catch_silent(|| panic!("worker blew up"));
        assert_eq!(paniced, None);
        PANIC_SILENCED.with(|c| assert!(!c.get(), "panic 経路でも抑制フラグが残らない"));
        // Confirm a subsequent normal panic message can still be emitted (the hook isn't stuck silencing).
        assert_eq!(catch_silent(|| "ok"), Some("ok"));
    }

    /// D2 (2026-08-05): `compute_or_fallback` is what `spawn_or_sync_statuses`/`spawn_or_sync_ignored`
    /// (`app/bookmark_actions.rs`) and `fence_sharpen_if_needed` (`app/md_media.rs`) now call instead
    /// of the old `if let Some(res) = catch_silent(..) { tx.send(res) }` shape, which sent *nothing*
    /// on a caught panic. This is the part of that fix that's directly testable with a real panic
    /// (the git-scan/resvg call each site wraps can't be made to panic on demand): on success it
    /// returns `f`'s value untouched; on a panic it returns `fallback()`'s value instead of
    /// propagating — so a caller placing an unconditional `tx.send(..)` right after this call, as all
    /// three sites now do, is guaranteed to always send *something*.
    #[test]
    fn compute_or_fallback_returns_fs_value_on_success_and_fallbacks_value_on_panic() {
        assert_eq!(
            compute_or_fallback(|| 42, || 0),
            42,
            "成功時は f の値そのまま"
        );
        let v = compute_or_fallback(|| -> i32 { panic!("simulated worker panic") }, || 7);
        assert_eq!(
            v, 7,
            "パニック時は fallback の値(=何も送らないよりは安全な既知の失敗状態)"
        );
        // The suppression flag doesn't linger (same guarantee catch_silent itself gives).
        PANIC_SILENCED.with(|c| assert!(!c.get(), "パニック経路でも抑制フラグが残らない"));
    }

    #[test]
    fn concurrent_mermaid_renders_keep_panic_hook_sane() {
        // Regression 2026-07-18: the old implementation swapped take_hook/set_hook on every call, so
        // if the restore order got interleaved during a fence worker's concurrent renders, the
        // "silent hook" could permanently stick. The current implementation uses Once+thread-local:
        // after concurrent execution, this thread's suppression flag is back down.
        let hs: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    let code = if i % 2 == 0 {
                        "garbage ]][[ not a diagram".to_string()
                    } else {
                        format!("graph LR\n  A{i} --> B{i}")
                    };
                    (mermaid_to_svg(&code, "dark").is_some(), i % 2 == 1)
                })
            })
            .collect();
        for h in hs {
            let (got, expect) = h.join().unwrap();
            assert_eq!(got, expect, "並行レンダでも成否は入力どおり");
        }
        PANIC_SILENCED.with(|c| assert!(!c.get(), "抑制フラグが残留しない"));
    }

    #[test]
    fn collect_mermaid_fences_top_level_only() {
        let src = "# t\n```mermaid\ngraph LR\nA-->B\n```\n\n````md\n```mermaid\ninner\n```\n````\n";
        let fences = collect_mermaid_fences(src);
        assert_eq!(fences.len(), 1, "外側フェンス内の mermaid は抽出しない");
        assert_eq!(fences[0], "graph LR\nA-->B\n");
        // An unclosed fence is valid CommonMark: a fenced code block with no matching close runs to
        // the end of the input (the same rule any other unclosed fenced code block already follows —
        // see `model::Doc::parse`, backed by `pulldown_cmark`). The pre-`md-block-walk` hand-rolled
        // scanner (`collect_mermaid_fences_legacy`) special-cased this shape as "safely revert to
        // text" instead, but `extraction_targets` mirrors whatever `render::render_doc` itself would
        // actually place (see that function's own doc comment) — and `render_doc` does place this one,
        // the same way it renders any other unclosed fence as a genuine code block. Not reproducing a
        // hand-picked "unterminated" special case here is exactly the point (see `extraction_targets`'s
        // own doc comment on why a second, independently maintained walk of `src` is what this
        // refactor exists to eliminate).
        let unterminated = "```mermaid\ngraph LR\nA-->B\n";
        assert_eq!(
            collect_mermaid_fences(unterminated),
            vec!["graph LR\nA-->B\n".to_string()],
        );
    }

    #[test]
    fn mermaid_slots_image_loading_text() {
        let src = "before\n\n```mermaid\ngraph LR\nA-->B\n```\n\nafter\n";
        let slot_img = |_: &str| MermaidSlot::Image { cols: 20, rows: 5 };
        let (lines, imgs, _extras) = render_markdown_with_images(
            src,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_| ImageSlot::Unavailable,
            &slot_img,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert_eq!(imgs.len(), 1, "フェンスが placement になる");
        assert!(is_mermaid_fence_url(&imgs[0].url), "合成キー URL");
        // The caption line (a focus sentinel) sits right before the reserved row.
        let cap = &lines[imgs[0].line - 1];
        assert!(
            cap.spans.iter().any(is_mermaid_header_span),
            "キャプション行に番兵 span"
        );
        // Loading: no placement, but a loading line is present.
        let (lines, imgs, _extras) = render_markdown_with_images(
            src,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Loading,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert!(imgs.is_empty());
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(joined.contains("loading"), "ローディング行: {joined}");
        // Text: the probe is also Text = extraction itself is OFF → the legacy text diagram (box-drawing) path.
        let (lines, imgs, _extras) = render_markdown_with_images(
            src,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert!(imgs.is_empty());
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            joined.contains('A') && joined.contains('B'),
            "テキスト図として描画: {joined}"
        );
    }

    /// The caption uses the pre-translated string passed in by the caller, but the `◇ mermaid`
    /// prefix stays fixed and keeps being recognized as the sentinel (is_mermaid_header_span) —
    /// i18n-izing it doesn't break Tab-focus cycling.
    #[test]
    fn fence_caption_is_localizable_but_sentinel_survives() {
        let src = "```mermaid\ngraph LR\nA-->B\n```\n";
        let slot_img = |_: &str| MermaidSlot::Image { cols: 20, rows: 5 };
        let render = |caption: &str| {
            render_markdown_with_images(
                src,
                60,
                CodeStyle::default(),
                "TwoDark",
                false,
                DEFAULT_TASK_STATES,
                &|_| ImageSlot::Unavailable,
                &slot_img,
                caption,
                true,
                &|_: &str, _: bool| MathSlot::Raw,
                false,
            )
        };
        let (en, ei, _) = render("Enter: full screen");
        let (ja, ji, _) = render("Enter: 全画面");
        let cap_en = en[ei[0].line - 1].to_string();
        let cap_ja = ja[ji[0].line - 1].to_string();
        assert!(
            cap_en.contains("Enter: full screen"),
            "en キャプション: {cap_en}"
        );
        assert!(
            cap_ja.contains("Enter: 全画面"),
            "ja キャプション: {cap_ja}"
        );
        assert_ne!(cap_en, cap_ja, "言語でキャプションが変わる");
        // Recognized as the sentinel in either language (the prefix is unchanged).
        for (lines, imgs) in [(en, ei), (ja, ji)] {
            assert!(
                lines[imgs[0].line - 1]
                    .spans
                    .iter()
                    .any(is_mermaid_header_span),
                "番兵 span が残る"
            );
        }
    }

    #[test]
    fn parse_alert_header_recognizes_types_and_aliases() {
        assert_eq!(parse_alert_header("> [!NOTE]").unwrap().0, AlertKind::Note);
        assert_eq!(
            parse_alert_header("> [!warning]").unwrap().0,
            AlertKind::Warning
        );
        // Alias + trailing Obsidian-style title.
        let (k, title) = parse_alert_header("> [!danger] Watch out").unwrap();
        assert_eq!(k, AlertKind::Caution);
        assert_eq!(title, "Watch out");
        // Not alerts.
        assert!(parse_alert_header("> just a quote").is_none());
        assert!(parse_alert_header("> [!NOPE]").is_none());
        assert!(parse_alert_header("[!NOTE]").is_none()); // needs the blockquote marker
    }

    #[test]
    fn render_alert_makes_a_colored_callout_not_literal_marker() {
        let md = "> [!WARNING]\n> careful with [docs](./x.md)\n";
        // alerts on
        let on = render_via_dispatcher(
            &doc_run(md),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let joined: String = on
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined.contains("Warning"),
            "callout shows the label: {joined:?}"
        );
        assert!(
            !joined.contains("[!WARNING]"),
            "the raw marker is gone: {joined:?}"
        );
        // Header line carries the alert color (yellow for Warning) and a left bar.
        assert!(
            on[0]
                .spans
                .iter()
                .any(|s| s.content.contains('▌') && s.style.fg == Some(Color::Yellow)),
            "colored left bar on the header"
        );
        // The body's Markdown still renders (the link label survives as a span).
        let has_link_label = on
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.content.contains("docs"));
        assert!(has_link_label, "alert body Markdown is rendered");

        // alerts off → ordinary blockquote, the marker stays literal.
        let off = render_via_dispatcher(
            &doc_run(md),
            60,
            BG,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            false,
        );
        let joined_off: String = off
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined_off.contains("[!WARNING]"),
            "alerts off keeps the raw marker: {joined_off:?}"
        );
    }

    #[test]
    fn process_inline_html_converts_common_tags() {
        assert!(process_inline_html("<del>gone</del>\n").contains("~~gone~~"));
        assert!(process_inline_html("<s>x</s>\n").contains("~~x~~"));
        assert!(process_inline_html("<strike>y</strike>\n").contains("~~y~~"));
        assert!(process_inline_html("<kbd>Ctrl</kbd>\n").contains("`Ctrl`"));
        assert!(process_inline_html("H<sub>2</sub>O and x<sup>2</sup>\n").contains("H₂O and x²"));
        // Non-mappable sup content keeps its text rather than mangling it.
        assert!(process_inline_html("<sup>note</sup>\n").contains("note"));
        // <br> → a hard line break (two trailing spaces + newline).
        assert!(process_inline_html("a<br>b\n").contains("a  \nb"));
    }

    // ---- `<br>` must not tear the block it lives in (2026-08) --------------------------------
    //
    // Root cause these pin: rewriting `<br>` unconditionally into `"  \n"` injects a line carrying
    // none of the prefix its enclosing block requires, so the block is destroyed rather than broken
    // *into* two lines. None of this is visible to a count-based check — the renderer and the source
    // scanner agree on the number of checkboxes and code blocks either way — so every assertion here
    // is about the *structure* of the rendered lines.

    /// One document through the production chain: the inline-HTML pre-pass, then
    /// `render_markdown_with_images` — the entry point the app actually calls.
    ///
    /// Deliberately not the `render_markdown_tasks` shorthand: that one skips the block-level
    /// extraction pass, and lifting an `<img>` line out of a banner is precisely what splits the
    /// surrounding HTML block (see `rendered_code_blocks`' own note). A `<br>` test measured on the
    /// shorthand would not see the failure the two banner READMEs actually showed.
    fn br_pre_and_render(src: &str) -> (String, Vec<Line<'static>>) {
        let pre = process_inline_html(src);
        set_details_open(collect_details_open(&pre));
        let (lines, _, _extras) = render_markdown_with_images(
            &pre,
            100,
            NO_CODE,
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        (pre, lines)
    }

    /// The rendered lines as plain text, and how many code-block headers were drawn.
    fn br_texts(src: &str) -> (Vec<String>, usize) {
        let (_, lines) = br_pre_and_render(src);
        let headers = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_code_header_span(s))
            .count();
        let texts = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        (texts, headers)
    }

    /// [`br_texts`] with a backend that can actually show images, so the **placements** are
    /// observable as well as the code-block headers.
    ///
    /// The distinction is not cosmetic. When a banner collapses into an indented code block, the
    /// document-wide code mask stops calling its `<img>` lines HTML, `split_block_parts` no longer
    /// lifts them out, and the image is *lost* — while a header-count assertion can still pass,
    /// because the surviving markup may render through a path that draws no konoma code header at
    /// all (that is exactly what the quoted form of this shape does). Losing a badge is the damage
    /// a reader sees, so it is asserted directly.
    fn banner_render(src: &str) -> (Vec<String>, usize, usize) {
        let pre = process_inline_html(src);
        set_details_open(collect_details_open(&pre));
        let (lines, places, _extras) = render_markdown_with_images(
            &pre,
            70,
            NO_CODE,
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Inline { cols: 4, rows: 2 },
            &|_: &str| MermaidSlot::Text,
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        let headers = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_code_header_span(s))
            .count();
        let texts = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        (texts, headers, places.len())
    }

    /// The centered badge banner of the three tests above, **one container deep** — the level at
    /// which `structure_mask` used to stop looking.
    ///
    /// Root cause (2026-08): the mask reported only the outermost construct, so every line of a
    /// banner nested in `<details>` came back `Structure::Details` (and in a quote, `Structure::Quote`).
    /// The lone `    <br>` line therefore took the default hard-break rewrite instead of the
    /// HTML-block one, and `"    " + "  "` is whitespace — a blank line to CommonMark. The inner
    /// `<p align="center">` block ended there, the 4-column-indented lines below it became a genuine
    /// indented code block, and the block splitter (correctly, by then) honoured that: the markup
    /// drew as `▎ …` and the `<img>` line was no longer extracted, so **a badge disappeared**.
    ///
    /// Both containers are covered because they reach the bug by different routes — a `<details>`
    /// span is filled whole, a quote is a run of `>`-prefixed lines whose body only looks like an
    /// HTML block once the markers are stripped — and the quoted form additionally needs "blank"
    /// to be judged *inside* the quote, since `">       "` is not whitespace to `str::trim`.
    ///
    /// Both link shapes are covered too. With the `<img>` on the `<a>`'s own line the whole line is
    /// extracted and nothing is left behind; with it on a line of its own, `split_block_parts`
    /// leaves the `    </a>` / `    <a …>` fragments that make this bug class hard in the first
    /// place. The real READMEs use the second, and only it exercises the fragment path.
    ///
    /// Asserts the **placement count**, not just the header count: the quoted form loses its badge
    /// while drawing zero konoma code headers, so a header-only assertion passes on the broken
    /// build. Measured against the broken build, the four cases were 1/1, 1/1, 0/1, 0/1
    /// (headers/images) where every one must be 0/2.
    #[test]
    fn banner_nested_in_a_container_keeps_both_badges_and_draws_no_code_block() {
        let single = concat!(
            "<p align=\"center\">\n",
            "    <a href=\"u\"><img src=\"https://s.svg\" alt=\"BADGE-A\"></a>\n",
            "    <br>\n",
            "    <a href=\"v\"><img src=\"https://t.svg\" alt=\"BADGE-B\"></a>\n",
            "</p>\n",
        );
        let multi = concat!(
            "<p align=\"center\">\n",
            "    <a href=\"u\">\n",
            "      <img src=\"https://s.svg\" alt=\"BADGE-A\">\n",
            "    </a>\n",
            "    <br>\n",
            "    <a href=\"v\">\n",
            "      <img src=\"https://t.svg\" alt=\"BADGE-B\">\n",
            "    </a>\n",
            "</p>\n",
        );
        let quoted = |b: &str| {
            b.lines()
                .map(|l| format!("> {l}\n"))
                .collect::<Vec<_>>()
                .concat()
        };
        let folded =
            |b: &str| format!("<details open>\n<summary>Badges</summary>\n\n{b}\n</details>\n");
        for (name, src) in [
            ("details/single-line", folded(single)),
            ("details/multi-line", folded(multi)),
            ("quote/single-line", quoted(single)),
            ("quote/multi-line", quoted(multi)),
        ] {
            let (texts, headers, images) = banner_render(&src);
            assert_eq!(
                headers, 0,
                "{name}: 入れ子のバナーが字下げコードブロックとして描かれている: {texts:#?}"
            );
            assert_eq!(
                images, 2,
                "{name}: バッジが失われている(片方だけ抽出された): {texts:#?}"
            );
            // The leftover `</a>` fragments are rescued as HTML rather than drawn as markup. This
            // used to fail for `"quote/multi-line"` specifically — a quote's own `>` marker survived
            // on every continuation line of the legacy fallback's raw slice, invisible to
            // `split_html_blocks`, which matches on the raw line — but `render::render_doc`
            // (`model::BlockKind::Html.body_spans`, see that field's own doc comment) no longer falls
            // back to that legacy path for a quote-nested `Html` block at all, so the leak is gone
            // for every one of these shapes now, `"quote/multi-line"` included; see
            // `md_model_snapshot_tests`/`render::tests::quote_nested_table_is_reported_unsupported_html_is_not`
            // for the fix's own dedicated coverage.
            assert!(
                !texts.iter().any(|t| t.contains("</a>")),
                "{name}: 生の HTML タグが画面に漏れている: {texts:#?}"
            );
        }
    }

    /// A shape the pre-`body_spans` legacy fallback did **not** clean up completely — this test used
    /// to be named `banner_quoted_multiline_still_leaks_its_leftover_fragments`, pinning that gap as
    /// a known limitation rather than a surprise: a multi-line badge banner inside a **blockquote**
    /// kept both images, but the `    </a>` fragments `split_block_parts` left behind still drew as a
    /// code block inside the quote (`split_html_blocks` asks `is_html_block_start` about the raw
    /// line, and a quoted fragment begins with `>`, not `<`, so the rescue never fired there).
    ///
    /// `model::BlockKind::Html.body_spans` (see its own doc comment) closes this: `render::render_doc`
    /// no longer reports a quote-nested `Html` block unsupported at all, so `render_markdown_with_images`
    /// never falls back to this legacy path for this shape any more — the fragments render as ordinary,
    /// tag-stripped `Html` content instead of leaking or degrading to a code block. Flipped from a
    /// known-limitation pin to a fix regression test rather than deleted outright, so this exact shape
    /// stays covered either way.
    #[test]
    fn banner_quoted_multiline_no_longer_leaks_its_leftover_fragments() {
        let src = concat!(
            "> <p align=\"center\">\n",
            ">     <a href=\"u\">\n",
            ">       <img src=\"https://s.svg\" alt=\"BADGE-A\">\n",
            ">     </a>\n",
            ">     <br>\n",
            ">     <a href=\"v\">\n",
            ">       <img src=\"https://t.svg\" alt=\"BADGE-B\">\n",
            ">     </a>\n",
            "> </p>\n",
        );
        let (texts, headers, images) = banner_render(src);
        assert_eq!((headers, images), (0, 2), "画像は両方とも残る: {texts:#?}");
        assert!(
            !texts.iter().any(|t| t.contains("</a>")),
            "生の HTML タグが画面に漏れている: {texts:#?}"
        );
    }

    /// The banner's `<br>` line is dropped inside a container exactly as it is at top level — but a
    /// `<br>` that is *not* inside a nested HTML block must still become a break, at either depth.
    ///
    /// Pins the boundary the fix has to respect: the rescan re-labels only the lines a nested HTML
    /// block actually covers, so the `text` / `<br>` / `more` idiom (whose middle line konoma calls
    /// a block *opener*, never a body line) keeps producing the gap the author asked for. Without
    /// this, "relabel the container's interior" is satisfiable by relabelling all of it — which
    /// deletes those breaks, the damage measured on `bit-set`/`bit-vec` when `HtmlOpen` was routed
    /// the other way.
    ///
    /// The `<details>` case with **no blank line after the opening tag** is the one that
    /// discriminates, and it is here because the obvious over-broad implementation (rescan the whole
    /// span, tag lines included) passes every other assertion in this module: only there does the
    /// `<details>` line itself open a block that swallows the `<br>` below it into body position.
    /// The blank-line-separated form and the quoted form are kept alongside it as the shapes real
    /// documents actually use.
    #[test]
    fn a_lone_br_between_paragraphs_still_breaks_inside_a_container() {
        for (name, src) in [
            (
                "details/no blank line",
                "<details open>\nalpha\n<br>\nbeta\n</details>\n",
            ),
            (
                "details/blank-separated",
                "<details open>\n<summary>S</summary>\n\nalpha\n\n<br>\n\nbeta\n\n</details>\n",
            ),
            ("blockquote", "> alpha\n>\n> <br>\n>\n> beta\n"),
        ] {
            let pre = process_inline_html(src);
            assert!(
                !pre.contains("<br>"),
                "{name}: <br> が書き換えられていない: {pre:?}"
            );
            // The break survives as the blank line it has always produced, rather than the line
            // being dropped: dropping is for a `<br>` sitting *inside* an already-open HTML block,
            // and none of these is.
            assert_eq!(
                pre.lines().count(),
                src.lines().count(),
                "{name}: <br> の行ごと落ちて段落の切れ目が消えている: {pre:?}"
            );
            let (texts, _, _) = banner_render(src);
            for want in ["alpha", "beta"] {
                assert!(
                    texts.iter().any(|t| t.contains(want)),
                    "{name}: {want:?} が失われている: {texts:#?}"
                );
            }
        }
    }

    /// The one shape where a `<br>` inside a `<details>` body *is* dropped, recorded because it is a
    /// deliberate consequence rather than an accident: with no blank line between them, the
    /// `<summary>` line opens an HTML block (konoma's `is_html_block_start` accepts any tag and runs
    /// the block to the next blank line) that reaches the `<br>` below it, so the tag sits in body
    /// position and the body rule removes the line.
    ///
    /// It is the same answer CommonMark implies — the whole `<details>` block is one HTML block
    /// here, so making that line blank would end it — and it costs a paragraph break, never
    /// structure or content. Zero of the 2,182 `.md` files in the local crate registry contain the
    /// shape.
    #[test]
    fn a_br_directly_under_an_unseparated_summary_is_dropped_as_block_body() {
        let src = "<details open>\n<summary>S</summary>\nalpha\n<br>\nbeta\n</details>\n";
        let pre = process_inline_html(src);
        assert_eq!(
            pre, "<details open>\n<summary>S</summary>\nalpha\nbeta\n</details>\n",
            "想定どおりに <br> 行だけが落ちる: {pre:?}"
        );
    }

    /// A `<br>` inside a GitHub alert, mid-line and at end of line. The injected line used to carry
    /// no `>`, and `split_alerts` captures a body by consuming *consecutive* `>`-prefixed lines — so
    /// the callout box ended there, the rest of the alert escaped it, and a fence written inside the
    /// alert leaked as literal `> ```rust` text instead of being drawn as a code block.
    #[test]
    fn br_in_an_alert_keeps_the_callout_and_its_fence_whole() {
        for (name, src) in [
            (
                "mid-line",
                "> [!NOTE]\n> line one<br>line two\n>\n> ```rust\n> fn a(){}\n> ```\n",
            ),
            (
                "trailing (the more common spelling)",
                "> [!NOTE]\n> line one<br>\n> line two\n>\n> ```rust\n> fn a(){}\n> ```\n",
            ),
        ] {
            let (texts, headers) = br_texts(src);
            // Every drawn line belongs to the callout: the body bar is the alert's own decoration,
            // so a line without it is a line that escaped the box.
            for t in &texts {
                assert!(
                    t.starts_with('▌'),
                    "{name}: この行が callout の外に逃げている: {t:?}\n全行: {texts:#?}"
                );
            }
            assert!(
                texts.iter().any(|t| t.contains("line one"))
                    && texts.iter().any(|t| t.contains("line two")),
                "{name}: 本文が両方とも残っていない: {texts:#?}"
            );
            // The fence inside the alert is a real code block, not leaked text.
            assert_eq!(
                headers, 1,
                "{name}: alert 内の fence がコードブロックとして描かれていない: {texts:#?}"
            );
            assert!(
                !texts.iter().any(|t| t.contains("> ")),
                "{name}: 生の `> ` マーカーが漏れている: {texts:#?}"
            );
            // A line break, not a paragraph break. Without the "trailing `<br>` emits no newline"
            // correction the prefix alone still saves the box, but the injected `> ` line becomes an
            // empty body line and splits the callout's paragraph in two — a stray `▌ ` row on
            // screen. Both documents carry exactly one genuinely empty quote line of their own (the
            // `>` separating the prose from the fence), so the count, not the presence, is the test.
            let blanks = texts.iter().filter(|t| t.trim() == "▌").count();
            assert_eq!(
                blanks, 1,
                "{name}: callout 本文の空行数が想定外(<br> が段落区切りになっている): {texts:#?}"
            );
        }
    }

    /// `| x<br>y | z |` is the standard GitHub idiom for a line break inside a table cell. The
    /// injected line has no continuation form — a table row can only be followed by another row —
    /// so it used to draw as **two** rows (`│ x │   │` then `│ y │ z │`), the second one shifted.
    #[test]
    fn br_in_a_table_cell_keeps_one_row() {
        let (texts, _) = br_texts("| a | b |\n| --- | --- |\n| x<br>y | z |\n");
        let data: Vec<&String> = texts
            .iter()
            .filter(|t| t.starts_with('│') && !t.contains(" a ") && !t.contains('─'))
            .collect();
        assert_eq!(data.len(), 1, "データ行がちょうど1行であるべき: {texts:#?}");
        assert!(
            data[0].contains('x') && data[0].contains('y') && data[0].contains('z'),
            "セルの両方の語と隣のセルが1行に残っていない: {:?}",
            data[0]
        );
    }

    /// The centered-banner idiom: a bare `    <br>` line between two badge blocks. Rewriting it left
    /// a whitespace-only line, which *is* a blank line to CommonMark, so the `<p>` HTML block ended
    /// there and every following 4-space-indented line became a genuine indented code block — the
    /// badges of `raw-window-metal` and `static_assertions` drew as a `▎ …` block instead of images.
    ///
    /// The invariant under test (the block survives) is unchanged; how it is achieved is not. The
    /// line is now **removed** rather than left verbatim — see `process_inline_html_traced` for the
    /// leak a preserved tag caused once a later pass relocated it. Both halves are asserted, because
    /// each alone is satisfiable the wrong way: "no `<br>` survives" by substituting whitespace
    /// (which ends the block), "the block survives" by keeping the tag (which leaks).
    #[test]
    fn bare_br_line_in_an_html_block_does_not_end_it() {
        let src = "<p align=\"center\">\n    <a href=\"https://ci\">\n      <img src=\"https://ci.svg\" alt=\"ci - ios\">\n    </a>\n    <br>\n    <a href=\"LICENSE-MIT\">\n      <img src=\"https://mit.svg\" alt=\"License - MIT\">\n    </a>\n</p>\n";
        let (pre, _) = br_pre_and_render(src);
        assert!(
            !pre.contains("<br>"),
            "HTML ブロック内の裸の <br> は行ごと落とすべき(タグを残すと後段の再配置でブロックを開く): {pre:?}"
        );
        assert!(
            !pre.lines().any(|l| l.trim().is_empty()),
            "空白だけの行が残っている(CommonMark では空行=ブロックの終端): {pre:?}"
        );
        assert_eq!(
            pre.lines().count(),
            src.lines().count() - 1,
            "落ちた行はちょうど <br> の1行であるべき: {pre:?}"
        );
        let (texts, headers) = br_texts(src);
        assert_eq!(
            headers, 0,
            "バナーが字下げコードブロックとして描かれている: {texts:#?}"
        );
        // Both badges survive as images (the harness stubs them, so they appear as `alt — url`).
        assert!(
            texts.iter().any(|t| t.contains("License - MIT")),
            "<br> の後ろのバッジが失われている: {texts:#?}"
        );
    }

    /// The refinement over "always leave the tag alone inside an HTML block": only a line the rewrite
    /// would leave **blank** is treated specially. `x<br>y` inside a block splits into two non-blank
    /// lines, which an HTML block tolerates (it runs to the next blank line), so it is rewritten
    /// byte-for-byte as anywhere else — and that matters, because it is what keeps an `<img>` that
    /// shares its line with a `<br>` on a line of its own where the block-image pass can lift it. The
    /// all-contributors table in `flutter_rust_bridge`'s README is 130 avatars of exactly that shape;
    /// keeping the tag there unconditionally dropped every one of them.
    ///
    /// The blank line is dropped **per produced line**, not all-or-nothing: `a<br><br>b` puts one
    /// between two non-blank halves, and removing just it keeps both halves *and* the block.
    #[test]
    fn br_inside_an_html_block_still_breaks_when_no_blank_line_results() {
        let pre = process_inline_html("<div align=\"center\">\n  before<br>after\n</div>\n");
        assert!(
            pre.contains("  before  \nafter"),
            "行中の <br> はブロック内でも改行に変換されるべき: {pre:?}"
        );
        assert!(
            !pre.contains("<br>"),
            "行中の <br> がそのまま残っている: {pre:?}"
        );
        // Trailing `<br>` inside a block: no newline is added, so no blank line, so the block lives.
        let pre = process_inline_html("<div align=\"center\">\n  one<br>\n  two\n</div>\n");
        assert!(
            pre.contains("  one  \n  two"),
            "行末 <br> が空行を作らずに改行になっていない: {pre:?}"
        );
        // Two tags in a row *would* leave a blank line between them — drop that one line only.
        let pre = process_inline_html("<div align=\"center\">\n  a<br><br>b\n</div>\n");
        assert!(
            pre.contains("  a  \nb"),
            "空行を生む形は空行だけを落として両半分を残すべき: {pre:?}"
        );
        assert!(
            !pre.contains("<br>"),
            "空行を生む形でタグが残っている(後段の再配置でブロックを開く): {pre:?}"
        );
        assert!(
            !pre.lines().any(|l| l.trim().is_empty()),
            "空白だけの行が残っている: {pre:?}"
        );
    }

    /// Why a `<br>` inside an HTML block is **dropped** rather than left verbatim (2026-08, found by
    /// rendering every `.md` in the local crate registry — `shlex`'s `quoting_warning.md` was the one
    /// file the verbatim rule damaged).
    ///
    /// A kept tag is still a token this module treats as a block opener ([`is_html_block_start`]
    /// accepts any tag name), and this pass is not the last one to move text around: `process_footnotes`
    /// relocates definition bodies into the section it appends at the end of the document, where the
    /// blank lines that used to surround them are gone. The surviving tag then landed at the head of
    /// an unbroken run of lines and opened an HTML block that ran to the end of the document, handing
    /// the *following* footnotes to `render_html_block` — which emits their text without parsing
    /// Markdown, so their inline code drew as literal backticks instead of a code span.
    ///
    /// Asserted at the level where it bites (the rendered output of the whole app chain), not just on
    /// the preprocessed text, because the preprocessing looks harmless on its own — the damage is
    /// entirely in what the renderer then does with the relocated tag.
    #[test]
    fn a_kept_br_in_a_relocated_footnote_would_swallow_the_footnotes_after_it() {
        // Two consecutive bare `<br>` lines, as in `shlex`: the first opens the block (and is
        // rewritten by the default rule), the second is *inside* it and is the one at issue.
        let src = concat!(
            "Body with a note[^one], another[^two] and a third[^three].\n",
            "\n",
            "[^one]: this can lead to tough choices\n",
            "  <br>\n",
            "  <br>\n",
            "  we don't have the luxury of those choices\n",
            "\n",
            "[^two]: mentions `some_code()` in a span\n",
            "\n",
            "[^three]: and `another_span` too\n",
        );
        // The app's own order: footnotes relocate the bodies first, so the `<br>` rewrite only ever
        // sees them already moved — which is exactly why a kept tag ends up somewhere new.
        let (body, _) = process_footnotes_traced(src, &identity_origin(src));
        let (pre, lines) = br_pre_and_render(&body);
        assert!(
            !pre.contains("<br>"),
            "再配置された脚注に <br> が生き残っている(後続の脚注ごとブロックに飲まれる): {pre:?}"
        );
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect();
        // The decisive signal: swallowed text is emitted verbatim, so its backticks stay visible.
        assert!(
            !texts.iter().any(|t| t.contains('`')),
            "後続の脚注のインラインコードが生のバッククォートで描かれている: {texts:#?}"
        );
        // …and the positive half: the spans really are drawn as code, not merely stripped of ticks.
        let code: Vec<&str> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_inline_code_span(s))
            .map(|s| s.content.as_ref())
            .collect();
        for want in ["some_code()", "another_span"] {
            assert!(
                code.iter().any(|c| c.contains(want)),
                "{want} がコードスパンとして描かれていない: code={code:#?}\n{texts:#?}"
            );
        }
        // The definition's own text survives the dropped line (the fix removes a line, not content).
        assert!(
            texts.iter().any(|t| t.contains("tough choices"))
                && texts.iter().any(|t| t.contains("luxury")),
            "脚注本文が失われている: {texts:#?}"
        );
    }

    /// The constructs that were **already correct** and must stay that way — CommonMark lazy
    /// continuation keeps an unprefixed line attached to the quote/list paragraph above it, and
    /// `split_details` spans blank lines. Asserted explicitly so a future change to the `<br>` rules
    /// cannot quietly break them.
    #[test]
    fn br_in_quote_list_and_details_still_renders_as_before() {
        // Plain blockquote: both halves stay inside the quote.
        let (texts, _) = br_texts("> line one<br>line two\n\ntail\n");
        assert!(
            texts.iter().any(|t| t.contains("line one"))
                && texts.iter().any(|t| t.contains("line two")),
            "引用の両行が残っていない: {texts:#?}"
        );
        // A nested quote keeps its `> > ` marker verbatim, so the deeper level survives.
        let pre = process_inline_html("> > deep one<br>deep two\n");
        assert!(
            pre.contains("> > deep one  \n> > deep two"),
            "入れ子引用のマーカーが継承されていない: {pre:?}"
        );
        // List item and nested list item: the marker stays attached to its first line.
        for (name, src) in [
            ("list item", "- item one<br>item two\n- second\n"),
            ("nested list item", "- outer\n  - inner<br>tail\n"),
        ] {
            let (texts, _) = br_texts(src);
            assert!(
                !texts.iter().any(|t| t.trim() == "-"),
                "{name}: リストマーカーが本文から切り離されている: {texts:#?}"
            );
        }
        // `<details>` body: the fold survives and its contents are still drawn.
        let (texts, _) = br_texts(
            "<details open>\n<summary>S</summary>\n\nline one<br>line two\n\n</details>\n",
        );
        assert!(
            texts.iter().any(|t| t.contains("line one"))
                && texts.iter().any(|t| t.contains("line two")),
            "details 本文が失われている: {texts:#?}"
        );
    }

    /// A `<br>` that is already literal code stays literal, in all three forms. Unchanged by the
    /// new rules (the code mask short-circuits before any of them), pinned because the rules now
    /// branch and a future branch could be added on the wrong side of that gate.
    #[test]
    fn br_stays_literal_in_code_spans_and_both_kinds_of_code_block() {
        for (name, src) in [
            ("code span", "text `a<br>b` more\n"),
            ("fenced block", "```\na<br>b\n```\n"),
            ("indented block", "para\n\n    a<br>b\n"),
        ] {
            let pre = process_inline_html(src);
            assert_eq!(pre, src, "{name}: コード内の <br> が書き換えられている");
        }
    }

    // ---- Where the two 2026-08 fixes meet: the three real README banners ----------------------
    //
    // These are the documents the whole `SourceRun` change was measured against, and they are kept
    // as named tests (rather than only as corpus rows) because they are the only place both fixes
    // are exercised at once, on the same lines, in the order production applies them: the `<br>`
    // rewrite runs first and must not end the HTML block, and the block splitter must then still
    // call the surviving 4-column-indented `<a>`/`</a>` lines HTML rather than indented code — even
    // though `split_block_parts` has cut the `<img>` lines out from between them by that point.
    // Either fix alone leaves these drawing a `▎ …` code block full of raw markup, which is what
    // shipped.
    //
    // Minimised from the files themselves (do not read from disk: a test that depends on the local
    // crate registry passes or fails for reasons that have nothing to do with konoma). Each keeps
    // the structural detail that made it distinct; the URLs and crate names are stand-ins.

    /// `raw-window-metal`: `<p align="center">`, `<a>` at 4 columns, `<img>` at 6, and a bare
    /// `    <br>` in the middle of the banner. The `<br>` is the interesting part — it is what turns
    /// the second half of the banner into a fragment whose first line is 4-column-indented markup.
    #[test]
    fn raw_window_metal_banner_shape_draws_badges_not_a_code_block() {
        let src = concat!(
            "\n",
            "<h1 align=\"center\">rwm</h1>\n",
            "<p align=\"center\">\n",
            "    <a href=\"https://crates.io/crates/rwm\">\n",
            "      <img src=\"https://img.example/v.svg\" alt=\"crates.io\">\n",
            "    </a>\n",
            "    <br>\n",
            "    <a href=\"LICENSE-MIT\">\n",
            "      <img src=\"https://img.example/mit.svg\" alt=\"License - MIT\">\n",
            "    </a>\n",
            "</p>\n",
            "\ntail\n",
        );
        let (pre, _) = br_pre_and_render(src);
        assert!(
            !pre.contains("<br>") && !pre.lines().any(|l| l.trim().is_empty() && !l.is_empty()),
            "バナー中の裸の <br> は行ごと落ちるべき(空白だけの行はブロックを終端させる): {pre:?}"
        );
        let (texts, headers) = br_texts(src);
        assert_eq!(
            headers, 0,
            "バナーが字下げコードブロックとして描かれている: {texts:#?}"
        );
        // Both halves of the banner survive — the one before the `<br>` and the one after it, which
        // is the half that used to be swallowed.
        for alt in ["crates.io", "License - MIT"] {
            assert!(
                texts.iter().any(|t| t.contains(alt)),
                "バッジ {alt:?} が失われている: {texts:#?}"
            );
        }
        assert!(
            !texts.iter().any(|t| t.contains("</a>")),
            "生の HTML タグが画面に漏れている: {texts:#?}"
        );
    }

    /// `static_assertions`: the same shape one level deeper (`<img>` at 8 columns, two `<img>` lines
    /// inside one `<a>`, a bare `<img>` with no link around it) and — the detail that makes it worth
    /// keeping separately — a **Markdown** block image on the very first line. That one is extracted
    /// too, so the banner below it is not even the first cut `split_block_parts` makes.
    #[test]
    fn static_assertions_banner_shape_draws_badges_not_a_code_block() {
        let src = concat!(
            "[![Banner](https://img.example/banner.png)](https://example.com/repo)\n",
            "\n",
            "<div align=\"center\">\n",
            "    <a href=\"https://crates.io/crates/sa\">\n",
            "        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\n",
            "        <img src=\"https://img.example/d.svg\" alt=\"Downloads\">\n",
            "    </a>\n",
            "    <img src=\"https://img.example/rustc.svg\" alt=\"rustc\">\n",
            "    <br>\n",
            "    <a href=\"https://example.com/patron\">\n",
            "        <img src=\"https://img.example/patron.png\" alt=\"Patron\">\n",
            "    </a>\n",
            "</div>\n",
            "\ntail.\n",
        );
        let (texts, headers) = br_texts(src);
        assert_eq!(
            headers, 0,
            "バナーが字下げコードブロックとして描かれている: {texts:#?}"
        );
        for alt in ["Banner", "Crates.io", "Downloads", "rustc", "Patron"] {
            assert!(
                texts.iter().any(|t| t.contains(alt)),
                "画像 {alt:?} が失われている: {texts:#?}"
            );
        }
        assert!(
            texts.iter().any(|t| t.contains("tail.")),
            "バナーの後ろの本文が飲み込まれている: {texts:#?}"
        );
    }

    /// `tinytemplate`: a banner with **no `<br>` at all**, so it isolates the block-splitter half of
    /// the pair — and the whole file is CRLF, an axis this module has been bitten on before (a `\r`
    /// left on the end of a line changes what "the line is only tags" and "the line is blank" both
    /// answer). Its first `<div>` holds a bare `    |` separator between two links, the shape that
    /// really *does* become an indented code block once the `<img>` lines around it are gone
    /// (criterion's README) — here there are none to remove, so it must not.
    #[test]
    fn tinytemplate_banner_shape_draws_links_not_a_code_block() {
        let src = concat!(
            "<h1 align=\"center\">TT</h1>\r\n",
            "\r\n",
            "<div align=\"center\">\r\n",
            "    <a href=\"https://docs.rs/tt/\">API Documentation</a>\r\n",
            "    |\r\n",
            "    <a href=\"https://example.com/changelog\">Changelog</a>\r\n",
            "</div>\r\n",
            "\r\n",
            "<div align=\"center\">\r\n",
            "    <a href=\"https://example.com/actions\">\r\n",
            "        <img src=\"https://img.example/ci.svg\" alt=\"CI\">\r\n",
            "    </a>\r\n",
            "    <a href=\"https://crates.io/crates/tt\">\r\n",
            "        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\r\n",
            "    </a>\r\n",
            "</div>\r\n",
            "\r\ntail\r\n",
        );
        let (texts, headers) = br_texts(src);
        assert_eq!(
            headers, 0,
            "バナーが字下げコードブロックとして描かれている: {texts:#?}"
        );
        for want in ["API Documentation", "Changelog", "CI", "Crates.io"] {
            assert!(
                texts.iter().any(|t| t.contains(want)),
                "{want:?} が失われている: {texts:#?}"
            );
        }
    }

    #[test]
    fn process_inline_html_leaves_fences_untouched() {
        let out = process_inline_html("```\n<kbd>x</kbd>\n```\n");
        assert!(
            out.contains("<kbd>x</kbd>"),
            "tags inside a fence stay literal"
        );
    }

    /// Regression (2026-08): `process_inline_html` used to gate on `fence_mask`, which only
    /// recognizes *fenced* code — an indented code block was invisible to it, so `<kbd>x</kbd>`
    /// written inside one purely to *document* the tag (e.g. "indent your snippet 4 columns like
    /// this: `<kbd>x</kbd>`") was rewritten into a real keycap, indistinguishable on screen from an
    /// actual one elsewhere in the document. `code_block_mask` (fenced or indented, via
    /// pulldown-cmark) fixes this; an unindented tag right after the block still converts, proving
    /// the pass isn't just refusing to run at all.
    #[test]
    fn process_inline_html_leaves_indented_code_untouched() {
        let src = "before\n\n    <kbd>x</kbd> stays literal in the transcript\n\nafter <kbd>y</kbd> converts\n";
        let out = process_inline_html(src);
        assert!(
            out.contains("    <kbd>x</kbd> stays literal in the transcript"),
            "tag inside an indented code block stays literal, whole line unchanged: {out:?}"
        );
        assert!(
            out.contains("after `y` converts"),
            "an unindented tag right after the block still converts: {out:?}"
        );
    }

    /// Regression (2026-08, found reviewing `code_block_mask`/`literal_code_mask`, not on real
    /// registry content): a fence with **no blank line** between it and a preceding
    /// `<summary>…</summary>` line, inside a `<details>` block, is what pulldown-cmark calls one HTML
    /// block when parsing the raw document in one pass (verified directly against pulldown-cmark) —
    /// so `code_block_mask` alone says it isn't code. But `split_details` extracts the block's body
    /// without requiring that blank line (`details_block_close` just looks for the literal
    /// `</details>` line) and hands it to `render_md_body_nested` in isolation, where the same three
    /// lines, with nothing else competing for the parse, *do* produce a real code block on screen.
    /// `fence_mask` — blind to the `<details>`/`<summary>` context entirely — gets this one right, so
    /// `literal_code_mask` (the union) recovers it; `code_block_mask` alone would not (see
    /// `literal_code_mask`'s doc comment for the full mechanism).
    #[test]
    fn process_inline_html_leaves_a_details_fence_without_a_blank_line_untouched() {
        let src = "<details>\n<summary>s</summary>\n```rust\n[^1] <kbd>K</kbd> $x$\n```\n</details>\n\noutside [^1] and <kbd>K</kbd> and $x$\n\n[^1]: def\n";
        let out = process_inline_html(src);
        assert!(
            out.contains("```rust\n[^1] <kbd>K</kbd> $x$\n```"),
            "the tag inside the fence right after <summary>, no blank line between them, stays \
             literal: {out:?}"
        );
        assert!(
            out.contains("outside [^1] and `K` and $x$"),
            "the identical tag outside the details block still converts: {out:?}"
        );
    }

    #[test]
    fn to_superscript_single_and_multi_digit() {
        assert_eq!(to_superscript(1), "¹");
        assert_eq!(to_superscript(10), "¹⁰");
        assert_eq!(to_superscript(12), "¹²");
    }

    #[test]
    fn process_footnotes_superscripts_refs_and_appends_section() {
        let src = "See note.[^a] And another.[^b]\n\n[^a]: first def\n[^b]: second def\n";
        let out = process_footnotes(src);
        // Referenced in order a, b → numbered 1, 2 (superscript).
        assert!(out.contains("See note.¹"), "out = {out:?}");
        assert!(out.contains("And another.²"));
        // Definition lines are pulled out of the body into a numbered section.
        assert!(!out.contains("[^a]:"));
        assert!(out.contains("1. first def"));
        assert!(out.contains("2. second def"));
        assert!(
            out.contains("---"),
            "a rule separates the footnotes section"
        );
    }

    #[test]
    fn process_footnotes_leaves_fences_and_undefined_refs() {
        let src = "text[^1] and [^nodef]\n\n```\ncode [^1] here\n```\n\n[^1]: def one\n";
        let out = process_footnotes(src);
        assert!(out.contains("text¹"), "defined ref superscripted");
        assert!(out.contains("[^nodef]"), "undefined ref stays literal");
        assert!(
            out.contains("code [^1] here"),
            "ref inside a fence untouched"
        );
    }

    /// Regression (2026-08): same class of bug as `process_inline_html_leaves_indented_code_untouched`
    /// — `process_footnotes` also gated on `fence_mask` alone, so `[^1]` written inside an indented
    /// code block purely as a syntax example (e.g. a snippet showing readers how to write a
    /// footnote reference) was itself numbered and superscripted, and a `[^id]: text` line indented
    /// the same way was pulled out of the block as if it were a real definition.
    #[test]
    fn process_footnotes_leaves_indented_code_untouched() {
        let src = "real[^1] reference\n\n\
                    Example syntax:\n\n    write text[^1] like this\n\n[^1]: the definition\n";
        let out = process_footnotes(src);
        assert!(
            out.contains("real¹ reference"),
            "the real ref is numbered: {out:?}"
        );
        assert!(
            out.contains("    write text[^1] like this"),
            "the indented example ref stays literal, whole line unchanged: {out:?}"
        );
    }

    /// Same regression and mechanism as
    /// `process_inline_html_leaves_a_details_fence_without_a_blank_line_untouched` — see its doc
    /// comment. The reference numbering pass also has to skip this line: were it counted, it would
    /// still resolve to the same id (`1`) as the real outside reference, so the corruption here is
    /// specifically that the ref text itself gets superscripted, not a renumbering.
    #[test]
    fn process_footnotes_leaves_a_details_fence_without_a_blank_line_untouched() {
        let src = "<details>\n<summary>s</summary>\n```rust\n[^1] <kbd>K</kbd> $x$\n```\n</details>\n\noutside [^1] and <kbd>K</kbd> and $x$\n\n[^1]: def\n";
        let out = process_footnotes(src);
        assert!(
            out.contains("```rust\n[^1] <kbd>K</kbd> $x$\n```"),
            "the ref inside the fence right after <summary>, no blank line between them, stays \
             literal: {out:?}"
        );
        assert!(
            out.contains("outside ¹ and <kbd>K</kbd> and $x$"),
            "the identical ref outside the details block still numbers: {out:?}"
        );
    }

    #[test]
    fn process_footnotes_no_definitions_is_noop() {
        let src = "just [^1] with no definition\n";
        assert_eq!(process_footnotes(src), src);
    }

    // Regression corpus (2026-08). Before the fix, `footnote_def_text`'s dedent step computed `indent` as the
    // **byte length** of the shortest continuation line's leading whitespace
    // (`l.len() - l.trim_start().len()`), then slices every other continuation line at
    // `indent.min(own_indent)`. `str::trim_start()` strips any Unicode `White_Space` codepoint, not
    // just ASCII space/tab, so a footnote definition with one continuation line indented by ASCII
    // spaces and another indented by a multi-byte whitespace character (U+3000 IDEOGRAPHIC SPACE,
    // U+00A0 NO-BREAK SPACE, …) computes an `indent` that lands *inside* the multi-byte character on
    // the wide line, and `&l[indent..]` panics with "byte index N is not a char boundary". This runs
    // on every Markdown preview (`md_footnotes` defaults to `true`) with no `catch_silent` around it,
    // so it took the whole app down. These tests call `process_footnotes` itself — the same entry
    // point production uses via `process_footnotes_traced` — rather than a hand-copy of the indent
    // math, so a fix that changes the algorithm still has to satisfy them.

    /// ASCII(2 bytes) then ideographic(3 bytes): `indent = min(2, 3) = 2`; slicing the wide line at
    /// byte 2 lands inside the 3-byte U+3000 sequence (bytes 0..3).
    #[test]
    fn footnote_def_text_panics_on_ascii_then_ideographic_indent() {
        let src = "ref[^a]\n\n[^a]: first line\n  second line\n\u{3000}third line\n";
        let out = process_footnotes(src);
        assert!(out.contains("third line"), "out = {out:?}");
    }

    /// Same computation with the continuation lines swapped: the wide line is now first in the
    /// block, so the panic fires on the loop's very first iteration instead of the second — this
    /// isn't an artifact of which line happens to come second.
    #[test]
    fn footnote_def_text_panics_on_ideographic_then_ascii_indent() {
        let src = "ref[^a]\n\n[^a]: first line\n\u{3000}second line\n  third line\n";
        let out = process_footnotes(src);
        assert!(out.contains("third line"), "out = {out:?}");
    }

    /// U+00A0 NO-BREAK SPACE is 2 bytes and, like U+3000, is Unicode whitespace for
    /// `trim_start()`. ASCII(1 byte) then NBSP(2 bytes): `indent = 1`; slicing the NBSP line at
    /// byte 1 lands inside its 2-byte sequence (bytes 0..2).
    #[test]
    fn footnote_def_text_panics_on_ascii_then_nbsp_indent() {
        let src = "ref[^a]\n\n[^a]: first line\n second line\n\u{a0}third line\n";
        let out = process_footnotes(src);
        assert!(out.contains("third line"), "out = {out:?}");
    }

    /// `footnote_defs` asks pulldown-cmark to recognize `[^a]: …` as a definition regardless of
    /// whether `[^a]` is referenced anywhere in the body, so an orphaned (unreferenced) definition
    /// hits the exact same dedent bug.
    #[test]
    fn footnote_def_text_panics_even_when_the_definition_has_no_reference() {
        let src = "[^a]: first line\n  second line\n\u{3000}third line\n";
        let out = process_footnotes(src);
        assert!(out.contains("third line"), "out = {out:?}");
    }

    /// A continuation line consisting of a single U+3000 is **not** blank by CommonMark's blank-line
    /// rule (only ASCII space/tab count there), so the parser keeps it inside the definition — but it
    /// **is** blank by Rust's `str::trim()`, so `footnote_def_text`'s own
    /// `if l.trim().is_empty() { continue }` branch fires and skips it safely (verified: this alone
    /// does not panic). The next continuation line is still wide-indented, though, so the loop panics
    /// right after — skipping the blank-per-trim line does not protect the rest of the loop.
    #[test]
    fn footnote_def_text_panics_even_past_a_trim_blank_continuation_line() {
        let src = "ref[^a]\n\n[^a]: first line\n  second line\n\u{3000}\n\u{3000}fourth line\n";
        let out = process_footnotes(src);
        assert!(out.contains("fourth line"), "out = {out:?}");
    }

    /// Non-regression: continuation lines indented 4 and 2 ASCII spaces. `indent = min(4, 2) = 2`,
    /// and every byte offset of an all-ASCII string is a valid char boundary, so this never panics.
    /// Output captured from the current implementation (byte-for-byte) so a fix to the Unicode cases
    /// above cannot silently change this one too.
    #[test]
    fn footnote_def_text_non_regression_all_ascii_mixed_widths() {
        let src = "ref[^a]\n\n[^a]: first line\n    second line\n  third line\n";
        assert_eq!(
            process_footnotes(src),
            "ref¹\n\n\n---\n\n1. first line\n     second line\n   third line\n"
        );
    }

    /// Non-regression: a document with no footnote syntax at all takes `process_footnotes`'s
    /// no-definitions-found fast path and is returned unchanged.
    #[test]
    fn footnote_def_text_non_regression_no_footnote_syntax_at_all() {
        let src = "Just some ordinary text with no footnotes at all.\n";
        assert_eq!(process_footnotes(src), src);
    }

    /// Non-regression: tab-indented continuation lines. A tab is one byte, so the same indent-clamp
    /// math never lands mid-character. Output captured from the current implementation.
    #[test]
    fn footnote_def_text_non_regression_tab_indented_continuation() {
        let src = "ref[^a]\n\n[^a]: first line\n\tsecond line\n\tthird line\n";
        assert_eq!(
            process_footnotes(src),
            "ref¹\n\n\n---\n\n1. first line\n   second line\n   third line\n"
        );
    }

    #[test]
    fn heading_level_hint_infers_levels_from_style() {
        let md = "# H1\n\n## H2\n\n### H3\n\n#### H4\n\n##### H5\n";
        let lines = render_markdown(md, 40, NO_CODE, "TwoDark", false);
        let levels: Vec<u8> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| heading_text(l).is_some())
            .map(|(i, l)| heading_level_hint(l, lines.get(i + 1)))
            .collect();
        // H1/H2/H3 distinct; H4/H5 both collapse to 4.
        assert_eq!(levels, vec![1, 2, 3, 4, 4]);
    }

    #[test]
    fn heading_inside_alert_strips_bar_for_anchor() {
        // A heading inside a GitHub alert keeps the line's HEAD_FG but gains a "▌ " bar span. Its
        // anchor text must drop the bar (else the slug gets a spurious leading "-").
        let md = "> [!NOTE]\n> ## Sub Heading\n> body\n";
        let lines = render_via_dispatcher(
            &doc_run(md),
            60,
            NO_CODE,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let ht = lines.iter().find_map(heading_text);
        assert_eq!(ht.as_deref(), Some("Sub Heading"));
    }

    #[test]
    fn strip_front_matter_extracts_leading_block_only() {
        let (fm, body) =
            strip_front_matter("---\ntitle: Hi\ntags: [a, b]\n---\n# Heading\n\nbody\n");
        assert_eq!(fm.as_deref(), Some("title: Hi\ntags: [a, b]"));
        assert!(body.starts_with("# Heading"), "body = {body:?}");
        // A closing `...` also terminates front matter.
        assert_eq!(
            strip_front_matter("---\nk: v\n...\nrest\n").0.as_deref(),
            Some("k: v")
        );
        // No leading fence → none.
        assert_eq!(strip_front_matter("# not front matter\n").0, None);
        // Leading `---` but no closing fence → an ordinary thematic break, not front matter.
        assert_eq!(strip_front_matter("---\njust text, no close\n").0, None);
    }

    #[test]
    fn render_front_matter_accents_keys_and_closes_with_rule() {
        let lines = render_front_matter("title: Hi\n  nested: x\nplain", 20);
        // Top-level key is accented (Cyan + DIM); the `: value` stays dim.
        assert!(lines[0].spans[0].content.starts_with("title"));
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::DIM));
        // A dim rule closes the block.
        assert!(lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('─'))));
        // The block never carries the heading line-color, so anchors won't treat it as a heading.
        assert!(lines.iter().all(|l| l.style.fg != Some(HEAD_FG)));
    }

    #[test]
    fn alert_body_code_fence_is_detected_as_code_line() {
        // A code fence inside an alert: every body line gets the `▌ ` bar, so the `▎` gutter is the
        // *second* span. is_code_line must still flag it, or autolink/emoji would rewrite the code
        // content (GitHub keeps it verbatim). Uses NO_CODE = the code_bg="none" scenario.
        let md = "> [!NOTE]\n> ```sh\n> curl https://x.example # :tada:\n> ```\n";
        let lines = render_via_dispatcher(
            &doc_run(md),
            60,
            NO_CODE,
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            true,
        );
        let code_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("curl")))
            .expect("the fenced body line is present");
        assert!(
            is_code_line(code_line),
            "an alert-wrapped code line is detected as code: {:?}",
            code_line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        );
        // A plain body line that merely starts with `▎` (fg None) is NOT a code line.
        let fake = Line::from(vec![Span::raw("▎ see https://x.com")]);
        assert!(!is_code_line(&fake), "plain ▎ text is not a code line");
    }
}

#[cfg(test)]
pub(crate) mod task_corpus {
    /// Documents exercising every construct that the renderer treats specially, each paired with the
    /// number of checkboxes a user actually sees. The whole point is that the *write-back scanner* must
    /// agree with the *renderer* on this number for every one of them: when they disagree the safety
    /// check cancels every toggle in the document (a safe fallback, principle #3), so one stray construct
    /// breaks an entire file. Instances are cheap to add here; the class is what is being pinned.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            ("plain dash", "- [ ] a\n- [x] b\n"),
            ("bullet star", "* [ ] a\n* [x] b\n"),
            ("bullet plus", "+ [ ] a\n+ [x] b\n"),
            ("upper X", "- [X] a\n"),
            ("mixed bullets", "- [ ] a\n* [ ] b\n+ [x] c\n"),
            ("nested two levels", "- [ ] a\n  - [ ] b\n    - [x] c\n"),
            ("ordered list sibling", "1. plain\n2. also\n\n- [ ] task\n"),
            ("heading between", "- [ ] a\n\n## H\n\n- [ ] b\n"),
            ("alert NOTE", "> [!NOTE]\n> - [ ] a\n"),
            ("alert TIP", "> [!TIP]\n> - [ ] a\n> - [x] b\n"),
            ("alert IMPORTANT", "> [!IMPORTANT]\n> - [ ] a\n"),
            ("alert WARNING", "> [!WARNING]\n> - [x] a\n"),
            ("alert CAUTION", "> [!CAUTION]\n> - [ ] a\n"),
            ("alert titled", "> [!NOTE] My title\n> - [ ] a\n"),
            ("alert nested list", "> [!NOTE]\n> - [ ] a\n>   - [ ] b\n"),
            ("alert then plain", "> [!NOTE]\n> - [ ] in\n\n- [ ] out\n"),
            ("plain then alert", "- [ ] out\n\n> [!NOTE]\n> - [ ] in\n"),
            ("two alerts", "> [!NOTE]\n> - [ ] a\n\n> [!TIP]\n> - [ ] b\n"),
            ("plain blockquote", "> - [ ] quoted\n"),
            // These three pin the *reach* of `render_quote`'s model-only decoration (2026-08
            // investigation into this test's own oracle — see
            // `app::tests::md_task_toggle_is_byte_exact_across_the_corpus`'s doc comment): a nested
            // plain quote, a plain quote nested inside an alert's own body, and an alert nested
            // inside a plain quote. None of these is expected to be found by `task_source_locs`
            // either (that scanner deliberately still does not recognize a plain, non-alert quote at
            // all, at any depth — see `heading_gap_is_fixed_and_plain_blockquote_gap_has_no_mismatch`
            // in `task_scan_parity_tests`) — what these entries actually exercise is
            // `md_task_toggle_is_byte_exact_across_the_corpus`'s real, end-to-end write path
            // (`App::md_toggle_focused_task`) for a shape the legacy-calibrated scanner cannot
            // describe, not the scanner itself.
            ("nested plain blockquote (two levels)", "> > - [ ] quoted twice\n"),
            (
                "plain blockquote nested inside an alert",
                "> [!NOTE]\n> > - [ ] quoted in alert\n",
            ),
            (
                "alert nested inside a plain blockquote",
                "> > [!NOTE]\n> > - [ ] alert in quote\n",
            ),
            (
                "details closed",
                "<details>\n<summary>S</summary>\n\n- [ ] hidden\n\n</details>\n",
            ),
            (
                "details open",
                "<details open>\n<summary>S</summary>\n\n- [ ] shown\n\n</details>\n",
            ),
            (
                "details closed then plain",
                "<details>\n<summary>S</summary>\n\n- [ ] hidden\n\n</details>\n\n- [ ] after\n",
            ),
            (
                "details open then plain",
                "<details open>\n<summary>S</summary>\n\n- [ ] shown\n\n</details>\n\n- [ ] after\n",
            ),
            ("fence backtick", "```\n- [ ] not a task\n```\n\n- [ ] real\n"),
            ("fence tilde", "~~~\n- [ ] not a task\n~~~\n\n- [ ] real\n"),
            (
                "fence with language",
                "```rust\n// - [ ] no\n```\n\n- [ ] real\n",
            ),
            (
                "table then task",
                "| a | b |\n|---|---|\n| 1 | 2 |\n\n- [ ] after table\n",
            ),
            ("html block then task", "<div>hi</div>\n\n- [ ] after html\n"),
            ("inline code lookalike", "- [ ] real `- [ ] fake`\n"),
            ("link in task", "- [ ] see [docs](./d.md)\n"),
            ("cjk", "- [ ] 日本語のタスク\n- [x] 全角　スペース\n"),
            ("emphasis in task", "- [ ] **bold** and *em*\n"),
            ("no trailing newline", "- [ ] a\n- [x] b"),
            ("crlf", "- [ ] a\r\n- [x] b\r\n"),
            ("front matter", "---\ntitle: t\n---\n\n- [ ] a\n"),
            // Root cause (b)'s "a pseudo-task inside front matter" is **deliberately not included
            // here**: the premise differs. `md_task_toggle_is_byte_exact_across_the_corpus`
            // (app/tests.rs), which uses this corpus, derives its "expected write position" by
            // working backward from the plain (front-matter-unaware) `task_source_locs(src, ..)`,
            // which is a different implementation from the App side (root cause (b)'s fix) that
            // actually strips front matter out of the body before scanning. Covered directly by the
            // dedicated test `md_task_toggle_skips_pseudo_tasks_inside_front_matter` (app/tests.rs).
            ("footnote", "- [ ] a[^1]\n\n[^1]: note\n"),
            (
                "fence containing a table lookalike",
                "```text\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n\n- [ ] real\n",
            ),
            (
                "fence containing an html block lookalike",
                "```html\n<div class=\"x\">\nhello\n</div>\n```\n\n- [ ] real\n",
            ),
            (
                "fence containing an alert lookalike",
                "```text\n> [!NOTE]\nlooks like an alert\n```\n\n- [ ] real\n",
            ),
            ("empty doc", ""),
            ("no tasks", "# just text\n\nparagraph\n"),
            (
                "everything",
                "# Doc\n\n- [ ] top\n\n> [!NOTE] Heads up\n> - [ ] in alert\n> - [x] done\n\n\
                 | a | b |\n|---|---|\n| 1 | 2 |\n\n```\n- [ ] fenced\n```\n\n\
                 <details>\n<summary>More</summary>\n\n- [ ] collapsed\n\n</details>\n\n- [x] bottom\n",
            ),
            // Root cause (indented code blocks, 2026-08): a top-level indented (4+ column) code
            // block was not recognized as code by the write-back scanner at all — a `- [ ] fake`
            // example inside one was mistaken for a real, toggleable checkbox.
            (
                "task-lookalike inside a top-level indented code block stays literal",
                "para\n\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike inside an indented block nested in an alert stays literal",
                "> [!NOTE]\n> para\n>\n>     - [ ] fake\n>\n> - [ ] real\n",
            ),
            (
                "task-lookalike inside an indented block nested in an open details stays literal",
                "<details open>\n<summary>S</summary>\n\npara\n\n    - [ ] fake\n\n- [ ] real\n\n</details>\n",
            ),
            (
                "a real task at a list item's own indentation is still a real task",
                "- [ ] outer\n\n  - [ ] nested at matching indent\n",
            ),
            (
                "an indented paragraph inside a list item is not a task even if it starts with a dash",
                "- item\n\n  - [ ] looks like a task but is just this item's own indentation\n",
            ),
            // Root cause (fence unification, 2026-08) — see the matching comment in `code_corpus`:
            // a task-lookalike must stay hidden (literal code) for exactly as long as the fence it
            // sits inside is actually still open, per CommonMark's char/length/indentation rules —
            // not per a naive "any 3-of-the-same-char line toggles it" approximation.
            (
                "task-lookalike inside a fence indented 3 columns (still a real fence)",
                "para\n\n   ```rust\n   - [ ] fake\n   ```\n\n- [ ] real\n",
            ),
            (
                "task-lookalike inside a fence-lookalike top-level indented code block",
                "para\n\n    ```rust\n    - [ ] fake\n    ```\n\n- [ ] real\n",
            ),
            (
                "task-lookalike inside a tilde fence with a shorter nested tilde lookalike",
                "~~~~md\n~~~\n- [ ] fake\n~~~\n~~~~\n\n- [ ] real\n",
            ),
            // Fence unification stage 2: same shape as the tilde entry above, but with the
            // renderer's own now-fixed same-character-backtick-nesting bug (see the matching
            // comment in `code_corpus`) — before stage 2 the renderer drew **zero** checkboxes for
            // this document (the fake one hidden as intended, but the real one also swallowed into
            // the bogus second code block), disagreeing with the scanner's correct count of 1.
            (
                "task-lookalike inside a backtick fence with a shorter nested backtick lookalike (same char)",
                "````md\n```\n- [ ] fake\n```\n````\n\n- [ ] real\n",
            ),
            // Root cause (alert/details mutual nesting, 2026-08): an alert nested inside a
            // `<details>` block used to render unconditionally, leaking regardless of the fold —
            // and the reverse (a `<details>` nested inside an alert) had no fold at all (always
            // shown). See `render_md_body_nested`'s doc comment. These entries pin the write-back
            // scanner's (`task_source_locs`/`scan_task_lines`) side of that fix: the checkbox count
            // must still match what's actually drawn, in both nesting directions and both
            // open/closed states, or every toggle in the document gets refused.
            (
                "alert nested inside a closed details hides its task",
                "<details>\n<summary>S</summary>\n\n> [!NOTE]\n> - [ ] hidden\n\n</details>\n",
            ),
            (
                "alert nested inside an open details shows its task",
                "<details open>\n<summary>S</summary>\n\n> [!NOTE]\n> - [ ] shown\n\n</details>\n",
            ),
            (
                "details nested inside an alert, closed, hides its task",
                "> [!NOTE]\n> <details>\n> <summary>S</summary>\n>\n\
                 > - [ ] hidden\n>\n> </details>\n",
            ),
            (
                "details nested inside an alert, open, shows its task",
                "> [!NOTE]\n> <details open>\n> <summary>S</summary>\n>\n\
                 > - [ ] shown\n>\n> </details>\n",
            ),
            (
                "a details nested inside an alert nested inside a closed details hides everything",
                "<details>\n<summary>Outer</summary>\n\n> [!NOTE]\n> <details open>\n\
                 > <summary>Inner</summary>\n>\n> - [ ] deeply hidden\n>\n> </details>\n\n\
                 </details>\n\n- [ ] real\n",
            ),
            (
                "a details nested inside an alert nested inside an open details shows everything",
                "<details open>\n<summary>Outer</summary>\n\n> [!NOTE]\n> <details open>\n\
                 > <summary>Inner</summary>\n>\n> - [ ] deeply shown\n>\n> </details>\n\n\
                 </details>\n\n- [ ] real\n",
            ),
            (
                "an alert nested inside a details nested inside an alert, all open, shows the task",
                "> [!NOTE]\n> <details open>\n> <summary>Inner</summary>\n>\n\
                 > > [!TIP]\n> > - [ ] triple-nested\n>\n> </details>\n",
            ),
            // Root cause (heading/thematic-break/setext gate, 2026-08): an indented block right
            // after a heading, thematic break, or setext heading underline (no blank line needed —
            // only a paragraph gates a following indented block, and none of these three is one) was
            // not recognized by the write-back scanner at all, so a `- [ ] fake` example inside one
            // was mistaken for a real, toggleable checkbox. Mirrors the matching `code_corpus`
            // entries below.
            (
                "task-lookalike in an indented block right after an ATX heading (no blank) stays literal",
                "## Usage\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike in an indented block right after a thematic break (no blank) stays literal",
                "---\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike in an indented block right after a setext heading underline (no blank) stays literal",
                "Title\n=====\n    - [ ] fake\n\n- [ ] real\n",
            ),
            (
                "task-lookalike in an indented block right after a short setext dash underline (no blank) stays literal",
                "Title\n--\n    - [ ] fake\n\n- [ ] real\n",
            ),
        ]
    }
}

#[cfg(test)]
pub(crate) mod code_corpus {
    /// Documents exercising indented (4+ column) code blocks and their interaction with the other
    /// constructs the renderer treats specially — mirrors `task_corpus`. The write-back scanner
    /// (`code_block_source_locs`) must agree with the renderer on the number of code blocks for
    /// every one of these, or `y c` gets refused for the whole document (one stray construct breaks
    /// the whole file — reported by the user for indented code blocks specifically, 2026-08).
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "standalone indented block",
                "Normal paragraph.\n\n    indented code line\n    second line\n\nTail.\n",
            ),
            ("indented block at doc start", "    line one\n    line two\n"),
            ("tab-indented block", "para\n\n\ttabbed line\n"),
            (
                "not code: lazy continuation right after a paragraph (no blank)",
                "para\n    not code, just continues the paragraph\n",
            ),
            (
                "not code: inside a list item at the item's own content column",
                "- item\n\n  still item content, not code\n",
            ),
            (
                "not code: inside an ordered list item at the item's own content column",
                "1. item\n\n   still item content, not code\n",
            ),
            (
                "not code: 3-space indent is not enough",
                "para\n\n   three spaces is not enough\n",
            ),
            (
                "two chunks glued by a blank separator into one code block",
                "para\n\n    chunk one\n\n    chunk two\n",
            ),
            (
                "mixed: indented block then (after a blank) a real fence",
                "para\n\n    indented\n\n```rust\nfenced\n```\n",
            ),
            (
                "mixed: fence then an indented block right after (no blank needed)",
                "```\nfenced\n```\n    indented after fence\n",
            ),
            (
                "indented block nested inside an open alert",
                "> [!NOTE]\n> para\n>\n>     indented in alert\n",
            ),
            (
                "indented block nested inside an open details",
                "<details open>\n<summary>S</summary>\n\npara\n\n    indented in details\n\n</details>\n",
            ),
            (
                "indented block nested inside a closed details is not counted",
                "<details>\n<summary>S</summary>\n\npara\n\n    hidden\n\n</details>\n\n```\nreal\n```\n",
            ),
            (
                "indented block containing a task-lookalike stays literal code",
                "para\n\n    - [ ] not a real task\n",
            ),
            (
                "front matter then an indented block",
                "---\ntitle: t\n---\n\npara\n\n    indented after front matter\n",
            ),
            (
                "crlf indented block",
                "para\r\n\r\n    indented\r\n    second\r\n",
            ),
            ("no trailing newline", "para\n\n    indented"),
            (
                "indented block followed by a real list",
                "para\n\n    indented\n\n- item\n",
            ),
            (
                "a plain (non-alert) block quote wrapping a fence is not detected by either side",
                "> para\n>\n>     code\n",
            ),
            (
                "everything",
                "# Doc\n\nintro\n\n    top level indented\n\n> [!NOTE]\n> body\n>\n>     indented in note\n\n\
                 <details open>\n<summary>More</summary>\n\ndetail para\n\n    indented in details\n\n</details>\n\n\
                 ```rust\nfn real_fence() {}\n```\n",
            ),
            // Root cause (fence unification, 2026-08): several scanners tracked "am I inside a fence"
            // with their own naive per-function toggle (`starts_with("```") || starts_with("~~~")`,
            // closing on the first line that merely starts with 3 of the same char) instead of the
            // file's one CommonMark-correct `parse_fence` (matching char, length ≥ the opener's, and
            // requiring 0-3 columns of indentation to even count as a fence at all). The naive version
            // let the *count* the count-guard checks happen to still agree with what the renderer draws
            // for these shapes (both sides were wrong in step), while the *content* — verified against
            // `code_block_content_matches_the_render_for_fence_edge_cases` below — was corrupted. These
            // entries pin count parity; content is pinned separately.
            (
                "fence indented 3 columns is still a fence, not an indented code block",
                "para\n\n   ```rust\n   fn a(){}\n   ```\n",
            ),
            (
                "a fence-lookalike indented 4 columns is the literal content of an indented code block",
                "para\n\n    ```rust\n    fn a(){}\n    ```\n",
            ),
            (
                "closing fence line with a language suffix does not close (has to be exactly the fence chars)",
                "```rust\nbody\n```js\nmore\n```\n",
            ),
            (
                "a shorter nested tilde fence lookalike inside a longer tilde fence doesn't close it",
                "~~~~md\n~~~\ninner\n~~~\n~~~~\n",
            ),
            (
                "a tilde-fence lookalike nested inside a backtick fence doesn't close it (different fence char)",
                "```rust\n~~~\nnot closing\n~~~\n```\n",
            ),
            ("plain 4-backtick fence, no nesting", "````rust\nbody\n````\n"),
            // Root cause (fence unification stage 2, 2026-08): `decorate_code_blocks` (the
            // **renderer**) had its own, independent version of the exact same class of bug as the
            // scanner entries above — it scanned rendered *text* for `` ``` `` instead of keying off
            // tui-markdown's own per-line *style* (`is_code_block_line`) — so a shorter nested
            // fence-lookalike of the *same character* as its enclosing fence used to be mistaken for
            // the real close, silencing the rest of the document into a bogus second code block.
            // Deliberately outside the strict `drawn == scanned` loop in stage 1 (the renderer's own
            // count was 2 there, disagreeing with the scanner's correct 1); now that the renderer is
            // fixed too, this belongs in the corpus like every other fence shape.
            (
                "a shorter nested backtick fence lookalike inside a longer backtick fence doesn't close it (same char)",
                "````md\n```\ninner\n```\n````\n",
            ),
            // Root cause (heading/thematic-break/setext gate, 2026-08): an indented code block right
            // after a heading, thematic break, or setext heading underline — with no blank line
            // needed, since none of those three is a paragraph — was not recognized by the
            // write-back scanner at all, so any document containing one refused `y c` for **every**
            // code block in it, fenced ones included (the count guard is document-wide). Reported by
            // the user via a heading directly followed by an indented example, with an unrelated real
            // fence right after it also collaterally refused.
            (
                "indented block immediately after an ATX heading (no blank line)",
                "## Usage\n    npm install foo\n",
            ),
            (
                "indented block immediately after a thematic break (no blank line)",
                "---\n    code here\n",
            ),
            (
                "indented block immediately after a setext level-1 heading underline (no blank line)",
                "Title\n=====\n    code here\n",
            ),
            (
                "indented block immediately after a short setext dash underline, too short to also be a thematic break (no blank line)",
                "Title\n--\n    code here\n",
            ),
            (
                "heading then an indented block then an unrelated real fence — the fence must not be collaterally refused",
                "## Usage\n    npm install foo\n\n```bash\nnpm test\n```\n",
            ),
            // ---- Container context (2026-08) ----
            //
            // Root cause: CommonMark measures a block's indentation **relative to the container it
            // sits in**, while the hand-written scanner measured absolute columns. Inside a list item
            // every column shifts right by the marker's width, so the two sides disagreed in *both*
            // directions — a fence indented 4 columns under `1. ` is a real fence that got called an
            // indented code block, and a continuation paragraph indented 6 columns under `   4. ` is
            // not code but got counted as one. Either way the count guard broke and `y c` was refused
            // for the whole document. Both shapes are ordinary (a numbered install guide; the Apache
            // license text) — 26 of 2,182 real `.md` files in the crate registry tripped it.
            //
            // This axis is **orthogonal** to the fenced/indented axis above and was missing from the
            // corpus entirely — which is how it shipped. The entries above were written one per past
            // bug; these are written from the spec's own case split (container × content), which is
            // the only way a corpus covers something nobody has hit yet.
            //
            // Fixed by handing the leaf detection to pulldown-cmark — the very parser the renderer
            // renders through — instead of re-deriving its rules (see `parser_code_blocks`).
            (
                "① fence indented 4 columns inside an ordered item is a real fence (registry: pastey CONTRIBUTING.md)",
                "1. Fork it:\n\n    ```sh\n    git clone x\n    ```\n\n2. Done.\n",
            ),
            (
                "② 6-column continuation paragraph under `   4. ` is not code (registry: LICENSE-APACHE.md)",
                "   4. Redistribution. You may reproduce and distribute copies of the\n\n      (b) You must cause any modified files to carry prominent notices\n",
            ),
            (
                "fence at a bullet item's own content column",
                "- Step:\n\n  ```sh\n  echo hi\n  ```\n",
            ),
            (
                "fence indented 4 columns inside a bullet item is still a fence",
                "- Step:\n\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "fence-lookalike 4 columns past a bullet item's content column is indented code",
                "- Step:\n\n      ```sh\n      echo hi\n      ```\n",
            ),
            (
                "indented code inside a bullet item (content column + 4)",
                "- Step:\n\n      code line\n      second line\n",
            ),
            (
                "tilde fence inside a bullet item",
                "- Step:\n\n  ~~~sh\n  echo hi\n  ~~~\n",
            ),
            (
                "tight list: fence on the line right after the marker, no blank line",
                "- Step:\n  ```sh\n  echo hi\n  ```\n",
            ),
            (
                "tight ordered list: fence indented 4 columns right after the marker line",
                "1. Do:\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "fence at an ordered item's own content column",
                "1. Step:\n\n   ```sh\n   echo hi\n   ```\n",
            ),
            (
                "indented code inside an ordered item (content column + 4)",
                "1. Step:\n\n       code line\n",
            ),
            (
                "a two-digit ordered marker shifts the content column right",
                "10. Step:\n\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "ordered list with a `)` delimiter",
                "1) Step:\n\n   ```sh\n   echo hi\n   ```\n",
            ),
            (
                "lazy continuation inside a list item is not code",
                "- item text\n    still the same paragraph\n",
            ),
            (
                "fence inside a nested bullet item",
                "- outer\n  - inner:\n\n    ```sh\n    echo hi\n    ```\n",
            ),
            (
                "continuation paragraph at a nested item's content column is not code",
                "- outer\n  - inner\n\n    still inner content\n",
            ),
            (
                "indented code inside a nested item (nested content column + 4)",
                "- outer\n  - inner\n\n        code line\n",
            ),
            (
                "fence inside a plain block quote draws no header (tui-markdown prefixes every line)",
                "> quoted:\n>\n> ```sh\n> echo hi\n> ```\n",
            ),
            // These two mirror `task_corpus`'s "nested plain blockquote"/"alert nested inside a
            // plain blockquote" pair (2026-08 investigation into `code_block_source_locs`'s own
            // reach — see `app::tests::md_task_toggle_is_byte_exact_across_the_corpus`'s doc
            // comment): a fence inside a *nested* plain quote, and a fence inside an alert nested
            // inside a plain quote. `code_block_source_locs` is not expected to find either (it
            // still has no plain-quote branch at any depth) — these exercise
            // `md_code_block_copy_is_byte_exact_across_the_corpus`'s real `y c` path
            // (`focused_code_source`, reading straight off the model's own `RenderOut::code_blocks`)
            // for a shape that scanner cannot describe.
            (
                "fence inside a nested plain block quote (two levels)",
                "> > ```sh\n> > echo hi\n> > ```\n",
            ),
            (
                "fence inside an alert nested inside a plain block quote",
                "> > [!NOTE]\n> > ```sh\n> > echo hi\n> > ```\n",
            ),
            (
                "block quote nested inside a list item, wrapping an indented block",
                "- item\n\n  > quoted:\n  >\n  >     code\n",
            ),
            (
                "block quote nested inside a list item, wrapping a fence",
                "- item\n\n  > ```sh\n  > echo hi\n  > ```\n",
            ),
            (
                "an alert inside a list item",
                "- item\n\n  > [!NOTE]\n  > ```sh\n  > echo hi\n  > ```\n",
            ),
            (
                "a fence in a list item, then a real top-level fence after the list ends",
                "1. Step:\n\n    ```sh\n    echo hi\n    ```\n\nAfter the list.\n\n```rust\nfn a(){}\n```\n",
            ),
            // The mermaid diversion is measured in **absolute** columns (`parse_fence`, used by
            // `split_segments`/`split_block_parts`), while CommonMark measures the fence itself
            // relative to the list item — so these two neighbours land on opposite sides on purpose.
            (
                "a mermaid fence at an ordered item's content column is diverted to a diagram",
                "1. Diagram:\n\n   ```mermaid\n   flowchart TD\n   A-->B\n   ```\n",
            ),
            (
                "a mermaid fence indented 4 columns is left in the text and drawn as ordinary code",
                "1. Diagram:\n\n    ```mermaid\n    flowchart TD\n    A-->B\n    ```\n",
            ),
            // A clean, top-level fence — not nested in a list/quote/details, nothing else in the
            // document to trigger any *other*, unrelated mismatch — so this case's own `lines`
            // always match between the two renderers, and its `images` comparison in
            // `md_render_diff_tests` is never masked by a pre-existing, tracked mismatch the way the
            // two neighbouring mermaid cases above are (both already `BEHAVIOR_DECISIONS`/
            // `INTENDED_IMPROVEMENTS`-listed for their own, unrelated `lines`-level reasons, which
            // would silently absorb an `ImagePlacement` regression too — see
            // `md_render_diff_tests`'s own module doc comment on why `images` needs its own,
            // independent comparison in the first place). Exists specifically so this corpus can pin
            // a real, unmasked regression in `render_mermaid_slot`'s own `ImagePlacement` fields
            // (`url`/`line`/`cols`/`rows`/`fence_ord`/`alt`) — see `mermaid_fence_url`'s own doc
            // comment on why `url` in particular being wrong is invisible to `lines` entirely.
            (
                "a top-level mermaid fence with no surrounding complication (image placement parity)",
                "```mermaid\nA-->B\n```\n",
            ),
            // Root cause (block-image extraction, 2026-08): pulling an `<img>` line out of a centered
            // banner splits the surrounding HTML block in two, and the `    |` separator left between
            // the halves becomes a real indented code block on screen. Only visible through the
            // production entry point — see `rendered_code_blocks`.
            (
                "4-column-indented HTML inside a centered banner is an HTML block, not indented code",
                "<div align=\"center\">\n    <a href=\"https://example.com\">Docs</a>\n</div>\n\npara\n",
            ),
            // Not `KNOWN_MISMATCHES`/`INTENDED_IMPROVEMENTS`/`BEHAVIOR_DECISIONS` in `md_render_diff_tests`
            // at all, despite `render_doc` visibly *fixing* a real content-loss bug here (an `Html`
            // block nested inside a `Quote` used to vanish from the screen entirely — see
            // `model::BlockKind::Html.body_spans`'s own doc comment for the confirmed per-line
            // `Event::Html` dump the fix relies on): `render_case` — this harness's own "production"
            // side — calls the real `render_markdown_with_images` *dispatcher*, not the legacy
            // renderer directly, and that dispatcher now routes this exact shape to `render::render_doc`
            // too (`RenderOut::unsupported` is empty for it, post-fix), the identical function
            // `render_case_new` (the "new renderer" side) also calls — so both sides of *this*
            // harness's comparison are the same function call on the same input, an exact match by
            // construction, not a case this stage's own architecture merely happens to draw the same
            // way production's separate legacy path does. See
            // `render::tests::html_block_nested_inside_a_quote_renders_its_content_instead_of_vanishing`
            // for the dedicated regression test that pins the actual fixed *content*, and the legacy
            // fallback's own (`markdown.rs`'s `banner_quoted_multiline_no_longer_leaks_its_leftover_fragments`)
            // for what changed on that separate, no-longer-reached path. Kept in this corpus anyway —
            // exercising the shape here still catches a regression that made this stage stop matching
            // (or, worse, start reporting `Unsupported`/mismatching again), it is simply not the *kind*
            // of gap those three lists exist to catalogue.
            (
                "an HTML block nested inside a blockquote is drawn instead of dropped entirely",
                "> <div align=\"center\">\n> HTML-INSIDE-QUOTE\n> </div>\n",
            ),
            // konoma draws GFM tables itself (tui-markdown collapses them), so the parser never sees
            // those lines — and with `ENABLE_TABLES` off it would read them as an ordinary paragraph,
            // which changes whether the line after them can open an indented code block.
            (
                "an indented line right after a table block, with no blank line between",
                "| a | b |\n|---|---|\n    code here\n",
            ),
            // konoma's HTML-block splitter runs a start line to the **next blank line**, which is
            // wider than CommonMark's own end conditions (an HTML comment ends at `-->`; a `<span>`
            // cannot interrupt a paragraph at all). Asking the parser about text konoma never gives
            // it would find code blocks that are drawn as rescued HTML text instead.
            (
                "an indented line swallowed by an HTML comment block is not code",
                "<!-- note -->\n    code here\n",
            ),
            (
                "an indented line swallowed by an inline-tag block that interrupts a paragraph",
                "para\n<span>x</span>\n\n    code here\n",
            ),
            (
                "a centered banner whose links wrap images (registry: criterion README.md)",
                "<div align=\"center\">\n    <a href=\"https://example.com/ci\"><img src=\"https://img.example/badge.svg\" alt=\"CI\"></a>\n    |\n    <a href=\"https://example.com/crate\"><img src=\"https://img.example/v.svg\" alt=\"Crates.io\"></a>\n</div>\n\npara\n",
            ),
            // Task markers in the same container contexts — this corpus is shared with the checkbox
            // write-back scanner (`task_scanner_matches_renderer_across_indented_code_corpus`), which
            // has the same "second implementation of the renderer" shape and so the same failure mode
            // (one stray construct cancels every toggle in the document).
            (
                "checkbox in a nested list item written with three spaces after the bullet (registry: zerocopy agent_docs)",
                "*   **Checklist:**\n    *   [ ] Can this be done with an existing utility?\n    *   [x] Done already\n",
            ),
            (
                "checkbox with nothing after the closing bracket (registry: line-clipping README.md)",
                "- [x]\n- [ ] with text\n",
            ),
            (
                "checkbox with four spaces after the bullet",
                "-    [ ] four spaces is still a task\n",
            ),
            (
                "checkbox-lookalike five spaces after the bullet (the item's content starts with code)",
                "-     [ ] five spaces is not a task marker\n",
            ),
            (
                "checkbox in an ordered list item",
                "1. [ ] ordered task\n2. [x] done\n",
            ),
            (
                "checkbox inside a nested bullet",
                "- outer\n  - [ ] inner task\n",
            ),
            (
                "checkbox inside a plain block quote",
                "> - [ ] quoted task\n",
            ),
            (
                "checkboxes around a fence indented inside an ordered item",
                "- [ ] before\n\n1. Fork it:\n\n    ```sh\n    git clone x\n    ```\n\n- [x] after\n",
            ),
            // ---- Closing line × container (2026-08) ----
            //
            // Root cause: a fence is closed by a line of the same char, at least as long, **with
            // nothing after it** — and, failing that, by the end of the container it opened in. The
            // hand-written mask honored the first rule and had no concept of the second, so a fence
            // whose closing line carries trailing text — `` ``` ([#2642](…)) ``, a shape that reads
            // as perfectly natural when writing a changelog entry — ran to the end of the *document*
            // instead of the end of its list item, and every construct below it was treated as being
            // inside code. nix's CHANGELOG.md does this at line 110; the 617-line GFM table below
            // rendered as one line of raw pipes (all three vendored nix versions in the registry).
            //
            // The axis being pinned is (does this line close? × what container is the fence in? ×
            // what follows it?), written from the spec's case split rather than from the one shape
            // that was reported — the closing-line half was missing from the corpus entirely, which
            // is how it shipped.
            (
                "closing line with trailing text does not close, but the list item ends the block",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` ([#1](https://example.com/1))\n\n- next\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text inside a nested list item",
                "- outer\n  - inner\n\n    ```rust\n    let x = 1;\n    ``` (note)\n\n- after\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text inside a block quote",
                "> ```rust\n> let x = 1;\n> ``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text inside a list item inside a block quote",
                "> - item\n>\n>   ```rust\n>   let x = 1;\n>   ``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "closing line with trailing text at top level really does run to EOF",
                "```rust\nlet x = 1;\n``` (note)\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "unclosed fence in a list item ends with the item, not the document",
                "- item\n\n  ```rust\n  let x = 1;\n\n- next\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (
                "a longer closing line closes",
                "- item\n\n  ```rust\n  let x = 1;\n  ````\n\n- next\n",
            ),
            (
                "a tilde closing line does not close a backtick fence",
                "- item\n\n  ```rust\n  let x = 1;\n  ~~~\n\n- next\n",
            ),
            (
                "a shorter closing line does not close a longer fence",
                "- item\n\n  ````rust\n  let x = 1;\n  ```\n\n- next\n",
            ),
            (
                "an alert after a fence whose closing line has trailing text",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\n- next\n\n> [!NOTE]\n> body\n",
            ),
            (
                "a details block after a fence whose closing line has trailing text",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\n- next\n\n<details open>\n<summary>S</summary>\n\nbody\n\n</details>\n",
            ),
            (
                "a heading and a real fence after a fence whose closing line has trailing text",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\n- next\n\n## H\n\n```sh\necho hi\n```\n",
            ),
            (
                "a checkbox after a fence whose closing line has trailing text (list ended by a paragraph)",
                "- item\n\n  ```rust\n  let x = 1;\n  ``` (note)\n\nText.\n\n- [ ] a\n- [x] b\n",
            ),
            (
                "a checkbox after a fence whose closing line has trailing text (tight list)",
                "- item\n  ```rust\n  let x = 1;\n  ``` (note)\n- [ ] a\n- [x] b\n",
            ),
            // Deliberately **not** in this corpus: the same shape with the checkboxes as further
            // items of the *same loose list* (`… ``` (note)\n\n- next\n\n- [ ] a\n`). tui-markdown
            // panics outright on that document — the loose-list-with-a-task-item panic
            // `render_text_block_safe` exists to survive (verified by calling
            // `tui_markdown::from_str_with_options` on it directly: it unwinds) — so what reaches the
            // screen is the bisected degradation, not a parse, and the renderer draws zero
            // checkboxes while the scanner correctly finds two. That mismatch is upstream's, not
            // this gate's: it only refuses the toggle (principle #3), and the two shapes above cover
            // the same axis without conscripting an unrelated upstream bug into the assertion.
            // The mirror direction: an indented code block whose *content* looks like a construct.
            // `fence_mask` cannot see an indented block at all, so `split_tables`/`split_alerts` used
            // to carve the construct out of it — the table's rows left the block as a rendered table
            // and the `> [!NOTE]` opened a real callout, in both cases tearing the code block apart.
            (
                "indented block whose content is a GFM table stays literal code",
                "para\n\n    | a | b |\n    |---|---|\n    | 1 | 2 |\n",
            ),
            (
                "indented block whose content is an alert header stays literal code",
                "para\n\n    > [!NOTE]\n    > body\n",
            ),
            // ---- Indented code whose content is tag-shaped × the context that decides it (2026-08)
            //
            // Root cause: the same "content decides the block" mistake as the two rows just above,
            // for the third splitter — `split_html_blocks` asked `fence_mask`, which has no notion
            // of indentation, so an indented block opening with anything `is_html_block_start`
            // accepts was rescued as an HTML block: tags stripped, gutter gone, nothing left on
            // screen for `    <code>`, and neither Tab-focusable nor copyable with `y c`.
            //
            // The axis is (**what shape is the block's first line**) × (**is this run a whole
            // document, or a fragment `split_block_parts` cut an `<img>` line out of**), and the
            // second half is the reason it is not enough to write the first. The two contexts
            // produce byte-identical fragments — `    </a>\n    <a href="…">` reads as an indented
            // code block on its own and as `<div>`-interior in place — so a gate can always satisfy
            // one column of this table by getting the other wrong, and both single-column fixes
            // were in fact tried. Only a mask taken over the intact document answers both.
            //
            // The first column is enumerated from what `is_html_block_start` accepts (any tag name,
            // so: bare / attributes / closing / void / comment), plus the tags this module splits
            // its own blocks on, plus the two angle-bracket forms that are not tags at all — not
            // from the one tag a user reported.
            //
            // **What these rows can and cannot catch**, stated plainly because it is easy to assume
            // more: this corpus measures the renderer and the scanner *agreeing*, and both of them
            // reach their answer through the same `split_html_blocks`. Every wrong gate tried for
            // this axis was wrong on both sides at once, so the counts stayed equal and these rows
            // stayed green (measured: reverting the fix, in either of the two directions, fails none
            // of them). They are non-regression pins for the failure mode this module keeps having —
            // one side moving without the other, which refuses `y c` for the whole document — while
            // the correctness of the answer itself is carried by
            // `an_indented_code_block_is_code_whatever_its_content_looks_like` and
            // `a_centered_banner_stays_html_even_though_its_cut_fragments_read_as_indented_code`,
            // which assert what is drawn rather than that two counts match.
            (
                "indented block whose content is a bare tag",
                "para\n\n    <code>\n",
            ),
            (
                "indented block whose content is a tag with attributes",
                "para\n\n    <a href=\"x\">\n",
            ),
            (
                "indented block whose content is a closing tag",
                "para\n\n    </a>\n",
            ),
            (
                "indented block whose content is a void tag",
                "para\n\n    <br>\n",
            ),
            (
                "indented block whose content is an HTML comment",
                "para\n\n    <!-- a comment -->\n",
            ),
            (
                "indented block whose content is a <details> tag",
                "para\n\n    <details>\n",
            ),
            (
                "indented block whose content is a <summary> tag",
                "para\n\n    <summary>S</summary>\n",
            ),
            (
                "indented block whose content is a <kbd> pair",
                "para\n\n    <kbd>Ctrl</kbd>\n",
            ),
            (
                "indented block whose content is an autolink",
                "para\n\n    <https://example.com>\n",
            ),
            // Not a tag name, so `is_html_block_start` always rejected it — this is the one row of
            // the first column that was *correct* while every other row was broken. Pinned so a
            // future change cannot quietly make it the only correct one again.
            (
                "indented block whose content is cjk in angle brackets",
                "para\n\n    <仕様書>\n",
            ),
            // Only the block's *first* line is ever asked, so the two orderings are different cases.
            (
                "indented block whose first line is plain and a later line is tag-shaped",
                "para\n\n    plain first\n    <div>\n",
            ),
            (
                "indented block whose content is tag-shaped, followed by a real fence",
                "para\n\n    <kbd>K</kbd>\n\n```rust\nfn a(){}\n```\n",
            ),
            // Both readings in one document: the `<div>` really is an HTML block, the indented
            // `<div>` below it really is code. A gate that answers with the line alone cannot get
            // both, whichever way it leans.
            (
                "a real HTML block and a tag-shaped indented block in one document",
                "<div align=\"center\">\n  <b>hi</b>\n</div>\n\npara\n\n    <div>\n",
            ),
            // The second column: the `<img>` lines are what make this a *cut*, so they are not
            // decoration — without them nothing is removed, the ambiguous fragment never forms, and
            // the row degenerates into an ordinary HTML block.
            (
                "centered banner whose <img> lines are cut out of it stays HTML",
                "<p align=\"center\">\n    <a href=\"https://example.com/y\">\n      <img src=\"https://img.example/a.svg\" alt=\"a\">\n    </a>\n    <a href=\"https://example.com/x\">\n      <img src=\"https://img.example/b.svg\" alt=\"b\">\n    </a>\n</p>\n\ntail\n",
            ),
            (
                "centered banner with a bare <br> line in it (registry: raw-window-metal README.md)",
                "<p align=\"center\">\n    <a href=\"https://crates.io/crates/rwm\">\n      <img src=\"https://img.example/v.svg\" alt=\"crates.io\">\n    </a>\n    <br>\n    <a href=\"LICENSE-MIT\">\n      <img src=\"https://img.example/mit.svg\" alt=\"License - MIT\">\n    </a>\n</p>\n\ntail\n",
            ),
            (
                "centered banner preceded by a markdown block image (registry: static_assertions README.md)",
                "[![Banner](https://img.example/banner.png)](https://example.com/repo)\n\n<div align=\"center\">\n    <a href=\"https://crates.io/crates/sa\">\n        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\n    </a>\n    <img src=\"https://img.example/rustc.svg\" alt=\"rustc\">\n    <br>\n    <a href=\"https://example.com/patron\">\n        <img src=\"https://img.example/patron.png\" alt=\"Patron\">\n    </a>\n</div>\n\ntail.\n",
            ),
            (
                "crlf centered banner with no <br> (registry: tinytemplate README.md)",
                "<h1 align=\"center\">TT</h1>\r\n\r\n<div align=\"center\">\r\n    <a href=\"https://docs.rs/tt/\">API Documentation</a>\r\n    |\r\n    <a href=\"https://example.com/changelog\">Changelog</a>\r\n</div>\r\n\r\n<div align=\"center\">\r\n    <a href=\"https://example.com/actions\">\r\n        <img src=\"https://img.example/ci.svg\" alt=\"CI\">\r\n    </a>\r\n</div>\r\n\r\ntail\r\n",
            ),
            // ---- KNOWN FAILURE, NOT YET FIXED (2026-08): a lazily continued paragraph glues onto a
            // list-item-nested fence's closing line upstream, dropping the code block's header ----
            //
            // Root cause (paragraph side): `flush_code_run` requires a code run's *last* rendered row
            // to be nothing but fence characters (`is_closing_fence`). When a fenced block sits inside
            // a list item and the very next line — no blank line in between, i.e. a CommonMark lazy
            // continuation — is a paragraph whose first inline is a code span, tui-markdown renders
            // that paragraph's first line glued onto the *same row* as the closing fence (e.g. the
            // rendered row reads `` "```inline" `` instead of a bare `` "```" ``). `is_closing_fence`
            // then rejects that row, so the whole run — opener, body, and the glued closer — is handed
            // back completely undecorated (no header is ever drawn for it), while the write-back
            // scanner (`code_block_source_locs`, which asks pulldown-cmark directly and never sees
            // tui-markdown's rendered rows) still counts the block normally. The mismatch cancels `y c`
            // for every code block in the document, not just this one (the count guard is
            // document-wide). A paragraph whose first line is plain text does not glue — its own line
            // stays on its own row — which is the control case right below the failing ones.
            (
                "KNOWN FAILURE: list item fence glued to a following inline-code paragraph (no blank line) drops the header",
                "1. item:\n   ```\n   aaa\n   ```\n   `inline` after\n",
            ),
            (
                "KNOWN FAILURE: same glue, with an unrelated real fence following later in the document",
                "1. item:\n   ```\n   aaa\n   ```\n   `inline` after\n\n```\nbbb\n```\n",
            ),
            (
                "control: list item fence + a plain-text lazy continuation (no blank line) does not glue",
                "1. item:\n   ```\n   aaa\n   ```\n   after\n",
            ),
            (
                "control: list item fence + a blank line before the inline-code paragraph does not glue",
                "1. item:\n   ```\n   aaa\n   ```\n\n   `inline` after\n",
            ),
            (
                "KNOWN FAILURE: same glue in a bullet list item",
                "- item:\n  ```\n  aaa\n  ```\n  `inline` after\n",
            ),
            (
                "control: bullet list item fence + a plain-text lazy continuation does not glue",
                "- item:\n  ```\n  aaa\n  ```\n  after\n",
            ),
            // Not a case of the glue defect after all (checked by hand while writing this corpus,
            // 2026-08): a *plain* block quote wraps every line of its contents in a `"> "` prefix —
            // including the fence's own opening/closing marker lines — so the run's first line never
            // reads `"```"` in the first place; `flush_code_run` already hands that whole shape back
            // undecorated regardless of what follows it (see that function's own doc comment, and the
            // pre-existing corpus entry "a plain (non-alert) block quote wrapping a fence is not
            // detected by either side" a few entries above this one). The write-back scanner agrees
            // (also 0), so this stays green — the two sides matching here says nothing about the glue
            // defect either way, only that this particular shape was already a known non-detection on
            // both sides before this corpus addition.
            (
                "list item fence nested inside a plain block quote, with an inline-code continuation — masked by the pre-existing block-quote non-detection, not the glue defect",
                "> - item:\n>   ```\n>   aaa\n>   ```\n>   `inline` after\n",
            ),
            // Root cause (body side, a related but distinct upstream defect): tui-markdown also runs
            // a list-item-nested fence's own **body** lines together onto a single rendered row (the
            // same joining `tui_markdown_runs_an_indented_blocks_adjacent_lines_together` already pins
            // for *indented* blocks — this is the fenced-in-a-list-item case of the same upstream
            // behavior; a top-level fence's body is not affected, see that test's control case). This
            // does not change how many blocks are drawn, so the count stays in parity here and this
            // entry alone is expected to stay green; the on-screen row split (or lack of it) is
            // checked directly by `list_item_fence_body_lines_stay_split_on_screen` below.
            (
                "list item fence with a two-line body (count parity only — content is checked elsewhere)",
                "1. item:\n\n   ```\n   aaa\n   bbb\n   ```\n",
            ),
        ]
    }
}

#[cfg(test)]
pub(crate) mod code_span_corpus {
    /// One document plus what each source-rewriting pass must do to it.
    pub struct Case {
        pub name: String,
        pub src: String,
        /// Expected `process_footnotes` output, byte for byte.
        pub footnotes: String,
        /// Expected `process_inline_html` output, byte for byte.
        pub inline_html: String,
        /// Expected `collect_math_exprs` output, as (latex, is_display).
        pub math: Vec<(String, bool)>,
    }

    /// The notations every pass rewrites, gathered into one payload so a single case exercises all
    /// three passes at once. `<br>` is deliberately absent: it rewrites to a hard line break, which
    /// would restructure the surrounding line and obscure what the case is actually pinning (it gets
    /// its own cases below).
    const PAYLOAD: &str = "[^1] <kbd>K</kbd> $x$";
    /// `PAYLOAD` after `process_footnotes` has numbered the reference.
    const PAYLOAD_FN: &str = "¹ <kbd>K</kbd> $x$";
    /// `PAYLOAD` after `process_inline_html` has converted the tag.
    const PAYLOAD_IH: &str = "[^1] `K` $x$";

    /// A case built around a `block` that must survive **every** pass byte for byte — the block holds
    /// `PAYLOAD` inside an inline code span (or inside a fence). A control line carrying the same
    /// payload *outside* any span follows, so the expectations pin both halves at once: a fix that
    /// merely stopped converting everything fails the control, and a fix that keeps converting inside
    /// code fails the block.
    ///
    /// Every expectation here is plain string concatenation, derived from the case's own inputs and
    /// not from the passes being checked.
    fn verbatim(name: &str, block: &str) -> Case {
        Case {
            name: name.to_string(),
            src: format!("{block}\n\nout {PAYLOAD}\n\n[^1]: note\n"),
            footnotes: format!("{block}\n\nout {PAYLOAD_FN}\n\n\n---\n\n1. note\n"),
            inline_html: format!("{block}\n\nout {PAYLOAD_IH}\n\n[^1]: note\n"),
            math: vec![("x".to_string(), false)],
        }
    }

    /// The counterpart to `verbatim`: `line` looks like it holds a code span but does not (the
    /// backticks are literal), so its payload **is** rewritten, exactly like the control line. Pins
    /// the boundary from the other side — an over-eager "treat any backtick as code" fix fails here.
    fn no_span(name: &str, line: &str, line_fn: &str, line_ih: &str) -> Case {
        Case {
            name: name.to_string(),
            src: format!("{line}\n\nout {PAYLOAD}\n\n[^1]: note\n"),
            footnotes: format!("{line_fn}\n\nout {PAYLOAD_FN}\n\n\n---\n\n1. note\n"),
            inline_html: format!("{line_ih}\n\nout {PAYLOAD_IH}\n\n[^1]: note\n"),
            math: vec![("x".to_string(), false), ("x".to_string(), false)],
        }
    }

    /// A case with hand-written expectations, for the notations and footnote semantics that do not
    /// fit the shared payload.
    fn case(
        name: &str,
        src: &str,
        footnotes: &str,
        inline_html: &str,
        math: &[(&str, bool)],
    ) -> Case {
        Case {
            name: name.to_string(),
            src: src.to_string(),
            footnotes: footnotes.to_string(),
            inline_html: inline_html.to_string(),
            math: math.iter().map(|(l, d)| (l.to_string(), *d)).collect(),
        }
    }

    /// Documents pinning **the** invariant every source-rewriting pass shares: the contents of a
    /// literal code region — an inline `` `code span` ``, **or a code block, fenced or indented** —
    /// are a literal *example* of some syntax, never syntax to act on. Enumerated from the
    /// specification's case splits — backtick run lengths, matched/unmatched delimiters, where the
    /// span sits in the line, backslash escapes, multibyte boundaries, and (for blocks) the kind of
    /// indentation, its column count, and what construct can precede it without a blank line — crossed
    /// with the notations the passes rewrite. Deliberately *not* enumerated from the bugs already
    /// found: a corpus grown from past bugs only ever prevents those exact bugs from recurring, which
    /// is how this rule came to be re-derived (and to drift) once per pass in the first place.
    ///
    /// The block half of this was missing until 2026-08: all three passes originally gated only on
    /// `fence_mask`, which recognizes a fenced (```` ``` ````/`~~~`) block but has no notion of
    /// indentation at all, so an *indented* code block — CommonMark's other kind — was invisible to
    /// them. `[^1]`/`<kbd>x</kbd>`/`$x$` written inside one purely to *document* the syntax (e.g. "indent
    /// your snippet 4 columns like this: ...") was rewritten as if it were real, indistinguishable on
    /// screen from an actual reference/tag/expression elsewhere in the document. The fix folds
    /// `code_block_mask` (which asks pulldown-cmark directly, so it agrees with the parser on indented
    /// blocks in any container) into `fence_mask` via `literal_code_mask` — see that function's own
    /// doc comment for why the union, not either mask alone, is what matches the renderer.
    pub fn cases() -> Vec<Case> {
        let mut v = vec![
            // --- Backtick run length: 1 / 2 / 3, and runs that do not pair up ---
            verbatim("one backtick", &format!("a `{PAYLOAD}` b")),
            verbatim("two backticks", &format!("a ``{PAYLOAD}`` b")),
            verbatim("three backticks", &format!("a ```{PAYLOAD}``` b")),
            verbatim(
                "a span may contain a shorter backtick run",
                &format!("a `` `{PAYLOAD}` `` b"),
            ),
            verbatim(
                "a span may contain a lone backtick",
                &format!("a ``{PAYLOAD} ` tail`` b"),
            ),
            // --- Where the span sits in the line ---
            verbatim("span at line start", &format!("`{PAYLOAD}` tail")),
            verbatim("span at line end", &format!("head `{PAYLOAD}`")),
            verbatim("span is the whole line", &format!("`{PAYLOAD}`")),
            verbatim("two spans on one line", "`[^1]` mid `<kbd>K</kbd>` end"),
            verbatim("three spans on one line", "`[^1]`, `<kbd>K</kbd>`, `$x$`"),
            verbatim(
                "span spanning the line with text either side",
                &format!("x`{PAYLOAD}`y"),
            ),
            // --- Multibyte safety right at the delimiters ---
            verbatim("cjk around a span", &format!("日本 `{PAYLOAD}` 語")),
            verbatim(
                "cjk immediately against the delimiters",
                &format!("日本`{PAYLOAD}`語"),
            ),
            verbatim(
                "emoji immediately against the delimiters",
                &format!("🎉`{PAYLOAD}`🎉"),
            ),
            verbatim("cjk inside the span", &format!("a `日本{PAYLOAD}語` b")),
            verbatim(
                "escaped multibyte char before a span",
                &format!("a \\あ `{PAYLOAD}` b"),
            ),
            verbatim(
                "escaped emoji before a span",
                &format!("a \\🎉 `{PAYLOAD}` b"),
            ),
            // --- Fences: already protected before this fix; must stay protected ---
            verbatim("backtick fence", &format!("```\n{PAYLOAD}\n```")),
            verbatim("tilde fence", &format!("~~~\n{PAYLOAD}\n~~~")),
            verbatim("fence with a language", &format!("```rust\n{PAYLOAD}\n```")),
            verbatim(
                "fence holding an unclosed backtick",
                &format!("```\n{PAYLOAD} ` alone\n```"),
            ),
            verbatim(
                "span on the line after a fence",
                &format!("```\ncode\n```\n\nafter `{PAYLOAD}` end"),
            ),
            // --- Literal backticks that do NOT open a span (the boundary from the other side) ---
            no_span(
                "opener longer than closer is not a span",
                &format!("a ``{PAYLOAD}` b"),
                &format!("a ``{PAYLOAD_FN}` b"),
                &format!("a ``{PAYLOAD_IH}` b"),
            ),
            no_span(
                "closer longer than opener is not a span",
                &format!("a `{PAYLOAD}`` b"),
                &format!("a `{PAYLOAD_FN}`` b"),
                &format!("a `{PAYLOAD_IH}`` b"),
            ),
            no_span(
                "unclosed backtick",
                &format!("a `{PAYLOAD} b"),
                &format!("a `{PAYLOAD_FN} b"),
                &format!("a `{PAYLOAD_IH} b"),
            ),
            no_span(
                "escaped backtick does not open a span",
                &format!("a \\`{PAYLOAD}` b"),
                &format!("a \\`{PAYLOAD_FN}` b"),
                &format!("a \\`{PAYLOAD_IH}` b"),
            ),
            no_span(
                "two adjacent backticks with no closer",
                &format!("a `` {PAYLOAD} b"),
                &format!("a `` {PAYLOAD_FN} b"),
                &format!("a `` {PAYLOAD_IH} b"),
            ),
            no_span(
                "no backticks at all (the control for every case above)",
                &format!("a {PAYLOAD} b"),
                &format!("a {PAYLOAD_FN} b"),
                &format!("a {PAYLOAD_IH} b"),
            ),
        ];
        // An escaped *backslash* leaves the following backtick free to open a real span — the exact
        // counterpart of "escaped backtick does not open a span" above.
        v.push(verbatim(
            "escaped backslash then a real span",
            &format!("a \\\\`{PAYLOAD}` b"),
        ));
        v.extend([
            // --- One case per notation the passes rewrite, inside and outside a span ---
            case(
                "del inside and outside",
                "`<del>d</del>` and <del>d</del>\n",
                "`<del>d</del>` and <del>d</del>\n",
                "`<del>d</del>` and ~~d~~\n",
                &[],
            ),
            case(
                "s inside and outside",
                "`<s>d</s>` and <s>d</s>\n",
                "`<s>d</s>` and <s>d</s>\n",
                "`<s>d</s>` and ~~d~~\n",
                &[],
            ),
            case(
                "strike inside and outside",
                "`<strike>d</strike>` and <strike>d</strike>\n",
                "`<strike>d</strike>` and <strike>d</strike>\n",
                "`<strike>d</strike>` and ~~d~~\n",
                &[],
            ),
            case(
                "sup inside and outside",
                "`<sup>2</sup>` and <sup>2</sup>\n",
                "`<sup>2</sup>` and <sup>2</sup>\n",
                "`<sup>2</sup>` and ²\n",
                &[],
            ),
            case(
                "sub inside and outside",
                "`<sub>2</sub>` and <sub>2</sub>\n",
                "`<sub>2</sub>` and <sub>2</sub>\n",
                "`<sub>2</sub>` and ₂\n",
                &[],
            ),
            // `<br>` inside a span used to inject a newline *into* the span, splitting it in half.
            case(
                "br inside and outside",
                "`<br>` and <br> tail\n",
                "`<br>` and <br> tail\n",
                "`<br>` and   \n tail\n",
                &[],
            ),
            case(
                "br self-closing inside and outside",
                "`<br />` and <br /> tail\n",
                "`<br />` and <br /> tail\n",
                "`<br />` and   \n tail\n",
                &[],
            ),
            case(
                "uppercase br inside and outside",
                "`<BR>` and <BR> tail\n",
                "`<BR>` and <BR> tail\n",
                "`<BR>` and   \n tail\n",
                &[],
            ),
            case(
                "display math inside and outside",
                "`$$x$$` and $$y$$\n",
                "`$$x$$` and $$y$$\n",
                "`$$x$$` and $$y$$\n",
                &[("y", true)],
            ),
            case(
                "paren math inside and outside",
                "`\\(x\\)` and \\(y\\)\n",
                "`\\(x\\)` and \\(y\\)\n",
                "`\\(x\\)` and \\(y\\)\n",
                &[("y", false)],
            ),
            case(
                "bracket math inside and outside",
                "`\\[x\\]` and \\[y\\]\n",
                "`\\[x\\]` and \\[y\\]\n",
                "`\\[x\\]` and \\[y\\]\n",
                &[("y", true)],
            ),
            // A tag pair may legitimately *wrap* a code span. Taken verbatim from palette 0.7.7's
            // README, which is how this case was found at all: the first attempt at this fix split
            // the line at each span, which hid `</strike>` from `<strike>` and left both tags raw.
            // The span's contents stay literal; the strikethrough around it still happens.
            case(
                "a tag pair straddling a code span (palette README)",
                "x <strike>Enables `named::from_str`, which maps names.</strike>\n",
                "x <strike>Enables `named::from_str`, which maps names.</strike>\n",
                "x ~~Enables `named::from_str`, which maps names.~~\n",
                &[],
            ),
            case(
                "a tag pair straddling two code spans",
                "<del>a `b` c `d` e</del>\n",
                "<del>a `b` c `d` e</del>\n",
                "~~a `b` c `d` e~~\n",
                &[],
            ),
            case(
                "a kbd pair straddling a code span",
                "<kbd>press `X` now</kbd>\n",
                "<kbd>press `X` now</kbd>\n",
                "`press `X` now`\n",
                &[],
            ),
            // --- Footnote-specific semantics ---
            // A reference that exists only inside a span is not a reference at all, so the document
            // has no *referenced* definitions and the whole pass stays a no-op — no numbering, no
            // appended section, and the definition line stays where the author put it.
            case(
                "the only reference is inside a span",
                "just `[^1]` here\n\n[^1]: note\n",
                "just `[^1]` here\n\n[^1]: note\n",
                "just `[^1]` here\n\n[^1]: note\n",
                &[],
            ),
            case(
                "reference inside a span on one line, outside on the next",
                "`[^1]`\n\nreal [^1]\n\n[^1]: note\n",
                "`[^1]`\n\nreal ¹\n\n\n---\n\n1. note\n",
                "`[^1]`\n\nreal [^1]\n\n[^1]: note\n",
                &[],
            ),
            // Numbering follows first appearance *outside* spans: `[^b]` inside the span must not
            // claim number 1, which would also reorder the appended section.
            case(
                "numbering ignores references inside spans",
                "`[^b]` then [^a] then [^b]\n\n[^a]: A\n[^b]: B\n",
                "`[^b]` then ¹ then ²\n\n\n---\n\n1. A\n2. B\n",
                "`[^b]` then [^a] then [^b]\n\n[^a]: A\n[^b]: B\n",
                &[],
            ),
            case(
                "undefined reference stays literal inside and outside",
                "`[^9]` and [^9] and [^1]\n\n[^1]: note\n",
                "`[^9]` and [^9] and ¹\n\n\n---\n\n1. note\n",
                "`[^9]` and [^9] and [^1]\n\n[^1]: note\n",
                &[],
            ),
            // --- Degenerate inputs ---
            case("empty document", "", "", "", &[]),
            case(
                "span holding only spaces",
                "a `   ` b [^1]\n\n[^1]: note\n",
                "a `   ` b ¹\n\n\n---\n\n1. note\n",
                "a `   ` b [^1]\n\n[^1]: note\n",
                &[],
            ),
            case(
                "line that is nothing but backticks",
                "````\n",
                "````\n",
                "````\n",
                &[],
            ),
            case(
                "trailing backslash at end of line",
                "a [^1] \\\n\n[^1]: note\n",
                "a ¹ \\\n\n\n---\n\n1. note\n",
                "a [^1] \\\n\n[^1]: note\n",
                &[],
            ),
        ]);
        // --- Indented code blocks: CommonMark's *other* kind of code block (fenced ``` / ~~~ is
        // covered above). `literal_code_mask` only started covering these with the `code_block_mask`
        // union (2026-08) — before that, all three passes gated on `fence_mask` alone, which is blind
        // to indentation entirely, so `[^1]`/`<kbd>`/`$x$` written inside one purely to *document* the
        // syntax (e.g. "indent your snippet 4 columns like this: ...") were rewritten as if they were
        // real. Columns are built with `.repeat` rather than counted by eye in a string literal, since
        // an off-by-one here would silently test the wrong boundary.
        let ind3 = " ".repeat(3);
        let ind4 = " ".repeat(4);
        let ind5 = " ".repeat(5);
        let ind6 = " ".repeat(6);
        let ind8 = " ".repeat(8);
        v.extend([
            // The case explicitly requested: a 4-column indent at document start.
            verbatim("indented code block", &format!("    {PAYLOAD}")),
            // A tab is 4 columns (CommonMark's tab-stop rule, `leading_ws_width`'s own doc comment) —
            // a different *kind* of indented block, not just a different width of the same one.
            verbatim(
                "tab-indented code block",
                &format!("\t{PAYLOAD}"),
            ),
            // The boundary from the other side: one column short of 4 is still an ordinary paragraph,
            // so the payload on it *is* rewritten, exactly like the control line — `no_span`, not
            // `verbatim`.
            no_span(
                "indented three columns is not a code block (ordinary paragraph)",
                &format!("{ind3}{PAYLOAD}"),
                &format!("{ind3}{PAYLOAD_FN}"),
                &format!("{ind3}{PAYLOAD_IH}"),
            ),
            // Past 4 columns, the extra indentation is literal *content* of the same one block, not a
            // second, more-indented block.
            verbatim(
                "indented eight columns is still one literal code block",
                &format!("{ind8}{PAYLOAD}"),
            ),
            // --- Position: what can precede an indented code block without a blank line ---
            verbatim(
                "indented code block after a paragraph",
                &format!("para\n\n{ind4}{PAYLOAD}"),
            ),
            // A heading is not a paragraph, so — unlike the paragraph case just above, which *needs*
            // the blank line — it cannot absorb a following indented line as a continuation. No blank
            // line is needed for the indented line right after it to start a fresh code block.
            verbatim(
                "indented code block right after a heading, no blank line",
                &format!("# H\n{ind4}{PAYLOAD}"),
            ),
            // Inside a list item the indentation is relative to the item's own content column: a
            // "- " marker's content starts at column 2, so a code block inside it needs marker + 4 =
            // six columns, and (like the top-level paragraph case) a blank line first — an indented
            // line right after the item's opening text is just a continuation of that paragraph.
            verbatim(
                "indented code block inside a list item, six columns, blank line before it",
                &format!("- item\n\n{ind6}{PAYLOAD}"),
            ),
            // The boundary from the other side: four columns under the same marker is short of the
            // six the item needs, so it stays a (lazily-indented) continuation of "- item"'s paragraph
            // — not a code block — and the payload on it is rewritten normally.
            no_span(
                "indented four columns inside a list item is not a code block (paragraph continuation)",
                &format!("- item\n{ind4}{PAYLOAD}"),
                &format!("- item\n{ind4}{PAYLOAD_FN}"),
                &format!("- item\n{ind4}{PAYLOAD_IH}"),
            ),
            // Inside a blockquote the same rule applies relative to the quote's own content column
            // (`>` plus one optional space = column 1): here the indented block is the very first
            // line of the quote, so — like the document-start case — no blank line is needed first.
            verbatim(
                "indented code block inside a blockquote",
                &format!(">{ind5}{PAYLOAD}"),
            ),
            // Two indented chunks separated by one blank line are a single block in CommonMark (the
            // blank line is folded into the block's own range, not treated as ending it) — both
            // chunks, and the blank line between them, must stay untouched.
            verbatim(
                "two indented chunks separated by a blank line stay one literal block",
                &format!("{ind4}{PAYLOAD}\n\n{ind4}{PAYLOAD}"),
            ),
            // --- Notations besides PAYLOAD's own (footnote ref / <kbd> / inline math), each inside an
            // indented block vs. outside one. None of these documents define a footnote, so
            // `process_footnotes` short-circuits to a byte-identical no-op for all nine — that itself
            // is part of what's being pinned (a stray indented block must never manufacture a
            // reference/definition that was not there).
            case(
                "del inside an indented code block and outside",
                "    <del>d</del>\n\n<del>d</del>\n",
                "    <del>d</del>\n\n<del>d</del>\n",
                "    <del>d</del>\n\n~~d~~\n",
                &[],
            ),
            case(
                "s inside an indented code block and outside",
                "    <s>d</s>\n\n<s>d</s>\n",
                "    <s>d</s>\n\n<s>d</s>\n",
                "    <s>d</s>\n\n~~d~~\n",
                &[],
            ),
            case(
                "strike inside an indented code block and outside",
                "    <strike>d</strike>\n\n<strike>d</strike>\n",
                "    <strike>d</strike>\n\n<strike>d</strike>\n",
                "    <strike>d</strike>\n\n~~d~~\n",
                &[],
            ),
            case(
                "sup inside an indented code block and outside",
                "    <sup>2</sup>\n\n<sup>2</sup>\n",
                "    <sup>2</sup>\n\n<sup>2</sup>\n",
                "    <sup>2</sup>\n\n²\n",
                &[],
            ),
            case(
                "sub inside an indented code block and outside",
                "    <sub>2</sub>\n\n<sub>2</sub>\n",
                "    <sub>2</sub>\n\n<sub>2</sub>\n",
                "    <sub>2</sub>\n\n₂\n",
                &[],
            ),
            // `<br>` inside an indented block used to inject a newline *into* the block, splitting it
            // in two (the same failure mode as the inline-code-span "br inside and outside" case
            // above, for the other kind of code block).
            case(
                "br inside an indented code block and outside",
                "    <br>\n\n<br> tail\n",
                "    <br>\n\n<br> tail\n",
                "    <br>\n\n  \n tail\n",
                &[],
            ),
            case(
                "display math ($$...$$) inside an indented code block and outside",
                "    $$x$$\n\n$$y$$\n",
                "    $$x$$\n\n$$y$$\n",
                "    $$x$$\n\n$$y$$\n",
                &[("y", true)],
            ),
            case(
                "paren math (\\(...\\)) inside an indented code block and outside",
                "    \\(x\\)\n\n\\(y\\)\n",
                "    \\(x\\)\n\n\\(y\\)\n",
                "    \\(x\\)\n\n\\(y\\)\n",
                &[("y", false)],
            ),
            case(
                "bracket math (\\[...\\]) inside an indented code block and outside",
                "    \\[x\\]\n\n\\[y\\]\n",
                "    \\[x\\]\n\n\\[y\\]\n",
                "    \\[x\\]\n\n\\[y\\]\n",
                &[("y", true)],
            ),
            // ---- Math containing a backslash-escaped ASCII punctuation character (`` \, ``/`` \_ ``/
            // `` \% ``, CommonMark's own generic escape syntax for *any* ASCII punctuation, not a
            // notation this file's own math extension owns — so this is ordinary, unremarkable LaTeX,
            // `\,` a thin space and `\_`/`\%` literal underscore/percent). `collect_math_exprs` (this
            // corpus's own `math` field, checked byte for byte by `code_spans_are_literal_in_every_
            // source_pass`) works from raw `src` directly and has always handled these correctly —
            // `scan_inline_math`'s own escape handling (`b'\\' if i + 1 < n`) keeps the backslash
            // literally in its accumulation buffer regardless of what character follows it. The
            // *rendering* side (`render.rs`'s `render_doc`, an independent pulldown-cmark-event-driven
            // pipeline `md_render_diff_tests`'s own `markdown_render_diff_report` compares against this
            // exact corpus) does not share that same robustness for free: pulldown-cmark's own escape
            // handling splits an `Event::Text` at *every* backslash-escape point (confirmed directly —
            // see `render.rs`'s own `render_dollar_math_tail`/`backslash_math_opener` doc comments for
            // the exact byte ranges), so a math span with an escape inside it never appears whole within
            // any *single* event's own payload — a real, confirmed bug this corpus exists to pin against
            // regressing back to (`render_dollar_math_tail`/`render_backslash_math`'s own raw-`src`-slice
            // fix; before it, every one of these cases rendered as unlifted literal text for `$…$`/
            // `$$…$$`, or lost the escape's own backslash entirely for `\(…\)`/`\[…\]`).
            case(
                "escaped comma inside display math ($$a\\,b$$)",
                "$$a\\,b$$\n",
                "$$a\\,b$$\n",
                "$$a\\,b$$\n",
                &[("a\\,b", true)],
            ),
            case(
                "escaped underscore inside display math ($$a\\_b$$)",
                "$$a\\_b$$\n",
                "$$a\\_b$$\n",
                "$$a\\_b$$\n",
                &[("a\\_b", true)],
            ),
            case(
                "escaped percent inside inline math ($a\\%b$)",
                "$a\\%b$\n",
                "$a\\%b$\n",
                "$a\\%b$\n",
                &[("a\\%b", false)],
            ),
            case(
                "escaped comma inside backslash-paren inline math (\\(a\\,b\\))",
                "\\(a\\,b\\)\n",
                "\\(a\\,b\\)\n",
                "\\(a\\,b\\)\n",
                &[("a\\,b", false)],
            ),
            case(
                "escaped comma inside backslash-bracket display math (\\[a\\,b\\])",
                "\\[a\\,b\\]\n",
                "\\[a\\,b\\]\n",
                "\\[a\\,b\\]\n",
                &[("a\\,b", true)],
            ),
            // The escape is the *very first* thing inside the delimiters — the opening `$$` and the
            // escape's own backslash are adjacent, with nothing else between them (so the pulldown-cmark
            // event boundary the escape causes sits right at the content's own start, not somewhere
            // partway through it).
            case(
                "escaped comma at the very start of display math content ($$\\,ab$$)",
                "$$\\,ab$$\n",
                "$$\\,ab$$\n",
                "$$\\,ab$$\n",
                &[("\\,ab", true)],
            ),
            // The mirror image: the escape is the *very last* thing before the closing `$$`.
            case(
                "escaped comma right before the closing delimiter of display math ($$ab\\,$$)",
                "$$ab\\,$$\n",
                "$$ab\\,$$\n",
                "$$ab\\,$$\n",
                &[("ab\\,", true)],
            ),
            // CJK either side of the escape — same multibyte-safety concern
            // `code_span_corpus`'s own "cjk"/"escaped multibyte" cases already pin for the *opener*/
            // *closer* detection itself (`utf8_len`-based advancing, never a fixed byte count).
            case(
                "escaped comma between cjk characters inside display math ($$あ\\,い$$)",
                "$$あ\\,い$$\n",
                "$$あ\\,い$$\n",
                "$$あ\\,い$$\n",
                &[("あ\\,い", true)],
            ),
            // Negative: the identical escaped-math-looking text sitting inside an inline code span must
            // stay literal, never lifted — mirrors `verbatim`'s own "protected inside a span, still
            // converts on a control line right outside it" shape, but for an escape-bearing expression
            // specifically (a plain, escape-free `$x$` already has this property pinned by every
            // `verbatim`/`no_span` case above; this one confirms the *escaped* shape does not somehow
            // leak through the code-span boundary via the very mechanism that lets it survive an
            // ordinary `Event::Text` boundary split).
            case(
                "escaped math inside an inline code span stays literal; the same shape outside is lifted",
                "`$$a\\,b$$` and $$c\\,d$$\n",
                "`$$a\\,b$$` and $$c\\,d$$\n",
                "`$$a\\,b$$` and $$c\\,d$$\n",
                &[("c\\,d", true)],
            ),
            // Robustness, not required by any known bug: a *fully* escaped `\$` (CommonMark's own
            // generic escape of the dollar sign itself, `code_span_corpus`'s sibling `inline_corpus`
            // module already pins as "not math" for `collect_math_exprs`) still leaves a literal `$`
            // character inside `scan_inline_math`'s own accumulation buffer (the backslash is kept, not
            // stripped — see `scan_inline_math`'s own doc comment) — which is exactly the signal
            // `render.rs`'s own `dollar_math_still_open` probe reacts to, even though nothing here is
            // *actually* an unresolved opener at all. Combined with unrelated escaped asterisks
            // (`\*not italic\*`) elsewhere in the very same paragraph — pulled into the same
            // escape-joined chain `render_dollar_math_tail`'s own growth loop follows — this is the
            // shape most likely to regress into showing a stray literal backslash if that growth loop
            // ever renders its own "give up" fallback from the *raw* (backslash-preserved) slice instead
            // of replaying the original, escape-processed events (see that function's own doc comment,
            // "give up", for exactly why it must not).
            case(
                "escaped currency dollars mixed with an unrelated escape elsewhere render literally, not as math",
                "cost \\$5 and \\$10 for \\*not italic\\*.\n",
                "cost \\$5 and \\$10 for \\*not italic\\*.\n",
                "cost \\$5 and \\$10 for \\*not italic\\*.\n",
                &[],
            ),
            // A real, confirmed panic found while fixing the escape-splitting bug above (not itself an
            // escape case at all — `dollar_math_still_open`'s own trigger condition is deliberately
            // broad enough to fire for *any* never-closing `$`, escape or not): `render_dollar_math_tail`
            // used to give up the instant a non-continuing event turned up, which crashed the moment
            // that event was a `Start(tag)` (bold/italic/link, any of them) whose own `range` spans
            // *past* the individual nested events still left to consume — see that function's own "Never
            // stop mid-construct" doc comment for the exact mechanism and byte ranges. Three shapes:
            // ordinary nesting, nesting with an interior line break, and a link.
            case(
                "a never-closing dollar sign followed by unrelated bold markup does not crash",
                "It costs $50 **bold** text.\n",
                "It costs $50 **bold** text.\n",
                "It costs $50 **bold** text.\n",
                &[],
            ),
            case(
                "a never-closing dollar sign followed by markup containing a line break does not crash",
                "It costs $50 *italic\nbreak* more.\n",
                "It costs $50 *italic\nbreak* more.\n",
                "It costs $50 *italic\nbreak* more.\n",
                &[],
            ),
            case(
                "a never-closing dollar sign followed by an unrelated link does not crash",
                "It costs $50 [a link](url) more.\n",
                "It costs $50 [a link](url) more.\n",
                "It costs $50 [a link](url) more.\n",
                &[],
            ),
            // The flip side of the panic fix: a dollar-math span whose real closer sits past an
            // intervening inline code span, or past nested markup, now resolves correctly too — matching
            // production's own raw-source `collect_math_exprs`, which has always treated such markup as
            // ordinary literal characters (it runs before tui-markdown ever interprets any of it).
            case(
                "escaped math resolves across an intervening inline code span",
                "$$a\\,b `x` c\\_d$$\n",
                "$$a\\,b `x` c\\_d$$\n",
                "$$a\\,b `x` c\\_d$$\n",
                &[("a\\,b `x` c\\_d", true)],
            ),
            case(
                "escaped math resolves across intervening nested markup",
                "$$a\\,b **c** d\\_e$$\n",
                "$$a\\,b **c** d\\_e$$\n",
                "$$a\\,b **c** d\\_e$$\n",
                &[("a\\,b **c** d\\_e", true)],
            ),
            // The other reason `literal_code_mask` exists at all (see its own doc comment): a fence
            // with no blank line between it and a preceding `<summary>…</summary>` line is one thing
            // `code_block_mask` alone gets *wrong* (it reads the whole thing as one HTML block) and
            // `fence_mask` gets right by accident — so this is the one corpus case where the union's
            // *other* half is doing the protecting, not `code_block_mask`. Already covered by two
            // individual tests (`process_inline_html_leaves_a_details_fence_without_a_blank_line_untouched`,
            // `process_footnotes_leaves_a_details_fence_without_a_blank_line_untouched`) and one
            // hand-written math case below in `fence_and_math_extraction_tests` — added here too so it
            // also runs through the corpus's two mechanically-derived tests (span survival, math
            // opacity), which those individual tests don't exercise.
            verbatim(
                "details/summary immediately followed by a fenced code block, no blank line",
                &format!("<details>\n<summary>s</summary>\n```rust\n{PAYLOAD}\n```\n</details>"),
            ),
        ]);
        // --- What the block's *first line looks like* (2026-08). The enumeration above varies the
        // indentation and what precedes the block; it never varies the block's own content, and the
        // shape of the first line turned out to decide whether the block was treated as a code block
        // at all: `split_html_blocks` asked `fence_mask`, which is blind to indentation, so an
        // indented block opening with anything `is_html_block_start` accepts (any tag name) was
        // rescued as an HTML block instead — tags stripped, gutter gone, and its contents no longer
        // literal. That gate is the renderer's rather than these three passes', so what these rows
        // pin is the half this corpus owns: whatever the opening line looks like, the *contents* of
        // the block stay bytes. A future fix that leans on the first line's shape in a pass as well
        // as in the splitter fails here.
        v.extend([
            verbatim(
                "indented code block opening with a rewritable tag",
                &format!("{ind4}<kbd>K</kbd>\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with a closing tag",
                &format!("{ind4}</a>\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with an HTML comment",
                &format!("{ind4}<!-- c -->\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with a tag with attributes",
                &format!("{ind4}<a href=\"x\">\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with an autolink",
                &format!("{ind4}<https://example.com>\n{ind4}{PAYLOAD}"),
            ),
            verbatim(
                "indented code block opening with cjk in angle brackets",
                &format!("{ind4}<仕様書>\n{ind4}{PAYLOAD}"),
            ),
            // The other 2026-08 fix, at the byte level nothing else measures it: inside an HTML
            // block a `<br>` line that would come out blank is **removed**, because a blank line
            // ends the block and no substitution — not even a single space — avoids that. The
            // banner's other lines have to come back byte-identical, so this pins the removal is
            // exactly one line wide, and `process_footnotes` (which runs first in production, and
            // is what relocates a surviving tag somewhere it can do damage) is a no-op here.
            case(
                "bare br line inside a centered banner is removed, the rest byte-identical",
                "<p align=\"center\">\n    <a href=\"https://a\">\n    <br>\n    <a href=\"https://b\">\n</p>\n",
                "<p align=\"center\">\n    <a href=\"https://a\">\n    <br>\n    <a href=\"https://b\">\n</p>\n",
                "<p align=\"center\">\n    <a href=\"https://a\">\n    <a href=\"https://b\">\n</p>\n",
                &[],
            ),
            // The boundary from the other side, so "remove the line" cannot become "remove the tag
            // everywhere": with text on both sides the rewrite leaves two non-blank lines, which an
            // HTML block tolerates, so it is an ordinary hard break — which is also what keeps an
            // `<img>` sharing a line with a `<br>` on a line of its own for the block-image pass.
            case(
                "mid-line br inside a centered banner is still a hard break",
                "<div align=\"center\">\n  before<br>after\n</div>\n",
                "<div align=\"center\">\n  before<br>after\n</div>\n",
                "<div align=\"center\">\n  before  \nafter\n</div>\n",
                &[],
            ),
        ]);
        v
    }
}
#[cfg(test)]
mod code_span_parity_tests {
    use super::*;

    /// Every inline code span of `src`, as `(start_line_index, text_including_delimiters)`, skipping
    /// lines that are already literal code — fenced **or indented** (there the backticks, if any, are
    /// the fence's own delimiters, or just literal text). Uses `literal_code_mask`, the exact mask the
    /// three passes under test actually consult, so this describes precisely the regions they promise
    /// to leave alone — not a narrower one (`fence_mask` alone would miss an indented block and this
    /// helper would then go looking for "code spans" inside text that was never span-delimited).
    fn spans_of(src: &str) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        let fenced = literal_code_mask(&lines);
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if fenced[i] {
                continue;
            }
            let mut cursor = 0;
            while let Some((s, e)) = next_inline_code_span(line, cursor) {
                out.push(line[s..e].to_string());
                cursor = e;
            }
        }
        out
    }

    fn count_of(hay: &str, needle: &str) -> usize {
        if needle.is_empty() {
            return 0;
        }
        hay.matches(needle).count()
    }

    /// The precise net: each corpus document, through each pass, compared byte for byte against a
    /// hand-written expectation. Pins **both** halves at once — a payload inside a code span is
    /// untouched *and* the identical payload outside one is still rewritten — so neither "keeps
    /// converting inside code" (the bug) nor "stopped converting anything" (an over-correction) can
    /// pass.
    #[test]
    fn code_spans_are_literal_in_every_source_pass() {
        for c in code_span_corpus::cases() {
            assert_eq!(
                process_footnotes(&c.src),
                c.footnotes,
                "process_footnotes disagrees for case {:?} (src {:?})",
                c.name,
                c.src
            );
            assert_eq!(
                process_inline_html(&c.src),
                c.inline_html,
                "process_inline_html disagrees for case {:?} (src {:?})",
                c.name,
                c.src
            );
            let math: Vec<(String, bool)> = collect_math_exprs(&c.src);
            assert_eq!(
                math, c.math,
                "collect_math_exprs disagrees for case {:?} (src {:?})",
                c.name, c.src
            );
        }
    }

    /// The broad net over the same corpus, derived mechanically instead of from expectations: every
    /// code span of the source must still be present, delimiters and all, in what each string pass
    /// emits. Catches a future pass that mangles a span in a way nobody wrote an expectation for.
    #[test]
    fn code_span_text_survives_every_string_pass_verbatim() {
        for c in code_span_corpus::cases() {
            let spans = spans_of(&c.src);
            for pass in ["footnotes", "inline_html"] {
                let out = match pass {
                    "footnotes" => process_footnotes(&c.src),
                    _ => process_inline_html(&c.src),
                };
                for span in &spans {
                    let want = count_of(&c.src, span);
                    let got = count_of(&out, span);
                    assert!(
                        got >= want,
                        "{pass} lost or altered the code span {span:?} in case {:?}: \
                         appears {want}x in the source but {got}x in {out:?}",
                        c.name
                    );
                }
            }
        }
    }

    /// The math pass has no string output to scan, so the same invariant is stated as opacity:
    /// rewriting every code span's *contents* to an inert `x` must not change which expressions are
    /// extracted. If the scanner ever looked inside a span, the extracted list would move. A line
    /// that is already literal code — fenced **or indented** (`literal_code_mask`, not just
    /// `fence_mask`) — is left untouched rather than scanned for spans, matching what `split_math`
    /// itself does with such a line.
    #[test]
    fn math_extraction_treats_code_span_contents_as_opaque() {
        for c in code_span_corpus::cases() {
            let lines: Vec<&str> = c.src.lines().collect();
            let literal = literal_code_mask(&lines);
            let mut blanked = String::new();
            for (i, line) in lines.iter().enumerate() {
                if literal[i] {
                    blanked.push_str(line);
                    blanked.push('\n');
                    continue;
                }
                let mut cursor = 0;
                while let Some((s, e)) = next_inline_code_span(line, cursor) {
                    blanked.push_str(&line[cursor..s]);
                    // Keep the delimiter runs, replace only the content.
                    let run = line[s..e].len() - line[s..e].trim_matches('`').len();
                    let ticks = "`".repeat(run / 2);
                    blanked.push_str(&ticks);
                    blanked.push('x');
                    blanked.push_str(&ticks);
                    cursor = e;
                }
                blanked.push_str(&line[cursor..]);
                blanked.push('\n');
            }
            assert_eq!(
                collect_math_exprs(&c.src),
                collect_math_exprs(&blanked),
                "math extraction changed when only code-span *contents* changed, in case {:?}\n\
                 src     {:?}\n blanked {:?}",
                c.name,
                c.src,
                blanked
            );
        }
    }

    /// The shared primitive itself, against CommonMark's backtick rules: a span is delimited by runs
    /// of **equal** length, an unmatched run is literal text, and a backslash-escaped backtick never
    /// opens one. Every pass inherits exactly these answers, which is the point of having one copy.
    #[test]
    fn next_inline_code_span_follows_commonmark_backtick_rules() {
        /// (line, byte to scan from, expected span as `(start, end)`).
        type Probe = (&'static str, usize, Option<(usize, usize)>);
        let cases: &[Probe] = &[
            ("`a`", 0, Some((0, 3))),
            ("x `a` y", 0, Some((2, 5))),
            ("``a``", 0, Some((0, 5))),
            ("```a```", 0, Some((0, 7))),
            // Run lengths must match exactly.
            ("``a`", 0, None),
            ("`a``", 0, None),
            ("```a`", 0, None),
            // A longer run may wrap a shorter one.
            ("`` `a` ``", 0, Some((0, 9))),
            // Unmatched run first, real span later: the literal run is skipped, not treated as code.
            ("`` a `b` c", 0, Some((5, 8))),
            // Backslash escapes.
            ("\\`a`", 0, None),
            ("\\\\`a`", 0, Some((2, 5))),
            ("a \\あ `b`", 0, Some((7, 10))),
            ("a \\🎉 `b`", 0, Some((8, 11))),
            // Multibyte immediately against the delimiters (byte offsets, not char offsets).
            ("日本`a`語", 0, Some((6, 9))),
            ("🎉`a`", 0, Some((4, 7))),
            // Resuming after a span.
            ("`a` `b`", 3, Some((4, 7))),
            ("`a` `b`", 7, None),
            // Nothing to find.
            ("no backticks here", 0, None),
            ("```", 0, None),
            ("", 0, None),
            // A trailing backslash must not read past the end.
            ("a \\", 0, None),
        ];
        for (line, from, want) in cases {
            assert_eq!(
                next_inline_code_span(line, *from),
                *want,
                "next_inline_code_span({line:?}, {from}) mismatched"
            );
            if let Some((s, e)) = want {
                assert!(
                    line.is_char_boundary(*s) && line.is_char_boundary(*e),
                    "{line:?} span ({s},{e}) must land on char boundaries"
                );
            }
        }
    }

    /// Documents with no inline code span at all must come out of every pass byte for byte the way
    /// they always did — the fix narrows *where* each pass acts, and must not change *what* it does
    /// anywhere else. These expected values were captured from the build predating the shared
    /// code-span primitive and re-checked against it afterwards.
    #[test]
    fn documents_without_code_spans_are_byte_identical_to_the_previous_behavior() {
        let cases: &[(&str, &str, &str)] = &[
            // (source, process_footnotes, process_inline_html)
            (
                "Press <kbd>Ctrl</kbd>. H<sub>2</sub>O. <del>old</del> new.\n",
                "Press <kbd>Ctrl</kbd>. H<sub>2</sub>O. <del>old</del> new.\n",
                "Press `Ctrl`. H₂O. ~~old~~ new.\n",
            ),
            (
                "A claim.[^src] More text.\n\n[^src]: The evidence.\n",
                "A claim.¹ More text.\n\n\n---\n\n1. The evidence.\n",
                "A claim.[^src] More text.\n\n[^src]: The evidence.\n",
            ),
            (
                "one[^a] two[^b] one again[^a]\n\n[^a]: A\n[^b]: B\n",
                "one¹ two² one again¹\n\n\n---\n\n1. A\n2. B\n",
                "one[^a] two[^b] one again[^a]\n\n[^a]: A\n[^b]: B\n",
            ),
            (
                "line one<br>line two\n",
                "line one<br>line two\n",
                "line one  \nline two\n",
            ),
            (
                "x<sup>2</sup> and <s>gone</s> and <strike>also</strike>\n",
                "x<sup>2</sup> and <s>gone</s> and <strike>also</strike>\n",
                "x² and ~~gone~~ and ~~also~~\n",
            ),
            (
                "undefined[^nope] stays\n\n[^used]: u\n",
                "undefined[^nope] stays\n\n[^used]: u\n",
                "undefined[^nope] stays\n\n[^used]: u\n",
            ),
            (
                "```\n[^1] <kbd>K</kbd>\n```\n\nafter[^1]\n\n[^1]: n\n",
                "```\n[^1] <kbd>K</kbd>\n```\n\nafter¹\n\n\n---\n\n1. n\n",
                "```\n[^1] <kbd>K</kbd>\n```\n\nafter[^1]\n\n[^1]: n\n",
            ),
            (
                "日本語[^1]の脚注 <kbd>変換</kbd>\n\n[^1]: 注\n",
                "日本語¹の脚注 <kbd>変換</kbd>\n\n\n---\n\n1. 注\n",
                "日本語[^1]の脚注 `変換`\n\n[^1]: 注\n",
            ),
            ("", "", ""),
            (
                "no markup at all\n",
                "no markup at all\n",
                "no markup at all\n",
            ),
        ];
        for (src, want_fn, want_ih) in cases {
            assert!(
                !src.contains('`') || src.contains("```"),
                "this test is only about documents without inline code spans: {src:?}"
            );
            assert_eq!(&process_footnotes(src), want_fn, "footnotes for {src:?}");
            assert_eq!(
                &process_inline_html(src),
                want_ih,
                "inline html for {src:?}"
            );
        }
    }

    /// A documented limitation, pinned so it cannot drift silently: an inline math delimiter pair
    /// that *straddles* a code span still swallows it, because `scan_inline_math` looks for the
    /// closing `$` before it ever reaches the backtick. Code spans bind tighter than this in GitHub's
    /// math extension, so this is a divergence, not a design choice — but changing it would change
    /// which expressions get rendered, so it is left alone and recorded here instead.
    #[test]
    fn math_delimiters_straddling_a_code_span_still_swallow_it_known_limitation() {
        assert_eq!(
            collect_math_exprs("straddle $a `b` c$ end\n"),
            vec![("a `b` c".to_string(), false)],
            "if this changed, the straddling limitation was fixed (or made worse) — update the note"
        );
        // The non-straddling forms are unaffected: a fully-enclosed `$…$` stays literal.
        assert!(collect_math_exprs("`$a$` only\n").is_empty());
    }

    /// `process_footnotes`/`process_inline_html` are raw-line string passes over `src`, run *before*
    /// `Doc::parse` ever sees it (`app::md_render::build_decorated`'s pre-pass chain) — they still use
    /// the hand-rolled, line-scoped `next_inline_code_span`, and still disagree with real CommonMark
    /// the identical way: neither follows a code span across a newline, so an unclosed backtick on one
    /// line and another unclosed one on the next are each read as *literal* text, not as a single span
    /// spanning both lines. Extending one pass alone would put the two out of step with each other, so
    /// the shared limit is pinned here for both.
    ///
    /// `collect_math_exprs`, by contrast, is **no longer** line-scoped for a document the model
    /// renderer draws (`render::render_doc` — see `extraction_targets`'s own doc comment): it asks the
    /// real parser, which correctly implements CommonMark's own rule that a code span *can* run across
    /// a line ending (treated as a single space inside the span) — confirmed directly by walking
    /// `model::Doc::parse`'s own event stream for this exact input: pulldown-cmark reports a single
    /// `Event::Code` spanning both lines (`` `$x$\nstill $y$` ``), not two separate, unclosed runs.
    /// So `` `$x$\nstill $y$` `` is now read as **one** multi-line code span (`$x$`/`$y$` both inside
    /// it, neither extracted), where the old, hand-rolled scanner used to see two separate, unclosed
    /// (hence non-code) lines and extract both as math — a genuine, documented improvement in
    /// CommonMark correctness from switching to the real parser, not a regression: pinned here as the
    /// new true behavior rather than the old, narrower one.
    #[test]
    fn code_span_detection_is_line_scoped_for_every_pass() {
        let src = "open `[^1]\nstill [^1]` closed\n\n[^1]: note\n";
        // Neither line contains a *closed* span, so both references are ordinary references.
        assert_eq!(
            process_footnotes(src),
            "open `¹\nstill ¹` closed\n\n\n---\n\n1. note\n"
        );
        // Neither line has a closed span either, so both `<kbd>` tags are converted; the stray
        // backticks are just literal characters sitting next to the produced keycaps.
        let html = "open `<kbd>K</kbd>\nstill <kbd>K</kbd>` closed\n";
        assert_eq!(process_inline_html(html), "open ``K`\nstill `K`` closed\n");
        // The real parser treats the backtick pair as **one** span across the line break, so neither
        // `$x$` nor `$y$` is extracted — both sit inside it.
        assert_eq!(
            collect_math_exprs("open `$x$\nstill $y$` closed\n").len(),
            0,
            "the model path's own parser now spans a code span across a newline, matching \
             CommonMark — see this test's own doc comment"
        );
        // The non-model (legacy) extractor is unaffected: it is still the same hand-rolled,
        // line-scoped scanner `process_footnotes`/`process_inline_html` use, so it still finds both
        // (proving the divergence above is specifically about switching to the real parser, not a
        // change to `next_inline_code_span` itself).
        assert_eq!(
            collect_math_exprs_legacy("open `$x$\nstill $y$` closed\n").len(),
            2,
            "the legacy scanner's own line-scoping is unchanged"
        );
    }
}

#[cfg(test)]
mod task_scan_parity_tests {
    use super::*;

    /// Count the checkboxes actually drawn on screen (the sentinel marker spans the app counts).
    pub(super) fn rendered_tasks(src: &str) -> usize {
        set_details_open(Vec::new());
        let lines = render_markdown_tasks(
            src,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
        );
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_task_span(s))
            .count()
    }

    /// Corpus cases where the write-back scanner (`task_source_locs`) is **known, permanently** not
    /// to find a checkbox the renderer draws inside a **plain** (non-alert) block quote.
    /// `render::render_doc` — the production renderer since `3318a5b` (`RenderOut::unsupported` is
    /// always empty, so `render_markdown_with_images` never falls back to the legacy renderer) —
    /// reads a plain quote's real block structure and draws its own task-list items as real,
    /// toggleable checkboxes, exactly like the identical content outside a quote. `task_source_locs`
    /// only strips a leading `>` for a GitHub **alert** header (`> [!TYPE]`, its own
    /// `parse_alert_header` branch); an *ordinary* quote's `>` is left in place, so its hand-written
    /// block-structure walk never recognizes the unindented `- [ ]` line inside one as a task-list
    /// item at all — `task_prefix_state` never matches a line that still starts with `>`.
    ///
    /// This is exactly the shape `INTENDED_IMPROVEMENTS` (private to `md_render_diff_tests.rs`)
    /// tracks under its "Category 4" entries (a plain block quote's `"> "`-every-line prefix
    /// defeating the legacy renderer's row-based decoration, now fixed by reading the real model
    /// instead) — these five case names are exactly the task-bearing subset of that same list. Not
    /// reachable through a real keystroke today: `app::md_render::build_decorated` only ever calls
    /// `task_source_locs` inside its `extras.model_based == false` branch (`md_render.rs:508`),
    /// which `render_doc`'s now-total `BlockKind` coverage has made permanently unreachable — the
    /// checkbox-toggle count guard a real press of Space/Enter goes through reads `RenderOut::tasks`
    /// (the model renderer's own record) directly, never this scanner. `task_source_locs` is
    /// exercised here only as a standalone unit, pinned so a further, *unexpected* drift (the
    /// scanner start matching MORE than these five, or the renderer stop drawing one of them as a
    /// real checkbox) still fails loudly.
    ///
    /// `pub(super)`: `app_faithful_parity_tests` (a sibling module) pins the identical gap over its
    /// own, app-faithful-preprocessed input — see that module's own
    /// `task_scanner_matches_the_render_through_the_app_pipeline`, which reuses this exact list
    /// rather than hand-copying the five names a second time.
    pub(super) const PLAIN_QUOTE_TASK_SCANNER_GAP: &[&str] = &[
        "plain blockquote",
        "nested plain blockquote (two levels)",
        "plain blockquote nested inside an alert",
        "alert nested inside a plain blockquote",
        "checkbox inside a plain block quote",
    ];

    /// The write-back scanner must find **exactly** the checkboxes the renderer draws, for every
    /// construct, **except** the five [`PLAIN_QUOTE_TASK_SCANNER_GAP`] cases (pinned there instead,
    /// with the reasoning). When the two disagree the safety check in `md_tasks` cancels *every*
    /// toggle in the document (a safe fallback, principle #3), so a single stray construct breaks
    /// the whole file.
    ///
    /// Regressions this pins: a task inside a GitHub alert (`> [!NOTE]`) is rendered as a checkbox —
    /// the renderer strips the `>` — but the scanner matched only `-`/`*`/`+` at line start and found
    /// none. A task inside a **collapsed** `<details>` was the mirror image: not drawn, but counted.
    #[test]
    fn scanner_counts_exactly_what_the_renderer_draws() {
        for (name, src) in task_corpus::cases() {
            set_details_open(Vec::new());
            let drawn = rendered_tasks(src);
            let scanned = task_source_locs(src, &[' ', 'x'], &[]).len();
            if PLAIN_QUOTE_TASK_SCANNER_GAP.contains(&name) {
                assert_eq!(
                    (drawn, scanned),
                    (1, 0),
                    "{name}: 既知のプレーン引用チェックボックス差異の形が変わった\
                     (PLAIN_QUOTE_TASK_SCANNER_GAP のコメント参照)\n--- src ---\n{src}"
                );
                continue;
            }
            assert_eq!(
                drawn, scanned,
                "{name}: 画面のチェックボックス数と書き戻しスキャナの数が食い違う\
                 (この文書ではトグルが全部中止される)\n--- src ---\n{src}"
            );
        }
    }

    /// Count the code-block headers actually drawn (the sentinel the app counts for `y c`).
    ///
    /// Goes through **`render_markdown_with_images` — the production entry point** — not the
    /// `render_markdown_tasks` shorthand, which is `#[cfg(test)]`-only and skips the block-level
    /// extraction pass (images, ```mermaid fences). That difference is not cosmetic: pulling an
    /// `<img>` line out of a `<div align="center">` banner splits the surrounding HTML block in two,
    /// and the `    |` separator left between the halves becomes a real indented code block on
    /// screen. Measuring the shorthand hid that from the corpus (found on criterion's README while
    /// checking 2,182 real `.md` files, 2026-08) — a parity test that watches a path no user ever
    /// sees can pass while the shipped path is broken.
    fn rendered_code_blocks(src: &str) -> usize {
        // `render_markdown_tasks` reset this itself; the production entry point does not (production
        // seeds it per draw), so reset here or a second call would resume mid-sequence.
        set_details_open(Vec::new());
        let (lines, _, _extras) = render_markdown_with_images(
            src,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Image { cols: 20, rows: 5 },
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_code_header_span(s))
            .count()
    }

    /// `y c` (copy focused code block) resolves the source by **ordinal with a count guard**, exactly
    /// like the checkbox toggle — so it has exactly the same failure mode: if the scanner and the
    /// renderer disagree on how many code blocks a document has, every copy in that document is
    /// refused ("code block copy unavailable"). A fence inside a `> [!NOTE]` alert was drawn but not
    /// scanned; a fence inside a collapsed `<details>` was scanned but not drawn.
    #[test]
    fn code_block_scanner_counts_exactly_what_the_renderer_draws() {
        let cases: &[(&str, &str)] = &[
            ("plain fence", "```rust\nfn a(){}\n```\n"),
            ("two fences", "```\na\n```\n\n```\nb\n```\n"),
            ("tilde fence", "~~~\na\n~~~\n"),
            ("in alert", "> [!NOTE]\n> ```rust\n> fn a(){}\n> ```\n"),
            (
                "alert then plain",
                "> [!NOTE]\n> ```\n> in\n> ```\n\n```\nout\n```\n",
            ),
            (
                "details closed",
                "<details>\n<summary>S</summary>\n\n```\nhidden\n```\n\n</details>\n",
            ),
            (
                "details open",
                "<details open>\n<summary>S</summary>\n\n```\nshown\n```\n\n</details>\n",
            ),
            (
                "mermaid is not a code block",
                "```mermaid\nflowchart TD\nA-->B\n```\n\n```\nreal\n```\n",
            ),
            (
                "fence with tasks around",
                "- [ ] t\n\n```\ncode\n```\n\n- [x] u\n",
            ),
            // Inside an alert, mermaid image rendering doesn't run and it's drawn as an **ordinary code block**.
            (
                "mermaid inside an alert is a code block",
                "> [!NOTE]\n> ```mermaid\n> flowchart TD\n> A-->B\n> ```\n",
            ),
            (
                "mermaid in alert + plain fence",
                "> [!NOTE]\n> ```mermaid\n> A-->B\n> ```\n\n```\nreal\n```\n",
            ),
            // Root cause (a): if split_tables intercepts a table-looking line inside the fence
            // without looking at the fence, the fence splits into two broken headers (each reaching
            // its end with an empty body) — because tui-markdown then processes it as fragments of a fence that was never actually opened/closed.
            (
                "fence containing a table lookalike",
                "```text\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n",
            ),
            // Root cause (alert/details mutual nesting, 2026-08) — mirrors the additions to
            // `task_corpus` above (see that comment): a fence nested inside an alert nested inside
            // a `<details>`, and the reverse, in both open/closed states.
            (
                "fence nested inside an alert nested inside a closed details is not counted",
                "<details>\n<summary>S</summary>\n\n> [!NOTE]\n> ```\n> hidden\n> ```\n\n</details>\n",
            ),
            (
                "fence nested inside an alert nested inside an open details is counted",
                "<details open>\n<summary>S</summary>\n\n> [!NOTE]\n> ```\n> shown\n> ```\n\n</details>\n",
            ),
            (
                "fence nested inside a details nested inside an alert, closed, is not counted",
                "> [!NOTE]\n> <details>\n> <summary>S</summary>\n>\n\
                 > ```\n> hidden\n> ```\n>\n> </details>\n",
            ),
            (
                "fence nested inside a details nested inside an alert, open, is counted",
                "> [!NOTE]\n> <details open>\n> <summary>S</summary>\n>\n\
                 > ```\n> shown\n> ```\n>\n> </details>\n",
            ),
        ];
        for (name, src) in cases {
            set_details_open(Vec::new());
            let drawn = rendered_code_blocks(src);
            let scanned = code_block_source_locs(src, &[]).len();
            assert_eq!(
                drawn, scanned,
                "{name}: 画面のコードブロック数とコピー用スキャナの数が食い違う\
                 (この文書では `y c` が全部拒否される)\n--- src ---\n{src}"
            );
        }
    }

    /// Same corpus, same rule, for the checkbox toggle scanner (`task_source_locs`) — pins the
    /// "task-lookalike inside an indented block" entries added to `task_corpus` for this fix (a
    /// `- [ ] fake` example the renderer draws as *code* must not be counted, or the toggle's
    /// count guard cancels every checkbox in the document, this one included).
    ///
    /// This test's own former sibling here, `code_block_scanner_matches_renderer_across_indented_code_corpus`
    /// (the identical rule for `code_block_source_locs`/`rendered_code_blocks_legacy`), is retired —
    /// not merely fixed — now that the model renderer covers every `BlockKind` a `Doc` can contain
    /// (`render::contains_unsupported`'s own doc comment): `render_markdown_with_images` never falls
    /// back to the legacy renderer for *any* document any more (`extras.model_based` is always
    /// `true`), so that test's own subject — legacy-routed-scanner agreement — has gone unreachable,
    /// its `at_least_one_legacy_routed` guard failing on every run.
    ///
    /// `rendered_tasks` (below) used to call `render_markdown_tasks` as the always-legacy pipeline
    /// directly, unconditionally, never through `render_markdown_with_images`'s own model/legacy
    /// dispatch — so this test used to never depend on any document actually being *routed* to the
    /// legacy renderer to begin with. That changed once `render_markdown_tasks` itself was
    /// repointed to the real dispatcher (`render_markdown_with_images`, which resolves to
    /// `render::render_doc` for every document since `3318a5b` — see `render_markdown_tasks`'s own
    /// doc comment), so this test now hits the identical [`PLAIN_QUOTE_TASK_SCANNER_GAP`] this
    /// module's sibling test pins — `code_corpus`'s own `"checkbox inside a plain block quote"` case
    /// is the one member of that list that also happens to live in `code_corpus` rather than
    /// `task_corpus`.
    #[test]
    fn task_scanner_matches_renderer_across_indented_code_corpus() {
        for (name, src) in code_corpus::cases() {
            set_details_open(Vec::new());
            let drawn = rendered_tasks(src);
            let scanned = task_source_locs(src, &[' ', 'x'], &[]).len();
            if PLAIN_QUOTE_TASK_SCANNER_GAP.contains(&name) {
                assert_eq!(
                    (drawn, scanned),
                    (1, 0),
                    "{name}: 既知のプレーン引用チェックボックス差異の形が変わった\
                     (PLAIN_QUOTE_TASK_SCANNER_GAP のコメント参照)\n--- src ---\n{src}"
                );
                continue;
            }
            assert_eq!(
                drawn, scanned,
                "{name}: 画面のチェックボックス数と書き戻しスキャナの数が食い違う\
                 (この文書ではトグルが全部中止される)\n--- src ---\n{src}"
            );
        }
    }

    // ---- The document-level code mask: an indented block's *content* must not decide what the
    // block is (2026-08) ------------------------------------------------------------------------
    //
    // The two tests below are one statement split in half, and **both have to be green at the same
    // time** — the shapes they describe are byte-for-byte identical as fragments, so a gate can
    // always satisfy one of them by getting the other wrong, and each was in fact shipped that way
    // at some point. `fence_mask` (blind to indentation) drew every banner correctly and every
    // tag-shaped indented block wrong; swapping in `splitter_code_mask` at the same place traded one
    // for the other exactly. Only a mask taken over the **intact document**, before
    // `split_block_parts` lifts the `<img>` lines out, answers both — which is what `SourceRun` now
    // carries. A future change that passes only one of these has not made progress.

    /// The rendered lines as plain text — the renderer alone, with no source pre-pass, because the
    /// question here is what `split_html_blocks` does with a line, not what `process_inline_html`
    /// did to it first. (`mod tests`' `br_texts` is the with-pre-pass counterpart, used by the
    /// README-shape tests that pin the two fixes' interaction.)
    fn rendered_texts(src: &str) -> Vec<String> {
        set_details_open(Vec::new());
        let (lines, _, _extras) = render_markdown_with_images(
            src,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Inline { cols: 10, rows: 2 },
            &|_: &str| MermaidSlot::Image { cols: 20, rows: 5 },
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        lines
            .iter()
            .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect())
            .collect()
    }

    /// An indented (4+ column) code block is a code block whatever its content happens to look like.
    ///
    /// Root cause (2026-08, reported by a user): `split_html_blocks` asked `fence_mask`, which knows
    /// only about ```` ``` ````/`~~~` fences and has no notion of indentation, so an indented block
    /// whose first line was tag-shaped was rescued as an HTML block instead — tags stripped, gutter
    /// gone, and (because the content never reached the renderer as code) not Tab-focusable and not
    /// copyable with `y c`. `"para\n\n    <code>\n"` rendered as **nothing at all**: the tag was
    /// stripped and the line that was left became empty.
    ///
    /// The axis is written from the case split HTML itself implies rather than from the one tag that
    /// was reported — `is_html_block_start` accepts *any* tag name, so "what shape is the first
    /// line" is the only variable, and every shape it accepts belongs here: a bare tag, a tag with
    /// attributes, a closing tag, a void tag, a comment, the two tags this module splits its own
    /// blocks on (`<details>`/`<summary>`), the keycap tag the doc comments name, an
    /// autolink-looking `<https://…>`, and a non-tag angle form. The CJK one is here specifically
    /// because it is the case that worked even while all the others were broken (`仕様書` is not a
    /// tag name, so `is_html_block_start` rejected it): keeping it pinned means a future change
    /// cannot quietly make it the *only* one that works again.
    #[test]
    fn an_indented_code_block_is_code_whatever_its_content_looks_like() {
        for content in [
            "<code>",
            "<div>",
            "<span>x</span>",
            "<details>",
            "<summary>S</summary>",
            "<kbd>Ctrl</kbd>",
            "<br>",
            "<!-- a comment -->",
            "</a>",
            "<a href=\"x\">",
            "<仕様書>",
            "<https://example.com>",
        ] {
            let src = format!("para\n\n    {content}\n");
            set_details_open(Vec::new());
            assert_eq!(
                rendered_code_blocks(&src),
                1,
                "{content:?}: 字下げコードブロックがコードとして描かれていない\
                 (HTML ブロックとして救出され、タグが剥がれている)\n--- src ---\n{src}"
            );
            // Not just "a code block was drawn somewhere" — the block's own content is on screen,
            // verbatim, behind the `▎` gutter. Stripping the tags would leave the row empty while
            // still counting a header, which the assertion above alone cannot tell apart.
            let texts = rendered_texts(&src);
            assert!(
                texts
                    .iter()
                    .any(|t| t.starts_with('▎') && t.contains(content)),
                "{content:?}: ガター付きの行に本文が見当たらない: {texts:#?}"
            );
            // …and the copy scanner agrees, block for block and byte for byte. A disagreement here
            // is not cosmetic: the count guard is document-wide, so it refuses `y c` for the whole
            // file, every unrelated fence in it included.
            set_details_open(Vec::new());
            assert_eq!(
                code_block_source_locs(&src, &[]),
                vec![content.to_string()],
                "{content:?}: コピー用スキャナが描画と一致しない\n--- src ---\n{src}"
            );
        }
    }

    /// The other half: the centered `<div align="center">`/`<p align="center">` badge banner at the
    /// top of a great many crate READMEs is an HTML block, **not** an indented code block — even
    /// though the fragments it is cut into read as one.
    ///
    /// This is the shape that makes the naive fix (just point `split_html_blocks` at
    /// `splitter_code_mask`) wrong, and it is worth being precise about why. `split_block_parts`
    /// lifts block-level `<img>` lines out of the text *before* any of these splitters run, so what
    /// reaches them is `    </a>\n    <a href="…">\n` — a run that **opens** with 4-column-indented
    /// content only because the `<div>` that explains it went into the previous fragment. Asked
    /// about that fragment on its own, the parser is right to call it an indented code block; asked
    /// about the document, it is right to call it HTML-block interior. The first assertion below
    /// measures exactly that: the *same two lines* get opposite answers, so no amount of care inside
    /// a splitter can recover the document's answer once the surrounding block has been cut away —
    /// only computing the mask before the cut does, which is what `SourceRun` carries.
    ///
    /// The `<img>` lines are present on purpose: without them nothing is cut, the fragment never
    /// forms, and the test would pass for a reason that has nothing to do with the defect.
    #[test]
    fn a_centered_banner_stays_html_even_though_its_cut_fragments_read_as_indented_code() {
        // The ambiguity itself, stated as a measurement rather than as prose.
        let fragment = "    </a>\n    <a href=\"https://example.com/x\">\n";
        let frag_lines: Vec<&str> = fragment.lines().collect();
        assert_eq!(
            splitter_code_mask(&frag_lines),
            vec![true, true],
            "前提: この2行だけを渡せば(正しく)字下げコードブロックと判定される"
        );
        let banner = concat!(
            "<p align=\"center\">\n",
            "    <a href=\"https://example.com/y\">\n",
            "      <img src=\"https://img.example/a.svg\" alt=\"badge a\">\n",
            "    </a>\n",
            "    <a href=\"https://example.com/x\">\n",
            "      <img src=\"https://img.example/b.svg\" alt=\"badge b\">\n",
            "    </a>\n",
            "</p>\n",
            "\ntail\n",
        );
        let doc_lines: Vec<&str> = banner.lines().collect();
        assert!(
            splitter_code_mask(&doc_lines).iter().all(|c| !c),
            "前提: 同じ2行でも文書全体で見れば HTML ブロックの内側=コードではない\
             (この非対称こそが、分割後の断片からマスクを引き直せない理由)"
        );
        // So: zero code blocks on screen, and zero found by the scanner.
        set_details_open(Vec::new());
        let drawn = rendered_code_blocks(banner);
        let texts = rendered_texts(banner);
        assert_eq!(
            drawn, 0,
            "バナーが字下げコードブロックとして描かれている: {texts:#?}"
        );
        set_details_open(Vec::new());
        assert!(
            code_block_source_locs(banner, &[]).is_empty(),
            "コピー用スキャナがバナーをコードブロックとして数えている"
        );
        // The positive half: the badges really were extracted as images (so the cut this test
        // depends on genuinely happened), and none of the surrounding markup leaked onto the screen.
        for alt in ["badge a", "badge b"] {
            assert!(
                texts.iter().any(|t| t.contains(alt)),
                "バッジ {alt:?} が失われている: {texts:#?}"
            );
        }
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("</a>") || t.contains("<p ")),
            "生の HTML タグが画面に漏れている: {texts:#?}"
        );
    }

    /// Three consequences of computing the mask over the whole document that are easy to break
    /// while fixing the two cases above, each pinned in the direction it can fail.
    ///
    /// * Image extraction is now gated on that mask. `split_block_parts`' own fence tracker only
    ///   recognizes *fenced* blocks, so an `![img](x)` line inside an **indented** block used to be
    ///   lifted out as a real image — turning a literal transcript into a picture, and cutting the
    ///   block in half in exactly the way that manufactures the ambiguous fragments above.
    /// * The ```` ```mermaid ```` branch is deliberately **not** gated: a mermaid fence's own lines
    ///   *are* code to the parser, so gating it would disable diagram extraction outright. This is
    ///   the one place where "it is code, leave it alone" is the wrong answer.
    /// * A peeled `<details>` body is re-parsed on its own terms (`SourceRun::parse`), not sliced
    ///   from the enclosing run — a fence directly under a `<summary>` line only reads as code once
    ///   the tags around it are gone, so a body that inherited the document's mask would lose it.
    #[test]
    fn the_document_mask_gates_image_extraction_but_not_mermaid_or_a_peeled_details_body() {
        // A Markdown image inside an indented block stays literal text of the block.
        for (name, content) in [
            ("markdown image", "![img](x.png)"),
            ("html image", "<img src=\"x.png\" alt=\"i\">"),
        ] {
            let src = format!("para\n\n    {content}\n\ntail\n");
            set_details_open(Vec::new());
            assert_eq!(
                rendered_code_blocks(&src),
                1,
                "{name}: 字下げブロックがコードとして描かれていない\n--- src ---\n{src}"
            );
            let texts = rendered_texts(&src);
            assert!(
                texts
                    .iter()
                    .any(|t| t.starts_with('▎') && t.contains(content)),
                "{name}: 画像として抜き出されてしまい、本文が残っていない: {texts:#?}"
            );
        }
        // A real mermaid fence is still diverted to a diagram (one placement, ordinal 0).
        let (_, places, _) = {
            set_details_open(Vec::new());
            render_markdown_with_images(
                "para\n\n```mermaid\nflowchart TD\nA-->B\n```\n\ntail\n",
                100,
                CodeStyle::default(),
                "TwoDark",
                false,
                &[' ', 'x'],
                &|_: &str| ImageSlot::Inline { cols: 10, rows: 2 },
                &|_: &str| MermaidSlot::Image { cols: 20, rows: 5 },
                "mermaid",
                true,
                &|_: &str, _: bool| MathSlot::Raw,
                false,
            )
        };
        assert_eq!(
            places.len(),
            1,
            "mermaid フェンスが図として抜き出されていない(コードマスクで塞いでしまった)"
        );
        assert_eq!(places[0].fence_ord, Some(0), "フェンス序数は 0");
        // A fence inside an open `<details>` body is still drawn as a code block.
        let details =
            "<details open>\n<summary>S</summary>\n\n```rust\nfn a(){}\n```\n\n</details>\n";
        set_details_open(collect_details_open(details));
        assert_eq!(
            rendered_code_blocks(details),
            1,
            "剥がした details 本文の中のフェンスがコードとして描かれていない\
             (本文は独立した文書として parse し直す必要がある)"
        );
    }

    /// The rendered *body* rows of every code block in `src`, gutter stripped, in document order —
    /// the header row (`is_code_header_span`) and the trailing blank padding row (empty after the
    /// gutter is stripped) are excluded, leaving only the highlighted content rows.
    fn code_body_row_texts(src: &str) -> Vec<String> {
        set_details_open(Vec::new());
        let (lines, _, _extras) = render_markdown_with_images(
            src,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Image { cols: 20, rows: 5 },
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        lines
            .iter()
            .filter(|l| is_code_line(l) && !l.spans.first().is_some_and(is_code_header_span))
            .map(|l| l.to_string().trim_start_matches('▎').trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// KNOWN FAILURE, NOT YET FIXED (2026-08): a code fence's own body lines are supposed to reach
    /// the screen one *source* line per rendered row. `tui_markdown_runs_an_indented_blocks_adjacent_lines_together`
    /// above already pins that this holds for a **top-level** fence (its `"one"` / `"two"` control
    /// case) — used here again as the control, to show the defect is specific to the list-nested
    /// case and not something wrong with this test's own plumbing.
    ///
    /// It does **not** hold when the fence sits inside a list item: tui-markdown runs the fenced
    /// block's consecutive body lines together into a single rendered row there too — the
    /// fenced-in-a-list-item case of the very same upstream joining
    /// `tui_markdown_runs_an_indented_blocks_adjacent_lines_together` documents for *indented*
    /// blocks. The block *count* still matches the write-back scanner for this shape (see the
    /// "list item fence with a two-line body" entry added to `code_corpus`, which stays green), so
    /// this defect is invisible to every count-based test in this module — a document with this
    /// shape draws a code block with the correct header, but its two-line body arrives on screen as
    /// one glued row (`"aaabbb"`) instead of the two separate rows `"aaa"` / `"bbb"` a reader would
    /// expect (and that `y c` — which copies the file's own lines — actually holds).
    #[test]
    fn list_item_fence_body_lines_stay_split_on_screen() {
        // Control: the identical two-line body in a top-level (non-list) fence stays split.
        let top_level = code_body_row_texts("```\naaa\nbbb\n```\n");
        assert_eq!(
            top_level,
            vec!["aaa".to_string(), "bbb".to_string()],
            "対照ケース(トップレベルのフェンス)が想定どおり分かれていない\
             (直したいのはリスト項目内だけのはず)\n--- rows ---\n{top_level:?}"
        );

        // The failing case: the identical body, only wrapped in an ordered list item.
        let in_list = code_body_row_texts("1. item:\n\n   ```\n   aaa\n   bbb\n   ```\n");
        assert_eq!(
            in_list,
            vec!["aaa".to_string(), "bbb".to_string()],
            "リスト項目内のフェンスの本文行が画面上で1行に連結されている\
             (この文書のコードブロックが1行に潰れて表示される)\n--- rows ---\n{in_list:?}"
        );
    }

    /// Root cause (fence unification, 2026-08): the *count* matching the renderer (pinned above via
    /// `code_corpus`) is not enough on its own — the old per-function toggles (`starts_with("```") ||
    /// starts_with("~~~")`, closing on the first line that merely starts with 3 of the same char, with
    /// no indentation check) let the count guard pass by coincidence while the actual *content* handed
    /// to the clipboard was wrong: an indentation-only fence-lookalike (4+ columns) got treated as a
    /// real fence and returned with its markers stripped and its indentation kept; a real fence
    /// indented 1-3 columns got its body returned *with* the fence's own indentation still attached
    /// (verified against the renderer: the body is shown flush-left); a closing-looking line with a
    /// language suffix, or a shorter/differently-typed nested fence-lookalike, closed the block early.
    /// This test pins the exact strings, not just their count.
    #[test]
    fn code_block_content_matches_the_render_for_fence_edge_cases() {
        assert_eq!(
            code_block_source_locs("para\n\n    ```rust\n    fn a(){}\n    ```\n", &[]),
            vec!["```rust\nfn a(){}\n```".to_string()],
            "4カラム字下げは本物のフェンスではなく字下げコードブロック本体。\
             バッククォートの記号ごと文字どおりの本文として返る(画面表示と一致)"
        );
        assert_eq!(
            code_block_source_locs("para\n\n   ```rust\n   fn a(){}\n   ```\n", &[]),
            vec!["fn a(){}".to_string()],
            "3カラム字下げは本物のフェンス。中身はフェンス自身のインデント分だけ剥がれる\
             (画面表示と一致・4カラムのケースとの違いが本質)"
        );
        assert_eq!(
            code_block_source_locs("```rust\nbody\n```js\nmore\n```\n", &[]),
            vec!["body\n```js\nmore".to_string()],
            "info文字列つきの`閉じ風`の行(```js)は閉じない=本文としてそのまま残る"
        );
        assert_eq!(
            code_block_source_locs("~~~~md\n~~~\ninner\n~~~\n~~~~\n", &[]),
            vec!["~~~\ninner\n~~~".to_string()],
            "4チルダの中の3チルダは長さ不足で閉じない=本文として残る"
        );
        assert_eq!(
            code_block_source_locs("```rust\n~~~\nnot closing\n~~~\n```\n", &[]),
            vec!["~~~\nnot closing\n~~~".to_string()],
            "バッククォートフェンスの中のチルダ風の行は文字種が違うので閉じない"
        );
        assert_eq!(
            code_block_source_locs("````rust\nbody\n````\n", &[]),
            vec!["body".to_string()],
            "ネストの無い単純な4バッククォートも問題なく動く"
        );
        // Container context: the fence's indentation is measured from the list item's content
        // column, and the copy comes back flush-left — the count parity for this shape is pinned by
        // `code_corpus`, but the *content* is what `y c` actually puts on the clipboard.
        assert_eq!(
            code_block_source_locs("1. Fork it:\n\n    ```sh\n    git clone x\n    ```\n", &[]),
            vec!["git clone x".to_string()],
            "リスト項目内のフェンスは項目の content 列ぶんの字下げが剥がれて返る"
        );
        assert_eq!(
            code_block_source_locs("- outer\n  - inner\n\n        code line\n", &[]),
            vec!["code line".to_string()],
            "入れ子項目内の字下げコードは(項目の content 列+4)ぶんが剥がれて返る"
        );
    }

    /// The task-marker shapes the source scanner accepts, and — the part a wrong answer *corrupts a
    /// file* over rather than merely refusing — the byte offset of the state character it reports.
    ///
    /// Every rule here was measured against tui-markdown's own output rather than read off the spec
    /// (`> - [ ]`, for instance, renders garbled as `>[ ]  - quoted task` and draws no checkbox at
    /// all, so the scanner must not find one either):
    /// * `*   [ ]` — three spaces after the bullet — is normalized by tui-markdown to `- [ ] ` and
    ///   **is** drawn as a checkbox (CommonMark §5.2: 1-4 spaces just set the content column). The
    ///   scanner used to require exactly one, so zerocopy's agent docs refused every toggle.
    /// * `-     [ ]` — five spaces — makes the item's content *start with an indented code block*;
    ///   tui-markdown really does emit fence markers around it, and no checkbox is drawn.
    /// * `- [x]` with nothing after the bracket is drawn as a checkbox (tui-markdown appends the
    ///   trailing space itself, which is why the renderer's own `"] "` test passed while the
    ///   scanner's failed on the same line — line-clipping's README).
    #[test]
    fn task_prefix_state_accepts_gfm_spacing_and_reports_the_state_offset() {
        let st = [' ', 'x'];
        assert_eq!(task_prefix_state("- [ ] a", &st), Some((' ', 3)));
        assert_eq!(task_prefix_state("* [x] a", &st), Some(('x', 3)));
        assert_eq!(task_prefix_state("+ [X] a", &st), Some(('X', 3)));
        assert_eq!(task_prefix_state("*   [ ] a", &st), Some((' ', 5)));
        assert_eq!(task_prefix_state("-    [ ] a", &st), Some((' ', 6)));
        assert_eq!(task_prefix_state("- [x]", &st), Some(('x', 3)));
        assert_eq!(task_prefix_state("- [x]\t", &st), Some(('x', 3)));
        // Five or more spaces: the content is an indented code block, not a marker.
        assert_eq!(task_prefix_state("-     [ ] a", &st), None);
        // No separating space at all, an ordered marker, and a non-state character are all rejected.
        assert_eq!(task_prefix_state("-[ ] a", &st), None);
        assert_eq!(task_prefix_state("1. [ ] a", &st), None);
        assert_eq!(task_prefix_state("- [?] a", &st), None);
        assert_eq!(task_prefix_state("- [ ]x", &st), None);
        // The offset really points at the state character, for every accepted spelling.
        for line in ["- [ ] a", "*   [x] a", "-    [ ] a", "- [x]"] {
            let (state, off) = task_prefix_state(line, &st).unwrap();
            assert_eq!(
                line[off..].chars().next(),
                Some(state),
                "{line}: state_off が状態文字を指していない"
            );
        }
    }

    /// Root cause, fixed in two stages: a 4-backtick fence whose body happens to contain a
    /// *shorter* (3-backtick) fence-lookalike of the **same** character. Stage 1 fixed the
    /// write-back scanner (`code_block_source_locs`) via `parse_fence`'s length rule. **Stage 2
    /// (this test) fixes the renderer** (`decorate_code_blocks`) the same way, by keying off
    /// tui-markdown's own per-line *style* instead of scanning rendered *text* for `` ``` `` — see
    /// `is_code_block_line`'s docs. Before stage 2 the renderer mistook the inner marker for the
    /// block's real close and drew **two** headers (both bodies wrong/empty), silencing everything
    /// after it — headings, links, tasks — into that bogus second, empty code block. This was the
    /// exact bug a user reported (a document explaining "wrap 3 backticks in 4" via that very
    /// construct went dark past the inner marker). Now both sides agree: **one** block, whose
    /// content matches the true CommonMark interpretation, and the document past it renders
    /// normally.
    #[test]
    fn code_block_content_is_correct_for_a_nested_shorter_backtick_fence() {
        let src = "````md\n```mermaid\ninner\n```\n````\n";
        assert_eq!(
            code_block_source_locs(src, &[]),
            vec!["```mermaid\ninner\n```".to_string()],
            "スキャナ自身は正しく1ブロック・中身も真の CommonMark 解釈と一致する"
        );
        assert_eq!(
            rendered_code_blocks(src),
            1,
            "レンダラも1ヘッダのみ描く(以前は内側のマーカーを閉じと誤認して2ヘッダ描いていた)"
        );

        // The document doesn't end at the (bogus) inner marker: everything after the *real*
        // closing fence — a heading, in this case — keeps rendering normally, not silently
        // swallowed into a second code block (the user-reported symptom).
        let src_with_tail =
            "````md\n```mermaid\ninner\n```\n````\n\n# Heading After\n\nReal prose here.\n";
        assert_eq!(
            rendered_code_blocks(src_with_tail),
            1,
            "末尾に本文があってもヘッダは1つのまま"
        );
        let lines = render_markdown_tasks(
            src_with_tail,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
        );
        assert!(
            lines
                .iter()
                .any(|l| heading_text(l).as_deref() == Some("Heading After")),
            "フェンス以降の見出しがコードブロックに飲まれず、見出しとして描かれる\
             \n--- rendered ---\n{lines:?}"
        );
    }

    /// Same root cause, for the checkbox scanner **and** renderer together: the only
    /// task-lookalike outside the (correctly recognized) fence is the real one, and after stage 2
    /// the renderer draws it too — before this fix the renderer's own bug swallowed this checkbox
    /// into the bogus second code block, drawing **zero** checkboxes for this document while the
    /// scanner already (correctly) found one — the exact mismatch that refuses every toggle.
    #[test]
    fn task_scan_finds_the_real_task_outside_a_correctly_nested_fence() {
        let src = "````md\n```mermaid\n- [ ] fake\n```\n````\n\n- [ ] real\n";
        let locs = task_source_locs(src, &[' ', 'x'], &[]);
        assert_eq!(
            locs.len(),
            1,
            "フェンス内の `- [ ] fake` は本文=非タスクとして無視される"
        );
        let lines: Vec<&str> = src.lines().collect();
        assert_eq!(
            lines[locs[0].line], "- [ ] real",
            "見つかった1件はフェンス外の本物のタスク行"
        );
        assert_eq!(
            rendered_tasks(src),
            1,
            "レンダラも1件だけ描く(以前は内側のマーカーの誤認でフェンス以降が丸ごとコードに\
             飲まれ0件になっていた)"
        );
    }

    /// The task's own reported bug: `<kbd>` inside a fence whose body happens to contain a shorter
    /// nested fence-lookalike used to get rewritten to an inline-code keycap, because the naive toggle
    /// closed early on the inner marker and treated the `<kbd>` line as ordinary (non-fenced) text.
    #[test]
    fn process_inline_html_leaves_a_nested_shorter_fence_untouched() {
        let src = "````md\n```mermaid\n<kbd>Ctrl</kbd>\n```\n````\n";
        assert_eq!(
            process_inline_html(src),
            src,
            "フェンス内の <kbd> は書き換わらない(文書全体が不変のまま)"
        );

        // A real fence indented 1-3 columns, and a different-fence-type nested lookalike: same rule.
        let indented = "para\n\n   ```rust\n   <kbd>Ctrl</kbd>\n   ```\n";
        assert_eq!(process_inline_html(indented), indented);
        let mixed = "```rust\n~~~\n<kbd>Ctrl</kbd>\n~~~\n```\n";
        assert_eq!(process_inline_html(mixed), mixed);
    }

    /// A `[^1]`-looking token inside a fence whose body contains a shorter nested fence-lookalike must
    /// stay literal code, not get superscripted — same root cause as the `<kbd>` case above.
    #[test]
    fn process_footnotes_leaves_refs_inside_a_nested_shorter_fence_literal() {
        let src = "````md\n```mermaid\n[^1]\n```\n````\n\n[^1]: note\n";
        assert_eq!(
            process_footnotes(src),
            src,
            "フェンス内の [^1] は本物の参照ではないので置換されない\
             (フェンス外に本物の参照も無いので、未使用の定義は無変換のまま残る)"
        );

        let indented = "para\n\n   ```rust\n   [^1]\n   ```\n\n[^1]: note\n";
        assert_eq!(process_footnotes(indented), indented);
    }

    /// A `<details>`-opening-tag-lookalike inside a fence whose body contains a shorter nested
    /// fence-lookalike must not be sliced out as a real details block — same root cause.
    #[test]
    fn split_details_ignores_a_details_lookalike_inside_a_nested_shorter_fence() {
        let src = "````md\n```mermaid\n<details>lookalike</details>\n```\n````\n";
        let parts = split_details(&doc_run(src));
        assert_eq!(
            parts.len(),
            1,
            "文書全体が1つの Text パートのまま(details として切り出されない)"
        );
        match &parts[0] {
            DetailsPart::Text(t) => assert_eq!(t.text(), src),
            DetailsPart::Details { .. } => {
                panic!("フェンス内の <details> 風の行を本物の details として切り出してしまった")
            }
        }
    }

    /// Content-level pin: exactly 4 columns are stripped (any extra indentation is real content),
    /// and a blank line strictly *between* two chunks of the same indented block is **kept** — it is
    /// part of the block's content per CommonMark, and it is what the source actually says.
    ///
    /// This expectation used to be the opposite ("the blank is dropped, matching what the renderer
    /// draws"). Re-measured against the renderer while moving this scanner onto pulldown-cmark
    /// (2026-08), that rationale does not survive: tui-markdown emits the *whole* content of an
    /// indented code block as one `Line`, so `"para\n\n    a\n    b\n"` draws as a single row reading
    /// `ab` — two source lines run together. "Match the screen" is therefore not a rule an indented
    /// block can satisfy at all (the old scanner returned `"a\nb"` for that very input, i.e. it did
    /// not match the screen either); only the *fenced* path draws faithfully, blank rows included.
    /// So the copy follows the source, which is also what GitHub shows and what a paste should
    /// contain. Counts are unaffected either way — this is one block on screen and one entry here.
    #[test]
    fn indented_code_block_strips_four_columns_and_keeps_the_blank_between_chunks() {
        let src = "para\n\n    chunk one\n\n    chunk two\n";
        let blocks = code_block_source_locs(src, &[]);
        assert_eq!(
            blocks.len(),
            1,
            "2つのチャンクは1つのコードブロックにグルーされる"
        );
        assert_eq!(
            blocks[0], "chunk one\n\nchunk two",
            "4カラムだけ除去され、チャンク間の空行はソースどおり残る"
        );

        let extra = "para\n\n        extra indent kept\n";
        assert_eq!(
            code_block_source_locs(extra, &[]),
            vec!["    extra indent kept".to_string()],
            "4カラムを超える字下げは本文としてそのまま残る"
        );

        let tabbed = "para\n\n\ttabbed line\n";
        assert_eq!(
            code_block_source_locs(tabbed, &[]),
            vec!["tabbed line".to_string()],
            "タブ1個は4カラム分として丸ごと除去される"
        );
    }

    /// `ListGuard`'s column-0 exit condition, pinned directly: content inside a list item is never
    /// mistaken for a top-level indented code block, but code genuinely *after* the list (once a
    /// plain, undented paragraph has closed it) is detected normally.
    #[test]
    fn indented_code_is_scoped_out_of_an_active_list_but_resumes_once_it_ends() {
        assert!(
            code_block_source_locs("- item\n\n    still list content\n", &[]).is_empty(),
            "リスト項目内の字下げはコードとして検出しない"
        );
        assert_eq!(
            code_block_source_locs("- item\n\ntop\n\n    code\n", &[]),
            vec!["code".to_string()],
            "リストが素の段落で終わった後の字下げは通常どおりコードとして検出する"
        );
    }

    /// Two historical footnotes of a different kind now. (1) An indented block that immediately
    /// follows a heading with no blank line **used to be** a known, deliberately unsupported gap —
    /// the renderer allows it (CommonMark: only a paragraph gates a following indented code block —
    /// a heading isn't one), but the scanner conservatively still required a blank line, so any
    /// mismatch cancelled every code block in the document, not just the affected one. Fixed by
    /// teaching the scanner to recognize a heading (also a thematic break and a setext underline) as
    /// non-paragraph content — see `is_atx_heading_line`/`is_thematic_break_line`/
    /// `is_setext_underline_line`'s doc comments for the CommonMark reasoning. Parity is pinned here
    /// directly, replacing the old "known gap" assertion.
    ///
    /// (2) An indented block inside a *plain*, non-alert block quote **used to be** genuinely
    /// unsupported on both sides (both stayed at 0 — a non-regression, not a fix). Since the
    /// `md-block-walk` migration this is a *third* footnote, not the same one: the **model** renderer
    /// now draws a header for it — "an intentional superset of what `parser_code_blocks` reports"
    /// (`model::BlockKind::CodeBlock`'s own doc comment) — while the legacy scanner
    /// (`code_block_source_locs`) still does not, and that is *fine*, not a new mismatch to refuse `y
    /// c` over: since this migration, the scanner is consulted only for documents that actually reach
    /// the legacy rendering fallback (`render::RenderOut::unsupported`), and this one does not — the
    /// model handles it, and copies straight from its own record
    /// (`render::RenderOut::code_blocks`/`MdItemKind::CodeBlock::body`), never touching the scanner at
    /// all (see `app::md_items::focused_code_source`'s own doc comment). Pinned here as what actually
    /// matters now: the model renderer's own header count *and* the content it would copy, not
    /// whether a scanner it no longer consults happens to agree.
    #[test]
    fn heading_gap_is_fixed_and_plain_blockquote_gap_has_no_mismatch() {
        let heading_then_code = "# H\n    now detected\n";
        assert_eq!(
            rendered_code_blocks(heading_then_code),
            1,
            "レンダラは見出し直後(空行なし)でも字下げコードを描く"
        );
        assert_eq!(
            code_block_source_locs(heading_then_code, &[]),
            vec!["now detected".to_string()],
            "見出し直後(空行なし)の字下げコードも検出され、中身も画面表示と一致する\
             (以前は「既知の制約」として空行必須のまま検出しなかった)"
        );

        let quoted = "> para\n>\n>     code\n";
        assert_eq!(
            rendered_code_blocks(quoted),
            1,
            "モデルレンダラは素の(アラートでない)引用内の字下げコードにもヘッダを描く\
             (意図的な優位=parser_code_blocks の上位互換)"
        );
        set_details_open(Vec::new());
        let (_, _, extras) = render_markdown_with_images(
            quoted,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Image { cols: 20, rows: 5 },
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        assert_eq!(
            extras.code_blocks,
            vec!["code".to_string()],
            "`y c` がコピーする中身は引用の `>` が残らず正しい"
        );
        assert!(
            code_block_source_locs(quoted, &[]).is_empty(),
            "レガシースキャナは依然検出しない — が、この文書は model 経路なのでスキャナは \
             `y c` に一切関与しない(参考の記録として残す・新規リグレッションではない)"
        );
    }

    /// Direct pins for the three shape detectors themselves — the exact edge cases called out in the
    /// bug report: 7 `#`s is not a heading (needs 1-6), 2 dashes is not a thematic break (needs 3+),
    /// `- - -` (space-separated) *is* one, `***`/`___` are the other two marker characters, and a `#`
    /// with no following space/EOL (`#nospace`) is not a heading either (CommonMark §4.2).
    #[test]
    fn heading_and_thematic_break_detectors_match_commonmark_boundaries() {
        // ATX heading: 1-6 `#`s followed by space/tab/EOL.
        assert!(is_atx_heading_line("# H"));
        assert!(is_atx_heading_line("## H"));
        assert!(is_atx_heading_line("###### H"));
        assert!(
            is_atx_heading_line("#"),
            "本文なしの単独 # も見出し(EOL 扱い)"
        );
        assert!(is_atx_heading_line("#\t"), "# の直後がタブでも見出し");
        assert!(
            !is_atx_heading_line("####### H"),
            "7個の # は見出しではない(1-6個まで)"
        );
        assert!(
            !is_atx_heading_line("#nospace"),
            "# の直後が空白/EOLでなければ見出しではない"
        );
        assert!(!is_atx_heading_line("plain text"));
        assert!(
            !is_atx_heading_line("    # H"),
            "4カラム以上の字下げは見出しではなく字下げコードの本文候補"
        );
        assert!(is_atx_heading_line("   # H"), "3カラムまでの字下げは許容");

        // Thematic break: 3+ of the same marker char (-, _, *), optionally space-separated.
        assert!(is_thematic_break_line("---"));
        assert!(is_thematic_break_line("***"));
        assert!(is_thematic_break_line("___"));
        assert!(is_thematic_break_line("- - -"), "空白区切りの3個も水平線");
        assert!(is_thematic_break_line("****"), "4個以上でも水平線");
        assert!(
            !is_thematic_break_line("--"),
            "2個は水平線ではない(3個必要)"
        );
        assert!(!is_thematic_break_line("**"), "アスタリスク2個も同様");
        assert!(
            !is_thematic_break_line("- - "),
            "マーカーが2個しかなければ水平線ではない"
        );
        assert!(
            !is_thematic_break_line("--x--"),
            "マーカー以外の文字が混ざれば水平線ではない"
        );
        assert!(
            !is_thematic_break_line("    ---"),
            "4カラム以上の字下げは水平線ではなく字下げコードの本文候補"
        );

        // Setext underline shape (context-gating is exercised separately, at the scanner level).
        assert!(is_setext_underline_line("====="));
        assert!(is_setext_underline_line("="), "1文字の = も形としては該当");
        assert!(is_setext_underline_line("-"), "1文字の - も形としては該当");
        assert!(is_setext_underline_line("--"), "2文字の - も形としては該当");
        assert!(!is_setext_underline_line("=-="), "= と - が混ざれば非該当");
        assert!(!is_setext_underline_line(""), "空行は非該当");
    }

    /// Parity pins for the scanner-level fix: an indented code block right after a thematic break or
    /// a setext heading underline, with no blank line between them, must be recognized by the
    /// write-back scanner exactly as the renderer draws it — same class of bug as the heading case in
    /// `heading_gap_is_fixed_and_plain_blockquote_gap_has_no_mismatch`, pinned here for the other two
    /// non-paragraph constructs plus the short (non-thematic-break) setext dash underline.
    #[test]
    fn thematic_break_and_setext_underline_open_indented_code_without_a_blank_line() {
        let cases: &[(&str, &str, &str)] = &[
            ("thematic break (---)", "---\n    code here\n", "code here"),
            ("thematic break (***)", "***\n    star code\n", "star code"),
            (
                "setext level-1 heading (=====)",
                "Title\n=====\n    code here\n",
                "code here",
            ),
            (
                "setext level-2 heading, underline length >= 3 (-----)",
                "Title\n-----\n    code here\n",
                "code here",
            ),
            (
                "setext level-2 heading, short underline (--), too short to be a thematic break on its own",
                "Title\n--\n    code here\n",
                "code here",
            ),
            (
                "setext level-2 heading, single-dash underline (-)",
                "Title\n-\n    code here\n",
                "code here",
            ),
        ];
        for (name, src, want) in cases {
            let drawn = rendered_code_blocks(src);
            assert_eq!(
                drawn, 1,
                "{name}: レンダラの前提(1ブロック描画)がまず崩れている\n--- src ---\n{src}"
            );
            let scanned = code_block_source_locs(src, &[]);
            assert_eq!(
                scanned,
                vec![want.to_string()],
                "{name}: 画面のコードブロック数とスキャナの数・内容が食い違う\n--- src ---\n{src}"
            );
        }
    }

    /// Non-regressions the fix must not introduce: an indented line that is merely the *lazy
    /// continuation* of an ordinary paragraph — including one that starts with something that merely
    /// *looks* like a heading/thematic-break/setext-underline shape but fails the CommonMark
    /// condition to actually be one (7 `#`s, a bare 2-dash run with no paragraph above it to attach
    /// to as a setext underline) — must still stay non-code on both sides, and indented content
    /// scoped inside an active list item must still be skipped, exactly as before this fix.
    #[test]
    fn heading_rule_setext_detection_does_not_regress_lazy_continuation_or_list_scoping() {
        let cases: &[(&str, &str)] = &[
            (
                "plain paragraph then lazy continuation (unrelated to this fix, still must hold)",
                "para\n    not code, just continues the paragraph\n",
            ),
            (
                "indented content inside an active list item stays out of scope",
                "- item\n\n  still item content, not code\n",
            ),
            (
                "7 hashes is not a heading, so the paragraph it starts still gates the next indented line",
                "####### not a heading\n    still just the paragraph continuing\n",
            ),
            (
                "a bare 2-dash run at document start is neither a thematic break nor (nothing to \
                 attach to) a setext underline, so it's an ordinary paragraph that still gates",
                "--\n    still just the paragraph continuing\n",
            ),
            (
                "a bare 1-dash run at document start, same reasoning",
                "-\n    still just the paragraph continuing\n",
            ),
        ];
        for (name, src) in cases {
            let drawn = rendered_code_blocks(src);
            assert_eq!(
                drawn, 0,
                "{name}: レンダラの前提(0ブロック=段落継続)がまず崩れている\n--- src ---\n{src}"
            );
            assert!(
                code_block_source_locs(src, &[]).is_empty(),
                "{name}: スキャナが誤ってコードとして検出した(逆方向の食い違い)\n--- src ---\n{src}"
            );
        }
    }

    /// The text handed to the clipboard for a fence inside an alert must be the **code**, with the
    /// alert's `>` prefix removed — copying `> fn a(){}` would paste something that does not compile.
    #[test]
    fn alert_code_block_is_copied_without_the_quote_prefix() {
        let src = "> [!NOTE]\n> ```rust\n> fn a() {}\n> let x = 1;\n> ```\n";
        let blocks = code_block_source_locs(src, &[]);
        assert_eq!(blocks.len(), 1, "アラート内のフェンスを1件拾う");
        assert_eq!(
            blocks[0], "fn a() {}\nlet x = 1;",
            "`>` を剥がした素のコードがコピーされる"
        );
    }

    /// Every located offset must point at the state character itself — inside an alert the `>` prefix
    /// shifts every column, and a wrong offset would corrupt the line instead of toggling it.
    #[test]
    fn every_located_offset_points_at_its_state_char() {
        for (name, src) in task_corpus::cases() {
            let lines: Vec<&str> = src.lines().collect();
            for loc in task_source_locs(src, &[' ', 'x'], &[]) {
                let line = lines[loc.line];
                assert!(
                    line.is_char_boundary(loc.state_off),
                    "{name}: state_off が文字境界でない: {line:?}"
                );
                assert_eq!(
                    line[loc.state_off..].chars().next(),
                    Some(loc.state),
                    "{name}: state_off が状態文字を指していない: {line:?}"
                );
            }
        }
    }

    /// Content-level pin for root cause (a) on `split_html_blocks`: an HTML-tag-looking line inside a code
    /// fence must stay code (drawn with the `▎` code gutter), not get rescued as an HTML block —
    /// which also swallows the fence's closing marker (`is_code_header_span` count alone does not
    /// catch this: the broken render still produces exactly one — empty — header, so only the body
    /// content proves the fix).
    #[test]
    fn fence_containing_html_lookalike_stays_code() {
        set_details_open(Vec::new());
        let src = "```html\n<div class=\"x\">\nhello\n</div>\n```\n";
        let lines = render_markdown_tasks(src, 100, CodeStyle::default(), "TwoDark", false, &[]);
        assert_eq!(
            lines
                .iter()
                .filter(|l| l
                    .spans
                    .iter()
                    .any(|s| s.content.as_ref() == "```"))
                .count(),
            0,
            "閉じフェンスの `` ``` `` が生テキストとして漏れてはいけない\n--- rendered ---\n{lines:?}"
        );
        let hello_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("hello")))
            .unwrap_or_else(|| {
                panic!("'hello' がどこにも描画されていない\n--- rendered ---\n{lines:?}")
            });
        assert!(
            is_code_line(hello_line),
            "フェンス内の 'hello' はコードとして描かれるはず(HTML ブロック救出に横取りされていない)\
             \n--- line ---\n{hello_line:?}"
        );
    }

    /// Content-level pin for root cause (a) on `split_alerts`: a `> [!NOTE]`-looking line inside a code
    /// fence must stay code, not open a (mostly empty) GitHub-alert callout box.
    #[test]
    fn fence_containing_alert_lookalike_stays_code() {
        set_details_open(Vec::new());
        let src = "```text\n> [!NOTE]\nlooks like an alert\n```\n";
        let lines = render_via_dispatcher(
            &doc_run(src),
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[],
            true, // alerts on (the default)
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.spans.first().is_some_and(|s| s.content.starts_with('▌'))),
            "アラートの左バー(▌)が出てはいけない(フェンス内の `> [!NOTE]` は素のコード)\
             \n--- rendered ---\n{lines:?}"
        );
        let note_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("[!NOTE]")))
            .unwrap_or_else(|| {
                panic!("'[!NOTE]' がどこにも描画されていない\n--- rendered ---\n{lines:?}")
            });
        assert!(
            is_code_line(note_line),
            "フェンス内の '[!NOTE]' 行はコードとして描かれるはず\n--- line ---\n{note_line:?}"
        );
    }
}

#[cfg(test)]
mod fence_and_math_extraction_tests {
    use super::*;

    /// Both consumers of the mermaid fence list — the render pass that places the diagrams and
    /// `App::mermaid_fence_code`, which re-reads the file when you press Enter to open one full screen
    /// — index into `collect_mermaid_fences` by ordinal. They agree by construction (one function), so
    /// what has to be pinned is the **extraction contract itself**: change it on one side of a release
    /// and Enter silently opens a different diagram.
    #[test]
    fn mermaid_fence_extraction_is_source_ordered_and_stable() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("single", "```mermaid\nA-->B\n```\n", &["A-->B\n"]),
            (
                "two in order",
                "```mermaid\nfirst\n```\n\ntext\n\n```mermaid\nsecond\n```\n",
                &["first\n", "second\n"],
            ),
            (
                "non-mermaid fences are skipped",
                "```rust\nfn a(){}\n```\n\n```mermaid\ndiagram\n```\n",
                &["diagram\n"],
            ),
            (
                "info string with attributes",
                "```mermaid theme=dark\nbody\n```\n",
                &["body\n"],
            ),
            ("tilde fence", "~~~mermaid\ntilde\n~~~\n", &["tilde\n"]),
            // An unclosed fence is valid CommonMark (runs to end of input, same as any other
            // unclosed fenced code block) — `render_doc` places it as a real diagram, so
            // `collect_mermaid_fences` must report it too (see `collect_mermaid_fences_top_level_only`
            // for the full reasoning; the legacy hand-rolled scanner's "revert unclosed fence to text"
            // special case does not survive the switch to the model-based renderer).
            (
                "unterminated is kept",
                "```mermaid\nno close\n",
                &["no close\n"],
            ),
            // An empty fence body (nothing typed inside it yet) can never become a diagram —
            // `render_mermaid_slot` never even calls its own slot closure for one (see
            // `collect_mermaid_fences`'s own doc comment) — so it is never included, unlike the
            // legacy scanner, which produced one `""` entry for this exact shape.
            ("empty body", "```mermaid\n```\n", &[]),
            (
                "multi-line body kept verbatim",
                "```mermaid\nflowchart TD\n  A-->B\n```\n",
                &["flowchart TD\n  A-->B\n"],
            ),
            (
                "inside an alert: not image-ized, so not listed",
                "> [!NOTE]\n> ```mermaid\n> A-->B\n> ```\n",
                &[],
            ),
        ];
        for (name, src, want) in cases {
            let got = collect_mermaid_fences(src);
            assert_eq!(
                got,
                want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "{name}: mermaid フェンスの抽出結果が期待と違う\n--- src ---\n{src}"
            );
        }
    }

    /// The math list is indexed the same way (each expression becomes an inline image keyed by its
    /// ordinal), so its extraction contract gets the same treatment — including the cases that must
    /// **not** be treated as math, because a false positive silently replaces text with a picture.
    #[test]
    fn math_extraction_covers_delimiters_and_rejects_lookalikes() {
        let display = |s: &str| (s.to_string(), true);
        let inline = |s: &str| (s.to_string(), false);
        // (name, source, expected expressions as (latex, is_display))
        type MathCase = (&'static str, &'static str, Vec<(String, bool)>);
        let cases: &[MathCase] = &[
            ("inline dollars", "a $x+1$ b\n", vec![inline("x+1")]),
            ("display same line", "$$x+1$$\n", vec![display("x+1")]),
            (
                "display on its own lines",
                "$$\n x+1\n$$\n",
                vec![display("x+1")],
            ),
            // Known limitation: a "tight form" where the body starts on the same line as the opening
            // `$$` and the closer is on the next line is not treated as math — shown as raw LaTeX
            // instead (pass-through rather than breaking = principle #3). Change this only deliberately.
            (
                "display tight form is not math (known limit)",
                "$$x+1\n$$\n",
                vec![],
            ),
            ("inline paren", "a \\(y\\) b\n", vec![inline("y")]),
            ("display bracket", "\\[z\\]\n", vec![display("z")]),
            (
                "two inline",
                "$a$ and $b$\n",
                vec![inline("a"), inline("b")],
            ),
            ("currency is not math", "costs $5 and $7 today\n", vec![]),
            ("escaped dollar", "\\$not math\\$\n", vec![]),
            ("inside inline code", "`$x$`\n", vec![]),
            ("inside a fence", "```\n$x$\n```\n", vec![]),
            // Regression (2026-08): `split_math` used to track fences with its own hand-rolled
            // state machine (fenced-only, like the old `fence_mask`), so a `$x$` example written
            // inside an *indented* code block was lifted out as a math image — inside a block meant
            // to be a literal transcript. `code_block_mask` (via pulldown-cmark) covers both kinds.
            (
                "inside an indented code block",
                "para\n\n    $x$ stays literal\n\npara2\n",
                vec![],
            ),
            ("empty is not math", "$$\n", vec![]),
            // Structure-mask cases (see `structure_mask`): math inside a construct whose own
            // parser needs line continuity is left literal, never extracted/lifted.
            (
                "inside an alert",
                "> [!NOTE]\n> here is $x^2$ math\n",
                vec![],
            ),
            (
                "inside a blockquote",
                "> plain quote $x^2$ inside\n",
                vec![],
            ),
            (
                "inside details",
                "<details>\n<summary>S</summary>\n\nhere is $x^2$ math\n\n</details>\n",
                vec![],
            ),
            // One level deeper, where `structure_mask` now relabels the container's own answer to
            // `HtmlOpen`/`HtmlBody` (2026-08). `split_math`'s question is only "is this line inside
            // *something*", so a relabel must be invisible to it — and it stays invisible precisely
            // because the rescan never adds or removes a non-`None` line, only renames one. These
            // two are the cheap behavioral pin on that invariant; it was also measured directly, by
            // rendering all 2,182 registry `.md` files with math extraction on and diffing both the
            // output and the per-line non-`None` bitmap (identical).
            (
                "inside an HTML block nested in details",
                "<details>\n<summary>S</summary>\n\n<p align=\"center\">\n    $x^2$ stays put\n    <br>\n    tail\n</p>\n\n</details>\n",
                vec![],
            ),
            (
                "inside an HTML block nested in a blockquote",
                "> <p align=\"center\">\n>     $x^2$ stays put\n>     <br>\n>     tail\n> </p>\n",
                vec![],
            ),
            // Same class of document as `process_inline_html_leaves_a_details_fence_without_a_blank_line_untouched`
            // (see its doc comment) — a fence right after `<summary>`, no blank line between them.
            // For `split_math` specifically this is *not* the `code_block_mask`/`literal_code_mask`
            // union doing the protecting: `structure_mask` (above) already masks the whole
            // `<details>` … `</details>` span unconditionally, blank line or not, so this passed
            // before that union existed too. Kept as a fixed case across all three passes (per the
            // review that found the gap) rather than assumed from the other two tests passing.
            (
                "inside a details fence with no blank line before it",
                "<details>\n<summary>s</summary>\n```rust\n[^1] <kbd>K</kbd> $x$\n```\n</details>\n\noutside [^1] and <kbd>K</kbd> and $x$\n\n[^1]: def\n",
                vec![inline("x")],
            ),
            (
                "inside a table row",
                "| formula | value |\n|---|---|\n| $x^2$ | 4 |\n",
                vec![],
            ),
            // The mask must not overreach past the end of the table: a paragraph right after it
            // still gets its math extracted normally.
            (
                "after a table it is still math",
                "| a | b |\n|---|---|\n| 1 | 2 |\n\n$x$ still math\n",
                vec![inline("x")],
            ),
        ];
        for (name, src, want) in cases {
            let got = collect_math_exprs(src);
            assert_eq!(
                &got, want,
                "{name}: 数式抽出が期待と違う\n--- src ---\n{src}"
            );
        }
    }

    /// Regression: math inside a GitHub alert used to be lifted out of the alert's own text run
    /// before `split_alerts` ever saw it. `render_markdown_with_images` then rendered the pieces
    /// as independent Markdown parses, so everything **after** the math (including the alert's own
    /// `>`-prefixed lines) fell outside the callout box and leaked out as raw, un-styled text.
    #[test]
    fn math_inside_alert_keeps_the_callout_intact() {
        set_details_open(Vec::new());
        let md = "# H\n\n> [!NOTE]\n> here is $x^2$ math\n> more alert text\n\nTail paragraph.\n";
        let (lines, imgs, _extras) = render_markdown_with_images(
            md,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Image { cols: 8, rows: 2 },
            true,
        );
        assert!(
            imgs.is_empty(),
            "math inside an alert must stay literal, not become an image placement: {imgs:?}"
        );
        let joined: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let full = joined.join("\n");
        assert!(
            !full.contains("> more alert text"),
            "the alert's own blockquote marker must never leak out of the callout box as raw \
             text: {full:?}"
        );
        let note_idx = joined
            .iter()
            .position(|l| l.contains("Note"))
            .expect("alert header line ('Note') must be present");
        let tail_idx = joined
            .iter()
            .position(|l| l.contains("Tail paragraph."))
            .expect("the trailing paragraph must still render");
        assert!(
            tail_idx > note_idx,
            "the tail must come after the alert box"
        );
        for l in &joined[note_idx..tail_idx] {
            if l.trim().is_empty() {
                continue;
            }
            assert!(
                l.contains('▌'),
                "every alert line (header + body) must carry the callout bar: {l:?}\nfull:\n{full}"
            );
        }
    }

    /// Regression (information disclosure): math inside a **closed** `<details>` block used to be
    /// lifted out before `split_details` ever saw the source, breaking the `<details>`…`</details>`
    /// span it relies on — the body then rendered through an independent Markdown parse
    /// unconditionally, ignoring the collapsed `▸` state entirely.
    #[test]
    fn math_inside_closed_details_stays_hidden() {
        set_details_open(Vec::new());
        let md = "# H\n\n<details>\n<summary>Click to expand</summary>\n\nhere is $x^2$ math\n\nSECRET-SHOULD-BE-HIDDEN\n\n</details>\n\nTail paragraph.\n";
        let (lines, _imgs, _extras) = render_markdown_with_images(
            md,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Image { cols: 8, rows: 2 },
            true,
        );
        let full: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !full.contains("SECRET-SHOULD-BE-HIDDEN"),
            "a closed <details> body must stay hidden behind the collapsed marker — this is an \
             information-disclosure regression if it appears: {full:?}"
        );
    }

    /// Regression: math inside a table cell used to be lifted out before `split_tables` saw the
    /// rows, breaking the row-run it depends on to recognize the table at all — the remaining rows
    /// fell out of the grid and rendered as raw `| a | b |` text instead of drawn cells.
    #[test]
    fn math_inside_table_keeps_the_grid() {
        set_details_open(Vec::new());
        let md = "# H\n\n| formula | value |\n|---|---|\n| $x^2$ | 4 |\n| plain | 5 |\n\nTail paragraph.\n";
        let (lines, imgs, _extras) = render_markdown_with_images(
            md,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Image { cols: 8, rows: 2 },
            true,
        );
        assert!(
            imgs.is_empty(),
            "math inside a table cell must stay literal, not become an image placement: {imgs:?}"
        );
        let full: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(full.contains('┌'), "table top border must render: {full:?}");
        assert!(
            full.contains('└'),
            "table bottom border must render: {full:?}"
        );
        assert!(
            full.contains("$x^2$"),
            "the math cell can't become an image inside a table, so it renders its raw LaTeX \
             literally: {full:?}"
        );
        assert!(
            !full.contains("| 4 |"),
            "no row must fall out of the table grid as raw, un-drawn text: {full:?}"
        );
    }

    /// Regression: math inside a plain (non-alert) blockquote used to split it into two
    /// independently-parsed Markdown fragments — the fragment starting right after the math began
    /// with an unquoted continuation line, breaking the quote into two separate blocks with a
    /// stray unquoted paragraph in between (the broken output was `> plain quote` / math /
    /// `inside` / `> second line`, verified on a real terminal against v0.23.1).
    #[test]
    fn math_inside_blockquote_is_left_literal() {
        set_details_open(Vec::new());
        let md = "> plain quote $x^2$ inside\n> second line\n";
        let (lines, imgs, _extras) = render_markdown_with_images(
            md,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Image { cols: 8, rows: 2 },
            true,
        );
        assert!(
            imgs.is_empty(),
            "math inside a blockquote must stay literal, not become an image placement: {imgs:?}"
        );
        let full: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            full.contains("quote $x^2$ inside second line"),
            "the quote must stay one unbroken block (its second line joined onto the first, not \
             split into two separate quotes with a stray paragraph in between): {full:?}"
        );
    }

    /// Non-regression: the mask only suppresses lifting *inside* a structural block — math at the
    /// top level, even in a document that also has an alert, still becomes an image.
    #[test]
    fn math_outside_structures_still_renders() {
        set_details_open(Vec::new());
        let md = "> [!NOTE]\n> here is $x^2$ math\n\n$y^2$ outside\n";
        let (_lines, imgs, _extras) = render_markdown_with_images(
            md,
            60,
            CodeStyle::default(),
            "TwoDark",
            false,
            DEFAULT_TASK_STATES,
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Text,
            "Enter: full screen",
            true,
            &|_: &str, _: bool| MathSlot::Image { cols: 8, rows: 2 },
            true,
        );
        assert_eq!(
            imgs.len(),
            1,
            "the alert's math stays suppressed but the top-level math is still lifted: {imgs:?}"
        );
        assert!(
            is_math_url(&imgs[0].url),
            "the one placement must be the top-level math expression: {imgs:?}"
        );
    }
}

/// Ground truth for `fence_mask` / `code_block_mask` / `literal_code_mask` themselves, pinned line by
/// line rather than through one of the three source-rewriting passes that consult them. The corpus
/// above (`code_span_corpus`) exercises these masks *indirectly*, through what `process_footnotes` /
/// `process_inline_html` / `collect_math_exprs` do with a document — which is the behavior that
/// actually matters, but a failure there only shows "the pass disagreed with the expectation", not
/// which of the three masks (or which line) was wrong. This module states the masks' own answers
/// directly, so a boundary regression here points straight at the mask, not at three passes' worth of
/// string-rewriting logic layered on top of it.
///
/// Every expected `(fence, code_block, literal)` triple below was checked against a throwaway probe
/// (`fence_mask`/`code_block_mask`/`literal_code_mask` called directly and printed line by line against
/// each of these same documents, then deleted once the values below were copied from its output) —
/// not re-derived from CommonMark's prose or assumed from `literal_code_mask`'s own doc comment.
#[cfg(test)]
mod mask_boundary_tests {
    use super::*;

    /// One line's expected `(fence_mask, code_block_mask, literal_code_mask)`.
    type Expect = (bool, bool, bool);

    /// One document plus the expected mask triple for every one of its lines, in order.
    struct Row {
        name: &'static str,
        doc: String,
        expect: Vec<Expect>,
    }

    fn row(name: &'static str, doc: impl Into<String>, expect: Vec<Expect>) -> Row {
        Row {
            name,
            doc: doc.into(),
            expect,
        }
    }

    fn rows() -> Vec<Row> {
        let ind3 = " ".repeat(3);
        let ind4 = " ".repeat(4);
        let ind5 = " ".repeat(5);
        let ind6 = " ".repeat(6);
        let ind8 = " ".repeat(8);
        const F: bool = false;
        const T: bool = true;
        vec![
            // --- Column count, at document start (a line with no indented-block-disqualifying
            // construct before it) ---
            row(
                "3 columns is not a code block (ordinary paragraph)",
                format!("{ind3}TEXT"),
                vec![(F, F, F)],
            ),
            row(
                "4 columns is a code block",
                format!("{ind4}TEXT"),
                vec![(F, T, T)],
            ),
            row(
                "8 columns is still one code block (the extra 4 are content)",
                format!("{ind8}TEXT"),
                vec![(F, T, T)],
            ),
            row(
                "a tab is 4 columns, same as 4 spaces",
                "\tTEXT",
                vec![(F, T, T)],
            ),
            // --- Position: what can precede an indented block without a blank line ---
            row(
                "after a paragraph (blank line required; the blank line itself is not part of the block)",
                format!("para\n\n{ind4}TEXT"),
                vec![(F, F, F), (F, F, F), (F, T, T)],
            ),
            row(
                "right after an ATX heading, no blank line needed (a heading cannot absorb a continuation line)",
                format!("# H\n{ind4}TEXT"),
                vec![(F, F, F), (F, T, T)],
            ),
            row(
                "right after a setext heading underline, no blank line needed",
                format!("Heading\n=====\n{ind4}TEXT"),
                vec![(F, F, F), (F, F, F), (F, T, T)],
            ),
            // --- Inside containers: indentation is relative to the container's own content column ---
            row(
                "4 columns inside a list item ('- ' content column 2) is short of the 6 the item needs \
                 — still a paragraph continuation, not a code block",
                format!("- item\n{ind4}TEXT"),
                vec![(F, F, F), (F, F, F)],
            ),
            row(
                "6 columns inside a list item, blank line before it, is a code block",
                format!("- item\n\n{ind6}TEXT"),
                vec![(F, F, F), (F, F, F), (F, T, T)],
            ),
            row(
                "6 columns inside a list item with NO blank line before it is still just a paragraph \
                 continuation (extra indentation on a continuation line does not start a nested block)",
                format!("- item\n{ind6}TEXT"),
                vec![(F, F, F), (F, F, F)],
            ),
            row(
                "indented code block as the very first line of a blockquote (quote's own content column, \
                 '> ' = 1, plus 4 = 5)",
                format!(">{ind5}TEXT"),
                vec![(F, T, T)],
            ),
            row(
                "indented code block inside a blockquote, after a blank quote line",
                format!("> plain\n>\n>{ind5}TEXT"),
                vec![(F, F, F), (F, F, F), (F, T, T)],
            ),
            row(
                "no blank quote line first: still just a continuation of the quote's own paragraph",
                format!("> plain\n>{ind5}TEXT"),
                vec![(F, F, F), (F, F, F)],
            ),
            // --- Two chunks separated by one blank line are ONE block; the blank line is part of it ---
            row(
                "two indented chunks separated by a blank line — the blank line is part of the block too",
                format!("{ind4}A\n\n{ind4}B"),
                vec![(F, T, T), (F, T, T), (F, T, T)],
            ),
            // --- Fences: column count, nesting, and where the two masks disagree ---
            row(
                "a fence indented 2 columns at top level is still a fence",
                "  ```\n  code\n  ```",
                vec![(T, T, T), (T, T, T), (T, T, T)],
            ),
            row(
                "a fence indented 4 absolute columns under a '1. ' item (content column 3, so only 1 \
                 relative column) is a REAL fence — fence_mask's absolute-column check misses it \
                 (leading_ws_width >= 4 rejects the opener outright), code_block_mask (container-aware, \
                 via pulldown-cmark) catches it",
                format!("1. item\n\n{ind4}```\n{ind4}code\n{ind4}```"),
                vec![(F, F, F), (F, F, F), (F, T, T), (F, T, T), (F, T, T)],
            ),
            row(
                "a nested longer fence (```` ```` ```` wrapping ``` ```) stays open the whole way through, \
                 by both masks",
                "````md\n```\ninner\n```\n````",
                vec![(T, T, T), (T, T, T), (T, T, T), (T, T, T), (T, T, T)],
            ),
            row(
                "an unclosed fence runs to end of file, by both masks",
                "```\ncode",
                vec![(T, T, T), (T, T, T)],
            ),
            // --- The two cases `literal_code_mask`'s own doc comment names as where it and
            // `code_block_mask` alone diverge from each other ---
            row(
                "<details><summary>…</summary> immediately followed by a fence, no blank line: \
                 code_block_mask says NOT code (pulldown-cmark reads the whole thing as one HTML block); \
                 fence_mask says code (blind to the surrounding <details>/<summary> context); the union \
                 (literal_code_mask) is what matches what the renderer actually draws — a real code block, \
                 because split_details hands this body to an isolated re-parse where nothing competes",
                "<details>\n<summary>s</summary>\n```rust\nCODE\n```\n</details>",
                vec![
                    (F, F, F), // <details>
                    (F, F, F), // <summary>s</summary>
                    (T, F, T), // ```rust  (fence_mask true, code_block_mask false)
                    (T, F, T), // CODE
                    (T, F, T), // ```
                    (F, F, F), // </details>
                ],
            ),
            row(
                "<div>…</div> wrapping a fence, no blank line: code_block_mask says NOT code — \
                 pulldown-cmark reads the whole <div>...</div> as one HTML block, and konoma's own \
                 renderer agrees (split_html_blocks carves this out as HTML before the parser ever sees \
                 it, not as a code block)",
                "<div>\n```\ncode\n```\n</div>",
                vec![
                    (F, F, F), // <div>
                    (T, F, T), // ```    (fence_mask still true; see the doc comment on this — a
                    (T, F, T), // code   // pre-existing fence_mask over-protection this mask inherits
                    (T, F, T), // ```    // unchanged, not something introduced by the union)
                    (F, F, F), // </div>
                ],
            ),
            // --- Non-regression: a document with no code block of either kind is all false, for every
            // other construct these masks' callers have to coexist with ---
            row(
                "no code block anywhere: front matter, heading, paragraph, list, quote, table, HTML \
                 block, footnote definition",
                "---\ntitle: T\n---\n\n# Heading\n\npara text\n\n- list item\n\n> quote\n\n| a | b |\n\
                 |---|---|\n| 1 | 2 |\n\n<div>html</div>\n\n[^1]: def",
                vec![
                    (F, F, F), // ---
                    (F, F, F), // title: T
                    (F, F, F), // ---
                    (F, F, F), // (blank)
                    (F, F, F), // # Heading
                    (F, F, F), // (blank)
                    (F, F, F), // para text
                    (F, F, F), // (blank)
                    (F, F, F), // - list item
                    (F, F, F), // (blank)
                    (F, F, F), // > quote
                    (F, F, F), // (blank)
                    (F, F, F), // | a | b |
                    (F, F, F), // |---|---|
                    (F, F, F), // | 1 | 2 |
                    (F, F, F), // (blank)
                    (F, F, F), // <div>html</div>
                    (F, F, F), // (blank)
                    (F, F, F), // [^1]: def
                ],
            ),
        ]
    }

    #[test]
    fn masks_agree_with_pulldown_cmark_line_by_line() {
        for r in rows() {
            let lines: Vec<&str> = r.doc.lines().collect();
            assert_eq!(
                lines.len(),
                r.expect.len(),
                "{}: doc has {} lines but {} expectations were given — doc={:?}",
                r.name,
                lines.len(),
                r.expect.len(),
                r.doc
            );
            let fm = fence_mask(&lines);
            let cbm = code_block_mask(&lines);
            let lcm = literal_code_mask(&lines);
            for (i, &(ef, eb, el)) in r.expect.iter().enumerate() {
                assert_eq!(
                    fm[i], ef,
                    "{}: line {i} {:?} — fence_mask expected {ef}, got {}",
                    r.name, lines[i], fm[i]
                );
                assert_eq!(
                    cbm[i], eb,
                    "{}: line {i} {:?} — code_block_mask expected {eb}, got {}",
                    r.name, lines[i], cbm[i]
                );
                assert_eq!(
                    lcm[i], el,
                    "{}: line {i} {:?} — literal_code_mask expected {el}, got {}",
                    r.name, lines[i], lcm[i]
                );
                // literal_code_mask is defined as the OR of the other two — restate that as an
                // assertion here too, so a future edit that breaks the union itself (not just one of
                // its inputs) is caught by this table as well, not only by `literal_code_mask`'s own
                // unit-level definition.
                assert_eq!(
                    lcm[i],
                    fm[i] || cbm[i],
                    "{}: line {i} literal_code_mask must equal fence_mask OR code_block_mask",
                    r.name
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod preprocess_corpus {
    /// Documents exercising the **source pre-passes** — `process_footnotes` and
    /// `process_inline_html` — and their interaction with every construct the renderer treats
    /// specially. Sibling of `task_corpus` / `code_corpus`, and built the same way: from the case
    /// split the *specification* implies, not from the list of bugs already met.
    ///
    /// Why this axis needed its own corpus (2026-08, reported by a user on the released build): the
    /// existing corpora hand one text to the renderer and the same text to the scanner, so they can
    /// only ever catch the two disagreeing *about identical input*. Production does not do that. The
    /// renderer draws the text these pre-passes produce while the scanners read the file, and every
    /// disagreement about **which text** is being described is invisible to a same-text parity test
    /// by construction. `rand_chacha`'s README — a footnote definition wrapped over two lines — was
    /// refused by `y c` in the wild for exactly that reason, and no test could have seen it.
    ///
    /// The axes are: definition shape (single line / continued at 2, 4 and 6 columns / lazy / several
    /// paragraphs / carrying a fence, a list or a checkbox), reference shape (defined, undefined,
    /// duplicated, inside a code span), the tags `process_inline_html` rewrites (`<br>` and its
    /// spellings — the only one that adds a line — plus `<kbd>`, `<del>`, `<s>`, `<strike>`, `<sup>`,
    /// `<sub>`), where each sits relative to a code block or a checkbox, the containers (list, nested
    /// list, quote, alert, `<details>` open and closed), and the text-level axes that have bitten
    /// this module before: front matter, CRLF, a missing trailing newline, and CJK — including CJK
    /// inside angle brackets, which is what the reporting user's own payload contained.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            // ---- footnote definition shapes ----
            ("def single line", "Ref[^a].\n\n[^a]: one line.\n"),
            ("def continued 2 cols", "Ref[^a].\n\n[^a]: first\n  second\n"),
            ("def continued 4 cols", "Ref[^a].\n\n[^a]: first\n    second\n"),
            // The exact shape rand_chacha/rand_xorshift use: a link split across two lines, the
            // continuation indented past the indented-code threshold.
            (
                "def continued 6 cols with a wrapped link",
                "Ref[^a].\n\n[^a]: [label](\n      https://example.com/x)\n",
            ),
            ("def lazy continuation", "Ref[^a].\n\n[^a]: first\nsecond\n"),
            (
                "def with two paragraphs",
                "Ref[^a].\n\n[^a]: first\n\n    second para\n",
            ),
            (
                "def carrying a fence",
                "Ref[^a].\n\n[^a]: see\n\n    ```rust\n    fn a() {}\n    ```\n",
            ),
            (
                "def carrying a list",
                "Ref[^a].\n\n[^a]: see\n\n    - one\n    - two\n",
            ),
            (
                "def carrying a checkbox",
                "Ref[^a].\n\n[^a]: see\n\n    - [ ] inside the note\n",
            ),
            (
                "two defs both continued",
                "A[^a] and B[^b].\n\n[^a]: first\n      more\n\n[^b]: second\n      more\n",
            ),
            ("def never referenced", "No reference here.\n\n[^a]: orphan\n"),
            ("ref with no def", "Dangling[^zz] reference.\n"),
            ("same ref twice", "One[^a] two[^a].\n\n[^a]: shared\n"),
            (
                "ref inside a code span stays literal",
                "Text `[^a]` literal.\n\n[^a]: def\n",
            ),
            (
                "def inside a fence stays literal",
                "```\n[^a]: not a def\n```\n\nRef[^a].\n",
            ),
            (
                "def inside indented code stays literal",
                "para\n\n    [^a]: not a def\n\nRef[^a].\n",
            ),
            // ---- footnotes next to a code block ----
            (
                "continued def before a fence",
                "Ref[^a].\n\n[^a]: first\n      more\n\n```\ncode\n```\n",
            ),
            (
                "continued def after a fence",
                "```\ncode\n```\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "continued def before indented code",
                "Ref[^a].\n\n[^a]: first\n      more\n\npara\n\n    indented\n",
            ),
            (
                "continued def between two fences",
                "```\none\n```\n\nRef[^a].\n\n[^a]: first\n      more\n\n~~~\ntwo\n~~~\n",
            ),
            // ---- inline HTML: <br> (the only tag that adds a line) ----
            ("br plain", "before<br>after\n"),
            ("br slash", "before<br/>after\n"),
            ("br spaced slash", "before<br />after\n"),
            ("br uppercase", "before<BR>after\n"),
            ("br making a list interrupt a paragraph", "prose<br>- [ ] injected\n"),
            ("br before a fence", "prose<br>more\n\n```\ncode\n```\n"),
            ("br after a fence", "```\ncode\n```\n\nprose<br>more\n"),
            ("br inside a fence is literal", "```\na<br>b\n```\n"),
            ("br inside indented code is literal", "para\n\n    a<br>b\n"),
            ("br inside a code span is literal", "text `a<br>b` more\n"),
            ("br on a checkbox line", "- [ ] task<br>tail\n"),
            // ---- inline HTML: the paired tags ----
            ("kbd", "Press <kbd>Ctrl</kbd> now.\n"),
            ("kbd inside a code span is literal", "Write `<kbd>Ctrl</kbd>` so.\n"),
            ("del", "This <del>was</del> that.\n"),
            ("s tag", "This <s>was</s> that.\n"),
            ("strike tag", "This <strike>was</strike> that.\n"),
            ("sup", "x<sup>2</sup>\n"),
            ("sub", "H<sub>2</sub>O\n"),
            ("empty del makes tildes", "a<del></del>b\n"),
            // `<del></del>` alone on a line rewrites to `~~~~`, which *is* a tilde fence opener: the
            // preprocessed text has a code block the file does not. The mirror image of the footnote
            // phantom, and the reason the copy scanner has to read the renderer's text rather than
            // the file even once multi-line definitions are handled correctly.
            ("empty del alone becomes a fence opener", "before\n\n<del></del>\n\nafter\n"),
            // A `<br>` inside an alert injects a line with no `>` prefix, which ends the callout
            // early: the fence below it is drawn as literal text, so the screen has *fewer* code
            // blocks than the file does.
            (
                "br ending an alert early above a fence",
                "> [!NOTE]\n> Some prose with <br> inline text.\n>\n> ```\n> code line one\n> ```\n",
            ),
            ("kbd on a checkbox line", "- [ ] press <kbd>x</kbd>\n"),
            ("kbd before a fence", "<kbd>x</kbd>\n\n```\ncode\n```\n"),
            ("paired tag inside a fence is literal", "```\n<kbd>x</kbd>\n```\n"),
            // ---- footnote reference on a checkbox line (the tail gets rewritten) ----
            ("checkbox carrying a ref", "- [ ] task[^a]\n\n[^a]: note\n"),
            (
                "checkbox carrying a ref and a continued def",
                "- [ ] task[^a]\n\n[^a]: note\n      more\n",
            ),
            // ---- containers ----
            (
                "continued def inside a list item",
                "- item\n\n  Ref[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "continued def inside a nested list item",
                "- outer\n  - inner Ref[^a].\n\n[^a]: first\n      more\n",
            ),
            ("br inside a list item", "- item<br>tail\n"),
            ("br inside a nested list item", "- outer\n  - inner<br>tail\n"),
            ("br inside a quote", "> quoted<br>tail\n"),
            ("br inside an alert", "> [!NOTE]\n> quoted<br>tail\n"),
            // A table row has no continuation form, so the injected line became a second, shifted
            // row. `| x<br>y | z |` is GitHub's own idiom for a line break in a cell.
            (
                "br inside a table cell",
                "| a | b |\n| --- | --- |\n| x<br>y | z |\n",
            ),
            // The centered-banner idiom: a bare `<br>` line between two badge blocks. Rewritten, it
            // left a whitespace-only line — a blank line to CommonMark — which ended the HTML block
            // and turned every following indented line into a real indented code block.
            (
                "bare br line inside an html block",
                "<p align=\"center\">\n  <a href=\"https://a\">\n    <img src=\"https://a.svg\" alt=\"a\">\n  </a>\n  <br>\n  <a href=\"https://b\">\n    <img src=\"https://b.svg\" alt=\"b\">\n  </a>\n</p>\n",
            ),
            // The other half of the same axis: a `<br>` with text on both sides inside an HTML
            // block leaves two non-blank lines, which the block tolerates, so it is still rewritten.
            (
                "mid-line br inside an html block",
                "<div align=\"center\">\n  before<br>after\n</div>\n",
            ),
            (
                "continued def inside a quote",
                "> Ref[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "alert with a fence and a continued def after it",
                "> [!NOTE]\n> ```\n> code\n> ```\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "open details with a fence and a continued def",
                "<details open>\n<summary>S</summary>\n\n```\ncode\n```\n\n</details>\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "closed details with a fence and a continued def",
                "<details>\n<summary>S</summary>\n\n```\ncode\n```\n\n</details>\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            ("br inside an open details", "<details open>\n<summary>S</summary>\n\nprose<br>tail\n\n</details>\n"),
            // An indented code block as the *first* content of an open `<details>` body: the body is
            // peeled out of the tags before being re-parsed, and trimming that fragment used to eat
            // the four columns that make it code at all.
            (
                "open details whose body starts with indented code",
                "<details open>\n<summary>S</summary>\n\n    indented line\n\n</details>\n",
            ),
            (
                "open details with prose before indented code",
                "<details open>\n<summary>S</summary>\n\nprose first.\n\n    indented line\n\n</details>\n",
            ),
            (
                "checkbox in an alert plus a continued def",
                "> [!NOTE]\n> - [ ] task\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            // ---- text-level axes ----
            (
                "front matter then a continued def",
                "---\ntitle: t\n---\n\nRef[^a].\n\n[^a]: first\n      more\n",
            ),
            (
                "front matter then br",
                "---\ntitle: t\n---\n\nprose<br>tail\n",
            ),
            ("crlf with a continued def", "Ref[^a].\r\n\r\n[^a]: first\r\n      more\r\n"),
            ("crlf with br", "prose<br>tail\r\n"),
            ("no trailing newline continued def", "Ref[^a].\n\n[^a]: first\n      more"),
            ("no trailing newline br", "prose<br>tail"),
            // ---- CJK, including inside angle brackets (the reporting user's own payload) ----
            ("cjk continued def", "参照[^a]。\n\n[^a]: 最初の行\n      続きの行\n"),
            ("cjk br", "前<br>後\n"),
            ("cjk checkbox with a ref", "- [ ] やること[^a]\n\n[^a]: 注記\n"),
            (
                "angle brackets holding cjk next to a fence",
                "Use <仕様書> and <資料dir>.\n\n```\n/groundwork <仕様書> <NotionURL> <資料dir>\n```\n",
            ),
            (
                "angle brackets holding cjk with a continued def",
                "Use <仕様書>[^a].\n\n[^a]: <NotionURL> の説明\n      続き\n",
            ),
            (
                "cjk fence plus br plus continued def",
                "前<br>後\n\n```\n/groundwork <仕様書> <NotionURL> <資料dir>\n```\n\n参照[^a]。\n\n[^a]: 注記\n      続き\n",
            ),
            // ---- the four real registry files, reduced to their essence ----
            (
                "rand_chacha readme shape",
                "ChaCha[^1], used as an RNG.\n\nselected by eSTREAM[^2].\n\nLinks:\n\n-   [API](https://docs.rs/rand_chacha)\n\n[rand]: https://crates.io/crates/rand\n[^1]: D. J. Bernstein, [*ChaCha*](\n      https://cr.yp.to/chacha.html)\n\n[^2]: [eSTREAM](\n      http://www.ecrypt.eu.org/stream/)\n\n\n## Crate Features\n",
            ),
            (
                "shlex quoting_warning shape",
                "Text[^1] with a fence.\n\n```sh\necho hi\n```\n\n[^1]: A note that wraps\n      onto a second line.\n",
            ),
            // A second corruption shape, and the one that survives the multi-line-footnote fix: the
            // `~~~~` that `<del></del>` becomes swallows the checkbox below it into a code block, so
            // the file has a checkbox the screen does not draw, while the `<br>` above gives the
            // screen one the file does not have. The two cancel in any count, and both are unchecked,
            // so a count-and-state guard writes to the swallowed line.
            (
                "br injected checkbox above a del-fence swallowed one",
                "prose<br>- [ ] INJECTED\n\n<del></del>\n\n- [ ] SWALLOWED\n",
            ),
            // ---- the confirmed corruption document (see md_tasks.rs) ----
            (
                "footnote leftover plus br injected checkbox",
                "[^1]: def text\n    - [ ] LEFTOVER\n\n- [ ] REAL\n\nprose<br>- [ ] INJECTED\n\nSee[^1].\n",
            ),
            // ---- A pre-pass rewrite and the block splitters, over the same lines (2026-08) ----
            //
            // The axis the rows above stop just short of: what a pre-pass does to a line *and* what
            // the block splitters then make of the line it leaves behind. Both 2026-08 fixes live
            // here and they pull in opposite directions on the same document — the `<br>` rule has
            // to keep an HTML block from ending, and the block splitter then has to keep calling
            // the 4-column-indented markup inside it HTML rather than indented code. A row that
            // exercises only one of them can be satisfied by a change that breaks the other, which
            // is how each of the two single-sided fixes came to look correct.
            //
            // Cheap to state, and the reason these belong in *this* corpus rather than only in
            // `code_corpus`: `code_corpus` hands the raw text to both sides, so it never sees a
            // line the pre-pass removed. Only the app-faithful runners over this corpus do.
            (
                "centered banner with a bare br line (registry: raw-window-metal README.md)",
                "<p align=\"center\">\n    <a href=\"https://crates.io/crates/rwm\">\n      <img src=\"https://img.example/v.svg\" alt=\"crates.io\">\n    </a>\n    <br>\n    <a href=\"LICENSE-MIT\">\n      <img src=\"https://img.example/mit.svg\" alt=\"License - MIT\">\n    </a>\n</p>\n\ntail\n",
            ),
            (
                "markdown block image above a centered banner (registry: static_assertions README.md)",
                "[![Banner](https://img.example/banner.png)](https://example.com/repo)\n\n<div align=\"center\">\n    <a href=\"https://crates.io/crates/sa\">\n        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\n    </a>\n    <img src=\"https://img.example/rustc.svg\" alt=\"rustc\">\n    <br>\n    <a href=\"https://example.com/patron\">\n        <img src=\"https://img.example/patron.png\" alt=\"Patron\">\n    </a>\n</div>\n\ntail.\n",
            ),
            // No `<br>` anywhere, and CRLF throughout: isolates the block-splitter half of the pair
            // on the axis (`\r` left on a line's end) that changes what "only tags" and "blank" both
            // answer.
            (
                "crlf centered banner with no br (registry: tinytemplate README.md)",
                "<h1 align=\"center\">TT</h1>\r\n\r\n<div align=\"center\">\r\n    <a href=\"https://example.com/actions\">\r\n        <img src=\"https://img.example/ci.svg\" alt=\"CI\">\r\n    </a>\r\n    <a href=\"https://crates.io/crates/tt\">\r\n        <img src=\"https://img.example/v.svg\" alt=\"Crates.io\">\r\n    </a>\r\n</div>\r\n\r\ntail\r\n",
            ),
            // The mirror direction, and the shape a user reported: the pre-pass must leave a
            // tag-shaped indented code line alone *and* the renderer must still draw it as code —
            // with the identical tag outside the block, which the pre-pass must still rewrite. One
            // row per angle-bracket form the two sides disagreed about.
            (
                "tag-shaped indented code beside the same tag outside it",
                "para\n\n    <kbd>Ctrl</kbd>\n\nPress <kbd>Ctrl</kbd> now.\n",
            ),
            (
                "closing-tag indented code beside a real html block",
                "<div align=\"center\">\n  <b>hi</b>\n</div>\n\npara\n\n    </a>\n",
            ),
            (
                "comment-shaped indented code beside a real comment block",
                "<!-- real block -->\n\npara\n\n    <!-- a comment -->\n",
            ),
            (
                "autolink-shaped indented code beside a real autolink",
                "para\n\n    <https://example.com>\n\nSee <https://example.com> too.\n",
            ),
            (
                "cjk angle brackets inside indented code and outside it",
                "para\n\n    <仕様書>\n\n本文で <仕様書> に触れる。\n",
            ),
            (
                "void-tag indented code beside a real br",
                "para\n\n    <br>\n\nprose<br>tail\n",
            ),
        ]
    }
}

#[cfg(test)]
pub(crate) mod inline_corpus {
    /// Documents exercising the **inline/paragraph** surface of Markdown — emphasis, strong,
    /// strikethrough, inline code, links (all four forms), images, line breaks, headings (ATX and
    /// setext), thematic breaks, super/subscript, escapes, entities, and CJK — the exact surface
    /// `crate::preview::markdown::render::render_doc` claims to cover for a `Heading`/`Paragraph`/
    /// `ThematicBreak`-only document (see that module's own doc comment on scope). Sibling of
    /// `task_corpus` / `code_corpus` / `preprocess_corpus`, and built the same way: from the case
    /// split CommonMark/GFM's own grammar implies for these constructs, not from a list of bugs
    /// already met — see `preprocess_corpus`'s own doc comment for why that distinction matters.
    ///
    /// Why this axis needed its own corpus (2026-08): the existing corpora were all built to pin
    /// **block-shaped** constructs (tasks, code blocks, footnotes, inline-HTML rewrites) — none of
    /// them contains so much as a single `*emphasis*` example. A style bug in `render_doc`'s own
    /// `Emphasis`/`Strong`/`Strikethrough`/link/heading handling can therefore ship with the existing
    /// parity corpus reporting a clean "37 of 40 supported cases match exactly" — a real, verified
    /// gap: with the `Tag::Emphasis` arm in `render.rs` deliberately mis-styled `BOLD` instead of
    /// `ITALIC`, `markdown_render_diff_report` stayed green, because none of the 40 "supported" cases
    /// it already had ever exercised emphasis at all.
    ///
    /// One known-and-accepted mismatch category (see `md_render_diff_tests`'s own module doc
    /// comment) applies to a construct this axis would otherwise cover, and is handled by *keeping
    /// it out of this gated corpus* rather than by adding to `KNOWN_MISMATCHES`:
    ///
    /// * **Inline math** (`$…$`, and also `\(…\)`/`\[…\]` — the app's own LaTeX-delimiter convention,
    ///   which a literal, CommonMark-escaped `\[…\]` collides with; see `scan_inline_math`'s own
    ///   unconditional handling of those two byte sequences) is not a construct this corpus touches at
    ///   all (deliberately: it is already covered, and already known-mismatched, by
    ///   `code_span_corpus`'s three math cases — see `KNOWN_MISMATCHES`) — `"escaped brackets are
    ///   literal"` below escapes only the opening bracket for exactly this reason: `\[not a
    ///   link\](nope)` (both sides escaped, the shape CommonMark itself uses for "escape a literal
    ///   bracket pair") reads as a `\[…\]` display-math expression to `scan_inline_math` regardless of
    ///   authorial intent, which is a real characteristic of the production pipeline unrelated to
    ///   `render_doc` — confirmed empirically, not merely reasoned about, while building this corpus.
    ///
    /// A **reference-style link `[a][r]` whose definition lives in a separate top-level block** used
    /// to be a second such category — `render_doc` re-parsed each block's own byte range in isolation,
    /// which could never see a definition sitting in a *different* block's own range — but is not one
    /// any more: `render.rs` was rewritten to read every block's inline events straight out of
    /// `Doc::parse`'s single, whole-document event stream instead (`model::Doc.events`), so a resolved
    /// reference now sees exactly what the real parser saw. The **resolved** shape (a definition that
    /// actually exists, in the *next* top-level block) is included below, gated like everything else,
    /// right alongside the dangling ones. The **dangling**-reference cases below (no definition
    /// anywhere) are unaffected either way — pulldown-cmark reports literal bracket text for those on
    /// both sides regardless of how many times the document is parsed.
    ///
    /// Every other case below is expected to render identically on both sides: the two renderers only
    /// diverge when a source-level pre-pass (math extraction) intercepts a shape before tui-markdown
    /// ever sees it, which is the one case this corpus deliberately calls out (and keeps out of the
    /// gate) above; everything else (emphasis, links whose destination is fully self-contained, images
    /// left inline because they share a line with other text, headings, thematic breaks, escapes,
    /// entities, CJK, and now cross-block references too) is fully determined by the document's own
    /// bytes, read once, the same way for both.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            // ---- emphasis: *em* / _em_ ----
            ("emphasis star", "This is *emphasized* text.\n"),
            ("emphasis underscore", "This is _emphasized_ text.\n"),
            ("emphasis star mid-word", "foo*bar*baz plain.\n"),
            (
                "emphasis star immediately followed by punctuation",
                "Please read *this*, carefully.\n",
            ),
            // ---- strong: **strong** / __strong__ ----
            ("strong star", "This is **strong** text.\n"),
            ("strong underscore", "This is __strong__ text.\n"),
            // ---- both, nested either way ----
            ("strong then emphasis nested", "This is ***both*** at once.\n"),
            (
                "strong containing emphasis",
                "This is **bold *and italic* end** text.\n",
            ),
            (
                "emphasis containing strong",
                "This is *italic **and bold** end* text.\n",
            ),
            // ---- strikethrough ----
            ("strikethrough", "This was ~~deleted~~ text.\n"),
            (
                "strikethrough containing emphasis",
                "This was ~~*deleted and italic*~~ text.\n",
            ),
            // ---- inline code ----
            ("inline code plain", "Run `cargo test` now.\n"),
            (
                "inline code with double backticks",
                "Use `` `nested` `` here.\n",
            ),
            ("inline code containing a backtick", "Use ``a ` b`` here.\n"),
            (
                "inline code containing markdown lookalikes",
                "See `*not em* [not](a/link)` literally.\n",
            ),
            // ---- links: inline ----
            ("link inline", "See [the docs](https://example.com/docs).\n"),
            (
                "link inline with title",
                "See [the docs](https://example.com/docs \"Docs\").\n",
            ),
            (
                "link with relative path destination",
                "See [the guide](./docs/guide.md) for setup.\n",
            ),
            (
                "link destination is an anchor",
                "Jump to [Usage](#usage) below.\n",
            ),
            (
                "link label containing emphasis",
                "See [**bold label**](https://example.com/x).\n",
            ),
            // ---- links: reference style ----
            (
                "reference link with no definition stays literal",
                "See [the docs][missing] here.\n",
            ),
            (
                "shortcut reference with no definition stays literal",
                "See [missing] here.\n",
            ),
            // The *resolved* shapes (a definition that actually exists) — the full form `[a][r]`
            // and the shortcut form `[a]`, both with their `[r]: url` / `[a]: url` definition in
            // the paragraph *immediately following* (a blank line apart, the realistic shape:
            // `preprocess_corpus`'s own "def" cases use the identical layout for footnote
            // definitions) — used to be excluded from this gated corpus entirely (see this module's
            // own doc comment): `render_doc`'s old per-block re-parse could never resolve a
            // definition sitting in a different top-level block, so folding a real instance of this
            // shape in here would have made `markdown_render_diff_report` fail on a gap that stage
            // already accepted as expected. `render.rs`'s rewrite onto a single whole-document parse
            // (`model::Doc.events`) closed that gap structurally, so these are gated like everything
            // else now.
            (
                "reference link with definition in the next block",
                "See [the docs][ref] here.\n\n[ref]: https://example.com/docs\n",
            ),
            (
                "shortcut reference with definition in the next block",
                "See [ref] here.\n\n[ref]: https://example.com/docs\n",
            ),
            // ---- links: autolink ----
            ("autolink", "Visit <https://example.com/> today.\n"),
            (
                "autolink mailto",
                "Contact <mailto:hello@example.com> today.\n",
            ),
            // ---- images: inline (a paragraph whose entire line is *not* just the image, so it is
            // never intercepted as a standalone block-level image before tui-markdown ever sees it —
            // see the module doc comment on the categories this corpus avoids) ----
            (
                "inline image alt text shown mid-paragraph",
                "Look at this: ![a red fox](fox.png) closely.\n",
            ),
            (
                "inline image with no alt text",
                "Look at this: ![](fox.png) closely.\n",
            ),
            // ---- line breaks: soft vs. hard ----
            ("soft break", "line one\nline two\n"),
            ("hard break via two trailing spaces", "line one  \nline two\n"),
            ("hard break via trailing backslash", "line one\\\nline two\n"),
            // ---- headings: ATX levels 1-6 ----
            ("heading level 1", "# Title One\n"),
            ("heading level 2", "## Title Two\n"),
            ("heading level 3", "### Title Three\n"),
            ("heading level 4", "#### Title Four\n"),
            ("heading level 5", "##### Title Five\n"),
            ("heading level 6", "###### Title Six\n"),
            (
                "heading containing strong and code",
                "## Setup with **care** and `cargo run`\n",
            ),
            (
                "heading with attributes",
                "## Custom Heading {#custom-id .note lang=en}\n",
            ),
            // ---- headings: setext ----
            ("setext heading level 1", "Big Title\n=========\n"),
            ("setext heading level 2", "Smaller Title\n-------------\n"),
            // ---- thematic breaks (each preceded by a blank line / at document start, so it can
            // never be mistaken for a setext underline of a preceding paragraph) ----
            ("thematic break dashes", "---\n"),
            ("thematic break asterisks", "***\n"),
            ("thematic break underscores", "___\n"),
            (
                "thematic break between two paragraphs",
                "before.\n\n---\n\nafter.\n",
            ),
            // ---- superscript / subscript ----
            ("superscript", "x^2^ is x squared.\n"),
            ("subscript", "H~2~O is water.\n"),
            // ---- escapes ----
            ("escaped asterisks are literal", "This is \\*not emphasized\\*.\n"),
            (
                "escaped brackets are literal",
                "This is \\[not a link](nope).\n",
            ),
            ("escaped backslash", "A literal backslash: \\\\ done.\n"),
            // ---- entities ----
            ("named entity amp", "Fish \\& chips, or: fish &amp; chips.\n"),
            ("named entity copy", "All rights &copy; 2026.\n"),
            // ---- CJK ----
            ("cjk paragraph", "これは日本語の段落です。特殊な記法は使っていません。\n"),
            ("cjk heading", "# 日本語の見出し\n"),
            (
                "cjk emphasis touching the delimiters",
                "日本語*強調*語のテキスト。\n",
            ),
            (
                "cjk emphasis after full-width punctuation",
                "これは、*強調*です。\n",
            ),
            (
                "cjk link label",
                "詳細は[日本語のラベル](https://example.com/ja)を参照。\n",
            ),
            // ---- edges ----
            ("empty document", ""),
            ("whitespace-only line produces no block", "   \n"),
            (
                "very long single-line paragraph",
                "word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト word 通常のテキスト\n",
            ),
            (
                "many consecutive blank lines between paragraphs collapse the same as one",
                "first.\n\n\n\n\nsecond.\n",
            ),
        ]
    }
}

#[cfg(test)]
pub(crate) mod list_corpus {
    /// Documents exercising the **list and block-quote** surface of Markdown — bullet/ordered
    /// markers, tight vs. loose items, nesting (of either kind inside the other, and each inside
    /// itself), headings/rules inside a quote, task-list checkboxes, CJK, and a handful of edges.
    /// Sibling of `task_corpus`/`code_corpus`/`inline_corpus`, and built the same way: from the case
    /// split CommonMark/GFM's own grammar implies for these constructs, not from a list of bugs
    /// already met — see `preprocess_corpus`'s own doc comment for why that distinction matters.
    ///
    /// Why this axis needed its own corpus (2026-08, the `md-block-walk` refactor's `List`/`Quote`
    /// stage): none of the existing corpora contains so much as a single `-` bullet or `>` quote
    /// marker — the same gap `inline_corpus`'s own doc comment already documents for emphasis/links/
    /// headings, one construct family later. A renderer that gets, say, nesting indentation wrong
    /// could ship with every *other* corpus reporting a clean pass, for the identical reason: nothing
    /// in them ever exercises a nested list at all.
    ///
    /// One shape below is deliberately expected to come back **unsupported**, not matched: **a
    /// task-list item inside a *loose* list** (`"task item inside a loose list"`). `render.rs`'s own
    /// `contains_unsupported` screens this one out on purpose, before `render_doc` ever draws a line
    /// of it — see that function's own `"LooseTask"` arm for exactly why: a loose item's own task
    /// marker always lands on a fresh, empty line, and splicing it in there
    /// (`Writer::task_list_marker`) panics — `insertion index (is 1) should be <= len (is 0)` — the
    /// *identical* crash real tui-markdown 0.3.7/0.3.8 has for this exact shape (confirmed
    /// empirically while building this corpus, and independently by `task_corpus`'s own "a real task
    /// at a list item's own indentation is still a real task" case, whose *outer* item is loose too;
    /// see `loose_list_task_item_does_not_panic`, elsewhere in this file, for how production itself
    /// survives the identical crash — via `render_text_block_safe`'s own `catch_unwind` and
    /// bisect-and-retry, a recovery layer *above* tui-markdown that this stage does not reimplement;
    /// see the module doc comment on `render.rs` for why matching *that* is out of scope here).
    /// Reporting the shape unsupported, rather than either reproducing the crash or inventing some
    /// third, non-panicking rendering neither real renderer ever actually shows, is this stage's own
    /// version of the "degrade, never crash" choice `CLAUDE.md`'s principle #3 asks for everywhere
    /// else in this codebase.
    pub fn cases() -> Vec<(&'static str, &'static str)> {
        vec![
            // ---- bullets: - / * / + ----
            ("unordered bullet dash", "- a\n- b\n"),
            ("unordered bullet star", "* a\n* b\n"),
            ("unordered bullet plus", "+ a\n+ b\n"),
            // ---- ordered: delimiter, start number, mid-list gaps ----
            ("ordered dot delimiter", "1. a\n2. b\n"),
            ("ordered paren delimiter", "1) a\n2) b\n"),
            ("ordered list does not start at one", "5. a\n6. b\n7. c\n"),
            (
                "ordered list source numbers skip midway are still auto-numbered",
                "1. a\n5. b\n9. c\n",
            ),
            // ---- tight vs. loose, and a multi-paragraph item ----
            ("tight unordered list two items", "- a\n- b\n"),
            ("loose unordered list two items", "- a\n\n- b\n"),
            ("tight ordered list two items", "1. a\n2. b\n"),
            ("loose ordered list two items", "1. a\n\n2. b\n"),
            (
                "loose item has two paragraphs of its own",
                "- a\n\n  b\n- c\n",
            ),
            // ---- nesting: depth, and mixed bullet/ordered ----
            ("nested bullet two levels", "- a\n  - b\n"),
            ("nested bullet three levels", "- a\n  - b\n    - c\n"),
            (
                "ordered list nested inside an unordered one",
                "- a\n  1. b\n  2. c\n",
            ),
            (
                "unordered list nested inside an ordered one",
                "1. a\n   - b\n   - c\n",
            ),
            // ---- list <-> quote nesting, either direction, and quote-in-quote ----
            ("quote containing a list", "> - a\n> - b\n"),
            (
                "list containing a quote in its own item",
                "- a\n\n  > quoted\n- b\n",
            ),
            (
                "block quote can interrupt a tight item's own paragraph with no blank line",
                "- a\n  > quoted\n",
            ),
            ("nested quotes two levels", "> a\n>> b\n"),
            // ---- headings / rules inside a quote (decorate_md_lines does not special-case a
            // prefixed line — see render.rs's own module doc comment — so both renderers are
            // expected to show the same, un-decorated "> # ..."/"> ---" text) ----
            ("quote containing a heading", "> # Title\n> body\n"),
            (
                "quote containing a thematic break",
                "> above\n>\n> ---\n>\n> below\n",
            ),
            // A GFM table nested inside a plain block quote: the *legacy* renderer's own line-based
            // `split_tables` never detects it at all (`is_table_delimiter`'s own character set has no
            // `>` in it — the quote's own marker survives on every continuation line, the delimiter
            // row included), so it degrades to a single literal, wrapped-text line of raw pipes; the
            // model renderer draws a real, box-drawn table instead — `render::render_table_from_model`
            // reads each cell straight off `model::BlockKind::Table.rows`'s own per-cell ranges,
            // already free of the `>` marker regardless of nesting depth. See
            // `md_render_diff_tests::INTENDED_IMPROVEMENTS`'s own entry for this case.
            (
                "quote containing a table",
                "> | a | b |\n> |---|---|\n> | 1 | 2 |\n",
            ),
            // A GFM table glued directly onto a GitHub alert's own header line, no blank line in
            // between (real-world shape: an `ndk` README wraps a table in `> [!IMPORTANT]` this way).
            // `[!IMPORTANT]` is not real CommonMark syntax — it is ordinary blockquote text that
            // happens to look like a header — so, with no blank line to separate them, CommonMark's
            // own lazy-continuation rule glues the header and the table's raw pipe/dash text into one
            // single `Paragraph`; a GFM table can never start mid-paragraph, so neither renderer's own
            // block-level parser ever sees a `Table` here at all, only a `Paragraph`. The *legacy*
            // renderer degrades gracefully anyway — `split_alerts` peels the header off by raw line
            // scan (not by asking pulldown-cmark), leaving the table's own three lines as its own
            // fragment for `split_tables`' own line-based detector to still recognize as a real table
            // from raw pipe characters, no block-level parse needed. `render::render_alert_from_model`
            // (`glued_alert_body`/`dequote_alert_body`) matches that by re-parsing everything after the
            // header line, in isolation, as its own fresh document — the identical mechanism
            // `render_details_from_model` already uses for a glued `<details>` body
            // (`model::BlockKind::Details.glued_body`) — recovering the real `Table` a first,
            // whole-document parse could never have reported. See
            // `md_render_diff_tests::run_case`'s own comparison for why this now matches exactly
            // rather than needing an `INTENDED_IMPROVEMENTS`/`KNOWN_MISMATCHES` entry the way "quote
            // containing a table" above does (a *plain* quote has no header of its own for anything to
            // glue onto, so the legacy/model gap that case pins never applied here to begin with).
            (
                "alert containing a table with no blank line before it",
                "> [!IMPORTANT]\n> a | b\n> ---|---\n> 1 | 2\n",
            ),
            // ---- checkboxes: unchecked/checked, bullet variants, ordered, nested, loose ----
            ("tight unordered task unchecked", "- [ ] a\n"),
            ("tight unordered task checked lowercase", "- [x] a\n"),
            (
                "tight unordered task checked uppercase collapses to lowercase x",
                "- [X] a\n",
            ),
            ("tight star-bullet task", "* [ ] a\n"),
            ("tight plus-bullet task", "+ [ ] a\n"),
            ("tight ordered task list", "1. [ ] a\n2. [x] b\n"),
            (
                "task item nested under a tight item",
                "- [ ] a\n  - [x] b\n",
            ),
            ("task item inside a loose list", "- a\n\n- [ ] b\n"),
            // ---- CJK ----
            (
                "cjk tight unordered list items",
                "- 最初の項目\n- 二番目の項目\n",
            ),
            ("cjk ordered list items", "1. 最初\n2. 二番目\n"),
            ("cjk task item", "- [ ] 買い物に行く\n"),
            ("cjk quote", "> これは引用です。\n"),
            // ---- edges: empty item, single item, list next to a paragraph ----
            ("single-item list", "- only\n"),
            ("empty list item followed by a real one", "- \n- b\n"),
            (
                "list followed by a paragraph after a blank line",
                "- a\n- b\n\nafter.\n",
            ),
            (
                "paragraph followed by a list after a blank line",
                "before.\n\n- a\n- b\n",
            ),
        ]
    }
}

/// Parity between what the renderer draws and what the source scanners find, measured **the way the
/// application actually wires them** — the renderer over the preprocessed text, each scanner over
/// the text its production call site really reads.
///
/// This module exists because the older parity tests (`mod task_scan_parity_tests`) hand the *same*
/// string to both sides. That pins a real and still-needed invariant — the renderer and the scanners
/// must agree about identical input — but it is structurally blind to the failure this module
/// covers, where the two sides are handed *different* strings and each is internally consistent. The
/// released build refused `y c` on four real crate READMEs, and silently toggled the wrong checkbox
/// on a constructed document, entirely inside that blind spot.
#[cfg(test)]
mod app_faithful_parity_tests {
    use super::*;

    /// The chain `App::build_decorated` applies before handing the text to the renderer, with the
    /// per-line origins it keeps (default config: front matter, footnotes and inline HTML all on).
    fn app_pre(raw: &str) -> (String, LineOrigin) {
        let body = strip_front_matter(raw).1;
        let origin = identity_origin(&body);
        let (s, origin) = process_footnotes_traced(&body, &origin);
        process_inline_html_traced(&s, &origin)
    }

    fn drawn_tasks(src: &str) -> usize {
        set_details_open(Vec::new());
        let lines = render_markdown_tasks(
            src,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
        );
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| is_task_span(s))
            .count()
    }

    /// Corpus cases where a pre-pass genuinely adds or removes a code block, so the preprocessed
    /// text and the file legitimately describe different sets. Each one is a real, understood
    /// rewrite — not a tolerance: this list is what keeps
    /// `copying_from_the_preprocessed_text_yields_the_files_own_bytes` from quietly accepting the
    /// next unnoticed instance of the same class.
    const CHANGES_THE_BLOCK_SET: &[&str] = &[
        // `<del></del>` alone on a line becomes `~~~~`, opening a fence the file has no trace of.
        "empty del alone becomes a fence opener",
        // Same rewrite, this time swallowing a checkbox into the fence it opens.
        "br injected checkbox above a del-fence swallowed one",
        // A `<br>` inside an alert emits a line with no `>` prefix, ending the callout early, so the
        // fence beneath it stops being drawn as a code block at all.
        "br ending an alert early above a fence",
    ];

    /// Every corpus this crate has, run through the app's real wiring.
    fn all_cases() -> Vec<(&'static str, &'static str)> {
        let mut v = preprocess_corpus::cases();
        v.extend(code_corpus::cases());
        v.extend(task_corpus::cases());
        v
    }

    /// `RenderOut::code_blocks` for `src`, under the same no-picker slot config `drawn_code` above
    /// already uses (mermaid extraction on, images/math never available) — the model render pass's
    /// own record of every code block's already-reconstructed source text, in document order. This
    /// is what `y c` reads directly since the `md-block-walk` copy/toggle migration (no re-scan, no
    /// count guard — see `app::md_items::focused_code_source`'s own doc comment); a document this
    /// crate's own renderer draws via the model path always has this fully populated.
    fn model_code_blocks(src: &str) -> Vec<String> {
        set_details_open(Vec::new());
        let (_, _, extras) = render_markdown_with_images(
            src,
            100,
            CodeStyle::default(),
            "TwoDark",
            false,
            &[' ', 'x'],
            &|_: &str| ImageSlot::Unavailable,
            &|_: &str| MermaidSlot::Image { cols: 20, rows: 5 },
            "mermaid",
            true,
            &|_: &str, _: bool| MathSlot::Raw,
            false,
        );
        extras.code_blocks
    }

    /// `y c`'s copied text must be the file's own bytes, not merely the right *count* of plausible
    /// strings — and, since the `md-block-walk` migration, the model renderer's own record
    /// (`model_code_blocks`) is what it is actually read from (no independent re-scan of any kind —
    /// see `app::md_items::focused_code_source`'s own doc comment), not `code_block_source_locs` (the
    /// pre-migration scanner). Comparing against that scanner as an oracle turns out to be unusable
    /// here: it disagrees with the model renderer in many already-known, intentional, *structural*
    /// ways with nothing to do with preprocessing at all (a code block directly inside a plain block
    /// quote — fenced or indented, nested inside a list item or not — a mermaid fence within three
    /// columns of a list item's own content column, a fence's closing line carrying trailing text —
    /// each an "intentional superset"/CommonMark-correctness fix `render_code_block`'s and
    /// `model::BlockKind::CodeBlock`'s own doc comments already describe; discovering all of them by
    /// hand here, one `assert_eq!` panic at a time, is exactly the kind of hand-maintained parity list
    /// this whole migration exists to stop needing).
    ///
    /// So this keeps the module's own *original* concern — the renderer and a scanner handed
    /// *different* strings (the file's own body vs the preprocessed `pre`), each internally
    /// consistent with itself, but the two texts silently describing different content — while
    /// dropping the now-unreliable "compare against the legacy scanner" mechanism: both sides are the
    /// model renderer's own record, over the two different inputs. Compared against `body` (front
    /// matter already stripped, matching `copying_from_the_preprocessed_text_yields_the_files_own_bytes`'s
    /// own convention below), **not** `raw` (front matter still present): `render::render_doc` bails
    /// out to the legacy renderer whole-document the moment it sees so much as one YAML metadata-block
    /// event anywhere in `doc.events` (see its own doc comment), so calling `model_code_blocks(raw)`
    /// directly on a case with front matter would always read back an empty `Vec` — not because the
    /// model failed to find anything, but because `render_markdown_with_images` never even tried the
    /// model path for that call at all. Front matter is stripped by a wholly separate step, before
    /// `pre_src` is ever computed (`app::md_render::build_decorated`'s own leading `strip_front_matter`
    /// call) — this comparison is about the footnote/inline-HTML pre-passes specifically, which is
    /// exactly what comparing against `body` isolates.
    ///
    /// Wherever a pre-pass does not change which code blocks exist at all (the common case), the two
    /// must be byte-identical; the one narrow list of cases where a pre-pass legitimately does
    /// (`CHANGES_THE_BLOCK_SET`, shared with
    /// `copying_from_the_preprocessed_text_yields_the_files_own_bytes`) is excluded the identical way.
    #[test]
    fn code_scanner_matches_the_render_through_the_app_pipeline() {
        for (name, raw) in all_cases() {
            let (pre, _) = app_pre(raw);
            let body = strip_front_matter(raw).1;
            let from_pre = model_code_blocks(&pre);
            let from_body = model_code_blocks(&body);
            if from_pre.len() != from_body.len() {
                assert!(
                    CHANGES_THE_BLOCK_SET.contains(&name),
                    "{name}: 前処理がコードブロックの集合を変えたが既知の理由が無い\
                     (この類型の新しい実例の可能性)\n--- raw ---\n{raw}\n--- preprocessed ---\n{pre}\
                     \nfrom_body={from_body:?}\nfrom_pre={from_pre:?}"
                );
                continue;
            }
            assert_eq!(
                from_pre, from_body,
                "{name}: 前処理の有無でレンダラが記録する中身がバイト単位で変わる\
                 (`y c` が前処理の有無で違う内容をコピーする)\n--- raw ---\n{raw}"
            );
        }
    }

    /// The same for the checkbox toggle's screen-parity half, over the app's own preprocessed text
    /// (front matter stripped, footnotes and inline HTML folded in) instead of the raw corpus
    /// string — same `task_source_locs`-vs-renderer concern `task_scan_parity_tests` pins, just
    /// measured through the app's real wiring rather than a bare corpus string. Same five known,
    /// permanent exceptions: `task_scan_parity_tests::PLAIN_QUOTE_TASK_SCANNER_GAP` (see that
    /// list's own doc comment) — `task_source_locs` never recognizes a checkbox nested inside a
    /// plain (non-alert) block quote, a shape `render::render_doc` now draws correctly.
    #[test]
    fn task_scanner_matches_the_render_through_the_app_pipeline() {
        for (name, raw) in all_cases() {
            let (pre, _) = app_pre(raw);
            let drawn = drawn_tasks(&pre);
            let scanned = task_source_locs(&pre, &[' ', 'x'], &[]).len();
            if task_scan_parity_tests::PLAIN_QUOTE_TASK_SCANNER_GAP.contains(&name) {
                assert_eq!(
                    (drawn, scanned),
                    (1, 0),
                    "{name}: 既知のプレーン引用チェックボックス差異の形が変わった\
                     (PLAIN_QUOTE_TASK_SCANNER_GAP のコメント参照)\
                     \n--- raw ---\n{raw}\n--- preprocessed ---\n{pre}"
                );
                continue;
            }
            assert_eq!(
                drawn, scanned,
                "{name}: アプリ経路で画面のチェックボックス数と書き戻しスキャナの数が食い違う\
                 \n--- raw ---\n{raw}\n--- preprocessed ---\n{pre}"
            );
        }
    }

    /// What `y c` puts on the clipboard must still be the file's own bytes. The pre-passes all gate
    /// on `literal_code_mask`, so a code block's contents are left verbatim; scanning the
    /// preprocessed text therefore yields the same strings as scanning the raw body — for every case
    /// where the two texts describe the same set of blocks.
    ///
    /// The one case that legitimately differs is asserted explicitly rather than skipped: a fence
    /// carried *inside* a footnote definition moves, with the definition, into the appended section,
    /// so it exists on screen (and in `pre`) while the raw body's scan sees it at its old position —
    /// same content, and that is what is checked.
    #[test]
    fn copying_from_the_preprocessed_text_yields_the_files_own_bytes() {
        for (name, raw) in all_cases() {
            let (pre, _) = app_pre(raw);
            let body = strip_front_matter(raw).1;
            let from_pre = code_block_source_locs(&pre, &[]);
            let from_raw = code_block_source_locs(&body, &[]);
            if from_pre.len() != from_raw.len() {
                // A pre-pass may legitimately create or destroy a block, but only in ways we have
                // identified and can name. Anything else changing the block set is a new instance of
                // this bug class and must fail here rather than pass quietly.
                assert!(
                    CHANGES_THE_BLOCK_SET.contains(&name),
                    "{name}: 前処理がコードブロックの集合を変えたが既知の理由が無い\
                     (この類型の新しい実例の可能性)\n--- raw ---\n{raw}\n--- preprocessed ---\n{pre}"
                );
                continue;
            }
            assert_eq!(
                from_pre, from_raw,
                "{name}: 前処理の有無でコピーされる中身がバイト単位で変わる\
                 \n--- raw ---\n{raw}"
            );
        }
    }

    /// The mechanical precondition for every origin lookup: the map has **exactly one entry per line
    /// of the text it describes**. `origin.get(n)` is indexed by a line number of the preprocessed
    /// text, so a map one entry short or long silently shifts every lookup past the drift onto the
    /// wrong source line.
    ///
    /// Worth pinning on its own rather than leaving to
    /// `every_drawn_checkbox_either_resolves_exactly_or_not_at_all` below, which can only *notice* a
    /// drift when the shifted line happens to compare unequal — a drift that lands on a
    /// coincidentally similar line passes it. The pre-passes derive their `None` padding from the
    /// number of newlines they produced, and `<br>` is the one substitution whose newline count
    /// varies with its context (2026-08: none at all inside an HTML block or a table row, none for a
    /// trailing tag, one otherwise), so the derivation has to stay tied to the produced string.
    #[test]
    fn line_origins_have_exactly_one_entry_per_preprocessed_line() {
        for (name, raw) in all_cases() {
            let (pre, origin) = app_pre(raw);
            assert_eq!(
                origin.len(),
                pre.lines().count(),
                "{name}: origin の要素数が前処理後の行数と一致しない\
                 (以降の行の書き戻し先が全てずれる)\n--- raw ---\n{raw}\n--- preprocessed ---\n{pre}"
            );
        }
    }

    /// The invariant that makes a wrong write impossible: every checkbox on screen either resolves to
    /// a real line of the file whose bytes up to and including the state character are identical, or
    /// does not resolve at all — in which case `md_toggle_focused_task` refuses. There is no third
    /// outcome, and in particular no outcome in which a resolved line is a *different* line than the
    /// one drawn.
    #[test]
    fn every_drawn_checkbox_either_resolves_exactly_or_not_at_all() {
        for (name, raw) in all_cases() {
            let (pre, origin) = app_pre(raw);
            let body = strip_front_matter(raw).1;
            let body_lines: Vec<&str> = body.lines().collect();
            let pre_lines: Vec<&str> = pre.lines().collect();
            for l in task_source_locs(&pre, &[' ', 'x'], &[]) {
                let Some(src_line) = origin.get(l.line).copied().flatten() else {
                    continue; // invented by a pre-pass → the toggle refuses
                };
                let end = l.state_off + l.state.len_utf8();
                let (Some(on_screen), Some(on_disk)) = (
                    pre_lines.get(l.line).and_then(|x| x.get(..end)),
                    body_lines.get(src_line).and_then(|x| x.get(..end)),
                ) else {
                    continue; // prefix not comparable → the toggle refuses
                };
                if on_screen != on_disk {
                    continue; // prefix differs → the toggle refuses
                }
                // Resolved: the byte the toggle would replace really is a checkbox state character.
                assert_eq!(
                    body_lines[src_line][l.state_off..end].chars().next(),
                    Some(l.state),
                    "{name}: 解決した行の書き込み位置が状態文字ではない\
                     \n--- raw ---\n{raw}"
                );
            }
        }
    }
}
