// 内蔵 コードレンダラ (syntect によるシンタックスハイライト)。
//
// 拡張子/言語トークンからシンタックスを判定し、行ごとに syntect でハイライトして ratatui の
// Line 列にする。重い SyntaxSet/Theme は OnceLock で 1 度だけロードする。
// シンタックスは two-face(bat 由来)拡張セット、テーマは TwoDark(= Zed の One Dark 配色)。
// 正規表現は純 Rust の fancy-regex(default-fancy)＝ oniguruma(C) 不要で配布容易性を保つ。
// md 内のフェンスコードは tui-markdown 側が担当する(別経路)。色は前景のみ採用し、背景は端末/
// アプリ配色に委ねる(アイコンと同じ「テーマ追従」方針)。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};
use unicode_width::UnicodeWidthStr;

struct Assets {
    syntaxes: SyntaxSet,
    themes: EmbeddedLazyThemeSet,
}

/// The SyntaxSet/theme set is expensive to load (decompressing a binary dump), so build it only once per process.
/// Both come from `two-face` (derived from bat). The syntaxes also cover TypeScript/TOML/Dockerfile and more.
/// The themes include all 32 `EmbeddedThemeName` variants (Dracula/Nord/Gruvbox/Catppuccin/TwoDark, etc.).
fn assets() -> &'static Assets {
    static A: OnceLock<Assets> = OnceLock::new();
    A.get_or_init(|| Assets {
        syntaxes: two_face::syntax::extra_newlines(),
        themes: two_face::theme::extra(),
    })
}

/// Resolve a configured theme name to `two-face`'s `EmbeddedThemeName`. Separators (space/-/_) and case are ignored.
/// Aliases: "one-dark"/"onedark"/"default"/empty → TwoDark (= Zed One Dark). Unknown names also map to TwoDark.
fn parse_theme(name: &str) -> EmbeddedThemeName {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_' | '.' | '(' | ')'))
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let want = norm(name);
    if matches!(want.as_str(), "" | "default" | "onedark" | "one") {
        return EmbeddedThemeName::TwoDark;
    }
    EmbeddedLazyThemeSet::theme_names()
        .iter()
        .copied()
        .find(|t| norm(t.as_name()) == want)
        .unwrap_or(EmbeddedThemeName::TwoDark)
}

/// Already-warmed extensions (for idempotency). The same extension is never compiled twice.
fn warmed_set() -> &'static Mutex<HashSet<String>> {
    static WARMED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WARMED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether the syntax for extension `ext` is **compiled (warmed)**. Used at preview start to decide whether this is
/// a heavy first time that needs a loading indicator, or warm and instant. `highlight`/`warm_file` mark it on completion.
pub fn is_ext_warm(ext: &str) -> bool {
    warmed_set()
        .lock()
        .map(|s| s.contains(ext))
        .unwrap_or(false)
}

/// Record an extension as "warmed" (call after compilation finishes).
fn mark_warm(ext: &str) {
    if !ext.is_empty() {
        if let Ok(mut s) = warmed_set().lock() {
            s.insert(ext.to_string());
        }
    }
}

/// At startup, scan the current tree `root` and warm code-file extensions in the background, **most-frequent first**.
/// Each extension highlights the head of a representative file to compile the syntax's regexes and
/// cache them in the static SyntaxSet (resolving the first-preview freeze = once per language;
/// a few seconds in debug, ~0.2s in release). The more a language is used, the sooner it is warmed. It runs **single-threaded
/// with a `yield_now` per language** so as not to hurt the responsiveness of the UI/other work. Only if a not-yet-warmed
/// language is opened first does that first time wait synchronously as usual (it does not crash).
pub fn warm_dir(root: PathBuf) {
    let _ = assets();
    for (ext, path) in warm_order(&root) {
        warm_file(&ext, &path);
        std::thread::yield_now(); // 他処理に CPU を譲る(体感悪化を避ける)
    }
}

