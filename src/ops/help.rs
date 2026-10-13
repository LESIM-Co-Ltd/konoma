// The `?` help: which operations a context lists, with the key notation generated from the keymap.
//
// The sections follow the layers of the key design: the operations of this screen (the context's own
// table and the table of its view type), movement (layer 1), what launches something outside konoma
// (layer 3: `e`), the menus behind `y` and `Space`, and last what is true everywhere (layer 0: tabs,
// help, quit). The screen's own operations come first because that is what the help is opened
// for. A row lands in its section by *where its key is defined*, so the help follows a key that
// moves from one layer to another.
//
// A row is shown only while its operation acts (`ops::gate`), exactly like a footer token. The few
// rows that explain how to use a feature rather than a key that acts right now (the keys of the
// focused Markdown item) are marked `ungated`: they are the reference that says "press `Tab`, then
// ...".

use crate::app::App;
use crate::i18n::{tr, Msg};
use crate::keymap::{Action, Context, KeyLayer, KeyPress, LeaderId, Motion};
#[cfg(feature = "git")]
use crate::keymap::{Backend, Screen};
use crate::ops::decl::{decl, text};
use crate::ops::gate::action_enabled;
use crate::ops::keys::{columns, help_key};
use crate::ui::help::HelpSection;

/// Where a row is listed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sec {
    Tabs,
    Global,
    Move,
    View,
    Launch,
    Custom(Msg),
}

/// One help row: the keys of one or more operations and what they do.
#[derive(Clone)]
struct Row {
    groups: Vec<&'static [Action]>,
    lit: Option<&'static str>,
    /// Keys written out by hand (not rebindable and not an operation: `1 - 9`, the drop of a file).
    lit_only: Option<&'static str>,
    /// The same, but a word that is translated (`drag & drop`).
    lit_msg: Option<Msg>,
    desc: Option<Msg>,
    ungated: bool,
    sec: Option<Sec>,
}

fn r(acts: &'static [Action]) -> Row {
    Row {
        groups: vec![acts],
        lit: None,
        lit_only: None,
        lit_msg: None,
        desc: None,
        ungated: false,
        sec: None,
    }
}

fn lit_row(keys: &'static str, desc: Msg, sec: Sec) -> Row {
    Row {
        groups: Vec::new(),
        lit: None,
        lit_only: Some(keys),
        lit_msg: None,
        desc: Some(desc),
        ungated: true,
        sec: Some(sec),
    }
}

fn lit_msg_row(keys: Msg, desc: Msg, sec: Sec) -> Row {
    Row {
        lit_msg: Some(keys),
        lit_only: Some(""),
        ..lit_row("", desc, sec)
    }
}

impl Row {
    /// A second group of operations, written after the first (`/  n / N`).
    fn then(mut self, acts: &'static [Action]) -> Row {
        self.groups.push(acts);
        self
    }
    /// Keys that cannot be rebound, written after the generated ones (`↑ ↓`).
    fn lit(mut self, s: &'static str) -> Row {
        self.lit = Some(s);
        self
    }
    fn d(mut self, m: Msg) -> Row {
        self.desc = Some(m);
        self
    }
    fn ungated(mut self) -> Row {
        self.ungated = true;
        self
    }
    fn sec(mut self, s: Sec) -> Row {
        self.sec = Some(s);
        self
    }
}

