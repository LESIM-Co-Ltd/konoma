// 内蔵 Markdown / Mermaid レンダラ (M3)。
//
// 構成 (2026-06 確定):
//   - md 装飾   : `tui-markdown`(ratatui-core 0.1 = 我々の 0.30 と同一型)の `from_str`。
//                 見出し/強調/コード/リスト/表/引用/リンク等を装飾する。tui-markdown の
//                 highlight-code(syntect 既定=oniguruma C)は無効化し、```lang フェンスの
//                 コードは konoma 側で純 Rust syntect(`preview::code::highlight_lang`)で着色する
//                 ＝ oniguruma 不要で配布容易性(PRD §5)を保つ。単体コードファイルと同一経路。
//   - Mermaid   : `mermaid-text`(依存 unicode-width のみ・純 Rust)で Unicode 罫線テキスト化。
//                 ブラウザ・画像プロトコル不要。md 内の ```mermaid フェンスは横取りして合成する。
//
// 当初候補の `ratatui-markdown` は ratatui ^0.29 依存で、画像プレビュー(ratatui-image 11 =
// ratatui 0.30 必須)と両立できないため不採用。詳細は Cargo.toml のコメント参照。
//
// 失敗時の安全側 (設計原則3): mermaid の描画に失敗/未対応(例: state 図)なら、生ソースを
// 淡色で全画面表示する (クラッシュさせない)。

use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use tui_markdown::{Options, StyleSheet};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// 装飾の配色。コードブロックは「特殊エリア」として背景＋左ガターで囲う。
const CODE_GUTTER_FG: Color = Color::Cyan;
const HEAD_FG: Color = Color::Cyan;
const TABLE_BORDER_FG: Color = Color::Rgb(90, 98, 120); // 淡い罫線(本文より控えめ)

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
            1 | 2 => base, // 直下に全幅ルールを足して“大きさ”を表現
            3 => base.add_modifier(Modifier::ITALIC),
            _ => Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::DIM | Modifier::ITALIC),
        }
    }
    fn code(&self) -> Style {
        // インラインコード (`...`)。背景色は設定で可変 (None なら背景なし)。
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
                            AlertPart::Text(t) => {
                                render_md_text(&mut out, &t, width, code, theme, icons, tasks)
                            }
                            AlertPart::Alert { kind, title, body } => out.extend(render_alert(
                                kind, &title, &body, width, code, theme, icons, tasks,
                            )),
                        }
                    }
                } else {
                    render_md_text(&mut out, &text, width, code, theme, icons, tasks);
                }
            }
            Segment::Mermaid(code) => out.extend(render_mermaid_block(&code, width)),
        }
    }
    out
}

/// One Markdown text run: pull out collapsible `<details>` blocks first (they span blank lines, so
/// the plain HTML-block splitter can't keep them whole), then render the rest through the normal
/// tables → HTML → tui-markdown pipeline. Shared by the top level and by GitHub-alert bodies.
fn render_md_text(
    out: &mut Vec<Line<'static>>,
    text: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) {
    for part in split_details(text) {
        match part {
            DetailsPart::Text(t) => render_md_text_inner(out, &t, width, code, theme, icons, tasks),
            DetailsPart::Details {
                open_attr,
                summary,
                body,
            } => {
                let open = next_details_open(open_attr);
                out.extend(render_details(
                    open, &summary, &body, width, code, theme, icons, tasks,
                ));
            }
        }
    }
}

