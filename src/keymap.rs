// konoma keymap layer (Run2: the low-level pure-data layer of the key-system redesign).
//
// Role: hold a declarative "Context × KeyPress → Action (command)" map as the built-in
// default, override/add/disable it via config (`[keys.<context>]`), and detect conflicts at
// startup, falling back to the default when they occur.
// Touches no App state at all (the context is passed in as an argument); it only references
// crossterm's KeyCode/KeyModifiers.
//
// A key is looked up in layers: the context's own table, the tables of its view type (text,
// table, image, diff, list), the basic table shared by everything with a cursor, then `global`.
// `Surface` (what receives keys: a dialog, an overlay, a full screen) is still the type the
// dispatcher and the UI stack speak; `Context` (what is on screen, finer than a surface) is the
// unit of key tables and of `[keys.<context>]`. `App::key_context` maps one to the other.
//
// The caller (main::handle_key / dispatch_action) executes the `Action` resolved here.

use std::collections::HashMap;

use anyhow::{bail, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[cfg(feature = "git")]
use crate::app::GitCopyKind;
use crate::app::{CopyKind, SortKey, TableCopyKind};
use crate::i18n::Msg;

#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod defaults_snapshot;
#[cfg(test)]
mod defaults_tests;
#[cfg(test)]
mod override_resolution;

// =============================================================================
// Motion (shared movement/scroll amount). Per-Surface interpretation lives on the dispatch_action side (Stage 3).
// =============================================================================

/// Abstract amount of cursor movement / scrolling, mapped to concrete behavior per surface (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Up,
    Down,
    Top,
    Bottom,
    PageUp,
    PageDown,
    HalfUp,
    HalfDown,
    Left,
    Right,
    LineHome,
    LineEnd,
}

// =============================================================================
// Action (the complete set of commands).
// =============================================================================

/// The full set of commands. Corresponds both ways between keymap values (`Binding::Run`) and config strings (§1.2).
/// Git-related variants exist only under `feature="git"` (keeps `--no-default-features` green).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A key disabled via config `"noop"`/`"disabled"` (does nothing at runtime).
    Noop,
    /// Move / scroll (interpreted per surface).
    Navigate(Motion),

    // --- Global (tabs / help / path copy) ---
    /// `t`: new tab, always appended at the end of the tab list.
    TabNew,
    /// `I`: new tab inserted immediately to the right of the currently active one (mirrors vim's
    /// `:tabnew` / tmux's `new-window -a`). `t` keeps appending.
    TabNewAfter,
    TabClose,
    TabPrev,
    TabNext,
    /// 1..=9 → zero-based index. Not configurable (fixed to digit keys).
    TabGoto(u8),
    ToggleHelp,
    /// Path copy via y→{n,r,f,p} (reuses the existing app::CopyKind).
    CopyPath(CopyKind),
    /// `y → c` (Markdown preview): copy the source of the focused code block. Shown in the copy
    /// which-key menu only while a code block is focused; a no-op (flash) otherwise.
    CopyCodeBlock,
    /// `P` (global): read a path / GitHub link from the clipboard and jump there (reveal + preview).
    PasteJump,

    // --- Tree (normal) ---
    Quit,
    /// `q` at the tree top level: close the current tab if more than one is open, otherwise quit the app.
    CloseTabOrQuit,
    FilterStart,
    TreeDescend,
    TreeActivate,
    TreeLeave,
    ToggleHidden,
    /// Toggle line wrapping of the text and document preview for this session (`[ui] wrap` says
    /// the start-up value). Lives in the `,` menu.
    ToggleWrap,
    ToggleInfo,
    RequestEdit,
    /// `o`: open the file in another app (layer 3, the `open` verb of the plugin that shows it:
    /// an Office app for Office files, else the OS default application). Runs on a worker.
    OpenExternal,
    OpenGitView,
    Refresh,
    CyclePathStyle,
    OpenSortMenu,
    MarkSet,
    MarkJump,
    /// `a`: set the anchor to the current root (reanchor_root; replaces the old `:`).
    SetAnchor,
    /// `A`: reset the anchor to the startup dir (new).
    ResetAnchor,
    /// `d`: diff of the changed file under the cursor (old `=`).
    OpenGitDiffCursor,
    EnterVisual,
    ToggleSelect,
    /// `C`: toggle the changed-files-only tree view (flat list of files with a git status; Agent Watch).
    ToggleChangedFilter,
    /// `n`/`N`: jump the tree cursor to the next/previous changed file (wraps; reveals collapsed dirs).
    JumpNextChange,
    JumpPrevChange,
    /// `F` (global): toggle follow mode — externally changed files are auto-selected and previewed
    /// (watch an AI agent work). Sticky: only `q` (leaving the follow diff), entering a text/modal
    /// surface, or another `F` stops following; scroll/`n`/`N`/`f` keep it on.
    ToggleFollow,

    // --- File management (under the Space→ leader / also shared with Visual) ---
    FileCreate,
    FileRename,
    FileDelete,
    FileCopy,
    FileCut,
    FilePaste,
    FileDuplicate,

    // --- Tree:Visual sub ---
    VisualCommit,
    VisualSelectSiblings,
    VisualSelectAll,

    // --- Preview: text/markdown ---
    PreviewBack,
    SearchStart,
    SearchNext,
    SearchPrev,
    /// `v`: start a charwise (exact character range) selection in a windowed code/text preview.
    PreviewEnterVisual,
    /// `V`: start a linewise (whole-line) selection in a windowed code/text preview.
    PreviewEnterVisualLine,
    /// `y` (in preview-visual): copy the selection to the clipboard.
    PreviewCopySelection,
    /// `Y`: copy an `@path#L12-34` reference (Claude Code file+line context) for the selection/caret line.
    PreviewCopySelectionRef,
    /// `v`/`V`/`q` (in preview-visual): exit selection without copying.
    PreviewExitVisual,
    /// `R`: cycle the representation of the preview — a Markdown document between its decorated
    /// render and its raw source, a slide deck between pictures and text, a diff between its
    /// presentations. (Named `toggle_markdown_raw` before; that name is still read.)
    CycleRepresentation,
    /// `R` in `Surface::PreviewImage`: returns to the diff while this image/PDF/SVG preview *is*
    /// its own `Preview` representation, and is a no-op otherwise — `App::image_return_to_diff`'s
    /// own doc comment on why this is a separate action from `CycleRepresentation`.
    ImageReturnToDiff,
    /// Tab/BackTab/Enter (triggered as fixed keys; not listed in the keymap).
    LinkFocusNext,
    LinkFocusPrev,
    LinkOpen,
    /// `Ctrl-t`: open the focused Markdown link in a **new tab** (local file/dir; URLs still go to
    /// the browser). Enter keeps opening in the current tab. `Ctrl-t` mirrors the TUI convention
    /// (fzf/Telescope) and konoma's `t`=new tab, and is reliable in every terminal + tmux.
    OpenLinkNewTab,
    /// `Ctrl-t` in the tree: open the entry under the cursor in a **new tab** (file → preview,
    /// directory → the new tab's root). The current tab is left untouched. Same key/meaning as
    /// `OpenLinkNewTab` in the preview — "open this in a new tab" — pairing with global `t`=new tab.
    OpenInNewTab,

    // --- Preview: image ---
    ImageZoomIn,
    ImageZoomOut,
    ImageZoomReset,
    /// Next / previous "page" of the preview on screen: a PDF page, a spreadsheet sheet, or a media
    /// diff's PDF page (`J` / `K`, `PageDown` / `PageUp` on the image surface). One action;
    /// `App::page_turn` picks what it means from the kind of preview. Config names: `page_next` /
    /// `page_prev` (the former `pdf_next_page`, `sheet_next`, `media_diff_page_next`, ... stay as
    /// aliases).
    PageNext,
    PagePrev,

    // --- Preview: file paging (shared across text/image/table) ---
    /// Preview the next/previous **file** in tree display order (directories are skipped,
    /// wraps at the ends, and the tree cursor follows).
    PreviewFileNext,
    PreviewFilePrev,

    // --- Preview: table (csv/tsv) ---
    /// Copy the current cell / row / column of a CSV/TSV table (via the `y→` menu).
    TableCopy(TableCopyKind),

    // --- Preview: gitdiff / GitDetail shared ---
    #[cfg(feature = "git")]
    GitDiffDiscard,
    #[cfg(feature = "git")]
    CycleDiffLayout,
    /// `R` in `Surface::PreviewGitDiff`: cycles the diff's three presentations (source/rendered/
    /// preview) — `docs/FEATURE-MD-RENDERED-DIFF.md` §4.
    #[cfg(feature = "git")]
    CycleDiffView,
    /// In a follow-opened diff: toggle between the diff since follow-start and the full git diff.
    #[cfg(feature = "git")]
    ToggleFollowDiffScope,
    /// `n` / `N` in a diff (when it is not the media side-by-side view): scroll to the next /
    /// previous change block of the file. (In a text or decorated preview `n` / `N` are
    /// `SearchNext` / `SearchPrev`, which jump to the next / previous change while no search is
    /// active.)
    #[cfg(feature = "git")]
    DiffNextChange,
    #[cfg(feature = "git")]
    DiffPrevChange,
    // --- Git changes hub (p) ---
    #[cfg(feature = "git")]
    GitStage,
    #[cfg(feature = "git")]
    GitUnstage,
    #[cfg(feature = "git")]
    GitStageAll,
    #[cfg(feature = "git")]
    GitUnstageAll,
    #[cfg(feature = "git")]
    GitDiscard,
    #[cfg(feature = "git")]
    GitCommit,
    #[cfg(feature = "git")]
    GitWorktreeDiff,
    #[cfg(feature = "git")]
    GitOpenLog,
    #[cfg(feature = "git")]
    GitOpenGraph,
    #[cfg(feature = "git")]
    GitOpenBranches,
    #[cfg(feature = "git")]
    GitLaunchTool,
    #[cfg(feature = "git")]
    GitOpenSelectedDiff,

    // --- Git log/graph ---
    #[cfg(feature = "git")]
    GitOpenDetail,
    /// Graph: set the selected commit as the base branch (pins its first-parent chain to lane0 on the left).
    #[cfg(feature = "git")]
    GitGraphSetBase,
    /// Graph: clear the pinned base.
    #[cfg(feature = "git")]
    GitGraphClearBase,
    /// Show every revision in the graph instead of the backend's own range (jj only; git's graph
    /// is already everything).
    #[cfg(feature = "git")]
    GitGraphToggleAll,
    /// Ask jj to take a snapshot — the only thing konoma writes to a jj repository, and only when
    /// asked. See `vcs::jj::snapshot`.
    #[cfg(feature = "git")]
    JjSync,
    /// Graph: open the branch-visibility panel (toggles which branches show when there are many).
    #[cfg(feature = "git")]
    GitGraphOpenPicker,
    /// Graph branch panel: toggle the cursor row's visibility on/off.
    #[cfg(feature = "git")]
    GitGraphPickerToggle,
    /// Graph branch panel: show all branches.
    #[cfg(feature = "git")]
    GitGraphPickerAll,
    /// Graph branch panel: show only the current (plus base) branch.
    #[cfg(feature = "git")]
    GitGraphPickerCurrentOnly,
    /// Graph branch panel: move the cursor row up one in priority order (`K`).
    #[cfg(feature = "git")]
    GitGraphPickerMoveUp,
    /// Graph branch panel: move the cursor row down one in priority order (`J`).
    #[cfg(feature = "git")]
    GitGraphPickerMoveDown,

    /// `Tab` / `Shift-Tab` on a version-control list screen: switch to the next / previous screen
    /// of the cycle (changes, log, graph, branches, worktrees; jj has no worktrees).
    #[cfg(feature = "git")]
    GitScreenNext,
    #[cfg(feature = "git")]
    GitScreenPrev,
    /// `/` on a list: filter the rows by the text they show. The changes, log and graph screens
    /// and the overlay lists (outline / page list, tab list, bookmark list) share one filter (the
    /// branch and worktree lists have their own `BranchFilterStart` / `WorktreeFilterStart`).
    ListFilterStart,

    // --- Git branches ---
    #[cfg(feature = "git")]
    BranchFilterStart,
    #[cfg(feature = "git")]
    BranchCheckout,
    #[cfg(feature = "git")]
    BranchCreate,
    #[cfg(feature = "git")]
    BranchDelete,

    // --- Git worktrees (linked working trees — `git worktree add`; unrelated to the rest of this
    // codebase's "worktree" = the uncommitted working tree, e.g. `GitWorktreeDiff`/`GraphRow.worktree`) ---
    /// Changes hub: open the linked-worktree list (`w`).
    #[cfg(feature = "git")]
    GitOpenWorktrees,
    /// Worktree list: start filtering by branch name / path (`/`).
    #[cfg(feature = "git")]
    WorktreeFilterStart,
    /// Worktree list: switch this tab's root (and `open_dir`) to the selected worktree. The real
    /// trigger is the fixed `Enter` key (see `handle_enter`); this variant exists purely so it is
    /// config-registrable and round-trips through `action_from_str`/`action_name` — same idiom as
    /// `BranchCheckout` (Enter, hardcoded) coexisting with its own action. It has no default key
    /// binding here, same as `GitOpenSelectedDiff`.
    #[cfg(feature = "git")]
    WorktreeGoto,
    /// Worktree list: open the selected worktree in a new (foreground) tab, leaving this tab (and
    /// its open worktree list) untouched.
    #[cfg(feature = "git")]
    WorktreeGotoNewTab,
    /// Worktree list: close it (returns to the changes hub).
    #[cfg(feature = "git")]
    WorktreeClose,
    /// Worktree list: show the selected worktree's diff since the base branch (committed +
    /// uncommitted together), overlaid as a detail view on top of the list.
    #[cfg(feature = "git")]
    WorktreeShowChanges,
    /// Worktree list: open the input dialog for a new linked worktree (`n`). The typed name is a
    /// branch name — `App::dialog_submit` auto-detects new-vs-existing (no separate prompt for it)
    /// and runs `git worktree add` under `[git] worktree_dir`, next to the main worktree.
    #[cfg(feature = "git")]
    WorktreeCreate,

    // --- Git copy (y→ commit info / branches branch name) ---
    /// log/graph/detail: copy the selected commit's info (hash / subject / message / author / date).
    #[cfg(feature = "git")]
    GitCopy(GitCopyKind),
    /// branches: copy the selected branch name.
    #[cfg(feature = "git")]
    CopyBranchName,

    // --- Git-related: close ---
    #[cfg(feature = "git")]
    GitClose,

    // --- Sort menu ---
    SortSet(SortKey),
    SortToggleReverse,
    SortToggleDirsFirst,

    // --- Tab list (`T`) ---
    ToggleTabList,
    TabListClose,
    ToggleOutline,
    // --- Bookmark list ---
    BookmarkJump,
    BookmarkEdit,
    BookmarkDelete,
    BookmarkClose,

    // --- Info ---
    InfoClose,

    // --- Table cell full-text popup (`Enter` in a table preview) ---
    /// Open/close the full-cell popup (mirrors `ToggleOutline`/`ToggleInfo`). The real trigger is
    /// the fixed `Enter` key (see `handle_enter`/`handle_esc` in main.rs, same pattern as
    /// `LinkOpen`); this variant exists so the action is nameable/config-registrable (e.g. `q`
    /// inside the popup closes it via this same action) and round-trips through
    /// `action_from_str`/`action_name`.
    ToggleTableCell,

    // --- The default operation of the fixed keys (`Enter`, `Esc`, `Space`; plugin stage 2 / B2) ---
    // These keys cannot be rebound, but they run through `dispatch_action` like every other key so
    // that the same guards apply (`writes_repository`, the `ops::gate` check) and so that the help
    // and the footer can name them from the same declaration.
    /// `Esc`: leave / cancel / clear whatever the frontmost context calls "the thing I am in"
    /// (close an overlay, clear a search or filter, go back one screen). What it does in each
    /// context is `handle_esc` in main.rs.
    Cancel,
    /// `Enter` in the tab list: switch to the selected tab.
    TabListActivate,
    /// `Enter` in the heading outline: jump to the selected heading.
    OutlineJump,
    /// `Enter` in the graph's branch panel: apply the visibility choice.
    #[cfg(feature = "git")]
    GitGraphPickerApply,
    /// `Space` in a decorated document: toggle the focused checkbox or `<details>`.
    ToggleFocusedItem,
    /// An action the open plugin declared (`Manifest::actions`). It has no config name: its key
    /// is the plugin's own (`[keys.<plugin id>]`), and it is always reachable from `Space`.
    Plugin(crate::plugins::PluginAct),
}

#[cfg(feature = "git")]
impl Action {
    /// Whether running this changes the repository.
    ///
    /// Not every backend can take one: konoma reads a jj repository and never writes to it, so a
    /// write asked of jj is answered with an explanation instead of a failure. Keeping the list
    /// here means a new write action is refused by default rather than by remembering to guard it.
    pub fn writes_repository(self) -> bool {
        matches!(
            self,
            Action::GitStage
                | Action::GitUnstage
                | Action::GitStageAll
                | Action::GitUnstageAll
                | Action::GitDiscard
                | Action::GitDiffDiscard
                | Action::GitCommit
                | Action::BranchCheckout
                | Action::BranchCreate
                | Action::BranchDelete
                | Action::WorktreeCreate
        )
    }
}

// =============================================================================
// Surface (the frontmost surface = the single source of truth). Replaces internal_mode in Stage 3.
// =============================================================================

/// The frontmost "surface" currently receiving keys. Four categories: fixed text input / confirm
/// modal / overlay / basic full-screen. Git-related surfaces exist only under `feature="git"` (§1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    // --- Fixed text input (keymap does not apply; intercepts character/editing keys) ---
    DialogInput,
    Filter,
    Search,
    Mark,
    #[cfg(feature = "git")]
    BranchFilter,
    #[cfg(feature = "git")]
    WorktreeFilter,
    /// The `/` filter of a list that shares the one list filter: the changes list, the log, the
    /// graph, the outline / page list, the tab list or the bookmark list (which one is
    /// `App::filtering_list`).
    ListFilter,

    // --- Confirm modal (y/n/c/m/! fixed) ---
    DialogConfirmDelete,
    DialogConfirmDrop,
    DialogRenamePreview,
    /// App-quit confirmation (`q`/`y`/Enter = quit, `n`/Esc = cancel).
    DialogConfirmQuit,
    /// Bookmark-overwrite confirmation (`y`/Enter = overwrite, `n`/Esc = cancel).
    DialogConfirmBookmark,

    // --- Overlays (keymap-driven) ---
    Help,
    Bookmarks,
    /// Tab-list overlay (`T`): j/k + Enter switch, `w` closes the selected tab.
    Tabs,
    /// Heading-outline overlay (`o` in a Markdown preview): j/k + Enter jump to a heading.
    Outline,
    Info,
    /// Full-cell popup (`Enter` in a table preview): the cursor cell's untruncated text, wrapped
    /// and scrollable (`j`/`k`/`g`/`G`/PageUp/PageDown), `q`/Esc/Enter close it.
    TableCell,
    #[cfg(feature = "git")]
    GitDetail,
    #[cfg(feature = "git")]
    GitLog,
    #[cfg(feature = "git")]
    GitGraph,
    #[cfg(feature = "git")]
    GitGraphPicker,
    #[cfg(feature = "git")]
    GitBranches,
    #[cfg(feature = "git")]
    GitChanges,
    /// The linked-worktree list (`w` from the changes hub).
    #[cfg(feature = "git")]
    GitWorktrees,

    // --- Basic full-screen surfaces (keymap-driven) ---
    Visual,
    Tree,
    PreviewText,
    /// Windowed (Code/Text) preview line-selection mode (`v`): `j`/`k` extend, `y` copies the lines.
    PreviewTextVisual,
    PreviewImage,
    /// CSV/TSV table preview (cell cursor + `y→` cell/row/column copy).
    PreviewTable,
    /// A plugin's free-drawing view (`View::Canvas`): the host keeps only a few keys here.
    PreviewCanvas,
    /// A plugin's canvas that asked for text input (`CanvasView::text_input`): every key but
    /// `Esc` (and every paste) goes to the plugin, none is looked up in the keymap.
    PluginInput,
    #[cfg(feature = "git")]
    PreviewGitDiff,
}

impl Surface {
    /// Whether this is a text-input surface (intercepts character/editing keys and does not apply the keymap).
    pub fn is_text_input(self) -> bool {
        match self {
            Surface::DialogInput
            | Surface::Filter
            | Surface::Search
            | Surface::Mark
            | Surface::PluginInput => true,
            #[cfg(feature = "git")]
            Surface::BranchFilter => true,
            Surface::ListFilter => true,
            #[cfg(feature = "git")]
            Surface::WorktreeFilter => true,
            _ => false,
        }
    }

    /// Whether this is a confirm-modal surface (handles y/n/c/m/! as fixed keys).
    pub fn is_modal_confirm(self) -> bool {
        matches!(
            self,
            Surface::DialogConfirmDelete
                | Surface::DialogConfirmDrop
                | Surface::DialogRenamePreview
                | Surface::DialogConfirmQuit
                | Surface::DialogConfirmBookmark
        )
    }

    /// Whether this is one of the version-control screens, which are opened for git or for jj.
    #[cfg(feature = "git")]
    pub fn is_vcs_screen(self) -> bool {
        matches!(
            self,
            Surface::GitDetail
                | Surface::GitLog
                | Surface::GitGraph
                | Surface::GitGraphPicker
                | Surface::GitBranches
                | Surface::GitChanges
                | Surface::GitWorktrees
        )
    }

    /// Whether this surface inherits the Global-layer keys such as tabs/help (= a keymap/Global-driven
    /// surface). Equivalent to `Context::of_surface(..).is_some()`; the old keymap's lookup used it.
    #[cfg(test)]
    pub fn allows_tabs(self) -> bool {
        !self.is_text_input() && !self.is_modal_confirm()
    }

    /// Whether this surface inherits the path-copy `y` leader (Tree/Preview surfaces only; Visual and
    /// Git surfaces do not). By default the relevant surface's ContextMap holds `y`=Leader directly, so
    /// resolve does not use this; it is kept to spell out the spec and as a test invariant.
    #[allow(dead_code)]
    pub fn inherits_copy_leader(self) -> bool {
        match self {
            Surface::Tree | Surface::PreviewText | Surface::PreviewImage => true,
            #[cfg(feature = "git")]
            Surface::PreviewGitDiff => true,
            _ => false,
        }
    }
}

// =============================================================================
// KeyPress / KeyChord / Binding
// =============================================================================

/// Normalized representation of a single keystroke. SHIFT is folded into the uppercase char and ALT is unused, so it holds only CONTROL (§1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub code: KeyCode,
    pub ctrl: bool,
}

