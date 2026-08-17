//! Golden-snapshot safety net for Markdown rendering (`src/preview/markdown.rs`), added ahead of a
//! planned rendering refactor. This module renders every case in the existing parity corpora
//! (`task_corpus` / `code_corpus` / `code_span_corpus` / `preprocess_corpus`) plus the repo's
//! `samples/*.md` files, through **exactly the pipeline `App::build_decorated` +
//! `App::ensure_md_cache`'s post-process use** (see `src/app/md_render.rs`, `src/app/md_text.rs`,
//! and the precedent this mirrors, `app_faithful_parity_tests` in `markdown.rs`), and dumps the
//! *entire* observable result — every rendered line's style, every span's (content, style), every
//! inline-image placement, and everything the source scanners (`code_block_source_locs`,
//! `task_source_locs`, `collect_details_open`) and the app-side Tab/anchor builders
//! (`build_md_items`, `compute_md_anchors`) report — to a deterministic text file checked into
//! `snapshots/`.
//!
//! This is **not** a judgment that the current output is correct. Known-broken shapes (e.g. the
//! list-item-fence glue documented next to `code_corpus`) are captured exactly as broken; the
//! point is that a refactor which changes *anything* observable shows up as a diff here, so it can
//! be reviewed and either accepted (`KONOMA_UPDATE_SNAPSHOTS=1`) or recognized as a real
//! regression.
//!
//! ## Regenerating
//!
//! ```text
//! KONOMA_UPDATE_SNAPSHOTS=1 cargo test --features git markdown_render_snapshot
//! KONOMA_UPDATE_SNAPSHOTS=1 cargo test --no-default-features markdown_render_snapshot
//! ```
//!
//! (Markdown rendering does not depend on the `git` feature, so either invocation regenerates the
//! same two files; running both is only redundant, never wrong.) Review the resulting diff under
//! `snapshots/` like any other code change before committing it.

use std::fmt::Write as _;
use std::path::PathBuf;

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};

// Brings in `App`'s private-to-`app`-and-descendants items this dump drives directly
// (`Config`, `MdItem`/`MdItemKind`, `Line`/`Span`, and the `md_text` free functions
// `collapse_links`/`autolink_bare_urls`/`substitute_emoji`/`build_md_items`/`compute_md_anchors`
// glob-imported into `app`'s own scope by `app.rs`'s `use md_text::*;`) — the same pattern every
// sibling submodule (`md_items.rs`, `md_render.rs`, `md_text.rs`, `tests.rs`, ...) uses.
use super::*;

/// Terminal width the whole corpus renders at. Matches the width already used by the precedent
/// this mirrors (`app_faithful_parity_tests::drawn_code`/`drawn_tasks` in `markdown.rs`) — wide
/// enough that the corpus's short lines don't force awkward wraps, narrow enough to keep the dump
/// readable.
///
/// `pub(super)`: `md_render_diff_tests` (a sibling module) renders its own new-renderer side at this
/// exact same width, so the two sides of that comparison can never quietly drift onto different
/// widths.
pub(super) const SNAPSHOT_WIDTH: u16 = 100;

/// `samples/*.md` included alongside the corpora, read from disk (not `include_str!`) so a build
/// from the *published* crate — where `/samples` is excluded (see `Cargo.toml`) — degrades to
/// "skip this file" instead of a hard compile failure. Mirrors `tests.rs::sample_path_or_skip`'s
/// contract exactly (that helper lives in a sibling module and isn't visible here, so this is a
/// small deliberate duplicate rather than a refactor of product test code).
const SAMPLE_FILES: &[&str] = &[
    "images.md",
    "links.md",
    "links.ja.md",
    "markdown.md",
    "markdown.ja.md",
    "README.md",
    "tutorial.md",
    "tutorial.ja.md",
];

fn sample_src(name: &str) -> Option<String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("samples")
        .join(name);
    match std::fs::read_to_string(&p) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!(
                "markdown snapshot: skipping samples/{name} ({e}) \
                 — not present (published-crate build?)"
            );
            None
        }
    }
}

