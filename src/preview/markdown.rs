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
use ratatui::text::{Line, Span, Text};
// `Options` is already tui-markdown's *style* options here, so the parser's own option type takes
// the name tui-markdown itself uses for it internally (`ParseOptions`) — see
// `tui_markdown_parse_options`, which must stay in lockstep with tui-markdown's own set.
use pulldown_cmark::Options as ParseOptions;
use tui_markdown::{Options, StyleSheet};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
/// chars (e.g. `/`) are also recognized as toggleable task markers. Test-only shorthand — production
/// calls `render_markdown_tasks_opts` (with the `ui.md_alerts` flag) via `render_markdown_with_images`.
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
    // Default entry (tests / the `render_markdown` wrapper): GitHub alerts on.
    render_markdown_tasks_opts(src, width, code, theme, icons, tasks, true)
}

/// Shared trailing render options threaded through the alert/details/text-block renderers
/// ([`render_alert`], [`render_details`], [`render_text_block_safe`]) — bundling same-typed
/// positional args (e.g. two `bool`s, `&str`/`&[char]`) so a mix-up at a call site is a compile
/// error on the field name instead of a silent swap.
#[derive(Clone, Copy)]
struct MdRenderCtx<'a> {
    width: u16,
    code: CodeStyle,
    theme: &'a str,
    icons: bool,
    tasks: &'a [char],
    /// Whether `> [!TYPE]` blockquotes render as colored GitHub-alert callouts (`ui.md_alerts`).
    /// Carried on the context (rather than as a separate loose parameter) so every body-rendering
    /// pipeline that recurses through [`render_md_body_nested`] — an alert's own body, a
    /// `<details>` block's body, one nested inside the other, arbitrarily deep — agrees on it
    /// without a call site forgetting to thread it through.
    alerts: bool,
}

/// Like [`render_markdown_tasks`], with the GitHub-alert (`> [!NOTE]` …) rendering gated by
/// `alerts` (`ui.md_alerts`). When off, an alert blockquote renders as an ordinary blockquote.
fn render_markdown_tasks_opts(
    src: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
    alerts: bool,
) -> Vec<Line<'static>> {
    let ctx = MdRenderCtx {
        width,
        code,
        theme,
        icons,
        tasks,
        alerts,
    };
    let mut out = Vec::new();
    for seg in split_segments(src) {
        match seg {
            Segment::Md(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                if alerts {
                    // GitHub alerts are blockquotes led by `> [!TYPE]`; pull them out first (like
                    // tables/HTML) and draw a colored callout box, rendering the rest normally.
                    for ap in split_alerts(&text) {
                        match ap {
                            AlertPart::Text(t) => render_md_text(&mut out, &t, &ctx),
                            AlertPart::Alert { kind, title, body } => {
                                out.extend(render_alert(kind, &title, &body, &ctx))
                            }
                        }
                    }
                } else {
                    render_md_text(&mut out, &text, &ctx);
                }
            }
            Segment::Mermaid(code) => out.extend(render_mermaid_block(&code, width)),
        }
    }
    out
}

/// One Markdown text run: pull out collapsible `<details>` blocks first (they span blank lines, so
/// the plain HTML-block splitter can't keep them whole), then render the rest through the normal
/// tables → HTML → tui-markdown pipeline. This is the **single top-level entry point** that
/// advances the document-wide `<details>` Tab-toggle ordinal sequence (`next_details_open`) — a
/// `<details>` discovered any other way (inside an alert's body, inside another `<details>`'s
/// body) goes through [`render_md_body_nested`] instead, which renders it statically without
/// touching that sequence. See `render_md_body_nested`'s doc comment for why.
fn render_md_text(out: &mut Vec<Line<'static>>, text: &str, ctx: &MdRenderCtx) {
    for part in split_details(text) {
        match part {
            DetailsPart::Text(t) => render_md_text_inner(out, &t, ctx),
            DetailsPart::Details {
                open_attr,
                summary,
                body,
            } => {
                let open = next_details_open(open_attr);
                out.extend(render_details(open, &summary, &body, true, ctx));
            }
        }
    }
}

/// Render one Markdown text run that lives **inside** an alert's or a `<details>`'s own body (as
/// opposed to the single top-level entry point, [`render_md_text`]) — used by both [`render_alert`]
/// and [`render_details`] for their bodies. Handles GitHub alerts recursively (`ctx.alerts`) —
/// alerts carry no per-block state, so nesting them arbitrarily deep is always safe — and
/// collapsible `<details>` blocks, but renders any `<details>` found here **statically**
/// (`render_details(.., interactive: false, ..)`), honoring its own `open` attribute directly
/// instead of consuming a slot from the document-wide Tab-toggle ordinal sequence
/// (`next_details_open`/`collect_details_open`, which only ever walks the single top-level entry
/// point — `collect_details_open`'s own doc comment already establishes that a `<details>` nested
/// inside another is "swallowed into its parent's body and not counted separately", precisely to
/// keep that sequence in lockstep with `build_md_items`'s on-screen span count; the identical
/// reasoning extends to a `<details>` reachable only through an alert, so it gets the same static
/// treatment here). A `<details>` rendered by this path is therefore not individually
/// Tab/Space-toggleable, and its summary marker deliberately does not use
/// `details_marker_style()` (`is_details_header_span` won't match it) — but this still fixes the
/// actual information-disclosure this function exists for: nested inside an alert or another
/// `<details>` (recursively, for `<details>`-in-`<details>` or
/// `<details>`-in-alert-in-`<details>` etc.), a closed one no longer always shows its body — the
/// generic HTML-block fallback used before this existed had no concept of open vs. closed at all.
fn render_md_body_nested(out: &mut Vec<Line<'static>>, text: &str, ctx: &MdRenderCtx) {
    if ctx.alerts {
        for ap in split_alerts(text) {
            match ap {
                AlertPart::Text(t) => render_md_details_static(out, &t, ctx),
                AlertPart::Alert { kind, title, body } => {
                    out.extend(render_alert(kind, &title, &body, ctx))
                }
            }
        }
    } else {
        render_md_details_static(out, text, ctx);
    }
}

/// The `<details>`-only half of [`render_md_body_nested`] (factored out so both the `ctx.alerts`
/// and the `!ctx.alerts` branch there share it) — every `<details>` found here renders statically
/// (see `render_md_body_nested`'s doc comment for why); the rest of the text goes to
/// [`render_md_text_inner`].
fn render_md_details_static(out: &mut Vec<Line<'static>>, text: &str, ctx: &MdRenderCtx) {
    for dp in split_details(text) {
        match dp {
            DetailsPart::Text(t2) => render_md_text_inner(out, &t2, ctx),
            DetailsPart::Details {
                open_attr,
                summary,
                body,
            } => {
                out.extend(render_details(open_attr, &summary, &body, false, ctx));
            }
        }
    }
}

/// The tables → HTML-block → tui-markdown pipeline for one Markdown text run (no `<details>`).
fn render_md_text_inner(out: &mut Vec<Line<'static>>, text: &str, ctx: &MdRenderCtx) {
    let opts = Options::new(KonomaStyles {
        code_bg: ctx.code.bg,
    });
    // tui-markdown collapses a GFM table into one line, so table blocks alone are intercepted
    // first and drawn with our own borders. The rest of the text still goes to tui-markdown as before.
    for part in split_tables(text) {
        match part {
            MdPart::Text(t) => {
                if t.trim().is_empty() {
                    continue;
                }
                // An HTML block (`<details>` etc.) gets thrown away, contents and all, by
                // tui-markdown, so intercept it first and show it as text with the tags stripped (principle #3).
                for hp in split_html_blocks(&t) {
                    match hp {
                        HtmlPart::Text(t2) => {
                            if t2.trim().is_empty() {
                                continue;
                            }
                            // from_str_with_options returns a borrowed Text<'_>, so clone it to
                            // 'static right away. tui-markdown panics on certain input (e.g. a task
                            // item inside a loose list — confirmed on 0.3.7/0.3.8). Principle #3 =
                            // never crash: catch it and, by recursively bisecting, degrade **only
                            // the smallest failing block** to plain text (degrading the whole thing
                            // would leave a real document entirely undecorated — reported by the
                            // user on 2026-07-07).
                            render_text_block_safe(out, &t2, &opts, ctx, 0);
                        }
                        HtmlPart::Html(h) => out.extend(render_html_block(&h)),
                    }
                }
            }
            MdPart::Table(raw) => out.extend(render_table(&raw, ctx.width, ctx.icons)),
        }
    }
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

/// Run tui-markdown on one text segment, catching panics (upstream can panic on some
/// inputs, e.g. a loose list followed by a task item — seen in 0.3.7/0.3.8). Returns
/// None on panic so the caller degrades that segment to plain text (principle #3).
fn render_md_segment(src: &str, opts: &Options<KonomaStyles>) -> Option<Vec<Line<'static>>> {
    // The default panic hook writes to stderr and corrupts the raw-mode screen, so silence it only while catching.
    silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            into_static_lines(tui_markdown::from_str_with_options(src, opts))
        }))
        .ok()
    })
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

enum BlockPart {
    Text(String),
    Image {
        alt: String,
        url: String,
    },
    /// A top-level ```mermaid fence (only produced when the caller asked for fence extraction —
    /// i.e. image-mode mermaid; in text mode fences stay in the text runs and render as today).
    Mermaid {
        code: String,
    },
    /// A LaTeX math expression lifted out of a text run (only when the caller asked for math
    /// extraction — image-mode math). `display` = block `$$…$$`/`\[…\]` (vs inline `$…$`/`\(…\)`).
    Math {
        latex: String,
        display: bool,
    },
}

