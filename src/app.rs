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
mod copy_actions;
mod dialog_actions;
mod file_actions;
mod follow;
mod git_view;
mod md_items;
mod md_media;
mod md_render;
mod md_tasks;
mod md_text;
mod media_load;
mod outline;
mod paste_jump;
mod preview_visual;
mod search;
mod session_actions;
mod tab_lifecycle;
mod table_actions;

use md_text::*;
// Only referenced by tests (`crate::app::PreviewSelection`) — `preview_visual` itself uses the
// type unqualified since it's defined there, so this re-export would be unused_imports outside cfg(test).
#[cfg(test)]
pub use preview_visual::PreviewSelection;
use preview_visual::*;
// Referenced by `ui/table.rs` (the full-cell popup renderer) as `crate::app::TableCellView`.
pub use table_actions::TableCellView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
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
    /// The `?` help overlay. It renders as a centered popup that does not cover the top/bottom
    /// chrome, so the chip/footer must reflect the overlay's own keys (j/k/g/G scroll, q/Esc close)
    /// rather than whatever surface sits behind it (Tree/Preview keep rendering underneath).
    Help,
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
    Tabs,
    Outline,
    Info,
    /// Full-cell popup (`Enter` in a table preview): the cursor cell's untruncated text.
    TableCell,
    Create,
    Rename,
    BatchRename,
    RenamePreview,
    DeleteConfirm,
    DropConfirm,
    QuitConfirm,
    BookmarkConfirm,
    GitChanges,
    GitDiff,
    Commit,
    GitLog,
    GitDetail,
    GitBranch,
    /// The linked-worktree list (`w` from the changes hub).
    GitWorktrees,
    GitGraph,
    GitGraphPicker,
}

/// What the **bottom** of the UI stack is showing, once nothing is overlaying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BaseKind {
    Tree,
    /// Any preview that is neither a renderable image nor a parsed table (text, code, Markdown,
    /// a git diff, `[can not preview]`, …). They all answer to the same keymap surface.
    PreviewText,
    PreviewImage,
    PreviewTable,
}

/// The bottom layer of the UI stack, plus the two facts that decorate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BaseLayer {
    pub kind: BaseKind,
    /// A visual selection is active in a **windowed** preview (`v`/`V`). Deliberately kept
    /// independent of `kind` rather than folded into it: the chip reports the selection whatever
    /// the preview is, while the keymap only turns it into `PreviewTextVisual` for a text preview
    /// (an image/table resolves to its own surface first). Both readings are pinned by
    /// `ui_stack_priority_is_characterized`.
    pub preview_visual: bool,
    /// The changed-files-only tree filter (`C`) is on.
    pub changed_filter: bool,
}

/// One layer of the UI stack.
///
/// **The declaration order below is the priority order** — front (topmost, the layer that receives
/// the keys) to back — and it is written down exactly once. [`App::frontmost_layer`] walks it in
/// this sequence, and both [`App::surface`] and [`App::internal_mode`] are pure projections of the
/// layer it returns ([`UiLayer::surface`] / [`UiLayer::internal_mode`]). Before this existed the two
/// were separate hand-written cascades that had to be kept in step by hand, and they drifted: the
/// `?` help overlay reached `surface()` but not `internal_mode()`, so the chip and footer
/// advertised keys that did nothing.
// `Ord` follows the declaration order, which lets
// `the_walk_visits_the_layers_in_their_declaration_order` check mechanically that the walk really
// does go front to back in the order written below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UiLayer {
    // --- Dialogs (frontmost; the kinds below are mutually exclusive) ---
    /// Confirm dialog: quit the app (non-destructive, so it gets its own chip/footer).
    ConfirmQuit,
    /// Confirm dialog: overwrite an existing bookmark (also non-destructive).
    ConfirmBookmark,
    /// Confirm dialog: a drag-and-drop transfer (`c`=copy / `m`=move).
    ConfirmDrop,
    /// Confirm dialog: everything else (delete, discard, delete-branch, …).
    ConfirmDelete,
    /// The batch-rename "old → new" preview dialog.
    RenamePreview,
    /// Input dialog, by pending operation. They all share one keymap surface (`DialogInput`) but
    /// each gets its own chip/footer wording.
    InputCreate,
    InputBatchRename,
    InputCommit,
    InputCreateBranch,
    InputCreateWorktree,
    /// Input dialog: rename, and the catch-all for any other pending operation.
    InputRename,

    // --- The help overlay (above everything but the dialogs) ---
    Help,

    // --- The git views (detail sits over log; the branch panel sits over the graph) ---
    GitDetail,
    GitLog,
    GitGraphPicker,
    GitGraph,
    /// The branch list with its `/` filter input active.
    GitBranchFilter,
    GitBranches,
    /// The worktree list with its `/` filter input active.
    GitWorktreeFilter,
    GitWorktrees,
    GitChanges,
    GitDiff,

    // --- Input surfaces / menus / lists / popups ---
    Filter,
    Search,
    Sort,
    Mark,
    Bookmarks,
    Tabs,
    Outline,
    Info,
    TableCell,
    Visual,

    /// The base full screen. Always active, so the walk ends here.
    Base(BaseLayer),
}

impl BaseLayer {
    fn surface(self) -> crate::keymap::Surface {
        use crate::keymap::Surface as S;
        match self.kind {
            BaseKind::Tree => S::Tree,
            BaseKind::PreviewImage => S::PreviewImage,
            BaseKind::PreviewTable => S::PreviewTable,
            BaseKind::PreviewText => {
                if self.preview_visual {
                    S::PreviewTextVisual
                } else {
                    S::PreviewText
                }
            }
        }
    }

    fn internal_mode(self) -> Option<InternalMode> {
        if self.preview_visual {
            return Some(InternalMode::PreviewVisual);
        }
        if self.changed_filter && matches!(self.kind, BaseKind::Tree) {
            return Some(InternalMode::ChangedFilter);
        }
        None
    }
}

impl UiLayer {
    /// The keymap surface this layer owns.
    ///
    /// `None` means the layer has no surface **on this build** — only the git layers, and only when
    /// the `git` feature is off, where `keymap::Surface` has no git variants at all. [`App::surface`]
    /// then keeps walking to the layer behind it, which is exactly what the old hand-written cascade
    /// did by wrapping its whole git block in `#[cfg(feature = "git")]`. (Such state is unreachable
    /// through the UI on a no-git build — every git opener bails out because the `git.rs` stubs
    /// return empty/None — but the fall-through is characterized so it cannot change by accident.)
    pub fn surface(self) -> Option<crate::keymap::Surface> {
        use crate::keymap::Surface as S;
        Some(match self {
            Self::ConfirmQuit => S::DialogConfirmQuit,
            Self::ConfirmBookmark => S::DialogConfirmBookmark,
            Self::ConfirmDrop => S::DialogConfirmDrop,
            Self::ConfirmDelete => S::DialogConfirmDelete,
            Self::RenamePreview => S::DialogRenamePreview,
            Self::InputCreate
            | Self::InputBatchRename
            | Self::InputCommit
            | Self::InputCreateBranch
            | Self::InputCreateWorktree
            | Self::InputRename => S::DialogInput,
            Self::Help => S::Help,
            #[cfg(feature = "git")]
            Self::GitDetail => S::GitDetail,
            #[cfg(feature = "git")]
            Self::GitLog => S::GitLog,
            #[cfg(feature = "git")]
            Self::GitGraphPicker => S::GitGraphPicker,
            #[cfg(feature = "git")]
            Self::GitGraph => S::GitGraph,
            #[cfg(feature = "git")]
            Self::GitBranchFilter => S::BranchFilter,
            #[cfg(feature = "git")]
            Self::GitBranches => S::GitBranches,
            #[cfg(feature = "git")]
            Self::GitWorktreeFilter => S::WorktreeFilter,
            #[cfg(feature = "git")]
            Self::GitWorktrees => S::GitWorktrees,
            #[cfg(feature = "git")]
            Self::GitChanges => S::GitChanges,
            #[cfg(feature = "git")]
            Self::GitDiff => S::PreviewGitDiff,
            #[cfg(not(feature = "git"))]
            Self::GitDetail
            | Self::GitLog
            | Self::GitGraphPicker
            | Self::GitGraph
            | Self::GitBranchFilter
            | Self::GitBranches
            | Self::GitWorktreeFilter
            | Self::GitWorktrees
            | Self::GitChanges
            | Self::GitDiff => return None,
            Self::Filter => S::Filter,
            Self::Search => S::Search,
            Self::Sort => S::Sort,
            Self::Mark => S::Mark,
            Self::Bookmarks => S::Bookmarks,
            Self::Tabs => S::Tabs,
            Self::Outline => S::Outline,
            Self::Info => S::Info,
            Self::TableCell => S::TableCell,
            Self::Visual => S::Visual,
            Self::Base(b) => b.surface(),
        })
    }

    /// The mode this layer shows in the inner chip / footer. `None` = nothing is being operated
    /// (the plain base screen). Unlike [`UiLayer::surface`] this is never gated on the `git`
    /// feature: `InternalMode` has its git variants on every build.
    pub fn internal_mode(self) -> Option<InternalMode> {
        Some(match self {
            Self::ConfirmQuit => InternalMode::QuitConfirm,
            Self::ConfirmBookmark => InternalMode::BookmarkConfirm,
            Self::ConfirmDrop => InternalMode::DropConfirm,
            Self::ConfirmDelete => InternalMode::DeleteConfirm,
            Self::RenamePreview => InternalMode::RenamePreview,
            Self::InputCreate => InternalMode::Create,
            Self::InputBatchRename => InternalMode::BatchRename,
            Self::InputCommit => InternalMode::Commit,
            // The branch/worktree creation prompts reuse their list's chip rather than inventing a
            // dedicated "creating a …" mode.
            Self::InputCreateBranch => InternalMode::GitBranch,
            Self::InputCreateWorktree => InternalMode::GitWorktrees,
            Self::InputRename => InternalMode::Rename,
            Self::Help => InternalMode::Help,
            Self::GitDetail => InternalMode::GitDetail,
            Self::GitLog => InternalMode::GitLog,
            Self::GitGraphPicker => InternalMode::GitGraphPicker,
            Self::GitGraph => InternalMode::GitGraph,
            // A list's `/` filter deliberately keeps the list's own chip, so there is no
            // `InternalMode::BranchFilter`/`WorktreeFilter` to map to. The three consumers each
            // want something different and that is on purpose: the keymap needs a separate surface
            // because typing has to be captured rather than dispatched (`Surface::is_text_input`),
            // the chip must *not* change because the list is still what you are looking at (cursor
            // and all), and the footer swaps to the `/query` prompt through its own branch in
            // `ui::status::footer_spans`, ahead of the mode footer. Pinned by
            // `a_list_filter_switches_the_keymap_and_the_footer_but_not_the_chip`.
            Self::GitBranchFilter | Self::GitBranches => InternalMode::GitBranch,
            Self::GitWorktreeFilter | Self::GitWorktrees => InternalMode::GitWorktrees,
            Self::GitChanges => InternalMode::GitChanges,
            Self::GitDiff => InternalMode::GitDiff,
            Self::Filter => InternalMode::Filter,
            Self::Search => InternalMode::Search,
            Self::Sort => InternalMode::Sort,
            Self::Mark => InternalMode::Mark,
            Self::Bookmarks => InternalMode::Bookmarks,
            Self::Tabs => InternalMode::Tabs,
            Self::Outline => InternalMode::Outline,
            Self::Info => InternalMode::Info,
            Self::TableCell => InternalMode::TableCell,
            Self::Visual => InternalMode::Visual,
            Self::Base(b) => return b.internal_mode(),
        })
    }
}

impl crate::keymap::Surface {
    /// Where this surface sits in the UI stack.
    ///
    /// **Exhaustive on purpose**: a new `Surface` variant does not compile until it is given a
    /// place in [`UiLayer`]'s priority order, so a new full-screen/overlay surface cannot be added
    /// without deciding what it sits in front of. `surface_round_trips_through_its_layer` then
    /// checks that the placement really does produce this surface. Only `mod tests` needs it (hence
    /// `#[cfg(test)]`), which means the guarantee holds on every `cargo test` run — the same
    /// arrangement as `keymap::surface_config_name`.
    #[cfg(test)]
    pub fn layer(self) -> UiLayer {
        use crate::keymap::Surface as S;
        // A base surface names the base layer that produces it; `preview_visual` is the one bit the
        // base needs to tell PreviewText from PreviewTextVisual.
        let base = |kind: BaseKind, preview_visual: bool| {
            UiLayer::Base(BaseLayer {
                kind,
                preview_visual,
                changed_filter: false,
            })
        };
        match self {
            S::DialogInput => UiLayer::InputRename,
            S::Filter => UiLayer::Filter,
            S::Search => UiLayer::Search,
            S::Mark => UiLayer::Mark,
            #[cfg(feature = "git")]
            S::BranchFilter => UiLayer::GitBranchFilter,
            #[cfg(feature = "git")]
            S::WorktreeFilter => UiLayer::GitWorktreeFilter,
            S::DialogConfirmDelete => UiLayer::ConfirmDelete,
            S::DialogConfirmDrop => UiLayer::ConfirmDrop,
            S::DialogRenamePreview => UiLayer::RenamePreview,
            S::DialogConfirmQuit => UiLayer::ConfirmQuit,
            S::DialogConfirmBookmark => UiLayer::ConfirmBookmark,
            S::Help => UiLayer::Help,
            S::Sort => UiLayer::Sort,
            S::Bookmarks => UiLayer::Bookmarks,
            S::Tabs => UiLayer::Tabs,
            S::Outline => UiLayer::Outline,
            S::Info => UiLayer::Info,
            S::TableCell => UiLayer::TableCell,
            #[cfg(feature = "git")]
            S::GitDetail => UiLayer::GitDetail,
            #[cfg(feature = "git")]
            S::GitLog => UiLayer::GitLog,
            #[cfg(feature = "git")]
            S::GitGraph => UiLayer::GitGraph,
            #[cfg(feature = "git")]
            S::GitGraphPicker => UiLayer::GitGraphPicker,
            #[cfg(feature = "git")]
            S::GitBranches => UiLayer::GitBranches,
            #[cfg(feature = "git")]
            S::GitChanges => UiLayer::GitChanges,
            #[cfg(feature = "git")]
            S::GitWorktrees => UiLayer::GitWorktrees,
            #[cfg(feature = "git")]
            S::PreviewGitDiff => UiLayer::GitDiff,
            S::Visual => UiLayer::Visual,
            S::Tree => base(BaseKind::Tree, false),
            S::PreviewText => base(BaseKind::PreviewText, false),
            S::PreviewTextVisual => base(BaseKind::PreviewText, true),
            S::PreviewImage => base(BaseKind::PreviewImage, false),
            S::PreviewTable => base(BaseKind::PreviewTable, false),
        }
    }
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
            _ => Self::Unified, // "unified" / "vertical" / unknown
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
// The git-only variants (GitDiscard/GitDeleteBranch) have no construction path in a non-git build
// (what constructs them is git-dispatch only). The variants themselves are kept for match
// exhaustiveness, and dead_code is allowed only for the non-git build.
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
    /// In the worktree list, `n`=enter a branch name for a new linked worktree. On confirm,
    /// `dialog_submit` auto-detects new-vs-existing (`git::branch_tip`) and runs `git::worktree_add`.
    WorktreeCreate,
    /// In the branch list, `d`=delete confirmation. `y`=safe (-d) / `!`=force (-D).
    GitDeleteBranch { name: String },
    /// A transfer received via file drag & drop (through a terminal paste). `c`=copy / `m`=move / Esc=cancel.
    /// `sources`=the dropped existing paths, `dir`=the drop destination (directory based on the cursor).
    DropTransfer { sources: Vec<PathBuf>, dir: PathBuf },
    /// Quit the whole app. A confirmation is requested before exiting (see `ui.confirm_quit`).
    Quit,
    /// Overwrite an existing bookmark under `key` with `target` (confirmation via `ui.confirm_bookmark_overwrite`).
    BookmarkOverwrite { key: char, target: PathBuf },
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

/// Size of `App` in bytes — a memory guard reads this to catch a struct that suddenly balloons.
#[cfg(test)]
pub fn sizeof_app() -> usize {
    std::mem::size_of::<App>()
}

/// Size of one `PerTab` in bytes. `PerTab` is cloned per tab (snapshot / restore), so a regression
/// that inlines a large buffer into it multiplies memory by the tab count — this guards against that.
#[cfg(test)]
pub fn sizeof_pertab() -> usize {
    std::mem::size_of::<PerTab>()
}

/// The media payload loaded on a separate thread (sent back to the UI thread).
pub enum MediaPayload {
    /// A still image (single-frame GIF / video thumb / PDF page) → goes to image_src.
    Static(image::DynamicImage),
    /// All GIF frames (composited RGBA + delay) → goes to gif_frames.
    Gif(Vec<(image::DynamicImage, std::time::Duration)>),
    /// A raster of a vector source (SVG file / mermaid diagram). The SVG is retained so zooming
    /// re-rasterizes at the needed density (sharp zoom) instead of blowing up pixels.
    Vector {
        img: image::DynamicImage,
        svg: std::sync::Arc<Vec<u8>>,
    },
    /// Text-mode `PreviewKind::Command` output: the temp file its captured stdout / `{out}`
    /// artifact was written to. Goes to `tab.command_out`, which the windowed (less-style) text
    /// reader reads from instead of the previewed path — see `App::windowed_src`.
    CommandText(PathBuf),
    /// A `PreviewKind::Command` job failed (missing binary, non-zero exit, `{out}` never produced,
    /// or a produced image that couldn't be decoded). Carried as a payload — rather than the plain
    /// `None` every other job's failure uses — so the reason travels through the same
    /// generation-checked channel as success and can't be raced by a newer attempt's result;
    /// `apply_payload` turns it into `App::command_err` for the render side's `[can not preview]`
    /// fallback (`ui/preview.rs`).
    CommandFailed(String),
}

/// Result of media loading from another thread. Matched by generation via `gen`; results made stale by navigation are discarded.
pub struct MediaResult {
    gen: u64,
    /// None = decode/rasterize failure (the render side shows a fallback).
    payload: Option<MediaPayload>,
}

/// The geometry a kitty image targets: `(crop rect (x,y,w,h) in source px, display cols, rows)`.
type KittyGeom = ((u32, u32, u32, u32), u16, u16);

/// Result of an async kitty image build (resize + `o=z` compress) from a worker thread. Matched by
/// `gen` (latest-wins): a build superseded by a newer zoom/pan is discarded. None = build failed.
pub struct KittyResult {
    gen: u64,
    image: Option<crate::preview::kitty::KittyImage>,
}

/// Heavy media loading to run on a separate thread (pure work that does not reference App).
enum MediaJob {
    /// Rasterize an SVG (path, max-edge px). Vector-backed: keeps the source for sharp zoom.
    Svg(PathBuf, u32),
    /// Expand all GIF frames. Falls back to still-image decode for single-frame/non-animated GIFs.
    Gif(PathBuf),
    /// Extract one representative frame from a video (delegated to ffmpegthumbnailer/ffmpeg). Thumbnail only; no playback.
    Video(PathBuf),
    /// Rasterize page N (1-based) of a PDF — `hayro` (pure Rust) first, external tools
    /// (pdftocairo/pdftoppm/qlmanage/sips) only as a fallback and only when the `bool` (`[external]
    /// pdf`) allows it. Loaded one page at a time.
    Pdf(PathBuf, u32, bool),
    /// Render a standalone mermaid file to SVG (pure Rust) and rasterize it (max-edge px, theme).
    /// None (unsupported diagram / parse failure) → the caller's text-diagram fallback shows.
    Mermaid(PathBuf, u32, String),
    /// Render mermaid source text (a ```mermaid fence opened full-screen) and rasterize it.
    MermaidSrc(String, u32, String),
    /// Re-rasterize the retained SVG source at a new max-edge px (sharp zoom). The path is only
    /// the base for relative resources inside the SVG (mermaid output has none).
    SvgReraster(std::sync::Arc<Vec<u8>>, PathBuf, u32),
    /// Run a non-detached `PreviewKind::Command` delegation (`preview::command::run_capture`).
    /// `as_image` (the resolved `render_as == Some("image")`) decides whether the produced artifact
    /// is decoded as an image (`MediaPayload::Static`) or shown as text (`MediaPayload::CommandText`).
    Command {
        argv: Vec<String>,
        out: PathBuf,
        /// Whether the template contains `{out}` (the command is expected to write it itself) —
        /// otherwise captured stdout is written to `out` instead. See `command::run_capture`.
        uses_out: bool,
        as_image: bool,
    },
}

impl MediaJob {
    /// Load the actual data (called on a separate thread or via the synchronous fallback). None on failure.
    fn run(self) -> Option<MediaPayload> {
        match self {
            MediaJob::Svg(p, max_px) => {
                let data = std::fs::read(&p).ok()?;
                let img = crate::preview::svg::rasterize_bytes(&data, &p, max_px)?;
                Some(MediaPayload::Vector {
                    img,
                    svg: std::sync::Arc::new(data),
                })
            }
            MediaJob::Gif(p) => match crate::preview::image::decode_gif(&p) {
                Some(frames) => Some(MediaPayload::Gif(frames)),
                // A single-frame / non-animated GIF → display as a still image.
                None => crate::preview::image::decode_static(&p).map(MediaPayload::Static),
            },
            MediaJob::Video(p) => crate::preview::video::thumbnail(&p).map(MediaPayload::Static),
            MediaJob::Pdf(p, page, allow_external) => {
                crate::preview::pdf::render_page(&p, page, allow_external).map(MediaPayload::Static)
            }
            MediaJob::Mermaid(p, max_px, theme) => {
                let code = std::fs::read_to_string(&p).ok()?;
                MediaJob::MermaidSrc(code, max_px, theme).run()
            }
            MediaJob::MermaidSrc(code, max_px, theme) => {
                let svg = crate::preview::markdown::mermaid_to_svg(&code, &theme)?;
                let data = svg.into_bytes();
                let img =
                    crate::preview::svg::rasterize_bytes(&data, Path::new("mermaid.svg"), max_px)?;
                Some(MediaPayload::Vector {
                    img,
                    svg: std::sync::Arc::new(data),
                })
            }
            MediaJob::SvgReraster(svg, p, max_px) => {
                let img = crate::preview::svg::rasterize_bytes(&svg, &p, max_px)?;
                Some(MediaPayload::Vector { img, svg })
            }
            MediaJob::Command {
                argv,
                out,
                uses_out,
                as_image,
            } => match crate::preview::command::run_capture(&argv, &out, uses_out) {
                Ok(result_path) => {
                    if as_image {
                        let img = image::ImageReader::open(&result_path)
                            .ok()
                            .and_then(|r| r.with_guessed_format().ok())
                            .and_then(|r| r.decode().ok());
                        // Consumed immediately, like `preview::video::thumbnail`'s temp PNG — the
                        // image path only needs the decoded pixels from here on.
                        let _ = std::fs::remove_file(&result_path);
                        match img {
                            Some(im) => Some(MediaPayload::Static(im)),
                            None => Some(MediaPayload::CommandFailed(format!(
                                "could not read image output: {}",
                                result_path.display()
                            ))),
                        }
                    } else {
                        Some(MediaPayload::CommandText(result_path))
                    }
                }
                Err(e) => Some(MediaPayload::CommandFailed(e.to_string())),
            },
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

/// Result of a **filter-pool scan** (`/`'s recursive population walk) finished on a separate thread.
///
/// A whole-tree walk cannot fit in the <60ms draw budget once the tree is big — 50,000 entries cost
/// ~45ms in `readdir` alone — so the scan starts synchronously with a time budget and, only if that
/// runs out, hands the unfinished part here (see `collect_scan`). Staleness is judged by `gen` just
/// like [`IgnoredResult`]/[`StatusResult`]; `root` is re-checked on apply so a walk that finishes
/// after the user moved elsewhere is discarded rather than dropped into another directory's pool.
pub struct FilterPoolResult {
    gen: u64,
    /// The root this scan was collecting for.
    root: PathBuf,
    /// Already sorted by path, so the caller's `sort_by` over pool+this is a run merge.
    ///
    /// `None` = the walk panicked. It is deliberately **not** flattened to an empty Vec: an empty
    /// result is a legitimate answer ("the directory really is empty"), and for a `append == false`
    /// refresh that would *replace* a good pool with nothing — turning a crashed worker into a
    /// filter that silently matches no files. `None` instead leaves the pool alone and only
    /// releases the in-flight bookkeeping.
    entries: Option<Vec<Entry>>,
    /// `true` = the tail of a walk whose head is already in the pool (the budgeted hand-off from
    /// `/`), so it is appended. `false` = a complete re-walk that replaces the pool wholesale (the
    /// FS-event refresh, which keeps showing the previous pool until this lands).
    append: bool,
}

/// One decoded media payload kept across a tab switch (see `App::media_cache`).
struct MediaCache {
    path: PathBuf,
    mtime: Option<std::time::SystemTime>,
    /// File size alongside the mtime. Archive extraction and `cp -p` / `rsync -a` **preserve the mtime**,
    /// so mtime alone would keep serving the old picture; the size catches the common case of those.
    len: Option<u64>,
    /// PDF page the image belongs to (1 for every other kind), so paging is not confused with a hit.
    page: u32,
    /// Which mermaid fence inside the document this render belongs to. A full-screen fence keeps
    /// `preview_path` pointing at the **Markdown file**, so without this two fences of the same document
    /// look identical to the cache and switching tabs would show the other diagram.
    fence_ord: Option<usize>,
    src: std::sync::Arc<image::DynamicImage>,
    logical: Option<(u32, u32)>,
    vector_svg: Option<std::sync::Arc<Vec<u8>>>,
}

/// Result of `git status` + branch computed on a separate thread. Staleness is judged by `gen` exactly like
/// [`IgnoredResult`], so a scan that finishes after the user has already moved on is discarded.
///
/// `git status` is a **whole-worktree scan**: even on a tiny repository it costs ~5ms (process spawn +
/// repo discovery), and it grows with the working tree. Running it on the UI thread made every tab switch
/// pay that cost synchronously, which is why it is offloaded here (design principle #4 "never block the UI").
pub struct StatusResult {
    gen: u64,
    workdir: Option<PathBuf>,
    statuses: std::collections::HashMap<PathBuf, crate::git::FileStatus>,
    branch: Option<String>,
    /// The origin repo name when `root` is inside a **linked worktree**, else `None`. Same
    /// lifecycle as `branch` (computed alongside it in `scan_statuses`, applied together in
    /// `apply_statuses`) — see `App::worktree_origin`'s doc comment for why it rides along here
    /// instead of having its own refresh path.
    worktree_origin: Option<String>,
}

/// Which long-running filesystem operation a background job is performing.
/// The variants differ in the completion message and in whether a failing target aborts the
/// rest: paste/duplicate continue past a failing target (existing behaviour), while a
/// drag-and-drop transfer stops at the first error (existing `drop_apply` behaviour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpKind {
    PasteCopy,
    PasteMove,
    DropCopy,
    DropMove,
    Duplicate,
    Trash,
    DeletePermanent,
}

/// A queued filesystem operation handed to the background worker (or run synchronously when no
/// worker is attached — see `App::start_file_op`). `dest` is the destination directory for
/// `Paste*`/`Drop*`; `None` for `Duplicate`/`Trash`/`DeletePermanent` (each target uses its own
/// parent, or is deleted in place). The two error strings are pre-translated by the caller
/// (`self.lang`), since the worker thread has no `&self` to translate with.
pub struct FileOpJob {
    kind: FileOpKind,
    targets: Vec<PathBuf>,
    dest: Option<PathBuf>,
    /// The tree root of the tab this operation was dispatched from. **Callers leave this empty**
    /// (`PathBuf::new()`, like the unused `err_self_paste`): `start_file_op` overwrites it from
    /// `self.tab.root` so there is a single choke point and no call site can forget it. It travels
    /// through to `FileOpResult` so the completion can tell whether it is landing on the tab it
    /// started on (see `apply_file_op`).
    root: PathBuf,
    err_self_paste: String,
    err_failed: String,
}

/// Outcome of a background filesystem operation, applied on the run loop's thread by `App::apply_file_op`.
pub struct FileOpResult {
    gen: u64,
    kind: FileOpKind,
    /// The tree root the operation was dispatched from (copied from `FileOpJob`). `apply_file_op`
    /// compares it with the now-active tab's root before touching the tree cursor.
    root: PathBuf,
    ok: usize,
    last: Option<PathBuf>,
    err: Option<String>,
}

/// Which **git write** a background job is performing (design principle #4 — see
/// `App::start_git_op`). Every variant shells out to `git`, whose runtime is unbounded in practice:
/// a `pre-commit` hook can lint a huge repo for a minute, a checkout over a network mount can stall,
/// and any of them can wait on another process's `index.lock`. Doing that on the UI thread froze
/// konoma outright, while the *read* side (`git status` / `ignored`) had already been moved off it.
///
/// Deliberately **not** solved with a timeout+kill: killing `git commit` mid-flight leaves
/// `.git/index.lock` behind and the repository unusable until the user removes it by hand, which is
/// worse than the freeze — and a slow-but-legitimate hook is not an error to abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOpKind {
    Stage,
    Unstage,
    StageAll,
    UnstageAll,
    Discard,
    Commit,
    Checkout,
    CreateBranch,
    DeleteBranch,
    WorktreeAdd,
}

/// A queued git write handed to the background worker (or run synchronously when no worker is
/// attached — see `App::start_git_op`). The operands are flattened into one struct rather than
/// carried per-variant so the worker stays a single `match`; each field documents which kinds use it.
pub struct GitOpJob {
    kind: GitOpKind,
    /// The tree root the write is dispatched against. **Callers leave this empty** (`PathBuf::new()`,
    /// like `FileOpJob::root`): `start_git_op` overwrites it from `self.tab.root` so there is a
    /// single choke point and no call site can forget it. It travels through to `GitOpResult` so the
    /// completion can tell whether it is landing on the repository it changed (see `apply_git_op`).
    root: PathBuf,
    /// Identity of the tab the write was dispatched from (`PerTab::id`), filled in by `start_git_op`
    /// alongside `root`. `root` alone cannot answer "did *this* tab ask for it": `t` opens the new
    /// tab at the current root, so two tabs sharing a root is the ordinary case.
    tab_id: u64,
    /// Path operand: the file for `Stage`/`Unstage`/`Discard`, the new directory for `WorktreeAdd`.
    /// Empty for the kinds that take no path.
    path: PathBuf,
    /// Text operand: the message for `Commit`, the branch name for `Checkout`/`CreateBranch`/
    /// `DeleteBranch`/`WorktreeAdd`. Empty for the kinds that take no text.
    text: String,
    /// `DeleteBranch`: force (`-D` instead of `-d`). `WorktreeAdd`: also create the branch (`-b`).
    flag: bool,
    /// Pre-formatted display text for the completion flash, resolved on the UI thread at dispatch
    /// time. `format_path` renders relative to the *dispatching* tab's `open_dir`, and by the time
    /// the result lands the active tab may be a different one — so the label is frozen here rather
    /// than recomputed in `apply_git_op` (same "resolve what needs `&self` before dispatch" rule as
    /// `FileOpJob::err_failed`).
    label: String,
    /// `StageAll`: how many entries the Git view listed at dispatch time (the count in its flash).
    count: usize,
}

/// Outcome of a background git write, applied on the run loop's thread by `App::apply_git_op`.
pub struct GitOpResult {
    gen: u64,
    kind: GitOpKind,
    /// The tree root the write was dispatched from (copied from `GitOpJob`). `apply_git_op` compares
    /// it with the now-active tab's root to answer "is that tab showing the repository that changed".
    root: PathBuf,
    /// The dispatching tab's identity (copied from `GitOpJob`), which answers the stricter question
    /// "is the active tab the one that asked".
    tab_id: u64,
    path: PathBuf,
    text: String,
    label: String,
    count: usize,
    /// `None` = success. Otherwise git's own message, produced by `git.rs`'s `git_error_message`
    /// (stderr's first `fatal:` line first, so the reason is never pushed off the end of the flash).
    err: Option<String>,
}

pub struct App {
    /// The directory opened at startup (immutable). The reset target for `A` ResetAnchor. Kept separately because open_dir moves with `a`.
    launch_dir: PathBuf,
    pub path_style: PathStyle,
    pub key_scheme: KeyScheme,
    pub lang: crate::i18n::Lang,
    pub cfg: Config,

