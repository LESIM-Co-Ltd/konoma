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
pub fn render_markdown(src: &str, width: u16, code: CodeStyle, theme: &str) -> Vec<Line<'static>> {
    let opts = Options::new(KonomaStyles { code_bg: code.bg });
    let mut out = Vec::new();
    for seg in split_segments(src) {
        match seg {
            Segment::Md(text) => {
                if text.trim().is_empty() {
                    continue;
                }
                // tui-markdown は GFM 表を1行に潰すため、表ブロックだけ先に横取りして
                // 自前で罫線描画する。残りのテキストは従来どおり tui-markdown へ。
                for part in split_tables(&text) {
                    match part {
                        MdPart::Text(t) => {
                            if t.trim().is_empty() {
                                continue;
                            }
                            // from_str_with_options は借用した Text<'_> を返すので即 'static へ複製。
                            let rendered = tui_markdown::from_str_with_options(&t, &opts);
                            let lines = into_static_lines(rendered);
                            // 見出し強調・コードブロックの特殊エリア化を後処理で付与。
                            out.extend(decorate_md_lines(lines, width, code, theme));
                        }
                        MdPart::Table(raw) => out.extend(render_table(&raw, width)),
                    }
                }
            }
            Segment::Mermaid(code) => out.extend(render_mermaid_block(&code, width)),
        }
    }
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
) -> Vec<Line<'static>> {
    let lines = decorate_code_blocks(lines, width, code, theme);
    decorate_headings(lines, width)
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
        ));
        out.push(pad_to_width(vec![gutter_span(code_bg)], w, code_bg));
    }
    out
}

/// Syntect-highlight the collected code body using the `lang` token and `theme`, then add a left gutter, background, and
/// full-width padding to each line. If the body is empty (a fence with no content), no lines are added.
fn highlight_body(
    body: &[String],
    lang: &str,
    w: usize,
    code_bg: Option<Color>,
    theme: &str,
    tab_width: usize,
) -> Vec<Line<'static>> {
    if body.is_empty() {
        return Vec::new();
    }
    let src = body.join("\n");
    // タブ展開は **ガター/全幅パディングを付ける前** に行う(後だと幅計算が狂って帯が崩れる)。
    // 桁追跡はコード先頭(0桁)基準。ガター付与で全行が一律右シフトするので整列は保たれる。
    let hl = crate::preview::code::expand_tabs(
        crate::preview::code::highlight_lang(&src, lang, theme),
        tab_width,
    );
    hl.into_iter()
        .map(|line| {
            let mut spans = vec![gutter_span(code_bg)];
            for s in line.spans {
                let st = match code_bg {
                    Some(bg) => s.style.bg(bg),
                    None => s.style,
                };
                spans.push(Span::styled(s.content, st));
            }
            pad_to_width(spans, w, code_bg)
        })
        .collect()
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
    let gutter = gutter_span(code_bg);
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
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mermaid_text::render_with_width(code, max_width)
    }));
    std::panic::set_hook(prev);
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
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Wrap a string into chunks no wider than display width `w` (full-width = 2 columns; CJK has no spaces, so split naively by width).
fn wrap_by_width(s: &str, w: usize) -> Vec<String> {
    if w == 0 || UnicodeWidthStr::width(s) <= w {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        if cur_w + cw > w && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Render a table block with box-drawing lines. `width` is the column count of the display area (inside the frame).
fn render_table(raw: &str, width: u16) -> Vec<Line<'static>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut header_rows = 0usize; // 区切り行より前(=ヘッダ)の行数
    for line in raw.lines() {
        if is_table_delimiter(line) {
            header_rows = rows.len();
            continue;
        }
        rows.push(parse_table_row(line));
    }
    let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if rows.is_empty() || ncol == 0 {
        return Vec::new();
    }
    for r in &mut rows {
        r.resize(ncol, String::new());
    }
    // 自然列幅(全角考慮)。最低1。
    let mut col_w = vec![1usize; ncol];
    for r in &rows {
        for (c, cell) in r.iter().enumerate() {
            col_w[c] = col_w[c].max(UnicodeWidthStr::width(cell.as_str()));
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
        let wrapped: Vec<Vec<String>> = r
            .iter()
            .enumerate()
            .map(|(c, cell)| wrap_by_width(cell, col_w[c]))
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
                let cell = wrapped[c].get(p).map(|s| s.as_str()).unwrap_or("");
                let pad = col_w[c].saturating_sub(UnicodeWidthStr::width(cell));
                let content = format!(" {cell}{} ", " ".repeat(pad));
                spans.push(Span::styled(content, cell_style));
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
    };
    /// No background (equivalent to code_bg="none").
    const NO_CODE: CodeStyle = CodeStyle {
        bg: None,
        label_bg: None,
        label_right: true,
        tab_width: 4,
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
        let lines = render_markdown(md, 40, BG, "TwoDark");
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
    fn cjk_table_is_rectangular_and_aligned() {
        // tui-markdown は表を1行に潰す(#1)。横取りした自前レンダラは全角幅を測って
        // 桁揃えする。全角ヘッダ + ASCII データが混在しても全行が同一表示幅=矩形になること。
        let md = "| 種別 | ライブラリ | 依存 |\n|------|------------|------|\n\
                  | md   | tui-markdown | ratatui-core |\n| 図   | mermaid-text | unicode-width |\n";
        let lines = render_markdown(md, 80, BG, "TwoDark");
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
        let lines = render_markdown(md, 30, BG, "TwoDark");
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
        let lines = render_markdown("# Hello\n\nworld\n", 80, BG, "TwoDark");
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
        let lines = render_markdown("# Title\n\nbody\n", 20, BG, "TwoDark");
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
        let lines = render_markdown("text\n\n```rust\nlet x = 1;\n```\n", 30, BG, "TwoDark");
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
    fn code_header_language_is_a_right_aligned_badge() {
        let lines = render_markdown("```rust\nlet x = 1;\n```\n", 28, BG, "TwoDark");
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
        let right = render_markdown("```rust\nx\n```\n", 28, BG, "TwoDark");
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
        let lines = render_markdown("```rust\nx\n```\n", 28, style, "TwoDark");
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
        let lines = render_markdown("```rust\nx\n```\n", 28, NO_CODE, "TwoDark");
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
        let lines = render_markdown(md, 40, style, "TwoDark");
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
        let lines = render_markdown(md, 40, NO_CODE, "TwoDark");
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
        let lines = render_markdown(src, 70, BG, "TwoDark");
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
        let lines = render_markdown(md, 70, BG, "TwoDark");
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
}
