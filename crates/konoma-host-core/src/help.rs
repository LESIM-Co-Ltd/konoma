//! The `?` help and the footer hints of the key layers 1 to 3, built once for every host.
//!
//! konoma and the standalone plugin app both list "what the keys do" from the same declaration:
//! a row names the operations it describes, and what it prints (the key notation, the order of the
//! keys, which section it lands in, the words) is worked out here from the actual bindings
//! ([`KeyLookup`]), so a rebound key changes the label and an unbound key drops its line. What a
//! host knows and this module cannot (an operation that does not act right now, a word that
//! changes with the open file such as page / sheet / slide) comes in as [`HelpFacts`].

use crossterm::event::KeyCode;

use crate::help_msg::CoreMsg;
use crate::keymap::{
    CoreAction, CoreBinding, CoreContext, CoreKeys, CoreMenu, CoreResolution, KeyPress, Motion,
};
use crate::lang::Lang;

/// What a help row or a footer token names: an operation of the shared layers, the fixed `Esc`
/// (a host's "cancel"), or the key that opens a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpOp {
    Act(CoreAction),
    /// The fixed `Esc` key (never rebindable).
    Cancel,
    /// The key that opens a menu (`,` for the options).
    Menu(CoreMenu),
}

/// Which layer of the keymap a key comes from, for the help's sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLayer {
    /// Layer 0: every context inherits it.
    Global,
    /// Layer 1: movement every context with a cursor shares.
    Basic,
    /// The context's own table and the table of its view type (layers 2 and 3).
    View,
}

/// The section a row lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreSec {
    View,
    Move,
    Launch,
    Global,
}

/// How a host answers "which keys run this, and where does a key come from" for one context.
pub trait KeyLookup {
    /// Every key that runs `op` (config applied), in the order they are listed.
    fn keys_of(&self, op: HelpOp) -> Vec<KeyPress>;
    /// Where `key` is defined (`None` = not bound).
    fn layer_of(&self, key: KeyPress) -> Option<KeyLayer>;
}

/// What the builders cannot know by themselves.
#[derive(Debug, Clone, Default)]
pub struct HelpFacts {
    /// Operations that do not act right now: their rows and tokens are left out.
    pub off: Vec<HelpOp>,
    /// The footer label an operation has right now (`None` = it names none). An operation that is
    /// not listed has its default label ([`default_label`]).
    pub label: Vec<(CoreAction, Option<CoreMsg>)>,
    /// The same for the help description ([`default_help`]).
    pub help: Vec<(CoreAction, Option<CoreMsg>)>,
}

impl HelpFacts {
    fn is_on(&self, op: HelpOp) -> bool {
        !self.off.contains(&op)
    }

    fn label_of(&self, ctx: CoreContext, op: HelpOp) -> Option<CoreMsg> {
        let HelpOp::Act(a) = op else {
            return None;
        };
        match self.label.iter().find(|(x, _)| *x == a) {
            Some((_, m)) => *m,
            None => default_label(ctx, a),
        }
    }

    fn help_of(&self, ctx: CoreContext, op: HelpOp) -> Option<CoreMsg> {
        let HelpOp::Act(a) = op else {
            return None;
        };
        match self.help.iter().find(|(x, _)| *x == a) {
            Some((_, m)) => *m,
            None => default_help(ctx, a),
        }
    }
}

/// The footer label of `a` before the state is looked at.
pub fn default_label(_ctx: CoreContext, a: CoreAction) -> Option<CoreMsg> {
    use CoreAction as A;
    use CoreMsg as M;
    match a {
        A::Navigate(m) => match m {
            Motion::Up | Motion::Down => Some(M::Scroll),
            Motion::Top | Motion::Bottom => Some(M::HintEnds),
            Motion::PageUp | Motion::PageDown => Some(M::HintPage),
            _ => None,
        },
        A::Quit => Some(M::HintQuit),
        A::ToggleHelp => Some(M::HintHelp),
        A::PreviewBack => Some(M::GitBack),
        A::ImageZoomIn | A::ImageZoomOut => Some(M::Zoom),
        A::ImageZoomReset => Some(M::HintFit),
        A::PageNext | A::PagePrev => Some(M::HintPage),
        A::ToggleOutline => Some(M::HintOutline),
        A::RequestEdit => Some(M::HintEdit),
        A::OpenExternal => Some(M::HintOpenApp),
        _ => None,
    }
}