impl KeyPress {
    /// A character key (no modifiers).
    pub fn ch(c: char) -> KeyPress {
        KeyPress {
            code: KeyCode::Char(c),
            ctrl: false,
        }
    }
    /// Ctrl + a character key.
    pub fn ctrl_ch(c: char) -> KeyPress {
        KeyPress {
            code: KeyCode::Char(c),
            ctrl: true,
        }
    }
    /// A special key (no modifiers).
    pub fn key(code: KeyCode) -> KeyPress {
        KeyPress { code, ctrl: false }
    }

    /// Normalize a crossterm key event (extracting only CONTROL).
    pub fn norm(ev: &KeyEvent) -> KeyPress {
        KeyPress {
            code: ev.code,
            ctrl: ev.modifiers.contains(KeyModifiers::CONTROL),
        }
    }

    /// How the key is written for a person: `Space`, `Enter`, `Tab`, `Ctrl-t`, `q`.
    pub fn label(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            other => format!("{other:?}"),
        };
        if self.ctrl {
            format!("Ctrl-{base}")
        } else {
            base
        }
    }

    /// Parse one config-string token. Handles the `ctrl-x`/`c-x` modifier, `space`, literals `0 $ ! …`,
    /// named keys (`tab enter up …`), and single characters (uppercase = SHIFT included).
    pub fn parse(s: &str) -> Result<KeyPress> {
        let s = s.trim();
        if s.is_empty() {
            bail!("empty key token");
        }
        let (ctrl, rest) = if let Some(r) = s.strip_prefix("ctrl-") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("c-") {
            (true, r)
        } else {
            (false, s)
        };
        if rest.is_empty() {
            bail!("missing key after modifier: {s}");
        }
        let code = Self::parse_code(rest)?;
        Ok(KeyPress { code, ctrl })
    }

    fn parse_code(s: &str) -> Result<KeyCode> {
        let lower = s.to_ascii_lowercase();
        let code = match lower.as_str() {
            "space" => KeyCode::Char(' '),
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" | "pagedn" => KeyCode::PageDown,
            _ => {
                // A single character (keeps its original case). Multiple characters is an invalid token.
                let mut chars = s.chars();
                let c = chars
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("empty key token"))?;
                if chars.next().is_some() {
                    bail!("unknown key token: {s}");
                }
                KeyCode::Char(c)
            }
        };
        Ok(code)
    }
}

/// A single keystroke or a two-keystroke chord (leader). In config it is 1-2 whitespace-separated tokens (§1.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChord {
    Single(KeyPress),
    Chord(KeyPress, KeyPress),
}

impl KeyChord {
    /// Parse a config string. Whitespace-separated, 1 token → Single, 2 tokens → Chord.
    pub fn parse(s: &str) -> Result<KeyChord> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        match tokens.len() {
            1 => Ok(KeyChord::Single(KeyPress::parse(tokens[0])?)),
            2 => Ok(KeyChord::Chord(
                KeyPress::parse(tokens[0])?,
                KeyPress::parse(tokens[1])?,
            )),
            n => bail!("expected 1 or 2 key tokens, got {n}: {s:?}"),
        }
    }
}

/// The value bound to a key: a direct command, or a leader (which-key) trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Run(Action),
    Leader(LeaderId),
    /// Only in a context's own table: this key is deliberately unbound in that context, so the
    /// binding it would inherit from layer 1 or 2 does not apply. (`global` still does.) Written
    /// by `noop`/`disabled` in config and for the few places where a context lacks a key its
    /// layers give.
    Unbound,
}

// =============================================================================
// Leader (which-key) definitions.
// =============================================================================

/// Leader kind. No anonymous leaders are created (avoids label-less which-key entries; §0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeaderId {
    /// `y` path copy.
    Copy,
    /// `Space` file management.
    File,
    /// `y` cell/row/column copy (CSV/TSV table preview).
    TableCopy,
    /// `y` commit-info copy (git log/graph/detail).
    #[cfg(feature = "git")]
    GitCopy,
    /// `y` copy of the branch list (the selected branch's name).
    #[cfg(feature = "git")]
    BranchCopy,
    /// `,` sorting and display options. Its content depends on where you are (the tree sorts, a
    /// preview wraps, a diff follows), so each context has its own menu.
    Options,
    /// `Space` outside the tree: every operation that acts in the context. Built from the
    /// operation declarations and `ops::gate` when it opens (`ops::menu`), not stored.
    Actions,
}

/// One leader-menu item (suffix key + command + i18n label).
#[derive(Debug, Clone)]
pub struct LeaderItem {
    pub key: KeyPress,
    pub action: Action,
    pub label: Msg,
}

/// The whole menu shown in the which-key popup.
#[derive(Debug, Clone)]
pub struct LeaderMenu {
    /// This menu's kind (redundant with the HashMap key, but kept for self-description).
    #[allow(dead_code)]
    pub id: LeaderId,
    pub title: Msg,
    pub items: Vec<LeaderItem>,
}

impl LeaderMenu {
    fn find(&self, kp: KeyPress) -> Option<Action> {
        self.items
            .iter()
            .find(|it| it.key == kp)
            .map(|it| it.action)
    }
    /// Override/add a suffix key (an existing item for the same key is replaced).
    fn set(&mut self, kp: KeyPress, action: Action) {
        let label = leader_label(action);
        self.items.retain(|it| it.key != kp);
        self.items.push(LeaderItem {
            key: kp,
            action,
            label,
        });
    }
    fn remove(&mut self, kp: KeyPress) {
        self.items.retain(|it| it.key != kp);
    }
}

// =============================================================================
// KeyMap (two layers: per-Surface + Global) and the resolution result.
// =============================================================================

/// Per-surface "key → value" table.
pub type ContextMap = HashMap<KeyPress, Binding>;

/// Paging profile. Corresponds to "vim"/"less" of `ui.keys` (§2 scheme).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyScheme {
    #[default]
    Vim,
    Less,
}

/// Choose the profile from the `ui.keys` string (anything but "less" is vim).
pub fn scheme_from_str(s: &str) -> KeyScheme {
    if s.eq_ignore_ascii_case("less") {
        KeyScheme::Less
    } else {
        KeyScheme::Vim
    }
}

pub use konoma_host_core::keys::KeysFileConfig;

/// One key conflict detected at startup (#17 / FR-8).
/// The startup flash (`App::keymap_report`) shows the count and the first conflict's key, `kept`
/// and `dropped`.
#[derive(Debug, Clone)]
pub struct KeyConflict {
    /// The surface of the context the conflict is in (a context is finer; `dropped` names it).
    pub surface: Surface,
    pub key: KeyPress,
    /// Description of the kept side (the default).
    pub kept: String,
    /// Description of the dropped override, with the `[keys.<context>]` table it was written in.
    pub dropped: String,
    pub reason: ConflictKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// A leader prefix key was overwritten by a single command.
    PrefixVsSingle,
    /// A surface-local binding stole a Global default key, making the Global function unreachable.
    GlobalShadow,
}

/// The result of `resolve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// A command to run immediately.
    Action(Action),
    /// Transition to the leader-pending state (shows which-key).
    EnterLeader(LeaderId),
    /// Not bound to any key (swallowed safely).
    Unbound,
}

/// The whole resolvable keymap. Built once at startup (off the draw path; §5 performance requirement).
///
/// Keys are looked up in layers, most specific first: the context's own table (`contexts`), the
/// tables of its view types (`types`, layer 2), the basic table (`basic`, layer 1), then `global`
/// (layer 0). See [`Context`]. Layer 3 (verbs shared by every context: `e`, `o`, ...) is not
/// materialized yet because today no verb key is bound identically in all the contexts that would
/// inherit it; B3 introduces it together with the new default keys.
#[derive(Debug, Clone)]
pub struct KeyMap {
    /// Each context's own table (layer 4 in the design's numbering): what it adds to, or does
    /// differently from, the layers below it.
    pub contexts: HashMap<Context, ContextMap>,
    /// Layer 2: keys shared by the contexts that show the same kind of thing.
    pub types: HashMap<ViewType, ContextMap>,
    /// Layer 1: keys every context with a cursor shares ([`Context::inherits_basic`]).
    pub basic: ContextMap,
    /// Layer 0: tabs/help etc. Every context inherits these as a fallback.
    pub global: ContextMap,
    /// The menus every context that opens them shares: `y`'s copy menus and the tree's `Space`
    /// file menu.
    pub leaders: HashMap<LeaderId, LeaderMenu>,
    /// A context's own version of a menu: the `,` menus (which differ per context) and the
    /// copies made when `[keys.<context>]` writes `y x` or `space x` for one context only. It
    /// wins over `leaders`; see [`KeyMap::menu`].
    pub ctx_menus: HashMap<(Context, LeaderId), LeaderMenu>,
    /// The operations each context has by default, whatever the config did to their keys: the
    /// `Space` menu lists them even when a key was moved away or lost to a conflict.
    pub default_ops: HashMap<Context, Vec<Action>>,
    pub conflicts: Vec<KeyConflict>,
    /// Warnings for ignored config such as unknown context names/actions or fixed-key rebinds (for the startup flash).
    pub warnings: Vec<String>,
    /// Old `[keys.<surface>]` names that were read, each with the context header(s) it is now
    /// written as: `("[keys.preview_text]", "[keys.text] + [keys.document]")`. For the startup flash.
    pub renames: Vec<(String, String)>,
}

// =============================================================================
// Context (the unit of `[keys.<context>]` and of key lookup) and the layers it inherits.
// =============================================================================

/// Which version-control backend a git-family screen is open for. The same screen can be opened
/// for git or for jj and the two get separate key tables (`git.changes` / `jj.changes`), so a key
/// can mean different things in the two later on.
#[cfg(feature = "git")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Git,
    Jj,
}

#[cfg(feature = "git")]
impl Backend {
    pub const ALL: [Backend; 2] = [Backend::Git, Backend::Jj];

    /// The id used as the first half of a dotted context name (`git.changes`).
    pub fn id(self) -> &'static str {
        match self {
            Backend::Git => "git",
            Backend::Jj => "jj",
        }
    }
}

/// The screens of the version-control views (the second half of `git.changes`).
#[cfg(feature = "git")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Changes,
    Log,
    Graph,
    GraphPicker,
    Branches,
    Worktrees,
    Detail,
}

#[cfg(feature = "git")]
impl Screen {
    pub const ALL: [Screen; 7] = [
        Screen::Changes,
        Screen::Log,
        Screen::Graph,
        Screen::GraphPicker,
        Screen::Branches,
        Screen::Worktrees,
        Screen::Detail,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Screen::Changes => "changes",
            Screen::Log => "log",
            Screen::Graph => "graph",
            Screen::GraphPicker => "graph_picker",
            Screen::Branches => "branches",
            Screen::Worktrees => "worktrees",
            Screen::Detail => "detail",
        }
    }

    /// The old (pre-context) surface name of this screen: `git_changes` for [`Screen::Changes`].
    fn old_name(self) -> String {
        format!("git_{}", self.name())
    }
}

/// A key context: the unit of a `[keys.<context>]` table and of key lookup (layer 4 of the key
/// design, `docs/FEATURE-PLUGIN-ARCHITECTURE.md` §10-3). It is what the user is looking at, finer
/// than a [`Surface`]: the text preview is one `Surface` but two contexts (`text` for source and
/// plain text, `document` for a decorated Markdown / Word / slide view), and the git screens are
/// one `Surface` each but get a context per backend.
///
/// The lookup order for a key is: the context's own table, then the tables of its view type
/// (layer 2, [`Context::view_types`]), then the basic table (layer 1, if
/// [`Context::inherits_basic`]), then `global` (layer 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Tree,
    TreeVisual,
    /// Windowed source / plain text, and previews that cannot be shown.
    Text,
    /// A decorated view: rendered Markdown, a Word document, slides, a mermaid diagram as text.
    Document,
    TextVisual,
    Image,
    Table,
    /// A view the plugin draws itself (cells and pictures). The host keeps layer 0, the verbs,
    /// `q`, `Esc`, `Space`, `,`, `R` / `J` / `K` / `O` and the base keys that are not motion
    /// (`y` `Y` `m` `'` `r` `Ctrl-n` `Ctrl-p` `Ctrl-t`: `keys::reserved("canvas")`), bound in its
    /// own table. It inherits neither the motion keys of layer 1 (`j` `k` are the plugin's) nor a
    /// layer-2 table.
    Canvas,
    TableCell,
    Bookmarks,
    Tabs,
    Outline,
    Info,
    Help,
    /// The file-diff preview (full-screen diff of one file).
    #[cfg(feature = "git")]
    Diff,
    /// One screen of the version-control views for one backend.
    #[cfg(feature = "git")]
    Screen(Backend, Screen),
}

/// A layer-2 table: keys shared by the contexts that show the same kind of thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewType {
    Text,
    /// Keys of decorated documents. A document is also text, so its lookup continues into
    /// [`ViewType::Text`].
    Document,
    Table,
    Image,
    #[cfg(feature = "git")]
    Diff,
    /// The version-control list screens.
    #[cfg(feature = "git")]
    List,
}

impl ViewType {
    #[cfg(test)]
    pub fn all() -> Vec<ViewType> {
        #[cfg_attr(not(feature = "git"), allow(unused_mut))]
        let mut v = vec![
            ViewType::Text,
            ViewType::Document,
            ViewType::Table,
            ViewType::Image,
        ];
        #[cfg(feature = "git")]
        {
            v.push(ViewType::Diff);
            v.push(ViewType::List);
        }
        v
    }
}

impl Context {
    /// Every context of this build, in a stable order.
    pub fn all() -> Vec<Context> {
        #[cfg_attr(not(feature = "git"), allow(unused_mut))]
        let mut v = vec![
            Context::Tree,
            Context::TreeVisual,
            Context::Text,
            Context::Document,
            Context::TextVisual,
            Context::Image,
            Context::Table,
            Context::Canvas,
            Context::TableCell,
            Context::Bookmarks,
            Context::Tabs,
            Context::Outline,
            Context::Info,
            Context::Help,
        ];
        #[cfg(feature = "git")]
        {
            v.push(Context::Diff);
            for b in Backend::ALL {
                for s in Screen::ALL {
                    v.push(Context::Screen(b, s));
                }
            }
        }
        v
    }

    /// The name used in `[keys.<name>]` (dotted for a backend screen: `git.changes`). Exhaustive
    /// on purpose: a new context does not compile until it has a name.
    pub fn name(self) -> String {
        match self {
            Context::Tree => "tree".into(),
            Context::TreeVisual => "tree_visual".into(),
            Context::Text => "text".into(),
            Context::Document => "document".into(),
            Context::TextVisual => "text_visual".into(),
            Context::Image => "image".into(),
            Context::Table => "table".into(),
            Context::Canvas => "canvas".into(),
            Context::TableCell => "table_cell".into(),
            Context::Bookmarks => "bookmarks".into(),
            Context::Tabs => "tabs".into(),
            Context::Outline => "outline".into(),
            Context::Info => "info".into(),
            Context::Help => "help".into(),
            #[cfg(feature = "git")]
            Context::Diff => "diff".into(),
            #[cfg(feature = "git")]
            Context::Screen(b, s) => format!("{}.{}", b.id(), s.name()),
        }
    }

    /// The reverse of [`Context::name`]. Unknown names (including the old surface names, see
    /// [`old_surface_contexts`]) return `None`.
    pub fn from_name(name: &str) -> Option<Context> {
        Context::all().into_iter().find(|c| c.name() == name)
    }

    /// The `[keys.<name>]` header as it must be written in TOML (a dotted name needs quotes).
    pub fn header(self) -> String {
        let n = self.name();
        if n.contains('.') {
            format!("[keys.\"{n}\"]")
        } else {
            format!("[keys.{n}]")
        }
    }

    /// The layer-2 tables this context looks into, most specific first.
    pub fn view_types(self) -> &'static [ViewType] {
        match self {
            Context::Text | Context::TextVisual => &[ViewType::Text],
            Context::Document => &[ViewType::Document, ViewType::Text],
            Context::Image => &[ViewType::Image],
            Context::Table => &[ViewType::Table],
            Context::Tree
            | Context::TreeVisual
            | Context::Canvas
            | Context::TableCell
            | Context::Bookmarks
            | Context::Tabs
            | Context::Outline
            | Context::Info
            | Context::Help => &[],
            #[cfg(feature = "git")]
            Context::Diff => &[ViewType::Diff],
            #[cfg(feature = "git")]
            Context::Screen(_, Screen::Detail) => &[ViewType::Text],
            #[cfg(feature = "git")]
            Context::Screen(..) => &[ViewType::List],
        }
    }

    /// Whether the basic table (layer 1) applies. Everything that has a cursor does; the info
    /// popup is a one-shot picker without one.
    pub fn inherits_basic(self) -> bool {
        !matches!(self, Context::Info | Context::Canvas)
    }

    /// The keymap [`Surface`] this context is a refinement of (the surface that [`Context::of_surface`]
    /// maps to it).
    pub fn surface(self) -> Surface {
        match self {
            Context::Tree => Surface::Tree,
            Context::TreeVisual => Surface::Visual,
            Context::Text | Context::Document => Surface::PreviewText,
            Context::TextVisual => Surface::PreviewTextVisual,
            Context::Image => Surface::PreviewImage,
            Context::Table => Surface::PreviewTable,
            Context::Canvas => Surface::PreviewCanvas,
            Context::TableCell => Surface::TableCell,
            Context::Bookmarks => Surface::Bookmarks,
            Context::Tabs => Surface::Tabs,
            Context::Outline => Surface::Outline,
            Context::Info => Surface::Info,
            Context::Help => Surface::Help,
            #[cfg(feature = "git")]
            Context::Diff => Surface::PreviewGitDiff,
            #[cfg(feature = "git")]
            Context::Screen(_, s) => match s {
                Screen::Changes => Surface::GitChanges,
                Screen::Log => Surface::GitLog,
                Screen::Graph => Surface::GitGraph,
                Screen::GraphPicker => Surface::GitGraphPicker,
                Screen::Branches => Surface::GitBranches,
                Screen::Worktrees => Surface::GitWorktrees,
                Screen::Detail => Surface::GitDetail,
            },
        }
    }

    /// The context for the frontmost `sfc`, or `None` for a surface that does not use the keymap
    /// (text input and confirm modals). `document` says the text preview is a decorated view;
    /// `jj` says a version-control screen is open for jj rather than git. **The one place that
    /// decides the context**: `App::key_context` supplies the two facts and key lookup
    /// (`handle_key`) uses nothing else. Exhaustive over `Surface` on purpose.
    pub fn of_surface(sfc: Surface, document: bool, jj: bool) -> Option<Context> {
        #[cfg(feature = "git")]
        let backend = if jj { Backend::Jj } else { Backend::Git };
        #[cfg(not(feature = "git"))]
        let _ = jj;
        Some(match sfc {
            Surface::DialogInput
            | Surface::Filter
            | Surface::Search
            | Surface::Mark
            | Surface::PluginInput
            | Surface::DialogConfirmDelete
            | Surface::DialogConfirmDrop
            | Surface::DialogRenamePreview
            | Surface::DialogConfirmQuit
            | Surface::DialogConfirmBookmark => return None,
            Surface::ListFilter => return None,
            #[cfg(feature = "git")]
            Surface::BranchFilter | Surface::WorktreeFilter => return None,
            Surface::Help => Context::Help,
            Surface::Bookmarks => Context::Bookmarks,
            Surface::Tabs => Context::Tabs,
            Surface::Outline => Context::Outline,
            Surface::Info => Context::Info,
            Surface::TableCell => Context::TableCell,
            #[cfg(feature = "git")]
            Surface::GitDetail => Context::Screen(backend, Screen::Detail),
            #[cfg(feature = "git")]
            Surface::GitLog => Context::Screen(backend, Screen::Log),
            #[cfg(feature = "git")]
            Surface::GitGraph => Context::Screen(backend, Screen::Graph),
            #[cfg(feature = "git")]
            Surface::GitGraphPicker => Context::Screen(backend, Screen::GraphPicker),
            #[cfg(feature = "git")]
            Surface::GitBranches => Context::Screen(backend, Screen::Branches),
            #[cfg(feature = "git")]
            Surface::GitChanges => Context::Screen(backend, Screen::Changes),
            #[cfg(feature = "git")]
            Surface::GitWorktrees => Context::Screen(backend, Screen::Worktrees),
            Surface::Visual => Context::TreeVisual,
            Surface::Tree => Context::Tree,
            Surface::PreviewText => {
                if document {
                    Context::Document
                } else {
                    Context::Text
                }
            }
            Surface::PreviewTextVisual => Context::TextVisual,
            Surface::PreviewImage => Context::Image,
            Surface::PreviewTable => Context::Table,
            Surface::PreviewCanvas => Context::Canvas,
            #[cfg(feature = "git")]
            Surface::PreviewGitDiff => Context::Diff,
        })
    }
}

/// Whether `[keys.<name>]` is a table of the host (a context, `global`, the old `sort`, or an old
/// surface name) rather than the keys of a plugin of that name.
pub fn is_host_key_table(name: &str) -> bool {
    name == "global"
        || name == "sort"
        || Context::from_name(name).is_some()
        || old_surface_contexts(name).is_some()
}

/// The contexts an **old** (pre-context) `[keys.<surface>]` name stands for, for the names that
/// changed. A table under an old name is read as if it were written under each of these. Names
/// that did not change (`tree`, `sort`, ...) are ordinary context names and are not listed.
fn old_surface_contexts(name: &str) -> Option<Vec<Context>> {
    match name {
        "preview_text" => return Some(vec![Context::Text, Context::Document]),
        "preview_text_visual" => return Some(vec![Context::TextVisual]),
        "preview_image" => return Some(vec![Context::Image]),
        "preview_table" => return Some(vec![Context::Table]),
        #[cfg(feature = "git")]
        "preview_git_diff" => return Some(vec![Context::Diff]),
        _ => {}
    }
    #[cfg(feature = "git")]
    for s in Screen::ALL {
        if name == s.old_name() {
            return Some(
                Backend::ALL
                    .iter()
                    .map(|&b| Context::Screen(b, s))
                    .collect(),
            );
        }
    }
    None
}

