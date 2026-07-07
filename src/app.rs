//! Application state: the `App` struct and its methods — mode (Tree/Preview), the tree,
//! tabs, and the current preview target. Git view methods live in the `git_view` submodule.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui_image::errors::Errors;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use ratatui_image::{FilterType, Resize};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Config;
use crate::i18n::tr;
use crate::preview::PreviewKind;

mod bookmark_actions;
mod file_actions;
mod git_view;
mod md_tasks;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tree,
    Preview,
}

/// Display mode (outer): what is currently shown full-screen. The **outer chip** in the status bar (colors assigned by `ui`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Tree,
    Preview,
    Image,
    /// CSV/TSV table preview (aligned grid + cell cursor).
    Table,
}

/// Internal mode (inner): what is currently being operated. Shows the **inner chip** only while active and switches the footer keys too.
/// Priority: dialog > each footer mode (filter/search/sort/bookmarks) > visual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalMode {
    Visual,
    /// Windowed-preview line selection (`v` in a code/text preview).
    PreviewVisual,
    Filter,
    /// Changed-files-only tree view (`C`; Agent Watch).
    ChangedFilter,
    Search,
    Sort,
    Mark,
    Bookmarks,
    Info,
    Create,
    Rename,
    BatchRename,
    RenamePreview,
    DeleteConfirm,
    DropConfirm,
    QuitConfirm,
    GitChanges,
    GitDiff,
    Commit,
    GitLog,
    GitDetail,
    GitBranch,
    GitGraph,
    GitGraphPicker,
}

/// Preview paging key style. Specified via the `ui.keys` setting (default vim).
/// Diffs use only the page/half-page keys. j/k line movement, arrows, and PageUp/Down are common to both styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyScheme {
    /// vim style: Ctrl-f/Ctrl-b=page, Ctrl-d/Ctrl-u=half.
    Vim,
    /// less style: Space/b=page, d/u=half.
    Less,
}

impl KeyScheme {
    pub fn parse(s: &str) -> Self {
        match s {
            "less" => Self::Less,
            _ => Self::Vim,
        }
    }
}

/// diff layout (config `git.diff`; cycle at runtime with `s`). Auto picks vertical/horizontal automatically from the terminal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLayout {
    /// Vertical (unified).
    Unified,
    /// Side-by-side.
    Split,
    /// Horizontal if wide, vertical if narrow.
    Auto,
}

impl DiffLayout {
    /// Parse a config value. "split"/"horizontal"/"side-by-side"=horizontal / "auto"=auto / otherwise=vertical.
    pub fn parse(s: &str) -> Self {
        let n: String = s
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(|c| c.to_lowercase())
            .collect();
        match n.as_str() {
            "split" | "horizontal" | "sidebyside" | "side" => Self::Split,
            "auto" => Self::Auto,
            _ => Self::Unified, // "unified" / "vertical" / 不明
        }
    }
    /// Resolve whether to lay out side-by-side at the actual display width `width`. Auto decides by a threshold.
    pub fn is_split(self, width: u16) -> bool {
        match self {
            DiffLayout::Unified => false,
            DiffLayout::Split => true,
            DiffLayout::Auto => width >= DIFF_AUTO_MIN_WIDTH,
        }
    }
    /// Cycle to the next layout (vertical→horizontal→Auto→vertical). For `s`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn next(self) -> Self {
        match self {
            DiffLayout::Unified => DiffLayout::Split,
            DiffLayout::Split => DiffLayout::Auto,
            DiffLayout::Auto => DiffLayout::Unified,
        }
    }
}

/// Minimum inner width for side-by-side under Auto (below this, vertical). A guideline so two-column code is not cramped.
const DIFF_AUTO_MIN_WIDTH: u16 = 90;

/// Layout of the status chrome (config `ui.statusbar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusbarLayout {
    /// Context (mode/path/zoom) on top (right of the tab row), key hints at the bottom (default).
    Split,
    /// Combine both context and key hints into a single bottom line.
    Bottom,
}

impl StatusbarLayout {
    pub fn parse(s: &str) -> Self {
        match s {
            "bottom" => Self::Bottom,
            _ => Self::Split,
        }
    }
}

/// Path display style. Used for the title (tree/preview). Cycle with the `p` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    /// Relative to the launch directory name (e.g. konoma/src/main.rs). Default.
    Relative,
    /// Relative to HOME (e.g. ~/work/konoma).
    Home,
    /// Full path (e.g. /Users/me/work/konoma).
    Full,
}

impl PathStyle {
    pub fn parse(s: &str) -> Self {
        match s {
            "home" => Self::Home,
            "full" => Self::Full,
            _ => Self::Relative,
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Relative => Self::Home,
            Self::Home => Self::Full,
            Self::Full => Self::Relative,
        }
    }
}

/// A single entry shown in the tree.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

/// Tree sort key (FR / M7 auxiliary). Switch via the footer menu (`s`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Ext,
}

/// Sort settings. `reverse`=flip ascending/descending, `dirs_first`=group directories at the top.
/// Default is "name, ascending, directories first" (matches the previous behavior). Global (not per-tab).
#[derive(Clone, Copy)]
pub struct Sort {
    pub key: SortKey,
    pub reverse: bool,
    pub dirs_first: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            reverse: false,
            dirs_first: true,
        }
    }
}

impl SortKey {
    /// Parse a config string (case-insensitive). Unknown/empty falls back to Name.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "size" => SortKey::Size,
            "modified" | "mtime" | "mod" => SortKey::Modified,
            "ext" | "extension" | "type" => SortKey::Ext,
            _ => SortKey::Name,
        }
    }
}

impl Sort {
    /// Build the startup sort order from the `[ui.sort]` config.
    pub fn from_config(c: &crate::config::SortConfig) -> Self {
        Self {
            key: SortKey::parse(&c.key),
            reverse: c.reverse,
            dirs_first: c.dirs_first,
        }
    }
}

/// Destructive/creation operations confirmed through a modal dialog (M7 Phase B, safety first).
// git 専用バリアント(GitDiscard/GitDeleteBranch)は非 git ビルドでは構築経路が無い(その構築元は
// git ディスパッチ専用)。バリアント自体は match の網羅性のため残し、非 git のみ dead_code を許す。
#[cfg_attr(not(feature = "git"), allow(dead_code))]
#[derive(Clone)]
enum PendingOp {
    /// Create with the entered name. A trailing `/` means a folder. `dir` is the destination (based on the cursor).
    Create { dir: PathBuf },
    /// Rename the target to the entered name within the same parent.
    Rename { target: PathBuf },
    /// Send the target to the trash (recoverable).
    Delete { targets: Vec<PathBuf> },
    /// Template-input stage of batch rename (`targets`=targets in display order). On confirm, builds a plan and moves to preview.
    BatchRenameInput { targets: Vec<PathBuf> },
    /// Preview→apply stage of batch rename (`plan`=(old, new) pairs).
    BatchRenameApply { plan: Vec<(PathBuf, PathBuf)> },
    /// In the Git view, `x`=discard a file's changes (via a confirmation dialog). On confirm, git::discard.
    GitDiscard { path: PathBuf },
    /// In the Git view, `c`=enter a commit message. On confirm, git::commit (uses the staged index).
    GitCommit,
    /// In the branch list, `n`=enter a new branch name. On confirm, git::create_branch (create and switch).
    GitCreateBranch,
    /// In the branch list, `d`=delete confirmation. `y`=safe (-d) / `!`=force (-D).
    GitDeleteBranch { name: String },
    /// A transfer received via file drag & drop (through a terminal paste). `c`=copy / `m`=move / Esc=cancel.
    /// `sources`=the dropped existing paths, `dir`=the drop destination (directory based on the cursor).
    DropTransfer { sources: Vec<PathBuf>, dir: PathBuf },
    /// Quit the whole app. A confirmation is requested before exiting (see `ui.confirm_quit`).
    Quit,
}

/// A confirmation (yes/no) or text-input modal. On confirm, calls fileops according to `op`.
struct Dialog {
    op: PendingOp,
    kind: DialogKind,
}

/// Dialog kind. Confirm=y/n for a destructive op (delete) / Input=name entry for create/rename.
/// `allow_permanent`=whether the delete confirmation also offers `!`=permanent delete (unrecoverable) (true=delete confirmation only).
enum DialogKind {
    Confirm {
        message: String,
        allow_permanent: bool,
    },
    Input {
        title: String,
        buffer: String,
        /// Insertion position (**character** index within buffer, 0..=char count). Move with ←→/Home/End.
        cursor: usize,
    },
    /// Confirmation preview for batch rename. `lines`=the "old → new" list, `scroll`=the top visible line.
    Preview {
        title: String,
        lines: Vec<String>,
        scroll: usize,
    },
}

/// File clipboard operation kind. Copy=duplicate / Cut=move (consumed on paste).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipOp {
    Copy,
    Cut,
}

/// Copy/cut targets pushed with `Y`/`X`. Apply them with `P` to the cursor-based directory.
#[derive(Clone)]
struct Clipboard {
    op: ClipOp,
    paths: Vec<PathBuf>,
}

/// Snapshot of one tab's tree context (FR-5).
/// The active tab's working state lives in App's fields as the source of truth, and is saved/loaded here on switch.
/// It keeps not just the tree but also the **mode/preview state per tab**, restoring them on switch instead of dropping to Tree.
/// The heavy image state (protocol/source image) is not carried over and is reloaded on restore (zoom/center are kept).
#[derive(Clone)]
struct TabState {
    root: PathBuf,
    open_dir: PathBuf,
    entries: Vec<Entry>,
    selected: usize,
    show_hidden: bool,
    tree_viewport: u16,
    mode: Mode,
    preview_path: Option<PathBuf>,
    preview_kind: Option<PreviewKind>,
    preview_scroll: u16,
    preview_hscroll: u16,
    preview_viewport: u16,
    preview_byte_top: u64,
    preview_top_line: usize,
    image_zoom: f64,
    image_center: (f64, f64),
    // PDF の現在ページ/総ページ数もタブごとに保持(別タブから戻ったとき同じページを再描画する)。
    pdf_page: u32,
    pdf_pages: Option<u32>,
    // CSV/TSV テーブルのセルカーソル/スクロールもタブごとに保持(テーブル本体は復元時に再パース)。
    table_cur_row: usize,
    table_cur_col: usize,
    table_top_row: usize,
    table_left_col: usize,
    // windowed(Code/Text)プレビューの 2D キャレット位置(別タブから戻っても同じ行/列に戻る)。選択(anchor)は持ち越さない。
    preview_cursor_line: usize,
    preview_cursor_col: usize,
    // Markdown/Mermaid の raw ソース表示(`R`)状態もタブごとに保持する。
    md_raw: bool,
    // git オーバーレイもタブごとに保持する(別タブでドキュメントを見て戻っても git モードのまま)。
    git_view: bool,
    git_view_sel: usize,
    git_view_entries: Vec<crate::git::ChangeEntry>,
    came_from_git_view: bool,
    git_log: Option<Vec<crate::git::CommitInfo>>,
    git_log_sel: usize,
    git_detail: Option<Vec<crate::git::DiffLine>>,
    git_detail_meta: Option<crate::git::CommitMeta>,
    git_detail_title: Option<String>,
    git_detail_scroll: u16,
    git_detail_hscroll: u16,
    git_detail_viewport: u16,
    git_detail_total: usize,
    git_branches: Option<Vec<crate::git::BranchInfo>>,
    git_branch_sel: usize,
    git_branch_filter: String,
    git_branch_filtering: bool,
    git_graph: Option<Vec<crate::git::GraphRow>>,
    git_graph_sel: usize,
    // --- タブ毎の選択 / 絞り込み / プレビュー検索 (#2/#6: タブ跨ぎのデータ損失 footgun 対策) ---
    // これらが TabState に無いと、タブAで選択 → 別タブ/別 dir で D/X/Y/P したとき、
    // 見えていない旧選択のファイルが操作対象になる(マーカー不可視で気付けない)。
    // クリップボード(Y/X/P の paths バッファ)は app 全体共有のままで、ここには入れない(ユーザ確定事項)。
    selection: BTreeSet<PathBuf>,
    visual_anchor: Option<usize>,
    tree_filter: Option<String>,
    filter_input: Option<String>,
    filter_pool: Vec<Entry>,
    changed_filter: bool,
    preview_search: Option<String>,
    search_input: Option<String>,
    search_matches: Vec<(u64, usize, usize)>,
    search_idx: usize,
}

/// The media payload loaded on a separate thread (sent back to the UI thread).
pub enum MediaPayload {
    /// A still image (SVG raster result / single-frame GIF, etc.) → goes to image_src.
    Static(image::DynamicImage),
    /// All GIF frames (composited RGBA + delay) → goes to gif_frames.
    Gif(Vec<(image::DynamicImage, std::time::Duration)>),
}

/// Result of media loading from another thread. Matched by generation via `gen`; results made stale by navigation are discarded.
pub struct MediaResult {
    gen: u64,
    /// None = decode/rasterize failure (the render side shows a fallback).
    payload: Option<MediaPayload>,
}

/// Heavy media loading to run on a separate thread (pure work that does not reference App).
enum MediaJob {
    /// Rasterize an SVG (path, max-edge px).
    Svg(PathBuf, u32),
    /// Expand all GIF frames. Falls back to still-image decode for single-frame/non-animated GIFs.
    Gif(PathBuf),
    /// Extract one representative frame from a video (delegated to ffmpegthumbnailer/ffmpeg). Thumbnail only; no playback.
    Video(PathBuf),
    /// Rasterize page N (1-based) of a PDF (delegated to pdftocairo/pdftoppm/qlmanage/sips). Loaded one page at a time.
    Pdf(PathBuf, u32),
}

impl MediaJob {
    /// Load the actual data (called on a separate thread or via the synchronous fallback). None on failure.
    fn run(self) -> Option<MediaPayload> {
        match self {
            MediaJob::Svg(p, max_px) => {
                crate::preview::svg::rasterize(&p, max_px).map(MediaPayload::Static)
            }
            MediaJob::Gif(p) => match crate::preview::image::decode_gif(&p) {
                Some(frames) => Some(MediaPayload::Gif(frames)),
                // 単一フレーム/アニメでない GIF → 静止画として表示。
                None => crate::preview::image::decode_static(&p).map(MediaPayload::Static),
            },
            MediaJob::Video(p) => crate::preview::video::thumbnail(&p).map(MediaPayload::Static),
            MediaJob::Pdf(p, page) => {
                crate::preview::pdf::render_page(&p, page).map(MediaPayload::Static)
            }
        }
    }
}

/// Result of the **heavy ignored set (`git::ignored`)** computed on a separate thread. Staleness is judged by `gen`, and
/// only the latest generation is applied (results from after moving to another repo while computing are discarded). `workdir` is the
/// repo workdir being computed; on apply it is put into `git_ignored_for` to serve as the Phase G cache key.
pub struct IgnoredResult {
    gen: u64,
    workdir: PathBuf,
    set: std::collections::HashSet<PathBuf>,
}

pub struct App {
    pub mode: Mode,
    pub root: PathBuf,
    /// The directory opened at startup. The base for relative-path display (kept separately because root moves up with h).
    pub open_dir: PathBuf,
    /// The directory opened at startup (immutable). The reset target for `A` ResetAnchor. Kept separately because open_dir moves with `a`.
    launch_dir: PathBuf,
    pub path_style: PathStyle,
    pub key_scheme: KeyScheme,
    pub lang: crate::i18n::Lang,
    pub cfg: Config,

    /// The tree flattened into display order. For M0 it is fine to simply rebuild it every time.
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub show_hidden: bool,
    /// Tree sort settings (FR / M7 auxiliary). Global (not per-tab). Changed via the `s` menu.
    pub sort: Sort,
    /// Whether the `s` sort-selection menu is showing (while true, main intercepts keys).
    sort_menu: bool,
    /// Bookmarks (M7 auxiliary). Lowercase a-z=local (per launch dir) / uppercase A-Z=global.
    pub bookmarks: crate::bookmarks::Bookmarks,
    /// State of waiting for a letter after `m`/`'` (Set=register / Jump=jump).
    /// Waiting for the letter key after `m` (bookmark set). (`'` opens the list directly.)
    mark_set_pending: bool,
    /// Whether the bookmark-list overlay is showing.
    bookmark_list: bool,
    /// Selection position in the list (a flat index over local on top and global below, concatenated).
    bookmark_list_sel: usize,
    /// Confirmation/input dialog (create `a` / rename `R` / delete `D`). While showing, main intercepts keys.
    dialog: Option<Dialog>,
    /// Multi-selection (M7 Phase B). The set of **absolute paths** selected via `V`/visual. The target of batch delete, etc.
    /// Held by path (not by index), so the selection is preserved across tree rebuilds and filtering.
    selection: BTreeSet<PathBuf>,
    /// Anchor of visual (range) selection mode (entries index). Visual mode is active while it is Some.
    /// Range = anchor to cursor. On confirm, it is taken into `selection`.
    visual_anchor: Option<usize>,
    /// File clipboard (M7 Phase B). Push with `Y`=copy/`X`=cut, apply with `P`=paste.
    clipboard: Option<Clipboard>,
    /// Height (in rows) of the tree display area at the last render. Used as the page size for paging.
    pub tree_viewport: u16,

    /// Tree filter (`/`). Some=filter is active (the query). entries holds the result of narrowing filter_pool.
    tree_filter: Option<String>,
    /// Editing buffer for the filter query. Some=input mode (intercepts keys).
    filter_input: Option<String>,
    /// All entries to filter from (recursively collected under root). Built once when `/` starts.
    filter_pool: Vec<Entry>,
    /// Changed-files-only tree filter (`C`). While true, entries is a flat list of the files with a git
    /// status under root (the review companion for AI-made changes; broot's `:gs` equivalent). Per tab.
    changed_filter: bool,
    /// Follow mode (`F`, global): while true, an external file change (e.g. an AI agent writing) jumps the
    /// tree selection to that file and opens its preview. Any other keypress turns it off (Zed-style).
    follow_mode: bool,
    /// Files changed during the current follow session (since `F` was last turned ON; first-change order,
    /// deduped). Powers the session-scoped `n`/`N` review in follow-opened diffs — "what just changed",
    /// not the whole dirty worktree. Kept after follow turns off (review continues); reset by the next ON.
    follow_session: Vec<PathBuf>,
    /// Whether the current GitDiff preview was opened by follow mode. While true, `n`/`N` and the title's
    /// position indicator use `follow_session` instead of the full git change set.
    diff_follow_scope: bool,

    /// The target, kind, and scroll position (vertical/horizontal) while in Preview mode.
    /// Horizontal scroll is used to view long lines when not wrapping (ui.wrap=false).
    /// To avoid scrolling past the end, the actual clamp is done at render time (when content and screen size are known).
    pub preview_path: Option<PathBuf>,
    pub preview_kind: Option<PreviewKind>,
    pub preview_scroll: u16,
    pub preview_hscroll: u16,
    /// Height (in rows) of the text display area at the last render. Used as the page size for paging.
    pub preview_viewport: u16,

    /// less-style windowed reading for Code/Text (does not read the whole file). Always Some during a Code/Text preview.
    /// While Some, scrolling uses preview_byte_top (the line-head byte) instead of preview_scroll.
    preview_win: Option<crate::preview::window::FileWindow>,
    /// Byte offset of the top line of the window (the scroll position when windowed).
    preview_byte_top: u64,
    /// Absolute line number of the top of the window (0-based). For the line-number gutter. Changes with scrolling, and
    /// is corrected from the total line count when reaching the end.
    preview_top_line: usize,
    /// Cache of the total line count (computed once with count_lines when line numbers are ON).
    preview_total_lines: Option<usize>,
    /// Cache of the highlighted window (avoids re-highlighting on every render).
    win_cache: Option<WinCache>,

    /// Whether the current Code preview is a "heavy first time" (waiting for grammar compilation of a cold language).
    /// While true, the render shows a loading display (indicator) or plain text (progressive).
    /// Judged with `code::is_ext_warm` at preview start; if already warm, false from the start (immediate coloring).
    hl_pending: bool,
    /// Whether a background-thread warm is in progress (prevents double-launch). Common to indicator/progressive.
    hl_warming: bool,
    /// Frame index of the central spinner (advanced by the run loop while waiting on the indicator = animation).
    spinner_frame: usize,

    /// The file requested for external-editor launch via `e`. The run loop takes it, suspends the TUI, and launches
    /// (because entering/leaving the terminal must happen on the render thread, here we only raise the "request").
    pending_edit: Option<PathBuf>,

    /// Flag requesting launch of an external git tool (lazygit, etc.) via `O`. The run loop takes it, suspends the TUI, and launches.
    pending_git_tool: bool,

    /// Cache of decorated preview content (Markdown/Mermaid).
    /// Avoids re-parsing (pulldown-cmark/syntect/mermaid layout) on every scroll.
    /// It depends on width (to fit mermaid to the display width), so the key is (path, width).
    md_cache: Option<MdCache>,

    /// `R`: show a Markdown/Mermaid preview as its raw source (windowed, like a code file) instead of the
    /// decorated render. Raw mode makes lines/columns match the file, so the 2D caret selection/copy works.
    md_raw: bool,

    /// Cache of raw diff lines for the GitDiff preview (per path). Avoids recomputing `git diff` every frame.
    diff_cache: Option<DiffCache>,
    gutter_cache: Option<GutterCache>,

    /// Interactive items in the Markdown preview (links + task checkboxes, collected on each render).
    /// Focus with Tab/⇧Tab; Enter opens a link / toggles a checkbox, Space toggles a checkbox.
    md_items: Vec<MdItem>,
    /// Index of the focused item (within md_items). None=nothing selected.
    focused_item: Option<usize>,

    /// In-preview search (`/`, less style). Some=search is active (the query). Highlights matches and moves with n/N.
    /// Currently only Code/Text (windowed) previews are supported.
    preview_search: Option<String>,
    /// Editing buffer for the search query. Some=input mode (intercepts keys, runs on Enter).
    search_input: Option<String>,
    /// For each occurrence: (line-head byte offset, 0-based line number, byte column within the line). The result of `find_all_matches`.
    /// Multiple occurrences on one line become separate elements (so `n`/`N` move per occurrence).
    search_matches: Vec<(u64, usize, usize)>,
    /// Index of the current match (within search_matches).
    search_idx: usize,

    /// Image backend (M2). None if the terminal is unsupported or uninitialized, in which case images fall back to text.
    /// For rendering, ui::preview passes `image` by &mut to StatefulImage. Resize/encode is
    /// offloaded to a separate thread via `img_tx`, and the result is applied in apply_image_resize.
    picker: Option<Picker>,
    img_tx: Option<UnboundedSender<ResizeRequest>>,
    pub image: Option<ThreadProtocol>,
    /// The source image (decoded). Zoom/pan crops this at render time and rebuilds the protocol
    /// (because ratatui-image has no offset pan).
    image_src: Option<image::DynamicImage>,
    /// Zoom factor. 1.0=the whole image fits, >1.0 zooms in (overflowing the viewport clips and enables pan).
    pub image_zoom: f64,
    /// Display center (image-normalized coordinates [0,1]). Moved by pan. Default is the center.
    image_center: (f64, f64),
    /// The most recently built crop rectangle (source-image px: x,y,w,h). The protocol is rebuilt only when it changes.
    image_crop: Option<(u32, u32, u32, u32)>,
    /// The most recent visible fraction (0..1, w/h). Less than 1 = the image overflows the viewport and is clipped = pan is possible.
    image_vis_frac: (f64, f64),

    /// PDF: current page (1-based). Each page is rasterized on demand (one at a time) into image_src.
    pdf_page: u32,
    /// PDF: total page count from poppler `pdfinfo`. None = unknown (no poppler) → page navigation disabled.
    pdf_pages: Option<u32>,
    /// mtime of the previewed media file at load time. Media (image/svg/gif/video/pdf) is reloaded on an
    /// FS event only when this changes (avoids re-decoding / re-running external tools on unrelated edits).
    preview_media_mtime: Option<std::time::SystemTime>,

    /// Parsed CSV/TSV table (Some while a table preview is active and parsing succeeded).
    /// None while not a table, or when parsing failed (then the preview degrades to raw text).
    table_data: Option<crate::preview::table::TableData>,
    /// Cell cursor: 0-based data-row index (header is a fixed non-selectable row above).
    table_cur_row: usize,
    /// Cell cursor: 0-based column index.
    table_cur_col: usize,
    /// First visible data row (vertical scroll). Adjusted at render time to keep the cursor visible.
    table_top_row: usize,
    /// First visible column (horizontal scroll). Adjusted at render time to keep the cursor visible.
    table_left_col: usize,
    /// Visible data-row count at the last render (the page size for PageUp/Down). Set by the renderer.
    table_viewport_rows: u16,

    /// Line cursor for windowed (Code/Text) previews: absolute 0-based logical line under the cursor.
    /// The current line is highlighted; `j`/`k` move it (the window follows). Only meaningful while windowed.
    preview_cursor_line: usize,
    /// Column cursor: 0-based char index within the logical line (`h`/`l` move it). The render clamps it to the
    /// line's length and writes the clamp back. Used for the block caret and charwise (`v`) selection.
    preview_cursor_col: usize,
    /// Visual-selection anchor `(line, col)`. Some = selecting. Charwise (`v`) uses both; linewise (`V`) uses the line.
    preview_visual_anchor: Option<(usize, usize)>,
    /// Whether the active selection is linewise (`V`, whole lines) rather than charwise (`v`, exact character range).
    preview_visual_linewise: bool,

    /// GIF animation (M6). All frames (composited RGBA) + each display duration. Empty means no animation (still image).
    /// A separate path from the still-image asynchronous ThreadProtocol: to avoid the "render an unencoded
    /// new protocol → nothing shows" churn in an animation whose frame changes each time, the current frame is **synchronously encoded**
    /// and atomically swapped into `gif_protocol` (see below). Zoom/pan share image_zoom/image_center.
    gif_frames: Vec<(image::DynamicImage, std::time::Duration)>,
    /// Index of the currently displayed GIF frame.
    gif_idx: usize,
    /// The time the current frame began showing (the start point for the next-frame deadline). None=before the first tick.
    gif_shown_at: Option<std::time::Instant>,
    /// The render protocol (kitty, etc.) of the current frame synchronously encoded to the display size. Drawn with the Image widget.
    gif_protocol: Option<Protocol>,
    /// The (frame index, crop rectangle) gif_protocol was built from. Re-encoded only when it changes (avoids wasteful re-encode).
    gif_proto_key: Option<(usize, (u32, u32, u32, u32))>,