/// The help description of `a` before the state is looked at.
pub fn default_help(ctx: CoreContext, a: CoreAction) -> Option<CoreMsg> {
    use CoreAction as A;
    use CoreMsg as M;
    match a {
        A::Navigate(m) => Some(match m {
            Motion::Up | Motion::Down => M::Scroll,
            Motion::Top | Motion::Bottom => M::TopBottom,
            Motion::PageUp | Motion::PageDown => M::HintPage,
            Motion::HalfUp | Motion::HalfDown => M::HelpHalfPage,
            Motion::Left | Motion::Right => M::HScroll,
            Motion::LineHome | Motion::LineEnd => M::LineStartEnd,
        }),
        A::Quit => Some(M::Quit),
        A::ToggleHelp => Some(M::ToggleHelp),
        A::PreviewBack => Some(M::BackToTree),
        A::ImageZoomIn | A::ImageZoomOut => Some(M::Zoom),
        A::ImageZoomReset => Some(M::ResetFit),
        A::PageNext | A::PagePrev => Some(M::HintPage),
        A::ToggleOutline => Some(M::HintOutline),
        A::RequestEdit => Some(
            if matches!(ctx, CoreContext::Text | CoreContext::Document) {
                M::EditExternalEnv
            } else {
                M::EditExternal
            },
        ),
        A::OpenExternal => Some(M::OpenAppHelp),
        _ => None,
    }
}

// =============================================================================
// Key notation.
// =============================================================================

/// A key as the `?` help writes it: `j`, `Space`, `Enter`, `Ctrl-t`, `PageDown`, `⇧Tab`.
pub fn help_key(kp: KeyPress) -> String {
    let base = match kp.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "⇧Tab".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        other => format!("{other:?}"),
    };
    if kp.ctrl {
        format!("Ctrl-{base}")
    } else {
        base
    }
}

