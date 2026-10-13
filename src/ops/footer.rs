// The footer: which operations a context shows, in what order, with the key notation generated from
// the keymap. The order is the priority order: when the line is too narrow, `status::fit_tokens`
// cuts from the end, so the important operations come first.
//
// A token is `<keys>:<label>`. The keys are every key that runs the operation in that context
// (config applied, layers followed), joined with `/`. A token is shown only while its operation
// acts (`ops::gate`), so the footer never offers a key that would refuse.

use crate::app::{App, MdFocus};
use crate::i18n::{tr, Msg};
use crate::keymap::{Action, Context, KeyPress, LeaderId, Motion};
#[cfg(feature = "git")]
use crate::keymap::{Backend, Screen};
use crate::ops::decl::text;
use crate::ops::gate::action_enabled;
use crate::ops::keys::{columns, footer_key, short_enter};

/// One entry of a footer plan.
#[derive(Clone)]
pub enum Foot {
    /// The keys of one operation, labelled as `decl::text` says (skipped when it names none).
    Op(Action),
    /// The keys of one operation under a label of this plan's own.
    As(Action, Msg),
    /// The keys of several operations under one label (`j/k:scroll`, `s/S:stage(all)`). Two
    /// operations are zipped (`j/k`, `J/K/PgDn/PgUp`).
    Group(&'static [Action], Msg),
    /// Like `Group`, labelled as `decl::text` says for the first operation (`J/K:slide|sheet|page`).
    GroupOp(&'static [Action]),
    /// The keys of several operations merged (`Space/↵:toggle`).
    Union(&'static [Action], Msg),
    /// The key that opens a leader menu (`y:copy`, `Space:file ops`).
    Leader(LeaderId, Msg),
    /// A token worked out from the state (the search status).
    Dyn(fn(&App, Context) -> Option<String>),
}

const SCROLL: &[Action] = &[Action::Navigate(Motion::Down), Action::Navigate(Motion::Up)];
const ENDS: &[Action] = &[
    Action::Navigate(Motion::Top),
    Action::Navigate(Motion::Bottom),
];
const PAGE_KEYS: &[Action] = &[
    Action::Navigate(Motion::PageDown),
    Action::Navigate(Motion::PageUp),
];
const HSCROLL: &[Action] = &[
    Action::Navigate(Motion::Left),
    Action::Navigate(Motion::Right),
];
const LINE_ENDS: &[Action] = &[
    Action::Navigate(Motion::LineHome),
    Action::Navigate(Motion::LineEnd),
];
const HJKL: &[Action] = &[
    Action::Navigate(Motion::Left),
    Action::Navigate(Motion::Down),
    Action::Navigate(Motion::Up),
    Action::Navigate(Motion::Right),
];
const TABS: &[Action] = &[Action::TabPrev, Action::TabNext];
const FILE_JUMP: &[Action] = &[Action::PreviewFileNext, Action::PreviewFilePrev];
const ZOOM: &[Action] = &[Action::ImageZoomIn, Action::ImageZoomOut];
const PAGE_TURN: &[Action] = &[Action::PageNext, Action::PagePrev];
#[cfg(feature = "git")]
const NEXT_PREV_CHANGE: &[Action] = &[Action::JumpNextChange, Action::JumpPrevChange];
const SEARCH_MATCH: &[Action] = &[Action::SearchNext, Action::SearchPrev];
const HOME: &[Action] = &[Action::Navigate(Motion::LineHome)];
#[cfg(feature = "git")]
const BACK_ESC_PREVIEW: &[Action] = &[Action::PreviewBack, Action::Cancel];
const CLOSE_HELP: &[Action] = &[Action::ToggleHelp, Action::Cancel];
const CLOSE_INFO: &[Action] = &[Action::InfoClose, Action::Cancel];
const CLOSE_CELL: &[Action] = &[Action::ToggleTableCell, Action::Cancel];
const TOGGLE_ITEM: &[Action] = &[Action::ToggleFocusedItem, Action::LinkOpen];
const VISUAL_EXIT: &[Action] = &[Action::PreviewExitVisual, Action::Cancel];
const ALL_FILES: &[Action] = &[Action::ToggleChangedFilter, Action::TreeLeave];
const SELECT: &[Action] = &[Action::PreviewEnterVisual, Action::PreviewEnterVisualLine];
const CHANGED_KEYS: &[Action] = &[Action::JumpNextChange, Action::JumpPrevChange];

/// The footer plan for `ctx` while nothing more specific is going on. State that changes what a
/// screen offers (the decorated document vs the raw text, the media diff, git vs jj) is looked at
/// here, in one function per screen.
pub fn plan(app: &App, ctx: Context) -> Vec<Foot> {
    match ctx {
        Context::Tree => tree(app),
        Context::TreeVisual => vec![
            Foot::Group(SCROLL, Msg::FtExtend),
            Foot::Op(Action::VisualSelectSiblings),
            Foot::Op(Action::VisualSelectAll),
            Foot::Op(Action::VisualCommit),
            Foot::Leader(LeaderId::File, Msg::FtOps),
            Foot::As(Action::Cancel, Msg::FtCancel),
        ],
        Context::Text | Context::Document => text_preview(app),
        Context::TextVisual => vec![
            Foot::Group(
                if app.preview_visual_linewise() {
                    LINEWISE_EXTEND
                } else {
                    HJKL
                },
                Msg::FtExtend,
            ),
            Foot::Op(Action::PreviewCopySelection),
            Foot::As(Action::PreviewCopySelectionRef, Msg::WkAtRef),
            Foot::Union(VISUAL_EXIT, Msg::FtCancel),
        ],
        Context::Image => image(app),
        Context::Table => table(app),
        Context::TableCell => vec![
            Foot::Group(SCROLL, Msg::Scroll),
            Foot::Group(ENDS, Msg::FtTopBottom),
            Foot::Union(CLOSE_CELL, Msg::FtClose),
        ],
        Context::Help => vec![
            Foot::Group(SCROLL, Msg::Scroll),
            Foot::Group(ENDS, Msg::FtTopBottom),
            Foot::Union(CLOSE_HELP, Msg::FtClose),
        ],
        Context::Info => vec![Foot::Union(CLOSE_INFO, Msg::FtClose)],
        Context::Sort => vec![
            Foot::Op(Action::SortSet(crate::app::SortKey::Name)),
            Foot::Op(Action::SortSet(crate::app::SortKey::Size)),
            Foot::Op(Action::SortSet(crate::app::SortKey::Modified)),
            Foot::Op(Action::SortSet(crate::app::SortKey::Ext)),
            Foot::Op(Action::SortToggleReverse),
            Foot::Op(Action::SortToggleDirsFirst),
            Foot::As(Action::Cancel, Msg::FtCancel),
        ],
        // The bookmark list, the tab list and the outline have no footer of their own: they show
        // the screen underneath (see `status::footer_spans`).
        Context::Bookmarks | Context::Tabs | Context::Outline => Vec::new(),
        #[cfg(feature = "git")]
        Context::Diff => diff(app),
        #[cfg(feature = "git")]
        Context::Screen(b, s) => screen(app, b, s),
    }
}

const LINEWISE_EXTEND: &[Action] = &[
    Action::Navigate(Motion::Down),
    Action::Navigate(Motion::Up),
    Action::Navigate(Motion::Top),
    Action::Navigate(Motion::Bottom),
];

fn tree(app: &App) -> Vec<Foot> {
    // With multiple tabs, q = close the tab (+ Q = quit); with the last one, q = quit.
    let multi = app.tab_count() > 1;
    let mut v = vec![
        Foot::Group(SCROLL, Msg::GitMove),
        Foot::Op(Action::TreeDescend),
        Foot::Op(Action::TreeLeave),
        Foot::Op(Action::CloseTabOrQuit),
    ];
    if multi {
        v.push(Foot::Op(Action::Quit));
    }
    v.extend([
        Foot::Op(Action::ToggleHelp),
        Foot::Op(Action::FilterStart),
        Foot::Leader(LeaderId::File, Msg::HintFileOps),
        Foot::Op(Action::RequestEdit),
        Foot::Op(Action::OpenGitView),
        Foot::Op(Action::OpenGitDiffCursor),
        Foot::Op(Action::ToggleChangedFilter),
        Foot::Op(Action::ToggleFollow),
        Foot::Leader(LeaderId::Copy, Msg::CopyHint),
        Foot::Op(Action::TabNew),
        Foot::Op(Action::OpenInNewTab),
        Foot::Group(TABS, Msg::HintTab),
        Foot::Leader(LeaderId::Options, Msg::HintOptions),
        Foot::Op(Action::ToggleHidden),
        Foot::Op(Action::ToggleInfo),
        Foot::Op(Action::OpenSortMenu),
        Foot::Op(Action::MarkSet),
        Foot::Op(Action::MarkJump),
        Foot::Op(Action::PasteJump),
        Foot::Op(Action::EnterVisual),
        Foot::Op(Action::ToggleSelect),
        Foot::Op(Action::SetAnchor),
        Foot::Group(PAGE_KEYS, Msg::HintPage),
    ]);
    v
}

/// The tree while only the changed files are listed (`C`).
pub fn changed_filter_plan() -> Vec<Foot> {
    vec![
        Foot::As(Action::TreeActivate, Msg::FtPreview),
        Foot::Group(CHANGED_KEYS, Msg::FtNextPrev),
        Foot::Union(ALL_FILES, Msg::FtAllFiles),
    ]
}

fn table(_app: &App) -> Vec<Foot> {
    vec![
        Foot::Group(HJKL, Msg::HintCell),
        Foot::Op(Action::ToggleTableCell),
        Foot::Dyn(search_status),
        Foot::GroupOp(PAGE_TURN),
        Foot::Op(Action::SearchStart),
        Foot::Leader(LeaderId::TableCopy, Msg::CopyHint),
        Foot::Group(ENDS, Msg::HintEnds),
        Foot::Group(FILE_JUMP, Msg::HintFileJump),
        Foot::Op(Action::PreviewBack),
        Foot::Op(Action::ToggleHelp),
        Foot::Op(Action::RequestEdit),
        Foot::Group(TABS, Msg::HintTab),
        Foot::Leader(LeaderId::Options, Msg::HintOptions),
    ]
}

fn image(_app: &App) -> Vec<Foot> {
    vec![
        Foot::Group(ZOOM, Msg::Zoom),
        Foot::Op(Action::ImageZoomReset),
        Foot::Group(HJKL, Msg::HintPan),
        Foot::GroupOp(PAGE_TURN),
        Foot::Op(Action::ImageReturnToDiff),
        Foot::Group(FILE_JUMP, Msg::HintFileJump),
        Foot::Op(Action::PreviewBack),
        Foot::Op(Action::ToggleHelp),
        Foot::Op(Action::RequestEdit),
        Foot::Group(TABS, Msg::HintTab),
        Foot::Leader(LeaderId::Options, Msg::HintOptions),
    ]
}

/// Whether the text preview is shown as a decorated Markdown / document view (the screen with
/// links to follow) rather than as text.
fn is_decorated_footer(app: &App) -> bool {
    use crate::preview::PreviewKind;
    (matches!(app.tab.preview_kind, Some(PreviewKind::Markdown(_))) || app.document_ready())
        && !app.is_raw_source()
}

fn text_preview(app: &App) -> Vec<Foot> {
    // A Word/OpenDocument file that is still loading or failed to convert has no text yet: `R`,
    // `/`, scrolling and the like do nothing, so only the keys that still act are listed.
    if app.document_text_missing() {
        return vec![
            Foot::Op(Action::ToggleFollow),
            Foot::Group(FILE_JUMP, Msg::HintFileJump),
            Foot::Op(Action::PreviewBack),
            Foot::Op(Action::ToggleHelp),
            Foot::Op(Action::RequestEdit),
            Foot::Group(TABS, Msg::HintTab),
            Foot::Leader(LeaderId::Options, Msg::HintOptions),
        ];
    }
    if is_decorated_footer(app) {
        // `Tab` always cycles focus; everything else here depends on the focused item.
        let mut v = vec![
            Foot::Group(SCROLL, Msg::Scroll),
            Foot::Op(Action::LinkFocusNext),
            Foot::GroupOp(PAGE_TURN),
        ];
        match app.md_focused_kind() {
            Some(MdFocus::Task) | Some(MdFocus::Details) => {
                v.push(Foot::Union(TOGGLE_ITEM, Msg::HintToggle));
            }
            Some(MdFocus::CodeBlock) => v.push(Foot::Dyn(copy_code_token)),
            Some(MdFocus::MermaidFence) => {
                v.push(Foot::Op(Action::LinkOpen));
                v.push(Foot::Group(ZOOM, Msg::Zoom));
                // hjkl/0 pan only once zoomed AND the diagram is fully on screen.
                if app.fence_zoom_level() > 1.001 && app.focused_fence_fully_visible() {
                    v.push(Foot::Group(HJKL, Msg::HintPan));
                    v.push(Foot::Group(HOME, Msg::HintFit));
                }
            }
            Some(_) => {
                v.push(Foot::Op(Action::LinkOpen));
                v.push(Foot::Op(Action::OpenLinkNewTab));
            }
            None => {}
        }
        v.extend([
            Foot::Op(Action::ToggleOutline),
            Foot::Op(Action::ToggleMarkdownRaw),
            Foot::Op(Action::SearchStart),
            Foot::Op(Action::ToggleFollow),
            Foot::Group(FILE_JUMP, Msg::HintFileJump),
            Foot::Op(Action::PreviewBack),
            Foot::Op(Action::ToggleHelp),
            Foot::Op(Action::RequestEdit),
            Foot::Group(ENDS, Msg::HintEnds),
            Foot::Group(TABS, Msg::HintTab),
            Foot::Leader(LeaderId::Options, Msg::HintOptions),
            Foot::Group(PAGE_KEYS, Msg::HintPage),
        ]);
        return v;
    }
    // Code / plain text. While searching, show n/N (match cur/total) up front.
    let mut v = vec![
        Foot::Group(SCROLL, Msg::Scroll),
        Foot::Dyn(search_status),
        Foot::Op(Action::SearchStart),
    ];
    // Range-selection copy (v=char / V=line) is windowed (Code/Text/raw Markdown) only. The label
    // of `Y` is the short one here (the long wording is the help's).
    if app.is_windowed() {
        v.push(Foot::Group(SELECT, Msg::HintSelect));
        v.push(Foot::As(Action::PreviewCopySelectionRef, Msg::WkAtRef));
    }
    v.push(Foot::Op(Action::ToggleFollow));
    #[cfg(feature = "git")]
    v.push(Foot::Op(Action::ToggleFollowDiffScope));
    v.extend([
        Foot::Op(Action::ToggleMarkdownRaw),
        Foot::Group(FILE_JUMP, Msg::HintFileJump),
        Foot::Op(Action::PreviewBack),
        Foot::Op(Action::ToggleHelp),
        Foot::Op(Action::RequestEdit),
        Foot::Group(ENDS, Msg::HintEnds),
        Foot::Group(HSCROLL, Msg::HintHscroll),
        Foot::Group(LINE_ENDS, Msg::HintLineEnds),
        Foot::Group(TABS, Msg::HintTab),
        Foot::Leader(LeaderId::Options, Msg::HintOptions),
        Foot::Group(PAGE_KEYS, Msg::HintPage),
    ]);
    v
}

/// `n/N:match[3/12]` while a search has hits (`+` once collecting stopped at the cap), `n/N:match`
/// while a search is active but nothing matched.
fn search_status(app: &App, ctx: Context) -> Option<String> {
    let keys = join_keys(app, ctx, SEARCH_MATCH);
    if keys.is_empty() {
        return None;
    }
    let label = tr(app.lang, Msg::Match);
    if let Some((cur, total)) = app.search_status() {
        let plus = if app.search_capped() { "+" } else { "" };
        return Some(format!("{keys}:{label}[{cur}/{total}{plus}]"));
    }
    app.preview_search_query()
        .is_some()
        .then(|| format!("{keys}:{label}"))
}

/// `y c:copy code`: the copy menu's key followed by the code-block item's key.
fn copy_code_token(app: &App, ctx: Context) -> Option<String> {
    let lead = app
        .keymaps
        .leader_keys_of(ctx)
        .into_iter()
        .find(|(_, id)| *id == LeaderId::Copy)?
        .0;
    let item = app
        .keymaps
        .leaders
        .get(&LeaderId::Copy)?
        .items
        .iter()
        .find(|it| it.action == Action::CopyCodeBlock)?;
    Some(format!(
        "{} {}:{}",
        footer_key(lead, false),
        footer_key(item.key, false),
        tr(app.lang, Msg::HintCopyCode)
    ))
}

#[cfg(feature = "git")]
fn diff(app: &App) -> Vec<Foot> {
    // The media diff's side-by-side view, and the binary-summary body a non-picture binary kind
    // shows, use the reduced set: nothing to scroll or lay out as unified/split.
    let media = app.diff_footer_is_media_or_summary();
    let rendered = app.diff_rendered_active();
    let mut v = Vec::new();
    if !media {
        v.push(Foot::Group(SCROLL, Msg::Scroll));
    }
    v.push(Foot::Group(NEXT_PREV_CHANGE, Msg::HintNextPrevFile));
    if media {
        v.push(Foot::Op(Action::CycleDiffLayout));
        v.push(Foot::GroupOp(PAGE_TURN));
        v.push(Foot::Op(Action::CycleDiffView));
    } else {
        // `h/l` scroll is a no-op in the rendered presentation while lines wrap.
        if !(rendered && app.cfg.ui.wrap) {
            v.push(Foot::Group(HSCROLL, Msg::HintHscroll));
        }
        v.push(Foot::Op(Action::CycleDiffLayout));
    }
    v.push(Foot::Op(Action::GitDiffDiscard));
    v.push(Foot::Union(BACK_ESC_PREVIEW, Msg::GitBack));
    if !media {
        v.push(Foot::Op(Action::CycleDiffView));
    }
    v.push(Foot::Leader(LeaderId::Options, Msg::HintOptions));
    v
}

#[cfg(feature = "git")]
fn screen(_app: &App, b: Backend, s: Screen) -> Vec<Foot> {
    const BACK: &[Action] = &[Action::GitClose, Action::Cancel];
    const WT_BACK: &[Action] = &[Action::WorktreeClose, Action::Cancel];
    let jj = b == Backend::Jj;
    let nav = Foot::Group(SCROLL, Msg::FtNav);
    match s {
        Screen::Changes => vec![
            Foot::Group(&[Action::GitStage, Action::GitStageAll], Msg::FtStage),
            Foot::Group(&[Action::GitUnstage, Action::GitUnstageAll], Msg::FtUnstage),
            Foot::Op(Action::GitDiscard),
            Foot::Op(Action::GitCommit),
            Foot::Op(Action::GitOpenSelectedDiff),
            Foot::Op(Action::GitWorktreeDiff),
            Foot::Op(Action::GitOpenLog),
            Foot::Op(Action::GitOpenGraph),
            Foot::As(
                Action::GitOpenBranches,
                if jj {
                    Msg::HintBookmarks
                } else {
                    Msg::FtBranch
                },
            ),
            Foot::Op(Action::GitOpenWorktrees),
            Foot::Op(Action::JjSync),
            Foot::Op(Action::GitLaunchTool),
            Foot::As(Action::GitClose, Msg::FtClose),
        ],
        Screen::Log => vec![
            nav,
            Foot::Op(Action::GitOpenDetail),
            Foot::Union(BACK, Msg::GitBack),
        ],
        Screen::Graph => {
            let mut v = vec![nav, Foot::Op(Action::GitOpenDetail)];
            if jj {
                v.push(Foot::Op(Action::GitGraphToggleAll));
                v.push(Foot::As(Action::GitGraphOpenPicker, Msg::HintBookmarks));
            } else {
                v.push(Foot::Op(Action::GitGraphSetBase));
                v.push(Foot::Op(Action::GitGraphClearBase));
                v.push(Foot::As(Action::GitGraphOpenPicker, Msg::GitBranches));
            }
            v.push(Foot::Union(BACK, Msg::GitBack));
            v
        }
        Screen::GraphPicker => vec![
            Foot::Op(Action::GitGraphPickerToggle),
            Foot::Group(
                &[Action::GitGraphPickerMoveDown, Action::GitGraphPickerMoveUp],
                Msg::FtReorder,
            ),
            Foot::Op(Action::GitGraphPickerAll),
            Foot::Op(Action::GitGraphPickerCurrentOnly),
            Foot::Op(Action::GitGraphPickerApply),
            Foot::As(Action::GitClose, Msg::FtCancel),
        ],
        Screen::Branches => vec![
            nav,
            Foot::Op(Action::BranchCheckout),
            Foot::Op(Action::BranchCreate),
            Foot::Op(Action::BranchDelete),
            Foot::Op(Action::BranchFilterStart),
            Foot::Union(BACK, Msg::GitBack),
        ],
        Screen::Worktrees => vec![
            nav,
            Foot::Op(Action::WorktreeGoto),
            Foot::Op(Action::WorktreeCreate),
            Foot::Op(Action::WorktreeGotoNewTab),
            Foot::Op(Action::WorktreeShowChanges),
            Foot::Op(Action::WorktreeFilterStart),
            Foot::Union(WT_BACK, Msg::GitBack),
        ],
        Screen::Detail => vec![
            Foot::Group(SCROLL, Msg::Scroll),
            Foot::Group(HSCROLL, Msg::HintHscroll),
            Foot::Op(Action::CycleDiffLayout),
            Foot::Group(ENDS, Msg::HintEnds),
            Foot::Union(BACK, Msg::GitBack),
        ],
    }
}

/// One footer token: `<keys>:<label>`, with the keys and operations it was made from (for the
/// tests that check the notation against the keymap; `Dyn` tokens have none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub keys: Vec<KeyPress>,
    pub actions: Vec<Action>,
}

/// The keys of an operation group, de-duplicated. Two operations are zipped (`j/k`, `J/K/PgDn/PgUp`).
fn group_keys(app: &App, ctx: Context, acts: &[Action]) -> Vec<KeyPress> {
    let enabled: Vec<Action> = acts
        .iter()
        .copied()
        .filter(|a| action_enabled(app, ctx, *a))
        .collect();
    let (cols, _) = columns(app, ctx, &enabled);
    let mut keys: Vec<KeyPress> = Vec::new();
    for kp in cols.into_iter().flatten() {
        if !keys.contains(&kp) {
            keys.push(kp);
        }
    }
    keys
}

/// The keys of several operations merged in the order given (for `Union`).
fn union_keys(app: &App, ctx: Context, acts: &[Action]) -> Vec<KeyPress> {
    let mut keys: Vec<KeyPress> = Vec::new();
    for a in acts.iter().filter(|a| action_enabled(app, ctx, **a)) {
        for kp in crate::ops::keys::keys_of(app, ctx, *a) {
            if !keys.contains(&kp) {
                keys.push(kp);
            }
        }
    }
    keys
}

fn leader_keys(app: &App, ctx: Context, id: LeaderId) -> Vec<KeyPress> {
    app.keymaps
        .leader_keys_of(ctx)
        .into_iter()
        .filter(|(_, l)| *l == id)
        .map(|(k, _)| k)
        .collect()
}

fn write_keys(keys: &[KeyPress], ctx: Context) -> String {
    let short = short_enter(ctx);
    let mut parts: Vec<String> = Vec::new();
    for k in keys {
        let s = footer_key(*k, short);
        if !parts.contains(&s) {
            parts.push(s);
        }
    }
    parts.join("/")
}

/// The keys of an operation group written for the footer: every key, joined with `/`.
fn join_keys(app: &App, ctx: Context, acts: &[Action]) -> String {
    write_keys(&group_keys(app, ctx, acts), ctx)
}

/// Render a plan into tokens, dropping what does not act or is unbound.
pub fn render_tokens(app: &App, ctx: Context, plan: &[Foot]) -> Vec<Token> {
    let lang = app.lang;
    let tok = |keys: Vec<KeyPress>, label: Msg, actions: &[Action]| -> Option<Token> {
        (!keys.is_empty()).then(|| Token {
            text: format!("{}:{}", write_keys(&keys, ctx), tr(lang, label)),
            keys,
            actions: actions.to_vec(),
        })
    };
    let mut out = Vec::new();
    for item in plan {
        let t = match item {
            Foot::Op(a) => {
                if !action_enabled(app, ctx, *a) {
                    continue;
                }
                match text(app, ctx, *a).label {
                    Some(label) => tok(group_keys(app, ctx, &[*a]), label, &[*a]),
                    None => None,
                }
            }
            Foot::As(a, label) => {
                if !action_enabled(app, ctx, *a) {
                    continue;
                }
                tok(group_keys(app, ctx, &[*a]), *label, &[*a])
            }
            Foot::Group(acts, label) => {
                if !acts.iter().any(|a| action_enabled(app, ctx, *a)) {
                    continue;
                }
                tok(group_keys(app, ctx, acts), *label, acts)
            }
            Foot::GroupOp(acts) => {
                if !acts.iter().any(|a| action_enabled(app, ctx, *a)) {
                    continue;
                }
                match text(app, ctx, acts[0]).label {
                    Some(label) => tok(group_keys(app, ctx, acts), label, acts),
                    None => None,
                }
            }
            Foot::Union(acts, label) => {
                if !acts.iter().any(|a| action_enabled(app, ctx, *a)) {
                    continue;
                }
                tok(union_keys(app, ctx, acts), *label, acts)
            }
            Foot::Leader(id, label) => tok(leader_keys(app, ctx, *id), *label, &[]),
            Foot::Dyn(f) => f(app, ctx).map(|text| Token {
                text,
                keys: Vec::new(),
                actions: Vec::new(),
            }),
        };
        out.extend(t);
    }
    out
}

/// Render a plan into `<keys>:<label>` strings.
pub fn render(app: &App, ctx: Context, plan: &[Foot]) -> Vec<String> {
    render_tokens(app, ctx, plan)
        .into_iter()
        .map(|t| t.text)
        .collect()
}

/// The footer tokens of `ctx`.
pub fn tokens(app: &App, ctx: Context) -> Vec<String> {
    render(app, ctx, &plan(app, ctx))
}