/// Scan `root` and return **(extension, representative file) ordered by file count, most first** (= warm order).
/// Ties are stabilized by extension name. The ordering ensures more-used languages are warmed first.
fn warm_order(root: &Path) -> Vec<(String, PathBuf)> {
    let mut count: HashMap<String, usize> = HashMap::new();
    let mut sample: HashMap<String, PathBuf> = HashMap::new();
    let mut budget = 20_000usize; // 走査ファイル数の上限(巨大ツリー対策)
    collect_exts(root, &mut count, &mut sample, &mut budget);
    let mut exts: Vec<(String, usize)> = count.into_iter().collect();
    exts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    exts.into_iter()
        .filter_map(|(ext, _)| sample.get(&ext).map(|p| (ext, p.clone())))
        .collect()
}

/// Recursively walk under `dir`, collecting the file count and a representative file per extension. Hidden (`.`) entries are excluded,
/// symlinked dirs are not followed (avoiding loops), and the walk stops after `budget` entries (to handle huge trees).
fn collect_exts(
    dir: &Path,
    count: &mut HashMap<String, usize>,
    sample: &mut HashMap<String, PathBuf>,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in rd.flatten() {
        if *budget == 0 {
            return;
        }
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue; // 隠し除外
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_dir() {
            subdirs.push(path); // symlink dir は is_dir() が false なので辿らない
        } else if ft.is_file() {
            *budget -= 1;
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                *count.entry(ext.to_string()).or_insert(0) += 1;
                sample.entry(ext.to_string()).or_insert(path);
            }
        }
    }
    for sub in subdirs {
        collect_exts(&sub, count, sample, budget);
    }
}

/// Warm the syntax for extension `ext` by highlighting the head (up to 64KB) of the representative file `path`.
/// Extensions with no syntax, or already warmed, are skipped. Color is irrelevant, so TwoDark is fixed to trigger compilation only.
/// Call `mark_warm` **only after compilation finishes** (so `is_ext_warm` correctly reflects "done").
/// Public because progressive mode's background warming also calls it.
pub fn warm_file(ext: &str, path: &Path) {
    if is_ext_warm(ext) {
        return; // 既に温め済み(完了)
    }
    let a = assets();
    let Some(syntax) = a.syntaxes.find_syntax_by_extension(ext) else {
        mark_warm(ext); // 対応文法なし(プレーン扱い)＝温め不要だが「済み」にして再試行を防ぐ
        return;
    };
    let Ok(text) = read_head(path, 64 * 1024) else {
        return; // 読めない(消えた等)。次回再試行を許す
    };
    let mut hl = HighlightLines::new(syntax, a.themes.get(EmbeddedThemeName::TwoDark));
    for line in LinesWithEndings::from(&text) {
        let _ = hl.highlight_line(line, &a.syntaxes); // 失敗は無視(温めが目的)
    }
    mark_warm(ext); // コンパイル完了をマーク
}

/// Read at most `max_bytes` from the head of the file (keeps warming of huge files light). Non-UTF-8 is read lossily.
fn read_head(path: &Path, max_bytes: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; max_bytes];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Highlight a standalone code file by its extension into decorated lines (`PreviewKind::Code`).
/// `theme` is the configured theme name (unknown/empty → TwoDark). Even if it cannot be detected, it falls back to plain text without crashing.
pub fn highlight(src: &str, path: &Path, theme: &str) -> Vec<Line<'static>> {
    let assets = assets();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let syntax = assets
        .syntaxes
        .find_syntax_by_extension(ext)
        .or_else(|| {
            assets
                .syntaxes
                .find_syntax_by_first_line(src.lines().next().unwrap_or(""))
        })
        .unwrap_or_else(|| assets.syntaxes.find_syntax_plain_text());
    let out = highlight_with(src, syntax, theme);
    // この呼び出しで ext の文法はコンパイル済み(syntect の static キャッシュに載った)。
    // is_ext_warm が「次回以降は即時」を正しく返すよう記録(同期ハイライト=indicator 経路も含む)。
    mark_warm(ext);
    out
}

