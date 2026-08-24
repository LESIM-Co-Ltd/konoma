// Built-in code renderer (syntax highlighting via syntect).
//
// Determines syntax from the extension/language token, highlights it line by line with syntect,
// and turns it into ratatui Line values. The heavy SyntaxSet/Theme is loaded only once via OnceLock.
// Syntax comes from the two-face (bat-derived) extended set; the theme is TwoDark (= Zed's One Dark palette).
// Regex is pure-Rust fancy-regex (default-fancy) — no oniguruma (C) needed, keeping distribution easy.
// Fenced code inside md is handled by konoma's own markdown renderer path (separate). Only the foreground color is
// adopted; the background is left to the terminal/app palette (the same "follow the theme" policy as icons).

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
        std::thread::yield_now(); // yield CPU to other work (avoid a perceptible slowdown)
    }
}

/// Scan `root` and return **(extension, representative file) ordered by file count, most first** (= warm order).
/// Ties are stabilized by extension name. The ordering ensures more-used languages are warmed first.
fn warm_order(root: &Path) -> Vec<(String, PathBuf)> {
    let mut count: HashMap<String, usize> = HashMap::new();
    let mut sample: HashMap<String, PathBuf> = HashMap::new();
    let mut budget = 20_000usize; // cap on the number of scanned files (guards against huge trees)
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
            continue; // exclude hidden
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_dir() {
            subdirs.push(path); // a symlinked dir has is_dir() == false, so it is never followed
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
        return; // already warmed (done)
    }
    let a = assets();
    let Some(syntax) = a.syntaxes.find_syntax_by_extension(ext) else {
        mark_warm(ext); // no matching syntax (treated as plain) = no warming needed, but mark it "done" to prevent retries
        return;
    };
    let Ok(text) = read_head(path, 64 * 1024) else {
        return; // unreadable (deleted, etc). Allow retrying next time
    };
    let mut hl = HighlightLines::new(syntax, a.themes.get(EmbeddedThemeName::TwoDark));
    // `warm_loop`'s `false` means a panic was caught partway (principle #3): this runs on a
    // background thread (main.rs's warm job) whose caller sends its "done" signal right after this
    // call returns — an unguarded panic would kill that thread before the send, latching
    // `hl_pending` forever (a spinner that never stops). Stop warming this file right here on a
    // panic rather than keep feeding `hl` more lines (its state afterward is unspecified) or marking
    // `ext` warm (compilation didn't provably finish — a later real `highlight()` call still
    // succeeds or degrades line-by-line on its own, and calls `mark_warm` itself either way, so
    // nothing is left permanently broken). A plain `Err` from a single line is unaffected: tolerated
    // exactly as before, since the return value was always thrown away — the only goal here is to
    // trigger the grammar's one-time regex compilation.
    if warm_loop(&text, |line| {
        call_highlight_line(&mut hl, &a.syntaxes, line).map(|_| ())
    }) {
        mark_warm(ext); // mark compilation as complete
    }
}

/// The warm-up loop: call `try_line` for every line of `text`, stopping (and reporting `false`,
/// i.e. "don't mark this extension warm") on the first line where it returns `None` — a caught
/// panic, in production. Extracted so the "stop early without crashing or marking done" control
/// flow can be exercised directly with an injected panic (below), rather than needing to make
/// syntect itself panic on demand from a test. Production always passes `call_highlight_line(...)
/// .map(|_| ())`, which collapses "syntect returned `Ok`" and "syntect returned a tolerated `Err`"
/// into the same `Some(())` (this loop doesn't care which — only a caught panic stops it).
fn warm_loop(text: &str, mut try_line: impl FnMut(&str) -> Option<()>) -> bool {
    for line in LinesWithEndings::from(text) {
        if try_line(line).is_none() {
            return false;
        }
    }
    true
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

/// Map a file whose extension/name two-face has no dedicated syntax for onto a close relative, the way `bat`
/// maps such files. Only families common in CLI work are aliased: ignore files (`.dockerignore`/`.npmignore`/…)
/// → Git Ignore, and JSON with comments/extensions (`.jsonc`/`.json5`) → JSON. Returns a token to look up.
fn alias_syntax_token(name: &str, ext: &str) -> Option<&'static str> {
    if name.ends_with("ignore") {
        return Some(".gitignore");
    }
    match ext {
        "jsonc" | "json5" => Some("json"),
        _ => None,
    }
}

