//! The key layers every host shares: konoma and the plugin SDK's standalone app.
//!
//! The key design (`docs/FEATURE-PLUGIN-ARCHITECTURE.md` §10-3) puts keys in layers: layer 0
//! (global), layer 1 (the basics every view with a cursor shares), layer 2 (per view type) and
//! layer 3 (the verbs `e` / `o`). Layers 0 to 3 mean the same thing in every host, so they are
//! declared once, here, in terms of [`CoreAction`]. konoma builds those layers of its own keymap
//! from these tables (its `Action` has an `impl From<CoreAction>`); the standalone app resolves
//! its keys with [`CoreKeys::resolve`]. What stays in the host: the tree's keys, the version
//! control screens, layer 4 (plugin keys) and everything a host-only action does.
//!
//! The overrides of `[keys]` ([`KeysFileConfig`]) are applied by [`CoreKeys::from_config`] with
//! the rules konoma's keymap uses, restricted to the contexts and actions defined here. The one
//! difference: an action name this module does not know (a host-only action, or a typo) is
//! ignored, so the key keeps its default. konoma's own keymap warns about unknown names.

use std::collections::HashMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::help_msg::CoreMsg;
use crate::keys::{KeysConfig, KeysFileConfig};

// =============================================================================
// Payload types of the actions.
// =============================================================================

/// Abstract amount of cursor movement / scrolling, mapped to concrete behavior per view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl Motion {
    /// Every motion, for tests and tables.
    pub const ALL: [Motion; 12] = [
        Motion::Up,
        Motion::Down,
        Motion::Top,
        Motion::Bottom,
        Motion::PageUp,
        Motion::PageDown,
        Motion::HalfUp,
        Motion::HalfDown,
        Motion::Left,
        Motion::Right,
        Motion::LineHome,
        Motion::LineEnd,
    ];

    /// The name used after `navigate:` in a config string.
    pub fn name(self) -> &'static str {
        match self {
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

    /// The reverse of [`Motion::name`].
    pub fn from_name(s: &str) -> Option<Motion> {
        Motion::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// Path-copy kind (FR-6). Chosen by the key after `y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CopyKind {
    /// File name only.
    Name,
    /// Full path.
    Full,
    /// Path relative to where it was opened.
    Relative,
    /// Full path of the parent directory.
    Parent,
    /// `@relative/path` reference for AI agents. Claude Code reads `@path` as file context, so
    /// the path is strictly relative to the launch/anchor dir where the agent usually runs,
    /// **without** the leading directory name that `Relative` prepends for display.
    AtRef,
}

/// CSV/TSV table copy kind (chosen after `y` in the table preview).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableCopyKind {
    /// The current cell's value.
    Cell,
    /// The current row's cells joined by the delimiter (`,` / tab).
    Row,
    /// The current column's header plus every cell value, one per line.
    Column,
}

/// Tree sort key. Switch via the `,` menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Ext,
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

// =============================================================================
// KeyPress.
// =============================================================================

/// Why a key token of a config string was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParseError(String);

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KeyParseError {}

/// Normalized representation of a single keystroke. SHIFT is folded into the uppercase char and
/// ALT is unused, so it holds only CONTROL.
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

    /// Parse one config-string token. Handles the `ctrl-x`/`c-x` modifier, `space`, literals
    /// `0 $ ! …`, named keys (`tab enter up …`), and single characters (uppercase = SHIFT included).
    pub fn parse(s: &str) -> Result<KeyPress, KeyParseError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(KeyParseError("empty key token".into()));
        }
        let (ctrl, rest) = if let Some(r) = s.strip_prefix("ctrl-") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("c-") {
            (true, r)
        } else {
            (false, s)
        };
        if rest.is_empty() {
            return Err(KeyParseError(format!("missing key after modifier: {s}")));
        }
        let code = Self::parse_code(rest)?;
        Ok(KeyPress { code, ctrl })
    }

    /// Parse the key part of a token (no modifier).
    pub fn parse_code(s: &str) -> Result<KeyCode, KeyParseError> {
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
                    .ok_or_else(|| KeyParseError("empty key token".into()))?;
                if chars.next().is_some() {
                    return Err(KeyParseError(format!("unknown key token: {s}")));
                }
                KeyCode::Char(c)
            }
        };
        Ok(code)
    }
}

/// Fixed keys that cannot be rebound. A config rebind of these is ignored. Character keys are
/// not fixed.
pub fn is_fixed_key(kp: KeyPress) -> bool {
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

/// Paging profile. Corresponds to "vim"/"less" of `ui.keys`.
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

// =============================================================================
// CoreAction and its names.
// =============================================================================

/// The commands of layers 0 to 3 (and the payloads of the menus they open). Names are the
/// config strings of `[keys]`; a host's own action type converts from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreAction {
    /// Move / scroll (interpreted per view).
    Navigate(Motion),
    // --- Layer 0 ---
    Quit,
    ToggleFollow,
    /// `t`: a new tab right of the active one.
    TabNewAfter,
    ToggleTabList,
    PasteJump,
    TabPrev,
    TabNext,
    /// 1..=9 -> zero-based index. Not configurable (fixed to digit keys).
    TabGoto(u8),
    ToggleHelp,
    // --- Layer 1 ---
    /// `q`: go back one step (close the preview; a standalone app quits).
    PreviewBack,
    // --- Layer 2 ---
    ImageZoomIn,
    ImageZoomOut,
    ImageZoomReset,
    /// Next / previous page of the view (a PDF page, a sheet, a slide).
    PageNext,
    PagePrev,
    /// The list of the pages (or the headings of a document).
    ToggleOutline,
    // --- Layer 3 ---
    /// `e`: edit the file in the editor.
    RequestEdit,
    /// `o`: open the file in another app.
    OpenExternal,
    // --- Payloads of the menus (no default key of their own) ---
    CopyPath(CopyKind),
    TableCopy(TableCopyKind),
    SortSet(SortKey),
    /// `y c`: copy the source of the focused code block (a no-op where none is focused).
    CopyCodeBlock,
    /// Cycle how the path is shown (relative / `~` / absolute).
    CyclePathStyle,
    /// Toggle line wrapping of the text and document views.
    ToggleWrap,
}

impl CoreAction {
    /// Every action, each payload once: for tests that walk the whole set.
    pub fn all() -> Vec<CoreAction> {
        let mut v: Vec<CoreAction> = Motion::ALL.into_iter().map(CoreAction::Navigate).collect();
        v.extend([
            CoreAction::Quit,
            CoreAction::ToggleFollow,
            CoreAction::TabNewAfter,
            CoreAction::ToggleTabList,
            CoreAction::PasteJump,
            CoreAction::TabPrev,
            CoreAction::TabNext,
            CoreAction::ToggleHelp,
            CoreAction::PreviewBack,
            CoreAction::ImageZoomIn,
            CoreAction::ImageZoomOut,
            CoreAction::ImageZoomReset,
            CoreAction::PageNext,
            CoreAction::PagePrev,
            CoreAction::ToggleOutline,
            CoreAction::RequestEdit,
            CoreAction::OpenExternal,
            CoreAction::CopyCodeBlock,
            CoreAction::CyclePathStyle,
            CoreAction::ToggleWrap,
        ]);
        v.extend((0..9).map(CoreAction::TabGoto));
        for k in [
            CopyKind::Name,
            CopyKind::Full,
            CopyKind::Relative,
            CopyKind::Parent,
            CopyKind::AtRef,
        ] {
            v.push(CoreAction::CopyPath(k));
        }
        for k in [
            TableCopyKind::Cell,
            TableCopyKind::Row,
            TableCopyKind::Column,
        ] {
            v.push(CoreAction::TableCopy(k));
        }
        for k in [
            SortKey::Name,
            SortKey::Size,
            SortKey::Modified,
            SortKey::Ext,
        ] {
            v.push(CoreAction::SortSet(k));
        }
        v
    }

    /// The canonical config string. For `TabGoto` it is `tab_goto` for every index (it is fixed
    /// to the digit keys and cannot be written in a config).
    pub fn name(self) -> String {
        let s: &str = match self {
            CoreAction::Navigate(m) => return format!("navigate:{}", m.name()),
            CoreAction::Quit => "quit",
            CoreAction::ToggleFollow => "toggle_follow",
            CoreAction::TabNewAfter => "tab_new_after",
            CoreAction::ToggleTabList => "toggle_tab_list",
            CoreAction::PasteJump => "paste_jump",
            CoreAction::TabPrev => "tab_prev",
            CoreAction::TabNext => "tab_next",
            CoreAction::TabGoto(_) => "tab_goto",
            CoreAction::ToggleHelp => "toggle_help",
            CoreAction::PreviewBack => "preview_back",
            CoreAction::ImageZoomIn => "image_zoom_in",
            CoreAction::ImageZoomOut => "image_zoom_out",
            CoreAction::ImageZoomReset => "image_zoom_reset",
            CoreAction::PageNext => "page_next",
            CoreAction::PagePrev => "page_prev",
            CoreAction::ToggleOutline => "toggle_outline",
            CoreAction::RequestEdit => "request_edit",
            CoreAction::OpenExternal => "open_external",
            CoreAction::CopyPath(CopyKind::Name) => "copy_name",
            CoreAction::CopyPath(CopyKind::Relative) => "copy_relative",
            CoreAction::CopyPath(CopyKind::Full) => "copy_full",
            CoreAction::CopyPath(CopyKind::Parent) => "copy_parent",
            CoreAction::CopyPath(CopyKind::AtRef) => "copy_at_ref",
            CoreAction::TableCopy(TableCopyKind::Cell) => "table_copy_cell",
            CoreAction::TableCopy(TableCopyKind::Row) => "table_copy_row",
            CoreAction::TableCopy(TableCopyKind::Column) => "table_copy_column",
            CoreAction::SortSet(SortKey::Name) => "sort_name",
            CoreAction::SortSet(SortKey::Size) => "sort_size",
            CoreAction::SortSet(SortKey::Modified) => "sort_modified",
            CoreAction::SortSet(SortKey::Ext) => "sort_ext",
            CoreAction::CopyCodeBlock => "copy_code_block",
            CoreAction::CyclePathStyle => "cycle_path_style",
            CoreAction::ToggleWrap => "toggle_wrap",
        };
        s.to_string()
    }

