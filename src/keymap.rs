// konoma キーマップ層 (Run2: キー体系再設計の下位純データ層)。
//
// 役割: 「面(Surface) × キー(KeyPress) → コマンド(Action)」の宣言的マップを内蔵既定として持ち、
// 設定(`[keys.<surface>]`)で上書き/追加/無効化し、起動時に衝突を検知してフォールバックする。
// App 状態には一切触れない (面は引数で受ける)。crossterm の KeyCode/KeyModifiers のみ参照する。
//
// 上位 (main::handle_key / dispatch_action) はここで解決した `Action` を実行する。

use std::collections::HashMap;

use anyhow::{bail, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[cfg(feature = "git")]
use crate::app::GitCopyKind;
use crate::app::{CopyKind, SortKey, TableCopyKind};
use crate::i18n::Msg;

// =============================================================================
// Motion (共有の移動/スクロール量)。per-Surface 解釈は dispatch_action 側 (Stage 3)。
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
// Action (全コマンド集合)。
// =============================================================================

/// The full set of commands. Corresponds both ways between keymap values (`Binding::Run`) and config strings (§1.2).
/// Git-related variants exist only under `feature="git"` (keeps `--no-default-features` green).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A key disabled via config `"noop"`/`"disabled"` (does nothing at runtime).
    Noop,
    /// Move / scroll (interpreted per surface).
    Navigate(Motion),

    // --- Global (タブ/ヘルプ/パスコピー) ---
    TabNew,
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

    // --- Tree (通常) ---
    Quit,
    /// `q` at the tree top level: close the current tab if more than one is open, otherwise quit the app.
    CloseTabOrQuit,
    FilterStart,
    TreeDescend,
    TreeActivate,
    TreeLeave,
    ToggleHidden,
    ToggleInfo,
    RequestEdit,
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
    /// (watch an AI agent work); any other key stops following.
    ToggleFollow,

    // --- ファイル管理 (Space→ リーダー配下 / Visual も共有) ---
    FileCreate,
    FileRename,
    FileDelete,
    FileCopy,
    FileCut,
    FilePaste,
    FileDuplicate,

    // --- Tree:Visual サブ ---
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
    /// `R`: toggle a Markdown/Mermaid preview between its decorated render and raw source (selectable).
    ToggleMarkdownRaw,
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
    /// PDF: next/previous page (inert for non-PDF image previews; the handler gates on the kind).
    PdfNextPage,
    PdfPrevPage,

    // --- Preview: file paging (text/image/table 共通) ---
    /// Preview the next/previous **file** in tree display order (directories are skipped,
    /// wraps at the ends, and the tree cursor follows).
    PreviewFileNext,
    PreviewFilePrev,

    // --- Preview: table (csv/tsv) ---
    /// Copy the current cell / row / column of a CSV/TSV table (via the `y→` menu).
    TableCopy(TableCopyKind),

    // --- Preview: gitdiff / GitDetail 共通 ---
    #[cfg(feature = "git")]
    GitDiffDiscard,
    #[cfg(feature = "git")]
    CycleDiffLayout,

    // --- Git 変更ハブ (o) ---
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

    // --- Git branches ---
    #[cfg(feature = "git")]
    BranchFilterStart,
    #[cfg(feature = "git")]
    BranchCheckout,
    #[cfg(feature = "git")]
    BranchCreate,
    #[cfg(feature = "git")]
    BranchDelete,

    // --- Git コピー (y→ コミット情報 / branches ブランチ名) ---
    /// log/graph/detail: copy the selected commit's info (hash / subject / message / author / date).
    #[cfg(feature = "git")]
    GitCopy(GitCopyKind),
    /// branches: copy the selected branch name.
    #[cfg(feature = "git")]
    CopyBranchName,

    // --- Git 系・閉じる ---
    #[cfg(feature = "git")]
    GitClose,

    // --- Sort メニュー ---
    SortSet(SortKey),
    SortToggleReverse,
    SortToggleDirsFirst,

    // --- タブ一覧 (`T`) ---
    ToggleTabList,
    TabListClose,
    // --- Bookmark 一覧 ---
    BookmarkJump,
    BookmarkEdit,
    BookmarkDelete,
    BookmarkClose,

    // --- Info ---
    InfoClose,
}

// =============================================================================
// Surface (最前面サーフェス = 唯一の真実源)。Stage 3 で internal_mode を置換する。
// =============================================================================

/// The frontmost "surface" currently receiving keys. Four categories: fixed text input / confirm
/// modal / overlay / basic full-screen. Git-related surfaces exist only under `feature="git"` (§1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    // --- 固定テキスト入力 (keymap 非適用・文字/編集キー横取り) ---
    DialogInput,
    Filter,
    Search,
    Mark,
    #[cfg(feature = "git")]
    BranchFilter,

    // --- 確認モーダル (y/n/c/m/! 固定) ---
    DialogConfirmDelete,
    DialogConfirmDrop,
    DialogRenamePreview,
    /// App-quit confirmation (`q`/`y`/Enter = quit, `n`/Esc = cancel).
    DialogConfirmQuit,
    /// Bookmark-overwrite confirmation (`y`/Enter = overwrite, `n`/Esc = cancel).
    DialogConfirmBookmark,

    // --- オーバーレイ (keymap 駆動) ---
    Help,
    Sort,
    Bookmarks,
    /// Tab-list overlay (`T`): j/k + Enter switch, `w` closes the selected tab.
    Tabs,
    Info,
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

    // --- 基本全画面 (keymap 駆動) ---
    Visual,
    Tree,
    PreviewText,
    /// Windowed (Code/Text) preview line-selection mode (`v`): `j`/`k` extend, `y` copies the lines.
    PreviewTextVisual,
    PreviewImage,
    /// CSV/TSV table preview (cell cursor + `y→` cell/row/column copy).
    PreviewTable,
    #[cfg(feature = "git")]
    PreviewGitDiff,
}

