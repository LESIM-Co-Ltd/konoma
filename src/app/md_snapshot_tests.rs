//! Golden-snapshot safety net for Markdown rendering (`src/preview/markdown.rs`), added ahead of a
//! planned rendering refactor. This module renders every case in the existing parity corpora
//! (`task_corpus` / `code_corpus` / `code_span_corpus` / `preprocess_corpus`) plus the repo's
//! `samples/*.md` files, through **exactly the pipeline `App::build_decorated` +
//! `App::ensure_md_cache`'s post-process use** (see `src/app/md_render.rs`, `src/app/md_text.rs`,
//! and the precedent this mirrors, `app_faithful_parity_tests` in `markdown.rs`), and dumps the
//! *entire* observable result — every rendered line's style, every span's (content, style), every
//! inline-image placement, the render pass's own record of what `y c` would copy and what byte the
//! checkbox toggle would write (`MdRenderExtras`, read here exactly as production reads it), the
//! `<details>` open states (`collect_details_open`), and everything the app-side Tab/anchor builders
//! (`build_md_items_from_render`, `compute_md_anchors`) report — to a deterministic text file
//! checked into `snapshots/`.
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
const SNAPSHOT_WIDTH: u16 = 100;

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
    for (name, src) in crate::preview::markdown::html_table_corpus::cases() {
        v.push((format!("html_table_corpus: {name}"), src.to_string()));
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
struct CaseRender {
    lines: Vec<Line<'static>>,
    images: Vec<crate::preview::markdown::ImagePlacement>,
    items: Vec<MdItem>,
    anchors: Vec<(String, usize)>,
    /// The render pass's own record of every code block and task checkbox it drew — **the same
    /// struct production reads** (`app::DecoratedMarkdown::extras`), kept whole rather than unpacked
    /// so `build_md_items_from_render` below can take it directly. This used to be two hand-computed
    /// fields filled by the source scanners (`code_block_source_locs`/`task_source_locs`) — a pair of
    /// derivations nothing in production has called since the `md-block-walk` migration, dumped but
    /// never compared against anything, while the items were built from *empty* slices; see
    /// `build_md_items_from_render`'s own doc comment.
    extras: crate::preview::markdown::MdRenderExtras,
    details_states: Vec<bool>,
}

/// Renders `src` exactly the way `App::build_decorated`'s Markdown branch
/// (`src/app/md_render.rs`) does, followed by `App::ensure_md_cache`'s post-process
/// (`src/app/md_text.rs`'s `collapse_links` / `autolink_bare_urls` / `substitute_emoji`) and item
/// builders (`build_md_items_from_render` / `compute_md_anchors`) — the same order, the same
/// arguments, driven off the same `cfg` fields `build_decorated` reads — through the production
/// Markdown dispatcher (`render_markdown_with_images`, which resolves to `render::render_doc` for
/// every document — see that function's own doc comment). That "same arguments" is not a promise
/// this comment makes: `golden_items_match_the_live_app_across_the_corpus` renders the whole corpus
/// through a **real `App`** and requires the two to agree item for item.
///
/// The one deliberate departure from `build_decorated`: the image/mermaid/math *slot* closures there
/// depend on a live `Picker` (font metrics) and on-disk file state, neither of which exists in a unit
/// test and neither of which is deterministic across machines — see [`Slots`], which names the two
/// sets of answers this harness runs under (the goldens dump [`Slots::Extracted`]).
fn render_case(cfg: &Config, src: &str) -> CaseRender {
    render_case_with(cfg, src, Slots::Extracted)
}

/// Which answers the image / mermaid / math **slot** closures give — the one axis on which this
/// harness cannot be `App::build_decorated`, because those closures there depend on a live `Picker`
/// (font metrics) and on-disk file state, neither of which exists in a unit test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slots {
    /// What the goldens dump under: images "unavailable" (as with no image backend), every mermaid
    /// fence and math expression always "extracted" (`Image{cols:20,rows:5}` / `Raw` with extraction
    /// on), so the extraction and placement logic itself is still exercised end to end — the same
    /// values `app_faithful_parity_tests` fixes for the same reason.
    Extracted,
    /// Exactly what a **picker-less `App`** resolves to, so a dump made under this mode is
    /// comparable, item for item, with what the real app produces for the same file:
    /// `build_decorated`'s `font.is_none()` forces every image `Unavailable`, and both
    /// `mermaid_image_mode()` and `math_image_mode()` are `false` without a picker — so every fence
    /// degrades to text (the probe included) and math extraction is off.
    /// `golden_items_match_the_live_app_across_the_corpus` is the only caller.
    PickerLess,
}