    /// config string -> action (`None` = not a core action; `tab_goto` is `None` too, it is fixed
    /// to the digit keys). Accepts the old names that were merged into `page_next` / `page_prev`.
    pub fn from_name(s: &str) -> Option<CoreAction> {
        if let Some(m) = s.strip_prefix("navigate:") {
            return Motion::from_name(m).map(CoreAction::Navigate);
        }
        Some(match s {
            "quit" => CoreAction::Quit,
            "toggle_follow" => CoreAction::ToggleFollow,
            "tab_new_after" => CoreAction::TabNewAfter,
            "toggle_tab_list" => CoreAction::ToggleTabList,
            "paste_jump" => CoreAction::PasteJump,
            "tab_prev" => CoreAction::TabPrev,
            "tab_next" => CoreAction::TabNext,
            "toggle_help" => CoreAction::ToggleHelp,
            "preview_back" => CoreAction::PreviewBack,
            "image_zoom_in" => CoreAction::ImageZoomIn,
            "image_zoom_out" => CoreAction::ImageZoomOut,
            "image_zoom_reset" => CoreAction::ImageZoomReset,
            // `page_next` / `page_prev`, plus the names they replaced (kept as aliases).
            "page_next" | "pdf_next_page" | "sheet_next" | "media_diff_page_next" => {
                CoreAction::PageNext
            }
            "page_prev" | "pdf_prev_page" | "sheet_prev" | "media_diff_page_prev" => {
                CoreAction::PagePrev
            }
            "toggle_outline" => CoreAction::ToggleOutline,
            "request_edit" => CoreAction::RequestEdit,
            "open_external" => CoreAction::OpenExternal,
            "copy_name" => CoreAction::CopyPath(CopyKind::Name),
            "copy_relative" => CoreAction::CopyPath(CopyKind::Relative),
            "copy_full" => CoreAction::CopyPath(CopyKind::Full),
            "copy_parent" => CoreAction::CopyPath(CopyKind::Parent),
            "copy_at_ref" => CoreAction::CopyPath(CopyKind::AtRef),
            "table_copy_cell" => CoreAction::TableCopy(TableCopyKind::Cell),
            "table_copy_row" => CoreAction::TableCopy(TableCopyKind::Row),
            "table_copy_column" => CoreAction::TableCopy(TableCopyKind::Column),
            "sort_name" => CoreAction::SortSet(SortKey::Name),
            "sort_size" => CoreAction::SortSet(SortKey::Size),
            "sort_modified" => CoreAction::SortSet(SortKey::Modified),
            "sort_ext" => CoreAction::SortSet(SortKey::Ext),
            "copy_code_block" => CoreAction::CopyCodeBlock,
            "cycle_path_style" => CoreAction::CyclePathStyle,
            "toggle_wrap" => CoreAction::ToggleWrap,
            _ => return None,
        })
    }
}

// =============================================================================
// HostOp: the operations only the full app runs.
// =============================================================================

/// An operation a context's keys can name but that only the full app (konoma) runs: it needs the
/// tree, the project's screens, the marks or the search state, none of which the standalone app
/// has. The standalone app never runs one; it still knows their names and default keys, so that
/// its `Space` menu is built over the same candidates as konoma's (a letter is given to an
/// operation before the ones that do not act are hidden), and `[keys.<context>]` reads them with
/// the same rules.
///
/// Kept apart from [`CoreAction`] on purpose: a `CoreAction` is something a standalone app must be
/// able to carry out (it matches on every one), and konoma's `Action::core()` answers `None` for
/// everything outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostOp {
    /// `/`: start a search.
    SearchStart,
    /// `n` / `N`: the next / previous match.
    SearchNext,
    SearchPrev,
    /// `v` / `V`: a charwise / linewise selection.
    PreviewEnterVisual,
    PreviewEnterVisualLine,
    /// `Y`: copy an `@path#L12-34` reference.
    PreviewCopySelectionRef,
    /// `m` / `'`: set a bookmark / open the list.
    MarkSet,
    MarkJump,
    /// `r`: reload the preview.
    Refresh,
    /// `p`: the project's screen.
    OpenGitView,
    /// `!`: the project's tool (needs konoma's `git` feature; konoma leaves the key unbound
    /// without it).
    GitLaunchTool,
    /// `R`: the next representation of the preview.
    CycleRepresentation,
    /// `R` over a picture that is a diff's own representation: back to the diff.
    ImageReturnToDiff,
    /// `Ctrl-n` / `Ctrl-p`: the next / previous file in the tree's order.
    PreviewFileNext,
    PreviewFilePrev,
    /// `Ctrl-t`: open the focused link in a new tab.
    OpenLinkNewTab,
    /// Tab / BackTab (fixed keys): the next / previous focusable item.
    LinkFocusNext,
    LinkFocusPrev,
    /// Enter / Space on a focused item (a fixed key): open or fold it.
    ToggleFocusedItem,
}

impl HostOp {
    /// Every operation, once: for tests that walk the whole set.
    pub const ALL: [HostOp; 19] = [
        HostOp::SearchStart,
        HostOp::SearchNext,
        HostOp::SearchPrev,
        HostOp::PreviewEnterVisual,
        HostOp::PreviewEnterVisualLine,
        HostOp::PreviewCopySelectionRef,
        HostOp::MarkSet,
        HostOp::MarkJump,
        HostOp::Refresh,
        HostOp::OpenGitView,
        HostOp::GitLaunchTool,
        HostOp::CycleRepresentation,
        HostOp::ImageReturnToDiff,
        HostOp::PreviewFileNext,
        HostOp::PreviewFilePrev,
        HostOp::OpenLinkNewTab,
        HostOp::LinkFocusNext,
        HostOp::LinkFocusPrev,
        HostOp::ToggleFocusedItem,
    ];

    /// The canonical config string (the name `[keys]` and konoma's own keymap use).
    pub fn name(self) -> &'static str {
        match self {
            HostOp::SearchStart => "search_start",
            HostOp::SearchNext => "search_next",
            HostOp::SearchPrev => "search_prev",
            HostOp::PreviewEnterVisual => "preview_enter_visual",
            HostOp::PreviewEnterVisualLine => "preview_enter_visual_line",
            HostOp::PreviewCopySelectionRef => "preview_copy_selection_ref",
            HostOp::MarkSet => "mark_set",
            HostOp::MarkJump => "mark_jump",
            HostOp::Refresh => "refresh",
            HostOp::OpenGitView => "open_git_view",
            HostOp::GitLaunchTool => "git_launch_tool",
            HostOp::CycleRepresentation => "cycle_representation",
            HostOp::ImageReturnToDiff => "image_return_to_diff",
            HostOp::PreviewFileNext => "preview_next_file",
            HostOp::PreviewFilePrev => "preview_prev_file",
            HostOp::OpenLinkNewTab => "open_link_new_tab",
            HostOp::LinkFocusNext => "link_focus_next",
            HostOp::LinkFocusPrev => "link_focus_prev",
            HostOp::ToggleFocusedItem => "toggle_focused_item",
        }
    }

    /// The inverse of [`HostOp::name`]. The old name `toggle_markdown_raw` is an alias of
    /// `cycle_representation`.
    pub fn from_name(s: &str) -> Option<HostOp> {
        if s == "toggle_markdown_raw" {
            return Some(HostOp::CycleRepresentation);
        }
        HostOp::ALL.into_iter().find(|o| o.name() == s)
    }
}

/// The letter that says what an operation is, tried by the `Space` menu before the free letters
/// (`menu::assign`). Keyed by the config name, so one table serves konoma's `Action` (which also
/// has operations of screens a standalone app does not have) and the standalone app's
/// [`CoreAction`]; an operation konoma has only under its `git` feature is simply never asked
/// for without it.
pub fn mnemonic_of_name(name: &str) -> Option<char> {
    Some(match name {
        "bookmark_delete" => 'd',
        "bookmark_edit" => 'e',
        "cycle_path_style" => 'p',
        "toggle_wrap" => 'w',
        "toggle_focused_item" => 't',
        "toggle_follow_diff_scope" => 'f',
        "git_open_log" => 'l',
        "git_open_graph" => 'g',
        _ => return None,
    })
}

/// The names [`mnemonic_of_name`] knows, for the test that each one names a real operation.
pub const MNEMONIC_NAMES: [&str; 8] = [
    "bookmark_delete",
    "bookmark_edit",
    "cycle_path_style",
    "toggle_wrap",
    "toggle_focused_item",
    "toggle_follow_diff_scope",
    "git_open_log",
    "git_open_graph",
];

// =============================================================================
// Bindings, contexts, tables.
// =============================================================================

/// The menus a key can open (a host fills them in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreMenu {
    /// `Space`: every operation that acts in the context.
    Actions,
    /// `y`: path copy.
    Copy,
    /// `y` in a table: cell / row / column copy.
    TableCopy,
    /// `,`: sorting and display options.
    Options,
}

/// What a key is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreBinding {
    Run(CoreAction),
    /// An operation only the full app runs: the standalone app resolves the key to
    /// [`CoreResolution::Unbound`] (it has nothing to run) and konoma maps it to its own action.
    Host(HostOp),
    Menu(CoreMenu),
    /// Only in a context's own table: this key is deliberately unbound there, so what it would
    /// inherit from layer 1 or 2 does not apply (`global` still does).
    Unbound,
}

/// What [`CoreKeys::resolve`] answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreResolution {
    Action(CoreAction),
    Menu(CoreMenu),
    Unbound,
}

fn resolution_of(b: &CoreBinding) -> CoreResolution {
    match b {
        CoreBinding::Run(a) => CoreResolution::Action(*a),
        CoreBinding::Menu(m) => CoreResolution::Menu(*m),
        // Nothing the standalone app can run (see `CoreKeys::host_op`).
        CoreBinding::Host(_) | CoreBinding::Unbound => CoreResolution::Unbound,
    }
}

/// A key table.
pub type CoreTable = HashMap<KeyPress, CoreBinding>;

/// The contexts a standalone app can show. Names are the `[keys.<name>]` tables of konoma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreContext {
    /// Source and plain text.
    Text,
    /// A decorated view: rendered Markdown, a Word document, slides.
    Document,
    Image,
    Table,
    /// A view the plugin draws itself; it inherits neither layer 1 nor a layer-2 table.
    Canvas,
}