/// `s` padded with spaces on the right to `width` terminal cells (not characters: a full-width
/// key is two cells). A string that is already as wide is returned as it is.
pub fn pad_cells(s: &str, width: usize) -> String {
    let w = unicode_width::UnicodeWidthStr::width(s);
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// A key as the footer writes it, shorter than the help: `C-t`, `PgDn`, `⇧Tab`. `short_enter`
/// writes `Enter` as `↵` (the preview screens do).
pub fn footer_key(kp: KeyPress, short_enter: bool) -> String {
    let base = match kp.code {
        KeyCode::Enter if short_enter => "↵".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        _ => {
            return if kp.ctrl {
                format!("C-{}", help_key(KeyPress { ctrl: false, ..kp }))
            } else {
                help_key(kp)
            }
        }
    };
    if kp.ctrl {
        format!("C-{base}")
    } else {
        base
    }
}

/// The order keys are listed in: plain characters (by code point, so `j` before `k`, `J` before
/// `j`), then Ctrl-chords, then named keys.
pub fn display_order(k: KeyPress) -> (u8, u32) {
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

/// The keys of an operation group as columns. A group of exactly two operations (`next` /
/// `prev`) is zipped (`J K`, `PageDown PageUp`), anything else is one key per column. The flag
/// says whether it was zipped. Generic over the operation type so a host with more operations
/// than the shared layers uses the same rule.
pub fn zip_columns<T: Copy>(
    acts: &[T],
    keys_of: impl Fn(T) -> Vec<KeyPress>,
) -> (Vec<Vec<KeyPress>>, bool) {
    if let [a, b] = acts {
        let ka = keys_of(*a);
        let kb = keys_of(*b);
        let cols = (0..ka.len().max(kb.len()))
            .map(|i| {
                let mut col = Vec::new();
                col.extend(ka.get(i).copied());
                col.extend(kb.get(i).copied());
                col
            })
            .collect();
        return (cols, true);
    }
    let cols = acts
        .iter()
        .flat_map(|a| keys_of(*a))
        .map(|k| vec![k])
        .collect();
    (cols, false)
}

/// [`zip_columns`] over a [`KeyLookup`].
pub fn columns(lookup: &dyn KeyLookup, acts: &[HelpOp]) -> (Vec<Vec<KeyPress>>, bool) {
    zip_columns(acts, |a| lookup.keys_of(a))
}

// =============================================================================
// The help rows.
// =============================================================================

/// One help row of the shared layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreRow {
    /// `j / k` and the arrows.
    Scroll,
    /// `g / G`.
    Ends,
    /// `h / l` in a text.
    HScroll,
    /// `0 / $` in a text.
    LineEnds,
    /// `h j k l` in a table: between cells.
    TableMove,
    /// `0 / $` in a table: first / last column.
    TableCols,
    /// `h j k l` in an image: pan.
    Pan,
    /// `+ / -`.
    Zoom,
    /// `=`.
    ZoomReset,
    /// `J / K`.
    PageTurn,
    /// `O`.
    Outline,
    /// `Ctrl-f / Ctrl-b` and `PageDown / PageUp`.
    PageKeys,
    /// `Ctrl-d / Ctrl-u`.
    HalfKeys,
    /// `e`.
    Edit,
    /// `o`.
    Open,
    /// `q` and `Esc`.
    Back,
    /// `?`.
    Help,
    /// `Q`.
    Quit,
}

const SCROLL: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::Down)),
    HelpOp::Act(CoreAction::Navigate(Motion::Up)),
];
const ENDS: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::Top)),
    HelpOp::Act(CoreAction::Navigate(Motion::Bottom)),
];
const HSCROLL: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::Left)),
    HelpOp::Act(CoreAction::Navigate(Motion::Right)),
];
const LINE_ENDS: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::LineHome)),
    HelpOp::Act(CoreAction::Navigate(Motion::LineEnd)),
];
const HJKL: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::Left)),
    HelpOp::Act(CoreAction::Navigate(Motion::Down)),
    HelpOp::Act(CoreAction::Navigate(Motion::Up)),
    HelpOp::Act(CoreAction::Navigate(Motion::Right)),
];
const PAGE_KEYS: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::PageDown)),
    HelpOp::Act(CoreAction::Navigate(Motion::PageUp)),
];
const HALF_KEYS: &[HelpOp] = &[
    HelpOp::Act(CoreAction::Navigate(Motion::HalfDown)),
    HelpOp::Act(CoreAction::Navigate(Motion::HalfUp)),
];
const ZOOM: &[HelpOp] = &[
    HelpOp::Act(CoreAction::ImageZoomIn),
    HelpOp::Act(CoreAction::ImageZoomOut),
];
const ZOOM_RESET: &[HelpOp] = &[HelpOp::Act(CoreAction::ImageZoomReset)];
const PAGE_TURN: &[HelpOp] = &[
    HelpOp::Act(CoreAction::PageNext),
    HelpOp::Act(CoreAction::PagePrev),
];
const OUTLINE: &[HelpOp] = &[HelpOp::Act(CoreAction::ToggleOutline)];
const EDIT: &[HelpOp] = &[HelpOp::Act(CoreAction::RequestEdit)];
const OPEN: &[HelpOp] = &[HelpOp::Act(CoreAction::OpenExternal)];
const BACK: &[HelpOp] = &[HelpOp::Act(CoreAction::PreviewBack), HelpOp::Cancel];
const HELP: &[HelpOp] = &[HelpOp::Act(CoreAction::ToggleHelp)];
const QUIT: &[HelpOp] = &[HelpOp::Act(CoreAction::Quit)];