/// Find a file's syntax from its extension, then its **full file name** (covers dotfiles like `.bashrc` and named
/// files like `Makefile`/`Dockerfile`, whose `Path::extension()` is empty), then a family alias (see
/// `alias_syntax_token`). two-face/Sublime list such names in a syntax's file_extensions (sometimes with the
/// leading dot, sometimes without), so the raw name and the dot-trimmed name are both tried. No first-line/plain.
fn find_named_syntax<'a>(ss: &'a SyntaxSet, path: &Path) -> Option<&'a SyntaxReference> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let name = path.file_name().and_then(|e| e.to_str()).unwrap_or("");
    (!ext.is_empty())
        .then(|| ss.find_syntax_by_extension(ext))
        .flatten()
        .or_else(|| ss.find_syntax_by_extension(name))
        .or_else(|| ss.find_syntax_by_extension(name.trim_start_matches('.')))
        .or_else(|| alias_syntax_token(name, ext).and_then(|t| ss.find_syntax_by_extension(t)))
}

/// Resolve the syntax for a file (extension/name/alias, then first line/shebang), falling back to plain text.
fn resolve_syntax<'a>(ss: &'a SyntaxSet, path: &Path, src: &str) -> &'a SyntaxReference {
    find_named_syntax(ss, path)
        .or_else(|| ss.find_syntax_by_first_line(src.lines().next().unwrap_or("")))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

/// Whether a **real** (non-plain-text) syntax resolves for `path` from its extension, file name, or alias —
/// WITHOUT reading the file (no first-line/shebang check). Lets the windowed preview color extensionless config
/// files (`.bashrc`, `Makefile`, `Dockerfile`, …) while leaving genuinely plain text untouched.
pub fn has_named_syntax(path: &Path) -> bool {
    let ss = &assets().syntaxes;
    match find_named_syntax(ss, path) {
        Some(s) => s.name != ss.find_syntax_plain_text().name,
        None => false,
    }
}

/// Call `hl.highlight_line`, catching a panic (principle #3: syntect's regex-based parser/scope
/// engine has known upstream panics on pathological input — e.g. index-out-of-bounds in the scope
/// stack, stack overflow in fancy-regex — and this is the hottest path in the app: every code/text
/// preview and every diff line goes through it, while `main.rs` installs no top-level
/// `catch_unwind`; an unguarded panic here would take down the whole TUI). Keeps the panic/no-panic
/// distinction visible in the return type instead of collapsing it: outer `None` = a panic was
/// caught, `Some(_)` = syntect ran to completion (its `Ok`/`Err` unpacked inside — every caller
/// already tolerated an `Err` the same way as "nothing to color," so it isn't worth a named error
/// type here too). That distinction matters to `warm_file`, which must keep tolerating a benign
/// `Err` exactly as before while treating only an actual panic as a reason to stop warming.
fn call_highlight_line<'b>(
    hl: &mut HighlightLines<'_>,
    ss: &SyntaxSet,
    line: &'b str,
) -> Option<Option<Vec<(SynStyle, &'b str)>>> {
    crate::preview::markdown::catch_silent(|| hl.highlight_line(line, ss).ok())
}