    /// Sending end that offloads heavy media loading (SVG rasterize / GIF full-frame decode) to a separate thread.
    /// The result is received by main's poll loop and applied in apply_media. None=synchronous fallback (tests, etc.).
    media_tx: Option<std::sync::mpsc::Sender<MediaResult>>,
    /// Decoded/encoded cache for inline Markdown images, keyed by resolved absolute path.
    md_image_cache: std::collections::HashMap<PathBuf, MdImgEntry>,
    /// Sender that offloads inline-image decoding to a background thread.
    md_img_tx: Option<std::sync::mpsc::Sender<MdImageResult>>,
    /// Remote inline-image URLs whose background download is in flight (deduplicates fetches).
    md_remote_inflight: std::collections::HashSet<String>,
    /// Remote inline-image URLs whose download failed — do not retry; they show a text placeholder.
    md_remote_failed: std::collections::HashSet<String>,
    /// Sender that reports the completion of a background remote-image download to the run loop.
    md_remote_tx: Option<std::sync::mpsc::Sender<RemoteFetch>>,
    /// Sender that offloads inline-image encoding (resize + protocol) to the encode worker thread.
    md_enc_tx: Option<std::sync::mpsc::Sender<MdEncodeRequest>>,
    /// Media-load generation. Incremented in enter_preview/clear to make old thread results stale.
    media_gen: u64,
    /// Whether waiting on another thread's media load (used by the render side to show "Loading…").
    media_loading: bool,

    /// Run2 keymap (Surface × key → Action). Built from the config at startup.
    pub keymaps: crate::keymap::KeyMap,
    /// which-key leader-pending state (`Space`=file management / `y`=path copy). After the press, a
    /// which-key menu is shown in the footer and the next keystroke confirms/cancels (a generalization of the old two-key pending_chord).
    pub pending_leader: Option<crate::keymap::LeaderId>,

    /// A transient message (copy result, etc.). Cleared on the next key input. Shown in the status line.
    pub flash: Option<String>,

    /// Visibility state and scroll position of the `?` help (full key list) overlay.
    pub show_help: bool,
    pub help_scroll: u16,
    /// Visibility state of the `i` file-info popup.
    pub show_info: bool,

    /// Tabs (FR-5). Each tab is a tree context. The active tab's working state is held in the fields above as the source of truth.
    tabs: Vec<TabState>,
    active_tab: usize,

    /// git status (FR-7). Absolute path → kind. Cached per root and re-fetched when root changes.
    git_status: std::collections::HashMap<PathBuf, crate::git::FileStatus>,
    /// The set of top-level gitignore-excluded entries (`git::ignored`). Used to decide which tree entries are dimmed.
    /// Since it is a heavy computation, it is cached **per repo (workdir)** (`git_ignored_for`), so moving root to a
    /// subdirectory within the same repository does not rebuild it.
    git_ignored: std::collections::HashSet<PathBuf>,
    /// The root at which `git_status`/`git_branch` were computed. If it differs from the current root, a (cheap) re-fetch is needed.
    git_status_for: Option<PathBuf>,
    /// The **repo workdir** at which `git_ignored` (heavy) was computed. If it is the same, root moves within the same repository
    /// do not rebuild it (avoids the 410ms recomputation when descending into a subdirectory with `l`).
    git_ignored_for: Option<PathBuf>,
    /// The workdir for which `ignored` is being computed on a separate thread (a guard against duplicate dispatch). None=not computing.
    git_ignored_pending: Option<PathBuf>,
    /// Generation of the `ignored` computation. Incremented by +1 on dispatch; a result is applied only if it matches this generation
    /// (discards stale results from after moving to another repo while computing).
    git_ignored_gen: u64,
    /// Flag to **rebuild even for the same repo** because the ignore rules (`.gitignore`/`.git/info/exclude`) changed.
    /// Comes from FS events. Set when a recompute is wanted even if workdir is the same (the old set is kept until the result is applied).
    git_ignored_dirty: bool,
    /// Sender that returns results to the worker computing `ignored` in the background. If not attached (tests, etc.), a synchronous fallback is used.
    ignored_tx: Option<std::sync::mpsc::Sender<IgnoredResult>>,
    /// git diff layout (vertical/horizontal/Auto). Initialized from the `git.diff` setting and cycled with `s`. Used by both the GitDiff preview
    /// and the commit/working-tree detail.
    diff_layout: DiffLayout,
    /// Title override for the detail (git_detail). Used when opening all working-tree changes from the git view, etc.
    git_detail_title: Option<String>,
    /// The current branch name (fetched at the same time as git status). None if not a repo.
    git_branch: Option<String>,

    /// Whether the Git view (the changes hub) is showing. While true, content shows the change list instead of the tree, and
    /// main intercepts keys. When the feature is disabled or it is not a repo, open_git_view does not set it, so it is always false.
    git_view: bool,
    /// Cursor position in the Git view (index within git_view_entries).
    git_view_sel: usize,
    /// The list of changed files shown in the Git view (rebuilt after open/refresh/write).
    git_view_entries: Vec<crate::git::ChangeEntry>,
    /// Whether the GitDiff preview was entered from the Git view. If true, Esc/q returns to the Git view.
    came_from_git_view: bool,

    /// Some if showing the git log (linear, newest first). Shows a full-screen list in content, and main intercepts keys.
    /// When the feature is disabled, it is not a repo, or there are no commits, open_git_log does not set it, so it stays None.
    git_log: Option<Vec<crate::git::CommitInfo>>,
    /// Cursor position in the git log (index within git_log).
    git_log_sel: usize,
    /// Some if showing the commit detail (the DiffLine list from commit_diff). A full screen overlaid on the log.
    git_detail: Option<Vec<crate::git::DiffLine>>,
    /// The **full message, etc. shown at the top** of the commit detail (on Enter from log/graph). None for worktree diffs.
    git_detail_meta: Option<crate::git::CommitMeta>,
    /// Vertical scroll amount of the commit detail.
    git_detail_scroll: u16,
    /// Horizontal scroll amount of the commit detail (for long diff lines).
    git_detail_hscroll: u16,
    /// Number of visible rows when rendering the detail (for clamping g/G and scrolling; updated by the render side).
    git_detail_viewport: u16,
    /// Total line count when rendering the detail (the commit-message header + diff; for clamping the scroll limit; updated by the render side).
    git_detail_total: usize,
    /// Some if showing the branch list (`b`). Shows a full-screen list in content and main intercepts keys.
    git_branches: Option<Vec<crate::git::BranchInfo>>,
    /// Cursor position in the branch list (index within the **filtered** display list).
    git_branch_sel: usize,
    /// Branch filter query (`/`). Empty means all entries.
    git_branch_filter: String,
    /// Whether the branch filter is being typed (while true, main picks up keys as characters).
    git_branch_filtering: bool,
    /// Some if showing the commit graph (`G`, SourceTree/Git Graph style). Shown full-screen in content.
    git_graph: Option<Vec<crate::git::GraphRow>>,
    /// Cursor position in the graph (index within git_graph; sits on a commit row).
    git_graph_sel: usize,
    /// Phase 2: the base branch tip OID for the graph (if Some, pinned in a straight line on lane0). `s`=set / `x`=clear.
    git_graph_base: Option<String>,
    /// Display label for the base (for the title `base: …`; a branch name or a short hash).
    git_graph_base_label: Option<String>,
    /// The **set of local branch names shown** in the graph (for the cap/toggle when there are many branches).
    /// At startup, auto-selected up to `ui.graph_max_branches` by HEAD + base + recency. If it equals the total branch count, `--all`.
    git_graph_visible: std::collections::HashSet<String>,
    /// Legend (branch ⇄ lane color). Computed and cached on graph rebuild.
    git_graph_legend: Vec<crate::git::LegendEntry>,
    /// The number of local branches **hidden by the cap/toggle** (for the legend's `(+K hidden)`).
    git_graph_hidden: usize,
    /// Whether the branch-visibility panel (`b`) is open.
    git_graph_picker: bool,
    /// Cursor position in the panel (row = branch).
    git_graph_picker_sel: usize,
    /// Tentative selection while editing the panel (committed to `git_graph_visible` with Enter / discarded with q).
    git_graph_picker_set: std::collections::HashSet<String>,
    /// The graph's **priority order (display order of all local branches)**. Initialized by `[ui.graph_base_branches]`→HEAD→recency, and
    /// reordered for the current session only with the panel's `J`/`K`. Used for the order of the legend/panel/cap selection/base derivation.
    git_graph_order: Vec<String>,
    /// Whether `J`/`K` reordering was done in the panel (so the base is re-derived to the top branch on Enter confirm).
    git_graph_reordered: bool,
}

/// Path-copy kind (FR-6). Chosen by the key after `c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyKind {
    /// File name only (`cn`).
    Name,
    /// Full path (`cp`).
    Full,
    /// Path relative to where it was opened (`cr`).
    Relative,
    /// Full path of the parent directory (`cd`).
    Parent,
    /// `@relative/path` reference for AI agents (`y@`). Claude Code reads `@path` as file context, so
    /// the path is strictly relative to `open_dir` (the launch/anchor dir where the agent usually runs),
    /// **without** the leading directory name that `Relative` prepends for display.
    AtRef,
}

/// Commit-info copy kind (chosen after `y` in git log/graph/detail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub enum GitCopyKind {
    /// Short hash (7 digits).
    ShortHash,
    /// Full hash (40 digits).
    FullHash,
    /// Subject (the first line of the message).
    Subject,
    /// Full message (subject + body, multi-line).
    Message,
    /// Author name.
    Author,
    /// Date.
    Date,
}

/// CSV/TSV table copy kind (chosen after `y` in the table preview).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableCopyKind {
    /// The current cell's value.
    Cell,
    /// The current row's cells joined by the delimiter (`,` / tab).
    Row,
    /// The current column's header plus every cell value, one per line.
    Column,
}

/// Cache of decorated preview content. The key is (path, display width).
struct MdCache {
    path: PathBuf,
    width: u16,
    lines: Vec<Line<'static>>,
    /// Block-level inline images reserved within `lines` (Markdown only; empty otherwise).
    images: Vec<crate::preview::markdown::ImagePlacement>,
}

/// Cache of raw diff lines for the GitDiff preview. `file_diff` (the git call) does not depend on display width
/// (side-by-side/coloring is done separately at render time), so the key is the path only. This avoids re-invoking git on every
/// scroll/horizontal scroll/resize (aligned with log/graph: load once on open).
/// Since the diff changes when the working tree changes, it is invalidated by `refresh()` (FS events / manual refresh).
struct DiffCache {
    path: PathBuf,
    lines: Vec<crate::git::DiffLine>,
}

/// Cache of the editor-style git gutter marks (per new-file line) for the currently-previewed code/text
/// file. Keyed by path; a working-tree change (`refresh`) drops it so external edits re-derive. Avoids
/// re-invoking `file_diff` (a git call) on every scroll/keypress while previewing.
struct GutterCache {
    path: PathBuf,
    marks: std::collections::HashMap<u32, GutterMark>,
}

/// One interactive item within a rendered Markdown preview: a link or a task-list checkbox.
/// `line`=index of the decorated line (for scrolling). Both are collected straight from the render
/// result (links = underlined blue spans + collapsed targets, checkboxes = `task_marker_style` spans),
/// so no source re-parsing is needed.
struct MdItem {
    line: usize,
    kind: MdItemKind,
}

enum MdItemKind {
    /// A link; `target`=URL/relative path.
    Link { target: String },
    /// A task checkbox; `state`=its current state char (` `/`x`/custom) as shown.
    Task { state: char },
}

/// Cache of highlighted lines for windowed reading.
/// The key is (path, top byte, height). Rebuilt only when scroll/vertical resize changes it.
/// (Line content does not depend on display width, so width is not part of the key.)
struct WinCache {
    path: PathBuf,
    byte_top: u64,
    height: u16,
    lines: Vec<Line<'static>>,
}

/// A decoded inline Markdown image plus its background-encoded render protocol(s).
#[derive(Default)]
struct MdImgEntry {
    /// The decoded source image (None while the background decode is in flight). Shared with the encode
    /// worker via `Arc` (cheap clone) so cropping/encoding happens off the UI thread.
    decoded: Option<Arc<image::DynamicImage>>,
    /// The graphics protocol for the fully-visible image, encoded for `proto_size`.
    protocol: Option<Protocol>,
    /// The (cols, rows) `protocol` was encoded for.
    proto_size: Option<(u16, u16)>,
    /// The graphics protocol for a partially-scrolled image: only the visible vertical band, cropped and
    /// encoded so the image renders clipped instead of being hidden. Re-encoded when the band changes.
    clip_protocol: Option<Protocol>,
    /// The (cols, full_rows, row_off, vis_rows) band `clip_protocol` was encoded for.
    clip_key: Option<(u16, u16, u16, u16)>,
    /// An encode request is in flight on the worker thread (at most one per image, so scrolling does not
    /// queue a backlog — when it returns, the next render requests the then-current band).
    enc_inflight: bool,
    /// Decode or encode failed — do not retry; the placeholder/text fallback stays visible.
    failed: bool,
}

/// Which protocol a background encode produces (so the result is stored in the right slot).
#[derive(Clone, Copy, PartialEq, Debug)]
enum MdEncodeKey {
    /// The whole image at (cols, rows) cells.
    Full { cols: u16, rows: u16 },
    /// A cropped vertical band, keyed by (cols, full_rows, row_off, vis_rows).
    Clip {
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    },
}

/// A request to encode an inline image (or a cropped band of it) off the UI thread (principle #4).
pub struct MdEncodeRequest {
    path: PathBuf,
    key: MdEncodeKey,
    image: Arc<image::DynamicImage>,
    /// Pixel band to crop before encoding; None encodes the whole image.
    crop: Option<(u32, u32, u32, u32)>,
    cols: u16,
    rows: u16,
}

/// Result of a background inline-image encode, delivered to the run loop.
pub struct MdEncodeResult {
    path: PathBuf,
    key: MdEncodeKey,
    protocol: Protocol,
}

/// Background worker that encodes inline-image protocols (the whole image or a cropped band) with a
/// cloned Picker, so the UI thread never blocks on the resize/encode (principle #4). Exits when the
/// request sender is dropped (App teardown).
pub fn md_encode_worker(
    picker: Picker,
    rx: std::sync::mpsc::Receiver<MdEncodeRequest>,
    tx: std::sync::mpsc::Sender<MdEncodeResult>,
) {
    while let Ok(req) = rx.recv() {
        let img = match req.crop {
            Some((x, y, w, h)) => req.image.crop_imm(x, y, w, h),
            None => (*req.image).clone(),
        };
        let size = ratatui::layout::Size::new(req.cols, req.rows);
        if let Ok(protocol) =
            picker.new_protocol(img, size, Resize::Fit(Some(FilterType::Lanczos3)))
        {
            if tx
                .send(MdEncodeResult {
                    path: req.path,
                    key: req.key,
                    protocol,
                })
                .is_err()
            {
                break;
            }
        }
    }
}

/// Result of a background inline-image decode, delivered to the run loop.
pub struct MdImageResult {
    path: PathBuf,
    image: Result<image::DynamicImage, String>,
}

/// Result of a background remote-image download (curl → local cache file), delivered to the run loop.
/// It carries no bytes: on success the file is already in the cache, so the run loop just invalidates
/// the decoration cache to re-lay the image out; on failure the URL is remembered so it is not retried.
pub struct RemoteFetch {
    url: String,
    ok: bool,
}

impl App {
    pub fn new(root: PathBuf, cfg: Config) -> Result<Self> {
        let path_style = PathStyle::parse(&cfg.ui.path_style);
        let key_scheme = KeyScheme::parse(&cfg.ui.keys);
        let lang = crate::i18n::Lang::resolve(&cfg.ui.lang);
        let sort = Sort::from_config(&cfg.ui.sort);
        // diff の並び初期値 (設定 git.diff)。実行時 `s` で 縦→横→Auto を巡回。
        let diff_layout = DiffLayout::parse(&cfg.git.diff);
        // Run2 キーマップ: 既定 + 設定 (`[keys.<surface>]` + 旧 copy_* alias) をマージし衝突を検証する。
        // page/half プロファイルは ui.keys (vim/less) に従う。Stage 2 は構築のみ (ディスパッチ未接続)。
        let keymaps = crate::keymap::KeyMap::from_config(
            crate::keymap::scheme_from_str(&cfg.ui.keys),
            &cfg.keys.to_keymap_config(),
        );
        let mut app = Self {
            mode: Mode::Tree,
            root: root.clone(),
            open_dir: root.clone(),
            launch_dir: root.clone(),
            path_style,
            key_scheme,
            lang,
            cfg,
            entries: Vec::new(),
            selected: 0,
            show_hidden: false,
            sort,
            sort_menu: false,
            bookmarks: crate::bookmarks::Bookmarks::load(&root),
            mark_set_pending: false,
            bookmark_list: false,
            bookmark_list_sel: 0,
            dialog: None,
            selection: BTreeSet::new(),
            visual_anchor: None,
            clipboard: None,
            tree_viewport: 0,
            tree_filter: None,
            filter_input: None,
            filter_pool: Vec::new(),
            changed_filter: false,
            follow_mode: false,
            follow_session: Vec::new(),
            diff_follow_scope: false,
            preview_path: None,
            preview_kind: None,
            preview_scroll: 0,
            preview_hscroll: 0,
            preview_viewport: 0,
            preview_win: None,
            preview_byte_top: 0,
            preview_top_line: 0,
            preview_total_lines: None,
            win_cache: None,
            hl_pending: false,
            hl_warming: false,
            spinner_frame: 0,
            pending_edit: None,
            pending_git_tool: false,
            md_cache: None,
            md_raw: false,
            diff_cache: None,
            gutter_cache: None,
            md_items: Vec::new(),
            focused_item: None,
            preview_search: None,
            search_input: None,
            search_matches: Vec::new(),
            search_idx: 0,
            picker: None,
            img_tx: None,
            image: None,
            image_src: None,
            image_zoom: 1.0,
            image_center: (0.5, 0.5),
            image_crop: None,
            image_vis_frac: (1.0, 1.0),
            pdf_page: 1,
            pdf_pages: None,
            preview_media_mtime: None,
            table_data: None,
            table_cur_row: 0,
            table_cur_col: 0,
            table_top_row: 0,
            table_left_col: 0,
            table_viewport_rows: 0,
            preview_cursor_line: 0,
            preview_cursor_col: 0,
            preview_visual_anchor: None,
            preview_visual_linewise: false,
            gif_frames: Vec::new(),
            gif_idx: 0,
            gif_shown_at: None,
            gif_protocol: None,
            gif_proto_key: None,
            media_tx: None,
            md_image_cache: std::collections::HashMap::new(),
            md_img_tx: None,
            md_remote_inflight: std::collections::HashSet::new(),
            md_remote_failed: std::collections::HashSet::new(),
            md_remote_tx: None,
            md_enc_tx: None,
            media_gen: 0,
            media_loading: false,
            keymaps,
            pending_leader: None,
            flash: None,
            show_help: false,
            show_info: false,
            help_scroll: 0,
            tabs: Vec::new(),
            active_tab: 0,
            git_status: std::collections::HashMap::new(),
            git_ignored: std::collections::HashSet::new(),
            git_status_for: None,
            git_ignored_for: None,
            git_ignored_pending: None,
            git_ignored_gen: 0,
            git_ignored_dirty: false,
            ignored_tx: None,
            diff_layout,
            git_detail_title: None,
            git_branch: None,
            git_view: false,
            git_view_sel: 0,
            git_view_entries: Vec::new(),
            came_from_git_view: false,
            git_log: None,
            git_log_sel: 0,
            git_detail: None,
            git_detail_meta: None,
            git_detail_scroll: 0,
            git_detail_hscroll: 0,
            git_detail_viewport: 0,
            git_detail_total: 0,
            git_branches: None,
            git_branch_sel: 0,
            git_branch_filter: String::new(),
            git_branch_filtering: false,
            git_graph: None,
            git_graph_sel: 0,
            git_graph_base: None,
            git_graph_base_label: None,
            git_graph_visible: std::collections::HashSet::new(),
            git_graph_legend: Vec::new(),
            git_graph_hidden: 0,
            git_graph_picker: false,
            git_graph_picker_sel: 0,
            git_graph_picker_set: std::collections::HashSet::new(),
            git_graph_order: Vec::new(),
            git_graph_reordered: false,
        };
        app.rebuild_tree()?;
        // 最初のタブとして現在の状態を登録する。
        app.tabs.push(app.snapshot_tab());
        Ok(app)
    }

    /// Summarize startup keymap conflicts / ignored settings into one line (for the startup flash; i18n).
    /// Conflicts have already safely fallen back to defaults (#17/FR-8). None if there is nothing.
    pub fn keymap_report(&self) -> Option<String> {
        let nc = self.keymaps.conflicts.len();
        let nw = self.keymaps.warnings.len();
        if nc == 0 && nw == 0 {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if nc > 0 {
            parts.push(match self.lang {
                crate::i18n::Lang::Jp => format!("キー衝突{nc}件(既定で継続)"),
                crate::i18n::Lang::En => format!("{nc} key conflict(s) (using defaults)"),
            });
        }
        if nw > 0 {
            parts.push(match self.lang {
                crate::i18n::Lang::Jp => format!("無効な設定{nw}件を無視"),
                crate::i18n::Lang::En => format!("{nw} invalid key setting(s) ignored"),
            });
        }
        let head = tr(self.lang, crate::i18n::Msg::Keymap);
        Some(format!("{head}: {}", parts.join(" / ")))
    }

    /// Starting from directly under root, recursively expand the expanded directories and flatten them.
    /// A naive implementation for M0. Support for large directories will later be replaced with lazy loading.
    pub fn rebuild_tree(&mut self) -> Result<()> {
        let mut out = Vec::new();
        let expanded_dirs: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| e.is_dir && e.expanded)
            .map(|e| e.path.clone())
            .collect();

        build_dir(
            &self.root,
            0,
            &expanded_dirs,
            self.show_hidden,
            self.sort,
            &mut out,
        )?;
        self.entries = out;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        // entries を作り直したら visual_anchor(entries 添字)は stale。必ず無効化する。
        self.visual_anchor = None;
        Ok(())
    }

    /// Calls `rebuild_tree`, and on failure does not swallow it but reports via flash. Returns `true` on success.
    /// Used on paths that cannot bail out with `?` (bookmark jump, clearing the filter, rebuilding after a git discard).
    /// The return value lets the caller decide whether to show a subsequent success flash (=false means don't).
    fn rebuild_tree_notify(&mut self) -> bool {
        if let Err(e) = self.rebuild_tree() {
            self.flash = Some(format!(
                "{}{e}",
                crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
            ));
            false
        } else {
            true
        }
    }

    // --- ソート (FR・M7 補助) -------------------------------------------------
    /// Whether the `s` sort-selection menu is showing.
    pub fn is_sort_menu(&self) -> bool {
        self.sort_menu
    }
    /// Open the sort menu with `s`. main routes the next key here.
    pub fn open_sort_menu(&mut self) {
        self.sort_menu = true;
    }
    /// Close the sort menu (Esc or an out-of-scope key).
    pub fn close_sort_menu(&mut self) {
        self.sort_menu = false;
    }
    /// Key handling for the sort menu. n/s/m/e=select the key (closes after selecting), r=flip asc/desc, .=directories first
    /// (toggles keep it open). Out-of-scope keys close it. If the order changes, rebuild (while filtering, applied after clearing).
    pub fn sort_menu_key(&mut self, c: char) -> Result<()> {
        let keep_open = match c {
            'n' => {
                self.sort.key = SortKey::Name;
                false
            }
            's' => {
                self.sort.key = SortKey::Size;
                false
            }
            'm' => {
                self.sort.key = SortKey::Modified;
                false
            }
            'e' => {
                self.sort.key = SortKey::Ext;
                false
            }
            'r' => {
                self.sort.reverse = !self.sort.reverse;
                true
            }
            '.' => {
                self.sort.dirs_first = !self.sort.dirs_first;
                true
            }
            _ => {
                self.sort_menu = false;
                return Ok(());
            }
        };
        // 絞り込み中は結果順を変えない(クリア後にツリー再構築で反映)。
        if self.tree_filter.is_none() {
            self.rebuild_tree()?;
        }
        self.sort_menu = keep_open;
        Ok(())
    }
    /// Current sort display for the top context bar (e.g. `sort: mod ↑`).
    pub fn sort_label(&self) -> String {
        let k = match self.sort.key {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "mod",
            SortKey::Ext => "ext",
        };
        let arrow = if self.sort.reverse { "↓" } else { "↑" };
        format!("sort: {k} {arrow}")
    }

    // --- モード表示 (2軸: 表示モード × 内部モード) ----------------------------
    /// Display mode (for the outer chip). Preview becomes Image if it is an image.
    pub fn display_mode(&self) -> DisplayMode {
        match self.mode {
            Mode::Tree => DisplayMode::Tree,
            Mode::Preview => {
                if self.is_image_preview() {
                    DisplayMode::Image
                } else if self.is_table_preview() {
                    DisplayMode::Table
                } else {
                    DisplayMode::Preview
                }
            }
        }
    }
    /// Internal mode (for the inner chip and footer switching). None if nothing is being operated.
    /// Priority: dialog > filter/search/sort/mark/bookmarks > visual.
    pub fn internal_mode(&self) -> Option<InternalMode> {
        if let Some(d) = &self.dialog {
            return Some(match &d.kind {
                // アプリ終了確認は非破壊。削除/ドロップとは別チップ/フッターにする。
                DialogKind::Confirm { .. } if self.confirm_is_quit() => InternalMode::QuitConfirm,
                // D&D 転送は破壊的でないので削除確認とは別チップ/フッターにする。
                DialogKind::Confirm { .. } if self.confirm_is_drop() => InternalMode::DropConfirm,
                DialogKind::Confirm { .. } => InternalMode::DeleteConfirm,
                DialogKind::Preview { .. } => InternalMode::RenamePreview,
                DialogKind::Input { .. } => match &d.op {
                    PendingOp::Create { .. } => InternalMode::Create,
                    PendingOp::BatchRenameInput { .. } => InternalMode::BatchRename,
                    PendingOp::GitCommit => InternalMode::Commit,
                    PendingOp::GitCreateBranch => InternalMode::GitBranch,
                    _ => InternalMode::Rename,
                },
            });
        }
        // コミット詳細は log の上に被さるので先に判定する。
        if self.is_git_detail() {
            return Some(InternalMode::GitDetail);
        }
        if self.is_git_log() {
            return Some(InternalMode::GitLog);
        }
        // パネルはグラフの上に被さる(is_git_graph も true なので先に判定)。
        if self.is_git_graph_picker() {
            return Some(InternalMode::GitGraphPicker);
        }
        if self.is_git_graph() {
            return Some(InternalMode::GitGraph);
        }
        if self.is_git_branches() {
            return Some(InternalMode::GitBranch);
        }
        if self.is_git_view() {
            return Some(InternalMode::GitChanges);
        }
        if self.is_git_diff_preview() {
            return Some(InternalMode::GitDiff);
        }
        if self.is_filtering() {
            return Some(InternalMode::Filter);
        }
        if self.is_searching() {
            return Some(InternalMode::Search);
        }
        if self.is_sort_menu() {
            return Some(InternalMode::Sort);
        }
        if self.is_marking() {
            return Some(InternalMode::Mark);
        }
        if self.is_bookmark_list() {
            return Some(InternalMode::Bookmarks);
        }
        if self.is_info() {
            return Some(InternalMode::Info);
        }
        if self.is_visual() {
            return Some(InternalMode::Visual);
        }
        if self.is_preview_visual() {
            return Some(InternalMode::PreviewVisual);
        }
        if self.changed_filter && matches!(self.mode, Mode::Tree) {
            return Some(InternalMode::ChangedFilter);
        }
        None
    }

