//! konoma — a full-screen preview-focused terminal file browser.
//!
//! Pick something in the tree, then preview it full-screen. The core model is a transition
//! between two full-screen modes — Tree and Preview — with no in-between split view.
//! See `docs/PRD.md` for the specification.
//!
//! This file is the entry point: argument handling, terminal initialization, and the event
//! loop. Heavy work (image resize/encode, the git `ignored` set) is offloaded to worker
//! threads so the draw loop stays responsive; the screen is redrawn only when something changes.

mod app;
mod bookmarks;
mod config;
#[cfg(test)]
mod e2e_tests;
mod fileops;
mod git;
mod i18n;
mod keymap;
#[cfg(test)]
mod mem_tests;
mod preview;
mod session;
#[cfg(test)]
mod speed_tests;
#[cfg(test)]
mod test_support;
mod ui;
mod vcs;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui_image::errors::Errors;
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use app::{
    App, FileOpResult, FilterPoolResult, FsBurstKinds, GitOpResult, IgnoredResult, KittyResult,
    MdEncodeRequest, MdEncodeResult, MdImageResult, MediaResult, RemoteFetch, SortKey,
    StatusResult,
};
use keymap::{Action, KeyPress, Motion, Resolution, Surface};

/// Re-encode result sent from the worker to the UI.
type ResizeResult = Result<ResizeResponse, Errors>;

/// What process arguments say `main` should do — parsed by a pure function so it's testable
/// without touching the environment. Mirrors `std::env::args()`, `argv[0]` included at index 0.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedArgs {
    /// `--version` / `-V`.
    Version,
    /// `--help` / `-h`.
    Help,
    /// The raw path argument, exactly as given (unresolved, uncanonicalized). `None` = no
    /// argument was given, which falls back to the current directory.
    Open(Option<PathBuf>),
}

/// Parse process arguments into a `ParsedArgs`. Only the first argument is inspected — extra
/// arguments are ignored, matching the pre-existing `konoma [DIR]` contract of one optional
/// positional argument (no subcommands).
///
/// Only `--version`/`-V` and `--help`/`-h` are recognized as flags. Anything else — including an
/// unrecognized `--foo`-shaped argument or an empty string — is treated as a directory path and
/// never rejected: a real directory that happens to be named like a flag still opens, and this
/// keeps parsing dependency-free (no `clap`) rather than growing an "unknown flag" error surface
/// for what has always been a single positional argument.
fn parse_args(args: &[String]) -> ParsedArgs {
    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-V") => ParsedArgs::Version,
        Some("--help") | Some("-h") => ParsedArgs::Help,
        Some(arg) => ParsedArgs::Open(Some(PathBuf::from(arg))),
        None => ParsedArgs::Open(None),
    }
}

/// What `main` does once `resolve_startup` has run: print-and-exit, or the validated directory to
/// browse.
#[derive(Debug)]
enum Startup {
    Version,
    Help,
    Open(PathBuf),
}

/// Resolve process arguments into a `Startup`, **validating that an `Open` target is actually an
/// openable directory** before returning it. Called from `main` before `ratatui::init()` touches
/// the terminal (raw mode + the alternate screen) — see `validate_root`'s doc for why that order
/// matters. The `?` here is what makes the order load-bearing: a bad path returns `Err` straight
/// out of this function, so `main` never reaches the line that initializes the terminal.
fn resolve_startup(args: &[String]) -> Result<Startup> {
    Ok(match parse_args(args) {
        ParsedArgs::Version => Startup::Version,
        ParsedArgs::Help => Startup::Help,
        ParsedArgs::Open(path_arg) => {
            let dir = match path_arg {
                Some(p) => p,
                None => std::env::current_dir().context("could not get the current directory")?,
            };
            // Normalize so path-copy still produces a correct absolute path even for a relative
            // argument (e.g. `konoma samples`). On canonicalize failure (the common case: the path
            // doesn't exist), fall back to joining it with cwd so validate_root below still reports
            // a sensible (if unresolved) path instead of losing it.
            let dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&dir))
                    .unwrap_or(dir)
            });
            validate_root(&dir)?;
            Startup::Open(dir)
        }
    })
}

/// Whether `path` is a directory konoma can actually open — checked by `resolve_startup`, before
/// `ratatui::init()` ever runs. This used to only surface once `App::new()` failed, which was
/// *after* `ratatui::init()` had already put the terminal into raw mode + the alternate screen: on
/// a non-interactive shell (a script, plain `sh`, etc. — anything that doesn't re-initialize the
/// tty per prompt the way an interactive zsh/bash does) that left the terminal stuck — no echo, no
/// newline translation — until the user manually ran `reset`.
///
/// Distinguishes not-found / not-a-directory / unreadable rather than one blanket message: a plain
/// `std::fs::metadata` check alone would miss a directory that exists and passes `is_dir()` but
/// can't actually be listed (e.g. `chmod 000`), so `read_dir` is checked too.
fn validate_root(path: &Path) -> Result<()> {
    let meta =
        std::fs::metadata(path).with_context(|| format!("cannot open {}", path.display()))?;
    if !meta.is_dir() {
        anyhow::bail!("cannot open {}: not a directory", path.display());
    }
    // metadata() alone doesn't catch a directory that exists and is_dir()=true but can't actually
    // be listed (e.g. chmod 000) — read_dir() opens it for real.
    std::fs::read_dir(path).with_context(|| format!("cannot open {}", path.display()))?;
    Ok(())
}

/// The `--version`/`-V` output (a plain function, not printed directly, so it's testable and its
/// content is guaranteed to match what actually prints).
fn version_text() -> String {
    format!("konoma {}\n", env!("CARGO_PKG_VERSION"))
}

/// The `--help`/`-h` output. Kept in sync with the README's "Usage" section by hand (both are short).
fn help_text() -> String {
    format!(
        "konoma {version} — a full-screen preview-focused terminal file browser\n\
         \n\
         Usage: konoma [DIR]\n\
         \n\
         Opens DIR in the tree view (defaults to the current directory).\n\
         \n\
         Options:\n\
         \x20\x20-h, --help      Print this help and exit\n\
         \x20\x20-V, --version   Print version and exit\n\
         \n\
         Press ? inside konoma for the full, context-sensitive key reference.\n\
         Documentation: https://lesim-co-ltd.github.io/konoma/\n",
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// The font size `Picker::from_query_stdio` falls back to (`picker.rs`'s private `DEFAULT_PICKER`)
/// whenever the terminal answered the graphics-capability query but not the font-size query, or
/// didn't answer stdio queries at all. Every pixel-size calculation derived from
/// `Picker::font_size()` (still images, and the inline Markdown mermaid/math image pipeline)
/// inherits this fixed 10/20 = 0.5000 cell aspect ratio, which is off by several percent on real
/// fonts (measured via `fontTools`' cmap + advance-width dump: HackGen Console NF ≈0.4704,
/// HackGen35 ≈0.5176, Menlo ≈0.5172), skewing the displayed image.
const DEFAULT_PICKER_FONT_SIZE: (u16, u16) = (10, 20);

/// Derive a terminal cell's pixel size `(width, height)` from `crossterm::terminal::window_size()`'s
/// raw fields — a pure function (no I/O) so it's testable without a real terminal. `columns`/`rows`
/// is the character grid, `width_px`/`height_px` is the same window measured in pixels.
///
/// Returns `None` when the terminal didn't report usable pixel dimensions: crossterm's own doc for
/// `window_size()` warns "the width and height in pixels may not be reliably implemented or default
/// to 0" (true on plenty of unix ttys/terminals, and unimplemented on Windows) — dividing by a zero
/// column/row count would panic, and a zero pixel size can't be turned into a sane cell size either,
/// so any zero input is treated as "unknown" rather than producing a bogus `(0, 0)` cell.
fn cell_px_from_window_size(
    columns: u16,
    rows: u16,
    width_px: u16,
    height_px: u16,
) -> Option<(u16, u16)> {
    if columns == 0 || rows == 0 || width_px == 0 || height_px == 0 {
        return None;
    }
    // max(1): guard the (unreachable in practice — width_px/height_px are already checked non-zero
    // above, and integer division only rounds down) case of a window narrower than its own column
    // count, so callers never receive a 0-pixel cell.
    let cell_w = (width_px / columns).max(1);
    let cell_h = (height_px / rows).max(1);
    Some((cell_w, cell_h))
}

/// Replace `picker`'s font size with the real cell size derived from `window_size` — but **only**
/// when `picker.font_size()` still holds the exact `DEFAULT_PICKER_FONT_SIZE` sentinel (a picker
/// with any other font size came from a real capability response and is left untouched) **and**
/// `window_size` (the caller's `crossterm::terminal::window_size()` result, pre-flattened to a plain
/// tuple so this function needs no terminal / stays a pure transform of its inputs) yields a usable
/// cell size via `cell_px_from_window_size`. Otherwise `picker` is returned unchanged.
///
/// This only ever replaces the font size — never `protocol_type` (Halfblocks/Kitty/Sixel/...): that
/// field is decided from a *different*, private signal inside ratatui-image (`capability_proto`,
/// local to `Picker::from_query_stdio_with_options`) that a caller outside the crate cannot recover,
/// so a terminal that answered the graphics-capability query but not the font-size query still
/// renders via whatever protocol it already fell back to (typically Halfblocks) even after this
/// fix — fixing that is out of scope here (see `docs/STATUS.md`).
fn fix_picker_font_size(picker: Picker, window_size: Option<(u16, u16, u16, u16)>) -> Picker {
    let fs = picker.font_size();
    if (fs.width, fs.height) != DEFAULT_PICKER_FONT_SIZE {
        return picker;
    }
    let Some((columns, rows, width_px, height_px)) = window_size else {
        return picker;
    };
    let Some((cell_w, cell_h)) = cell_px_from_window_size(columns, rows, width_px, height_px)
    else {
        return picker;
    };
    // `Picker` has no public setter for its (private) `font_size` field, so the only way to build
    // one with a chosen font size is the deprecated `from_fontsize` constructor (deprecated in favor
    // of `from_query_stdio`/`halfblocks`, neither of which accepts a custom font size — there is no
    // non-deprecated alternative). `set_protocol_type` right after restores the protocol the
    // original `picker` already had, so nothing but the font size actually changes; `background_color`
    // and `capabilities` are always empty/`None` on a picker that reached the default font size in
    // the first place (every code path that produces the sentinel also produces those as its
    // defaults), so nothing is lost by rebuilding via `from_fontsize` instead of mutating in place.
    #[allow(deprecated)]
    let mut fixed = Picker::from_fontsize(ratatui_image::FontSize::new(cell_w, cell_h));
    fixed.set_protocol_type(picker.protocol_type());
    fixed
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    // Resolved — and, for `Open`, validated — **before any terminal initialization** below (see
    // `resolve_startup`/`validate_root`). `--version`/`--help` print and exit here without ever
    // touching the terminal, and a bad directory fails here too: entering raw mode + the alternate
    // screen further down used to run *before* `App::new()` discovered a bad path — too late,
    // since a non-interactive shell (a script, plain `sh`, etc.) doesn't reset terminal modes per
    // prompt the way an interactive zsh/bash does, leaving the terminal stuck (no echo, no newline
    // translation) until the user manually ran `reset`.
    let dir = match resolve_startup(&args)? {
        Startup::Version => {
            print!("{}", version_text());
            return Ok(());
        }
        Startup::Help => {
            print!("{}", help_text());
            return Ok(());
        }
        Startup::Open(dir) => dir,
    };

    let (cfg, cfg_err) = config::Config::load_reporting();

    // Loading syntect's assets + compiling a grammar's regexes (once per language, a few seconds
    // in debug) is heavy. On startup, warm it up on a separate thread — going through the current
    // root's extensions **in order of most files first** — to eliminate the freeze on the first
    // code preview (if it doesn't finish in time, just wait normally for the first one only). A
    // single thread + yield so it doesn't get in the way of other work.
    // With highlighting off (ui.syntax_highlight=false), syntect is never used at all, so no
    // warm-up is needed.
    if cfg.ui.syntax_highlight {
        let root = dir.clone();
        std::thread::spawn(move || preview::code::warm_dir(root));
    }

    let mut terminal = ratatui::init();
    // Enable bracketed paste: so a file drag & drop (the terminal delivers the path as a paste) is
    // received as `Event::Paste` and wired into the copy/move dialog. Ignored harmlessly on
    // unsupported terminals.
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);

    // Image backend: query the terminal (detects kitty, etc.). Once, after entering the alt
    // screen and before reading events.
    // Held as an Option so that on failure (unsupported terminal, etc.) everything but the
    // preview still works.
    let picker = Picker::from_query_stdio().ok();
    // The font-size query above can come back empty even when the graphics-capability query
    // succeeded (see `fix_picker_font_size`'s doc); recover the real cell aspect from the
    // terminal's actual pixel dimensions when that happened, so image/mermaid/math sizing isn't
    // skewed by ratatui-image's fixed 10x20 fallback. `window_size()` itself can fail or report
    // zero pixels (crossterm's own doc: "may not be reliably implemented"), in which case
    // `fix_picker_font_size` leaves `picker` untouched.
    let window_size = crossterm::terminal::window_size()
        .ok()
        .map(|w| (w.columns, w.rows, w.width, w.height));
    let picker = picker.map(|p| fix_picker_font_size(p, window_size));

    // On a terminal that can show images, warm up SVG's system-font enumeration (~tens of ms the
    // first time) in the background at startup ahead of time
    // (hides the UI freeze on the first SVG display — the same policy as syntax highlighting's warm_dir).
    // In mermaid image mode, likewise warm up the renderer's lazy font measurement (hides the
    // tens-of-ms freeze on the first diagram display).
    if picker.is_some() {
        std::thread::spawn(preview::svg::warm_fontdb);
        if cfg.ui.mermaid != "text" {
            std::thread::spawn(preview::markdown::warm_mermaid);
        }
    }

    // Channel and worker thread for offloading resize/encode.
    let (req_tx, req_rx) = unbounded_channel::<ResizeRequest>();
    let (resp_tx, resp_rx) = unbounded_channel::<ResizeResult>();
    let worker = std::thread::spawn(move || resize_worker(req_rx, resp_tx));

    // Read heavy media (SVG rasterization / full-frame GIF decode) on a separate thread, and send
    // the result to the run loop.
    let (media_tx, media_rx) = std::sync::mpsc::channel::<MediaResult>();
    // Do a kitty image's resize+compress (zoom/pan) on a separate thread, and send the result to
    // the run loop (the first one is synchronous).
    let (kitty_tx, kitty_rx) = std::sync::mpsc::channel::<KittyResult>();
    // Decode inline Markdown images on a separate thread, and send the result to the run loop.
    let (md_img_tx, md_img_rx) = std::sync::mpsc::channel::<MdImageResult>();
    // Notify the run loop when a remote (http(s)) Markdown image's curl download completes.
    let (md_remote_tx, md_remote_rx) = std::sync::mpsc::channel::<RemoteFetch>();
    // Offload inline Markdown image encoding (resize + protocol) to a dedicated worker (doesn't
    // block the UI).
    let (md_enc_tx, md_enc_worker_rx) = std::sync::mpsc::channel::<MdEncodeRequest>();
    let (md_enc_res_tx, md_enc_res_rx) = std::sync::mpsc::channel::<MdEncodeResult>();
    // Compute the heavy git ignored (the ignore set — ~800ms on a large repo) on a separate
    // thread, and send the result to the run loop.
    let (ignored_tx, ignored_rx) = std::sync::mpsc::channel::<IgnoredResult>();
    // A full `git status` (a whole-worktree scan) also goes to a separate thread. Even on a small
    // repo it's ~5ms, and on a large repo it can take hundreds of ms, stalling the UI on every tab
    // switch/directory move.
    let (status_tx, status_rx) = std::sync::mpsc::channel::<StatusResult>();
    // Long-running file operations (copy/move/duplicate/delete) also go to a separate thread.
    // Pasting/deleting a large directory used to freeze input/rendering (design principle #4).
    let (fileop_tx, fileop_rx) = std::sync::mpsc::channel::<FileOpResult>();
    // Git **writes** (stage/unstage/discard/commit/checkout/branch/worktree) too. `git` has no
    // bounded runtime — a pre-commit hook, a network mount, or another process holding
    // `.git/index.lock` can each stall it — and running one on the UI thread froze konoma
    // (design principle #4). The read side (status/ignored) was already off-thread above.
    let (gitop_tx, gitop_rx) = std::sync::mpsc::channel::<GitOpResult>();
    // The `/` filter's population walk too. 50,000 entries cost ~45ms in `readdir` alone, so a big
    // tree cannot be walked inside the <60ms draw budget; the scan starts synchronously and hands
    // over only what didn't fit (see `App::start_filter`).
    let (pool_tx, pool_rx) = std::sync::mpsc::channel::<FilterPoolResult>();

    let start_dir = dir.clone();
    let mut app = App::new(dir, cfg)?;
    app.attach_media_loader(media_tx);
    app.attach_kitty_loader(kitty_tx);
    app.attach_md_image_loader(md_img_tx);
    app.attach_remote_md_loader(md_remote_tx);
    app.attach_git_loader(ignored_tx);
    app.attach_status_loader(status_tx);
    app.attach_fileop_runner(fileop_tx);
    app.attach_gitop_runner(gitop_tx);
    app.attach_filter_pool_loader(pool_tx);
    // Report a config load error + keymap conflicts/ignored settings via a startup message
    // (silently falling back to the default would go unnoticed). If both are present, combine
    // them into one line.
    let km_report = app.keymap_report();
    app.flash = match (cfg_err, km_report) {
        (Some(a), Some(b)) => Some(format!("{a} / {b}")),
        (Some(a), None) => Some(a),
        (None, b) => b,
    };
    // Inline image encode worker: launched only when there is a backend (clones the Picker to pass in).
    // Dropping App makes md_enc_tx disappear, and the worker's recv returns Err, terminating it cleanly.
    if let Some(pk) = picker.clone() {
        std::thread::spawn(move || app::md_encode_worker(pk, md_enc_worker_rx, md_enc_res_tx));
        app.attach_md_encoder(md_enc_tx);
    }
    if let Some(picker) = picker {
        // Pass tx to App (each image's ThreadProtocol clones and uses it).
        app.attach_image_backend(picker, req_tx);
    }
    // Note: don't keep a clone of req_tx on the main side. Dropping App makes every Sender
    // disappear, so the worker's recv returns None and can terminate cleanly.

    // Tab session ([ui] restore_tabs): restore the tab layout that was open in this start
    // directory last time.
    // Done after wiring up each loader/image backend (restoring re-opens previews, which fires
    // media jobs).
    app.attach_session_store(session::SessionStore::load(&start_dir));
    app.restore_session();

    let result = run(
        &mut terminal,
        &mut app,
        resp_rx,
        WorkerRx {
            media: media_rx,
            kitty: kitty_rx,
            md_img: md_img_rx,
            md_remote: md_remote_rx,
            md_enc: md_enc_res_rx,
            ignored: ignored_rx,
            status: status_rx,
            fileop: fileop_rx,
            gitop: gitop_rx,
            pool: pool_rx,
        },
    );

    // Save the tab session on exit (the state at exit time is the final form. no-op when restore_tabs=false).
    app.save_session();

    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();

    // Fold up App, dropping every Sender, so the workers terminate.
    app.detach_image_backend();
    drop(app);
    let _ = worker.join();

    result
}

/// Handle resize requests on a current-thread tokio runtime on a dedicated thread.
/// `resize_encode()` is CPU-heavy, but running it on this dedicated thread keeps the UI thread unblocked.
fn resize_worker(mut rx: UnboundedReceiver<ResizeRequest>, tx: UnboundedSender<ResizeResult>) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
        return;
    };
    rt.block_on(async move {
        while let Some(req) = rx.recv().await {
            // 1 request = 1 encode. Return completion to the UI (the UI applies it via try_recv).
            if tx.send(req.resize_encode()).is_err() {
                break; // the UI side has terminated
            }
        }
    });
}