impl CoreRow {
    /// The operations the row describes (one group).
    pub fn ops(self) -> &'static [HelpOp] {
        match self {
            CoreRow::Scroll => SCROLL,
            CoreRow::Ends => ENDS,
            CoreRow::HScroll => HSCROLL,
            CoreRow::LineEnds | CoreRow::TableCols => LINE_ENDS,
            CoreRow::TableMove | CoreRow::Pan => HJKL,
            CoreRow::Zoom => ZOOM,
            CoreRow::ZoomReset => ZOOM_RESET,
            CoreRow::PageTurn => PAGE_TURN,
            CoreRow::Outline => OUTLINE,
            CoreRow::PageKeys => PAGE_KEYS,
            CoreRow::HalfKeys => HALF_KEYS,
            CoreRow::Edit => EDIT,
            CoreRow::Open => OPEN,
            CoreRow::Back => BACK,
            CoreRow::Help => HELP,
            CoreRow::Quit => QUIT,
        }
    }

    /// Keys that cannot be rebound, written after the generated ones.
    fn lit(self) -> Option<&'static str> {
        match self {
            CoreRow::Scroll => Some("↑ ↓"),
            CoreRow::HScroll => Some("← →"),
            CoreRow::TableMove | CoreRow::Pan => Some("arrows"),
            _ => None,
        }
    }

    /// The description the row always has (`None` = the first operation's own).
    fn fixed_desc(self) -> Option<CoreMsg> {
        match self {
            CoreRow::Pan => Some(CoreMsg::PanHint),
            CoreRow::TableMove => Some(CoreMsg::TableMoveHelp),
            CoreRow::TableCols => Some(CoreMsg::TableColsHelp),
            _ => None,
        }
    }

    /// Rows that always close the list, whatever key they have.
    fn forced_sec(self) -> Option<CoreSec> {
        matches!(self, CoreRow::Help | CoreRow::Quit).then_some(CoreSec::Global)
    }
}

/// The rows of `ctx`'s help in the order they are listed (a host lists its own rows between
/// them; the order of these stays).
pub fn rows_of(ctx: CoreContext) -> &'static [CoreRow] {
    use CoreRow as R;
    match ctx {
        CoreContext::Text | CoreContext::Document => &[
            R::Scroll,
            R::Ends,
            R::HScroll,
            R::LineEnds,
            R::PageTurn,
            R::Outline,
            R::Edit,
            R::Open,
            R::PageKeys,
            R::HalfKeys,
            R::Back,
            R::Help,
            R::Quit,
        ],
        CoreContext::Image => &[
            R::Zoom,
            R::ZoomReset,
            R::Pan,
            R::PageTurn,
            R::Outline,
            R::Edit,
            R::Open,
            R::Back,
            R::Help,
            R::Quit,
        ],
        CoreContext::Table => &[
            R::TableMove,
            R::Ends,
            R::TableCols,
            R::PageTurn,
            R::Outline,
            R::PageKeys,
            R::HalfKeys,
            R::Edit,
            R::Open,
            R::Back,
            R::Help,
            R::Quit,
        ],
        CoreContext::Canvas => &[
            R::PageTurn,
            R::Outline,
            R::Edit,
            R::Open,
            R::Back,
            R::Help,
            R::Quit,
        ],
    }
}

/// The title of the section of the context's own operations.
pub fn view_title(ctx: CoreContext) -> CoreMsg {
    match ctx {
        CoreContext::Text | CoreContext::Document => CoreMsg::PreviewTextMarkdown,
        CoreContext::Image => CoreMsg::PreviewImage,
        CoreContext::Table => CoreMsg::PreviewTable,
        CoreContext::Canvas => CoreMsg::PreviewCanvas,
    }
}

/// A rendered help row, with what it was made from.
#[derive(Debug, Clone)]
pub struct CoreRowOut {
    pub sec: CoreSec,
    pub keys_text: String,
    pub desc: String,
    pub keys: Vec<KeyPress>,
    pub ops: Vec<HelpOp>,
}