    /// Tree sort settings (FR / M7 auxiliary). Global (not per-tab). Changed via the `s` menu.
    pub sort: Sort,
    /// Whether the `s` sort-selection menu is showing (while true, main intercepts keys).
    sort_menu: bool,
    /// Bookmarks (M7 auxiliary). Lowercase a-z=local (per launch dir) / uppercase A-Z=global.
    pub bookmarks: crate::bookmarks::Bookmarks,
    /// Tab-session persistence (`[ui] restore_tabs`). Attached by main like the loader senders;
    /// `None` (tests) = tab operations never write session files.
    session_store: Option<crate::session::SessionStore>,
    /// True only while `restore_session` is rebuilding the saved tabs. Suppresses the per-op
    /// `save_session` calls that `tab_new`/`tab_goto` would otherwise fire, so a crash mid-restore
    /// cannot overwrite the on-disk session with a partial set; one complete write happens at the end.
    session_restoring: bool,
    /// State of waiting for a letter after `m`/`'` (Set=register / Jump=jump).
    /// Waiting for the letter key after `m` (bookmark set). (`'` opens the list directly.)
    mark_set_pending: bool,
    /// Whether the bookmark-list overlay is showing.
    bookmark_list: bool,
    /// Selection position in the list (a flat index over local on top and global below, concatenated).
    bookmark_list_sel: usize,
    /// Confirmation/input dialog (create `a` / rename `R` / delete `D`). While showing, main intercepts keys.
    dialog: Option<Dialog>,
    /// File clipboard (M7 Phase B). Push with `Y`=copy/`X`=cut, apply with `P`=paste.
    clipboard: Option<Clipboard>,

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
    /// In a follow-opened diff, show the FULL git diff (vs HEAD/index) instead of the diff since
    /// follow-start. Toggled per-session by the diff-scope key; reset to false (baseline) each `F`-on.
    follow_diff_full: bool,
    /// Snapshot captured at follow-start (`F` on), so follow diffs show only changes SINCE that point —
    /// pre-existing uncommitted changes are folded into the baseline and stay invisible. Held in memory,
    /// bounded to the files that were dirty at follow-start (clean files use the pinned HEAD blob on
    /// demand). Captured on each `F`-on and KEPT afterwards (through `follow_break`/off, so the session's
    /// diffs stay reviewable via `n`/`N`/`f`) until the next `F`-on recaptures it; only ever consulted
    /// under `diff_follow_scope`. See `docs/FEATURE-FOLLOW-BASELINE-2026-07.md`.
    #[cfg(feature = "git")]
    follow_baseline: Option<FollowBaseline>,
    /// The tab root that was active when `follow_session`/`follow_baseline` were (re)captured. Follow
    /// mode is a global (not per-tab) mode, but its session and baseline describe one specific repo —
    /// tab switching, `l`/`h` navigation, worktree switching (`o`→`w`→`Enter`), paste-jump, and bookmark
    /// jumps can all change `tab.root` without going through `toggle_follow`. Without this, the session
    /// and baseline silently kept referring to the OLD root: `n`/`N`'s population mixed paths from a
    /// different repo (the denominator in the title's `(2/5)` lied), and worse — because linked
    /// worktrees share one object database, `follow_baseline_diff`'s `blob_at` call would *successfully*
    /// resolve against the wrong worktree's HEAD, producing a diff that looked plausible but was wrong.
    /// `follow_scope_valid` compares this against `tab.root` (cheap, no I/O) so every consumer can
    /// degrade safely; only `follow_note_change` (the event-drain side, never the render path) recaptures
    /// it when it goes stale.
    follow_root: Option<PathBuf>,

    /// Total wrapped display rows of the current decorated Markdown at the last render (what
    /// `preview_scroll` is clamped against). Set by the preview renderer; used with `MdCache::src_lines`
    /// to map the scroll position back to an approximate source line when opening the editor (`e`).
    pub md_view_rows: usize,

    /// less-style windowed reading for Code/Text (does not read the whole file). Always Some during a Code/Text preview.
    /// While Some, scrolling uses `tab.preview_byte_top` (the line-head byte) instead of preview_scroll.
    preview_win: Option<crate::preview::window::FileWindow>,
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

    /// The file requested for external-editor launch via `e`, plus the 1-based line to open at (matching
    /// the on-screen preview position; None = open at the top). The run loop takes it, suspends the TUI, and
    /// launches (because entering/leaving the terminal must happen on the render thread, here we only raise the "request").
    pending_edit: Option<(PathBuf, Option<usize>)>,

    /// Flag requesting launch of an external git tool (lazygit, etc.) via `O`. The run loop takes it, suspends the TUI, and launches.
    pending_git_tool: bool,

    /// Cache of decorated preview content (Markdown/Mermaid).
    /// Avoids re-parsing (pulldown-cmark/syntect/mermaid layout) on every scroll.
    /// It depends on width (to fit mermaid to the display width), so the key is (path, width).
    md_cache: Option<MdCache>,

    /// Cache of raw diff lines for the GitDiff preview (per path). Avoids recomputing `git diff` every frame.
    diff_cache: Option<DiffCache>,
    gutter_cache: Option<GutterCache>,

    /// Interactive items in the Markdown preview (links + task checkboxes, collected on each render).
    /// Focus with Tab/⇧Tab; Enter opens a link / toggles a checkbox, Space toggles a checkbox.
    md_items: Vec<MdItem>,
    /// Per-`<details>`-block open override (ordinal → open), set by `Space`/`Enter` on the summary.
    /// Absent = the default (from the `open` attribute + `ui.md_details`). Reset on file/tab change.
    details_open: std::collections::HashMap<usize, bool>,

    /// The matching table cells as a set, for O(1) lookup while rendering (mirrors `tab.search_matches`).
    /// Kept separate so a large result set does not turn cell drawing into a linear scan.
    table_search_hits: std::collections::HashSet<(usize, usize)>,

    /// Image backend (M2). None if the terminal is unsupported or uninitialized, in which case images fall back to text.
    /// For rendering, ui::preview passes `image` by &mut to StatefulImage. Resize/encode is
    /// offloaded to a separate thread via `img_tx`, and the result is applied in apply_image_resize.
    picker: Option<Picker>,
    img_tx: Option<UnboundedSender<ResizeRequest>>,
    pub image: Option<ThreadProtocol>,
    /// On a kitty-graphics terminal, still images take konoma's own transmit path instead of
    /// ratatui-image's `image`/`ThreadProtocol`: the pixels are **zlib-compressed** (`o=z`), which
    /// shrinks the terminal transfer 2×–50× (the real latency; see docs/PERF-IMAGE-TRANSFER-2026-07).
    /// `use_kitty` is set from the picker's protocol type; on other terminals (sixel/iterm2/
    /// halfblocks) `kitty_image` stays None and the `image` path is used unchanged.
    use_kitty: bool,
    kitty_is_tmux: bool,
    /// The prepared compressed image for the current crop (None until built / on non-kitty terminals).
    kitty_image: Option<crate::preview::kitty::KittyImage>,
    /// Offloads the kitty resize+compress (~30–50 ms) to a worker thread so rapid zoom/pan does not
    /// hitch the UI. The **first** build after opening a file stays synchronous (so the image appears
    /// without a blank frame); subsequent rebuilds (zoom/pan/resize) go async and the previous image
    /// keeps showing until the new one arrives. None in tests → everything runs synchronously.
    kitty_tx: Option<std::sync::mpsc::Sender<KittyResult>>,
    /// Latest-wins generation for async kitty builds: only the newest spawn's result is applied.
    kitty_gen: u64,
    /// The (crop, cols, rows) the current kitty image (or in-flight build) targets. Prevents
    /// re-spawning the same build every frame while the geometry is unchanged.
    kitty_want: Option<KittyGeom>,
    /// The geometry the **currently shown** kitty image was actually built for. A build is in flight
    /// exactly when this differs from `kitty_want` (covers pan, where the cell size is unchanged).
    kitty_shown: Option<KittyGeom>,
    /// The source image (decoded). Zoom/pan crops this at render time and rebuilds the protocol
    /// (because ratatui-image has no offset pan). Held in an `Arc` so the async kitty-build worker
    /// can share it without copying the (potentially large) pixels.
    image_src: Option<std::sync::Arc<image::DynamicImage>>,
    /// The most recently built crop rectangle (source-image px: x,y,w,h). The protocol is rebuilt only when it changes.
    image_crop: Option<(u32, u32, u32, u32)>,
    /// The most recent visible fraction (0..1, w/h). Less than 1 = the image overflows the viewport and is clipped = pan is possible.
    image_vis_frac: (f64, f64),
    /// SVG source of the current image preview when it is vector-backed (SVG file / mermaid diagram).
    /// Zooming past the raster's density re-rasterizes this at the needed scale on a worker thread
    /// ("sharp zoom"); raster images leave it None (pixel zoom only).
    vector_svg: Option<std::sync::Arc<Vec<u8>>>,
    /// Logical (layout) pixel size of a vector-backed image = the dimensions of its **first** raster.
    /// `image_layout` sizes/zooms against this identity, so swapping in a sharper raster changes only
    /// pixel density, never the on-screen geometry (HiDPI-style backing store).
    image_logical: Option<(u32, u32)>,
    /// A full-screen sharp-zoom re-raster is in flight (at most one — key-repeat `+` must not spawn
    /// the same job a dozen times; the inline-fence analog is `MdImgEntry::reraster_inflight`).
    /// Cleared when a media result arrives (`apply_media`) and by `clear_image` (generation bump).
    vector_reraster_inflight: bool,
    /// Signature of the inline-image overlay drawn last frame (urls + screen rects). When it
    /// changes (scroll/focus moved a diagram, or the document was left), the run loop clears the
    /// terminal once to sweep orphaned kitty unicode-placeholder rows (they are printed into the
    /// terminal grid and a blank-over-blank diff never repaints them).
    md_overlay_last: Option<u64>,
    /// Overlay signature seen during the current frame (None until the overlay draws).
    md_overlay_seen: Option<u64>,
    /// The overlay moved/vanished since the previous frame → the run loop full-redraws once.
    md_overlay_moved: bool,

    /// mtime of the previewed media file at load time. Media (image/svg/gif/video/pdf) is reloaded on an
    /// FS event only when this changes (avoids re-decoding / re-running external tools on unrelated edits).
    preview_media_mtime: Option<std::time::SystemTime>,
    /// Failure reason for the current `PreviewKind::Command` attempt (detached-launch error,
    /// non-zero exit, missing tool, `{out}` never produced, ...). None on success/in-flight/every
    /// other preview kind. Reset by `clear_image` (the same "starting a fresh attempt" checkpoint
    /// used for the rest of the media state) and set either directly (a synchronous detached-launch
    /// failure) or via `apply_payload`'s `MediaPayload::CommandFailed`. Read by `ui/preview.rs`'s
    /// `[can not preview]` fallback through `App::command_error`.
    command_err: Option<String>,
    /// The decoded media of the tab we most recently switched **away from**, so switching back does not
    /// redo the work that produced it. That work is not just an image decode: SVG/mermaid are
    /// rasterized and PDF/video shell out to `pdftocairo` / `ffmpeg`, which costs hundreds of
    /// milliseconds. Exactly **one** slot, holding an `Arc` clone of the source image that was already
    /// in memory, so the extra footprint is bounded to a single image. Animated GIFs are never cached
    /// (their frames are separate state). Reuse requires an exact `(path, mtime, page)` match, so an
    /// externally edited file is always re-read.
    media_cache: Option<MediaCache>,

    /// Parsed CSV/TSV table (Some while a table preview is active and parsing succeeded).
    /// None while not a table, or when parsing failed (then the preview degrades to raw text).
    table_data: Option<crate::preview::table::TableData>,
    /// Per-tab bundle — see `PerTab` (root/mode/preview target/scroll, table cursor/scroll, git-view
    /// overlay, windowed-preview scroll/caret, image/PDF pan-page, Markdown raw/focus/fence-zoom,
    /// selection/filter/search). `pub(crate)` because ui/main read the 13 fields that used to be
    /// flat `pub` on `App` (root/open_dir/entries/selected/show_hidden/tree_viewport/mode/
    /// preview_path/preview_kind/preview_scroll/preview_hscroll/preview_viewport/image_zoom) directly
    /// as `app.tab.<field>`.
    pub(crate) tab: PerTab,
    /// Visible data-row count at the last render (the page size for PageUp/Down). Set by the renderer.
    table_viewport_rows: u16,

    /// Full-cell popup (`Enter` in a table preview): shows the cursor cell's untruncated text,
    /// wrapped and scrollable. App-global (not per-tab), matching `outline_open`/`show_info` — it
    /// is always reset on a new preview / tab switch / new tab (never worth restoring).
    table_cell_open: bool,
    /// Vertical scroll offset within the cell popup (wrapped-row units; clamped at render time).
    table_cell_scroll: u16,
    /// The popup's visible row count at the last render (the page size for PageUp/Down).
    table_cell_viewport: u16,

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
    /// Render cache for the tree's detail columns (`ui.details`): path → formatted cells.
    /// Filled lazily for visible rows (render pre-pass) and dropped on every tree rebuild, so the
    /// per-row stat (and the `items` column's read_dir) runs once per tree generation instead of
    /// on every keypress.
    detail_cells_cache: std::collections::HashMap<PathBuf, Vec<String>>,
    /// Whether the listing currently on screen is known to be **out of date**: the last attempt to
    /// rebuild it failed, so what the tree shows is the last good snapshot rather than the
    /// filesystem as it is now.
    ///
    /// Owned by `rebuild_tree` and by nothing else — it is set on every failure and cleared on
    /// every success, at the one choke point every rebuild passes through. Both the persistent
    /// `STALE` chip (level: it is stale *now*) and the one-shot flash (edge: it *became* stale) are
    /// derived from this single fact, so there is no second "already notified" bit to keep in sync
    /// with it.
    ///
    /// It lives on `App` rather than on `PerTab` because `rebuild_tree` only ever rebuilds the
    /// **active** tab (`self.tab`) and every tab activation ends in a rebuild
    /// (`load_active` → `refresh_fs_after_tab_switch`), so this always describes the listing the
    /// user is looking at — switching to a healthy tab clears it, switching back to a broken one
    /// sets it again.
    tree_stale: bool,
    /// Sender that offloads inline-image decoding to a background thread.
    md_img_tx: Option<std::sync::mpsc::Sender<MdImageResult>>,
    /// Remote inline-image URLs whose background download is in flight (deduplicates fetches).
    md_remote_inflight: std::collections::HashSet<String>,
    /// Remote inline-image URLs whose download failed — show a text placeholder instead of retrying
    /// on every subsequent frame / FS-watch event (see `ensure_remote_md_fetch`'s early-return guard).
    /// Cleared by an explicit `refresh()` (the `r` key etc.), which gives a failed URL another
    /// chance — see `refresh`'s doc comment for why the FS-watch path deliberately does not.
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
    tabs: Vec<PerTab>,
    /// Source of `PerTab::id`. Only ever incremented, so an id is never reused for a later tab.
    tab_seq: u64,
    active_tab: usize,
    /// Tab-list overlay (`T`). App-global (not per-tab).
    tab_list: bool,
    tab_list_sel: usize,
    /// Heading-outline overlay (`o` in a Markdown preview): jump to any heading.
    outline_open: bool,
    outline_sel: usize,