    /// The **frontmost surface** that currently receives keys (the single source of truth for Run2 keymap dispatch).
    /// Strictly follows the same priority as internal_mode, while also including the base full screens (Tree/Preview) and overlays (§4).
    /// Off the render path; references only existing bool predicates (same cost as now).
    pub fn surface(&self) -> crate::keymap::Surface {
        use crate::keymap::Surface as S;
        // ダイアログ最優先 (種別で細分: 入力 / 削除確認 / ドロップ確認 / リネームプレビュー)。
        if self.is_dialog() {
            if self.dialog_is_preview() {
                return S::DialogRenamePreview;
            }
            if self.dialog_is_confirm() {
                if self.confirm_is_quit() {
                    return S::DialogConfirmQuit;
                }
                if self.confirm_is_drop() {
                    return S::DialogConfirmDrop;
                }
                return S::DialogConfirmDelete;
            }
            return S::DialogInput;
        }
        // ヘルプ (全要素の上)。
        if self.show_help {
            return S::Help;
        }
        // Git オーバーレイ (詳細 > log > graph > branches > 変更ハブ > diff)。feature 無効時は到達しない。
        #[cfg(feature = "git")]
        {
            if self.is_git_detail() {
                return S::GitDetail;
            }
            if self.is_git_log() {
                return S::GitLog;
            }
            // パネルはグラフの上に被さる(is_git_graph も true なので先に判定)。
            if self.is_git_graph_picker() {
                return S::GitGraphPicker;
            }
            if self.is_git_graph() {
                return S::GitGraph;
            }
            if self.is_git_branches() {
                return if self.git_branch_filtering() {
                    S::BranchFilter
                } else {
                    S::GitBranches
                };
            }
            if self.is_git_view() {
                return S::GitChanges;
            }
            if self.is_git_diff_preview() {
                return S::PreviewGitDiff;
            }
        }
        // 入力系 / メニュー / 一覧 / 情報 / ビジュアル。
        if self.is_filtering() {
            return S::Filter;
        }
        if self.is_searching() {
            return S::Search;
        }
        if self.is_sort_menu() {
            return S::Sort;
        }
        if self.is_marking() {
            return S::Mark;
        }
        if self.is_bookmark_list() {
            return S::Bookmarks;
        }
        if self.is_info() {
            return S::Info;
        }
        if self.is_visual() {
            return S::Visual;
        }
        // 基本全画面 (Preview は画像/テキストで分岐)。
        match self.mode {
            Mode::Preview => {
                if self.is_image_preview() {
                    S::PreviewImage
                } else if self.is_table_preview() {
                    S::PreviewTable
                } else if self.is_preview_visual() {
                    S::PreviewTextVisual
                } else {
                    S::PreviewText
                }
            }
            Mode::Tree => S::Tree,
        }
    }