impl KeyMap {
    /// Build the built-in default keymap (§2). `scheme` only swaps the page/half rows of the
    /// scrolling contexts.
    ///
    /// The default keys follow the layers of the key design (`docs/FEATURE-PLUGIN-ARCHITECTURE.md`
    /// §10-3): layer 1 (`basic`) holds what every context with a cursor shares, layer 2 (`types`)
    /// what contexts showing the same kind of thing share, and each context's own table holds the
    /// rest. `defaults_tests` pins the result key by key.
    pub fn defaults(scheme: KeyScheme) -> KeyMap {
        let nav = |m: Motion| Binding::Run(Action::Navigate(m));
        let run = |a: Action| Binding::Run(a);

        // The movement every scrolling view shares (text, table, diff). Paging follows `ui.keys`.
        let scroll = || -> ContextMap {
            let mut m: ContextMap = HashMap::new();
            m.insert(KeyPress::ch('g'), nav(Motion::Top));
            m.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            m.insert(KeyPress::ch('h'), nav(Motion::Left));
            m.insert(KeyPress::ch('l'), nav(Motion::Right));
            m.insert(KeyPress::ch('0'), nav(Motion::LineHome));
            m.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
            m.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
            m.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
            apply_scheme_paging(&mut m, scheme);
            m
        };

        // =====================================================================
        // Layer 1: the same in every context that has a cursor (`inherits_basic`).
        // =====================================================================
        let mut basic: ContextMap = HashMap::new();
        basic.insert(KeyPress::ch('j'), nav(Motion::Down));
        basic.insert(KeyPress::ch('k'), nav(Motion::Up));
        // `Space` = the menu of this context's operations (the tree and its range selection own
        // the file menu instead). It opens only when the context has an operation that acts.
        basic.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::Actions));

        // =====================================================================
        // Layer 2: per view type.
        // =====================================================================
        let mut types: HashMap<ViewType, ContextMap> = HashMap::new();

        // Text (text, document, text_visual, the commit detail): the scrolling keys.
        types.insert(ViewType::Text, scroll());
        // Document: nothing of its own yet (a document is also text, so lookup continues into
        // Text). B3 puts the document keys (outline, focus, representation cycle) here.
        types.insert(ViewType::Document, HashMap::new());
        // Table: the cell cursor moves with the scrolling keys.
        types.insert(ViewType::Table, scroll());

        // Image: pan with h/l (j/k from layer 1), zoom, and the page of a multi-page file.
        let mut image: ContextMap = HashMap::new();
        image.insert(KeyPress::ch('h'), nav(Motion::Left));
        image.insert(KeyPress::ch('l'), nav(Motion::Right));
        image.insert(KeyPress::ch('+'), run(Action::ImageZoomIn));
        image.insert(KeyPress::ch('-'), run(Action::ImageZoomOut));
        image.insert(KeyPress::ch('='), run(Action::ImageZoomReset));
        // PDF page paging (the handler no-ops for non-PDF). lowercase jk = pan within the page /
        // uppercase JK = move to the next/previous page.
        image.insert(KeyPress::ch('J'), run(Action::PageNext));
        image.insert(KeyPress::ch('K'), run(Action::PagePrev));
        image.insert(KeyPress::key(KeyCode::PageDown), run(Action::PageNext));
        image.insert(KeyPress::key(KeyCode::PageUp), run(Action::PagePrev));
        // `O` = the list of the pages (a PDF with 2+ pages; the gate refuses elsewhere).
        image.insert(KeyPress::ch('O'), run(Action::ToggleOutline));
        types.insert(ViewType::Image, image);

        #[cfg(feature = "git")]
        {
            // Diff: scrolling, the layout / representation switches, and the media page turn.
            let mut diff = scroll();
            diff.insert(KeyPress::ch('s'), run(Action::CycleDiffLayout));
            diff.insert(KeyPress::ch('R'), run(Action::CycleDiffView));
            // J/K: turn a media diff's PDF page (both sides at once). No-op outside a multi-page
            // PDF side-by-side view (`App::media_diff_page_turn`'s own gate) — PageDown/PageUp are
            // *not* rebound here: `dispatch_navigate`'s `PreviewGitDiff` arm reroutes them to the
            // same handler only while the media diff is active, and keeps their ordinary scroll
            // meaning otherwise (`docs/FEATURE-MEDIA-DIFF.md` §6).
            diff.insert(KeyPress::ch('J'), run(Action::PageNext));
            diff.insert(KeyPress::ch('K'), run(Action::PagePrev));
            types.insert(ViewType::Diff, diff);

            // List screens: jump to the first / last row.
            let mut list: ContextMap = HashMap::new();
            list.insert(KeyPress::ch('g'), nav(Motion::Top));
            list.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            types.insert(ViewType::List, list);
        }

        // =====================================================================
        // The contexts' own tables.
        // =====================================================================
        let mut contexts: HashMap<Context, ContextMap> = HashMap::new();

        // --- Tree (normal) ---
        let mut tree: ContextMap = HashMap::new();
        tree.insert(KeyPress::ch('g'), nav(Motion::Top));
        tree.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        tree.insert(KeyPress::ctrl_ch('d'), nav(Motion::HalfDown));
        tree.insert(KeyPress::ctrl_ch('u'), nav(Motion::HalfUp));
        tree.insert(KeyPress::ctrl_ch('f'), nav(Motion::PageDown));
        tree.insert(KeyPress::ctrl_ch('b'), nav(Motion::PageUp));
        tree.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        tree.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        tree.insert(KeyPress::ch('h'), run(Action::TreeLeave));
        tree.insert(KeyPress::ch('l'), run(Action::TreeDescend));
        tree.insert(KeyPress::ch('q'), run(Action::CloseTabOrQuit));
        tree.insert(KeyPress::ch('/'), run(Action::FilterStart));
        tree.insert(KeyPress::ch('.'), run(Action::ToggleHidden));
        tree.insert(KeyPress::ch('r'), run(Action::Refresh));
        // `s` (sort menu) and `p` (path style) have no default key: both live in the `,` menu.
        // The actions stay for `[keys]`: `open_sort_menu` opens the `,` menu, `cycle_path_style`
        // runs the path style.
        tree.insert(KeyPress::ch(','), Binding::Leader(LeaderId::Options));
        tree.insert(KeyPress::ch('i'), run(Action::ToggleInfo));
        tree.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        // `o` = open in another app (layer 3): the Office apps for an Office file, else the OS
        // default. `e` edits, `o` opens.
        tree.insert(KeyPress::ch('o'), run(Action::OpenExternal));
        tree.insert(KeyPress::ch('m'), run(Action::MarkSet));
        tree.insert(KeyPress::ch('\''), run(Action::MarkJump));
        tree.insert(KeyPress::ch('d'), run(Action::OpenGitDiffCursor));
        // `p` = the project's screen (the Git hub for now); it was `o`.
        tree.insert(KeyPress::ch('p'), run(Action::OpenGitView));
        // `!` = the project's own tool (lazygit / lazyjj), from the tree as from a preview.
        #[cfg(feature = "git")]
        tree.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
        tree.insert(KeyPress::ch('a'), run(Action::SetAnchor));
        tree.insert(KeyPress::ch('A'), run(Action::ResetAnchor));
        tree.insert(KeyPress::ch('v'), run(Action::EnterVisual));
        tree.insert(KeyPress::ch('V'), run(Action::ToggleSelect));
        // Agent Watch: C = show only changed files / n·N = jump to the next/previous changed file.
        tree.insert(KeyPress::ch('C'), run(Action::ToggleChangedFilter));
        tree.insert(KeyPress::ch('n'), run(Action::JumpNextChange));
        tree.insert(KeyPress::ch('N'), run(Action::JumpPrevChange));
        tree.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        tree.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::File));
        // Ctrl-t = open the entry under the cursor in a new tab (file = preview / directory = the
        // new tab's root). Same key/meaning ("open this in a new tab") as preview's OpenLinkNewTab
        // — paired with global t (new tab).
        tree.insert(KeyPress::ctrl_ch('t'), run(Action::OpenInNewTab));
        contexts.insert(Context::Tree, tree);

        // --- Tree:Visual ---
        let mut visual: ContextMap = HashMap::new();
        visual.insert(KeyPress::ch('g'), nav(Motion::Top));
        visual.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        visual.insert(KeyPress::ctrl_ch('d'), nav(Motion::HalfDown));
        visual.insert(KeyPress::ctrl_ch('u'), nav(Motion::HalfUp));
        visual.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        visual.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        visual.insert(KeyPress::ch('v'), run(Action::VisualCommit));
        visual.insert(KeyPress::ch('a'), run(Action::VisualSelectSiblings));
        visual.insert(KeyPress::ch('A'), run(Action::VisualSelectAll));
        visual.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::File));
        // `q` cancels the selection, like `Esc` (it used to quit the app: the one exception to
        // layer 1's "q = go back one step").
        visual.insert(KeyPress::ch('q'), run(Action::Cancel));
        contexts.insert(Context::TreeVisual, visual);

        // --- Preview: text / markdown. `text` and `document` carry the same keys for now (they
        // were one table); B3 tells them apart. ---
        let mut ptext: ContextMap = HashMap::new();
        ptext.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        ptext.insert(KeyPress::ch('/'), run(Action::SearchStart));
        ptext.insert(KeyPress::ch('n'), run(Action::SearchNext));
        ptext.insert(KeyPress::ch('N'), run(Action::SearchPrev));
        // `p` is the project's screen on every context (below); the path style is in the `,` menu.
        ptext.insert(KeyPress::ch(','), Binding::Leader(LeaderId::Options));
        ptext.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        ptext.insert(KeyPress::ch('o'), run(Action::OpenExternal));
        ptext.insert(KeyPress::ch('p'), run(Action::OpenGitView));
        #[cfg(feature = "git")]
        ptext.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
        ptext.insert(KeyPress::ch('r'), run(Action::Refresh));
        ptext.insert(KeyPress::ch('v'), run(Action::PreviewEnterVisual));
        ptext.insert(KeyPress::ch('V'), run(Action::PreviewEnterVisualLine));
        ptext.insert(KeyPress::ch('R'), run(Action::CycleRepresentation));
        // Ctrl-t = open the focused Markdown link in a new tab (Enter still opens in the same tab).
        // Consistent with the TUI convention (fzf/Telescope's Ctrl-t = new tab) + konoma's t = new
        // tab. Ctrl+letter is reliable in every terminal + tmux (unlike Ctrl+Enter, no kitty
        // protocol required). No-op when no link is focused.
        ptext.insert(KeyPress::ctrl_ch('t'), run(Action::OpenLinkNewTab));
        // Ctrl-n/Ctrl-p = preview the next/previous file in tree display order (file paging). J/K
        // conflict with the image context's PDF page paging, and Ctrl-j is indistinguishable from
        // Enter (LF) on legacy terminals/tmux, so neither is used.
        ptext.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        ptext.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        // Y = copy an @path#L reference for the caret line (hands the location to Claude Code). If
        // there's a selection, the range is used instead (Y in the visual context).
        ptext.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        // +/-/= : zoom/reset the focused inline mermaid diagram in place (the display area stays
        // fixed). No-op when no diagram is focused (guarded on the image_zoom_by side). `0` is kept
        // as-is for LineHome (go to line start).
        ptext.insert(KeyPress::ch('+'), run(Action::ImageZoomIn));
        ptext.insert(KeyPress::ch('-'), run(Action::ImageZoomOut));
        ptext.insert(KeyPress::ch('='), run(Action::ImageZoomReset));
        ptext.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        // Bookmarks: `m` registers the previewed file, `'` opens the list (same action as the tree).
        ptext.insert(KeyPress::ch('m'), run(Action::MarkSet));
        ptext.insert(KeyPress::ch('\''), run(Action::MarkJump));
        // Heading outline (Markdown preview). `O` toggles it open/closed (`o` is the verb "open").
        ptext.insert(KeyPress::ch('O'), run(Action::ToggleOutline));
        // J/K = next/previous slide of a presentation (the same keys as the PDF page / sheet turn;
        // `App::page_turn` does nothing for any other text preview).
        ptext.insert(KeyPress::ch('J'), run(Action::PageNext));
        ptext.insert(KeyPress::ch('K'), run(Action::PagePrev));
        contexts.insert(Context::Document, ptext.clone());
        contexts.insert(Context::Text, ptext);

        // --- Preview: text/code visual selection (v charwise / V linewise) ---
        // h/j/k/l move the 2D caret to extend the range, `y` copies it, and v/V/q/Esc exit.
        let mut pvis: ContextMap = HashMap::new();
        pvis.insert(KeyPress::ch('y'), run(Action::PreviewCopySelection));
        pvis.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        pvis.insert(KeyPress::ch('v'), run(Action::PreviewExitVisual));
        pvis.insert(KeyPress::ch('V'), run(Action::PreviewExitVisual));
        pvis.insert(KeyPress::ch('q'), run(Action::PreviewExitVisual));
        contexts.insert(Context::TextVisual, pvis);

        // --- Preview: image ---
        let mut pimg: ContextMap = HashMap::new();
        pimg.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        // R: while this image/PDF/SVG preview *is* the diff's own `Preview` representation
        // (`PerTab::preview_from_diff`), returns to the diff; a no-op otherwise
        // (`App::image_return_to_diff`'s own doc comment — **not** `Action::CycleRepresentation`,
        // whose non-diff fallthrough (`toggle_md_raw`) is a real, if unadvertised, bug on this
        // context for a standalone `.mmd` image preview, `docs/FEATURE-MEDIA-DIFF.md` §6).
        pimg.insert(KeyPress::ch('R'), run(Action::ImageReturnToDiff));
        pimg.insert(KeyPress::ch(','), Binding::Leader(LeaderId::Options));
        pimg.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        pimg.insert(KeyPress::ch('o'), run(Action::OpenExternal));
        pimg.insert(KeyPress::ch('p'), run(Action::OpenGitView));
        #[cfg(feature = "git")]
        pimg.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
        pimg.insert(KeyPress::ch('r'), run(Action::Refresh));
        // Ctrl-n/Ctrl-p = file paging (J/K are already assigned to PDF page paging, hence Ctrl here).
        pimg.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        pimg.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        pimg.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        pimg.insert(KeyPress::ch('m'), run(Action::MarkSet));
        pimg.insert(KeyPress::ch('\''), run(Action::MarkJump));
        contexts.insert(Context::Image, pimg);

        // --- Preview: table (csv/tsv) ---
        // hjkl = move the cell cursor / g·G = first/last row / 0·$ = first/last column / y→ = copy
        // the cell/row/column.
        let mut ptbl: ContextMap = HashMap::new();
        ptbl.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        // J/K = next/previous sheet of a spreadsheet (the same keys as the PDF page turn; the
        // handler no-ops unless the workbook has 2+ visible sheets, so CSV/archive tables ignore them).
        ptbl.insert(KeyPress::ch('J'), run(Action::PageNext));
        ptbl.insert(KeyPress::ch('K'), run(Action::PagePrev));
        // `O` = the list of the sheets of a workbook (2+ visible sheets; the gate refuses elsewhere).
        ptbl.insert(KeyPress::ch('O'), run(Action::ToggleOutline));
        // R = the next representation of a table plugin that declares several (the host walks
        // them; nothing happens, and no hint shows, for a table with one).
        ptbl.insert(KeyPress::ch('R'), run(Action::CycleRepresentation));
        ptbl.insert(KeyPress::ch(','), Binding::Leader(LeaderId::Options));
        ptbl.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        ptbl.insert(KeyPress::ch('o'), run(Action::OpenExternal));
        ptbl.insert(KeyPress::ch('p'), run(Action::OpenGitView));
        #[cfg(feature = "git")]
        ptbl.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
        ptbl.insert(KeyPress::ch('r'), run(Action::Refresh));
        // Ctrl-n/Ctrl-p = file paging (same keys as text/image).
        ptbl.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        ptbl.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        // In-table search (same `/` n N as the text preview). The cursor jumps to the matching cell.
        ptbl.insert(KeyPress::ch('/'), run(Action::SearchStart));
        ptbl.insert(KeyPress::ch('n'), run(Action::SearchNext));
        ptbl.insert(KeyPress::ch('N'), run(Action::SearchPrev));
        ptbl.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::TableCopy));
        ptbl.insert(KeyPress::ch('m'), run(Action::MarkSet));
        ptbl.insert(KeyPress::ch('\''), run(Action::MarkJump));
        contexts.insert(Context::Table, ptbl);

        // --- Preview: a plugin's canvas ---
        // A key means the same thing everywhere: the host keeps layer 0 (the `global` table), the
        // base keys every file preview shares that are not motion (`y` `Y` copy, `m` `'`
        // bookmarks, `r` reload, `Ctrl-n` / `Ctrl-p` file paging, `Ctrl-t`), the verbs (`e` `o` `p`
        // `!`), `q`, `Esc` (a fixed key), `Space` (the menu of the plugin's operations), `,`, and
        // `R` `J` `K` `O` (a canvas that is one representation or one page can be left). It gives
        // up the type layer (scrolling, searching, selecting) and the navigation fixed keys
        // (arrows, Home / End, PageUp / PageDown, Enter, Tab) to the plugin: that is
        // `keys::reserved("canvas")`. The context does not inherit the basic table (`j` `k` are
        // the plugin's).
        let mut pcvs: ContextMap = HashMap::new();
        pcvs.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        pcvs.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::Actions));
        pcvs.insert(KeyPress::ch(','), Binding::Leader(LeaderId::Options));
        pcvs.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        pcvs.insert(KeyPress::ch('o'), run(Action::OpenExternal));
        pcvs.insert(KeyPress::ch('p'), run(Action::OpenGitView));
        #[cfg(feature = "git")]
        pcvs.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
        pcvs.insert(KeyPress::ch('R'), run(Action::CycleRepresentation));
        pcvs.insert(KeyPress::ch('J'), run(Action::PageNext));
        pcvs.insert(KeyPress::ch('K'), run(Action::PagePrev));
        pcvs.insert(KeyPress::ch('O'), run(Action::ToggleOutline));
        pcvs.insert(KeyPress::ch('r'), run(Action::Refresh));
        pcvs.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        pcvs.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        pcvs.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        pcvs.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        pcvs.insert(KeyPress::ch('m'), run(Action::MarkSet));
        pcvs.insert(KeyPress::ch('\''), run(Action::MarkJump));
        contexts.insert(Context::Canvas, pcvs);

        // --- Version-control contexts (feature gate) ---
        #[cfg(feature = "git")]
        {
            // Preview: gitdiff
            let mut pgit: ContextMap = HashMap::new();
            pgit.insert(KeyPress::ch('q'), run(Action::PreviewBack));
            pgit.insert(KeyPress::ch('x'), run(Action::GitDiffDiscard));
            // `e` opens the editor at the change block on screen; `o` opens the file in another
            // app; `p` / `!` reach the project's screen and tool; `r` reads the diff again.
            pgit.insert(KeyPress::ch('e'), run(Action::RequestEdit));
            pgit.insert(KeyPress::ch('o'), run(Action::OpenExternal));
            pgit.insert(KeyPress::ch('p'), run(Action::OpenGitView));
            pgit.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
            pgit.insert(KeyPress::ch('r'), run(Action::Refresh));
            // `n` / `N` = the next / previous change block of this file (layer 1).
            pgit.insert(KeyPress::ch('n'), run(Action::DiffNextChange));
            pgit.insert(KeyPress::ch('N'), run(Action::DiffPrevChange));
            // The follow scope (`toggle_follow_diff_scope`) has no default key: it lives in the
            // `,` menu (its old `f` was the less-style page key).
            pgit.insert(KeyPress::ch(','), Binding::Leader(LeaderId::Options));
            // Ctrl-n / Ctrl-p = switch to the next/previous changed file's diff (cycle through
            // changes without leaving the view; layer 1's "next/previous file"). `n` / `N` are
            // the next / previous change block within the file.
            pgit.insert(KeyPress::ctrl_ch('n'), run(Action::JumpNextChange));
            pgit.insert(KeyPress::ctrl_ch('p'), run(Action::JumpPrevChange));
            pgit.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
            contexts.insert(Context::Diff, pgit);

            // The version-control screens: git and jj start with the same tables.
            for backend in Backend::ALL {
                // Changes hub. Enter→GitOpenSelectedDiff fires as a fixed key.
                let mut gchg: ContextMap = HashMap::new();
                gchg.insert(KeyPress::ch('s'), run(Action::GitStage));
                gchg.insert(KeyPress::ch('u'), run(Action::GitUnstage));
                gchg.insert(KeyPress::ch('S'), run(Action::GitStageAll));
                gchg.insert(KeyPress::ch('U'), run(Action::GitUnstageAll));
                gchg.insert(KeyPress::ch('x'), run(Action::GitDiscard));
                gchg.insert(KeyPress::ch('c'), run(Action::GitCommit));
                gchg.insert(KeyPress::ch('d'), run(Action::GitWorktreeDiff));
                gchg.insert(KeyPress::ch('b'), run(Action::GitOpenBranches));
                gchg.insert(KeyPress::ch('w'), run(Action::GitOpenWorktrees));
                // The log and the graph have no default key here: `g` / `l` are layer 1's
                // top and open (the screens' `Tab` cycle reaches them). The actions stay
                // (`git_open_log`, `git_open_graph`) for `[keys]`.
                // `l` opens the selected file's diff (`Enter`'s operation), `h` goes back.
                gchg.insert(KeyPress::ch('l'), run(Action::GitOpenSelectedDiff));
                gchg.insert(KeyPress::ch('h'), run(Action::GitClose));
                gchg.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
                // The row is a file: edit it, open it in another app, in a new tab; filter rows.
                gchg.insert(KeyPress::ch('e'), run(Action::RequestEdit));
                gchg.insert(KeyPress::ch('o'), run(Action::OpenExternal));
                gchg.insert(KeyPress::ctrl_ch('t'), run(Action::OpenInNewTab));
                gchg.insert(KeyPress::ch('/'), run(Action::ListFilterStart));
                gchg.insert(KeyPress::ch('r'), run(Action::Refresh));
                // jj has no staging area, and its changes screen syncs (fetch + rebase) on `s`
                // where git stages: the same key, a different meaning per backend.
                if backend == Backend::Jj {
                    gchg.insert(KeyPress::ch('s'), run(Action::JjSync));
                }
                gchg.insert(KeyPress::ch('q'), run(Action::GitClose));
                // y→ copies the selected changed file's path (reuses the same path-copy menu as the tree).
                gchg.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
                contexts.insert(Context::Screen(backend, Screen::Changes), gchg);

                // Git log / graph (identical). Enter→GitOpenDetail is a fixed key.
                let mut glog: ContextMap = HashMap::new();
                glog.insert(KeyPress::ch('l'), run(Action::GitOpenDetail));
                glog.insert(KeyPress::ch('h'), run(Action::GitClose));
                glog.insert(KeyPress::ch('q'), run(Action::GitClose));
                glog.insert(KeyPress::ch('/'), run(Action::ListFilterStart));
                glog.insert(KeyPress::ch('r'), run(Action::Refresh));
                // y→ copy commit info (short/full hash, subject, full message, author, date). Shared by
                // log/graph.
                glog.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::GitCopy));
                // The graph has base-branch pinning (Phase 2): s = set / x = clear / b = branch
                // visibility panel. log has none of this, hence a separate map.
                let mut ggraph = glog.clone();
                ggraph.insert(KeyPress::ch('s'), run(Action::GitGraphSetBase));
                ggraph.insert(KeyPress::ch('a'), run(Action::GitGraphToggleAll));
                // `x` only: `0` is layer 1's line start (it used to clear the base too).
                ggraph.insert(KeyPress::ch('x'), run(Action::GitGraphClearBase));
                ggraph.insert(KeyPress::ch('b'), run(Action::GitGraphOpenPicker));
                contexts.insert(Context::Screen(backend, Screen::Graph), ggraph);
                contexts.insert(Context::Screen(backend, Screen::Log), glog);

                // The graph's branch visibility panel: j/k/g/G navigate + x:toggle / a:all /
                // c:current-only (it was `n`, which is layer 1's next match). Enter and `l`:apply,
                // `q` and `h`: cancel (Esc is a fixed key). The toggle was `Space`, which is the
                // operation menu everywhere now.
                let mut gpick: ContextMap = HashMap::new();
                gpick.insert(KeyPress::ch('x'), run(Action::GitGraphPickerToggle));
                gpick.insert(KeyPress::ch('a'), run(Action::GitGraphPickerAll));
                gpick.insert(KeyPress::ch('c'), run(Action::GitGraphPickerCurrentOnly));
                gpick.insert(KeyPress::ch('l'), run(Action::GitGraphPickerApply));
                gpick.insert(KeyPress::ch('h'), run(Action::GitClose));
                // Reorder priority (uppercase J/K). Lowercase j/k move the cursor.
                gpick.insert(KeyPress::ch('K'), run(Action::GitGraphPickerMoveUp));
                gpick.insert(KeyPress::ch('J'), run(Action::GitGraphPickerMoveDown));
                gpick.insert(KeyPress::ch('q'), run(Action::GitClose));
                contexts.insert(Context::Screen(backend, Screen::GraphPicker), gpick);

                // Git branches. Enter→BranchCheckout is a fixed key.
                let mut gbr: ContextMap = HashMap::new();
                gbr.insert(KeyPress::ch('/'), run(Action::BranchFilterStart));
                gbr.insert(KeyPress::ch('r'), run(Action::Refresh));
                gbr.insert(KeyPress::ch('l'), run(Action::BranchCheckout));
                // `c` = create (`n` is layer 1's next match).
                gbr.insert(KeyPress::ch('c'), run(Action::BranchCreate));
                gbr.insert(KeyPress::ch('d'), run(Action::BranchDelete));
                gbr.insert(KeyPress::ch('h'), run(Action::GitClose));
                gbr.insert(KeyPress::ch('q'), run(Action::GitClose));
                // y→ the copy menu, whose one item is the selected branch's name.
                gbr.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::BranchCopy));
                contexts.insert(Context::Screen(backend, Screen::Branches), gbr);

                // Git worktrees (`w` from the changes hub). `Enter`→worktree_goto and `Esc`→
                // worktree_close (via handle_esc) are fixed keys (same idiom as branches' Enter→
                // checkout); `WorktreeGoto` has no default key binding here, same as
                // `GitOpenSelectedDiff` — it still round-trips through action_from_str/action_name so a
                // user can rebind another key to it. Ctrl-t (config-rebindable, unlike Enter) opens the
                // selection in a new tab. `c` opens the "new worktree" input dialog, same idiom as
                // `BranchCreate`'s `c` in `[keys."git.branches"]` above. `l` goes to the selected
                // worktree (`Enter`'s operation) and `h` back.
                let mut gwt: ContextMap = HashMap::new();
                gwt.insert(KeyPress::ch('/'), run(Action::WorktreeFilterStart));
                gwt.insert(KeyPress::ch('r'), run(Action::Refresh));
                gwt.insert(KeyPress::ch('c'), run(Action::WorktreeCreate));
                gwt.insert(KeyPress::ch('l'), run(Action::WorktreeGoto));
                gwt.insert(KeyPress::ch('h'), run(Action::WorktreeClose));
                gwt.insert(KeyPress::ctrl_ch('t'), run(Action::WorktreeGotoNewTab));
                gwt.insert(KeyPress::ch('d'), run(Action::WorktreeShowChanges));
                gwt.insert(KeyPress::ch('q'), run(Action::WorktreeClose));
                contexts.insert(Context::Screen(backend, Screen::Worktrees), gwt);

                // Git detail (the scrolling keys come from the text layer; page/half are swapped per scheme).
                let mut gdet: ContextMap = HashMap::new();
                gdet.insert(KeyPress::ch('s'), run(Action::CycleDiffLayout));
                gdet.insert(KeyPress::ch('q'), run(Action::GitClose));
                // y→ copy commit info (can copy while reading the full message).
                gdet.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::GitCopy));
                contexts.insert(Context::Screen(backend, Screen::Detail), gdet);
            }
        }

        // --- Bookmark list. Enter→BookmarkJump is a fixed key. Plain letters are reserved for
        // bookmark-name jumps (main's Unbound fallback), so editing is on a Ctrl-modified key and
        // deleting is only in the `Space` menu. ---
        let mut bm: ContextMap = HashMap::new();
        bm.insert(KeyPress::ctrl_ch('e'), run(Action::BookmarkEdit));
        // Deleting a bookmark is in the list's `Space` menu (`d`): `Ctrl-d` is layer 1's half page.
        bm.insert(KeyPress::ch('q'), run(Action::BookmarkClose));
        bm.insert(KeyPress::ch('\''), run(Action::BookmarkClose));
        // `/` filters the rows, like on every list. `h` and `l` stay *unbound* here, unlike the
        // other lists (layer 1's back / open): in this list every plain letter is the name of a
        // bookmark and jumps straight to it, and `h` / `l` can be bookmark names too.
        bm.insert(KeyPress::ch('/'), run(Action::ListFilterStart));
        contexts.insert(Context::Bookmarks, bm);

        // --- Tab list (`T` toggles it open/closed. Enter=switch is a fixed key; T closes it via
        // Global inheritance) ---
        let mut tl: ContextMap = HashMap::new();
        tl.insert(KeyPress::ch('d'), run(Action::TabListClose));
        tl.insert(KeyPress::ch('q'), run(Action::ToggleTabList));
        // Layer 1 on a list: `l` = open (`Enter`'s operation), `h` = back (`q`), `/` = filter.
        tl.insert(KeyPress::ch('l'), run(Action::TabListActivate));
        tl.insert(KeyPress::ch('h'), run(Action::ToggleTabList));
        tl.insert(KeyPress::ch('/'), run(Action::ListFilterStart));
        contexts.insert(Context::Tabs, tl);

        // --- Heading outline list (`O` toggles it open/closed. Enter=jump is a fixed key) ---
        let mut outline: ContextMap = HashMap::new();
        outline.insert(KeyPress::ch('g'), nav(Motion::Top));
        outline.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        outline.insert(KeyPress::ch('q'), run(Action::ToggleOutline));
        outline.insert(KeyPress::ch('O'), run(Action::ToggleOutline));
        // Layer 1 on a list: `l` = go to the heading / page (`Enter`'s operation), `h` = back
        // (`q`), `/` = filter.

        outline.insert(KeyPress::ch('l'), run(Action::OutlineJump));
        outline.insert(KeyPress::ch('h'), run(Action::ToggleOutline));
        outline.insert(KeyPress::ch('/'), run(Action::ListFilterStart));
        contexts.insert(Context::Outline, outline);

        // --- Info ---
        let mut info: ContextMap = HashMap::new();
        info.insert(KeyPress::ch('i'), run(Action::InfoClose));
        info.insert(KeyPress::ch('q'), run(Action::InfoClose));
        contexts.insert(Context::Info, info);

        // --- Table cell full-text popup (`Enter` in a table preview opens it; fixed key, see
        // handle_enter). j/k/g/G/PageUp/PageDown scroll the (possibly wrapped/long) cell text.
        // `q` closes (Enter/Esc also close via the fixed-key path). ---
        let mut tcell: ContextMap = HashMap::new();
        tcell.insert(KeyPress::ch('g'), nav(Motion::Top));
        tcell.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        tcell.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        tcell.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        tcell.insert(KeyPress::ch('q'), run(Action::ToggleTableCell));
        contexts.insert(Context::TableCell, tcell);

        // --- Help (?/Esc close it via Global/fixed keys. `q` closes it here too) ---
        let mut help: ContextMap = HashMap::new();
        help.insert(KeyPress::ch('g'), nav(Motion::Top));
        help.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        help.insert(KeyPress::ch('q'), run(Action::ToggleHelp));
        contexts.insert(Context::Help, help);

        // =====================================================================
        // Layer 0: global (inherited by every context).
        // =====================================================================
        let mut global: ContextMap = HashMap::new();
        // Q = quit the whole app. Every context inherits global, so Q exits from any of them.
        // Changeable via `[keys.global]`; conflicts are detected by validate() at startup.
        global.insert(KeyPress::ch('Q'), run(Action::Quit));
        // F = follow mode (auto-jump to externally changed files). Global so it can be toggled from
        // either Tree or Preview.
        global.insert(KeyPress::ch('F'), run(Action::ToggleFollow));
        // t = a new tab right of the active one (`tab_new_after`). Appending at the end
        // (`tab_new`) and the old `I` have no default key now; both actions stay for `[keys]`.
        global.insert(KeyPress::ch('t'), run(Action::TabNewAfter));
        global.insert(KeyPress::ch('T'), run(Action::ToggleTabList));
        // P = read a path/GitHub link from the clipboard and jump there (reveal + preview). Global
        // so it works from either Tree or Preview. Changeable via `[keys.global]`.
        global.insert(KeyPress::ch('P'), run(Action::PasteJump));
        // No default binding is placed on `w`: vim's word-motion habit makes it easy to trigger by
        // accident, so closing a tab is consolidated onto the tree's `q` (CloseTabOrQuit) (user
        // decision, 2026-07-07). The `tab_close` action itself still exists, so it can be revived
        // via `[keys.global] w = "tab_close"`.
        global.insert(KeyPress::ch('['), run(Action::TabPrev));
        global.insert(KeyPress::ch(']'), run(Action::TabNext));
        for i in 1..=9u8 {
            let c = char::from(b'0' + i);
            global.insert(KeyPress::ch(c), run(Action::TabGoto(i - 1)));
        }
        global.insert(KeyPress::ch('?'), run(Action::ToggleHelp));

        // --- Leaders ---
        let mut leaders: HashMap<LeaderId, LeaderMenu> = HashMap::new();
        leaders.insert(LeaderId::Copy, copy_leader_default());
        leaders.insert(LeaderId::File, file_leader_default());
        leaders.insert(LeaderId::TableCopy, table_copy_leader_default());
        #[cfg(feature = "git")]
        leaders.insert(LeaderId::GitCopy, git_copy_leader_default());
        #[cfg(feature = "git")]
        leaders.insert(LeaderId::BranchCopy, branch_copy_leader_default());

        // The `,` menus: one per context that has one.
        let mut ctx_menus: HashMap<(Context, LeaderId), LeaderMenu> = HashMap::new();
        for (c, menu) in options_menus_default() {
            ctx_menus.insert((c, LeaderId::Options), menu);
        }

        let mut km = KeyMap {
            contexts,
            types,
            basic,
            global,
            leaders,
            ctx_menus,
            default_ops: HashMap::new(),
            conflicts: Vec::new(),
            warnings: Vec::new(),
            renames: Vec::new(),
        };
        km.default_ops = Context::all()
            .into_iter()
            .map(|c| (c, km.bound_actions(c).into_iter().map(|(_, a)| a).collect()))
            .collect();
        km
    }

    /// defaults → merge config → validate conflicts (§6/§7). Always falls back and returns green (never panics).
    ///
    /// A `[keys.<name>]` table is read under its context name. The names from before contexts
    /// (`preview_text`, `git_changes`, ...) are still read: each is applied to every context it
    /// now covers, and noted in `renames` so the startup flash can say what to write instead. When a
    /// context is configured both ways, the new name wins.
    pub fn from_config(scheme: KeyScheme, cfg: &KeysFileConfig) -> KeyMap {
        let mut map = KeyMap::defaults(scheme);
        let defaults = KeyMap::defaults(scheme); // kept aside for rollback.
        let mut warnings: Vec<String> = Vec::new();
        let mut renames: Vec<(String, String)> = Vec::new();

        // Sorted, so the warnings and notes come out in the same order every run.
        let mut names: Vec<&String> = cfg.surfaces.keys().collect();
        names.sort();
        let mut old_tables: Vec<(Vec<KeyTarget>, &HashMap<String, String>)> = Vec::new();
        let mut new_tables: Vec<(Vec<KeyTarget>, &HashMap<String, String>)> = Vec::new();
        for name in names {
            let table = &cfg.surfaces[name];
            if name == "global" {
                new_tables.push((vec![KeyTarget::Global], table));
            } else if name == "sort" {
                old_tables.push((vec![KeyTarget::OldSort], table));
            } else if let Some(c) = Context::from_name(name) {
                new_tables.push((vec![KeyTarget::Context(c)], table));
            } else if let Some(cs) = old_surface_contexts(name) {
                let now = cs
                    .iter()
                    .map(|c| c.header())
                    .collect::<Vec<_>>()
                    .join(" + ");
                renames.push((format!("[keys.{name}]"), now));
                old_tables.push((cs.into_iter().map(KeyTarget::Context).collect(), table));
            } else {
                warnings.push(format!("unknown key surface: {name}"));
            }
        }
        // Old names first, so a table written under the new name overrides them. `global` goes
        // before the contexts: a menu entry in `global` reaches every copy of the menu, and a
        // context's own entry for the same key must come after it to win.
        new_tables.sort_by_key(|(targets, _)| !targets.contains(&KeyTarget::Global));
        for (targets, table) in old_tables.into_iter().chain(new_tables) {
            let mut entries: Vec<(&String, &String)> = table.iter().collect();
            entries.sort();
            for (chord_str, action_str) in entries {
                map.apply_one(&targets, chord_str, action_str, &mut warnings);
            }
        }

        // An old operation name that was renamed (not merely aliased by a merge): say what to write.
        if cfg
            .surfaces
            .values()
            .any(|t| t.values().any(|a| a == "toggle_markdown_raw"))
        {
            renames.push((
                "toggle_markdown_raw".to_string(),
                "cycle_representation".to_string(),
            ));
        }
        if cfg.surfaces.get("sort").is_some_and(|t| !t.is_empty()) {
            renames.push((
                "[keys.sort]".to_string(),
                "the `,` menu (e.g. [keys.tree] \", n\" = \"sort_name\")".to_string(),
            ));
        }
        map.validate(&defaults);
        map.warnings = warnings;
        map.renames = renames;
        map
    }

    /// Apply one (chord string → action string) to every target (§6 step ③). The entry is parsed
    /// and checked once, so a table that covers several contexts warns once, not once per
    /// context. On failure, warn and ignore.
    fn apply_one(
        &mut self,
        targets: &[KeyTarget],
        chord_str: &str,
        action_str: &str,
        warnings: &mut Vec<String>,
    ) {
        let chord = match KeyChord::parse(chord_str) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("invalid key {chord_str:?}: {e}"));
                return;
            }
        };
        let is_noop = action_str == "noop" || action_str == "disabled";
        // Explicitly warn if the config tries to bind the digit-fixed TabGoto (§1.2).
        if action_str == "tab_goto" {
            warnings.push("tab_goto is fixed to digit keys and cannot be rebound".into());
            return;
        }
        let action = if is_noop {
            None
        } else {
            match action_from_str(action_str) {
                Some(a) => Some(a),
                None => {
                    warnings.push(format!("unknown action: {action_str}"));
                    return;
                }
            }
        };

        match chord {
            KeyChord::Single(kp) => {
                if is_fixed_key(kp) {
                    warnings.push(format!("cannot rebind fixed key: {chord_str:?}"));
                    return;
                }
                for &target in targets {
                    self.set_key(target, kp, action);
                    // The old `[keys.sort]` table re-letters the sort menu; that menu is now the
                    // tree's `,` menu, so its sort entries (and removals) carry over to it.
                    if target == KeyTarget::OldSort
                        && action.is_none_or(|a| {
                            matches!(
                                a,
                                Action::SortSet(_)
                                    | Action::SortToggleReverse
                                    | Action::SortToggleDirsFirst
                            )
                        })
                    {
                        let _ = self.set_menu_entry(
                            KeyTarget::Context(Context::Tree),
                            KeyPress::ch(','),
                            kp,
                            action,
                        );
                    }
                }
            }
            KeyChord::Chord(pre, suf) => {
                if prefix_menu(pre).is_none() {
                    warnings.push(format!(
                        "unsupported chord prefix (only `space`, `y` and `,` open menus): {chord_str:?}"
                    ));
                    return;
                }
                // One entry may cover several contexts (an old table name does): it is refused
                // only when it reached none of them, and then it says so once.
                let mut reached = false;
                let mut why = String::new();
                for &target in targets {
                    match self.set_menu_entry(target, pre, suf, action) {
                        Ok(()) => reached = true,
                        Err(w) => why = w,
                    }
                }
                if !reached {
                    warnings.push(format!("{why}: {chord_str:?}"));
                }
            }
        }
    }

    /// Put `suf` (or remove it, `None`) in the menu that `pre` opens, for `target`. `global`
    /// changes the shared menu and every context's own copy of it (what `[keys.global] "y x"`
    /// always did); a context changes only its own copy of the menu `pre` opens *there* — the
    /// `y` of the tree and the `y` of a table are different menus.
    fn set_menu_entry(
        &mut self,
        target: KeyTarget,
        pre: KeyPress,
        suf: KeyPress,
        action: Option<Action>,
    ) -> std::result::Result<(), String> {
        let edit = |m: &mut LeaderMenu| match action {
            Some(a) => m.set(suf, a),
            None => m.remove(suf),
        };
        match target {
            KeyTarget::OldSort => Err(format!("`{}` opens no menu in [keys.sort]", pre.label())),
            KeyTarget::Global => {
                let id = prefix_menu(pre).ok_or_else(|| "no such menu".to_string())?;
                if let Some(m) = self.leaders.get_mut(&id) {
                    edit(m);
                }
                for ((_, mid), m) in self.ctx_menus.iter_mut() {
                    if *mid == id {
                        edit(m);
                    }
                }
                Ok(())
            }
            KeyTarget::Context(c) => {
                let id = match self.local(c, pre) {
                    Some(Binding::Leader(id)) => *id,
                    _ => return Err(format!("`{}` opens no menu in {}", pre.label(), c.header())),
                };
                if id == LeaderId::Actions {
                    return Err(format!(
                        "the Space menu of {} lists its operations itself (bind the operation to a key there instead)",
                        c.header()
                    ));
                }
                let base = self
                    .ctx_menus
                    .get(&(c, id))
                    .or_else(|| self.leaders.get(&id))
                    .cloned()
                    .ok_or_else(|| format!("{} has no menu to change", c.header()))?;
                edit(self.ctx_menus.entry((c, id)).or_insert(base));
                Ok(())
            }
        }
    }

    /// Bind `kp` to `action` in `target`'s own table, or unbind it there (`None`). Unbinding must
    /// also hide what the context inherits from layers 1-2, hence the explicit
    /// [`Binding::Unbound`] mark; layer 0 still applies, as it did when the key was merely absent
    /// from a surface's table.
    fn set_key(&mut self, target: KeyTarget, kp: KeyPress, action: Option<Action>) {
        match target {
            KeyTarget::OldSort => {}
            KeyTarget::Global => match action {
                Some(a) => {
                    self.global.insert(kp, Binding::Run(a));
                }
                None => {
                    self.global.remove(&kp);
                }
            },
            KeyTarget::Context(c) => {
                let inherited = self.inherited(c, kp).is_some();
                let table = self.contexts.entry(c).or_default();
                match action {
                    Some(a) => {
                        table.insert(kp, Binding::Run(a));
                    }
                    None if inherited => {
                        table.insert(kp, Binding::Unbound);
                    }
                    None => {
                        table.remove(&kp);
                    }
                }
            }
        }
    }

    /// What layers 1-2 give `kp` in `ctx` (ignoring the context's own table and `global`).
    fn inherited(&self, ctx: Context, kp: KeyPress) -> Option<&Binding> {
        for vt in ctx.view_types() {
            if let Some(b) = self.types.get(vt).and_then(|m| m.get(&kp)) {
                return Some(b);
            }
        }
        if ctx.inherits_basic() {
            return self.basic.get(&kp);
        }
        None
    }

    /// The binding `kp` has in `ctx` before `global`: the context's own table, then layers 2 and 1.
    /// `None` when it has none or the context unbound it ([`Binding::Unbound`]).
    fn local(&self, ctx: Context, kp: KeyPress) -> Option<&Binding> {
        match self.contexts.get(&ctx).and_then(|m| m.get(&kp)) {
            Some(Binding::Unbound) => None,
            Some(b) => Some(b),
            None => self.inherited(ctx, kp),
        }
    }

    /// Every key that has an entry in `ctx`'s own table, its layers 2 and 1 (not `global`).
    fn local_keys(&self, ctx: Context) -> Vec<KeyPress> {
        let mut keys: Vec<KeyPress> = Vec::new();
        if let Some(m) = self.contexts.get(&ctx) {
            keys.extend(m.keys().copied());
        }
        for vt in ctx.view_types() {
            if let Some(m) = self.types.get(vt) {
                keys.extend(m.keys().copied());
            }
        }
        if ctx.inherits_basic() {
            keys.extend(self.basic.keys().copied());
        }
        keys.sort_by_key(|k| format!("{k:?}"));
        keys.dedup();
        keys
    }

    /// Detect post-merge conflicts and revert conflicting overrides to the defaults (§7). `defaults` is the kept default copy.
    fn validate(&mut self, defaults: &KeyMap) {
        let mut conflicts: Vec<KeyConflict> = Vec::new();
        let mut global_keys: Vec<KeyPress> = self.global.keys().copied().collect();
        global_keys.sort_by_key(|k| format!("{k:?}"));
        let mut ctxs: Vec<Context> = self.contexts.keys().copied().collect();
        ctxs.sort_by_key(|c| c.name());

        for ctx in ctxs {
            // --- PrefixVsSingle: a key that defaulted to a leader prefix was stolen by a Run ---
            let leader_keys: Vec<(KeyPress, LeaderId)> = defaults
                .local_keys(ctx)
                .into_iter()
                .filter_map(|k| match defaults.local(ctx, k) {
                    Some(Binding::Leader(id)) => Some((k, *id)),
                    _ => None,
                })
                .collect();
            for (k, id) in leader_keys {
                if let Some(Binding::Run(a)) = self.local(ctx, k).cloned() {
                    conflicts.push(KeyConflict {
                        surface: ctx.surface(),
                        key: k,
                        kept: format!("leader:{}", leader_name(id)),
                        dropped: format!("{} in {}", action_name(a), ctx.header()),
                        reason: ConflictKind::PrefixVsSingle,
                    });
                    self.contexts
                        .entry(ctx)
                        .or_default()
                        .insert(k, Binding::Leader(id));
                }
            }

            // --- GlobalShadow: a context stole a Global default key for another Action ---
            // A context-specific binding that the default map itself has (e.g. the tab list's
            // `w` = close the selected tab) is legitimate. Only correct it when the user config
            // steals a Global key **in a way that differs from the default**, restoring the
            // default's context binding if one exists, or Global otherwise.
            let shadows: Vec<(KeyPress, String, String, Option<Binding>)> = global_keys
                .iter()
                .filter_map(|gk| {
                    let local = self.local(ctx, *gk)?;
                    let global = self.global.get(gk);
                    if Some(local) == global {
                        return None;
                    }
                    let default_local = defaults.local(ctx, *gk);
                    if Some(local) == default_local {
                        return None; // context specialization matching the default
                    }
                    let restore = default_local.cloned();
                    let kept = match &restore {
                        Some(b) => binding_name(b),
                        None => global.map(binding_name).unwrap_or_default(),
                    };
                    Some((
                        *gk,
                        kept,
                        format!("{} in {}", binding_name(local), ctx.header()),
                        restore,
                    ))
                })
                .collect();
            for (gk, kept, dropped, restore) in shadows {
                conflicts.push(KeyConflict {
                    surface: ctx.surface(),
                    key: gk,
                    kept,
                    dropped,
                    reason: ConflictKind::GlobalShadow,
                });
                match restore {
                    Some(b) => {
                        self.contexts.entry(ctx).or_default().insert(gk, b);
                    }
                    None => self.set_key(KeyTarget::Context(ctx), gk, None),
                }
            }
        }

        // One context-less table can stand for several contexts (a table under the old name
        // `preview_text` covers `text` and `document`), and both would report the same thing:
        // count it once, as it was when there was one surface.
        let mut seen: Vec<(Surface, KeyPress, ConflictKind)> = Vec::new();
        conflicts.retain(|c| {
            let id = (c.surface, c.key, c.reason);
            if seen.contains(&id) {
                false
            } else {
                seen.push(id);
                true
            }
        });
        self.conflicts = conflicts;
    }

    /// The menu `id` is in `ctx`: the context's own copy when it has one, else the shared one.
    /// `None` for a menu that is built when it opens (`LeaderId::Actions`, see `ops::menu`) and
    /// for a context that has no such menu.
    pub fn menu(&self, ctx: Context, id: LeaderId) -> Option<&LeaderMenu> {
        self.ctx_menus
            .get(&(ctx, id))
            .or_else(|| self.leaders.get(&id))
    }

    /// The command a key runs inside the open menu `id` of `ctx`. (The `Space` menu outside the
    /// tree is built from the app's state: `ops::menu::find` answers for it.)
    pub fn resolve_leader(&self, ctx: Context, id: LeaderId, kp: KeyPress) -> Resolution {
        match self.menu(ctx, id).and_then(|m| m.find(kp)) {
            Some(a) => Resolution::Action(a),
            None => Resolution::Unbound,
        }
    }

    /// context + (leader pending) + keystroke → resolution (§1.5). One input = a few HashMap lookups.
    ///
    /// With a leader pending, only the leader's menu is consulted (an unknown suffix is `Unbound`,
    /// it does not fall through). Otherwise: the context's own table, its layer-2 tables, the basic
    /// layer, then `global`.
    pub fn resolve(&self, ctx: Context, pending: Option<LeaderId>, kp: KeyPress) -> Resolution {
        if let Some(id) = pending {
            return self.resolve_leader(ctx, id, kp);
        }
        if let Some(b) = self.local(ctx, kp) {
            return binding_to_resolution(b);
        }
        if let Some(b) = self.global.get(&kp) {
            return binding_to_resolution(b);
        }
        Resolution::Unbound
    }
}