fn render_case_with(cfg: &Config, src: &str, slots: Slots) -> CaseRender {
    let extract = slots == Slots::Extracted;
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

    let slot_of = |_: &str, _: Option<u16>| crate::preview::markdown::ImageSlot::Unavailable;
    let mermaid_slot = |_: &str| {
        if extract {
            crate::preview::markdown::MermaidSlot::Image { cols: 20, rows: 5 }
        } else {
            crate::preview::markdown::MermaidSlot::Text
        }
    };
    let math_slot = |_: &str, _: bool| crate::preview::markdown::MathSlot::Raw;
    let (mut lines, mut images, extras) = crate::preview::markdown::render_markdown_with_images(
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
        // math_on: extraction "on" mirrors the default `math = "image"` + a live picker; a
        // picker-less `App` computes `font.is_some() && math_image_mode()` = `false` instead.
        extract,
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

    // Items exactly the way `App::ensure_md_cache` builds them: the same function, taking the same
    // render pass's own record (`extras`) and the same placements the mermaid ordinals come from.
    // Nothing here re-derives "where are the code blocks / checkboxes" a second time — that is the
    // whole point (see `build_md_items_from_render`'s own doc comment): the dump below shows the
    // *production* source text `y c` would copy and the *production* byte the checkbox toggle would
    // write, so a change to either fails this golden.
    let items = build_md_items_from_render(&lines, &targets, &images, &extras);
    let anchors = compute_md_anchors(&lines);

    CaseRender {
        lines,
        images,
        items,
        anchors,
        extras,
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
        // `state_at` (the byte `Space` writes) and `body` (the text `y c` copies) are dumped in
        // full. They were left out until 2026-08 as "verification data, not rendered structure" —
        // which made the golden blind to the two bugs that actually matter here: `y c` copying a
        // *different* block and `Space` flipping a *different* line both left every line, span and
        // style in this dump untouched.
        MdItemKind::Task { state, state_at } => {
            format!("Task{{state={state:?},state_at={state_at:?}}}")
        }
        MdItemKind::CodeBlock { body } => format!("CodeBlock{{body={body:?}}}"),
        MdItemKind::MermaidFence { ordinal } => format!("MermaidFence{{ordinal={ordinal}}}"),
        MdItemKind::Details { ordinal } => format!("Details{{ordinal={ordinal}}}"),
    }
}

/// One `MdItem` exactly as the `ITEMS` section prints it. Factored out of `dump_case` so
/// `golden_items_match_the_live_app_across_the_corpus` compares **the dumped text**, character for
/// character, against the live app's own items — not a second, more forgiving rendering of them.
fn fmt_item(it: &MdItem) -> String {
    format!("line={:04} kind={}", it.line, fmt_item_kind(&it.kind))
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
    // Both sections are the render pass's own record — `MdRenderExtras`, exactly as production
    // reads it (`app::DecoratedMarkdown::extras`). `CODE_BLOCKS` is the source text `y c` copies,
    // by ordinal; `TASKS` is `(state character, byte offset **into `pre_src`**)`, the byte the
    // checkbox toggle overwrites. The byte offset is dumped raw, not resolved to a line/column,
    // because the raw byte is what production actually writes with.
    writeln!(out, "-- CODE_BLOCKS ({}) --", r.extras.code_blocks.len()).unwrap();
    for (i, b) in r.extras.code_blocks.iter().enumerate() {
        writeln!(out, "  c{i:02} {b:?}").unwrap();
    }
    writeln!(out, "-- TASKS ({}) --", r.extras.tasks.len()).unwrap();
    for (i, (state, off)) in r.extras.tasks.iter().enumerate() {
        writeln!(out, "  t{i:02} state={state:?} state_at={off:04}").unwrap();
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
        writeln!(out, "  m{i:02} {}", fmt_item(it)).unwrap();
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

/// Text an author commented out is never *drawn* — checked over the whole golden corpus, in both
/// snapshot configs, on the same rendered `Line`s the goldens dump.
///
/// The convention this rests on: a corpus case that hides something inside `<!-- … -->` names it
/// `SECRET-…`. Cases without that marker are skipped, so this costs nothing for the rest of the
/// corpus and needs no list of its own to be kept in sync — a shape added to any corpus later is
/// covered the moment it uses the marker.
///
/// Why a test and not just the goldens: a golden diff only fails for someone who reads it. This
/// fails by name, and states the property (`html_table_corpus`'s own doc comment explains the
/// disclosure that made it necessary — a commented-out `<tr>` folded into a real, drawn row).
///
/// The mirror assertion matters as much: a case that hides its marker by drawing *nothing at all*
/// would satisfy the first half while silently losing the author's real content, so every case
/// whose source keeps a live `keep` cell must still draw it.
#[test]
fn no_corpus_case_ever_draws_commented_out_text() {
    let mut code_bg_none = Config::default();
    code_bg_none.ui.theme.code_bg = "none".to_string();
    let mut checked = 0usize;
    for cfg in [Config::default(), code_bg_none] {
        for (name, src) in all_cases() {
            if !src.contains("SECRET") {
                continue;
            }
            checked += 1;
            let drawn: Vec<String> = render_case(&cfg, &src)
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
                .collect();
            for (i, line) in drawn.iter().enumerate() {
                assert!(
                    !line.contains("SECRET"),
                    "{name}: commented-out text reached the screen at line {i}: {line:?}"
                );
            }
            if src.contains(">keep<") {
                assert!(
                    drawn.iter().any(|l| l.contains("keep")),
                    "{name}: the live content went missing along with the commented-out text \
                     — drawn: {drawn:?}"
                );
            }
        }
    }
    assert!(
        checked > 0,
        "no corpus case carries a `SECRET-…` marker any more — this guard is checking nothing"
    );
}

/// Text an author wrote **outside** every `<td>`/`<th>` of an HTML table still reaches the screen —
/// the mirror of the test right above, and the other half of the same rule: a fold must never cost
/// the document a character, whether the character is one the author hid (checked there) or one the
/// author meant to be read (checked here).
///
/// The convention this rests on: a corpus case with text outside its table cells names that text
/// `LOOSE-…`. Cases without the marker are skipped, so a shape added to any corpus later is covered
/// the moment it uses the marker, with no list to keep in sync.
///
/// Why a test and not just the goldens: the loss this guards was not merely un-pinned before, it was
/// pinned *as the specification* — a corpus case named "caption alongside real rows is dropped" froze
/// the dropped caption into both goldens, so the bug would have survived any number of reviewed
/// snapshot diffs. A golden records what happens; this states what must happen.
#[test]
fn no_corpus_case_ever_drops_text_outside_a_table_cell() {
    let mut code_bg_none = Config::default();
    code_bg_none.ui.theme.code_bg = "none".to_string();
    let mut checked = 0usize;
    for cfg in [Config::default(), code_bg_none] {
        for (name, src) in all_cases() {
            // Every `LOOSE-…` token the source holds, each of which must be drawn somewhere.
            let markers: Vec<String> = src
                .split("LOOSE-")
                .skip(1)
                .map(|rest| {
                    let tail: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .collect();
                    format!("LOOSE-{tail}")
                })
                .collect();
            if markers.is_empty() {
                continue;
            }
            checked += 1;
            let drawn: Vec<String> = render_case(&cfg, &src)
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
                .collect();
            for m in &markers {
                assert!(
                    drawn.iter().any(|l| l.contains(m.as_str())),
                    "{name}: text outside the table's cells never reached the screen ({m}) \
                     — drawn: {drawn:?}"
                );
            }
        }
    }
    assert!(
        checked > 0,
        "no corpus case carries a `LOOSE-…` marker any more — this guard is checking nothing"
    );
}

// =================================================================================================
// Sentinels: the dump has to keep measuring **production's own values**
// =================================================================================================
//
// Until 2026-08 this harness computed its own `CODE_BLOCKS`/`TASKS` sections with the two retired
// source scanners (`code_block_source_locs`/`task_source_locs`) — functions **no production code
// has called** since the `md-block-walk` migration — and built its `ITEMS` with *empty* slices where
// production passes the render pass's own record. So `y c` could start copying a different code
// block, and `Space` could start flipping a different line of the file, without a single byte of
// either golden moving: the dump was measuring a function the app does not run, and printing `None`
// for the two fields that carry the answer. That is the same failure the `md-block-walk` diff
// harness hit one migration earlier (a live A/B comparison that quietly became self-comparison), so
// it gets the same treatment: not a comment promising the wiring is right, but tests that go red
// when it is not.

/// The dump must reproduce, item for item, what the **real `App`** builds for the same file.
///
/// This is the tie that makes the `ITEMS` section (and, through it, `MdItemKind::CodeBlock::body` =
/// what `y c` copies and `MdItemKind::Task::state_at` = the byte `Space` writes) worth anything: the
/// two sides are genuinely separate code — `App::build_decorated` + `App::ensure_md_cache` against
/// this harness's `render_case_with` mirror of them — so a mirror that stops matching production
/// fails here rather than silently freezing a fiction into `snapshots/`. Restoring the old
/// `build_md_items(&lines, &targets, &fence_ords, &[], &[])` call, for instance, fails on the first
/// corpus case that has a code block or a checkbox.
///
/// Rendered under [`Slots::PickerLess`] — the only mode a picker-less `App` can be compared against
/// (see that variant's own doc comment). The goldens themselves dump [`Slots::Extracted`]; the two
/// differ only in what the mermaid/math slots answer, and
/// `golden_sections_are_the_render_record_not_the_retired_scanners` covers the dumped text itself.
#[test]
fn golden_items_match_the_live_app_across_the_corpus() {
    let dir = crate::test_support::unique_tmp("konoma_golden_items_vs_app");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let f = root.join("doc.md");
    let cfg = Config::default();
    let (mut bodies, mut states) = (0usize, 0usize);
    for (name, src) in all_cases() {
        std::fs::write(&f, &src).unwrap();
        let mut app = App::new(root.clone(), Config::default()).unwrap();
        app.enter_preview(&f);
        app.ensure_md_cache(SNAPSHOT_WIDTH);
        let live: Vec<String> = app.md_items.iter().map(fmt_item).collect();
        let dumped: Vec<String> = render_case_with(&cfg, &src, Slots::PickerLess)
            .items
            .iter()
            .map(fmt_item)
            .collect();
        assert_eq!(
            dumped, live,
            "{name}: ゴールデンの ITEMS が実アプリの md_items と食い違う\
             (ハーネスが本番から外れている)\n--- src ---\n{src}"
        );
        // Every code block and checkbox on screen must carry its source data. `build_md_items`
        // resolves both with `.get(ordinal)`, so an empty (or short) record degrades to `None`
        // silently — exactly what the empty-slice call used to produce for the whole corpus.
        for it in &app.md_items {
            match &it.kind {
                MdItemKind::CodeBlock { body } => {
                    assert!(
                        body.is_some(),
                        "{name}: 画面のコードブロックに `y c` が読むソースが無い\
                         (レンダラの記録が届いていない)\n--- src ---\n{src}"
                    );
                    bodies += 1;
                }
                MdItemKind::Task { state_at, .. } => {
                    assert!(
                        state_at.is_some(),
                        "{name}: 画面のチェックボックスに書き戻し位置が無い\
                         (レンダラの記録が届いていない)\n--- src ---\n{src}"
                    );
                    states += 1;
                }
                _ => {}
            }
        }
        // The goldens themselves dump [`Slots::Extracted`], which the live app cannot be driven into
        // without a picker — so the one thing that mode is checked for here is the property the whole
        // exercise is about: whatever the mermaid/math slots answer, every sentinel the dump prints
        // still carries the render pass's record.
        for it in &render_case(&cfg, &src).items {
            match &it.kind {
                MdItemKind::CodeBlock { body } => assert!(
                    body.is_some(),
                    "{name}: ゴールデンが dump するモードでコードブロックのソースが欠落\n--- src ---\n{src}"
                ),
                MdItemKind::Task { state_at, .. } => assert!(
                    state_at.is_some(),
                    "{name}: ゴールデンが dump するモードでチェックボックスの書き戻し位置が欠落\n--- src ---\n{src}"
                ),
                _ => {}
            }
        }
    }
    // If the corpus ever stopped containing code blocks or checkboxes, everything above would pass
    // while checking nothing at all.
    assert!(
        bodies > 50 && states > 50,
        "コーパスのコードブロック({bodies})/チェックボックス({states})が激減している\
         — この番人が検査するものが無くなっている"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The count on a `-- SECTION (n) --` header line of one case's dump.
fn section_count(case_dump: &str, section: &str) -> usize {
    let needle = format!("-- {section} (");
    let at = case_dump
        .find(&needle)
        .unwrap_or_else(|| panic!("dump has no `{section}` section:\n{case_dump}"));
    let rest = &case_dump[at + needle.len()..];
    let end = rest.find(')').expect("section header closes its paren");
    rest[..end]
        .parse()
        .expect("section header count is a number")
}

/// Cases where the retired source scanners and the render pass's own record **provably** disagree,
/// as `(case label, what the retired scanner reports, what the render pass records)`.
///
/// These are the foils that make the sentinel below discriminate: on such a case, a dump built from
/// the scanner and a dump built from the record cannot print the same number, so the assertion
/// cannot be satisfied by both. Both directions are represented on purpose — the scanner *missing*
/// something that is really on screen (a plain, non-alert block quote: `render::render_doc` reads
/// its real block structure and draws real, toggleable checkboxes and real code-block headers inside
/// one, while the scanner only ever strips a `>` for a GitHub *alert* header) and the scanner
/// *inventing* something that is not (an indented line after a table, which the renderer does not
/// draw as a code block at all).
const RETIRED_SCANNER_FOILS: &[(&str, &str, usize, usize)] = &[
    ("task_corpus: plain blockquote", "TASKS", 0, 1),
    (
        "code_corpus: fence inside a plain block quote draws no header (tui-markdown prefixes every line)",
        "CODE_BLOCKS",
        0,
        1,
    ),
    (
        "code_corpus: an indented line right after a table block, with no blank line between",
        "CODE_BLOCKS",
        1,
        0,
    ),
];

/// Sentinel (番人): the dumped `CODE_BLOCKS`/`TASKS` sections carry the **render pass's own record**,
/// not the retired source scanners.
///
/// `golden_items_match_the_live_app_across_the_corpus` above pins the `ITEMS` section to production,
/// but it compares two `Vec<MdItem>` — it would not notice `dump_case` printing its two source
/// sections from somewhere else entirely, which is exactly what this harness did for a year. So this
/// one asserts on **the dump text itself**, for cases where the two derivations cannot agree.
///
/// Two-stage on purpose, the same shape the `md-block-walk` sentinel needed: stage 1 checks that the
/// foil still *is* a foil (the retired scanner really does still report the other number — if some
/// future change makes the two agree on one of these cases, this fails loudly and asks for a new
/// foil rather than degrading into a test that cannot tell them apart), stage 2 checks the dump
/// matches the record and not the scanner. Plus the `ITEMS` line format itself, which is what
/// carries the two fields into the golden at all.
#[test]
fn golden_sections_are_the_render_record_not_the_retired_scanners() {
    let cfg = Config::default();
    let cases = all_cases();
    for &(label, section, scanned, recorded) in RETIRED_SCANNER_FOILS {
        assert_ne!(
            scanned, recorded,
            "{label}: 食い違わない組を番人の対照に使っている(検査になっていない)"
        );
        let (_, src) = cases
            .iter()
            .find(|(n, _)| n == label)
            .unwrap_or_else(|| panic!("corpus case {label:?} is gone — pick a new sentinel foil"));

        // Stage 1: the retired scanner still reports what it always did for this document.
        let pre_src = pre_src_for(&cfg, src);
        let details = crate::preview::markdown::collect_details_open(&pre_src);
        let n_scanned = match section {
            "CODE_BLOCKS" => {
                crate::preview::markdown::code_block_source_locs(&pre_src, &details).len()
            }
            "TASKS" => crate::preview::markdown::task_source_locs(
                &pre_src,
                &cfg.ui.md_task_state_chars(),
                &details,
            )
            .len(),
            other => panic!("unknown section {other:?}"),
        };
        assert_eq!(
            n_scanned, scanned,
            "{label}: 退役スキャナの結果が変わった — 対照(foil)として機能しなくなったので選び直すこと"
        );

        // Stage 2: the dump prints the render pass's record.
        let mut dump = String::new();
        dump_case(&cfg, label, src, &mut dump);
        assert_eq!(
            section_count(&dump, section),
            recorded,
            "{label}: ゴールデンの {section} 節がレンダラの記録と違う\
             (退役スキャナ {n_scanned} 件に戻っていないか)\n{dump}"
        );
    }

    // The `ITEMS` line format: `body`/`state_at` must actually reach the dump. Both were omitted
    // from `fmt_item_kind` until 2026-08, so every item printed the same text whatever `y c` would
    // have copied and whatever byte `Space` would have written.
    let mut dump = String::new();
    let (label, src) = cases
        .iter()
        .find(|(n, _)| n == "task_corpus: plain blockquote")
        .expect("foil case checked above");
    dump_case(&cfg, label, src, &mut dump);
    assert!(
        dump.contains("state_at=Some("),
        "ITEMS 節にチェックボックスの書き戻し位置が出ていない\n{dump}"
    );
    let mut dump = String::new();
    let (label, src) = cases
        .iter()
        .find(|(n, _)| {
            n == "code_corpus: fence inside a plain block quote draws no header (tui-markdown prefixes every line)"
        })
        .expect("foil case checked above");
    dump_case(&cfg, label, src, &mut dump);
    assert!(
        dump.contains("CodeBlock{body=Some("),
        "ITEMS 節にコードブロックのソースが出ていない\n{dump}"
    );
}