    // --- コピー/カット&ペースト (M7 Phase B・既定キー Y/X/P・keymap で変更可) ---
    /// Label for the context display: "copy 3" / "cut 3". None if the clipboard is empty.
    pub fn clipboard_label(&self) -> Option<String> {
        self.clipboard.as_ref().map(|c| {
            let verb = match c.op {
                ClipOp::Copy => crate::i18n::tr(self.lang, crate::i18n::Msg::CopyHint),
                ClipOp::Cut => crate::i18n::tr(self.lang, crate::i18n::Msg::CutHint),
            };
            format!("{verb} {}", c.paths.len())
        })
    }
    /// `Y`=copy: push the selection (or the cursor if none) to the clipboard (for duplication). The selection is cleared.
    pub fn copy_selection(&mut self) {
        let targets = self.op_targets();
        if targets.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return;
        }
        let n = targets.len();
        self.clipboard = Some(Clipboard {
            op: ClipOp::Copy,
            paths: targets,
        });
        self.clear_selection();
        self.flash = Some(format!(
            "{} ({n})",
            crate::i18n::tr(self.lang, crate::i18n::Msg::Copied)
        ));
    }
    /// `X`=cut: push the selection (or the cursor if none) to the clipboard (for moving). The selection is cleared.
    pub fn cut_selection(&mut self) {
        let targets = self.op_targets();
        if targets.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return;
        }
        let n = targets.len();
        self.clipboard = Some(Clipboard {
            op: ClipOp::Cut,
            paths: targets,
        });
        self.clear_selection();
        self.flash = Some(format!(
            "{} ({n})",
            crate::i18n::tr(self.lang, crate::i18n::Msg::CutDone)
        ));
    }
    /// `P`=paste: apply to the cursor-based directory. Copy duplicates, cut moves (consumed). **Never overwrites.**
    pub fn paste(&mut self) -> Result<()> {
        let Some(clip) = self.clipboard.clone() else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::ClipboardEmpty).into());
            return Ok(());
        };
        let dir = self.op_base_dir();
        let (mut ok, mut last, mut err) = (0usize, None, None);
        for src in &clip.paths {
            // 自分自身(やその中)への貼り付けは無限コピーになるので弾く。
            if dir.starts_with(src) {
                err = Some(
                    crate::i18n::tr(self.lang, crate::i18n::Msg::CannotPasteIntoSelf).to_string(),
                );
                continue;
            }
            let res = match clip.op {
                ClipOp::Copy => crate::fileops::copy_into(&dir, src),
                ClipOp::Cut => crate::fileops::move_into(&dir, src),
            };
            match res {
                Ok(p) => {
                    ok += 1;
                    last = Some(p);
                }
                Err(e) => err = Some(e.to_string()),
            }
        }
        if matches!(clip.op, ClipOp::Cut) {
            self.clipboard = None; // カットは消費(元が移動済み)
        }
        self.refresh()?;
        if let Some(p) = &last {
            self.reveal_and_select(p)?;
        }
        self.flash = Some(match err {
            Some(e) => format!(
                "{} {ok} / {}: {e}",
                crate::i18n::tr(self.lang, crate::i18n::Msg::Pasted),
                crate::i18n::tr(self.lang, crate::i18n::Msg::Failed),
            ),
            None => format!(
                "{} ({ok})",
                crate::i18n::tr(self.lang, crate::i18n::Msg::Pasted)
            ),
        });
        Ok(())
    }

    /// The selection listed in the **current sort order (tree display order)**. Used as the numbering order for sequential rename.
    /// Selections not present in entries (collapsed, etc.) are appended at the end in path order.
    fn selection_in_display_order(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| self.selection.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();
        for p in &self.selection {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
        v
    }
    /// Heading for the batch-rename input dialog (the count + a placeholder legend).
    fn batch_rename_title(&self, count: usize) -> String {
        format!(
            "{} {} {}  {{n}} {{n:0W}} {{name}} {{ext}}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::BatchRename),
            count,
            crate::i18n::tr(self.lang, crate::i18n::Msg::Items),
        )
    }
    /// `R` (with a selection)=batch rename. Opens template input that numbers the selection sequentially in display order.
    pub fn start_batch_rename(&mut self) {
        let targets = self.selection_in_display_order();
        if targets.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return;
        }
        let title = self.batch_rename_title(targets.len());
        self.dialog = Some(Dialog {
            op: PendingOp::BatchRenameInput { targets },
            kind: DialogKind::Input {
                title,
                buffer: String::new(),
                cursor: 0,
            },
        });
    }

    /// `a`=create. Opens an input dialog to create with the entered name in the cursor-based directory (a trailing `/` makes a folder).
    pub fn start_create(&mut self) {
        let dir = self.op_base_dir();
        let where_ = self.format_path(&dir);
        self.dialog = Some(Dialog {
            op: PendingOp::Create { dir },
            kind: DialogKind::Input {
                title: format!(
                    "{} in {where_}  ({})",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::Create),
                    crate::i18n::tr(self.lang, crate::i18n::Msg::TrailingSlashFolder),
                ),
                buffer: String::new(),
                cursor: 0,
            },
        });
    }

    /// `R`=rename. An input dialog to change the cursor's file/directory name (prefilled with the current name).
    pub fn start_rename(&mut self) {
        let Some(target) = self.entries.get(self.selected).map(|e| e.path.clone()) else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return;
        };
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let cursor = name.chars().count(); // 末尾にカーソルを置く。
        self.dialog = Some(Dialog {
            op: PendingOp::Rename { target },
            kind: DialogKind::Input {
                title: crate::i18n::tr(self.lang, crate::i18n::Msg::Rename).into(),
                buffer: name,
                cursor,
            },
        });
    }

    /// `D`=delete (with confirmation). Opens a confirmation dialog targeting **all selected items if there is a selection**, or the cursor item otherwise.
    /// `y`=send to trash (recoverable) / `!`=permanent delete (unrecoverable) / `n`,Esc=cancel.
    pub fn start_delete(&mut self) {
        let targets = self.op_targets();
        if targets.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return;
        }
        // メッセージ: 1件は名前、複数件は「N 件 (先頭名 他)」。
        let label = if targets.len() == 1 {
            targets[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string()
        } else {
            let first = targets[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            format!(
                "{} {} ({first} {})",
                targets.len(),
                crate::i18n::tr(self.lang, crate::i18n::Msg::Items),
                crate::i18n::tr(self.lang, crate::i18n::Msg::Etc),
            )
        };
        self.dialog = Some(Dialog {
            op: PendingOp::Delete { targets },
            kind: DialogKind::Confirm {
                message: format!(
                    "{}: {label}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::DeleteTarget),
                ),
                allow_permanent: true,
            },
        });
    }

    /// A mutable reference to the (buffer, cursor) being edited. None if not an input dialog.
    fn dialog_input_mut(&mut self) -> Option<(&mut String, &mut usize)> {
        match self.dialog.as_mut() {
            Some(Dialog {
                kind: DialogKind::Input { buffer, cursor, .. },
                ..
            }) => Some((buffer, cursor)),
            _ => None,
        }
    }

    /// Insert one character at the cursor position in the input dialog and advance the cursor by one.
    pub fn dialog_input_push(&mut self, c: char) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            let at = char_byte(buffer, *cursor);
            buffer.insert(at, c);
            *cursor += 1;
        }
    }
    /// Delete the character just before the cursor (Backspace).
    pub fn dialog_input_backspace(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            if *cursor > 0 {
                let start = char_byte(buffer, *cursor - 1);
                let end = char_byte(buffer, *cursor);
                buffer.replace_range(start..end, "");
                *cursor -= 1;
            }
        }
    }
    /// Delete the character at the cursor position (Delete). The cursor does not move.
    pub fn dialog_input_delete(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            let count = buffer.chars().count();
            if *cursor < count {
                let start = char_byte(buffer, *cursor);
                let end = char_byte(buffer, *cursor + 1);
                buffer.replace_range(start..end, "");
            }
        }
    }
    /// Move the cursor left (←).
    pub fn dialog_cursor_left(&mut self) {
        if let Some((_, cursor)) = self.dialog_input_mut() {
            *cursor = cursor.saturating_sub(1);
        }
    }
    /// Move the cursor right (→). Clamped at the end.
    pub fn dialog_cursor_right(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            *cursor = (*cursor + 1).min(buffer.chars().count());
        }
    }
    /// Move the cursor to the start (Home).
    pub fn dialog_cursor_home(&mut self) {
        if let Some((_, cursor)) = self.dialog_input_mut() {
            *cursor = 0;
        }
    }
    /// Move the cursor to the end (End).
    pub fn dialog_cursor_end(&mut self) {
        if let Some((buffer, cursor)) = self.dialog_input_mut() {
            *cursor = buffer.chars().count();
        }
    }
    /// Cancel the dialog (Esc / confirm n).
    pub fn dialog_cancel(&mut self) {
        self.dialog = None;
    }

    /// Handle a quit request (`q` at the top level / `Q` from anywhere). If `ui.confirm_quit` is on,
    /// open a yes/no confirmation dialog and return `true` (the caller must NOT quit yet). Otherwise
    /// return `false` (the caller quits immediately).
    pub fn request_quit(&mut self) -> bool {
        if !self.cfg.ui.confirm_quit {
            return false;
        }
        self.dialog = Some(Dialog {
            op: PendingOp::Quit,
            kind: DialogKind::Confirm {
                message: crate::i18n::tr(self.lang, crate::i18n::Msg::QuitConfirm).into(),
                allow_permanent: false,
            },
        });
        true
    }

    /// Confirm the text-input dialog (Enter). Performs create/rename.
    pub fn dialog_submit(&mut self) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        let DialogKind::Input { buffer, .. } = dialog.kind else {
            return Ok(()); // 確認ダイアログはここへ来ない
        };
        // コミットは名前と扱いが違う(末尾 / を剥がさない・独自の空チェック)ので先に分岐。
        if matches!(dialog.op, PendingOp::GitCommit) {
            let message = buffer.trim();
            if message.is_empty() {
                self.flash =
                    Some(crate::i18n::tr(self.lang, crate::i18n::Msg::MessageEmpty).into());
                return Ok(());
            }
            match crate::git::commit(&self.root, message) {
                Ok(()) => {
                    // ステージ済み index でコミット成功 → git データ再取得＋ビュー更新。
                    self.refresh()?;
                    if self.is_git_view() {
                        self.git_view_reload();
                    }
                    self.flash =
                        Some(crate::i18n::tr(self.lang, crate::i18n::Msg::Committed).into());
                }
                Err(e) => {
                    // 失敗(stderr)を表示し、同じメッセージで入力ダイアログを再オープン(やり直せる)。
                    self.flash = Some(format!("{e}"));
                    let cursor = message.chars().count();
                    self.dialog = Some(Dialog {
                        op: PendingOp::GitCommit,
                        kind: DialogKind::Input {
                            title: crate::i18n::tr(self.lang, crate::i18n::Msg::CommitMessage)
                                .into(),
                            buffer: message.to_string(),
                            cursor,
                        },
                    });
                }
            }
            return Ok(());
        }
        // 新規ブランチ作成(コミット同様、名前を素直に trim して扱う)。
        if matches!(dialog.op, PendingOp::GitCreateBranch) {
            let bname = buffer.trim();
            if bname.is_empty() {
                self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NameEmpty).into());
                return Ok(());
            }
            match crate::git::create_branch(&self.root, bname) {
                Ok(()) => {
                    self.refresh()?;
                    self.refresh_git_if_needed(); // ブランチ名/状態のキャッシュを即更新
                    self.close_git_branches(); // 作成＆切替済み → 一覧を閉じて Git ビューへ
                    self.flash = Some(format!(
                        "{}: {bname}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::CreatedBranch)
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!("{e}"));
                    let cursor = bname.chars().count();
                    self.dialog = Some(Dialog {
                        op: PendingOp::GitCreateBranch,
                        kind: DialogKind::Input {
                            title: crate::i18n::tr(self.lang, crate::i18n::Msg::NewBranch).into(),
                            buffer: bname.to_string(),
                            cursor,
                        },
                    });
                }
            }
            return Ok(());
        }
        let name = buffer.trim().trim_end_matches('/').trim();
        let want_folder = buffer.trim_end().ends_with('/');
        if name.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NameEmpty).into());
            return Ok(());
        }
        match dialog.op {
            PendingOp::Create { dir } => {
                let created = if want_folder {
                    crate::fileops::create_dir(&dir, name)
                } else {
                    crate::fileops::create_file(&dir, name)
                };
                match created {
                    Ok(path) => {
                        self.refresh()?;
                        self.reveal_and_select(&path)?;
                        self.flash = Some(format!(
                            "{}: {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Created),
                            self.format_path(&path)
                        ));
                    }
                    Err(e) => {
                        self.flash = Some(format!(
                            "{}: {e}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                        ))
                    }
                }
            }
            PendingOp::Rename { target } => match crate::fileops::rename(&target, name) {
                Ok(path) => {
                    self.refresh()?;
                    self.reveal_and_select(&path)?;
                    self.flash = Some(format!(
                        "{}: {}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Renamed),
                        self.format_path(&path)
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            },
            PendingOp::BatchRenameInput { targets } => {
                match build_rename_plan(&targets, name) {
                    Ok(plan) => {
                        // プレビュー(旧 → 新)へ遷移。適用は dialog_preview_apply。
                        let lines: Vec<String> = plan
                            .iter()
                            .map(|(s, d)| {
                                format!(
                                    "{}  →  {}",
                                    s.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                    d.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                )
                            })
                            .collect();
                        let title = format!(
                            "{} {} {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Rename),
                            plan.len(),
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Items),
                        );
                        self.dialog = Some(Dialog {
                            op: PendingOp::BatchRenameApply { plan },
                            kind: DialogKind::Preview {
                                title,
                                lines,
                                scroll: 0,
                            },
                        });
                    }
                    Err(e) => {
                        // 入力をやり直せるよう、同じテンプレで入力ダイアログを再オープン。
                        self.flash = Some(format!(
                            "{}: {e}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                        ));
                        let cursor = name.chars().count();
                        let title = self.batch_rename_title(targets.len());
                        self.dialog = Some(Dialog {
                            op: PendingOp::BatchRenameInput { targets },
                            kind: DialogKind::Input {
                                title,
                                buffer: name.to_string(),
                                cursor,
                            },
                        });
                    }
                }
            }
            // 削除/Git 破棄は確認ダイアログ側 / 一括リネーム適用はプレビュー側。
            // GitCommit は上で早期 return 済みなので到達しない。
            PendingOp::Delete { .. }
            | PendingOp::BatchRenameApply { .. }
            | PendingOp::GitDiscard { .. }
            | PendingOp::GitCommit
            | PendingOp::GitCreateBranch
            | PendingOp::GitDeleteBranch { .. }
            | PendingOp::DropTransfer { .. }
            | PendingOp::Quit => {}
        }
        Ok(())
    }

    /// In the preview, `y`=apply the batch rename. Two-phase rename → reload → clear selection.
    pub fn dialog_preview_apply(&mut self) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        if let PendingOp::BatchRenameApply { plan } = dialog.op {
            match crate::fileops::batch_rename(&plan) {
                Ok(()) => {
                    self.refresh()?;
                    self.clear_selection();
                    self.flash = Some(format!(
                        "{} ({})",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Renamed),
                        plan.len()
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            }
        }
        Ok(())
    }

    /// Response to a confirmation dialog. yes=execute (delete → trash, recoverable) / no=cancel.
    pub fn dialog_confirm(&mut self, yes: bool) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        if !yes {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::Canceled).into());
            return Ok(());
        }
        match dialog.op {
            PendingOp::Delete { targets } => match crate::fileops::move_to_trash(&targets) {
                Ok(()) => {
                    self.refresh()?;
                    self.clear_selection();
                    self.flash = Some(format!(
                        "{} ({})",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::MovedToTrash),
                        targets.len()
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            },
            // Git ビューの破棄: git::discard → 一覧/ツリーの git status を取り直す。
            // GitDiff プレビューからの破棄(came_from_git_view)なら Git ビューを開き直して戻す。
            PendingOp::GitDiscard { path } => match crate::git::discard(&self.root, &path) {
                Ok(()) => {
                    let from_diff = self.is_git_diff_preview();
                    if from_diff {
                        // プレビューを畳んで Git ビューへ復帰。
                        self.came_from_git_view = false;
                        self.back_to_tree();
                        self.open_git_view();
                    }
                    self.git_view_reload();
                    // 破棄自体は成功。ツリー再構築が失敗した時はその旨を通知し、
                    // 成功 flash で上書きしない(誤って「成功」と見せない)。
                    if self.rebuild_tree_notify() {
                        self.flash = Some(format!(
                            "{}: {}",
                            crate::i18n::tr(self.lang, crate::i18n::Msg::Discarded),
                            self.format_path(&path)
                        ));
                    }
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            },
            // ブランチ削除(安全): git branch -d。失敗(未マージ等)は git の stderr を flash。
            PendingOp::GitDeleteBranch { name } => self.git_delete_branch(&name, false),
            _ => {}
        }
        Ok(())
    }

    /// In a confirmation dialog, `!`=**permanent delete / force** (delete=without going through the trash / branch=`-D` force).
    pub fn dialog_delete_permanent(&mut self) -> Result<()> {
        let Some(dialog) = self.dialog.take() else {
            return Ok(());
        };
        // ブランチの強制削除 (`-D`)。
        if let PendingOp::GitDeleteBranch { name } = &dialog.op {
            self.git_delete_branch(&name.clone(), true);
            return Ok(());
        }
        if let PendingOp::Delete { targets } = dialog.op {
            match crate::fileops::delete_permanently(&targets) {
                Ok(()) => {
                    self.refresh()?;
                    self.clear_selection();
                    self.flash = Some(format!(
                        "{} ({})",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::DeletedPermanently),
                        targets.len()
                    ));
                }
                Err(e) => {
                    self.flash = Some(format!(
                        "{}: {e}",
                        crate::i18n::tr(self.lang, crate::i18n::Msg::Failed)
                    ))
                }
            }
        }
        Ok(())
    }

    pub fn tree_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    pub fn tree_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// To the top of the tree.
    pub fn tree_first(&mut self) {
        self.selected = 0;
    }

    /// To the bottom of the tree.
    pub fn tree_last(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }

    /// Page the tree by one (dir: +1=next / -1=previous). Overlaps one row to keep context.
    pub fn tree_page(&mut self, dir: i32) {
        let page = self.tree_viewport.saturating_sub(1).max(1) as i32;
        self.tree_move(dir * page);
    }

    /// Half-page the tree (vim: Ctrl-d/Ctrl-u, less: d/u).
    pub fn tree_half_page(&mut self, dir: i32) {
        let half = (self.tree_viewport / 2).max(1) as i32;
        self.tree_move(dir * half);
    }

    /// Move the selection by delta rows and clamp it to [0, end].
    fn tree_move(&mut self, delta: i32) {
        self.selected = clamp_cursor(self.selected, delta, self.entries.len());
    }

    /// If a directory, toggle expansion; if a file, transition to Preview.
    /// While filtering (a flat result list), expand-toggle is meaningless, so behave the same as `tree_descend`.
    pub fn tree_activate(&mut self) -> Result<()> {
        if self.tree_filter.is_some() || self.changed_filter {
            return self.tree_descend();
        }
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return Ok(());
        };
        if entry.is_dir {
            // expanded をトグルして再構築
            if let Some(e) = self.entries.get_mut(self.selected) {
                e.expanded = !e.expanded;
            }
            self.rebuild_tree()?;
        } else {
            self.enter_preview(&entry.path);
        }
        Ok(())
    }

    /// Before switching root, discard state that points to items of the old root.
    /// `selection` is held by path, so under a different root it stays invisible yet becomes a target for mis-operations (a footgun), and
    /// `visual_anchor` is an entries index, so it goes stale under a different root. The filter and preview search are also
    /// for the old root, so they are cleared together (rebuild_tree also invalidates visual_anchor, but we do it explicitly).
    fn clear_for_root_change(&mut self) {
        self.selection.clear();
        self.visual_anchor = None;
        self.clear_filter_state();
        self.search_clear();
    }

    /// Move to the parent directory (raise the root). For `h`. While filtering, first clear the filter (the normal tree of the current root).
    pub fn tree_leave(&mut self) -> Result<()> {
        if self.tree_filter.is_some() {
            self.filter_clear();
            return Ok(());
        }
        if self.changed_filter {
            self.toggle_changed_filter(); // OFF に戻す(通常ツリーへ)
            return Ok(());
        }
        if let Some(parent) = self.root.parent().map(Path::to_path_buf) {
            self.clear_for_root_change();
            self.root = parent;
            self.entries.clear();
            self.selected = 0;
            self.rebuild_tree()?;
        }
        Ok(())
    }

    /// Descend into the selected directory (make that directory the new root). For `l`.
    /// Symmetric to `h` (to the parent). If a file, transition to Preview.
    /// Works on filter results too: directory → move there and clear the filter / file → preview.
    pub fn tree_descend(&mut self) -> Result<()> {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return Ok(());
        };
        let was_filtering = self.tree_filter.is_some();
        if entry.is_dir {
            // root を変えるので旧 root の選択/ビジュアル/絞り込み/検索を破棄する(持ち越さない)。
            self.clear_for_root_change();
            self.root = entry.path;
            self.entries.clear();
            self.selected = 0;
            self.rebuild_tree()?;
        } else {
            // ファイルは絞り込みを保ったままプレビュー(戻ると結果一覧に復帰)。
            let _ = was_filtering;
            self.enter_preview(&entry.path);
        }
        Ok(())
    }

    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.show_hidden = !self.show_hidden;
        self.rebuild_tree()
    }

    // --- フォローモード (`F`) — Agent Watch ② -------------------------------------
    /// `F`: toggle follow mode (auto-jump to externally changed files; the "watch the AI work" view).
    /// Turning it ON starts a fresh follow session (the "what changed while following" list for `n`/`N`).
    pub fn toggle_follow(&mut self) {
        self.follow_mode = !self.follow_mode;
        if self.follow_mode {
            self.follow_session.clear();
        }
        let msg = if self.follow_mode {
            crate::i18n::Msg::FollowOn
        } else {
            crate::i18n::Msg::FollowOff
        };
        self.flash = Some(tr(self.lang, msg).into());
    }

    /// Whether follow mode is on (chip display / the run loop's jump gate).
    pub fn follow_enabled(&self) -> bool {
        self.follow_mode
    }

    /// The user took over the keyboard (pressed any key not bound to `F`): stop following (Zed-style —
    /// following is a hands-off state; re-enable with one `F`). Flash so the state change is visible.
    pub fn follow_break(&mut self) {
        if self.follow_mode {
            self.follow_mode = false;
            self.flash = Some(tr(self.lang, crate::i18n::Msg::FollowOff).into());
        }
    }

    /// Follow-mode jump: reveal `path` in the tree and open its preview. Gated to meaningful targets:
    /// under root, an existing visible file, not gitignored, not already being previewed (the preview
    /// auto-reload handles content changes of the current file). No-op outside Tree/Preview surfaces
    /// (dialogs and git views are never hijacked — normally unreachable anyway since any key breaks follow).
    pub fn follow_jump(&mut self, path: &Path) {
        use crate::keymap::Surface;
        if !self.follow_mode {
            return;
        }
        // フォロー自身が開いた diff ビュー(PreviewGitDiff)からも次のファイルへ追従を続ける。
        // それ以外の git ビュー/ダイアログ等は乗っ取らない(通常はキー押下で follow が切れるので届かない)。
        let surface_ok = matches!(
            self.surface(),
            Surface::Tree | Surface::PreviewText | Surface::PreviewImage | Surface::PreviewTable
        );
        #[cfg(feature = "git")]
        let surface_ok = surface_ok || matches!(self.surface(), Surface::PreviewGitDiff);
        if !surface_ok {
            return;
        }
        if self.preview_path.as_deref() == Some(path) {
            return;
        }
        if !self.follow_target_ok(path) {
            return;
        }
        // 追尾がビューを動かす瞬間に古い flash(「follow: on」等)を消す=フッターを
        // その面の操作ヒントに明け渡す(flash は本来キー入力で消えるが、追尾はキー無しで進むため)。
        self.flash = None;
        if self.changed_filter {
            // 変更一覧を最新化してから対象を選択(一覧に居るはず=変更イベント由来)。
            self.reapply_changed_filter();
            if let Some(i) = self.entries.iter().position(|e| e.path == path) {
                self.selected = i;
            }
        } else if !matches!(self.reveal_path_deep(path), Ok(true)) {
            return;
        }
        // 既定(`ui.follow_view="diff"`)は**全画面 diff** で開く=何から何に変わったかがハンク単位で
        // 見える(hunk/livediff/diffpane と同じ提示)。diff を出せない未追跡(全行新規)・リポジトリ外や、
        // バイナリのメディア系はファイルプレビュー(+最初の変更ハンクへスクロール)へフォールバック。
        if self.cfg.ui.follow_view != "file" && !self.follow_is_media(path) {
            let diff = crate::git::file_diff(&self.root, path);
            if !diff.is_empty() {
                self.open_git_diff(path);
                // いま取った diff をキャッシュに載せ、描画での再取得(git 再実行)を省く。
                self.diff_cache = Some(DiffCache {
                    path: path.to_path_buf(),
                    lines: diff,
                });
                // フォロー由来の diff: n/N と位置表示は「このセッションで変わったファイル」を回遊。
                self.diff_follow_scope = true;
                return;
            }
        }
        self.enter_preview(path);
        self.follow_scroll_to_first_change();
    }

    /// Whether `path` is a valid follow target: under root, an existing file, not gitignored, and not
    /// inside a hidden (dot) directory unless hidden files are shown. Shared by the jump and the
    /// session-list recording so both see the same set.
    fn follow_target_ok(&self, path: &Path) -> bool {
        if !path.starts_with(&self.root) || !path.is_file() || self.is_ignored(path) {
            return false;
        }
        if !self.show_hidden {
            // 隠し(ドット)ディレクトリ/ファイル配下はツリーに出せないので追わない(show_hidden で解禁)。
            let hidden = path
                .strip_prefix(&self.root)
                .map(|r| {
                    r.components().any(|c| {
                        c.as_os_str()
                            .to_str()
                            .map(|s| s.starts_with('.'))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(true);
            if hidden {
                return false;
            }
        }
        true
    }

    /// Record one changed file into the current follow session (called from the run loop's event drain
    /// while follow is ON). Returns whether the path is a valid follow target (the caller uses this to
    /// decide whether it may become the pending jump target). First-change order, deduped.
    pub fn follow_note_change(&mut self, path: &Path) -> bool {
        if !self.follow_mode || !self.follow_target_ok(path) {
            return false;
        }
        if !self.follow_session.iter().any(|p| p == path) {
            self.follow_session.push(path.to_path_buf());
        }
        true
    }

    /// Whether `path` previews as media (image/SVG/video/PDF) — its git diff would be a useless
    /// "binary files differ" line, so follow shows the content preview instead.
    fn follow_is_media(&self, path: &Path) -> bool {
        matches!(
            self.cfg.resolve_preview(path),
            PreviewKind::Image(_)
                | PreviewKind::Svg(_)
                | PreviewKind::Video(_)
                | PreviewKind::Pdf(_)
        )
    }

    /// After a follow jump, scroll the (windowed) preview to the **first changed hunk** instead of the
    /// file top — an edit deep in a large file would otherwise be off-screen and invisible, which defeats
    /// "watch the agent work" (Zed follows the edit position; diffpane scrolls to the latest change).
    /// A few context lines are kept above, and the caret lands on the changed line (ready for `v`/`Y`).
    /// No-ops for non-windowed previews, untracked files (all-new → top is right), and outside a repo.
    fn follow_scroll_to_first_change(&mut self) {
        if !self.is_windowed() {
            return;
        }
        let Some(path) = self.preview_path.clone() else {
            return;
        };
        // ガター設定 OFF でも変更行は取得する(ON なら同じ計算が gutter_cache に載り描画でも再利用)。
        let marks = if self.cfg.ui.git_gutter {
            self.git_gutter_marks()
        } else {
            gutter_marks(&crate::git::file_diff(&self.root, &path))
        };
        let Some(first) = marks.keys().min().copied() else {
            return;
        };
        let line0 = (first as usize).saturating_sub(1); // marks は 1-based
        let top = line0.saturating_sub(3); // 上に文脈を数行残す
        let Some(win) = self.preview_win.as_mut() else {
            return;
        };
        if let Ok((off, _)) = win.advance(0, top) {
            self.preview_byte_top = off;
            self.preview_top_line = top;
            self.preview_cursor_line = line0;
        }
    }

    fn enter_preview(&mut self, path: &Path) {
        self.preview_path = Some(path.to_path_buf());
        // 設定駆動でプレビュー種別を解決。未対応は CanNotPreview。
        let kind = self.cfg.resolve_preview(path);
        self.preview_scroll = 0;
        self.preview_hscroll = 0;
        self.preview_byte_top = 0;
        self.preview_top_line = 0;
        // 画像状態は毎回リセット。SVG/GIF は別スレッドで読み込み開始(UI を塞がない)。
        self.clear_image();
        // PDF はページ数を先に求める(pdfinfo・~数 ms)。None なら単ページ扱い=ページ移動なし。
        if matches!(kind, PreviewKind::Pdf(_)) {
            self.pdf_pages = crate::preview::pdf::page_count(path);
        }
        self.start_media_load(&kind, path);
        self.preview_kind = Some(kind);
        self.md_raw = false; // 新規ファイルは装飾表示から(Markdown/Mermaid)。`R` で raw へ。
        self.md_cache = None; // 別ファイルに切替: 装飾キャッシュを無効化
        self.md_items.clear();
        self.md_image_cache.clear(); // 別ファイル: インライン画像キャッシュも破棄
        self.focused_item = None;
        self.preview_search = None;
        self.search_input = None;
        self.search_matches.clear();
        self.search_idx = 0;
        self.setup_windowed(); // 大きい Code/Text なら less 風ウィンドウ読みに切替
                               // windowed プレビューの 2D キャレット/選択を先頭へリセット。
        self.preview_cursor_line = 0;
        self.preview_cursor_col = 0;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        // CSV/TSV はテーブルへパース。カーソル/スクロールを先頭へ戻してから読み込む。
        self.table_cur_row = 0;
        self.table_cur_col = 0;
        self.table_top_row = 0;
        self.table_left_col = 0;
        self.load_table();
        self.mode = Mode::Preview;
    }

    /// Code/Text previews use less-style windowed reading **regardless of size**.
    /// This avoids whole-file highlighting (1.3s/debug for 1600 lines) and reads/colors only the visible window (tens of lines), so
    /// opening is instant even for small files and behavior is symmetric with huge files. Images/md/mermaid are excluded
    /// (md/mermaid need their whole structure understood and are usually small).
    fn setup_windowed(&mut self) {
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        // 重いハイライト待ち状態を判定: Code かつ ハイライト有効 かつ 文法が未コンパイル(cold)なら
        // 初回だけローディング表示/段階表示が要る。温まっていれば最初から即時着色。
        self.hl_pending = self.cfg.ui.syntax_highlight
            && matches!(self.preview_kind, Some(PreviewKind::Code(_)))
            && !crate::preview::code::is_ext_warm(self.current_preview_ext());
        self.hl_warming = false;
        // Code/Text は常に windowed。Markdown/Mermaid は raw 表示(`R`)のときだけ windowed 化＝
        // 素のソースを行/桁一致で読み、2D キャレット選択をそのまま乗せる。
        let windowed_kind = matches!(
            self.preview_kind,
            Some(PreviewKind::Code(_)) | Some(PreviewKind::Text(_))
        ) || self.is_raw_source();
        if !windowed_kind {
            return;
        }
        let Some(path) = self.preview_path.clone() else {
            return;
        };
        if let Ok(w) = crate::preview::window::FileWindow::open(&path) {
            self.preview_win = Some(w);
        }
    }

    /// Extension of the current preview target file (empty string if none). Used for the highlight-warm check, etc.
    pub fn current_preview_ext(&self) -> &str {
        self.preview_path
            .as_deref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .unwrap_or("")
    }

    /// Whether waiting on heavy code highlighting (whether the render shows a loading/progressive display).
    pub fn is_highlight_pending(&self) -> bool {
        self.hl_pending
    }

    /// Whether the loading display is the "central spinner" style (false=progressive=immediate plain text).
    pub fn loading_is_indicator(&self) -> bool {
        self.cfg.ui.preview_loading != "progressive"
    }

    /// Clear the loading state on warm completion (swap in the colored version). Common to indicator/progressive.
    pub fn clear_highlight_pending(&mut self) {
        self.hl_pending = false;
        self.hl_warming = false;
    }

    /// If the background warm has not started yet, return the launch target (the caller runs `code::warm_file` on a separate thread).
    /// Both indicator and progressive **compile on a background thread** so the UI is not stalled (the spinner keeps spinning).
    /// Launch only when the return value is Some((ext, path)) (= prevents double-launch).
    pub fn take_warm_job(&mut self) -> Option<(String, PathBuf)> {
        if self.hl_pending && !self.hl_warming {
            if let Some(path) = self.preview_path.clone() {
                self.hl_warming = true;
                return Some((self.current_preview_ext().to_string(), path));
            }
        }
        None
    }

    /// Background jobs currently in flight, as i18n labels (drives the top-right busy indicator).
    /// **Derived** from the existing per-job state (no separate begin/end pairing that could leak
    /// a stuck spinner): git-ignored scan / media decode / syntax-highlight warm-up / inline images.
    pub fn busy_jobs(&self) -> Vec<crate::i18n::Msg> {
        let mut v = Vec::new();
        if self.git_ignored_pending.is_some() {
            v.push(crate::i18n::Msg::BusyGitScan);
        }
        if self.media_loading {
            v.push(crate::i18n::Msg::BusyMedia);
        }
        if self.hl_pending || self.hl_warming {
            v.push(crate::i18n::Msg::BusyHighlight);
        }
        if self.md_images_loading() {
            v.push(crate::i18n::Msg::BusyImages);
        }
        v
    }

    /// Whether the top-right busy indicator should be shown/animated right now
    /// (config on + at least one background job in flight). The run loop only schedules
    /// animation ticks while this is true, so idle CPU stays at zero.
    pub fn busy_indicator_active(&self) -> bool {
        self.cfg.ui.busy_indicator && !self.busy_jobs().is_empty()
    }

    /// Advance the spinner by one frame (the run loop calls this periodically while waiting = keeps it spinning).
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
    }

    /// The current spinner glyph (no emoji; the classic braille-pattern spinner = single-width and color-free in the terminal).
    pub fn spinner_glyph(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[self.spinner_frame % FRAMES.len()]
    }

    /// `e`: decide the target to open in the external editor and raise the request. Tree=the selected file / Preview=the file being shown.
    /// For a directory or no target, notify via flash and do nothing (only files can be edited).
    pub fn request_edit(&mut self) {
        let target = match self.mode {
            Mode::Tree => match self.entries.get(self.selected) {
                Some(e) if e.is_dir => {
                    self.flash = Some(tr(self.lang, crate::i18n::Msg::CannotEditDirectory).into());
                    return;
                }
                Some(e) => Some(e.path.clone()),
                None => None,
            },
            Mode::Preview => self.preview_path.clone(),
        };
        match target {
            Some(p) => self.pending_edit = Some(p),
            None => self.flash = Some(tr(self.lang, crate::i18n::Msg::NoFileToEdit).into()),
        }
    }

    /// Taken by the run loop: if set, return the editor-launch target path and clear it.
    pub fn take_pending_edit(&mut self) -> Option<PathBuf> {
        self.pending_edit.take()
    }

    /// `O`: request launching an external git tool (lazygit, etc.) (the run loop picks it up, suspends, then launches).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn launch_git_tool(&mut self) {
        self.pending_git_tool = true;
    }

    /// Taken by the run loop: if a git-tool launch request is set, return true and clear it.
    pub fn take_launch_git_tool(&mut self) -> bool {
        std::mem::take(&mut self.pending_git_tool)
    }

    /// Reopen the preview after the external editor exits (reflect the edited content). Caches are also discarded.
    /// Even if the editor replaces the file (temp→rename), the FileWindow is reopened so the latest is read.
    pub fn reload_preview(&mut self) {
        self.md_cache = None;
        self.win_cache = None;
        if matches!(self.mode, Mode::Preview) {
            self.setup_windowed();
            self.reload_media_if_changed();
            // CSV/TSV は外部編集で内容が変わり得る。再パースしてカーソルを範囲内へクランプ(位置は保つ)。
            if matches!(self.preview_kind, Some(PreviewKind::Table { .. })) {
                self.load_table();
                self.clamp_table_cursor();
            }
        }
    }

    /// On an FS-driven reload, re-load media (image/svg/gif/video/pdf) **only when the previewed file
    /// actually changed** (mtime guard). Without this, an image preview stays stale after an external
    /// edit: text/markdown follows via the md_cache clear, but images go through a separate path and were
    /// missed — so when a file is edited externally (e.g. an editor on the right), the konoma preview on the left did not refresh.
    /// The mtime guard avoids re-decoding / re-running external tools (pdftocairo/ffmpeg) on unrelated FS
    /// events. Zoom / pan / page are preserved across the reload.
    fn reload_media_if_changed(&mut self) {
        let is_media = matches!(
            self.preview_kind,
            Some(
                PreviewKind::Image(_)
                    | PreviewKind::Svg(_)
                    | PreviewKind::Video(_)
                    | PreviewKind::Pdf(_)
            )
        );
        if !is_media {
            return;
        }
        let Some(path) = self.preview_path.clone() else {
            return;
        };
        let Some(kind) = self.preview_kind.clone() else {
            return;
        };
        if file_mtime(&path) == self.preview_media_mtime {
            return; // 対象ファイルは未変更 → 無駄な再デコード/外部ツール実行を避ける
        }
        // 表示状態(ズーム/中心/ページ)を保持して再ロード。
        let (zoom, center, page) = (self.image_zoom, self.image_center, self.pdf_page);
        self.clear_image(); // ズーム/中心/ページ/メディア世代をリセット
        if matches!(kind, PreviewKind::Pdf(_)) {
            self.pdf_pages = crate::preview::pdf::page_count(&path);
            self.pdf_page = page.clamp(1, self.pdf_pages.unwrap_or(1).max(1));
        }
        self.start_media_load(&kind, &path); // preview_media_mtime も更新される
        self.image_zoom = zoom;
        self.image_center = center;
    }

    /// Whether in windowed (large file) mode. Used for the render and scroll branching.
    pub fn is_windowed(&self) -> bool {
        self.preview_win.is_some()
    }

    /// Whether the current preview is a Markdown/Mermaid file shown as its raw source (`R` toggled on).
    /// Such a preview is windowed like a code file, so the 2D caret selection/copy applies.
    pub fn is_raw_source(&self) -> bool {
        self.md_raw
            && matches!(
                self.preview_kind,
                Some(PreviewKind::Markdown(_)) | Some(PreviewKind::Mermaid(_))
            )
    }

    /// Whether the current preview is Markdown/Mermaid (has a decorated render, so `R` can toggle raw source).
    pub fn is_decorated_kind(&self) -> bool {
        matches!(
            self.preview_kind,
            Some(PreviewKind::Markdown(_)) | Some(PreviewKind::Mermaid(_))
        )
    }

    /// Whether raw source view is currently on (for the footer/title indicator).
    pub fn is_md_raw(&self) -> bool {
        self.md_raw
    }

    /// `R`: toggle a Markdown/Mermaid preview between its decorated render and raw source. No-op for other kinds.
    /// Rebuilds the windowed reader and resets the caret/scroll/selection so the new view starts at the top.
    pub fn toggle_md_raw(&mut self) {
        if !self.is_decorated_kind() {
            return;
        }
        self.md_raw = !self.md_raw;
        // 表示切替＝先頭から。窓読み(raw)/装飾(rendered)を張り直す。
        self.preview_byte_top = 0;
        self.preview_top_line = 0;
        self.preview_scroll = 0;
        self.preview_hscroll = 0;
        self.preview_cursor_line = 0;
        self.preview_cursor_col = 0;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        self.md_cache = None;
        self.setup_windowed();
    }

    // ---- windowed プレビューの 2D キャレット / ビジュアル選択コピー -----------

    /// Whether a visual selection is active in a windowed preview (routes the PreviewTextVisual surface).
    pub fn is_preview_visual(&self) -> bool {
        self.is_windowed() && self.preview_visual_anchor.is_some()
    }

    /// Whether the active preview selection is linewise (`V`) rather than charwise (`v`). For the status chip/footer.
    pub fn preview_visual_linewise(&self) -> bool {
        self.preview_visual_linewise
    }

    /// The active selection for the render/copy: charwise (`v`) or linewise (`V`), normalized so start ≤ end.
    pub fn preview_selection(&self) -> PreviewSelection {
        let Some((al, ac)) = self.preview_visual_anchor else {
            return PreviewSelection::None;
        };
        let (cl, cc) = (self.preview_cursor_line, self.preview_cursor_col);
        if self.preview_visual_linewise {
            PreviewSelection::Line {
                lo: al.min(cl),
                hi: al.max(cl),
            }
        } else {
            // charwise: order by (line, col)
            let (start, end) = if (al, ac) <= (cl, cc) {
                ((al, ac), (cl, cc))
            } else {
                ((cl, cc), (al, ac))
            };
            PreviewSelection::Char { start, end }
        }
    }

    /// `v` (charwise) / `V` (linewise): start a visual selection at the current 2D caret (windowed previews only).
    pub fn preview_enter_visual(&mut self, linewise: bool) {
        if self.is_windowed() {
            self.preview_visual_anchor = Some((self.preview_cursor_line, self.preview_cursor_col));
            self.preview_visual_linewise = linewise;
        }
    }

    /// Exit visual selection without copying (`v`/`V` again / Esc / q).
    pub fn preview_exit_visual(&mut self) {
        self.preview_visual_anchor = None;
    }

    /// The selected text read from the real file: whole logical lines (linewise) or an exact character range
    /// (charwise, end-inclusive). Uses the file's unwrapped/logical lines so a paste keeps the original layout.
    /// Empty when there is no path or the range is out of bounds.
    fn preview_selection_text(&self) -> String {
        let Some(path) = self.preview_path.as_ref() else {
            return String::new();
        };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return String::new(),
        };
        let s = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = s.lines().collect();
        if lines.is_empty() {
            return String::new();
        }
        let last = lines.len() - 1;
        match self.preview_selection() {
            PreviewSelection::Line { lo, hi } => {
                let end = hi.min(last);
                if lo > end {
                    String::new()
                } else {
                    lines[lo..=end].join("\n")
                }
            }
            PreviewSelection::None => {
                // not selecting → the current logical line
                let l = self.preview_cursor_line.min(last);
                lines[l].to_string()
            }
            PreviewSelection::Char { start, end } => selection_char_text(&lines, start, end),
        }
    }

    /// `y`: copy the current selection (or the current line if not selecting) to the clipboard, then exit visual.
    pub fn preview_copy_selection(&mut self) {
        let text = self.preview_selection_text();
        self.preview_visual_anchor = None;
        if text.is_empty() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        }
        self.set_clipboard_flash(&text);
    }

    /// The `@path#L12-34` reference text (Claude Code's file+line context syntax) for the current
    /// selection — or the caret line when not selecting. Line numbers are 1-based and inclusive; a single
    /// line is `#L12`. In non-windowed previews (no caret) it degrades to `@path`. None without a target.
    fn preview_selection_ref_text(&self) -> Option<String> {
        let path = self.preview_path.as_ref()?;
        let base = at_ref_text(&self.open_dir, path);
        if !self.is_windowed() {
            return Some(base);
        }
        let (lo, hi) = match self.preview_selection() {
            PreviewSelection::Line { lo, hi } => (lo, hi),
            PreviewSelection::Char { start, end } => (start.0, end.0),
            PreviewSelection::None => (self.preview_cursor_line, self.preview_cursor_line),
        };
        Some(if lo == hi {
            format!("{base}#L{}", lo + 1)
        } else {
            format!("{base}#L{}-{}", lo + 1, hi + 1)
        })
    }

    /// `Y`: copy the `@path#L…` reference of the selection/caret to the clipboard, then exit visual
    /// (paste it to Claude Code to point the conversation at this exact spot).
    pub fn preview_copy_selection_ref(&mut self) {
        let Some(text) = self.preview_selection_ref_text() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        self.preview_visual_anchor = None;
        self.set_clipboard_flash(&text);
    }

    /// `h`/`l`: move the column caret by `dir` chars (windowed previews). The render clamps it to the line length
    /// and writes the clamp back, so overshoot is corrected on the next frame. Falls back to hscroll otherwise.
    pub fn preview_col_move(&mut self, dir: i32) {
        if self.is_windowed() {
            let next = (self.preview_cursor_col as i64 + dir as i64).max(0) as usize;
            self.preview_cursor_col = next;
        } else {
            self.preview_hscroll(dir * 2);
        }
    }

    /// `0`: move the caret to the first column (windowed), else scroll home.
    pub fn preview_col_home(&mut self) {
        if self.is_windowed() {
            self.preview_cursor_col = 0;
        } else {
            self.preview_hscroll_home();
        }
    }

    /// `$`: move the caret to the last column (windowed). The render clamps the sentinel to the line length.
    pub fn preview_col_end(&mut self) {
        if self.is_windowed() {
            self.preview_cursor_col = usize::MAX;
        } else {
            self.preview_hscroll_end();
        }
    }

    pub fn back_to_tree(&mut self) {
        self.mode = Mode::Tree;
        self.preview_path = None;
        self.preview_kind = None;
        self.clear_image(); // graphics 状態を解放
        self.md_cache = None;
        self.md_raw = false;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.hl_pending = false;
        self.hl_warming = false;
        self.preview_byte_top = 0;
        self.preview_top_line = 0;
        self.md_items.clear();
        self.focused_item = None;
        self.preview_search = None;
        self.search_input = None;
        self.search_matches.clear();
        self.search_idx = 0;
        self.came_from_git_view = false;
        self.table_data = None;
        self.table_cur_row = 0;
        self.table_cur_col = 0;
        self.table_top_row = 0;
        self.table_left_col = 0;
        self.preview_cursor_line = 0;
        self.preview_cursor_col = 0;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
    }

    /// Release and reset the image display state (protocol, source image, zoom/pan).
    fn clear_image(&mut self) {
        self.image = None;
        self.image_src = None;
        self.image_zoom = 1.0;
        self.image_center = (0.5, 0.5);
        self.image_crop = None;
        self.image_vis_frac = (1.0, 1.0);
        self.gif_frames = Vec::new();
        self.gif_idx = 0;
        self.gif_shown_at = None;
        self.gif_protocol = None;
        self.gif_proto_key = None;
        self.pdf_page = 1;
        self.pdf_pages = None;
        // 世代を進めて、別ファイルの読み込み中に届く前ファイルのメディア結果を陳腐化させる。
        self.media_gen = self.media_gen.wrapping_add(1);
        self.media_loading = false;
    }

    /// Return the decorated lines for a Markdown/Mermaid/code preview (inside the frame of display width `width`).
    /// If (path, width) matches, the cache is reused and regenerated only when it changes (avoids re-highlighting
    /// /re-parsing on every scroll; rebuilt only on a width change = resize). The return value is cloned every frame for rendering.
    /// For targets other than these, returns empty (the caller uses the text path).
    pub fn decorated_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let Some(path) = self.preview_path.clone() else {
            return Vec::new();
        };
        let hit = matches!(&self.md_cache, Some(c) if c.path == path && c.width == width);
        if !hit {
            let (lines, images, remote) = self.build_decorated(&path, width);
            // Kick off background downloads for any remote images shown as "loading". Each completes
            // by invalidating md_cache (apply_remote_fetch) so this rebuilds with the cached file.
            for url in &remote {
                self.ensure_remote_md_fetch(url);
            }
            self.md_cache = Some(MdCache {
                path: path.clone(),
                width,
                lines,
                images,
            });
        }
        self.md_cache
            .as_ref()
            .map(|c| c.lines.clone())
            .unwrap_or_default()
    }

    /// Block-level inline images reserved in the current decorated Markdown (empty for other kinds
    /// or when there is no image backend). `decorated_lines` must be called first (it fills the cache).
    pub fn md_images(&self) -> Vec<crate::preview::markdown::ImagePlacement> {
        self.md_cache
            .as_ref()
            .map(|c| c.images.clone())
            .unwrap_or_default()
    }

    /// Read the file with a cap and generate decorated lines (plus inline-image placements and the list
    /// of remote image URLs to fetch) according to its kind. A read failure becomes a single safe line.
    /// Only Markdown yields image placements / remote URLs.
    fn build_decorated(
        &self,
        path: &Path,
        width: u16,
    ) -> (
        Vec<Line<'static>>,
        Vec<crate::preview::markdown::ImagePlacement>,
        Vec<String>,
    ) {
        let src = match crate::preview::text::load(path) {
            Ok(content) => {
                let mut s = content.lines.join("\n");
                if content.truncated {
                    s.push_str("\n\n— (省略: 表示上限に達しました) —");
                }
                s
            }
            Err(e) => {
                return (
                    vec![Line::from(format!("[can not preview: 読み込み失敗] {e}"))],
                    Vec::new(),
                    Vec::new(),
                )
            }
        };
        match &self.preview_kind {
            Some(PreviewKind::Markdown(_)) => {
                let theme = &self.cfg.ui.theme;
                let code = crate::preview::markdown::CodeStyle {
                    bg: theme.code_bg(),
                    label_bg: theme.code_label_bg(),
                    label_right: theme.code_label_right(),
                    tab_width: self.cfg.ui.tab_width,
                    wrap: self.cfg.ui.wrap,
                };
                // Decide how to render each image URL. A local file or a cached remote fetch resolves to
                // a path → Inline (with its display size in cells). An uncached remote URL is Loading
                // (a fetch is kicked off separately, in `decorated_lines`) unless it has already failed.
                // Anything else (no backend / missing file / data: URL) degrades to text (principle #3).
                let font = self.picker.as_ref().map(|p| p.font_size());
                let base_dir = path.parent().map(|p| p.to_path_buf());
                let avail = width.saturating_sub(2);
                let slot_of = |url: &str| -> crate::preview::markdown::ImageSlot {
                    use crate::preview::markdown::ImageSlot;
                    let Some(font) = font else {
                        return ImageSlot::Unavailable;
                    };
                    if let Some(p) = resolve_md_image_path(url, base_dir.as_deref()) {
                        match md_image_dims(&p) {
                            Some((pw, ph)) => {
                                let (cols, rows) = md_image_cells(
                                    pw,
                                    ph,
                                    font.width,
                                    font.height,
                                    avail,
                                    MD_IMAGE_MAX_ROWS,
                                );
                                ImageSlot::Inline { cols, rows }
                            }
                            None => ImageSlot::Unavailable,
                        }
                    } else if crate::preview::markdown::is_remote_image_url(url)
                        && !self.md_remote_failed.contains(url)
                    {
                        ImageSlot::Loading
                    } else {
                        ImageSlot::Unavailable
                    }
                };
                let (lines, images) = crate::preview::markdown::render_markdown_with_images(
                    &src,
                    width,
                    code,
                    &theme.code_theme,
                    self.cfg.ui.icons,
                    &self.cfg.ui.md_task_state_chars(),
                    &slot_of,
                );
                let remote = if font.is_some() {
                    crate::preview::markdown::collect_remote_image_urls(&src)
                } else {
                    Vec::new()
                };
                (lines, images, remote)
            }
            Some(PreviewKind::Mermaid(_)) => (
                crate::preview::markdown::render_mermaid_file(&src, width),
                Vec::new(),
                Vec::new(),
            ),
            // 単体コードファイルは syntect でシンタックスハイライト。
            Some(PreviewKind::Code(_)) => (
                crate::preview::code::highlight(&src, path, &self.cfg.ui.theme.code_theme),
                Vec::new(),
                Vec::new(),
            ),
            _ => (Vec::new(), Vec::new(), Vec::new()),
        }
    }

    /// Attach the image backend (terminal Picker and the offload tx) at startup.
    pub fn attach_image_backend(&mut self, picker: Picker, tx: UnboundedSender<ResizeRequest>) {
        self.picker = Some(picker);
        self.img_tx = Some(tx);
    }

    /// Attach the sending end that offloads heavy media loading (SVG/GIF) to a separate thread.
    pub fn attach_media_loader(&mut self, tx: std::sync::mpsc::Sender<MediaResult>) {
        self.media_tx = Some(tx);
    }

    /// Attach the sender that offloads inline Markdown image decoding to a background thread.
    pub fn attach_md_image_loader(&mut self, tx: std::sync::mpsc::Sender<MdImageResult>) {
        self.md_img_tx = Some(tx);
    }

    /// Attach the sender that offloads inline-image encoding (resize + protocol) to the encode worker.
    pub fn attach_md_encoder(&mut self, tx: std::sync::mpsc::Sender<MdEncodeRequest>) {
        self.md_enc_tx = Some(tx);
    }

    /// Apply a completed background decode of an inline Markdown image. Returns whether to redraw.
    pub fn apply_md_image(&mut self, res: MdImageResult) -> bool {
        let entry = self.md_image_cache.entry(res.path).or_default();
        match res.image {
            Ok(img) => {
                entry.decoded = Some(Arc::new(img));
                entry.failed = false;
            }
            Err(_) => entry.failed = true,
        }
        true
    }

    /// Apply a completed background encode: store the protocol in its full or clip slot. Returns redraw.
    pub fn apply_md_encode(&mut self, res: MdEncodeResult) -> bool {
        let Some(entry) = self.md_image_cache.get_mut(&res.path) else {
            return false;
        };
        entry.enc_inflight = false;
        match res.key {
            MdEncodeKey::Full { cols, rows } => {
                entry.protocol = Some(res.protocol);
                entry.proto_size = Some((cols, rows));
            }
            MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            } => {
                entry.clip_protocol = Some(res.protocol);
                entry.clip_key = Some((cols, full_rows, row_off, vis_rows));
            }
        }
        true
    }

    /// Attach the sender that reports background remote-image download completions to the run loop.
    pub fn attach_remote_md_loader(&mut self, tx: std::sync::mpsc::Sender<RemoteFetch>) {
        self.md_remote_tx = Some(tx);
    }

    /// Apply a completed remote-image download. On success the file is now cached, so drop the
    /// decoration cache to re-lay the image out inline; on failure remember the URL so it is not
    /// retried and shows a text placeholder instead. Returns whether to redraw.
    pub fn apply_remote_fetch(&mut self, res: RemoteFetch) -> bool {
        self.md_remote_inflight.remove(&res.url);
        if !res.ok {
            self.md_remote_failed.insert(res.url);
        }
        // Re-decorate so a now-cached image is laid out (or a failed one degrades to text).
        self.md_cache = None;
        true
    }

    /// Ensure a background download is in flight for the remote image `url` (deduplicated). Skips URLs
    /// that are already cached, already downloading, or known to have failed. The download runs off the
    /// UI thread (principle #4) via `curl`; on completion it reports through `md_remote_tx`.
    fn ensure_remote_md_fetch(&mut self, url: &str) {
        if !crate::preview::markdown::is_remote_image_url(url) {
            return;
        }
        // Already downloaded (cache file exists), already failed, or already downloading → nothing to do.
        if resolve_md_image_path(url, None).is_some()
            || self.md_remote_failed.contains(url)
            || self.md_remote_inflight.contains(url)
        {
            return;
        }
        let (Some(tx), Some(dest)) = (self.md_remote_tx.clone(), md_remote_cache_path(url)) else {
            return;
        };
        self.md_remote_inflight.insert(url.to_string());
        let u = url.to_string();
        std::thread::spawn(move || {
            let ok = fetch_remote_image(&u, &dest);
            let _ = tx.send(RemoteFetch { url: u, ok });
        });
    }

    /// Ensure the inline image for `url` is decoding in the background and that the protocol for the
    /// currently-visible portion (whole image, or a cropped band when partially scrolled) is encoding on
    /// the worker thread. Called from the renderer for each visible inline image. Both decoding and
    /// encoding are off-thread (principle #4) so this never blocks the UI; the protocol appears a frame
    /// or two later. At most one encode is in flight per image, so scrolling never queues a backlog.
    pub fn ensure_md_image(
        &mut self,
        url: &str,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    ) {
        let base = self
            .preview_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        let Some(path) = resolve_md_image_path(url, base.as_deref()) else {
            return;
        };
        // Kick off a one-time background decode.
        if !self.md_image_cache.contains_key(&path) {
            self.md_image_cache
                .insert(path.clone(), MdImgEntry::default());
            if let Some(tx) = self.md_img_tx.clone() {
                let p = path.clone();
                let svg_max_px = self.cfg.ui.svg_max_px;
                std::thread::spawn(move || {
                    // Sniff the format from content (remote-cache files have no extension); rasterize SVG.
                    let image =
                        md_decode_image(&p, svg_max_px).ok_or_else(|| "decode failed".to_string());
                    let _ = tx.send(MdImageResult { path: p, image });
                });
            }
            return;
        }
        let Some(enc_tx) = self.md_enc_tx.clone() else {
            return;
        };
        let Some(entry) = self.md_image_cache.get_mut(&path) else {
            return;
        };
        // Wait if it failed, is still decoding, or already has an encode in flight (one at a time).
        if entry.failed || entry.enc_inflight {
            return;
        }
        let Some(img) = entry.decoded.clone() else {
            return;
        };
        // Fully visible: request an encode of the whole image at (cols, full_rows) unless already cached.
        if row_off == 0 && vis_rows >= full_rows {
            if entry.proto_size == Some((cols, full_rows)) {
                return;
            }
            entry.enc_inflight = true;
            let _ = enc_tx.send(MdEncodeRequest {
                path,
                key: MdEncodeKey::Full {
                    cols,
                    rows: full_rows,
                },
                image: img,
                crop: None,
                cols,
                rows: full_rows,
            });
            return;
        }
        // Partially scrolled: request an encode of just the visible pixel band at (cols, vis_rows), so the
        // image renders clipped to the viewport rather than being hidden.
        if entry.clip_key == Some((cols, full_rows, row_off, vis_rows)) {
            return;
        }
        let (dw, dh) = (img.width(), img.height());
        let (y0, h) = md_band_pixels(full_rows, row_off, vis_rows, dh);
        entry.enc_inflight = true;
        let _ = enc_tx.send(MdEncodeRequest {
            path,
            key: MdEncodeKey::Clip {
                cols,
                full_rows,
                row_off,
                vis_rows,
            },
            image: img,
            crop: Some((0, y0, dw, h)),
            cols,
            rows: vis_rows,
        });
    }

    /// The render protocol to draw for the visible portion of inline image `url`. Prefers the protocol
    /// that exactly matches the current position (full image when fully visible, or the band matching
    /// `(cols, full_rows, row_off, vis_rows)`); while a newly-scrolled band is still encoding it returns
    /// the last encoded protocol so the image stays on screen (and snaps to the exact band on arrival)
    /// rather than blinking out. None only until the very first protocol for this image is ready.
    pub fn md_image_proto(
        &self,
        url: &str,
        cols: u16,
        full_rows: u16,
        row_off: u16,
        vis_rows: u16,
    ) -> Option<&Protocol> {
        let base = self
            .preview_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        let path = resolve_md_image_path(url, base.as_deref())?;
        let entry = self.md_image_cache.get(&path)?;
        // Exact match for the current position.
        if row_off == 0 && vis_rows >= full_rows {
            if entry.proto_size == Some((cols, full_rows)) {
                return entry.protocol.as_ref();
            }
        } else if entry.clip_key == Some((cols, full_rows, row_off, vis_rows)) {
            return entry.clip_protocol.as_ref();
        }
        // Not yet encoded for this exact position: keep the last band (or the full image) visible.
        entry.clip_protocol.as_ref().or(entry.protocol.as_ref())
    }

    /// Whether any inline Markdown image is still loading — a remote download in flight, a decode not yet
    /// finished, or an encode in flight (used by the run loop to keep ticking so results are applied
    /// promptly and the image appears without waiting for the next key press).
    pub fn md_images_loading(&self) -> bool {
        !self.md_remote_inflight.is_empty()
            || self
                .md_image_cache
                .values()
                .any(|e| (e.decoded.is_none() && !e.failed) || e.enc_inflight)
    }

    /// Drop tx and the image state on exit (to terminate the worker thread).
    pub fn detach_image_backend(&mut self) {
        self.clear_image();
        self.img_tx = None;
    }

    /// Decode a still image and place it in image_src (the ThreadProtocol is built asynchronously at render time).
    /// Still images (PNG/JPG) decode fast, so this stays synchronous. On decode failure / no backend, image_src=None.
    fn load_image(&mut self, path: &Path) {
        if self.picker.is_none() || self.img_tx.is_none() {
            return; // バックエンド無し: 描画側がテキストにフォールバック
        }
        let Some(dyn_img) = crate::preview::image::decode_static(path) else {
            return;
        };
        self.set_static_image(dyn_img);
    }

    /// Common processing to set a still image (including SVG raster results / single-frame GIFs) as the display source.
    /// zoom/center are left untouched (enter_preview has the preceding clear_image set defaults, and tab restore has already
    /// overwritten them with the restored values; so that a late asynchronous result does not break the restored zoom/center).
    fn set_static_image(&mut self, img: image::DynamicImage) {
        self.image_src = Some(img);
        self.image_crop = None; // 次の描画(prepare_image)でプロトコルを構築させる
    }

    /// Common processing to set all GIF frames into the display state (used from both the synchronous and asynchronous paths).
    /// zoom/center are left untouched for the same reason as set_static_image.
    fn set_gif_frames(&mut self, frames: Vec<(image::DynamicImage, std::time::Duration)>) {
        self.gif_frames = frames;
        self.gif_idx = 0;
        self.gif_shown_at = None; // 最初の tick で計時開始
        self.gif_protocol = None;
        self.gif_proto_key = None;
        self.image_crop = None;
    }

    /// Start loading image-type media according to the preview kind (from enter_preview / tab restore).
    /// Still images are synchronous, while **heavy SVG rasterization / GIF full-frame decode are offloaded to a separate thread**
    /// (to start display fast without blocking the UI thread). With no media_tx (tests, etc.), a synchronous fallback is used.
    fn start_media_load(&mut self, kind: &PreviewKind, path: &Path) {
        // メディア自動再読込の基準時刻を記録(reload_media_if_changed が mtime 比較に使う)。
        self.preview_media_mtime = file_mtime(path);
        match kind {
            PreviewKind::Image(_) if Self::looks_like_gif(path) => {
                self.spawn_or_sync_media(MediaJob::Gif(path.to_path_buf()))
            }
            // 静止画(PNG/JPG 等)はデコードが速いので同期のまま(エンコードは描画時に worker が非同期化)。
            PreviewKind::Image(_) => self.load_image(path),
            PreviewKind::Svg(_) => {
                self.spawn_or_sync_media(MediaJob::Svg(path.to_path_buf(), self.cfg.ui.svg_max_px))
            }
            PreviewKind::Video(_) => self.spawn_or_sync_media(MediaJob::Video(path.to_path_buf())),
            PreviewKind::Pdf(_) => {
                self.spawn_or_sync_media(MediaJob::Pdf(path.to_path_buf(), self.pdf_page))
            }
            _ => {}
        }
    }

    /// Run a media-load job on a separate thread (when media_tx is present). Otherwise run it synchronously.
    /// If there is no backend (picker), do nothing (the render side falls back).
    fn spawn_or_sync_media(&mut self, job: MediaJob) {
        if self.picker.is_none() {
            return; // 端末非対応: 描画側がテキスト/メッセージへフォールバック
        }
        let Some(tx) = self.media_tx.clone() else {
            // 同期フォールバック(テスト/チャネル未装着)。
            if let Some(payload) = job.run() {
                self.apply_payload(payload);
            }
            return;
        };
        self.media_gen = self.media_gen.wrapping_add(1);
        self.media_loading = true;
        let gen = self.media_gen;
        std::thread::spawn(move || {
            let _ = tx.send(MediaResult {
                gen,
                payload: job.run(),
            });
        });
    }

    /// Apply a media-load result from another thread. Stale results (from after moving to another file) are discarded.
    /// Returns true if the state changes from applying / staleness judgment (the caller re-renders).
    pub fn apply_media(&mut self, result: MediaResult) -> bool {
        if result.gen != self.media_gen {
            return false; // 陳腐化: 既に別ファイルへ移っている
        }
        self.media_loading = false;
        match result.payload {
            Some(payload) => {
                self.apply_payload(payload);
                true
            }
            None => true, // 失敗: loading を解除し描画側のフォールバック(生XML/メッセージ)へ
        }
    }

    fn apply_payload(&mut self, payload: MediaPayload) {
        match payload {
            MediaPayload::Static(img) => self.set_static_image(img),
            MediaPayload::Gif(frames) => self.set_gif_frames(frames),
        }
    }

    /// Whether waiting on another thread's media load (used by the render side to decide on the "Loading…" display).
    pub fn is_media_loading(&self) -> bool {
        self.media_loading
    }

    /// Decide whether it is a GIF by the leading bytes (looks at the magic, not the extension).
    fn looks_like_gif(path: &Path) -> bool {
        use std::io::Read;
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut head = [0u8; 6];
        if f.read_exact(&mut head).is_err() {
            return false;
        }
        &head[..4] == b"GIF8" // GIF87a / GIF89a
    }

    /// Drive the GIF animation. If the current frame's display time has elapsed, advance to the next frame index.
    /// The actual re-encode is done by prepare_gif on the next render (detecting the gif_idx change).
    /// Returns true if advanced (the caller re-renders). Always false if not a GIF.
    pub fn advance_gif_if_due(&mut self) -> bool {
        if self.gif_frames.len() < 2 {
            return false;
        }
        let now = std::time::Instant::now();
        let Some(shown_at) = self.gif_shown_at else {
            // 最初の tick: 計時を開始するだけ(先頭フレームは表示済み)。
            self.gif_shown_at = Some(now);
            return false;
        };
        let delay = self.gif_frames[self.gif_idx].1;
        if now.duration_since(shown_at) < delay {
            return false;
        }
        self.gif_idx = (self.gif_idx + 1) % self.gif_frames.len();
        self.gif_shown_at = Some(now);
        true
    }

    /// Whether a GIF animation is active (used for branching in the render path and footer/zoom checks).
    pub fn is_gif_active(&self) -> bool {
        self.gif_frames.len() >= 2
    }

    /// The render protocol of the current GIF frame synchronously encoded to the display size (referenced by the render side).
    pub fn gif_protocol(&self) -> Option<&Protocol> {
        self.gif_protocol.as_ref()
    }

    /// Call just before rendering (for GIF). Compute the display rectangle target and crop from the current (gif_idx, zoom, center, inner), and
    /// if the frame/crop has changed, **synchronously encode** the current frame and swap it into gif_protocol.
    /// Being synchronous, "render an unencoded protocol → empty" does not happen, and the frame switches atomically (no churn).
    /// The return value is the render-target target. None when there is no backend / size 0.
    pub fn prepare_gif(&mut self, inner: Rect) -> Option<Rect> {
        let picker = self.picker.as_ref()?;
        let scale = self.cfg.ui.image_render_scale;
        let src = &self.gif_frames.get(self.gif_idx)?.0;
        let (target, crop_rect, center, frac) = image_layout(
            src,
            picker.font_size(),
            self.image_zoom,
            self.image_center,
            inner,
            scale,
        )?;
        let key = (self.gif_idx, crop_rect);
        // フレーム or crop(ズーム/パン/リサイズ)が変わったときだけ再エンコード。
        let built = if self.gif_proto_key != Some(key) {
            let (x0, y0, cw, ch) = crop_rect;
            let crop = src.crop_imm(x0, y0, cw, ch);
            let size = ratatui::layout::Size::new(target.width.max(1), target.height.max(1));
            // src/picker の借用はこの式で完結(結果は所有値)→以降 self を変更可。
            // GIF は UI スレッドで毎フレーム同期エンコードするため、軽量で滑らかな Triangle(bilinear) を使う
            // (最近傍 None だとアニメ各フレームがジャギる。最高品質の Lanczos3 は静止画側で使用)。
            Some(picker.new_protocol(crop, size, Resize::Scale(Some(FilterType::Triangle))))
        } else {
            None
        };
        if let Some(res) = built {
            match res {
                Ok(p) => {
                    self.gif_protocol = Some(p);
                    self.gif_proto_key = Some(key);
                }
                Err(_) => {
                    // エンコード失敗: 描画側はメッセージにフォールバック。次フレームで再試行。
                    self.gif_protocol = None;
                    self.gif_proto_key = None;
                }
            }
        }
        self.image_center = center;
        self.image_vis_frac = frac;
        self.image_crop = Some(crop_rect);
        Some(target)
    }

    /// Wait time until the next frame while a GIF is playing (for the poll timeout). None when not playing.
    /// Clamped to [10ms, 100ms] for smoothness (100ms is the same as the normal idle-tick upper bound).
    pub fn gif_poll_timeout(&self) -> Option<std::time::Duration> {
        use std::time::Duration;
        if self.gif_frames.len() < 2 {
            return None;
        }
        let remaining = match self.gif_shown_at {
            None => Duration::ZERO, // まだ計時前: すぐ次の tick を回して計時開始
            Some(t) => self.gif_frames[self.gif_idx]
                .1
                .checked_sub(t.elapsed())
                .unwrap_or(Duration::ZERO),
        };
        Some(remaining.clamp(Duration::from_millis(10), Duration::from_millis(100)))
    }

    // ---- CSV/TSV テーブルプレビュー ------------------------------------------

    /// Parse the current Table-kind preview into `table_data` (None on failure = raw-text fallback).
    /// Does not touch the cursor/scroll (callers reset or restore those as appropriate).
    fn load_table(&mut self) {
        self.table_data = None;
        if let Some(PreviewKind::Table { path, delimiter }) = self.preview_kind.clone() {
            if let Ok(t) = crate::preview::table::parse(&path, delimiter) {
                self.table_data = Some(t);
            }
        }
    }

    /// Clamp the cell cursor into the table's bounds (after a reload/restore that may have shrunk it).
    fn clamp_table_cursor(&mut self) {
        match &self.table_data {
            Some(t) if t.nrows() > 0 && t.ncols > 0 => {
                self.table_cur_row = self.table_cur_row.min(t.nrows() - 1);
                self.table_cur_col = self.table_cur_col.min(t.ncols - 1);
            }
            _ => {
                self.table_cur_row = 0;
                self.table_cur_col = 0;
            }
        }
    }

    /// Whether a CSV/TSV table preview is active **and parsed** (routes the PreviewTable surface / renderer).
    /// A Table kind whose parse failed returns false → the preview degrades to raw text.
    pub fn is_table_preview(&self) -> bool {
        matches!(self.preview_kind, Some(PreviewKind::Table { .. })) && self.table_data.is_some()
    }

    /// The field-separator byte of the active table (`,` by default).
    fn table_delimiter(&self) -> u8 {
        match self.preview_kind {
            Some(PreviewKind::Table { delimiter, .. }) => delimiter,
            _ => b',',
        }
    }

    /// The parsed table (for the renderer). None when not a table preview.
    pub fn table_data(&self) -> Option<&crate::preview::table::TableData> {
        self.table_data.as_ref()
    }

    /// The cell cursor as (data-row, column), both 0-based.
    pub fn table_cursor(&self) -> (usize, usize) {
        (self.table_cur_row, self.table_cur_col)
    }

    /// The current (top data row, left column) scroll offsets.
    pub fn table_scroll(&self) -> (usize, usize) {
        (self.table_top_row, self.table_left_col)
    }

    /// Renderer feedback: store the scroll offsets it settled on (to keep the cursor visible) plus the
    /// visible data-row count (used as the PageUp/Down step). Mirrors how `preview_scroll`/`preview_viewport`
    /// are clamped/recorded at render time.
    pub fn set_table_view(&mut self, top_row: usize, left_col: usize, viewport_rows: u16) {
        self.table_top_row = top_row;
        self.table_left_col = left_col;
        self.table_viewport_rows = viewport_rows;
    }

    /// Move the cell cursor by (drow, dcol), clamped to the table. The renderer scrolls to follow.
    pub fn table_cursor_move(&mut self, drow: i32, dcol: i32) {
        let Some(t) = &self.table_data else {
            return;
        };
        let (nr, nc) = (t.nrows(), t.ncols);
        if nr == 0 || nc == 0 {
            return;
        }
        let r = (self.table_cur_row as i64 + drow as i64).clamp(0, nr as i64 - 1);
        let c = (self.table_cur_col as i64 + dcol as i64).clamp(0, nc as i64 - 1);
        self.table_cur_row = r as usize;
        self.table_cur_col = c as usize;
    }

    /// Jump to the first (`bottom=false`) or last (`bottom=true`) data row.
    pub fn table_row_to(&mut self, bottom: bool) {
        let Some(t) = &self.table_data else {
            return;
        };
        self.table_cur_row = if bottom {
            t.nrows().saturating_sub(1)
        } else {
            0
        };
    }

    /// Jump to the first (`end=false`) or last (`end=true`) column.
    pub fn table_col_to(&mut self, end: bool) {
        let Some(t) = &self.table_data else {
            return;
        };
        self.table_cur_col = if end { t.ncols.saturating_sub(1) } else { 0 };
    }

    /// Move the cursor down/up by whole pages (`dir` = +1 / -1). The page size is the last render's visible rows.
    pub fn table_page(&mut self, dir: i32) {
        let page = self.table_viewport_rows.max(1) as i32;
        self.table_cursor_move(dir * page, 0);
    }

    /// Move the cursor down/up by half a page (`dir` = +1 / -1).
    pub fn table_half_page(&mut self, dir: i32) {
        let half = (self.table_viewport_rows / 2).max(1) as i32;
        self.table_cursor_move(dir * half, 0);
    }

    /// Build the text a table copy would place on the clipboard (None when there is no table).
    /// Cell = the current cell's value; Row = the current row's cells joined by the delimiter;
    /// Column = the column's header + every cell value, one per line.
    fn table_copy_text(&self, kind: TableCopyKind) -> Option<String> {
        let t = self.table_data.as_ref()?;
        let (r, c) = (self.table_cur_row, self.table_cur_col);
        let sep = (self.table_delimiter() as char).to_string();
        Some(match kind {
            TableCopyKind::Cell => {
                if t.nrows() == 0 {
                    t.header(c).to_string()
                } else {
                    t.cell(r, c).to_string()
                }
            }
            TableCopyKind::Row => {
                if t.nrows() == 0 {
                    t.headers.join(&sep)
                } else {
                    t.rows.get(r).map(|row| row.join(&sep)).unwrap_or_default()
                }
            }
            TableCopyKind::Column => {
                let mut vals = vec![t.header(c).to_string()];
                vals.extend(
                    t.rows
                        .iter()
                        .map(|row| row.get(c).cloned().unwrap_or_default()),
                );
                vals.join("\n")
            }
        })
    }

    /// Copy the current cell / row / column to the clipboard and flash the result.
    pub fn table_copy(&mut self, kind: TableCopyKind) {
        let Some(text) = self.table_copy_text(kind) else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        self.set_clipboard_flash(&text);
    }

    /// Whether the current preview is a (renderable) image. Used to route image-only keys (zoom/pan).
    /// Still images are judged by image_src, GIFs by gif_frames (image_src is not used). SVGs are already rasterized.
    pub fn is_image_preview(&self) -> bool {
        (self.image_src.is_some() || self.is_gif_active())
            && matches!(
                self.preview_kind,
                Some(
                    PreviewKind::Image(_)
                        | PreviewKind::Svg(_)
                        | PreviewKind::Video(_)
                        | PreviewKind::Pdf(_)
                )
            )
    }

    /// Whether the current preview is a PDF (used to enable page-navigation keys / footer hints).
    pub fn is_pdf_preview(&self) -> bool {
        matches!(self.preview_kind, Some(PreviewKind::Pdf(_)))
    }

    /// (current, total) page for the PDF preview, or None when not a PDF or the page count is unknown
    /// (no poppler → single-page fallback, navigation disabled). Used for the footer/status indicator.
    pub fn pdf_page_indicator(&self) -> Option<(u32, u32)> {
        if !self.is_pdf_preview() {
            return None;
        }
        let total = self.pdf_pages?;
        Some((self.pdf_page.min(total), total))
    }

    /// Whether PDF page navigation is possible (a known multi-page PDF; requires poppler for both the
    /// count and per-page rendering, so this gates `J`/`K` from doing anything on single-page/no-poppler PDFs).
    pub fn pdf_can_navigate(&self) -> bool {
        matches!(self.pdf_pages, Some(n) if n > 1)
    }

    /// Go to the next PDF page (clamped to the last page). Re-rasterizes that page on demand (one at a time).
    pub fn pdf_next_page(&mut self) {
        self.pdf_goto(self.pdf_page.saturating_add(1));
    }

    /// Go to the previous PDF page (clamped to page 1).
    pub fn pdf_prev_page(&mut self) {
        self.pdf_goto(self.pdf_page.saturating_sub(1));
    }

    /// Jump to a 1-based page (clamped to [1, total]). No-op if not a navigable PDF or already there.
    /// Kicks off an off-thread rasterization of the new page and resets the view to fit (each page shows whole).
    fn pdf_goto(&mut self, page: u32) {
        if !self.pdf_can_navigate() {
            return;
        }
        let Some(total) = self.pdf_pages else { return };
        let page = page.clamp(1, total);
        if page == self.pdf_page {
            return;
        }
        self.pdf_page = page;
        // 新ページは全体が見えるよう fit に戻す(set_static_image は zoom/center を触らないので明示リセット)。
        self.image_zoom = 1.0;
        self.image_center = (0.5, 0.5);
        self.image_crop = None;
        if let Some(PreviewKind::Pdf(p)) = self.preview_kind.clone() {
            // 旧ページの画像は到着まで表示したまま(media_gen で陳腐化判定・スピナーが重畳)。
            self.spawn_or_sync_media(MediaJob::Pdf(p, self.pdf_page));
        }
    }

    /// Zoom (multiply the magnification by `factor`; clamped to 1.0–16.0). The actual crop is applied at render time.
    pub fn image_zoom_by(&mut self, factor: f64) {
        if self.image_src.is_none() && !self.is_gif_active() {
            return;
        }
        self.image_zoom = (self.image_zoom * factor).clamp(1.0, 16.0);
    }

    /// Reset to 1x (fit). Zoom=1 and recenter.
    pub fn image_zoom_reset(&mut self) {
        if self.image_src.is_none() && !self.is_gif_active() {
            return;
        }
        self.image_zoom = 1.0;
        self.image_center = (0.5, 0.5);
    }

    /// Pan. dx/dy are directions (-1/0/+1). Only clipped axes move the center, scaled by the visible fraction.
    /// Clamping is done at render time (prepare_image), looking at the visible fraction.
    pub fn image_pan(&mut self, dx: f64, dy: f64) {
        if self.image_src.is_none() && !self.is_gif_active() {
            return;
        }
        let (fw, fh) = self.image_vis_frac;
        // 1回で可視窓の約25%。見切れていない軸(frac>=1)は動かしても描画時にクランプされる。
        self.image_center.0 += dx * 0.25 * fw;
        self.image_center.1 += dy * 0.25 * fh;
    }

    /// Call just before rendering. From the current (zoom, center, display area inner), compute the image's display rectangle target
    /// (centered; z=1=fit, grows when zooming and clips once it exceeds the viewport) and the source-image crop of
    /// the visible portion; rebuild the protocol if the crop changed. The return value is the render-target target.
    /// Realizes "fit the render area to the image size (fit) → grow when zooming → clip + pan once exceeded".
    pub fn prepare_image(&mut self, inner: Rect) -> Option<Rect> {
        let src = self.image_src.as_ref()?;
        let picker = self.picker.as_ref()?;
        let scale = self.cfg.ui.image_render_scale;
        let (target, crop_rect, center, frac) = image_layout(
            src,
            picker.font_size(),
            self.image_zoom,
            self.image_center,
            inner,
            scale,
        )?;
        // crop が変わったときだけプロトコル再構築(毎フレームの再エンコードを避ける)。
        let new_tp = if self.image_crop != Some(crop_rect) {
            let (x0, y0, cw, ch) = crop_rect;
            let crop = src.crop_imm(x0, y0, cw, ch);
            let proto = picker.new_resize_protocol(crop);
            // src/picker/img_tx の借用はこの式で完結(tp は所有値)→以降 self を変更可。
            Some(ThreadProtocol::new(
                self.img_tx.as_ref()?.clone(),
                Some(proto),
            ))
        } else {
            None
        };
        if let Some(tp) = new_tp {
            self.image = Some(tp);
        }
        self.image_center = center;
        self.image_vis_frac = frac;
        self.image_crop = Some(crop_rect);
        Some(target)
    }

    /// Page the text preview by one page (dir: +1=next page / -1=previous page).
    /// Overlaps one row to keep context. The upper bound is clamped at render time.
    pub fn preview_page(&mut self, dir: i32) {
        let page = self.preview_viewport.saturating_sub(1).max(1) as i32;
        self.preview_scroll(dir * page);
    }

    /// Half-page the text preview (vim: Ctrl-d/Ctrl-u, less: d/u).
    pub fn preview_half_page(&mut self, dir: i32) {
        let half = (self.preview_viewport / 2).max(1) as i32;
        self.preview_scroll(dir * half);
    }

    /// To the top of the preview.
    pub fn preview_to_top(&mut self) {
        if self.is_windowed() {
            self.preview_cursor_line = 0;
            self.preview_byte_top = 0;
            self.preview_top_line = 0;
        } else {
            self.preview_scroll = 0;
        }
    }

    /// To the bottom of the preview. The actual maximum is clamped at render time, when content and screen size are known.
    /// When windowed, moves to the line-head byte of the last page (found by a backward seek, without scanning the whole file).
    /// If line numbers are ON, the bottom line number is also corrected from the total line count.
    pub fn preview_to_bottom(&mut self) {
        if !self.is_windowed() {
            self.preview_scroll = u16::MAX;
            return;
        }
        let vh = self.preview_viewport.max(1) as usize;
        // 行カーソルの末尾クランプに総行数が要るので常に求める(キャッシュされる)。
        let total = self.win_total();
        let cur = self.preview_top_line;
        if let Some(b) = self
            .preview_win
            .as_mut()
            .map(|w| w.last_page_top(vh).unwrap_or(0))
        {
            self.preview_byte_top = b;
            self.preview_top_line = total.map(|t| t.saturating_sub(vh)).unwrap_or(cur);
        }
        if let Some(t) = total {
            self.preview_cursor_line = t.saturating_sub(1);
        }
    }

    /// Total line count (computed and cached only when line numbers are ON). Scans the whole file, so call it minimally.
    fn win_total(&mut self) -> Option<usize> {
        if let Some(t) = self.preview_total_lines {
            return Some(t);
        }
        let t = self.preview_win.as_mut()?.count_lines().ok()?;
        self.preview_total_lines = Some(t);
        Some(t)
    }

    /// Apply a re-encode result from the worker to the current image state. Returns true if applied.
    pub fn apply_image_resize(&mut self, resp: Result<ResizeResponse, Errors>) -> bool {
        let Ok(resp) = resp else {
            return false; // エンコード失敗は無視(クラッシュさせない)
        };
        match self.image.as_mut() {
            Some(state) => state.update_resized_protocol(resp),
            None => false,
        }
    }

    pub fn preview_scroll(&mut self, delta: i32) {
        if self.is_windowed() {
            // windowed(Code/Text)は行カーソルを動かし、窓を追従させる(常時カーソルモデル)。
            self.preview_cursor_move(delta);
            return;
        }
        let next = self.preview_scroll as i32 + delta;
        // 下限のみここで。上限 (末尾) は内容・画面サイズが判る描画時にクランプする。
        self.preview_scroll = next.max(0) as u16;
    }

    /// Move the windowed line cursor by `delta` (clamped to the file), then scroll the window to keep it visible.
    /// In visual mode the anchor stays fixed, so moving the cursor extends the selection.
    fn preview_cursor_move(&mut self, delta: i32) {
        let total = self.win_total().unwrap_or(usize::MAX);
        let maxl = total.saturating_sub(1) as i64;
        let next = (self.preview_cursor_line as i64 + delta as i64).clamp(0, maxl.max(0)) as usize;
        self.preview_cursor_line = next;
        self.follow_cursor();
    }

    /// Scroll the window (byte-based) just enough that the line cursor is on screen.
    fn follow_cursor(&mut self) {
        let vh = self.preview_viewport.max(1) as usize;
        let top = self.preview_top_line;
        let cur = self.preview_cursor_line;
        if cur < top {
            self.win_scroll_lines(-((top - cur) as i32));
        } else if cur >= top + vh {
            self.win_scroll_lines((cur + 1 - (top + vh)) as i32);
        }
    }

    /// Windowed scroll (delta lines). Computes the surrounding line-head bytes each time to move preview_byte_top, and
    /// updates preview_top_line (the line number) by the actual number of moved lines. Downward is clamped so as not to pass the last page.
    /// On end-clamp, the line number is corrected from the total line count when line numbers are ON.
    fn win_scroll_lines(&mut self, delta: i32) {
        let vh = self.preview_viewport.max(1) as usize;
        let top = self.preview_byte_top;
        let line = self.preview_top_line;
        // 行番号 ON、または(行カーソルのため)総行数が既にキャッシュされていれば末尾クランプに使う。
        let total = if self.cfg.ui.line_numbers || self.preview_total_lines.is_some() {
            self.win_total()
        } else {
            None
        };
        let result = self.preview_win.as_mut().map(|w| {
            if delta > 0 {
                let (adv, moved) = w.advance(top, delta as usize).unwrap_or((top, 0));
                let maxt = w.last_page_top(vh).unwrap_or(top);
                if adv >= maxt {
                    // 末尾ページ到達: 行番号は総行数から(無ければ素朴に加算)。
                    let bl = total.map(|t| t.saturating_sub(vh)).unwrap_or(line + moved);
                    (maxt, bl)
                } else {
                    (adv, line + moved)
                }
            } else if delta < 0 {
                let (ret, moved) = w.retreat(top, (-delta) as usize).unwrap_or((0, 0));
                (ret, line.saturating_sub(moved))
            } else {
                (top, line)
            }
        });
        if let Some((nt, nl)) = result {
            self.preview_byte_top = nt;
            self.preview_top_line = nl;
        }
    }

    /// For windowed display, read the current window (from the top byte for the height), highlight Code with syntect
    /// / leave Text plain, and return it as a list of ratatui Lines. Cached by (path, top byte, height),
    /// re-read only on scroll/resize. If the top exceeds the last page, it is clamped.
    pub fn windowed_lines(&mut self, height: u16, width: u16) -> Vec<Line<'static>> {
        let _ = width; // 行内容は幅に依存しない(折返しは描画側)。将来用に受けておく。
        let h = height.max(1) as usize;
        // リサイズ等で末尾を超えていたらクランプ(行番号も総行数から補正)。
        let top0 = self.preview_byte_top;
        let maxt = self
            .preview_win
            .as_mut()
            .and_then(|w| w.last_page_top(h).ok());
        if let Some(maxt) = maxt {
            if top0 > maxt {
                self.preview_byte_top = maxt;
                // 行番号ガター OFF でも検索の現在一致(橙)が abs=preview_top_line+i で
                // 参照するため、末尾ページへのクランプ時は line_numbers の有無に関わらず
                // 常に preview_top_line を総行数から補正する(#5)。
                if let Some(t) = self.win_total() {
                    self.preview_top_line = t.saturating_sub(h);
                }
            }
        }
        let top = self.preview_byte_top;
        let path = self.preview_path.clone().unwrap_or_default();
        // syntax を着けるか: ハイライト無効(off-switch) / progressive で待ち中(hl_pending)なら素。
        // Code は常に対象。Text は「拡張子/ファイル名から実在の文法が解決できる時だけ」対象にする＝
        // .bashrc/Makefile/Dockerfile 等の設定ファイルを着色しつつ、本当に素のテキストは無色のまま保つ。
        // progressive 待ち中だけは「差し替え前の素テキスト」なのでキャッシュしない(後で着色版に替わる)。
        let syntax_kind = match &self.preview_kind {
            Some(PreviewKind::Code(_)) => true,
            Some(PreviewKind::Text(p)) => crate::preview::code::has_named_syntax(p),
            // raw ソース表示(`R`)の Markdown/Mermaid はファイルの文法(.md→Markdown 等)で着色する。
            Some(PreviewKind::Markdown(p)) | Some(PreviewKind::Mermaid(p)) if self.md_raw => {
                crate::preview::code::has_named_syntax(p)
            }
            _ => false,
        };
        let want_syntax = self.cfg.ui.syntax_highlight
            && syntax_kind
            && (!self.hl_pending || self.loading_is_indicator());
        let mut content: Vec<Line<'static>> = if want_syntax {
            let hit = matches!(
                &self.win_cache,
                Some(c) if c.path == path && c.byte_top == top && c.height == height
            );
            if !hit {
                let raw = self
                    .preview_win
                    .as_mut()
                    .and_then(|w| w.read_lines(top, h).ok())
                    .unwrap_or_default();
                let src = raw.join("\n");
                let lines =
                    crate::preview::code::highlight(&src, &path, &self.cfg.ui.theme.code_theme);
                self.win_cache = Some(WinCache {
                    path,
                    byte_top: top,
                    height,
                    lines,
                });
            }
            self.win_cache
                .as_ref()
                .map(|c| c.lines.clone())
                .unwrap_or_default()
        } else {
            // 素テキスト(キャッシュしない): off-switch / progressive 待ち中 / Text。
            let raw = self
                .preview_win
                .as_mut()
                .and_then(|w| w.read_lines(top, h).ok())
                .unwrap_or_default();
            raw.into_iter().map(Line::from).collect()
        };
        // タブを可視マーカー(→)＋タブストップ空白へ展開する(端末はタブを桁揃えしないため)。
        // 行番号ガター/検索強調の前に行う＝桁追跡が本文の先頭(0桁)基準になる。窓の可視行のみ=軽い。
        content = crate::preview::code::expand_tabs(content, self.cfg.ui.tab_width);
        // 検索中は一致箇所(クエリ)を強調する(ガターより先=本文だけ強調)。
        // 現在の出現(search_idx)1箇所だけオレンジ、他は黄色。表示行 i の絶対行 = preview_top_line + i。
        // その行が現在の出現の行なら、その列(col)の出現だけオレンジにする(同一行に複数あっても1つ)。
        if let Some(q) = self.preview_search.clone() {
            // 現在一致(search_idx)の行と、その「行内での出現順位」(0始まり)を求める。
            // タブ展開でバイト列がずれるため、列(バイト位置)ではなく出現順位で
            // 現在一致を同定する(#14)。同一行の一致は search_matches 内で連続・列昇順。
            let (cur_line, cur_rank) = match self.search_matches.get(self.search_idx).copied() {
                Some((_, line, _)) => {
                    let rank = self.search_matches[..self.search_idx]
                        .iter()
                        .filter(|(_, l, _)| *l == line)
                        .count();
                    (Some(line), Some(rank))
                }
                None => (None, None),
            };
            let top_line = self.preview_top_line;
            content = content
                .into_iter()
                .enumerate()
                .map(|(i, l)| {
                    let abs = top_line + i;
                    let rank = if cur_line == Some(abs) {
                        cur_rank
                    } else {
                        None
                    };
                    highlight_query_in_line(l, &q, rank)
                })
                .collect();
        }
        let top_line = self.preview_top_line;
        // 2D キャレット列を可視のカーソル行の実長さにクランプし、書き戻す(`l`/`$` の行き過ぎ補正)。
        // キャレット行はビューポート追従で常に可視なので、ここで content から実長を取得できる。
        if self.preview_cursor_line >= top_line
            && self.preview_cursor_line < top_line + content.len()
        {
            let ln = &content[self.preview_cursor_line - top_line];
            let len: usize = ln.spans.iter().map(|s| s.content.chars().count()).sum();
            // 読み取り専用キャレットは常に実在の文字の上に置く(空行は 0)。$ は最後の文字へ。
            self.preview_cursor_col = self.preview_cursor_col.min(len.saturating_sub(1));
        }
        // 行カーソル/選択範囲のハイライト(ガター前＝列インデックスが本文先頭 0 基準に揃う)。
        let content = apply_preview_caret(
            content,
            top_line,
            self.preview_cursor_line,
            self.preview_cursor_col,
            self.preview_selection(),
        );
        // 行番号ガター(設定 ON のときだけ)。先頭行番号 = preview_top_line。
        let content = if self.cfg.ui.line_numbers {
            with_line_numbers(content, top_line)
        } else {
            content
        };
        // git 変更ガター(設定 ON・変更のあるファイルのみ)。行番号の左に 1 セルのマーカーを前置する。
        let marks = self.git_gutter_marks();
        with_git_gutter(content, top_line, &marks)
    }

    /// Git gutter marks (per 1-based new-file line) for the current code/text preview, cached per path.
    /// Empty when the gutter is disabled, outside a repo, or the file is unchanged (→ no gutter column).
    /// Called from `windowed_lines`, which only runs for code/text previews, so no kind check is needed.
    fn git_gutter_marks(&mut self) -> std::collections::HashMap<u32, GutterMark> {
        if !self.cfg.ui.git_gutter {
            return std::collections::HashMap::new();
        }
        let Some(path) = self.preview_path.clone() else {
            return std::collections::HashMap::new();
        };
        let hit = matches!(&self.gutter_cache, Some(c) if c.path == path);
        if !hit {
            let diff = crate::git::file_diff(&self.root, &path);
            let marks = gutter_marks(&diff);
            self.gutter_cache = Some(GutterCache {
                path: path.clone(),
                marks,
            });
        }
        self.gutter_cache
            .as_ref()
            .map(|c| c.marks.clone())
            .unwrap_or_default()
    }

    /// Windowed scroll progress (0..=100 %). For the title display. None if not windowed.
    pub fn window_progress(&self) -> Option<u16> {
        let w = self.preview_win.as_ref()?;
        let len = w.len();
        if len == 0 {
            return Some(0);
        }
        Some((self.preview_byte_top.min(len) * 100 / len) as u16)
    }

    /// Whether in-preview search input mode is active (intercepting keys).
    pub fn is_searching(&self) -> bool {
        self.search_input.is_some()
    }

    /// The active search query (for highlighting).
    pub fn preview_search_query(&self) -> Option<&str> {
        self.preview_search.as_deref()
    }

    /// The search input being edited (for the footer prompt).
    pub fn search_input(&self) -> Option<&str> {
        self.search_input.as_deref()
    }

    /// Search status (current/total). Some((1-based, total)) when active and there are matches.
    pub fn search_status(&self) -> Option<(usize, usize)> {
        if self.preview_search.is_some() && !self.search_matches.is_empty() {
            Some((self.search_idx + 1, self.search_matches.len()))
        } else {
            None
        }
    }

    /// Start a search (`/`). Currently only Code/Text (windowed) previews are supported. Enters input mode.
    pub fn start_search(&mut self) {
        if self.preview_win.is_none() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::SearchCodeTextOnly).into());
            return;
        }
        self.search_input = Some(String::new());
    }

    pub fn search_input_push(&mut self, c: char) {
        if let Some(s) = self.search_input.as_mut() {
            s.push(c);
        }
    }

    pub fn search_input_backspace(&mut self) {
        if let Some(s) = self.search_input.as_mut() {
            s.pop();
        }
    }

    /// Confirm input (Enter): run the query (collect all matching lines) and jump to the first match at or after the current position.
    pub fn search_commit(&mut self) {
        let q = self.search_input.take().unwrap_or_default();
        if q.is_empty() {
            self.preview_search = None;
            self.search_matches.clear();
            return;
        }
        const CAP: usize = 5000;
        let matches = self
            .preview_win
            .as_mut()
            .and_then(|w| w.find_all_matches(&q, CAP).ok())
            .unwrap_or_default();
        self.preview_search = Some(q);
        self.search_matches = matches;
        if self.search_matches.is_empty() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoMatch).into());
            return;
        }
        // 現在の表示位置(byte_top)以降の最初の出現へ。無ければ先頭へ巡回。
        let top = self.preview_byte_top;
        self.search_idx = self
            .search_matches
            .iter()
            .position(|(off, _, _)| *off >= top)
            .unwrap_or(0);
        self.jump_to_match();
    }

    /// `n`/`N`: to the next/previous match (cyclic).
    pub fn search_next(&mut self, dir: i32) {
        if self.search_matches.is_empty() {
            return;
        }
        let n = self.search_matches.len() as i32;
        self.search_idx = (self.search_idx as i32 + dir).rem_euclid(n) as usize;
        self.jump_to_match();
    }

    /// Clear the search (Esc).
    pub fn search_clear(&mut self) {
        self.preview_search = None;
        self.search_input = None;
        self.search_matches.clear();
        self.search_idx = 0;
    }

    /// Bring the line of the current occurrence to the top of the display (updates the line-head byte and line number). For moves within the same line,
    /// the top does not change, and only the highlight color (orange) moves to that occurrence (the column is referenced by the render side).
    fn jump_to_match(&mut self) {
        if let Some(&(off, line, _col)) = self.search_matches.get(self.search_idx) {
            self.preview_byte_top = off;
            self.preview_top_line = line;
        }
    }

    pub fn preview_hscroll(&mut self, delta: i32) {
        let next = self.preview_hscroll as i32 + delta;
        self.preview_hscroll = next.max(0) as u16;
    }
    /// Reset horizontal scroll to the line start (left edge) in one step. `0`. When wrapping, it is already 0, so no effect.
    pub fn preview_hscroll_home(&mut self) {
        self.preview_hscroll = 0;
    }
    /// Send horizontal scroll to the line end (right edge) in one step. `$`. The actual maximum is clamped to the longest line width at render time
    /// (here we set the upper bound, and the render side's `.min(max_h)` settles it at the correct right edge).
    pub fn preview_hscroll_end(&mut self) {
        self.preview_hscroll = u16::MAX;
    }

    /// Arrange the links in decorated Markdown lines to "show label only", build `md_items` (links +
    /// task checkboxes), and return the line list with the focused item rendered inverted (called just
    /// before rendering). Since tui-markdown outputs the "label (URL)" form, `collapse_links` folds the
    /// accompanying URL (the URL is the hidden destination) and styles the label as a link plus (when
    /// configured) an icon. Checkbox markers are recognized by their dedicated style (`is_task_span`).
    pub fn decorate_md_items(&mut self, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        let (lines, targets) = collapse_links(lines, self.cfg.ui.icons);
        // 畳んだリンク span とタスクマーカー span を出現順に走査して md_items を作る。
        let mut items = Vec::new();
        let mut k = 0usize;
        for (li, line) in lines.iter().enumerate() {
            for span in &line.spans {
                if is_link_span(span) {
                    let target = targets.get(k).cloned().unwrap_or_default();
                    items.push(MdItem {
                        line: li,
                        kind: MdItemKind::Link { target },
                    });
                    k += 1;
                } else if crate::preview::markdown::is_task_span(span) {
                    if let Some(state) =
                        crate::preview::markdown::task_span_state(span.content.as_ref())
                    {
                        items.push(MdItem {
                            line: li,
                            kind: MdItemKind::Task { state },
                        });
                    }
                }
            }
        }
        // フォーカス添字を範囲内にクランプ。
        match self.focused_item {
            Some(_) if items.is_empty() => self.focused_item = None,
            Some(f) if f >= items.len() => self.focused_item = Some(items.len() - 1),
            _ => {}
        }
        self.md_items = items;

        let Some(target_idx) = self.focused_item else {
            return lines;
        };
        // フォーカス中アイテム(n 番目のリンク/マーカー span)を反転して強調。
        use ratatui::style::Modifier;
        let mut seen = 0usize;
        lines
            .into_iter()
            .map(|line| {
                let style = line.style;
                let spans = line
                    .spans
                    .into_iter()
                    .map(|mut span| {
                        if is_link_span(&span) || crate::preview::markdown::is_task_span(&span) {
                            if seen == target_idx {
                                span.style = span.style.add_modifier(Modifier::REVERSED);
                            }
                            seen += 1;
                        }
                        span
                    })
                    .collect::<Vec<_>>();
                Line::from(spans).style(style)
            })
            .collect()
    }

    /// Move the item focus of the Markdown preview (dir: +1=next / -1=previous, cyclic over links and
    /// checkboxes in document order). If the focused line is off-screen, scroll until it is visible.
    pub fn md_focus_move(&mut self, dir: i32) {
        if self.md_items.is_empty() {
            return;
        }
        let n = self.md_items.len() as i32;
        let next = match self.focused_item {
            Some(f) => (f as i32 + dir).rem_euclid(n),
            None if dir >= 0 => 0,
            None => n - 1,
        } as usize;
        self.focused_item = Some(next);
        // フォーカス行を表示範囲に収める。
        let line = self.md_items[next].line;
        let vh = self.preview_viewport.max(1) as usize;
        let scroll = self.preview_scroll as usize;
        if line < scroll {
            self.preview_scroll = line as u16;
        } else if line >= scroll + vh {
            self.preview_scroll = (line + 1).saturating_sub(vh) as u16;
        }
    }

    /// Activate the focused item: a link opens (URLs externally, local paths within konoma),
    /// a task checkbox toggles.
    pub fn md_activate_focused(&mut self) -> Result<()> {
        let Some(f) = self.focused_item else {
            return Ok(());
        };
        match self.md_items.get(f) {
            Some(MdItem {
                kind: MdItemKind::Link { target },
                ..
            }) => {
                let target = target.clone();
                self.open_link_target(&target)
            }
            Some(MdItem {
                kind: MdItemKind::Task { .. },
                ..
            }) => {
                self.md_toggle_focused_task();
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Whether the currently focused Markdown item is a task checkbox (drives the Space fixed key:
    /// only then does Space toggle instead of falling through to the keymap).
    pub fn md_focused_task(&self) -> bool {
        !self.is_raw_source()
            && self
                .focused_item
                .and_then(|f| self.md_items.get(f))
                .is_some_and(|it| matches!(it.kind, MdItemKind::Task { .. }))
    }

    /// Whether the rendered Markdown preview has any task checkboxes (drives the footer hint).
    pub fn md_has_tasks(&self) -> bool {
        self.md_items
            .iter()
            .any(|it| matches!(it.kind, MdItemKind::Task { .. }))
    }

    /// Open a link target. `scheme://`/`mailto:`, etc. are delegated externally; local paths open in konoma
    /// (file=preview / directory=make it the new root and Tree).
    fn open_link_target(&mut self, target: &str) -> Result<()> {
        let t = target.trim();
        if t.contains("://") || t.starts_with("mailto:") || t.starts_with("tel:") {
            return self.open_external(t);
        }
        if t.starts_with('#') {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::AnchorsUnsupported).into());
            return Ok(());
        }
        // ローカルパス: md ファイルのディレクトリ基準で解決 (# 以降のアンカーは捨てる)。
        let path_part = t.split('#').next().unwrap_or(t);
        let base = self
            .preview_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        let p = Path::new(path_part);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        };
        let resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        if resolved.is_dir() {
            self.back_to_tree();
            // root を変えるので旧 root の選択/ビジュアル/絞り込み/検索を破棄する(持ち越さない)。
            self.clear_for_root_change();
            self.root = resolved;
            self.open_dir = self.root.clone();
            self.entries.clear();
            self.selected = 0;
            self.rebuild_tree()?;
        } else if resolved.is_file() {
            self.enter_preview(&resolved);
        } else {
            self.flash = Some(format!(
                "{}{}",
                tr(self.lang, crate::i18n::Msg::NotFound),
                path_part
            ));
        }
        Ok(())
    }

    /// Open a URL/file with an external command (macOS `open`). The result is reported via flash.
    fn open_external(&mut self, url: &str) -> Result<()> {
        match std::process::Command::new("open").arg(url).spawn() {
            Ok(_) => self.flash = Some(format!("{}{url}", tr(self.lang, crate::i18n::Msg::Opened))),
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::OpenFailed)
                ))
            }
        }
        Ok(())
    }

    pub fn cycle_path_style(&mut self) {
        self.path_style = self.path_style.next();
    }

    /// Toggle the `?` help. When opening, reset scroll to the top.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
        self.help_scroll = 0;
    }

    /// Toggle the `i` file-info popup. Does not open if there is no target.
    pub fn toggle_info(&mut self) {
        if self.show_info {
            self.show_info = false;
        } else if self.info_target().is_some() {
            self.show_info = true;
        } else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
        }
    }
    /// Whether the file-info popup is showing.
    pub fn is_info(&self) -> bool {
        self.show_info
    }
    /// The target path for the info display: Tree=the cursor item / Preview=the file being previewed.
    pub fn info_target(&self) -> Option<PathBuf> {
        match self.mode {
            Mode::Tree => self.entries.get(self.selected).map(|e| e.path.clone()),
            Mode::Preview => self.preview_path.clone(),
        }
    }

    /// Scroll within the help. The upper bound is clamped at render time (since content and screen size are then known).
    pub fn help_scroll_by(&mut self, delta: i32) {
        self.help_scroll = (self.help_scroll as i32 + delta).max(0) as u16;
    }

    /// The path to copy. In Preview it is the preview target; in Tree it is the entry selected in the tree.
    fn copy_target(&self) -> Option<PathBuf> {
        // Git 変更ハブ表示中は、ツリーの選択ではなく**変更ファイル**のパスをコピー対象にする。
        #[cfg(feature = "git")]
        if self.surface() == crate::keymap::Surface::GitChanges {
            return self.git_view_selected();
        }
        match self.mode {
            Mode::Preview => self.preview_path.clone(),
            Mode::Tree => self.entries.get(self.selected).map(|e| e.path.clone()),
        }
    }

    /// Copy the selected path to the clipboard according to the kind, and show the result in flash (FR-6).
    pub fn copy_path(&mut self, kind: CopyKind) {
        let Some(path) = self.copy_target() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        let text = copy_text(&path, &self.open_dir, kind);
        match set_clipboard(&text) {
            Ok(()) => {
                self.flash = Some(format!(
                    "{}{text}",
                    tr(self.lang, crate::i18n::Msg::CopiedPrefix)
                ))
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::CopyFailed)
                ))
            }
        }
    }

    /// Get metadata for the selected commit in log/graph/detail. detail uses the already-loaded data, while log/graph
    /// fetch `commit_meta` from the selected commit id. None for non-commits (uncommitted rows, etc.) or out-of-scope surfaces.
    #[cfg(feature = "git")]
    fn current_commit_meta(&self) -> Option<crate::git::CommitMeta> {
        use crate::keymap::Surface;
        match self.surface() {
            Surface::GitDetail => self.git_detail_meta.clone(),
            Surface::GitLog => {
                let id = self.git_log_selected_id()?;
                crate::git::commit_meta(&self.root, &id)
            }
            Surface::GitGraph => {
                let id = self
                    .git_graph_selected_row()
                    .and_then(|r| r.commit.clone())?;
                crate::git::commit_meta(&self.root, &id)
            }
            _ => None,
        }
    }

    /// git log/graph/detail: copy the selected commit's info (short/full hash, subject, full message, author, date)
    /// to the clipboard. If there is no commit, notify via flash.
    #[cfg(feature = "git")]
    pub fn git_copy(&mut self, kind: GitCopyKind) {
        let Some(meta) = self.current_commit_meta() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCommitToCopy).into());
            return;
        };
        let text = match kind {
            GitCopyKind::ShortHash => meta.short,
            GitCopyKind::FullHash => meta.id,
            GitCopyKind::Subject => meta.message.lines().next().unwrap_or("").to_string(),
            GitCopyKind::Message => meta.message,
            GitCopyKind::Author => meta.author,
            GitCopyKind::Date => meta.date,
        };
        self.set_clipboard_flash(&text);
    }

    /// branches view: copy the selected branch name to the clipboard.
    #[cfg(feature = "git")]
    pub fn git_copy_branch_name(&mut self) {
        let Some(b) = self.git_branch_selected() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        let name = b.name;
        self.set_clipboard_flash(&name);
    }

    /// Write to the clipboard and flash success/failure (common processing for copy operations). On success, shows a one-line preview
    /// (multi-line content such as a full message is rounded to the first line + `…` so the footer does not overflow).
    fn set_clipboard_flash(&mut self, text: &str) {
        match set_clipboard(text) {
            Ok(()) => {
                let first = text.lines().next().unwrap_or("");
                let mut preview: String = first.chars().take(60).collect();
                if first.chars().count() > 60 || text.lines().nth(1).is_some() {
                    preview.push('…');
                }
                self.flash = Some(format!(
                    "{}{preview}",
                    tr(self.lang, crate::i18n::Msg::CopiedPrefix)
                ));
            }
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    tr(self.lang, crate::i18n::Msg::CopyFailed)
                ))
            }
        }
    }

    // ---- タブ (FR-5) ----

    /// Snapshot the current tree working state.
    fn snapshot_tab(&self) -> TabState {
        TabState {
            root: self.root.clone(),
            open_dir: self.open_dir.clone(),
            entries: self.entries.clone(),
            selected: self.selected,
            show_hidden: self.show_hidden,
            tree_viewport: self.tree_viewport,
            mode: self.mode,
            preview_path: self.preview_path.clone(),
            preview_kind: self.preview_kind.clone(),
            preview_scroll: self.preview_scroll,
            preview_hscroll: self.preview_hscroll,
            preview_viewport: self.preview_viewport,
            preview_byte_top: self.preview_byte_top,
            preview_top_line: self.preview_top_line,
            image_zoom: self.image_zoom,
            image_center: self.image_center,
            pdf_page: self.pdf_page,
            pdf_pages: self.pdf_pages,
            table_cur_row: self.table_cur_row,
            table_cur_col: self.table_cur_col,
            table_top_row: self.table_top_row,
            table_left_col: self.table_left_col,
            preview_cursor_line: self.preview_cursor_line,
            preview_cursor_col: self.preview_cursor_col,
            md_raw: self.md_raw,
            git_view: self.git_view,
            git_view_sel: self.git_view_sel,
            git_view_entries: self.git_view_entries.clone(),
            came_from_git_view: self.came_from_git_view,
            git_log: self.git_log.clone(),
            git_log_sel: self.git_log_sel,
            git_detail: self.git_detail.clone(),
            git_detail_meta: self.git_detail_meta.clone(),
            git_detail_title: self.git_detail_title.clone(),
            git_detail_scroll: self.git_detail_scroll,
            git_detail_hscroll: self.git_detail_hscroll,
            git_detail_viewport: self.git_detail_viewport,
            git_detail_total: self.git_detail_total,
            git_branches: self.git_branches.clone(),
            git_branch_sel: self.git_branch_sel,
            git_branch_filter: self.git_branch_filter.clone(),
            git_branch_filtering: self.git_branch_filtering,
            git_graph: self.git_graph.clone(),
            git_graph_sel: self.git_graph_sel,
            // 選択 / 絞り込み / プレビュー検索もタブ毎に保存する(切替で旧タブへ持ち越さない)。
            selection: self.selection.clone(),
            visual_anchor: self.visual_anchor,
            tree_filter: self.tree_filter.clone(),
            filter_input: self.filter_input.clone(),
            filter_pool: self.filter_pool.clone(),
            changed_filter: self.changed_filter,
            preview_search: self.preview_search.clone(),
            search_input: self.search_input.clone(),
            search_matches: self.search_matches.clone(),
            search_idx: self.search_idx,
        }
    }

    /// Save the current working state to the active tab.
    fn save_active(&mut self) {
        let snap = self.snapshot_tab();
        self.tabs[self.active_tab] = snap;
    }

    /// Load the active tab's snapshot into the working fields.
    /// **Also restores that tab's mode/preview** (does not drop to Tree). Images are restored by reloading.
    fn load_active(&mut self) {
        let t = self.tabs[self.active_tab].clone();
        self.root = t.root;
        self.open_dir = t.open_dir;
        self.entries = t.entries;
        self.selected = t.selected;
        self.show_hidden = t.show_hidden;
        self.tree_viewport = t.tree_viewport;
        self.mode = t.mode;
        self.preview_path = t.preview_path;
        self.preview_kind = t.preview_kind;
        self.preview_scroll = t.preview_scroll;
        self.preview_hscroll = t.preview_hscroll;
        self.preview_viewport = t.preview_viewport;
        self.preview_byte_top = t.preview_byte_top;
        self.preview_top_line = t.preview_top_line;
        // git オーバーレイもそのタブの状態を復元する(タブごとに git モードを保持)。
        self.git_view = t.git_view;
        self.git_view_sel = t.git_view_sel;
        self.git_view_entries = t.git_view_entries;
        self.came_from_git_view = t.came_from_git_view;
        self.git_log = t.git_log;
        self.git_log_sel = t.git_log_sel;
        self.git_detail = t.git_detail;
        self.git_detail_meta = t.git_detail_meta;
        self.git_detail_title = t.git_detail_title;
        self.git_detail_scroll = t.git_detail_scroll;
        self.git_detail_hscroll = t.git_detail_hscroll;
        self.git_detail_viewport = t.git_detail_viewport;
        self.git_detail_total = t.git_detail_total;
        self.git_branches = t.git_branches;
        self.git_branch_sel = t.git_branch_sel;
        self.git_branch_filter = t.git_branch_filter;
        self.git_branch_filtering = t.git_branch_filtering;
        self.git_graph = t.git_graph;
        self.git_graph_sel = t.git_graph_sel;
        // 選択 / 絞り込み / プレビュー検索もそのタブの状態を復元する(entries も同時に復元するため
        // visual_anchor(entries 添字)は整合する)。load_active は rebuild_tree を呼ばない。
        self.selection = t.selection;
        self.visual_anchor = t.visual_anchor;
        self.tree_filter = t.tree_filter;
        self.filter_input = t.filter_input;
        self.filter_pool = t.filter_pool;
        self.changed_filter = t.changed_filter;
        self.preview_search = t.preview_search;
        self.search_input = t.search_input;
        self.search_matches = t.search_matches;
        self.search_idx = t.search_idx;
        // 装飾キャッシュは持ち越さない (decorated_lines が再生成)。
        self.md_cache = None;
        // 復元した diff プレビューはフォロー由来の印を持ち越さない(セッションはタブ横断の概念でない)。
        self.diff_follow_scope = false;
        // 画像は重い状態(protocol/元画像/GIFフレーム)を持ち越さず、画像系プレビューなら再読込で復元する。
        self.clear_image();
        if let (Some(kind), Some(path)) = (self.preview_kind.clone(), self.preview_path.clone()) {
            if matches!(
                kind,
                PreviewKind::Image(_)
                    | PreviewKind::Svg(_)
                    | PreviewKind::Video(_)
                    | PreviewKind::Pdf(_)
            ) {
                // SVG/動画サムネ/GIF は別スレッドで読み込み開始(set_* は zoom/center を触らない)。
                // 保存値を即セットしておけば、後から結果が届いても復元したズーム/中心が保たれる。
                // GIF は先頭フレームから再生。
                // PDF は保存ページを start_media_load の前に戻す(その世代でそのページをラスタライズする)。
                self.pdf_page = t.pdf_page.max(1);
                self.pdf_pages = t.pdf_pages;
                self.start_media_load(&kind, &path);
                self.image_zoom = t.image_zoom;
                self.image_center = t.image_center;
                self.image_crop = None;
            }
        }
        // raw ソース表示状態を復元してから windowed を張り直す(raw md は窓読みにするため順序が重要)。
        self.md_raw = t.md_raw;
        // 大きい Code/Text(＋raw の Markdown/Mermaid)なら ウィンドウ読みリーダを張り直す(byte_top は上で復元済み)。
        self.setup_windowed();
        // windowed プレビューの 2D キャレットを復元(選択は持ち越さない)。範囲は次描画/移動でクランプされる。
        self.preview_cursor_line = t.preview_cursor_line;
        self.preview_cursor_col = t.preview_cursor_col;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        // CSV/TSV テーブルは本体を再パースし、保存済みカーソル/スクロールを復元してクランプする。
        self.load_table();
        self.table_cur_row = t.table_cur_row;
        self.table_cur_col = t.table_cur_col;
        self.table_top_row = t.table_top_row;
        self.table_left_col = t.table_left_col;
        self.clamp_table_cursor();
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    /// Display name of tab `i`. While showing Tree, the **root directory name**; while showing Preview/image, etc.,
    /// the **preview target's file name**. The active tab references the latest working state (App fields),
    /// while inactive tabs reference the snapshot (`TabState`) (since saving happens only on switch).
    pub fn tab_label(&self, i: usize) -> String {
        // (mode, root, preview_path) をアクティブ/非アクティブで使い分ける。
        let (mode, root, preview) = if i == self.active_tab {
            (self.mode, self.root.as_path(), self.preview_path.as_deref())
        } else if let Some(t) = self.tabs.get(i) {
            (t.mode, t.root.as_path(), t.preview_path.as_deref())
        } else {
            return String::new();
        };
        let path = match mode {
            Mode::Preview => preview.unwrap_or(root), // 念のため preview 無しは root へ
            Mode::Tree => root,
        };
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }

    /// New tab. Create another tree context starting from the current root and switch to it.
    /// The new tab starts in Tree (since it has no preview target). The source tab's preview is
    /// preserved by `save_active`, so it is not lost.
    pub fn tab_new(&mut self) -> Result<()> {
        self.save_active();
        let root = self.root.clone();
        self.open_dir = root.clone();
        self.root = root;
        self.selected = 0;
        self.entries.clear();
        self.rebuild_tree()?;
        // snapshot 前に Tree へリセットする (snapshot がプレビュー状態も取り込むため順序が重要)。
        self.mode = Mode::Tree;
        self.clear_image();
        self.preview_path = None;
        self.preview_kind = None;
        self.preview_scroll = 0;
        self.preview_hscroll = 0;
        self.preview_byte_top = 0;
        self.preview_top_line = 0;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.md_cache = None;
        // 新規タブは git オーバーレイ無しの素の Tree から始める(現タブの git 状態は save_active で保持済み)。
        self.git_view = false;
        self.git_view_sel = 0;
        self.git_view_entries.clear();
        self.came_from_git_view = false;
        self.git_log = None;
        self.git_log_sel = 0;
        self.git_detail = None;
        self.git_detail_meta = None;
        self.git_detail_title = None;
        self.git_detail_scroll = 0;
        self.git_detail_hscroll = 0;
        self.git_detail_viewport = 0;
        self.git_detail_total = 0;
        self.git_branches = None;
        self.git_branch_sel = 0;
        self.git_branch_filter.clear();
        self.git_branch_filtering = false;
        self.git_graph = None;
        self.git_graph_sel = 0;
        // 新規タブは選択 / 絞り込み / プレビュー検索を空から始める
        // (現タブのこれらは直前の save_active で TabState に保持済み)。
        self.selection.clear();
        self.visual_anchor = None;
        self.clear_filter_state();
        self.search_clear();
        self.tabs.push(self.snapshot_tab());
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }

    /// Close the active tab. The last one is not closed.
    pub fn tab_close(&mut self) {
        if self.tabs.len() <= 1 {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::CantCloseLastTab).into());
            return;
        }
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.load_active();
    }

    /// Move tabs relatively (dir: +1=next / -1=previous). Wraps at the ends.
    pub fn tab_cycle(&mut self, dir: i32) {
        if self.tabs.len() <= 1 {
            return;
        }
        self.save_active();
        let n = self.tabs.len() as i32;
        self.active_tab = (self.active_tab as i32 + dir).rem_euclid(n) as usize;
        self.load_active();
    }

    /// Select a tab by number (0-based). Out-of-range or the same tab is ignored.
    pub fn tab_goto(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active_tab {
            return;
        }
        self.save_active();
        self.active_tab = i;
        self.load_active();
    }

    /// Format a path into a display string using the current path_style (shared by the tree/preview title).
    pub fn format_path(&self, path: &Path) -> String {
        match self.path_style {
            PathStyle::Full => path.display().to_string(),
            PathStyle::Home => home_relative(path),
            PathStyle::Relative => rel_to_open(&self.open_dir, path),
        }
    }
}