const SCROLL: &[Action] = &[Action::Navigate(Motion::Down), Action::Navigate(Motion::Up)];
const ENDS: &[Action] = &[
    Action::Navigate(Motion::Top),
    Action::Navigate(Motion::Bottom),
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
const PAGE_KEYS: &[Action] = &[
    Action::Navigate(Motion::PageDown),
    Action::Navigate(Motion::PageUp),
];
const HALF_KEYS: &[Action] = &[
    Action::Navigate(Motion::HalfDown),
    Action::Navigate(Motion::HalfUp),
];
const TABS: &[Action] = &[Action::TabPrev, Action::TabNext];
const FILE_JUMP: &[Action] = &[Action::PreviewFileNext, Action::PreviewFilePrev];
const BOOKMARKS: &[Action] = &[Action::MarkSet, Action::MarkJump];
const ZOOM: &[Action] = &[Action::ImageZoomIn, Action::ImageZoomOut];
const ZOOM_ALL: &[Action] = &[
    Action::ImageZoomIn,
    Action::ImageZoomOut,
    Action::ImageZoomReset,
];
const PAGE_TURN: &[Action] = &[Action::PageNext, Action::PagePrev];
#[cfg(feature = "git")]
const DIFF_CHANGES: &[Action] = &[Action::DiffNextChange, Action::DiffPrevChange];
#[cfg(feature = "git")]
const SCREENS: &[Action] = &[Action::GitScreenNext, Action::GitScreenPrev];
const CHANGES: &[Action] = &[Action::JumpNextChange, Action::JumpPrevChange];
const SEARCH: &[Action] = &[Action::SearchStart];
const SEARCH_MATCH: &[Action] = &[Action::SearchNext, Action::SearchPrev];
const SELECT: &[Action] = &[Action::PreviewEnterVisual, Action::PreviewEnterVisualLine];
const FOCUS: &[Action] = &[Action::LinkFocusNext, Action::LinkFocusPrev];
const BACK_PREVIEW: &[Action] = &[Action::PreviewBack, Action::Cancel];
const TOGGLE_ITEM: &[Action] = &[Action::ToggleFocusedItem, Action::LinkOpen];
#[cfg(feature = "git")]
const BACK_GIT: &[Action] = &[Action::GitClose, Action::Cancel];
#[cfg(feature = "git")]
const BACK_WT: &[Action] = &[Action::WorktreeClose, Action::Cancel];
#[cfg(feature = "git")]
const STAGE: &[Action] = &[Action::GitStage, Action::GitStageAll];
#[cfg(feature = "git")]
const UNSTAGE: &[Action] = &[Action::GitUnstage, Action::GitUnstageAll];
#[cfg(feature = "git")]
const LOG_GRAPH: &[Action] = &[Action::GitOpenLog, Action::GitOpenGraph];

/// The context the help describes: the screen the help was opened over.
pub fn help_context(app: &App) -> Context {
    #[cfg(feature = "git")]
    {
        let backend = crate::ops::backend(app);
        let screen = if app.is_git_detail() {
            Some(Screen::Detail)
        } else if app.is_git_log() {
            Some(Screen::Log)
        } else if app.is_git_graph() {
            Some(Screen::Graph)
        } else if app.is_git_branches() {
            Some(Screen::Branches)
        } else if app.is_git_worktrees() {
            Some(Screen::Worktrees)
        } else if app.is_git_view() {
            Some(Screen::Changes)
        } else {
            None
        };
        if let Some(s) = screen {
            return Context::Screen(backend, s);
        }
        if app.tab.mode == crate::app::Mode::Preview && app.is_git_diff_preview() {
            return Context::Diff;
        }
    }
    match app.tab.mode {
        crate::app::Mode::Tree => Context::Tree,
        crate::app::Mode::Preview => {
            if app.is_table_preview() {
                Context::Table
            } else if app.document_text_missing() {
                Context::Text
            } else if app.is_image_preview() {
                Context::Image
            } else if app.shows_document() {
                Context::Document
            } else {
                Context::Text
            }
        }
    }
}

/// The rows every screen lists (layer 0).
fn global_rows() -> Vec<Row> {
    vec![
        r(&[Action::TabNew]).sec(Sec::Tabs),
        r(&[Action::TabNewAfter]).sec(Sec::Tabs),
        r(TABS).sec(Sec::Tabs),
        lit_row("1 - 9", Msg::HelpJumpTab, Sec::Tabs),
        r(&[Action::ToggleTabList]).sec(Sec::Tabs),
        r(&[Action::PasteJump]).sec(Sec::Global),
        r(&[Action::ToggleHelp]).sec(Sec::Global),
        r(&[Action::ToggleFollow]).sec(Sec::Global),
        r(&[Action::Quit]).sec(Sec::Global),
    ]
}

/// The title of the section of this screen's own operations and its rows.
fn view_rows(app: &App, ctx: Context) -> (Msg, Vec<Row>) {
    match ctx {
        Context::Tree => (Msg::TreeSection, tree_rows()),
        Context::Text | Context::Document => text_rows(app, ctx),
        Context::Image => (Msg::PreviewImage, image_rows()),
        Context::Table => (Msg::PreviewTable, table_rows()),
        #[cfg(feature = "git")]
        Context::Diff => (Msg::PreviewGitDiff, diff_rows(app)),
        #[cfg(feature = "git")]
        Context::Screen(b, s) => screen_rows(b, s),
        // The overlays are never what the help is opened over (`help_context`), but a plan for
        // each keeps the function total.
        Context::TreeVisual => (
            Msg::TreeSection,
            vec![
                r(SCROLL).lit("↑ ↓").d(Msg::TreeMoveUpDown),
                r(&[Action::VisualSelectSiblings]),
                r(&[Action::VisualSelectAll]),
                r(&[Action::VisualCommit]),
            ],
        ),
        Context::TextVisual
        | Context::TableCell
        | Context::Bookmarks
        | Context::Tabs
        | Context::Outline
        | Context::Info
        | Context::Help => (Msg::TreeSection, Vec::new()),
    }
}

fn tree_rows() -> Vec<Row> {
    vec![
        r(SCROLL).lit("↑ ↓").d(Msg::TreeMoveUpDown),
        r(ENDS).d(Msg::TopBottom),
        r(&[Action::TreeDescend]),
        r(&[Action::TreeActivate]),
        r(&[Action::OpenInNewTab]),
        r(&[Action::TreeLeave]),
        r(&[Action::SetAnchor]),
        r(&[Action::ResetAnchor]),
        r(&[Action::OpenGitDiffCursor]),
        r(&[Action::FilterStart]),
        r(&[Action::ToggleHidden]),
        r(&[Action::ToggleInfo]),
        r(&[Action::RequestEdit]),
        r(&[Action::OpenExternal]),
        r(&[Action::OpenGitView]),
        #[cfg(feature = "git")]
        r(&[Action::GitLaunchTool]).d(Msg::GitExternalTool),
        r(&[Action::ToggleChangedFilter]),
        r(CHANGES),
        r(&[Action::Refresh]),
        r(&[Action::OpenSortMenu]),
        r(BOOKMARKS).d(Msg::TreeBookmarkHint),
        r(PAGE_KEYS),
        r(HALF_KEYS),
        r(&[Action::CyclePathStyle]),
        r(&[Action::CloseTabOrQuit]),
        // The range selection and the status legend are sections of their own.
        r(&[Action::EnterVisual]).sec(Sec::Custom(Msg::TreeSelection)),
        r(&[Action::ToggleSelect]).sec(Sec::Custom(Msg::TreeSelection)),
        lit_msg_row(
            Msg::TreeDragDrop,
            Msg::TreeDropFiles,
            Sec::Custom(Msg::TreeSelection),
        ),
        lit_row("M", Msg::TreeModified, Sec::Custom(Msg::TreeGitStatus)),
        lit_row("A", Msg::TreeAddedStaged, Sec::Custom(Msg::TreeGitStatus)),
        lit_row("U", Msg::Untracked, Sec::Custom(Msg::TreeGitStatus)),
        lit_row("D", Msg::TreeDeleted, Sec::Custom(Msg::TreeGitStatus)),
        lit_row(
            "R / T / !",
            Msg::TreeStatusRenamed,
            Sec::Custom(Msg::TreeGitStatus),
        ),
    ]
}

fn text_rows(app: &App, ctx: Context) -> (Msg, Vec<Row>) {
    // A Word/OpenDocument file that is still converting or failed has no text: only the keys the
    // footer lists act.
    if app.document_text_missing() {
        return (
            Msg::PreviewTextMarkdown,
            vec![
                r(FILE_JUMP),
                r(BOOKMARKS).d(Msg::PreviewBookmarkHint),
                r(&[Action::RequestEdit]),
                r(&[Action::OpenExternal]),
                r(BACK_PREVIEW),
            ],
        );
    }
    let mut v = vec![
        r(SCROLL).lit("↑ ↓"),
        r(ENDS),
        r(HSCROLL).lit("← →"),
        r(LINE_ENDS),
        r(SEARCH).then(SEARCH_MATCH).d(Msg::SearchHint),
    ];
    // Range selection is windowed (raw source / plain code or text): `preview_enter_visual`
    // requires it. The decorated view still has `Y`, which copies the whole-file `@path` there.
    v.push(r(SELECT));
    v.push(r(&[Action::PreviewCopySelectionRef]));
    v.push(r(PAGE_TURN));
    v.push(r(&[Action::ToggleMarkdownRaw]));
    v.push(r(&[Action::ToggleOutline]));
    // The keys of the focused Markdown item are the reference for "press Tab, then ...": listed
    // for every decorated document, whether or not an item is focused right now.
    if ctx == Context::Document {
        v.push(r(FOCUS));
        v.push(r(&[Action::LinkOpen]).ungated());
        v.push(r(&[Action::OpenLinkNewTab]).ungated());
        // A Word document has no diagrams, checkboxes or `<details>`.
        if !app.is_document() {
            v.push(r(ZOOM_ALL).d(Msg::MermaidZoomHelp).ungated());
            v.push(r(TOGGLE_ITEM).d(Msg::MdTaskToggleHelp).ungated());
            v.push(r(TOGGLE_ITEM).d(Msg::HintDetailsToggle).ungated());
        }
    }
    v.push(r(FILE_JUMP));
    v.push(r(BOOKMARKS).d(Msg::PreviewBookmarkHint));
    v.push(r(&[Action::RequestEdit]));
    v.push(r(&[Action::OpenExternal]));
    v.push(r(&[Action::OpenGitView]));
    #[cfg(feature = "git")]
    v.push(r(&[Action::GitLaunchTool]).d(Msg::GitExternalTool));
    v.push(r(&[Action::Refresh]));
    v.push(r(PAGE_KEYS));
    v.push(r(HALF_KEYS));
    v.push(r(&[Action::CyclePathStyle]));
    v.push(r(BACK_PREVIEW));
    (Msg::PreviewTextMarkdown, v)
}

fn image_rows() -> Vec<Row> {
    vec![
        r(ZOOM),
        r(&[Action::ImageZoomReset]),
        r(HJKL).lit("arrows").d(Msg::PanHint),
        r(PAGE_TURN),
        r(&[Action::ToggleOutline]),
        r(&[Action::ImageReturnToDiff]),
        r(FILE_JUMP),
        r(BOOKMARKS).d(Msg::PreviewBookmarkHint),
        r(&[Action::RequestEdit]),
        r(&[Action::OpenExternal]),
        r(&[Action::OpenGitView]),
        #[cfg(feature = "git")]
        r(&[Action::GitLaunchTool]).d(Msg::GitExternalTool),
        r(&[Action::Refresh]),
        r(&[Action::CyclePathStyle]),
        r(BACK_PREVIEW),
    ]
}

fn table_rows() -> Vec<Row> {
    vec![
        r(HJKL).lit("arrows").d(Msg::TableMoveHelp),
        r(ENDS),
        r(LINE_ENDS).d(Msg::TableColsHelp),
        r(PAGE_TURN),
        r(&[Action::ToggleOutline]),
        r(&[Action::ToggleMarkdownRaw]),
        r(SEARCH).then(SEARCH_MATCH).d(Msg::TableSearchHelp),
        r(&[Action::ToggleTableCell]),
        r(PAGE_KEYS),
        r(HALF_KEYS),
        r(FILE_JUMP),
        r(BOOKMARKS).d(Msg::PreviewBookmarkHint),
        r(&[Action::RequestEdit]),
        r(&[Action::OpenExternal]),
        r(&[Action::OpenGitView]),
        #[cfg(feature = "git")]
        r(&[Action::GitLaunchTool]).d(Msg::GitExternalTool),
        r(&[Action::Refresh]),
        r(&[Action::CyclePathStyle]),
        r(BACK_PREVIEW),
    ]
}

#[cfg(feature = "git")]
fn diff_rows(app: &App) -> Vec<Row> {
    // The media diff's side-by-side view and the binary-summary body have nothing to scroll.
    let media_or_summary = app.diff_footer_is_media_or_summary();
    let mut v = Vec::new();
    if !media_or_summary {
        v.push(r(SCROLL).lit("↑ ↓"));
        v.push(r(ENDS));
        v.push(r(HSCROLL).then(LINE_ENDS).d(Msg::GitHScrollEnds));
    }
    v.push(r(&[Action::CycleDiffLayout]));
    v.push(r(PAGE_TURN));
    v.push(r(&[Action::CycleDiffView]));
    v.push(r(CHANGES));
    v.push(r(DIFF_CHANGES));
    v.push(r(&[Action::ToggleFollowDiffScope]));
    v.push(r(&[Action::RequestEdit]));
    v.push(r(&[Action::OpenExternal]));
    v.push(r(&[Action::OpenGitView]));
    #[cfg(feature = "git")]
    v.push(r(&[Action::GitLaunchTool]).d(Msg::GitExternalTool));
    v.push(r(&[Action::Refresh]));
    if !media_or_summary {
        v.push(r(PAGE_KEYS));
        v.push(r(HALF_KEYS));
    }
    v.push(r(&[Action::GitDiffDiscard]));
    v.push(r(BACK_PREVIEW).d(Msg::BackToGitView));
    v
}

#[cfg(feature = "git")]
fn screen_rows(b: Backend, s: Screen) -> (Msg, Vec<Row>) {
    let jj = b == Backend::Jj;
    match s {
        Screen::Detail => (
            Msg::GitCommitWorktreeDetail,
            vec![
                r(SCROLL).lit("↑ ↓"),
                r(HSCROLL).then(LINE_ENDS).d(Msg::GitHScrollEnds),
                r(PAGE_KEYS),
                r(HALF_KEYS).d(Msg::Scroll10Lines),
                r(&[Action::CycleDiffLayout]),
                r(ENDS),
                r(BACK_GIT).d(Msg::GitBack),
            ],
        ),
        Screen::Log => (
            Msg::GitLogLabel,
            vec![
                r(SCROLL).d(Msg::GitMove),
                r(ENDS),
                r(&[Action::GitOpenDetail]).d(Msg::GitCommitDetail),
                r(&[Action::ListFilterStart]),
                r(&[Action::Refresh]),
                r(SCREENS).d(if jj {
                    Msg::JjScreensHelp
                } else {
                    Msg::GitScreensHelp
                }),
                r(BACK_GIT).d(Msg::BackToChanges),
            ],
        ),
        Screen::Graph => {
            let mut v = vec![
                r(SCROLL).d(Msg::GitMoveCommit),
                r(ENDS),
                r(&[Action::GitOpenDetail]),
            ];
            if jj {
                v.push(r(&[Action::GitGraphToggleAll]));
                v.push(r(&[Action::GitGraphOpenPicker]).d(Msg::JjBookmarksRow));
            } else {
                v.push(r(&[Action::GitGraphSetBase]));
                v.push(r(&[Action::GitGraphClearBase]));
                v.push(r(&[Action::GitGraphOpenPicker]));
            }
            v.push(r(&[Action::ListFilterStart]));
            v.push(r(&[Action::Refresh]));
            v.push(r(SCREENS).d(if jj {
                Msg::JjScreensHelp
            } else {
                Msg::GitScreensHelp
            }));
            v.push(r(BACK_GIT).d(Msg::BackToChanges));
            (
                if jj {
                    Msg::JjGraphLabel
                } else {
                    Msg::GitGraphLabel
                },
                v,
            )
        }
        Screen::GraphPicker => (
            Msg::GraphPickerTitle,
            vec![
                r(SCROLL).d(Msg::GitMove),
                r(ENDS),
                r(&[Action::GitGraphPickerToggle]).d(Msg::HintToggle),
                r(&[Action::GitGraphPickerMoveDown, Action::GitGraphPickerMoveUp])
                    .d(Msg::FtReorder),
                r(&[Action::GitGraphPickerAll]).d(Msg::FtAll),
                r(&[Action::GitGraphPickerCurrentOnly]).d(Msg::FtCurrentOnly),
                r(&[Action::GitGraphPickerApply]).d(Msg::FtApply),
                r(BACK_GIT).d(Msg::FtCancel),
            ],
        ),
        Screen::Branches => (
            Msg::GitBranchesLabel,
            vec![
                r(SCROLL).d(Msg::GitMove),
                r(ENDS),
                r(&[Action::BranchCheckout]),
                r(&[Action::BranchCreate]),
                r(&[Action::BranchDelete]),
                r(&[Action::BranchFilterStart]),
                r(&[Action::Refresh]),
                r(SCREENS).d(if jj {
                    Msg::JjScreensHelp
                } else {
                    Msg::GitScreensHelp
                }),
                r(BACK_GIT).d(Msg::BackToChanges),
            ],
        ),
        Screen::Worktrees => (
            Msg::GitWorktreesLabel,
            vec![
                r(SCROLL).d(Msg::GitMove),
                r(ENDS),
                r(&[Action::WorktreeGoto]),
                r(&[Action::WorktreeCreate]),
                r(&[Action::WorktreeGotoNewTab]),
                r(&[Action::WorktreeShowChanges]),
                r(&[Action::WorktreeFilterStart]),
                r(&[Action::Refresh]),
                r(SCREENS).d(if jj {
                    Msg::JjScreensHelp
                } else {
                    Msg::GitScreensHelp
                }),
                r(BACK_WT).d(Msg::BackToChanges),
            ],
        ),
        Screen::Changes => {
            // Staging, committing and the worktree list are git's alone: a backend that only
            // reads refuses them (`ops::gate`), so they are simply not listed for jj.
            let v = vec![
                r(SCROLL).d(Msg::GitMove),
                r(ENDS),
                r(STAGE).d(Msg::StageHint),
                r(UNSTAGE).d(Msg::UnstageHint),
                r(&[Action::GitDiscard]),
                r(&[Action::GitCommit]),
                r(&[Action::GitOpenSelectedDiff]),
                r(&[Action::GitWorktreeDiff]).d(if jj { Msg::JjDiffAll } else { Msg::GitDiffAll }),
                r(LOG_GRAPH).d(Msg::GitLogGraph),
                r(&[Action::GitOpenBranches]).d(if jj {
                    Msg::JjBookmarksRow
                } else {
                    Msg::GitBranches
                }),
                r(&[Action::GitOpenWorktrees]),
                r(&[Action::JjSync]),
                r(&[Action::RequestEdit]),
                r(&[Action::OpenExternal]),
                r(&[Action::OpenInNewTab]).d(Msg::OpenInNewTabHelp),
                r(&[Action::ListFilterStart]),
                r(&[Action::Refresh]),
                r(SCREENS).d(if jj {
                    Msg::JjScreensHelp
                } else {
                    Msg::GitScreensHelp
                }),
                r(&[Action::GitLaunchTool]).d(if jj {
                    Msg::JjExternalTool
                } else {
                    Msg::GitExternalTool
                }),
                r(BACK_GIT).d(if jj {
                    Msg::JjCloseView
                } else {
                    Msg::GitCloseView
                }),
            ];
            (
                if jj {
                    Msg::JjChangesLabel
                } else {
                    Msg::GitChangesLabel
                },
                v,
            )
        }
    }
}

/// One rendered help row, with the keys and operations it was made from (for the tests that check
/// the notation against the keymap).
#[derive(Debug, Clone)]
#[allow(dead_code)] // `keys`/`actions` are read by the tests
pub struct RowOut {
    pub keys_text: String,
    pub desc: String,
    pub keys: Vec<KeyPress>,
    pub actions: Vec<Action>,
    /// A reference row that is listed whether or not its key acts right now.
    pub ungated: bool,
    sec: Sec,
}

impl std::fmt::Debug for Sec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Sec::Tabs => write!(f, "Tabs"),
            Sec::Global => write!(f, "Global"),
            Sec::Move => write!(f, "Move"),
            Sec::View => write!(f, "View"),
            Sec::Launch => write!(f, "Launch"),
            Sec::Custom(_) => write!(f, "Custom"),
        }
    }
}