impl Surface {
    /// Whether this is a text-input surface (intercepts character/editing keys and does not apply the keymap).
    pub fn is_text_input(self) -> bool {
        match self {
            Surface::DialogInput | Surface::Filter | Surface::Search | Surface::Mark => true,
            #[cfg(feature = "git")]
            Surface::BranchFilter => true,
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

    /// Whether this surface inherits the Global-layer keys such as tabs/help (= a keymap/Global-driven surface).
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
// KeyPress / KeyChord / Binding。
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
                // 単文字 (元の大文字小文字を保持)。複数文字なら不正トークン。
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
}

// =============================================================================
// リーダー (which-key) 定義。
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
// KeyMap (二層: per-Surface + Global) と解決結果。
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

/// Raw representation of the config file's `[keys.<surface>]` (surface name → (chord string → action string)).
/// In Stage 2, config/mod.rs uses serde to populate this type and passes it to App. The keymap layer does not depend on serde.
#[derive(Debug, Clone, Default)]
pub struct KeysFileConfig {
    /// Surface name (snake_case; including "global") → (chord string → action string).
    pub surfaces: HashMap<String, HashMap<String, String>>,
}

/// One key conflict detected at startup (#17 / FR-8).
/// The current startup flash only summarizes the count (`App::keymap_report`). The fields are kept for
/// future detailed display (the conflict breakdown in which-key/help).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct KeyConflict {
    pub surface: Surface,
    pub key: KeyPress,
    /// Description of the kept side (the default).
    pub kept: String,
    /// Description of the dropped override.
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
#[derive(Debug, Clone)]
pub struct KeyMap {
    pub per_surface: HashMap<Surface, ContextMap>,
    /// Tabs/help etc. `allows_tabs()` surfaces inherit these as a fallback.
    pub global: ContextMap,
    pub leaders: HashMap<LeaderId, LeaderMenu>,
    pub conflicts: Vec<KeyConflict>,
    /// Warnings for ignored config such as unknown surface names/actions or fixed-key rebinds (for the startup flash).
    pub warnings: Vec<String>,
}

impl KeyMap {
    /// Build the built-in default keymap (§2). `scheme` only swaps the page/half rows of the Preview surfaces.
    pub fn defaults(scheme: KeyScheme) -> KeyMap {
        let nav = |m: Motion| Binding::Run(Action::Navigate(m));
        let run = |a: Action| Binding::Run(a);

        let mut per_surface: HashMap<Surface, ContextMap> = HashMap::new();

        // --- Tree (通常) ---
        let mut tree: ContextMap = HashMap::new();
        tree.insert(KeyPress::ch('j'), nav(Motion::Down));
        tree.insert(KeyPress::ch('k'), nav(Motion::Up));
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
        tree.insert(KeyPress::ch('s'), run(Action::OpenSortMenu));
        tree.insert(KeyPress::ch('i'), run(Action::ToggleInfo));
        tree.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        tree.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        tree.insert(KeyPress::ch('m'), run(Action::MarkSet));
        tree.insert(KeyPress::ch('\''), run(Action::MarkJump));
        tree.insert(KeyPress::ch('d'), run(Action::OpenGitDiffCursor));
        tree.insert(KeyPress::ch('o'), run(Action::OpenGitView));
        tree.insert(KeyPress::ch('a'), run(Action::SetAnchor));
        tree.insert(KeyPress::ch('A'), run(Action::ResetAnchor));
        tree.insert(KeyPress::ch('v'), run(Action::EnterVisual));
        tree.insert(KeyPress::ch('V'), run(Action::ToggleSelect));
        // Agent Watch: C=変更ファイルのみ表示 / n・N=次/前の変更ファイルへジャンプ。
        tree.insert(KeyPress::ch('C'), run(Action::ToggleChangedFilter));
        tree.insert(KeyPress::ch('n'), run(Action::JumpNextChange));
        tree.insert(KeyPress::ch('N'), run(Action::JumpPrevChange));
        tree.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        tree.insert(KeyPress::ch(' '), Binding::Leader(LeaderId::File));
        // Ctrl-t=カーソル下のエントリを別タブで開く(ファイル=プレビュー / ディレクトリ=新タブの root)。
        // preview の OpenLinkNewTab と同じキー/意味(「これを別タブで」)＝global t(新規タブ)と対。
        tree.insert(KeyPress::ctrl_ch('t'), run(Action::OpenInNewTab));
        per_surface.insert(Surface::Tree, tree);

        // --- Tree:Visual ---
        let mut visual: ContextMap = HashMap::new();
        visual.insert(KeyPress::ch('j'), nav(Motion::Down));
        visual.insert(KeyPress::ch('k'), nav(Motion::Up));
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
        visual.insert(KeyPress::ch('q'), run(Action::Quit)); // 旧 Visual の q=終了 を保全(回帰防止)
        per_surface.insert(Surface::Visual, visual);

        // --- Preview: text/markdown ---
        let mut ptext: ContextMap = HashMap::new();
        ptext.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        ptext.insert(KeyPress::ch('/'), run(Action::SearchStart));
        ptext.insert(KeyPress::ch('n'), run(Action::SearchNext));
        ptext.insert(KeyPress::ch('N'), run(Action::SearchPrev));
        ptext.insert(KeyPress::ch('j'), nav(Motion::Down));
        ptext.insert(KeyPress::ch('k'), nav(Motion::Up));
        ptext.insert(KeyPress::ch('g'), nav(Motion::Top));
        ptext.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        ptext.insert(KeyPress::ch('l'), nav(Motion::Right));
        ptext.insert(KeyPress::ch('h'), nav(Motion::Left));
        ptext.insert(KeyPress::ch('0'), nav(Motion::LineHome));
        ptext.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
        ptext.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        ptext.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        ptext.insert(KeyPress::ch('v'), run(Action::PreviewEnterVisual));
        ptext.insert(KeyPress::ch('V'), run(Action::PreviewEnterVisualLine));
        ptext.insert(KeyPress::ch('R'), run(Action::ToggleMarkdownRaw));
        // Ctrl-t=フォーカス中の Markdown リンクを別タブで開く(Enter は同タブのまま)。
        // TUI 定番(fzf/Telescope の Ctrl-t=新タブ)＋konoma の t=新規タブと一貫。Ctrl+英字は
        // 全端末＋tmux で確実(Ctrl+Enter と違い kitty protocol 不要)。リンク未フォーカスでは no-op。
        ptext.insert(KeyPress::ctrl_ch('t'), run(Action::OpenLinkNewTab));
        // Ctrl-n/Ctrl-p=ツリー表示順で次/前のファイルをプレビュー(ファイル送り)。J/K は画像面の
        // PDF ページ送りと衝突、Ctrl-j はレガシー端末/tmux で Enter(LF)と区別不能のため不採用。
        ptext.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        ptext.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        // Y=キャレット行の @path#L 参照コピー(Claude Code へ場所を渡す)。選択中は範囲(visual 面の Y)。
        ptext.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        ptext.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        ptext.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        apply_scheme_paging(&mut ptext, scheme);
        ptext.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        // ブックマーク: プレビュー中のファイルを m で登録・' で一覧(ツリーと同じ action)。
        ptext.insert(KeyPress::ch('m'), run(Action::MarkSet));
        ptext.insert(KeyPress::ch('\''), run(Action::MarkJump));
        per_surface.insert(Surface::PreviewText, ptext);

        // --- Preview: text/code visual selection (v charwise / V linewise) ---
        // h/j/k/l で 2D キャレットを動かして範囲を伸ばし、y でコピー、v・V・q・Esc で抜ける。
        let mut pvis: ContextMap = HashMap::new();
        pvis.insert(KeyPress::ch('j'), nav(Motion::Down));
        pvis.insert(KeyPress::ch('k'), nav(Motion::Up));
        pvis.insert(KeyPress::ch('g'), nav(Motion::Top));
        pvis.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        pvis.insert(KeyPress::ch('l'), nav(Motion::Right));
        pvis.insert(KeyPress::ch('h'), nav(Motion::Left));
        pvis.insert(KeyPress::ch('0'), nav(Motion::LineHome));
        pvis.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
        pvis.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        pvis.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        apply_scheme_paging(&mut pvis, scheme);
        pvis.insert(KeyPress::ch('y'), run(Action::PreviewCopySelection));
        pvis.insert(KeyPress::ch('Y'), run(Action::PreviewCopySelectionRef));
        pvis.insert(KeyPress::ch('v'), run(Action::PreviewExitVisual));
        pvis.insert(KeyPress::ch('V'), run(Action::PreviewExitVisual));
        pvis.insert(KeyPress::ch('q'), run(Action::PreviewExitVisual));
        per_surface.insert(Surface::PreviewTextVisual, pvis);

        // --- Preview: image ---
        let mut pimg: ContextMap = HashMap::new();
        pimg.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        pimg.insert(KeyPress::ch('+'), run(Action::ImageZoomIn));
        pimg.insert(KeyPress::ch('-'), run(Action::ImageZoomOut));
        pimg.insert(KeyPress::ch('0'), run(Action::ImageZoomReset));
        pimg.insert(KeyPress::ch('='), run(Action::ImageZoomReset));
        pimg.insert(KeyPress::ch('h'), nav(Motion::Left));
        pimg.insert(KeyPress::ch('l'), nav(Motion::Right));
        pimg.insert(KeyPress::ch('k'), nav(Motion::Up));
        pimg.insert(KeyPress::ch('j'), nav(Motion::Down));
        // PDF ページ送り(画像系で共有・非 PDF では handler が no-op)。lowercase jk=ページ内パン / 大文字 JK=ページ移動。
        pimg.insert(KeyPress::ch('J'), run(Action::PdfNextPage));
        pimg.insert(KeyPress::ch('K'), run(Action::PdfPrevPage));
        pimg.insert(KeyPress::key(KeyCode::PageDown), run(Action::PdfNextPage));
        pimg.insert(KeyPress::key(KeyCode::PageUp), run(Action::PdfPrevPage));
        pimg.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        pimg.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        // Ctrl-n/Ctrl-p=ファイル送り(J/K は PDF ページ送りに割当済みなので Ctrl 側)。
        pimg.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        pimg.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        pimg.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
        pimg.insert(KeyPress::ch('m'), run(Action::MarkSet));
        pimg.insert(KeyPress::ch('\''), run(Action::MarkJump));
        per_surface.insert(Surface::PreviewImage, pimg);

        // --- Preview: table (csv/tsv) ---
        // hjkl = セルカーソル移動 / g・G = 先頭・末尾行 / 0・$ = 先頭・末尾列 / y→ = セル/行/列コピー。
        let mut ptbl: ContextMap = HashMap::new();
        ptbl.insert(KeyPress::ch('q'), run(Action::PreviewBack));
        ptbl.insert(KeyPress::ch('j'), nav(Motion::Down));
        ptbl.insert(KeyPress::ch('k'), nav(Motion::Up));
        ptbl.insert(KeyPress::ch('h'), nav(Motion::Left));
        ptbl.insert(KeyPress::ch('l'), nav(Motion::Right));
        ptbl.insert(KeyPress::ch('g'), nav(Motion::Top));
        ptbl.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        ptbl.insert(KeyPress::ch('0'), nav(Motion::LineHome));
        ptbl.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
        ptbl.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
        ptbl.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
        apply_scheme_paging(&mut ptbl, scheme);
        ptbl.insert(KeyPress::ch('p'), run(Action::CyclePathStyle));
        ptbl.insert(KeyPress::ch('e'), run(Action::RequestEdit));
        // Ctrl-n/Ctrl-p=ファイル送り(text/image と同キー)。
        ptbl.insert(KeyPress::ctrl_ch('n'), run(Action::PreviewFileNext));
        ptbl.insert(KeyPress::ctrl_ch('p'), run(Action::PreviewFilePrev));
        ptbl.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::TableCopy));
        ptbl.insert(KeyPress::ch('m'), run(Action::MarkSet));
        ptbl.insert(KeyPress::ch('\''), run(Action::MarkJump));
        per_surface.insert(Surface::PreviewTable, ptbl);

        // --- Git 系の面 (feature gate) ---
        #[cfg(feature = "git")]
        {
            // Preview: gitdiff
            let mut pgit: ContextMap = HashMap::new();
            pgit.insert(KeyPress::ch('q'), run(Action::PreviewBack));
            pgit.insert(KeyPress::ch('x'), run(Action::GitDiffDiscard));
            pgit.insert(KeyPress::ch('s'), run(Action::CycleDiffLayout));
            pgit.insert(KeyPress::ch('j'), nav(Motion::Down));
            pgit.insert(KeyPress::ch('k'), nav(Motion::Up));
            pgit.insert(KeyPress::ch('l'), nav(Motion::Right));
            pgit.insert(KeyPress::ch('h'), nav(Motion::Left));
            pgit.insert(KeyPress::ch('0'), nav(Motion::LineHome));
            pgit.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
            pgit.insert(KeyPress::ch('g'), nav(Motion::Top));
            pgit.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            pgit.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
            pgit.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
            apply_scheme_paging(&mut pgit, scheme);
            // n/N=次/前の変更ファイルの diff へ切替(ビューを出ずに変更を回遊。ツリーの n/N と同義)。
            pgit.insert(KeyPress::ch('n'), run(Action::JumpNextChange));
            pgit.insert(KeyPress::ch('N'), run(Action::JumpPrevChange));
            pgit.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
            per_surface.insert(Surface::PreviewGitDiff, pgit);

            // Git 変更ハブ (o)。Enter→GitOpenSelectedDiff は固定キーで発火。
            let mut gchg: ContextMap = HashMap::new();
            gchg.insert(KeyPress::ch('j'), nav(Motion::Down));
            gchg.insert(KeyPress::ch('k'), nav(Motion::Up));
            gchg.insert(KeyPress::ch('s'), run(Action::GitStage));
            gchg.insert(KeyPress::ch('u'), run(Action::GitUnstage));
            gchg.insert(KeyPress::ch('S'), run(Action::GitStageAll));
            gchg.insert(KeyPress::ch('U'), run(Action::GitUnstageAll));
            gchg.insert(KeyPress::ch('x'), run(Action::GitDiscard));
            gchg.insert(KeyPress::ch('c'), run(Action::GitCommit));
            gchg.insert(KeyPress::ch('d'), run(Action::GitWorktreeDiff));
            gchg.insert(KeyPress::ch('b'), run(Action::GitOpenBranches));
            gchg.insert(KeyPress::ch('l'), run(Action::GitOpenLog));
            gchg.insert(KeyPress::ch('g'), run(Action::GitOpenGraph));
            gchg.insert(KeyPress::ch('!'), run(Action::GitLaunchTool));
            gchg.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ 選択中の変更ファイルのパスをコピー(ツリーと同じパスコピーメニューを流用)。
            gchg.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::Copy));
            per_surface.insert(Surface::GitChanges, gchg);