/// A layer-2 table: keys shared by the contexts that show the same kind of thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreViewType {
    Text,
    Document,
    Table,
    Image,
}

impl CoreContext {
    pub const ALL: [CoreContext; 5] = [
        CoreContext::Text,
        CoreContext::Document,
        CoreContext::Image,
        CoreContext::Table,
        CoreContext::Canvas,
    ];

    /// The name used in `[keys.<name>]`.
    pub fn name(self) -> &'static str {
        match self {
            CoreContext::Text => "text",
            CoreContext::Document => "document",
            CoreContext::Image => "image",
            CoreContext::Table => "table",
            CoreContext::Canvas => "canvas",
        }
    }

    pub fn from_name(s: &str) -> Option<CoreContext> {
        CoreContext::ALL.into_iter().find(|c| c.name() == s)
    }

    /// The layer-2 tables this context looks into, most specific first.
    pub fn view_types(self) -> &'static [CoreViewType] {
        match self {
            CoreContext::Text => &[CoreViewType::Text],
            CoreContext::Document => &[CoreViewType::Document, CoreViewType::Text],
            CoreContext::Image => &[CoreViewType::Image],
            CoreContext::Table => &[CoreViewType::Table],
            CoreContext::Canvas => &[],
        }
    }

    /// Whether layer 1 (the basic table) applies.
    pub fn inherits_basic(self) -> bool {
        !matches!(self, CoreContext::Canvas)
    }
}

/// The contexts an **old** (pre-context) `[keys.<surface>]` name stands for.
fn old_surface_contexts(name: &str) -> Option<&'static [CoreContext]> {
    match name {
        "preview_text" => Some(&[CoreContext::Text, CoreContext::Document]),
        "preview_image" => Some(&[CoreContext::Image]),
        "preview_table" => Some(&[CoreContext::Table]),
        _ => None,
    }
}

fn run(a: CoreAction) -> CoreBinding {
    CoreBinding::Run(a)
}

fn nav(m: Motion) -> CoreBinding {
    CoreBinding::Run(CoreAction::Navigate(m))
}

fn host(o: HostOp) -> CoreBinding {
    CoreBinding::Host(o)
}

// --- The default tables, one function per table so a host can take the pieces it uses. ---

/// Layer 0: every context inherits these.
pub fn global_table() -> CoreTable {
    let mut g = CoreTable::new();
    g.insert(KeyPress::ch('Q'), run(CoreAction::Quit));
    g.insert(KeyPress::ch('F'), run(CoreAction::ToggleFollow));
    g.insert(KeyPress::ch('t'), run(CoreAction::TabNewAfter));
    g.insert(KeyPress::ch('T'), run(CoreAction::ToggleTabList));
    g.insert(KeyPress::ch('P'), run(CoreAction::PasteJump));
    g.insert(KeyPress::ch('['), run(CoreAction::TabPrev));
    g.insert(KeyPress::ch(']'), run(CoreAction::TabNext));
    for i in 1..=9u8 {
        g.insert(
            KeyPress::ch(char::from(b'0' + i)),
            run(CoreAction::TabGoto(i - 1)),
        );
    }
    g.insert(KeyPress::ch('?'), run(CoreAction::ToggleHelp));
    g
}

/// Layer 1: the keys every context with a cursor shares.
pub fn basic_table() -> CoreTable {
    let mut b = CoreTable::new();
    b.insert(KeyPress::ch('j'), nav(Motion::Down));
    b.insert(KeyPress::ch('k'), nav(Motion::Up));
    // `Space` = the menu of this context's operations.
    b.insert(KeyPress::ch(' '), CoreBinding::Menu(CoreMenu::Actions));
    b
}

/// `q`: go back one step.
pub fn back_table() -> CoreTable {
    CoreTable::from([(KeyPress::ch('q'), run(CoreAction::PreviewBack))])
}

/// Layer 3: the verbs every context has.
pub fn verbs_table() -> CoreTable {
    CoreTable::from([
        (KeyPress::ch('e'), run(CoreAction::RequestEdit)),
        (KeyPress::ch('o'), run(CoreAction::OpenExternal)),
    ])
}

/// The scrolling keys of the views that scroll (text, table, diff). Paging follows the scheme:
/// vim has `Ctrl-f/b/d/u`, less has `f/b/d/u`.
pub fn scroll_table(scheme: KeyScheme) -> CoreTable {
    let mut m = CoreTable::new();
    m.insert(KeyPress::ch('g'), nav(Motion::Top));
    m.insert(KeyPress::ch('G'), nav(Motion::Bottom));
    m.insert(KeyPress::ch('h'), nav(Motion::Left));
    m.insert(KeyPress::ch('l'), nav(Motion::Right));
    m.insert(KeyPress::ch('0'), nav(Motion::LineHome));
    m.insert(KeyPress::ch('$'), nav(Motion::LineEnd));
    m.insert(KeyPress::key(KeyCode::PageDown), nav(Motion::PageDown));
    m.insert(KeyPress::key(KeyCode::PageUp), nav(Motion::PageUp));
    let (f, b, d, u) = match scheme {
        KeyScheme::Vim => (
            KeyPress::ctrl_ch('f'),
            KeyPress::ctrl_ch('b'),
            KeyPress::ctrl_ch('d'),
            KeyPress::ctrl_ch('u'),
        ),
        KeyScheme::Less => (
            KeyPress::ch('f'),
            KeyPress::ch('b'),
            KeyPress::ch('d'),
            KeyPress::ch('u'),
        ),
    };
    m.insert(f, nav(Motion::PageDown));
    m.insert(b, nav(Motion::PageUp));
    m.insert(d, nav(Motion::HalfDown));
    m.insert(u, nav(Motion::HalfUp));
    m
}

/// `+` `-` `=`: zoom the picture in view.
pub fn zoom_table() -> CoreTable {
    CoreTable::from([
        (KeyPress::ch('+'), run(CoreAction::ImageZoomIn)),
        (KeyPress::ch('-'), run(CoreAction::ImageZoomOut)),
        (KeyPress::ch('='), run(CoreAction::ImageZoomReset)),
    ])
}

/// `J` / `K`: the next / previous page of a page sequence.
pub fn page_turn_table() -> CoreTable {
    CoreTable::from([
        (KeyPress::ch('J'), run(CoreAction::PageNext)),
        (KeyPress::ch('K'), run(CoreAction::PagePrev)),
    ])
}

/// `J` / `K` and `O`: the page sequence (slides, sheets, PDF pages) and its list.
pub fn pages_table() -> CoreTable {
    let mut m = page_turn_table();
    m.insert(KeyPress::ch('O'), run(CoreAction::ToggleOutline));
    m
}

/// Layer 2: the table of a view type.
pub fn type_table(vt: CoreViewType, scheme: KeyScheme) -> CoreTable {
    match vt {
        CoreViewType::Text | CoreViewType::Table => scroll_table(scheme),
        // A document is also text, so lookup continues into the text table.
        CoreViewType::Document => CoreTable::new(),
        CoreViewType::Image => {
            let mut m = CoreTable::new();
            m.insert(KeyPress::ch('h'), nav(Motion::Left));
            m.insert(KeyPress::ch('l'), nav(Motion::Right));
            m.extend(zoom_table());
            m.extend(pages_table());
            m.insert(KeyPress::key(KeyCode::PageDown), run(CoreAction::PageNext));
            m.insert(KeyPress::key(KeyCode::PageUp), run(CoreAction::PagePrev));
            m
        }
    }
}

/// The default keys of the operations only the full app runs ([`HostOp`]), per context: the
/// search, the bookmarks, the reload, the project's screen, the file paging and the rest. They
/// live here, next to the shared keys, so that a key is never given twice and both hosts see the
/// same keys taken.
pub fn host_ops_table(ctx: CoreContext) -> CoreTable {
    let mut m = CoreTable::new();
    // `p` is the project's screen on every context; the path style is in the `,` menu.
    m.insert(KeyPress::ch('p'), host(HostOp::OpenGitView));
    m.insert(KeyPress::ch('!'), host(HostOp::GitLaunchTool));
    m.insert(KeyPress::ch('r'), host(HostOp::Refresh));
    // `m` registers the previewed file, `'` opens the list.
    m.insert(KeyPress::ch('m'), host(HostOp::MarkSet));
    m.insert(KeyPress::ch('\''), host(HostOp::MarkJump));
    // `Ctrl-n` / `Ctrl-p` page through the files in tree order (`J` / `K` are the page turn, and
    // `Ctrl-j` is indistinguishable from Enter on legacy terminals, so neither is used).
    m.insert(KeyPress::ctrl_ch('n'), host(HostOp::PreviewFileNext));
    m.insert(KeyPress::ctrl_ch('p'), host(HostOp::PreviewFilePrev));
    let searching = |m: &mut CoreTable| {
        m.insert(KeyPress::ch('/'), host(HostOp::SearchStart));
        m.insert(KeyPress::ch('n'), host(HostOp::SearchNext));
        m.insert(KeyPress::ch('N'), host(HostOp::SearchPrev));
    };
    match ctx {
        CoreContext::Text | CoreContext::Document => {
            searching(&mut m);
            m.insert(KeyPress::ch('v'), host(HostOp::PreviewEnterVisual));
            m.insert(KeyPress::ch('V'), host(HostOp::PreviewEnterVisualLine));
            m.insert(KeyPress::ch('R'), host(HostOp::CycleRepresentation));
            // Open the focused link in a new tab (Enter still opens it in the same tab).
            m.insert(KeyPress::ctrl_ch('t'), host(HostOp::OpenLinkNewTab));
            // `Y`: an `@path#L` reference for the caret line (or the selection).
            m.insert(KeyPress::ch('Y'), host(HostOp::PreviewCopySelectionRef));
        }
        CoreContext::Image => {
            // Back to the diff while this picture is the diff's own representation; a no-op
            // otherwise (not `CycleRepresentation`, whose fallthrough is for text).
            m.insert(KeyPress::ch('R'), host(HostOp::ImageReturnToDiff));
        }
        CoreContext::Table => {
            searching(&mut m);
            m.insert(KeyPress::ch('R'), host(HostOp::CycleRepresentation));
        }
        CoreContext::Canvas => {
            m.insert(KeyPress::ch('R'), host(HostOp::CycleRepresentation));
            m.insert(KeyPress::ch('Y'), host(HostOp::PreviewCopySelectionRef));
        }
    }
    m
}