/// Highlight one line into owned, trimmed, non-empty spans — the "syntect ranges → ratatui spans"
/// conversion shared by `highlight_with` (whole-file highlighting) and `highlight_line_by_ext` (one
/// diff line), which used to duplicate it. Wraps `call_highlight_line`'s panic safety net (principle
/// #3). `None` means "degrade this line to plain text" — callers don't need to know whether that was
/// a syntect `Err` or a caught panic, since both are the same "can't color this, but the text itself
/// is fine" outcome. Folding the conversion into one place also means the panic guard can't be
/// forgotten if a third caller of syntect is ever added here.
fn spans_for_line(
    hl: &mut HighlightLines<'_>,
    ss: &SyntaxSet,
    line: &str,
) -> Option<Vec<Span<'static>>> {
    let ranges = call_highlight_line(hl, ss, line).flatten()?;
    Some(
        ranges
            .into_iter()
            .map(|(st, text)| Span::styled(trim_eol(text).to_string(), to_style(st)))
            .filter(|s| !s.content.is_empty())
            .collect(),
    )
}

/// Highlight a standalone code/text file into decorated lines. Resolves the syntax by extension, file name, or
/// first line (see `resolve_syntax`), so dotfiles and named config files (`.bashrc`, `Makefile`, `Dockerfile`, …)
/// are colored too. `theme` is the configured theme name (unknown/empty → TwoDark). Undetectable content falls
/// back to plain text without crashing.
pub fn highlight(src: &str, path: &Path, theme: &str) -> Vec<Line<'static>> {
    let assets = assets();
    let syntax = resolve_syntax(&assets.syntaxes, path, src);
    let out = highlight_with(src, syntax, theme);
    // This call has compiled ext's syntax (it landed in syntect's static cache).
    // Record it so is_ext_warm correctly reports "instant from now on" (including the synchronous-highlight/indicator path).
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
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
    // syntect assumes a trailing newline for its state transitions, so pass one line plus \n.
    let owned = format!("{}\n", trim_eol(line));
    // `spans_for_line` guards this the same way as `highlight_with` (principle #3). This is called
    // once per diff line (`gitdiff::diff_line_to_line`/`diff_half_to_line`), so a panic here — or a
    // plain syntect `Err` — must degrade only this one row, not the whole diff view.
    line_spans_with(line, || spans_for_line(&mut hl, &assets.syntaxes, &owned))
}

/// Turn one line's guarded highlight attempt into its final spans: the successful/non-empty case
/// passes through unchanged; a failure (`spans_for()` returning `None` — syntect `Err` or a caught
/// panic) degrades to a single unstyled span carrying the **original** `line` text, and a
/// successful-but-empty result (e.g. a blank line) degrades to a single empty span. Extracted out
/// of `highlight_line_by_ext` so the degrade path can be exercised directly with an injected panic
/// (below) instead of needing to make syntect itself panic on demand from a test.
fn line_spans_with(
    line: &str,
    spans_for: impl FnOnce() -> Option<Vec<Span<'static>>>,
) -> Vec<Span<'static>> {
    let Some(spans) = spans_for() else {
        return vec![Span::raw(line.to_string())];
    };
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
    // A highlight failure — syntect's own `Err`, or (rarer, but real: syntect's regex-based parser
    // has known upstream panics on pathological input) a panic caught inside `spans_for_line` — only
    // degrades *that* line to plain text, without dragging down the whole file (principle #3).
    // Guarding **per line** here (rather than once around the whole loop in `highlight_lines_with`)
    // preserves that same per-line contract for panics that already existed for a plain `Err`, and
    // costs nothing extra on the non-panic path: `catch_unwind` doesn't allocate, and this function
    // isn't on a per-keystroke path (results are cached by the caller — e.g. the `highlight_body`
    // LRU — so it runs once per file load / cache miss, not once per frame). Confirmed by
    // `highlight_lang_large_source_is_bounded`/`warm_then_highlight_is_fast` staying well within
    // their generous bounds after adding this per-line guard.
    highlight_lines_with(src, |line| spans_for_line(&mut hl, &assets.syntaxes, line))
}