            // Git log / graph (同一)。Enter→GitOpenDetail は固定キー。
            let mut glog: ContextMap = HashMap::new();
            glog.insert(KeyPress::ch('j'), nav(Motion::Down));
            glog.insert(KeyPress::ch('k'), nav(Motion::Up));
            glog.insert(KeyPress::ch('g'), nav(Motion::Top));
            glog.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            glog.insert(KeyPress::ch('l'), run(Action::GitOpenDetail));
            glog.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ コミット情報コピー(短/完全ハッシュ・件名・全文メッセージ・著者・日付)。log/graph 共通。
            glog.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::GitCopy));
            // グラフは基準ブランチ固定(Phase 2)を持つ: s=設定 / x=解除 / b=ブランチ表示パネル。log には無いので別マップ。
            let mut ggraph = glog.clone();
            ggraph.insert(KeyPress::ch('s'), run(Action::GitGraphSetBase));
            ggraph.insert(KeyPress::ch('x'), run(Action::GitGraphClearBase));
            ggraph.insert(KeyPress::ch('b'), run(Action::GitGraphOpenPicker));
            per_surface.insert(Surface::GitGraph, ggraph);
            per_surface.insert(Surface::GitLog, glog);

            // グラフのブランチ表示パネル: j/k/g/G ナビ ＋ Space:切替 / a:全部 / n:現在のみ。
            // Enter:適用 / q・Esc:取消 は固定キー。
            let mut gpick: ContextMap = HashMap::new();
            gpick.insert(KeyPress::ch('j'), nav(Motion::Down));
            gpick.insert(KeyPress::ch('k'), nav(Motion::Up));
            gpick.insert(KeyPress::ch('g'), nav(Motion::Top));
            gpick.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gpick.insert(KeyPress::ch(' '), run(Action::GitGraphPickerToggle));
            gpick.insert(KeyPress::ch('a'), run(Action::GitGraphPickerAll));
            gpick.insert(KeyPress::ch('n'), run(Action::GitGraphPickerCurrentOnly));
            // 優先順の並び替え(大文字 J/K)。小文字 j/k はカーソル移動。
            gpick.insert(KeyPress::ch('K'), run(Action::GitGraphPickerMoveUp));
            gpick.insert(KeyPress::ch('J'), run(Action::GitGraphPickerMoveDown));
            gpick.insert(KeyPress::ch('q'), run(Action::GitClose));
            per_surface.insert(Surface::GitGraphPicker, gpick);

            // Git branches。Enter→BranchCheckout は固定キー。
            let mut gbr: ContextMap = HashMap::new();
            gbr.insert(KeyPress::ch('j'), nav(Motion::Down));
            gbr.insert(KeyPress::ch('k'), nav(Motion::Up));
            gbr.insert(KeyPress::ch('g'), nav(Motion::Top));
            gbr.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gbr.insert(KeyPress::ch('/'), run(Action::BranchFilterStart));
            gbr.insert(KeyPress::ch('l'), run(Action::BranchCheckout));
            gbr.insert(KeyPress::ch('n'), run(Action::BranchCreate));
            gbr.insert(KeyPress::ch('d'), run(Action::BranchDelete));
            gbr.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ 選択ブランチ名をコピー(対象が1つなのでメニュー無しで即コピー)。
            gbr.insert(KeyPress::ch('y'), run(Action::CopyBranchName));
            per_surface.insert(Surface::GitBranches, gbr);

            // Git detail (全 Motion 有効・scheme で page/half 差し替え)。
            let mut gdet: ContextMap = HashMap::new();
            gdet.insert(KeyPress::ch('j'), nav(Motion::Down));
            gdet.insert(KeyPress::ch('k'), nav(Motion::Up));
            gdet.insert(KeyPress::ch('l'), nav(Motion::Right));
            gdet.insert(KeyPress::ch('h'), nav(Motion::Left));
            gdet.insert(KeyPress::ch('0'), nav(Motion::LineHome));
            gdet.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
            gdet.insert(KeyPress::ch('g'), nav(Motion::Top));
            gdet.insert(KeyPress::ch('G'), nav(Motion::Bottom));
            gdet.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
            gdet.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
            gdet.insert(KeyPress::ch('s'), run(Action::CycleDiffLayout));
            gdet.insert(KeyPress::ch('q'), run(Action::GitClose));
            // y→ コミット情報コピー(全文メッセージを読みながらコピーできる)。
            gdet.insert(KeyPress::ch('y'), Binding::Leader(LeaderId::GitCopy));
            apply_scheme_paging(&mut gdet, scheme);
            per_surface.insert(Surface::GitDetail, gdet);
        }

        // --- Sort メニュー (scheme 不問) ---
        let mut sort: ContextMap = HashMap::new();
        sort.insert(KeyPress::ch('n'), run(Action::SortSet(SortKey::Name)));
        sort.insert(KeyPress::ch('s'), run(Action::SortSet(SortKey::Size)));
        sort.insert(KeyPress::ch('m'), run(Action::SortSet(SortKey::Modified)));
        sort.insert(KeyPress::ch('e'), run(Action::SortSet(SortKey::Ext)));
        sort.insert(KeyPress::ch('r'), run(Action::SortToggleReverse));
        sort.insert(KeyPress::ch('.'), run(Action::SortToggleDirsFirst));
        per_surface.insert(Surface::Sort, sort);

        // --- Bookmark 一覧。Enter→BookmarkJump は固定キー。素の英字はブックマーク名ジャンプに
        // 予約(main の Unbound フォールバック)なので、編集/削除は Ctrl 修飾に置く。 ---
        let mut bm: ContextMap = HashMap::new();
        bm.insert(KeyPress::ch('j'), nav(Motion::Down));
        bm.insert(KeyPress::ch('k'), nav(Motion::Up));
        bm.insert(KeyPress::ctrl_ch('e'), run(Action::BookmarkEdit));
        bm.insert(KeyPress::ctrl_ch('d'), run(Action::BookmarkDelete));
        bm.insert(KeyPress::ch('q'), run(Action::BookmarkClose));
        bm.insert(KeyPress::ch('\''), run(Action::BookmarkClose));
        per_surface.insert(Surface::Bookmarks, bm);

        // --- タブ一覧 (`T` で開閉。Enter=切替 は固定キー・T は global 継承で閉じる) ---
        let mut tl: ContextMap = HashMap::new();
        tl.insert(KeyPress::ch('j'), nav(Motion::Down));
        tl.insert(KeyPress::ch('k'), nav(Motion::Up));
        tl.insert(KeyPress::ch('d'), run(Action::TabListClose));
        tl.insert(KeyPress::ch('q'), run(Action::ToggleTabList));
        per_surface.insert(Surface::Tabs, tl);

        // --- Info ---
        let mut info: ContextMap = HashMap::new();
        info.insert(KeyPress::ch('i'), run(Action::InfoClose));
        info.insert(KeyPress::ch('q'), run(Action::InfoClose));
        per_surface.insert(Surface::Info, info);

        // --- Help (?/Esc は Global/固定で閉じる。q もここで閉じる) ---
        let mut help: ContextMap = HashMap::new();
        help.insert(KeyPress::ch('j'), nav(Motion::Down));
        help.insert(KeyPress::ch('k'), nav(Motion::Up));
        help.insert(KeyPress::ch('g'), nav(Motion::Top));
        help.insert(KeyPress::ch('G'), nav(Motion::Bottom));
        help.insert(KeyPress::ch('q'), run(Action::ToggleHelp));
        per_surface.insert(Surface::Help, help);

        // --- Global (allows_tabs 面が継承) ---
        let mut global: ContextMap = HashMap::new();
        // Q=アプリ全終了。allows_tabs() の全面(入力/確認モーダル以外)が global を継承するので、
        // どの面からでも Q で抜けられる。`[keys.global]` で変更可・起動時 validate() で衝突検知。
        global.insert(KeyPress::ch('Q'), run(Action::Quit));
        // F=フォローモード(外部変更へ自動ジャンプ)。Tree/Preview どちらからでも切替できるよう global。
        global.insert(KeyPress::ch('F'), run(Action::ToggleFollow));
        global.insert(KeyPress::ch('t'), run(Action::TabNew));
        global.insert(KeyPress::ch('T'), run(Action::ToggleTabList));
        // P=クリップボードのパス/GitHub リンクを読んでその位置へ移動(reveal + preview)。
        // Tree/Preview どちらからでも使えるよう global。`[keys.global]` で変更可。
        global.insert(KeyPress::ch('P'), run(Action::PasteJump));
        // `w` に既定バインドは置かない: vim の単語移動の癖で誤爆しやすく、タブを閉じるのは
        // ツリーの `q`(CloseTabOrQuit)に一本化(2026-07-07 ユーザー決定)。`tab_close` アクション
        // 自体は残っているので `[keys.global] w = "tab_close"` で復活できる。
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

        KeyMap {
            per_surface,
            global,
            leaders,
            conflicts: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// defaults → merge config → validate conflicts (§6/§7). Always falls back and returns green (never panics).
    pub fn from_config(scheme: KeyScheme, cfg: &KeysFileConfig) -> KeyMap {
        let mut map = KeyMap::defaults(scheme);
        let defaults = KeyMap::defaults(scheme); // ロールバック用の控え。
        let mut warnings: Vec<String> = Vec::new();

        for (sfc_name, table) in &cfg.surfaces {
            let target = match key_target_from_name(sfc_name) {
                Some(t) => t,
                None => {
                    warnings.push(format!("unknown key surface: {sfc_name}"));
                    continue;
                }
            };
            for (chord_str, action_str) in table {
                map.apply_one(target, chord_str, action_str, &mut warnings);
            }
        }

        map.validate(&defaults);
        map.warnings = warnings;
        map
    }

    /// Apply one (chord string → action string) (§6 step ③). On failure, warn and ignore.
    fn apply_one(
        &mut self,
        target: KeyTarget,
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
        // 数字固定の TabGoto を設定しようとしたら明示警告 (§1.2)。
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
                let cmap = self.context_mut(target);
                match action {
                    Some(a) => {
                        cmap.insert(kp, Binding::Run(a));
                    }
                    None => {
                        cmap.remove(&kp);
                    }
                }
            }
            KeyChord::Chord(pre, suf) => {
                let lead = match leader_for_prefix(pre) {
                    Some(l) => l,
                    None => {
                        warnings.push(format!(
                            "unsupported chord prefix (only `space`/`y` leaders are supported): {chord_str:?}"
                        ));
                        return;
                    }
                };
                let menu = match self.leaders.get_mut(&lead) {
                    Some(m) => m,
                    None => return,
                };
                match action {
                    Some(a) => menu.set(suf, a),
                    None => menu.remove(suf),
                }
            }
        }
    }

    fn context_mut(&mut self, target: KeyTarget) -> &mut ContextMap {
        match target {
            KeyTarget::Global => &mut self.global,
            KeyTarget::Surface(s) => self.per_surface.entry(s).or_default(),
        }
    }

    /// Detect post-merge conflicts and revert conflicting overrides to the defaults (§7). `defaults` is the kept default copy.
    fn validate(&mut self, defaults: &KeyMap) {
        let mut conflicts: Vec<KeyConflict> = Vec::new();
        let global_keys: Vec<KeyPress> = self.global.keys().copied().collect();
        let surfaces: Vec<Surface> = self.per_surface.keys().copied().collect();

        for sfc in surfaces {
            // --- PrefixVsSingle: 既定でリーダー prefix だったキーが Run に奪われた ---
            let leader_keys: Vec<(KeyPress, LeaderId)> = defaults
                .per_surface
                .get(&sfc)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, b)| match b {
                            Binding::Leader(id) => Some((*k, *id)),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            for (k, id) in leader_keys {
                let cur = self.per_surface.get(&sfc).and_then(|m| m.get(&k)).cloned();
                if let Some(Binding::Run(a)) = cur {
                    conflicts.push(KeyConflict {
                        surface: sfc,
                        key: k,
                        kept: format!("leader:{}", leader_name(id)),
                        dropped: action_name(a),
                        reason: ConflictKind::PrefixVsSingle,
                    });
                    self.per_surface
                        .get_mut(&sfc)
                        .expect("surface present")
                        .insert(k, Binding::Leader(id));
                }
            }

            // --- GlobalShadow: allows_tabs 面で Global 既定キーを別 Action に奪った ---
            // 既定マップ自身が持つ面別特化(例: タブ一覧の `w`=選択タブを閉じる)は合法。
            // ユーザー config が**既定と違う形で** Global キーを奪った時だけ矯正し、
            // 既定に面別バインドがあればそれへ、無ければ Global へ戻す。
            if sfc.allows_tabs() {
                let shadows: Vec<(KeyPress, String, String, Option<Binding>)> = {
                    let cmap = match self.per_surface.get(&sfc) {
                        Some(m) => m,
                        None => continue,
                    };
                    let dmap = defaults.per_surface.get(&sfc);
                    global_keys
                        .iter()
                        .filter_map(|gk| {
                            let local = cmap.get(gk)?;
                            let global = self.global.get(gk);
                            if Some(local) == global {
                                return None;
                            }
                            let default_local = dmap.and_then(|m| m.get(gk));
                            if Some(local) == default_local {
                                return None; // 既定どおりの面別特化
                            }
                            let restore = default_local.cloned();
                            let kept = match &restore {
                                Some(b) => binding_name(b),
                                None => global.map(binding_name).unwrap_or_default(),
                            };
                            Some((*gk, kept, binding_name(local), restore))
                        })
                        .collect()
                };
                for (gk, kept, dropped, restore) in shadows {
                    conflicts.push(KeyConflict {
                        surface: sfc,
                        key: gk,
                        kept,
                        dropped,
                        reason: ConflictKind::GlobalShadow,
                    });
                    let m = self.per_surface.get_mut(&sfc).expect("surface present");
                    match restore {
                        Some(b) => {
                            m.insert(gk, b);
                        }
                        None => {
                            m.remove(&gk);
                        }
                    }
                }
            }
        }

        self.conflicts = conflicts;
    }

    /// surface + (leader pending) + keystroke → resolution (§1.5). One input = 1-2 HashMap lookups.
    pub fn resolve(&self, sfc: Surface, pending: Option<LeaderId>, kp: KeyPress) -> Resolution {
        if let Some(id) = pending {
            return match self.leaders.get(&id).and_then(|m| m.find(kp)) {
                Some(a) => Resolution::Action(a),
                None => Resolution::Unbound,
            };
        }
        if let Some(b) = self.per_surface.get(&sfc).and_then(|m| m.get(&kp)) {
            return binding_to_resolution(b);
        }
        if sfc.allows_tabs() {
            if let Some(b) = self.global.get(&kp) {
                return binding_to_resolution(b);
            }
        }
        Resolution::Unbound
    }
}