/// Common helper that moves a list cursor by delta and returns it clamped to `[0, len-1]`.
/// Even if the caller (`g`/`G`/Home/End) passes `delta=i32::MIN/MAX`, the computation is done in i64, so it
/// does not panic on overflow (the plain `as i32 + delta` addition used to crash in debug).
/// `len==0` returns 0. The behavior is unchanged from the previous `(cur as i32 + delta).clamp(0, len-1)`.
fn clamp_cursor(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len.saturating_sub(1) as i64;
    (cur as i64).saturating_add(delta as i64).clamp(0, last) as usize
}

/// Whether a span is a Markdown link (blue underline). Since konoma's StyleSheet.link()=underline+blue draws them,
/// these two conditions decide it (code/headings/rules have different colors, so no false positives).
fn is_link_span(span: &Span<'static>) -> bool {
    use ratatui::style::{Color, Modifier};
    span.style.add_modifier.contains(Modifier::UNDERLINED) && span.style.fg == Some(Color::Blue)
}

/// Fold the accompanying URL of the "label (URL)" that tui-markdown emits, leaving **only the label** in link style (blue underline,
/// with a leading link icon when `icons=true`). The URLs are collected in order into `targets` and returned (hidden destinations).
/// Pattern: `[label]` `" ("` `[URL(blue underline)]` `")"`. Spans that do not match are passed through unchanged.
fn collapse_links(lines: Vec<Line<'static>>, icons: bool) -> (Vec<Line<'static>>, Vec<String>) {
    use ratatui::style::{Color, Modifier, Style};
    let link_style = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    let mut out = Vec::with_capacity(lines.len());
    let mut targets = Vec::new();
    for line in lines {
        let style = line.style;
        let spans = line.spans;
        let n = spans.len();
        let mut new: Vec<Span<'static>> = Vec::with_capacity(n);
        let mut i = 0;
        while i < n {
            // 表セル由来の「ラベル＋隠しターゲット」ペア(markdown::render_table が生成):
            // ラベルはそのまま残し(幅を変えない=表の桁揃え維持のためアイコンも付けない)、
            // 隠しスパンを取り除いて URL を targets へ回収する(Tab/Enter は通常リンクと同じ)。
            if i + 1 < n
                && is_link_span(&spans[i])
                && crate::preview::markdown::is_hidden_link_target(&spans[i + 1])
            {
                targets.push(spans[i + 1].content.to_string());
                new.push(spans[i].clone());
                i += 2;
                continue;
            }
            let is_link_pattern = i + 2 < n
                && spans[i + 1].content.as_ref() == " ("
                && is_link_span(&spans[i + 2])
                && spans.get(i + 3).is_some_and(|s| s.content.starts_with(')'));
            if is_link_pattern {
                let label = spans[i].content.as_ref();
                targets.push(spans[i + 2].content.to_string());
                let text = if icons {
                    format!("{} {label}", crate::ui::icons::link_icon())
                } else {
                    label.to_string()
                };
                new.push(Span::styled(text, link_style));
                // 閉じ括弧 ")" の後ろに続く文字(句読点等)は保持する。
                let closer = spans[i + 3].content.as_ref();
                if closer.len() > 1 {
                    new.push(Span::styled(closer[1..].to_string(), spans[i + 3].style));
                }
                i += 4;
            } else {
                new.push(spans[i].clone());
                i += 1;
            }
        }
        out.push(Line::from(new).style(style));
    }
    (out, targets)
}