/// Whether the operation starts something outside the view (layer 3: `e`, `o`).
fn is_launch(op: HelpOp) -> bool {
    matches!(
        op,
        HelpOp::Act(CoreAction::RequestEdit | CoreAction::OpenExternal)
    )
}

/// One row as the help writes it, or `None` when nothing is left to list (no operation acts,
/// no key is bound, the operation has no description).
pub fn render_row(
    row: CoreRow,
    ctx: CoreContext,
    facts: &HelpFacts,
    lookup: &dyn KeyLookup,
    lang: Lang,
) -> Option<CoreRowOut> {
    let acts: Vec<HelpOp> = row
        .ops()
        .iter()
        .copied()
        .filter(|a| facts.is_on(*a))
        .collect();
    if acts.is_empty() {
        return None;
    }
    let (cols, zipped) = columns(lookup, &acts);
    if cols.is_empty() {
        return None;
    }
    let lead = acts[0];
    let first_key = cols.first().and_then(|c| c.first()).copied();
    let keys: Vec<KeyPress> = cols.iter().flatten().copied().collect();
    // A key is written once per row even if two columns hold it.
    let mut seen: Vec<String> = Vec::new();
    let rendered: Vec<String> = cols
        .iter()
        .map(|c| {
            let mut one: Vec<String> = Vec::new();
            for k in c {
                let s = help_key(*k);
                if !seen.contains(&s) {
                    seen.push(s.clone());
                    one.push(s);
                }
            }
            one.join(" / ")
        })
        .filter(|s| !s.is_empty())
        .collect();
    let mut keys_text = rendered.join(if zipped { "  ·  " } else { " / " });
    if let Some(l) = row.lit() {
        keys_text = format!("{keys_text} / {l}");
    }
    let desc = match row.fixed_desc() {
        Some(m) => m,
        None => facts.help_of(ctx, lead)?,
    };
    let sec = row.forced_sec().unwrap_or_else(|| {
        if is_launch(lead) {
            CoreSec::Launch
        } else {
            match first_key.and_then(|k| lookup.layer_of(k)) {
                Some(KeyLayer::Global) => CoreSec::Global,
                Some(KeyLayer::Basic) => CoreSec::Move,
                _ => CoreSec::View,
            }
        }
    });
    Some(CoreRowOut {
        sec,
        keys_text,
        desc: desc.tr(lang).to_string(),
        keys,
        ops: acts,
    })
}

/// One section of the help: a heading and `(keys, description)` rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

/// The sections of the help for layers 1 to 3 of `ctx`: the context's own operations, movement,
/// the launch verbs, and what is true everywhere (help, quit). A section with no row is left out.
pub fn core_sections(
    ctx: CoreContext,
    facts: &HelpFacts,
    lookup: &dyn KeyLookup,
    lang: Lang,
) -> Vec<CoreSection> {
    let mut by: Vec<(CoreSec, Vec<(String, String)>)> = Vec::new();
    for row in rows_of(ctx) {
        if let Some(o) = render_row(*row, ctx, facts, lookup, lang) {
            match by.iter_mut().find(|(s, _)| *s == o.sec) {
                Some((_, v)) => v.push((o.keys_text, o.desc)),
                None => by.push((o.sec, vec![(o.keys_text, o.desc)])),
            }
        }
    }
    let mut out = Vec::new();
    for (sec, title) in [
        (CoreSec::View, view_title(ctx)),
        (CoreSec::Move, CoreMsg::HelpMove),
        (CoreSec::Launch, CoreMsg::HelpLaunch),
        (CoreSec::Global, CoreMsg::HelpGlobal),
    ] {
        if let Some(i) = by.iter().position(|(s, _)| *s == sec) {
            out.push(CoreSection {
                title: title.tr(lang).to_string(),
                rows: by.remove(i).1,
            });
        }
    }
    out
}