fn binding_to_resolution(b: &Binding) -> Resolution {
    match b {
        Binding::Run(a) => Resolution::Action(*a),
        Binding::Leader(id) => Resolution::EnterLeader(*id),
    }
}

/// The target of a config `[keys.<name>]` surface name.
#[derive(Debug, Clone, Copy)]
enum KeyTarget {
    Global,
    Surface(Surface),
}

/// Surface name (snake_case) → target. Unknown names return None (the caller warns). Git names are None when the feature is off.
fn key_target_from_name(name: &str) -> Option<KeyTarget> {
    let s = match name {
        "global" => return Some(KeyTarget::Global),
        "tree" => Surface::Tree,
        "tree_visual" => Surface::Visual,
        "preview_text" => Surface::PreviewText,
        "preview_text_visual" => Surface::PreviewTextVisual,
        "preview_image" => Surface::PreviewImage,
        "preview_table" => Surface::PreviewTable,
        "sort" => Surface::Sort,
        "bookmarks" => Surface::Bookmarks,
        "tabs" => Surface::Tabs,
        "info" => Surface::Info,
        "help" => Surface::Help,
        #[cfg(feature = "git")]
        "preview_git_diff" => Surface::PreviewGitDiff,
        #[cfg(feature = "git")]
        "git_changes" => Surface::GitChanges,
        #[cfg(feature = "git")]
        "git_log" => Surface::GitLog,
        #[cfg(feature = "git")]
        "git_graph" => Surface::GitGraph,
        #[cfg(feature = "git")]
        "git_branches" => Surface::GitBranches,
        #[cfg(feature = "git")]
        "git_detail" => Surface::GitDetail,
        _ => return None,
    };
    Some(KeyTarget::Surface(s))
}

