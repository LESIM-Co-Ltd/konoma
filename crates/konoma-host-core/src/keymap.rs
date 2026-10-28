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
            _ => return None,
        })
    }
}

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
        CoreBinding::Unbound => CoreResolution::Unbound,
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

/// The part of a context's own table that is shared. A host adds the keys only it has.
pub fn own_table(ctx: CoreContext) -> CoreTable {
    let mut m = back_table();
    m.extend(verbs_table());
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
        }
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

    /// Apply one (key string -> action string) to every target. Anything this module cannot
    /// honour is ignored: a chord (a menu entry), an invalid or fixed key, `tab_goto`, a name
    /// that is no core action.
    fn apply_one(&mut self, targets: &[Target], key: &str, action: &str) {
        let mut tokens = key.split_whitespace();
        let (Some(tok), None) = (tokens.next(), tokens.next()) else {
            return;
        };
        let Ok(kp) = KeyPress::parse(tok) else {
            return;
        };
        let action = if action == "noop" || action == "disabled" {
            None
        } else {
            match CoreAction::from_name(action) {
                Some(a) => Some(a),
                None => return,
            }
        };
        if is_fixed_key(kp) {
            return;
        }
        for &t in targets {
            self.set_key(t, kp, action);
        }
    }

    /// Bind `kp` to `action` in `target`'s own table, or unbind it there (`None`). Unbinding
    /// must also hide what the context inherits from layers 1-2, hence the explicit
    /// [`CoreBinding::Unbound`] mark; layer 0 still applies.
    fn set_key(&mut self, target: Target, kp: KeyPress, action: Option<CoreAction>) {
        match target {
            Target::Global => match action {
                Some(a) => {
                    self.global.insert(kp, run(a));
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
                        table.insert(kp, run(a));
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
                if let Some(CoreBinding::Run(_)) = self.local(ctx, k) {
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

    /// What `kp` does in `ctx`: the context's own table, its layer-2 tables, the basic layer,
    /// then `global`.
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
}