/// The part of a context's own table that is shared: the commands every host runs and the keys
/// of the operations only the full app runs ([`host_ops_table`]).
pub fn own_table(ctx: CoreContext) -> CoreTable {
    let mut m = back_table();
    m.extend(verbs_table());
    m.extend(host_ops_table(ctx));
    m.insert(KeyPress::ch(','), CoreBinding::Menu(CoreMenu::Options));
    match ctx {
        CoreContext::Text | CoreContext::Document => {
            m.extend(zoom_table());
            m.extend(pages_table());
            m.insert(KeyPress::ch('y'), CoreBinding::Menu(CoreMenu::Copy));
        }
        CoreContext::Image => {
            m.insert(KeyPress::ch('y'), CoreBinding::Menu(CoreMenu::Copy));
        }
        CoreContext::Table => {
            m.extend(pages_table());
            m.insert(KeyPress::ch('y'), CoreBinding::Menu(CoreMenu::TableCopy));
        }
        CoreContext::Canvas => {
            m.extend(pages_table());
            m.insert(KeyPress::ch(' '), CoreBinding::Menu(CoreMenu::Actions));
            m.insert(KeyPress::ch('y'), CoreBinding::Menu(CoreMenu::Copy));
        }
    }
    m
}

// =============================================================================
// The menus `y` and `,` open: their default items and the overrides of `[keys]`.
// =============================================================================

/// One item of a menu: the key pressed after the menu key, what it runs, and its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem<A, L> {
    pub key: KeyPress,
    pub action: A,
    pub label: L,
}

/// A menu (the which-key popup of `y` or `,`). Generic over the action and the label so a host
/// keeps its own types in it while the editing rules stay here, written once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu<A, L> {
    pub title: L,
    pub items: Vec<MenuItem<A, L>>,
}

impl<A: Copy, L: Copy> Menu<A, L> {
    /// What the item for `kp` runs.
    pub fn find(&self, kp: KeyPress) -> Option<A> {
        self.items
            .iter()
            .find(|it| it.key == kp)
            .map(|it| it.action)
    }

    /// Add an item, or replace the one for the same key.
    pub fn set(&mut self, kp: KeyPress, action: A, label: L) {
        self.items.retain(|it| it.key != kp);
        self.items.push(MenuItem {
            key: kp,
            action,
            label,
        });
    }

    /// Remove the item for `kp` (nothing happens when there is none).
    pub fn remove(&mut self, kp: KeyPress) {
        self.items.retain(|it| it.key != kp);
    }

    /// The same menu in another host's action and label types.
    pub fn map<B, M>(&self, fa: &impl Fn(A) -> B, fl: &impl Fn(L) -> M) -> Menu<B, M> {
        Menu {
            title: fl(self.title),
            items: self
                .items
                .iter()
                .map(|it| MenuItem {
                    key: it.key,
                    action: fa(it.action),
                    label: fl(it.label),
                })
                .collect(),
        }
    }
}

/// Which menus a `[keys]` entry like `"y x" = "copy_full"` changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuScope {
    /// `[keys.global]`: the shared menu and every context's own copy of it.
    Global,
    /// `[keys.<context>]`: only that context's own copy.
    Context(CoreContext),
}

/// [`MenuSet::edit`] found no menu to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoMenu;

/// The menus of the preview contexts: the ones every context shares (`y`'s two menus) and the
/// ones a context has of its own (the `,` menus, and a copy of a shared menu once a context
/// changes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSet<A, L> {
    pub shared: HashMap<CoreMenu, Menu<A, L>>,
    pub per_context: HashMap<(CoreContext, CoreMenu), Menu<A, L>>,
}

impl<A: Copy, L: Copy> MenuSet<A, L> {
    /// The menu `menu` is in `ctx`: the context's own copy when it has one, else the shared one.
    pub fn get(&self, ctx: CoreContext, menu: CoreMenu) -> Option<&Menu<A, L>> {
        self.per_context
            .get(&(ctx, menu))
            .or_else(|| self.shared.get(&menu))
    }

    /// Put `entry` (an action and its label) on `key` in the menu `menu` opens, or remove the
    /// key (`None`). `Global` changes the shared menu and every context's copy of it; a context
    /// changes only its own copy, made from the shared menu on the first change. `Err` when
    /// there is no menu to change.
    pub fn edit(
        &mut self,
        scope: MenuScope,
        menu: CoreMenu,
        key: KeyPress,
        entry: Option<(A, L)>,
    ) -> Result<(), NoMenu> {
        let apply = |m: &mut Menu<A, L>| match entry {
            Some((a, l)) => m.set(key, a, l),
            None => m.remove(key),
        };
        match scope {
            MenuScope::Global => {
                if let Some(m) = self.shared.get_mut(&menu) {
                    apply(m);
                }
                for ((_, id), m) in self.per_context.iter_mut() {
                    if *id == menu {
                        apply(m);
                    }
                }
                Ok(())
            }
            MenuScope::Context(c) => {
                let base = self
                    .per_context
                    .get(&(c, menu))
                    .or_else(|| self.shared.get(&menu))
                    .cloned()
                    .ok_or(NoMenu)?;
                apply(self.per_context.entry((c, menu)).or_insert(base));
                Ok(())
            }
        }
    }

    /// The same menus in another host's action and label types.
    pub fn map<B: Copy, M: Copy>(
        &self,
        fa: &impl Fn(A) -> B,
        fl: &impl Fn(L) -> M,
    ) -> MenuSet<B, M> {
        MenuSet {
            shared: self
                .shared
                .iter()
                .map(|(k, m)| (*k, m.map(fa, fl)))
                .collect(),
            per_context: self
                .per_context
                .iter()
                .map(|(k, m)| (*k, m.map(fa, fl)))
                .collect(),
        }
    }
}

/// The label of a menu item that runs `a` (`CoreMsg::Empty` for an action no menu labels).
pub fn menu_label(a: CoreAction) -> CoreMsg {
    match a {
        CoreAction::CopyPath(CopyKind::Name) => CoreMsg::WkName,
        CoreAction::CopyPath(CopyKind::Relative) => CoreMsg::WkRelative,
        CoreAction::CopyPath(CopyKind::Full) => CoreMsg::WkFull,
        CoreAction::CopyPath(CopyKind::Parent) => CoreMsg::WkParent,
        CoreAction::CopyPath(CopyKind::AtRef) => CoreMsg::WkAtRef,
        CoreAction::CopyCodeBlock => CoreMsg::WkCodeBlock,
        CoreAction::TableCopy(TableCopyKind::Cell) => CoreMsg::WkCell,
        CoreAction::TableCopy(TableCopyKind::Row) => CoreMsg::WkRow,
        CoreAction::TableCopy(TableCopyKind::Column) => CoreMsg::WkColumn,
        CoreAction::SortSet(SortKey::Name) => CoreMsg::FtSortName,
        CoreAction::SortSet(SortKey::Size) => CoreMsg::FtSortSize,
        CoreAction::SortSet(SortKey::Modified) => CoreMsg::FtSortMod,
        CoreAction::SortSet(SortKey::Ext) => CoreMsg::FtSortExt,
        CoreAction::CyclePathStyle => CoreMsg::HintPath,
        CoreAction::ToggleWrap => CoreMsg::HintWrap,
        _ => CoreMsg::Empty,
    }
}

/// The menu a prefix key opens in a `[keys]` chord such as `"y x"`: `y` the copy menu, `,` the
/// options menu. (`Space` opens a menu a host builds when it opens, which a chord cannot edit.)
pub fn chord_prefix_menu(pre: KeyPress) -> Option<CoreMenu> {
    if pre == KeyPress::ch('y') {
        Some(CoreMenu::Copy)
    } else if pre == KeyPress::ch(',') {
        Some(CoreMenu::Options)
    } else {
        None
    }
}

fn menu_of(title: CoreMsg, items: &[(char, CoreAction)]) -> Menu<CoreAction, CoreMsg> {
    Menu {
        title,
        items: items
            .iter()
            .map(|&(c, a)| MenuItem {
                key: KeyPress::ch(c),
                action: a,
                label: menu_label(a),
            })
            .collect(),
    }
}

/// The `y` menu: path copy. `c` (the code block) is shown only while a code block is focused.
pub fn copy_menu_default() -> Menu<CoreAction, CoreMsg> {
    menu_of(
        CoreMsg::WkCopyPathTitle,
        &[
            ('n', CoreAction::CopyPath(CopyKind::Name)),
            ('r', CoreAction::CopyPath(CopyKind::Relative)),
            ('f', CoreAction::CopyPath(CopyKind::Full)),
            ('p', CoreAction::CopyPath(CopyKind::Parent)),
            ('@', CoreAction::CopyPath(CopyKind::AtRef)),
            ('c', CoreAction::CopyCodeBlock),
        ],
    )
}

/// The `y` menu of a table: cell / row / column copy (`f` keeps the full-path copy reachable).
pub fn table_copy_menu_default() -> Menu<CoreAction, CoreMsg> {
    menu_of(
        CoreMsg::WkTableCopyTitle,
        &[
            ('c', CoreAction::TableCopy(TableCopyKind::Cell)),
            ('r', CoreAction::TableCopy(TableCopyKind::Row)),
            ('C', CoreAction::TableCopy(TableCopyKind::Column)),
            ('f', CoreAction::CopyPath(CopyKind::Full)),
        ],
    )
}

/// The `,` menu of a context: what it lets you show. A text or document view also wraps lines.
pub fn options_menu_default(ctx: CoreContext) -> Menu<CoreAction, CoreMsg> {
    let path = ('p', CoreAction::CyclePathStyle);
    let wrap = ('w', CoreAction::ToggleWrap);
    match ctx {
        CoreContext::Text | CoreContext::Document => {
            menu_of(CoreMsg::WkOptionsTitle, &[path, wrap])
        }
        CoreContext::Image | CoreContext::Table | CoreContext::Canvas => {
            menu_of(CoreMsg::WkOptionsTitle, &[path])
        }
    }
}