/// Leader prefix key → LeaderId (only Space/y are accepted).
fn leader_for_prefix(kp: KeyPress) -> Option<LeaderId> {
    if kp == KeyPress::ch(' ') {
        Some(LeaderId::File)
    } else if kp == KeyPress::ch('y') {
        Some(LeaderId::Copy)
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
                KeyPress::ch(' '),
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
        _ => Msg::Empty,
    }
}

// =============================================================================
// Action ↔ 設定文字列の双方向対応。
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
        "toggle_info" => Action::ToggleInfo,
        "request_edit" => Action::RequestEdit,
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
        // ファイル管理
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
        "toggle_markdown_raw" => Action::ToggleMarkdownRaw,
        "link_focus_next" => Action::LinkFocusNext,
        "link_focus_prev" => Action::LinkFocusPrev,
        "link_open" => Action::LinkOpen,
        "open_link_new_tab" => Action::OpenLinkNewTab,
        "open_in_new_tab" => Action::OpenInNewTab,
        // Preview: image
        "image_zoom_in" => Action::ImageZoomIn,
        "image_zoom_out" => Action::ImageZoomOut,
        "image_zoom_reset" => Action::ImageZoomReset,
        "pdf_next_page" => Action::PdfNextPage,
        "preview_next_file" => Action::PreviewFileNext,
        "preview_prev_file" => Action::PreviewFilePrev,
        "pdf_prev_page" => Action::PdfPrevPage,
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
        "bookmark_jump" => Action::BookmarkJump,
        "bookmark_edit" => Action::BookmarkEdit,
        "bookmark_delete" => Action::BookmarkDelete,
        "bookmark_close" => Action::BookmarkClose,
        // Info
        "info_close" => Action::InfoClose,
        // Git (feature gate)
        #[cfg(feature = "git")]
        "git_diff_discard" => Action::GitDiffDiscard,
        #[cfg(feature = "git")]
        "cycle_diff_layout" => Action::CycleDiffLayout,
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
        #[cfg(feature = "git")]
        "branch_filter_start" => Action::BranchFilterStart,
        #[cfg(feature = "git")]
        "branch_checkout" => Action::BranchCheckout,
        #[cfg(feature = "git")]
        "branch_create" => Action::BranchCreate,
        #[cfg(feature = "git")]
        "branch_delete" => Action::BranchDelete,
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
        Action::ToggleInfo => "toggle_info",
        Action::RequestEdit => "request_edit",
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
        Action::ToggleMarkdownRaw => "toggle_markdown_raw",
        Action::LinkFocusNext => "link_focus_next",
        Action::LinkFocusPrev => "link_focus_prev",
        Action::LinkOpen => "link_open",
        Action::OpenLinkNewTab => "open_link_new_tab",
        Action::OpenInNewTab => "open_in_new_tab",
        Action::ImageZoomIn => "image_zoom_in",
        Action::ImageZoomOut => "image_zoom_out",
        Action::ImageZoomReset => "image_zoom_reset",
        Action::PdfNextPage => "pdf_next_page",
        Action::PreviewFileNext => "preview_next_file",
        Action::PreviewFilePrev => "preview_prev_file",
        Action::PdfPrevPage => "pdf_prev_page",
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
        Action::BookmarkJump => "bookmark_jump",
        Action::BookmarkEdit => "bookmark_edit",
        Action::BookmarkDelete => "bookmark_delete",
        Action::BookmarkClose => "bookmark_close",
        Action::InfoClose => "info_close",
        #[cfg(feature = "git")]
        Action::GitDiffDiscard => "git_diff_discard",
        #[cfg(feature = "git")]
        Action::CycleDiffLayout => "cycle_diff_layout",
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
        Action::BranchFilterStart => "branch_filter_start",
        #[cfg(feature = "git")]
        Action::BranchCheckout => "branch_checkout",
        #[cfg(feature = "git")]
        Action::BranchCreate => "branch_create",
        #[cfg(feature = "git")]
        Action::BranchDelete => "branch_delete",
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
    }
}