    /// git status (FR-7). Absolute path → kind. Cached per root and re-fetched when root changes.
    git_status: std::collections::HashMap<PathBuf, crate::git::FileStatus>,
    /// The set of top-level gitignore-excluded entries (`git::ignored`). Used to decide which tree entries are dimmed.
    /// Since it is a heavy computation, it is cached **per repo (workdir)** (`git_ignored_for`), so moving root to a
    /// subdirectory within the same repository does not rebuild it.
    git_ignored: std::collections::HashSet<PathBuf>,
    /// The root at which `git_status`/`git_branch` were computed. If it differs from the current root, a re-fetch is *considered* (but see `git_status_workdir`).
    git_status_for: Option<PathBuf>,
    /// The **repo workdir** `git_status`/`git_branch` cover. `git status` is run from the workdir, so its
    /// result is identical anywhere within the same repo — moving root with `l`/`h` reuses it instead of
    /// re-running a full worktree scan (slow on large repos). Same idea as `git_ignored_for` for the ignore set.
    git_status_workdir: Option<PathBuf>,
    /// The cached `git_status` may be stale (a file changed, a commit/checkout happened, ignore rules
    /// changed): force a recompute on the next `refresh_git_if_needed` even if the workdir is unchanged.
    git_status_dirty: bool,
    /// The root for which `git status` is being computed on a separate thread (guard against duplicate
    /// dispatch). None = not computing. Keyed by **root** (never None while in flight), so the
    /// `pending == target` comparison can't be satisfied by two `None`s the way `git_ignored_pending` can.
    git_status_pending: Option<PathBuf>,
    /// Generation of the `statuses` computation. Incremented on dispatch; a result is applied only if it
    /// still matches (discards scans superseded by a newer root/tab change).
    git_status_gen: u64,
    /// Sender returning results from the worker computing `statuses`+`branch` in the background.
    /// If not attached (tests), `spawn_or_sync_statuses` falls back to computing synchronously.
    status_tx: Option<std::sync::mpsc::Sender<StatusResult>>,
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
    /// The generation and root of a filter-pool scan (`/`'s population walk) running on a separate
    /// thread. `None` = nothing in flight. Keyed by **root** (never `None` while running), so the
    /// `pending == target` comparison can't be satisfied by two `None`s.
    ///
    /// The generation is carried alongside so the slot is released by **the result of the scan that
    /// took it** and by nothing else. Two things depend on that: (a) a walk superseded mid-flight
    /// (`filter_pool_gen` bumped without a new dispatch — every tab switch does this) still frees
    /// the slot when it lands, instead of latching it forever; (b) a superseded result must *not*
    /// free the slot of a **newer** walk that is genuinely still running.
    filter_pool_pending: Option<(u64, PathBuf)>,
    /// Generation of the filter-pool scan. Bumped on every dispatch **and on a tab switch**; a
    /// result is applied only if it still matches, so a walk that finishes after the user moved
    /// root, left the filter or switched tabs is discarded instead of landing in the wrong pool.
    filter_pool_gen: u64,
    /// A re-scan was requested while one was in flight. Rather than piling scans up (an agent
    /// writing files produces a burst of FS events), the requests **coalesce** into this flag and
    /// are honoured exactly once when the running scan lands — the same shape `git_status_dirty`
    /// uses, and for the same reason: async removes the natural rate-limit the synchronous version
    /// got from its own duration.
    filter_pool_dirty: bool,
    /// Sender returning results from the worker finishing a filter-pool scan. If not attached
    /// (tests / no channel), `spawn_or_sync_pool` falls back to computing synchronously, so the
    /// whole feature degrades to exactly the old blocking behaviour.
    pool_tx: Option<std::sync::mpsc::Sender<FilterPoolResult>>,
    /// git diff layout (vertical/horizontal/Auto). Initialized from the `git.diff` setting and cycled with `s`. Used by both the GitDiff preview
    /// and the commit/working-tree detail.
    diff_layout: DiffLayout,
    /// The current branch name (fetched at the same time as git status). None if not a repo.
    git_branch: Option<String>,
    /// The origin repo name when the current root is inside a **linked worktree** (`git worktree
    /// add`), else `None`. Same lifecycle/cache as `git_branch` — fetched by the same background
    /// scan (`scan_statuses`), applied by the same `apply_statuses`, cleared whenever `git_branch`
    /// is (moving to a different repo). Drives the persistent "WT <origin>" chip (`ui/status.rs`),
    /// which needs it recomputed exactly once per root change, never per render.
    git_worktree_origin: Option<String>,

    /// Whether the branch-visibility panel (`b`) is open. Modal/transient — not per-tab (reset, not
    /// restored, on tab switch; see `PerTab` for the per-tab git-view overlay state).
    git_graph_picker: bool,
    /// Cursor position in the panel (row = branch).
    git_graph_picker_sel: usize,
    /// Tentative selection while editing the panel (committed to `git_graph_visible` with Enter / discarded with q).
    git_graph_picker_set: std::collections::HashSet<String>,
    /// Whether `J`/`K` reordering was done in the panel (so the base is re-derived to the top branch on Enter confirm).
    git_graph_reordered: bool,

    /// Sender returning results from the worker running background filesystem operations
    /// (paste/duplicate/trash/permanent-delete/drop-transfer). If not attached (tests), `start_file_op`
    /// falls back to computing synchronously, exactly like `spawn_or_sync_statuses`/`_ignored`.
    fileop_tx: Option<std::sync::mpsc::Sender<FileOpResult>>,
    /// Generation of the current/most recent background file operation. Incremented on dispatch;
    /// a result is applied only if it still matches (guards against a stray stale send).
    fileop_gen: u64,
    /// The kind of filesystem operation currently running in the background (None = idle). Only one
    /// runs at a time — starting another while this is `Some` is rejected with a flash.
    fileop_pending: Option<FileOpKind>,
    /// Number of top-level targets in the in-flight operation (the `M` in the `N/M` progress readout).
    fileop_total: usize,
    /// Live progress counters for the in-flight operation, shared with the worker thread. `Arc` so the
    /// UI thread can read it every frame without waiting on the worker.
    fileop_progress: Option<Arc<crate::fileops::Progress>>,

    /// Sender returning results from the worker running background **git writes** (stage/unstage/
    /// discard/commit/checkout/branch/worktree). If not attached (tests), `start_git_op` falls back
    /// to running the write synchronously, exactly like `fileop_tx`/`spawn_or_sync_statuses`.
    gitop_tx: Option<std::sync::mpsc::Sender<GitOpResult>>,
    /// Generation of the current/most recent background git write. Incremented on dispatch; a result
    /// is applied only if it still matches (guards against a stray stale send).
    gitop_gen: u64,
    /// The git write currently running in the background (None = idle). Only one runs at a time —
    /// starting another while this is `Some` is rejected with a flash, because two concurrent writes
    /// would contend on `.git/index.lock` and the second would simply fail.
    gitop_pending: Option<GitOpKind>,
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

/// What the in-preview search (`/`) searches, which decides how matches are found and how `n`/`N`
/// move. Each preview kind addresses positions differently, so the search plumbing routes on this
/// instead of testing preview fields in several places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchTarget {
    /// Code/Text and raw (`R`) Markdown: byte offsets into the file, moved via the read window.
    Windowed,
    /// CSV/TSV: (data row, column) cell coordinates, moved via the cell cursor.
    Table,
    /// Decorated Markdown/Mermaid: indices into the **decorated** lines, moved by scrolling.
    Markdown,
    /// Media and diffs — no search model (`/` flashes a hint).
    Unsupported,
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
/// Everything derivable once per (path, width) lives here so a frame only clones the visible
/// slice (`md_slice`) instead of re-cloning / re-flowing the whole document on every keypress.
struct MdCache {
    path: PathBuf,
    width: u16,
    /// Decorated lines with links already collapsed (label-only; URLs recovered into `items`) —
    /// exactly what the renderer draws, minus the per-frame focus inversion.
    lines: Vec<Line<'static>>,
    /// Tab-cycle items (links / task checkboxes / code-block headers) in document order,
    /// built once from `lines`. Mirrored into `App::md_items` when the cache is (re)built.
    items: Vec<MdItem>,
    /// Block-level inline images reserved within `lines` (Markdown only; empty otherwise).
    images: Vec<crate::preview::markdown::ImagePlacement>,
    /// Number of source lines in the file. Used with the on-screen wrapped-row count to map the scroll
    /// position back to an approximate source line, so `e` opens the editor near the top of the view.
    src_lines: usize,
    /// Widest line in display cells (horizontal-scroll clamp for wrap=off).
    max_line_cols: usize,
    /// wrap=on: visual (post-wrap) start row of each line via ratatui's own per-line reflow;
    /// `len = lines.len() + 1`, last element = total display rows. Empty when wrap=off or width=0.
    row_prefix: Vec<usize>,
    /// Effective mermaid target rows the cache was built with (fit-to-view). A viewport change
    /// only invalidates documents that actually contain fence diagrams.
    fence_rows: u16,
    /// In-page anchor map (GitHub slug → decorated logical-line index) for `[x](#slug)` jumps.
    anchors: Vec<(String, usize)>,
    /// Effective open/closed state of each `<details>` block (document order) as rendered — the base
    /// a `Space`/`Enter` toggle flips.
    details_states: Vec<bool>,
    /// The exact text the renderer parsed to produce `lines`: the file's body after front matter is
    /// stripped and after `process_footnotes` / `process_inline_html` have run.
    ///
    /// Root cause this exists for (2026-08, reported by a user on the released build): the source
    /// scanners behind `y c` and the checkbox toggle read the **raw file** while the renderer draws
    /// this preprocessed text. Any pre-pass that changes block structure therefore desynchronizes
    /// them, and the count guards then refuse the whole document (`y c`) — or, worse, cancel out and
    /// let the toggle write to the wrong line. Keeping the string the renderer actually used, instead
    /// of recomputing the same chain at each scan site, removes the class rather than one instance:
    /// the two sides cannot disagree about text they no longer derive separately.
    pre_src: String,
    /// For each line of `pre_src`, the line of the on-disk body it originated from (`None` = a line
    /// a pre-pass invented). This is what lets the checkbox toggle — which must edit bytes of the
    /// real file, not of `pre_src` — prove that the checkbox it is about to write is the one the
    /// reader is looking at. See `crate::preview::markdown::LineOrigin`.
    pre_origin: crate::preview::markdown::LineOrigin,
}

/// Cache of raw diff lines for the GitDiff preview. `file_diff` (the git call) does not depend on display width
/// (side-by-side/coloring is done separately at render time), so the key is the path only. This avoids re-invoking git on every
/// scroll/horizontal scroll/resize (aligned with log/graph: load once on open).
/// Since the diff changes when the working tree changes, it is invalidated by `refresh()` (FS events / manual refresh).
struct DiffCache {
    path: PathBuf,
    lines: Vec<crate::git::DiffLine>,
}

/// Per-file cap for a follow baseline snapshot: a dirty file larger than this is not snapshotted
/// (recorded as `None`), and its follow diff falls back to the full git diff (honest degradation).
#[cfg(feature = "git")]
const FOLLOW_BASELINE_FILE_CAP: usize = 5 * 1024 * 1024;
/// Total cap across all snapshotted baseline files: once exceeded, remaining dirty files are recorded
/// as `None` (full-diff fallback) so pressing `F` on a fully-dirty tree never balloons memory.
#[cfg(feature = "git")]
const FOLLOW_BASELINE_TOTAL_CAP: usize = 64 * 1024 * 1024;

/// The follow-start snapshot. `dirty` holds the content of files that were already modified/untracked
/// at follow-start (keyed by canonicalized path): `Some(bytes)` = snapshotted, `None` = skipped for
/// size (falls back to full diff). Files NOT in `dirty` were clean at follow-start, so their baseline
/// is the blob at `head`.
#[cfg(feature = "git")]
struct FollowBaseline {
    dirty: std::collections::HashMap<PathBuf, Option<Vec<u8>>>,
    head: Option<String>,
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
#[derive(Clone)]
struct MdItem {
    line: usize,
    kind: MdItemKind,
}

#[derive(Clone)]
enum MdItemKind {
    /// A link; `target`=URL/relative path.
    Link { target: String },
    /// A task checkbox; `state`=its current state char (` `/`x`/custom) as shown.
    Task { state: char },
    /// A fenced code block (its header line is the focus target; `Enter` copies its raw source).
    CodeBlock,
    /// An inline mermaid diagram (image mode); `ordinal`=fence index in document order.
    /// `Enter` opens it full screen with zoom/pan.
    MermaidFence { ordinal: usize },
    /// A `<details>` summary line; `ordinal`=details-block index in document order.
    /// `Enter`/`Space` toggle it collapsed ⇄ expanded.
    Details { ordinal: usize },
}

/// Cache of highlighted lines for windowed reading.
/// The key is (path, top byte, height). Rebuilt only when scroll/vertical resize changes it.
/// (Line content does not depend on display width, so width is not part of the key.)
struct WinCache {
    path: PathBuf,
    byte_top: u64,
    height: u16,
    /// Whether `lines` carry syntax colors. Plain text is cached too (same key space); the flag
    /// keeps a colored window from being mistaken for a plain one when the highlight state flips.
    styled: bool,
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
    /// In-place zoom protocol for a **focused inline mermaid diagram** (`+`/`-` while focused):
    /// a crop of the source encoded into the same (cols, rows) cell area (the layout never moves).
    zoom_protocol: Option<Protocol>,
    /// The (cols, rows, crop px rect) `zoom_protocol` was encoded for.
    zoom_key: Option<(u16, u16, PxRect)>,
    /// SVG source of a rendered mermaid fence — kept so in-place zoom can re-rasterize at a higher
    /// density (sharp zoom, same mechanism as the full-screen vector zoom).
    svg: Option<std::sync::Arc<Vec<u8>>>,
    /// Layout size = the **first** raster's dimensions. The markdown layout (cells) is computed from
    /// this, so a sharper re-raster never changes the reserved area (the user-visible size is fixed).
    layout_px: Option<(u32, u32)>,
    /// A sharpening re-raster is in flight (at most one per diagram).
    reraster_inflight: bool,
    /// All frames of an animated inline GIF (empty = not a GIF, or a static/single-frame one — those
    /// already decode through the plain `decoded` path above and never populate this). Kept as `Arc`s
    /// so advancing to the next frame (`App::advance_md_gifs_if_due`) is a cheap pointer clone into
    /// `decoded` rather than a full image copy — mirrors the full-screen `gif_frames` mechanism
    /// (`App::advance_gif_if_due`) but per cache entry, since several inline GIFs can animate at once.
    frames: Vec<(Arc<image::DynamicImage>, std::time::Duration)>,
    /// Index of the currently-displayed frame within `frames`.
    idx: usize,
    /// The time the current frame began showing (mirrors `App::gif_shown_at`). None = before the
    /// first tick (frame 0 is already shown via `decoded`; timing starts on the next tick).
    shown_at: Option<std::time::Instant>,
}

/// Outcome of a fence-sharpen check (the sync variant lets tests re-run with the new raster).
#[derive(PartialEq)]
enum FenceSharpen {
    NotNeeded,
    Spawned,
    AppliedSync,
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
    /// An in-place zoom crop of a focused inline mermaid diagram, encoded into the same
    /// (cols, rows) area. Keyed by the pixel crop so pan/zoom/re-raster all re-encode naturally.
    Zoom { cols: u16, rows: u16, crop: PxRect },
}

/// A request to encode an inline image (or a cropped band of it) off the UI thread (principle #4).
pub struct MdEncodeRequest {
    path: PathBuf,
    key: MdEncodeKey,
    image: Arc<image::DynamicImage>,
    /// Pixel band to crop before encoding; None encodes the whole image.
    crop: Option<PxRect>,
    cols: u16,
    rows: u16,
}

/// Result of a background inline-image encode, delivered to the run loop.
pub struct MdEncodeResult {
    path: PathBuf,
    key: MdEncodeKey,
    /// None = the encode failed. The worker **always** sends a result: swallowing failures left
    /// `enc_inflight` latched on, freezing all re-encodes of that image and keeping
    /// `md_images_loading()` true forever (busy spinner + 16ms polling — idle CPU 0% broken).
    protocol: Option<Protocol>,
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
        let (path, key) = (req.path.clone(), req.key);
        let picker = &picker; // borrow: so the move closure doesn't consume picker across loop iterations
                              // Wrap the whole request handling in a panic catch: even if one new_protocol/crop
                              // panics, don't kill the worker thread (killing it would stop every future encode,
                              // latching enc_inflight forever).
        let protocol = crate::preview::markdown::catch_silent(move || {
            let img = match req.crop {
                Some((x, y, w, h)) => req.image.crop_imm(x, y, w, h),
                None => (*req.image).clone(),
            };
            let size = ratatui::layout::Size::new(req.cols, req.rows);
            // A fence diagram (vector-derived) is **also scaled up** to the reserved grid (Scale):
            // even if the raster is small, it fills the area instead of leaving a top-left-packed
            // margin band — sharpness is handled by density-following re-rasterization instead.
            // A photo keeps the previous Fit behavior (never scaled up past its natural size = never blurred).
            let resize =
                if crate::preview::markdown::is_mermaid_fence_url(&req.path.to_string_lossy()) {
                    Resize::Scale(Some(FilterType::Lanczos3))
                } else {
                    Resize::Fit(Some(FilterType::Lanczos3))
                };
            // Always send back a result even on failure (Err): swallowing it would leave enc_inflight stuck true forever.
            picker.new_protocol(img, size, resize).ok()
        })
        .flatten();
        if tx
            .send(MdEncodeResult {
                path,
                key,
                protocol,
            })
            .is_err()
        {
            break;
        }
    }
}

/// Result of a background inline-image decode, delivered to the run loop.
pub struct MdImageResult {
    path: PathBuf,
    image: Result<image::DynamicImage, String>,
    /// SVG source alongside a rendered mermaid fence (kept for in-place sharp zoom). None otherwise.
    svg: Option<std::sync::Arc<Vec<u8>>>,
    /// True when this is a sharpening re-raster of an already-shown diagram: only the pixels are
    /// replaced — the layout size and the decoration cache stay untouched.
    reraster: bool,
    /// All frames when this is an animated inline GIF (`image` above is just its first frame, kept
    /// so the existing layout-sizing code path is unchanged). None for everything else, including a
    /// static/single-frame GIF, which already fell back to the plain still-image decode.
    frames: Option<Vec<(image::DynamicImage, std::time::Duration)>>,
}

/// Result of a background remote-image download (curl → local cache file), delivered to the run loop.
/// It carries no bytes: on success the file is already in the cache, so the run loop just invalidates
/// the decoration cache to re-lay the image out; on failure the URL is remembered so it is not retried.
pub struct RemoteFetch {
    url: String,
    ok: bool,
}

/// Result of `build_decorated`: the rendered lines plus everything needed to finish setting up the
/// Markdown cache (inline images, follow-up fetches/renders to kick off, source line count).
struct DecoratedMarkdown {
    lines: Vec<Line<'static>>,
    images: Vec<crate::preview::markdown::ImagePlacement>,
    remote_urls: Vec<String>,
    mermaid_fences: Vec<String>,
    math_exprs: Vec<(String, bool)>,
    src_lines: usize,
    /// Exactly the text handed to the renderer: front matter stripped, footnotes and inline HTML
    /// already rewritten. Stored so the source scanners can read *this* string rather than
    /// re-deriving the same chain from the file — see `MdCache::pre_src`.
    pre_src: String,
    /// Per line of `pre_src`, the body line it came from — see `MdCache::pre_origin`.
    pre_origin: crate::preview::markdown::LineOrigin,
}

/// Per-tab state bundle. Migrated concern-by-concern out of the flat App fields so tab save/load
/// is one clone and adding a per-tab field touches one place (see docs/REFACTOR-2026-07.md).
/// `App.tabs` is `Vec<PerTab>` (one entry per tab, snapshot/restore on switch); the active tab's
/// working state is held in `App.tab` as the source of truth. Currently: Table preview cursor + scroll, the git-view overlay
/// (changes hub / log / graph / branches / commit detail) state, windowed-preview scroll/caret
/// internals (byte/line top, 2D cursor), image/PDF pan-and-page position, Markdown raw-mode/Tab-focus/
/// inline-fence zoom state, and the tab-scoped selection/filter/search state.
///
/// `Default` is implemented manually (not derived): a few fields have a non-zero "fresh tab" value
/// (`image_center`/`fence_center` start centered, `fence_zoom` starts at 1.0=fit, `pdf_page` starts
/// at 1), matching what `App::new` used to initialize them to before this migration.
#[derive(Clone)]
pub(crate) struct PerTab {
    /// Identity of this tab, unique for the lifetime of the process and never reused.
    ///
    /// Needed because a tab has no other stable name: its index moves when tabs are closed or
    /// reordered, and its `root` is not unique either — `t` (`tab_new`) opens the new tab **at the
    /// current root**, so two tabs sharing a root is the ordinary case, not a corner case. A
    /// background git write that finishes after the user pressed `t` must still be able to tell
    /// which of them asked for it (see `apply_git_op`). Handed out by `App::next_tab_id`.
    pub(crate) id: u64,
    // --- Tree/root navigation state ---
    pub(crate) root: PathBuf,
    /// The directory opened at startup. The base for relative-path display (kept separately because root moves up with h).
    pub(crate) open_dir: PathBuf,
    /// The tree flattened into display order. For M0 it is fine to simply rebuild it every time.
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: usize,
    pub(crate) show_hidden: bool,
    /// Height (in rows) of the tree display area at the last render. Used as the page size for paging.
    pub(crate) tree_viewport: u16,
    pub(crate) mode: Mode,
    /// The target, kind, and scroll position (vertical/horizontal) while in Preview mode.
    /// Horizontal scroll is used to view long lines when not wrapping (ui.wrap=false).
    /// To avoid scrolling past the end, the actual clamp is done at render time (when content and screen size are known).
    pub(crate) preview_path: Option<PathBuf>,
    pub(crate) preview_kind: Option<PreviewKind>,
    pub(crate) preview_scroll: u16,
    pub(crate) preview_hscroll: u16,
    /// Height (in rows) of the text display area at the last render. Used as the page size for paging.
    pub(crate) preview_viewport: u16,
    table_cur_row: usize,
    table_cur_col: usize,
    table_top_row: usize,
    table_left_col: usize,
    // The git overlay is also kept per tab (still in git mode after viewing a doc in another tab and coming back).
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
    // The linked-worktree list (`w` from the changes hub). Same shape as the branch-list fields
    // right above (list / cursor / filter query / filter-input-active).
    git_worktrees: Option<Vec<crate::git::WorktreeInfo>>,
    git_worktree_sel: usize,
    git_worktree_filter: String,
    git_worktree_filtering: bool,
    git_graph: Option<Vec<crate::git::GraphRow>>,
    git_graph_sel: usize,
    // Since we save the graph body itself (git_graph), the derived state that decorates it (base
    // pin, legend, visible branches, priority order) must be saved to the same tab too, or the
    // legend/`base:` title left over from another tab would stick around after changing base/view
    // there and coming back (load_active does not rebuild the graph). Same treatment as saving
    // git_detail_meta/title alongside git_detail. The picker's (modal) transient state is not
    // saved, and gets reset on restore.
    git_graph_base: Option<String>,
    git_graph_base_label: Option<String>,
    git_graph_visible: std::collections::HashSet<String>,
    git_graph_legend: Vec<crate::git::LegendEntry>,
    git_graph_hidden: usize,
    git_graph_order: Vec<String>,
    // --- Internal scroll state for a windowed (Code/Text/raw md) preview ---
    preview_byte_top: u64,
    preview_top_line: usize,
    /// Text-mode `PreviewKind::Command`'s generated output (the `{out}` artifact / captured
    /// stdout) — the windowed reader opens **this** instead of `preview_path` (see
    /// `App::windowed_src`), so the temp path never leaks into the title. `None` for every other
    /// preview kind, and while a fresh run hasn't landed yet (the render side shows a loading/
    /// failure state instead — see `App::setup_windowed`/`ui/preview.rs`). Owned per tab so a
    /// background tab's leftover artifact survives a switch away and back long enough to be
    /// cleaned up (`App::clear_command_out`) rather than orphaned.
    command_out: Option<PathBuf>,
    // --- Windowed preview's 2D caret position (the selection anchor is not carried over) ---
    preview_cursor_line: usize,
    preview_cursor_col: usize,
    // --- Image/PDF pan and page position ---
    /// Zoom factor. 1.0=the whole image fits, >1.0 zooms in (overflowing the viewport clips and enables pan).
    pub(crate) image_zoom: f64,
    image_center: (f64, f64),
    pdf_page: u32,
    pdf_pages: Option<u32>,
    // --- Markdown/Mermaid raw-source display (`R`) / Tab focus / inline-diagram in-place zoom & full-screen return ---
    md_raw: bool,
    focused_item: Option<usize>,
    fence_zoom: f64,
    fence_center: (f64, f64),
    fence_return: Option<(u16, Option<usize>)>,
    // --- Per-tab selection / filter / preview search ---
    selection: BTreeSet<PathBuf>,
    visual_anchor: Option<usize>,
    tree_filter: Option<String>,
    filter_input: Option<String>,
    filter_pool: Vec<Entry>,
    changed_filter: bool,
    /// The file the `C` cursor was on, for a changed-list rebuild that had to be **deferred** until
    /// the in-flight `git status` lands (`refresh_fs_inner` hands it to `apply_statuses`).
    ///
    /// The identity has to travel with the deferred work because `rebuild_tree` has already replaced
    /// `entries` with the ordinary listing by the time the scan arrives — `apply_statuses` reading
    /// the cursor for itself there gets a file from the *whole tree* (measured: a cursor on the third
    /// changed file read back as an unchanged, committed one). See `reapply_changed_filter`.
    ///
    /// Lives on `PerTab` (not `App`) precisely so it travels with a tab switch rather than being
    /// lost by one: this used to be an `App`-level field that `save_active` explicitly reset to
    /// `None` on every switch (reasoning: "one tab's anchor must never be found and acted on by
    /// another tab's rebuild"), but that same reset also discarded a tab's *own* still-pending
    /// anchor the instant the user switched away from it — the deferred rebuild that eventually
    /// landed (`apply_statuses`, run against whichever tab happens to be active by then) fell back
    /// to reading `entries[selected]` out of the stale full-tree snapshot left behind by
    /// `rebuild_tree`, landing the cursor on an arbitrary, unrelated file instead of the one the
    /// deferred rebuild was originally anchored on. Being a genuine per-tab field closes both holes
    /// at once: a fresh tab naturally starts at `None` (no leak *in*), and a tab's own pending
    /// anchor rides along in its own snapshot across any number of switches until it is actually
    /// consumed (no loss *out*).
    changed_anchor_pending: Option<PathBuf>,
    preview_search: Option<String>,
    search_input: Option<String>,
    search_matches: Vec<(u64, usize, usize)>,
    search_idx: usize,
}

impl Default for PerTab {
    fn default() -> Self {
        Self {
            // 0 = "no identity assigned yet". Every tab the user can actually reach gets a real id
            // from `App::next_tab_id` (App::new for the first, tab_new for the rest); the default is
            // only ever seen by the momentarily-empty slot `mem::take` leaves behind in `tabs`.
            id: 0,
            // root/open_dir has no meaningful default — App::new overrides both right after
            // `PerTab::default()`, and `rebuild_tree` fills `entries`.
            root: PathBuf::new(),
            open_dir: PathBuf::new(),
            entries: Vec::new(),
            selected: 0,
            show_hidden: false,
            tree_viewport: 0,
            // App::new starts in Tree, not whatever `Mode::default()` would be.
            mode: Mode::Tree,
            preview_path: None,
            preview_kind: None,
            preview_scroll: 0,
            preview_hscroll: 0,
            preview_viewport: 0,
            table_cur_row: 0,
            table_cur_col: 0,
            table_top_row: 0,
            table_left_col: 0,
            git_view: false,
            git_view_sel: 0,
            git_view_entries: Vec::new(),
            came_from_git_view: false,
            git_log: None,
            git_log_sel: 0,
            git_detail: None,
            git_detail_meta: None,
            git_detail_title: None,
            git_detail_scroll: 0,
            git_detail_hscroll: 0,
            git_detail_viewport: 0,
            git_detail_total: 0,
            git_branches: None,
            git_branch_sel: 0,
            git_branch_filter: String::new(),
            git_branch_filtering: false,
            git_worktrees: None,
            git_worktree_sel: 0,
            git_worktree_filter: String::new(),
            git_worktree_filtering: false,
            git_graph: None,
            git_graph_sel: 0,
            git_graph_base: None,
            git_graph_base_label: None,
            git_graph_visible: std::collections::HashSet::new(),
            git_graph_legend: Vec::new(),
            git_graph_hidden: 0,
            git_graph_order: Vec::new(),
            preview_byte_top: 0,
            preview_top_line: 0,
            command_out: None,
            preview_cursor_line: 0,
            preview_cursor_col: 0,
            // App::new used to initialize image_zoom to 1.0 (whole image fits), not 0.0.
            image_zoom: 1.0,
            // The image/fence center's initial value is "centered" (same as App::new used to set it to (0.5, 0.5)).
            image_center: (0.5, 0.5),
            // PDF pages are 1-based (same as App::new used to set it to 1).
            pdf_page: 1,
            pdf_pages: None,
            md_raw: false,
            focused_item: None,
            // A fence's in-place zoom starts at 1.0=fit (same as App::new used to set it to 1.0).
            fence_zoom: 1.0,
            fence_center: (0.5, 0.5),
            fence_return: None,
            selection: BTreeSet::new(),
            visual_anchor: None,
            tree_filter: None,
            filter_input: None,
            filter_pool: Vec::new(),
            changed_filter: false,
            changed_anchor_pending: None,
            preview_search: None,
            search_input: None,
            search_matches: Vec::new(),
            search_idx: 0,
        }
    }
}

impl App {
    pub fn new(root: PathBuf, cfg: Config) -> Result<Self> {
        let path_style = PathStyle::parse(&cfg.ui.path_style);
        let key_scheme = KeyScheme::parse(&cfg.ui.keys);
        let lang = crate::i18n::Lang::resolve(&cfg.ui.lang);
        let sort = Sort::from_config(&cfg.ui.sort);
        // `[ui] show_hidden` — captured before `cfg` is moved into `Self` below, and used as the
        // fresh tab's starting value (rebuild_tree, called right after construction, reads
        // `self.tab.show_hidden`, so this has to land before that first build).
        let show_hidden = cfg.ui.show_hidden;
        // The diff layout's initial value (config git.diff). At runtime, `s` cycles vertical → horizontal → Auto.
        let diff_layout = DiffLayout::parse(&cfg.git.diff);
        // Run2 keymap: merge the defaults + config (`[keys.<surface>]` + the old copy_* alias) and
        // validate for conflicts. The page/half profile follows ui.keys (vim/less). Stage 2 is
        // construction only (dispatch is not yet wired up).
        let keymaps = crate::keymap::KeyMap::from_config(
            crate::keymap::scheme_from_str(&cfg.ui.keys),
            &cfg.keys.to_keymap_config(),
        );
        let mut app = Self {
            launch_dir: root.clone(),
            path_style,
            key_scheme,
            lang,
            cfg,
            sort,
            sort_menu: false,
            bookmarks: crate::bookmarks::Bookmarks::load(&root),
            session_store: None,
            session_restoring: false,
            mark_set_pending: false,
            bookmark_list: false,
            bookmark_list_sel: 0,
            dialog: None,
            clipboard: None,
            follow_mode: false,
            follow_session: Vec::new(),
            diff_follow_scope: false,
            follow_diff_full: false,
            #[cfg(feature = "git")]
            follow_baseline: None,
            follow_root: None,
            md_view_rows: 0,
            preview_win: None,
            preview_total_lines: None,
            win_cache: None,
            hl_pending: false,
            hl_warming: false,
            spinner_frame: 0,
            pending_edit: None,
            pending_git_tool: false,
            md_cache: None,
            diff_cache: None,
            gutter_cache: None,
            md_items: Vec::new(),
            details_open: std::collections::HashMap::new(),
            table_search_hits: std::collections::HashSet::new(),
            picker: None,
            img_tx: None,
            image: None,
            use_kitty: false,
            kitty_is_tmux: false,
            kitty_image: None,
            kitty_tx: None,
            kitty_gen: 0,
            kitty_want: None,
            kitty_shown: None,
            image_src: None,
            image_crop: None,
            image_vis_frac: (1.0, 1.0),
            vector_svg: None,
            image_logical: None,
            vector_reraster_inflight: false,
            md_overlay_last: None,
            md_overlay_seen: None,
            md_overlay_moved: false,
            preview_media_mtime: None,
            command_err: None,
            media_cache: None,
            table_data: None,
            tab: PerTab {
                id: 1,
                root: root.clone(),
                open_dir: root.clone(),
                show_hidden,
                ..PerTab::default()
            },
            tab_seq: 1,
            table_viewport_rows: 0,
            table_cell_open: false,
            table_cell_scroll: 0,
            table_cell_viewport: 0,
            preview_visual_anchor: None,
            preview_visual_linewise: false,
            gif_frames: Vec::new(),
            gif_idx: 0,
            gif_shown_at: None,
            gif_protocol: None,
            gif_proto_key: None,
            media_tx: None,
            md_image_cache: std::collections::HashMap::new(),
            detail_cells_cache: std::collections::HashMap::new(),
            tree_stale: false,
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
            tab_list: false,
            tab_list_sel: 0,
            outline_open: false,
            outline_sel: 0,
            git_status: std::collections::HashMap::new(),
            git_ignored: std::collections::HashSet::new(),
            git_status_for: None,
            git_status_workdir: None,
            git_status_dirty: false,
            git_status_pending: None,
            git_status_gen: 0,
            status_tx: None,
            git_ignored_for: None,
            git_ignored_pending: None,
            git_ignored_gen: 0,
            git_ignored_dirty: false,
            ignored_tx: None,
            filter_pool_pending: None,
            filter_pool_gen: 0,
            filter_pool_dirty: false,
            pool_tx: None,
            diff_layout,
            git_branch: None,
            git_worktree_origin: None,
            git_graph_picker: false,
            git_graph_picker_sel: 0,
            git_graph_picker_set: std::collections::HashSet::new(),
            git_graph_reordered: false,
            fileop_tx: None,
            fileop_gen: 0,
            fileop_pending: None,
            fileop_total: 0,
            fileop_progress: None,
            gitop_tx: None,
            gitop_gen: 0,
            gitop_pending: None,
        };
        // Apply `[external] git` to this thread now, before entering the first call that touches
        // git (rebuild_tree). It's thread-local, so it doesn't race with other tests (see git.rs's
        // EXTERNAL_GIT_ENABLED).
        #[cfg(feature = "git")]
        crate::git::set_external_git_enabled(app.cfg.external.git);
        app.rebuild_tree()?;
        // Register the current state as the first tab.
        app.tabs.push(app.snapshot_tab());
        Ok(app)
    }