/// The keys of a row as the help writes them, and the description. `None` = nothing to list.
fn render_row(app: &App, ctx: Context, row: &Row) -> Option<RowOut> {
    if let Some(keys) = row.lit_only {
        let desc = tr(app.lang, row.desc?).to_string();
        let keys = match row.lit_msg {
            Some(m) => tr(app.lang, m).to_string(),
            None => keys.to_string(),
        };
        return Some(RowOut {
            keys_text: keys,
            desc,
            keys: Vec::new(),
            actions: Vec::new(),
            ungated: true,
            sec: row.sec.unwrap_or(Sec::View),
        });
    }
    let mut parts: Vec<String> = Vec::new();
    let mut all_keys: Vec<KeyPress> = Vec::new();
    let mut all_acts: Vec<Action> = Vec::new();
    let mut lead: Option<Action> = None;
    let mut first_key = None;
    for group in &row.groups {
        let acts: Vec<Action> = group
            .iter()
            .copied()
            .filter(|a| row.ungated || action_enabled(app, ctx, *a))
            .collect();
        if acts.is_empty() {
            continue;
        }
        let (cols, zipped) = columns(app, ctx, &acts);
        if cols.is_empty() {
            continue;
        }
        if lead.is_none() {
            lead = acts.first().copied();
            first_key = cols.first().and_then(|c| c.first()).copied();
        }
        all_acts.extend(acts.iter().copied());
        all_keys.extend(cols.iter().flatten().copied());
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
        parts.push(rendered.join(if zipped { "  ·  " } else { " / " }));
    }
    let lead = lead?;
    let mut keys = parts.join("  ");
    if let Some(l) = row.lit {
        keys = format!("{keys} / {l}");
    }
    let desc = match row.desc {
        Some(m) => tr(app.lang, m).to_string(),
        // `R` on a plugin's own list of representations is named by the plugin.
        None => match crate::ops::decl::repr_label(app, lead) {
            Some(label) => label,
            None => tr(app.lang, text(app, ctx, lead).help?).to_string(),
        },
    };
    let sec = row.sec.unwrap_or_else(|| {
        if decl(lead).launch {
            Sec::Launch
        } else {
            match first_key.and_then(|k| app.keymaps.layer_of_key(ctx, k)) {
                Some(KeyLayer::Global) => Sec::Global,
                Some(KeyLayer::Basic) => Sec::Move,
                _ => Sec::View,
            }
        }
    });
    Some(RowOut {
        keys_text: keys,
        desc,
        keys: all_keys,
        actions: all_acts,
        ungated: row.ungated,
        sec,
    })
}