impl KeyMap {
    /// Every key that runs `action` in `ctx` right now (config applied), in a fixed display order:
    /// plain characters first, then Ctrl-chords, then named keys. A key counts only if it
    /// *resolves* to the action, so a key shadowed by a more specific layer is not listed. The
    /// fixed keys (`Enter`, `Esc`, ...) are not in the keymap; `ops::keys` adds them.
    pub fn keys_of(&self, ctx: Context, action: Action) -> Vec<KeyPress> {
        let want = Binding::Run(action);
        let mut out: Vec<KeyPress> = Vec::new();
        self.each_key_of(ctx, |k, b| {
            if *b == want
                && !out.contains(&k)
                && self.resolve(ctx, None, k) == Resolution::Action(action)
            {
                out.push(k);
            }
        });
        out.sort_by_key(|k| display_order(*k));
        out
    }

    /// Every `(key, operation)` pair that resolves in `ctx` right now (config applied), in the
    /// order of `display_order`. Menus, `Unbound` marks and shadowed keys are left out.
    pub fn bound_actions(&self, ctx: Context) -> Vec<(KeyPress, Action)> {
        let mut out: Vec<(KeyPress, Action)> = Vec::new();
        self.each_key_of(ctx, |k, b| {
            if let Binding::Run(a) = b {
                if self.resolve(ctx, None, k) == Resolution::Action(*a)
                    && !out.iter().any(|(o, _)| *o == k)
                {
                    out.push((k, *a));
                }
            }
        });
        out.sort_by_key(|(k, _)| display_order(*k));
        out
    }