    /// Attach the per-project session store (`[ui] restore_tabs`). Called by main only, like the
    /// loader senders — tests that need persistence attach a temp-based store explicitly, so plain
    /// `App::new` (unit/e2e tests) never touches the real `~/.config`.
    pub fn attach_session_store(&mut self, store: crate::session::SessionStore) {
        self.session_store = Some(store);
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
        // HashSet: build_dir checks membership once per child (a Vec scan is E×N comparisons).
        let expanded_dirs: std::collections::HashSet<PathBuf> = self
            .tab
            .entries
            .iter()
            .filter(|e| e.is_dir && e.expanded)
            .map(|e| e.path.clone())
            .collect();

        // Record whether the listing is up to date **here**, the one point every rebuild passes
        // through, so "what is on screen is out of date" is a single fact owned by one place
        // rather than a flag each caller has to remember to maintain. Failing keeps the last good
        // `entries` (deliberate — nothing breaks on screen), which is exactly why the fact has to
        // be recorded: otherwise a failure is indistinguishable from a success.
        if let Err(e) = build_dir(
            &self.tab.root,
            0,
            &expanded_dirs,
            self.tab.show_hidden,
            self.sort,
            &mut out,
        ) {
            self.tree_stale = true;
            return Err(e);
        }
        self.tree_stale = false;
        self.tab.entries = out;
        if self.tab.selected >= self.tab.entries.len() {
            self.tab.selected = self.tab.entries.len().saturating_sub(1);
        }
        // Once entries are rebuilt, visual_anchor (an entries index) is stale. Always invalidate it.
        self.tab.visual_anchor = None;
        // The detail-cells cache also changes generation (a checkpoint where fs may have changed = always discard it here).
        self.detail_cells_cache.clear();
        Ok(())
    }

    /// Fill the detail-column cell cache for the given visible rows (tree render pre-pass).
    /// A path already cached is skipped; the whole cache is dropped by `rebuild_tree` (the choke
    /// point every fs change goes through), so cells are computed once per tree generation.
    pub fn ensure_detail_cells(&mut self, rows: &[(PathBuf, bool)], cols: &[String]) {
        for (path, is_dir) in rows {
            if self.detail_cells_cache.contains_key(path) {
                continue;
            }
            let meta = crate::fileops::quick_meta(path).unwrap_or(crate::fileops::RowMeta {
                is_dir: *is_dir,
                is_symlink: false,
                size: 0,
                mtime: None,
                mode: 0,
            });
            let cells: Vec<String> = cols
                .iter()
                .map(|id| crate::fileops::detail_cell(id, path, &meta).unwrap_or_default())
                .collect();
            self.detail_cells_cache.insert(path.clone(), cells);
        }
    }

    /// Cached detail cells for one row (filled by `ensure_detail_cells`).
    pub fn detail_cells_get(&self, path: &Path) -> Option<&Vec<String>> {
        self.detail_cells_cache.get(path)
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

    /// Whether the listing on screen is out of date (the last rebuild failed and the previous
    /// snapshot is still being shown). Drives the persistent `STALE` chip in the context bar
    /// (`ui/status.rs::context_spans`); see the field for why it is not per-tab.
    pub fn tree_stale(&self) -> bool {
        self.tree_stale
    }

    /// Run a refresh whose failure **cannot** be propagated — the fs-watch driven refresh (`main`'s
    /// run loop has no user action to attribute an error to) and the tab switch (`load_active`
    /// returns nothing) — and announce the **start** of an outage. Both used to drop the failure
    /// with `let _ = …`.
    ///
    /// `rebuild_tree` keeps the last good listing instead of crashing, so a swallowed failure is
    /// not visible as breakage — it is **silently stale**, which is the worst outcome: the user
    /// reads an out-of-date tree believing it is current. The staleness itself is now permanently
    /// visible as a chip, so what is left here is only the one-shot announcement.
    ///
    /// The flash is deliberately **edge-triggered** rather than emitted on every failure: these two
    /// paths fire on every watcher burst and every tab switch, so a persistent outage (an
    /// unreadable root) would otherwise overwrite the flash line continuously and drown out
    /// everything else. The edge is read straight off `tree_stale` — the level `rebuild_tree`
    /// maintains — rather than from a separate "already notified" latch, so the two can never
    /// disagree: whatever makes the chip appear is exactly what emits the flash.
    ///
    /// Taking the refresh as a closure keeps "sample the level, run, compare" in one place; a
    /// caller cannot pass the wrong before-value because it never sees one.
    fn refresh_reporting_staleness(&mut self, refresh: impl FnOnce(&mut Self) -> Result<()>) {
        let was_stale = self.tree_stale;
        let Err(e) = refresh(self) else { return };
        if was_stale {
            return; // same outage, already announced — the chip is what keeps it visible
        }
        self.flash = Some(format!(
            "{}{e}",
            crate::i18n::tr(self.lang, crate::i18n::Msg::ListingStale)
        ));
    }

    // --- Sort (FR, M7 helper) -------------------------------------------------
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
        // While filtering, don't change the result order (it takes effect on tree rebuild after clearing).
        if self.tab.tree_filter.is_none() {
            // Anchor captured before the rebuild below — same hazard as `toggle_hidden`/
            // `refresh_fs_inner`: once `rebuild_tree` replaces `entries` with the ordinary listing,
            // reading it again would name a different file (or, while `C` is up, a plain-tree
            // directory that was never in the changed list).
            let anchor = self.visibility_change_anchor();
            self.rebuild_tree()?;
            // `rebuild_tree` always produces the plain tree, so while `C` is up it must be
            // reapplied here too, the same way `toggle_hidden` does — otherwise the header keeps
            // claiming CHANGED while the list underneath silently becomes the ordinary tree (the
            // sort order itself never applies to `C`'s list, which stays a fixed path order).
            self.refilter_after_visibility_change(anchor);
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

    // --- Mode display (2 axes: display mode × internal mode) ----------------------------
    /// Display mode (for the outer chip). Preview becomes Image if it is an image.
    pub fn display_mode(&self) -> DisplayMode {
        match self.tab.mode {
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
    /// The bottom of the UI stack: what is on screen once nothing is overlaying it.
    fn base_layer(&self) -> BaseLayer {
        BaseLayer {
            kind: match self.tab.mode {
                Mode::Preview if self.is_image_preview() => BaseKind::PreviewImage,
                Mode::Preview if self.is_table_preview() => BaseKind::PreviewTable,
                Mode::Preview => BaseKind::PreviewText,
                Mode::Tree => BaseKind::Tree,
            },
            preview_visual: self.is_preview_visual(),
            changed_filter: self.tab.changed_filter,
        }
    }

    /// Walk the **single priority order** ([`UiLayer`], front to back) and return the frontmost
    /// layer that is active.
    ///
    /// `visible` lets a caller be blind to layers that mean nothing to it, in which case the walk
    /// carries on to the layer behind. Its only real use is [`App::surface`] on a no-git build,
    /// where `keymap::Surface` has no git variants; [`App::internal_mode`] sees every layer.
    /// The base layer is always active, so the walk always terminates there.
    ///
    /// Off the render path in the sense that it only reads existing `bool` predicates — the same
    /// work the two hand-written cascades did.
    fn frontmost_layer(&self, visible: impl Fn(UiLayer) -> bool) -> UiLayer {
        macro_rules! layer {
            ($active:expr, $l:expr) => {
                if $active && visible($l) {
                    return $l;
                }
            };
        }
        // --- Dialogs (frontmost). The three dialog kinds are mutually exclusive; within a confirm,
        // the non-destructive operations are split out so they get their own chip/footer wording. ---
        let confirm = self.dialog_is_confirm();
        layer!(confirm && self.confirm_is_quit(), UiLayer::ConfirmQuit);
        layer!(
            confirm && self.confirm_is_bookmark(),
            UiLayer::ConfirmBookmark
        );
        layer!(confirm && self.confirm_is_drop(), UiLayer::ConfirmDrop);
        layer!(confirm, UiLayer::ConfirmDelete);
        layer!(self.dialog_is_preview(), UiLayer::RenamePreview);
        let input = self.dialog_is_input();
        layer!(input && self.pending_is_create(), UiLayer::InputCreate);
        layer!(
            input && self.pending_is_batch_rename_input(),
            UiLayer::InputBatchRename
        );
        layer!(input && self.pending_is_git_commit(), UiLayer::InputCommit);
        layer!(
            input && self.pending_is_git_create_branch(),
            UiLayer::InputCreateBranch
        );
        layer!(
            input && self.pending_is_worktree_create(),
            UiLayer::InputCreateWorktree
        );
        layer!(input, UiLayer::InputRename);

        // --- Help, above everything but the dialogs ---
        layer!(self.show_help, UiLayer::Help);

        // --- The git views. The commit detail sits over the log, and the branch panel sits over
        // the graph (`is_git_graph` is still true underneath it), so each goes before what it covers. ---
        layer!(self.is_git_detail(), UiLayer::GitDetail);
        layer!(self.is_git_log(), UiLayer::GitLog);
        layer!(self.is_git_graph_picker(), UiLayer::GitGraphPicker);
        layer!(self.is_git_graph(), UiLayer::GitGraph);
        let branches = self.is_git_branches();
        layer!(
            branches && self.git_branch_filtering(),
            UiLayer::GitBranchFilter
        );
        layer!(branches, UiLayer::GitBranches);
        let worktrees = self.is_git_worktrees();
        layer!(
            worktrees && self.git_worktree_filtering(),
            UiLayer::GitWorktreeFilter
        );
        layer!(worktrees, UiLayer::GitWorktrees);
        layer!(self.is_git_view(), UiLayer::GitChanges);
        layer!(self.is_git_diff_preview(), UiLayer::GitDiff);

        // --- Input surfaces / menus / lists / popups ---
        layer!(self.is_filtering(), UiLayer::Filter);
        layer!(self.is_searching(), UiLayer::Search);
        layer!(self.is_sort_menu(), UiLayer::Sort);
        layer!(self.is_marking(), UiLayer::Mark);
        layer!(self.is_bookmark_list(), UiLayer::Bookmarks);
        layer!(self.is_tab_list(), UiLayer::Tabs);
        layer!(self.is_outline(), UiLayer::Outline);
        layer!(self.is_info(), UiLayer::Info);
        layer!(self.is_table_cell_open(), UiLayer::TableCell);
        layer!(self.is_visual(), UiLayer::Visual);

        UiLayer::Base(self.base_layer())
    }

    /// Internal mode (for the inner chip and footer switching). None if nothing is being operated.
    /// A projection of the frontmost layer — see [`UiLayer`] for the priority order.
    pub fn internal_mode(&self) -> Option<InternalMode> {
        self.frontmost_layer(|_| true).internal_mode()
    }

    /// The **frontmost surface** that currently receives keys (the single source of truth for Run2
    /// keymap dispatch). A projection of the frontmost layer that has a surface on this build — see
    /// [`UiLayer`] for the priority order and [`UiLayer::surface`] for the one case that has none.
    pub fn surface(&self) -> crate::keymap::Surface {
        let layer = self.frontmost_layer(|l| l.surface().is_some());
        match layer.surface() {
            Some(s) => s,
            // Unreachable: `frontmost_layer` only returns layers that just answered `Some`, and the
            // base layer always has a surface. Degrade rather than panic (design principle #3).
            None => self.base_layer().surface(),
        }
    }

    // --- Copy/cut & paste (M7 Phase B, default keys Y/X/P, changeable via keymap) ---
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
    /// Runs in the background (design principle #4): a large copy/move no longer freezes input/rendering.
    /// With no runner attached (tests), it still completes synchronously — see `start_file_op`.
    pub fn paste(&mut self) -> Result<()> {
        let Some(clip) = self.clipboard.clone() else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::ClipboardEmpty).into());
            return Ok(());
        };
        let dir = self.op_base_dir();
        let kind = match clip.op {
            ClipOp::Copy => FileOpKind::PasteCopy,
            ClipOp::Cut => FileOpKind::PasteMove,
        };
        let is_cut = matches!(clip.op, ClipOp::Cut);
        let job = FileOpJob {
            kind,
            targets: clip.paths,
            dest: Some(dir),
            root: PathBuf::new(), // filled in by start_file_op with tab.root at dispatch time
            err_self_paste: crate::i18n::tr(self.lang, crate::i18n::Msg::CannotPasteIntoSelf)
                .to_string(),
            err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed).to_string(),
        };
        // A cut is consumed the moment it's dispatched (the original is already "in use"). But
        // **only when the job actually started** — if another operation was already in progress
        // and this one was rejected, keep the clipboard so the user can try again.
        if self.start_file_op(job) && is_cut {
            self.clipboard = None;
        }
        Ok(())
    }