/// Within line `line` (multiple colored spans), highlight the parts matching `query` (substring, case-insensitive)
/// in a highlighter style (background color + black text + bold). Even for matches crossing span boundaries, each span is split at the
/// match boundaries and the background is applied (the original fg/bold/italic are kept). If the byte length changes when lowercasing
/// (multibyte), it does nothing to avoid mis-slicing.
/// When `current_occurrence`=Some(occurrence rank within the line, 0-based), **only that one occurrence is orange**, and
/// the others are yellow (vim's CurSearch/Search style; `n`/`N` change the color of only the selected occurrence). None means all yellow.
/// Since occurrences are identified by rank rather than column (byte position), the current match stays stable even if the byte sequence shifts from tab expansion (#14).
fn highlight_query_in_line(
    line: Line<'static>,
    query: &str,
    current_occurrence: Option<usize>,
) -> Line<'static> {
    use ratatui::style::{Color, Modifier};
    if query.is_empty() {
        return line;
    }
    let full: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let lower = full.to_lowercase();
    let q = query.to_lowercase();
    if lower.len() != full.len() {
        return line; // 非 ASCII でバイト長が変わる場合は安全側で強調なし
    }
    // 一致バイト範囲を収集。
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while let Some(rel) = lower[i..].find(&q) {
        let s = i + rel;
        let e = s + q.len();
        ranges.push((s, e));
        i = e;
    }
    if ranges.is_empty() {
        return line;
    }
    let style = line.style;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut pos = 0usize; // full 内のバイト位置
    for span in line.spans {
        let text = span.content.into_owned();
        let span_start = pos;
        let span_end = pos + text.len();
        // この span 内の分割点(span 端＋一致境界)。
        let mut points = vec![span_start, span_end];
        for (s, e) in &ranges {
            if *s > span_start && *s < span_end {
                points.push(*s);
            }
            if *e > span_start && *e < span_end {
                points.push(*e);
            }
        }
        points.sort_unstable();
        points.dedup();
        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            if let Some(seg) = text.get(a - span_start..b - span_start) {
                if seg.is_empty() {
                    continue;
                }
                // この区間が属する一致範囲の出現順位(あれば)。その順位が current_occurrence
                // なら現在の出現=オレンジ、他の一致は黄色。範囲外は素のまま。
                let hit = ranges.iter().position(|(s, e)| a >= *s && b <= *e);
                let st = if let Some(idx) = hit {
                    let bg = if current_occurrence == Some(idx) {
                        Color::Rgb(0xff, 0x8c, 0x00) // 現在の出現
                    } else {
                        Color::Yellow // 他の出現
                    };
                    span.style
                        .bg(bg)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    span.style
                };
                out.push(Span::styled(seg.to_string(), st));
            }
        }
        pos = span_end;
    }
    Line::from(out).style(style)
}