/// Classify the paths from an fs event. Returns `(meaningful, ignore_rules_changed)`:
/// - `meaningful`: whether reloading is worthwhile. Any non-empty event is meaningful — **including
///   `.git/*.lock` churn from an external git operation**. konoma's own git reads take no locks (CLI
///   reads pass `--no-optional-locks`; git2 diffs never set `update_index`), so a `.git` lock event can
///   only come from an *external* git op (a commit/amend/reset by the user or an AI agent) whose result
///   we must reflect. (Historically we swallowed lock-only events to break a self-feedback loop in which
///   konoma's own `git status` created `.git/index.lock` → the watcher picked it up → git ran again.
///   `--no-optional-locks` (2026-07-07) removed that loop at the source, so swallowing now only *hides*
///   external commits — e.g. the stale change marker seen after an agent commits while you preview a file.)
/// - `ignore_rules_changed`: whether `.gitignore` or `.git/info/exclude` changed (= the heavy
///   ignore set must be rebuilt). For other changes, `ignored` is left alone and only the cheap `statuses` is updated.
fn classify_fs_paths(paths: &[PathBuf]) -> (bool, bool) {
    if paths.is_empty() {
        return (true, false); // An event with an unknown path errs safe (reload, don't touch ignored)
    }
    // Any path change is worth reloading (the run loop coalesces a burst into one). Only request
    // rebuilding the heavy ignored set when an event that changed the ignore rules is included.
    let ignore_rules = paths.iter().any(|p| is_ignore_rule_file(p));
    (true, ignore_rules)
}

/// Whether a raw notify `EventKind` reflects an actual content change and is worth reacting to at
/// all — checked **before** `classify_fs_paths` even looks at the paths. notify's inotify backend
/// (Linux) subscribes to `IN_OPEN`/`IN_ACCESS`, so merely *opening or reading* a file emits
/// `EventKind::Access(..)` — no write happened. Left unfiltered this breaks two things on Linux:
/// follow mode (`F`) jumping to files that were only `cat`, and a self-feeding loop where konoma's
/// own `git status` opens tracked files, the reads get reported back as "changes", and that refresh
/// runs `git status` again forever (the busy "git scan" spinner never stops). macOS's FSEvents
/// backend never reports reads at all, so this predicate is a no-op there — `Access` events simply
/// don't occur, and every branch below `Access` (`Any`/`Create`/`Modify`/`Remove`/`Other`) still
/// returns `true`, keeping macOS behaviour byte-identical.
///
/// The one `Access` case that *is* a real change: `Close(Write)` (inotify's `IN_CLOSE_WRITE`) fires
/// after a file that was opened for writing is closed — i.e. a write actually happened and is now
/// flushed. Every other `Access` variant (opens, plain reads, close-without-write) is a no-op read.
fn is_content_event(kind: &notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode};
    match kind {
        notify::EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        notify::EventKind::Access(_) => false,
        _ => true,
    }
}

/// Whether an fs event changes **which entries exist** (a create, a removal, or a rename) rather
/// than only the contents of an entry that already existed. Feeds `FsBurstKinds::structural`, which
/// the build-churn guard uses to decide what it may skip.
///
/// The distinction is the whole point of that guard: a write changes what a row *says*, so skipping
/// it leaves a stale number on screen and the next event fixes it. A create/removal/rename changes
/// whether the row is *there at all*, and skipping that leaves the tree listing files that no
/// longer exist — a listing that is not merely stale but false, and that nothing later repairs
/// (see `App::fs_burst_is_build_churn`).
///
/// Only two shapes are classified as content-only, and both say "an entry that exists had its bytes
/// rewritten": `Modify(Data(..))` (macOS `ITEM_MODIFIED`, Linux `IN_MODIFY`) and `Access(Close(Write))`
/// (Linux `IN_CLOSE_WRITE`). `Modify(Metadata(..))` joins them because metadata applies *to an entry
/// that exists* — it can never add or remove a row — and excluding it matters in practice: macOS
/// raises `ITEM_INODE_META_MOD` alongside `ITEM_MODIFIED` on an ordinary write, so treating metadata
/// as structural would defeat the guard for exactly the plain build writes it exists to absorb.
///
/// Everything else — including the deliberately vague `EventKind::Any`/`Other` and `Modify(Any/Other)`
/// that coarse backends emit — counts as structural, i.e. err towards refreshing. `Access` variants
/// other than `Close(Write)` never reach here (`is_content_event` drops them first); classifying
/// them as structural is the harmless safe side if that ever changes.
fn is_structural_event(kind: &notify::EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    !matches!(
        kind,
        notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_))
            | notify::EventKind::Access(AccessKind::Close(AccessMode::Write))
    )
}

/// Hard cap on how many changed paths one filesystem burst reports individually.
/// Beyond this the burst is treated as "paths unknown", which the caller handles conservatively
/// (a full refresh). A burst that large is a build or a bulk copy, where per-path information is
/// worthless anyway — and collecting it used to cost O(N²).
const MAX_BURST_PATHS: usize = 1024;

/// Accumulates the distinct paths of one filesystem-event burst, in arrival order, with a hard cap.
///
/// De-duplication is a hash lookup, not a linear scan: an agent creating tens of thousands of files
/// produced one burst that took minutes of 100% CPU to de-duplicate with `Vec::contains`, freezing
/// the UI. Once the cap is passed the collected list is discarded and `finish` reports `None`,
/// meaning "something changed but we don't know what" — the caller's safe path.
#[derive(Default)]
struct BurstPaths {
    seen: std::collections::HashSet<PathBuf>,
    paths: Vec<PathBuf>,
    overflowed: bool,
}

impl BurstPaths {
    /// Record one path. Returns true when it is newly seen (the caller uses that to avoid repeating
    /// per-path work). Returns false once the cap is reached (whether or not `p` was already seen).
    fn push(&mut self, p: &Path) -> bool {
        if self.overflowed {
            return false;
        }
        if self.seen.contains(p) {
            return false;
        }
        if self.paths.len() >= MAX_BURST_PATHS {
            self.overflowed = true;
            self.seen.clear();
            self.paths.clear();
            return false;
        }
        let owned = p.to_path_buf();
        self.seen.insert(owned.clone());
        self.paths.push(owned);
        true
    }

    /// The distinct paths in arrival order, or `None` when the burst overflowed the cap.
    fn finish(self) -> Option<Vec<PathBuf>> {
        if self.overflowed {
            None
        } else {
            Some(self.paths)
        }
    }
}

/// Paths from an fs event that follow mode may jump to: anything not under a `.git` directory
/// (repository internals — index/refs/lock churn — are never review targets). Existence/kind/root
/// checks happen later in `App::follow_jump` (the event may be a deletion, or outside the tree).
fn follow_candidates(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|p| !p.components().any(|c| c.as_os_str() == ".git"))
        .cloned()
        .collect()
}

/// Whether `p` is a file that holds ignore rules (a change requires recomputing the ignore set):
/// the `.gitignore` itself, or `.git/info/exclude`. A `.gitignore` under `node_modules` is excluded
/// because it sits inside a wholly-ignored tree and does not affect git's decision (avoiding a wasteful recompute).
fn is_ignore_rule_file(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        ".gitignore" => !p.components().any(|c| c.as_os_str() == "node_modules"),
        "exclude" => {
            // .git/info/exclude (parent=info, grandparent=.git)
            p.parent()
                .and_then(|pp| pp.file_name())
                .and_then(|n| n.to_str())
                == Some("info")
                && p.parent()
                    .and_then(|pp| pp.parent())
                    .and_then(|g| g.file_name())
                    .and_then(|n| n.to_str())
                    == Some(".git")
        }
        _ => false,
    }
}

/// Background-worker result receivers drained each iteration of the run loop.
struct WorkerRx {
    media: std::sync::mpsc::Receiver<MediaResult>,
    kitty: std::sync::mpsc::Receiver<KittyResult>,
    md_img: std::sync::mpsc::Receiver<MdImageResult>,
    md_remote: std::sync::mpsc::Receiver<RemoteFetch>,
    md_enc: std::sync::mpsc::Receiver<MdEncodeResult>,
    ignored: std::sync::mpsc::Receiver<IgnoredResult>,
    status: std::sync::mpsc::Receiver<StatusResult>,
    fileop: std::sync::mpsc::Receiver<FileOpResult>,
    gitop: std::sync::mpsc::Receiver<GitOpResult>,
    pool: std::sync::mpsc::Receiver<FilterPoolResult>,
}