    /// `Space→D`=duplicate: copy each target **in place** (into its own parent directory) with a
    /// collision-free name (`note copy.md`, then `note copy 2.md`). Operates on the selection, or the
    /// cursor entry when none. Directories are duplicated recursively (as a sibling `dir copy`), and
    /// symlinks are copied as links — both via `copy_into_with_progress`. Never overwrites. The last
    /// new item is revealed and selected so the result is visible.
    /// Runs in the background (design principle #4) — see `start_file_op`.
    pub fn duplicate_selection(&mut self) -> Result<()> {
        let targets = self.op_targets();
        if targets.is_empty() {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return Ok(());
        }
        let job = FileOpJob {
            kind: FileOpKind::Duplicate,
            targets,
            dest: None, // The duplication target is each target's own parent (run_file_op decides via src.parent())
            root: PathBuf::new(), // filled in by start_file_op with tab.root at dispatch time
            err_self_paste: String::new(), // Duplicate doesn't use the self-paste guard
            err_failed: crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed).to_string(),
        };
        // Clear the selection only when the job actually started (leave it when another operation was in progress and this one was rejected).
        if self.start_file_op(job) {
            self.clear_selection();
        }
        Ok(())
    }

    /// The selection listed in the **current sort order (tree display order)**. Used as the numbering order for sequential rename.
    /// Selections not present in entries (collapsed, etc.) are appended at the end in path order.
    fn selection_in_display_order(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .tab
            .entries
            .iter()
            .filter(|e| self.tab.selection.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();
        for p in &self.tab.selection {
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
        let Some(target) = self
            .tab
            .entries
            .get(self.tab.selected)
            .map(|e| e.path.clone())
        else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::NoTarget).into());
            return;
        };
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let cursor = name.chars().count(); // Place the cursor at the end.
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
        // Message: a single item shows its name, multiple items show "N items (first name and others)".
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

    pub fn tree_next(&mut self) {
        if self.tab.selected + 1 < self.tab.entries.len() {
            self.tab.selected += 1;
        }
    }

    pub fn tree_prev(&mut self) {
        self.tab.selected = self.tab.selected.saturating_sub(1);
    }

    /// To the top of the tree.
    pub fn tree_first(&mut self) {
        self.tab.selected = 0;
    }

    /// To the bottom of the tree.
    pub fn tree_last(&mut self) {
        self.tab.selected = self.tab.entries.len().saturating_sub(1);
    }

    /// Page the tree by one (dir: +1=next / -1=previous). Overlaps one row to keep context.
    pub fn tree_page(&mut self, dir: i32) {
        let page = self.tab.tree_viewport.saturating_sub(1).max(1) as i32;
        self.tree_move(dir * page);
    }

    /// Half-page the tree (vim: Ctrl-d/Ctrl-u, less: d/u).
    pub fn tree_half_page(&mut self, dir: i32) {
        let half = (self.tab.tree_viewport / 2).max(1) as i32;
        self.tree_move(dir * half);
    }

    /// Move the selection by delta rows and clamp it to [0, end].
    fn tree_move(&mut self, delta: i32) {
        self.tab.selected = clamp_cursor(self.tab.selected, delta, self.tab.entries.len());
    }