/// The section of the open plugin's keys (`None` when there is none to list).
fn plugin_section(app: &App, ctx: Context) -> Option<HelpSection> {
    let lang = app.plugin_lang();
    let mut sec: Option<HelpSection> = None;
    let mut plugin: Option<&str> = None;
    for b in app.plugin_acts_in(ctx) {
        let Some(key) = b.key else {
            continue;
        };
        if !action_enabled(app, ctx, Action::Plugin(b.act)) {
            continue;
        }
        plugin = Some(b.act.plugin());
        let desc = match b.help.get(lang) {
            "" => b.label.get(lang),
            h => h,
        };
        sec.get_or_insert_with(|| HelpSection::new(String::new()))
            .rows
            .push((help_key(key), desc.to_string()));
    }
    let mut sec = sec?;
    sec.title = plugin
        .and_then(|id| app.plugin_keys.name_of(id))
        .map(|n| n.get(lang).to_string())
        .unwrap_or_default();
    Some(sec)
}

/// Every row of `ctx`'s help (before it is grouped into sections), for tests.
#[cfg(test)]
pub fn rows_for(app: &App, ctx: Context) -> Vec<RowOut> {
    let (_, mut rows) = view_rows(app, ctx);
    rows.extend(global_rows());
    rows.iter()
        .filter_map(|r| render_row(app, ctx, r))
        .collect()
}