/// The default menus of the contexts this module defines.
pub fn default_menus() -> MenuSet<CoreAction, CoreMsg> {
    MenuSet {
        shared: HashMap::from([
            (CoreMenu::Copy, copy_menu_default()),
            (CoreMenu::TableCopy, table_copy_menu_default()),
        ]),
        per_context: CoreContext::ALL
            .into_iter()
            .map(|c| ((c, CoreMenu::Options), options_menu_default(c)))
            .collect(),
    }
}

// =============================================================================
// CoreKeys: the layered tables, the overrides and the lookup.
// =============================================================================

/// Layers 0 to 3 with the user's overrides applied.
///
/// Keys are looked up in layers, most specific first: the context's own table, the tables of its
/// view types, the basic table (if the context [inherits](CoreContext::inherits_basic) it), then
/// `global`.
#[derive(Debug, Clone)]
pub struct CoreKeys {
    pub contexts: HashMap<CoreContext, CoreTable>,
    pub types: HashMap<CoreViewType, CoreTable>,
    pub basic: CoreTable,
    pub global: CoreTable,
    /// The menus `y` and `,` open, with the `"y x" = "..."` overrides applied.
    pub menus: MenuSet<CoreAction, CoreMsg>,
}

/// What a `[keys.<name>]` table writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Global,
    Context(CoreContext),
}

impl CoreKeys {
    /// The built-in defaults.
    pub fn defaults(scheme: KeyScheme) -> CoreKeys {
        let view_types = [
            CoreViewType::Text,
            CoreViewType::Document,
            CoreViewType::Table,
            CoreViewType::Image,
        ];
        CoreKeys {
            contexts: CoreContext::ALL
                .into_iter()
                .map(|c| (c, own_table(c)))
                .collect(),
            types: view_types
                .into_iter()
                .map(|t| (t, type_table(t, scheme)))
                .collect(),
            basic: basic_table(),
            global: global_table(),
            menus: default_menus(),
        }
    }

    /// The menu `menu` is in `ctx`, with the overrides applied (`None` for `Space`'s menu, which
    /// a host builds when it opens).
    pub fn menu(&self, ctx: CoreContext, menu: CoreMenu) -> Option<&Menu<CoreAction, CoreMsg>> {
        self.menus.get(ctx, menu)
    }

    /// Defaults, then the `[keys]` overrides of `cfg` (the old `copy_*` aliases included, as
    /// konoma reads them), then the same conflict rules as konoma: a key that defaulted to a
    /// menu stays a menu, and a context cannot take a `global` key for another action.
    pub fn from_keys_config(scheme: KeyScheme, cfg: &KeysConfig) -> CoreKeys {
        CoreKeys::from_config(scheme, &cfg.to_keymap_config())
    }