// =============================================================================
// The footer.
// =============================================================================

/// One footer entry of the shared layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFoot {
    /// `j/k:scroll`.
    Scroll,
    /// `g/G:ends`.
    Ends,
    /// `h/l:hscroll` (text that does not wrap).
    HScroll,
    /// `0/$:line ends` (text that does not wrap).
    LineEnds,
    /// `h/j/k/l:cell` (a table).
    Cell,
    /// `h/j/k/l:pan` (an image).
    Pan,
    /// `+/-:zoom`.
    Zoom,
    /// `=:fit`.
    ZoomReset,
    /// `J/K:page` (or sheet / slide).
    PageTurn,
    /// `O:outline` (or pages).
    Outline,
    /// `PgDn/PgUp:page`.
    PageKeys,
    /// `q:back`.
    Back,
    /// `?:help`.
    Help,
    /// `e:edit`.
    Edit,
    /// `o:open in app`.
    Open,
    /// `,:view` (the options menu).
    Options,
    /// `Q:quit`.
    Quit,
}

/// How an entry reads, before the state is looked at.
enum FootKind {
    Op(CoreAction),
    Group(&'static [HelpOp], CoreMsg),
    GroupOp(&'static [HelpOp]),
    Leader(CoreMenu, CoreMsg),
}

impl CoreFoot {
    /// The operations the entry names (a menu key names none).
    pub fn ops(self) -> Vec<HelpOp> {
        match self.kind() {
            FootKind::Op(a) => vec![HelpOp::Act(a)],
            FootKind::Group(acts, _) | FootKind::GroupOp(acts) => acts.to_vec(),
            FootKind::Leader(..) => Vec::new(),
        }
    }

    fn kind(self) -> FootKind {
        match self {
            CoreFoot::Scroll => FootKind::Group(SCROLL, CoreMsg::Scroll),
            CoreFoot::Ends => FootKind::Group(ENDS, CoreMsg::HintEnds),
            CoreFoot::HScroll => FootKind::Group(HSCROLL, CoreMsg::HintHscroll),
            CoreFoot::LineEnds => FootKind::Group(LINE_ENDS, CoreMsg::HintLineEnds),
            CoreFoot::Cell => FootKind::Group(HJKL, CoreMsg::HintCell),
            CoreFoot::Pan => FootKind::Group(HJKL, CoreMsg::HintPan),
            CoreFoot::Zoom => FootKind::Group(ZOOM, CoreMsg::Zoom),
            CoreFoot::ZoomReset => FootKind::Op(CoreAction::ImageZoomReset),
            CoreFoot::PageTurn => FootKind::GroupOp(PAGE_TURN),
            CoreFoot::Outline => FootKind::Op(CoreAction::ToggleOutline),
            CoreFoot::PageKeys => FootKind::Group(PAGE_KEYS, CoreMsg::HintPage),
            CoreFoot::Back => FootKind::Op(CoreAction::PreviewBack),
            CoreFoot::Help => FootKind::Op(CoreAction::ToggleHelp),
            CoreFoot::Edit => FootKind::Op(CoreAction::RequestEdit),
            CoreFoot::Open => FootKind::Op(CoreAction::OpenExternal),
            CoreFoot::Options => FootKind::Leader(CoreMenu::Options, CoreMsg::HintOptions),
            CoreFoot::Quit => FootKind::Op(CoreAction::Quit),
        }
    }
}

/// One footer token: `<keys>:<label>`, with the keys and operations it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreToken {
    pub text: String,
    pub keys: Vec<KeyPress>,
    pub ops: Vec<HelpOp>,
}

/// The keys of an operation group: gated, zipped when two, flattened and de-duplicated.
pub fn group_keys(lookup: &dyn KeyLookup, facts: &HelpFacts, acts: &[HelpOp]) -> Vec<KeyPress> {
    let enabled: Vec<HelpOp> = acts.iter().copied().filter(|a| facts.is_on(*a)).collect();
    let (cols, _) = columns(lookup, &enabled);
    let mut keys: Vec<KeyPress> = Vec::new();
    for kp in cols.into_iter().flatten() {
        if !keys.contains(&kp) {
            keys.push(kp);
        }
    }
    keys
}

/// The keys written for the footer, joined with `/`.
pub fn write_keys(keys: &[KeyPress], short_enter: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    for k in keys {
        let s = footer_key(*k, short_enter);
        if !parts.contains(&s) {
            parts.push(s);
        }
    }
    parts.join("/")
}

/// Render one entry, or `None` when it does not act, is unbound or names nothing.
pub fn render_foot(
    foot: CoreFoot,
    ctx: CoreContext,
    facts: &HelpFacts,
    lookup: &dyn KeyLookup,
    lang: Lang,
) -> Option<CoreToken> {
    let tok = |keys: Vec<KeyPress>, label: CoreMsg, ops: &[HelpOp]| -> Option<CoreToken> {
        (!keys.is_empty()).then(|| CoreToken {
            text: format!("{}:{}", write_keys(&keys, true), label.tr(lang)),
            keys,
            ops: ops.to_vec(),
        })
    };
    match foot.kind() {
        FootKind::Op(a) => {
            let op = HelpOp::Act(a);
            if !facts.is_on(op) {
                return None;
            }
            let label = facts.label_of(ctx, op)?;
            tok(group_keys(lookup, facts, &[op]), label, &[op])
        }
        FootKind::Group(acts, label) => {
            if !acts.iter().any(|a| facts.is_on(*a)) {
                return None;
            }
            tok(group_keys(lookup, facts, acts), label, acts)
        }
        FootKind::GroupOp(acts) => {
            if !acts.iter().any(|a| facts.is_on(*a)) {
                return None;
            }
            let label = facts.label_of(ctx, acts[0])?;
            tok(group_keys(lookup, facts, acts), label, acts)
        }
        FootKind::Leader(menu, label) => {
            let op = HelpOp::Menu(menu);
            if !facts.is_on(op) {
                return None;
            }
            tok(lookup.keys_of(op), label, &[])
        }
    }
}

/// The tokens of `plan` in order.
pub fn footer_tokens(
    plan: &[CoreFoot],
    ctx: CoreContext,
    facts: &HelpFacts,
    lookup: &dyn KeyLookup,
    lang: Lang,
) -> Vec<CoreToken> {
    plan.iter()
        .filter_map(|f| render_foot(*f, ctx, facts, lookup, lang))
        .collect()
}

/// The operations an entry names with the label it has (the `Space` menu lists operations in
/// footer order under these labels). A leader names no single operation.
pub fn foot_priority(
    foot: CoreFoot,
    ctx: CoreContext,
    facts: &HelpFacts,
) -> Vec<(HelpOp, Option<CoreMsg>)> {
    match foot.kind() {
        FootKind::Op(a) => vec![(HelpOp::Act(a), facts.label_of(ctx, HelpOp::Act(a)))],
        FootKind::Group(acts, label) => acts.iter().map(|a| (*a, Some(label))).collect(),
        FootKind::GroupOp(acts) => {
            let label = acts.first().and_then(|a| facts.label_of(ctx, *a));
            acts.iter().map(|a| (*a, label)).collect()
        }
        FootKind::Leader(..) => Vec::new(),
    }
}

/// The title of the `?` help overlay, from the keys that really act: `close` are the keys that
/// close it (the fixed `Esc` is always added), `scroll` the keys that scroll it (the clause is
/// left out when none does). A rebound key changes the title and an unbound one drops out of it.
pub fn help_title(close: &[KeyPress], scroll: &[KeyPress], lang: Lang) -> String {
    let mut close: Vec<KeyPress> = close.to_vec();
    let esc = KeyPress::key(KeyCode::Esc);
    if !close.contains(&esc) {
        close.push(esc);
    }
    let template = if scroll.is_empty() {
        CoreMsg::HelpTitleNoScroll
    } else {
        CoreMsg::HelpTitle
    };
    template
        .tr(lang)
        .replace("{close}", &write_keys(&close, false))
        .replace("{scroll}", &write_keys(scroll, false))
}

/// The keys that scroll an overlay: the `j` / `k` operations of `lookup`, gated by `facts`.
pub fn scroll_keys(lookup: &dyn KeyLookup, facts: &HelpFacts) -> Vec<KeyPress> {
    group_keys(lookup, facts, SCROLL)
}

/// The footer token of Ctrl-C, which always quits a standalone app (`C-c:quit`).
pub fn ctrl_c_token(lang: Lang) -> CoreToken {
    let key = KeyPress::ctrl_ch('c');
    CoreToken {
        text: format!("{}:{}", footer_key(key, true), CoreMsg::QuitApp.tr(lang)),
        keys: vec![key],
        ops: Vec::new(),
    }
}

// =============================================================================
// The lookup of the shared tables.
// =============================================================================

/// [`KeyLookup`] over the shared tables, for one context: what the standalone app uses.
pub struct CoreLookup<'a> {
    pub keys: &'a CoreKeys,
    pub ctx: CoreContext,
}