/// The line loop shared by every whole-document highlight (`highlight`/`highlight_lang`, via
/// `highlight_with`): call `spans_for` for each line, pushing its spans, or — on `None` — a single
/// plain-text `Line` carrying that line's own (trimmed) text, so the returned `Vec<Line>` always has
/// exactly one entry per source line regardless of how many lines failed to highlight. Extracted so
/// this count-preserving degrade behavior can be exercised directly with an injected panic (below),
/// rather than needing to make syntect itself panic on demand from a test.
fn highlight_lines_with(
    src: &str,
    mut spans_for: impl FnMut(&str) -> Option<Vec<Span<'static>>>,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in LinesWithEndings::from(src) {
        let Some(spans) = spans_for(line) else {
            out.push(Line::from(trim_eol(line).to_string()));
            continue;
        };
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
///
/// `tab_width` is clamped defensively (`config::clamp_tab_width`) before it is used for anything:
/// below, a tab at column 0 allocates `String::with_capacity(tab_width)` and loops `tab_width`
/// times to fill it, so an unclamped huge value straight from `[ui] tab_width` (a config typo, or
/// the reported `9223372036854775807`) can exhaust memory and abort the process (`handle_alloc_error`,
/// not a catchable panic). The clamp is enforced here, at the sole point of allocation, rather than
/// on the caller — this is the same file's `svg_max_px` pattern (`preview::svg::HARD_MAX_PX`
/// bounds the raster size at its own point of use), so callers are free to pass the raw config
/// value straight through.
pub fn expand_tabs(lines: Vec<Line<'static>>, tab_width: usize) -> Vec<Line<'static>> {
    let tab_width = crate::config::clamp_tab_width(tab_width);
    // If there isn't a single tab, pass through unchanged to avoid a wasted clone.
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
            let mut col = 0usize; // display column (full-width = 2)
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
                        let stop = tab_width - (col % tab_width); // 1..=tab_width to the next tab stop
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
    use crate::test_support::unique_tmp;
    use std::path::PathBuf;

    #[test]
    fn highlights_rust_into_multiple_colored_spans() {
        let src = "fn main() {\n    let x = 42;\n}\n";
        let lines = highlight(src, &PathBuf::from("a.rs"), "TwoDark");
        // Becomes 3 lines (fn / let / }).
        assert_eq!(lines.len(), 3, "行数が一致しない: {}", lines.len());
        // Colored by keywords etc., so a line ends up with multiple spans (= it's highlighted).
        let multi = lines.iter().any(|l| l.spans.len() > 1);
        assert!(multi, "ハイライトされていない(全行単一 span)");
        // There is a span carrying a foreground color (Rgb).
        let colored = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _))));
        assert!(colored, "前景色が付いていない");
    }

    #[test]
    fn config_dotfiles_and_named_files_resolve_a_real_syntax() {
        // Extensionless config files/named files can also resolve a syntax from the file name (distinct from plain text).
        // The last 4 (.dockerignore/.npmignore/.jsonc/.json5) aren't supported by two-face → resolved via an alias.
        for name in [
            ".bashrc",
            ".zshrc",
            ".gitconfig",
            "Makefile",
            "Dockerfile",
            ".dockerignore",
            ".npmignore",
            "tsconfig.jsonc",
            "app.json5",
        ] {
            assert!(
                has_named_syntax(&PathBuf::from(name)),
                "{name} が文法解決できていない(素テキスト扱い)"
            );
        }
        // Genuinely plain text stays has_named_syntax=false (no theme fg applied either).
        for name in ["notes.txt", "README", "output.dat"] {
            assert!(
                !has_named_syntax(&PathBuf::from(name)),
                "{name} は素テキスト扱いのはず"
            );
        }
        // .bashrc is actually highlighted, producing spans in multiple colors.
        let src = "# comment\nexport PATH=\"$HOME/bin:$PATH\"\nalias ll='ls -la'\n";
        let lines = highlight(src, &PathBuf::from(".bashrc"), "TwoDark");
        let distinct: std::collections::HashSet<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| match s.style.fg {
                Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
                _ => None,
            })
            .collect();
        assert!(
            distinct.len() >= 2,
            ".bashrc が単色(ハイライトされていない): {} 色",
            distinct.len()
        );
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        // Even an unknown extension doesn't crash, and the content lines are preserved.
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
        // Theme = TwoDark (Atom/Zed One Dark). Representative colors must match the One Dark palette.
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
        // TypeScript/TOML, which syntect's bundled set doesn't have, are also colored in multiple colors via two-face's extended set.
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
        let dir = unique_tmp("konoma_warm_order_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let touch = |p: PathBuf| {
            let mut f = std::fs::File::create(p).unwrap();
            f.write_all(b"x\n").unwrap();
        };
        // rs=3 (one of which is under sub/), py=2, md=1, hidden excluded.
        touch(dir.join("a.rs"));
        touch(dir.join("b.rs"));
        touch(dir.join("sub").join("c.rs"));
        touch(dir.join("d.py"));
        touch(dir.join("e.py"));
        touch(dir.join("f.md"));
        touch(dir.join(".hidden.rs")); // hidden → not counted
        let order = warm_order(&dir);
        let exts: Vec<&str> = order.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(exts, vec!["rs", "py", "md"], "ファイル数の多い順: {exts:?}");
        // The representative files actually exist.
        for (_, p) in &order {
            assert!(p.exists(), "代表ファイルが無い: {p:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // GUARDS: after `warm_dir` scans a directory and pre-compiles its languages' grammars, a
    // subsequent `highlight()` for one of those languages is fast (the warm-up actually did its job —
    // this exercises the directory-scanning entry point specifically, distinct from
    // `warm_then_highlight_is_fast`'s direct `warm_file` call). Converted from a wall-clock bound
    // (which — being tied to a one-time, process-global regex-compilation cost — is exactly the kind
    // of thing that becomes free noise-margin guesswork once the previous run's grammar is already
    // cached, and flakes if it isn't) to a **deterministic allocation ceiling**: a cold first-time
    // highlight compiles many regex patterns across the language's grammar contexts (an
    // allocation-heavy one-time cost — see `assets()`'s doc comment on why this is expensive), so an
    // unwarmed highlight allocates orders of magnitude more than a warm one.
    //
    // Uses "lua" rather than "rust"/"ts"/"py" (used throughout this test suite's own fixtures)
    // specifically because it is **not** used as a file extension anywhere else in this codebase's
    // tests: if the test used an already-commonly-warmed language, it would pass even if `warm_dir`
    // itself did nothing (some *other* test would have warmed it first) — no longer testing what it
    // claims to.
    #[test]
    fn warm_dir_makes_subsequent_highlight_fast() {
        let dir = crate::test_support::unique_tmp("konoma_code_warmdir");
        std::fs::create_dir_all(&dir).unwrap();
        let src = "-- a lua comment\nlocal function greet(name)\n  print('hello, ' .. name)\nend\n";
        let f = dir.join("a.lua");
        std::fs::write(&f, src).unwrap();
        assert!(
            has_named_syntax(&f),
            "lua の文法が解決できない(テスト前提が崩れている)"
        );

        warm_dir(dir.clone());
        let alloc = crate::mem_tests::allocated_by(|| {
            let lines = highlight(src, &f, "TwoDark");
            assert!(!lines.is_empty());
        });
        assert!(
            alloc < 2_000_000, // measured ~82KB warm vs ~26.5MB cold (an unwarmed "nim" highlight) — ~24x/~13x headroom
            "warm_dir 後の初回ハイライトの確保バイト数が多すぎる(warm_dir が効いていない?): {alloc}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Display string of a Line (all spans concatenated).
    fn line_str(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn expand_tabs_aligns_to_stops_and_marks() {
        // Two leading tabs + content. tab_width=4 → each tab expands to "→   " (4 columns), aligning the columns.
        let src = "\t\tconst x = 1;\n";
        let lines = expand_tabs(highlight(src, &PathBuf::from("a.ts"), "TwoDark"), 4);
        let s = line_str(&lines[0]);
        assert!(s.starts_with("→   →   const"), "タブ展開がおかしい: {s:?}");
        // The 2 leading tabs give 8 columns of indent ((1 marker + 3 spaces) × 2).
        assert_eq!(UnicodeWidthStr::width(s.as_str()), s.chars().count());
        // The span containing the → marker is a pale color (DarkGray).
        let marker = lines[0]
            .spans
            .iter()
            .find(|sp| sp.content.contains('→'))
            .expect("マーカー span が無い");
        assert_eq!(marker.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn expand_tabs_mid_line_aligns_to_next_stop() {
        // 2 characters + a tab → up to the next tab stop (column 4) = 2 columns ("→ ").
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

    /// Regression test for the unbounded `[ui] tab_width` bug: `expand_tabs` allocates
    /// `String::with_capacity(stop)` and loops `stop` times for the marker of the first tab on a
    /// line, where `stop == tab_width` when the tab is at column 0. Before the fix this was taken
    /// straight from config with no cap, so a value like the reported `9223372036854775807` (or a
    /// plain typo like `1000000000`) turns into a multi-gigabyte-or-more allocation attempt that
    /// fails and calls `handle_alloc_error` — a hard process abort, not a catchable panic — and
    /// even values that *do* allocate block the UI thread for `tab_width` iterations. We can't
    /// safely exercise the actually-dangerous magnitudes here (that would abort/hang the test
    /// process itself, exactly what must not happen), so this uses `1_000` — completely safe to
    /// allocate/loop over, but outside the realistic 1..=16 tab-stop range the same as any larger
    /// value, so it is equivalent evidence: the clamp treats "1_000" and "9223372036854775807"
    /// identically (both fall back to the default), so this proves the real call site
    /// (`expand_tabs`, not just an isolated accessor) is protected.
    #[test]
    fn expand_tabs_clamps_unreasonably_large_tab_width() {
        let line = Line::from("\tx".to_string());
        let out = expand_tabs(vec![line], 1_000);
        assert_eq!(
            line_str(&out[0]),
            "→   x",
            "現実的範囲(1..=16)外の tab_width は既定(4)にクランプされるべき"
        );
    }

    #[test]
    fn parse_theme_resolves_names_and_aliases() {
        // Default/alias → TwoDark. Name (separators/case ignored) → the matching theme. Unknown → TwoDark.
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
        // Known token (rust): the lines are preserved, and at least one is colored into multiple spans.
        let lines = highlight_lang("let x = 1;\nlet y = 2;\n", "rust", "TwoDark");
        assert_eq!(lines.len(), 2, "行数は入力どおり");
        assert_eq!(line_str(&lines[0]), "let x = 1;");
        assert!(
            lines.iter().any(|l| l.spans.len() > 1),
            "rust は複数 span に着色されるはず"
        );
        // An unknown token stays plain: the lines are preserved and the content still matches (no crash).
        let plain = highlight_lang("hello\nworld\n", "no-such-lang", "TwoDark");
        assert_eq!(plain.len(), 2);
        assert_eq!(line_str(&plain[0]), "hello");
        assert_eq!(line_str(&plain[1]), "world");
    }

    #[test]
    fn read_head_caps_bytes_and_errors_on_missing() {
        let dir = unique_tmp("konoma_read_head_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("data.txt");
        std::fs::write(&f, "0123456789ABCDEF".as_bytes()).unwrap();
        // Read only the first max_bytes.
        let head = read_head(&f, 5).unwrap();
        assert_eq!(head, "01234", "先頭5バイトだけ");
        // Even with a cap larger than the file, get the full content (truncated to that cap).
        let all = read_head(&f, 1000).unwrap();
        assert_eq!(all, "0123456789ABCDEF");
        // A nonexistent file returns Err.
        assert!(read_head(&dir.join("nope.txt"), 10).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn warm_file_marks_ext_warm_for_known_and_unknown() {
        let dir = unique_tmp("konoma_warm_file_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Known syntax (rust): after warming, is_ext_warm is true.
        let rs = dir.join("sample.rs");
        std::fs::write(&rs, b"fn main() { let x = 1; }\n").unwrap();
        warm_file("rs", &rs);
        assert!(is_ext_warm("rs"), "warm 後は rs が温まり済み");
        // Extensions with no matching syntax are also marked "done" to prevent retries.
        let weird = dir.join("x.konoma_zzqq");
        std::fs::write(&weird, b"x").unwrap();
        warm_file("konoma_zzqq", &weird);
        assert!(is_ext_warm("konoma_zzqq"), "文法なしでも warm 済み扱い");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- D1: syntect panic safety net -------------------------------------------
    //
    // syntect can't be made to panic on demand from a test, so these exercise the
    // exact production degrade paths (`highlight_lines_with`/`line_spans_with`/
    // `warm_loop`) by injecting a real panic through `catch_silent` (the same
    // primitive `call_highlight_line` uses) in place of the real syntect call. Each
    // pairs with a "the guard is load-bearing" check: temporarily undoing the
    // corresponding production guard (see the session's regression-detection step)
    // makes exactly these tests fail.

    #[test]
    fn highlight_lines_with_degrades_a_panic_to_plain_text_preserving_line_count() {
        // Every line's highlight attempt panics (simulating a pathological-input syntect panic on
        // every call). The loop must still return exactly one Line per source line, uncolored.
        let src = "alpha\nbeta\ngamma\n";
        let lines = highlight_lines_with(src, |_line| {
            crate::preview::markdown::catch_silent(|| -> Vec<Span<'static>> {
                panic!("simulated syntect panic")
            })
        });
        assert_eq!(
            lines.len(),
            3,
            "パニックしても行数はソースと一致するはず(欠落しない): {}",
            lines.len()
        );
        let texts: Vec<String> = lines.iter().map(line_str).collect();
        assert_eq!(
            texts,
            vec!["alpha", "beta", "gamma"],
            "各行のテキストは保たれる"
        );
        for l in &lines {
            assert!(
                l.spans.iter().all(|s| s.style.fg.is_none()),
                "パニックした行は無装飾のはず: {l:?}"
            );
        }
    }

    #[test]
    fn highlight_lines_with_degrades_only_the_panicking_line() {
        // Only the second of three lines panics; the other two keep their real (colored) spans —
        // "one bad line degrades only that line," the same contract syntect's own Err already had.
        let src = "one\ntwo\nthree\n";
        let colored =
            |text: &str| vec![Span::styled(text.to_string(), Style::new().fg(Color::Red))];
        let mut call = 0;
        let lines = highlight_lines_with(src, |line| {
            call += 1;
            if call == 2 {
                crate::preview::markdown::catch_silent(|| -> Vec<Span<'static>> {
                    panic!("simulated syntect panic on line 2")
                })
            } else {
                Some(colored(trim_eol(line)))
            }
        });
        assert_eq!(lines.len(), 3);
        assert_eq!(line_str(&lines[0]), "one");
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.style.fg == Some(Color::Red)),
            "パニックしていない1行目は着色されたまま"
        );
        assert_eq!(
            line_str(&lines[1]),
            "two",
            "パニックした行もテキストは保たれる"
        );
        assert!(
            lines[1].spans.iter().all(|s| s.style.fg.is_none()),
            "パニックした2行目だけ無装飾に降格するはず"
        );
        assert_eq!(line_str(&lines[2]), "three");
        assert!(
            lines[2]
                .spans
                .iter()
                .any(|s| s.style.fg == Some(Color::Red)),
            "パニックしていない3行目は着色されたまま(後続行に波及しない)"
        );
    }

    #[test]
    fn line_spans_with_degrades_a_panic_to_one_unstyled_span_with_original_text() {
        let spans = line_spans_with("hello world", || {
            crate::preview::markdown::catch_silent(|| -> Vec<Span<'static>> {
                panic!("simulated syntect panic")
            })
        });
        assert_eq!(spans.len(), 1, "1 span に降格するはず: {spans:?}");
        assert_eq!(
            spans[0].content.as_ref(),
            "hello world",
            "元の行テキストを保持するはず(diff の行が消えない)"
        );
        assert!(spans[0].style.fg.is_none(), "無装飾のはず");
    }

    #[test]
    fn line_spans_with_passes_a_successful_result_through_unchanged() {
        // Non-regression: when spans_for() succeeds, line_spans_with must not alter the result.
        let styled = vec![Span::styled(
            "colored".to_string(),
            Style::new().fg(Color::Red),
        )];
        let spans = line_spans_with("unused-fallback-text", || Some(styled.clone()));
        assert_eq!(spans, styled);
    }

    #[test]
    fn warm_loop_stops_early_on_panic_without_completing() {
        let text = "line1\nline2\nline3\n";
        let mut seen = 0;
        let completed = warm_loop(text, |_line| {
            seen += 1;
            if seen == 2 {
                crate::preview::markdown::catch_silent(|| panic!("simulated syntect panic"))
            } else {
                Some(())
            }
        });
        assert!(
            !completed,
            "パニックしたら completed=false(=呼び出し側は mark_warm しない合図)のはず"
        );
        assert_eq!(seen, 2, "パニックした行で止まり、以降の行は処理しないはず");
    }

    #[test]
    fn warm_loop_completes_when_every_line_is_ok_or_a_benign_err() {
        // Non-regression: a plain (non-panicking) per-line failure must NOT stop the loop early —
        // only a caught panic (None) does. This is the "ignore failures" tolerance warm_file always had.
        let text = "a\nb\nc\n";
        let mut seen = 0;
        let completed = warm_loop(text, |_line| {
            seen += 1;
            Some(()) // stands in for both a real Ok and a tolerated Err (call_highlight_line collapses both)
        });
        assert!(completed, "パニックが無ければ最後まで完走するはず");
        assert_eq!(seen, 3, "全行処理されるはず");
    }

    #[test]
    fn warm_file_does_not_mark_warm_when_the_underlying_call_panics() {
        // Regression guard for the real (non-injected) path: replaces `call_highlight_line`'s
        // result at the `warm_loop` call site the same way `warm_file` itself does, but forces the
        // very first line to look like a caught panic. Confirms `warm_file`'s own wiring — "on panic,
        // call `warm_loop`'s closure with a None-producing step and don't call mark_warm" — end to
        // end, without needing syntect to actually panic.
        let text = "does not matter\n";
        let completed = warm_loop(text, |_line| -> Option<()> { None });
        assert!(
            !completed,
            "warm_loop が false を返すことが mark_warm 抑止の唯一の根拠"
        );
    }

    #[test]
    fn highlight_line_by_ext_still_colors_a_real_line_after_the_refactor() {
        // Non-regression: highlight_line_by_ext (rewritten atop line_spans_with/spans_for_line)
        // still highlights real, non-panicking input exactly as before — a keyword-bearing Rust
        // line comes back as multiple spans with at least one carrying a foreground color.
        let spans = highlight_line_by_ext("fn add(x: i32) -> i32 { x + 1 }", "rs", "TwoDark");
        assert!(
            spans.len() > 1,
            "着色されていない(単一 span のまま): {spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _)))),
            "前景色が付いていない"
        );
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            joined, "fn add(x: i32) -> i32 { x + 1 }",
            "テキスト内容は保たれる"
        );
    }
}