    /// As [`CoreKeys::from_keys_config`], from the raw tables.
    pub fn from_config(scheme: KeyScheme, cfg: &KeysFileConfig) -> CoreKeys {
        let mut keys = CoreKeys::defaults(scheme);
        let defaults = CoreKeys::defaults(scheme);

        // Sorted, so the result is the same on every run.
        let mut names: Vec<&String> = cfg.surfaces.keys().collect();
        names.sort();
        type Tables<'a> = Vec<(Vec<Target>, &'a HashMap<String, String>)>;
        let mut old_tables: Tables = Vec::new();
        let mut new_tables: Tables = Vec::new();
        for name in names {
            let table = &cfg.surfaces[name];
            if name == "global" {
                new_tables.push((vec![Target::Global], table));
            } else if let Some(c) = CoreContext::from_name(name) {
                new_tables.push((vec![Target::Context(c)], table));
            } else if let Some(cs) = old_surface_contexts(name) {
                old_tables.push((cs.iter().map(|&c| Target::Context(c)).collect(), table));
            }
            // Any other table belongs to a context this module does not have.
        }
        // Old names first, so a table written under the new name overrides them; `global`
        // before the contexts.
        new_tables.sort_by_key(|(targets, _)| !targets.contains(&Target::Global));
        for (targets, table) in old_tables.into_iter().chain(new_tables) {
            let mut entries: Vec<(&String, &String)> = table.iter().collect();
            entries.sort();
            for (chord, action) in entries {
                keys.apply_one(&targets, chord, action);
            }
        }
        keys.validate(&defaults);
        keys
    }

    /// Apply one (key string -> action string) to every target. A chord (`"y x"`) is an entry of
    /// a menu. Anything this module cannot honour is ignored: an invalid or fixed key, a chord
    /// whose prefix opens no menu, `tab_goto`, a name that is no core action.
    fn apply_one(&mut self, targets: &[Target], key: &str, action: &str) {
        let tokens: Vec<&str> = key.split_whitespace().collect();
        let keys: Vec<KeyPress> = match tokens.iter().map(|t| KeyPress::parse(t)).collect() {
            Ok(k) => k,
            Err(_) => return,
        };
        let action = if action == "noop" || action == "disabled" {
            None
        } else if let Some(a) = CoreAction::from_name(action) {
            Some(CoreBinding::Run(a))
        } else if let Some(o) = HostOp::from_name(action) {
            Some(CoreBinding::Host(o))
        } else {
            return;
        };
        match keys[..] {
            [kp] => {
                if is_fixed_key(kp) {
                    return;
                }
                for &t in targets {
                    self.set_key(t, kp, action);
                }
            }
            [pre, suf] => {
                if pre != KeyPress::ch(' ') && chord_prefix_menu(pre).is_none() {
                    return;
                }
                // The menus hold what a standalone app can run; an operation of the full app
                // has no entry there.
                let action = match action {
                    Some(CoreBinding::Run(a)) => Some(a),
                    Some(_) => return,
                    None => None,
                };
                for &t in targets {
                    self.set_menu_entry(t, pre, suf, action);
                }
            }
            _ => {}
        }
    }

    /// Put `suf` (or remove it, `None`) in the menu that `pre` opens, for `target`. A context
    /// changes the menu its own `pre` opens (`y` is a different menu in a table); a prefix that
    /// opens no menu there, or `Space`'s menu, is left alone.
    fn set_menu_entry(
        &mut self,
        target: Target,
        pre: KeyPress,
        suf: KeyPress,
        action: Option<CoreAction>,
    ) {
        let entry = action.map(|a| (a, menu_label(a)));
        match target {
            Target::Global => {
                if let Some(m) = chord_prefix_menu(pre) {
                    let _ = self.menus.edit(MenuScope::Global, m, suf, entry);
                }
            }
            Target::Context(c) => {
                if let Some(CoreBinding::Menu(m)) = self.local(c, pre) {
                    if *m != CoreMenu::Actions {
                        let m = *m;
                        let _ = self.menus.edit(MenuScope::Context(c), m, suf, entry);
                    }
                }
            }
        }
    }

    /// Bind `kp` to `action` in `target`'s own table, or unbind it there (`None`). Unbinding
    /// must also hide what the context inherits from layers 1-2, hence the explicit
    /// [`CoreBinding::Unbound`] mark; layer 0 still applies.
    fn set_key(&mut self, target: Target, kp: KeyPress, action: Option<CoreBinding>) {
        match target {
            Target::Global => match action {
                Some(a) => {
                    self.global.insert(kp, a);
                }
                None => {
                    self.global.remove(&kp);
                }
            },
            Target::Context(c) => {
                let inherited = self.inherited(c, kp).is_some();
                let table = self.contexts.entry(c).or_default();
                match action {
                    Some(a) => {
                        table.insert(kp, a);
                    }
                    None if inherited => {
                        table.insert(kp, CoreBinding::Unbound);
                    }
                    None => {
                        table.remove(&kp);
                    }
                }
            }
        }
    }

    /// What layers 1-2 give `kp` in `ctx`.
    fn inherited(&self, ctx: CoreContext, kp: KeyPress) -> Option<&CoreBinding> {
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

    /// The binding `kp` has in `ctx` before `global`. `None` when it has none or the context
    /// unbound it.
    fn local(&self, ctx: CoreContext, kp: KeyPress) -> Option<&CoreBinding> {
        match self.contexts.get(&ctx).and_then(|m| m.get(&kp)) {
            Some(CoreBinding::Unbound) => None,
            Some(b) => Some(b),
            None => self.inherited(ctx, kp),
        }
    }

    /// Every key that has an entry in `ctx`'s own table, its layers 2 and 1 (not `global`).
    fn local_keys(&self, ctx: CoreContext) -> Vec<KeyPress> {
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

    /// Revert the overrides that conflict with the defaults: a key that opened a menu and was
    /// given an action, and a `global` key that a context took for another action.
    fn validate(&mut self, defaults: &CoreKeys) {
        let mut global_keys: Vec<KeyPress> = self.global.keys().copied().collect();
        global_keys.sort_by_key(|k| format!("{k:?}"));
        for ctx in CoreContext::ALL {
            let menu_keys: Vec<(KeyPress, CoreMenu)> = defaults
                .local_keys(ctx)
                .into_iter()
                .filter_map(|k| match defaults.local(ctx, k) {
                    Some(CoreBinding::Menu(m)) => Some((k, *m)),
                    _ => None,
                })
                .collect();
            for (k, m) in menu_keys {
                if let Some(CoreBinding::Run(_) | CoreBinding::Host(_)) = self.local(ctx, k) {
                    self.contexts
                        .entry(ctx)
                        .or_default()
                        .insert(k, CoreBinding::Menu(m));
                }
            }

            // A binding the defaults have too is legitimate; otherwise restore the default's
            // binding for the context, or fall back to `global`.
            let shadows: Vec<(KeyPress, Option<CoreBinding>)> = global_keys
                .iter()
                .filter_map(|gk| {
                    let local = self.local(ctx, *gk)?;
                    let global = self.global.get(gk);
                    if Some(local) == global {
                        return None;
                    }
                    let default_local = defaults.local(ctx, *gk);
                    if Some(local) == default_local {
                        return None;
                    }
                    Some((*gk, default_local.copied()))
                })
                .collect();
            for (gk, restore) in shadows {
                match restore {
                    Some(b) => {
                        self.contexts.entry(ctx).or_default().insert(gk, b);
                    }
                    None => self.set_key(Target::Context(ctx), gk, None),
                }
            }
        }
    }

    /// The operation of the full app `kp` is bound to in `ctx` ([`HostOp`]; `None` for any other
    /// key). The same lookup as [`CoreKeys::resolve`], which answers
    /// [`CoreResolution::Unbound`] for such a key.
    pub fn host_op(&self, ctx: CoreContext, kp: KeyPress) -> Option<HostOp> {
        match self.local(ctx, kp).or_else(|| self.global.get(&kp)) {
            Some(CoreBinding::Host(o)) => Some(*o),
            _ => None,
        }
    }

    /// What `kp` does in `ctx`: the context's own table, its layer-2 tables, the basic layer,
    /// then `global`. A key bound to an operation only the full app runs ([`HostOp`]) is
    /// `Unbound` here: the standalone app has nothing to run for it.
    pub fn resolve(&self, ctx: CoreContext, kp: KeyPress) -> CoreResolution {
        if let Some(b) = self.local(ctx, kp) {
            return resolution_of(b);
        }
        if let Some(b) = self.global.get(&kp) {
            return resolution_of(b);
        }
        CoreResolution::Unbound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
        KeysFileConfig {
            surfaces: tables
                .iter()
                .map(|(n, es)| {
                    (
                        n.to_string(),
                        es.iter()
                            .map(|(k, a)| (k.to_string(), a.to_string()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    fn at(k: &CoreKeys, c: CoreContext, key: char) -> CoreResolution {
        k.resolve(c, KeyPress::ch(key))
    }

    #[test]
    fn every_action_name_round_trips_except_the_fixed_tab_goto() {
        for a in CoreAction::all() {
            match a {
                CoreAction::TabGoto(_) => {
                    assert_eq!(a.name(), "tab_goto");
                    assert_eq!(CoreAction::from_name("tab_goto"), None);
                }
                _ => assert_eq!(CoreAction::from_name(&a.name()), Some(a), "{}", a.name()),
            }
        }
    }

    #[test]
    fn action_names_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for a in CoreAction::all() {
            if !matches!(a, CoreAction::TabGoto(_)) {
                assert!(seen.insert(a.name()), "duplicate name {}", a.name());
            }
        }
    }

    #[test]
    fn old_page_names_are_aliases() {
        for n in ["pdf_next_page", "sheet_next", "media_diff_page_next"] {
            assert_eq!(CoreAction::from_name(n), Some(CoreAction::PageNext));
        }
        for n in ["pdf_prev_page", "sheet_prev", "media_diff_page_prev"] {
            assert_eq!(CoreAction::from_name(n), Some(CoreAction::PagePrev));
        }
    }

    #[test]
    fn the_defaults_resolve_the_layers_in_order() {
        let k = CoreKeys::defaults(KeyScheme::Vim);
        let t = CoreContext::Text;
        assert_eq!(
            at(&k, t, 'j'),
            CoreResolution::Action(CoreAction::Navigate(Motion::Down))
        );
        assert_eq!(
            at(&k, t, 'g'),
            CoreResolution::Action(CoreAction::Navigate(Motion::Top))
        );
        assert_eq!(
            at(&k, t, 'q'),
            CoreResolution::Action(CoreAction::PreviewBack)
        );
        assert_eq!(
            at(&k, t, 'e'),
            CoreResolution::Action(CoreAction::RequestEdit)
        );
        assert_eq!(
            at(&k, t, 'o'),
            CoreResolution::Action(CoreAction::OpenExternal)
        );
        assert_eq!(at(&k, t, 'Q'), CoreResolution::Action(CoreAction::Quit));
        assert_eq!(at(&k, t, ' '), CoreResolution::Menu(CoreMenu::Actions));
        assert_eq!(at(&k, t, 'y'), CoreResolution::Menu(CoreMenu::Copy));
        assert_eq!(
            at(&k, CoreContext::Table, 'y'),
            CoreResolution::Menu(CoreMenu::TableCopy)
        );
        assert_eq!(
            at(&k, CoreContext::Image, 'J'),
            CoreResolution::Action(CoreAction::PageNext)
        );
        assert_eq!(
            at(&k, CoreContext::Canvas, 'j'),
            CoreResolution::Unbound,
            "the canvas has no layer 1"
        );
        assert_eq!(
            at(&k, CoreContext::Canvas, 'q'),
            CoreResolution::Action(CoreAction::PreviewBack)
        );
    }

    #[test]
    fn the_less_scheme_pages_with_plain_letters() {
        let v = CoreKeys::defaults(KeyScheme::Vim);
        let l = CoreKeys::defaults(KeyScheme::Less);
        let t = CoreContext::Text;
        let page = CoreResolution::Action(CoreAction::Navigate(Motion::PageDown));
        assert_eq!(v.resolve(t, KeyPress::ctrl_ch('f')), page);
        assert_eq!(at(&v, t, 'f'), CoreResolution::Unbound);
        assert_eq!(at(&l, t, 'f'), page);
        assert_eq!(
            l.resolve(t, KeyPress::ctrl_ch('f')),
            CoreResolution::Unbound
        );
    }

    #[test]
    fn a_rebind_moves_the_key_and_leaves_the_old_one_alone() {
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("text", &[("x", "page_next")])]));
        assert_eq!(
            at(&k, CoreContext::Text, 'x'),
            CoreResolution::Action(CoreAction::PageNext)
        );
        assert_eq!(
            at(&k, CoreContext::Text, 'J'),
            CoreResolution::Action(CoreAction::PageNext)
        );
        assert_eq!(
            at(&k, CoreContext::Table, 'x'),
            CoreResolution::Unbound,
            "only the text context"
        );
    }

    #[test]
    fn noop_hides_an_inherited_key_but_not_global() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[("text", &[("j", "noop"), ("Q", "noop")])]),
        );
        assert_eq!(at(&k, CoreContext::Text, 'j'), CoreResolution::Unbound);
        assert_eq!(
            at(&k, CoreContext::Document, 'j'),
            CoreResolution::Action(CoreAction::Navigate(Motion::Down))
        );
        assert_eq!(
            at(&k, CoreContext::Text, 'Q'),
            CoreResolution::Action(CoreAction::Quit),
            "layer 0 stays"
        );
    }

    #[test]
    fn a_context_cannot_take_a_global_key() {
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("text", &[("Q", "page_next")])]));
        assert_eq!(
            at(&k, CoreContext::Text, 'Q'),
            CoreResolution::Action(CoreAction::Quit)
        );
    }

    #[test]
    fn a_menu_key_stays_a_menu() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[("text", &[("space", "page_next"), ("y", "page_next")])]),
        );
        assert_eq!(
            at(&k, CoreContext::Text, ' '),
            CoreResolution::Menu(CoreMenu::Actions)
        );
        assert_eq!(
            at(&k, CoreContext::Text, 'y'),
            CoreResolution::Menu(CoreMenu::Copy)
        );
    }

    #[test]
    fn fixed_keys_chords_unknown_names_and_tab_goto_are_ignored() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[(
                "text",
                &[
                    ("enter", "page_next"),
                    ("home", "page_next"),
                    ("y x", "page_next"),
                    ("z", "no_such_action"),
                    ("w", "tab_goto"),
                    ("v", "git_stage"),
                    ("ctrl-", "page_next"),
                ],
            )]),
        );
        let d = CoreKeys::defaults(KeyScheme::Vim);
        for key in ["enter", "home", "z", "w", "v"] {
            let kp = KeyPress::parse(key).unwrap();
            assert_eq!(
                k.resolve(CoreContext::Text, kp),
                d.resolve(CoreContext::Text, kp),
                "{key}"
            );
        }
    }

    #[test]
    fn global_table_applies_to_every_context_and_the_new_name_beats_the_old() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[
                ("global", &[("X", "image_zoom_in")]),
                ("preview_text", &[("x", "page_next"), ("n", "page_next")]),
                ("text", &[("x", "page_prev")]),
            ]),
        );
        for c in CoreContext::ALL {
            assert_eq!(
                at(&k, c, 'X'),
                CoreResolution::Action(CoreAction::ImageZoomIn),
                "{c:?}"
            );
        }
        assert_eq!(
            at(&k, CoreContext::Text, 'x'),
            CoreResolution::Action(CoreAction::PagePrev)
        );
        assert_eq!(
            at(&k, CoreContext::Document, 'x'),
            CoreResolution::Action(CoreAction::PageNext),
            "the old name reaches document too"
        );
        assert_eq!(at(&k, CoreContext::Image, 'x'), CoreResolution::Unbound);
    }

    #[test]
    fn key_parse_reports_what_is_wrong() {
        assert!(KeyPress::parse("").is_err());
        assert!(KeyPress::parse("ctrl-").is_err());
        assert!(KeyPress::parse("ab").is_err());
        assert_eq!(KeyPress::parse("ctrl-t"), Ok(KeyPress::ctrl_ch('t')));
        assert_eq!(KeyPress::parse("Space"), Ok(KeyPress::ch(' ')));
        assert_eq!(KeyPress::ctrl_ch('t').label(), "Ctrl-t");
    }

    #[test]
    fn the_sort_key_parses_with_a_fallback() {
        assert_eq!(SortKey::parse(" MTime "), SortKey::Modified);
        assert_eq!(SortKey::parse("nonsense"), SortKey::Name);
    }

    // --- The menus `y` and `,` open. ---

    fn items(k: &CoreKeys, c: CoreContext, m: CoreMenu) -> Vec<(char, CoreAction)> {
        k.menu(c, m)
            .unwrap_or_else(|| panic!("{c:?} has no {m:?} menu"))
            .items
            .iter()
            .map(|it| match it.key.code {
                KeyCode::Char(ch) => (ch, it.action),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_default_copy_menu_has_the_path_copies_and_the_code_block() {
        let k = CoreKeys::defaults(KeyScheme::Vim);
        for c in [
            CoreContext::Text,
            CoreContext::Document,
            CoreContext::Image,
            CoreContext::Canvas,
        ] {
            assert_eq!(
                items(&k, c, CoreMenu::Copy),
                vec![
                    ('n', CoreAction::CopyPath(CopyKind::Name)),
                    ('r', CoreAction::CopyPath(CopyKind::Relative)),
                    ('f', CoreAction::CopyPath(CopyKind::Full)),
                    ('p', CoreAction::CopyPath(CopyKind::Parent)),
                    ('@', CoreAction::CopyPath(CopyKind::AtRef)),
                    ('c', CoreAction::CopyCodeBlock),
                ],
                "{c:?}"
            );
        }
        let m = k.menu(CoreContext::Text, CoreMenu::Copy).unwrap();
        assert_eq!(m.title, CoreMsg::WkCopyPathTitle);
        let labels: Vec<CoreMsg> = m.items.iter().map(|it| it.label).collect();
        assert_eq!(
            labels,
            vec![
                CoreMsg::WkName,
                CoreMsg::WkRelative,
                CoreMsg::WkFull,
                CoreMsg::WkParent,
                CoreMsg::WkAtRef,
                CoreMsg::WkCodeBlock
            ]
        );
    }

    #[test]
    fn the_default_table_menu_copies_a_cell_a_row_a_column_or_the_path() {
        let k = CoreKeys::defaults(KeyScheme::Vim);
        assert_eq!(
            items(&k, CoreContext::Table, CoreMenu::TableCopy),
            vec![
                ('c', CoreAction::TableCopy(TableCopyKind::Cell)),
                ('r', CoreAction::TableCopy(TableCopyKind::Row)),
                ('C', CoreAction::TableCopy(TableCopyKind::Column)),
                ('f', CoreAction::CopyPath(CopyKind::Full)),
            ]
        );
        let m = k.menu(CoreContext::Table, CoreMenu::TableCopy).unwrap();
        assert_eq!(m.title, CoreMsg::WkTableCopyTitle);
        assert_eq!(m.items[0].label, CoreMsg::WkCell);
        assert_eq!(m.items[2].label, CoreMsg::WkColumn);
    }

    #[test]
    fn the_default_options_menus_hold_the_path_style_and_the_wrap_of_text() {
        let k = CoreKeys::defaults(KeyScheme::Vim);
        for c in CoreContext::ALL {
            let want: Vec<(char, CoreAction)> = match c {
                CoreContext::Text | CoreContext::Document => vec![
                    ('p', CoreAction::CyclePathStyle),
                    ('w', CoreAction::ToggleWrap),
                ],
                _ => vec![('p', CoreAction::CyclePathStyle)],
            };
            assert_eq!(items(&k, c, CoreMenu::Options), want, "{c:?}");
            let m = k.menu(c, CoreMenu::Options).unwrap();
            assert_eq!(m.title, CoreMsg::WkOptionsTitle);
            assert_eq!(m.items[0].label, CoreMsg::HintPath);
        }
    }

    #[test]
    fn a_menu_the_context_does_not_open_is_not_there() {
        let k = CoreKeys::defaults(KeyScheme::Vim);
        // `Space`'s menu is built by the host when it opens.
        assert!(k.menu(CoreContext::Text, CoreMenu::Actions).is_none());
    }

    #[test]
    fn a_chord_in_a_context_adds_replaces_and_removes_an_item_of_its_own_menu_only() {
        let c = cfg(&[(
            "text",
            &[
                ("y x", "copy_full"),
                ("y n", "copy_parent"),
                ("y p", "noop"),
                (", s", "sort_size"),
            ],
        )]);
        let k = CoreKeys::from_config(KeyScheme::Vim, &c);
        let copy = items(&k, CoreContext::Text, CoreMenu::Copy);
        assert!(
            copy.contains(&('x', CoreAction::CopyPath(CopyKind::Full))),
            "{copy:?}"
        );
        assert!(
            copy.contains(&('n', CoreAction::CopyPath(CopyKind::Parent))),
            "{copy:?}"
        );
        assert!(!copy.iter().any(|(ch, _)| *ch == 'p'), "{copy:?}");
        assert_eq!(
            copy.len(),
            6,
            "one added, one replaced, one removed: {copy:?}"
        );
        // The label follows the action.
        let m = k.menu(CoreContext::Text, CoreMenu::Copy).unwrap();
        let x = m
            .items
            .iter()
            .find(|it| it.key == KeyPress::ch('x'))
            .unwrap();
        assert_eq!(x.label, CoreMsg::WkFull);
        let opts = items(&k, CoreContext::Text, CoreMenu::Options);
        assert!(
            opts.contains(&('s', CoreAction::SortSet(SortKey::Size))),
            "{opts:?}"
        );
        // Other contexts keep the defaults.
        let d = CoreKeys::defaults(KeyScheme::Vim);
        for other in [
            CoreContext::Document,
            CoreContext::Image,
            CoreContext::Canvas,
        ] {
            assert_eq!(
                items(&k, other, CoreMenu::Copy),
                items(&d, other, CoreMenu::Copy),
                "{other:?}"
            );
            assert_eq!(
                items(&k, other, CoreMenu::Options),
                items(&d, other, CoreMenu::Options),
                "{other:?}"
            );
        }
    }

    #[test]
    fn wide_and_cjk_keys_in_a_chord_do_not_panic_and_are_handled_like_any_other_key() {
        let c = cfg(&[
            (
                "text",
                &[
                    ("y あ", "copy_full"),
                    ("あ", "copy_full"),
                    ("ｙ x", "copy_full"),
                    (", 漢", "toggle_wrap"),
                ],
            ),
            ("global", &[("y い", "copy_name")]),
        ]);
        for scheme in [KeyScheme::Vim, KeyScheme::Less] {
            let k = CoreKeys::from_config(scheme, &c);
            let copy = items(&k, CoreContext::Text, CoreMenu::Copy);
            assert!(copy.contains(&('あ', CoreAction::CopyPath(CopyKind::Full))));
            // The single `あ` is an ordinary key, not a menu item.
            assert_eq!(copy.iter().filter(|(ch, _)| *ch == 'あ').count(), 1);
            assert!(copy.contains(&('い', CoreAction::CopyPath(CopyKind::Name))));
            // A full-width prefix is not the `y` key: it opens no menu and is ignored.
            assert!(!copy.iter().any(|(ch, _)| *ch == 'x'));
            let opts = items(&k, CoreContext::Text, CoreMenu::Options);
            assert!(opts.contains(&('漢', CoreAction::ToggleWrap)));
        }
    }

    #[test]
    fn a_chord_in_global_changes_the_shared_menu_and_every_copy_of_it() {
        let c = cfg(&[
            ("global", &[("y x", "copy_full"), (", z", "toggle_wrap")]),
            // The context's own copy is made after the global entry and keeps it.
            ("image", &[("y q", "copy_name")]),
        ]);
        let k = CoreKeys::from_config(KeyScheme::Vim, &c);
        for ctx in [
            CoreContext::Text,
            CoreContext::Document,
            CoreContext::Image,
            CoreContext::Canvas,
        ] {
            let copy = items(&k, ctx, CoreMenu::Copy);
            assert!(
                copy.contains(&('x', CoreAction::CopyPath(CopyKind::Full))),
                "{ctx:?}"
            );
        }
        for ctx in CoreContext::ALL {
            let opts = items(&k, ctx, CoreMenu::Options);
            assert!(opts.contains(&('z', CoreAction::ToggleWrap)), "{ctx:?}");
        }
        let image = items(&k, CoreContext::Image, CoreMenu::Copy);
        assert!(image.contains(&('q', CoreAction::CopyPath(CopyKind::Name))));
        assert!(!items(&k, CoreContext::Text, CoreMenu::Copy)
            .iter()
            .any(|(ch, _)| *ch == 'q'));
        // `y` in a table opens another menu, which a global `y x` does not touch.
        assert_eq!(
            items(&k, CoreContext::Table, CoreMenu::TableCopy),
            items(
                &CoreKeys::defaults(KeyScheme::Vim),
                CoreContext::Table,
                CoreMenu::TableCopy
            )
        );
    }

    #[test]
    fn the_y_of_a_table_is_the_table_menu_for_a_chord() {
        let c = cfg(&[("table", &[("y x", "copy_name"), ("y c", "noop")])]);
        let k = CoreKeys::from_config(KeyScheme::Vim, &c);
        let t = items(&k, CoreContext::Table, CoreMenu::TableCopy);
        assert!(
            t.contains(&('x', CoreAction::CopyPath(CopyKind::Name))),
            "{t:?}"
        );
        assert!(!t.iter().any(|(ch, _)| *ch == 'c'), "{t:?}");
    }

    #[test]
    fn a_chord_whose_prefix_opens_no_menu_there_is_ignored() {
        let d = CoreKeys::defaults(KeyScheme::Vim);
        for (table, key) in [
            ("text", "z x"),     // `z` opens nothing
            ("text", "space x"), // the Space menu lists its operations itself
            ("canvas", "space x"),
            ("text", "y x y"), // three keys
            ("text", "y"),     // a key, not a chord: `y` stays the menu key
        ] {
            let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[(table, &[(key, "copy_full")])]));
            for c in CoreContext::ALL {
                for m in [CoreMenu::Copy, CoreMenu::TableCopy, CoreMenu::Options] {
                    assert_eq!(k.menu(c, m), d.menu(c, m), "{table} {key} {c:?} {m:?}");
                }
            }
        }
        // A context that took `y` for an action has no `y` menu to change (`y` sorts before
        // `y x`, so the action is bound first); the default is restored afterwards.
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[("text", &[("y", "copy_full"), ("y x", "copy_name")])]),
        );
        assert_eq!(
            k.menu(CoreContext::Text, CoreMenu::Copy),
            d.menu(CoreContext::Text, CoreMenu::Copy)
        );
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('y')),
            CoreResolution::Menu(CoreMenu::Copy)
        );
    }

    #[test]
    fn the_old_preview_text_name_edits_the_text_and_the_document_menus() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[("preview_text", &[("y x", "copy_full")])]),
        );
        for c in [CoreContext::Text, CoreContext::Document] {
            let copy = items(&k, c, CoreMenu::Copy);
            assert!(
                copy.contains(&('x', CoreAction::CopyPath(CopyKind::Full))),
                "{c:?}"
            );
        }
        assert!(!items(&k, CoreContext::Image, CoreMenu::Copy)
            .iter()
            .any(|(ch, _)| *ch == 'x'));
    }

    #[test]
    fn a_host_only_action_name_in_a_chord_leaves_the_menu_as_it_was() {
        let d = CoreKeys::defaults(KeyScheme::Vim);
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[("text", &[("y x", "refresh"), ("y f", "no_such_action")])]),
        );
        assert_eq!(
            k.menu(CoreContext::Text, CoreMenu::Copy),
            d.menu(CoreContext::Text, CoreMenu::Copy)
        );
    }

    #[test]
    fn the_menu_actions_have_names_and_labels() {
        for a in [
            CoreAction::CopyCodeBlock,
            CoreAction::CyclePathStyle,
            CoreAction::ToggleWrap,
        ] {
            assert_eq!(CoreAction::from_name(&a.name()), Some(a));
            assert_ne!(menu_label(a), CoreMsg::Empty, "{a:?}");
        }
        assert_eq!(menu_label(CoreAction::Quit), CoreMsg::Empty);
    }

    #[test]
    fn menu_edits_work_on_the_set_directly() {
        let mut s = default_menus();
        let kp = KeyPress::ch('x');
        assert!(s
            .edit(
                MenuScope::Context(CoreContext::Text),
                CoreMenu::Copy,
                kp,
                Some((CoreAction::Quit, CoreMsg::Empty))
            )
            .is_ok());
        assert_eq!(
            s.get(CoreContext::Text, CoreMenu::Copy).unwrap().find(kp),
            Some(CoreAction::Quit)
        );
        assert_eq!(
            s.get(CoreContext::Image, CoreMenu::Copy).unwrap().find(kp),
            None
        );
        // No menu to change: the table menu has no copy in the text context's options.
        assert!(s
            .edit(
                MenuScope::Context(CoreContext::Text),
                CoreMenu::Actions,
                kp,
                None
            )
            .is_err());
    }
}