/// Highlight one line of text by its extension into a list of ratatui Spans (for the diff preview).
/// Each span carries only a foreground color; the caller (gitdiff) overpaints the background according to the diff line kind.
/// When there is no syntax / on failure, it falls back to a single plain-text span (does not crash).
/// Note: because each line is highlighted independently, state spanning multiple lines (strings/comments) is not carried over
/// (acceptable since each line is a fragment in a diff view).
pub fn highlight_line_by_ext(line: &str, ext: &str, theme: &str) -> Vec<Span<'static>> {
    let assets = assets();
    let Some(syntax) = assets.syntaxes.find_syntax_by_extension(ext) else {
        return vec![Span::raw(line.to_string())];
    };
    let theme = assets.themes.get(parse_theme(theme));
    let mut hl = HighlightLines::new(syntax, theme);
    // syntect は行末改行を前提に状態遷移するので 1 行＋\n を渡す。
    let owned = format!("{}\n", trim_eol(line));
    let Ok(ranges) = hl.highlight_line(&owned, &assets.syntaxes) else {
        return vec![Span::raw(line.to_string())];
    };
    let spans: Vec<Span<'static>> = ranges
        .into_iter()
        .map(|(st, text)| Span::styled(trim_eol(text).to_string(), to_style(st)))
        .filter(|s| !s.content.is_empty())
        .collect();
    if spans.is_empty() {
        vec![Span::raw(String::new())]
    } else {
        spans
    }
}

/// Highlight by a language token (e.g. "rust"/"py"/"bash"). For Markdown's ```lang fences.
/// If the token is unknown/empty, it becomes plain text (plain text syntax). `theme` is the configured theme name.
pub fn highlight_lang(src: &str, lang: &str, theme: &str) -> Vec<Line<'static>> {
    let assets = assets();
    let syntax = assets
        .syntaxes
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| assets.syntaxes.find_syntax_plain_text());
    highlight_with(src, syntax, theme)
}

/// Highlight the body line by line using the resolved syntax and theme name, producing a list of ratatui Lines (the shared core).
fn highlight_with(src: &str, syntax: &SyntaxReference, theme: &str) -> Vec<Line<'static>> {
    let assets = assets();
    let theme = assets.themes.get(parse_theme(theme));
    let mut hl = HighlightLines::new(syntax, theme);

    let mut out = Vec::new();
    for line in LinesWithEndings::from(src) {
        // 1 行のハイライト失敗はその行だけ素テキストにし、全体を巻き込まない。
        let Ok(ranges) = hl.highlight_line(line, &assets.syntaxes) else {
            out.push(Line::from(trim_eol(line).to_string()));
            continue;
        };
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .map(|(st, text)| Span::styled(trim_eol(text).to_string(), to_style(st)))
            .filter(|s| !s.content.is_empty())
            .collect();
        out.push(Line::from(spans));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

/// Strip trailing line breaks (\n/\r). Since we work per Line, the newline character is not needed.
fn trim_eol(s: &str) -> &str {
    s.trim_end_matches(['\n', '\r'])
}

/// The marker character for tab visualization (placed in the first cell of the tab stop). The rest is filled with spaces.
const TAB_MARKER: char = '→';

/// Expand tab characters in a line into "a marker (→) plus spaces up to the next tab stop". Terminals do not column-align
/// tabs, so this ① aligns indentation columns and ② makes tabs visible. When `tab_width`=0, no conversion.
/// Track the display column (full-width = 2) from the start of the line, extending each tab to the next multiple of `tab_width`. The marker is dimmed.
pub fn expand_tabs(lines: Vec<Line<'static>>, tab_width: usize) -> Vec<Line<'static>> {
    // タブが1つも無ければ無駄なクローンを避けて素通し。
    if tab_width == 0
        || !lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('\t')))
    {
        return lines;
    }
    let marker_style = Style::new().fg(Color::DarkGray);
    lines
        .into_iter()
        .map(|line| {
            let line_style = line.style;
            let mut col = 0usize; // 表示桁(全角=2)
            let mut out: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
            for span in line.spans {
                if !span.content.contains('\t') {
                    col += UnicodeWidthStr::width(span.content.as_ref());
                    out.push(span);
                    continue;
                }
                let style = span.style;
                let mut buf = String::new();
                for ch in span.content.chars() {
                    if ch == '\t' {
                        if !buf.is_empty() {
                            col += UnicodeWidthStr::width(buf.as_str());
                            out.push(Span::styled(std::mem::take(&mut buf), style));
                        }
                        let stop = tab_width - (col % tab_width); // 次タブストップまで 1..=tab_width
                        let mut marker = String::with_capacity(stop);
                        marker.push(TAB_MARKER);
                        for _ in 1..stop {
                            marker.push(' ');
                        }
                        out.push(Span::styled(marker, marker_style));
                        col += stop;
                    } else {
                        buf.push(ch);
                    }
                }
                if !buf.is_empty() {
                    col += UnicodeWidthStr::width(buf.as_str());
                    out.push(Span::styled(buf, style));
                }
            }
            Line::from(out).style(line_style)
        })
        .collect()
}