    /// The keys that open a leader menu in `ctx` (`y`, `Space`, ...), with the menu each opens.
    pub fn leader_keys_of(&self, ctx: Context) -> Vec<(KeyPress, LeaderId)> {
        let mut out: Vec<(KeyPress, LeaderId)> = Vec::new();
        self.each_key_of(ctx, |k, b| {
            if let Binding::Leader(id) = b {
                if !out.iter().any(|(o, _)| *o == k)
                    && self.resolve(ctx, None, k) == Resolution::EnterLeader(*id)
                {
                    out.push((k, *id));
                }
            }
        });
        out.sort_by_key(|(k, _)| display_order(*k));
        out
    }

    /// Visit every `(key, binding)` of the tables `ctx` looks into: its own, its view types',
    /// the basic one and `global`. Allocation-free; the footer asks every frame.
    fn each_key_of(&self, ctx: Context, mut f: impl FnMut(KeyPress, &Binding)) {
        if let Some(m) = self.contexts.get(&ctx) {
            m.iter().for_each(|(k, b)| f(*k, b));
        }
        for vt in ctx.view_types() {
            if let Some(m) = self.types.get(vt) {
                m.iter().for_each(|(k, b)| f(*k, b));
            }
        }
        if ctx.inherits_basic() {
            self.basic.iter().for_each(|(k, b)| f(*k, b));
        }
        self.global.iter().for_each(|(k, b)| f(*k, b));
    }
}

/// Which layer of the keymap a key comes from, for the `?` help's sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLayer {
    /// Layer 0: every context inherits it (tabs, help, quit).
    Global,
    /// Layer 1: movement every context with a cursor shares.
    Basic,
    /// The context's own table and the table of its view type.
    View,
}

impl KeyMap {
    /// Where `kp` is defined for `ctx` (`None` = not bound there). The same lookup order as
    /// [`KeyMap::resolve`].
    pub fn layer_of_key(&self, ctx: Context, kp: KeyPress) -> Option<KeyLayer> {
        if let Some(b) = self.contexts.get(&ctx).and_then(|m| m.get(&kp)) {
            return (*b != Binding::Unbound).then_some(KeyLayer::View);
        }
        for vt in ctx.view_types() {
            if self.types.get(vt).is_some_and(|m| m.contains_key(&kp)) {
                return Some(KeyLayer::View);
            }
        }
        if ctx.inherits_basic() && self.basic.contains_key(&kp) {
            return Some(KeyLayer::Basic);
        }
        self.global.contains_key(&kp).then_some(KeyLayer::Global)
    }
}

/// The order keys are listed in: plain characters (by code point, so `j` before `k`, `J` before
/// `j`), then Ctrl-chords, then named keys.
fn display_order(k: KeyPress) -> (u8, u32) {
    match (k.ctrl, k.code) {
        (false, KeyCode::Char(c)) => (0, c as u32),
        (true, KeyCode::Char(c)) => (1, c as u32),
        (_, other) => (
            2,
            match other {
                KeyCode::Enter => 0,
                KeyCode::Tab => 1,
                KeyCode::BackTab => 2,
                KeyCode::Esc => 3,
                KeyCode::PageDown => 4,
                KeyCode::PageUp => 5,
                KeyCode::Home => 6,
                KeyCode::End => 7,
                _ => 99,
            },
        ),
    }
}

fn binding_to_resolution(b: &Binding) -> Resolution {
    match b {
        Binding::Run(a) => Resolution::Action(*a),
        Binding::Leader(id) => Resolution::EnterLeader(*id),
        Binding::Unbound => Resolution::Unbound,
    }
}

/// What a `[keys.<name>]` table writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyTarget {
    Global,
    Context(Context),
    /// The old `[keys.sort]` table of the sort overlay, which no longer exists: it owns no keys,
    /// and its sort letters re-letter the tree's `,` menu (see `apply_one`).
    OldSort,
}

/// The key that opens a menu, as `[keys.global]` writes it in a chord → the menu it changes
/// there: `space x` is the file menu, `y x` the copy menu, `, x` the `,` menu. In one context's
/// table the menu is whatever the key opens *in that context* (`KeyMap::set_menu_entry`).
fn prefix_menu(kp: KeyPress) -> Option<LeaderId> {
    if kp == KeyPress::ch(' ') {
        Some(LeaderId::File)
    } else if kp == KeyPress::ch('y') {
        Some(LeaderId::Copy)
    } else if kp == KeyPress::ch(',') {
        Some(LeaderId::Options)
    } else {
        None
    }
}

fn leader_name(id: LeaderId) -> &'static str {
    match id {
        LeaderId::Copy => "copy",
        LeaderId::File => "file",
        LeaderId::TableCopy => "table_copy",
        #[cfg(feature = "git")]
        LeaderId::GitCopy => "git_copy",
        #[cfg(feature = "git")]
        LeaderId::BranchCopy => "branch_copy",
        LeaderId::Options => "options",
        LeaderId::Actions => "actions",
    }
}

/// Fixed keys that cannot be rebound (§3). A config rebind of these is warned and ignored.
/// Character keys are not treated as fixed (the fixedness of text-input surfaces is handled separately per Surface).
fn is_fixed_key(kp: KeyPress) -> bool {
    matches!(
        kp.code,
        KeyCode::Esc
            | KeyCode::Enter
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
    )
}

/// Swap the page/half rows of the Preview surfaces according to scheme (§2 scheme). Not called for Tree (always vim).
fn apply_scheme_paging(m: &mut ContextMap, scheme: KeyScheme) {
    match scheme {
        KeyScheme::Vim => {
            m.insert(
                KeyPress::ctrl_ch('f'),
                Binding::Run(Action::Navigate(Motion::PageDown)),
            );
            m.insert(
                KeyPress::ctrl_ch('b'),
                Binding::Run(Action::Navigate(Motion::PageUp)),
            );
            m.insert(
                KeyPress::ctrl_ch('d'),
                Binding::Run(Action::Navigate(Motion::HalfDown)),
            );
            m.insert(
                KeyPress::ctrl_ch('u'),
                Binding::Run(Action::Navigate(Motion::HalfUp)),
            );
        }
        KeyScheme::Less => {
            m.insert(
                KeyPress::ch('f'),
                Binding::Run(Action::Navigate(Motion::PageDown)),
            );
            m.insert(
                KeyPress::ch('b'),
                Binding::Run(Action::Navigate(Motion::PageUp)),
            );
            m.insert(
                KeyPress::ch('d'),
                Binding::Run(Action::Navigate(Motion::HalfDown)),
            );
            m.insert(
                KeyPress::ch('u'),
                Binding::Run(Action::Navigate(Motion::HalfUp)),
            );
        }
    }
}

fn copy_leader_default() -> LeaderMenu {
    LeaderMenu {
        id: LeaderId::Copy,
        title: Msg::WkCopyPathTitle,
        items: vec![
            LeaderItem {
                key: KeyPress::ch('n'),
                action: Action::CopyPath(CopyKind::Name),
                label: Msg::WkName,
            },
            LeaderItem {
                key: KeyPress::ch('r'),
                action: Action::CopyPath(CopyKind::Relative),
                label: Msg::WkRelative,
            },
            LeaderItem {
                key: KeyPress::ch('f'),
                action: Action::CopyPath(CopyKind::Full),
                label: Msg::WkFull,
            },
            LeaderItem {
                key: KeyPress::ch('p'),
                action: Action::CopyPath(CopyKind::Parent),
                label: Msg::WkParent,
            },
            LeaderItem {
                key: KeyPress::ch('@'),
                action: Action::CopyPath(CopyKind::AtRef),
                label: Msg::WkAtRef,
            },
            // Markdown code block copy. Displayed in the which-key menu only while a code block is
            // focused (see ui::status::whichkey_spans); harmless no-op otherwise.
            LeaderItem {
                key: KeyPress::ch('c'),
                action: Action::CopyCodeBlock,
                label: Msg::WkCodeBlock,
            },
        ],
    }
}

/// The `y` cell/row/column copy leader (CSV/TSV table preview). `f` keeps the full-path copy reachable.
fn table_copy_leader_default() -> LeaderMenu {
    LeaderMenu {
        id: LeaderId::TableCopy,
        title: Msg::WkTableCopyTitle,
        items: vec![
            LeaderItem {
                key: KeyPress::ch('c'),
                action: Action::TableCopy(TableCopyKind::Cell),
                label: Msg::WkCell,
            },
            LeaderItem {
                key: KeyPress::ch('r'),
                action: Action::TableCopy(TableCopyKind::Row),
                label: Msg::WkRow,
            },
            LeaderItem {
                key: KeyPress::ch('C'),
                action: Action::TableCopy(TableCopyKind::Column),
                label: Msg::WkColumn,
            },
            LeaderItem {
                key: KeyPress::ch('f'),
                action: Action::CopyPath(CopyKind::Full),
                label: Msg::WkFull,
            },
        ],
    }
}

/// The `y` commit-info copy leader (git log/graph/detail).
#[cfg(feature = "git")]
fn git_copy_leader_default() -> LeaderMenu {
    LeaderMenu {
        id: LeaderId::GitCopy,
        title: Msg::WkGitCopyTitle,
        items: vec![
            LeaderItem {
                key: KeyPress::ch('s'),
                action: Action::GitCopy(GitCopyKind::ShortHash),
                label: Msg::WkShortHash,
            },
            LeaderItem {
                key: KeyPress::ch('h'),
                action: Action::GitCopy(GitCopyKind::FullHash),
                label: Msg::WkFullHash,
            },
            LeaderItem {
                key: KeyPress::ch('t'),
                action: Action::GitCopy(GitCopyKind::Subject),
                label: Msg::WkSubject,
            },
            LeaderItem {
                key: KeyPress::ch('m'),
                action: Action::GitCopy(GitCopyKind::Message),
                label: Msg::WkMessage,
            },
            LeaderItem {
                key: KeyPress::ch('a'),
                action: Action::GitCopy(GitCopyKind::Author),
                label: Msg::WkAuthor,
            },
            LeaderItem {
                key: KeyPress::ch('d'),
                action: Action::GitCopy(GitCopyKind::Date),
                label: Msg::WkDate,
            },
        ],
    }
}

/// The `y` copy menu of the branch list: the name of the selected branch.
#[cfg(feature = "git")]
fn branch_copy_leader_default() -> LeaderMenu {
    LeaderMenu {
        id: LeaderId::BranchCopy,
        title: Msg::WkBranchCopyTitle,
        items: vec![LeaderItem {
            key: KeyPress::ch('n'),
            action: Action::CopyBranchName,
            label: Msg::WkBranchName,
        }],
    }
}

/// The `,` menus: what each context lets you sort and show. The letters of the tree's menu are
/// the old sort menu's (`n s m e` the key, `r` the direction, `.` the folders first).
fn options_menus_default() -> Vec<(Context, LeaderMenu)> {
    let item = |c: char, action: Action| LeaderItem {
        key: KeyPress::ch(c),
        action,
        label: leader_label(action),
    };
    let menu = |ctx: Context, items: Vec<LeaderItem>| {
        (
            ctx,
            LeaderMenu {
                id: LeaderId::Options,
                title: Msg::WkOptionsTitle,
                items,
            },
        )
    };
    let path = || item('p', Action::CyclePathStyle);
    let wrap = || item('w', Action::ToggleWrap);
    #[cfg_attr(not(feature = "git"), allow(unused_mut))]
    let mut v = vec![
        menu(
            Context::Tree,
            vec![
                item('n', Action::SortSet(SortKey::Name)),
                item('s', Action::SortSet(SortKey::Size)),
                item('m', Action::SortSet(SortKey::Modified)),
                item('e', Action::SortSet(SortKey::Ext)),
                item('r', Action::SortToggleReverse),
                item('.', Action::SortToggleDirsFirst),
                path(),
                item('h', Action::ToggleHidden),
            ],
        ),
        menu(Context::Text, vec![path(), wrap()]),
        menu(Context::Document, vec![path(), wrap()]),
        menu(Context::Image, vec![path()]),
        menu(Context::Table, vec![path()]),
        menu(Context::Canvas, vec![path()]),
    ];
    #[cfg(feature = "git")]
    v.push(menu(
        Context::Diff,
        vec![item('f', Action::ToggleFollowDiffScope), path()],
    ));
    v
}

fn file_leader_default() -> LeaderMenu {
    LeaderMenu {
        id: LeaderId::File,
        title: Msg::TreeFile,
        items: vec![
            LeaderItem {
                key: KeyPress::ch('n'),
                action: Action::FileCreate,
                label: Msg::WkCreate,
            },
            LeaderItem {
                key: KeyPress::ch('r'),
                action: Action::FileRename,
                label: Msg::WkRename,
            },
            LeaderItem {
                key: KeyPress::ch('d'),
                action: Action::FileDelete,
                label: Msg::WkDelete,
            },
            LeaderItem {
                key: KeyPress::ch('c'),
                action: Action::FileCopy,
                label: Msg::CopyHint,
            },
            LeaderItem {
                key: KeyPress::ch('x'),
                action: Action::FileCut,
                label: Msg::CutHint,
            },
            LeaderItem {
                key: KeyPress::ch('p'),
                action: Action::FilePaste,
                label: Msg::WkPaste,
            },
            LeaderItem {
                key: KeyPress::ch('D'),
                action: Action::FileDuplicate,
                label: Msg::WkDuplicate,
            },
        ],
    }
}

/// The label key for which-key display. Actions outside any leader get an empty label.
fn leader_label(a: Action) -> Msg {
    match a {
        Action::CopyPath(CopyKind::Name) => Msg::WkName,
        Action::CopyPath(CopyKind::Relative) => Msg::WkRelative,
        Action::CopyPath(CopyKind::Full) => Msg::WkFull,
        Action::CopyPath(CopyKind::Parent) => Msg::WkParent,
        Action::CopyPath(CopyKind::AtRef) => Msg::WkAtRef,
        Action::CopyCodeBlock => Msg::WkCodeBlock,
        Action::FileCreate => Msg::WkCreate,
        Action::FileRename => Msg::WkRename,
        Action::FileDelete => Msg::WkDelete,
        Action::FileCopy => Msg::CopyHint,
        Action::FileCut => Msg::CutHint,
        Action::FilePaste => Msg::WkPaste,
        Action::FileDuplicate => Msg::WkDuplicate,
        Action::TableCopy(TableCopyKind::Cell) => Msg::WkCell,
        Action::TableCopy(TableCopyKind::Row) => Msg::WkRow,
        Action::TableCopy(TableCopyKind::Column) => Msg::WkColumn,
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::ShortHash) => Msg::WkShortHash,
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::FullHash) => Msg::WkFullHash,
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Subject) => Msg::WkSubject,
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Message) => Msg::WkMessage,
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Author) => Msg::WkAuthor,
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Date) => Msg::WkDate,
        #[cfg(feature = "git")]
        Action::CopyBranchName => Msg::WkBranchName,
        Action::SortSet(SortKey::Name) => Msg::FtSortName,
        Action::SortSet(SortKey::Size) => Msg::FtSortSize,
        Action::SortSet(SortKey::Modified) => Msg::FtSortMod,
        Action::SortSet(SortKey::Ext) => Msg::FtSortExt,
        Action::SortToggleReverse => Msg::FtSortReverse,
        Action::SortToggleDirsFirst => Msg::FtSortDirs,
        Action::CyclePathStyle => Msg::HintPath,
        Action::ToggleHidden => Msg::HintHidden,
        Action::ToggleWrap => Msg::HintWrap,
        #[cfg(feature = "git")]
        Action::ToggleFollowDiffScope => Msg::HintFollowScope,
        _ => Msg::Empty,
    }
}

// =============================================================================
// Action ↔ config-string two-way mapping.
// =============================================================================

fn motion_name(m: Motion) -> &'static str {
    match m {
        Motion::Up => "up",
        Motion::Down => "down",
        Motion::Top => "top",
        Motion::Bottom => "bottom",
        Motion::PageUp => "page_up",
        Motion::PageDown => "page_down",
        Motion::HalfUp => "half_up",
        Motion::HalfDown => "half_down",
        Motion::Left => "left",
        Motion::Right => "right",
        Motion::LineHome => "line_home",
        Motion::LineEnd => "line_end",
    }
}

fn motion_from_str(s: &str) -> Option<Motion> {
    Some(match s {
        "up" => Motion::Up,
        "down" => Motion::Down,
        "top" => Motion::Top,
        "bottom" => Motion::Bottom,
        "page_up" => Motion::PageUp,
        "page_down" => Motion::PageDown,
        "half_up" => Motion::HalfUp,
        "half_down" => Motion::HalfDown,
        "left" => Motion::Left,
        "right" => Motion::Right,
        "line_home" => Motion::LineHome,
        "line_end" => Motion::LineEnd,
        _ => return None,
    })
}