    /// If a directory, toggle expansion; if a file, transition to Preview.
    /// While filtering (a flat result list), expand-toggle is meaningless, so behave the same as `tree_descend`.
    pub fn tree_activate(&mut self) -> Result<()> {
        if self.tab.tree_filter.is_some() || self.tab.changed_filter {
            return self.tree_descend();
        }
        let Some(entry) = self.tab.entries.get(self.tab.selected).cloned() else {
            return Ok(());
        };
        if entry.is_dir {
            // Toggle expanded and rebuild
            if let Some(e) = self.tab.entries.get_mut(self.tab.selected) {
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
        self.tab.selection.clear();
        self.tab.visual_anchor = None;
        self.clear_filter_state();
        self.search_clear();
    }

    /// Move to the parent directory (raise the root). For `h`. While filtering, first clear the filter (the normal tree of the current root).
    pub fn tree_leave(&mut self) -> Result<()> {
        if self.tab.tree_filter.is_some() {
            self.filter_clear();
            return Ok(());
        }
        if self.tab.changed_filter {
            self.toggle_changed_filter(); // turn it back OFF (back to the normal tree)
            return Ok(());
        }
        if let Some(parent) = self.tab.root.parent().map(Path::to_path_buf) {
            self.clear_for_root_change();
            self.tab.root = parent;
            self.tab.entries.clear();
            self.tab.selected = 0;
            self.rebuild_tree()?;
        }
        Ok(())
    }

    /// Descend into the selected directory (make that directory the new root). For `l`.
    /// Symmetric to `h` (to the parent). If a file, transition to Preview.
    /// Works on filter results too: directory → move there and clear the filter / file → preview.
    pub fn tree_descend(&mut self) -> Result<()> {
        let Some(entry) = self.tab.entries.get(self.tab.selected).cloned() else {
            return Ok(());
        };
        let was_filtering = self.tab.tree_filter.is_some();
        if entry.is_dir {
            // Since root changes, discard the old root's selection/visual/filter/search (do not carry them over).
            self.clear_for_root_change();
            self.tab.root = entry.path;
            self.tab.entries.clear();
            self.tab.selected = 0;
            self.rebuild_tree()?;
        } else {
            // A file previews while keeping the filter (returning restores the result list).
            let _ = was_filtering;
            self.enter_preview(&entry.path);
        }
        Ok(())
    }

    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.tab.show_hidden = !self.tab.show_hidden;
        // Which file the cursor is on, captured **before** the rebuild below replaces `entries` with
        // the unfiltered listing — same hazard, and same ordering, as `refresh_fs_inner`. Goes
        // through `visibility_change_anchor` (not a plain `filter_anchor` read) so that, while `C`
        // is up, an already-published still-pending anchor from a deferred rebuild is honored
        // rather than clobbered by a read of `entries` in the wrong (whole-tree) state — see
        // `changed_filter_anchor`.
        let anchor = self.visibility_change_anchor();
        // Keep the tree's Err (a rebuild can fail transiently) but don't let it skip the filter
        // work below, which depends on the pool rather than on the new tree — same reasoning as
        // `refresh_fs_inner`.
        let tree = self.rebuild_tree();
        // `rebuild_tree` puts the *whole* tree back into `entries`, so while a filter is up the
        // screen would go from filtered results to a plain directory listing under a heading that
        // still says it is filtering. Re-apply it. The pool also has to be collected again: it was
        // walked with the previous `show_hidden`, so without this `.` could reveal dotfiles in the
        // tree that `/` still refused to find.
        self.refilter_after_visibility_change(anchor);
        tree
    }

    /// The diff lines for a GitDiff preview of `path`. `follow` = this is a follow-opened diff, so
    /// (unless toggled to full) show the diff since follow-start; otherwise the full git diff. The
    /// single decision point shared by `follow_jump` (initial open) and `git_diff_lines` (recompute
    /// on scroll/FS-refresh/`n`/`N`), so every path stays consistent.
    pub(super) fn compute_gitdiff_lines(
        &self,
        path: &Path,
        follow: bool,
    ) -> Vec<crate::git::DiffLine> {
        #[cfg(feature = "git")]
        if follow && !self.follow_diff_full {
            if let Some(lines) = self.follow_baseline_diff(path) {
                return lines;
            }
        }
        let _ = follow;
        crate::git::file_diff(&self.tab.root, path)
    }

    fn enter_preview(&mut self, path: &Path) {
        // Is this re-entering the same file (returning via `q` from a full-screen fence)? If so,
        // keep the inline-image cache warm: the key is content-hash for a fence / path for an
        // image, so it never goes stale, and discarding it would restart every fence from Loading
        // = the one render pass with the collapsed layout would clamp the restored scroll/focus,
        // breaking the "return to where you were" promise.
        let same_file = self.tab.preview_path.as_deref() == Some(path);
        // Moving to a new preview target on this tab: any delegated-command output left over from
        // the *previous* target (which might not even be a Command kind — deleting it here, rather
        // than only from apply_payload's CommandText handler, is what covers "left a command
        // preview for something else entirely") is done for.
        self.clear_command_out();
        self.tab.preview_path = Some(path.to_path_buf());
        // Resolve the preview kind via config. Unsupported kinds become CanNotPreview.
        //
        // Guarded by `is_previewable` **before** `resolve_preview` even runs: every entry point
        // that opens a preview (tree activate/descend, paste-jump, bookmark jump, Ctrl-n/Ctrl-p
        // file paging, follow, session restore, a Markdown link, ...) funnels through this one
        // function, and every downstream opener (media load / the windowed reader / table parse /
        // the render side's raw-text fallbacks) dispatches purely on the resolved `PreviewKind` —
        // so forcing non-regular files (FIFOs, character/block devices, ...) to `CanNotPreview`
        // right here closes every `File::open` path in the one place they all share. This has to
        // happen before `resolve_preview`, not just around the Text/Code windowed reader, because
        // `resolve_preview` itself can already open the file to sniff it (a mime-glob rule match
        // via `infer::get_from_path`, or the no-rule-matched fallback via
        // `text::is_probably_text`) — a `File::open` on a FIFO with no writer on the other end
        // blocks forever (POSIX `open(2)`), and this all runs on the key-processing thread, so an
        // unguarded open here froze the whole UI (not even `q`/`Ctrl-C` got processed again).
        let kind = if crate::preview::is_previewable(path) {
            self.cfg.resolve_preview(path)
        } else {
            PreviewKind::can_not_preview(path)
        };
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        // Reset image state every time. SVG/GIF start loading on a separate thread (doesn't block the UI).
        self.clear_image();
        // For a PDF, get the page count first (hayro-syntax, pure Rust, no external process, ~a
        // few ms). Now that `hayro` is the first-choice renderer, it can draw any page without an
        // external tool, so "can count ⟹ can draw" holds almost universally (there is no
        // "first-page-only" constraint like qlmanage/sips has) — the navigation suppression that
        // used to live here via `arbitrary_page_renderer_available()` (whether poppler was present)
        // has been removed.
        // `[external] pdf` doesn't affect getting the page count either: `page_count` is a pure
        // Rust read that never launches an external process, so it keeps working the same whether
        // `pdf=false` (= never launch an external rendering tool) or not (an external tool is
        // only involved when `render_page` degrades to its fallback).
        if matches!(kind, PreviewKind::Pdf(_)) {
            self.tab.pdf_pages = crate::preview::pdf::page_count(path);
        }
        // Set `preview_kind` **before** `start_media_load`, not after: the sync-fallback media path
        // (no media_tx — tests, or a picker-less terminal) runs `apply_payload` synchronously inside
        // that call, and a text-mode `PreviewKind::Command`'s `apply_payload` handler calls
        // `setup_windowed`, which decides whether to open the windowed reader by reading
        // `self.tab.preview_kind` — it must already be the new kind, not whatever was showing before.
        self.tab.preview_kind = Some(kind.clone());
        self.start_media_load(&kind, path);
        self.tab.fence_return = None; // A normal preview transition means the fence-return info is no longer needed
        self.tab.fence_zoom = 1.0;
        self.tab.fence_center = (0.5, 0.5);
        self.tab.md_raw = false; // A new file starts in decorated display (Markdown/Mermaid). `R` switches to raw.
        self.md_cache = None; // Switching to a different file: invalidate the decoration cache
        self.md_items.clear();
        if !same_file {
            self.md_image_cache.clear(); // A different file: also discard the inline-image cache
        }
        self.tab.focused_item = None;
        self.outline_open = false; // A new preview closes the outline overlay
        self.table_cell_open = false; // Likewise closes the table cell full-text popup
        self.details_open.clear(); // <details> open/closed state is per document = reset on a different file
        self.tab.preview_search = None;
        self.tab.search_input = None;
        self.tab.search_matches.clear();
        self.table_search_hits.clear();
        self.tab.search_idx = 0;
        self.setup_windowed(); // Switches to less-style windowed reading for a large Code/Text
                               // Reset the windowed preview's 2D caret/selection to the start.
        self.tab.preview_cursor_line = 0;
        self.tab.preview_cursor_col = 0;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        // Also fold up the tree's range-selection (Visual) mode: `is_visual()` looks only at
        // whether `tab.visual_anchor` is set, not at `tab.mode`, so without folding it here,
        // `surface()` would keep returning Visual even after transitioning to full-screen preview
        // — what's visible is the preview, but keys would flow into the tree's Visual keymap
        // (e.g. `j` would move the tree's cursor). This guarantees the invariant here: "entered
        // full-screen preview = the tree's range selection has ended".
        //
        // Why discard (exit_visual_cancel) rather than commit (exit_visual_commit): committing via
        // `commit_visual_if_needed` has semantics dedicated to an **explicit select → act**
        // workflow like "Space→operation", and the caller that needs it (e.g. `P`=PasteJump) calls
        // it explicitly itself (see main.rs). This spot is a generic, caller-agnostic safety net —
        // closer to an "interruption" from unintentionally transitioning to a completely different
        // screen (the same idea as `Esc` discarding the in-progress range while leaving an
        // already-committed selection alone). Unconditionally committing the in-progress range to
        // the selection here could produce the side effect of an unrecognized file ending up
        // selected when returning to the tree via some unintended path (e.g. a future new
        // transition).
        if self.is_visual() {
            self.exit_visual_cancel();
        }
        // CSV/TSV/archives are parsed into a table. Reset the cursor/scroll to the start before loading.
        self.tab.table_cur_row = 0;
        self.tab.table_cur_col = 0;
        self.tab.table_top_row = 0;
        self.tab.table_left_col = 0;
        self.load_table();
        self.tab.mode = Mode::Preview;
    }

    /// Code/Text previews use less-style windowed reading **regardless of size**.
    /// This avoids whole-file highlighting (1.3s/debug for 1600 lines) and reads/colors only the visible window (tens of lines), so
    /// opening is instant even for small files and behavior is symmetric with huge files. Images/md/mermaid are excluded
    /// (md/mermaid need their whole structure understood and are usually small).
    fn setup_windowed(&mut self) {
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        // Determine whether we're in the heavy-highlight-wait state: if it's Code, highlighting
        // is enabled, and the grammar isn't compiled yet (cold), a loading display / staged
        // display is needed just for the first time. If already warm, it colors instantly from the start.
        self.hl_pending = self.cfg.ui.syntax_highlight
            && matches!(self.tab.preview_kind, Some(PreviewKind::Code(_)))
            && !crate::preview::code::is_ext_warm(self.current_preview_ext());
        self.hl_warming = false;
        // Code/Text are always windowed. Markdown/Mermaid become windowed only in raw display
        // (`R`) = read the plain source with matching line/column so the 2D caret selection works as-is.
        // A text-mode delegated command (`PreviewKind::Command` whose `render_as` isn't "image") is
        // windowed too, but only once its generated output has actually arrived (`tab.command_out`
        // — set by `apply_payload`'s `CommandText` handling): before that, staying non-windowed lets
        // the render side's media-loading / `[can not preview]` branches show instead of opening a
        // reader onto a file that doesn't exist yet (or, on a reload, a briefly-stale one — see
        // `windowed_src`'s doc comment).
        let is_text_command = self.tab.command_out.is_some()
            && matches!(
                self.tab.preview_kind,
                Some(PreviewKind::Command { ref render_as, .. }) if render_as.as_deref() != Some("image")
            );
        let windowed_kind = matches!(
            self.tab.preview_kind,
            Some(PreviewKind::Code(_)) | Some(PreviewKind::Text(_))
        ) || self.is_raw_source()
            || is_text_command;
        if !windowed_kind {
            return;
        }
        let Some(path) = self.windowed_src().map(Path::to_path_buf) else {
            return;
        };
        if let Ok(w) = crate::preview::window::FileWindow::open(&path) {
            self.preview_win = Some(w);
        }
    }

    /// Extension of the current preview target file (empty string if none). Used for the highlight-warm check, etc.
    pub fn current_preview_ext(&self) -> &str {
        self.tab
            .preview_path
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
            if let Some(path) = self.tab.preview_path.clone() {
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
        // A file operation is something the user is waiting on right now, so put it first (an always-visible, standalone label).
        if self.fileop_pending.is_some() {
            v.push(crate::i18n::Msg::BusyFileOp);
        }
        // A git write is the same class of thing: the user pressed `s`/`c`/Enter and is waiting on
        // it. It comes before the scan label below so a `git commit` stuck in a pre-commit hook is
        // never mistaken for the routine background status scan.
        if self.git_op_running() {
            v.push(crate::i18n::Msg::BusyGitOp);
        }
        // Both the ignored set and status are "git scan in progress" = represented by the same label (shown if either is running).
        if self.git_ignored_pending.is_some() || self.git_status_pending.is_some() {
            v.push(crate::i18n::Msg::BusyGitScan);
        }
        // The `/` filter's population walk. Unlike the git scans this one is visible in the results
        // themselves (the list keeps filling in as the user types), so the label exists to explain
        // *why* — without it a partially-populated pool just looks like missing files.
        //
        // Only a walk whose result will actually land in the pool on screen (generation still
        // current) counts. A superseded one is still burning a thread, but announcing it here would
        // put "scanning files" on a tab that is not filtering at all and whose list will never
        // change because of it — which is exactly what `t` used to do (`tab_new` left the previous
        // tab's walk marked in flight).
        if self.filter_pool_scanning_current() {
            v.push(crate::i18n::Msg::BusyFilterScan);
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
    /// (at least one background job in flight, and either the config is on or the job is a file
    /// operation). The run loop only schedules animation ticks while this is true, so idle CPU
    /// stays at zero.
    ///
    /// A running file operation is shown **even with `ui.busy_indicator = false`**: unlike the
    /// other jobs (git scans, decodes, warm-ups — background bookkeeping the user did not ask
    /// for), a copy/move/delete is the very thing the user is waiting on. Hiding it would leave a
    /// multi-minute copy with no feedback at all, which is worse than the old synchronous version
    /// that at least froze visibly.
    ///
    /// A running **git write** is forced on for the same reason, and one stronger: a file operation
    /// at least makes the tree visibly grow as it goes, whereas nothing on screen moves while a
    /// `pre-commit` hook runs — with the indicator suppressed there would be no evidence at all that
    /// the commit is still going, and the unchanged Git view would read as "the key did nothing".
    pub fn busy_indicator_active(&self) -> bool {
        if self.busy_jobs().is_empty() {
            return false;
        }
        self.cfg.ui.busy_indicator || self.fileop_pending.is_some() || self.git_op_running()
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
    /// In Preview the editor is asked to open at the on-screen position (see `preview_edit_line`).
    pub fn request_edit(&mut self) {
        let target = match self.tab.mode {
            Mode::Tree => match self.tab.entries.get(self.tab.selected) {
                Some(e) if e.is_dir => {
                    self.flash = Some(tr(self.lang, crate::i18n::Msg::CannotEditDirectory).into());
                    return;
                }
                Some(e) => Some(e.path.clone()),
                None => None,
            },
            Mode::Preview => self.tab.preview_path.clone(),
        };
        match target {
            Some(p) => self.pending_edit = Some((p, self.preview_edit_line())),
            None => self.flash = Some(tr(self.lang, crate::i18n::Msg::NoFileToEdit).into()),
        }
    }

    /// The 1-based file line to open the external editor at, matching the on-screen position.
    /// - Windowed preview (plain text / code / raw-Markdown via `R`): the exact caret line.
    /// - Rendered Markdown: an **approximate** source line for the top of the current view. Reflow means
    ///   there is no exact scroll-row → source-line mapping, so the top-of-view fraction through the wrapped
    ///   output (`preview_scroll / md_view_rows`) is mapped to that fraction of the source. This lands the
    ///   editor near where you were reading rather than at the top (block-structured docs land on the
    ///   section being read; see `preview_edit_line` verification).
    /// - Otherwise (tree edits, Mermaid, images): None (open at the top).
    fn preview_edit_line(&self) -> Option<usize> {
        if self.tab.mode != Mode::Preview {
            return None;
        }
        if self.is_windowed() {
            return Some(self.tab.preview_cursor_line + 1);
        }
        // Rendered Markdown: reflow means there is no exact scroll-row → source-line mapping. Anchor on a
        // logical line (a Tab-focused item if one is visible, else the top of the view), estimate its
        // source line proportionally, then refine by content (see md_content_anchor_line).
        if matches!(self.tab.preview_kind, Some(PreviewKind::Markdown(_))) {
            if let Some(c) = &self.md_cache {
                let same = self.tab.preview_path.as_ref().is_some_and(|p| *p == c.path);
                if same && c.src_lines > 0 && self.md_view_rows > 0 {
                    // Prefer the on-screen Tab focus (the visible "cursor"); otherwise the top of view.
                    let (logical, row) = match self.md_focused_visible_line() {
                        Some(hit) => hit,
                        None => (
                            self.md_top_logical_line().unwrap_or(0),
                            self.tab.preview_scroll as usize,
                        ),
                    };
                    let est = (row * c.src_lines / self.md_view_rows).min(c.src_lines - 1);
                    return Some(self.md_content_anchor_line(logical, est).unwrap_or(est) + 1);
                }
            }
        }
        None
    }

    /// The (decorated logical line, top display row) of the Tab-focused Markdown item, but only when it
    /// is currently on screen — so `e` opens where the visible cursor is. None if nothing is focused or
    /// the focus has been scrolled out of view (then `e` falls back to the top of the view).
    fn md_focused_visible_line(&self) -> Option<(usize, usize)> {
        let item = self.tab.focused_item.and_then(|i| self.md_items.get(i))?;
        let (row, h) = self.md_visual_span(item.line);
        let scroll = self.tab.preview_scroll as usize;
        let vh = self.tab.preview_viewport.max(1) as usize;
        (row < scroll + vh && row + h > scroll).then_some((item.line, row))
    }

    /// The 0-based decorated (logical) line at the top of the current Markdown view, resolving wrapping
    /// so it matches what the renderer draws (`preview_scroll` is in post-wrap display rows).
    fn md_top_logical_line(&self) -> Option<usize> {
        let c = self.md_cache.as_ref()?;
        if c.lines.is_empty() {
            return None;
        }
        let scroll = self.tab.preview_scroll as usize;
        if !self.cfg.ui.wrap || c.width == 0 || c.row_prefix.len() != c.lines.len() + 1 {
            return Some(scroll.min(c.lines.len() - 1));
        }
        // prefix is each line's starting display row (monotonically increasing). The line
        // containing scroll = pp(p <= scroll) - 1.
        let i = c
            .row_prefix
            .partition_point(|&p| p <= scroll)
            .saturating_sub(1);
        Some(i.min(c.lines.len() - 1))
    }

    /// Refine the proportional editor-open estimate for rendered Markdown by content: take the longest
    /// visible span of decorated logical line `logical` (a single span carries no Markdown markers, so its
    /// text is a verbatim substring of the source) and find the source line containing it, closest to
    /// `est`. Returns a 0-based source line, or None to fall back to `est`. Re-reads the file so it
    /// matches what the editor will open (an agent may have edited it since the preview was built).
    fn md_content_anchor_line(&self, logical: usize, est: usize) -> Option<usize> {
        use ratatui::style::Modifier;
        let c = self.md_cache.as_ref()?;
        let line = c.lines.get(logical)?;
        // Longest non-hidden, non-blank span (hidden spans carry link/URL targets, not on-screen text).
        let key = line
            .spans
            .iter()
            .filter(|s| !s.style.add_modifier.contains(Modifier::HIDDEN))
            .map(|s| s.content.trim())
            .filter(|t| !t.is_empty())
            .max_by_key(|t| t.chars().count())?;
        if key.chars().count() < 4 {
            return None;
        }
        let content = crate::preview::text::load(c.path.as_path()).ok()?;
        let mut best: Option<usize> = None;
        let mut best_d = usize::MAX;
        for (i, sl) in content.lines.iter().enumerate() {
            if sl.contains(key) {
                let d = i.abs_diff(est);
                if d < best_d {
                    best_d = d;
                    best = Some(i);
                }
            }
        }
        best
    }

    /// Taken by the run loop: if set, return the editor-launch target path (+ line) and clear it.
    pub fn take_pending_edit(&mut self) -> Option<(PathBuf, Option<usize>)> {
        self.pending_edit.take()
    }

    /// `O`: request launching an external git tool (lazygit, etc.) (the run loop picks it up, suspends, then launches).
    /// No-op (flash) when `[external] git_tool = false`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub fn launch_git_tool(&mut self) {
        if !self.cfg.external.git_tool {
            self.flash =
                Some(crate::i18n::tr(self.lang, crate::i18n::Msg::ExternalGitToolDisabled).into());
            return;
        }
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
        if matches!(self.tab.mode, Mode::Preview) {
            self.setup_windowed();
            self.reload_media_if_changed();
            // CSV/TSV/archive content can change from an external edit. Re-parse and clamp the
            // cursor within range (preserving its position).
            if matches!(
                self.tab.preview_kind,
                Some(PreviewKind::Table { .. }) | Some(PreviewKind::Archive { .. })
            ) {
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
        let is_media = match &self.tab.preview_kind {
            Some(kind) => self.kind_loads_media(kind),
            None => false,
        };
        if !is_media {
            return;
        }
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        let Some(kind) = self.tab.preview_kind.clone() else {
            return;
        };
        if file_mtime(&path) == self.preview_media_mtime {
            return; // the target file is unchanged → avoid a wasted re-decode / external-tool run
        }
        // Preserve the display state (zoom/center/page) and reload.
        let (zoom, center, page) = (
            self.tab.image_zoom,
            self.tab.image_center,
            self.tab.pdf_page,
        );
        self.clear_image(); // reset zoom/center/page/media generation
        if matches!(kind, PreviewKind::Pdf(_)) {
            // See the comment on the enter_preview side for the gating rationale (page_count has
            // no external process, and hayro can draw any page, so there's no navigation
            // suppression via arbitrary_page_renderer_available).
            self.tab.pdf_pages = crate::preview::pdf::page_count(&path);
            self.tab.pdf_page = page.clamp(1, self.tab.pdf_pages.unwrap_or(1).max(1));
        }
        self.start_media_load(&kind, &path); // also updates preview_media_mtime
        self.tab.image_zoom = zoom;
        self.tab.image_center = center;
    }

    /// The directory to watch **in addition** to the tree root so external (agent) edits to the
    /// currently shown file are still detected. Returns the shown file's parent directory **only when
    /// that file lives outside the watched root**: a global-bookmark target, or the repo-wide git view
    /// when the root is a repo subdirectory. Files under the root are already covered by the recursive
    /// root watch (so this returns `None` for them, and whenever no file is shown / not in Preview).
    ///
    /// This closes the gap where the FSEvents watcher only covers `app.tab.root`: an out-of-root preview
    /// or diff never received change events, so an AI/external edit left it stale (the user's own `e`
    /// edit still reloaded via the editor-return path — hence "my edits show, the AI's don't").
    /// Consumed by the run loop in `main.rs`, which adds a non-recursive watch on this directory.
    pub fn out_of_root_watch_dir(&self) -> Option<PathBuf> {
        if !matches!(self.tab.mode, Mode::Preview) {
            return None;
        }
        let p = self.tab.preview_path.as_ref()?;
        if p.starts_with(&self.tab.root) {
            return None; // already covered by the recursive root watch
        }
        p.parent().map(|d| d.to_path_buf())
    }

    /// The git directory to watch **non-recursively** when it is not already covered by the
    /// recursive root watch. Without this, an external git op that touches only the git directory
    /// (a commit of already-staged files, an external checkout) fires no event under root, so the
    /// cached `git status` / branch would linger stale — the per-workdir status cache no longer
    /// re-verifies on `h`/`l`. Two cases land here:
    /// - **root is a subdirectory of the repo**: the parent `.git` sits above `root`, outside the
    ///   recursive watch.
    /// - **root is a linked worktree** (`git worktree add`): that worktree's own `HEAD`/`index` live
    ///   under `<main-repo>/.git/worktrees/<name>/`, not under the worktree's own checkout at all —
    ///   so *no* event ever reaches the recursive root watch, no matter how deep the tree goes.
    ///   ([`crate::git::git_dir`] returns this location; [`crate::git::workdir`] would instead
    ///   return the worktree's own root, which is exactly what makes this case easy to miss.)
    ///
    /// `HEAD`/`index` are direct children of the git directory, so a non-recursive watch catches
    /// the signals that change status/branch. Returns None when the git directory is already under
    /// `root` (nothing extra to watch) or outside any repo. konoma's own reads are lock-free
    /// (`--no-optional-locks`), so this does not create a self-feedback loop.
    pub fn git_dir_watch(&self) -> Option<PathBuf> {
        let gd = crate::git::git_dir(&self.tab.root)?;
        let root = self
            .tab
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.tab.root.clone());
        // Watch it only when it is NOT already covered by the recursive root watch.
        (!gd.starts_with(&root)).then_some(gd)
    }

    /// Whether in windowed (large file) mode. Used for the render and scroll branching.
    pub fn is_windowed(&self) -> bool {
        self.preview_win.is_some()
    }

    /// Index of the focused Markdown item (link/checkbox), if any. Read-only query used by the
    /// E2E simulation tests to assert Tab/⇧Tab focus movement.
    #[cfg(test)]
    pub fn focused_item(&self) -> Option<usize> {
        self.tab.focused_item
    }

    /// The windowed preview's current top line (0-based, the first line shown on screen).
    /// Read-only query used by the E2E simulation tests to pin down page/half-page scrolling
    /// (whether the *window*, not just the caret, actually moved).
    #[cfg(test)]
    pub fn preview_top_line(&self) -> usize {
        self.tab.preview_top_line
    }

    /// The link targets in the current Markdown preview's `md_items`, in document order (E2E: assert
    /// exactly which URLs became focusable links — e.g. that a URL inside code did NOT).
    #[cfg(test)]
    pub fn md_link_targets(&self) -> Vec<String> {
        self.md_items
            .iter()
            .filter_map(|it| match &it.kind {
                MdItemKind::Link { target } => Some(target.clone()),
                _ => None,
            })
            .collect()
    }

    /// Whether the current preview is a Markdown/Mermaid file shown as its raw source (`R` toggled on).
    /// Such a preview is windowed like a code file, so the 2D caret selection/copy applies.
    pub fn is_raw_source(&self) -> bool {
        self.tab.md_raw
            && matches!(
                self.tab.preview_kind,
                Some(PreviewKind::Markdown(_)) | Some(PreviewKind::Mermaid(_))
            )
    }

    /// Whether the current preview is Markdown/Mermaid (has a decorated render, so `R` can toggle raw source).
    pub fn is_decorated_kind(&self) -> bool {
        matches!(
            self.tab.preview_kind,
            Some(PreviewKind::Markdown(_)) | Some(PreviewKind::Mermaid(_))
        )
    }

    /// Whether raw source view is currently on (for the footer/title indicator).
    pub fn is_md_raw(&self) -> bool {
        self.tab.md_raw
    }

    /// `R`: toggle a Markdown/Mermaid preview between its decorated render and raw source. No-op for other kinds.
    /// Rebuilds the windowed reader and resets the caret/scroll/selection so the new view starts at the top.
    pub fn toggle_md_raw(&mut self) {
        if !self.is_decorated_kind() {
            return;
        }
        self.tab.md_raw = !self.tab.md_raw;
        // A view switch = start from the top. Rebuild the windowed reader (raw) / decoration (rendered).
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_cursor_line = 0;
        self.tab.preview_cursor_col = 0;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        self.md_cache = None;
        self.setup_windowed();
    }

    /// Page the preview to the next/previous **file** in tree display order (`dir`=+1/-1, wraps
    /// at the ends). Directories are skipped; the tree cursor follows the shown file, so leaving
    /// the preview lands where you are. The anchor is the previewed file's tree position, falling
    /// back to the cursor when the previewed file is not in the tree (e.g. a global-bookmark
    /// target outside the root). With no other file to go to, this is a no-op.
    pub fn preview_jump_file(&mut self, dir: i32) {
        if self.tab.mode != Mode::Preview || self.tab.entries.is_empty() {
            return;
        }
        let anchor = self
            .tab
            .preview_path
            .as_ref()
            .and_then(|p| self.tab.entries.iter().position(|e| e.path == *p))
            .unwrap_or_else(|| self.tab.selected.min(self.tab.entries.len() - 1));
        let n = self.tab.entries.len() as i64;
        let mut i = anchor as i64;
        for _ in 0..n {
            i = (i + dir as i64).rem_euclid(n);
            if i as usize == anchor {
                break; // wrapped all the way around = there is no other file
            }
            if !self.tab.entries[i as usize].is_dir {
                self.tab.selected = i as usize;
                let path = self.tab.entries[i as usize].path.clone();
                self.enter_preview(&path);
                return;
            }
        }
    }

    pub fn back_to_tree(&mut self) {
        // `q` from a full-screen fence display returns not "to the tree" but "back to the original
        // Markdown view" (the same "return to where you came from" convention as git diff's
        // came_from_git_view). Scroll/focus are also restored.
        if matches!(self.tab.preview_kind, Some(PreviewKind::MermaidFence(_))) {
            if let Some(md) = self.tab.preview_path.clone() {
                let ret = self.tab.fence_return.take();
                self.enter_preview(&md);
                if let Some((scroll, focus)) = ret {
                    self.tab.preview_scroll = scroll;
                    self.tab.focused_item = focus;
                }
                return;
            }
        }
        self.tab.mode = Mode::Tree;
        // Re-verify git status when the tree becomes visible again. FSEvents is inherently lossy
        // (bursts get coalesced, and an external commit can arrive as `.git/*.lock`-only churn), so
        // an external git op during Preview may have left `git_status` stale. Invalidating here makes
        // the next render's `refresh_git_if_needed` refetch the cheap statuses+branch (the heavy
        // `ignored` set is kept, since the repo is unchanged) — so returning to the tree always shows
        // fresh markers, instead of a stale change marker until you navigate across a directory (`h`/`l`).
        // Mark dirty too, so the per-workdir status cache (reused across `h`/`l` in the same repo) is
        // bypassed for this deliberate re-verify — otherwise the stale markers would linger.
        self.git_status_for = None;
        self.git_status_dirty = true;
        // Same seam re-verification for the detail columns: their cells are cached per tree
        // generation (Phase G) and a missed fs event would otherwise pin stale size/mtime until
        // the next rebuild. Dropping here keeps them as robust as the git markers.
        self.detail_cells_cache.clear();
        self.tab.preview_path = None;
        self.tab.preview_kind = None;
        self.clear_command_out(); // release any delegated-command temp output
        self.clear_image(); // release the graphics state
        self.md_cache = None;
        self.tab.md_raw = false;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.hl_pending = false;
        self.hl_warming = false;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        self.md_items.clear();
        self.tab.focused_item = None;
        self.tab.preview_search = None;
        self.tab.search_input = None;
        self.tab.search_matches.clear();
        self.table_search_hits.clear();
        self.tab.search_idx = 0;
        self.tab.came_from_git_view = false;
        self.table_data = None;
        self.tab.table_cur_row = 0;
        self.tab.table_cur_col = 0;
        self.tab.table_top_row = 0;
        self.tab.table_left_col = 0;
        self.tab.preview_cursor_line = 0;
        self.tab.preview_cursor_col = 0;
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
    }

    /// Whether this preview kind displays through the media (image) pipeline — and therefore needs
    /// a `start_media_load` whenever its display state was dropped (tab restore / fs reload).
    /// Mermaid kinds count only in image mode; in text mode they draw via the decorated path.
    /// Remember the media currently on screen before a tab switch discards it, so switching back can
    /// reuse it. Skips animated GIFs (frames live in separate state) and anything without a source image.
    fn stash_media_cache(&mut self) {
        if !self.gif_frames.is_empty() {
            return;
        }
        let (Some(path), Some(src)) = (self.tab.preview_path.clone(), self.image_src.clone())
        else {
            return;
        };
        // Even though it's just one slot, it means "hold on to the last-seen image forever", so
        // give up and drop anything pathologically large (re-decoding is cheaper than keeping
        // hundreds of MB resident). Judge by **actual byte count**: pixel count alone would let a
        // 16-bit PNG (8B/px) or HDR/EXR (12B/px) slip past the limit and stay resident at hundreds
        // of MiB.
        const MAX_CACHED_BYTES: usize = 128 * 1024 * 1024;
        if src.as_bytes().len() > MAX_CACHED_BYTES {
            self.media_cache = None; // don't drag the old entry along with it
            return;
        }
        let meta = std::fs::metadata(&path).ok();
        self.media_cache = Some(MediaCache {
            path,
            mtime: self.preview_media_mtime,
            len: meta.map(|m| m.len()),
            page: self.tab.pdf_page.max(1),
            fence_ord: self.preview_fence_ord(),
            src,
            logical: self.image_logical,
            vector_svg: self.vector_svg.clone(),
        });
    }

    /// Release the one-slot media cache if it belongs to the preview currently on screen (the tab that
    /// is about to be closed). Without this, closing the only media tab leaves its image resident with
    /// nothing able to reuse it.
    fn drop_media_cache_for_active(&mut self) {
        let cur = self.tab.preview_path.as_deref();
        if matches!((&self.media_cache, cur), (Some(c), Some(p)) if c.path == p) {
            self.media_cache = None;
        }
        // Since this tab is being closed, don't stash the on-screen media itself either.
        if self.image_src.is_some() && cur.is_some() {
            self.media_cache = match self.media_cache.take() {
                Some(c) if Some(c.path.as_path()) != cur => Some(c),
                _ => None,
            };
        }
    }

    /// The mermaid-fence ordinal of the current preview, if it is a full-screen fence.
    fn preview_fence_ord(&self) -> Option<usize> {
        match &self.tab.preview_kind {
            Some(PreviewKind::MermaidFence(ord)) => Some(*ord),
            _ => None,
        }
    }

    /// Restore the stashed media for `path` if it is an exact match and the file has not changed since.
    /// Returns true when the caller can skip re-loading the media entirely.
    fn restore_media_cache(&mut self, path: &Path, page: u32) -> bool {
        let len = std::fs::metadata(path).ok().map(|m| m.len());
        let fence = self.preview_fence_ord();
        let hit = matches!(&self.media_cache, Some(c)
            if c.path == path
                && c.page == page
                && c.fence_ord == fence
                && c.mtime == file_mtime(path)
                && c.mtime.is_some()
                && c.len == len);
        if !hit {
            return false;
        }
        let c = self.media_cache.as_ref().expect("checked above");
        self.image_logical = c.logical;
        self.vector_svg = c.vector_svg.clone();
        self.preview_media_mtime = c.mtime;
        // The Arc's contents are shared, so the image is not duplicated. protocol was already
        // dropped by the preceding `clear_image`, and `image_crop = None` is the signal to rebuild
        // it on the next render (clear_image also handles sweeping kitty-side ghost artifacts).
        self.image_src = Some(c.src.clone());
        self.image_crop = None;
        true
    }

    fn kind_loads_media(&self, kind: &PreviewKind) -> bool {
        matches!(
            kind,
            PreviewKind::Image(_)
                | PreviewKind::Svg(_)
                | PreviewKind::Video(_)
                | PreviewKind::Pdf(_)
        ) || (matches!(kind, PreviewKind::Mermaid(_) | PreviewKind::MermaidFence(_))
            && self.mermaid_image_mode())
            // A non-detached delegated command (image or text render_as): its output is a
            // function of the source file, so a tab switch / mtime-changed reload must re-run it
            // exactly like every other media kind. `detached=true` is deliberately excluded — that
            // one launches once, from `enter_preview` only, and must never be re-triggered by a
            // tab switch (relaunching mpv every time you flip back to that tab).
            || matches!(kind, PreviewKind::Command { detached: false, .. })
    }

    /// Release and reset the image display state (protocol, source image, zoom/pan).
    fn clear_image(&mut self) {
        self.image = None;
        self.kitty_image = None;
        // Discard any in-flight kitty build from the file we are leaving (gen bump) and clear the
        // wanted geometry so the next image triggers a fresh (synchronous first) build.
        self.kitty_gen = self.kitty_gen.wrapping_add(1);
        self.kitty_want = None;
        self.kitty_shown = None;
        self.image_src = None;
        self.tab.image_zoom = 1.0;
        self.tab.image_center = (0.5, 0.5);
        self.image_crop = None;
        self.image_vis_frac = (1.0, 1.0);
        self.vector_svg = None;
        self.image_logical = None;
        self.gif_frames = Vec::new();
        self.gif_idx = 0;
        self.gif_shown_at = None;
        self.gif_protocol = None;
        self.gif_proto_key = None;
        self.tab.pdf_page = 1;
        self.tab.pdf_pages = None;
        // Advance the generation, so an old file's media result that arrives while loading a different file is treated as stale.
        self.media_gen = self.media_gen.wrapping_add(1);
        self.media_loading = false;
        self.vector_reraster_inflight = false;
        // When image state is discarded, also discard the mtime claim (an invariant). Leaving it
        // set becomes a landmine of "nothing is displayed, but mtime says up to date": that makes
        // reload_media_if_changed misjudge it as unchanged and block the reload (one of the root
        // causes behind mermaid degrading to a text diagram on tab return).
        self.preview_media_mtime = None;
        self.command_err = None;
    }

    /// Delete the current tab's delegated-command temp output (if any) and clear the reference.
    /// Called at preview-transition checkpoints — a new preview target (`enter_preview`), leaving
    /// Preview mode (`back_to_tree`), and this tab closing (`tab_close`) — so a text-mode command's
    /// `{out}`/captured-stdout scratch file doesn't accumulate on disk forever. Also called from
    /// `apply_payload`'s `CommandText` handling right before installing a freshly regenerated one
    /// (a source-file reload or a tab-switch-triggered re-run both replace, rather than reuse, the
    /// previous output).
    ///
    /// Deliberately **not** folded into `clear_image`: unlike the rest of the image state,
    /// `command_out` lives in `PerTab` (cloned into every tab snapshot) — calling this from
    /// `clear_image` while `self.tab` transiently holds a snapshot cloned from a *different* tab
    /// (as `tab_new` does, briefly, before resetting `preview_path`/`preview_kind`) would delete a
    /// file another (inactive) tab's snapshot still legitimately owns.
    fn clear_command_out(&mut self) {
        if let Some(p) = self.tab.command_out.take() {
            let _ = std::fs::remove_file(&p);
        }
    }

    /// The file the windowed (less-style) reader should actually read from. Normally the previewed
    /// file itself, but a text-mode delegated `PreviewKind::Command` reads from the **generated
    /// output** (`tab.command_out`) instead — while the preview title still shows `preview_path`
    /// unchanged (`ui/preview.rs::render_windowed` builds the title from that field, not from this
    /// one), so the temp path never leaks into the UI.
    pub fn windowed_src(&self) -> Option<&Path> {
        if matches!(self.tab.preview_kind, Some(PreviewKind::Command { .. })) {
            return self.tab.command_out.as_deref();
        }
        self.tab.preview_path.as_deref()
    }

    /// Failure reason for the current `PreviewKind::Command` attempt, if any (see `command_err`'s
    /// doc comment). Read by `ui/preview.rs`'s `[can not preview]` fallback.
    pub fn command_error(&self) -> Option<&str> {
        self.command_err.as_deref()
    }

    /// Page the text preview by one page (dir: +1=next page / -1=previous page).
    /// Overlaps one row to keep context. The upper bound is clamped at render time.
    pub fn preview_page(&mut self, dir: i32) {
        let page = self.tab.preview_viewport.saturating_sub(1).max(1) as i32;
        if self.is_windowed() {
            self.windowed_page_scroll(dir * page);
            return;
        }
        self.preview_scroll(dir * page);
    }

    /// Half-page the text preview (vim: Ctrl-d/Ctrl-u, less: d/u).
    pub fn preview_half_page(&mut self, dir: i32) {
        let half = (self.tab.preview_viewport / 2).max(1) as i32;
        if self.is_windowed() {
            self.windowed_page_scroll(dir * half);
            return;
        }
        self.preview_scroll(dir * half);
    }

    /// Windowed (Code/Text) page/half-page scroll: moves the **window** by `delta` lines and
    /// carries the caret along, preserving its on-screen row.
    ///
    /// Single-step scrolling (`j`/`k`, via `preview_scroll`→`preview_cursor_move`) intentionally
    /// moves the caret first and lets `follow_cursor` scroll the window only once the caret would
    /// leave the screen (an "always-visible cursor" model). Paging through that same path breaks
    /// down: `Ctrl-f` (dir=+1) lands the caret exactly on the last visible row (the closest
    /// `follow_cursor` gets while keeping it on screen — see its `cur >= top + vh` branch). The very
    /// next `Ctrl-b` (dir=-1) then moves the caret back up by one page, which lands it back at
    /// `top` — still inside the *current* (not-yet-scrolled) viewport — so `follow_cursor` sees
    /// nothing to do and the window doesn't move. The first upward page/half-page after a downward
    /// one is silently swallowed (and symmetrically for `Ctrl-u` after `Ctrl-d`). Paging must
    /// instead move the window directly (`win_scroll_lines`, which already clamps at both the top
    /// and the last page) and reposition the caret at the row it occupied before the move, so the
    /// caret and the window always move together on a page/half-page step.
    fn windowed_page_scroll(&mut self, delta: i32) {
        let row_in_view = self
            .tab
            .preview_cursor_line
            .saturating_sub(self.tab.preview_top_line);
        self.win_scroll_lines(delta);
        let total = self.win_total().unwrap_or(usize::MAX);
        let maxl = total.saturating_sub(1);
        self.tab.preview_cursor_line = self
            .tab
            .preview_top_line
            .saturating_add(row_in_view)
            .min(maxl);
    }

    /// To the top of the preview.
    pub fn preview_to_top(&mut self) {
        if self.is_windowed() {
            self.tab.preview_cursor_line = 0;
            self.tab.preview_byte_top = 0;
            self.tab.preview_top_line = 0;
        } else {
            self.tab.preview_scroll = 0;
        }
    }

    /// To the bottom of the preview. The actual maximum is clamped at render time, when content and screen size are known.
    /// When windowed, moves to the line-head byte of the last page (found by a backward seek, without scanning the whole file).
    /// If line numbers are ON, the bottom line number is also corrected from the total line count.
    pub fn preview_to_bottom(&mut self) {
        if !self.is_windowed() {
            self.tab.preview_scroll = u16::MAX;
            return;
        }
        let vh = self.tab.preview_viewport.max(1) as usize;
        // Always fetch the total line count (it's cached) since clamping the line cursor to the end needs it.
        let total = self.win_total();
        let cur = self.tab.preview_top_line;
        if let Some(b) = self
            .preview_win
            .as_mut()
            .map(|w| w.last_page_top(vh).unwrap_or(0))
        {
            self.tab.preview_byte_top = b;
            self.tab.preview_top_line = total.map(|t| t.saturating_sub(vh)).unwrap_or(cur);
        }
        if let Some(t) = total {
            self.tab.preview_cursor_line = t.saturating_sub(1);
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
            return false; // ignore an encode failure (does not crash)
        };
        match self.image.as_mut() {
            Some(state) => state.update_resized_protocol(resp),
            None => false,
        }
    }

    pub fn preview_scroll(&mut self, delta: i32) {
        if self.is_windowed() {
            // Windowed (Code/Text) moves the line cursor and lets the window follow it (an always-visible cursor model).
            self.preview_cursor_move(delta);
            return;
        }
        let next = self.tab.preview_scroll as i32 + delta;
        // Only the lower bound is handled here. The upper bound (the end) is clamped at render time, when content and screen size are known.
        self.tab.preview_scroll = next.max(0) as u16;
    }

    /// Move the windowed line cursor by `delta` (clamped to the file), then scroll the window to keep it visible.
    /// In visual mode the anchor stays fixed, so moving the cursor extends the selection.
    fn preview_cursor_move(&mut self, delta: i32) {
        let total = self.win_total().unwrap_or(usize::MAX);
        let maxl = total.saturating_sub(1) as i64;
        let next =
            (self.tab.preview_cursor_line as i64 + delta as i64).clamp(0, maxl.max(0)) as usize;
        self.tab.preview_cursor_line = next;
        self.follow_cursor();
    }

    /// Scroll the window (byte-based) just enough that the line cursor is on screen.
    fn follow_cursor(&mut self) {
        let vh = self.tab.preview_viewport.max(1) as usize;
        let top = self.tab.preview_top_line;
        let cur = self.tab.preview_cursor_line;
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
        let vh = self.tab.preview_viewport.max(1) as usize;
        let top = self.tab.preview_byte_top;
        let line = self.tab.preview_top_line;
        // Use it for end-clamping when line numbers are ON, or when the total line count is already cached (for the line cursor).
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
                    // Reached the last page: derive the line number from the total line count (or simply add, if unavailable).
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
            self.tab.preview_byte_top = nt;
            self.tab.preview_top_line = nl;
        }
    }

    /// For windowed display, read the current window (from the top byte for the height), highlight Code with syntect
    /// / leave Text plain, and return it as a list of ratatui Lines. Cached by (path, top byte, height),
    /// re-read only on scroll/resize. If the top exceeds the last page, it is clamped.
    pub fn windowed_lines(&mut self, height: u16, width: u16) -> Vec<Line<'static>> {
        let _ = width; // The line content doesn't depend on width (wrapping happens on the render side). Accepted for future use.
        let h = height.max(1) as usize;
        // If a resize etc. put it past the end, clamp it (also correct the line number from the total line count).
        let top0 = self.tab.preview_byte_top;
        let maxt = self
            .preview_win
            .as_mut()
            .and_then(|w| w.last_page_top(h).ok());
        if let Some(maxt) = maxt {
            if top0 > maxt {
                self.tab.preview_byte_top = maxt;
                // Even with the line-number gutter OFF, the current search match (orange)
                // references abs=preview_top_line+i, so when clamping to the last page, always
                // correct preview_top_line from the total line count regardless of line_numbers (#5).
                if let Some(t) = self.win_total() {
                    self.tab.preview_top_line = t.saturating_sub(h);
                }
            }
        }
        let top = self.tab.preview_byte_top;
        let path = self.tab.preview_path.clone().unwrap_or_default();
        // Whether to attach syntax: plain if highlighting is disabled (off-switch) / progressive is still waiting (hl_pending).
        // Code is always a target. Text is a target only "when an existing grammar can be resolved
        // from the extension/filename" = colors config files like .bashrc/Makefile/Dockerfile while
        // keeping truly plain text uncolored.
        // Only while progressive is waiting is it "plain text before the swap", so it isn't cached (it's replaced by the colored version later).
        let syntax_kind = match &self.tab.preview_kind {
            Some(PreviewKind::Code(_)) => true,
            Some(PreviewKind::Text(p)) => crate::preview::code::has_named_syntax(p),
            // Markdown/Mermaid in raw source display (`R`) is colored by the file's grammar (.md→Markdown, etc.).
            Some(PreviewKind::Markdown(p)) | Some(PreviewKind::Mermaid(p)) if self.tab.md_raw => {
                crate::preview::code::has_named_syntax(p)
            }
            _ => false,
        };
        let want_syntax = self.cfg.ui.syntax_highlight
            && syntax_kind
            && (!self.hl_pending || self.loading_is_indicator());
        // Only the plain text while progressive is waiting is left uncached (since it gets
        // replaced by the colored version later). Everything else is cached by (path, top, height)
        // even as plain text = never re-reads the file every frame. `styled` is part of the key so
        // the colored and plain versions are never confused.
        let cacheable = want_syntax || !(self.cfg.ui.syntax_highlight && syntax_kind);
        let mut content: Vec<Line<'static>> = if cacheable {
            let hit = matches!(
                &self.win_cache,
                Some(c) if c.path == path && c.byte_top == top && c.height == height
                    && c.styled == want_syntax
            );
            if !hit {
                let raw = self
                    .preview_win
                    .as_mut()
                    .and_then(|w| w.read_lines(top, h).ok())
                    .unwrap_or_default();
                let lines = if want_syntax {
                    let src = raw.join("\n");
                    crate::preview::code::highlight(&src, &path, &self.cfg.ui.theme.code_theme)
                } else {
                    raw.into_iter().map(Line::from).collect()
                };
                self.win_cache = Some(WinCache {
                    path,
                    byte_top: top,
                    height,
                    styled: want_syntax,
                    lines,
                });
            }
            self.win_cache
                .as_ref()
                .map(|c| c.lines.clone())
                .unwrap_or_default()
        } else {
            // Plain text while progressive is waiting (not cached = replaced once coloring completes).
            let raw = self
                .preview_win
                .as_mut()
                .and_then(|w| w.read_lines(top, h).ok())
                .unwrap_or_default();
            raw.into_iter().map(Line::from).collect()
        };
        // Expand tabs into a visible marker (→) + tab-stop spaces (since terminals don't align tabs to columns).
        // Done before the line-number gutter/search highlight = column tracking is based on the body's start (column 0). Only the window's visible lines = cheap.
        content = crate::preview::code::expand_tabs(content, self.cfg.ui.tab_width);
        // While searching, highlight the match locations (query) (before the gutter = only the body is highlighted).
        // Only the current occurrence (search_idx) is orange, the rest are yellow. Visible row i's absolute line = preview_top_line + i.
        // If that line is the current occurrence's line, only that occurrence's column (col) turns orange (only one even if the same line has several).
        if let Some(q) = self.tab.preview_search.clone() {
            // Find the current match's (search_idx) line and its "occurrence rank within the line" (0-based).
            // Since tab expansion shifts the byte sequence, identify the current match by occurrence
            // rank rather than column (byte position) (#14). Matches on the same line are consecutive and column-ascending within search_matches.
            let (cur_line, cur_rank) =
                match self.tab.search_matches.get(self.tab.search_idx).copied() {
                    Some((_, line, _)) => {
                        let rank = self.tab.search_matches[..self.tab.search_idx]
                            .iter()
                            .filter(|(_, l, _)| *l == line)
                            .count();
                        (Some(line), Some(rank))
                    }
                    None => (None, None),
                };
            let top_line = self.tab.preview_top_line;
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
        let top_line = self.tab.preview_top_line;
        // Clamp the 2D caret column to the visible cursor line's actual length and write it back (correcting `l`/`$` overrun).
        // Since the caret line is always visible via viewport follow, its actual length can be obtained from content here.
        if self.tab.preview_cursor_line >= top_line
            && self.tab.preview_cursor_line < top_line + content.len()
        {
            let ln = &content[self.tab.preview_cursor_line - top_line];
            let len: usize = ln.spans.iter().map(|s| s.content.chars().count()).sum();
            // The read-only caret always sits on an actual character (0 for an empty line). `$` goes to the last character.
            self.tab.preview_cursor_col = self.tab.preview_cursor_col.min(len.saturating_sub(1));
        }
        // Highlight the line cursor/selection range (before the gutter = column index is aligned to the body's start at 0).
        let content = apply_preview_caret(
            content,
            top_line,
            self.tab.preview_cursor_line,
            self.tab.preview_cursor_col,
            self.preview_selection(),
        );
        // The line-number gutter (only when the setting is ON). The starting line number = preview_top_line.
        let content = if self.cfg.ui.line_numbers {
            with_line_numbers(content, top_line)
        } else {
            content
        };
        // The git change gutter (setting ON, only files with changes). Prepends a 1-cell marker to the left of the line number.
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
        let Some(path) = self.tab.preview_path.clone() else {
            return std::collections::HashMap::new();
        };
        let hit = matches!(&self.gutter_cache, Some(c) if c.path == path);
        if !hit {
            let diff = crate::git::file_diff(&self.tab.root, &path);
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
        Some((self.tab.preview_byte_top.min(len) * 100 / len) as u16)
    }

    pub fn preview_hscroll(&mut self, delta: i32) {
        let next = self.tab.preview_hscroll as i32 + delta;
        self.tab.preview_hscroll = next.max(0) as u16;
    }
    /// Reset horizontal scroll to the line start (left edge) in one step. `0`. When wrapping, it is already 0, so no effect.
    pub fn preview_hscroll_home(&mut self) {
        self.tab.preview_hscroll = 0;
    }
    /// Send horizontal scroll to the line end (right edge) in one step. `$`. The actual maximum is clamped to the longest line width at render time
    /// (here we set the upper bound, and the render side's `.min(max_h)` settles it at the correct right edge).
    pub fn preview_hscroll_end(&mut self) {
        self.tab.preview_hscroll = u16::MAX;
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
        match self.tab.mode {
            Mode::Tree => self
                .tab
                .entries
                .get(self.tab.selected)
                .map(|e| e.path.clone()),
            Mode::Preview => self.tab.preview_path.clone(),
        }
    }

    /// Scroll within the help. The upper bound is clamped at render time (since content and screen size are then known).
    pub fn help_scroll_by(&mut self, delta: i32) {
        self.help_scroll = (self.help_scroll as i32 + delta).max(0) as u16;
    }

    /// Format a path into a display string using the current path_style (shared by the tree/preview title).
    pub fn format_path(&self, path: &Path) -> String {
        match self.path_style {
            PathStyle::Full => path.display().to_string(),
            PathStyle::Home => home_relative(path),
            PathStyle::Relative => rel_to_open(&self.tab.open_dir, path),
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
        return line; // when a non-ASCII char changes byte length, skip highlighting to stay safe
    }
    // Collect the matching byte ranges.
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
    let mut pos = 0usize; // the byte position within full
    for span in line.spans {
        let text = span.content.into_owned();
        let span_start = pos;
        let span_end = pos + text.len();
        // The split points within this span (span edges + match boundaries).
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
                // The occurrence rank of the match range this segment belongs to (if any). If that
                // rank equals current_occurrence it's the current occurrence = orange, other
                // matches are yellow. Outside a range, leave it plain.
                let hit = ranges.iter().position(|(s, e)| a >= *s && b <= *e);
                let st = if let Some(idx) = hit {
                    let bg = if current_occurrence == Some(idx) {
                        Color::Rgb(0xff, 0x8c, 0x00) // the current occurrence
                    } else {
                        Color::Yellow // another occurrence
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

/// Prepend a line-number gutter (right-aligned + a space) to each line. The first line's number is `top_line+1` (1-based).
/// A dim color (DarkGray) distinguishes it from the body. The original line style (background, etc.) is kept.
fn with_line_numbers(lines: Vec<Line<'static>>, top_line: usize) -> Vec<Line<'static>> {
    use ratatui::style::{Color, Style};
    let last = top_line + lines.len().max(1);
    let width = last.to_string().len().max(3); // guarantee at least 3 digits of width
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

/// Pure core of `cache_root`. Falls back to the system temp directory (`std::env::temp_dir()` —
/// always an absolute path, `$TMPDIR` or `/tmp` on Unix) when neither `$XDG_CACHE_HOME` nor `$HOME`
/// is available, instead of the old `None`.
///
/// This is a *cache*, not persistent user data (unlike `bookmarks::default_base`/
/// `config::dirs_config_path`), so losing it across a reboot is harmless — the next Markdown preview
/// just re-downloads the image. Returning `None` here used to leave
/// `app/md_media.rs::ensure_remote_md_fetch`'s `let (Some(tx), Some(dest)) = (.., md_remote_cache_path(url))
/// else { return false; }` permanently un-taken: the background fetch thread never spawns, no
/// `RemoteFetch` message ever arrives, and the URL is never recorded in `md_remote_failed` either —
/// so a Markdown image behind an unresolvable cache root would sit at "loading" forever, never
/// reaching principle #3's safe-degrade text placeholder. Falling through to a location that is
/// always available restores that: the fetch is actually attempted and its real success/failure is
/// reported through the existing `RemoteFetch`/`apply_remote_fetch` machinery.
fn cache_root_from(xdg_or_home: Option<PathBuf>) -> PathBuf {
    xdg_or_home.unwrap_or_else(std::env::temp_dir)
}

/// Root of konoma's on-disk cache (`$XDG_CACHE_HOME`, or `$HOME/.cache`, or a system-temp fallback
/// when neither is available — see `cache_root_from`'s doc for why).
fn cache_root() -> PathBuf {
    cache_root_from(crate::bookmarks::xdg_base_dir(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        ".cache",
    ))
}

/// Deterministic cache path for a remote image URL: `<cache>/konoma/remote-images/<hash>`. The file is
/// stored without an extension (the content type is unknown until fetched) and read via content sniffing.
/// Kept `Option`-returning (always `Some` now that `cache_root` is total) because
/// `app/md_media.rs::ensure_remote_md_fetch` pattern-matches `Some(dest)`.
fn md_remote_cache_path(url: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let root = cache_root();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.trim().hash(&mut hasher);
    Some(
        root.join("konoma")
            .join("remote-images")
            .join(format!("{:016x}", hasher.finish())),
    )
}

/// Distinct from the `mod tests;` at the bottom of this file (`src/app/tests.rs`, off-limits to a
/// concurrent fix in this same session): a small inline test module for `cache_root`/
/// `md_remote_cache_path`, kept next to the code it tests (same convention `collect_all`'s
/// `#[cfg(test)]` helpers above already use in this file).
#[cfg(test)]
mod cache_root_tests {
    use super::*;

    /// Regression test for the reported bug: when neither `$XDG_CACHE_HOME` nor `$HOME` is
    /// available, `cache_root()` used to return `None`, and `md_remote_cache_path` propagated that
    /// `None` straight through. `app/md_media.rs::ensure_remote_md_fetch` treats that `None` as "no
    /// destination to download to" via `let (Some(tx), Some(dest)) = (.., md_remote_cache_path(url))
    /// else { return false; }` — the background fetch thread is never spawned, so no `RemoteFetch`
    /// ever arrives, so the URL is never recorded in `md_remote_failed` either: the "loading"
    /// placeholder for that Markdown image sits there forever instead of degrading to a text
    /// placeholder (principle #3 not reached).
    ///
    /// Before this fix, `cache_root_from` was `xdg_or_home` (identity, `Option<PathBuf>`-returning)
    /// — confirmed FAILING when run against that formula: `cache_root_from(None).is_some()` was
    /// `false` (the identity of `None` is `None`). Now total (`PathBuf`-returning): it always
    /// resolves to *something*, falling back to the system temp dir.
    #[test]
    fn cache_root_from_falls_back_to_temp_dir_without_xdg_or_home() {
        assert_eq!(
            cache_root_from(None),
            std::env::temp_dir(),
            "XDG_CACHE_HOME/HOME どちらも無ければシステム一時ディレクトリへフォールバック\
             (md_remote_cache_path が None を返すと、リモート画像のダウンロードが一生スレッド化されない)"
        );
    }

    #[test]
    fn cache_root_from_uses_resolved_xdg_or_home_when_present() {
        assert_eq!(
            cache_root_from(Some(PathBuf::from("/x/.cache"))),
            PathBuf::from("/x/.cache")
        );
    }

    /// `cache_root()` (the real, env-reading wrapper) must always resolve to an absolute path, in
    /// whatever environment this test happens to run under.
    #[test]
    fn cache_root_is_always_absolute_in_the_real_environment() {
        assert!(
            cache_root().is_absolute(),
            "実環境でも cache_root は常に絶対パス"
        );
    }

    /// `md_remote_cache_path` must never return `None` purely because no cache directory could be
    /// found — the whole point of `cache_root`'s temp-dir fallback. (A malformed/empty URL is a
    /// different, unrelated reason `md_remote_cache_path` could theoretically fail; this crate's
    /// hashing never rejects any `&str`, so there is no such case in practice, but the assertion is
    /// scoped to the cache-root concern regardless.)
    #[test]
    fn md_remote_cache_path_is_always_some_in_the_real_environment() {
        assert!(
            md_remote_cache_path("https://example.com/never-cached.png").is_some(),
            "cache_root が常に Some を返すので md_remote_cache_path も常に Some のはず"
        );
    }
}

/// Maximum bytes to download for one remote image (guards against huge / hostile responses).
const MD_REMOTE_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Download a remote image with `ureq` into `dest` (atomically, via a temp file), validating that the
/// bytes decode as an image before committing. Returns whether a valid image now exists at `dest`.
/// Runs on a background thread. `ureq` is pure Rust (rustls + webpki-roots, no OpenSSL/native-tls),
/// so this adds no external process and no system TLS dependency — replaces a former `curl` spawn.
fn fetch_remote_image(url: &str, dest: &Path) -> bool {
    let Some(parent) = dest.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let tmp = dest.with_extension("part");
    let Some(bytes) = fetch_remote_bytes(url) else {
        let _ = std::fs::remove_file(&tmp);
        return false;
    };
    if std::fs::write(&tmp, &bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    // Reject non-images (e.g. an HTML error page served with 200) before caching them (accepts SVG).
    if md_image_dims(&tmp).is_none() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    let ok = std::fs::rename(&tmp, dest).is_ok();
    if ok {
        prune_remote_cache(parent, MD_REMOTE_CACHE_MAX);
    }
    ok
}

/// Perform the actual HTTP GET, bounded by a global timeout (principle #4 — don't let a background
/// thread hang forever even though it's already off the UI thread) and a max body size (mirrors the
/// old `curl --max-filesize`). Redirects are followed automatically (up to ureq's default of 10 —
/// GitHub proxies images through camo, so this matters, same as the old `curl -L`). `gzip` is
/// transparently decoded. Returns None on any failure: bad URL, connect/TLS error, non-2xx status
/// (ureq treats those as errors by default, mirroring `curl --fail`), or an oversized body.
fn fetch_remote_bytes(url: &str) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent("konoma image preview")
        .build()
        .into();
    let mut resp = agent.get(url).call().ok()?;
    resp.body_mut()
        .with_config()
        .limit(MD_REMOTE_MAX_BYTES)
        .read_to_vec()
        .ok()
}

/// Keep at most `keep` files in the remote-image cache directory, deleting the oldest (by mtime).
/// The cache is content-addressed (one file per distinct URL) with no cleanup, so without this it
/// grows unbounded over a machine's lifetime. Runs only after a **new** download (rare), so the
/// directory scan is cheap. Best-effort: any IO error just leaves the cache as-is.
fn prune_remote_cache(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let p = e.path();
            // Exclude an in-progress temp file (.part).
            if p.extension().is_some_and(|x| x == "part") {
                return None;
            }
            let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((mtime, p))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(t, _)| *t); // oldest first
    for (_, p) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(p);
    }
}

/// Max files kept in the remote-image disk cache (`~/.cache/konoma/remote-images`).
const MD_REMOTE_CACHE_MAX: usize = 256;

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

/// Cell box for an inline mermaid diagram: **fill to `target_rows`** (up- or downscale, keeping
/// the aspect from the layout pixels), clamped by the available width. Unlike raster images
/// (`md_image_cells`, never upscaled), diagrams are vector-backed — the raster follows the
/// display size via sharpening re-rasters, so growing beyond the natural size stays crisp.
fn mermaid_cells(pw: u32, ph: u32, fw: u16, fh: u16, avail: u16, target_rows: u16) -> (u16, u16) {
    let (fw, fh) = (fw.max(1) as f64, fh.max(1) as f64);
    let ar = (pw.max(1) as f64) / (ph.max(1) as f64); // the pixel aspect ratio
    let mut rows = target_rows.max(1) as f64;
    let mut cols = (rows * fh * ar / fw).round().max(1.0);
    let avail = avail.max(1) as f64;
    if cols > avail {
        cols = avail;
        rows = (cols * fw / ar / fh).round().max(1.0);
    }
    (cols as u16, rows as u16)
}

/// Cell box for an inline math image, sized proportionally from the SVG's intrinsic **em** units
/// (RaTeX renders at 40 units/em) so equations sit at roughly text scale: em-height → rows, intrinsic
/// aspect → cols. Display math gets a touch more height than inline. Clamped to the available width
/// (aspect preserved) and a sane row cap. Vector-backed, so the once-rendered raster is fit crisply.
fn math_cells(uw: u32, uh: u32, fw: u16, fh: u16, avail: u16, display: bool) -> (u16, u16) {
    let (fw, fh) = (fw.max(1) as f64, fh.max(1) as f64);
    let em_h = (uh.max(1) as f64) / 40.0;
    let rows_per_em = if display { 1.5 } else { 1.3 };
    let mut rows = (em_h * rows_per_em).round().max(1.0);
    let ar = (uw.max(1) as f64) / (uh.max(1) as f64);
    let mut cols = (rows * fh * ar / fw).round().max(1.0);
    let avail = avail.max(1) as f64;
    if cols > avail {
        cols = avail;
        rows = (cols * fw / ar / fh).round().max(1.0);
    }
    // A cap so an extremely tall expression (e.g. a matrix) doesn't fill the whole screen.
    let maxr = 24.0;
    if rows > maxr {
        let s = maxr / rows;
        rows = maxr;
        cols = (cols * s).round().max(1.0);
    }
    (cols as u16, rows as u16)
}

/// Source-pixel crop rectangle (x, y, w, h).
type PxRect = (u32, u32, u32, u32);

/// Crop window for the in-place zoom of an inline diagram. `f` is the visible fraction, i.e.
/// 1/zoom, of the source; `center` is clamped so the window stays inside the image. Returns the
/// crop px rect plus the clamped center. Fractions are scale-free, so the same zoom and center
/// map onto a sharper re-raster unchanged.
fn fence_crop(dims: (u32, u32), f: f64, center: (f64, f64)) -> (PxRect, (f64, f64)) {
    let (sw, sh) = dims;
    let f = f.clamp(0.0, 1.0);
    let cx = center.0.clamp(f / 2.0, 1.0 - f / 2.0);
    let cy = center.1.clamp(f / 2.0, 1.0 - f / 2.0);
    let cw = ((sw as f64 * f).round() as u32).clamp(1, sw);
    let ch = ((sh as f64 * f).round() as u32).clamp(1, sh);
    let x0 = ((cx * sw as f64) as i64 - cw as i64 / 2).clamp(0, (sw - cw) as i64) as u32;
    let y0 = ((cy * sh as f64) as i64 - ch as i64 / 2).clamp(0, (sh - ch) as i64) as u32;
    ((x0, y0, cw, ch), (cx, cy))
}

/// Result of [`image_layout`]. `center` and `frac` are both `(f64, f64)` pairs with different
/// meanings (image-space center vs. per-axis visible fraction), so this struct keeps them from
/// being swapped at the call site.
///
/// - `target`  : the render-target rectangle (centered; z=1=fit, grows when zooming, clips when exceeding the viewport)
/// - `crop_rect`: the source-image window of the visible portion (px: x,y,w,h)
/// - `center`  : the center clamped to where the visible window fits within the image [0,1]
/// - `frac`    : the visible fraction per axis (<1=clipped=pan possible)
#[derive(Clone, Copy)]
struct ImageLayout {
    target: Rect,
    crop_rect: PxRect,
    center: (f64, f64),
    frac: (f64, f64),
}

/// Compute the display layout for an image/GIF frame (a pure function shared by prepare_image / prepare_gif).
/// From (source image src, font_size, zoom, center[0,1], display area inner, render_scale), returns
/// an [`ImageLayout`] (None if size 0).
fn image_layout(
    src: &image::DynamicImage,
    font_size: ratatui_image::FontSize,
    zoom: f64,
    center: (f64, f64),
    inner: Rect,
    render_scale: f64,
    logical: Option<(u32, u32)>,
) -> Option<ImageLayout> {
    use image::GenericImageView;
    let (sw, sh) = src.dimensions();
    if sw == 0 || sh == 0 || inner.width == 0 || inner.height == 0 {
        return None;
    }
    // The z=1 fit display size (cells, never enlarged). A vector-derived preview is measured by its
    // **logical size** (the initial raster's dimensions), so swapping in a sharper raster never
    // moves the on-screen size or the crop window at all (the HiDPI backing-store approach — the
    // crop itself is mapped to real pixels by ratio).
    let (nw, nh) = match logical {
        Some((lw, lh)) if lw > 0 && lh > 0 => (
            (lw as u16).div_ceil(font_size.width.max(1)),
            (lh as u16).div_ceil(font_size.height.max(1)),
        ),
        _ => {
            let n = Resize::natural_size(src, font_size);
            (n.width, n.height)
        }
    };
    let base = centered_rect((nw, nh), inner, false);
    // The logical display size after zoom.
    let disp_w = (base.width as f64 * zoom).max(1.0);
    let disp_h = (base.height as f64 * zoom).max(1.0);
    // The on-screen size = min(display size, display area).
    let tw = (disp_w.min(inner.width as f64).round() as u16).max(1);
    let th = (disp_h.min(inner.height as f64).round() as u16).max(1);
    // The visible fraction on each axis (<1 means it's clipped = pan is possible).
    let fw = (tw as f64 / disp_w).min(1.0);
    let fh = (th as f64 / disp_h).min(1.0);
    // Clamp the center so the visible window stays within the image.
    let cx = center.0.clamp(fw / 2.0, 1.0 - fw / 2.0);
    let cy = center.1.clamp(fh / 2.0, 1.0 - fh / 2.0);
    // Crop the visible window out of the original image (px).
    let cw = ((sw as f64 * fw).round() as u32).clamp(1, sw);
    let ch = ((sh as f64 * fh).round() as u32).clamp(1, sh);
    let x0 = ((cx * sw as f64) as i64 - cw as i64 / 2).clamp(0, (sw - cw) as i64) as u32;
    let y0 = ((cy * sh as f64) as i64 - ch as i64 / 2).clamp(0, (sh - ch) as i64) as u32;
    let crop_rect = (x0, y0, cw, ch);
    // The display rect (centered). Optionally downscaled by render_scale (reduces transfer size = less render wait).
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
    Some(ImageLayout {
        target,
        crop_rect,
        center: (cx, cy),
        frac: (fw, fh),
    })
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
        // Use exactly the same basis as the top-left title display (format_path's Relative).
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
        // Under open_dir: relativize against its parent so the launch directory name comes first (e.g. A/sub/x).
        let base = open_dir.parent().unwrap_or(open_dir);
        match path.strip_prefix(base) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
            _ => home_relative(path),
        }
    } else {
        // Outside open_dir (siblings/ancestors, etc.): show a path relative to open_dir, including `..` (e.g. ../B/aaa.md).
        // Without this, a file in sibling B would look like `B/aaa.md` and be mistaken for something underneath.
        match rel_from(open_dir, path) {
            Some(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
            _ => home_relative(path),
        }
    }
}

/// Write to the clipboard. Returns Err on environments where arboard is unavailable (the caller shows a flash).
fn set_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        // On Linux the clipboard content is owned by the process holding the X11/Wayland
        // selection: dropping the Clipboard releases it and the copied text is lost unless a
        // clipboard manager grabbed it. Probe on the main thread first so the caller still gets
        // an Err (and shows its flash) on headless/unavailable environments, then hold the
        // selection in a detached thread via SetExtLinux::wait() so paste works. wait() returns
        // (and the thread exits) as soon as another app — or the next copy — takes ownership, so
        // at most one holder thread is alive at a time. Not unit-tested (requires an X11/Wayland
        // display); verified in a Linux VM.
        use arboard::SetExtLinux;
        let _ = arboard::Clipboard::new()?;
        let owned = text.to_string();
        std::thread::spawn(move || {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set().wait().text(owned);
            }
        });
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut cb = arboard::Clipboard::new()?;
        cb.set_text(text.to_string())?;
        Ok(())
    }
}

/// `~/...` if under HOME, otherwise the full path.
pub(crate) fn home_relative(path: &Path) -> String {
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
            cur.push(c); // preceded by `\` = this character is a literal (e.g. a path name containing spaces)
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
        .filter(|p| p.exists()) // only accept paths that actually exist (a safety net rejecting text pastes/bad input)
        .collect()
}

/// Build a plan for sequential rename. Number `targets` **in display order** as `{n}`=1,2,…,
/// expand the template, and create each (old, new) path. If the template has no extension, the original extension is kept automatically.
/// **Pre-validates** collisions: an empty name / a `/` mixed in / duplicate final names / a collision with an existing file (not vacated within the batch) is an Err.
/// A method-shaped free function (not `fileops`): unlike `fileops`, this already has `lang` in hand
/// at every call site (`App::dialog_submit`), so — unlike the typed-error/`describe_error` detour
/// `fileops` needs — it can just build the already-localized text with `tr()` directly.
fn build_rename_plan(
    targets: &[PathBuf],
    template: &str,
    lang: crate::i18n::Lang,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut plan: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(targets.len());
    for (idx, src) in targets.iter().enumerate() {
        let n = idx + 1;
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("");
        let rendered = crate::fileops::render_rename_template(template, n, stem, ext);
        let rendered = rendered.trim().to_string();
        if rendered.is_empty() {
            anyhow::bail!("{} (n={n})", tr(lang, crate::i18n::Msg::RenameEmptyName));
        }
        if rendered.contains('/') {
            anyhow::bail!(
                "{}{rendered}",
                tr(lang, crate::i18n::Msg::RenameSlashInName)
            );
        }
        // Extension: if the template result has no extension, automatically append the original extension (a user decision).
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
    // Validate that the final names have no duplicates.
    let mut seen = BTreeSet::new();
    for (_, dst) in &plan {
        if !seen.insert(dst.clone()) {
            anyhow::bail!(
                "{}{}",
                tr(lang, crate::i18n::Msg::RenameDestDuplicate),
                dst.display()
            );
        }
    }
    // Validate against collisions with existing files (excluding source paths that are "vacated by moving" within the batch).
    let vacated: BTreeSet<PathBuf> = plan
        .iter()
        .filter(|(s, d)| s != d)
        .map(|(s, _)| s.clone())
        .collect();
    for (src, dst) in &plan {
        // Exclude an unchanged name (src == dst) so its own existence is never mistaken for a collision (#11).
        // A true collision — colliding with a different file — is caught only by the final-name duplicate check above.
        if src == dst {
            continue;
        }
        if dst.exists() && !vacated.contains(dst) {
            // Shares `Msg::AlreadyExists` with `fileops::FileOpError::AlreadyExists` (via
            // `describe_error`) — same condition ("this path already exists"), one wording.
            anyhow::bail!(
                "{}{}",
                tr(lang, crate::i18n::Msg::AlreadyExists),
                dst.display()
            );
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
    // No common component, and one side starts at the root (absolute/Prefix) = cannot be relativized.
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
/// Bundles the decision info for one entry for sorting. Lowercased sort keys are precomputed
/// once per entry (sort_by runs O(n log n) comparisons — lowercasing inside the comparator
/// allocated two Strings per comparison). The stat is taken only when the sort key needs
/// size/mtime, or when the entry is a symlink (is_dir must follow the link like fs::metadata).
struct ChildMeta {
    path: PathBuf,
    /// Lowercased file name (also the stable tiebreak for every sort key).
    name_lower: String,
    /// Lowercased extension; filled only when sorting by extension.
    ext_lower: String,
    is_dir: bool,
    size: u64,
    mtime: Option<std::time::SystemTime>,
}

/// `fs::metadata` (**follows** symlinks), routed through one choke point so the directory walks
/// below have a single place that can issue a stat — which is what lets the test-only counter in
/// `test_support` assert "an ordinary entry costs zero stats".
fn stat_follow(path: &Path) -> Option<std::fs::Metadata> {
    #[cfg(test)]
    crate::test_support::note_stat_call();
    std::fs::metadata(path).ok()
}

fn child_meta(path: PathBuf, ft: Option<std::fs::FileType>, sort: Sort) -> ChildMeta {
    let need_stat = matches!(sort.key, SortKey::Size | SortKey::Modified);
    let is_symlink = ft.map(|t| t.is_symlink()).unwrap_or(true);
    let (is_dir, size, mtime) = if need_stat || is_symlink {
        // fs::metadata follows symlinks, so a symlink to a directory stays expandable.
        let md = stat_follow(&path);
        (
            md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            md.as_ref().map(|m| m.len()).unwrap_or(0),
            md.as_ref().and_then(|m| m.modified().ok()),
        )
    } else {
        // DirEntry::file_type is free on macOS (readdir d_type) — no extra syscall.
        (ft.map(|t| t.is_dir()).unwrap_or(false), 0, None)
    };
    let name_lower = path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext_lower = if matches!(sort.key, SortKey::Ext) {
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase()
    } else {
        String::new()
    };
    ChildMeta {
        path,
        name_lower,
        ext_lower,
        is_dir,
        size,
        mtime,
    }
}

/// Compare two entries according to the settings. dirs_first → key → reverse, finally stabilized by name.
fn sort_cmp(a: &ChildMeta, b: &ChildMeta, sort: Sort) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if sort.dirs_first {
        let d = b.is_dir.cmp(&a.is_dir); // directories (true) first
        if d != Ordering::Equal {
            return d;
        }
    }
    let mut ord = match sort.key {
        SortKey::Name => a.name_lower.cmp(&b.name_lower),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Modified => a.mtime.cmp(&b.mtime),
        SortKey::Ext => a.ext_lower.cmp(&b.ext_lower),
    };
    if sort.reverse {
        ord = ord.reverse();
    }
    // Stabilize equal values by name (ascending) so the ordering never wobbles.
    ord.then_with(|| a.name_lower.cmp(&b.name_lower))
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
    expanded_dirs: &std::collections::HashSet<PathBuf>,
    show_hidden: bool,
    sort: Sort,
    out: &mut Vec<Entry>,
) -> Result<()> {
    let mut children: Vec<ChildMeta> = std::fs::read_dir(dir)?
        .filter_map(|r| r.ok())
        .map(|e| {
            let ft = e.file_type().ok();
            (e.path(), ft)
        })
        .filter(|(p, _)| {
            if show_hidden {
                return true;
            }
            !p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false)
        })
        .map(|(p, ft)| child_meta(p, ft, sort))
        .collect();

    children.sort_by(|a, b| sort_cmp(a, b, sort));

    for c in children {
        let expanded = c.is_dir && expanded_dirs.contains(&c.path);
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

/// Recursively collect files/directories under `root` (the population for filtering), **in one
/// uninterrupted pass**. Hidden ones are excluded via `show_hidden`, and symlink directories are not
/// descended into (to prevent loops). The scan count is capped as a cost limit.
/// The return value is in ascending path order (grouped per directory). Each Entry is flat with depth=0 and expanded=false.
///
/// Production no longer calls this: a `/` scan goes through `collect_scan` with a time budget so a
/// large tree can be finished on a worker instead of freezing the UI. It stays as the reference
/// "walk it all in one go" definition the tests measure the budgeted/split version against — if the
/// two ever disagree, splitting a walk has started losing entries.
#[cfg(test)]
fn collect_all(root: &Path, show_hidden: bool) -> Vec<Entry> {
    collect_all_capped(root, show_hidden, COLLECT_CAP)
}

/// Cost limit for one `collect_all` scan.
const COLLECT_CAP: usize = 50_000;

/// `collect_all`'s body with the cap injected, so the truncation behaviour can be exercised with a
/// handful of files instead of a 50,000-entry fixture.
#[cfg(test)]
fn collect_all_capped(root: &Path, show_hidden: bool, cap: usize) -> Vec<Entry> {
    // No deadline = walk to completion, so the leftover stack is always empty.
    collect_scan(vec![root.to_path_buf()], show_hidden, cap, None).0
}

/// How long the **synchronous** part of a `/` scan may run before it hands the rest to a worker.
///
/// Not a tuning knob so much as a floor: a whole-tree walk of 50,000 entries costs ~45ms in
/// `readdir` alone on this machine, so no implementation can finish one inside the <60ms draw
/// budget. Anything smaller than the budget finishes synchronously and behaves exactly as it
/// always did (the overwhelming majority of directories); only a tree big enough to blow the
/// budget pays for the hand-off, and it pays with a partial pool that fills in rather than a
/// frozen UI.
const COLLECT_BUDGET: std::time::Duration = std::time::Duration::from_millis(20);

#[cfg(test)]
thread_local! {
    /// Test-only override for [`filter_scan_budget`]. `None` = production default;
    /// `Some(None)` = unlimited (always finishes synchronously); `Some(Some(d))` = that budget
    /// (`Duration::ZERO` = hand everything to the worker).
    ///
    /// Thread-local, like `test_support::STAT_CALLS`: a process-wide switch would leak into every
    /// other test running in parallel. Only the *calling* thread consults it, and the worker
    /// thread never does (it runs without a deadline by construction).
    static SCAN_BUDGET_OVERRIDE: std::cell::Cell<Option<Option<std::time::Duration>>> =
        const { std::cell::Cell::new(None) };
}

/// The budget the next synchronous scan gets. `None` = no deadline at all.
fn filter_scan_budget() -> Option<std::time::Duration> {
    #[cfg(test)]
    if let Some(o) = SCAN_BUDGET_OVERRIDE.with(|c| c.get()) {
        return o;
    }
    Some(COLLECT_BUDGET)
}

/// The deadline the next synchronous scan must stop by, or `None` for "no deadline".
fn filter_scan_deadline() -> Option<std::time::Instant> {
    // `checked_add` rather than `+`: an unbounded budget is expressed as a very large Duration in
    // tests, and Instant addition panics on overflow. Overflow means "effectively never", which is
    // exactly `None`.
    filter_scan_budget().and_then(|b| std::time::Instant::now().checked_add(b))
}

/// Force the synchronous scan budget for this thread (tests only).
///
/// Both code paths have to be reachable on demand: with the real budget a test fixture is far too
/// small to ever exhaust it, so the hand-off would never be exercised — the exact shape of hole
/// that let a broken image path ship once already ("the test never went down that branch").
#[cfg(test)]
pub(crate) fn set_filter_scan_budget(b: Option<Option<std::time::Duration>>) {
    SCAN_BUDGET_OVERRIDE.with(|c| c.set(b));
}

#[cfg(test)]
thread_local! {
    /// Test-only: make the next `collect_scan` on this thread panic. Same reasoning as
    /// `SCAN_BUDGET_OVERRIDE` — the walk's panic-safety net has to be reachable on demand, and a
    /// real `read_dir` walk cannot be made to panic to order. Thread-local so it cannot leak into
    /// tests running in parallel, and so the *worker* thread never sees it: what this exists to
    /// cover is the two walks that run on the **UI thread** (`kick_filter_pool_start`'s budgeted
    /// head start and `spawn_or_sync_pool`'s no-channel fallback), where an unwind would take the
    /// whole TUI down rather than just one background job.
    static SCAN_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Arm/disarm the test-only scan panic for this thread (tests only). See [`SCAN_PANIC`].
#[cfg(test)]
pub(crate) fn set_filter_scan_panic(on: bool) {
    SCAN_PANIC.with(|c| c.set(on));
}

/// Walk `stack` (LIFO of directories still to read) collecting entries until it is exhausted, the
/// `cap` is hit, or `deadline` passes.
///
/// Returns `(entries collected here — sorted, directories still unread)`. **An empty leftover
/// stack means the walk is complete**; hitting the cap counts as complete (it is a deliberate
/// truncation, not an interruption), so the stack is cleared in that case.
///
/// Splitting a walk in two and concatenating the halves yields exactly the same multiset as
/// walking it in one go: the stack is handed over verbatim, and the cap is carried across as a
/// remaining count. Both halves come back sorted, so the caller's `sort_by` over the concatenation
/// is a run merge rather than a fresh sort.
fn collect_scan(
    mut stack: Vec<PathBuf>,
    show_hidden: bool,
    cap: usize,
    deadline: Option<std::time::Instant>,
) -> (Vec<Entry>, Vec<PathBuf>) {
    #[cfg(test)]
    if SCAN_PANIC.with(|c| c.get()) {
        panic!("injected scan panic");
    }
    let mut out: Vec<Entry> = Vec::new();
    let over_budget = || deadline.is_some_and(|d| std::time::Instant::now() >= d);
    // Directories read on this call. The deadline is only consulted once at least one is done, so
    // **every call makes progress**: a hand-off can never bounce a walk back and forth without
    // advancing it, however small the budget.
    let mut read = 0usize;
    while let Some(dir) = stack.pop() {
        if out.len() >= cap {
            // Truncated by the cost limit: this is as complete as the scan is ever going to get.
            stack.clear();
            break;
        }
        if read > 0 && over_budget() {
            // Out of time: put the directory back so the worker resumes exactly here.
            stack.push(dir);
            out.sort_by(|a, b| a.path.cmp(&b.path));
            return (out, stack);
        }
        read += 1;
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.filter_map(|r| r.ok()) {
            if out.len() >= cap {
                break;
            }
            let path = e.path();
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false);
            if hidden && !show_hidden {
                continue;
            }
            // Same rule as `child_meta` on `build_dir`'s side: `DirEntry::file_type` comes from
            // readdir's d_type, so an ordinary entry costs no stat at all; treat an unavailable
            // file_type as "symlink", which is conservative because it routes to the stat below.
            let ft = e.file_type().ok();
            let is_symlink = ft.map(|t| t.is_symlink()).unwrap_or(true);
            // Don't descend into a symlinked dir (avoids cycles). is_dir follows the link target, so it's judged separately.
            let is_dir = if is_symlink {
                // fs::metadata follows symlinks, so a symlink to a directory still reports is_dir.
                stat_follow(&path).map(|m| m.is_dir()).unwrap_or(false)
            } else {
                ft.map(|t| t.is_dir()).unwrap_or(false)
            };
            // Clone only for the minority that gets walked (directories); the entry itself takes
            // ownership. Cloning unconditionally cost one extra PathBuf allocation per file.
            if is_dir && !is_symlink {
                stack.push(path.clone());
            }
            out.push(Entry {
                path,
                is_dir,
                depth: 0,
                expanded: false,
            });
        }
        // The deadline is checked once per directory (at the top), never inside the loop above:
        // `read_dir`'s iterator cannot be resumed, so stopping mid-directory would mean re-reading
        // it and emitting duplicates. Reading the clock once per `read_dir` is free by comparison
        // (~20ns against microseconds). The synchronous part can therefore overshoot by however
        // long the one directory it is in the middle of takes — bounded by `cap`, since the loop
        // stops emitting past it regardless.
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, stack)
}

#[cfg(test)]
mod tests;
