//! A real-world sweep of the block-model Markdown renderer (`crate::preview::markdown::render`)
//! against the pre-`md-block-walk` legacy renderer (`crate::preview::markdown::\
//! render_markdown_with_images_legacy`), over every `.md` file found under `~/.cargo/registry/src`
//! (populated by ordinary `cargo build`/`cargo fetch` activity — roughly 2,000 files on a machine
//! that has built more than a handful of crates).
//!
//! Unlike `md_render_diff_tests` (a **synthetic** corpus, hand-written to exercise specific shapes)
//! this sweep never invents input — it renders text real crates actually ship, through **the exact
//! preprocessing the production pipeline runs** (`md_snapshot_tests::pre_src_for` — front matter
//! stripped, footnotes renumbered, inline HTML converted, in that order, gated by the same `cfg`
//! fields `App::build_decorated` reads), so a document this sweep renders is byte-for-byte the same
//! text a real preview of that file would hand to `render::render_doc`.
//!
//! Three things are measured, described in full in each test's own doc comment below:
//!   1. [`real_markdown_corpus_does_not_panic_or_lose_content`] — no panic, on either renderer, over
//!      the whole sweep; and no text present in the legacy renderer's own output goes missing from
//!      the new renderer's output on any file the new renderer actually claims to support (i.e. any
//!      file `App::build_decorated` would really route to `render_doc` today — see
//!      `render_markdown_with_images`'s own doc comment on `RenderOut::unsupported`). Both are real
//!      defects if found, not "expected mismatches" — see that test's own doc comment for why a
//!      third, tolerated category (a rendering difference that is *not* content loss) is reported
//!      but never asserted empty.
//!   2. [`real_markdown_corpus_perf_probe`] — wall-clock and allocation on the sweep's own largest
//!      files, new vs. legacy.
//!
//! `#[ignore]`d: needs a populated Cargo registry cache, is not hermetic (its own report depends on
//! whatever crates happen to be cached on the machine that runs it), and is slow enough (a few
//! thousand real documents, each through two independent renderers) that it has no place in the
//! normal `cargo test` suite. Run explicitly:
//!
//! ```text
//! cargo test --features git --release real_markdown_corpus -- --ignored --nocapture
//! ```
//!
//! Gracefully **skips** (does not fail) when the registry cache is empty or absent — the identical
//! "not present, not a failure" contract `md_snapshot_tests::sample_src` already uses for
//! `samples/*.md` in a published-crate build, applied here to a machine-local cache instead of a
//! packaging exclusion.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

use super::md_snapshot_tests::{pre_src_for, SNAPSHOT_WIDTH};
use super::*;
use crate::preview::markdown as md;
use crate::preview::markdown::model::Doc;
use crate::preview::markdown::render::render_doc;
use crate::preview::markdown::ImagePlacement;

// ---------------------------------------------------------------------------------------------
// Panic containment (mirrors `markdown.rs`'s own `silence_panics`/`catch_silent`, duplicated
// rather than reused: those are private to `preview::markdown` and only ever report *whether* a
// panic happened, never the message — this sweep wants the message too, to name the actual defect
// in the "panicked files" report below, not just a count. Composite-hook-installed-once + a
// thread-local silence flag is the identical race-safe shape `silence_panics` documents (a naive
// take_hook/set_hook swap around every call races when panics happen on more than one thread at
// once); a second, independently-installed hook here layers safely on top of (or under) any other
// hook already installed, in either order, because each layer only ever swallows a panic it itself
// silenced and always forwards to whatever hook it captured as `prev` otherwise.
// ---------------------------------------------------------------------------------------------

thread_local! {
    static SWEEP_PANIC_MSG: RefCell<Option<String>> = const { RefCell::new(None) };
    static SWEEP_SILENCED: Cell<bool> = const { Cell::new(false) };
}

fn install_sweep_hook_once() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if SWEEP_SILENCED.with(|c| c.get()) {
                let msg = info
                    .payload()
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic (no message payload)".to_string());
                let loc = info
                    .location()
                    .map(|l| format!(" at {}:{}", l.file(), l.line()))
                    .unwrap_or_default();
                SWEEP_PANIC_MSG.with(|m| *m.borrow_mut() = Some(format!("{msg}{loc}")));
            } else {
                prev(info);
            }
        }));
    });
}