/// config string → Action. Unknown returns None (the caller warns). `tab_goto` is also None (fixed to digit keys).
pub fn action_from_str(s: &str) -> Option<Action> {
    if let Some(m) = s.strip_prefix("navigate:") {
        return motion_from_str(m).map(Action::Navigate);
    }
    Some(match s {
        "noop" | "disabled" => Action::Noop,
        // Global
        "tab_new" => Action::TabNew,
        "tab_new_after" => Action::TabNewAfter,
        "tab_close" => Action::TabClose,
        "tab_prev" => Action::TabPrev,
        "tab_next" => Action::TabNext,
        "toggle_help" => Action::ToggleHelp,
        "copy_name" => Action::CopyPath(CopyKind::Name),
        "copy_relative" => Action::CopyPath(CopyKind::Relative),
        "copy_full" => Action::CopyPath(CopyKind::Full),
        "copy_parent" => Action::CopyPath(CopyKind::Parent),
        "copy_at_ref" => Action::CopyPath(CopyKind::AtRef),
        "copy_code_block" => Action::CopyCodeBlock,
        "paste_jump" => Action::PasteJump,
        // Tree
        "quit" => Action::Quit,
        "close_tab_or_quit" => Action::CloseTabOrQuit,
        "filter_start" => Action::FilterStart,
        "tree_descend" => Action::TreeDescend,
        "tree_activate" => Action::TreeActivate,
        "tree_leave" => Action::TreeLeave,
        "toggle_hidden" => Action::ToggleHidden,
        "toggle_wrap" => Action::ToggleWrap,
        "toggle_info" => Action::ToggleInfo,
        "request_edit" => Action::RequestEdit,
        "open_external" => Action::OpenExternal,
        "open_git_view" => Action::OpenGitView,
        "refresh" => Action::Refresh,
        "cycle_path_style" => Action::CyclePathStyle,
        "open_sort_menu" => Action::OpenSortMenu,
        "mark_set" => Action::MarkSet,
        "mark_jump" => Action::MarkJump,
        "set_anchor" => Action::SetAnchor,
        "reset_anchor" => Action::ResetAnchor,
        "open_git_diff_cursor" => Action::OpenGitDiffCursor,
        "enter_visual" => Action::EnterVisual,
        "toggle_select" => Action::ToggleSelect,
        "toggle_changed_filter" => Action::ToggleChangedFilter,
        "jump_next_change" => Action::JumpNextChange,
        "jump_prev_change" => Action::JumpPrevChange,
        "toggle_follow" => Action::ToggleFollow,
        // File management
        "file_create" => Action::FileCreate,
        "file_rename" => Action::FileRename,
        "file_delete" => Action::FileDelete,
        "file_copy" => Action::FileCopy,
        "file_cut" => Action::FileCut,
        "file_paste" => Action::FilePaste,
        "file_duplicate" => Action::FileDuplicate,
        // Visual
        "visual_commit" => Action::VisualCommit,
        "visual_select_siblings" => Action::VisualSelectSiblings,
        "visual_select_all" => Action::VisualSelectAll,
        // Preview: text
        "preview_back" => Action::PreviewBack,
        "search_start" => Action::SearchStart,
        "search_next" => Action::SearchNext,
        "search_prev" => Action::SearchPrev,
        "preview_enter_visual" => Action::PreviewEnterVisual,
        "preview_enter_visual_line" => Action::PreviewEnterVisualLine,
        "preview_copy_selection" => Action::PreviewCopySelection,
        "preview_copy_selection_ref" => Action::PreviewCopySelectionRef,
        "preview_exit_visual" => Action::PreviewExitVisual,
        "cycle_representation" | "toggle_markdown_raw" => Action::CycleRepresentation,
        "image_return_to_diff" => Action::ImageReturnToDiff,
        "link_focus_next" => Action::LinkFocusNext,
        "link_focus_prev" => Action::LinkFocusPrev,
        "link_open" => Action::LinkOpen,
        "open_link_new_tab" => Action::OpenLinkNewTab,
        "open_in_new_tab" => Action::OpenInNewTab,
        // Preview: image
        "image_zoom_in" => Action::ImageZoomIn,
        "image_zoom_out" => Action::ImageZoomOut,
        "image_zoom_reset" => Action::ImageZoomReset,
        // `page_next` / `page_prev`, plus the names they replaced (kept as aliases).
        "page_next" | "pdf_next_page" | "sheet_next" | "media_diff_page_next" => Action::PageNext,
        "page_prev" | "pdf_prev_page" | "sheet_prev" | "media_diff_page_prev" => Action::PagePrev,
        "preview_next_file" => Action::PreviewFileNext,
        "preview_prev_file" => Action::PreviewFilePrev,
        // Preview: table (csv/tsv)
        "table_copy_cell" => Action::TableCopy(TableCopyKind::Cell),
        "table_copy_row" => Action::TableCopy(TableCopyKind::Row),
        "table_copy_column" => Action::TableCopy(TableCopyKind::Column),
        // Sort
        "sort_name" => Action::SortSet(SortKey::Name),
        "sort_size" => Action::SortSet(SortKey::Size),
        "sort_modified" => Action::SortSet(SortKey::Modified),
        "sort_ext" => Action::SortSet(SortKey::Ext),
        "sort_toggle_reverse" => Action::SortToggleReverse,
        "sort_toggle_dirs_first" => Action::SortToggleDirsFirst,
        // Bookmark
        "toggle_tab_list" => Action::ToggleTabList,
        "tab_list_close" => Action::TabListClose,
        "toggle_outline" => Action::ToggleOutline,
        "bookmark_jump" => Action::BookmarkJump,
        "bookmark_edit" => Action::BookmarkEdit,
        "bookmark_delete" => Action::BookmarkDelete,
        "bookmark_close" => Action::BookmarkClose,
        // Info
        "info_close" => Action::InfoClose,
        // Table cell popup
        "toggle_table_cell" => Action::ToggleTableCell,
        "cancel" => Action::Cancel,
        "tab_list_activate" => Action::TabListActivate,
        "outline_jump" => Action::OutlineJump,
        #[cfg(feature = "git")]
        "git_graph_picker_apply" => Action::GitGraphPickerApply,
        "toggle_focused_item" => Action::ToggleFocusedItem,
        // Git (feature gate)
        #[cfg(feature = "git")]
        "git_diff_discard" => Action::GitDiffDiscard,
        #[cfg(feature = "git")]
        "cycle_diff_layout" => Action::CycleDiffLayout,
        #[cfg(feature = "git")]
        "cycle_diff_view" => Action::CycleDiffView,
        #[cfg(feature = "git")]
        "toggle_follow_diff_scope" => Action::ToggleFollowDiffScope,
        #[cfg(feature = "git")]
        "git_stage" => Action::GitStage,
        #[cfg(feature = "git")]
        "git_unstage" => Action::GitUnstage,
        #[cfg(feature = "git")]
        "git_stage_all" => Action::GitStageAll,
        #[cfg(feature = "git")]
        "git_unstage_all" => Action::GitUnstageAll,
        #[cfg(feature = "git")]
        "git_discard" => Action::GitDiscard,
        #[cfg(feature = "git")]
        "git_commit" => Action::GitCommit,
        #[cfg(feature = "git")]
        "git_worktree_diff" => Action::GitWorktreeDiff,
        #[cfg(feature = "git")]
        "git_open_log" => Action::GitOpenLog,
        #[cfg(feature = "git")]
        "git_open_graph" => Action::GitOpenGraph,
        #[cfg(feature = "git")]
        "git_open_branches" => Action::GitOpenBranches,
        #[cfg(feature = "git")]
        "git_launch_tool" => Action::GitLaunchTool,
        #[cfg(feature = "git")]
        "git_open_selected_diff" => Action::GitOpenSelectedDiff,
        #[cfg(feature = "git")]
        "git_open_detail" => Action::GitOpenDetail,
        // The graph's base-branch pinning and branch visibility panel. These were present in
        // `action_name` but missing here, so config could not reassign them
        // (`keymap_actions_round_trip` catches a recurrence).
        #[cfg(feature = "git")]
        "git_graph_set_base" => Action::GitGraphSetBase,
        #[cfg(feature = "git")]
        "git_graph_clear_base" => Action::GitGraphClearBase,
        #[cfg(feature = "git")]
        "git_graph_toggle_all" => Action::GitGraphToggleAll,
        #[cfg(feature = "git")]
        "jj_sync" => Action::JjSync,
        #[cfg(feature = "git")]
        "git_graph_open_picker" => Action::GitGraphOpenPicker,
        #[cfg(feature = "git")]
        "git_graph_picker_toggle" => Action::GitGraphPickerToggle,
        #[cfg(feature = "git")]
        "git_graph_picker_all" => Action::GitGraphPickerAll,
        #[cfg(feature = "git")]
        "git_graph_picker_current_only" => Action::GitGraphPickerCurrentOnly,
        #[cfg(feature = "git")]
        "git_graph_picker_move_up" => Action::GitGraphPickerMoveUp,
        #[cfg(feature = "git")]
        "git_graph_picker_move_down" => Action::GitGraphPickerMoveDown,
        #[cfg(feature = "git")]
        "diff_next_change" => Action::DiffNextChange,
        #[cfg(feature = "git")]
        "diff_prev_change" => Action::DiffPrevChange,
        #[cfg(feature = "git")]
        "git_screen_next" => Action::GitScreenNext,
        #[cfg(feature = "git")]
        "git_screen_prev" => Action::GitScreenPrev,
        "list_filter_start" => Action::ListFilterStart,
        #[cfg(feature = "git")]
        "branch_filter_start" => Action::BranchFilterStart,
        #[cfg(feature = "git")]
        "branch_checkout" => Action::BranchCheckout,
        #[cfg(feature = "git")]
        "branch_create" => Action::BranchCreate,
        #[cfg(feature = "git")]
        "branch_delete" => Action::BranchDelete,
        #[cfg(feature = "git")]
        "git_open_worktrees" => Action::GitOpenWorktrees,
        #[cfg(feature = "git")]
        "worktree_filter_start" => Action::WorktreeFilterStart,
        #[cfg(feature = "git")]
        "worktree_goto" => Action::WorktreeGoto,
        #[cfg(feature = "git")]
        "worktree_goto_new_tab" => Action::WorktreeGotoNewTab,
        #[cfg(feature = "git")]
        "worktree_close" => Action::WorktreeClose,
        #[cfg(feature = "git")]
        "worktree_show_changes" => Action::WorktreeShowChanges,
        #[cfg(feature = "git")]
        "worktree_create" => Action::WorktreeCreate,
        #[cfg(feature = "git")]
        "git_copy_short_hash" => Action::GitCopy(GitCopyKind::ShortHash),
        #[cfg(feature = "git")]
        "git_copy_full_hash" => Action::GitCopy(GitCopyKind::FullHash),
        #[cfg(feature = "git")]
        "git_copy_subject" => Action::GitCopy(GitCopyKind::Subject),
        #[cfg(feature = "git")]
        "git_copy_message" => Action::GitCopy(GitCopyKind::Message),
        #[cfg(feature = "git")]
        "git_copy_author" => Action::GitCopy(GitCopyKind::Author),
        #[cfg(feature = "git")]
        "git_copy_date" => Action::GitCopy(GitCopyKind::Date),
        #[cfg(feature = "git")]
        "copy_branch_name" => Action::CopyBranchName,
        #[cfg(feature = "git")]
        "git_close" => Action::GitClose,
        _ => return None,
    })
}

/// Action → config string (canonical). For conflict reports and round-tripping.
pub fn action_name(a: Action) -> String {
    let s: &str = match a {
        Action::Noop => "noop",
        Action::Navigate(m) => return format!("navigate:{}", motion_name(m)),
        Action::TabNew => "tab_new",
        Action::TabNewAfter => "tab_new_after",
        Action::TabClose => "tab_close",
        Action::TabPrev => "tab_prev",
        Action::TabNext => "tab_next",
        Action::TabGoto(_) => "tab_goto",
        Action::ToggleHelp => "toggle_help",
        Action::CopyPath(CopyKind::Name) => "copy_name",
        Action::CopyPath(CopyKind::Relative) => "copy_relative",
        Action::CopyPath(CopyKind::Full) => "copy_full",
        Action::CopyPath(CopyKind::Parent) => "copy_parent",
        Action::CopyPath(CopyKind::AtRef) => "copy_at_ref",
        Action::CopyCodeBlock => "copy_code_block",
        Action::PasteJump => "paste_jump",
        Action::Quit => "quit",
        Action::CloseTabOrQuit => "close_tab_or_quit",
        Action::FilterStart => "filter_start",
        Action::TreeDescend => "tree_descend",
        Action::TreeActivate => "tree_activate",
        Action::TreeLeave => "tree_leave",
        Action::ToggleHidden => "toggle_hidden",
        Action::ToggleWrap => "toggle_wrap",
        Action::ToggleInfo => "toggle_info",
        Action::RequestEdit => "request_edit",
        Action::OpenExternal => "open_external",
        Action::OpenGitView => "open_git_view",
        Action::Refresh => "refresh",
        Action::CyclePathStyle => "cycle_path_style",
        Action::OpenSortMenu => "open_sort_menu",
        Action::MarkSet => "mark_set",
        Action::MarkJump => "mark_jump",
        Action::SetAnchor => "set_anchor",
        Action::ResetAnchor => "reset_anchor",
        Action::OpenGitDiffCursor => "open_git_diff_cursor",
        Action::EnterVisual => "enter_visual",
        Action::ToggleSelect => "toggle_select",
        Action::ToggleChangedFilter => "toggle_changed_filter",
        Action::JumpNextChange => "jump_next_change",
        Action::JumpPrevChange => "jump_prev_change",
        Action::ToggleFollow => "toggle_follow",
        Action::FileCreate => "file_create",
        Action::FileRename => "file_rename",
        Action::FileDelete => "file_delete",
        Action::FileCopy => "file_copy",
        Action::FileCut => "file_cut",
        Action::FilePaste => "file_paste",
        Action::FileDuplicate => "file_duplicate",
        Action::VisualCommit => "visual_commit",
        Action::VisualSelectSiblings => "visual_select_siblings",
        Action::VisualSelectAll => "visual_select_all",
        Action::PreviewBack => "preview_back",
        Action::SearchStart => "search_start",
        Action::SearchNext => "search_next",
        Action::SearchPrev => "search_prev",
        Action::PreviewEnterVisual => "preview_enter_visual",
        Action::PreviewEnterVisualLine => "preview_enter_visual_line",
        Action::PreviewCopySelection => "preview_copy_selection",
        Action::PreviewCopySelectionRef => "preview_copy_selection_ref",
        Action::PreviewExitVisual => "preview_exit_visual",
        Action::CycleRepresentation => "cycle_representation",
        Action::ImageReturnToDiff => "image_return_to_diff",
        Action::LinkFocusNext => "link_focus_next",
        Action::LinkFocusPrev => "link_focus_prev",
        Action::LinkOpen => "link_open",
        Action::OpenLinkNewTab => "open_link_new_tab",
        Action::OpenInNewTab => "open_in_new_tab",
        Action::ImageZoomIn => "image_zoom_in",
        Action::ImageZoomOut => "image_zoom_out",
        Action::ImageZoomReset => "image_zoom_reset",
        Action::PageNext => "page_next",
        Action::PagePrev => "page_prev",
        Action::PreviewFileNext => "preview_next_file",
        Action::PreviewFilePrev => "preview_prev_file",
        Action::TableCopy(TableCopyKind::Cell) => "table_copy_cell",
        Action::TableCopy(TableCopyKind::Row) => "table_copy_row",
        Action::TableCopy(TableCopyKind::Column) => "table_copy_column",
        Action::SortSet(SortKey::Name) => "sort_name",
        Action::SortSet(SortKey::Size) => "sort_size",
        Action::SortSet(SortKey::Modified) => "sort_modified",
        Action::SortSet(SortKey::Ext) => "sort_ext",
        Action::SortToggleReverse => "sort_toggle_reverse",
        Action::SortToggleDirsFirst => "sort_toggle_dirs_first",
        Action::ToggleTabList => "toggle_tab_list",
        Action::TabListClose => "tab_list_close",
        Action::ToggleOutline => "toggle_outline",
        Action::BookmarkJump => "bookmark_jump",
        Action::BookmarkEdit => "bookmark_edit",
        Action::BookmarkDelete => "bookmark_delete",
        Action::BookmarkClose => "bookmark_close",
        Action::InfoClose => "info_close",
        Action::ToggleTableCell => "toggle_table_cell",
        Action::Cancel => "cancel",
        Action::TabListActivate => "tab_list_activate",
        Action::OutlineJump => "outline_jump",
        #[cfg(feature = "git")]
        Action::GitGraphPickerApply => "git_graph_picker_apply",
        Action::ToggleFocusedItem => "toggle_focused_item",
        Action::Plugin(p) => return format!("plugin:{}.{}", p.plugin(), p.act()),
        #[cfg(feature = "git")]
        Action::GitDiffDiscard => "git_diff_discard",
        #[cfg(feature = "git")]
        Action::CycleDiffLayout => "cycle_diff_layout",
        #[cfg(feature = "git")]
        Action::CycleDiffView => "cycle_diff_view",
        #[cfg(feature = "git")]
        Action::ToggleFollowDiffScope => "toggle_follow_diff_scope",
        #[cfg(feature = "git")]
        Action::GitStage => "git_stage",
        #[cfg(feature = "git")]
        Action::GitUnstage => "git_unstage",
        #[cfg(feature = "git")]
        Action::GitStageAll => "git_stage_all",
        #[cfg(feature = "git")]
        Action::GitUnstageAll => "git_unstage_all",
        #[cfg(feature = "git")]
        Action::GitDiscard => "git_discard",
        #[cfg(feature = "git")]
        Action::GitCommit => "git_commit",
        #[cfg(feature = "git")]
        Action::GitWorktreeDiff => "git_worktree_diff",
        #[cfg(feature = "git")]
        Action::GitOpenLog => "git_open_log",
        #[cfg(feature = "git")]
        Action::GitOpenGraph => "git_open_graph",
        #[cfg(feature = "git")]
        Action::GitOpenBranches => "git_open_branches",
        #[cfg(feature = "git")]
        Action::GitLaunchTool => "git_launch_tool",
        #[cfg(feature = "git")]
        Action::GitOpenSelectedDiff => "git_open_selected_diff",
        #[cfg(feature = "git")]
        Action::GitOpenDetail => "git_open_detail",
        #[cfg(feature = "git")]
        Action::GitGraphSetBase => "git_graph_set_base",
        #[cfg(feature = "git")]
        Action::GitGraphClearBase => "git_graph_clear_base",
        #[cfg(feature = "git")]
        Action::GitGraphToggleAll => "git_graph_toggle_all",
        #[cfg(feature = "git")]
        Action::JjSync => "jj_sync",
        #[cfg(feature = "git")]
        Action::GitGraphOpenPicker => "git_graph_open_picker",
        #[cfg(feature = "git")]
        Action::GitGraphPickerToggle => "git_graph_picker_toggle",
        #[cfg(feature = "git")]
        Action::GitGraphPickerAll => "git_graph_picker_all",
        #[cfg(feature = "git")]
        Action::GitGraphPickerCurrentOnly => "git_graph_picker_current_only",
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveUp => "git_graph_picker_move_up",
        #[cfg(feature = "git")]
        Action::GitGraphPickerMoveDown => "git_graph_picker_move_down",
        #[cfg(feature = "git")]
        Action::DiffNextChange => "diff_next_change",
        #[cfg(feature = "git")]
        Action::DiffPrevChange => "diff_prev_change",
        #[cfg(feature = "git")]
        Action::GitScreenNext => "git_screen_next",
        #[cfg(feature = "git")]
        Action::GitScreenPrev => "git_screen_prev",
        Action::ListFilterStart => "list_filter_start",
        #[cfg(feature = "git")]
        Action::BranchFilterStart => "branch_filter_start",
        #[cfg(feature = "git")]
        Action::BranchCheckout => "branch_checkout",
        #[cfg(feature = "git")]
        Action::BranchCreate => "branch_create",
        #[cfg(feature = "git")]
        Action::BranchDelete => "branch_delete",
        #[cfg(feature = "git")]
        Action::GitOpenWorktrees => "git_open_worktrees",
        #[cfg(feature = "git")]
        Action::WorktreeFilterStart => "worktree_filter_start",
        #[cfg(feature = "git")]
        Action::WorktreeGoto => "worktree_goto",
        #[cfg(feature = "git")]
        Action::WorktreeGotoNewTab => "worktree_goto_new_tab",
        #[cfg(feature = "git")]
        Action::WorktreeClose => "worktree_close",
        #[cfg(feature = "git")]
        Action::WorktreeShowChanges => "worktree_show_changes",
        #[cfg(feature = "git")]
        Action::WorktreeCreate => "worktree_create",
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::ShortHash) => "git_copy_short_hash",
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::FullHash) => "git_copy_full_hash",
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Subject) => "git_copy_subject",
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Message) => "git_copy_message",
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Author) => "git_copy_author",
        #[cfg(feature = "git")]
        Action::GitCopy(GitCopyKind::Date) => "git_copy_date",
        #[cfg(feature = "git")]
        Action::CopyBranchName => "copy_branch_name",
        #[cfg(feature = "git")]
        Action::GitClose => "git_close",
    };
    s.to_string()
}

fn binding_name(b: &Binding) -> String {
    match b {
        Binding::Run(a) => action_name(*a),
        Binding::Leader(id) => format!("leader:{}", leader_name(*id)),
        Binding::Unbound => "unbound".into(),
    }
}