/// Editor-style git change status of one line in the previewed (working-tree) file, for the left gutter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GutterMark {
    /// Newly inserted line (no matching removal): green bar.
    Added,
    /// Line replaced/edited (part of a hunk that also removed lines): blue bar.
    Modified,
    /// One or more lines were removed just above this line: red boundary marker.
    Deleted,
}

// Gutter marker colors, matching Zed: green added, amber modified, red deleted.
const GUTTER_ADDED: ratatui::style::Color = ratatui::style::Color::Rgb(87, 171, 90);
const GUTTER_MODIFIED: ratatui::style::Color = ratatui::style::Color::Rgb(216, 166, 74);
const GUTTER_DELETED: ratatui::style::Color = ratatui::style::Color::Rgb(199, 84, 80);

/// Derive a per-new-line git gutter map from a file's working-tree diff. Keyed by 1-based new-file line
/// number. A change block is a maximal run of Added/Removed lines (Context breaks it): with additions it
/// marks each added line Modified (if the block also removed lines) or Added (pure insertion); a
/// pure-deletion block marks the following line Deleted (or the preceding line if the deletion is at EOF).
fn gutter_marks(diff: &[crate::git::DiffLine]) -> std::collections::HashMap<u32, GutterMark> {
    use crate::git::DiffLineKind;
    let mut marks = std::collections::HashMap::new();
    let mut i = 0;
    while i < diff.len() {
        if diff[i].kind == DiffLineKind::Context {
            i += 1;
            continue;
        }
        let start = i;
        let mut removed = 0usize;
        let mut added: Vec<u32> = Vec::new();
        while i < diff.len() && diff[i].kind != DiffLineKind::Context {
            match diff[i].kind {
                DiffLineKind::Removed => removed += 1,
                DiffLineKind::Added => {
                    if let Some(n) = diff[i].new_no {
                        added.push(n);
                    }
                }
                DiffLineKind::Context => {}
            }
            i += 1;
        }
        if !added.is_empty() {
            let mark = if removed > 0 {
                GutterMark::Modified
            } else {
                GutterMark::Added
            };
            for n in added {
                marks.insert(n, mark);
            }
        } else if removed > 0 {
            // Pure deletion: anchor the marker to the line just below the removed block; if the block is
            // at EOF, anchor to the last real line above it (deletion below).
            let anchor = diff
                .get(i)
                .and_then(|d| d.new_no)
                .or_else(|| diff[..start].iter().rev().find_map(|d| d.new_no));
            if let Some(a) = anchor {
                marks.entry(a).or_insert(GutterMark::Deleted);
            }
        }
    }
    marks
}

/// Prepend a one-cell git change marker to each line (before the line-number gutter, Zed-style). Only
/// applied when the file has changes (`marks` non-empty) — unchanged files / non-repos keep their layout.
/// The displayed line `i` maps to 1-based file line `top_line + i + 1`; unchanged lines get a blank cell.
fn with_git_gutter(
    lines: Vec<Line<'static>>,
    top_line: usize,
    marks: &std::collections::HashMap<u32, GutterMark>,
) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Style};
    if marks.is_empty() {
        return lines;
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let n = (top_line + i + 1) as u32;
            let (glyph, color) = match marks.get(&n) {
                Some(GutterMark::Added) => ("▌", GUTTER_ADDED),
                Some(GutterMark::Modified) => ("▌", GUTTER_MODIFIED),
                // Thin bar hugging the TOP edge of the line below a deletion, so it
                // sits on the seam between the two lines instead of looking like the
                // line itself was removed. (A terminal cell can't draw in the inter-row
                // gap, so the top edge is the closest we can get without adding a row.)
                Some(GutterMark::Deleted) => ("▔", GUTTER_DELETED),
                None => (" ", Color::Reset),
            };
            let style = line.style;
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::styled(glyph, Style::new().fg(color)));
            spans.extend(line.spans);
            Line::from(spans).style(style)
        })
        .collect()
}