#[cfg(test)]
mod host_op_tests {
    use super::*;

    fn cfg(tables: &[(&str, &[(&str, &str)])]) -> KeysFileConfig {
        KeysFileConfig {
            surfaces: tables
                .iter()
                .map(|(n, es)| {
                    (
                        n.to_string(),
                        es.iter()
                            .map(|(k, a)| (k.to_string(), a.to_string()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }

    fn op(k: &CoreKeys, c: CoreContext, key: char) -> Option<HostOp> {
        k.host_op(c, KeyPress::ch(key))
    }

    #[test]
    fn host_op_names_round_trip_and_are_distinct_from_the_core_names() {
        let mut seen = std::collections::HashSet::new();
        for o in HostOp::ALL {
            assert_eq!(HostOp::from_name(o.name()), Some(o), "{}", o.name());
            assert!(seen.insert(o.name()), "duplicate {}", o.name());
            assert_eq!(
                CoreAction::from_name(o.name()),
                None,
                "{} is a core action name too",
                o.name()
            );
        }
        assert_eq!(
            HostOp::from_name("toggle_markdown_raw"),
            Some(HostOp::CycleRepresentation)
        );
        assert_eq!(HostOp::from_name("quit"), None);
        assert_eq!(HostOp::from_name("tab_goto"), None);
    }

    #[test]
    fn every_mnemonic_name_is_a_real_operation_and_the_table_is_complete() {
        for n in MNEMONIC_NAMES {
            assert!(mnemonic_of_name(n).is_some(), "{n}");
        }
        // The shared operations: the path style, wrapping and the focused item.
        assert_eq!(
            mnemonic_of_name(&CoreAction::CyclePathStyle.name()),
            Some('p')
        );
        assert_eq!(mnemonic_of_name(&CoreAction::ToggleWrap.name()), Some('w'));
        assert_eq!(
            mnemonic_of_name(HostOp::ToggleFocusedItem.name()),
            Some('t')
        );
        // Nothing else of the shared operations has one.
        for a in CoreAction::all() {
            let n = a.name();
            if !matches!(n.as_str(), "cycle_path_style" | "toggle_wrap") {
                assert_eq!(mnemonic_of_name(&n), None, "{n}");
            }
        }
        for o in HostOp::ALL {
            if o != HostOp::ToggleFocusedItem {
                assert_eq!(mnemonic_of_name(o.name()), None, "{}", o.name());
            }
        }
        assert_eq!(mnemonic_of_name("no_such_operation"), None);
    }

    #[test]
    fn the_defaults_bind_the_host_operations_but_the_standalone_resolution_stays_unbound() {
        let k = CoreKeys::defaults(KeyScheme::Vim);
        for (c, key, want) in [
            (CoreContext::Text, 'r', HostOp::Refresh),
            (CoreContext::Text, '/', HostOp::SearchStart),
            (CoreContext::Document, 'n', HostOp::SearchNext),
            (CoreContext::Text, 'R', HostOp::CycleRepresentation),
            (CoreContext::Image, 'R', HostOp::ImageReturnToDiff),
            (CoreContext::Table, 'R', HostOp::CycleRepresentation),
            (CoreContext::Canvas, 'Y', HostOp::PreviewCopySelectionRef),
            (CoreContext::Table, 'm', HostOp::MarkSet),
            (CoreContext::Image, '\'', HostOp::MarkJump),
            (CoreContext::Canvas, 'p', HostOp::OpenGitView),
            (CoreContext::Text, '!', HostOp::GitLaunchTool),
            (CoreContext::Text, 'v', HostOp::PreviewEnterVisual),
            (CoreContext::Text, 'V', HostOp::PreviewEnterVisualLine),
        ] {
            assert_eq!(op(&k, c, key), Some(want), "{c:?} {key}");
            assert_eq!(
                k.resolve(c, KeyPress::ch(key)),
                CoreResolution::Unbound,
                "{c:?} {key}: the standalone app has nothing to run"
            );
        }
        assert_eq!(
            k.host_op(CoreContext::Text, KeyPress::ctrl_ch('t')),
            Some(HostOp::OpenLinkNewTab)
        );
        assert_eq!(
            k.host_op(CoreContext::Image, KeyPress::ctrl_ch('n')),
            Some(HostOp::PreviewFileNext)
        );
        // Searching is for the text and table contexts, not a picture or a canvas.
        assert_eq!(op(&k, CoreContext::Image, '/'), None);
        assert_eq!(op(&k, CoreContext::Canvas, 'n'), None);
        // A core key is not a host operation.
        assert_eq!(op(&k, CoreContext::Text, 'j'), None);
        assert_eq!(op(&k, CoreContext::Text, 'q'), None);
    }

    #[test]
    fn a_host_operation_name_is_read_from_a_context_table() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[(
                "text",
                &[("x", "search_start"), ("X", "toggle_markdown_raw")],
            )]),
        );
        assert_eq!(op(&k, CoreContext::Text, 'x'), Some(HostOp::SearchStart));
        assert_eq!(
            op(&k, CoreContext::Text, 'X'),
            Some(HostOp::CycleRepresentation)
        );
        assert_eq!(
            op(&k, CoreContext::Table, 'x'),
            None,
            "only the text context"
        );
        assert_eq!(
            op(&k, CoreContext::Text, 'r'),
            Some(HostOp::Refresh),
            "the default key stays"
        );
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('x')),
            CoreResolution::Unbound
        );
    }

    #[test]
    fn a_host_operation_taking_an_inherited_key_hides_what_it_inherited() {
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("text", &[("j", "refresh")])]));
        assert_eq!(op(&k, CoreContext::Text, 'j'), Some(HostOp::Refresh));
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('j')),
            CoreResolution::Unbound,
            "`j` no longer moves"
        );
        assert_eq!(
            k.resolve(CoreContext::Document, KeyPress::ch('j')),
            CoreResolution::Action(CoreAction::Navigate(Motion::Down))
        );
    }

    #[test]
    fn noop_frees_a_host_key_and_a_core_action_replaces_it() {
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("text", &[("r", "noop")])]));
        assert_eq!(op(&k, CoreContext::Text, 'r'), None);
        assert_eq!(op(&k, CoreContext::Image, 'r'), Some(HostOp::Refresh));
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("text", &[("r", "page_next")])]));
        assert_eq!(op(&k, CoreContext::Text, 'r'), None);
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('r')),
            CoreResolution::Action(CoreAction::PageNext)
        );
    }

    #[test]
    fn a_host_operation_cannot_take_a_menu_key_or_a_global_key() {
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[(
                "text",
                &[("y", "refresh"), ("space", "refresh"), ("Q", "refresh")],
            )]),
        );
        assert_eq!(op(&k, CoreContext::Text, 'y'), None);
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('y')),
            CoreResolution::Menu(CoreMenu::Copy)
        );
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch(' ')),
            CoreResolution::Menu(CoreMenu::Actions)
        );
        assert_eq!(op(&k, CoreContext::Text, 'Q'), None);
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('Q')),
            CoreResolution::Action(CoreAction::Quit)
        );
    }

    #[test]
    fn a_global_table_can_bind_a_host_operation_and_a_context_cannot_take_the_key() {
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("global", &[("x", "mark_set")])]));
        for c in CoreContext::ALL {
            assert_eq!(op(&k, c, 'x'), Some(HostOp::MarkSet), "{c:?}");
        }
        let k = CoreKeys::from_config(
            KeyScheme::Vim,
            &cfg(&[
                ("global", &[("x", "mark_set")]),
                ("text", &[("x", "page_next")]),
            ]),
        );
        // The same rule as for a core `global` key: a context cannot take it for another action.
        for c in CoreContext::ALL {
            assert_eq!(op(&k, c, 'x'), Some(HostOp::MarkSet), "{c:?}");
        }
        assert_eq!(
            k.resolve(CoreContext::Text, KeyPress::ch('x')),
            CoreResolution::Unbound
        );
    }

    #[test]
    fn a_chord_entry_of_a_host_operation_is_ignored() {
        let before = CoreKeys::defaults(KeyScheme::Vim);
        let k = CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("text", &[("y x", "refresh")])]));
        assert_eq!(
            format!("{:?}", k.menu(CoreContext::Text, CoreMenu::Copy)),
            format!("{:?}", before.menu(CoreContext::Text, CoreMenu::Copy))
        );
    }

    #[test]
    fn every_host_operation_can_be_written_in_a_config() {
        for o in HostOp::ALL {
            let k =
                CoreKeys::from_config(KeyScheme::Vim, &cfg(&[("document", &[("x", o.name())])]));
            assert_eq!(
                k.host_op(CoreContext::Document, KeyPress::ch('x')),
                Some(o),
                "{}",
                o.name()
            );
        }
    }
}