// =============================================================================
// Unit tests (§10).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A git-backed version-control screen as a context.
    #[cfg(feature = "git")]
    fn gs(s: Screen) -> Context {
        Context::Screen(Backend::Git, s)
    }

    fn cfg_with(surface: &str, entries: &[(&str, &str)]) -> KeysFileConfig {
        let mut table = HashMap::new();
        for (k, v) in entries {
            table.insert((*k).to_string(), (*v).to_string());
        }
        let mut cfg = KeysFileConfig::default();
        cfg.surfaces.insert(surface.to_string(), table);
        cfg
    }

    // §10.1 defaults coverage.
    #[test]
    fn defaults_tree_core_keys() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('d')),
            Resolution::Action(Action::OpenGitDiffCursor)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('a')),
            Resolution::Action(Action::SetAnchor)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('A')),
            Resolution::Action(Action::ResetAnchor)
        );
        // `p` opens the project's screen; `o` (its old key) now opens the file in another app;
        // `s` and `p`'s old meaning (the path style) have no default key in the tree.
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('p')),
            Resolution::Action(Action::OpenGitView)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('o')),
            Resolution::Action(Action::OpenExternal)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('s')),
            Resolution::Unbound,
            "s"
        );
    }

    #[test]
    fn visual_q_cancels_the_selection_and_does_not_quit() {
        // Regression guard: `q` in the tree's range selection used to quit the app. It is layer 1's
        // "go back one step" now: the same as `Esc`, which cancels the selection.
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::TreeVisual, None, KeyPress::ch('q')),
            Resolution::Action(Action::Cancel)
        );
        // `Q` is still the one key that quits, from the selection too.
        assert_eq!(
            m.resolve(Context::TreeVisual, None, KeyPress::ch('Q')),
            Resolution::Action(Action::Quit)
        );
    }

    #[test]
    fn tree_q_resolves_to_close_tab_or_quit() {
        // At the tree top level, q means "close the tab, or quit if it's the last one". Q remains
        // Quit as before.
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('q')),
            Resolution::Action(Action::CloseTabOrQuit)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('Q')),
            Resolution::Action(Action::Quit)
        );
    }

    #[test]
    fn sheet_switch_is_j_k_on_the_table_surface_only_and_config_roundtrips() {
        // J/K (the PDF page-turn keys) switch sheets on the table surface.
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('J')),
            Resolution::Action(Action::PageNext)
        );
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('K')),
            Resolution::Action(Action::PagePrev)
        );
        // The text surface turns the slides of a presentation with the same keys (and does nothing
        // for any other text: `App::page_turn`); the image surface keeps J/K = PDF pages.
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('J')),
            Resolution::Action(Action::PageNext)
        );
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('K')),
            Resolution::Action(Action::PagePrev)
        );
        assert_eq!(
            m.resolve(Context::Image, None, KeyPress::ch('J')),
            Resolution::Action(Action::PageNext)
        );
        // config-string two-way mapping.
        for old in [
            "page_next",
            "sheet_next",
            "pdf_next_page",
            "media_diff_page_next",
        ] {
            assert_eq!(action_from_str(old), Some(Action::PageNext), "{old}");
        }
        for old in [
            "page_prev",
            "sheet_prev",
            "pdf_prev_page",
            "media_diff_page_prev",
        ] {
            assert_eq!(action_from_str(old), Some(Action::PagePrev), "{old}");
        }
        assert_eq!(action_name(Action::PageNext), "page_next");
        assert_eq!(action_name(Action::PagePrev), "page_prev");
    }

    /// The merged page action: the default keys resolve to it on every surface that had a
    /// page/sheet binding, exactly as before the merge.
    #[test]
    fn page_keys_default_bindings_are_unchanged_by_the_merge() {
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            let m = KeyMap::defaults(scheme);
            let next = Resolution::Action(Action::PageNext);
            let prev = Resolution::Action(Action::PagePrev);
            assert_eq!(m.resolve(Context::Image, None, KeyPress::ch('J')), next);
            assert_eq!(m.resolve(Context::Image, None, KeyPress::ch('K')), prev);
            assert_eq!(
                m.resolve(Context::Image, None, KeyPress::key(KeyCode::PageDown)),
                next
            );
            assert_eq!(
                m.resolve(Context::Image, None, KeyPress::key(KeyCode::PageUp)),
                prev
            );
            assert_eq!(m.resolve(Context::Table, None, KeyPress::ch('J')), next);
            assert_eq!(m.resolve(Context::Table, None, KeyPress::ch('K')), prev);
            #[cfg(feature = "git")]
            {
                assert_eq!(m.resolve(Context::Diff, None, KeyPress::ch('J')), next);
                assert_eq!(m.resolve(Context::Diff, None, KeyPress::ch('K')), prev);
                // PageDown/PageUp stay a scroll on the diff surface (rerouted only for media).
                assert_ne!(
                    m.resolve(Context::Diff, None, KeyPress::key(KeyCode::PageDown)),
                    next
                );
            }
        }
    }

    #[test]
    fn sheet_switch_can_be_rebound_and_disabled_from_config() {
        let cfg = cfg_with(
            "table",
            &[("L", "sheet_next"), ("H", "sheet_prev"), ("J", "noop")],
        );
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('L')),
            Resolution::Action(Action::PageNext)
        );
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('H')),
            Resolution::Action(Action::PagePrev)
        );
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('J')),
            Resolution::Unbound
        );
    }

    #[test]
    fn paste_jump_p_is_global_and_config_roundtrips() {
        // P is global (inherited by allows_tabs surfaces) = resolves from either Tree or Preview.
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [Context::Tree, Context::Text, Context::Image] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('P')),
                Resolution::Action(Action::PasteJump),
                "P が {sfc:?} で paste_jump に解決する"
            );
        }
        // config-string two-way mapping.
        assert_eq!(action_from_str("paste_jump"), Some(Action::PasteJump));
        assert_eq!(action_name(Action::PasteJump), "paste_jump");
    }

    #[test]
    fn preview_file_paging_is_ctrl_n_p_on_preview_surfaces() {
        // Ctrl-n/Ctrl-p = file paging in tree display order (the three surfaces text/image/table).
        // J/K conflict with the image surface's PDF page paging, and Ctrl-j is identical to Enter
        // (LF) on legacy terminals, so neither is used.
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [Context::Text, Context::Image, Context::Table] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ctrl_ch('n')),
                Resolution::Action(Action::PreviewFileNext),
                "Ctrl-n が {sfc:?} で preview_next_file に解決する"
            );
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ctrl_ch('p')),
                Resolution::Action(Action::PreviewFilePrev),
                "Ctrl-p が {sfc:?} で preview_prev_file に解決する"
            );
        }
        // config-string two-way mapping.
        assert_eq!(
            action_from_str("preview_next_file"),
            Some(Action::PreviewFileNext)
        );
        assert_eq!(action_name(Action::PreviewFilePrev), "preview_prev_file");
    }

    #[test]
    fn open_link_new_tab_is_ctrl_t_in_preview_text() {
        // Ctrl-t = focused link opens in a new tab (PreviewText-only). Ctrl+letter is reliable on
        // every terminal.
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ctrl_ch('t')),
            Resolution::Action(Action::OpenLinkNewTab),
            "Ctrl-t が PreviewText で open_link_new_tab に解決する"
        );
        assert_eq!(
            action_from_str("open_link_new_tab"),
            Some(Action::OpenLinkNewTab)
        );
        assert_eq!(action_name(Action::OpenLinkNewTab), "open_link_new_tab");
    }

    #[test]
    fn open_in_new_tab_is_ctrl_t_in_tree() {
        // Ctrl-t = open the entry under the cursor in a new tab (Tree). Same key/meaning as
        // preview's OpenLinkNewTab.
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ctrl_ch('t')),
            Resolution::Action(Action::OpenInNewTab),
            "Ctrl-t が Tree で open_in_new_tab に解決する"
        );
        assert_eq!(
            action_from_str("open_in_new_tab"),
            Some(Action::OpenInNewTab)
        );
        assert_eq!(action_name(Action::OpenInNewTab), "open_in_new_tab");
    }

    #[test]
    fn t_opens_a_new_tab_after_the_current_one_via_global() {
        // t = insert a new tab right of the currently active one (tab_new_after; it was `I`).
        // Global, so it resolves the same way from Tree/Preview *and* from the Bookmarks/Tabs
        // list surfaces (neither binds `t` for anything of its own, so it falls through to global
        // rather than being available as a bookmark-name jump letter — same treatment as
        // T/F/Q). Appending at the end (`tab_new`) and `I` have no default key.
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [
            Context::Tree,
            Context::Text,
            Context::Bookmarks,
            Context::Tabs,
        ] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('t')),
                Resolution::Action(Action::TabNewAfter),
                "t が {sfc:?} で tab_new_after に解決する"
            );
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('I')),
                Resolution::Unbound,
                "I は {sfc:?} で既定のキーではない"
            );
        }
        // The actions keep their names, so `[keys]` can bind either to any key.
        assert_eq!(action_from_str("tab_new"), Some(Action::TabNew));
        assert_eq!(action_from_str("tab_new_after"), Some(Action::TabNewAfter));
        assert_eq!(action_name(Action::TabNewAfter), "tab_new_after");
    }

    #[test]
    fn shift_q_quits_via_global_on_keymap_surfaces() {
        // Q = quit the whole app. Placed on global, so every allows_tabs() surface (all but
        // text-input/confirm-modal) inherits it.
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [
            Context::Tree,
            Context::Text,
            Context::Table,
            Context::TreeVisual,
        ] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('Q')),
                Resolution::Action(Action::Quit),
                "Q が {sfc:?} で終了に解決する"
            );
        }
    }

    // §10.2 leader
    #[test]
    fn leader_resolution() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
        assert_eq!(
            m.resolve(Context::Tree, Some(LeaderId::File), KeyPress::ch('d')),
            Resolution::Action(Action::FileDelete)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        assert_eq!(
            m.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // An unknown suffix is Unbound (cancels the leader).
        assert_eq!(
            m.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('z')),
            Resolution::Unbound
        );
    }

    // §10 copy-leader reach (inherited by Preview-related surfaces; not by Visual/git).
    #[test]
    fn copy_leader_scope() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        assert_eq!(
            m.resolve(Context::Image, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        // Visual does not inherit the copy leader.
        assert_eq!(
            m.resolve(Context::TreeVisual, None, KeyPress::ch('y')),
            Resolution::Unbound
        );
    }

    // Preview:table (csv/tsv) surface key resolution. hjkl = cell movement / y→ = cell/row/column
    // copy / q = go back.
    #[test]
    fn table_surface_resolution() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // hjkl = move the cell cursor.
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('l')),
            Resolution::Action(Action::Navigate(Motion::Right))
        );
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('0')),
            Resolution::Action(Action::Navigate(Motion::LineHome))
        );
        // y→ is the table-specific TableCopy leader (a separate menu from the path-copy Copy leader).
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::TableCopy)
        );
        assert_eq!(
            m.resolve(Context::Table, Some(LeaderId::TableCopy), KeyPress::ch('c')),
            Resolution::Action(Action::TableCopy(TableCopyKind::Cell))
        );
        assert_eq!(
            m.resolve(Context::Table, Some(LeaderId::TableCopy), KeyPress::ch('C')),
            Resolution::Action(Action::TableCopy(TableCopyKind::Column))
        );
        // `f` is kept for the (full) path copy.
        assert_eq!(
            m.resolve(Context::Table, Some(LeaderId::TableCopy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // q = go back.
        assert_eq!(
            m.resolve(Context::Table, None, KeyPress::ch('q')),
            Resolution::Action(Action::PreviewBack)
        );
    }

    // Preview's line-selection (v) mode. v starts it → j/k extend → y copies → v/q cancels.
    #[test]
    fn preview_visual_resolution() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // In a normal text preview, v = start a selection.
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('v')),
            Resolution::Action(Action::PreviewEnterVisual)
        );
        // Selection surface: j/k = extend the range (cursor movement).
        assert_eq!(
            m.resolve(Context::TextVisual, None, KeyPress::ch('j')),
            Resolution::Action(Action::Navigate(Motion::Down))
        );
        // y = copy the selection.
        assert_eq!(
            m.resolve(Context::TextVisual, None, KeyPress::ch('y')),
            Resolution::Action(Action::PreviewCopySelection)
        );
        // v / q = clear the selection.
        assert_eq!(
            m.resolve(Context::TextVisual, None, KeyPress::ch('v')),
            Resolution::Action(Action::PreviewExitVisual)
        );
        assert_eq!(
            m.resolve(Context::TextVisual, None, KeyPress::ch('q')),
            Resolution::Action(Action::PreviewExitVisual)
        );
    }

    // §10.3 chord parse
    #[test]
    fn chord_parse() {
        assert_eq!(
            KeyChord::parse("space d").unwrap(),
            KeyChord::Chord(KeyPress::ch(' '), KeyPress::ch('d'))
        );
        assert_eq!(
            KeyChord::parse("ctrl-d").unwrap(),
            KeyChord::Single(KeyPress::ctrl_ch('d'))
        );
        assert_eq!(
            KeyChord::parse("c-d").unwrap(),
            KeyChord::Single(KeyPress::ctrl_ch('d'))
        );
        assert_eq!(
            KeyChord::parse("y f").unwrap(),
            KeyChord::Chord(KeyPress::ch('y'), KeyPress::ch('f'))
        );
        assert_eq!(
            KeyChord::parse("0").unwrap(),
            KeyChord::Single(KeyPress::ch('0'))
        );
        assert_eq!(
            KeyChord::parse("$").unwrap(),
            KeyChord::Single(KeyPress::ch('$'))
        );
        assert_eq!(
            KeyChord::parse("!").unwrap(),
            KeyChord::Single(KeyPress::ch('!'))
        );
        assert_eq!(
            KeyChord::parse("space").unwrap(),
            KeyChord::Single(KeyPress::ch(' '))
        );
        assert_eq!(
            KeyChord::parse("enter").unwrap(),
            KeyChord::Single(KeyPress::key(KeyCode::Enter))
        );
        // Uppercase is kept as SHIFT included.
        assert_eq!(
            KeyChord::parse("G").unwrap(),
            KeyChord::Single(KeyPress::ch('G'))
        );
        assert!(KeyChord::parse("").is_err());
        assert!(KeyChord::parse("a b c").is_err());
        assert!(KeyChord::parse("notakey").is_err());
    }

    // §10.4 action_from_str round trip + navigate.
    #[test]
    fn action_roundtrip() {
        // Suppressed because with git disabled there's no push and `mut` becomes unneeded.
        #[allow(unused_mut)]
        let mut samples = vec![
            Action::Noop,
            Action::Navigate(Motion::PageDown),
            Action::Navigate(Motion::LineHome),
            Action::TabNew,
            Action::TabNewAfter,
            Action::ToggleHelp,
            Action::CopyPath(CopyKind::Full),
            Action::CopyPath(CopyKind::Parent),
            Action::Quit,
            Action::Refresh,
            Action::SetAnchor,
            Action::ResetAnchor,
            Action::OpenGitDiffCursor,
            Action::OpenGitView,
            Action::PageNext,
            Action::PagePrev,
            Action::FileDelete,
            Action::SortSet(SortKey::Size),
            Action::SortToggleReverse,
            Action::InfoClose,
            Action::ImageReturnToDiff,
        ];
        #[cfg(feature = "git")]
        {
            samples.push(Action::GitStage);
            samples.push(Action::GitOpenGraph);
            samples.push(Action::CycleDiffLayout);
            samples.push(Action::CycleDiffView);
            samples.push(Action::BranchDelete);
            samples.push(Action::GitClose);
        }
        for a in samples {
            assert_eq!(action_from_str(&action_name(a)), Some(a), "roundtrip {a:?}");
        }
        assert_eq!(
            action_from_str("navigate:page_down"),
            Some(Action::Navigate(Motion::PageDown))
        );
        assert_eq!(action_from_str("totally_unknown"), None);
        // tab_goto is digit-fixed → None.
        assert_eq!(action_from_str("tab_goto"), None);
    }

    // §10.5 config merge: override / add / noop / anonymous leaders forbidden.
    #[test]
    fn config_merge_override_add_noop() {
        let cfg = cfg_with(
            "tree",
            &[
                ("d", "refresh"),          // override
                ("X", "refresh"),          // add (a new key)
                ("i", "noop"),             // disable (default i=ToggleInfo)
                ("g s", "open_sort_menu"), // anonymous g leader → warned and ignored
            ],
        );
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('d')),
            Resolution::Action(Action::Refresh)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('X')),
            Resolution::Action(Action::Refresh)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('i')),
            Resolution::Unbound
        );
        // g stays the default Nav(Top) (refuses to become an anonymous leader).
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('g')),
            Resolution::Action(Action::Navigate(Motion::Top))
        );
        assert!(
            m.warnings.iter().any(|w| w.contains("chord prefix")),
            "expected anonymous-prefix warning, got {:?}",
            m.warnings
        );
    }

    // §10.5 config: move an existing suffix to another key (leader override).
    #[test]
    fn config_leader_suffix_override() {
        let cfg = cfg_with("tree", &[("y z", "copy_full")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('z')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
    }

    // §10.6 validate PrefixVsSingle
    #[test]
    fn validate_prefix_vs_single() {
        let cfg = cfg_with("tree", &[("space", "quit")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        // Space stays a leader (the override is discarded).
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
        assert_eq!(
            m.conflicts
                .iter()
                .filter(|c| c.reason == ConflictKind::PrefixVsSingle)
                .count(),
            1
        );
    }

    // §10.7 validate GlobalShadow
    #[test]
    fn validate_global_shadow() {
        let cfg = cfg_with("tree", &[("t", "refresh")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        // t reverts to Global's TabNewAfter (the per-surface override is discarded).
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('t')),
            Resolution::Action(Action::TabNewAfter)
        );
        assert_eq!(
            m.conflicts
                .iter()
                .filter(|c| c.reason == ConflictKind::GlobalShadow)
                .count(),
            1
        );
    }

    #[test]
    fn global_shadow_default_specialization_rule() {
        // The default config has no conflicts (a permanent check that per-surface specializations
        // in the default map itself are legitimate).
        let m = KeyMap::from_config(KeyScheme::Vim, &crate::keymap::KeysFileConfig::default());
        assert!(
            m.conflicts.is_empty(),
            "既定 config に衝突なし: {:?}",
            m.conflicts
        );
        // The user steals a global key (T) for another Action on a surface → detected, and since
        // the default has no per-surface specialization for it, it reverts to the global binding.
        // (The restore-to-default-specialization path is a forward-looking rule that kicks in when
        // the default does have a per-surface specialization — no such key exists in the current
        // defaults.)
        let cfg = cfg_with("tabs", &[("T", "refresh")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.conflicts
                .iter()
                .filter(|c| c.reason == ConflictKind::GlobalShadow)
                .count(),
            1
        );
        assert_eq!(
            m.resolve(Context::Tabs, None, KeyPress::ch('T')),
            Resolution::Action(Action::ToggleTabList),
            "global の割当へ戻る"
        );
    }

    #[test]
    fn outline_key_resolves_and_action_round_trips() {
        let m = KeyMap::from_config(KeyScheme::Vim, &KeysFileConfig::default());
        // `O` in a Markdown/text preview opens the outline (`o` is the verb "open" now).
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('O')),
            Resolution::Action(Action::ToggleOutline)
        );
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('o')),
            Resolution::Action(Action::OpenExternal)
        );
        // In the outline overlay: j/k move, O/q close.
        assert_eq!(
            m.resolve(Context::Outline, None, KeyPress::ch('O')),
            Resolution::Action(Action::ToggleOutline)
        );
        assert_eq!(
            m.resolve(Context::Outline, None, KeyPress::ch('j')),
            Resolution::Action(Action::Navigate(Motion::Down))
        );
        assert_eq!(
            m.resolve(Context::Outline, None, KeyPress::ch('q')),
            Resolution::Action(Action::ToggleOutline)
        );
        // config string round-trip.
        assert_eq!(
            action_from_str("toggle_outline"),
            Some(Action::ToggleOutline)
        );
        assert_eq!(action_name(Action::ToggleOutline), "toggle_outline");
    }

    // §10.8 fixed-key rebind rejection.
    #[test]
    fn fixed_key_rebind_rejected() {
        let cfg = cfg_with("tree", &[("enter", "quit")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::key(KeyCode::Enter)),
            Resolution::Unbound
        );
        assert!(m.warnings.iter().any(|w| w.contains("fixed key")));
    }

    #[test]
    fn bookmark_list_defaults_reserve_plain_letters_for_jump() {
        // Plain letters in the list are reserved for bookmark-name jumps: edit/delete use Ctrl
        // modifiers, and `'`/q close it.
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ctrl_ch('e')),
            Resolution::Action(Action::BookmarkEdit)
        );
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ctrl_ch('d')),
            Resolution::Unbound,
            "Ctrl-d is layer 1's half page; deleting is in the list's Space menu"
        );
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ch('\'')),
            Resolution::Action(Action::BookmarkClose)
        );
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ch('q')),
            Resolution::Action(Action::BookmarkClose)
        );
        // Plain e/d are unbound (= falls through to letter-jump via main's Unbound fallback).
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ch('e')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ch('d')),
            Resolution::Unbound
        );
        // The three preview surfaces also have m=register / '=list (bookmark the displayed file).
        for sfc in [Context::Text, Context::Image, Context::Table] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('m')),
                Resolution::Action(Action::MarkSet)
            );
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('\'')),
                Resolution::Action(Action::MarkJump)
            );
        }
        // In the Tree, `'` stays MarkJump, opening the list (the config name is unchanged too).
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('\'')),
            Resolution::Action(Action::MarkJump)
        );
    }

    #[test]
    fn tab_list_defaults_resolve() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // global T toggles it open/closed (from any surface); in the list, w = close the selected
        // tab (overrides global's TabClose on this surface).
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('T')),
            Resolution::Action(Action::ToggleTabList)
        );
        assert_eq!(
            m.resolve(Context::Tabs, None, KeyPress::ch('T')),
            Resolution::Action(Action::ToggleTabList),
            "一覧内の T は global 継承で閉じる"
        );
        assert_eq!(
            m.resolve(Context::Tabs, None, KeyPress::ch('d')),
            Resolution::Action(Action::TabListClose)
        );
        assert_eq!(
            m.resolve(Context::Tabs, None, KeyPress::ch('q')),
            Resolution::Action(Action::ToggleTabList)
        );
        // The default `w` has **no** binding anywhere for closing a tab (avoids accidental
        // triggers; consolidated onto q).
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('w')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(Context::Tabs, None, KeyPress::ch('w')),
            Resolution::Unbound
        );
        // config-string round trip.
        assert_eq!(
            action_from_str("toggle_tab_list"),
            Some(Action::ToggleTabList)
        );
        assert_eq!(action_name(Action::ToggleTabList), "toggle_tab_list");
        assert_eq!(
            action_from_str("tab_list_close"),
            Some(Action::TabListClose)
        );
        assert_eq!(action_name(Action::TabListClose), "tab_list_close");
    }

    // §10.9 less profile.
    #[test]
    fn less_scheme_preview_paging() {
        let m = KeyMap::defaults(KeyScheme::Less);
        // `f` pages; `Space` does not (it is the operation menu, not a page key).
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('f')),
            Resolution::Action(Action::Navigate(Motion::PageDown))
        );
        for ctx in [Context::Text, Context::Document, Context::Table] {
            assert_eq!(
                m.resolve(ctx, None, KeyPress::ch(' ')),
                Resolution::EnterLeader(LeaderId::Actions),
                "{ctx:?}"
            );
        }
        #[cfg(feature = "git")]
        {
            // The diff's `f` was the follow scope, which collided with this page key; it is the
            // page key alone now.
            assert_eq!(
                m.resolve(Context::Diff, None, KeyPress::ch('f')),
                Resolution::Action(Action::Navigate(Motion::PageDown))
            );
            let vim = KeyMap::defaults(KeyScheme::Vim);
            assert_eq!(
                vim.resolve(Context::Diff, None, KeyPress::ch('f')),
                Resolution::Unbound
            );
        }
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('d')),
            Resolution::Action(Action::Navigate(Motion::HalfDown))
        );
        // Tree stays Space=File leader regardless of scheme.
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
    }

    // vim profile (default) Ctrl page paging.
    #[test]
    fn vim_scheme_preview_paging() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ctrl_ch('f')),
            Resolution::Action(Action::Navigate(Motion::PageDown))
        );
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ctrl_ch('d')),
            Resolution::Action(Action::Navigate(Motion::HalfDown))
        );
        // Under vim, plain f/Space are unbound in Preview.
        assert_eq!(
            m.resolve(Context::Text, None, KeyPress::ch('f')),
            Resolution::Unbound
        );
    }

    // §10.10 scheme-inapplicable menus are unaffected (the sort letters of the `,` menu).
    #[test]
    fn scheme_does_not_affect_sort() {
        let vim = KeyMap::defaults(KeyScheme::Vim);
        let less = KeyMap::defaults(KeyScheme::Less);
        for kp in [KeyPress::ch('n'), KeyPress::ch('s'), KeyPress::ch('r')] {
            assert_eq!(
                vim.resolve_leader(Context::Tree, LeaderId::Options, kp),
                less.resolve_leader(Context::Tree, LeaderId::Options, kp)
            );
        }
    }

    // §10.11 Tree has no horizontal movement (Left/Right/0/$).
    #[test]
    fn tree_has_no_horizontal_motion() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('h')),
            Resolution::Action(Action::TreeLeave)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('0')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('$')),
            Resolution::Unbound
        );
    }

    // Global tab/help inheritance and its non-inheritance on text-input surfaces.
    #[test]
    fn global_tab_inheritance() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('t')),
            Resolution::Action(Action::TabNewAfter)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('1')),
            Resolution::Action(Action::TabGoto(0))
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('9')),
            Resolution::Action(Action::TabGoto(8))
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('?')),
            Resolution::Action(Action::ToggleHelp)
        );
        // A text-input surface (Filter) has no context, so it does not inherit Global (nor
        // anything else): `handle_key` never looks a key up for it.
        assert_eq!(Context::of_surface(Surface::Filter, false, false), None);
        assert_eq!(Context::of_surface(Surface::Search, false, false), None);
        assert_eq!(
            Context::of_surface(Surface::DialogConfirmDelete, false, false),
            None
        );
    }

    // Surface predicates.
    #[test]
    fn surface_predicates() {
        assert!(Surface::Filter.is_text_input());
        assert!(!Surface::Tree.is_text_input());
        assert!(Surface::DialogConfirmDelete.is_modal_confirm());
        assert!(!Surface::Help.is_modal_confirm());
        assert!(Surface::Tree.allows_tabs());
        assert!(!Surface::Filter.allows_tabs());
        assert!(!Surface::DialogConfirmDelete.allows_tabs());
        assert!(Surface::Tree.inherits_copy_leader());
        assert!(!Surface::Visual.inherits_copy_leader());
    }

    // An unknown surface name is warned and ignored (other surfaces stay intact).
    #[test]
    fn unknown_surface_warns() {
        let cfg = cfg_with("nonsense", &[("d", "refresh")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert!(m.warnings.iter().any(|w| w.contains("unknown key surface")));
        // The default Tree d is preserved.
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('d')),
            Resolution::Action(Action::OpenGitDiffCursor)
        );
    }

    // §10.10/§10.12 Git-related: scheme-inapplicable surfaces unaffected + git_detail Motion coverage.
    #[cfg(feature = "git")]
    #[test]
    fn git_changes_scheme_invariant() {
        let vim = KeyMap::defaults(KeyScheme::Vim);
        let less = KeyMap::defaults(KeyScheme::Less);
        for kp in [KeyPress::ch('s'), KeyPress::ch('j'), KeyPress::ch('d')] {
            assert_eq!(
                vim.resolve(gs(Screen::Changes), None, kp),
                less.resolve(gs(Screen::Changes), None, kp)
            );
        }
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_detail_navigate_coverage() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(gs(Screen::Detail), None, KeyPress::ch('h')),
            Resolution::Action(Action::Navigate(Motion::Left))
        );
        assert_eq!(
            m.resolve(gs(Screen::Detail), None, KeyPress::ch('l')),
            Resolution::Action(Action::Navigate(Motion::Right))
        );
        assert_eq!(
            m.resolve(gs(Screen::Detail), None, KeyPress::ch('0')),
            Resolution::Action(Action::Navigate(Motion::LineHome))
        );
        assert_eq!(
            m.resolve(gs(Screen::Detail), None, KeyPress::ctrl_ch('f')),
            Resolution::Action(Action::Navigate(Motion::PageDown))
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_surfaces_copy_bindings() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // Changes hub: y→ path-copy leader (path of the changed file).
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        // log/graph/detail: y→ commit-info copy leader.
        for sfc in [gs(Screen::Log), gs(Screen::Graph), gs(Screen::Detail)] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('y')),
                Resolution::EnterLeader(LeaderId::GitCopy),
                "{sfc:?} の y は GitCopy リーダー"
            );
        }
        // Under the GitCopy leader: m→ full message, h→ full hash.
        assert_eq!(
            m.resolve(gs(Screen::Log), Some(LeaderId::GitCopy), KeyPress::ch('m')),
            Resolution::Action(Action::GitCopy(GitCopyKind::Message))
        );
        assert_eq!(
            m.resolve(
                gs(Screen::Graph),
                Some(LeaderId::GitCopy),
                KeyPress::ch('h')
            ),
            Resolution::Action(Action::GitCopy(GitCopyKind::FullHash))
        );
        // branches: y→ the copy menu, whose item `n` copies the branch name (it used to copy
        // directly).
        assert_eq!(
            m.resolve(gs(Screen::Branches), None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::BranchCopy)
        );
        assert_eq!(
            m.resolve(
                gs(Screen::Branches),
                Some(LeaderId::BranchCopy),
                KeyPress::ch('n')
            ),
            Resolution::Action(Action::CopyBranchName)
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_changes_mnemonics() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('d')),
            Resolution::Action(Action::GitWorktreeDiff)
        );
        // `l` opens (`Enter`'s operation), `h` goes back, `g` is the top: the log and the graph
        // have no default key here (the screens' `Tab` cycle reaches them).
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('l')),
            Resolution::Action(Action::GitOpenSelectedDiff)
        );
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('h')),
            Resolution::Action(Action::GitClose)
        );
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('g')),
            Resolution::Action(Action::Navigate(Motion::Top))
        );
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('G')),
            Resolution::Action(Action::Navigate(Motion::Bottom))
        );
        assert_eq!(action_from_str("git_open_log"), Some(Action::GitOpenLog));
        assert_eq!(
            action_from_str("git_open_graph"),
            Some(Action::GitOpenGraph)
        );
        // `s` stages in git and syncs in jj; `R` is neither (it is the representation cycle).
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('s')),
            Resolution::Action(Action::GitStage)
        );
        let jj = Context::Screen(Backend::Jj, Screen::Changes);
        assert_eq!(
            m.resolve(jj, None, KeyPress::ch('s')),
            Resolution::Action(Action::JjSync)
        );
        for c in [gs(Screen::Changes), jj] {
            assert_eq!(m.resolve(c, None, KeyPress::ch('R')), Resolution::Unbound);
        }
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('!')),
            Resolution::Action(Action::GitLaunchTool)
        );
        assert_eq!(
            m.resolve(gs(Screen::Changes), None, KeyPress::ch('w')),
            Resolution::Action(Action::GitOpenWorktrees)
        );
    }

    /// The worktree list's own bindings: `j`/`k`/`g`/`G` movement, `/`→filter, `Ctrl-t`→open in a
    /// new tab, `q`→close. `Enter` is deliberately **not** bound here (it is a fixed key handled by
    /// `handle_enter`, same as branches' Enter→checkout) — `WorktreeGoto` still round-trips through
    /// `action_from_str`/`action_name` below so a user can rebind another key to it.
    #[cfg(feature = "git")]
    #[test]
    fn git_worktrees_bindings() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('j')),
            Resolution::Action(Action::Navigate(Motion::Down))
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('k')),
            Resolution::Action(Action::Navigate(Motion::Up))
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('g')),
            Resolution::Action(Action::Navigate(Motion::Top))
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('G')),
            Resolution::Action(Action::Navigate(Motion::Bottom))
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('/')),
            Resolution::Action(Action::WorktreeFilterStart)
        );
        // `c` creates (`n` is layer 1's next match, so it is not bound in the list).
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('c')),
            Resolution::Action(Action::WorktreeCreate)
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('n')),
            Resolution::Unbound
        );
        // `l` goes to the selected worktree (`Enter`'s operation), `h` goes back.
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('l')),
            Resolution::Action(Action::WorktreeGoto)
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('h')),
            Resolution::Action(Action::WorktreeClose)
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ctrl_ch('t')),
            Resolution::Action(Action::WorktreeGotoNewTab)
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('d')),
            Resolution::Action(Action::WorktreeShowChanges)
        );
        assert_eq!(
            m.resolve(gs(Screen::Worktrees), None, KeyPress::ch('q')),
            Resolution::Action(Action::WorktreeClose)
        );
        // `WorktreeGoto` (`l`, and the fixed `Enter`) round-trips through the config-string layer.
        assert_eq!(action_from_str("worktree_goto"), Some(Action::WorktreeGoto));
        assert_eq!(action_name(Action::WorktreeGoto), "worktree_goto");
        // `WorktreeCreate` (`c`, the row just resolved above) round-trips too.
        assert_eq!(
            action_from_str("worktree_create"),
            Some(Action::WorktreeCreate)
        );
        assert_eq!(action_name(Action::WorktreeCreate), "worktree_create");
    }

    /// Config-string round trip for every new worktree action (mirrors `keymap_actions_round_trip`,
    /// but explicit for the actions that have no default binding to be auto-collected there).
    #[cfg(feature = "git")]
    #[test]
    fn worktree_action_strings_round_trip() {
        let pairs: &[(&str, Action)] = &[
            ("git_open_worktrees", Action::GitOpenWorktrees),
            ("worktree_filter_start", Action::WorktreeFilterStart),
            ("worktree_goto", Action::WorktreeGoto),
            ("worktree_goto_new_tab", Action::WorktreeGotoNewTab),
            ("worktree_close", Action::WorktreeClose),
            ("worktree_show_changes", Action::WorktreeShowChanges),
        ];
        for &(s, a) in pairs {
            assert_eq!(action_from_str(s), Some(a), "{s} → {a:?}");
            assert_eq!(action_name(a), s, "{a:?} → {s}");
        }
    }

    #[test]
    fn parse_code_named_keys_and_invalid() {
        // Covers named keys, aliases, and case-insensitivity.
        assert_eq!(KeyPress::parse_code("space").unwrap(), KeyCode::Char(' '));
        assert_eq!(KeyPress::parse_code("tab").unwrap(), KeyCode::Tab);
        assert_eq!(KeyPress::parse_code("backtab").unwrap(), KeyCode::BackTab);
        assert_eq!(KeyPress::parse_code("Enter").unwrap(), KeyCode::Enter);
        assert_eq!(KeyPress::parse_code("return").unwrap(), KeyCode::Enter);
        assert_eq!(KeyPress::parse_code("ESC").unwrap(), KeyCode::Esc);
        assert_eq!(KeyPress::parse_code("escape").unwrap(), KeyCode::Esc);
        assert_eq!(
            KeyPress::parse_code("backspace").unwrap(),
            KeyCode::Backspace
        );
        assert_eq!(KeyPress::parse_code("del").unwrap(), KeyCode::Delete);
        assert_eq!(KeyPress::parse_code("delete").unwrap(), KeyCode::Delete);
        assert_eq!(KeyPress::parse_code("up").unwrap(), KeyCode::Up);
        assert_eq!(KeyPress::parse_code("down").unwrap(), KeyCode::Down);
        assert_eq!(KeyPress::parse_code("left").unwrap(), KeyCode::Left);
        assert_eq!(KeyPress::parse_code("right").unwrap(), KeyCode::Right);
        assert_eq!(KeyPress::parse_code("home").unwrap(), KeyCode::Home);
        assert_eq!(KeyPress::parse_code("end").unwrap(), KeyCode::End);
        assert_eq!(KeyPress::parse_code("pgup").unwrap(), KeyCode::PageUp);
        assert_eq!(KeyPress::parse_code("pageup").unwrap(), KeyCode::PageUp);
        assert_eq!(KeyPress::parse_code("pgdn").unwrap(), KeyCode::PageDown);
        assert_eq!(KeyPress::parse_code("pagedown").unwrap(), KeyCode::PageDown);
        // A single character keeps its original case.
        assert_eq!(KeyPress::parse_code("a").unwrap(), KeyCode::Char('a'));
        assert_eq!(KeyPress::parse_code("Z").unwrap(), KeyCode::Char('Z'));
        assert_eq!(KeyPress::parse_code("$").unwrap(), KeyCode::Char('$'));
        // An unknown multi-character token is Err.
        assert!(KeyPress::parse_code("abc").is_err(), "未知の複数文字は Err");
        assert!(KeyPress::parse_code("nope").is_err());
    }

    #[test]
    fn leader_menu_set_find_and_remove() {
        // Directly verify LeaderMenu's set/find/remove (the core of leader-menu editing).
        let mut menu = LeaderMenu {
            id: LeaderId::File,
            title: Msg::StFilter,
            items: Vec::new(),
        };
        menu.set(KeyPress::ch('a'), Action::ToggleHelp);
        menu.set(KeyPress::ch('b'), Action::ToggleInfo);
        assert_eq!(menu.items.len(), 2);
        assert_eq!(menu.find(KeyPress::ch('a')), Some(Action::ToggleHelp));
        // A set on the same key replaces it (no duplicates).
        menu.set(KeyPress::ch('a'), Action::Refresh);
        assert_eq!(menu.items.len(), 2, "同一キーは置換");
        assert_eq!(menu.find(KeyPress::ch('a')), Some(Action::Refresh));
        // remove decreases it by one item and it can no longer be found.
        menu.remove(KeyPress::ch('a'));
        assert_eq!(menu.items.len(), 1);
        assert_eq!(
            menu.find(KeyPress::ch('a')),
            None,
            "remove 後は見つからない"
        );
        assert_eq!(
            menu.find(KeyPress::ch('b')),
            Some(Action::ToggleInfo),
            "他は残る"
        );
        // remove on a nonexistent key is a no-op.
        menu.remove(KeyPress::ch('z'));
        assert_eq!(menu.items.len(), 1);
    }

    // Agent Watch: C=changed-files filter / n·N=jump between changes / F=follow (global) /
    // y@=@reference / Y=@path#L.
    #[test]
    fn agent_watch_keys_resolve() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('C')),
            Resolution::Action(Action::ToggleChangedFilter)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('n')),
            Resolution::Action(Action::JumpNextChange)
        );
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch('N')),
            Resolution::Action(Action::JumpPrevChange)
        );
        // F is inherited via global (works from either Tree or Preview).
        for sfc in [Context::Tree, Context::Text, Context::Image] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('F')),
                Resolution::Action(Action::ToggleFollow),
                "F が {sfc:?} でフォロー切替に解決する"
            );
        }
        // y→@ = @reference path copy (a common menu item on every y-leader surface).
        assert_eq!(
            m.resolve(Context::Tree, Some(LeaderId::Copy), KeyPress::ch('@')),
            Resolution::Action(Action::CopyPath(CopyKind::AtRef))
        );
        // y→c = code-block copy (a Copy-leader menu item; whether it's shown depends on the
        // surface/focus).
        assert_eq!(
            m.resolve(Context::Text, Some(LeaderId::Copy), KeyPress::ch('c')),
            Resolution::Action(Action::CopyCodeBlock)
        );
        // Y = @path#L reference for the selection/caret (both the normal and visual surfaces).
        for sfc in [Context::Text, Context::TextVisual] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('Y')),
                Resolution::Action(Action::PreviewCopySelectionRef),
                "Y が {sfc:?} で @参照コピーに解決する"
            );
        }
        // Ctrl-n / Ctrl-p inside the diff view = cycle through changed files (the same Actions as
        // the tree's n/N). `n` / `N` are the next / previous change block of the file, and the
        // follow scope (`f`) has no default key (it moves into the `,` menu in B3b).
        #[cfg(feature = "git")]
        {
            for (kp, a) in [
                (KeyPress::ctrl_ch('n'), Action::JumpNextChange),
                (KeyPress::ctrl_ch('p'), Action::JumpPrevChange),
            ] {
                assert_eq!(
                    m.resolve(Context::Diff, None, kp),
                    Resolution::Action(a),
                    "diff ビューの {} が変更間ジャンプに解決する",
                    kp.label()
                );
            }
            for (c, a) in [('n', Action::DiffNextChange), ('N', Action::DiffPrevChange)] {
                assert_eq!(
                    m.resolve(Context::Diff, None, KeyPress::ch(c)),
                    Resolution::Action(a),
                    "diff ビューの {c}"
                );
            }
            assert_eq!(
                m.resolve(Context::Diff, None, KeyPress::ch('f')),
                Resolution::Unbound,
                "diff ビューの f"
            );
        }
        // Config-string round trip.
        for s in [
            "toggle_changed_filter",
            "jump_next_change",
            "jump_prev_change",
            "toggle_follow",
            "copy_at_ref",
            "copy_code_block",
            "preview_copy_selection_ref",
        ] {
            let a = action_from_str(s).unwrap_or_else(|| panic!("unknown action: {s}"));
            assert_eq!(action_name(a), s, "config 文字列が往復する");
        }
    }

    #[test]
    fn file_duplicate_resolves_and_round_trips() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // Space→ enters the file-management leader.
        assert_eq!(
            m.resolve(Context::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
        // Space→D = duplicate (Tree and Visual share the same File leader).
        for sfc in [Context::Tree, Context::TreeVisual] {
            assert_eq!(
                m.resolve(sfc, Some(LeaderId::File), KeyPress::ch('D')),
                Resolution::Action(Action::FileDuplicate),
                "{sfc:?} の Space→D が複製に解決する"
            );
        }
        // Config-string round trip.
        let a = action_from_str("file_duplicate").expect("file_duplicate は既知アクション");
        assert_eq!(action_name(a), "file_duplicate");
        assert_eq!(a, Action::FileDuplicate);
    }

    /// Clearing the graph's base works with both `x` and `0` (`0` is an alias for the "return to
    /// the anchor" feel). `s` (set base) and `b` (branch panel) work as before, and no base keys
    /// leak onto the log surface.
    #[cfg(feature = "git")]
    #[test]
    fn graph_base_clears_with_x_only() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(gs(Screen::Graph), None, KeyPress::ch('x')),
            Resolution::Action(Action::GitGraphClearBase),
            "グラフの `x` が基準解除に解決する"
        );
        // `0` is layer 1's line start, not a second way to clear the base: the graph does not
        // bind it (it used to).
        assert_eq!(
            m.resolve(gs(Screen::Graph), None, KeyPress::ch('0')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(gs(Screen::Graph), None, KeyPress::ch('s')),
            Resolution::Action(Action::GitGraphSetBase)
        );
        // The log surface has none of the graph's dedicated base keys (neither `0` nor `x` is bound).
        for k in ['x', '0', 's'] {
            assert_eq!(
                m.resolve(gs(Screen::Log), None, KeyPress::ch(k)),
                Resolution::Unbound,
                "log 面に `{k}` は割り当てない"
            );
        }
        // Config-string round trip.
        let a = action_from_str("git_graph_clear_base").expect("既知アクション");
        assert_eq!(action_name(a), "git_graph_clear_base");
        assert_eq!(a, Action::GitGraphClearBase);
    }

    /// A mechanical check that **every action bound to a key by default** round-trips through
    /// `action_name` → `action_from_str` — i.e. the promise that "every command can be reassigned
    /// via `[keys.*]`".
    ///
    /// An individual test only ever looks at "the action I just added", so it misses a wiring gap
    /// where only one side was added. In fact, the graph's base-pinning/branch-panel 8 actions
    /// existed only in `action_name` and not in `action_from_str`, so they could not be reassigned
    /// via config (detected by this test on 2026-07-20).
    #[test]
    fn actions_without_a_default_key_still_round_trip() {
        // Operations whose default key was taken away (they stay for `[keys]`) are not collected
        // by `keymap_actions_round_trip`, which only sees what is bound by default.
        #[cfg_attr(not(feature = "git"), allow(unused_mut))]
        let mut pairs: Vec<(&str, Action)> = vec![
            ("tab_new", Action::TabNew),
            ("open_sort_menu", Action::OpenSortMenu),
            ("toggle_focused_item", Action::ToggleFocusedItem),
        ];
        #[cfg(feature = "git")]
        pairs.extend([
            ("git_open_log", Action::GitOpenLog),
            ("git_open_graph", Action::GitOpenGraph),
            ("toggle_follow_diff_scope", Action::ToggleFollowDiffScope),
            ("jj_sync", Action::JjSync),
        ]);
        for (s, a) in pairs {
            assert_eq!(action_from_str(s), Some(a), "{s}");
            assert_eq!(action_name(a), s, "{a:?}");
        }
    }

    #[test]
    fn keymap_actions_round_trip() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        let mut actions: Vec<Action> = Vec::new();
        let mut collect = |b: &Binding| {
            if let Binding::Run(a) = b {
                actions.push(*a);
            }
        };
        for table in m.contexts.values().chain(m.types.values()) {
            table.values().for_each(&mut collect);
        }
        m.basic.values().for_each(&mut collect);
        m.global.values().for_each(&mut collect);
        for menu in m.leaders.values() {
            actions.extend(menu.items.iter().map(|i| i.action));
        }
        assert!(!actions.is_empty(), "既定キーマップが空ではない");

        // Intentional exception: `tab_goto` is fixed to digit keys 1-9, and trying to change it via
        // config makes `apply_binding` explicitly warn and ignore it (§1.2). So not round-tripping
        // is correct. A new exception may only be added once its reason is written here.
        const FIXED_KEY_ACTIONS: &[&str] = &["tab_goto"];

        let mut missing: Vec<String> = Vec::new();
        for a in actions {
            let name = action_name(a).to_string();
            if FIXED_KEY_ACTIONS.contains(&name.as_str()) {
                continue;
            }
            match action_from_str(&name) {
                // Round-trip back to the same action (does not turn into an alias).
                Some(back) if back == a => {}
                _ => missing.push(name),
            }
        }
        missing.sort_unstable();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "action_from_str に無い(=config で再割り当てできない)アクション: {missing:?}"
        );
    }

    /// Regression test for bug 3, for contexts: every context that has a table (= its key bindings
    /// actually exist and fire when pressed) has a `[keys.<name>]` name, the name resolves back to
    /// the context, and a binding written under that name really lands on it.
    /// `gs(Screen::GraphPicker)` once had a table but no config name, so the graph branch panel's 5
    /// actions (which `config.example.toml` advertised as configurable) could not be reassigned via
    /// config. This keeps the "registry" (`contexts`) and the "config-name registry"
    /// (`Context::name` / `Context::from_name`) in sync, and `Context::all()` with both.
    #[test]
    fn keymap_names_cover_every_bindable_context() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        let mut missing: Vec<String> = Vec::new();
        for ctx in Context::all() {
            if !m.contexts.contains_key(&ctx) {
                missing.push(format!("{ctx:?} は Context::all() にあるが既定の表が無い"));
            }
            let name = ctx.name();
            if Context::from_name(&name) != Some(ctx) {
                missing.push(format!("{ctx:?} => {name:?} が往復しない"));
            }
            // Reachable from config: a binding written under the name takes effect.
            let cfg = cfg_with(&name, &[("Z", "refresh")]);
            let c = KeyMap::from_config(KeyScheme::Vim, &cfg);
            if c.resolve(ctx, None, KeyPress::ch('Z')) != Resolution::Action(Action::Refresh) {
                missing.push(format!("[keys.{name}] の割り当てが {ctx:?} に届かない"));
            }
            if !c.warnings.is_empty() {
                missing.push(format!("[keys.{name}] が警告: {:?}", c.warnings));
            }
        }
        for &ctx in m.contexts.keys() {
            if !Context::all().contains(&ctx) {
                missing.push(format!("{ctx:?} は表があるが Context::all() に無い"));
            }
        }
        // The names are unique.
        let mut names: Vec<String> = Context::all().iter().map(|c| c.name()).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "文脈名が重複している");
        missing.sort_unstable();
        assert!(
            missing.is_empty(),
            "contexts ↔ config 文脈名がずれている: {missing:#?}"
        );
    }
}