impl KeyLookup for CoreLookup<'_> {
    fn keys_of(&self, op: HelpOp) -> Vec<KeyPress> {
        match op {
            HelpOp::Act(a) => self.keys.keys_of(self.ctx, a),
            HelpOp::Cancel => vec![KeyPress::key(KeyCode::Esc)],
            HelpOp::Menu(m) => self.keys.menu_keys_of(self.ctx, m),
        }
    }

    fn layer_of(&self, key: KeyPress) -> Option<KeyLayer> {
        self.keys.layer_of_key(self.ctx, key)
    }
}

impl CoreKeys {
    /// Every key that runs `action` in `ctx` right now (config applied), in the order the lists
    /// show them ([`display_order`]). A key counts only if it *resolves* to the action, so a key
    /// shadowed by a more specific layer is not listed.
    pub fn keys_of(&self, ctx: CoreContext, action: CoreAction) -> Vec<KeyPress> {
        self.keys_resolving(
            ctx,
            CoreResolution::Action(action),
            CoreBinding::Run(action),
        )
    }

    /// Every key that opens `menu` in `ctx`.
    pub fn menu_keys_of(&self, ctx: CoreContext, menu: CoreMenu) -> Vec<KeyPress> {
        self.keys_resolving(ctx, CoreResolution::Menu(menu), CoreBinding::Menu(menu))
    }