/// One piece of a text run after math extraction (used to lift inline `$…$` onto its own line).
enum MathPart {
    Text(String),
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
/// degrades to a one-line text placeholder (design principle #3). Text runs are rendered by
/// `render_markdown` unchanged, so all existing decoration behavior is preserved.
#[allow(clippy::too_many_arguments)] // the existing 7 args + mermaid/math slots (only one call site in app + tests)
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
) -> (Vec<Line<'static>>, Vec<ImagePlacement>) {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut placements: Vec<ImagePlacement> = Vec::new();
    // Mermaid fence extraction only happens "when even one could become Image/Loading" (= if it's
    // pinned to Text, it stays in the text run as before, changing no existing rendering or tests
    // whatsoever). The check is represented by an empty probe.
    let fences_on = !matches!(mermaid_slot(""), MermaidSlot::Text);
    let mut fence_ord = 0usize;
    // In math image mode, lift math out of each text run onto its own part (inline `$…$` becomes its
    // own image line). Mermaid fences are already extracted, so math scanning only sees text runs.
    let parts: Vec<BlockPart> = if math_on {
        split_block_parts(src, fences_on)
            .into_iter()
            .flat_map(|p| match p {
                BlockPart::Text(t) => split_math(&t)
                    .into_iter()
                    .map(|mp| match mp {
                        MathPart::Text(s) => BlockPart::Text(s),
                        MathPart::Math { latex, display } => BlockPart::Math { latex, display },
                    })
                    .collect::<Vec<_>>(),
                other => vec![other],
            })
            .collect()
    } else {
        split_block_parts(src, fences_on)
    };
    for part in parts {
        match part {
            BlockPart::Text(t) => out.extend(render_markdown_tasks_opts(
                &t, width, code, theme, icons, tasks, alerts,
            )),
            BlockPart::Image { alt, url } => match slot_of(&url) {
                ImageSlot::Inline { cols, rows } => {
                    placements.push(ImagePlacement {
                        url,
                        alt: alt.clone(),
                        line: out.len(),
                        cols,
                        rows,
                        fence_ord: None,
                    });
                    out.extend(image_placeholder_lines(cols, rows, &alt, width));
                }
                ImageSlot::Loading => out.extend(image_loading_line(&alt, &url, width)),
                ImageSlot::Unavailable => out.extend(image_text_fallback(&alt, &url, width)),
            },
            BlockPart::Mermaid { code: fence } => {
                let ord = fence_ord;
                fence_ord += 1;
                // An empty fence (just a half-written ```mermaid) can never become a diagram: skip
                // the slot and go straight to text rendering. An empty string passed to slot is
                // indistinguishable from the "is extraction on?" probe, so previously this would
                // pick up the probe's Loading response and get permanently stuck on a "loading" line.
                if fence.trim().is_empty() {
                    out.extend(render_mermaid_block(&fence, width));
                    continue;
                }
                match mermaid_slot(&fence) {
                    MermaidSlot::Image { cols, rows } => {
                        let url = mermaid_fence_url(&fence);
                        let mut ls = mermaid_placeholder_lines(cols, rows, width, mermaid_caption);
                        // The first entry is the caption line (a focus sentinel). The reserved rows = where the image overlays start right after it.
                        out.push(ls.remove(0));
                        placements.push(ImagePlacement {
                            url,
                            alt: "mermaid".into(),
                            line: out.len(),
                            cols,
                            rows,
                            fence_ord: Some(ord),
                        });
                        out.extend(ls);
                    }
                    MermaidSlot::Loading => {
                        out.extend(image_loading_line("mermaid", "diagram", width))
                    }
                    MermaidSlot::Text => out.extend(render_mermaid_block(&fence, width)),
                }
            }
            BlockPart::Math { latex, display } => match math_slot(&latex, display) {
                MathSlot::Image { cols, rows } => {
                    placements.push(ImagePlacement {
                        url: math_url(&latex, display),
                        alt: "math".into(),
                        line: out.len(),
                        cols,
                        rows,
                        fence_ord: None,
                    });
                    out.extend(math_placeholder_lines(cols, rows, width, display));
                }
                MathSlot::Loading => out.extend(image_loading_line("math", "equation", width)),
                MathSlot::Raw => out.extend(math_raw_lines(&latex, display)),
            },
        }
    }
    (out, placements)
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
pub fn collect_remote_image_urls(src: &str) -> Vec<String> {
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
fn split_block_parts(src: &str, mermaid_fences: bool) -> Vec<BlockPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    // Open code fence, as (fence char byte, fence length). `mermaid` = collecting a diagram body.
    let mut open: Option<(u8, usize)> = None;
    let mut mermaid: Option<String> = None;
    for line in src.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        match open {
            None => {
                if let Some((fence, info)) = parse_fence(bare) {
                    open = Some((fence.ch, fence.len));
                    if mermaid_fences && is_mermaid_info(&info) {
                        if !text.is_empty() {
                            parts.push(BlockPart::Text(std::mem::take(&mut text)));
                        }
                        mermaid = Some(String::new());
                    } else {
                        text.push_str(line);
                    }
                } else if let Some((alt, url)) = extract_block_image(bare) {
                    if !text.is_empty() {
                        parts.push(BlockPart::Text(std::mem::take(&mut text)));
                    }
                    parts.push(BlockPart::Image { alt, url });
                } else {
                    text.push_str(line);
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
                    (None, _) => text.push_str(line),
                }
                if closing {
                    open = None;
                }
            }
        }
    }
    // An unclosed mermaid fence: safely revert it to raw text (principle #3, never drop it).
    if let Some(code) = mermaid {
        text.push_str("```mermaid\n");
        text.push_str(&code);
    }
    if !text.is_empty() {
        parts.push(BlockPart::Text(text));
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

/// Add konoma's own decorations to the tui-markdown output as a post-processing step.
/// Turn code fences into a "special area" with a background, left gutter, and language header,
/// and strip the leading `#` from heading lines, laying a full-width rule below H1/H2 to convey hierarchy.
/// Process code blocks first (so a `# comment` inside code is not misdetected as a heading).
fn decorate_md_lines(
    lines: Vec<Line<'static>>,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) -> Vec<Line<'static>> {
    let lines = decorate_code_blocks(lines, width, code, theme);
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
struct ListGuard {
    in_list: bool,
}

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
/// screen, not an indented code block. The image step matters for the same banners: pulling an
/// `<img>` line out splits the `<div>` in two, and the `    |` separator left between the halves
/// *does* become a real indented code block on screen (criterion's README).
///
/// `in_alert_body` skips the `split_block_parts` step, because production never applies it to an
/// alert's body: that runs on the raw source, where those lines still carry their `>` prefix and
/// match neither an image nor a fence. Hence a diagram inside a `> [!NOTE]` really is drawn as code.
fn scan_code_run(run: &mut Vec<&str>, in_alert_body: bool, out: &mut Vec<String>) {
    if run.is_empty() {
        return;
    }
    let mut text = run.join("\n");
    text.push('\n');
    run.clear();
    if in_alert_body {
        scan_text_part(&text, out);
        return;
    }
    for part in split_block_parts(&text, true) {
        if let BlockPart::Text(t) = part {
            scan_text_part(&t, out);
        }
    }
}

/// One `BlockPart::Text` run: mirrors `render_md_text_inner`'s tables → HTML-blocks → parser chain.
fn scan_text_part(text: &str, out: &mut Vec<String>) {
    for part in split_tables(text) {
        let MdPart::Text(t) = part else {
            continue; // a table block: drawn with konoma's borders, never as code
        };
        for hp in split_html_blocks(&t) {
            match hp {
                HtmlPart::Text(t2) => parser_code_blocks(&t2, out),
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
pub(crate) fn code_block_source_locs(src: &str, details_open: &[bool]) -> Vec<String> {
    code_block_source_locs_inner(src, details_open, false)
}

/// `in_alert_body`: the block-level extraction pass (images, ```mermaid fences) runs on the raw
/// source, where an alert's lines still carry their `>` prefix and match nothing — so inside an alert
/// body it must be skipped here too, and a diagram there really is drawn as an ordinary code block.
/// See `scan_code_run`.
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
    // `scan_code_run`. Flushing *before* recursing keeps the results in document order.
    let mut run: Vec<&str> = Vec::new();
    let mut fence: Option<Fence> = None;
    while i < lines.len() {
        // An alert header / `<details>` tag inside a fenced code block is literal content, not a
        // block start (`fence_mask` gates the renderer's own splitters the same way) — track fences
        // here so the two peeled-off constructs below can never be triggered from inside one.
        if let Some(f) = fence {
            if parse_fence(lines[i])
                .is_some_and(|(nf, info)| nf.ch == f.ch && nf.len >= f.len && info.is_empty())
            {
                fence = None;
            }
            run.push(lines[i]);
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
        if let Some((f, _)) = parse_fence(lines[i]) {
            fence = Some(f);
        }
        run.push(lines[i]);
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
    /// Current state char in the source.
    pub state: char,
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
fn scan_task_lines(logical: &[(usize, &str, usize)], tasks: &[char], out: &mut Vec<TaskLoc>) {
    let mut list = ListGuard::new();
    let mut prev_not_paragraph = true;
    let mut idx = 0usize;
    while idx < logical.len() {
        let (orig_line, text, prefix) = logical[idx];
        let ws = leading_ws_width(text);
        let rest = strip_ws_columns(text, ws);
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
                state_off: prefix + ws + off,
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
/// Known, deliberately unhandled gap: `process_inline_html` (run by the caller *before* this scan,
/// on the same source) turns a `<br>` into a hard line break, which can make the text right after it
/// *look* like a new task line to the renderer even though it is not one on disk. This scanner has no
/// way to know that without re-running the same substitution, so it does not special-case it — the
/// resulting count mismatch just makes the caller's cross-check fail and refuse the toggle (principle
/// #3: refuse rather than write to the wrong place), it never mis-edits a line.
pub(crate) fn task_source_locs(src: &str, tasks: &[char], details_open: &[bool]) -> Vec<TaskLoc> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut fence: Option<Fence> = None;
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
        if let Some(f) = fence {
            let closing = parse_fence(line)
                .map(|(nf, info)| nf.ch == f.ch && nf.len >= f.len && info.is_empty())
                .unwrap_or(false);
            if closing {
                fence = None;
            }
            i += 1;
            continue;
        }
        if let Some((f, _info)) = parse_fence(line) {
            list.observe_special(ws);
            // Verified against the renderer: a non-paragraph block (fence/alert/details/html/table)
            // does not gate a following indented code block behind a blank line the way a paragraph
            // does.
            prev_not_paragraph = true;
            fence = Some(f);
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

/// Whether tui-markdown styled this **already-rendered** line as belonging to a fenced or
/// indented code block: its synthesized opening/closing marker lines, and every content/blank
/// line in between. **Depends on tui-markdown's internal implementation — verified directly
/// against its source** (`tui-markdown-0.3.7/src/lib.rs`: `start_codeblock`/`text`/
/// `end_codeblock`, and empirically against its actual output, see
/// `tui_markdown_codeblock_lines_are_styled_white` below): while pulldown-cmark (the parser
/// tui-markdown renders through) is inside a `Tag::CodeBlock`, tui-markdown pushes
/// `KonomaStyles::code()` (our `StyleSheet` impl) onto its internal line-style stack and patches
/// **every** `Line` it creates during that span with it — the synthesized `` ```lang `` opener,
/// every content and blank line, and the synthesized `` ``` `` closer alike — popping the style
/// only at `TagEnd::CodeBlock`. Crucially, this happens regardless of what the line's *text*
/// looks like, so a maximal run of styled lines is exactly one code block on screen, however many
/// `` ``` ``-looking substrings occur *inside* it: an inner fence nested inside a longer outer one
/// of the same character (`` ```` `` around `` ``` ``, the bug this function exists to fix — the
/// document past the inner marker used to render as a second, bogus code block, silencing every
/// heading/link/task after it), or an indented block whose literal first line happens to look
/// like a fence header. Scanning `line.style` instead of the line's *text* means those inner
/// look-alikes can never be mistaken for a boundary. `KonomaStyles::code()` is `fg(White)`
/// regardless of `ui.theme.code_bg` (same bg-independence as `is_inline_code_span`), so this
/// check is too. This is a **different** signal from `is_code_line` (which scans spans of the
/// *already-decorated* output — the `▎` gutter this very function adds — to let later passes like
/// autolink skip real code): `is_code_block_line` runs *before* decoration, straight on
/// tui-markdown's own output.
fn is_code_block_line(line: &Line<'_>) -> bool {
    line.style.fg == Some(Color::White)
}

/// Render one maximal run of [`is_code_block_line`] lines (drained from `run`) as a decorated
/// code block: language-badge header + syntect-highlighted body + bottom padding row. By
/// construction (see `is_code_block_line`'s docs — `start_codeblock`/`end_codeblock` always
/// synthesize both markers, and pulldown-cmark auto-closes any code block still open at end of
/// input) a run produced by real fenced/indented code is always at least 2 lines, starting with
/// the opening `` ```lang `` marker and ending with the closing `` ``` `` marker. The one case
/// that reaches this function without that shape: a *plain* (non-alert) block quote wrapping an
/// indented code block. konoma doesn't special-case those — they run through tui-markdown's own
/// native blockquote handling, which inserts a visual `"> "` prefix **in front of** every line
/// while it is active, including the (still `code()`-styled) marker/body lines of code nested
/// inside it — so the run's first line reads `"> ```"` rather than `"```"`. That shape is
/// unreachable for a real fence/indented block (the constructs this fix actually supports), so
/// rather than mis-parse it as code (stripping markers that were never really there) or panic
/// indexing into it, the safe response (principle #3) is to hand the run back completely
/// unchanged — exactly what this function's pre-fix caller did for the same input (it never
/// recognized a `` ``` `` preceded by other text as an opener at all).
fn flush_code_run(
    out: &mut Vec<Line<'static>>,
    run: &mut Vec<Line<'static>>,
    w: usize,
    code_bg: Option<Color>,
    theme: &str,
    code: CodeStyle,
) {
    if run.is_empty() {
        return;
    }
    let lines = std::mem::take(run);
    let opening = lines[0].to_string();
    let opening_trimmed = opening.trim_start();
    let closing_ok = lines.last().is_some_and(|l| {
        let text = l.to_string();
        is_closing_fence(text.trim_start())
    });
    if lines.len() < 2 || !opening_trimmed.starts_with("```") || !closing_ok {
        out.extend(lines);
        return;
    }
    let lang = opening_trimmed.trim_matches('`').trim().to_string();
    let label = if lang.is_empty() {
        "code"
    } else {
        lang.as_str()
    };
    out.push(code_header(label, w, code));
    let body: Vec<String> = lines[1..lines.len() - 1]
        .iter()
        .map(|l| l.to_string())
        .collect();
    out.extend(highlight_body(
        &body,
        &lang,
        w,
        code_bg,
        theme,
        code.tab_width,
        code.wrap,
    ));
    // Signal the block's end with a bottom padding row (gutter only).
    out.push(pad_to_width(vec![gutter_span(code_bg)], w, code_bg));
}

/// Turn the range enclosed by ```lang fences into a special area. When `code.bg`=None there is no background band
/// (left gutter and foreground color only). The body collects the raw text and highlights it all at once at the closing
/// fence via **our own syntect** (tui-markdown's highlight-code is disabled = avoids oniguruma).
/// Highlighting it all together lets multi-line comments/strings be colored correctly.
///
/// Block boundaries are found by [`is_code_block_line`] (tui-markdown's own per-line *style*),
/// not by scanning rendered *text* for `` ``` `` — the latter used to mistake an inner fence
/// marker nested inside a longer outer fence of the same character for the block's real close,
/// silently swallowing the rest of the document (headings, links, tasks) into a bogus second code
/// block (reported by a user; see `is_code_block_line`'s docs for the full mechanism).
fn decorate_code_blocks(
    lines: Vec<Line<'static>>,
    width: u16,
    code: CodeStyle,
    theme: &str,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let code_bg = code.bg;
    let mut out = Vec::with_capacity(lines.len());
    let mut run: Vec<Line<'static>> = Vec::new();
    for line in lines {
        if is_code_block_line(&line) {
            run.push(line);
            continue;
        }
        flush_code_run(&mut out, &mut run, w, code_bg, theme, code);
        out.push(line);
    }
    // Also flush a trailing run with no line after it (the safe side).
    flush_code_run(&mut out, &mut run, w, code_bg, theme, code);
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

/// Render one text block via tui-markdown with panic isolation. On panic the block is split
/// in half at a blank line **outside code fences** and both halves are rendered recursively, so
/// only the minimal offending paragraph degrades to plain text — not the whole document segment.
fn render_text_block_safe(
    out: &mut Vec<Line<'static>>,
    src: &str,
    opts: &Options<KonomaStyles>,
    ctx: &MdRenderCtx,
    depth: u8,
) {
    if src.trim().is_empty() {
        return;
    }
    if let Some(lines) = render_md_segment(src, opts) {
        out.extend(decorate_md_lines(
            lines, ctx.width, ctx.code, ctx.theme, ctx.icons, ctx.tasks,
        ));
        return;
    }
    // Depth limit reached, or it can't be split further → plain text for just this block (nothing is lost).
    if depth >= 8 {
        out.extend(src.lines().map(|l| Line::from(l.to_string())));
        return;
    }
    match split_block_for_retry(src) {
        Some((a, b)) => {
            render_text_block_safe(out, a, opts, ctx, depth + 1);
            render_text_block_safe(out, b, opts, ctx, depth + 1);
        }
        None => out.extend(src.lines().map(|l| Line::from(l.to_string()))),
    }
}

/// Split point for the panic-isolation retry: the blank line nearest the middle that is **not**
/// inside a code fence (splitting a fence would corrupt its rendering). Falls back to the
/// non-fenced newline nearest the middle; None when the block is a single line.
fn split_block_for_retry(src: &str) -> Option<(&str, &str)> {
    let mut blanks: Vec<usize> = Vec::new(); // start offsets of blank lines
    let mut newlines: Vec<usize> = Vec::new(); // newline positions (outside fences)
    let mut fence: Option<Fence> = None;
    let mut off = 0usize;
    for line in src.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        if let Some(f) = fence {
            let closing = parse_fence(bare)
                .map(|(nf, info)| nf.ch == f.ch && nf.len >= f.len && info.is_empty())
                .unwrap_or(false);
            if closing {
                fence = None;
            }
        } else if let Some((f, _info)) = parse_fence(bare) {
            fence = Some(f);
        }
        let end = off + line.len();
        if fence.is_none() && line.ends_with('\n') {
            if line.trim().is_empty() {
                blanks.push(off);
            }
            newlines.push(end);
        }
        off = end;
    }
    let mid = src.len() / 2;
    // A blank line = include that whole line in the first half; the second half starts right after
    // it. Only consider candidates where neither half ends up empty.
    let pick = |cands: &[usize]| -> Option<usize> {
        cands
            .iter()
            .copied()
            .filter(|&i| i > 0 && i < src.len())
            .min_by_key(|&i| i.abs_diff(mid))
    };
    if let Some(i) = pick(&blanks) {
        return Some((&src[..i], &src[i..]));
    }
    let cut = pick(&newlines)?;
    if cut == 0 || cut >= src.len() {
        return None;
    }
    Some((&src[..cut], &src[cut..]))
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

/// Whether this is a closing fence (a line of backticks only).
fn is_closing_fence(trimmed: &str) -> bool {
    let t = trimmed.trim_end();
    t.len() >= 3 && t.bytes().all(|b| b == b'`')
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
/// code fences are not diagrams and are excluded — same fence rules as the block splitter).
pub fn collect_mermaid_fences(src: &str) -> Vec<String> {
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
/// (`split_block_parts` → `split_math` per text run) so the extracted set matches what is drawn and
/// the caller can kick off exactly those renders. Fence/code-span aware (math inside a code fence or
/// `` `code span` `` is not extracted).
pub fn collect_math_exprs(src: &str) -> Vec<(String, bool)> {
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
fn flush_math(out: &mut Vec<MathPart>, buf: &mut String, latex: &str, display: bool) {
    if !buf.is_empty() {
        out.push(MathPart::Text(std::mem::take(buf)));
    }
    out.push(MathPart::Math {
        latex: latex.trim().to_string(),
        display,
    });
}

/// Split a text run into literal text and math expressions, lifting each math onto its own part (the
/// caller renders each as its own image line). Supports `$$…$$` / `\[…\]` (display, may span lines) and
/// `$…$` / `\(…\)` (inline, same line). Fence- and inline-code-aware: a `$` inside a ``` fence or a
/// `` `code span` `` is literal. Escaped `\$` is literal. A currency-style `$5 and $10` is not
/// mistaken for math (an inline closing `$` may not be preceded by whitespace nor followed by a digit).
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
fn split_math(text: &str) -> Vec<MathPart> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let bare_lines: Vec<&str> = lines
        .iter()
        .map(|l| l.strip_suffix('\n').unwrap_or(l))
        .collect();
    let structure = structure_mask(&bare_lines);
    let mut in_fence: Option<(u8, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let bare = raw.strip_suffix('\n').unwrap_or(raw);
        if let Some((ch, len)) = in_fence {
            buf.push_str(raw);
            if let Some((f, info)) = parse_fence(bare) {
                if f.ch == ch && f.len >= len && info.is_empty() {
                    in_fence = None;
                }
            }
            i += 1;
            continue;
        }
        if let Some((f, _info)) = parse_fence(bare) {
            in_fence = Some((f.ch, f.len));
            buf.push_str(raw);
            i += 1;
            continue;
        }
        // Inside a blockquote/alert, `<details>`, other HTML block, or table row: copy verbatim,
        // never scanning it for math (see the doc comment above for why).
        if structure[i] {
            buf.push_str(raw);
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
                // A structural line before the close: stop rather than swallow it into the math
                // body (which would silently eat, e.g., a table row into an unrendered equation).
                if structure[j] {
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
                flush_math(&mut out, &mut buf, &body, true);
                i = j + 1; // skip the closer line
                continue;
            }
            // No close (or empty): fall through and treat the line as ordinary text.
        }
        scan_inline_math(bare, &mut out, &mut buf);
        buf.push('\n');
        i += 1;
    }
    if !buf.is_empty() {
        out.push(MathPart::Text(buf));
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
fn scan_inline_math(line: &str, out: &mut Vec<MathPart>, buf: &mut String) {
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
                        flush_math(out, buf, content, false);
                        i = end;
                        continue;
                    }
                }
                b'[' => {
                    if let Some((content, end)) = find_close(line, i + 2, "\\]") {
                        flush_math(out, buf, content, true);
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
                        flush_math(out, buf, content, true);
                        i = end;
                        continue;
                    }
                }
                buf.push('$');
                i += 1;
                continue;
            }
            if let Some((content, end)) = find_inline_dollar(line, i + 1) {
                flush_math(out, buf, content, false);
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

/// Clone the borrowed Text returned by tui-markdown into an owned, 'static set of lines.
fn into_static_lines(text: Text) -> Vec<Line<'static>> {
    text.lines.into_iter().map(line_into_static).collect()
}

fn line_into_static(line: Line) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    let mut out = Line::from(spans).style(line.style);
    if let Some(alignment) = line.alignment {
        out = out.alignment(alignment);
    }
    out
}

/// A segment of the md body split at ```mermaid fence boundaries.
#[derive(Debug, PartialEq)]
enum Segment {
    Md(String),
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
fn split_segments(src: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut md = String::new();
    let mut mermaid = String::new();
    // The currently open fence. The bool is "is this a mermaid block".
    let mut open: Option<(Fence, bool)> = None;

    for line in src.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        match &open {
            None => {
                if let Some((fence, info)) = parse_fence(bare) {
                    if is_mermaid_info(&info) {
                        // A mermaid block starts: finalize the md accumulated so far, and drop the fence line itself.
                        if !md.is_empty() {
                            segments.push(Segment::Md(std::mem::take(&mut md)));
                        }
                        open = Some((fence, true));
                    } else {
                        // An ordinary code fence: append it to md as-is, to be passed on to tui-markdown.
                        md.push_str(line);
                        open = Some((fence, false));
                    }
                } else {
                    md.push_str(line);
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
                    }
                    open = None;
                } else if *is_mermaid {
                    mermaid.push_str(line);
                } else {
                    md.push_str(line);
                }
            }
        }
    }

    // Finalize whatever's left at the end. An unclosed mermaid still attempts to render (falls back to raw display on failure).
    if !md.is_empty() {
        segments.push(Segment::Md(md));
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

enum MdPart {
    Text(String),
    Table(String),
}

/// For each of `lines`, whether it is part of a fenced code block — the opening/closing marker
/// lines and everything between (using the same fence-matching rules as `split_segments`/
/// `parse_fence`: matching char, len ≥ opening len, empty info to close). `split_tables`,
/// `split_html_blocks` and `split_alerts` gate their block-start detection on this: none of them
/// otherwise know about code fences, so a fenced block whose *contents* happen to look like a GFM
/// table, an HTML tag, or a `> [!NOTE]` alert header got sliced out of the fence and rendered as
/// that construct instead of code — breaking the fence in two (each half growing its own code
/// header) or leaking its closing marker into a stray HTML-block rescue. `split_details` has its
/// own (simpler) fence toggle and is intentionally left alone here.
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

/// For each of `lines`, whether it sits inside a construct whose own parser requires line
/// continuity to work: a blockquote (a plain quote or a GitHub alert — both are `>`-prefixed),
/// a `<details>` block, another HTML block, or a GFM table row. `split_math` consults this mask
/// so it never lifts an inline math expression out onto its own line from inside one of these —
/// doing so breaks the very line sequence those blocks' own splitters (`split_alerts`,
/// `split_details`, `split_html_blocks`, `split_tables`) require, tearing the block apart (see
/// `split_math`'s doc comment for the concrete failure modes this prevents).
///
/// Reuses the same predicates the block splitters themselves use to decide where a block starts
/// and ends, so this mask can never drift out of sync with what those splitters actually carve
/// out. Gated by `fence_mask`, exactly like `split_tables`/`split_html_blocks`/`split_alerts`: a
/// `>`/`|`/`<div>`-looking line inside a code fence is ordinary code, not structure.
fn structure_mask(lines: &[&str]) -> Vec<bool> {
    let fenced = fence_mask(lines);
    let mut mask = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if fenced[i] {
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
            mask[i] = true;
            i += 1;
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
            mask[start..=end].fill(true);
            i = close.map_or(lines.len(), |c| c + 1);
            continue;
        }
        // GFM table: header row + delimiter row + consecutive data rows, mirroring `split_tables`.
        if i + 1 < lines.len()
            && !fenced[i + 1]
            && looks_like_table_row(lines[i])
            && is_table_delimiter(lines[i + 1])
        {
            let start = i;
            let mut j = start + 2;
            while j < lines.len() && !fenced[j] && looks_like_table_row(lines[j]) {
                j += 1;
            }
            mask[start..j].fill(true);
            i = j;
            continue;
        }
        // Any other HTML block: opening tag line through the line before the next blank line,
        // mirroring `split_html_blocks`.
        if is_html_block_start(lines[i]) {
            let start = i;
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().is_empty() {
                j += 1;
            }
            mask[start..j].fill(true);
            i = j;
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

enum HtmlPart {
    Text(String),
    Html(String),
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
/// Fence-aware (`fence_mask`): an HTML-looking line inside a code fence is left as ordinary text, or
/// its tags would be stripped and — worse — the "block" scan (which just runs to the next blank
/// line) would swallow the fence's own closing marker, leaking it into the rescued text.
fn split_html_blocks(md: &str) -> Vec<HtmlPart> {
    let lines: Vec<&str> = md.lines().collect();
    let fenced = fence_mask(&lines);
    let mut parts = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !fenced[i] && is_html_block_start(lines[i]) {
            if !buf.is_empty() {
                parts.push(HtmlPart::Text(buf.join("\n") + "\n"));
                buf.clear();
            }
            let mut block: Vec<&str> = Vec::new();
            while i < lines.len() && !lines[i].trim().is_empty() {
                block.push(lines[i]);
                i += 1;
            }
            parts.push(HtmlPart::Html(block.join("\n")));
            continue;
        }
        buf.push(lines[i]);
        i += 1;
    }
    if !buf.is_empty() {
        parts.push(HtmlPart::Text(buf.join("\n") + "\n"));
    }
    parts
}

/// Render an HTML block as its tag-stripped text (entities decoded, comments dropped entirely).
fn render_html_block(raw: &str) -> Vec<Line<'static>> {
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AlertKind {
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

enum AlertPart {
    Text(String),
    Alert {
        kind: AlertKind,
        title: String,
        body: String,
    },
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

/// Partition Markdown into plain-text runs and GitHub alerts. An alert starts at a `> [!TYPE]`
/// header and captures the following blockquote lines as its (marker-stripped) body. Fence-aware
/// (`fence_mask`): a `> [!NOTE]`-looking line inside a code fence must stay literal code, not open
/// a callout box (whose body-capture loop then stops at the first non-`>` line — usually the very
/// next line of code — leaving the rest of the fence, including its closing marker, to fall through
/// to plain text and break the code block in two). Also gated by `details_mask`: an alert header
/// found inside a still-folded `<details>` block must stay literal text there too — extracting it
/// here would render it *outside* the fold, leaking its body regardless of whether the `<details>`
/// block is open or closed (see `details_mask`'s doc comment). Left alone, that text flows through
/// to `render_md_text`'s own `split_details` call, which correctly folds it away; from there,
/// `render_details`/`render_md_body_nested` re-run `split_alerts` on just that block's body, where
/// this same gate no longer applies (the surrounding `<details>` tags aren't part of the substring).
fn split_alerts(md: &str) -> Vec<AlertPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let lines: Vec<&str> = md.lines().collect();
    let fenced = fence_mask(&lines);
    let in_details = details_mask(&lines, &fenced);
    let mut i = 0;
    while i < lines.len() {
        let header = if fenced[i] || in_details[i] {
            None
        } else {
            parse_alert_header(lines[i])
        };
        if let Some((kind, title)) = header {
            if !text.is_empty() {
                parts.push(AlertPart::Text(std::mem::take(&mut text)));
            }
            i += 1;
            let mut body = String::new();
            while i < lines.len() && is_blockquote_line(lines[i]) {
                body.push_str(&strip_blockquote(lines[i]));
                body.push('\n');
                i += 1;
            }
            parts.push(AlertPart::Alert { kind, title, body });
        } else {
            text.push_str(lines[i]);
            text.push('\n');
            i += 1;
        }
    }
    if !text.is_empty() {
        parts.push(AlertPart::Text(text));
    }
    parts
}

/// Render a GitHub alert as a colored callout box: a header (`icon Label` in the type's color, bold)
/// and the Markdown body, every line carrying a colored left bar (`▌`). The body goes through
/// [`render_md_body_nested`] (not the top-level [`render_md_text`]), so links, code, lists, a
/// nested alert, and a `<details>` block (rendered statically — see that function's doc comment)
/// inside an alert all work.
fn render_alert(kind: AlertKind, title: &str, body: &str, ctx: &MdRenderCtx) -> Vec<Line<'static>> {
    let color = kind.color();
    let bar = || Span::styled("▌ ".to_string(), Style::new().fg(color));
    let mut out = Vec::new();
    // Header: bar + optional icon + label (+ optional Obsidian-style title), all in the alert color.
    let mut header = vec![bar()];
    if ctx.icons {
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
    out.push(Line::from(header));
    // Body via the shared pipeline (width reduced by the 2-col bar), each line bar-prefixed.
    let inner_ctx = MdRenderCtx {
        width: ctx.width.saturating_sub(2),
        ..*ctx
    };
    let mut body_lines = Vec::new();
    render_md_body_nested(&mut body_lines, body, &inner_ctx);
    for bl in body_lines {
        let style = bl.style;
        let mut spans = vec![bar()];
        spans.extend(bl.spans);
        out.push(Line::from(spans).style(style));
    }
    out
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
    Text(String),
    Details {
        open_attr: bool,
        summary: String,
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
/// Fence-aware (a `<details>` inside a code fence is left as text) so the block count and order match
/// `collect_details_open`, which seeds the per-ordinal open state.
fn split_details(md: &str) -> Vec<DetailsPart> {
    let lines: Vec<&str> = md.lines().collect();
    let fenced = fence_mask(&lines);
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < lines.len() {
        if fenced[i] {
            text.push_str(lines[i]);
            text.push('\n');
            i += 1;
            continue;
        }
        if let Some(open_attr) = details_open_tag(lines[i]) {
            if !text.is_empty() {
                parts.push(DetailsPart::Text(std::mem::take(&mut text)));
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
        i += 1;
    }
    if !text.is_empty() {
        parts.push(DetailsPart::Text(text));
    }
    parts
}

/// For each of `lines`, whether it sits inside a top-level `<details>` … `</details>` block — the
/// same span `split_details` extracts (via the shared `details_block_close`). Used by `split_alerts`
/// so it doesn't pull a `> [!TYPE]` alert header out of a `<details>` block that is still folded: an
/// alert nested there used to render *outside* the fold unconditionally, leaking its body regardless
/// of the block's open/closed state (a closed `<details>` wrapping an alert showed the alert's body
/// — reported by the user). Gated by `fence_mask`, exactly like `split_details`/`structure_mask`.
///
/// Deliberately narrower than `structure_mask` (which also masks every blockquote line, table row,
/// and other HTML block): masking blockquote lines here would suppress `split_alerts` everywhere,
/// since an alert header *is* a blockquote line — this mask exists purely to protect `<details>`
/// blocks specifically.
fn details_mask(lines: &[&str], fenced: &[bool]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if fenced[i] {
            i += 1;
            continue;
        }
        if details_open_tag(lines[i]).is_some() {
            let close = details_block_close(lines, i);
            let end = close.unwrap_or(lines.len() - 1);
            mask[i..=end].fill(true);
            i = close.map_or(lines.len(), |c| c + 1);
            continue;
        }
        i += 1;
    }
    mask
}

/// Pull the `<summary>` text and the remaining body out of a details block's inner content. Tags in
/// the summary are stripped; the body keeps its Markdown. A missing summary yields an empty string
/// (the renderer supplies a default label).
fn extract_summary_body(inner: &str) -> (String, String) {
    let lower = inner.to_ascii_lowercase();
    if let (Some(s), Some(e)) = (lower.find("<summary"), lower.find("</summary>")) {
        // The `>` that closes the opening `<summary …>` tag must sit before `</summary>`. Bound the
        // search to `[s, e)`: a malformed `<summary` with no `>` of its own would otherwise match the
        // `>` inside `</summary>` (past `e`), giving an inverted slice range that panics. When the
        // opening tag never closes, fall back to no-summary (the whole inner becomes the body).
        if s < e {
            if let Some(gt) = inner[s..e].find('>') {
                let sum = strip_inline_html_tags(inner[s + gt + 1..e].trim());
                let body = inner[e + "</summary>".len()..].trim().to_string();
                return (sum, body);
            }
        }
    }
    (String::new(), inner.trim().to_string())
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
fn render_details(
    open: bool,
    summary: &str,
    body: &str,
    interactive: bool,
    ctx: &MdRenderCtx,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
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
    out.push(Line::from(vec![
        Span::styled(format!("{arrow} "), marker_style),
        Span::styled(label.to_string(), Style::new().add_modifier(Modifier::BOLD)),
    ]));
    if open && !body.trim().is_empty() {
        let bar = || Span::styled("▏ ".to_string(), Style::new().fg(TABLE_BORDER_FG));
        let inner_ctx = MdRenderCtx {
            width: ctx.width.saturating_sub(2),
            ..*ctx
        };
        let mut body_lines = Vec::new();
        render_md_body_nested(&mut body_lines, body, &inner_ctx);
        for bl in body_lines {
            let style = bl.style;
            let mut spans = vec![bar()];
            spans.extend(bl.spans);
            out.push(Line::from(spans).style(style));
        }
    }
    out
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
    split_details(src)
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

/// Convert the common inline HTML that GitHub renders (but tui-markdown strips) into Markdown/Unicode
/// it renders faithfully: `<del>/<s>/<strike>` → strikethrough, `<kbd>` → an inline-code keycap,
/// `<sup>/<sub>` → Unicode (when the content maps), `<br>` → a hard line break. Fence-aware; a no-op
/// per line without any of these tags. `<mark>`/`<ins>` have no faithful terminal form and are left
/// to tui-markdown (their tags are stripped, text kept).
pub fn process_inline_html(src: &str) -> String {
    let mut out = String::new();
    let mut fence: Option<Fence> = None;
    for line in src.lines() {
        if let Some(f) = fence {
            let closing = parse_fence(line)
                .map(|(nf, info)| nf.ch == f.ch && nf.len >= f.len && info.is_empty())
                .unwrap_or(false);
            if closing {
                fence = None;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some((f, _info)) = parse_fence(line) {
            fence = Some(f);
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !line.contains('<') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Inline code spans are literal: `` `<kbd>x</kbd>` `` is documentation *of* the tag, not a
        // tag to convert. Converting it also wrapped the content in a second pair of backticks
        // (`` ``x`` ``) and a `<br>` inside a span injected a newline, breaking the span outright.
        // The spans are masked rather than cut out, so a tag pair may still wrap one.
        let s = rewrite_masking_code_spans(line, |masked| {
            let mut s = replace_tag_pair(masked, "kbd", |i| format!("`{i}`"));
            s = replace_tag_pair(&s, "del", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "s", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "strike", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "sup", |i| map_all_or_keep(i, sup_char));
            s = replace_tag_pair(&s, "sub", |i| map_all_or_keep(i, sub_char));
            for br in ["<br>", "<br/>", "<br />", "<BR>", "<BR/>", "<BR />"] {
                s = s.replace(br, "  \n");
            }
            s
        });
        out.push_str(&s);
        out.push('\n');
    }
    out
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
/// footnotes section is appended after a rule. Fence-aware — refs/defs inside a code fence are left
/// verbatim. A no-op (returns `src` unchanged) when there are no definitions. Single-line
/// definitions only.
pub fn process_footnotes(src: &str) -> String {
    use std::collections::HashMap;
    let lines: Vec<&str> = src.lines().collect();
    // Same "is this line part of a fence" mask `split_tables`/`split_html_blocks`/`split_alerts` use —
    // all three passes below just need to know, per line, whether to skip/echo it verbatim.
    let fenced = fence_mask(&lines);
    // Pass 1: collect definitions (fence-aware) and mark their lines.
    let mut defs: Vec<(String, String)> = Vec::new();
    let mut is_def = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        if fenced[i] {
            continue;
        }
        if let Some((id, text)) = parse_footnote_def(line) {
            defs.push((id, text));
            is_def[i] = true;
        }
    }
    if defs.is_empty() {
        return src.to_string();
    }
    let def_ids: std::collections::HashSet<&str> = defs.iter().map(|(id, _)| id.as_str()).collect();
    // Pass 2: number by first reference appearance (fence-aware, defs excluded).
    let mut num: HashMap<String, usize> = HashMap::new();
    let mut next = 1usize;
    for (i, line) in lines.iter().enumerate() {
        if fenced[i] || is_def[i] {
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
        return src.to_string(); // definitions present but never referenced → leave as-is
    }
    // Pass 3: rebuild the body (drop def lines, replace refs; fences verbatim).
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if fenced[i] {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if is_def[i] {
            continue;
        }
        out.push_str(&replace_footnote_refs(line, &num));
        out.push('\n');
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
    out.push_str("\n---\n\n");
    for (n, text) in items {
        out.push_str(&format!("{n}. {text}\n"));
    }
    out
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
/// Fence-aware (`fence_mask`): pipe-delimited lines inside a code fence (e.g. a shell/markdown
/// example) must stay code, not get carved out as a rendered table — which also splits the fence
/// into two pieces, each growing its own (broken) code-block header.
fn split_tables(md: &str) -> Vec<MdPart> {
    let lines: Vec<&str> = md.lines().collect();
    let fenced = fence_mask(&lines);
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < lines.len() {
        // A table starts = the current line is a header candidate AND the next line is a delimiter row AND both are outside a fence.
        if !fenced[i]
            && i + 1 < lines.len()
            && !fenced[i + 1]
            && looks_like_table_row(lines[i])
            && is_table_delimiter(lines[i + 1])
        {
            if !text.is_empty() {
                parts.push(MdPart::Text(std::mem::take(&mut text)));
            }
            let mut raw = String::new();
            raw.push_str(lines[i]);
            raw.push('\n');
            raw.push_str(lines[i + 1]);
            raw.push('\n');
            let mut j = i + 2;
            while j < lines.len() && !fenced[j] && looks_like_table_row(lines[j]) {
                raw.push_str(lines[j]);
                raw.push('\n');
                j += 1;
            }
            parts.push(MdPart::Table(raw));
            i = j;
        } else {
            text.push_str(lines[i]);
            text.push('\n');
            i += 1;
        }
    }
    if !text.is_empty() {
        parts.push(MdPart::Text(text));
    }
    parts
}

/// Split one line of the form `| a | b |` into cell columns. Drops the leading/trailing empty cells from the boundary pipes.
fn parse_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    // Drop the trailing delimiter `|`. But `\|` (escaped = literal) is not a delimiter, so keep it.
    let t = if t.ends_with('|') && !t.ends_with("\\|") {
        &t[..t.len() - 1]
    } else {
        t
    };
    // GFM: split cells on an **unescaped** `|`, and turn `\|` back into a literal `|`.
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    cells.push(cur);
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

/// Column alignment from the delimiter row (`:---` left / `:---:` center / `---:` right).
#[derive(Clone, Copy, PartialEq)]
enum ColAlign {
    Left,
    Center,
    Right,
}

/// Parse the delimiter row's alignment colons per column.
fn parse_table_aligns(line: &str) -> Vec<ColAlign> {
    parse_table_row(line)
        .iter()
        .map(|c| {
            let l = c.starts_with(':');
            let r = c.ends_with(':');
            match (l, r) {
                (true, true) => ColAlign::Center,
                (false, true) => ColAlign::Right,
                _ => ColAlign::Left,
            }
        })
        .collect()
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

/// Render a table block with box-drawing lines. `width` is the column count of the display area (inside the frame).
/// Cell text is parsed into inline segments so `[label](url)` links render as links (label only,
/// hidden target span) instead of raw Markdown; column widths are measured on the displayed form.
fn render_table(raw: &str, width: u16, icons: bool) -> Vec<Line<'static>> {
    let mut rows: Vec<Vec<Vec<CellSeg>>> = Vec::new();
    let mut header_rows = 0usize; // number of rows before the delimiter row (= the header)
    let mut aligns: Vec<ColAlign> = Vec::new();
    for line in raw.lines() {
        if is_table_delimiter(line) {
            header_rows = rows.len();
            aligns = parse_table_aligns(line); // apply the alignment colons (:---:) per column
            continue;
        }
        rows.push(
            parse_table_row(line)
                .into_iter()
                .map(|c| {
                    let mut segs = parse_cell_segments(&c);
                    // Make it look the same as a paragraph link: if icons are enabled, prepend to the
                    // label and fold it in **before the width calculation** (adding it after would break column alignment).
                    if icons {
                        for seg in &mut segs {
                            if let CellSeg::Link { label, .. } = seg {
                                *label = format!("{} {label}", crate::ui::icons::link_icon());
                            }
                        }
                    }
                    segs
                })
                .collect(),
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_CODE_BG;

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
        let parts = split_details(md);
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
        assert!(split_details(fenced)
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
            let lines =
                render_markdown_tasks_opts(md, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let (_lines, imgs) = render_markdown_with_images(
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
        let (raw_lines, raw_imgs) = render_markdown_with_images(
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
        let (_l, off_imgs) = render_markdown_with_images(
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

    #[test]
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
        let lines =
            render_markdown_tasks_opts(md, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let lines =
            render_markdown_tasks_opts(md, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let lines =
            render_markdown_tasks_opts(md, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let lines =
            render_markdown_tasks_opts(closed, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let lines_open =
            render_markdown_tasks_opts(open, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let lines = render_markdown_tasks_opts(
            &closed,
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
        let lines_open =
            render_markdown_tasks_opts(&open, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        // tui-markdown 0.3.7/0.3.8 panics on "a task item inside a loose list (blank-line separated)"
        // (insertion index should be <= len). konoma catches it and **retries by bisecting at a
        // blank-line boundary**, and each resulting half renders normally = decoration (task markers
        // etc.) survives. Back when the whole thing degraded to plain text, just one instance of this
        // syntax turned an entire real document undecorated (reported against an actual document).
        let md = "# title\n\n- a\n\n- [ ] b\n\n**bold**\n";
        let lines = render_markdown(md, 60, BG, "TwoDark", false);
        let all: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            all.iter().any(|t| t.contains("- a")),
            "内容は読める形で残る: {all:?}"
        );
        // Decoration survives the bisect-and-retry: the task gets its dedicated marker span, and bold has its markers stripped.
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
    fn split_block_for_retry_avoids_fences() {
        // The retry's split point is a blank line outside a fence. A blank line inside a fence is not split on.
        let src = "para1\n\n```\ncode\n\nmore\n```\n\npara2\n";
        let (a, b) = split_block_for_retry(src).expect("分割できる");
        assert!(
            a.matches("```").count() % 2 == 0,
            "前半のフェンスは閉じている: {a:?}"
        );
        assert!(
            b.matches("```").count() % 2 == 0,
            "後半のフェンスは閉じている: {b:?}"
        );
        // A single-line block can't be split.
        assert!(split_block_for_retry("only-one-line").is_none());
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
        let segs = split_segments(src);
        assert_eq!(segs.len(), 3, "got {segs:?}");
        assert!(matches!(&segs[0], Segment::Md(s) if s.contains("Title")));
        assert!(matches!(&segs[1], Segment::Mermaid(s) if s.contains("graph TD")));
        assert!(matches!(&segs[2], Segment::Md(s) if s.contains("after")));
        // The fence lines are not included in the mermaid section.
        assert!(matches!(&segs[1], Segment::Mermaid(s) if !s.contains("```")));
    }

    #[test]
    fn normal_code_fence_is_kept_in_markdown() {
        // A ```rust block is not intercepted and is left in md (tui-markdown highlights it).
        let src = "text\n\n```rust\nlet x = 1;\n```\n";
        let segs = split_segments(src);
        assert_eq!(segs.len(), 1, "got {segs:?}");
        assert!(matches!(&segs[0], Segment::Md(s) if s.contains("let x = 1;")));
    }

    #[test]
    fn mermaid_inside_normal_fence_is_not_intercepted() {
        // A ```mermaid-looking line inside a normal fence never becomes a diagram (it's already inside a fence).
        let src = "~~~\n```mermaid\nnot a diagram\n```\n~~~\n";
        let segs = split_segments(src);
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
        let (_lines, imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        // An unclosed fence safely reverts to text (nothing is lost).
        let unterminated = "```mermaid\ngraph LR\nA-->B\n";
        assert!(collect_mermaid_fences(unterminated).is_empty());
    }

    #[test]
    fn mermaid_slots_image_loading_text() {
        let src = "before\n\n```mermaid\ngraph LR\nA-->B\n```\n\nafter\n";
        let slot_img = |_: &str| MermaidSlot::Image { cols: 20, rows: 5 };
        let (lines, imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (en, ei) = render("Enter: full screen");
        let (ja, ji) = render("Enter: 全画面");
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
    fn split_alerts_captures_body_and_surrounding_text() {
        let md = "intro\n\n> [!TIP]\n> be **bold**\n> line two\n\nafter\n";
        let parts = split_alerts(md);
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[0], AlertPart::Text(t) if t.contains("intro")));
        match &parts[1] {
            AlertPart::Alert { kind, body, .. } => {
                assert_eq!(*kind, AlertKind::Tip);
                assert!(body.contains("be **bold**") && body.contains("line two"));
                assert!(!body.contains('>'), "blockquote markers stripped from body");
            }
            _ => panic!("expected an alert"),
        }
        assert!(matches!(&parts[2], AlertPart::Text(t) if t.contains("after")));
    }

    #[test]
    fn render_alert_makes_a_colored_callout_not_literal_marker() {
        let md = "> [!WARNING]\n> careful with [docs](./x.md)\n";
        // alerts on
        let on =
            render_markdown_tasks_opts(md, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, true);
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
        let off =
            render_markdown_tasks_opts(md, 60, BG, "TwoDark", false, DEFAULT_TASK_STATES, false);
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

    #[test]
    fn process_inline_html_leaves_fences_untouched() {
        let out = process_inline_html("```\n<kbd>x</kbd>\n```\n");
        assert!(
            out.contains("<kbd>x</kbd>"),
            "tags inside a fence stay literal"
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

    #[test]
    fn process_footnotes_no_definitions_is_noop() {
        let src = "just [^1] with no definition\n";
        assert_eq!(process_footnotes(src), src);
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
        let lines = render_markdown_tasks_opts(
            md,
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
        let lines = render_markdown_tasks_opts(
            md,
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
            // Root cause (block-image extraction, 2026-08): pulling an `<img>` line out of a centered
            // banner splits the surrounding HTML block in two, and the `    |` separator left between
            // the halves becomes a real indented code block on screen. Only visible through the
            // production entry point — see `rendered_code_blocks`.
            (
                "4-column-indented HTML inside a centered banner is an HTML block, not indented code",
                "<div align=\"center\">\n    <a href=\"https://example.com\">Docs</a>\n</div>\n\npara\n",
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

    /// Documents pinning **the** invariant every source-rewriting pass shares: the contents of an
    /// inline code span are a literal *example* of some syntax, never syntax to act on. Enumerated
    /// from the specification's case splits — backtick run lengths, matched/unmatched delimiters,
    /// where the span sits in the line, backslash escapes, multibyte boundaries — crossed with the
    /// notations the passes rewrite. Deliberately *not* enumerated from the bugs already found: a
    /// corpus grown from past bugs only ever prevents those exact bugs from recurring, which is how
    /// this rule came to be re-derived (and to drift) once per pass in the first place.
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
        v
    }
}
#[cfg(test)]
mod code_span_parity_tests {
    use super::*;

    /// Every inline code span of `src`, as `(start_line_index, text_including_delimiters)`, skipping
    /// lines inside a fence (there the backticks are the fence's own delimiters). Uses the shared
    /// primitive, so it describes exactly the regions the passes promise to leave alone.
    fn spans_of(src: &str) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        let fenced = fence_mask(&lines);
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
    /// extracted. If the scanner ever looked inside a span, the extracted list would move.
    #[test]
    fn math_extraction_treats_code_span_contents_as_opaque() {
        for c in code_span_corpus::cases() {
            let lines: Vec<&str> = c.src.lines().collect();
            let fenced = fence_mask(&lines);
            let mut blanked = String::new();
            for (i, line) in lines.iter().enumerate() {
                if fenced[i] {
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

    /// The passes are line-scoped by construction, and all three agree on it. CommonMark would let a
    /// code span run across a newline; none of these passes follows it there. Extending one pass
    /// alone would put them back out of step, so the shared limit is pinned here.
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
        assert_eq!(
            collect_math_exprs("open `$x$\nstill $y$` closed\n").len(),
            2
        );
    }
}

#[cfg(test)]
mod task_scan_parity_tests {
    use super::*;

    /// Locks in tui-markdown's own behavior that `is_code_block_line` depends on (see its doc
    /// comment): while rendering a fenced/indented code block, **tui-markdown itself** — not any
    /// konoma code — styles the synthesized opening marker, every content line, and any blank line
    /// *inside* the block with `fg(White)` (`KonomaStyles::code()`, since konoma supplies
    /// `KonomaStyles` as the `StyleSheet`), and has already popped that style again by the time it
    /// pushes the very next line — so a maximal run of white lines really is exactly one code
    /// block, including the blank separator line tui-markdown inserts of its own accord between
    /// two sibling code blocks even when the *source* has no blank line between them (the fact
    /// that makes two adjacent blocks merging into one run structurally impossible). If
    /// tui-markdown ever changes this, `is_code_block_line`/`decorate_code_blocks` would silently
    /// stop grouping code blocks correctly — every fence in the app could render garbled or
    /// missing — so this test exists to fail loudly here instead of that happening silently.
    #[test]
    fn tui_markdown_codeblock_lines_are_styled_white() {
        fn check(label: &str, src: &str, expected: &[(&str, bool)]) {
            let opts = Options::new(KonomaStyles { code_bg: None });
            let text = tui_markdown::from_str_with_options(src, &opts);
            let actual: Vec<(String, bool)> = text
                .lines
                .iter()
                .map(|l| (l.to_string(), is_code_block_line(l)))
                .collect();
            let expected: Vec<(String, bool)> =
                expected.iter().map(|(t, w)| (t.to_string(), *w)).collect();
            assert_eq!(
                actual, expected,
                "{label}: tui-markdown の出力の前提が変わった — is_code_block_line が依拠する\
                 「コードブロックの行は fg(White) で塗られる」という前提が崩れている\
                 (このテストが落ちたら decorate_code_blocks は静かに壊れる。\
                 tui-markdown のバージョン/挙動を確認すること)"
            );
        }

        // Two sibling fences with no blank line between them in the *source* still get a
        // non-white separator line from tui-markdown itself — the fact that lets a run boundary
        // be found purely from style, with no risk of two blocks merging into one run.
        check(
            "adjacent blocks",
            "```a\nx\n```\n```b\ny\n```\n",
            &[
                ("```a", true),
                ("x", true),
                ("```", true),
                ("", false),
                ("```b", true),
                ("y", true),
                ("```", true),
            ],
        );

        // A shorter same-character fence nested inside a longer one never actually closes the
        // outer fence (real CommonMark, correctly parsed by pulldown-cmark) — its lines are
        // literal body text, still styled white throughout — and the document past the *real*
        // close (a heading, a blank line, a paragraph) is not.
        check(
            "4-backtick fence nesting a 3-backtick lookalike",
            "````markdown\n```rust\nlet x = 1;\n```\n````\n\n# Heading After\n\nReal prose here.\n",
            &[
                ("```markdown", true),
                ("```rust", true),
                ("let x = 1;", true),
                ("```", true),
                ("```", true),
                ("", false),
                ("# Heading After", false),
                ("", false),
                ("Real prose here.", false),
            ],
        );
    }

    /// `decorate_code_blocks` non-regressions for fence unification stage 2: the run-detection
    /// rewrite (grouping by `is_code_block_line`'s *style* signal, not by scanning rendered text
    /// for `` ``` ``) must not disturb any of the ordinary shapes it already handled correctly —
    /// a plain fence with and without a language label, an empty fence, a 4-column indented block,
    /// two blank-line-separated fences staying isolated from each other, a tilde fence, a
    /// 5-backtick fence, a fence inside a GitHub alert, and a fence inside an *open* `<details>`.
    #[test]
    fn decorate_code_blocks_render_non_regressions() {
        fn header_line<'a>(lines: &'a [Line<'static>]) -> &'a Line<'static> {
            lines
                .iter()
                .find(|l| l.spans.iter().any(is_code_header_span))
                .expect("コードヘッダが描かれていない")
        }

        // Plain fence with a language label.
        let src = "```rust\nfn a(){}\n```\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(
            header_line(&lines).to_string().contains("rust"),
            "言語ラベルが描かれていない"
        );
        assert!(
            lines.iter().any(|l| l.to_string().contains("fn a(){}")),
            "本文が描かれていない"
        );

        // Plain fence with no language label -> the "code" fallback label.
        let src = "```\nplain body\n```\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(
            header_line(&lines).to_string().contains("code"),
            "無ラベルは code と表示されるはず"
        );
        assert!(lines.iter().any(|l| l.to_string().contains("plain body")));

        // Empty fence: header + padding row, no leaked fence marker, no panic.
        let src = "```rust\n```\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(
            !lines.iter().any(|l| l.to_string().trim() == "```"),
            "空フェンスにフェンス記号が生テキストとして残ってはいけない"
        );

        // A top-level 4-column indented block.
        let src = "para\n\n    indented body\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(lines
            .iter()
            .any(|l| l.to_string().contains("indented body")));

        // Two blank-line-separated fences stay isolated — each keeps only its own body, not
        // merged into a single run by the style-based grouping.
        let src = "```a\nfirst\n```\n\n```b\nsecond\n```\n";
        assert_eq!(
            rendered_code_blocks(src),
            2,
            "2つの独立したブロックが1つに融合していないか"
        );
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        let first = lines
            .iter()
            .find(|l| l.to_string().contains("first"))
            .expect("1つ目の本文が無い");
        let second = lines
            .iter()
            .find(|l| l.to_string().contains("second"))
            .expect("2つ目の本文が無い");
        assert!(
            !first.to_string().contains("second"),
            "1つ目の本文に2つ目が混ざっている"
        );
        assert!(
            !second.to_string().contains("first"),
            "2つ目の本文に1つ目が混ざっている"
        );

        // A tilde fence.
        let src = "~~~rust\ntilde body\n~~~\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(lines.iter().any(|l| l.to_string().contains("tilde body")));

        // A 5-backtick fence, no nesting.
        let src = "`````rust\nfive body\n`````\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(lines.iter().any(|l| l.to_string().contains("five body")));

        // A fence inside a GitHub alert: body content survives with the `>` prefix stripped.
        let src = "> [!NOTE]\n> ```rust\n> alert body\n> ```\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        let body = lines
            .iter()
            .find(|l| l.to_string().contains("alert body"))
            .expect("アラート内の本文が描かれていない");
        assert!(
            !body.to_string().trim_start().starts_with('>'),
            "`>` がアラート本文に残ってはいけない"
        );

        // A fence inside an *open* `<details>`: body content survives.
        let src =
            "<details open>\n<summary>S</summary>\n\n```rust\ndetails body\n```\n\n</details>\n";
        assert_eq!(rendered_code_blocks(src), 1);
        let lines = render_markdown(src, 40, CodeStyle::default(), "TwoDark", false);
        assert!(lines.iter().any(|l| l.to_string().contains("details body")));
    }

    /// `flush_code_run`'s self-defense clause (see its doc comment): a run of
    /// `is_code_block_line`-styled lines that doesn't have the shape a real fence/indented block
    /// always produces — too short, or not opening with a `` ``` `` marker (the shape tui-markdown
    /// actually produces for an indented code block nested in a plain, non-alert block quote,
    /// where every line — including the still-`code()`-styled marker/body lines — gets a `"> "`
    /// prefix from tui-markdown's own native blockquote handling) — is handed back completely
    /// unchanged rather than mis-parsed as code or indexed into with a panic. This exercises that
    /// branch directly (bypassing tui-markdown) with both a too-short run and the blockquote
    /// shape, and separately confirms it's what tui-markdown really produces for that document.
    #[test]
    fn decorate_code_blocks_degrades_a_malformed_white_run_unchanged() {
        let lonely = vec![Line::styled("stray", Style::new().fg(Color::White))];
        let out = decorate_code_blocks(lonely, 40, CodeStyle::default(), "TwoDark");
        let texts: Vec<String> = out.iter().map(|l| l.to_string()).collect();
        assert_eq!(
            texts,
            vec!["stray".to_string()],
            "長さ1の run はそのまま素通しされるべき(パニックしない)"
        );

        let blockquote_like = vec![
            Line::styled("> ```", Style::new().fg(Color::White)),
            Line::styled("> code", Style::new().fg(Color::White)),
            Line::styled("> ```", Style::new().fg(Color::White)),
        ];
        let out = decorate_code_blocks(blockquote_like, 40, CodeStyle::default(), "TwoDark");
        let texts: Vec<String> = out.iter().map(|l| l.to_string()).collect();
        assert_eq!(
            texts,
            vec![
                "> ```".to_string(),
                "> code".to_string(),
                "> ```".to_string()
            ],
            "先頭が ``` で始まらない run はコードブロックとして解釈されず素通しされるべき"
        );

        // Confirm this is really the shape tui-markdown produces for a plain (non-alert) block
        // quote wrapping an indented code block — not a contrived shape this test invented.
        let opts = Options::new(KonomaStyles { code_bg: None });
        let text = tui_markdown::from_str_with_options("> para\n>\n>     code\n", &opts);
        let actual: Vec<(String, bool)> = text
            .lines
            .iter()
            .map(|l| (l.to_string(), is_code_block_line(l)))
            .collect();
        assert_eq!(
            actual,
            vec![
                ("> para".to_string(), false),
                ("> ".to_string(), false),
                ("> ```".to_string(), true),
                ("> code".to_string(), true),
                ("> ```".to_string(), true),
            ],
            "tui-markdown の前提が変わった(ブロック引用内の字下げコードの形)"
        );
        // ...and the full pipeline still draws zero code headers for it, matching this crate's
        // behavior (pinned end-to-end by
        // `heading_gap_is_fixed_and_plain_blockquote_gap_has_no_mismatch`).
        assert_eq!(rendered_code_blocks("> para\n>\n>     code\n"), 0);
    }

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

    /// The write-back scanner must find **exactly** the checkboxes the renderer draws, for every
    /// construct. When the two disagree the safety check in `md_tasks` cancels *every* toggle in the
    /// document (a safe fallback, principle #3), so a single stray construct breaks the whole file.
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
        let (lines, _) = render_markdown_with_images(
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

    /// Root cause (indented code blocks): a top-level 4+-column indented code block was not
    /// recognized by the write-back scanner at all — any document containing one refused `y c` for
    /// **every** code block in it, fenced ones included (the count guard is document-wide). Runs
    /// `code_corpus` the same way `code_block_scanner_counts_exactly_what_the_renderer_draws` runs
    /// its own hand-picked cases, covering indented blocks alone and interacting with every other
    /// construct the scanner special-cases (fences, alerts, `<details>`, front matter, CRLF, lists).
    #[test]
    fn code_block_scanner_matches_renderer_across_indented_code_corpus() {
        for (name, src) in code_corpus::cases() {
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
    #[test]
    fn task_scanner_matches_renderer_across_indented_code_corpus() {
        for (name, src) in code_corpus::cases() {
            set_details_open(Vec::new());
            let drawn = rendered_tasks(src);
            let scanned = task_source_locs(src, &[' ', 'x'], &[]).len();
            assert_eq!(
                drawn, scanned,
                "{name}: 画面のチェックボックス数と書き戻しスキャナの数が食い違う\
                 (この文書ではトグルが全部中止される)\n--- src ---\n{src}"
            );
        }
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
        let parts = split_details(src);
        assert_eq!(
            parts.len(),
            1,
            "文書全体が1つの Text パートのまま(details として切り出されない)"
        );
        match &parts[0] {
            DetailsPart::Text(t) => assert_eq!(t, src),
            DetailsPart::Details { .. } => {
                panic!("フェンス内の <details> 風の行を本物の details として切り出してしまった")
            }
        }
    }

    /// A document that is, in its entirety, one correctly-recognized fence containing a blank line as
    /// literal content (because the shorter nested marker inside it does not actually close it) has no
    /// safe split point anywhere — the function must refuse (`None`, falling back to plain-text
    /// rendering for the whole block) rather than pick the blank line inside the fence, which the old
    /// toggle did (closing early on the inner marker made everything after it look "outside the fence",
    /// including a blank line that is actually literal fence content — splitting there would tear the
    /// fence in half and corrupt its rendering).
    #[test]
    fn split_block_for_retry_never_splits_inside_a_nested_shorter_fence() {
        let src = "````md\n```mermaid\n\ninner blank\n\n```\n````\n";
        assert_eq!(split_block_for_retry(src), None);
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
    /// directly, replacing the old "known gap" assertion. (2) Any indented block inside a *plain*,
    /// non-alert block quote remains genuinely unsupported on **both** sides — out of scope for this
    /// fix (already true for fenced code too, before it: the block quote's own `>` marker sits in
    /// front of every source line, so `decorate_code_blocks`'s literal `` "```" `` check never
    /// matches it — matching the scanner, which also never opens a code block inside a bare `>`
    /// prefix) — pinned here as a non-regression: both sides stay at 0, so there is no drawn/scanned
    /// mismatch to refuse anything over.
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
            0,
            "素の(アラートでない)引用内のフェンス/字下げはレンダラ側も未対応(既存の制約)"
        );
        assert!(
            code_block_source_locs(quoted, &[]).is_empty(),
            "スキャナも同様に検出しない(食い違いなし・新規リグレッションではなく既知の制約の温存)"
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
        let lines = render_markdown_tasks_opts(
            src,
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
            ("unterminated is dropped", "```mermaid\nno close\n", &[]),
            ("empty body", "```mermaid\n```\n", &[""]),
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (lines, _imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (lines, imgs) = render_markdown_with_images(
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
        let (_lines, imgs) = render_markdown_with_images(
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