/// Convert a syntect Style into a ratatui Style. Adopt only the foreground color, leaving the background to the terminal/app
/// (no theme-specific bg, blending in with the terminal palette). Bold/italic/underline are applied.
fn to_style(st: SynStyle) -> Style {
    let fg = st.foreground;
    let mut style = Style::new().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if st.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if st.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if st.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn highlights_rust_into_multiple_colored_spans() {
        let src = "fn main() {\n    let x = 42;\n}\n";
        let lines = highlight(src, &PathBuf::from("a.rs"), "TwoDark");
        // 3 行(fn / let / })になる。
        assert_eq!(lines.len(), 3, "行数が一致しない: {}", lines.len());
        // キーワード等で色分けされ、1 行に複数 span ができる(=ハイライトされている)。
        let multi = lines.iter().any(|l| l.spans.len() > 1);
        assert!(multi, "ハイライトされていない(全行単一 span)");
        // 前景色 (Rgb) が付いている span がある。
        let colored = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
        assert!(colored, "前景色が付いていない");
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        // 未知拡張子でもクラッシュせず、内容の行が保たれる。
        let src = "hello world\nsecond line\n";
        let lines = highlight(src, &PathBuf::from("note.unknownext"), "TwoDark");
        assert_eq!(lines.len(), 2);
        let joined: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("hello world") && joined.contains("second line"));
    }

    #[test]
    fn empty_source_yields_one_line_no_panic() {
        let lines = highlight("", &PathBuf::from("a.rs"), "TwoDark");
        assert_eq!(lines.len(), 1);
    }

    /// The number of distinct foreground Rgb colors appearing in the decorated lines. The plain text syntax uses only the theme's single default color,
    /// so 2 or more colors = evidence that it is actually colored by syntactic differences.
    fn distinct_colors(lines: &[Line<'static>]) -> usize {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| match s.style.fg {
                Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    /// Extract the foreground Rgb color of the first span containing `target`.
    fn color_of(lines: &[Line<'static>], target: &str) -> Option<(u8, u8, u8)> {
        lines.iter().flat_map(|l| l.spans.iter()).find_map(|s| {
            if s.content.contains(target) {
                match s.style.fg {
                    Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
                    _ => None,
                }
            } else {
                None
            }
        })
    }

    #[test]
    fn theme_matches_zed_one_dark_palette() {
        // テーマ = TwoDark(Atom/Zed One Dark)。代表色が One Dark パレットと一致すること。
        let lines = highlight(
            "// c\nfn add() {\n    let s = \"hi\";\n    let n = 42;\n}\n",
            &PathBuf::from("a.rs"),
            "TwoDark",
        );
        assert_eq!(
            color_of(&lines, "fn"),
            Some((0xc6, 0x78, 0xdd)),
            "keyword 紫"
        );
        assert_eq!(
            color_of(&lines, "hi"),
            Some((0x98, 0xc3, 0x79)),
            "string 緑"
        );
        assert_eq!(
            color_of(&lines, "42"),
            Some((0xd1, 0x9a, 0x66)),
            "number 橙"
        );
        assert_eq!(
            color_of(&lines, " c"),
            Some((0x5c, 0x63, 0x70)),
            "comment 灰"
        );
    }

    #[test]
    fn typescript_and_toml_highlight_via_two_face() {
        // syntect 同梱には無い TypeScript/TOML も two-face の拡張セットで複数色に着色される。
        let ts = highlight(
            "interface T { id: number; }\nconst x: T = { id: 1 }; // c\n",
            &PathBuf::from("a.ts"),
            "TwoDark",
        );
        assert!(distinct_colors(&ts) >= 2, "TS が着色されない(単一色)");
        let toml = highlight(
            "# c\ntitle = \"x\"\n[server]\nport = 8080\n",
            &PathBuf::from("Cargo.toml"),
            "TwoDark",
        );
        assert!(distinct_colors(&toml) >= 2, "TOML が着色されない(単一色)");
    }

    #[test]
    fn warm_order_ranks_extensions_by_file_count() {
        use std::io::Write;
        let dir = std::env::temp_dir().join("konoma_warm_order_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let touch = |p: PathBuf| {
            let mut f = std::fs::File::create(p).unwrap();
            f.write_all(b"x\n").unwrap();
        };
        // rs=3(うち1つは sub/), py=2, md=1, 隠しは除外。
        touch(dir.join("a.rs"));
        touch(dir.join("b.rs"));
        touch(dir.join("sub").join("c.rs"));
        touch(dir.join("d.py"));
        touch(dir.join("e.py"));
        touch(dir.join("f.md"));
        touch(dir.join(".hidden.rs")); // 隠し → 数えない
        let order = warm_order(&dir);
        let exts: Vec<&str> = order.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(exts, vec!["rs", "py", "md"], "ファイル数の多い順: {exts:?}");
        // 代表ファイルは実在する。
        for (_, p) in &order {
            assert!(p.exists(), "代表ファイルが無い: {p:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore] // 文法コンパイルで数秒かかるため通常は除外(手動: --ignored で計測)
    fn warm_dir_makes_subsequent_highlight_fast() {
        use std::time::Instant;
        let root = std::path::Path::new("samples/code");
        if !root.exists() {
            return;
        }
        warm_dir(root.to_path_buf());
        // ウォーム後は app.ts の初回ハイライトが速い(<200ms 目安)。
        let ts = std::fs::read_to_string("samples/code/app.ts").unwrap();
        let t = Instant::now();
        let _ = highlight(&ts, &PathBuf::from("app.ts"), "TwoDark");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("app.ts after warm_dir: {ms:.0} ms");
        assert!(ms < 200.0, "ウォーム後も遅い: {ms:.0} ms");
    }

    /// Display string of a Line (all spans concatenated).
    fn line_str(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn expand_tabs_aligns_to_stops_and_marks() {
        // 行頭タブ2つ + 中身。tab_width=4 → 各タブは "→   "(4桁)に展開され桁が揃う。
        let src = "\t\tconst x = 1;\n";
        let lines = expand_tabs(highlight(src, &PathBuf::from("a.ts"), "TwoDark"), 4);
        let s = line_str(&lines[0]);
        assert!(s.starts_with("→   →   const"), "タブ展開がおかしい: {s:?}");
        // 行頭の2タブで8桁ぶんインデントが入る(マーカー1 + 空白3)×2。
        assert_eq!(UnicodeWidthStr::width(s.as_str()), s.chars().count());
        // マーカー → を含む span は淡色(DarkGray)。
        let marker = lines[0]
            .spans
            .iter()
            .find(|sp| sp.content.contains('→'))
            .expect("マーカー span が無い");
        assert_eq!(marker.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn expand_tabs_mid_line_aligns_to_next_stop() {
        // 2文字 + タブ → 次のタブストップ(4桁目)まで=2桁("→ ")。
        let line = Line::from("ab\tc".to_string());
        let out = expand_tabs(vec![line], 4);
        assert_eq!(line_str(&out[0]), "ab→ c");
    }

    #[test]
    fn expand_tabs_zero_width_is_noop() {
        let line = Line::from("\tx".to_string());
        let out = expand_tabs(vec![line], 0);
        assert_eq!(line_str(&out[0]), "\tx", "0 は無変換");
    }

    #[test]
    fn parse_theme_resolves_names_and_aliases() {
        // 既定/別名 → TwoDark。名前(区切り/大小無視) → 対応テーマ。不明 → TwoDark。
        assert_eq!(parse_theme(""), EmbeddedThemeName::TwoDark);
        assert_eq!(parse_theme("one-dark"), EmbeddedThemeName::TwoDark);
        assert_eq!(parse_theme("TwoDark"), EmbeddedThemeName::TwoDark);
        assert_eq!(parse_theme("dracula"), EmbeddedThemeName::Dracula);
        assert_eq!(parse_theme("Nord"), EmbeddedThemeName::Nord);
        assert_eq!(parse_theme("gruvbox-dark"), EmbeddedThemeName::GruvboxDark);
        assert_eq!(
            parse_theme("catppuccin mocha"),
            EmbeddedThemeName::CatppuccinMocha
        );
        assert_eq!(parse_theme("no-such-theme"), EmbeddedThemeName::TwoDark);
    }

    #[test]
    fn highlight_lang_tokenizes_known_and_plain() {
        // 既知トークン(rust): 行は保持され、少なくとも1行が複数 span に着色される。
        let lines = highlight_lang("let x = 1;\nlet y = 2;\n", "rust", "TwoDark");
        assert_eq!(lines.len(), 2, "行数は入力どおり");
        assert_eq!(line_str(&lines[0]), "let x = 1;");
        assert!(
            lines.iter().any(|l| l.spans.len() > 1),
            "rust は複数 span に着色されるはず"
        );
        // 未知トークンはプレーン: 行は保持され内容も一致(落ちない)。
        let plain = highlight_lang("hello\nworld\n", "no-such-lang", "TwoDark");
        assert_eq!(plain.len(), 2);
        assert_eq!(line_str(&plain[0]), "hello");
        assert_eq!(line_str(&plain[1]), "world");
    }

    #[test]
    fn read_head_caps_bytes_and_errors_on_missing() {
        let dir = std::env::temp_dir().join("konoma_read_head_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("data.txt");
        std::fs::write(&f, "0123456789ABCDEF".as_bytes()).unwrap();
        // 先頭 max_bytes のみ読む。
        let head = read_head(&f, 5).unwrap();
        assert_eq!(head, "01234", "先頭5バイトだけ");
        // ファイルより大きい上限でも全文(切り詰め)。
        let all = read_head(&f, 1000).unwrap();
        assert_eq!(all, "0123456789ABCDEF");
        // 存在しないファイルは Err。
        assert!(read_head(&dir.join("nope.txt"), 10).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warm_file_marks_ext_warm_for_known_and_unknown() {
        let dir = std::env::temp_dir().join("konoma_warm_file_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 既知文法(rust): warm 後は is_ext_warm が true。
        let rs = dir.join("sample.rs");
        std::fs::write(&rs, b"fn main() { let x = 1; }\n").unwrap();
        warm_file("rs", &rs);
        assert!(is_ext_warm("rs"), "warm 後は rs が温まり済み");
        // 対応文法の無い拡張子も「済み」にして再試行を防ぐ。
        let weird = dir.join("x.konoma_zzqq");
        std::fs::write(&weird, b"x").unwrap();
        warm_file("konoma_zzqq", &weird);
        assert!(is_ext_warm("konoma_zzqq"), "文法なしでも warm 済み扱い");
        std::fs::remove_dir_all(&dir).ok();
    }
}