/// Runs `f`, catching a panic (message suppressed on stderr, captured instead) on this thread.
fn catch_with_msg<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    install_sweep_hook_once();
    SWEEP_SILENCED.with(|c| c.set(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    SWEEP_SILENCED.with(|c| c.set(false));
    r.map_err(|_| {
        SWEEP_PANIC_MSG
            .with(|m| m.borrow_mut().take())
            .unwrap_or_else(|| "panic (no message captured)".to_string())
    })
}

// ---------------------------------------------------------------------------------------------
// File collection
// ---------------------------------------------------------------------------------------------

/// Every `.md` file under `$CARGO_HOME/registry/src` (falling back to `$HOME/.cargo` when
/// `CARGO_HOME` is unset, matching Cargo's own default), sorted for a deterministic sweep order.
/// Empty (never an error) when the registry cache doesn't exist — the caller treats that as "skip".
///
/// `KONOMA_SWEEP_MAX` (an env var, not a config field — this test module is never compiled into the
/// shipped binary) truncates the list, for a fast iteration loop while developing this sweep itself;
/// unset for a real, full run.
fn collect_registry_md_files() -> Vec<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    let Some(cargo_home) = cargo_home else {
        return Vec::new();
    };
    let src_root = cargo_home.join("registry").join("src");
    if !src_root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack = vec![src_root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            let p = entry.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out.sort();
    if let Some(max) = std::env::var("KONOMA_SWEEP_MAX")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        out.truncate(max);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Rendering both sides, faithfully mirroring the production pipeline
// (`md_snapshot_tests::render_case` / `md_render_diff_tests::render_case_new`) — see each helper's
// own doc comment for exactly which piece of that pipeline it reproduces.
// ---------------------------------------------------------------------------------------------

struct Rendered {
    lines: Vec<Line<'static>>,
    images: Vec<ImagePlacement>,
}

/// The front-matter metadata block (rendered rows), identically to `render_case`'s own leading
/// section — computed from the **raw** `src`, not `pre_src` (front matter is stripped *before*
/// `pre_src_for` hands text to either renderer, so neither renderer ever sees it as a block of its
/// own; this is prepended back afterward, the same way production does).
fn front_matter_lines(cfg: &Config, src: &str) -> Vec<Line<'static>> {
    if !cfg.ui.md_frontmatter {
        return Vec::new();
    }
    match md::strip_front_matter(src) {
        (Some(fm), _) => md::render_front_matter(&fm, SNAPSHOT_WIDTH),
        (None, _) => Vec::new(),
    }
}

/// `App::postprocess_md`'s three free functions, in order, gated by the same `cfg` fields — applied
/// identically to both sides of this sweep's own comparison, so a mismatch it reports can only come
/// from the renderer itself, never from the two sides running different post-processing.
fn postprocess(cfg: &Config, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let (lines, targets) = collapse_links(lines, cfg.ui.icons);
    let (lines, _targets) = if cfg.ui.md_autolink {
        autolink_bare_urls(lines, targets)
    } else {
        (lines, targets)
    };
    if cfg.ui.md_emoji {
        substitute_emoji(lines)
    } else {
        lines
    }
}

/// Fixed, picker-independent answers for the image/mermaid/math slot closures — the identical
/// values `md_snapshot_tests::render_case`/`md_render_diff_tests::render_case_new` use for the same
/// reason (no live `Picker` or on-disk file state in a unit test): extraction/placement logic is
/// still exercised end to end, just not the picker-dependent raster step.
fn slot_of(_: &str) -> md::ImageSlot {
    md::ImageSlot::Unavailable
}
fn mermaid_slot(_: &str) -> md::MermaidSlot {
    md::MermaidSlot::Image { cols: 20, rows: 5 }
}
fn math_slot(_: &str, _: bool) -> md::MathSlot {
    md::MathSlot::Raw
}

/// `md::collect_details_open`'s own per-block answers, or (`force_open`) every block forced `true`
/// regardless — used by the content-loss check specifically (see its own call site's doc comment): a
/// `<details>` block folded **closed** hides its body behind an interactive marker the user can open
/// (`Tab`+`Space`), which is by-design, reachable information, not the kind of permanently-invisible
/// text loss this sweep exists to catch — forcing every block open on *both* sides before comparing
/// makes that distinction instead of conflating the two.
fn details_states(pre_src: &str, force_open: bool) -> Vec<bool> {
    let states = md::collect_details_open(pre_src);
    if force_open {
        vec![true; states.len()]
    } else {
        states
    }
}

/// Renders `src` through `render::render_doc` (the new, block-model renderer) plus the identical
/// front-matter prepend and post-process every real preview applies — i.e. exactly what
/// `App::build_decorated` shows today when this file's own top-level block set is fully covered
/// (`unsupported` empty; see the second return value). When `unsupported` is non-empty, this is
/// **not** what production actually shows for this file (`render_markdown_with_images` falls back
/// to the legacy renderer whole — see that function's own doc comment) — the caller treats such a
/// file as "fell back", not as a mismatch candidate.
///
/// `force_details_open`: see `details_states`'s own doc comment — `false` for the match/mismatch
/// comparison (what production actually renders today), `true` for the content-loss check.
fn build_new(
    cfg: &Config,
    src: &str,
    pre_src: &str,
    force_details_open: bool,
) -> (Rendered, Vec<&'static str>) {
    let code = md::CodeStyle {
        bg: cfg.ui.theme.code_bg(),
        label_bg: cfg.ui.theme.code_label_bg(),
        label_right: cfg.ui.theme.code_label_right(),
        tab_width: cfg.ui.tab_width,
        wrap: cfg.ui.wrap,
    };
    let theme = cfg.ui.theme.code_theme.as_str();
    let tasks = cfg.ui.md_task_state_chars();

    md::set_details_open(details_states(pre_src, force_details_open));
    let doc = Doc::parse(pre_src);
    let out = render_doc(
        &doc,
        pre_src,
        SNAPSHOT_WIDTH,
        code,
        theme,
        cfg.ui.icons,
        &tasks,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        cfg.ui.md_alerts,
        &math_slot,
        true,
    );
    let mut images = out.images;
    let fm = front_matter_lines(cfg, src);
    if !fm.is_empty() {
        for p in &mut images {
            p.line += fm.len();
        }
    }
    let mut lines = fm;
    lines.extend(out.lines);
    let lines = postprocess(cfg, lines);
    (Rendered { lines, images }, out.unsupported)
}

/// Renders `src` through `render_markdown_with_images_legacy` (the pre-`md-block-walk` renderer,
/// unconditionally — never through the dispatcher, so this always reflects what shipped *before*
/// the switch, regardless of whether the new renderer happens to support this particular file)
/// plus the identical front-matter prepend and post-process `build_new` applies.
///
/// `force_details_open`: see `details_states`'s own doc comment.
fn build_legacy(cfg: &Config, src: &str, pre_src: &str, force_details_open: bool) -> Rendered {
    let code = md::CodeStyle {
        bg: cfg.ui.theme.code_bg(),
        label_bg: cfg.ui.theme.code_label_bg(),
        label_right: cfg.ui.theme.code_label_right(),
        tab_width: cfg.ui.tab_width,
        wrap: cfg.ui.wrap,
    };
    let theme = cfg.ui.theme.code_theme.as_str();
    let tasks = cfg.ui.md_task_state_chars();

    md::set_details_open(details_states(pre_src, force_details_open));
    let (raw_lines, mut images) = md::render_markdown_with_images_legacy(
        pre_src,
        SNAPSHOT_WIDTH,
        code,
        theme,
        cfg.ui.icons,
        &tasks,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        cfg.ui.md_alerts,
        &math_slot,
        true,
    );
    let fm = front_matter_lines(cfg, src);
    if !fm.is_empty() {
        for p in &mut images {
            p.line += fm.len();
        }
    }
    let mut lines = fm;
    lines.extend(raw_lines);
    let lines = postprocess(cfg, lines);
    Rendered { lines, images }
}

// ---------------------------------------------------------------------------------------------
// Content-loss detection
// ---------------------------------------------------------------------------------------------

/// Every rendered line's span contents, concatenated (no separator within a line — matching how
/// they appear on screen), one `\n`-joined string for the whole document.
fn flatten_text(lines: &[Line<'static>]) -> String {
    let mut s = String::new();
    for l in lines {
        for sp in &l.spans {
            s.push_str(sp.content.as_ref());
        }
        s.push('\n');
    }
    s
}

/// The left-bar gutter `render::render_code_block` prefixes onto **every** fenced/indented code
/// row (cyan, `CODE_GUTTER_FG`) — the one prefix [`flatten_for_words`] strips before joining two
/// adjacent code rows with no separator between them. Left in place, the glyph itself — non-
/// alphanumeric, so already a token boundary on its own as far as [`words`] is concerned — would
/// silently reintroduce exactly the separator that join exists to remove; the fix has to strip it,
/// not merely skip inserting `flatten_text`'s own `\n`.
const CODE_GUTTER_PREFIX: &str = "▎ ";

/// Every rendered line's span contents, concatenated across the whole document into one string
/// [`missing_words`]'s own callers tokenize — [`flatten_text`]'s `\n`-joined form, except for one
/// narrow, confirmed exception: two **consecutive rows that both start with the code gutter**
/// (`CODE_GUTTER_PREFIX`) are joined with the gutter stripped and **no** separator inserted between
/// them at all, everything else keeping `flatten_text`'s own real `\n`-as-boundary behavior
/// unchanged (turned into a plain space here, rather than a character [`words`] would already treat
/// as a separator on its own — the two are equivalent to `words`, kept as an explicit space only so
/// this function's own output reads as ordinary prose under a debugger).
///
/// The exception exists for one specific, confirmed false positive plain `\n`-joining cannot avoid:
/// `render::render_code_block`'s own `wrap_spans_by_width` is this renderer's *only* construct that
/// hard-splits real document text at a fixed column, mid-word, with no wrap-worthy whitespace
/// anywhere near the cut — a genuinely continuous word split across two wrapped rows this way
/// (`"...patent claims licen"` / `"sable by such Contributor..."`, confirmed directly:
/// `safe_arch-0.9.3/LICENSE-APACHE.md`, an Apache-license clause hard-wrapped as one long indented
/// code block) reads back as the two meaningless fragments `"licen"`/`"sable"` under `\n`-joined
/// `flatten_text`, neither of which is the real word `"licensable"` (nor any word the ground-truth
/// source or the other renderer would ever independently produce on its own) — a false "content
/// loss" this module's own first sweep run actually hit. Stripping the gutter and joining with no
/// separator at all reverses exactly that split (`"licen"` + `"sable"` → `"licensable"`); a row that
/// legitimately ends *not* mid-word already carries its own real separator in its own content — this
/// renderer's code rows are always padded with real space characters out to the block's full column
/// width (confirmed in the same file's own dump: `"granting the License.                    "` — the
/// clause's own last, short row), so nothing here has to specifically detect "was this row full or
/// short" to get the right answer either way.
///
/// Scoped this narrowly (rather than "no separator between any two non-blank rows", this function's
/// own first version) on purpose: that broader version *also* fused unrelated, adjacent non-code
/// content with no blank-line separator of its own (consecutive standalone badge-image rows, a
/// heading rule immediately followed by the next block, ...) into meaningless joint tokens neither
/// side ever independently produces — turning several files this sweep already rendered
/// byte-identically into new, spurious "content loss" findings of its own (confirmed directly: this
/// module's own second sweep run, re-run specifically to check the broader version's own effect,
/// found *more* findings than the version before it, not fewer). Code rows are identifiable and
/// narrow enough a target (one specific glyph, one specific renderer construct) that this exception
/// carries none of that risk.
fn flatten_for_words(lines: &[Line<'static>]) -> String {
    let mut s = String::new();
    let mut prev_was_code = false;
    for l in lines {
        let t = line_plain_text(l);
        let is_code = t.starts_with(CODE_GUTTER_PREFIX);
        if is_code && prev_was_code {
            s.push_str(&t[CODE_GUTTER_PREFIX.len()..]);
        } else {
            s.push_str(&t);
            s.push('\n');
        }
        prev_was_code = is_code;
    }
    s
}

/// A single rendered line's plain text (span contents concatenated, style dropped) — used to tell
/// a pure style/markup difference (same words, different decoration) apart from a real text change.
fn line_plain_text(line: &Line<'static>) -> String {
    let mut s = String::new();
    for sp in &line.spans {
        s.push_str(sp.content.as_ref());
    }
    s
}

/// Minimum token length [`words`] keeps — long enough that a short, common token (an article, a
/// single-digit number, a lone gutter-adjacent letter) essentially never collides by coincidence
/// between two independently-generated strings, short enough that a genuinely lost short sentence
/// still contributes several distinct tokens. Chosen (and directly validated by re-running the whole
/// sweep both ways) over the original detector's 12-**character** sliding-window "shingle": a
/// shingle's own false-positive floor was too high to ever assert empty (a single reflowed line break
/// inside an otherwise-identical sentence shifts every shingle spanning it, `render::render_code_\
/// block`'s per-source-line code rendering the dominant real-world trigger — see this constant's own
/// git history for the sliding-window version this replaced), which is exactly why the original
/// checker below could only ever *report*, never *assert*, its own findings — three or four hundred
/// of them on a real registry sweep, the overwhelming majority not text loss at all. A whole *word*
/// carries meaning on its own the way an arbitrary 12-character substring straddling a line break does
/// not, so word-set difference has no equivalent floor: on the same corpus, it produced two orders of
/// magnitude fewer findings, and the residue was overwhelmingly small enough to read and classify by
/// hand (see [`real_markdown_corpus_does_not_panic_or_lose_content`]'s own doc comment for that
/// classification).
const MIN_WORD_LEN: usize = 4;

/// Case-folded, whitespace-and-punctuation-insensitive tokens of at least [`MIN_WORD_LEN`] Unicode
/// *characters* extracted from `s` — a "word" is a maximal run of `char::is_alphanumeric` characters
/// (a CJK run counts characters, not the 3 bytes each one typically takes, matching a human's own
/// sense of "how many words is this"); every other character — whitespace, markdown punctuation, box-
/// drawing/gutter glyphs (`▎`/`▏`/`▌`/`▸`/`▾`/`━`/`─`/`┌┐└┘├┤┬┴┼│`), a Nerd Font private-use icon
/// codepoint (`\u{f046}`/`\u{f096}`/...) — is simply a separator, never itself a candidate token. This
/// is why, unlike the sliding-window shingle checker this replaced, no hand-maintained decoration
/// allowlist (that checker's own `DECORATION_CHARS`) is needed here at all: none of those glyphs is
/// alphanumeric, so every one of them already acts as a separator with zero extra code.
fn words(s: &str, min_len: usize) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            if cur.chars().count() >= min_len {
                out.insert(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.chars().count() >= min_len {
        out.insert(cur);
    }
    out
}

/// Byte ranges of every `Event::Html`/`Event::InlineHtml` pulldown-cmark reports anywhere in
/// `pre_src`'s own parse — scanned directly off `Doc::parse`'s own comprehensive `events` stream
/// (`Doc.events`'s own doc comment: "every `Event`/byte-range pair pulldown-cmark reported for the
/// *whole* document, not merely the ones some `Block` below happens to reference"), not the block
/// tree, so this catches both a top-level `Html` block's own range (a `<!-- ... -->` comment, a
/// `<div>` banner, a malformed/glued `<details>` whole construct pulldown-cmark reports as one
/// opaque `HtmlBlock` — `BlockKind::Details`'s own doc comment on `glued_body`) *and* a **scattered
/// inline** HTML tag inside any paragraph/heading/list item/table cell's own running text, which a
/// block-tree walk would never see a `Block` for at all: `render.rs`'s own `walk_inline` drops
/// `Event::Html`/`Event::InlineHtml` silently, no output whatsoever, the identical convention
/// tui-markdown itself documents (that function's own comment cites it) — confirmed directly:
/// `zerocopy-0.8.56/README.md`'s own decorative `***<span style="font-size: 140%">Fast, safe, <span
/// style="color:red;">compile error</span>. Pick two.</span>***` tagline, where `style` exists in
/// the raw source only inside these two `<span style="...">` tags' own attribute text, and both
/// this renderer and real GitHub draw only `"Fast, safe, compile error. Pick two."` — no block of
/// any kind here for a `BlockKind::Html`-only walk (this function's own first version) to ever find.
///
/// Adjacent/overlapping ranges are merged into one run before returning — `ground_truth_source_\
/// text`'s own caller needs whole runs, not fragments: `render_html_block`'s own comment-stripping
/// (a `<!-- ... -->` spanning many small `Event::Html` entries, one per physical source line —
/// confirmed directly in this module's own development) only finds its own real `-->` end condition
/// when handed the *whole* run's raw text in one call; called on any one line-sized fragment alone
/// it can never see past its own edges (see that function's own doc comment for exactly why "runs
/// to the next blank line" vs. a real `-->` end condition is the very distinction this module exists
/// to get right).
fn collect_html_event_ranges(pre_src: &str) -> Vec<Range<usize>> {
    let doc = Doc::parse(pre_src);
    let mut raw: Vec<Range<usize>> = doc
        .events
        .iter()
        .filter(|(ev, _)| {
            matches!(
                ev,
                pulldown_cmark::Event::Html(_) | pulldown_cmark::Event::InlineHtml(_)
            )
        })
        .map(|(_, r)| r.clone())
        .collect();
    raw.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for r in raw {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// `pre_src` with every [`collect_html_event_ranges`] run replaced by exactly the text
/// [`crate::preview::markdown::render_html_block`] would draw for it — the same tag/comment-stripping
/// **both** renderers actually call (`render.rs`'s own `render_html_block_from_model` for the new
/// renderer, `markdown.rs`'s own `render_html_block` call sites for the legacy one), reused here
/// rather than a second, hand-rolled comment/tag scanner, so this can never itself drift from what
/// either renderer actually strips. A `<!-- ... -->` comment strips to nothing (confirmed directly:
/// [`real_markdown_corpus_does_not_panic_or_lose_content`]'s own doc comment, "HTML comment" bullet,
/// against both the CommonMark HTML-block-type-2 spec and `johannesvollmer/exrs`'s own real,
/// GitHub-rendered README); a non-comment block (a `<div>` badge banner, a bare `<kbd>` tag not
/// already converted by `process_inline_html`, an inline `<span style="...">`, ...) keeps whatever
/// text content survives tag-stripping. This is the "ground truth" a real CommonMark renderer would
/// consider visible — the text this sweep's content-loss check requires a *legacy-shown* word to
/// still be traceable to before treating its absence from the new renderer's own output as a real
/// defect; a word that exists **only** inside a genuine comment or tag attribute (on *either* side
/// of it, in this same document, not just the occurrence closest to the one legacy happened to leak)
/// is filtered out of that ground truth entirely, wherever else in `pre_src` it may or may not also
/// occur, rather than blanket-removed as a bare word-set difference — the same word appearing again
/// in a real, visible sentence elsewhere in the same document still counts, because *that*
/// occurrence's own bytes are untouched here.
/// Byte ranges of every reference-style link/image definition line (`[label]: destination
/// "title"`) `pre_src`'s own parse recognizes as a *definition* — a construct CommonMark (§4.7)
/// gives **zero** visible output of its own: it exists only to resolve a `[text][label]` elsewhere
/// in the same document, and `render_doc` reads it off `Doc::parse`'s own already-resolved
/// `dest_url` at the *reference site*, never at the definition line itself (`Doc.events` carries no
/// event for a definition line at all — pulldown-cmark's own first pass consumes it entirely before
/// the main walk ever starts, so there is no `Event::Html`/`Event::InlineHtml` here for
/// `collect_html_event_ranges`'s own scan to find in the first place). The legacy renderer's own
/// line-based scanner
/// has no concept of a reference definition at all, and shows its literal text as an ordinary
/// paragraph line instead — confirmed directly: `displaydoc-0.2.6/CHANGELOG.md`'s own trailing
/// `[0.2.6]: https://github.com/yaahc/displaydoc/compare/v0.2.5...v0.2.6` block, a shape the
/// overwhelming majority of `cargo new`/`cargo-release`-style `CHANGELOG.md` trailers in this same
/// registry sweep share.
///
/// Queried via a **second**, independent `Parser` — `md::model::parse_options()`, the *same*
/// function `Doc::parse` itself calls to build its own, rather than a hand-copied option set, for
/// the identical reason that function's own doc comment gives for calling it instead of copying it
/// — because `Doc` itself keeps no reference to the `Parser` that built it once `Doc::parse`
/// returns, and there is nothing on `Doc` a definition's own span could be read back off of.
fn collect_refdef_ranges(pre_src: &str) -> Vec<Range<usize>> {
    let parser = pulldown_cmark::Parser::new_ext(pre_src, md::model::parse_options());
    parser
        .reference_definitions()
        .iter()
        .map(|(_, def)| def.span.clone())
        .collect()
}

/// Byte ranges of a **link's own destination-only portion** — everything from the end of its own
/// label content through the end of the link construct as a whole (`](url "title")` for an inline
/// link, `][label]`/`][]`/nothing for a reference/collapsed/shortcut one, or, for an autolink,
/// essentially nothing at all — for **every** `Event::Start(Tag::Link { .. })` reachable anywhere in
/// `pre_src`'s own parse, and, separately, for an `Event::Start(Tag::Image { link_type, .. })` **only
/// when `link_type` is not `LinkType::Inline`** — `render_image_slot`'s own `ImageSlot::Unavailable`
/// fallback (`image_text_fallback`) draws a bare, standalone image's own destination as real, visible
/// text (`"🖼 alt — url"`) on *both* renderers, and for an **inline** image (`![alt](url)`) that text
/// *is* the raw source's own destination syntax — keeping it in ground truth is correct. It is not
/// for any of the other five `LinkType`s (`Reference`/`ReferenceUnknown`/`Collapsed`/\
/// `CollapsedUnknown`/`Shortcut`): what `render_image_slot` actually draws there is the *resolved*
/// `dest_url` `Doc::parse` already looked up (`render.rs`'s own module doc comment, "How inline
/// content is rendered" — a reference is read straight off `Tag::Image.dest_url`, never off its own
/// raw `[label]` bracket text) — the raw `][crates.io shield]`/`][]` bracket syntax itself is never
/// shown as text by *any* correct renderer, confirmed directly: `bit-set-0.8.0/README.md`'s own
/// `[![crates.io][crates.io shield]][crates.io link]` badge (a link-wrapped, reference-style image),
/// where `"crates.io shield"` exists in raw source only as that reference *label*, distinct from
/// both the resolved image `alt` (`"crates.io"`) and its own resolved `dest_url`
/// (`https://img.shields.io/crates/v/bit-set?label=latest`) this renderer actually draws.
///
/// Only a **link's** own destination is additionally the part `App::postprocess_md`'s shared
/// `collapse_links` pass strips from the *displayed* line on both sides (`app/copy_actions.rs`'s own
/// "hidden link target" span, moved into a separate `targets` vector the focused-link-open feature
/// reads, never left behind in the rendered `Line`s `flatten_text`/`flatten_for_words` walk) — a
/// badge's own *wrapping* link (`[![alt](img_src)](href)`) still keeps its **inline** image's own
/// `img_src` words in ground truth exactly this way (an `Image` nested inside the excluded `Link`
/// span is walked past, not itself excluded, and is itself excluded only when its own `link_type`
/// warrants it, by the same rule as everywhere else), only the wrapping link's own `href` disappears,
/// matching what `render_image_slot` actually draws for one either way.
///
/// Confirmed as the source of three, real, distinct false-positive shapes a real registry sweep
/// otherwise reports as "content loss": (1) `svgtypes-0.15.3/CHANGELOG.md`'s own `https` — legacy
/// leaks `https://keepachangelog.com/…` from an internal-blank-line HTML comment (the already-
/// documented "HTML comment" class this module's own doc comment already gives), which
/// `ground_truth_source_text` already correctly hides — but *un-hides itself* again, for that exact
/// same word, from an entirely unrelated resolved link's own destination syntax elsewhere in the
/// same file (`[`angle`](https://www.w3.org/TR/SVG11/types.html#DataTypeAngle)`) that **neither**
/// renderer ever shows as visible text either — raw source text alone cannot tell "genuinely visible
/// prose" and "a link's own hidden destination" apart without this. (2) A **multi-physical-line**
/// badge whose own `](` and its own destination land on different *source* lines (`crossbeam-\
/// channel-0.5.16/README.md`'s own `[![Build Status](…/badge.svg)](\nhttps://…/actions)`, hard-
/// wrapped in the source for readability) — this renderer correctly recognizes the whole multi-line
/// construct as one `[![alt](img)](href)` badge (matching real CommonMark: a soft line break inside
/// one paragraph never ends the construct) and draws it exactly the same way it draws any *single*-
/// line badge (`render_image_slot`'s own `alt`/`img_src`, never a wrapping link's own `href`) — but
/// the legacy renderer's own **per-physical-line** `extract_block_image` scanner cannot recognize
/// either of those two lines as "just an image" *on its own*, so it falls through to raw text,
/// where a separate, later pass (`autolink_bare_urls`) turns the leftover bare `href` fragment into
/// a real, clickable autolink of its own — legacy showing a real, distinct **defect** (this
/// construct not recognized as one coherent badge at all, an unmatched trailing `)` visibly left
/// over right after it — confirmed directly in this module's own development,
/// `crossbeam-channel-0.5.16/README.md`'s own dump) as if it were *more* information than the new
/// renderer draws, not less. (3) `bit-set-0.8.0/README.md`'s own `shield`/`count` — the reference-
/// style badge shape above: legacy fails to resolve the reference at all (shows the raw, unresolved
/// `[![crates.io][crates.io shield]][crates.io link]` bracket syntax literally, `[crates.io shield]`
/// and all — the already-documented "reference-style link/image resolution" class), while the new
/// renderer correctly resolves every one of the same file's six badges to a real image (confirmed
/// directly, this module's own development).
fn collect_hidden_reference_ranges(pre_src: &str) -> Vec<Range<usize>> {
    let doc = Doc::parse(pre_src);
    let events = &doc.events;
    let mut out = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let wants_end = match &events[i].0 {
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link { .. }) => {
                pulldown_cmark::TagEnd::Link
            }
            pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { link_type, .. })
                if !matches!(link_type, pulldown_cmark::LinkType::Inline) =>
            {
                pulldown_cmark::TagEnd::Image
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let overall = events[i].1.clone();
        let mut depth = 0i32;
        let mut label_end = overall.start;
        let mut j = i + 1;
        while j < events.len() {
            match &events[j].0 {
                pulldown_cmark::Event::Start(_) => depth += 1,
                pulldown_cmark::Event::End(e) if *e == wants_end && depth == 0 => break,
                pulldown_cmark::Event::End(_) => depth -= 1,
                _ => {}
            }
            label_end = label_end.max(events[j].1.end);
            j += 1;
        }
        if label_end < overall.end {
            out.push(label_end..overall.end);
        }
        // *Not* `i = j + 1` (skipping straight past this whole construct's own matching `End`):
        // a `Link` can wrap an `Image` (the reference-style badge shape this function exists for —
        // `bit-set-0.8.0/README.md`'s own `[![crates.io][crates.io shield]][crates.io link]`), and
        // that inner `Image`'s own `Start` needs this exact same walk run on *it* too — its own
        // trailing `][crates.io shield]` reference-label portion is a **separate** hidden range,
        // nested entirely inside the outer link's own `label_end..overall.end` computation above
        // (which only ever reads `events[..].1.end` to find the outer label's own edge — it never
        // separately visits the inner `Image` as a trigger of its own). Skipping ahead here — this
        // function's own first version — silently produced zero ranges for every reference-style
        // *image* nested inside a reference-style *link* wrapping it (confirmed directly: this
        // module's own development, `bit-set-0.8.0/README.md`'s own `shield`/`count` still reported
        // "missing" after this function's own first pass, `collect_hidden_reference_ranges` finding
        // only the outer links' own `][crates.io link]`-shaped ranges, never the inner images' own).
        i += 1;
    }
    out
}

/// Which kind of "not real, visible prose" range [`ground_truth_source_text`] found — determines how
/// its own bytes are replaced (see that function's own doc comment): [`Html`](ReplaceKind::Html)
/// keeps whatever [`crate::preview::markdown::render_html_block`] would draw for it; the other two
/// vanish entirely, matching that neither renderer ever shows either one as visible text.
enum ReplaceKind {
    Html,
    RefDef,
    LinkDest,
}

fn ground_truth_source_text(pre_src: &str) -> String {
    let mut ranges: Vec<(Range<usize>, ReplaceKind)> = collect_html_event_ranges(pre_src)
        .into_iter()
        .map(|r| (r, ReplaceKind::Html))
        .chain(
            collect_refdef_ranges(pre_src)
                .into_iter()
                .map(|r| (r, ReplaceKind::RefDef)),
        )
        .chain(
            collect_hidden_reference_ranges(pre_src)
                .into_iter()
                .map(|r| (r, ReplaceKind::LinkDest)),
        )
        .collect();
    ranges.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(pre_src.len());
    let mut cursor = 0usize;
    for (r, kind) in ranges {
        if r.start < cursor {
            continue; // Nested inside (or overlapping) an already-flushed range above — skip.
        }
        out.push_str(&pre_src[cursor..r.start]);
        if let ReplaceKind::Html = kind {
            out.push_str(&flatten_text(&md::render_html_block(&pre_src[r.clone()])));
        }
        cursor = r.end;
    }
    out.push_str(&pre_src[cursor..]);
    out
}

/// Every [`words`] token that appears in `legacy_text` **and** is traceable to real, CommonMark-
/// visible source text (`ground_truth_source_text(pre_src)`) but does not appear anywhere in
/// `new_text` — a **real content-loss finding** unless it is independently understood and explained
/// (see the caller's own doc comment). `min_len` is always [`MIN_WORD_LEN`] in this module; a
/// parameter only so a unit test below can probe the boundary explicitly.
fn missing_words(
    legacy_text: &str,
    new_text: &str,
    ground_truth_source: &str,
    min_len: usize,
) -> Vec<String> {
    let legacy_w = words(legacy_text, min_len);
    let new_w = words(new_text, min_len);
    let src_w = words(ground_truth_source, min_len);
    let mut missing: Vec<String> = legacy_w
        .into_iter()
        .filter(|w| src_w.contains(w) && !new_w.contains(w))
        .collect();
    missing.sort();
    missing
}

// ---------------------------------------------------------------------------------------------
// Residual findings this detector's own structural fixes cannot avoid without weakening its own
// precision elsewhere — see [`explain_residual`]'s own doc comment for the "reason, not file"
// contract every one of these follows.
// ---------------------------------------------------------------------------------------------

/// A **reason** a residual "content loss" finding is not real loss, checked structurally (never by
/// bare file path) — see [`explain_residual`], the one function that produces these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResidualReason {
    /// Every missing word is a strict **prefix** of some word the *new* renderer's own output
    /// genuinely contains, differing only by a short (≤3-character) trailing suffix — the text is
    /// present, merely fused with whatever real content immediately follows it with no separator in
    /// between, exactly the way it reads on screen.
    ///
    /// Confirmed, by hand, as the *only* explanation for every occurrence this reason ever matches
    /// in the current sweep: `vello_common-0.0.{6,8,9}/README.md` all write the identical
    /// `[`Pixmap`][crate::pixmap::Pixmap]s from PNG images.` — a correctly **resolved** reference-
    /// style link (`render_doc` reads `Doc::parse`'s own already-resolved `dest_url` — `render.rs`'s
    /// own module doc comment, "How inline content is rendered") whose own label text, `"Pixmap"`,
    /// sits directly against the sentence's own plural `"s"` with zero characters between them in
    /// the raw source. [`words`]' own whole-token matching can only ever see the fused `"pixmaps"`,
    /// never `"pixmap"` on its own — even though every letter of it is genuinely on screen, styled
    /// as a real, clickable link. The **legacy** renderer only ever shows `"pixmap"` as its own
    /// isolated token here because it fails to resolve the very same reference at all (the already-
    /// documented "reference-style link/image resolution" false-positive class this test module's
    /// own main doc comment already gives, above): its own literal, unresolved `[`Pixmap`][crate::\
    /// pixmap::Pixmap]s` text happens to have bracket punctuation — a real token separator — sitting
    /// directly after `"Pixmap"`, which is exactly what keeps its own copy of the word from fusing
    /// the same way. Not a defect in either renderer; a limitation of comparing rendered *text* by
    /// whole word rather than by exact grapheme position — confirmed by hand for all three
    /// occurrences this reason currently matches, not merely assumed to generalize.
    FusedPluralSuffix,
    /// The source file is not real, renderable Markdown at all: `emojis-0.9.0/docs/README_TEMPLATE.\
    /// md` is a Tera/Jinja **build template** (`{% if config.badges.crates_io -%}` control
    /// statements and `{{ manifest.name }}` variable interpolations interleaved with Markdown, e.g.
    /// `[![Crates.io Version](https://badgers.space/crates/version/{{ manifest.name }})](https://\
    /// crates.io/crates/{{ manifest.name }})`) — meant to be run through a template engine, filling
    /// in every `{{ }}`/`{% %}`, *before* the result is ever shown as a README to a human or to any
    /// Markdown renderer, this one included; nothing renders this file directly, on GitHub or
    /// anywhere else, the same way nothing renders a `.rs.in` or `.tera` file directly either.
    ///
    /// Confirmed by hand this is not "legacy happens to work, new regressed": legacy's own output
    /// for this exact file is equally incoherent — literal, un-stripped `{% if config.badges.\
    /// crates_io -%}` control lines shown as visible text, badges landing on rows that do not
    /// correspond to their own source position at all (`debug_dump_file`'s own dump for this file,
    /// checked directly) — not a working baseline the new renderer fell short of matching.
    UntemplatedBuildFile,
}

impl ResidualReason {
    fn label(self) -> &'static str {
        match self {
            ResidualReason::FusedPluralSuffix => "fused-plural-suffix",
            ResidualReason::UntemplatedBuildFile => "untemplated-build-file",
        }
    }
}

/// Whether `missing` (a file's own [`missing_words`] output) is fully explained by one of
/// [`ResidualReason`]'s two reasons — checked **structurally** against this file's own real content
/// (`pre_src`/`new_text`), never by matching a bare file path, per this whole module's own directive
/// (its main test's own doc comment): "if a residual can't be reduced to zero, explain it per
/// *reason*, with a minimal repro, never blanket-allowlist it per file". A finding this returns
/// `None` for is a **real** defect — asserted empty by the caller, not silently tolerated.
fn explain_residual(pre_src: &str, new_text: &str, missing: &[String]) -> Option<ResidualReason> {
    // `UntemplatedBuildFile`: a real markdown document essentially never contains *both* `{%` and
    // `{{` (Jinja/Tera's own control- and variable-interpolation delimiters) — co-occurrence is a
    // strong, content-based signal this file was never meant to be rendered as Markdown directly,
    // checked here rather than trusting a file *name* (`README_TEMPLATE.md`'s own name is a hint,
    // not proof — a real crate could coincidentally name a real README that way).
    if pre_src.contains("{%") && pre_src.contains("{{") {
        return Some(ResidualReason::UntemplatedBuildFile);
    }
    // `FusedPluralSuffix`: every missing word must itself be genuinely present in the new renderer's
    // own output, just fused onto a short trailing suffix with no separator — never merely "some
    // similar-looking word happens to exist somewhere in this large a document" (the `<= 3`-\
    // character cap on the suffix, and requiring *every* missing word in this finding to match, not
    // just one, are both there specifically to keep this narrow enough not to risk masking an
    // unrelated real loss elsewhere in the same file).
    let new_w = words(new_text, MIN_WORD_LEN);
    if !missing.is_empty()
        && missing.iter().all(|w| {
            new_w.iter().any(|nw| {
                nw.starts_with(w.as_str()) && nw.len() > w.len() && nw.len() - w.len() <= 3
            })
        })
    {
        return Some(ResidualReason::FusedPluralSuffix);
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Mismatch classification (informational only — never asserted empty; see the main test's own
// doc comment for why a rendering *difference* that carries no text loss is tolerated here).
// ---------------------------------------------------------------------------------------------

fn classify_mismatch(new: &Rendered, legacy: &Rendered) -> &'static str {
    let lines_eq = new.lines == legacy.lines;
    let images_eq = new.images == legacy.images;
    if lines_eq && !images_eq {
        return "image placement differs (rendered lines identical)";
    }
    if new.lines.len() != legacy.lines.len() {
        return "line count differs";
    }
    let new_text: Vec<String> = new.lines.iter().map(line_plain_text).collect();
    let legacy_text: Vec<String> = legacy.lines.iter().map(line_plain_text).collect();
    if new_text == legacy_text {
        return "style/decoration only (plain text identical, same line count)";
    }
    "text differs (same line count)"
}

// ---------------------------------------------------------------------------------------------
// The sweep itself
// ---------------------------------------------------------------------------------------------

/// One panic (or missing-content) record: `(file, stage, detail)`.
struct Finding {
    file: String,
    stage: &'static str,
    detail: String,
}

/// The main sweep: every `.md` file under the Cargo registry cache, through both renderers.
///
/// **Any panic** — in preprocessing, the new renderer, or the legacy renderer — fails this test
/// unconditionally (design principle #3, `CLAUDE.md`: konoma must never crash on real input, however
/// malformed). This is one of two hard gates the sweep enforces; see below for the second.
///
/// A file whose top-level block set is not (yet) fully covered by `render_doc` (`unsupported`
/// non-empty) is never a defect candidate at all: `render_markdown_with_images` — the real dispatcher
/// every preview path calls — falls back to the legacy renderer *whole* for exactly that file (see
/// its own doc comment), so production never actually shows the new renderer's own, deliberately
/// incomplete, output for it. This sweep still tallies such files (and *why*, from `RenderOut::\
/// unsupported`'s own block-kind names) for the report, but excludes them from the match/mismatch/
/// content-loss accounting entirely.
///
/// ## Content loss — the sweep's other hard gate
///
/// **Content loss** ([`missing_words`] — a whole-*word* set difference against a real, CommonMark-
/// grounded ground truth, [`ground_truth_source_text`] — see that function's own doc comment) **is
/// asserted empty**, with one narrow, structural, reason-keyed exception ([`explain_residual`]
/// below). This is a harder bar than this test module started at: an earlier version of this same
/// check (a 12-**character** sliding-window substring difference, `shingles` — since removed, see
/// `git log` for the version this replaced) could only ever *report*, never *assert*, its own
/// findings, because straddling a single reflowed line break was enough to trip a false positive on
/// its own, and a real registry sweep reliably produced several hundred of them even after every
/// genuine bug this module's own development found got fixed. Whole-word matching does not share
/// that floor, but getting all the way to an assertable zero took real, iterative work, in three
/// categories:
///
/// 1. **[`ground_truth_source_text`] itself** (see that function's own doc comment for the full
///    detail on each): raw source text is *not* the same thing as "text a correct renderer would
///    ever show", and treating it as if it were is what produced most of this sweep's own early
///    false positives. A `<!-- ... -->` HTML comment, a `[label]: url` reference definition, a
///    link's own `(url "title")`/`[label]` destination syntax, and a *non-inline* image's own
///    reference-style `[label]` (as opposed to an inline one's `(url)`, which legitimately *is*
///    shown, via `image_text_fallback`) are all real Markdown source bytes that **no** correct
///    renderer ever draws as literal visible text — `ground_truth_source_text` strips exactly these,
///    and only these, before this sweep's own word-set comparison ever runs.
/// 2. **Real bugs this sweep's own development found and fixed** in `render.rs`, once (1) closed off
///    the noise obscuring them: `paragraph_as_block_images`'s own image-count check (closing a
///    paragraph of several concatenated `[![alt](url)](href)` badges silently losing every URL but
///    the first); `segment_as_block_images`/`split_top_level_image_units` (several badges sharing
///    one physical source line, separated only by whitespace or a trailing HTML comment — losing
///    every one of their own URLs, not just the extras); an empty top-level segment (a bare `\`
///    hard-break line between two rows of badges) silently disqualifying an entire paragraph's own
///    "nothing but images" recognition; `paragraph_trailing_images` (a real sentence directly
///    followed by one or more standalone image lines, no blank line between); its own mirror,
///    `paragraph_leading_images` (one or more standalone image lines directly followed by real text
///    — a shape an earlier version of this sweep's own doc comment named but left deliberately
///    open, closed here once confirmed real rather than left open a second time); and
///    `heading_trailing_images` (a heading — ATX or setext, one physical source line or several —
///    whose own title text is directly followed by one or more badges, a construct this renderer
///    had *no* image-extraction of any kind for before this sweep's own development, `walk_inline`'s
///    generic `Tag::Image` handling discarding every heading-embedded badge's own URL unconditionally).
/// 3. **Two narrow, structural residual reasons** ([`explain_residual`]) for findings that survive
///    (1) and (2) but are still not real loss — see that function's own doc comment for both,
///    including the confirmed, hand-checked repro each one is keyed to. Checked **per finding**,
///    never a bare per-file allowlist: a finding this function cannot explain is asserted to not
///    exist.
///
/// A `debug_dump_file`/`scratch_probe_*` investigation (`KONOMA_DEBUG_FILE=<path> cargo test \
/// --features git <test-name> -- --ignored --nocapture`, optionally `KONOMA_DEBUG_FORCE_OPEN=1` to
/// bypass `<details>` folding) is the right first step for any *new* finding a future sweep run
/// reports, before assuming it needs a code fix or a new residual reason — several of the findings
/// this module's own development chased down looked like real losses at first glance and turned out,
/// on inspection, to be the *new* renderer doing something *more* correct than the legacy one (the
/// legacy renderer's own reference-resolution failures and HTML-comment-blank-line leak are the two
/// most common shapes of that, both already accounted for structurally in (1) above).
///
/// A rendering **difference that is not content loss** (different styling, different line wrapping, a
/// reordered image placement, ...) is likewise reported (classified, with a per-category count) but
/// never asserted to be zero — `md_render_diff_tests`'s own module doc comment already establishes,
/// over a hand-picked synthetic corpus, that real, accepted differences exist between the two
/// renderers (`BEHAVIOR_DECISIONS`/`INTENDED_IMPROVEMENTS`); a real-file sweep is expected to surface
/// more instances of those exact same, already-understood categories, not to prove the two renderers
/// pixel-identical.
#[test]
#[ignore]
fn real_markdown_corpus_does_not_panic_or_lose_content() {
    let files = collect_registry_md_files();
    if files.is_empty() {
        eprintln!(
            "real_markdown_corpus: no .md files found under $CARGO_HOME/registry/src — \
             skipping (registry cache not populated on this machine?)"
        );
        return;
    }
    eprintln!("real_markdown_corpus: sweeping {} files", files.len());

    let cfg = Config::default();

    let mut read_failures = 0usize;
    let mut panics: Vec<Finding> = Vec::new();
    let mut content_loss: Vec<Finding> = Vec::new();
    let mut residuals: Vec<(Finding, ResidualReason)> = Vec::new();

    let mut unsupported_files = 0usize;
    let mut unsupported_reasons: HashMap<&'static str, usize> = HashMap::new();

    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut mismatch_kinds: HashMap<&'static str, usize> = HashMap::new();
    let mut mismatch_examples: HashMap<&'static str, Vec<String>> = HashMap::new();

    for (i, path) in files.iter().enumerate() {
        if i > 0 && i % 250 == 0 {
            eprintln!("real_markdown_corpus: {i}/{} done", files.len());
        }
        let name = path.display().to_string();
        let Ok(src) = std::fs::read_to_string(path) else {
            read_failures += 1;
            continue;
        };

        let pre_src = match catch_with_msg(|| pre_src_for(&cfg, &src)) {
            Ok(s) => s,
            Err(msg) => {
                panics.push(Finding {
                    file: name,
                    stage: "preprocess",
                    detail: msg,
                });
                continue;
            }
        };

        let new_res = catch_with_msg(|| build_new(&cfg, &src, &pre_src, false));
        let legacy_res = catch_with_msg(|| build_legacy(&cfg, &src, &pre_src, false));

        let (new_r, unsupported) = match new_res {
            Ok(v) => v,
            Err(msg) => {
                panics.push(Finding {
                    file: name,
                    stage: "render_doc",
                    detail: msg,
                });
                continue;
            }
        };
        let legacy_r = match legacy_res {
            Ok(v) => v,
            Err(msg) => {
                panics.push(Finding {
                    file: name,
                    stage: "legacy",
                    detail: msg,
                });
                continue;
            }
        };

        if !unsupported.is_empty() {
            unsupported_files += 1;
            for r in &unsupported {
                *unsupported_reasons.entry(r).or_insert(0) += 1;
            }
            continue; // production falls back to `legacy_r` whole for this file — nothing to compare.
        }

        if new_r.lines == legacy_r.lines && new_r.images == legacy_r.images {
            matched += 1;
        } else {
            mismatched += 1;
            let kind = classify_mismatch(&new_r, &legacy_r);
            *mismatch_kinds.entry(kind).or_insert(0) += 1;
            let examples = mismatch_examples.entry(kind).or_default();
            if examples.len() < 5 {
                examples.push(name.clone());
            }
        }

        // Content-loss check: re-render **with every `<details>` block forced open** on both sides
        // (see `details_states`'s own doc comment) — a closed fold hiding text behind `Tab`+`Space`
        // is by-design, reachable information, not the permanent, unrecoverable loss this check
        // exists to catch. A panic on *this* pass specifically (distinct from the match/mismatch
        // pass above, which already succeeded) still counts as a real defect.
        let new_open_res = catch_with_msg(|| build_new(&cfg, &src, &pre_src, true));
        let legacy_open_res = catch_with_msg(|| build_legacy(&cfg, &src, &pre_src, true));
        let (new_open, legacy_open) = match (new_open_res, legacy_open_res) {
            (Ok((n, _)), Ok(l)) => (n, l),
            (Err(msg), _) | (_, Err(msg)) => {
                panics.push(Finding {
                    file: name,
                    stage: "force-open-details",
                    detail: msg,
                });
                continue;
            }
        };
        let legacy_open_text = flatten_for_words(&legacy_open.lines);
        let new_open_text = flatten_for_words(&new_open.lines);
        let ground_truth = ground_truth_source_text(&pre_src);
        let mut missing = missing_words(
            &legacy_open_text,
            &new_open_text,
            &ground_truth,
            MIN_WORD_LEN,
        );
        if !missing.is_empty() {
            missing.truncate(30);
            match explain_residual(&pre_src, &new_open_text, &missing) {
                Some(reason) => residuals.push((
                    Finding {
                        file: name,
                        stage: "content-loss",
                        detail: format!("missing words: {missing:?}"),
                    },
                    reason,
                )),
                None => content_loss.push(Finding {
                    file: name,
                    stage: "content-loss",
                    detail: format!("missing words: {missing:?}"),
                }),
            }
        }
    }

    eprintln!("=== real_markdown_corpus report ===");
    eprintln!("  files swept: {}", files.len());
    eprintln!("  read failures (non-UTF8/unreadable): {read_failures}");
    eprintln!("  panics: {}", panics.len());
    for p in &panics {
        eprintln!("    [{}] {}: {}", p.stage, p.file, p.detail);
    }
    eprintln!(
        "  fell back to legacy (render_doc reports part of the file unsupported): {unsupported_files}"
    );
    let mut reasons: Vec<(&&str, &usize)> = unsupported_reasons.iter().collect();
    reasons.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (reason, count) in &reasons {
        eprintln!("    - {reason}: {count}");
    }
    let compared = matched + mismatched;
    eprintln!("  compared (new renderer actually ships for these): {compared}");
    eprintln!("    match (byte-identical to legacy): {matched}");
    eprintln!("    mismatch: {mismatched}");
    let mut kinds: Vec<(&&str, &usize)> = mismatch_kinds.iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (kind, count) in kinds.iter().take(8) {
        eprintln!("      - {kind}: {count}");
        if let Some(examples) = mismatch_examples.get(**kind) {
            for e in examples {
                eprintln!("          e.g. {e}");
            }
        }
    }
    // `residuals` — explained, structurally, by `explain_residual` — are reported (so a new instance
    // of a known reason is still visible in the log) but never asserted: they are not content loss.
    eprintln!(
        "  content-loss findings, explained (not asserted): {}",
        residuals.len()
    );
    for (c, reason) in &residuals {
        eprintln!("    [{}] {}: {}", reason.label(), c.file, c.detail);
    }
    // `content_loss` — anything `explain_residual` could **not** account for — is a real defect and
    // is asserted empty below (this test's own doc comment, "Content loss" section, has the full
    // three-part account of how this sweep gets to zero here: `ground_truth_source_text`'s own
    // exclusions, the real `render.rs` bugs this sweep's own development found and fixed, and the two
    // narrow residual reasons above). A newly-appearing entry here is worth investigating by hand
    // (`debug_dump_file`/`scratch_probe_ground_truth`, `KONOMA_DEBUG_FILE=<path> cargo test --features \
    // git <test-name> -- --ignored --nocapture`) before assuming it needs a code fix — several
    // findings this module's own development chased down turned out, on inspection, to be the *new*
    // renderer doing something *more* correct than the legacy one, not a regression.
    eprintln!(
        "  content-loss findings, unexplained: {}",
        content_loss.len()
    );
    for c in &content_loss {
        eprintln!("    {}: {}", c.file, c.detail);
    }
    eprintln!("=== end report ===");

    assert!(
        panics.is_empty(),
        "{} file(s) panicked while rendering — see the report above (--nocapture) for which \
         stage and message; this is a design-principle-#3 violation, not a tolerated mismatch",
        panics.len()
    );
    assert!(
        content_loss.is_empty(),
        "{} file(s) show real content loss — see the report above (--nocapture, \"unexplained\") \
         for which words and files; investigate each with debug_dump_file/scratch_probe_ground_truth \
         before assuming a new residual reason (`explain_residual`) explains it — several findings \
         this module's own development chased down turned out to be the new renderer doing something \
         more correct than the legacy one instead",
        content_loss.len()
    );
}

/// Sanity check on the sweep harness itself: if `collect_registry_md_files` silently broke (wrong
/// path, an early return), the main test above would "pass" by sweeping zero files — this fails
/// loudly instead, the same guard `markdown_render_diff_corpus_is_not_empty` gives the synthetic
/// corpus. Also `#[ignore]`d (same registry-cache dependency), kept as a separate test so a broken
/// harness reports clearly rather than as a suspiciously-instant "0 files, 0 panics" pass above.
#[test]
#[ignore]
fn real_markdown_corpus_sweep_is_not_empty() {
    let files = collect_registry_md_files();
    if std::env::var_os("CARGO_HOME").is_none() && std::env::var_os("HOME").is_none() {
        return; // genuinely no way to locate a registry cache on this machine — not a harness bug.
    }
    assert!(
        !files.is_empty(),
        "collect_registry_md_files() found 0 .md files under $CARGO_HOME/registry/src — \
         the registry cache may be genuinely empty (never built anything with dependencies on \
         this machine) or the collection helper itself broke"
    );
}

// ---------------------------------------------------------------------------------------------
// Performance: the sweep's own largest real files, new renderer vs. legacy.
// ---------------------------------------------------------------------------------------------

/// Wall-clock + allocation for one file, new vs. legacy, averaged over a few reps (rendering is
/// deterministic and cheap enough per-call that a handful of reps smooths out scheduler noise
/// without materially slowing the probe down).
struct PerfSample {
    file: String,
    bytes: u64,
    new_time: Duration,
    legacy_time: Duration,
    new_alloc: u64,
    legacy_alloc: u64,
}

/// Times + allocation-measures both renderers on the largest few files the sweep actually found
/// (by byte size — the files most likely to reveal an `O(n^2)` or an unbounded-clone regression),
/// reporting a ratio for each. Not a pass/fail gate on its own real-file numbers (a real registry
/// cache is not hermetic input to assert exact bounds against — file sizes and shapes vary machine
/// to machine); [`render_doc_scales_like_legacy_on_a_large_synthetic_document`] (in `speed_tests.rs`)
/// is the permanent, hermetic regression guard this probe's own findings fed the threshold for.
#[test]
#[ignore]
fn real_markdown_corpus_perf_probe() {
    let files = collect_registry_md_files();
    if files.is_empty() {
        eprintln!("real_markdown_corpus_perf_probe: no .md files found — skipping");
        return;
    }
    let mut sized: Vec<(u64, PathBuf)> = files
        .into_iter()
        .filter_map(|p| std::fs::metadata(&p).ok().map(|m| (m.len(), p)))
        .collect();
    sized.sort_by_key(|(len, _)| std::cmp::Reverse(*len));
    let top: Vec<(u64, PathBuf)> = sized.into_iter().take(5).collect();
    if top.is_empty() {
        eprintln!("real_markdown_corpus_perf_probe: no readable files — skipping");
        return;
    }

    let cfg = Config::default();
    const REPS: u32 = 3;

    let mut samples: Vec<PerfSample> = Vec::new();
    for (bytes, path) in &top {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let pre_src = match catch_with_msg(|| pre_src_for(&cfg, &src)) {
            Ok(s) => s,
            Err(_) => continue, // a panicking file is reported by the main sweep, not here.
        };

        // Warm (grammar compilation, etc. — a one-time process-global cost neither renderer should
        // be blamed for) before the measured reps.
        let _ = catch_with_msg(|| build_new(&cfg, &src, &pre_src, false));
        let _ = catch_with_msg(|| build_legacy(&cfg, &src, &pre_src, false));

        let t = Instant::now();
        let mut new_alloc_total = 0u64;
        for _ in 0..REPS {
            new_alloc_total += crate::mem_tests::allocated_by(|| {
                let _ = catch_with_msg(|| build_new(&cfg, &src, &pre_src, false));
            });
        }
        let new_time = t.elapsed() / REPS;

        let t = Instant::now();
        let mut legacy_alloc_total = 0u64;
        for _ in 0..REPS {
            legacy_alloc_total += crate::mem_tests::allocated_by(|| {
                let _ = catch_with_msg(|| build_legacy(&cfg, &src, &pre_src, false));
            });
        }
        let legacy_time = t.elapsed() / REPS;

        samples.push(PerfSample {
            file: path.display().to_string(),
            bytes: *bytes,
            new_time,
            legacy_time,
            new_alloc: new_alloc_total / u64::from(REPS),
            legacy_alloc: legacy_alloc_total / u64::from(REPS),
        });
    }

    eprintln!(
        "=== real_markdown_corpus_perf_probe (largest {} files) ===",
        samples.len()
    );
    for s in &samples {
        let time_ratio = s.new_time.as_secs_f64() / s.legacy_time.as_secs_f64().max(1e-9);
        let alloc_ratio = s.new_alloc as f64 / s.legacy_alloc.max(1) as f64;
        eprintln!(
            "  {} ({} bytes)\n    time:  new={:?} legacy={:?} ratio(new/legacy)={:.2}\n    alloc: new={} legacy={} ratio(new/legacy)={:.2}{}",
            s.file,
            s.bytes,
            s.new_time,
            s.legacy_time,
            time_ratio,
            s.new_alloc,
            s.legacy_alloc,
            alloc_ratio,
            if time_ratio > 2.0 || alloc_ratio > 2.0 { "  <-- >2x, flagged" } else { "" }
        );
    }
    eprintln!("=== end perf probe ===");
}

#[test]
#[ignore]
fn debug_dump_file() {
    let Some(path) = std::env::var_os("KONOMA_DEBUG_FILE") else {
        eprintln!("set KONOMA_DEBUG_FILE=<path> to use this");
        return;
    };
    let path = PathBuf::from(path);
    let force_open = std::env::var_os("KONOMA_DEBUG_FORCE_OPEN").is_some();
    let cfg = Config::default();
    let src = std::fs::read_to_string(&path).expect("read");
    let pre_src = pre_src_for(&cfg, &src);
    let new_res = catch_with_msg(|| build_new(&cfg, &src, &pre_src, force_open));
    let legacy_res = catch_with_msg(|| build_legacy(&cfg, &src, &pre_src, force_open));
    let (new_r, unsupported) = new_res.expect("new panicked");
    let legacy_r = legacy_res.expect("legacy panicked");
    eprintln!("unsupported: {unsupported:?}");
    let n = new_r.lines.len().max(legacy_r.lines.len());
    for i in 0..n {
        let a = new_r.lines.get(i);
        let b = legacy_r.lines.get(i);
        let a_text: String = a
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        let b_text: String = b
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        if a_text != b_text {
            eprintln!("L{i:04} DIFF");
            eprintln!("  new:    {a:?}");
            eprintln!("  legacy: {b:?}");
        }
    }
    eprintln!(
        "new images: {:?}\nlegacy images: {:?}",
        new_r.images, legacy_r.images
    );
}