// =============================================================================
// 単体テスト (§10)。
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg_with(surface: &str, entries: &[(&str, &str)]) -> KeysFileConfig {
        let mut table = HashMap::new();
        for (k, v) in entries {
            table.insert((*k).to_string(), (*v).to_string());
        }
        let mut cfg = KeysFileConfig::default();
        cfg.surfaces.insert(surface.to_string(), table);
        cfg
    }

    // §10.1 defaults 網羅。
    #[test]
    fn defaults_tree_core_keys() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('d')),
            Resolution::Action(Action::OpenGitDiffCursor)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('a')),
            Resolution::Action(Action::SetAnchor)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('A')),
            Resolution::Action(Action::ResetAnchor)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('o')),
            Resolution::Action(Action::OpenGitView)
        );
    }

    #[test]
    fn visual_q_quits() {
        // 回帰防止: 旧 Visual モードの q=アプリ終了 を保全する。
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Visual, None, KeyPress::ch('q')),
            Resolution::Action(Action::Quit)
        );
    }

    #[test]
    fn tree_q_resolves_to_close_tab_or_quit() {
        // ツリー最上位の q は「タブを閉じる or 最後なら終了」。Q は従来どおり Quit。
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('q')),
            Resolution::Action(Action::CloseTabOrQuit)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('Q')),
            Resolution::Action(Action::Quit)
        );
    }

    #[test]
    fn paste_jump_p_is_global_and_config_roundtrips() {
        // P はグローバル(allows_tabs 面が継承)＝Tree/Preview どちらからでも解決する。
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [Surface::Tree, Surface::PreviewText, Surface::PreviewImage] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('P')),
                Resolution::Action(Action::PasteJump),
                "P が {sfc:?} で paste_jump に解決する"
            );
        }
        // config 文字列の双方向対応。
        assert_eq!(action_from_str("paste_jump"), Some(Action::PasteJump));
        assert_eq!(action_name(Action::PasteJump), "paste_jump");
    }

    #[test]
    fn preview_file_paging_is_ctrl_n_p_on_preview_surfaces() {
        // Ctrl-n/Ctrl-p=ツリー表示順のファイル送り(text/画像/テーブルの3面)。
        // J/K は画像面の PDF ページ送りと衝突、Ctrl-j はレガシー端末で Enter(LF)と同一のため不採用。
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [
            Surface::PreviewText,
            Surface::PreviewImage,
            Surface::PreviewTable,
        ] {
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
        // config 文字列の双方向対応。
        assert_eq!(
            action_from_str("preview_next_file"),
            Some(Action::PreviewFileNext)
        );
        assert_eq!(action_name(Action::PreviewFilePrev), "preview_prev_file");
    }

    #[test]
    fn open_link_new_tab_is_ctrl_t_in_preview_text() {
        // Ctrl-t=フォーカス中リンクを別タブ(PreviewText 専用)。Ctrl+英字は全端末で確実。
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ctrl_ch('t')),
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
        // Ctrl-t=カーソル下のエントリを別タブで開く(Tree)。preview の OpenLinkNewTab と同じキー/意味。
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ctrl_ch('t')),
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
    fn shift_q_quits_via_global_on_keymap_surfaces() {
        // Q=アプリ全終了。global に置いたので allows_tabs() の面(入力/確認モーダル以外)が継承する。
        let m = KeyMap::defaults(KeyScheme::Vim);
        for sfc in [
            Surface::Tree,
            Surface::PreviewText,
            Surface::PreviewTable,
            Surface::Visual,
        ] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('Q')),
                Resolution::Action(Action::Quit),
                "Q が {sfc:?} で終了に解決する"
            );
        }
    }

    // §10.2 leader。
    #[test]
    fn leader_resolution() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
        assert_eq!(
            m.resolve(Surface::Tree, Some(LeaderId::File), KeyPress::ch('d')),
            Resolution::Action(Action::FileDelete)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        assert_eq!(
            m.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('f')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // 未知 suffix は Unbound (リーダー取消)。
        assert_eq!(
            m.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('z')),
            Resolution::Unbound
        );
    }

    // §10 copy leader の到達範囲 (Preview 系継承・Visual/git 非継承)。
    #[test]
    fn copy_leader_scope() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        assert_eq!(
            m.resolve(Surface::PreviewImage, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        // Visual は copy leader 非継承。
        assert_eq!(
            m.resolve(Surface::Visual, None, KeyPress::ch('y')),
            Resolution::Unbound
        );
    }

    // Preview:table (csv/tsv) の面のキー解決。hjkl=セル移動 / y→=セル/行/列コピー / q=戻る。
    #[test]
    fn table_surface_resolution() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // hjkl = セルカーソル移動。
        assert_eq!(
            m.resolve(Surface::PreviewTable, None, KeyPress::ch('l')),
            Resolution::Action(Action::Navigate(Motion::Right))
        );
        assert_eq!(
            m.resolve(Surface::PreviewTable, None, KeyPress::ch('0')),
            Resolution::Action(Action::Navigate(Motion::LineHome))
        );
        // y→ はテーブル専用の TableCopy リーダー(パスコピーの Copy とは別メニュー)。
        assert_eq!(
            m.resolve(Surface::PreviewTable, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::TableCopy)
        );
        assert_eq!(
            m.resolve(
                Surface::PreviewTable,
                Some(LeaderId::TableCopy),
                KeyPress::ch('c')
            ),
            Resolution::Action(Action::TableCopy(TableCopyKind::Cell))
        );
        assert_eq!(
            m.resolve(
                Surface::PreviewTable,
                Some(LeaderId::TableCopy),
                KeyPress::ch('C')
            ),
            Resolution::Action(Action::TableCopy(TableCopyKind::Column))
        );
        // f はパスコピー(フル)を残してある。
        assert_eq!(
            m.resolve(
                Surface::PreviewTable,
                Some(LeaderId::TableCopy),
                KeyPress::ch('f')
            ),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
        // q = 戻る。
        assert_eq!(
            m.resolve(Surface::PreviewTable, None, KeyPress::ch('q')),
            Resolution::Action(Action::PreviewBack)
        );
    }

    // Preview の行選択(v)モード。v で開始 → j/k 拡張 → y コピー → v/q 取消。
    #[test]
    fn preview_visual_resolution() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // 通常テキストプレビューで v = 選択開始。
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ch('v')),
            Resolution::Action(Action::PreviewEnterVisual)
        );
        // 選択面: j/k = 範囲拡張(カーソル移動)。
        assert_eq!(
            m.resolve(Surface::PreviewTextVisual, None, KeyPress::ch('j')),
            Resolution::Action(Action::Navigate(Motion::Down))
        );
        // y = 選択コピー。
        assert_eq!(
            m.resolve(Surface::PreviewTextVisual, None, KeyPress::ch('y')),
            Resolution::Action(Action::PreviewCopySelection)
        );
        // v / q = 選択解除。
        assert_eq!(
            m.resolve(Surface::PreviewTextVisual, None, KeyPress::ch('v')),
            Resolution::Action(Action::PreviewExitVisual)
        );
        assert_eq!(
            m.resolve(Surface::PreviewTextVisual, None, KeyPress::ch('q')),
            Resolution::Action(Action::PreviewExitVisual)
        );
    }

    // §10.3 chord parse。
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
        // 大文字は SHIFT 込みとして保持。
        assert_eq!(
            KeyChord::parse("G").unwrap(),
            KeyChord::Single(KeyPress::ch('G'))
        );
        assert!(KeyChord::parse("").is_err());
        assert!(KeyChord::parse("a b c").is_err());
        assert!(KeyChord::parse("notakey").is_err());
    }

    // §10.4 action_from_str ラウンドトリップ + navigate。
    #[test]
    fn action_roundtrip() {
        // git 無効時は push が無く mut が不要になるため抑止する。
        #[allow(unused_mut)]
        let mut samples = vec![
            Action::Noop,
            Action::Navigate(Motion::PageDown),
            Action::Navigate(Motion::LineHome),
            Action::TabNew,
            Action::ToggleHelp,
            Action::CopyPath(CopyKind::Full),
            Action::CopyPath(CopyKind::Parent),
            Action::Quit,
            Action::Refresh,
            Action::SetAnchor,
            Action::ResetAnchor,
            Action::OpenGitDiffCursor,
            Action::OpenGitView,
            Action::FileDelete,
            Action::SortSet(SortKey::Size),
            Action::SortToggleReverse,
            Action::InfoClose,
        ];
        #[cfg(feature = "git")]
        {
            samples.push(Action::GitStage);
            samples.push(Action::GitOpenGraph);
            samples.push(Action::CycleDiffLayout);
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
        // tab_goto は数字固定 → None。
        assert_eq!(action_from_str("tab_goto"), None);
    }

    // §10.5 config merge: override / add / noop / 匿名リーダー禁止。
    #[test]
    fn config_merge_override_add_noop() {
        let cfg = cfg_with(
            "tree",
            &[
                ("d", "refresh"),          // override
                ("X", "refresh"),          // add (新キー)
                ("i", "noop"),             // 無効化 (既定 i=ToggleInfo)
                ("g s", "open_sort_menu"), // 匿名 g リーダー → 警告で無視
            ],
        );
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('d')),
            Resolution::Action(Action::Refresh)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('X')),
            Resolution::Action(Action::Refresh)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('i')),
            Resolution::Unbound
        );
        // g は既定の Nav(Top) のまま (匿名リーダー化を拒否)。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('g')),
            Resolution::Action(Action::Navigate(Motion::Top))
        );
        assert!(
            m.warnings.iter().any(|w| w.contains("chord prefix")),
            "expected anonymous-prefix warning, got {:?}",
            m.warnings
        );
    }

    // §10.5 config: 既存 suffix を別キーへ移す (リーダー上書き)。
    #[test]
    fn config_leader_suffix_override() {
        let cfg = cfg_with("tree", &[("y z", "copy_full")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('z')),
            Resolution::Action(Action::CopyPath(CopyKind::Full))
        );
    }

    // §10.6 validate PrefixVsSingle。
    #[test]
    fn validate_prefix_vs_single() {
        let cfg = cfg_with("tree", &[("space", "quit")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        // Space はリーダーのまま (override 破棄)。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch(' ')),
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

    // §10.7 validate GlobalShadow。
    #[test]
    fn validate_global_shadow() {
        let cfg = cfg_with("tree", &[("t", "refresh")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        // t は Global の TabNew に戻る (per-surface override 破棄)。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('t')),
            Resolution::Action(Action::TabNew)
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
        // 既定 config に衝突が無いこと(既定マップ自身の面別特化があっても合法、の恒久検査)。
        let m = KeyMap::from_config(KeyScheme::Vim, &crate::keymap::KeysFileConfig::default());
        assert!(
            m.conflicts.is_empty(),
            "既定 config に衝突なし: {:?}",
            m.conflicts
        );
        // ユーザーが global キー(T)を面で別 Action に奪う → 検知し、既定に面別特化が無いので
        // global の割当へ戻す。(既定特化への復元パスは、既定に面別特化が存在する時に効く
        // 将来向けの規則 — 現状の既定には該当キーが無い。)
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
            m.resolve(Surface::Tabs, None, KeyPress::ch('T')),
            Resolution::Action(Action::ToggleTabList),
            "global の割当へ戻る"
        );
    }

    // §10.8 固定キー rebind 拒否。
    #[test]
    fn fixed_key_rebind_rejected() {
        let cfg = cfg_with("tree", &[("enter", "quit")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::key(KeyCode::Enter)),
            Resolution::Unbound
        );
        assert!(m.warnings.iter().any(|w| w.contains("fixed key")));
    }

    #[test]
    fn bookmark_list_defaults_reserve_plain_letters_for_jump() {
        // 一覧内の素の英字はブックマーク名ジャンプに予約: 編集/削除は Ctrl 修飾・`'`/q で閉じる。
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Bookmarks, None, KeyPress::ctrl_ch('e')),
            Resolution::Action(Action::BookmarkEdit)
        );
        assert_eq!(
            m.resolve(Surface::Bookmarks, None, KeyPress::ctrl_ch('d')),
            Resolution::Action(Action::BookmarkDelete)
        );
        assert_eq!(
            m.resolve(Surface::Bookmarks, None, KeyPress::ch('\'')),
            Resolution::Action(Action::BookmarkClose)
        );
        assert_eq!(
            m.resolve(Surface::Bookmarks, None, KeyPress::ch('q')),
            Resolution::Action(Action::BookmarkClose)
        );
        // 素の e/d は未割当(=main の Unbound フォールバックで英字ジャンプに落ちる)。
        assert_eq!(
            m.resolve(Surface::Bookmarks, None, KeyPress::ch('e')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(Surface::Bookmarks, None, KeyPress::ch('d')),
            Resolution::Unbound
        );
        // プレビュー3面でも m=登録 / '=一覧(表示中ファイルのブックマーク)。
        for sfc in [
            Surface::PreviewText,
            Surface::PreviewImage,
            Surface::PreviewTable,
        ] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('m')),
                Resolution::Action(Action::MarkSet)
            );
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('\'')),
                Resolution::Action(Action::MarkJump)
            );
        }
        // Tree の `'` は一覧を開く MarkJump のまま(config 名も不変)。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('\'')),
            Resolution::Action(Action::MarkJump)
        );
    }

    #[test]
    fn tab_list_defaults_resolve() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // global T で開閉(全面から)・一覧内 w=選択タブを閉じる(global の TabClose を面で上書き)。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('T')),
            Resolution::Action(Action::ToggleTabList)
        );
        assert_eq!(
            m.resolve(Surface::Tabs, None, KeyPress::ch('T')),
            Resolution::Action(Action::ToggleTabList),
            "一覧内の T は global 継承で閉じる"
        );
        assert_eq!(
            m.resolve(Surface::Tabs, None, KeyPress::ch('d')),
            Resolution::Action(Action::TabListClose)
        );
        assert_eq!(
            m.resolve(Surface::Tabs, None, KeyPress::ch('q')),
            Resolution::Action(Action::ToggleTabList)
        );
        // 既定の `w` は**どこにも**タブを閉じる割当を持たない(誤爆防止・q に一本化)。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('w')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(Surface::Tabs, None, KeyPress::ch('w')),
            Resolution::Unbound
        );
        // config 文字列の往復。
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

    // §10.9 less プロファイル。
    #[test]
    fn less_scheme_preview_paging() {
        let m = KeyMap::defaults(KeyScheme::Less);
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ch(' ')),
            Resolution::Action(Action::Navigate(Motion::PageDown))
        );
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ch('d')),
            Resolution::Action(Action::Navigate(Motion::HalfDown))
        );
        // Tree は scheme 不問で Space=File リーダーのまま。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
    }

    // vim プロファイル (既定) の Ctrl ページ送り。
    #[test]
    fn vim_scheme_preview_paging() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ctrl_ch('f')),
            Resolution::Action(Action::Navigate(Motion::PageDown))
        );
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ctrl_ch('d')),
            Resolution::Action(Action::Navigate(Motion::HalfDown))
        );
        // vim では素の f/Space は Preview で未割当。
        assert_eq!(
            m.resolve(Surface::PreviewText, None, KeyPress::ch('f')),
            Resolution::Unbound
        );
    }

    // §10.10 scheme 非該当面は不変 (Sort)。
    #[test]
    fn scheme_does_not_affect_sort() {
        let vim = KeyMap::defaults(KeyScheme::Vim);
        let less = KeyMap::defaults(KeyScheme::Less);
        for kp in [KeyPress::ch('n'), KeyPress::ch('s'), KeyPress::ch('r')] {
            assert_eq!(
                vim.resolve(Surface::Sort, None, kp),
                less.resolve(Surface::Sort, None, kp)
            );
        }
    }

    // §10.11 Tree は水平移動 (Left/Right/0/$) を持たない。
    #[test]
    fn tree_has_no_horizontal_motion() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('h')),
            Resolution::Action(Action::TreeLeave)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('0')),
            Resolution::Unbound
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('$')),
            Resolution::Unbound
        );
    }

    // Global タブ/ヘルプの継承とテキスト入力面での非継承。
    #[test]
    fn global_tab_inheritance() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('t')),
            Resolution::Action(Action::TabNew)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('1')),
            Resolution::Action(Action::TabGoto(0))
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('9')),
            Resolution::Action(Action::TabGoto(8))
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('?')),
            Resolution::Action(Action::ToggleHelp)
        );
        // テキスト入力面 (Filter) は Global を継承しない。
        assert_eq!(
            m.resolve(Surface::Filter, None, KeyPress::ch('t')),
            Resolution::Unbound
        );
    }

    // Surface 述語。
    #[test]
    fn surface_predicates() {
        assert!(Surface::Filter.is_text_input());
        assert!(!Surface::Tree.is_text_input());
        assert!(Surface::DialogConfirmDelete.is_modal_confirm());
        assert!(!Surface::Sort.is_modal_confirm());
        assert!(Surface::Tree.allows_tabs());
        assert!(!Surface::Filter.allows_tabs());
        assert!(!Surface::DialogConfirmDelete.allows_tabs());
        assert!(Surface::Tree.inherits_copy_leader());
        assert!(!Surface::Visual.inherits_copy_leader());
    }

    // 不明な面名は警告して無視 (他面は壊さない)。
    #[test]
    fn unknown_surface_warns() {
        let cfg = cfg_with("nonsense", &[("d", "refresh")]);
        let m = KeyMap::from_config(KeyScheme::Vim, &cfg);
        assert!(m.warnings.iter().any(|w| w.contains("unknown key surface")));
        // 既定の Tree d は維持。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('d')),
            Resolution::Action(Action::OpenGitDiffCursor)
        );
    }

    // §10.10/§10.12 Git 系: scheme 非該当面の不変 + git_detail の Motion 網羅。
    #[cfg(feature = "git")]
    #[test]
    fn git_changes_scheme_invariant() {
        let vim = KeyMap::defaults(KeyScheme::Vim);
        let less = KeyMap::defaults(KeyScheme::Less);
        for kp in [KeyPress::ch('s'), KeyPress::ch('j'), KeyPress::ch('d')] {
            assert_eq!(
                vim.resolve(Surface::GitChanges, None, kp),
                less.resolve(Surface::GitChanges, None, kp)
            );
        }
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_detail_navigate_coverage() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::GitDetail, None, KeyPress::ch('h')),
            Resolution::Action(Action::Navigate(Motion::Left))
        );
        assert_eq!(
            m.resolve(Surface::GitDetail, None, KeyPress::ch('l')),
            Resolution::Action(Action::Navigate(Motion::Right))
        );
        assert_eq!(
            m.resolve(Surface::GitDetail, None, KeyPress::ch('0')),
            Resolution::Action(Action::Navigate(Motion::LineHome))
        );
        assert_eq!(
            m.resolve(Surface::GitDetail, None, KeyPress::ctrl_ch('f')),
            Resolution::Action(Action::Navigate(Motion::PageDown))
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_surfaces_copy_bindings() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        // 変更ハブ: y→ パスコピーリーダー(変更ファイルのパス)。
        assert_eq!(
            m.resolve(Surface::GitChanges, None, KeyPress::ch('y')),
            Resolution::EnterLeader(LeaderId::Copy)
        );
        // log/graph/detail: y→ コミット情報コピーリーダー。
        for sfc in [Surface::GitLog, Surface::GitGraph, Surface::GitDetail] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('y')),
                Resolution::EnterLeader(LeaderId::GitCopy),
                "{sfc:?} の y は GitCopy リーダー"
            );
        }
        // GitCopy リーダー配下: m→ 全文メッセージ, h→ 完全ハッシュ。
        assert_eq!(
            m.resolve(Surface::GitLog, Some(LeaderId::GitCopy), KeyPress::ch('m')),
            Resolution::Action(Action::GitCopy(GitCopyKind::Message))
        );
        assert_eq!(
            m.resolve(
                Surface::GitGraph,
                Some(LeaderId::GitCopy),
                KeyPress::ch('h')
            ),
            Resolution::Action(Action::GitCopy(GitCopyKind::FullHash))
        );
        // branches: y→ ブランチ名コピー(直接)。
        assert_eq!(
            m.resolve(Surface::GitBranches, None, KeyPress::ch('y')),
            Resolution::Action(Action::CopyBranchName)
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_changes_mnemonics() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::GitChanges, None, KeyPress::ch('d')),
            Resolution::Action(Action::GitWorktreeDiff)
        );
        assert_eq!(
            m.resolve(Surface::GitChanges, None, KeyPress::ch('l')),
            Resolution::Action(Action::GitOpenLog)
        );
        assert_eq!(
            m.resolve(Surface::GitChanges, None, KeyPress::ch('g')),
            Resolution::Action(Action::GitOpenGraph)
        );
        assert_eq!(
            m.resolve(Surface::GitChanges, None, KeyPress::ch('!')),
            Resolution::Action(Action::GitLaunchTool)
        );
    }

    #[test]
    fn parse_code_named_keys_and_invalid() {
        // 名前つきキー・別名・大小無視を網羅。
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
        // 単文字は元の大小を保持。
        assert_eq!(KeyPress::parse_code("a").unwrap(), KeyCode::Char('a'));
        assert_eq!(KeyPress::parse_code("Z").unwrap(), KeyCode::Char('Z'));
        assert_eq!(KeyPress::parse_code("$").unwrap(), KeyCode::Char('$'));
        // 複数文字の未知トークンは Err。
        assert!(KeyPress::parse_code("abc").is_err(), "未知の複数文字は Err");
        assert!(KeyPress::parse_code("nope").is_err());
    }

    #[test]
    fn leader_menu_set_find_and_remove() {
        // LeaderMenu の set/find/remove を直接検証(リーダーメニュー編集の核)。
        let mut menu = LeaderMenu {
            id: LeaderId::File,
            title: Msg::StFilter,
            items: Vec::new(),
        };
        menu.set(KeyPress::ch('a'), Action::ToggleHelp);
        menu.set(KeyPress::ch('b'), Action::ToggleInfo);
        assert_eq!(menu.items.len(), 2);
        assert_eq!(menu.find(KeyPress::ch('a')), Some(Action::ToggleHelp));
        // 同じキーへの set は置き換え(重複しない)。
        menu.set(KeyPress::ch('a'), Action::Refresh);
        assert_eq!(menu.items.len(), 2, "同一キーは置換");
        assert_eq!(menu.find(KeyPress::ch('a')), Some(Action::Refresh));
        // remove で1件減り、見つからなくなる。
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
        // 存在しないキーの remove は no-op。
        menu.remove(KeyPress::ch('z'));
        assert_eq!(menu.items.len(), 1);
    }

    // Agent Watch: C=変更フィルタ / n・N=変更間ジャンプ / F=フォロー(global) / y@=@参照 / Y=@path#L。
    #[test]
    fn agent_watch_keys_resolve() {
        let m = KeyMap::defaults(KeyScheme::Vim);
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('C')),
            Resolution::Action(Action::ToggleChangedFilter)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('n')),
            Resolution::Action(Action::JumpNextChange)
        );
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch('N')),
            Resolution::Action(Action::JumpPrevChange)
        );
        // F は global 継承(Tree/Preview どちらからでも)。
        for sfc in [Surface::Tree, Surface::PreviewText, Surface::PreviewImage] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('F')),
                Resolution::Action(Action::ToggleFollow),
                "F が {sfc:?} でフォロー切替に解決する"
            );
        }
        // y→@ = @参照パスコピー(全 y リーダー面で共通メニュー)。
        assert_eq!(
            m.resolve(Surface::Tree, Some(LeaderId::Copy), KeyPress::ch('@')),
            Resolution::Action(Action::CopyPath(CopyKind::AtRef))
        );
        // y→c = コードブロックコピー(Copy リーダーのメニュー項目。表示は面/フォーカスで出し分け)。
        assert_eq!(
            m.resolve(
                Surface::PreviewText,
                Some(LeaderId::Copy),
                KeyPress::ch('c')
            ),
            Resolution::Action(Action::CopyCodeBlock)
        );
        // Y = 選択/キャレットの @path#L 参照(通常/visual 両面)。
        for sfc in [Surface::PreviewText, Surface::PreviewTextVisual] {
            assert_eq!(
                m.resolve(sfc, None, KeyPress::ch('Y')),
                Resolution::Action(Action::PreviewCopySelectionRef),
                "Y が {sfc:?} で @参照コピーに解決する"
            );
        }
        // diff ビュー内の n/N=変更ファイル回遊(ツリーの n/N と同じ Action に解決)。
        #[cfg(feature = "git")]
        for (k, a) in [('n', Action::JumpNextChange), ('N', Action::JumpPrevChange)] {
            assert_eq!(
                m.resolve(Surface::PreviewGitDiff, None, KeyPress::ch(k)),
                Resolution::Action(a),
                "diff ビューの {k} が変更間ジャンプに解決する"
            );
        }
        // 設定文字列の往復。
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
        // Space→ でファイル管理リーダーへ入る。
        assert_eq!(
            m.resolve(Surface::Tree, None, KeyPress::ch(' ')),
            Resolution::EnterLeader(LeaderId::File)
        );
        // Space→D = 複製(Tree と Visual は同じ File リーダーを共有)。
        for sfc in [Surface::Tree, Surface::Visual] {
            assert_eq!(
                m.resolve(sfc, Some(LeaderId::File), KeyPress::ch('D')),
                Resolution::Action(Action::FileDuplicate),
                "{sfc:?} の Space→D が複製に解決する"
            );
        }
        // 設定文字列の往復。
        let a = action_from_str("file_duplicate").expect("file_duplicate は既知アクション");
        assert_eq!(action_name(a), "file_duplicate");
        assert_eq!(a, Action::FileDuplicate);
    }
}