/// The sections of the `?` help for the screen the help was opened over.
pub fn sections(app: &App) -> Vec<HelpSection> {
    sections_for(app, help_context(app))
}

pub fn sections_for(app: &App, ctx: Context) -> Vec<HelpSection> {
    let (view_title, mut rows) = view_rows(app, ctx);
    rows.extend(global_rows());
    let mut by: Vec<(Sec, Vec<(String, String)>)> = Vec::new();
    for row in &rows {
        let Some(out) = render_row(app, ctx, row) else {
            continue;
        };
        let (sec, pair) = (out.sec, (out.keys_text, out.desc));
        match by.iter_mut().find(|(s, _)| *s == sec) {
            Some((_, v)) => v.push(pair),
            None => by.push((sec, vec![pair])),
        }
    }
    let take =
        |by: &mut Vec<(Sec, Vec<(String, String)>)>, sec: Sec| -> Option<Vec<(String, String)>> {
            let i = by.iter().position(|(s, _)| *s == sec)?;
            Some(by.remove(i).1)
        };
    let lang = app.lang;
    let mut out: Vec<HelpSection> = Vec::new();
    let push = |out: &mut Vec<HelpSection>, title: Msg, rows: Option<Vec<(String, String)>>| {
        if let Some(rows) = rows.filter(|r| !r.is_empty()) {
            out.push(HelpSection {
                title: tr(lang, title).to_string(),
                rows,
            });
        }
    };
    // This screen's own operations come first (that is what the help is opened for), then movement
    // and the launch verbs; what is true everywhere (layer 0) closes the list.
    push(&mut out, view_title, take(&mut by, Sec::View));
    // The open plugin's own keys: one section under the plugin's name, from its declarations. A
    // row is there only while the session says the action works (the gate), like every other row.
    out.extend(plugin_section(app, ctx));
    push(&mut out, Msg::HelpMove, take(&mut by, Sec::Move));
    push(&mut out, Msg::HelpLaunch, take(&mut by, Sec::Launch));
    // The menus behind `y` and `Space`: every leader this screen opens.
    let mut seen: Vec<LeaderId> = Vec::new();
    for (k, id) in app.keymaps.leader_keys_of(ctx) {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        if let Some(sec) = crate::ui::help::leader_section(app, ctx, id, &help_key(k)) {
            out.push(sec);
        }
    }
    push(&mut out, Msg::HelpTabs, take(&mut by, Sec::Tabs));
    push(&mut out, Msg::HelpGlobal, take(&mut by, Sec::Global));
    // Sections of their own (the tree's selection and status legend), in the order they appear.
    let customs: Vec<Sec> = by.iter().map(|(s, _)| *s).collect();
    for sec in customs {
        if let Sec::Custom(title) = sec {
            push(&mut out, title, take(&mut by, sec));
        }
    }
    out
}

/// Every operation a context's help lists, whether or not it acts right now (for the test that
/// every key binding is documented).
#[cfg(test)]
pub fn listed_actions(app: &App, ctx: Context) -> Vec<Action> {
    let (mut view, global) = declared_groups(app, ctx);
    view.extend(global);
    view
}

/// The operations the help of `ctx` lists, in the order it declares them: those of the screen
/// itself, and those of the rows every screen lists (layer 0: tabs, help, quit). The `Space` menu
/// lists the operations that are not in the footer in this order.
pub fn declared_groups(app: &App, ctx: Context) -> (Vec<Action>, Vec<Action>) {
    let flat = |rows: Vec<Row>| -> Vec<Action> {
        rows.iter()
            .flat_map(|r| r.groups.iter().flat_map(|g| g.iter().copied()))
            .collect()
    };
    (flat(view_rows(app, ctx).1), flat(global_rows()))
}