/// How long the run loop may block waiting for input before it comes back round to apply worker
/// results and advance animations.
///
/// A separate function so the rule is testable: "does a background job shorten the wait" is a
/// structural property, and asserting it by timing the loop would be a flaky way to ask a question
/// that has an exact answer.
///
/// While anything is running on another thread the loop wakes often, so its result is applied as
/// soon as it lands rather than up to a whole idle timeout later. The filter-pool walk belongs in
/// that list for the reason it exists at all — the results are supposed to fill in *while the user
/// types*, and a 100ms wait on a result already sitting in the channel works against exactly that.
/// Otherwise: by the next GIF frame's deadline if one is playing (whichever of the full-screen and
/// the Markdown-inline animations is sooner), and by a plain idle timeout when nothing is going on.
fn poll_timeout(app: &App) -> Duration {
    if app.is_media_loading()
        || app.md_images_loading()
        || app.kitty_build_pending()
        || app.filter_pool_scan_in_flight()
    {
        return Duration::from_millis(16);
    }
    app.gif_poll_timeout()
        .into_iter()
        .chain(app.md_gif_poll_timeout())
        .min()
        .unwrap_or(Duration::from_millis(100))
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut resp_rx: UnboundedReceiver<ResizeResult>,
    rx: WorkerRx,
) -> Result<()> {
    // File watching: auto-update the tree/git status on a change under the current root.
    // notify's callback is called from a separate thread, so send changes to the run loop over a
    // std channel.
    // The channel's FsBurstKinds = what this event did beyond touching paths: whether an ignore
    // rule (.gitignore / .git/info/exclude) changed, and whether it changed *which* entries exist
    // (create/remove/rename) rather than only their contents. The run loop ORs these across the
    // burst; both halves force a refresh (see App::fs_burst_is_build_churn).
    // `.git/*.lock` churn from an external git operation is also a reload target
    // (classify_fs_paths — konoma is lock-free, so no self-feedback loop occurs).
    // The channel's Vec<PathBuf> = candidate changed paths for follow mode (excluding under .git).
    let (fs_tx, fs_rx) = std::sync::mpsc::channel::<(FsBurstKinds, Vec<PathBuf>)>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            // Events that aren't a content change (Linux inotify's open/read notifications) are
            // filtered out right here (is_content_event). Both watches (recursive root + the
            // non-recursive extra/`.git` watch) share this one callback, so filtering here covers
            // every path.
            if !is_content_event(&ev.kind) {
                return;
            }
            let (meaningful, ignore_rules) = classify_fs_paths(&ev.paths);
            if meaningful {
                // The event kind is read here, at the only place it exists — `ev` is dropped right
                // after, so anything the skip decision needs must be carried on the channel.
                let kinds = FsBurstKinds {
                    ignore_rules_changed: ignore_rules,
                    structural: is_structural_event(&ev.kind),
                };
                let _ = fs_tx.send((kinds, follow_candidates(&ev.paths)));
            }
        }
    })
    .ok();
    let mut watched_root: Option<PathBuf> = None;
    // `attempted_*` tracks the last target we *tried* (success or failure), separately from
    // `watched_*` (which only reflects a *successful* watch) — see `watch_target_changed`.
    let mut attempted_root: Option<PathBuf> = Some(app.tab.root.clone());
    if rewatch(watcher.as_mut(), &mut watched_root, &app.tab.root) == WatchOutcome::Failed {
        app.flash = Some(watch_failed_flash(app.lang, &app.tab.root));
    }
    // Secondary watch: a file shown outside the tree root (a global-bookmark preview, or the repo-wide
    // git view when the root is a repo subdirectory) is not covered by the recursive root watch, so its
    // external/agent edits would never refresh. Watch its directory too (see `App::out_of_root_watch_dir`).
    let mut watched_extra: Option<PathBuf> = None;
    let mut attempted_extra: Option<PathBuf> = None;
    // When root is a subdirectory of a repo — or root is a *linked worktree* (`git worktree add`),
    // whose own `HEAD`/`index` live under the main repo's `.git/worktrees/<name>/`, entirely outside
    // the worktree's own checkout — the git directory falls outside the recursive watch. Watch it
    // non-recursively so external git operations (a commit that doesn't touch working files / an
    // external checkout) are picked up (`App::git_dir_watch`). When root *is* the repo root, this is
    // already covered by the recursive watch, so it's None.
    let mut watched_git: Option<PathBuf> = None;
    let mut attempted_git: Option<PathBuf> = None;

    // Loading: warm the code grammar in the background and notify the run loop of completion over
    // this channel.
    // Both indicator/progressive keep the UI unblocked (the spinner keeps spinning), and swap in
    // the colored version on completion.
    let (hl_tx, hl_rx) = std::sync::mpsc::channel::<()>();

    // Follow mode's switch debounce: even when an agent rewrites several files fast, view
    // switching (jumping across files) is at least this interval apart = the screen doesn't
    // bounce around and stays glanceable.
    // The most recent pending target (latest-wins) is picked up once the dwell period ends.
    // Reloading the same file is unlimited (a separate path).
    const FOLLOW_MIN_DWELL: Duration = Duration::from_millis(1000);
    let mut pending_follow: Option<PathBuf> = None;
    let mut last_follow_jump: Option<std::time::Instant> = None;

    // fs events produced by an in-progress file operation (a bulk write we caused ourselves)
    // aren't processed on the spot — they're accumulated and applied only once after the
    // operation finishes (should_defer_fs_events below).
    let mut deferred_fs = false;
    let mut deferred_kinds = FsBurstKinds::default();

    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            terminal.draw(|frame| ui::render(frame, app))?;
            needs_redraw = false;
            // On a frame where an inline image's (kitty unicode placeholder) display position
            // moved, do a full redraw of the terminal to clean up the leftover debris at the old
            // position. A placeholder row is a "string that encodes the image ID into the
            // foreground color" printed directly onto the terminal grid, but since ratatui's
            // diff-based rendering doesn't resend blank→blank, when the image moves (via scroll,
            // etc.) the old ID row is left behind as a colored bar (reported on real Ghostty as a
            // blue/pink bar — the color changes each time the ID changes).
            if app.take_md_overlay_moved() {
                terminal.clear()?;
                terminal.draw(|frame| ui::render(frame, app))?;
            }
        }

        // Waiting on heavy code highlighting (the first time for a cold language): offload the
        // grammar compile to a separate thread so the UI isn't blocked.
        // indicator spins a centered spinner, progressive keeps showing plain text, and either
        // swaps in the coloring on completion.
        if let Some((ext, path)) = app.take_warm_job() {
            let tx = hl_tx.clone();
            std::thread::spawn(move || {
                preview::code::warm_file(&ext, &path);
                let _ = tx.send(());
            });
        }

        // Wait for input (with a timeout). Even during the timeout, keep picking up worker results
        // and file changes.
        // A guard against holding a key down: with 1 event = 1 draw, if drawing is heavy, input
        // piles up and "it keeps scrolling even after you release the key." **Process all pending
        // events in a batch, then draw exactly once** (converge to the final state).
        if event::poll(poll_timeout(app))? {
            let mut quit = false;
            loop {
                let ev = event::read()?;
                match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Don't bring down the TUI on a recoverable failure during key handling
                        // (fs/git operations: refresh/rebuild_tree/paste/delete/rename etc.) —
                        // design principle #3, "never crash."
                        // handle_key touches neither terminal control nor drawing nor
                        // initialization, only App state, so every Err returned here is
                        // recoverable. Don't end run() outright with `?`, and don't swallow it
                        // either — show it to the user via flash and keep going (the truly fatal
                        // `?`s for drawing/terminal control stay in the body of run()).
                        let res = handle_key(app, key);
                        if resolve_key_result(app, res) {
                            quit = true;
                            break;
                        }
                        needs_redraw = true;
                    }
                    Event::Resize(_, _) => needs_redraw = true,
                    // A file drag & drop: the terminal delivers the path as a paste → receive it
                    // and route it into copy/move.
                    // But ignore it (don't let it interrupt) while a dialog/modal/overlay is shown (#13).
                    Event::Paste(s) if paste_accepted(app.surface()) => {
                        app.handle_paste(s);
                        needs_redraw = true;
                    }
                    _ => {}
                }
                // Keep processing if another event can be taken immediately (otherwise break out to draw).
                if !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
            if quit {
                break;
            }
        }

        // External editor request (`e`): suspend the TUI and launch synchronously, then restore +
        // reload the preview when it exits.
        // When opened from the preview, open the editor at the currently displayed line (line).
        if let Some((path, line)) = app.take_pending_edit() {
            run_editor(terminal, app, &path, line)?;
            needs_redraw = true;
        }

        // External git tool request (`O`): launch the configured git.tool (default lazygit)
        // synchronously in the repo workdir.
        if app.take_launch_git_tool() {
            run_git_tool(terminal, app)?;
            // The external tool may have changed the working tree/git state → re-fetch the listing,
            // git status, and derived views
            // (symmetric with run_editor's restore calling reload_preview. Prevents the Git view's
            // git_view_entries from going stale — #4).
            let _ = app.refresh();
            needs_redraw = true;
        }

        // Warm-up complete: re-render to swap in the colored version (the grammar is already warm,
        // so it's instant).
        while hl_rx.try_recv().is_ok() {
            app.clear_highlight_pending();
            needs_redraw = true;
        }

        // While the centered spinner is shown (waiting on code highlighting / a separate thread
        // loading SVG or GIF), advance it a frame on every waiting tick. While loading, poll is
        // 16ms, so it spins smoothly.
        if (app.is_highlight_pending() && app.loading_is_indicator())
            || app.is_media_loading()
            || app.busy_indicator_active()
        {
            app.tick_spinner();
            needs_redraw = true;
        }

        // Apply the re-encode result(s) from the worker (all of them, if there are several).
        while let Ok(resp) = resp_rx.try_recv() {
            if app.apply_image_resize(resp) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's media load (SVG/GIF) completion(s) (all of them, if there
        // are several; discard stale generations).
        while let Ok(result) = rx.media.try_recv() {
            if app.apply_media(result) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's kitty image build (zoom/pan resize+compress) completion
        // (only the latest generation is applied).
        while let Ok(result) = rx.kitty.try_recv() {
            if app.apply_kitty(result) {
                needs_redraw = true;
            }
        }

        // Apply the inline Markdown image decode completion(s) (all of them, if there are several).
        while let Ok(result) = rx.md_img.try_recv() {
            if app.apply_md_image(result) {
                needs_redraw = true;
            }
        }

        // Apply a remote Markdown image's download completion (invalidate md_cache and re-layout).
        while let Ok(result) = rx.md_remote.try_recv() {
            if app.apply_remote_fetch(result) {
                needs_redraw = true;
            }
        }

        // Apply the inline Markdown image encode completion(s) (a separate worker; all of them, if
        // there are several).
        while let Ok(result) = rx.md_enc.try_recv() {
            if app.apply_md_encode(result) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's git ignored (ignore set) computation completion(s) (all of
        // them, if there are several; discard stale generations).
        // Re-render since applying it makes the dimmed display (dim for gitignore-excluded
        // entries) appear.
        while let Ok(result) = rx.ignored.try_recv() {
            if app.apply_ignored(result) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's `git status` completion(s) (all of them, if there are
        // several; discard stale generations).
        // Re-render since applying it updates the tree's change marker/branch name.
        while let Ok(result) = rx.status.try_recv() {
            if app.apply_statuses(result) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's file operation (copy/move/duplicate/delete) completion.
        while let Ok(result) = rx.fileop.try_recv() {
            if app.apply_file_op(result) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's git write (stage/commit/checkout/...) completion.
        while let Ok(result) = rx.gitop.try_recv() {
            if app.apply_git_op(result) {
                needs_redraw = true;
            }
        }

        // Apply the separate thread's filter-pool scan completion(s) (discard stale generations).
        // Re-render because applying it re-runs the current query over the now-larger pool = the
        // result list fills in while the user is still typing.
        while let Ok(result) = rx.pool.try_recv() {
            if app.apply_filter_pool(result) {
                needs_redraw = true;
            }
        }

        // GIF animation: once the current frame's display time has elapsed, advance to the next
        // frame (swap image_src → the next render's prepare_image rebuilds → worker re-encodes →
        // applied by the branch above).
        if app.advance_gif_if_due() {
            needs_redraw = true;
        }
        // Inline Markdown GIF: likewise advance each entry independently (swap decoded → the next
        // render's ensure_md_image sees the key mismatch and requests a re-encode → applied by the
        // md_enc branch).
        if app.advance_md_gifs_if_due() {
            needs_redraw = true;
        }

        // Pick up file changes and auto-update (a burst is coalesced into one).
        // If even one event changed an ignore rule (.gitignore / .git/info/exclude), also rebuild
        // the heavy ignored set. Otherwise just update the cheap statuses+branch.
        let mut fs_changed = false;
        let mut burst_kinds = FsBurstKinds::default();
        // The paths that changed in this burst (the watcher side sends them excluding under
        // `.git`). Empty = either `.git` only or unknown (including exceeding the cap), in which
        // case `refresh_fs_changed` errs safe = also reloads the preview.
        // De-duplication is a hash lookup (O(1)) + capped (BurstPaths) = the fix for a bug where
        // Vec::contains's linear scan (O(N²)) froze the UI for minutes when an agent generated a
        // huge number of files (see MAX_BURST_PATHS).
        let mut burst = BurstPaths::default();
        // While one of konoma's own background file operations (copy/move/duplicate/delete) is in
        // progress, don't react to the huge number of watcher events that operation produces
        // (running refresh_fs_changed on every burst for a copy of ~10,000 entries would occupy
        // the run loop, so key input never gets processed and the app looks frozen to the user).
        // Accumulate them and apply only once after the operation finishes
        // (App::should_defer_fs_events — apply_file_op already calls refresh() on completion, so
        // nothing is missed).
        let defer_fs = app.should_defer_fs_events();
        while let Ok((kinds, paths)) = fs_rx.try_recv() {
            if defer_fs {
                deferred_fs = true;
                deferred_kinds.merge(kinds);
                continue;
            }
            fs_changed = true;
            burst_kinds.merge(kinds);
            // Follow mode: record a valid target into the session list (the population n/N reviews)
            // while also setting the last one as the pending target (latest-wins). An invalid path
            // doesn't consume a dwell slot.
            // On a burst that exceeds the cap (overflowed), push always returns false, so from
            // then on this just drains the channel without doing any per-path work (follow bookkeeping).
            for p in paths {
                if burst.push(&p) && app.follow_enabled() && app.follow_note_change(&p) {
                    pending_follow = Some(p);
                }
            }
        }
        let changed_paths = burst.finish().unwrap_or_default();
        // Apply the accumulated burst exactly once, on the first loop after the operation finishes
        // (defer is lifted).
        // changed_paths stays empty = "which paths changed is unknown" = err safe (also reload the preview).
        if !defer_fs && deferred_fs {
            deferred_fs = false;
            fs_changed = true;
            burst_kinds.merge(deferred_kinds);
            deferred_kinds = FsBurstKinds::default();
        }
        // Build-churn guard: when every path in the burst is gitignored, the ignore rules
        // themselves haven't changed, and nothing was created/removed/renamed, skip the whole tree
        // rebuild that a write to target/ or node_modules/ alone would otherwise trigger (wasted
        // work). Writes are all it may skip — see `App::fs_burst_is_build_churn`.
        if fs_changed && !app.fs_burst_is_build_churn(&changed_paths, burst_kinds) {
            // In addition to the listing + git status, refresh_fs() also uniformly re-fetches the
            // active derived view
            // (in Preview mode, reload the current preview → fixes the known bug where the preview
            //  stayed stale after an external-editor edit. In the Git view, update the change listing).
            app.refresh_fs_watched(burst_kinds.ignore_rules_changed, &changed_paths);
            needs_redraw = true;
        }
        // Follow's switch judgment (outside the drain = every loop). A target arriving during
        // dwell is held pending, and it jumps once, to the latest at the moment dwell ends. A
        // change to the file already shown doesn't need a switch (the refresh_fs reload above
        // already keeps up).
        if pending_follow.is_some() && !app.follow_enabled() {
            pending_follow = None; // once disabled, discard the pending target too
        }
        if let Some(p) = pending_follow.clone() {
            if app.tab.preview_path.as_deref() == Some(p.as_path()) {
                pending_follow = None;
            } else if last_follow_jump.is_none_or(|t| t.elapsed() >= FOLLOW_MIN_DWELL) {
                pending_follow = None;
                app.follow_jump(&p);
                last_follow_jump = Some(std::time::Instant::now());
                needs_redraw = true;
            }
        }

        // Re-point the watch target when root changes (h/l, tab switching, etc.). Gated on
        // `attempted_root`, not `watched_root` — a failed watch() (e.g. the Linux inotify
        // watch-descriptor limit) must not be retried on every idle poll tick; see
        // `watch_target_changed`.
        if watch_target_changed(attempted_root.as_deref(), Some(app.tab.root.as_path())) {
            attempted_root = Some(app.tab.root.clone());
            if rewatch(watcher.as_mut(), &mut watched_root, &app.tab.root) == WatchOutcome::Failed {
                app.flash = Some(watch_failed_flash(app.lang, &app.tab.root));
                needs_redraw = true;
            }
        }
        // For a file shown outside root (a bookmark target / the repo-wide git view), also watch
        // its parent directory.
        // Becomes None — and the watch is dropped — once the displayed file changes / it returns
        // inside root / it returns to the tree.
        let want_extra = app.out_of_root_watch_dir();
        if watch_target_changed(attempted_extra.as_deref(), want_extra.as_deref()) {
            attempted_extra = want_extra.clone();
            if set_extra_watch(watcher.as_mut(), &mut watched_extra, want_extra.as_deref())
                == WatchOutcome::Failed
            {
                if let Some(dir) = want_extra.as_deref() {
                    app.flash = Some(watch_failed_flash(app.lang, dir));
                    needs_redraw = true;
                }
            }
        }
        // When root is a subdirectory of a repo, or is a linked worktree whose own `HEAD`/`index`
        // live under the main repo's `.git/worktrees/<name>/`, watch that git directory
        // non-recursively to catch external git operations.
        let want_git = app.git_dir_watch();
        if watch_target_changed(attempted_git.as_deref(), want_git.as_deref()) {
            attempted_git = want_git.clone();
            if set_extra_watch(watcher.as_mut(), &mut watched_git, want_git.as_deref())
                == WatchOutcome::Failed
            {
                if let Some(dir) = want_git.as_deref() {
                    app.flash = Some(watch_failed_flash(app.lang, dir));
                    needs_redraw = true;
                }
            }
        }
    }
    Ok(())
}

/// Outcome of a single (re)watch attempt — see `rewatch`, `set_extra_watch`, and
/// `watch_target_changed` (the caller-side retry gate these outcomes feed into).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchOutcome {
    /// No `notify::RecommendedWatcher` is available at all (construction failed at startup) —
    /// unrelated to the per-target limit below; pre-existing, silent, unchanged by this fix.
    NoWatcher,
    /// The watch was removed and nothing new was requested (`want == None`).
    Removed,
    /// `watch()` succeeded.
    Watching,
    /// `watch()` failed. Most commonly on Linux: the per-user inotify watch-descriptor limit
    /// (`fs.inotify.max_user_watches`, shared across *every* process the user runs, and consumed
    /// one-per-directory by `RecursiveMode::Recursive` — this repo alone has 17k+ directories
    /// under it, `target/` accounting for most). Auto-refresh for this target silently stops
    /// working. The caller flashes this **once** (not every idle poll tick — see
    /// `watch_target_changed`), since Agent Watch/live git-status updates dying with no
    /// indication at all would be the worst possible failure mode for konoma's signature feature.
    Failed,
}

/// Whether the watch target changed since the last **attempt** (success or failure) — the gate
/// used before calling `rewatch`/`set_extra_watch` again, checked on every run() loop tick.
/// Comparing against the last **successful** target (what run() did before this fix — `watched`,
/// which a failed `watch()` never updates) means a failure is retried on every idle poll tick
/// (~100ms, no backoff) forever: a busy loop that never gives up, hammering the same syscall that
/// just failed (worse than doing nothing when the failure is the shared per-user inotify
/// watch-descriptor limit — see `WatchOutcome::Failed` — since concurrent retries from this
/// process compete with every other process's chance to get a watch). Comparing against the last
/// **attempted** target (set unconditionally, before the syscall) fixes that: the next retry
/// happens only once the caller actually asks for a different target (root change via `h`/`l`,
/// tab switch, a file shown outside root changing, etc.).
fn watch_target_changed(
    attempted: Option<&std::path::Path>,
    target: Option<&std::path::Path>,
) -> bool {
    attempted != target
}

/// Re-point the watcher at `root`. See `WatchOutcome::Failed` for what a failure means and why
/// the caller must gate retries with `watch_target_changed` instead of calling this every tick.
fn rewatch(
    watcher: Option<&mut notify::RecommendedWatcher>,
    watched: &mut Option<PathBuf>,
    root: &std::path::Path,
) -> WatchOutcome {
    use notify::{RecursiveMode, Watcher};
    let Some(w) = watcher else {
        return WatchOutcome::NoWatcher;
    };
    if let Some(old) = watched.take() {
        let _ = w.unwatch(&old);
    }
    if w.watch(root, RecursiveMode::Recursive).is_ok() {
        *watched = Some(root.to_path_buf());
        WatchOutcome::Watching
    } else {
        WatchOutcome::Failed
    }
}

/// Add/replace/remove the **secondary, non-recursive** watch for a file shown outside the tree root
/// (`App::out_of_root_watch_dir`). `want=None` removes it. Non-recursive so it never overlaps the
/// recursive root watch (no duplicate events). See `WatchOutcome::Failed` for what a failure means
/// and why the caller must gate retries with `watch_target_changed` instead of calling this every tick.
fn set_extra_watch(
    watcher: Option<&mut notify::RecommendedWatcher>,
    watched: &mut Option<PathBuf>,
    want: Option<&std::path::Path>,
) -> WatchOutcome {
    use notify::{RecursiveMode, Watcher};
    let Some(w) = watcher else {
        return WatchOutcome::NoWatcher;
    };
    if let Some(old) = watched.take() {
        let _ = w.unwatch(&old);
    }
    let Some(dir) = want else {
        return WatchOutcome::Removed;
    };
    if w.watch(dir, RecursiveMode::NonRecursive).is_ok() {
        *watched = Some(dir.to_path_buf());
        WatchOutcome::Watching
    } else {
        WatchOutcome::Failed
    }
}

/// One-shot flash text for `WatchOutcome::Failed`. The prefix goes through the `Msg` catalogue like
/// every other user-visible string; the path is appended the way `CopiedPrefix` appends its value.
fn watch_failed_flash(lang: crate::i18n::Lang, path: &std::path::Path) -> String {
    format!(
        "{}{}",
        crate::i18n::tr(lang, crate::i18n::Msg::WatchFailedPrefix),
        path.display()
    )
}

/// Edit `path` in an external editor. Temporarily suspends the TUI (drops raw/alt screen), runs the
/// editor **synchronously**, then restores the screen on return. The command is resolved from the
/// `[editor]` config (per-extension → default → $VISUAL → $EDITOR → vim). Both terminal-grabbing TUI
/// editors (vim etc.) and GUI editors (`code -w`) take the same path (both block and wait).
///
/// On return, it **explicitly resets the terminal input modes an editor (especially vim) may leave
/// enabled** (kitty keyboard protocol / bracketed paste / mouse reporting), and further **drains and
/// discards leftover input bytes (terminal-query responses, etc.)** to recover input-decoder sync.
/// Without this, keys can become garbled or stop working after return (ratatui::init assumes legacy
/// input, so we realign the state).
fn run_editor(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    path: &std::path::Path,
    line: Option<usize>,
) -> Result<()> {
    let argv = app.cfg.editor.resolve(path, line);
    let Some((prog, args)) = argv.split_first() else {
        return Ok(());
    };
    let cwd = path.parent().unwrap_or(path).to_path_buf();
    let status = run_external(terminal, app, prog, args, &cwd)?;
    match status {
        Ok(_) => app.reload_preview(), // apply the rewritten result (drop the cache + re-open the window)
        Err(e) => {
            app.flash = Some(format!(
                "{}{e}",
                i18n::tr(app.lang, crate::i18n::Msg::EditorFailed)
            ))
        }
    }
    Ok(())
}

/// Launch the configured `git.tool` (default lazygit) synchronously in the repo workdir. The command
/// is split on whitespace into prog + args. The cwd is the workdir discovered from app.tab.root via git
/// (or root if none). A launch failure (not installed, etc.) is reported as an error via flash.
fn run_git_tool(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    // `!` launches the tool for whichever system owns the directory: lazygit knows nothing about a
    // jj repository, and running it there would show a repository the user is not working in.
    let (tmpl, fallback) = match app.git_vcs {
        crate::vcs::VcsKind::Git => (app.cfg.git.tool.trim(), "lazygit"),
        #[cfg(feature = "git")]
        crate::vcs::VcsKind::Jj => (app.cfg.jj.tool.trim(), "lazyjj"),
    };
    let tmpl = if tmpl.is_empty() { fallback } else { tmpl };
    let mut parts = tmpl.split_whitespace().map(|s| s.to_string());
    let Some(prog) = parts.next() else {
        return Ok(());
    };
    let args: Vec<String> = parts.collect();
    let cwd = git_workdir(&app.tab.root);
    let status = run_external(terminal, app, &prog, &args, &cwd)?;
    if let Err(e) = status {
        app.flash = Some(format!(
            "{}{prog}: {e}",
            i18n::tr(
                app.lang,
                match app.git_vcs {
                    crate::vcs::VcsKind::Git => crate::i18n::Msg::GitToolFailed,
                    #[cfg(feature = "git")]
                    crate::vcs::VcsKind::Jj => crate::i18n::Msg::JjToolFailed,
                }
            )
        ));
    }
    Ok(())
}

/// Return the workdir of the repo containing `root` (when the git feature is enabled). Returns root
/// itself if none is found. Goes through `git::workdir` rather than calling libgit2 here, so the
/// external git tool starts in the working-tree root even in a repository libgit2 cannot open
/// (reftable — see the discovery section at the top of `git.rs`).
#[cfg(feature = "git")]
fn git_workdir(root: &std::path::Path) -> PathBuf {
    git::workdir(root).unwrap_or_else(|| root.to_path_buf())
}

#[cfg(not(feature = "git"))]
fn git_workdir(root: &std::path::Path) -> PathBuf {
    root.to_path_buf()
}

/// Generic helper that launches an arbitrary external command **synchronously** in `cwd`. Suspends the
/// TUI → launches → restores, returning the process exit status (an io::Error on launch failure). Shared
/// by the editor and git tool. See `run_editor` for suspend/restore details (alternate screen, input-mode reset, draining leftover bytes).
fn run_external(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    prog: &str,
    args: &[String],
    cwd: &std::path::Path,
) -> Result<std::io::Result<std::process::ExitStatus>> {
    use crossterm::event::{DisableMouseCapture, PopKeyboardEnhancementFlags};
    use crossterm::execute;
    use crossterm::terminal::{
        disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
        EnterAlternateScreen,
    };

    // --- Suspend the TUI ---
    // **Don't leave** the alternate screen (only disable raw mode). vim etc. enter the alternate
    // screen themselves (smcup), but since the terminal is already alt, no screen switch occurs,
    // preventing the plain terminal screen from flashing briefly at launch
    // (a common Ratatui-forum idiom). Since vim always returns to the primary screen with rmcup on
    // exit, re-enter via EnterAlternateScreen on the restore side.
    disable_raw_mode()?;

    // --- Run the external command synchronously (inherit stdio and block) ---
    let status = std::process::Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status();

    // --- Restore the TUI ---
    // Redraw immediately right after returning to the alternate screen, and further make the
    // "switch + redraw" appear atomically via synchronized output (DEC 2026).
    // This eliminates the plain-terminal/blank-screen flash between the external tool exiting and
    // konoma restoring (supported terminals only.
    // On unsupported terminals, ?2026 is ignored and only the immediate redraw takes effect =
    // harmless). Treat clear/draw as best-effort since they're cosmetic,
    // and always send EndSynchronizedUpdate even on error (so the terminal doesn't get stuck
    // waiting for the sync to end).
    enable_raw_mode()?;
    let _ = execute!(std::io::stdout(), BeginSynchronizedUpdate);
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let _ = terminal.clear(); // discard ratatui's previous buffer to force a full redraw (clears the external tool's leftovers)
    let _ = terminal.draw(|frame| ui::render(frame, app)); // draw immediately right after restoring (don't delay it until the next loop)
    let _ = execute!(std::io::stdout(), EndSynchronizedUpdate);

    // Legacy-izing the input mode + draining leftover bytes can happen after the screen is
    // restored (invisible).
    // Pop the kitty keyboard protocol and disable mouse reporting (left by the external tool).
    // konoma itself uses bracketed paste (to receive D&D), so **re-enable it** after cleaning up.
    let _ = execute!(
        std::io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapture,
        crossterm::event::EnableBracketedPaste,
    );
    // Drain and discard leftover input (terminal-query responses, extra keys) for a short time to
    // recover the decoder's sync.
    while event::poll(Duration::from_millis(10)).unwrap_or(false) {
        let _ = event::read();
    }
    Ok(status)
}

/// File operations from the Visual surface (Space→d/r/c/x …) first commit the range to the selection before running.
/// This absorbs, via the keymap, the old direct keys' (D/R/Y/X) behavior of calling `exit_visual_commit()` before acting.
fn commit_visual_if_needed(app: &mut App, sfc: Surface) {
    if sfc == Surface::Visual {
        app.exit_visual_commit();
    }
}

/// Whether the surface may accept a D&D paste (a file path the terminal delivers via Event::Paste).
/// While a dialog / confirm modal / any overlay (Help/Sort/Bookmarks/Info/Git/Visual …) is shown it returns
/// `false` to prevent interference. Text-input surfaces pass it through to capture as text, and only the basic
/// full-screen surfaces (Tree / Preview), where a drop is meaningful, accept it (actual drop handling is Tree-only inside `App::handle_paste`).
fn paste_accepted(sfc: Surface) -> bool {
    sfc.is_text_input()
        || matches!(
            sfc,
            Surface::Tree | Surface::PreviewText | Surface::PreviewImage
        )
}

/// Map `SortSet(key)` to the key char used to apply it through the existing `sort_menu_key`.
fn sort_key_char(k: SortKey) -> char {
    match k {
        SortKey::Name => 'n',
        SortKey::Size => 's',
        SortKey::Modified => 'm',
        SortKey::Ext => 'e',
    }
}

/// Map `Navigate(Motion)` to a concrete move/scroll **per surface** (§1.1).
/// A missing arm means j/k do nothing on that surface, so this interpretation table is pinned in tandem with the keymap tests.
fn dispatch_navigate(app: &mut App, sfc: Surface, m: Motion) {
    match sfc {
        Surface::Tree | Surface::Visual => match m {
            Motion::Up => app.tree_prev(),
            Motion::Down => app.tree_next(),
            Motion::Top => app.tree_first(),
            Motion::Bottom => app.tree_last(),
            Motion::PageUp => app.tree_page(-1),
            Motion::PageDown => app.tree_page(1),
            Motion::HalfUp => app.tree_half_page(-1),
            Motion::HalfDown => app.tree_half_page(1),
            Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd => {}
        },
        // The normal text/code preview, and its line-selection (visual) submode.
        // When windowed, preview_scroll/to_top etc. move the line cursor (the range grows while
        // visually selecting).
        // While the focused inline mermaid diagram is **zoomed**, assign hjkl/arrows to panning
        // the diagram instead
        // (returning to 1x restores normal scrolling — the feel of an embedded map).
        Surface::PreviewText | Surface::PreviewTextVisual if app.fence_pan_motion(m) => {}
        Surface::PreviewText | Surface::PreviewTextVisual => match m {
            Motion::Up => app.preview_scroll(-1),
            Motion::Down => app.preview_scroll(1),
            Motion::Top => app.preview_to_top(),
            Motion::Bottom => app.preview_to_bottom(),
            Motion::PageUp => app.preview_page(-1),
            Motion::PageDown => app.preview_page(1),
            Motion::HalfUp => app.preview_half_page(-1),
            Motion::HalfDown => app.preview_half_page(1),
            Motion::Left => app.preview_col_move(-1),
            Motion::Right => app.preview_col_move(1),
            Motion::LineHome => app.preview_col_home(),
            Motion::LineEnd => app.preview_col_end(),
        },
        Surface::PreviewImage => match m {
            Motion::Up => app.image_pan(0.0, -1.0),
            Motion::Down => app.image_pan(0.0, 1.0),
            Motion::Left => app.image_pan(-1.0, 0.0),
            Motion::Right => app.image_pan(1.0, 0.0),
            _ => {}
        },
        // Table (csv/tsv): hjkl = move the cell cursor / g·G = first/last row / 0·$ = first/last
        // column / paging.
        Surface::PreviewTable => match m {
            Motion::Up => app.table_cursor_move(-1, 0),
            Motion::Down => app.table_cursor_move(1, 0),
            Motion::Left => app.table_cursor_move(0, -1),
            Motion::Right => app.table_cursor_move(0, 1),
            Motion::Top => app.table_row_to(false),
            Motion::Bottom => app.table_row_to(true),
            Motion::PageUp => app.table_page(-1),
            Motion::PageDown => app.table_page(1),
            Motion::HalfUp => app.table_half_page(-1),
            Motion::HalfDown => app.table_half_page(1),
            Motion::LineHome => app.table_col_to(false),
            Motion::LineEnd => app.table_col_to(true),
        },
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => match m {
            Motion::Up => app.preview_scroll(-1),
            Motion::Down => app.preview_scroll(1),
            Motion::Top => app.preview_to_top(),
            Motion::Bottom => app.preview_to_bottom(),
            Motion::PageUp => app.preview_page(-1),
            Motion::PageDown => app.preview_page(1),
            Motion::HalfUp => app.preview_half_page(-1),
            Motion::HalfDown => app.preview_half_page(1),
            Motion::Left => app.preview_hscroll(-4),
            Motion::Right => app.preview_hscroll(4),
            Motion::LineHome => app.preview_hscroll_home(),
            Motion::LineEnd => app.preview_hscroll_end(),
        },
        #[cfg(feature = "git")]
        Surface::GitDetail => match m {
            Motion::Up => app.git_detail_scroll_by(-1),
            Motion::Down => app.git_detail_scroll_by(1),
            Motion::Top => app.git_detail_scroll_to(false),
            Motion::Bottom => app.git_detail_scroll_to(true),
            Motion::PageUp => app.git_detail_scroll_by(-20),
            Motion::PageDown => app.git_detail_scroll_by(20),
            Motion::HalfUp => app.git_detail_scroll_by(-10),
            Motion::HalfDown => app.git_detail_scroll_by(10),
            Motion::Left => app.git_detail_hscroll_by(-4),
            Motion::Right => app.git_detail_hscroll_by(4),
            Motion::LineHome => app.git_detail_hscroll_home(),
            Motion::LineEnd => app.git_detail_hscroll_end(),
        },
        #[cfg(feature = "git")]
        Surface::GitChanges => match m {
            Motion::Up => app.git_view_move(-1),
            Motion::Down => app.git_view_move(1),
            Motion::Top => app.git_view_move(i32::MIN),
            Motion::Bottom => app.git_view_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitLog => match m {
            Motion::Up => app.git_log_move(-1),
            Motion::Down => app.git_log_move(1),
            Motion::Top => app.git_log_move(i32::MIN),
            Motion::Bottom => app.git_log_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitGraph => match m {
            Motion::Up => app.git_graph_move(-1),
            Motion::Down => app.git_graph_move(1),
            Motion::Top => app.git_graph_move(i32::MIN),
            Motion::Bottom => app.git_graph_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => match m {
            Motion::Up => app.git_graph_picker_move(-1),
            Motion::Down => app.git_graph_picker_move(1),
            Motion::Top => app.git_graph_picker_jump(true),
            Motion::Bottom => app.git_graph_picker_jump(false),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitBranches => match m {
            Motion::Up => app.git_branch_move(-1),
            Motion::Down => app.git_branch_move(1),
            Motion::Top => app.git_branch_move(i32::MIN),
            Motion::Bottom => app.git_branch_move(i32::MAX),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::GitWorktrees => match m {
            Motion::Up => app.git_worktree_move(-1),
            Motion::Down => app.git_worktree_move(1),
            Motion::Top => app.git_worktree_move(i32::MIN),
            Motion::Bottom => app.git_worktree_move(i32::MAX),
            _ => {}
        },
        Surface::Bookmarks => match m {
            // j/k only, matching legacy behavior (g/G/Home/End are unassigned).
            Motion::Up => app.bookmark_list_move(-1),
            Motion::Down => app.bookmark_list_move(1),
            _ => {}
        },
        Surface::Tabs => match m {
            Motion::Up => app.tab_list_move(-1),
            Motion::Down => app.tab_list_move(1),
            _ => {}
        },
        Surface::Outline => match m {
            Motion::Up => app.outline_move(-1),
            Motion::Down => app.outline_move(1),
            Motion::Top => {
                let d = -(app.outline_sel() as i32);
                app.outline_move(d);
            }
            Motion::Bottom => {
                let d = (app.md_outline().len() as i32 - 1) - app.outline_sel() as i32;
                app.outline_move(d);
            }
            _ => {}
        },
        // Table-cell full-text popup: it's likely wrapped long text, so up/down scroll + paging.
        Surface::TableCell => match m {
            Motion::Up => app.table_cell_scroll_by(-1),
            Motion::Down => app.table_cell_scroll_by(1),
            Motion::Top => app.table_cell_scroll_to(false),
            Motion::Bottom => app.table_cell_scroll_to(true),
            Motion::PageUp => app.table_cell_page(-1),
            Motion::PageDown => app.table_cell_page(1),
            _ => {}
        },
        Surface::Help => match m {
            Motion::Up => app.help_scroll_by(-1),
            Motion::Down => app.help_scroll_by(1),
            Motion::Top => app.help_scroll = 0,
            Motion::Bottom => app.help_scroll = u16::MAX,
            _ => {}
        },
        // Sort / Info / fixed text-input surfaces, etc.: Navigate never arrives here (j/k mean
        // something else, or aren't keymap-driven).
        _ => {}
    }
}

/// Central dispatch that runs a resolved `Action`. Only `Quit` requests exit, returning `Ok(true)`.
/// Surface-dependent behavior (motion amount, the q/Esc return target) branches on `sfc` (§5).
fn dispatch_action(app: &mut App, action: Action, sfc: Surface) -> Result<bool> {
    // A backend that only reads gets asked politely and answers here, once, rather than each write
    // discovering it separately and failing in its own way.
    #[cfg(feature = "git")]
    if action.writes_repository() && !crate::vcs::caps(&app.tab.root).write {
        app.flash = Some(crate::i18n::tr(app.lang, crate::i18n::Msg::VcsReadOnly).to_string());
        return Ok(false);
    }
    match action {
        Action::Noop => {}
        Action::Navigate(m) => dispatch_navigate(app, sfc, m),
        Action::TabNew => app.tab_new()?,
        Action::TabClose => app.tab_close(),
        Action::TabPrev => app.tab_cycle(-1),
        Action::TabNext => app.tab_cycle(1),
        Action::TabGoto(i) => app.tab_goto(i as usize),
        Action::ToggleHelp => app.toggle_help(),
        Action::CopyPath(kind) => app.copy_path(kind),
        Action::CopyCodeBlock => app.md_copy_focused_code(),
        Action::PasteJump => {
            // `P` is global (inherited on every surface), so it also arrives from Visual. Like the
            // other seven Visual-originated actions
            // (the commit_visual_if_needed call sites in main.rs), commit the in-progress range
            // selection first before acting. Skipping this means is_visual() doesn't look at
            // tab.mode, so even after paste_jump transitions to the full-screen preview, surface()
            // stays Visual, and while looking at the preview, keys keep flowing into the tree's
            // Visual map — an accident.
            commit_visual_if_needed(app, sfc);
            app.paste_jump();
        }
        Action::Quit => {
            // When confirm_quit is ON, open the confirmation dialog and don't quit yet. When OFF,
            // quit immediately.
            if app.request_quit() {
                return Ok(false);
            }
            return Ok(true);
        }
        Action::CloseTabOrQuit => {
            // If there are multiple tabs, close the current one. If it's the last one, the normal
            // quit flow (same as Q).
            if app.tab_count() > 1 {
                app.tab_close();
            } else if app.request_quit() {
                return Ok(false);
            } else {
                return Ok(true);
            }
        }
        Action::FilterStart => app.start_filter(),
        Action::TreeDescend => app.tree_descend()?,
        Action::TreeActivate => app.tree_activate()?,
        Action::TreeLeave => app.tree_leave()?,
        Action::ToggleHidden => app.toggle_hidden()?,
        Action::ToggleInfo => app.toggle_info(),
        Action::RequestEdit => app.request_edit(),
        Action::OpenGitView => app.open_git_view(),
        Action::Refresh => app.refresh()?,
        Action::CyclePathStyle => app.cycle_path_style(),
        Action::OpenSortMenu => app.open_sort_menu(),
        Action::MarkSet => app.start_mark_set(),
        // `'`: instead of the old invisible "waiting to jump" state, open the bookmark list
        // immediately (which-key style).
        Action::MarkJump => app.open_bookmark_list(),
        Action::SetAnchor => app.reanchor_root(),
        Action::ResetAnchor => app.reset_anchor(),
        Action::OpenGitDiffCursor => app.tree_open_git_diff(),
        Action::EnterVisual => app.enter_visual(),
        Action::ToggleSelect => app.toggle_select(),
        Action::ToggleChangedFilter => app.toggle_changed_filter(),
        Action::JumpNextChange => app.jump_changed(1),
        Action::JumpPrevChange => app.jump_changed(-1),
        Action::ToggleFollow => app.toggle_follow(),
        Action::FileCreate => {
            commit_visual_if_needed(app, sfc);
            app.start_create();
        }
        Action::FileRename => {
            // Via Visual, commit the range first. With a selection, batch (numbered template);
            // without one, a single item.
            commit_visual_if_needed(app, sfc);
            if app.has_selection() {
                app.start_batch_rename();
            } else {
                app.start_rename();
            }
        }
        Action::FileDelete => {
            commit_visual_if_needed(app, sfc);
            app.start_delete();
        }
        Action::FileCopy => {
            commit_visual_if_needed(app, sfc);
            app.copy_selection();
        }
        Action::FileCut => {
            commit_visual_if_needed(app, sfc);
            app.cut_selection();
        }
        Action::FilePaste => {
            commit_visual_if_needed(app, sfc);
            app.paste()?;
        }
        Action::FileDuplicate => {
            commit_visual_if_needed(app, sfc);
            app.duplicate_selection()?;
        }
        Action::VisualCommit => app.exit_visual_commit(),
        Action::VisualSelectSiblings => app.visual_select_scope(false),
        Action::VisualSelectAll => app.visual_select_scope(true),
        Action::PreviewBack => {
            // A git diff preview returns to the Git view. Everything else returns to the tree.
            if app.is_git_diff_preview() {
                app.close_git_diff();
            } else {
                app.back_to_tree();
            }
        }
        Action::SearchStart => app.start_search(),
        Action::SearchNext => app.search_next(1),
        Action::SearchPrev => app.search_next(-1),
        Action::PreviewEnterVisual => app.preview_enter_visual(false),
        Action::PreviewEnterVisualLine => app.preview_enter_visual(true),
        Action::PreviewCopySelection => app.preview_copy_selection(),
        Action::PreviewCopySelectionRef => app.preview_copy_selection_ref(),
        Action::PreviewExitVisual => app.preview_exit_visual(),
        Action::ToggleMarkdownRaw => app.toggle_md_raw(),
        Action::LinkFocusNext => app.md_focus_move(1),
        Action::LinkFocusPrev => app.md_focus_move(-1),
        Action::LinkOpen => app.md_activate_focused()?,
        Action::OpenLinkNewTab => app.md_open_focused_link_new_tab()?,
        Action::OpenInNewTab => app.tab_new_from_selection()?,
        Action::ImageZoomIn => app.image_zoom_by(1.25),
        Action::ImageZoomOut => app.image_zoom_by(1.0 / 1.25),
        Action::ImageZoomReset => app.image_zoom_reset(),
        Action::PdfNextPage => app.pdf_next_page(),
        Action::PdfPrevPage => app.pdf_prev_page(),
        Action::PreviewFileNext => app.preview_jump_file(1),
        Action::PreviewFilePrev => app.preview_jump_file(-1),
        Action::TableCopy(kind) => app.table_copy(kind),
        Action::SortSet(k) => app.sort_menu_key(sort_key_char(k))?,
        Action::SortToggleReverse => app.sort_menu_key('r')?,
        Action::SortToggleDirsFirst => app.sort_menu_key('.')?,
        Action::ToggleTabList => app.toggle_tab_list(),
        Action::TabListClose => app.tab_list_close_selected(),
        Action::ToggleOutline => app.toggle_outline(),
        Action::BookmarkJump => app.bookmark_list_jump(),
        Action::BookmarkEdit => app.bookmark_list_edit(),
        Action::BookmarkDelete => app.bookmark_list_delete(),
        Action::BookmarkClose => app.close_bookmark_list(),
        Action::InfoClose => app.toggle_info(),
        Action::ToggleTableCell => app.toggle_table_cell_view(),
        #[cfg(feature = "git")]
        Action::GitDiffDiscard => app.git_diff_start_discard(),
        #[cfg(feature = "git")]
        Action::CycleDiffLayout => app.cycle_diff_layout(),
        #[cfg(feature = "git")]
        Action::ToggleFollowDiffScope => app.toggle_follow_diff_scope(),
        #[cfg(feature = "git")]
        Action::GitStage => app.git_view_stage(),
        #[cfg(feature = "git")]
        Action::GitUnstage => app.git_view_unstage(),
        #[cfg(feature = "git")]
        Action::GitStageAll => app.git_view_stage_all(),
        #[cfg(feature = "git")]
        Action::GitUnstageAll => app.git_view_unstage_all(),
        #[cfg(feature = "git")]
        Action::GitDiscard => app.git_view_start_discard(),
        #[cfg(feature = "git")]
        Action::GitCommit => app.start_git_commit(),
        #[cfg(feature = "git")]
        Action::GitWorktreeDiff => app.open_worktree_detail(),
        #[cfg(feature = "git")]
        Action::GitOpenLog => app.open_git_log(),
        #[cfg(feature = "git")]
        Action::GitOpenGraph => app.open_git_graph(),
        #[cfg(feature = "git")]
        Action::GitOpenBranches => app.open_git_branches(),
        #[cfg(feature = "git")]
        Action::GitOpenWorktrees => app.open_git_worktrees(),
        #[cfg(feature = "git")]
        Action::GitLaunchTool => app.launch_git_tool(),
        #[cfg(feature = "git")]
        Action::GitOpenSelectedDiff => {
            if let Some(p) = app.git_view_selected() {
                app.open_git_diff(&p);
            }
        }
        #[cfg(feature = "git")]
        Action::GitOpenDetail => match sfc {
            Surface::GitLog => app.open_git_commit_detail(),
            Surface::GitGraph => app.open_git_graph_detail(),
            _ => {}
        },
        #[cfg(feature = "git")]
        Action::GitGraphSetBase => app.git_graph_set_base(),
        #[cfg(feature = "git")]
        Action::GitGraphClearBase => app.git_graph_clear_base(),
        #[cfg(feature = "git")]
        Action::GitGraphToggleAll => app.git_graph_toggle_all(),
        #[cfg(feature = "git")]
        Action::GitGraphOpenPicker => app.git_graph_open_picker(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerToggle => app.git_graph_picker_toggle(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerAll => app.git_graph_picker_all(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerCurrentOnly => app.git_graph_picker_current_only(),
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveUp => app.git_graph_picker_reorder(-1),
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveDown => app.git_graph_picker_reorder(1),
        #[cfg(feature = "git")]
        Action::BranchFilterStart => app.git_branch_start_filter(),
        #[cfg(feature = "git")]
        Action::BranchCheckout => app.checkout_selected_branch()?,
        #[cfg(feature = "git")]
        Action::BranchCreate => app.start_create_branch(),
        #[cfg(feature = "git")]
        Action::BranchDelete => app.start_delete_branch(),
        #[cfg(feature = "git")]
        Action::WorktreeFilterStart => app.git_worktree_start_filter(),
        #[cfg(feature = "git")]
        Action::WorktreeGoto => app.worktree_goto(),
        #[cfg(feature = "git")]
        Action::WorktreeGotoNewTab => app.worktree_goto_new_tab()?,
        #[cfg(feature = "git")]
        Action::WorktreeClose => app.close_git_worktrees(),
        #[cfg(feature = "git")]
        Action::WorktreeShowChanges => app.worktree_show_changes(),
        #[cfg(feature = "git")]
        Action::WorktreeCreate => app.start_create_worktree(),
        #[cfg(feature = "git")]
        Action::GitCopy(kind) => app.git_copy(kind),
        #[cfg(feature = "git")]
        Action::CopyBranchName => app.git_copy_branch_name(),
        #[cfg(feature = "git")]
        Action::GitClose => match sfc {
            Surface::GitDetail => app.close_git_detail(),
            Surface::GitLog => app.close_git_log(),
            Surface::GitGraphPicker => app.git_graph_picker_cancel(),
            Surface::GitGraph => app.close_git_graph(),
            Surface::GitBranches => app.close_git_branches(),
            Surface::GitChanges => app.close_git_view(),
            _ => {}
        },
    }
    Ok(false)
}

/// Key handling for text-input surfaces (filter / search / mark / branch filter / dialog input).
/// Intercepts character and editing keys and does not apply the keymap (requirement 4).
fn handle_text_input(app: &mut App, sfc: Surface, key: KeyEvent) -> Result<bool> {
    let code = key.code;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match sfc {
        Surface::DialogInput => match (code, ctrl) {
            (KeyCode::Esc, _) => app.dialog_cancel(),
            (KeyCode::Enter, _) => app.dialog_submit()?,
            (KeyCode::Backspace, _) => app.dialog_input_backspace(),
            (KeyCode::Delete, _) => app.dialog_input_delete(),
            (KeyCode::Left, _) => app.dialog_cursor_left(),
            (KeyCode::Right, _) => app.dialog_cursor_right(),
            (KeyCode::Home, _) => app.dialog_cursor_home(),
            (KeyCode::End, _) => app.dialog_cursor_end(),
            (KeyCode::Char(c), false) => app.dialog_input_push(c),
            _ => {}
        },
        Surface::Filter => match (code, ctrl) {
            (KeyCode::Esc, _) => app.filter_clear(),
            (KeyCode::Enter, _) => app.filter_commit(),
            (KeyCode::Backspace, _) => app.filter_input_backspace(),
            (KeyCode::Down, _) => app.tree_next(),
            (KeyCode::Up, _) => app.tree_prev(),
            (KeyCode::Char(c), false) => app.filter_input_push(c),
            _ => {}
        },
        Surface::Search => match (code, ctrl) {
            (KeyCode::Esc, _) => app.search_clear(),
            (KeyCode::Enter, _) => app.search_commit(),
            (KeyCode::Backspace, _) => app.search_input_backspace(),
            (KeyCode::Char(c), false) => app.search_input_push(c),
            _ => {}
        },
        Surface::Mark => match (code, ctrl) {
            (KeyCode::Esc, _) => app.cancel_mark(),
            (KeyCode::Char(c), false) => app.mark_input(c),
            _ => app.cancel_mark(),
        },
        #[cfg(feature = "git")]
        Surface::BranchFilter => match (code, ctrl) {
            (KeyCode::Esc, _) => app.git_branch_filter_clear(),
            (KeyCode::Enter, _) => app.git_branch_filter_commit(),
            (KeyCode::Backspace, _) => app.git_branch_filter_backspace(),
            (KeyCode::Down, _) => app.git_branch_move(1),
            (KeyCode::Up, _) => app.git_branch_move(-1),
            (KeyCode::Char(c), false) => app.git_branch_filter_push(c),
            _ => {}
        },
        #[cfg(feature = "git")]
        Surface::WorktreeFilter => match (code, ctrl) {
            (KeyCode::Esc, _) => app.git_worktree_filter_clear(),
            (KeyCode::Enter, _) => app.git_worktree_filter_commit(),
            (KeyCode::Backspace, _) => app.git_worktree_filter_backspace(),
            (KeyCode::Down, _) => app.git_worktree_move(1),
            (KeyCode::Up, _) => app.git_worktree_move(-1),
            (KeyCode::Char(c), false) => app.git_worktree_filter_push(c),
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

/// Key handling for confirm-modal surfaces (delete confirm / drop confirm / rename preview) (requirement 4).
fn handle_modal_confirm(app: &mut App, sfc: Surface, key: KeyEvent) -> Result<bool> {
    match sfc {
        Surface::DialogRenamePreview => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.dialog_preview_apply()?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_cancel(),
            KeyCode::Char('j') | KeyCode::Down => app.dialog_preview_scroll(1),
            KeyCode::Char('k') | KeyCode::Up => app.dialog_preview_scroll(-1),
            _ => {}
        },
        Surface::DialogConfirmDrop => match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') => app.drop_apply(false)?,
            KeyCode::Char('m') | KeyCode::Char('M') => app.drop_apply(true)?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_cancel(),
            _ => {}
        },
        Surface::DialogConfirmDelete => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.dialog_confirm(true)?,
            // `!` = permanent delete (not recoverable). Accepted only during a delete confirmation
            // (allow_permanent).
            KeyCode::Char('!') if app.dialog_allow_permanent() => app.dialog_delete_permanent()?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_confirm(false)?,
            _ => {}
        },
        // App quit confirmation: y/q/Enter = quit (qq lets you exit quickly) / n/Esc = cancel.
        Surface::DialogConfirmQuit => match key.code {
            KeyCode::Char('y')
            | KeyCode::Char('Y')
            | KeyCode::Char('q')
            | KeyCode::Char('Q')
            | KeyCode::Enter => return Ok(true),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_cancel(),
            _ => {}
        },
        // Bookmark overwrite confirmation: y/Enter = overwrite / n/Esc = cancel.
        Surface::DialogConfirmBookmark => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.dialog_confirm(true)?,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.dialog_confirm(false)?,
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

/// Per-surface fixed handling of Esc (clear search/filter, close overlays, cancel visual …). Always quit=false.
fn handle_esc(app: &mut App, sfc: Surface) -> bool {
    match sfc {
        Surface::Help => app.show_help = false,
        Surface::Visual => app.exit_visual_cancel(),
        Surface::Sort => app.close_sort_menu(),
        Surface::Info => app.toggle_info(),
        Surface::TableCell => app.toggle_table_cell_view(),
        Surface::Bookmarks => app.close_bookmark_list(),
        Surface::Tabs => app.toggle_tab_list(),
        Surface::Outline => app.toggle_outline(),
        Surface::Tree => {
            // Esc with a query clears the filter; without one, clears the selection (legacy tree behavior).
            if app.filter_query().is_some() {
                app.filter_clear();
            } else if app.has_selection() {
                app.clear_selection();
            }
        }
        // Tables also have search, so use the same convention as text/image (previously Esc did
        // nothing here = the search highlight couldn't be cleared, and you couldn't return to the
        // tree either).
        Surface::PreviewText | Surface::PreviewImage | Surface::PreviewTable => {
            // If a search is active, clear it; otherwise return to the tree.
            if app.preview_search_query().is_some() {
                app.search_clear();
            } else {
                app.back_to_tree();
            }
        }
        // Esc while a line is selected clears the selection (doesn't return to the tree).
        Surface::PreviewTextVisual => app.preview_exit_visual(),
        #[cfg(feature = "git")]
        Surface::GitDetail => app.close_git_detail(),
        #[cfg(feature = "git")]
        Surface::GitLog => app.close_git_log(),
        #[cfg(feature = "git")]
        Surface::GitGraph => app.close_git_graph(),
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => app.git_graph_picker_cancel(),
        #[cfg(feature = "git")]
        Surface::GitBranches => {
            // Esc with a query clears the filter; without one, closes it (the same convention as
            // the tree filter).
            if app.git_branch_query().is_empty() {
                app.close_git_branches();
            } else {
                app.git_branch_filter_clear();
            }
        }
        #[cfg(feature = "git")]
        Surface::GitWorktrees => {
            // Esc with a query clears the filter; without one, closes it (same convention as
            // the branches list / tree filter).
            if app.git_worktree_query().is_empty() {
                app.close_git_worktrees();
            } else {
                app.git_worktree_filter_clear();
            }
        }
        #[cfg(feature = "git")]
        Surface::GitChanges => app.close_git_view(),
        #[cfg(feature = "git")]
        Surface::PreviewGitDiff => app.close_git_diff(),
        _ => {}
    }
    false
}

/// Per-surface fixed handling of Enter (tree activate / preview link / confirm in each list).
fn handle_enter(app: &mut App, sfc: Surface) -> Result<bool> {
    match sfc {
        Surface::Tree => app.tree_activate()?,
        Surface::PreviewText => app.md_activate_focused()?,
        Surface::Bookmarks => app.bookmark_list_jump(),
        Surface::Tabs => app.tab_list_activate(),
        Surface::Outline => app.outline_jump(),
        // `Enter` opens the full-cell popup from the table grid, and (the same toggle) closes it
        // again from inside the popup — mirrors `MermaidCaption`'s "Enter: full screen" idiom.
        Surface::PreviewTable | Surface::TableCell => app.toggle_table_cell_view(),
        #[cfg(feature = "git")]
        Surface::GitChanges => {
            if let Some(p) = app.git_view_selected() {
                app.open_git_diff(&p);
            }
        }
        #[cfg(feature = "git")]
        Surface::GitLog => app.open_git_commit_detail(),
        #[cfg(feature = "git")]
        Surface::GitGraph => app.open_git_graph_detail(),
        #[cfg(feature = "git")]
        Surface::GitGraphPicker => app.git_graph_picker_apply(),
        #[cfg(feature = "git")]
        Surface::GitBranches => app.checkout_selected_branch()?,
        #[cfg(feature = "git")]
        Surface::GitWorktrees => app.worktree_goto(),
        _ => {}
    }
    Ok(false)
}

/// Keys with a fixed meaning regardless of surface (arrows = Navigate alias / Esc / Enter / Tab/BackTab = link ops).
/// Returns `Some(quit)` when handled, or `None` if not a fixed key (falls through to keymap resolution). Requirements 3/4.
fn handle_fixed_key(app: &mut App, sfc: Surface, key: KeyEvent) -> Result<Option<bool>> {
    let done = match key.code {
        KeyCode::Up => {
            dispatch_navigate(app, sfc, Motion::Up);
            false
        }
        KeyCode::Down => {
            dispatch_navigate(app, sfc, Motion::Down);
            false
        }
        KeyCode::Left => {
            dispatch_navigate(app, sfc, Motion::Left);
            false
        }
        KeyCode::Right => {
            dispatch_navigate(app, sfc, Motion::Right);
            false
        }
        KeyCode::Home => {
            dispatch_navigate(app, sfc, Motion::Top);
            false
        }
        KeyCode::End => {
            dispatch_navigate(app, sfc, Motion::Bottom);
            false
        }
        KeyCode::Esc => handle_esc(app, sfc),
        KeyCode::Enter => handle_enter(app, sfc)?,
        // Markdown link/checkbox: Tab = next / ⇧Tab = previous (decorated text preview only —
        // while showing the raw source (R), the decorated view's items are stale, so don't move;
        // on other surfaces, swallowed with no effect).
        KeyCode::Tab => {
            if sfc == Surface::PreviewText && !app.is_raw_source() {
                app.md_focus_move(1);
            }
            false
        }
        KeyCode::BackTab => {
            if sfc == Surface::PreviewText && !app.is_raw_source() {
                app.md_focus_move(-1);
            }
            false
        }
        // Markdown checkbox: Space toggles only while focused. With no focus, fall through to
        // the keymap (doesn't steal less's convention of Space = PageDown).
        KeyCode::Char(' ') if sfc == Surface::PreviewText && app.md_focused_task() => {
            app.md_toggle_focused_task();
            false
        }
        // While focused on a <details> summary, Space toggles the collapse (same as Enter).
        KeyCode::Char(' ') if sfc == Surface::PreviewText && app.md_focused_details().is_some() => {
            if let Some(ord) = app.md_focused_details() {
                app.toggle_details(ord);
            }
            false
        }
        _ => return Ok(None),
    };
    Ok(Some(done))
}

/// Key handling (Run2: keymap-driven). Returns `Ok(true)` to request exit. Kept thin, following the §5 flow:
/// fixed text input → confirm modal → which-key leader resolution → per-surface fixed keys → keymap resolution.
/// Mode-specific keys are owned by the keymap (`App::keymaps`) and `dispatch_action`.
fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    // The previous transient message (flash) is cleared on the next key input.
    app.flash = None;
    let sfc = app.surface();

    if app.follow_enabled() {
        // User's choice (2026-07-23): follow is maximally sticky = within the follow diff, every
        // key except q (which leaves the diff) — scroll/move/n/N/f/layout — keeps follow active.
        // It's released only by "entering a text-input surface / a confirm modal" or "q leaving
        // the follow view." F (ToggleFollow) already turns it off explicitly via dispatch's
        // toggle_follow, so follow_break isn't needed there (= treated as kept here).
        let leaves_follow_view = app.pending_leader.is_none()
            && matches!(
                app.keymaps.resolve(sfc, None, KeyPress::norm(&key)),
                Resolution::Action(Action::PreviewBack)
            );
        if sfc.is_text_input() || sfc.is_modal_confirm() || leaves_follow_view {
            app.follow_break();
        }
    }

    // 1) Fixed surfaces (text input / confirm modal) go to their dedicated handler (no keymap applied — requirement 4).
    if sfc.is_text_input() {
        return handle_text_input(app, sfc, key);
    }
    if sfc.is_modal_confirm() {
        return handle_modal_confirm(app, sfc, key);
    }

    let kp = KeyPress::norm(&key);

    // 2) Waiting on a which-key leader: resolve from leaders (an unknown suffix cancels and does
    // not fall through — §5).
    if let Some(lead) = app.pending_leader.take() {
        return match app.keymaps.resolve(sfc, Some(lead), kp) {
            Resolution::Action(a) => dispatch_action(app, a, sfc),
            _ => Ok(false),
        };
    }

    // 3) Pre-empt surface-independent fixed keys (arrows = Navigate / Esc / Enter / Tab·BackTab) (requirements 3/4).
    if let Some(done) = handle_fixed_key(app, sfc, key)? {
        return Ok(done);
    }

    // 4) Keymap resolution (1 input = 1-2 HashMap lookups — requirement 5).
    match app.keymaps.resolve(sfc, None, kp) {
        Resolution::EnterLeader(id) => {
            // The copy leader (default `y`) always opens the which-key menu. While focused on a
            // code block in decorated Markdown, `c:code block` appears in that menu and is
            // selectable
            // (whichkey_spans switches it in based on surface/focus). Coexists with the existing
            // path-copy n/r/f/p/@.
            app.pending_leader = Some(id);
            Ok(false)
        }
        Resolution::Action(a) => dispatch_action(app, a, sfc),
        Resolution::Unbound => {
            // Bookmark list: a plain letter unbound in the keymap jumps directly as a bookmark name
            // (a-z = local / A-Z = global). q/j/k and global's t/T/F/Q etc. are already resolved
            // above = excluded here.
            if sfc == Surface::Bookmarks && !kp.ctrl {
                if let KeyCode::Char(c) = kp.code {
                    if c.is_ascii_alphabetic() {
                        app.bookmark_jump_letter(c);
                    }
                }
            }
            Ok(false)
        }
    }
}

/// Interpret `handle_key`'s result for the run loop. Returns `true` only when exit is requested.
///
/// Key handling only mutates app state (and does fs/git operations); it never touches terminal control,
/// drawing, or initialization, so every `Err` that reaches here is **recoverable** (e.g. a delete/paste
/// while another terminal removes the cwd makes the internal `refresh()?`/`rebuild_tree()?` fail). Rather
/// than crashing the whole run() or swallowing it, we show it to the user via flash and continue.
/// Unrecoverable failures (terminal control, drawing, initialization) are propagated with `?` in run()
/// itself (they never reach here).
fn resolve_key_result(app: &mut App, result: Result<bool>) -> bool {
    match result {
        Ok(quit) => quit,
        Err(e) => {
            app.flash = Some(format!(
                "{}{e:#}",
                i18n::tr(app.lang, crate::i18n::Msg::OperationFailed)
            ));
            false // recoverable → keep the loop going
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use app::Mode;
    use config::Config;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    // --- parse_args -----------------------------------------------------------------------

    fn argv(rest: &[&str]) -> Vec<String> {
        let mut v = vec!["konoma".to_string()];
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    }

    #[test]
    fn parse_args_recognizes_version_flags() {
        assert_eq!(parse_args(&argv(&["--version"])), ParsedArgs::Version);
        assert_eq!(parse_args(&argv(&["-V"])), ParsedArgs::Version);
    }

    #[test]
    fn parse_args_recognizes_help_flags() {
        assert_eq!(parse_args(&argv(&["--help"])), ParsedArgs::Help);
        assert_eq!(parse_args(&argv(&["-h"])), ParsedArgs::Help);
    }

    #[test]
    fn parse_args_with_no_argument_falls_back_to_none() {
        assert_eq!(parse_args(&argv(&[])), ParsedArgs::Open(None));
    }

    #[test]
    fn parse_args_treats_a_plain_path_as_open() {
        assert_eq!(
            parse_args(&argv(&["/some/dir"])),
            ParsedArgs::Open(Some(PathBuf::from("/some/dir")))
        );
        assert_eq!(
            parse_args(&argv(&["samples"])),
            ParsedArgs::Open(Some(PathBuf::from("samples")))
        );
    }

    /// An unrecognized `--foo`-shaped argument is **not** rejected as an unknown flag — it's
    /// treated as a directory path (the pre-existing `konoma [DIR]` contract, preserved on
    /// purpose: no `clap`, no "unknown flag" error surface for a single positional argument).
    #[test]
    fn parse_args_treats_an_unknown_flag_as_a_path() {
        assert_eq!(
            parse_args(&argv(&["--not-a-real-flag"])),
            ParsedArgs::Open(Some(PathBuf::from("--not-a-real-flag")))
        );
    }

    #[test]
    fn parse_args_treats_an_empty_string_as_a_path() {
        assert_eq!(
            parse_args(&argv(&[""])),
            ParsedArgs::Open(Some(PathBuf::from("")))
        );
    }

    #[test]
    fn parse_args_ignores_extra_arguments_after_the_first() {
        // Matches the pre-existing behavior (`args().nth(1)`): only the first argument mattered.
        assert_eq!(
            parse_args(&argv(&["/some/dir", "extra", "--version"])),
            ParsedArgs::Open(Some(PathBuf::from("/some/dir")))
        );
    }

    // --- validate_root ----------------------------------------------------------------------

    #[test]
    fn validate_root_rejects_a_nonexistent_path_and_names_it() {
        let missing = unique_tmp("konoma_validate_root_missing_test");
        let err = validate_root(&missing).expect_err("存在しないパスは Err");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&missing.display().to_string()),
            "エラーにパスが含まれる: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("no such file") || msg.to_lowercase().contains("not found"),
            "「見つからない」旨が含まれる: {msg}"
        );
    }

    #[test]
    fn validate_root_rejects_a_file_as_not_a_directory() {
        let dir = unique_tmp("konoma_validate_root_file_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("plain.txt");
        std::fs::write(&file, b"hi").unwrap();
        let err = validate_root(&file).expect_err("ファイルは Err");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&file.display().to_string()),
            "エラーにパスが含まれる: {msg}"
        );
        assert!(
            msg.contains("not a directory"),
            "「ディレクトリでない」旨が含まれる: {msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_root_accepts_a_real_readable_directory() {
        let dir = unique_tmp("konoma_validate_root_ok_test");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(validate_root(&dir).is_ok(), "実在し読めるディレクトリは Ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that exists and passes a plain `is_dir()` check but can't actually be listed
    /// (permissions denied) must also be rejected — this is exactly the gap a bare
    /// `std::fs::metadata` check would miss, which is why `validate_root` also tries `read_dir`.
    /// Skipped under root (root ignores directory permission bits and can read it anyway — the
    /// same root guard used elsewhere in this codebase, e.g. `session_actions`'s
    /// `session_restore_rolls_back_unreadable_root`).
    #[cfg(unix)]
    #[test]
    fn validate_root_rejects_an_unreadable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = unique_tmp("konoma_validate_root_unreadable_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable_as_root = std::fs::read_dir(&dir).is_ok();
        if !readable_as_root {
            let err = validate_root(&dir).expect_err("読めないディレクトリは Err");
            let msg = format!("{err:#}");
            assert!(
                msg.contains(&dir.display().to_string()),
                "エラーにパスが含まれる: {msg}"
            );
        } else {
            eprintln!(
                "validate_root_rejects_an_unreadable_directory: root で実行中のためパーミッション検証をスキップ"
            );
        }
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- resolve_startup (composition, still no terminal touched) -------------------------

    #[test]
    fn resolve_startup_reports_version_and_help_without_touching_the_filesystem() {
        assert!(matches!(
            resolve_startup(&argv(&["--version"])).unwrap(),
            Startup::Version
        ));
        assert!(matches!(
            resolve_startup(&argv(&["--help"])).unwrap(),
            Startup::Help
        ));
    }

    #[test]
    fn resolve_startup_fails_with_the_path_for_a_bad_directory() {
        let missing = unique_tmp("konoma_resolve_startup_missing_test");
        let err = resolve_startup(&argv(&[missing.to_str().unwrap()])).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&missing.display().to_string()),
            "エラーにパスが含まれる: {msg}"
        );
    }

    #[test]
    fn resolve_startup_opens_a_real_directory() {
        let dir = unique_tmp("konoma_resolve_startup_ok_test");
        std::fs::create_dir_all(&dir).unwrap();
        match resolve_startup(&argv(&[dir.to_str().unwrap()])).unwrap() {
            Startup::Open(got) => assert_eq!(got, std::fs::canonicalize(&dir).unwrap()),
            other => panic!("Open を期待したが: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- --version / --help text -----------------------------------------------------------

    #[test]
    fn version_text_contains_the_crate_version() {
        let t = version_text();
        assert!(t.contains(env!("CARGO_PKG_VERSION")));
        assert!(t.starts_with("konoma "));
    }

    #[test]
    fn help_text_documents_usage_and_points_at_the_docs_site() {
        let t = help_text();
        assert!(
            t.contains("konoma [DIR]"),
            "README の Usage 節と表記を揃える: {t}"
        );
        assert!(t.contains("--help"));
        assert!(t.contains("--version"));
        assert!(t.contains('?'), "アプリ内ヘルプキー ? への言及: {t}");
        assert!(t.contains("https://lesim-co-ltd.github.io/konoma/"));
    }

    // --- picker font-size fix (recovering the real cell ratio from window_size) ------------

    #[test]
    fn cell_px_from_window_size_computes_the_real_cell_ratio() {
        // A plausible real terminal: 80x24 characters over 752x816 pixels → 9.4x34 per cell,
        // floored to 9x34 (not the arbitrary 10x20 fallback, and not a 1:2 ratio either).
        assert_eq!(cell_px_from_window_size(80, 24, 752, 816), Some((9, 34)));

        // Any zero input (columns/rows/width_px/height_px) means "the terminal didn't report
        // usable pixel dimensions" (crossterm's own doc for `window_size()`) → None, not a
        // division panic or a bogus (0, 0)/(_, 0)/(0, _) cell.
        assert_eq!(cell_px_from_window_size(0, 24, 752, 816), None);
        assert_eq!(cell_px_from_window_size(80, 0, 752, 816), None);
        assert_eq!(cell_px_from_window_size(80, 24, 0, 816), None);
        assert_eq!(cell_px_from_window_size(80, 24, 752, 0), None);
        assert_eq!(cell_px_from_window_size(0, 0, 0, 0), None);

        // A window narrower than its own column count would floor-divide to 0 without the
        // `max(1)` guard — still never a 0-pixel cell.
        assert_eq!(cell_px_from_window_size(100, 10, 5, 5), Some((1, 1)));
    }

    #[test]
    fn fix_picker_font_size_leaves_non_default_font_size_untouched() {
        // A picker whose font size came from a real capability response (anything other than the
        // exact (10, 20) sentinel) is left alone — it's not "the query failed" — even when
        // window_size offers a different value.
        #[allow(deprecated)]
        let picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        let fixed = fix_picker_font_size(picker, Some((80, 24, 752, 816)));
        assert_eq!((fixed.font_size().width, fixed.font_size().height), (8, 16));
    }

    #[test]
    fn fix_picker_font_size_leaves_default_untouched_without_a_usable_window_size() {
        // Picker::halfblocks() carries the exact same (10, 20) sentinel `from_query_stdio`'s
        // fallback does, without touching stdio — safe to construct in a test.
        let picker = Picker::halfblocks();
        // window_size() itself failed (crossterm returned Err) → None passed through by main.
        let fixed = fix_picker_font_size(picker, None);
        assert_eq!(
            (fixed.font_size().width, fixed.font_size().height),
            (10, 20)
        );

        // window_size() succeeded but reported zero pixels (crossterm's own doc: "may not be
        // reliably implemented or default to 0") → still untouched.
        let picker2 = Picker::halfblocks();
        let fixed2 = fix_picker_font_size(picker2, Some((80, 24, 0, 0)));
        assert_eq!(
            (fixed2.font_size().width, fixed2.font_size().height),
            (10, 20)
        );
    }

    #[test]
    fn fix_picker_font_size_replaces_default_with_the_real_cell_ratio() {
        let picker = Picker::halfblocks();
        let fixed = fix_picker_font_size(picker, Some((80, 24, 752, 816)));
        assert_eq!((fixed.font_size().width, fixed.font_size().height), (9, 34));
    }

    #[test]
    fn fix_picker_font_size_preserves_protocol_type() {
        use ratatui_image::picker::ProtocolType;
        // A picker that reached the default font size but was still detected as e.g. Kitty
        // (font-size query failed, graphics-capability query didn't) must keep that protocol —
        // only the font size may change, never the protocol.
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        let fixed = fix_picker_font_size(picker, Some((80, 24, 752, 816)));
        assert_eq!(fixed.protocol_type(), ProtocolType::Kitty);
        assert_eq!((fixed.font_size().width, fixed.font_size().height), (9, 34));
    }

    // --- structural guard: validation must run before ratatui::init() touches the terminal --

    /// The property this whole fix exists for: `main`'s body must resolve+validate the startup
    /// target **before** `ratatui::init()` grabs the terminal (raw mode + the alternate screen).
    /// That can't be asserted by actually running `main` — doing so would enable raw mode / grab
    /// the alternate screen for real, inside `cargo test`, which is unsafe to do from a unit test
    /// (no real tty, parallel test threads sharing the process's stdio). See the project's
    /// tmux-based real-terminal verification (`stty -g` before/after) for the live proof instead.
    ///
    /// What this test *can* pin, honestly: the **code order** inside `fn main`, via a self-scan of
    /// `main.rs`'s own source — the same technique `test_support::guard` and
    /// `e2e_tests::extract_ui_config_field_names` already use elsewhere in this codebase. It fails
    /// if `main` stops calling `resolve_startup` at all, or if that call is ever moved below
    /// `ratatui::init()`.
    #[test]
    fn resolve_startup_runs_before_terminal_init_in_main() {
        let src = include_str!("main.rs");
        let main_start = src
            .find("\nfn main() -> Result<()> {")
            .expect("fn main が見つからない(自己スキャンが壊れている)");
        // Bounded to `fn main`'s own body (up to the next top-level `fn`, `resize_worker`) so a
        // mention of either name in a comment/doc *after* main() (e.g. in the test below, or in a
        // later helper) can never be mistaken for the real call site.
        let main_end = src[main_start..]
            .find("\nfn resize_worker(")
            .map(|off| main_start + off)
            .expect("main の直後の fn resize_worker が見つからない(自己スキャンが壊れている)");
        let body = &src[main_start..main_end];
        let resolve_at = body
            .find("resolve_startup(")
            .expect("main の本体が resolve_startup を呼んでいない");
        let init_at = body
            .find("let mut terminal = ratatui::init();")
            .expect("main の本体が ratatui::init を呼んでいない");
        assert!(
            resolve_at < init_at,
            "resolve_startup の呼び出しは ratatui::init() より前でなければならない\
             (不正なパスで端末(raw mode + alt screen)を触る前に落とすため)"
        );
    }

    #[test]
    fn fs_event_classification_reacts_to_git_locks_and_detects_ignore_rules() {
        let pb = |s: &str| vec![PathBuf::from(s)];
        // Also reacts to `.git/*.lock` (meaningful=true). Since konoma is lock-free
        // (--no-optional-locks), this is a signal of an external git operation (a commit, etc.),
        // and swallowing it would let the change marker go stale.
        assert_eq!(
            classify_fs_paths(&pb("/repo/.git/index.lock")),
            (true, false)
        );
        assert_eq!(classify_fs_paths(&pb("/r/.git/HEAD.lock")), (true, false));
        // The `.git` internals themselves (HEAD/refs/index) do react (following an external git
        // operation), but ignored is left alone.
        assert_eq!(classify_fs_paths(&pb("/repo/.git/HEAD")), (true, false));
        assert_eq!(classify_fs_paths(&pb("/repo/.git/index")), (true, false));
        // A normal file change: reacts, and ignored is left alone.
        assert_eq!(classify_fs_paths(&pb("/repo/src/main.rs")), (true, false));
        // A user's own *.lock (outside .git) is a target (meaningful).
        assert_eq!(classify_fs_paths(&pb("/repo/Cargo.lock")), (true, false));
        // .gitignore / .git/info/exclude needs an ignored recompute (true, true).
        assert_eq!(classify_fs_paths(&pb("/repo/.gitignore")), (true, true));
        assert_eq!(classify_fs_paths(&pb("/repo/sub/.gitignore")), (true, true));
        assert_eq!(
            classify_fs_paths(&pb("/repo/.git/info/exclude")),
            (true, true)
        );
        // A .gitignore inside node_modules is excluded (it's wholly ignored anyway, so no recompute is needed).
        assert_eq!(
            classify_fs_paths(&pb("/repo/node_modules/x/.gitignore")),
            (true, false)
        );
        // True if even one ignore-rule change is in the burst.
        assert_eq!(
            classify_fs_paths(&[
                PathBuf::from("/repo/.git/index.lock"),
                PathBuf::from("/repo/.gitignore"),
            ]),
            (true, true)
        );
        // An event with an unknown path errs safe (reload, don't touch ignored).
        assert_eq!(classify_fs_paths(&[]), (true, false));
    }

    #[test]
    fn follow_candidates_exclude_git_internals() {
        // Follow targets are anything outside `.git` (index/refs/lock churn isn't a review target).
        let got = follow_candidates(&[
            PathBuf::from("/repo/.git/index"),
            PathBuf::from("/repo/.git/refs/heads/main"),
            PathBuf::from("/repo/src/main.rs"),
            PathBuf::from("/repo/README.md"),
        ]);
        assert_eq!(
            got,
            vec![
                PathBuf::from("/repo/src/main.rs"),
                PathBuf::from("/repo/README.md"),
            ]
        );
    }

    #[test]
    fn burst_paths_dedupes_and_preserves_order() {
        let mut burst = BurstPaths::default();
        let a = PathBuf::from("/repo/a");
        let b = PathBuf::from("/repo/b");
        let c = PathBuf::from("/repo/c");
        assert!(burst.push(&a));
        assert!(burst.push(&b));
        assert!(!burst.push(&a), "重複は false(既にカウント済み)");
        assert!(burst.push(&c));
        assert_eq!(burst.finish(), Some(vec![a, b, c]));
    }

    #[test]
    fn burst_paths_overflow_reports_unknown() {
        let mut burst = BurstPaths::default();
        for i in 0..MAX_BURST_PATHS {
            assert!(burst.push(&PathBuf::from(format!("/repo/f{i}"))));
        }
        // Everything up to exactly the cap passes as distinct. The next one exceeds the cap = false.
        assert!(
            !burst.push(&PathBuf::from("/repo/overflow")),
            "上限を超えたら push は false"
        );
        assert_eq!(
            burst.finish(),
            None,
            "上限超過バーストは「パス不明」として扱われる"
        );
    }

    #[test]
    fn is_content_event_ignores_reads_reacts_to_writes() {
        use notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode,
        };
        use notify::EventKind;

        // Read-family events (open/read/close-without-write) don't react (a countermeasure for
        // inotify's IN_OPEN/IN_ACCESS).
        assert!(!is_content_event(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!is_content_event(&EventKind::Access(AccessKind::Open(
            AccessMode::Read
        ))));
        assert!(!is_content_event(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_event(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!is_content_event(&EventKind::Access(AccessKind::Any)));

        // A confirmed write (IN_CLOSE_WRITE) reacts. Every kind other than Access reacts (err safe).
        assert!(is_content_event(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(is_content_event(&EventKind::Any));
        assert!(is_content_event(&EventKind::Create(CreateKind::File)));
        assert!(is_content_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(is_content_event(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Any
        ))));
        assert!(is_content_event(&EventKind::Remove(RemoveKind::File)));
        assert!(is_content_event(&EventKind::Other));
    }

    #[test]
    fn is_structural_event_separates_writes_from_appearing_and_disappearing_entries() {
        use notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind,
            RenameMode,
        };
        use notify::EventKind;

        // Content-only: the entry already existed and only its bytes (or its metadata) changed, so
        // the set of rows is unchanged and the build-churn guard may skip the burst.
        assert!(!is_structural_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(!is_structural_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(!is_structural_event(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        // Metadata implies the entry exists — and macOS raises ITEM_INODE_META_MOD next to
        // ITEM_MODIFIED on an ordinary write, so counting it as structural would defeat the guard
        // for exactly the plain build writes it exists to absorb.
        assert!(!is_structural_event(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::Any)
        )));
        assert!(!is_structural_event(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::WriteTime)
        )));

        // Structural: which entries exist changed, so the listing must be rebuilt even when every
        // path is gitignored (this is the deleted-file-stays-on-screen bug).
        assert!(is_structural_event(&EventKind::Create(CreateKind::File)));
        assert!(is_structural_event(&EventKind::Create(CreateKind::Folder)));
        assert!(is_structural_event(&EventKind::Remove(RemoveKind::File)));
        assert!(is_structural_event(&EventKind::Remove(RemoveKind::Folder)));
        assert!(is_structural_event(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Any
        ))));
        assert!(is_structural_event(&EventKind::Modify(ModifyKind::Name(
            RenameMode::From
        ))));

        // Unclassifiable kinds (coarse backends, rescans) err towards refreshing.
        assert!(is_structural_event(&EventKind::Any));
        assert!(is_structural_event(&EventKind::Other));
        assert!(is_structural_event(&EventKind::Modify(ModifyKind::Any)));
        assert!(is_structural_event(&EventKind::Modify(ModifyKind::Other)));
    }

    #[test]
    fn burst_kinds_merge_is_sticky_across_a_burst() {
        // One structural event anywhere in a coalesced burst must disqualify the whole burst from
        // being skipped — a delete arriving alongside a thousand build writes is the real case.
        let mut kinds = FsBurstKinds::default();
        assert!(!kinds.structural && !kinds.ignore_rules_changed);
        for _ in 0..3 {
            kinds.merge(FsBurstKinds {
                ignore_rules_changed: false,
                structural: false,
            });
        }
        assert!(!kinds.structural, "書き込みだけを畳んでも structural=false");
        kinds.merge(FsBurstKinds {
            ignore_rules_changed: false,
            structural: true,
        });
        kinds.merge(FsBurstKinds {
            ignore_rules_changed: false,
            structural: false,
        });
        assert!(
            kinds.structural,
            "1件でも structural があればバースト全体が structural(後続の書き込みで戻らない)"
        );
        assert!(!kinds.ignore_rules_changed, "他方のフラグは巻き込まれない");
    }

    #[test]
    fn watcher_ignores_reads_but_reports_writes() {
        // An integration test that runs is_content_event through a real notify watcher in the same
        // order as run()'s callback (is_content_event → classify_fs_paths).
        //
        // The read-side assertion **fails on Linux without the is_content_event filter**
        // (inotify also subscribes to WatchMask::OPEN, so even a plain open/read fires
        // EventKind::Access(..) — without the filter this is misdetected as "there was a change").
        // macOS's FSEvents never reports reads at all, so on the read side **regardless of whether
        // the filter is present**, no event ever arrives = vacuous (it merely goes green without
        // any actual detection power), but it's harmless.
        // The write-side assertion holds on both OSes, and acts as a guardrail that the "filter
        // out reads" fix hasn't become an over-filter that swallows real content changes too.
        use notify::{RecursiveMode, Watcher};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let dir = unique_tmp("konoma_watch_filter_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, b"hello").unwrap();

        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                // The same-order filter as run()'s watcher callback.
                if !is_content_event(&ev.kind) {
                    return;
                }
                let (meaningful, _) = classify_fs_paths(&ev.paths);
                if meaningful {
                    let _ = tx.send(());
                }
            }
        })
        .expect("recommended_watcher");
        watcher
            .watch(&dir, RecursiveMode::Recursive)
            .expect("watch");

        // settle: discard the directory/file creation's own events over ~500ms.
        let settle_until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < settle_until {
            let _ = rx.recv_timeout(Duration::from_millis(50));
        }

        // A read alone (with the filter working) produces no event.
        for _ in 0..5 {
            let _ = std::fs::read(&file);
        }
        assert!(
            rx.recv_timeout(Duration::from_secs(1)).is_err(),
            "reading a file must not be reported as a change \
             (fails on Linux without the is_content_event filter; vacuous on macOS)"
        );

        // A write still produces an event (confirms we haven't over-filtered).
        std::fs::write(&file, b"changed").unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(10)).is_ok(),
            "writing a file must still be reported as a change"
        );

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watcher_reports_a_deletion_as_structural() {
        // An integration test that runs the *real* backend's event kinds through the same pipeline
        // as run()'s callback (is_content_event → classify_fs_paths → is_structural_event), because
        // the skip decision is only as good as what the OS actually reports. Deleting a file must
        // arrive with structural=true; without it, `fs_burst_is_build_churn` skips the refresh for
        // any file under an ignored directory and the deleted row stays on screen.
        //
        // Unlike the read-side assertion above, this one has detection power on **both** OSes:
        // macOS FSEvents raises ITEM_REMOVED and Linux inotify raises IN_DELETE.
        use notify::{RecursiveMode, Watcher};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let dir = unique_tmp("konoma_watch_structural_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doomed.txt");
        std::fs::write(&file, b"hello").unwrap();

        let (tx, rx) = mpsc::channel::<FsBurstKinds>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if !is_content_event(&ev.kind) {
                    return;
                }
                let (meaningful, ignore_rules) = classify_fs_paths(&ev.paths);
                if meaningful {
                    let _ = tx.send(FsBurstKinds {
                        ignore_rules_changed: ignore_rules,
                        structural: is_structural_event(&ev.kind),
                    });
                }
            }
        })
        .expect("recommended_watcher");
        watcher
            .watch(&dir, RecursiveMode::Recursive)
            .expect("watch");

        // settle: discard the directory/file creation's own events over ~500ms.
        let settle_until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < settle_until {
            let _ = rx.recv_timeout(Duration::from_millis(50));
        }

        std::fs::remove_file(&file).unwrap();
        // Collect what the burst reports for up to 10s, the same way the run loop ORs a burst
        // together — a backend is free to also emit unrelated content events for the parent
        // directory, and the guard only needs *one* of them to be structural.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut burst = FsBurstKinds::default();
        let mut got_any = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(kinds) => {
                    got_any = true;
                    burst.merge(kinds);
                    if burst.structural {
                        break;
                    }
                }
                Err(_) if got_any => break, // the burst is over
                Err(_) => {}
            }
        }
        assert!(got_any, "削除がイベントとして届かない(監視自体の失敗)");
        assert!(
            burst.structural,
            "ファイル削除は structural として届かなければならない\
             (届かないと ignored ディレクトリ配下の削除でツリーが更新されない)"
        );

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watch_target_changed_detects_actual_target_changes_only() {
        let a = unique_tmp("konoma_watch_target_a");
        let b = unique_tmp("konoma_watch_target_b");
        // never-yet-attempted → any concrete target counts as "changed."
        assert!(watch_target_changed(None, Some(a.as_path())));
        // same target as last attempted (success or failure) → not changed (no retry).
        assert!(!watch_target_changed(Some(a.as_path()), Some(a.as_path())));
        // a genuinely different target → changed.
        assert!(watch_target_changed(Some(a.as_path()), Some(b.as_path())));
        // target removed (extra/git watch going from Some to None) → changed.
        assert!(watch_target_changed(Some(a.as_path()), None));
        // both "no target" → not changed.
        assert!(!watch_target_changed(None, None));
    }

    #[test]
    fn failed_watch_is_not_retried_on_every_idle_tick() {
        // Simulates 5 idle poll ticks (run()'s ~100ms loop) with a root that can never be
        // watched (nonexistent directory — the same Err branch notify returns when the Linux
        // inotify per-user watch-descriptor limit, `fs.inotify.max_user_watches`, is hit; this
        // repo alone has 17k+ directories under it). Before the fix, run()'s guard compared
        // against `watched` (only set on a *successful* watch), so a failed attempt never
        // updated it and every one of these 5 ticks re-attempted the real w.watch() syscall with
        // no backoff (confirmed red: see watch_retry_storm_repro_before_fix in the commit that
        // introduced this test). The fixed guard (`watch_target_changed`) compares against
        // `attempted`, set unconditionally *before* the syscall, so only the first tick attempts.
        let mut watcher =
            notify::recommended_watcher(|_res: notify::Result<notify::Event>| {}).ok();
        let bad_root = unique_tmp("konoma_watch_retry_storm_test");

        let mut watched: Option<PathBuf> = None;
        let mut attempted: Option<PathBuf> = None;
        let mut attempts = 0;
        let mut failures = 0;
        for _tick in 0..5 {
            if watch_target_changed(attempted.as_deref(), Some(bad_root.as_path())) {
                attempted = Some(bad_root.clone());
                attempts += 1;
                if rewatch(watcher.as_mut(), &mut watched, &bad_root) == WatchOutcome::Failed {
                    failures += 1;
                }
            }
        }
        assert_eq!(
            attempts, 1,
            "同一 root への再試行は初回の1回だけ(リトライ嵐の再発防止)"
        );
        assert_eq!(failures, 1, "flash も初回の1回だけ立つはず");
        assert_eq!(
            watched, None,
            "失敗した watch は登録されない(unwatch 対象も無い=リークしない)"
        );

        // The target actually changing (root fixed / user navigates elsewhere) does retry.
        let dir2 = unique_tmp("konoma_watch_retry_storm_test_ok");
        std::fs::create_dir_all(&dir2).unwrap();
        assert!(watch_target_changed(
            attempted.as_deref(),
            Some(dir2.as_path())
        ));
        attempted = Some(dir2.clone());
        assert_eq!(
            rewatch(watcher.as_mut(), &mut watched, &dir2),
            WatchOutcome::Watching,
            "root がウォッチ可能な値に変われば再試行して成功する"
        );
        assert_eq!(watched, Some(dir2.clone()));
        assert_eq!(attempted.as_deref(), Some(dir2.as_path()));

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn set_extra_watch_reports_removed_and_failed_outcomes() {
        let mut watcher =
            notify::recommended_watcher(|_res: notify::Result<notify::Event>| {}).ok();
        let dir = unique_tmp("konoma_extra_watch_ok");
        std::fs::create_dir_all(&dir).unwrap();
        let bad = unique_tmp("konoma_extra_watch_bad"); // never created → watch() fails.

        let mut watched: Option<PathBuf> = None;
        assert_eq!(
            set_extra_watch(watcher.as_mut(), &mut watched, Some(dir.as_path())),
            WatchOutcome::Watching
        );
        assert_eq!(watched, Some(dir.clone()));

        // Switching to `None` (the file being shown returns inside root / to the tree) removes it.
        assert_eq!(
            set_extra_watch(watcher.as_mut(), &mut watched, None),
            WatchOutcome::Removed
        );
        assert_eq!(watched, None);

        // A target that can't be watched reports Failed and doesn't get registered.
        assert_eq!(
            set_extra_watch(watcher.as_mut(), &mut watched, Some(bad.as_path())),
            WatchOutcome::Failed
        );
        assert_eq!(watched, None);

        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn follow_is_sticky_and_breaks_only_on_toggle_or_text_input() {
        // User's choice (2026-07-23, maximally sticky): while following, a normal key (movement,
        // etc.) does not disable follow. It's released only by F (the toggle itself, via
        // toggle_follow) / a key while inside a text-input surface / a confirm modal / q
        // (PreviewBack leaves the follow view) (q/PreviewBack are verified in
        // e2e_follow_survives_scroll_and_cycle_breaks_only_on_q).
        let dir = unique_tmp("konoma_follow_break_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(app.follow_enabled(), "F で ON");
        // F once more: OFF via the toggle, not follow_break (indistinguishable from outside, but
        // it does turn OFF).
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(!app.follow_enabled(), "F 再押下で OFF");
        // Turn ON again and press j (a movement key — doesn't resolve to PreviewBack on the tree
        // surface) → kept via maximal stickiness.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(app.follow_enabled());
        handle_key(&mut app, key('j')).unwrap();
        assert!(
            app.follow_enabled(),
            "j のような通常キーでは最大粘着によりフォロー維持"
        );
        // '/' enters the text-input surface (Filter). The very keypress that enters it is still
        // judged on the Tree surface, so it's kept, but once inside the text-input surface, the
        // next key releases it (treated as manual operation).
        handle_key(&mut app, key('/')).unwrap();
        assert!(app.is_filtering());
        assert!(
            app.follow_enabled(),
            "テキスト入力面へ入る瞬間のキーでは未解除"
        );
        handle_key(&mut app, key('x')).unwrap();
        assert!(
            !app.follow_enabled(),
            "テキスト入力面に居る間のキーでフォロー解除"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quote_opens_bookmark_list_and_letters_jump() {
        // `'` alone opens the list (the invisible waiting state is gone), and a plain letter in the
        // list jumps directly as a bookmark name. The old e/d (edit/delete) moved to Ctrl
        // modifiers, so a plain e can also be used to jump.
        let root = unique_tmp("konoma_quote_list_test");
        let _ = std::fs::remove_dir_all(&root);
        let proj = root.join("proj");
        std::fs::create_dir_all(proj.join("sub")).unwrap();
        std::fs::write(proj.join("f.txt"), b"x").unwrap();
        let proj = proj.canonicalize().unwrap();
        let mut app = App::new(proj.clone(), Config::default()).unwrap();
        app.bookmarks = bookmarks::Bookmarks::with_base(root.join("cfgbase"), &proj);
        app.bookmarks.set('a', proj.join("sub")).unwrap();
        app.bookmarks.set('e', proj.join("f.txt")).unwrap();

        handle_key(&mut app, key('\'')).unwrap();
        assert!(app.is_bookmark_list(), "' 一発で一覧が開く");
        assert!(!app.is_marking(), "ジャンプの待ち受け状態は無い");
        // An unregistered letter: flash, and the list stays open.
        handle_key(&mut app, key('z')).unwrap();
        assert!(app.is_bookmark_list());
        assert!(app.flash.is_some(), "未登録は flash");
        // e jumps as a bookmark name (a file → preview).
        handle_key(&mut app, key('e')).unwrap();
        assert!(!app.is_bookmark_list(), "ジャンプで一覧が閉じる");
        assert_eq!(app.tab.mode, Mode::Preview);
        assert!(app
            .tab
            .preview_path
            .as_deref()
            .is_some_and(|p| p.ends_with("f.txt")));
        // Go back and press `'` a second time = closes it (feels like a toggle).
        handle_key(&mut app, key('q')).unwrap(); // preview → tree
        handle_key(&mut app, key('\'')).unwrap();
        assert!(app.is_bookmark_list());
        handle_key(&mut app, key('\'')).unwrap();
        assert!(!app.is_bookmark_list(), "' 再押下で閉じる");
        // Ctrl+D = delete the selected row (a plain d is reserved for jumping). Move down to e's
        // row with j, then delete it.
        handle_key(&mut app, key('\'')).unwrap();
        let before = app.bookmark_list_items().len();
        handle_key(&mut app, key('j')).unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        )
        .unwrap();
        assert_eq!(app.bookmark_list_items().len(), before - 1, "Ctrl+D で削除");
        assert!(app.bookmarks.get('e').is_none(), "消えたのは選択行の e");
        // A directory bookmark moves root.
        handle_key(&mut app, key('a')).unwrap();
        assert_eq!(app.tab.root, proj.join("sub"));
        assert!(!app.is_bookmark_list());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn filter_input_captures_literal_keys() {
        // While filter input is active, `?`/`c`/digits are also captured as plain "characters,"
        // not help/copy/tab.
        let dir = unique_tmp("konoma_filter_input_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key('/')).unwrap(); // start filtering
        assert!(app.is_filtering());
        // `c` (normally copy-code) and `?` (normally help) are captured as characters.
        handle_key(&mut app, key('c')).unwrap();
        handle_key(&mut app, key('?')).unwrap();
        assert_eq!(app.filter_query(), Some("c?"));
        assert_eq!(app.pending_leader, None, "コピーリーダーは始まらない");
        assert!(!app.show_help, "ヘルプは開かない");
        // Cleared by Esc.
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(!app.is_filtering() && app.filter_query().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn help_opens_in_git_view_and_shows_git_keys() {
        let dir = unique_tmp("konoma_git_help_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.open_git_view();
        assert!(app.is_git_view());

        // `?` in the git view → help opens (previously it got swallowed by the git intercept and
        // never opened).
        handle_key(&mut app, key('?')).unwrap();
        assert!(app.show_help, "git モードで ? がヘルプを開く");

        // The help content is the git-specific one (the changes-hub section, stage-family keys).
        let lines = ui::help::help_lines(&app);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect();
        assert!(
            text.contains("Git changes") || text.contains("変更ハブ"),
            "git 節が出る: {text}"
        );
        assert!(
            text.contains("stage") || text.contains("ステージ"),
            "ステージ系キーが出る"
        );

        // `?` while help is shown closes it.
        handle_key(&mut app, key('?')).unwrap();
        assert!(!app.show_help, "? で閉じる");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(feature = "git")]
    #[test]
    fn tab_keys_per_tab_git_mode_and_literal_in_filter() {
        let dir = unique_tmp("konoma_tab_gitview_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();

        // While filter input is active, t is captured as a plain "character," not a new tab.
        handle_key(&mut app, key('/')).unwrap();
        let tc = app.tab_count();
        handle_key(&mut app, key('t')).unwrap();
        assert_eq!(app.tab_count(), tc, "絞り込み中は t で新タブにしない");
        assert_eq!(app.filter_query(), Some("t"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();

        // Even while in the git view, t can open a new tab. The new tab is a plain Tree (git mode
        // is per-tab).
        app.open_git_view();
        assert!(app.is_git_view());
        handle_key(&mut app, key('t')).unwrap();
        assert_eq!(app.tab_count(), tc + 1, "git ビュー中でも t で新タブ");
        assert!(!app.is_git_view(), "新タブは git ビュー無しで始まる");
        // Returning to the original tab **restores git mode** (you can view a document in another
        // tab and come back).
        handle_key(&mut app, key('1')).unwrap(); // to tab 0
        assert!(app.is_git_view(), "タブを戻ると git モードのまま");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copy_leader_sets_then_clears_pending() {
        use keymap::LeaderId;
        let dir = unique_tmp("konoma_chord_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        // `y` starts the copy leader → pending_leader.
        handle_key(&mut app, key('y')).unwrap();
        assert_eq!(app.pending_leader, Some(LeaderId::Copy));
        // A key not in the leader is discarded and pending is cleared (the clipboard is untouched).
        handle_key(&mut app, key('x')).unwrap();
        assert_eq!(app.pending_leader, None);
    }

    #[test]
    fn file_leader_opens_on_space() {
        use keymap::LeaderId;
        let dir = unique_tmp("konoma_fileleader_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        // The default `c` is no longer a leader (the old copy prefix is gone).
        handle_key(&mut app, key('c')).unwrap();
        assert_eq!(app.pending_leader, None);
        // `Space` starts the file-management leader.
        handle_key(&mut app, key(' ')).unwrap();
        assert_eq!(app.pending_leader, Some(LeaderId::File));
    }

    #[test]
    fn space_leader_n_opens_create_dialog() {
        // Space→n = create a file (the destination the old Tree `a` moved to).
        let dir = unique_tmp("konoma_space_create_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key(' ')).unwrap();
        handle_key(&mut app, key('n')).unwrap();
        assert_eq!(app.pending_leader, None, "リーダーは確定で消える");
        assert!(app.is_dialog(), "Space→n で作成ダイアログが開く");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_keys_a_and_shift_a_dispatch() {
        // `a` = SetAnchor (the old `:`), `A` = ResetAnchor (new). Right after startup, both flash "already the anchor."
        let dir = unique_tmp("konoma_anchor_dispatch_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        handle_key(&mut app, key('a')).unwrap();
        let fa = app.flash.clone().unwrap_or_default();
        assert!(
            fa.contains("root") || fa.contains("ルート"),
            "a の flash: {fa}"
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
        )
        .unwrap();
        let fa2 = app.flash.clone().unwrap_or_default();
        assert!(
            fa2.contains("start") || fa2.contains("起動"),
            "A の flash: {fa2}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shift_q_opens_quit_confirm_then_qq_quits() {
        // Default (confirm_quit=ON): Q opens the confirmation dialog → doesn't quit yet. Confirmed
        // by q once more (qq).
        let dir = unique_tmp("konoma_quit_confirm_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let exit = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert!(!exit, "確認段階ではまだ終了しない");
        assert!(
            app.is_dialog() && app.confirm_is_quit(),
            "終了確認ダイアログが開く"
        );
        assert_eq!(app.surface(), Surface::DialogConfirmQuit);
        let exit2 = handle_key(&mut app, key('q')).unwrap();
        assert!(exit2, "qq の2打鍵目で終了");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quit_confirm_cancel_with_esc_keeps_running() {
        let dir = unique_tmp("konoma_quit_cancel_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key('q')).unwrap(); // Tree's q = Quit → the confirmation dialog
        assert!(app.is_dialog() && app.confirm_is_quit());
        let exit = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(!exit, "Esc では終了しない");
        assert!(!app.is_dialog(), "Esc で確認ダイアログが閉じる");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quit_without_confirm_quits_immediately() {
        // confirm_quit=false: Q quits immediately (no dialog).
        let dir = unique_tmp("konoma_quit_immediate_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.confirm_quit = false;
        let mut app = App::new(dir.clone(), cfg).unwrap();
        let exit = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert!(exit, "confirm_quit=false なら Q で即終了");
        assert!(!app.is_dialog(), "ダイアログは開かない");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shift_q_is_literal_while_filtering() {
        // While input (filtering) is active, Q is captured as a plain "character." Doesn't quit,
        // doesn't show the confirmation either.
        let dir = unique_tmp("konoma_quit_filter_literal_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app, key('/')).unwrap();
        let exit = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert!(!exit, "絞り込み中の Q では終了しない");
        assert!(!app.is_dialog(), "終了確認も出ない");
        assert_eq!(app.filter_query(), Some("Q"), "Q は文字として入力される");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn visual_space_d_commits_range_and_confirms_delete() {
        // The old direct `D` is gone → in Visual it's Space→d. Commit the range first, then go to
        // the delete confirmation.
        let dir = unique_tmp("konoma_visual_spaced_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"y").unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        app.rebuild_tree().unwrap();
        app.tab.selected = 0;
        handle_key(&mut app, key('v')).unwrap();
        assert!(app.is_visual(), "v でビジュアル");
        handle_key(&mut app, key(' ')).unwrap();
        handle_key(&mut app, key('d')).unwrap();
        assert!(!app.is_visual(), "Space→d で範囲を確定しビジュアルを抜ける");
        assert!(app.is_dialog(), "削除確認ダイアログが開く");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dnd_paste_ignored_while_dialog_or_overlay_open() {
        // #13: don't let a D&D paste interrupt while a dialog/modal/overlay is shown.
        // Only the basic full-screen surfaces (Tree/Preview) and text-input surfaces accept it.
        let dir = unique_tmp("konoma_dnd_guard_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
        // The normal Tree surface accepts it.
        assert_eq!(app.surface(), Surface::Tree);
        assert!(paste_accepted(app.surface()), "Tree ではドロップを受ける");
        // Open the file-creation dialog (input) → it's a text-input surface, so it's received as characters.
        handle_key(&mut app, key(' ')).unwrap();
        handle_key(&mut app, key('n')).unwrap();
        assert!(app.is_dialog());
        assert!(
            paste_accepted(app.surface()),
            "入力ダイアログは文字として取り込むため通す"
        );
        // Don't let it interrupt while help (an overlay) is shown.
        let mut app2 = App::new(dir.clone(), Config::default()).unwrap();
        handle_key(&mut app2, key('?')).unwrap();
        assert!(app2.show_help);
        assert!(
            !paste_accepted(app2.surface()),
            "ヘルプ表示中はドロップを無視する"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn help_toggles_and_swallows_keys() {
        let dir = unique_tmp("konoma_help_key_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        assert!(!app.show_help);
        // Opens with ?.
        handle_key(&mut app, key('?')).unwrap();
        assert!(app.show_help);
        // While it's open, j scrolls (doesn't flow into mode operations).
        handle_key(&mut app, key('j')).unwrap();
        assert_eq!(app.help_scroll, 1);
        // Closes with q (the app doesn't quit).
        assert!(!handle_key(&mut app, key('q')).unwrap());
        assert!(!app.show_help);
    }

    #[test]
    fn tab_keys_work_in_preview_and_preserve_mode() {
        // Bug fix: tab operations (t/w/[/]/1-9) also work while previewing, and each tab's mode
        // (Tree/Preview) is preserved. Switching doesn't drop it to Tree, and returning restores Preview.
        let dir = unique_tmp("konoma_tab_in_preview_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        // Put tab 0 into previewing.
        app.tab.mode = Mode::Preview;
        // t opens a new tab (works even while previewing). The new tab starts from Tree.
        handle_key(&mut app, key('t')).unwrap();
        assert_eq!(app.tab_count(), 2, "Preview 中でも新規タブが作れる");
        assert_eq!(app.tab.mode, Mode::Tree, "新規タブは Tree から");
        let new_tab = app.active_tab_index();
        // [ returns to the original tab 0 → Preview is restored without dropping to Tree.
        handle_key(&mut app, key('[')).unwrap();
        assert_ne!(app.active_tab_index(), new_tab, "タブが切り替わる");
        assert_eq!(
            app.tab.mode,
            Mode::Preview,
            "戻ったタブの Preview が復元される"
        );
    }

    #[test]
    fn flash_is_cleared_on_next_key() {
        let dir = unique_tmp("konoma_flash_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir, Config::default()).unwrap();
        app.flash = Some("x".into());
        handle_key(&mut app, key('j')).unwrap();
        assert_eq!(app.flash, None, "次のキーで flash が消える");
    }

    #[test]
    fn recoverable_key_error_flashes_and_does_not_quit() {
        // Design principle #3: don't bring down the TUI on a recoverable fs/git failure during key handling.
        // Even if handle_key returns Err, the run loop doesn't terminate (quit=false), and the
        // error is shown to the user via flash rather than swallowed.
        let dir = unique_tmp("konoma_recoverable_err_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let quit = resolve_key_result(&mut app, Err(anyhow::anyhow!("boom: refresh 失敗")));
        assert!(!quit, "回復可能な Err でループは終了しない");
        let flash = app
            .flash
            .as_deref()
            .expect("Err は握り潰さず flash で見せる");
        assert!(
            flash.contains("boom"),
            "原因メッセージが flash に出る: {flash}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn close_tab_or_quit_closes_tab_when_multiple_else_quits() {
        // Tree's q: with multiple tabs, closes the current one (doesn't quit); with the last one, requests quitting.
        let dir = unique_tmp("konoma_close_tab_or_quit_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.confirm_quit = false; // judge the last tab by quitting immediately (no dialog in between)
        let mut app = App::new(dir.clone(), cfg).unwrap();

        // 2 tabs → q just closes the tab (quit=false).
        app.tab_new().unwrap();
        assert_eq!(app.tab_count(), 2);
        let quit = dispatch_action(&mut app, Action::CloseTabOrQuit, Surface::Tree).unwrap();
        assert!(!quit, "複数タブでは終了しない");
        assert_eq!(app.tab_count(), 1, "現在タブが閉じて1枚に戻る");

        // The last tab → q requests quitting (confirm_quit=false, so immediately Ok(true)).
        let quit = dispatch_action(&mut app, Action::CloseTabOrQuit, Surface::Tree).unwrap();
        assert!(quit, "最後の1枚では終了する");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_key_result_passes_through_quit_and_continue() {
        // Ok(true) = a quit request passes straight through / Ok(false) = continuing passes
        // straight through, and doesn't touch flash either.
        let dir = unique_tmp("konoma_resolve_passthrough_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        assert!(
            resolve_key_result(&mut app, Ok(true)),
            "Ok(true) は終了要求を通す"
        );
        assert_eq!(app.flash, None, "終了要求では flash を立てない");
        assert!(!resolve_key_result(&mut app, Ok(false)), "Ok(false) は継続");
        assert_eq!(app.flash, None, "継続では flash を立てない");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// F: a filter-pool walk running on another thread must shorten the loop's wait, like every
    /// other background job — otherwise a result already sitting in the channel can wait up to a
    /// whole idle timeout before it reaches the screen, which is the opposite of what the budgeted
    /// hand-off is for (the list is meant to fill in while the user is still typing).
    ///
    /// Asserted on the predicate the loop actually branches on. Timing the loop would be a flaky
    /// way to ask a question with an exact answer.
    #[test]
    fn poll_is_fast_while_a_filter_pool_walk_is_in_flight() {
        let dir = unique_tmp("konoma_poll_pool");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/f.txt"), b"x").unwrap();
        let mut app = App::new(dir.clone(), Config::default()).unwrap();
        let idle = poll_timeout(&app);
        assert!(
            idle > Duration::from_millis(16),
            "土台: 何も走っていなければ待ちは長い ({idle:?})"
        );

        // Force the hand-off, and attach a loader so `/` really does leave a walk running.
        app::set_filter_scan_budget(Some(Some(Duration::ZERO)));
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_filter_pool_loader(tx);
        app.start_filter();
        assert!(app.filter_pool_scan_in_flight(), "土台: 受け渡しが起きた");
        assert_eq!(
            poll_timeout(&app),
            Duration::from_millis(16),
            "走査中は速く poll する"
        );

        let res = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        app.apply_filter_pool(res);
        assert!(!app.filter_pool_scan_in_flight());
        assert_eq!(poll_timeout(&app), idle, "着地したら元の待ちに戻る");
        app::set_filter_scan_budget(None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