/// Every case this dump covers, `(label, source)`, **sorted** so the file's case order never
/// depends on corpus insertion order or directory-listing order. Labels are corpus-prefixed so a
/// name that happens to repeat across two corpora (or between a corpus and a sample file) doesn't
/// collide in the sort — both entries simply appear, adjacently, under their own prefixed labels.
///
/// `pub(super)`: `md_model_snapshot_tests` (a sibling module) drives its own dump off the exact
/// same corpus/order — see that module's doc comment for why sharing this one function, rather
/// than re-gathering the same corpora a second time, matters.
pub(super) fn all_cases() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    for (name, src) in crate::preview::markdown::task_corpus::cases() {
        v.push((format!("task_corpus: {name}"), src.to_string()));
    }
    for (name, src) in crate::preview::markdown::code_corpus::cases() {
        v.push((format!("code_corpus: {name}"), src.to_string()));
    }
    for case in crate::preview::markdown::code_span_corpus::cases() {
        v.push((format!("code_span_corpus: {}", case.name), case.src));
    }
    for (name, src) in crate::preview::markdown::preprocess_corpus::cases() {
        v.push((format!("preprocess_corpus: {name}"), src.to_string()));
    }
    for (name, src) in crate::preview::markdown::inline_corpus::cases() {
        v.push((format!("inline_corpus: {name}"), src.to_string()));
    }
    for (name, src) in crate::preview::markdown::list_corpus::cases() {
        v.push((format!("list_corpus: {name}"), src.to_string()));
    }
    for name in SAMPLE_FILES {
        if let Some(src) = sample_src(name) {
            v.push((format!("samples: {name}"), src));
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Computes `pre_src` — the text `build_decorated`'s Markdown branch ultimately hands to
/// `render_markdown_with_images` (front matter stripped off, footnotes renumbered, inline HTML
/// converted), in that order and gated by the same `cfg` fields `build_decorated` reads. This is
/// also the text the new block model (`crate::preview::markdown::model`) is designed to walk (see
/// its module doc comment). `pub(super)` so `md_model_snapshot_tests` — a sibling module — can
/// drive `Doc::parse` off *exactly* this, rather than a second, hand-copied preprocessing mirror
/// that could quietly drift from this one (the two golden-snapshot harnesses would then silently
/// stop covering the same text).
pub(super) fn pre_src_for(cfg: &Config, src: &str) -> String {
    let body = if cfg.ui.md_frontmatter {
        crate::preview::markdown::strip_front_matter(src).1
    } else {
        src.to_string()
    };
    let origin = crate::preview::markdown::identity_origin(&body);
    let (s, origin) = if cfg.ui.md_footnotes {
        crate::preview::markdown::process_footnotes_traced(&body, &origin)
    } else {
        (body, origin)
    };
    let (pre_src, _origin) = if cfg.ui.md_inline_html {
        crate::preview::markdown::process_inline_html_traced(&s, &origin)
    } else {
        (s, origin)
    };
    pre_src
}

/// Everything one case renders to, over the app's real Markdown pipeline.
///
/// `pub(super)`: `md_render_diff_tests` (a sibling module) reads `lines` **and** `images` back out of
/// this as the "old renderer" side of its own comparison — see `render_case`'s own doc comment. Both
/// fields carry the identical `pub(super)` visibility for the identical reason: neither is more or
/// less "the production answer" than the other, and a diff harness that only ever looked at one of
/// the two would have no way to notice a placement-only regression (wrong `url`/`line`/`cols`/`rows`)
/// that leaves every rendered `Line` byte-for-byte unchanged — exactly the shape a wrong
/// `ImagePlacement.url` is (the rendered placeholder rows look identical either way; only the
/// synthetic cache key inside `url` differs) — see `run_case`'s own doc comment in
/// `md_render_diff_tests` for where this is actually compared.
pub(super) struct CaseRender {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) images: Vec<crate::preview::markdown::ImagePlacement>,
    items: Vec<MdItem>,
    anchors: Vec<(String, usize)>,
    code_blocks: Vec<String>,
    task_locs: Vec<crate::preview::markdown::TaskLoc>,
    details_states: Vec<bool>,
}

/// Renders `src` exactly the way `App::build_decorated`'s Markdown branch
/// (`src/app/md_render.rs`) does, followed by `App::ensure_md_cache`'s post-process
/// (`src/app/md_text.rs`'s `collapse_links` / `autolink_bare_urls` / `substitute_emoji`) and item
/// builders (`build_md_items` / `compute_md_anchors`) — the same order, the same arguments, driven
/// off the same `cfg` fields `build_decorated` reads.
///
/// The one deliberate departure from `build_decorated`: the image/mermaid/math *slot* closures
/// there depend on a live `Picker` (font metrics) and on-disk file state, neither of which exists
/// in a unit test and neither of which is deterministic across machines. Per the task brief, they
/// are fixed here to the same values `app_faithful_parity_tests` already uses for the same reason:
/// images "unavailable" (as with no image backend), mermaid fences and math expressions always
/// "extracted" (`Image{cols:20,rows:5}` / `Raw`) so the extraction and placement logic itself is
/// still exercised end to end, just not the picker-dependent raster step.
///
/// `pub(super)`: `md_render_diff_tests` (a sibling module) calls this directly for the "old
/// renderer" side of its own comparison, rather than a second, hand-copied assembly of the same
/// pipeline — see that module's own doc comment.
pub(super) fn render_case(cfg: &Config, src: &str) -> CaseRender {
    let icons = cfg.ui.icons;
    let code = crate::preview::markdown::CodeStyle {
        bg: cfg.ui.theme.code_bg(),
        label_bg: cfg.ui.theme.code_label_bg(),
        label_right: cfg.ui.theme.code_label_right(),
        tab_width: cfg.ui.tab_width,
        wrap: cfg.ui.wrap,
    };
    let theme = cfg.ui.theme.code_theme.as_str();
    let tasks = cfg.ui.md_task_state_chars();

    // Front matter: split the leading `---`…`---` off (mirrors `build_decorated`) — only the
    // *rendered* metadata-block lines are needed here; `pre_src_for` (below) independently derives
    // the same stripped body for the rest of the pipeline, so the two can never disagree about it.
    let fm_lines = if cfg.ui.md_frontmatter {
        match crate::preview::markdown::strip_front_matter(src) {
            (Some(fm), _) => crate::preview::markdown::render_front_matter(&fm, SNAPSHOT_WIDTH),
            (None, _) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Footnotes, then inline HTML — same order and gating as `build_decorated`.
    let pre_src = pre_src_for(cfg, src);

    // `<details>` open/closed state: `md_details = "auto"` (both variants here) reduces this to
    // exactly the tags' own `open` attribute — see `App::details_default_open`.
    let details_states = crate::preview::markdown::collect_details_open(&pre_src);
    crate::preview::markdown::set_details_open(details_states.clone());

    let slot_of = |_: &str| crate::preview::markdown::ImageSlot::Unavailable;
    let mermaid_slot = |_: &str| crate::preview::markdown::MermaidSlot::Image { cols: 20, rows: 5 };
    let math_slot = |_: &str, _: bool| crate::preview::markdown::MathSlot::Raw;
    let (mut lines, mut images, _extras) = crate::preview::markdown::render_markdown_with_images(
        &pre_src,
        SNAPSHOT_WIDTH,
        code,
        theme,
        icons,
        &tasks,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        cfg.ui.md_alerts,
        &math_slot,
        true, // math_on: extraction "on" (mirrors the default `math = "image"` + a live picker)
    );

    // Prepend the front-matter metadata block, shifting image placements down past it — verbatim
    // from `build_decorated`.
    if !fm_lines.is_empty() {
        for p in &mut images {
            p.line += fm_lines.len();
        }
        let mut all = fm_lines;
        all.extend(lines);
        lines = all;
    }

    // App-side post-process (`App::postprocess_md`), without needing a live `App`: the same three
    // free functions, gated by the same config fields, in the same order.
    let (lines, targets) = collapse_links(lines, icons);
    let (lines, targets) = if cfg.ui.md_autolink {
        autolink_bare_urls(lines, targets)
    } else {
        (lines, targets)
    };
    let lines = if cfg.ui.md_emoji {
        substitute_emoji(lines)
    } else {
        lines
    };

    let fence_ords: Vec<usize> = images.iter().filter_map(|p| p.fence_ord).collect();
    // The source scanners behind `y c` and the checkbox toggle, pre-`md-block-walk`: same input text
    // (`pre_src`) and same `<details>` states the renderer above used. Computed here (not read off
    // `MdItemKind::CodeBlock::body`/`Task::state_at`) so this harness's own parity tests — which
    // exist specifically to check the renderer's own sentinel count against *this independent* scan
    // — still compare two genuinely separate derivations, not one echoing the other back to itself.
    let code_blocks = crate::preview::markdown::code_block_source_locs(&pre_src, &details_states);
    let task_locs = crate::preview::markdown::task_source_locs(&pre_src, &tasks, &details_states);
    let items = build_md_items(&lines, &targets, &fence_ords, &[], &[]);
    let anchors = compute_md_anchors(&lines);

    CaseRender {
        lines,
        images,
        items,
        anchors,
        code_blocks,
        task_locs,
        details_states,
    }
}

/// `None` -> `"-"`; every other color exhaustively (a new `Color` variant is a compile error here,
/// not a silent gap in the dump). `Rgb`/`Indexed` carry their numbers so a color *value* change —
/// not just which named color — shows up in a diff.
fn fmt_color(c: Option<Color>) -> String {
    match c {
        None => "-".to_string(),
        Some(Color::Reset) => "reset".to_string(),
        Some(Color::Black) => "black".to_string(),
        Some(Color::Red) => "red".to_string(),
        Some(Color::Green) => "green".to_string(),
        Some(Color::Yellow) => "yellow".to_string(),
        Some(Color::Blue) => "blue".to_string(),
        Some(Color::Magenta) => "magenta".to_string(),
        Some(Color::Cyan) => "cyan".to_string(),
        Some(Color::Gray) => "gray".to_string(),
        Some(Color::DarkGray) => "darkgray".to_string(),
        Some(Color::LightRed) => "lightred".to_string(),
        Some(Color::LightGreen) => "lightgreen".to_string(),
        Some(Color::LightYellow) => "lightyellow".to_string(),
        Some(Color::LightBlue) => "lightblue".to_string(),
        Some(Color::LightMagenta) => "lightmagenta".to_string(),
        Some(Color::LightCyan) => "lightcyan".to_string(),
        Some(Color::White) => "white".to_string(),
        Some(Color::Rgb(r, g, b)) => format!("rgb({r},{g},{b})"),
        Some(Color::Indexed(i)) => format!("idx({i})"),
    }
}

/// One short letter per modifier flag actually used anywhere in `markdown.rs`
/// (BOLD/DIM/ITALIC/UNDERLINED/REVERSED/HIDDEN/CROSSED_OUT — confirmed by grep), plus the two
/// ratatui also defines (SLOW_BLINK/RAPID_BLINK) for completeness. Any bit outside that known set
/// — a modifier flag ratatui might add in a future version — still shows up, as a raw hex suffix,
/// rather than being silently dropped from the dump.
fn fmt_modifier(m: Modifier) -> String {
    let known = Modifier::BOLD
        | Modifier::DIM
        | Modifier::ITALIC
        | Modifier::UNDERLINED
        | Modifier::SLOW_BLINK
        | Modifier::RAPID_BLINK
        | Modifier::REVERSED
        | Modifier::HIDDEN
        | Modifier::CROSSED_OUT;
    let mut s = String::new();
    if m.contains(Modifier::BOLD) {
        s.push('B');
    }
    if m.contains(Modifier::DIM) {
        s.push('D');
    }
    if m.contains(Modifier::ITALIC) {
        s.push('I');
    }
    if m.contains(Modifier::UNDERLINED) {
        s.push('U');
    }
    if m.contains(Modifier::SLOW_BLINK) {
        s.push('l');
    }
    if m.contains(Modifier::RAPID_BLINK) {
        s.push('L');
    }
    if m.contains(Modifier::REVERSED) {
        s.push('R');
    }
    if m.contains(Modifier::HIDDEN) {
        s.push('H');
    }
    if m.contains(Modifier::CROSSED_OUT) {
        s.push('X');
    }
    let leftover = m & !known;
    if !leftover.is_empty() {
        write!(s, "+0x{:x}", leftover.bits()).unwrap();
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

fn fmt_style(s: Style) -> String {
    format!(
        "fg={},bg={},mod={}",
        fmt_color(s.fg),
        fmt_color(s.bg),
        fmt_modifier(s.add_modifier)
    )
}

fn fmt_align(a: Option<Alignment>) -> &'static str {
    match a {
        None => "-",
        Some(Alignment::Left) => "left",
        Some(Alignment::Center) => "center",
        Some(Alignment::Right) => "right",
    }
}

/// Exhaustive on purpose (see `fmt_color`): a new `MdItemKind` variant must be taught to this dump
/// rather than silently rendering as nothing.
fn fmt_item_kind(k: &MdItemKind) -> String {
    match k {
        MdItemKind::Link { target } => format!("Link{{target={target:?}}}"),
        // `state_at`/`body` (the model-derived source position/text, threaded through since the
        // `md-block-walk` copy/toggle migration) are deliberately left out of this dump: they are
        // verification data for `y c`/the checkbox toggle, not part of the *rendered* structure this
        // golden snapshot is pinning, and including them would force a snapshot regeneration with no
        // rendering behavior actually changing.
        MdItemKind::Task { state, .. } => format!("Task{{state={state:?}}}"),
        MdItemKind::CodeBlock { .. } => "CodeBlock".to_string(),
        MdItemKind::MermaidFence { ordinal } => format!("MermaidFence{{ordinal={ordinal}}}"),
        MdItemKind::Details { ordinal } => format!("Details{{ordinal={ordinal}}}"),
    }
}

/// Writes one case's full dump (header + every section) to `out`. Every string value goes through
/// `{:?}` (Rust's own, deterministic `Debug` escaping for `str`/`char`) rather than a hand-rolled
/// escaper, so control characters, quotes, and trailing whitespace all show up unambiguously
/// inside the quotes without needing a second, bespoke escaping scheme to keep in sync.
fn dump_case(cfg: &Config, name: &str, src: &str, out: &mut String) {
    let r = render_case(cfg, src);
    writeln!(out, "=== {name} ===").unwrap();
    writeln!(out, "-- LINES ({}) --", r.lines.len()).unwrap();
    for (i, line) in r.lines.iter().enumerate() {
        writeln!(
            out,
            "L{i:04} style={} align={}",
            fmt_style(line.style),
            fmt_align(line.alignment)
        )
        .unwrap();
        for (si, span) in line.spans.iter().enumerate() {
            writeln!(
                out,
                "  s{si:02} {} {:?}",
                fmt_style(span.style),
                span.content.as_ref()
            )
            .unwrap();
        }
    }
    writeln!(out, "-- IMAGES ({}) --", r.images.len()).unwrap();
    for (i, p) in r.images.iter().enumerate() {
        writeln!(
            out,
            "  i{i:02} line={:04} cols={:03} rows={:03} fence_ord={:?} alt={:?} url={:?}",
            p.line, p.cols, p.rows, p.fence_ord, p.alt, p.url
        )
        .unwrap();
    }
    writeln!(out, "-- CODE_BLOCKS ({}) --", r.code_blocks.len()).unwrap();
    for (i, b) in r.code_blocks.iter().enumerate() {
        writeln!(out, "  c{i:02} {b:?}").unwrap();
    }
    writeln!(out, "-- TASKS ({}) --", r.task_locs.len()).unwrap();
    for (i, t) in r.task_locs.iter().enumerate() {
        writeln!(
            out,
            "  t{i:02} line={:04} state_off={:04} state={:?}",
            t.line, t.state_off, t.state
        )
        .unwrap();
    }
    writeln!(out, "-- DETAILS_OPEN ({}) --", r.details_states.len()).unwrap();
    for (i, o) in r.details_states.iter().enumerate() {
        writeln!(out, "  d{i:02} {o}").unwrap();
    }
    writeln!(out, "-- ANCHORS ({}) --", r.anchors.len()).unwrap();
    for (i, (slug, line)) in r.anchors.iter().enumerate() {
        writeln!(out, "  a{i:02} line={line:04} slug={slug:?}").unwrap();
    }
    writeln!(out, "-- ITEMS ({}) --", r.items.len()).unwrap();
    for (i, it) in r.items.iter().enumerate() {
        writeln!(
            out,
            "  m{i:02} line={:04} kind={}",
            it.line,
            fmt_item_kind(&it.kind)
        )
        .unwrap();
    }
    writeln!(out).unwrap();
}

/// The full dump for `cfg`, over every case `all_cases()` reports, in that (sorted) order.
fn dump_variant(cfg: &Config) -> String {
    let mut out = String::new();
    for (name, src) in all_cases() {
        dump_case(cfg, &name, &src, &mut out);
    }
    out
}

fn snapshot_path(variant: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join(format!("markdown_render.{variant}.snap"))
}

/// Splits a full dump into `(case label, case block text)` pairs at `"=== label ===\n"` headers.
/// No other line in the dump can start with `"=== "` (every content line starts with `L`, `  s`,
/// `-- `, `  i`, `  c`, `  t`, `  d`, `  a`, or `  m` — see `dump_case`), so this split is exact.
fn split_cases(dump: &str) -> Vec<(&str, &str)> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    let mut header_start: Option<usize> = None;
    let mut name: &str = "";
    let mut offset = 0usize;
    for line in dump.split('\n') {
        let line_start = offset;
        offset += line.len() + 1;
        if let Some(n) = line
            .strip_prefix("=== ")
            .and_then(|s| s.strip_suffix(" ==="))
        {
            if let Some(hs) = header_start {
                out.push((name, &dump[hs..line_start]));
            }
            header_start = Some(line_start);
            name = n;
        }
    }
    if let Some(hs) = header_start {
        out.push((name, &dump[hs..]));
    }
    out
}

/// A small window of context around the first line at which `expected`/`actual` diverge, rather
/// than the whole (possibly thousand-line) case block.
fn first_diff_context(expected: &str, actual: &str, context: usize) -> String {
    let el: Vec<&str> = expected.lines().collect();
    let al: Vec<&str> = actual.lines().collect();
    let n = el.len().min(al.len());
    let mut i = 0;
    while i < n && el[i] == al[i] {
        i += 1;
    }
    let lo = i.saturating_sub(context);
    let e_hi = (i + context + 1).min(el.len());
    let a_hi = (i + context + 1).min(al.len());
    let mut s = String::new();
    writeln!(s, "first differing line within the case (0-based): {i}").unwrap();
    writeln!(s, "--- expected[{lo}..{e_hi}] ---").unwrap();
    for l in &el[lo..e_hi] {
        writeln!(s, "{l}").unwrap();
    }
    writeln!(s, "--- actual[{lo}..{a_hi}] ---").unwrap();
    for l in &al[lo..a_hi] {
        writeln!(s, "{l}").unwrap();
    }
    s
}

/// Compares `actual` against the on-disk snapshot at `path`, identified as `label` in any panic
/// message. `pub(super)`: `md_model_snapshot_tests` reuses this exact comparison/diff/regenerate
/// machinery (rather than a second, hand-copied copy) for its own, differently-named snapshot file
/// — see that module's doc comment.
///
/// With `KONOMA_UPDATE_SNAPSHOTS=1` set in the environment, writes `actual` to disk instead of
/// comparing (see the module doc comment for the full regenerate command) — review the resulting
/// `git diff` under `snapshots/` before committing it, the same as any other generated-file change.
///
/// A missing snapshot file is treated like a missing `samples/*.md` fixture (see `sample_src`):
/// `snapshots/` is excluded from the published crate, so a build from an extracted tarball
/// legitimately has nothing to compare against — this skips rather than fails in that case.
pub(super) fn assert_matches_snapshot(path: PathBuf, label: &str, actual: &str) {
    if std::env::var_os("KONOMA_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshot path has a parent dir"))
            .expect("create snapshots/ dir");
        std::fs::write(&path, actual).expect("write snapshot file");
        eprintln!("markdown snapshot: wrote {}", path.display());
        return;
    }
    let Ok(expected) = std::fs::read_to_string(&path) else {
        eprintln!(
            "markdown snapshot: {} not found — skipping (published-crate build?); \
             regenerate with `KONOMA_UPDATE_SNAPSHOTS=1 cargo test`",
            path.display()
        );
        return;
    };
    if expected == actual {
        return;
    }
    let exp_cases = split_cases(&expected);
    let act_cases = split_cases(actual);
    for (i, (e, a)) in exp_cases.iter().zip(act_cases.iter()).enumerate() {
        if e != a {
            let (name, _) = e;
            panic!(
                "markdown snapshot ({label}) drifted at case #{i} {name:?}\n{}\n\
                 Re-run with KONOMA_UPDATE_SNAPSHOTS=1 and inspect the diff if this is intentional.",
                first_diff_context(e.1, a.1, 3)
            );
        }
    }
    panic!(
        "markdown snapshot ({label}) drifted: expected {} cases, got {} \
         (the set of cases itself changed, not just their content) — \
         re-run with KONOMA_UPDATE_SNAPSHOTS=1 if this is intentional.",
        exp_cases.len(),
        act_cases.len()
    );
}

/// The default-config golden snapshot. See the module doc comment for what this covers and how to
/// regenerate it after an intentional rendering change.
#[test]
fn markdown_render_snapshot_default() {
    assert_matches_snapshot(
        snapshot_path("default"),
        "default",
        &dump_variant(&Config::default()),
    );
}

/// The `code_bg = "none"` golden snapshot — the one config value under which several real bugs
/// have shipped and gone undetected before (code-detection had to stop relying on a background
/// color at all; see `is_inline_code_span`/`is_code_line`'s doc comments), so it gets a permanent
/// second snapshot rather than being folded into the default one.
#[test]
fn markdown_render_snapshot_code_bg_none() {
    let mut cfg = Config::default();
    cfg.ui.theme.code_bg = "none".to_string();
    assert_matches_snapshot(
        snapshot_path("code_bg_none"),
        "code_bg_none",
        &dump_variant(&cfg),
    );
}

/// Permanent regression guard for the property item 1 of the task this module was built for asks
/// to be checked: rendering the same corpus twice **within one process** must produce byte-identical
/// output. If some part of the pipeline (a `HashMap` iteration order, a shared cache keyed loosely
/// enough to alias, ...) is not deterministic, this fails long before it could quietly corrupt a
/// snapshot comparison.
#[test]
fn markdown_render_snapshot_is_deterministic_within_a_process() {
    let cfg = Config::default();
    let a = dump_variant(&cfg);
    let b = dump_variant(&cfg);
    assert_eq!(
        a, b,
        "rendering the golden-snapshot corpus twice in the same process produced different \
         output — some part of the render pipeline is not deterministic"
    );
}

/// Guards `all_cases()` itself: if a corpus-gathering call silently broke (wrong path, renamed
/// function returning an empty `Vec`, ...) both snapshot tests would still "pass" by comparing an
/// empty dump against an empty snapshot, which defeats the entire point of this module.
#[test]
fn markdown_render_snapshot_corpus_is_not_empty() {
    let cases = all_cases();
    assert!(
        cases.len() > 100,
        "the golden snapshot corpus looks suspiciously small ({} cases) — \
         a corpus-gathering call in `all_cases` probably broke",
        cases.len()
    );
}