/// The tables → HTML-block → tui-markdown pipeline for one Markdown text run (no `<details>`).
fn render_md_text_inner(
    out: &mut Vec<Line<'static>>,
    text: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) {
    let opts = Options::new(KonomaStyles { code_bg: code.bg });
    // tui-markdown は GFM 表を1行に潰すため、表ブロックだけ先に横取りして
    // 自前で罫線描画する。残りのテキストは従来どおり tui-markdown へ。
    for part in split_tables(text) {
        match part {
            MdPart::Text(t) => {
                if t.trim().is_empty() {
                    continue;
                }
                // HTML ブロック(<details> 等)は tui-markdown が中身ごと捨てるため、
                // 先に横取りしてタグを剥いだテキストで見せる(原則#3)。
                for hp in split_html_blocks(&t) {
                    match hp {
                        HtmlPart::Text(t2) => {
                            if t2.trim().is_empty() {
                                continue;
                            }
                            // from_str_with_options は借用した Text<'_> を返すので即 'static へ複製。
                            // tui-markdown は特定入力(例: loose リスト内のタスク項目)で
                            // panic する(0.3.7/0.3.8 で確認)。原則#3=クラッシュさせない:
                            // 捕捉し、二分割の再帰で**最小の失敗ブロックだけ**を素の
                            // テキストへ降格する(丸ごと降格だと実在の文書が全編
                            // 無装飾になる — 2026-07-07 ユーザー報告)。
                            render_text_block_safe(
                                out, &t2, &opts, width, code, theme, icons, tasks, 0,
                            );
                        }
                        HtmlPart::Html(h) => out.extend(render_html_block(&h)),
                    }
                }
            }
            MdPart::Table(raw) => out.extend(render_table(&raw, width, icons)),
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
    // 既定の panic hook は stderr に書き raw mode の画面を汚すので、捕捉中だけ黙らせる。
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
#[allow(clippy::too_many_arguments)] // 既存7引数+mermaid/math スロット(呼び口は app 1箇所+テスト)
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
    // mermaid フェンスの抽出は「1つでも Image/Loading になり得る時」だけ(=Text 固定なら従来どおり
    // テキスト run に残し、既存の描画・テストを一切変えない)。判定は空 probe で代表させる。
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
                // 空フェンス(書きかけの ```mermaid だけ)は図になり得ない: slot を経由せず
                // テキスト描画へ。slot の空文字は「抽出 ON か」の probe と区別できないため、
                // 従来は probe 応答の Loading を拾って永久に「loading」行のままだった。
                if fence.trim().is_empty() {
                    out.extend(render_mermaid_block(&fence, width));
                    continue;
                }
                match mermaid_slot(&fence) {
                    MermaidSlot::Image { cols, rows } => {
                        let url = mermaid_fence_url(&fence);
                        let mut ls = mermaid_placeholder_lines(cols, rows, width, mermaid_caption);
                        // 先頭はキャプション行(フォーカス番兵)。予約行=画像の重畳先はその次から。
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
    // DIM は付けない: フォーカスの REVERSED と重なると文字が背景に沈んで読めなくなる
    // (Ghostty 実機で白バー化して捕まった)。Cyan+ITALIC+接頭辞で番兵として十分に一意。
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
    // 下マージン行: フォーカス枠の下辺を描く場所(予約域の外の本文行を上書きしないため)。
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
    // 未クローズの mermaid フェンス: 生テキストとして安全に戻す(原則#3・欠落させない)。
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
                // 水平線: tui-markdown は Rule をテキストのまま出すので全幅の罫線へ。
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
    let Some(state) = task_prefix_state(joined.trim_start(), tasks) else {
        return l;
    };
    let pat = format!("[{state}]");
    let Some(pos) = joined.find(&pat) else {
        return l;
    };
    // 直後の半角スペースもマーカー span に取り込む(必ず在る: 検出条件が "] " 必須)。
    // フォーカスの反転がグリフ+空白の2セルを覆うので、Nerd Font グリフを全角(2セル)幅で
    // 描くフォント(HackGen NF 等)でもグリフ全体が反転域に収まる。ツリーの
    // 「アイコン+空白」と同じ、はみ出し許容の流儀。
    let trail_space = joined[pos + pat.len()..].starts_with(' ');
    let end = pos + pat.len() + usize::from(trail_space);
    let (style, alignment) = (l.style, l.alignment);
    // joined 上のマーカー範囲 [pos, end) を専用 span に置き換える。tui-markdown はカスタム状態
    // (`[/]` 等)を複数 span に割ることがあるので、span 単位でなく範囲で分割する。
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut off = 0usize;
    let mut inserted = false;
    for sp in l.spans {
        let s = sp.content.as_ref();
        let (a, b) = (off, off + s.len());
        off = b;
        if b <= pos || a >= end {
            out.push(sp); // マーカー範囲外はそのまま
            continue;
        }
        // マーカー範囲と重なる span: 範囲外にはみ出す前後だけ元様式で残す。
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

/// The state char if the (whitespace-trimmed) line starts with a task marker `- [<c>] ` where `<c>`
/// is a recognized state (` `/`x`/`X` always, plus the configured custom states).
fn task_prefix_state(t: &str, tasks: &[char]) -> Option<char> {
    let rest = t.strip_prefix("- [")?;
    let c = rest.chars().next()?;
    if !rest[c.len_utf8()..].starts_with("] ") {
        return None;
    }
    is_task_state(c, tasks).then_some(c)
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

/// Raw inner text of each fenced code block (```` ``` ```` / `~~~`), in document order, skipping
/// `mermaid` fences (they are diverted to diagram rendering and produce no code-block header).
/// Used by the "copy focused code block" action: the Nth entry maps to the Nth focusable block.
/// The caller cross-checks the count against the on-screen headers before copying (safe fallback).
pub(crate) fn code_block_source_locs(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let fence = if t.starts_with("```") {
            Some('`')
        } else if t.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        let Some(fc) = fence else {
            i += 1;
            continue;
        };
        let info = t.trim_start_matches(fc).trim();
        let is_mermaid = is_mermaid_info(info);
        let close = fc.to_string().repeat(3);
        i += 1; // opening fence
        let mut body = Vec::new();
        while i < lines.len() && !lines[i].trim_start().starts_with(&close) {
            body.push(lines[i].to_string());
            i += 1;
        }
        if i < lines.len() {
            i += 1; // closing fence
        }
        if !is_mermaid {
            out.push(body.join("\n"));
        }
    }
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

/// Scan Markdown source for task markers, in document order, skipping exactly what the render
/// pipeline diverts away from `decorate_extras`: code/mermaid fences, HTML blocks (start line up
/// to the next blank line) and GFM table blocks. This keeps the Nth checkbox on screen aligned
/// with the Nth `TaskLoc`, so a toggle edits the right line. Pathological documents could still
/// disagree — the caller cross-checks count and current state before writing.
pub(crate) fn task_source_locs(src: &str, tasks: &[char]) -> Vec<TaskLoc> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if t.starts_with("```") {
            fence = Some('`');
            i += 1;
            continue;
        }
        if t.starts_with("~~~") {
            fence = Some('~');
            i += 1;
            continue;
        }
        if is_html_block_start(line) {
            while i < lines.len() && !lines[i].trim().is_empty() {
                i += 1;
            }
            continue;
        }
        if looks_like_table_row(line) && i + 1 < lines.len() && is_table_delimiter(lines[i + 1]) {
            i += 2;
            while i < lines.len() && looks_like_table_row(lines[i]) {
                i += 1;
            }
            continue;
        }
        let indent = line.len() - t.len();
        if let Some(state) = task_prefix_state(t, tasks) {
            out.push(TaskLoc {
                line: i,
                state_off: indent + 3, // "- [" の直後
                state,
            });
        }
        i += 1;
    }
    out
}

/// Turn the range enclosed by ```lang fences into a special area. When `code.bg`=None there is no background band
/// (left gutter and foreground color only). The body collects the raw text and highlights it all at once at the closing
/// fence via **our own syntect** (tui-markdown's highlight-code is disabled = avoids oniguruma).
/// Highlighting it all together lets multi-line comments/strings be colored correctly.
fn decorate_code_blocks(
    lines: Vec<Line<'static>>,
    width: u16,
    code: CodeStyle,
    theme: &str,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let code_bg = code.bg;
    let mut out = Vec::with_capacity(lines.len());
    let mut in_code = false;
    let mut lang = String::new();
    let mut body: Vec<String> = Vec::new();
    for line in lines {
        let text = line.to_string();
        let trimmed = text.trim_start();
        if !in_code && trimmed.starts_with("```") {
            in_code = true;
            lang = trimmed.trim_matches('`').trim().to_string();
            let label = if lang.is_empty() {
                "code"
            } else {
                lang.as_str()
            };
            out.push(code_header(label, w, code));
            body.clear();
            continue;
        }
        if in_code && is_closing_fence(trimmed) {
            in_code = false;
            out.extend(highlight_body(
                &body,
                &lang,
                w,
                code_bg,
                theme,
                code.tab_width,
                code.wrap,
            ));
            // 下端のパディング行 (ガターだけ) でブロックの終端を示す。
            out.push(pad_to_width(vec![gutter_span(code_bg)], w, code_bg));
            body.clear();
            continue;
        }
        if in_code {
            // 本文行は生テキストを集める(着色は閉じフェンスで一括＝複数行構文を正しく追う)。
            body.push(text);
            continue;
        }
        out.push(line);
    }
    // 閉じフェンスが無いまま終端した場合も本文を流す(安全側)。
    if in_code {
        out.extend(highlight_body(
            &body,
            &lang,
            w,
            code_bg,
            theme,
            code.tab_width,
            code.wrap,
        ));
        out.push(pad_to_width(vec![gutter_span(code_bg)], w, code_bg));
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
    // タブ展開は **ガター/全幅パディングを付ける前** に行う(後だと幅計算が狂って帯が崩れる)。
    // 桁追跡はコード先頭(0桁)基準。ガター付与で全行が一律右シフトするので整列は保たれる。
    let hl = crate::preview::code::expand_tabs(
        crate::preview::code::highlight_lang(&src, lang, theme),
        tab_width,
    );
    // 折返し有効時はここで幅に合わせて**事前折返し**する(ガター2桁を差し引いた幅)。
    // Paragraph の折返しに任せると、折返し2行目以降に ▎ ガターも背景帯も付かず縦の帯が
    // 途切れる(ユーザー報告 2026-07-07)。全視覚行をここで確定させれば Paragraph は
    // 折返し不要になり、ガター/帯が必ず連続する。
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
        // 上限超過は最も昔に使われたエントリから追い出す(有界 LRU)。
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
#[allow(clippy::too_many_arguments)]
fn render_text_block_safe(
    out: &mut Vec<Line<'static>>,
    src: &str,
    opts: &Options<KonomaStyles>,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
    depth: u8,
) {
    if src.trim().is_empty() {
        return;
    }
    if let Some(lines) = render_md_segment(src, opts) {
        out.extend(decorate_md_lines(lines, width, code, theme, icons, tasks));
        return;
    }
    // 深さ上限 or これ以上割れない → このブロックだけ素のテキストで(内容は失わない)。
    if depth >= 8 {
        out.extend(src.lines().map(|l| Line::from(l.to_string())));
        return;
    }
    match split_block_for_retry(src) {
        Some((a, b)) => {
            render_text_block_safe(out, a, opts, width, code, theme, icons, tasks, depth + 1);
            render_text_block_safe(out, b, opts, width, code, theme, icons, tasks, depth + 1);
        }
        None => out.extend(src.lines().map(|l| Line::from(l.to_string()))),
    }
}

/// Split point for the panic-isolation retry: the blank line nearest the middle that is **not**
/// inside a code fence (splitting a fence would corrupt its rendering). Falls back to the
/// non-fenced newline nearest the middle; None when the block is a single line.
fn split_block_for_retry(src: &str) -> Option<(&str, &str)> {
    let mut blanks: Vec<usize> = Vec::new(); // 空行の開始オフセット
    let mut newlines: Vec<usize> = Vec::new(); // 改行位置(フェンス外)
    let mut in_fence = false;
    let mut off = 0usize;
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let end = off + line.len();
        if !in_fence && line.ends_with('\n') {
            if line.trim().is_empty() {
                blanks.push(off);
            }
            newlines.push(end);
        }
        off = end;
    }
    let mid = src.len() / 2;
    // 空行 = その行ごと前半に含め、後半は空行の次から。前後どちらかが空にならない候補のみ。
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
            spans.remove(0); // 先頭の "#.. " を捨てる
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
    // ヘッダのガターは番兵スタイル(is_code_header_span)で識別=Tab フォーカス対象の目印。
    let gutter = {
        let st = code_header_marker_style();
        let st = match code_bg {
            Some(bg) => st.bg(bg),
            None => st,
        };
        Span::styled("▎ ", st)
    };
    let badge_text = format!(" {label} "); // 前後に余白を持つバッジ
                                           // バッジのスタイル: 背景ありなら(指定背景＋太字白)、無しなら淡色イタリック。
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
        // 右寄せ: ガターとバッジの間を本文背景色で埋め、バッジを右端へ。
        spans.push(Span::styled(" ".repeat(w - gutter_w - badge_w), fill_style));
        spans.push(badge);
    } else {
        // 左寄せ(または幅不足): ガター直後にバッジ、残りを本文背景色で埋める。
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
        // panic: mermaid-text 内部のバグ (CJK 境界等)。生ソース表示で安全に継続。
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
        _ => Theme::dark(), // 既定: konoma のダーク基調に馴染む(mermaid-js dark 移植)
    };
    // 背景は描かない(fill="none")= kitty graphics では端末背景が透ける。図がテーマ色の
    // 「白いカード」にならず、どの端末テーマにも馴染む(ユーザー指摘 2026-07-17)。
    t.background = "none".to_string();
    // フォント計測は**全テーマで modern に統一**する。テーマ毎にフォント(trebuchet 16 等)が違うと
    // ラベル箱の実測サイズが変わり、経路探索が「大きく左へ迂回するリング」を選ぶ配置バグを実測で
    // 再現・解消した(2026-07-17 ユーザー報告)。色はテーマのまま・レイアウトだけ検証済みの計測に固定。
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
fn split_math(text: &str) -> Vec<MathPart> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
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
        // Multi-line display opener: a line that is exactly `$$` or `\[` (collect to the matching close).
        let trimmed = bare.trim();
        if trimmed == "$$" || trimmed == "\\[" {
            let closer = if trimmed == "$$" { "$$" } else { "\\]" };
            let mut body = String::new();
            let mut j = i + 1;
            let mut found = false;
            while j < lines.len() {
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
            let start = i;
            let mut run = 0;
            while i < n && bytes[i] == b'`' {
                run += 1;
                i += 1;
            }
            let mut j = i;
            let mut close = None;
            while j < n {
                if bytes[j] == b'`' {
                    let mut r = 0;
                    while j < n && bytes[j] == b'`' {
                        r += 1;
                        j += 1;
                    }
                    if r == run {
                        close = Some(j);
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            match close {
                Some(end) => {
                    buf.push_str(&line[start..end]);
                    i = end;
                }
                None => buf.push_str(&line[start..i]), // unmatched backticks: literal
            }
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
struct Fence {
    ch: u8,
    len: usize,
}

/// If the line is a code fence (three or more of ``` or ~~~), return (fence, info string).
fn parse_fence(line: &str) -> Option<(Fence, String)> {
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
    // 開いているフェンス。bool は「mermaid ブロックか」。
    let mut open: Option<(Fence, bool)> = None;

    for line in src.split_inclusive('\n') {
        let bare = line.strip_suffix('\n').unwrap_or(line);
        match &open {
            None => {
                if let Some((fence, info)) = parse_fence(bare) {
                    if is_mermaid_info(&info) {
                        // mermaid ブロック開始: それまでの md を確定し、フェンス行自体は捨てる。
                        if !md.is_empty() {
                            segments.push(Segment::Md(std::mem::take(&mut md)));
                        }
                        open = Some((fence, true));
                    } else {
                        // 通常のコードフェンス: tui-markdown に渡すため md にそのまま積む。
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
                        md.push_str(line); // 閉じフェンスも md に含める
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

    // 末尾の積み残しを確定。未閉鎖の mermaid は描画を試みる(失敗時は raw 表示にフォールバック)。
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

// ---- GFM 表のレンダリング ----
// tui-markdown 0.3.7 は表を1行に潰す(表未対応)。そこで表ブロックを横取りし、列の表示幅
// (全角=2)を測って罫線(┌┬┐ │ ├┼┤ └┴┘)で描く。幅超過時はセルを折り返して収める。

enum MdPart {
    Text(String),
    Table(String),
}

// ---- HTML ブロックの救出 --------------------------------------------------------
// tui-markdown(pulldown-cmark) は Html イベントを黙って捨てるため、`<details>` 等の
// ブロックの**中身のテキストごと**消えていた。ブロックを横取りしてタグを剥いだテキストを
// 表示する(konoma は HTML を描画しない=安全な降格・原則#3)。コメント <!-- --> は丸ごと非表示。

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
fn split_html_blocks(md: &str) -> Vec<HtmlPart> {
    let lines: Vec<&str> = md.lines().collect();
    let mut parts = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_html_block_start(lines[i]) {
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
    // タグ/コメントを文字走査で除去(行を跨ぐ <!-- --> にも対応)。
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
/// header and captures the following blockquote lines as its (marker-stripped) body.
fn split_alerts(md: &str) -> Vec<AlertPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some((kind, title)) = parse_alert_header(lines[i]) {
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
/// and the Markdown body, every line carrying a colored left bar (`▌`). The body is rendered by the
/// shared Markdown pipeline, so links, code, lists etc. inside an alert work like anywhere else.
#[allow(clippy::too_many_arguments)]
fn render_alert(
    kind: AlertKind,
    title: &str,
    body: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) -> Vec<Line<'static>> {
    let color = kind.color();
    let bar = || Span::styled("▌ ".to_string(), Style::new().fg(color));
    let mut out = Vec::new();
    // Header: bar + optional icon + label (+ optional Obsidian-style title), all in the alert color.
    let mut header = vec![bar()];
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
    out.push(Line::from(header));
    // Body via the shared pipeline (width reduced by the 2-col bar), each line bar-prefixed.
    let inner = width.saturating_sub(2);
    let mut body_lines = Vec::new();
    render_md_text_inner(&mut body_lines, body, inner, code, theme, icons, tasks);
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

/// Split off `<details>` … `</details>` blocks (spanning blank lines) from a text run, extracting the
/// `<summary>` and the body. Non-details text is returned verbatim for the normal pipeline.
/// Fence-aware (a `<details>` inside a code fence is left as text) so the block count and order match
/// `collect_details_open`, which seeds the per-ordinal open state.
fn split_details(md: &str) -> Vec<DetailsPart> {
    let lines: Vec<&str> = md.lines().collect();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            text.push_str(lines[i]);
            text.push('\n');
            i += 1;
            continue;
        }
        if !in_fence {
            if let Some(open_attr) = details_open_tag(lines[i]) {
                if !text.is_empty() {
                    parts.push(DetailsPart::Text(std::mem::take(&mut text)));
                }
                i += 1;
                let mut block = Vec::new();
                while i < lines.len() && !is_details_close(lines[i]) {
                    block.push(lines[i]);
                    i += 1;
                }
                if i < lines.len() {
                    i += 1; // skip </details>
                }
                let (summary, body) = extract_summary_body(&block.join("\n"));
                parts.push(DetailsPart::Details {
                    open_attr,
                    summary,
                    body,
                });
                continue;
            }
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

/// Render a `<details>` block: a summary line marked `▾`/`▸` (a Tab-focus toggle), and — when open —
/// its Markdown body indented under a colored left bar (like an alert). `summary` empty → "Details".
#[allow(clippy::too_many_arguments)]
fn render_details(
    open: bool,
    summary: &str,
    body: &str,
    width: u16,
    code: CodeStyle,
    theme: &str,
    icons: bool,
    tasks: &[char],
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let arrow = if open { '▾' } else { '▸' };
    let label = if summary.trim().is_empty() {
        "Details"
    } else {
        summary.trim()
    };
    out.push(Line::from(vec![
        Span::styled(format!("{arrow} "), details_marker_style()),
        Span::styled(label.to_string(), Style::new().add_modifier(Modifier::BOLD)),
    ]));
    if open && !body.trim().is_empty() {
        let bar = || Span::styled("▏ ".to_string(), Style::new().fg(TABLE_BORDER_FG));
        let inner = width.saturating_sub(2);
        let mut body_lines = Vec::new();
        render_md_text_inner(&mut body_lines, body, inner, code, theme, icons, tasks);
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
/// `MdItemKind::Details` ordinals — a nested `<details>` is swallowed into its parent's body and not
/// counted separately (counting it here, as a flat tag scan did, drifted the per-ordinal open state
/// so a later block read the wrong slot). Fence-aware via `split_details`. Used by the app to seed
/// the default open state.
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
    let mut in_fence = false;
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence || !line.contains('<') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut s = line.to_string();
        s = replace_tag_pair(&s, "kbd", |i| format!("`{i}`"));
        s = replace_tag_pair(&s, "del", |i| format!("~~{i}~~"));
        s = replace_tag_pair(&s, "s", |i| format!("~~{i}~~"));
        s = replace_tag_pair(&s, "strike", |i| format!("~~{i}~~"));
        s = replace_tag_pair(&s, "sup", |i| map_all_or_keep(i, sup_char));
        s = replace_tag_pair(&s, "sub", |i| map_all_or_keep(i, sub_char));
        for br in ["<br>", "<br/>", "<br />", "<BR>", "<BR/>", "<BR />"] {
            s = s.replace(br, "  \n");
        }
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

/// Scan `line` for footnote references `[^id]`, returning the ids in order.
fn find_footnote_refs(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
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
    ids
}

/// Replace footnote references `[^id]` in `line` with a superscript number (only ids present in
/// `num`; an undefined reference is left literal, matching GitHub).
fn replace_footnote_refs(line: &str, num: &std::collections::HashMap<String, usize>) -> String {
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
    // Pass 1: collect definitions (fence-aware) and mark their lines.
    let mut defs: Vec<(String, String)> = Vec::new();
    let mut is_def = vec![false; lines.len()];
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
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
    in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || is_def[i] {
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
    in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence {
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
fn split_tables(md: &str) -> Vec<MdPart> {
    let lines: Vec<&str> = md.lines().collect();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < lines.len() {
        // 表開始 = 現在行がヘッダ候補 かつ 次行が区切り行。
        if i + 1 < lines.len() && looks_like_table_row(lines[i]) && is_table_delimiter(lines[i + 1])
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
            while j < lines.len() && looks_like_table_row(lines[j]) {
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
    // 末尾の区切り `|` を除く。ただし `\|`(エスケープ=リテラル)は区切りではないので残す。
    let t = if t.ends_with('|') && !t.ends_with("\\|") {
        &t[..t.len() - 1]
    } else {
        t
    };
    // GFM: セル分割は**エスケープされていない** `|` で行い、`\|` はリテラルの `|` に戻す。
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

// ---- テーブルセル内のインラインリンク --------------------------------------------
// 表は tui-markdown を通らない自前描画のため、セル内の `[label](url)` をここで解釈する。
// 表示は**ラベルのみ**(リンク様式=青下線)。URL はラベル直後に HIDDEN 修飾の「隠しターゲット」
// スパンとして埋め、app 側の collapse_links が描画直前に取り除いて targets(Tab/Enter の
// リンク先)へ回収する。桁揃えはラベル幅で計算する(隠しスパンは表示前に消えるので数えない)。

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
            continue; // 空(`****` 等)は素通し
        }
        let inner = &r[..end];
        if *open != "`"
            && (inner.starts_with(char::is_whitespace) || inner.ends_with(char::is_whitespace))
        {
            continue; // 強調は内側の空白を許さない(乗算 `2 * 3` 等の誤検出防止)
        }
        let style = match *open {
            "***" => Style::new().add_modifier(Modifier::BOLD | Modifier::ITALIC),
            "**" => Style::new().add_modifier(Modifier::BOLD),
            "*" => Style::new().add_modifier(Modifier::ITALIC),
            "~~" => Style::new().add_modifier(Modifier::CROSSED_OUT),
            // インラインコード: tui-markdown の段落内コードと同じ白前景。
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
        // インライン強調/コード/打消し(リンクより先に判定しても衝突しない=開始文字が異なる)。
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
                            // "[label](url)" 全体を消費して続きから。
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
    let mut header_rows = 0usize; // 区切り行より前(=ヘッダ)の行数
    let mut aligns: Vec<ColAlign> = Vec::new();
    for line in raw.lines() {
        if is_table_delimiter(line) {
            header_rows = rows.len();
            aligns = parse_table_aligns(line); // 整列コロン(:---:)を列ごとに反映
            continue;
        }
        rows.push(
            parse_table_row(line)
                .into_iter()
                .map(|c| {
                    let mut segs = parse_cell_segments(&c);
                    // 段落リンクと同じ見た目にする: アイコン有効ならラベルへ前置し、
                    // **幅計算の前に**組み込む(後付けだと桁揃えが崩れる)。
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
    // 自然列幅(全角考慮・リンクはラベル幅)。最低1。
    let mut col_w = vec![1usize; ncol];
    for r in &rows {
        for (c, cell) in r.iter().enumerate() {
            col_w[c] = col_w[c].max(segs_width(cell));
        }
    }
    // 表の総表示幅 = Σcol_w + 罫線│(ncol+1) + 各列の左右余白(2*ncol)。
    // 幅を超える間、最も広い列を1ずつ削る(最低1)。削られた列はセルを折返して収める。
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
        // 各セルを列幅で折返し、行内の最大物理行数に合わせて縦に展開する。
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
                // 整列コロン(:---:)に従いパディングを左右へ配分(既定は左寄せ)。
                let (lp, rp) = match aligns.get(c).copied().unwrap_or(ColAlign::Left) {
                    ColAlign::Left => (0, pad),
                    ColAlign::Right => (pad, 0),
                    ColAlign::Center => (pad / 2, pad - pad / 2),
                };
                spans.push(Span::styled(format!(" {}", " ".repeat(lp)), cell_style));
                for seg in segs {
                    match seg {
                        // スタイル付きテキスト(セル内の **bold**/*italic*/`code`/~~strike~~)。
                        // ヘッダの太字等(cell_style)の上に patch で重ねる。
                        CellSeg::Text { text, style } => {
                            spans.push(Span::styled(text.clone(), cell_style.patch(*style)))
                        }
                        // リンク: ラベル(青下線)＋隠しターゲット(URL・表示前に collapse_links が
                        // 除去して Tab/Enter のリンク先へ回収)。幅はラベルのみで数えてある。
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
        // ヘッダ最終行の直後に区切り罫線。
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

    #[test]
    fn code_block_tabs_expand_to_marker() {
        // Markdown のコードブロック内のタブも単体コードと同様に「→＋空白」に展開する(設定 tab_width)。
        let md = "```ts\nfunction f() {\n\tconst x = 1;\n}\n```\n";
        let lines = render_markdown(md, 40, BG, "TwoDark", false);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        // タブ行はマーカー → を含み、生のタブ文字は残さない。
        let tab_line = texts
            .iter()
            .find(|t| t.contains("const x"))
            .expect("コード行が無い");
        assert!(
            tab_line.contains('→'),
            "タブが可視化されていない: {tab_line:?}"
        );
        assert!(!tab_line.contains('\t'), "生タブが残っている: {tab_line:?}");
        // ガター(▎)→ マーカー(→) → コードの順で、インデントが入っている。
        assert!(
            tab_line.starts_with("▎ →"),
            "ガター+マーカーの並びが違う: {tab_line:?}"
        );
        // render は1回ぶんのコードブロック(マーカー行は1行だけ)。
        let marker_lines = texts.iter().filter(|t| t.contains('→')).count();
        assert_eq!(marker_lines, 1, "マーカー行数が想定外: {marker_lines}");
    }

    #[test]
    fn table_cell_link_renders_label_with_hidden_target() {
        // 表セル内の [label](url) はラベルのみ(青下線)で描画し、URL は HIDDEN の隠しスパンで携える。
        // 生の "[label](url)" テキストを表に出さない(ユーザー報告のバグ)。
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
        // ラベル span はリンク様式(青下線・HIDDEN 無し)。
        let label = row
            .spans
            .iter()
            .find(|sp| sp.content.as_ref() == "Docs")
            .unwrap();
        assert_eq!(label.style.fg, Some(Color::Blue));
        assert!(label.style.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!label.style.add_modifier.contains(Modifier::HIDDEN));
        // 直後に URL の隠しターゲット span。
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
        // 桁揃えは「隠しスパンを除いた表示幅」で成立する(collapse_links が描画前に除去する前提)。
        // 全行(罫線含む)が同じ表示幅になること。リンク行だけ広い/狭いは崩れ。
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
        // 狭い幅ではセルが折返されるが、リンクのラベル span は分割されない(ターゲット対応の維持)。
        let md = "| doc |\n|---|\n| intro text [Guide](./guide.md) tail |\n";
        let lines = render_markdown(md, 18, BG, "TwoDark", false);
        // ラベル "Guide" がどこかの物理行に**1つの span**として存在し、直後が隠し URL。
        let mut found = false;
        for l in &lines {
            if let Some(i) = l.spans.iter().position(|sp| sp.content.as_ref() == "Guide") {
                assert!(is_hidden_link_target(&l.spans[i + 1]));
                found = true;
            }
        }
        assert!(found, "折返し後もリンクラベルが1スパンで残る");
        // 隠しスパン除去後の幅は全行一致。
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
        // 基本形・前後テキスト・画像(!付き)は素通し・未対応括弧は素通し。
        let segs = parse_cell_segments("see [a](b.md) end");
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[1], CellSeg::Link { label, url } if label == "a" && url == "b.md"));
        let img = parse_cell_segments("![alt](x.png)");
        assert!(matches!(&img[..], [CellSeg::Text { text, .. }] if text == "![alt](x.png)"));
        let broken = parse_cell_segments("[no url] and [y](");
        assert!(broken.iter().all(|s| matches!(s, CellSeg::Text { .. })));
        // title 付き・<> 囲みのリンク先は URL/パスだけに縮める(開けるターゲットにする)。
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
        // `ui.icons` 有効時: 表内リンクにも段落リンクと同じアイコンが付き(見た目の一貫性)、
        // アイコンは**幅計算前に**ラベルへ組み込むので桁揃えは崩れない。
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
        // 隠しスパン除去後(=表示)の幅が全行一致。
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
        // GFM: `\|` はリテラルの `|`(セル区切りではない)。従来は分割されて幽霊列が生えていた。
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
        // :---(左) / :---:(中央) / ---:(右) のパディング配分。
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
        // セル内の **bold** / *italic* / `code` / ~~strike~~ が記号なしのスタイル付きで出る。
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
        // 誤検出防止: 乗算風の * は素通し。
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

    #[test]
    fn html_block_text_survives_and_autolink_untouched() {
        // <details>(open 無し)は既定で折りたたみ: summary は残り本文は隠れる(GitHub 流)。
        // <details open> は展開して本文が出る。コメントは非表示・autolink は誤検知しない。
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
        // icons=false: ASCII ブラケット表示(☐/☑ は CJK フォールバックで全角描画されるため廃止)。
        assert!(
            all.iter().any(|t| t.contains("[ ] open task")),
            "未完 [ ]: {all:?}"
        );
        assert!(all.iter().any(|t| t.contains("[x] done task")), "完了 [x]");
        // マーカーは専用 span(スタイル番兵)として発出される。
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
        // 幅超過のコード行は**事前折返し**され、折返し後の全視覚行に ▎ ガターが付き、
        // どの行も幅を超えない(Paragraph 任せだと2行目以降のガター/帯が途切れる回帰)。
        let long = "abcdefghij".repeat(6); // 60 桁
        let md = format!("```\n{long}\nshort\n```\n");
        let lines = render_markdown(&md, 30, BG, "TwoDark", false);
        let code_rows: Vec<&Line> = lines
            .iter()
            .filter(|l| l.spans.first().is_some_and(|s| s.content.starts_with('▎')))
            .collect();
        // バッジ行 1 + 60桁は 28桁(=30-ガター2)ごとに 3 行 + short 1 行 + 終端パディング 1 行。
        assert_eq!(code_rows.len(), 6, "{:?}", code_rows.len());
        for l in &code_rows {
            let w: usize = l.spans.iter().map(|s| s.content.as_ref().width()).sum();
            assert!(w <= 30, "行幅が枠を超えない: {w}");
        }
        // 中身が失われていない(全行の連結に元テキストが含まれる)。
        let joined: String = code_rows
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref().trim_start_matches("▎ "))
            .collect::<String>()
            .replace(' ', "");
        assert!(joined.contains(&long), "折返しで文字が欠けない");

        // CJK: 全角20文字(40桁)は 28桁境界で分割され、境界で桁を壊さない。
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

        // wrap=false(横スクロール運用)では従来どおり1論理行のまま(事前折返ししない)。
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
        // tui-markdown 0.3.7/0.3.8 は「loose リスト(空行区切り)内のタスク項目」で panic する
        // (insertion index should be <= len)。konoma は捕捉して**空行境界の二分割で再試行**し、
        // 割れた各半分は普通に描ける=装飾(タスクマーカー等)が生き残る。丸ごと素テキスト降格に
        // していた頃は、この記法を1箇所含むだけで文書全編が無装飾になった(実文書で報告)。
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
        // 二分割の再試行で装飾が生きる: タスクは専用マーカー span・bold は記号が剥がれる。
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
        // 再試行の分割点はフェンス外の空行。フェンス内の空行では割らない。
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
        // 1行だけのブロックは割れない。
        assert!(split_block_for_retry("only-one-line").is_none());
    }

    #[test]
    fn task_markers_become_dedicated_spans_with_custom_states() {
        // 標準 (` `/`x`) は専用 span(icons=NF アイコン/false=ブラケット)、カスタム `/` は設定時のみ対象。
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
        // icons=true: 標準状態は Nerd Font アイコン(1セル固定)・カスタムはブラケットのまま。
        let nf_off = format!("{} ", crate::ui::icons::task_icon(false));
        let nf_on = format!("{} ", crate::ui::icons::task_icon(true));
        let iconed = render_markdown_tasks(md, 40, BG, "TwoDark", true, &[' ', '/', 'x']);
        assert_eq!(
            markers(&iconed),
            vec![nf_off.clone(), nf_on.clone(), "[/] ".into()]
        );
        // 状態の復元(トグルの照合に使う)。
        assert_eq!(
            task_span_state(&nf_off),
            Some(' '),
            "末尾スペース込みで復元"
        );
        assert_eq!(task_span_state(&nf_on), Some('x'));
        assert_eq!(task_span_state("[ ]"), Some(' '));
        assert_eq!(task_span_state("[/]"), Some('/'));
        assert_eq!(task_span_state("[ab]"), None, "2文字はマーカーでない");
        // 文中の [ ] は不変(既存保証の再確認)。
        let mid = render_markdown("text with [ ] brackets\n", 40, BG, "TwoDark", false);
        assert!(mid
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| !is_task_span(s)));
    }

    #[test]
    fn task_source_locs_skip_fences_html_and_tables() {
        // 描画パイプラインが decorate_extras に流さない領域(フェンス/HTML/表)を同じ規則でスキップし、
        // 実タスクの行番号・状態文字・バイト位置を正しく返す(トグル書込みの座標になる)。
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
        let locs = task_source_locs(src, &[' ', '/', 'x']);
        let got: Vec<(usize, char)> = locs.iter().map(|l| (l.line, l.state)).collect();
        assert_eq!(
            got,
            vec![(0, ' '), (12, 'X'), (13, '/')],
            "実タスクのみ: {got:?}"
        );
        // state_off は行内の状態文字を正確に指す(CJK/インデント混在でも)。
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
    fn cjk_table_is_rectangular_and_aligned() {
        // tui-markdown は表を1行に潰す(#1)。横取りした自前レンダラは全角幅を測って
        // 桁揃えする。全角ヘッダ + ASCII データが混在しても全行が同一表示幅=矩形になること。
        let md = "| 種別 | ライブラリ | 依存 |\n|------|------------|------|\n\
                  | md   | tui-markdown | ratatui-core |\n| 図   | mermaid-text | unicode-width |\n";
        let lines = render_markdown(md, 80, BG, "TwoDark", false);
        // 1行潰れ(tui-markdown 既定)でなく、罫線込みで複数行になっていること。
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
        // 罫線(箱の角)を含む。
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.as_ref()))
            .collect();
        assert!(joined.contains('┌') && joined.contains('┼') && joined.contains('┘'));
    }

    #[test]
    fn wide_table_wraps_within_terminal_width() {
        // 長いセルは端末幅にキャップして折り返す(罫線が画面外へ溢れない)。
        let md = "| 名前 | 説明 |\n|---|---|\n\
                  | konoma | 全画面プレビュー特化のターミナルファイルブラウザです長い説明 |\n";
        let lines = render_markdown(md, 30, BG, "TwoDark", false);
        for (i, l) in lines.iter().enumerate() {
            assert!(line_disp_width(l) <= 30, "{i}行目が幅30を超過");
        }
        // 同一表示幅で矩形を保つこと。
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
        // mermaid 区間にフェンス行は含めない。
        assert!(matches!(&segs[1], Segment::Mermaid(s) if !s.contains("```")));
    }

    #[test]
    fn normal_code_fence_is_kept_in_markdown() {
        // ```rust ブロックは横取りせず md に残す (tui-markdown がハイライトする)。
        let src = "text\n\n```rust\nlet x = 1;\n```\n";
        let segs = split_segments(src);
        assert_eq!(segs.len(), 1, "got {segs:?}");
        assert!(matches!(&segs[0], Segment::Md(s) if s.contains("let x = 1;")));
    }

    #[test]
    fn mermaid_inside_normal_fence_is_not_intercepted() {
        // 通常フェンス内の ```mermaid 風行は (既にフェンス内なので) 図にしない。
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
        // パースできないソースは raw 表示 (先頭に注記、本文を保持) になる。
        let lines = render_mermaid_file("this is definitely not mermaid syntax", 80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn cjk_sequence_diagram_renders_not_fallback() {
        // upstream mermaid-text 0.56 は CJK 参加者/メッセージで内部 panic していた
        // (strip_keyword_prefix のバイト境界無視スライス)。vendor/mermaid-text の
        // is_char_boundary ガードで解消済み → 罫線図として「実際に描画」される。
        // これが回帰ガード: patch が外れると panic→raw fallback で罫線が消え、本テストが落ちる。
        let src = "sequenceDiagram\n  U->>K: ツリーで .mmd を選ぶ\n  K-->>U: 全画面プレビュー";
        let lines = render_mermaid_file(src, 70);
        assert!(!lines.is_empty(), "CJK 入力でも行を返すこと");
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            !joined.contains("cannot render mermaid"),
            "fallback に落ちている (patch 不在の疑い): {joined}"
        );
        // U+2500..U+257F = 罫線描画ブロック。図になっていれば含む。
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2500}'..='\u{257F}').contains(&c)),
            "CJK 図に罫線が無い (panic→fallback の疑い): {joined}"
        );
    }

    #[test]
    fn ascii_sequence_diagram_renders_box_drawing() {
        // ASCII ラベルの sequence 図は実際に罫線図として描画される (fallback でない)。
        let src = "sequenceDiagram\n  participant U as User\n  participant K as konoma\n  U->>K: open\n  K-->>U: preview";
        let lines = render_mermaid_file(src, 70);
        let joined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(
            !joined.contains("cannot render mermaid"),
            "fallback に落ちている: {joined}"
        );
        // U+2500..U+257F は罫線描画ブロック。図になっていれば必ず含む。
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
        // 先頭 `#` が消え、見出しテキストだけになる。
        assert_eq!(lines[0].to_string(), "Title");
        // 直下に全幅ルール (━) が入る。
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
        // コードブロック由来の行は背景色 (DEFAULT_CODE_BG) を持ち、左ガターで始まる。
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
        // フェンス行 ``` はそのまま出さず、言語ヘッダ(rust)に置換されている。
        assert!(lines.iter().all(|l| !l.to_string().contains("```")));
        assert!(lines.iter().any(|l| l.to_string().contains("rust")));
    }

    #[test]
    fn code_block_content_is_syntax_highlighted_and_indented() {
        // tui-markdown の highlight-code を無効化し、md フェンスコードも自前 syntect で着色する。
        let lines = render_markdown(
            "```rust\nfn f() {\n    let x = 1;\n}\n```\n",
            40,
            BG,
            "TwoDark",
            false,
        );
        // ハイライト: キーワード等に Rgb 前景色が付く。
        let colored = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
        assert!(colored, "md コードがハイライトされていない");
        // インデント保持: ガターの後に元の 4 スペースが残る。
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
        // ヘッダのガター span は is_code_header_span で識別でき(Tab フォーカスの目印)、
        // 本文行のガター(番兵でない)は識別されない。
        let lines = render_markdown("```rust\nlet x = 1;\n```\n", 28, BG, "TwoDark", false);
        let header = lines
            .iter()
            .find(|l| l.to_string().contains("rust"))
            .expect("言語ヘッダが無い");
        assert!(
            header.spans.iter().any(is_code_header_span),
            "ヘッダに番兵ガターが無い"
        );
        // 本文行(コード行)のガターは番兵ではない。
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
        let blocks = code_block_source_locs(src);
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
        // 言語名は行末(右寄せ)に置かれる。
        assert!(
            header.to_string().trim_end().ends_with("rust"),
            "右寄せでない: {:?}",
            header.to_string()
        );
        // バッジ span は本文背景より明るい背景を持つ(区別可能)。
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
        // 右寄せ(既定): 言語名は行末。
        let right = render_markdown("```rust\nx\n```\n", 28, BG, "TwoDark", false);
        let rh = right
            .iter()
            .find(|l| l.to_string().contains("rust"))
            .unwrap();
        let rs = rh.to_string();
        assert!(rs.trim_end().ends_with("rust"), "右寄せでない: {rs:?}");
        let right_pos = rs.find("rust").unwrap();
        // 左寄せ: ガター直後(行頭側)に言語名。
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
        // バッジ背景を任意色に指定できる。
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
        // code_bg=None: バッジは背景なし(淡色で区別)。
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
        // 設定色 (緑) が inline code とコードブロックの両方に反映される。
        let green = Color::Rgb(10, 80, 20);
        let md = "本文 `inline` 続き\n\n```rust\nlet x = 1;\n```\n";
        let style = CodeStyle {
            bg: Some(green),
            ..BG
        };
        let lines = render_markdown(md, 40, style, "TwoDark", false);
        // inline code span が設定色の背景。
        let inline_bg = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "inline")
            .and_then(|s| s.style.bg);
        assert_eq!(inline_bg, Some(green), "inline code に設定色が乗っていない");
        // コードブロック行も設定色。
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
        // code_bg=None: inline code もコードブロックも背景なし (左ガターは残る)。
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
        // 背景を消してもコードと分かるよう左ガターは維持。
        assert!(coded.to_string().starts_with("▎"), "左ガターは残すべき");
    }

    #[test]
    fn cjk_in_markdown_with_mermaid_fence_does_not_panic() {
        // md 内 mermaid フェンス + CJK でもアプリ経路 (render_markdown) が panic しないこと。
        let src =
            "# 図\n\n```mermaid\nsequenceDiagram\n  甲->>乙: こんにちは\n  乙-->>甲: どうも\n```\n";
        let lines = render_markdown(src, 70, BG, "TwoDark", false);
        assert!(!lines.is_empty());
    }

    #[test]
    fn konoma_stylesheet_arms_return_expected_styles() {
        // 使用中の tui-markdown は blockquote/metadata の StyleSheet メソッドを出力 span へ
        // 反映しない(描画経路では到達しない)ため、StyleSheet 実装を直接検証して全 arm を網羅する。
        let s = KonomaStyles {
            code_bg: Some(Color::Rgb(1, 2, 3)),
        };
        // blockquote = 緑 + イタリック。
        let bq = s.blockquote();
        assert_eq!(bq.fg, Some(Color::Green));
        assert!(bq.add_modifier.contains(Modifier::ITALIC));
        // metadata block = LightYellow。
        assert_eq!(s.metadata_block().fg, Some(Color::LightYellow));
        // heading_meta = DIM。
        assert!(s.heading_meta().add_modifier.contains(Modifier::DIM));
        // 見出しレベル別(1/2=太字, 3=イタリック, それ以外=DIM+イタリック)。
        assert_eq!(s.heading(1).fg, Some(HEAD_FG));
        assert!(s.heading(1).add_modifier.contains(Modifier::BOLD));
        assert!(s.heading(3).add_modifier.contains(Modifier::ITALIC));
        assert!(s.heading(6).add_modifier.contains(Modifier::DIM));
        // インラインコード: 設定の背景色を反映 / None なら背景なし。
        assert_eq!(s.code().bg, Some(Color::Rgb(1, 2, 3)));
        let no_bg = KonomaStyles { code_bg: None };
        assert_eq!(no_bg.code().bg, None);
        // リンクは下線。
        assert!(s.link().add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn render_markdown_with_mermaid_fence_renders_box_drawing() {
        // md 内の ```mermaid フェンスは横取りされ render_mermaid_safe 経由で罫線図になる。
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
        // 同一フェンスの再ハイライトはキャッシュヒットになり、出力(グリフ・スタイル)が完全一致する。
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
        // 容量は有界: 上限より多くの異なるキーを流し込んでもエントリ数は CAP を超えない。
        for i in 0..(CODE_BLOCK_CACHE_CAP + 20) {
            let one = vec![format!("let v{i} = {i};")];
            let _ = highlight_body(&one, "rust", 60, None, "TwoDark", 4, true);
        }
        assert!(code_block_cache_len() <= CODE_BLOCK_CACHE_CAP);
    }

    // ---- mermaid 画像化 (v0.15 feature) ----------------------------------

    /// SVG 互換の回帰テスト: mermaid-rs-renderer の SVG 出力が konoma 側の resvg/usvg で
    /// そのままラスタライズできること(レンダラと usvg のバージョン乖離を封じる)。CJK 込み。
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

    /// レイアウト計測は全テーマで modern に固定される(dark のフォント計測差で
    /// エッジが迂回リングを描く配置バグの回帰防止・2026-07-17 実測)。
    #[test]
    fn mermaid_dark_theme_uses_modern_font_metrics() {
        let svg = mermaid_to_svg("graph LR\nA-->B", "dark").unwrap();
        assert!(svg.contains("Inter"), "modern のフォントファミリで計測");
        assert!(!svg.contains("trebuchet"), "dark 固有フォントは使わない");
    }

    #[test]
    fn mermaid_to_svg_fails_safely_on_garbage() {
        // 未対応/壊れた入力は None(呼び出し側がテキスト図へ降格)。パニックしない。
        assert!(mermaid_to_svg("definitely not a diagram !!!", "dark").is_none());
    }

    #[test]
    fn catch_silent_returns_none_on_panic_and_some_on_success() {
        // ワーカーの安全網: panic は None(呼び出し側が失敗結果を送って inflight を解く)、
        // 正常値は Some でそのまま返る。抑制フラグはどちらの経路でも残留しない。
        assert_eq!(catch_silent(|| 42), Some(42));
        let paniced: Option<i32> = catch_silent(|| panic!("worker blew up"));
        assert_eq!(paniced, None);
        PANIC_SILENCED.with(|c| assert!(!c.get(), "panic 経路でも抑制フラグが残らない"));
        // 続けて通常の panic メッセージが出せる状態か(フックが黙らせ固定になっていない)。
        assert_eq!(catch_silent(|| "ok"), Some("ok"));
    }

    #[test]
    fn concurrent_mermaid_renders_keep_panic_hook_sane() {
        // 回帰 2026-07-18: 旧実装は呼び出し毎に take_hook/set_hook を swap しており、フェンス
        // ワーカーの並行レンダで復元順序が交錯すると「無音フック」が恒久残留し得た。
        // 現実装は Once+thread-local: 並行実行後もこのスレッドの抑制フラグは倒れている。
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
        // 閉じられていないフェンスは安全にテキストへ戻る(欠落しない)。
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
        // キャプション行(フォーカス番兵)が予約行の直前にある。
        let cap = &lines[imgs[0].line - 1];
        assert!(
            cap.spans.iter().any(is_mermaid_header_span),
            "キャプション行に番兵 span"
        );
        // Loading: placement 無し・ローディング行あり。
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
        // Text: probe も Text = 抽出そのものが OFF → 従来のテキスト図(罫線)経路。
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

    /// キャプションは呼び出し側から渡された翻訳済み文字列を使うが、`◇ mermaid` プレフィックスは
    /// 不変で番兵(is_mermaid_header_span)として認識され続ける(i18n 化してもフォーカス巡回が壊れない)。
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
        // どちらの言語でも番兵として認識される(プレフィックスが不変)。
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