/// The active visual selection in a windowed preview, normalized so `start ≤ end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewSelection {
    /// Not selecting (only the block caret is drawn).
    None,
    /// Charwise (`v`): an inclusive `(line, col)` character range across one or more lines.
    Char {
        start: (usize, usize),
        end: (usize, usize),
    },
    /// Linewise (`V`): whole logical lines `lo..=hi`.
    Line { lo: usize, hi: usize },
}

/// Extract the text of a charwise selection from the file's logical lines (`start`/`end` end-inclusive,
/// columns are char indices). Slicing by chars (not bytes) keeps multibyte/CJK content intact.
fn selection_char_text(lines: &[&str], start: (usize, usize), end: (usize, usize)) -> String {
    let (sl, sc) = start;
    let (el, ec) = end;
    if sl >= lines.len() {
        return String::new();
    }
    let el = el.min(lines.len() - 1);
    if sl == el {
        // 単一行: [sc..=ec](正規化済みで sc ≤ ec)
        lines[sl]
            .chars()
            .skip(sc)
            .take((ec + 1).saturating_sub(sc))
            .collect()
    } else {
        let mut out = String::new();
        out.push_str(&lines[sl].chars().skip(sc).collect::<String>());
        for l in &lines[sl + 1..el] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&lines[el].chars().take(ec + 1).collect::<String>());
        out
    }
}

/// The character range `[lo, hi)` to highlight on absolute line `abs` for a charwise selection, or None.
/// `usize::MAX` for `hi` means "to end of line". End is inclusive, so the last covered char is `end.1`.
fn char_range_for_line(
    abs: usize,
    start: (usize, usize),
    end: (usize, usize),
) -> Option<(usize, usize)> {
    if abs < start.0 || abs > end.0 {
        return None;
    }
    let lo = if abs == start.0 { start.1 } else { 0 };
    let hi = if abs == end.0 {
        end.1.saturating_add(1)
    } else {
        usize::MAX
    };
    Some((lo, hi))
}

/// Paint the 2D caret and selection onto the visible content lines (char-column based, applied BEFORE the
/// line-number/git gutters so column indices align with the file text). The whole-line cases also set the
/// line's base style so the gutter cells inherit the tint; charwise partial ranges tint only the spanned chars.
fn apply_preview_caret(
    lines: Vec<Line<'static>>,
    top_line: usize,
    cursor_line: usize,
    cursor_col: usize,
    sel: PreviewSelection,
) -> Vec<Line<'static>> {
    use ratatui::style::Color;
    // 暗色テーマ向け: 控えめな現在行背景と、選択範囲の少し強い青系背景。
    const CURSOR_BG: Color = Color::Rgb(55, 60, 74);
    const SEL_BG: Color = Color::Rgb(40, 66, 104);
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let abs = top_line + i;
            let caret = if abs == cursor_line {
                Some(cursor_col)
            } else {
                None
            };
            let (range, whole): (Option<(usize, usize, Color)>, bool) = match sel {
                PreviewSelection::None => {
                    if abs == cursor_line {
                        (Some((0, usize::MAX, CURSOR_BG)), true)
                    } else {
                        (None, false)
                    }
                }
                PreviewSelection::Line { lo, hi } => {
                    if abs >= lo && abs <= hi {
                        (Some((0, usize::MAX, SEL_BG)), true)
                    } else {
                        (None, false)
                    }
                }
                PreviewSelection::Char { start, end } => match char_range_for_line(abs, start, end)
                {
                    Some((lo, hi)) => (Some((lo, hi, SEL_BG)), lo == 0 && hi == usize::MAX),
                    None => (None, false),
                },
            };
            if range.is_none() && caret.is_none() {
                return line;
            }
            restyle_line(line, range, whole, caret)
        })
        .collect()
}

/// Rebuild a line applying a background to chars in `range` `(lo, hi, color)` and the REVERSED modifier to the
/// `caret` char. When `whole` is set, the line's base style also gets the bg (so gutters inherit it). Consecutive
/// chars with the same resulting style are coalesced into one span to keep the span count low.
fn restyle_line(
    line: Line<'static>,
    range: Option<(usize, usize, ratatui::style::Color)>,
    whole: bool,
    caret: Option<usize>,
) -> Line<'static> {
    use ratatui::style::Modifier;
    let base = line.style;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut idx = 0usize; // char index within the line content
    let mut cur = String::new();
    let mut cur_style: Option<ratatui::style::Style> = None;
    for span in line.spans.into_iter() {
        for ch in span.content.chars() {
            let mut style = span.style;
            if let Some((lo, hi, color)) = range {
                if idx >= lo && idx < hi {
                    style = style.bg(color);
                }
            }
            if caret == Some(idx) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if cur_style != Some(style) {
                if !cur.is_empty() {
                    out.push(Span::styled(std::mem::take(&mut cur), cur_style.unwrap()));
                }
                cur_style = Some(style);
            }
            cur.push(ch);
            idx += 1;
        }
    }
    if !cur.is_empty() {
        out.push(Span::styled(cur, cur_style.unwrap_or(base)));
    }
    let mut line = Line::from(out);
    line.style = if whole {
        if let Some((_, _, color)) = range {
            base.bg(color)
        } else {
            base
        }
    } else {
        base
    };
    line
}

/// Prepend a line-number gutter (right-aligned + a space) to each line. The first line's number is `top_line+1` (1-based).
/// A dim color (DarkGray) distinguishes it from the body. The original line style (background, etc.) is kept.
fn with_line_numbers(lines: Vec<Line<'static>>, top_line: usize) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Style};
    let last = top_line + lines.len().max(1);
    let width = last.to_string().len().max(3); // 最低 3 桁ぶんの幅を確保
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let n = top_line + i + 1;
            let gutter = Span::styled(format!("{n:>width$} "), Style::new().fg(Color::DarkGray));
            let style = line.style;
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(gutter);
            spans.extend(line.spans);
            Line::from(spans).style(style)
        })
        .collect()
}

/// The rectangle that centers an image of size `cells` (natural size) within `inner` while preserving the aspect ratio.
/// With `allow_upscale=false`, downscale only (scale<=1=fit behavior); with `true`, it may upscale to fill the area.
fn centered_rect(cells: (u16, u16), inner: Rect, allow_upscale: bool) -> Rect {
    let (cw, ch) = cells;
    if cw == 0 || ch == 0 || inner.width == 0 || inner.height == 0 {
        return inner;
    }
    let mut scale = (inner.width as f64 / cw as f64).min(inner.height as f64 / ch as f64);
    if !allow_upscale {
        scale = scale.min(1.0);
    }
    let w = ((cw as f64 * scale).round() as u16).clamp(1, inner.width);
    let h = ((ch as f64 * scale).round() as u16).clamp(1, inner.height);
    let x = inner.x + (inner.width - w) / 2;
    let y = inner.y + (inner.height - h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Max reserved rows for one inline Markdown image; a taller image is scaled down (keeping aspect) so it never dominates the viewport.
const MD_IMAGE_MAX_ROWS: u16 = 24;

/// Resolve a Markdown image URL to a local file path. A remote (`http(s)://`) URL resolves to its
/// download-cache path, but only once it has been fetched (so callers treat a cached remote image
/// exactly like a local one). `data:` URLs return None. Relative paths are resolved against the
/// Markdown file's directory. Returns the path only if the file exists.
fn resolve_md_image_path(url: &str, base: Option<&Path>) -> Option<PathBuf> {
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    if crate::preview::markdown::is_remote_image_url(u) {
        let p = md_remote_cache_path(u)?;
        return p.is_file().then_some(p);
    }
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("data:") {
        return None;
    }
    let u = u.strip_prefix("file://").unwrap_or(u);
    let p = PathBuf::from(u);
    let p = if p.is_absolute() {
        p
    } else if let Some(b) = base {
        b.join(p)
    } else {
        p
    };
    p.is_file().then_some(p)
}

/// Pixel dimensions of a cached inline-image file, accepting both raster formats and SVG (parsed cheaply
/// via usvg, without rasterizing). None if the file is neither a known raster image nor an SVG.
fn md_image_dims(path: &Path) -> Option<(u32, u32)> {
    crate::preview::image::dimensions(path).or_else(|| crate::preview::svg::intrinsic_size(path))
}

/// Decode a cached inline-image file to an image, rasterizing SVG (at `svg_max_px`) when the raster
/// decoders reject it (GitHub READMEs are full of SVG badges/logos). None if it is not a decodable image.
fn md_decode_image(path: &Path, svg_max_px: u32) -> Option<image::DynamicImage> {
    crate::preview::image::decode_static(path)
        .or_else(|| crate::preview::svg::rasterize(path, svg_max_px))
}

/// The source-pixel band `(y0, height)` of an image `dh` pixels tall that corresponds to the visible
/// cell rows `[row_off, row_off + vis_rows)` out of `full_rows` total. The result is always within
/// `[0, dh]` (so `crop_imm(0, y0, _, height)` never exceeds the image and never panics), and the height
/// is at least 1.
fn md_band_pixels(full_rows: u16, row_off: u16, vis_rows: u16, dh: u32) -> (u32, u32) {
    let fr = full_rows.max(1) as u32;
    let y0 = (row_off as u32 * dh) / fr;
    let y1 = ((row_off as u32 + vis_rows as u32).min(fr) * dh) / fr;
    (y0, y1.saturating_sub(y0).max(1))
}

/// Root of konoma's on-disk cache (`$XDG_CACHE_HOME` or `~/.cache`). None if neither is available.
fn cache_root() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".cache"))
}

/// Deterministic cache path for a remote image URL: `<cache>/konoma/remote-images/<hash>`. The file is
/// stored without an extension (the content type is unknown until fetched) and read via content sniffing.
fn md_remote_cache_path(url: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let root = cache_root()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.trim().hash(&mut hasher);
    Some(
        root.join("konoma")
            .join("remote-images")
            .join(format!("{:016x}", hasher.finish())),
    )
}

/// Maximum bytes to download for one remote image (guards against huge / hostile responses).
const MD_REMOTE_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Download a remote image with `curl` into `dest` (atomically, via a temp file), validating that the
/// bytes decode as an image before committing. Returns whether a valid image now exists at `dest`.
/// Runs on a background thread. Uses the system `curl` (always present on macOS) to avoid adding a
/// TLS/HTTP dependency — consistent with konoma's external-tool delegation model (PRD §5).
fn fetch_remote_image(url: &str, dest: &Path) -> bool {
    use std::process::{Command, Stdio};
    let Some(parent) = dest.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let tmp = dest.with_extension("part");
    let status = Command::new("curl")
        .args([
            "-sSL", // silent, show errors, follow redirects (GitHub proxies images via camo)
            "--fail",
            "--max-time",
            "20",
            "--max-filesize",
            &MD_REMOTE_MAX_BYTES.to_string(),
            "-A",
            "konoma image preview",
            "-o",
        ])
        .arg(&tmp)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let downloaded = matches!(status, Ok(s) if s.success());
    if !downloaded {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    // Reject non-images (e.g. an HTML error page served with 200) before caching them (accepts SVG).
    if md_image_dims(&tmp).is_none() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    std::fs::rename(&tmp, dest).is_ok()
}

/// Compute the display size in terminal cells for an image of `pw`x`ph` pixels, given the terminal
/// font cell size (`fw`x`fh` px). The image is fit to `avail_cols` (never upscaled beyond its natural
/// cell size) and then capped to `max_rows`, preserving aspect ratio in both steps.
fn md_image_cells(
    pw: u32,
    ph: u32,
    fw: u16,
    fh: u16,
    avail_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    let fw = (fw.max(1)) as f64;
    let fh = (fh.max(1)) as f64;
    let nat_cols = (pw as f64 / fw).ceil().max(1.0);
    let nat_rows = (ph as f64 / fh).ceil().max(1.0);
    let avail = avail_cols.max(1) as f64;
    let (mut cols, mut rows) = if nat_cols <= avail {
        (nat_cols, nat_rows)
    } else {
        let s = avail / nat_cols;
        (avail, (nat_rows * s).round().max(1.0))
    };
    let maxr = max_rows.max(1) as f64;
    if rows > maxr {
        let s = maxr / rows;
        rows = maxr;
        cols = (cols * s).round().max(1.0);
    }
    (cols as u16, rows as u16)
}

/// Compute the display layout for an image/GIF frame (a pure function shared by prepare_image / prepare_gif).
/// From (source image src, font_size, zoom, center[0,1], display area inner, render_scale), returns
/// `(target, crop_rect, center, frac)` (None if size 0).
///
/// - `target`  : the render-target rectangle (centered; z=1=fit, grows when zooming, clips when exceeding the viewport)
/// - `crop_rect`: the source-image window of the visible portion (px: x,y,w,h)
/// - `center`  : the center clamped to where the visible window fits within the image [0,1]
/// - `frac`    : the visible fraction per axis (<1=clipped=pan possible)
#[allow(clippy::type_complexity)]
fn image_layout(
    src: &image::DynamicImage,
    font_size: ratatui_image::FontSize,
    zoom: f64,
    center: (f64, f64),
    inner: Rect,
    render_scale: f64,
) -> Option<(Rect, (u32, u32, u32, u32), (f64, f64), (f64, f64))> {
    use image::GenericImageView;
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 || inner.width == 0 || inner.height == 0 {
        return None;
    }
    // z=1 のフィット表示サイズ(セル, 拡大しない)。
    let natural = Resize::natural_size(src, font_size);
    let base = centered_rect((natural.width, natural.height), inner, false);
    // ズーム後の論理表示サイズ。
    let disp_w = (base.width as f64 * zoom).max(1.0);
    let disp_h = (base.height as f64 * zoom).max(1.0);
    // 画面に出る大きさ = min(表示サイズ, 表示領域)。
    let tw = (disp_w.min(inner.width as f64).round() as u16).max(1);
    let th = (disp_h.min(inner.height as f64).round() as u16).max(1);
    // 各軸の可視率(<1 なら見切れ＝パン可能)。
    let fw = (tw as f64 / disp_w).min(1.0);
    let fh = (th as f64 / disp_h).min(1.0);
    // 中心を可視窓が画像内に収まる範囲へクランプ。
    let cx = center.0.clamp(fw / 2.0, 1.0 - fw / 2.0);
    let cy = center.1.clamp(fh / 2.0, 1.0 - fh / 2.0);
    // 元画像から見える窓を切り出す(px)。
    let cw = ((sw as f64 * fw).round() as u32).clamp(1, sw);
    let ch = ((sh as f64 * fh).round() as u32).clamp(1, sh);
    let x0 = ((cx * sw as f64) as i64 - cw as i64 / 2).clamp(0, (sw - cw) as i64) as u32;
    let y0 = ((cy * sh as f64) as i64 - ch as i64 / 2).clamp(0, (sh - ch) as i64) as u32;
    let crop_rect = (x0, y0, cw, ch);
    // 表示矩形(中央寄せ)。任意で render_scale により縮小(転送量=描画待ち削減)。
    let scale = render_scale.clamp(0.1, 1.0);
    let (tw, th) = if scale >= 0.999 {
        (tw, th)
    } else {
        (
            ((tw as f64 * scale).round() as u16).max(1),
            ((th as f64 * scale).round() as u16).max(1),
        )
    };
    let target = Rect {
        x: inner.x + (inner.width - tw.min(inner.width)) / 2,
        y: inner.y + (inner.height - th.min(inner.height)) / 2,
        width: tw.min(inner.width),
        height: th.min(inner.height),
    };
    Some((target, crop_rect, (cx, cy), (fw, fh)))
}

/// The modification time of `path`, or None if it cannot be read. Used by the FS-driven media auto-reload guard.
fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Assemble the copy string (a pure function; does not touch the clipboard).
/// `open_dir` is the base for relative paths (the launch directory).
fn copy_text(path: &Path, open_dir: &Path, kind: CopyKind) -> String {
    match kind {
        CopyKind::Name => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        CopyKind::Full => path.display().to_string(),
        CopyKind::Parent => path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        // 左上のタイトル表示 (format_path の Relative) と完全に同じ基準にする。
        CopyKind::Relative => rel_to_open(open_dir, path),
        CopyKind::AtRef => at_ref_text(open_dir, path),
    }
}

/// `@path` reference for AI agents (Claude Code's `@file` context syntax). Under `open_dir` the path is
/// strictly relative to it (`src/app.rs` — no leading dir name, unlike `rel_to_open`); outside it is
/// `..`-relative; if neither works, absolute. Prefixed with `@`.
fn at_ref_text(open_dir: &Path, path: &Path) -> String {
    let rel = match path.strip_prefix(open_dir) {
        Ok(r) if !r.as_os_str().is_empty() => r.display().to_string(),
        _ => match rel_from(open_dir, path) {
            Some(r) if !r.as_os_str().is_empty() => r.display().to_string(),
            _ => path.display().to_string(),
        },
    };
    format!("@{rel}")
}

/// A relative display string based on `open_dir` (the launch directory). For paths underneath, it puts the launch directory name first (e.g. `A/sub/x`),
/// and for things outside open_dir (siblings/ancestors) it relativizes with `..` (e.g. `../B/aaa.md`, `..`). If it cannot be relativized, `~/...`.
/// A shared function to guarantee the **same base** for the top-left path display (`format_path`'s Relative) and path copy (`cr`/`yr`).
fn rel_to_open(open_dir: &Path, path: &Path) -> String {
    if path.starts_with(open_dir) {
        // open_dir 配下: 起動ディレクトリ名を先頭に出すため、その親を基準に相対化 (例: A/sub/x)。
        let base = open_dir.parent().unwrap_or(open_dir);
        match path.strip_prefix(base) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
            _ => home_relative(path),
        }
    } else {
        // open_dir の外(兄弟/上位など): open_dir 基準の相対パスを `..` 込みで表示 (例: ../B/aaa.md)。
        // これが無いと兄弟 B のファイルが `B/aaa.md` のように見え、配下にあるかのように誤解される。
        match rel_from(open_dir, path) {
            Some(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
            _ => home_relative(path),
        }
    }
}

/// Write to the clipboard. Returns Err on environments where arboard is unavailable (the caller shows a flash).
fn set_clipboard(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_text(text.to_string())?;
    Ok(())
}

/// `~/...` if under HOME, otherwise the full path.
fn home_relative(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if let Ok(rel) = path.strip_prefix(&home) {
            if rel.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Resolve a dropped (=pasted) string into a list of **existing paths**. Terminals (Ghostty, etc.) pass a drop
/// as shell-escaped text (spaces as `\ `; multiple files separated by unescaped spaces).
/// Undo the backslash escapes, split on unescaped whitespace/newlines, and return only the tokens that **exist on disk**
/// (= safely excludes plain text pastes, junk, and control-character mixes).
fn parse_dropped_paths(text: &str) -> Vec<PathBuf> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in text.chars() {
        if escaped {
            cur.push(c); // 直前が `\` ＝ この文字はリテラル(空白入りパス名等)
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists()) // 実在パスのみ採用(テキストペースト/不正入力を弾く安全網)
        .collect()
}

/// Build a plan for sequential rename. Number `targets` **in display order** as `{n}`=1,2,…,
/// expand the template, and create each (old, new) path. If the template has no extension, the original extension is kept automatically.
/// **Pre-validates** collisions: an empty name / a `/` mixed in / duplicate final names / a collision with an existing file (not vacated within the batch) is an Err.
fn build_rename_plan(targets: &[PathBuf], template: &str) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(targets.len());
    for (idx, src) in targets.iter().enumerate() {
        let n = idx + 1;
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("");
        let rendered = crate::fileops::render_rename_template(template, n, stem, ext);
        let rendered = rendered.trim().to_string();
        if rendered.is_empty() {
            anyhow::bail!("空の名前になります (n={n})");
        }
        if rendered.contains('/') {
            anyhow::bail!("名前に / は使えません: {rendered}");
        }
        // 拡張子: テンプレ結果に拡張子が無ければ元拡張子を自動付与(ユーザ決定)。
        let has_ext = Path::new(&rendered)
            .extension()
            .filter(|e| !e.is_empty())
            .is_some();
        let new_name = if !has_ext && !ext.is_empty() {
            format!("{rendered}.{ext}")
        } else {
            rendered
        };
        let dst = src
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&new_name);
        plan.push((src.clone(), dst));
    }
    // 最終名の重複検証。
    let mut seen = BTreeSet::new();
    for (_, dst) in &plan {
        if !seen.insert(dst.clone()) {
            anyhow::bail!("リネーム先が重複: {}", dst.display());
        }
    }
    // 既存ファイルとの衝突検証(バッチ内で「移動して空く」元パスは除外)。
    let vacated: BTreeSet<PathBuf> = plan
        .iter()
        .filter(|(s, d)| s != d)
        .map(|(s, _)| s.clone())
        .collect();
    for (src, dst) in &plan {
        // 名前据え置き(src == dst)は自分自身の存在を衝突と誤検出しないよう除外する(#11)。
        // 真の衝突=別ファイルと被る場合のみ、最終名重複検証(上)で弾く。
        if src == dst {
            continue;
        }
        if dst.exists() && !vacated.contains(dst) {
            anyhow::bail!("既に存在します: {}", dst.display());
        }
    }
    Ok(plan)
}

/// Return the byte position in `s` corresponding to character index `char_idx` (past the end gives `s.len()`).
/// A conversion to safely compute insertion/deletion positions even for multibyte text (e.g. Japanese file names).
fn char_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Compute the relative path from `base` to `target`, including `..` (equivalent to pathdiff). It removes the common leading components and
/// pushes one `..` for each remaining level on the base side. None if the root kinds (absolute/drive) disagree and it cannot be relativized.
fn rel_from(base: &Path, target: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let bc: Vec<Component> = base.components().collect();
    let tc: Vec<Component> = target.components().collect();
    let mut i = 0;
    while i < bc.len() && i < tc.len() && bc[i] == tc[i] {
        i += 1;
    }
    // 共通成分が無く、かつ一方がルート(絶対/Prefix)で始まる=相対化不能。
    if i == 0 {
        let is_root = |c: Option<&Component>| {
            matches!(c, Some(Component::Prefix(_)) | Some(Component::RootDir))
        };
        if is_root(bc.first()) || is_root(tc.first()) {
            return None;
        }
    }
    let mut rel = PathBuf::new();
    for _ in i..bc.len() {
        rel.push("..");
    }
    for c in &tc[i..] {
        rel.push(c.as_os_str());
    }
    Some(rel)
}

/// Append one directory's worth of entries to out. Recurse into expanded subdirectories.
/// Bundles the decision info for one entry for sorting (stats the metadata only once).
struct ChildMeta {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    mtime: Option<std::time::SystemTime>,
}

fn child_meta(path: PathBuf) -> ChildMeta {
    let md = std::fs::metadata(&path).ok();
    let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = md.as_ref().and_then(|m| m.modified().ok());
    ChildMeta {
        path,
        is_dir,
        size,
        mtime,
    }
}

/// Lowercase comparison of file names (a case-insensitive ordering).
fn name_cmp(a: &Path, b: &Path) -> std::cmp::Ordering {
    let an = a
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bn = b
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    an.cmp(&bn)
}

/// The lowercase extension (empty if none).
fn ext_lower(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Compare two entries according to the settings. dirs_first → key → reverse, finally stabilized by name.
fn sort_cmp(a: &ChildMeta, b: &ChildMeta, sort: Sort) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if sort.dirs_first {
        let d = b.is_dir.cmp(&a.is_dir); // ディレクトリ(true)を先頭に
        if d != Ordering::Equal {
            return d;
        }
    }
    let mut ord = match sort.key {
        SortKey::Name => name_cmp(&a.path, &b.path),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.mtime.cmp(&b.mtime),
        SortKey::Ext => ext_lower(&a.path).cmp(&ext_lower(&b.path)),
    };
    if sort.reverse {
        ord = ord.reverse();
    }
    // 同値は名前(昇順)で安定化して並びがブレないようにする。
    ord.then_with(|| name_cmp(&a.path, &b.path))
}

/// Take the title label for the graph base from refs (`%D`). From `(HEAD -> main, origin/main)`, etc.,
/// pick one branch name (strip the HEAD-> and skip tag:). If there is no branch ref, the short hash.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
fn base_label_from(refs: &str, short: &str) -> String {
    for part in refs.split(',').map(str::trim) {
        let p = part.strip_prefix("HEAD -> ").unwrap_or(part);
        if p.is_empty() || p.starts_with("tag:") {
            continue;
        }
        return p.to_string();
    }
    short.to_string()
}

fn build_dir(
    dir: &Path,
    depth: usize,
    expanded_dirs: &[PathBuf],
    show_hidden: bool,
    sort: Sort,
    out: &mut Vec<Entry>,
) -> Result<()> {
    let mut children: Vec<ChildMeta> = std::fs::read_dir(dir)?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| {
            if show_hidden {
                return true;
            }
            !p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false)
        })
        .map(child_meta)
        .collect();

    children.sort_by(|a, b| sort_cmp(a, b, sort));

    for c in children {
        let expanded = c.is_dir && expanded_dirs.iter().any(|d| d == &c.path);
        out.push(Entry {
            path: c.path.clone(),
            is_dir: c.is_dir,
            depth,
            expanded,
        });
        if expanded {
            build_dir(&c.path, depth + 1, expanded_dirs, show_hidden, sort, out)?;
        }
    }
    Ok(())
}

/// Recursively collect files/directories under `root` (the population for filtering). Hidden ones are excluded via `show_hidden`,
/// and symlink directories are not descended into (to prevent loops). The scan count is capped as a cost limit.
/// The return value is in ascending path order (grouped per directory). Each Entry is flat with depth=0 and expanded=false.
fn collect_all(root: &Path, show_hidden: bool) -> Vec<Entry> {
    const CAP: usize = 50_000;
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= CAP {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in rd.filter_map(|r| r.ok()).map(|e| e.path()) {
            if out.len() >= CAP {
                break;
            }
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false);
            if hidden && !show_hidden {
                continue;
            }
            // シンボリックリンクの dir には潜らない(循環回避)。is_dir はリンク先を辿るので別途判定。
            let is_symlink = std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            let is_dir = path.is_dir();
            out.push(Entry {
                path: path.clone(),
                is_dir,
                depth: 0,
                expanded: false,
            });
            if is_dir && !is_symlink {
                stack.push(path);
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests;