    fn keys_resolving(
        &self,
        ctx: CoreContext,
        want: CoreResolution,
        binding: CoreBinding,
    ) -> Vec<KeyPress> {
        let mut out: Vec<KeyPress> = Vec::new();
        self.each_table(ctx, |t| {
            for (k, b) in t {
                if *b == binding && !out.contains(k) && self.resolve(ctx, *k) == want {
                    out.push(*k);
                }
            }
        });
        out.sort_by_key(|k| display_order(*k));
        out
    }

    /// Where `kp` is defined for `ctx` (`None` = not bound there). The lookup order of
    /// [`CoreKeys::resolve`].
    pub fn layer_of_key(&self, ctx: CoreContext, kp: KeyPress) -> Option<KeyLayer> {
        if let Some(b) = self.contexts.get(&ctx).and_then(|m| m.get(&kp)) {
            return (*b != CoreBinding::Unbound).then_some(KeyLayer::View);
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

    fn each_table(&self, ctx: CoreContext, mut f: impl FnMut(&crate::keymap::CoreTable)) {
        if let Some(m) = self.contexts.get(&ctx) {
            f(m);
        }
        for vt in ctx.view_types() {
            if let Some(m) = self.types.get(vt) {
                f(m);
            }
        }
        if ctx.inherits_basic() {
            f(&self.basic);
        }
        f(&self.global);
    }
}
