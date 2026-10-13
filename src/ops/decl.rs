// The declaration of every operation: a short label for the footer, a description for the help.
//
// `decl` is exhaustive over `Action` on purpose: a new `Action` does not compile until it has a
// declaration here. `text` is the declaration as it reads *now* in a context: the few labels that
// change with the state (`e` is "edit" or "open", `R` names whatever it switches to, `J`/`K` are
// "slide", "sheet" or "page") are functions of the app, not constants.

use crate::app::{App, MdFocus};
use crate::i18n::Msg;
use crate::keymap::{Action, Context, Motion};

/// What an operation is called, before the state is looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decl {
    /// Footer label (`<keys>:<label>`). `None` = never named in a footer by default.
    pub label: Option<Msg>,
    /// Help description. `None` = the operation has no row of its own (it is part of a menu, a
    /// group row, or a fixed alias).
    pub help: Option<Msg>,
    /// Layer 3: an operation that starts something outside konoma's own views (`e`).
    pub launch: bool,
}

const fn d(label: Option<Msg>, help: Option<Msg>) -> Decl {
    Decl {
        label,
        help,
        launch: false,
    }
}

const NONE: Decl = d(None, None);

/// The default declaration of `a`.
pub fn decl(a: Action) -> Decl {
    use Action as A;
    use Msg as M;
    match a {
        A::Noop => NONE,
        A::Navigate(m) => match m {
            Motion::Up | Motion::Down => d(Some(M::Scroll), Some(M::Scroll)),
            Motion::Top | Motion::Bottom => d(Some(M::HintEnds), Some(M::TopBottom)),
            Motion::PageUp | Motion::PageDown => d(Some(M::HintPage), Some(M::HintPage)),
            Motion::HalfUp | Motion::HalfDown => d(None, Some(M::HelpHalfPage)),
            Motion::Left | Motion::Right => d(Some(M::HintHscroll), Some(M::HScroll)),
            Motion::LineHome | Motion::LineEnd => d(Some(M::HintLineEnds), Some(M::LineStartEnd)),
        },

        // Tabs, help, copy, jump (layer 0 and the clipboard)
        A::TabNew => d(Some(M::HintTab), Some(M::HelpNewTab)),
        A::TabNewAfter => d(None, Some(M::HelpNewTabAfter)),
        A::TabClose => NONE,
        A::TabPrev | A::TabNext => d(Some(M::HintTab), Some(M::PrevNextTab)),
        A::TabGoto(_) => d(None, Some(M::HelpJumpTab)),
        A::ToggleHelp => d(Some(M::HintHelp), Some(M::ToggleHelp)),
        A::CopyPath(_) => NONE,
        A::CopyCodeBlock => d(Some(M::HintCopyCode), None),
        A::PasteJump => d(Some(M::HintPasteJump), Some(M::PasteJumpHelp)),

        // Tree
        A::Quit => d(Some(M::HintQuit), Some(M::Quit)),
        A::CloseTabOrQuit => d(Some(M::HintQuit), Some(M::QuitOrCloseTab)),
        A::FilterStart => d(Some(M::HintFilter), Some(M::TreeFilter)),
        A::TreeDescend => d(Some(M::HintEnter), Some(M::EnterDirectory)),
        A::TreeActivate => d(None, Some(M::ExpandInPlace)),
        A::TreeLeave => d(Some(M::HintUp), Some(M::ToParent)),
        A::ToggleHidden => d(Some(M::HintHidden), Some(M::ToggleHidden)),
        A::ToggleWrap => d(Some(M::HintWrap), Some(M::ToggleWrapHelp)),
        A::ToggleInfo => d(Some(M::HintInfo), Some(M::TreeFileInfo)),
        A::RequestEdit => Decl {
            label: Some(M::HintEdit),
            help: Some(M::EditExternalEnv),
            launch: true,
        },
        A::OpenExternal => Decl {
            label: Some(M::HintOpenApp),
            help: Some(M::OpenAppHelp),
            launch: true,
        },
        A::OpenGitView => d(Some(M::HintGit), Some(M::TreeGitChangesHub)),
        A::Refresh => d(None, Some(M::Refresh)),
        A::CyclePathStyle => d(Some(M::HintPath), Some(M::CyclePathStyle)),
        A::OpenSortMenu => d(Some(M::HintSort), Some(M::SortHint)),
        A::MarkSet => d(Some(M::HintMark), Some(M::TreeBookmarkHint)),
        A::MarkJump => d(Some(M::HintBookmarks), None),
        A::SetAnchor => d(Some(M::HintAnchor), Some(M::TreeAnchorRoot)),
        A::ResetAnchor => d(None, Some(M::ResetRoot)),
        A::OpenGitDiffCursor => d(Some(M::HintDiff), Some(M::TreeDiffFile)),
        A::EnterVisual => d(Some(M::HintVisual), Some(M::VisualRangeHint)),
        A::ToggleSelect => d(Some(M::HintPick), Some(M::ToggleOne)),
        A::ToggleChangedFilter => d(Some(M::StChangedOnly), Some(M::ChangedFilterHelp)),
        A::JumpNextChange | A::JumpPrevChange => d(None, Some(M::JumpChangeHelp)),
        A::ToggleFollow => d(Some(M::StFollow), Some(M::FollowHelp)),

        // File management (the `Space` menu) and the tree's range selection
        A::FileCreate
        | A::FileRename
        | A::FileDelete
        | A::FileCopy
        | A::FileCut
        | A::FilePaste
        | A::FileDuplicate => NONE,
        A::VisualCommit => d(Some(M::FtRangeCommit), None),
        A::VisualSelectSiblings => d(Some(M::FtThisDir), None),
        A::VisualSelectAll => d(Some(M::FtAll), None),

        // Preview: text and Markdown
        A::PreviewBack => d(Some(M::GitBack), Some(M::BackToTree)),
        A::SearchStart => d(Some(M::HintSearch), Some(M::SearchHint)),
        A::SearchNext | A::SearchPrev => d(None, Some(M::SearchHint)),
        A::PreviewEnterVisual | A::PreviewEnterVisualLine => {
            d(Some(M::HintSelect), Some(M::PreviewSelectHelp))
        }
        A::PreviewCopySelection => d(Some(M::CopyHint), None),
        A::PreviewCopySelectionRef => d(Some(M::WkAtRef), Some(M::AtRefHelp)),
        A::PreviewExitVisual => d(Some(M::FtCancel), None),
        A::ToggleMarkdownRaw => d(Some(M::HintRawSource), Some(M::MdRawToggleHelp)),
        A::ImageReturnToDiff => d(Some(M::HintReturnToDiff), Some(M::HintReturnToDiff)),
        A::LinkFocusNext | A::LinkFocusPrev => d(Some(M::HintFocus), Some(M::FocusMdLink)),
        A::LinkOpen => d(Some(M::HintOpen), Some(M::OpenLinkHint)),
        A::OpenLinkNewTab => d(Some(M::HintNewTab), Some(M::OpenLinkNewTabHelp)),
        A::OpenInNewTab => d(Some(M::HintNewTab), Some(M::OpenInNewTabHelp)),

        // Preview: image
        A::ImageZoomIn | A::ImageZoomOut => d(Some(M::Zoom), Some(M::Zoom)),
        A::ImageZoomReset => d(Some(M::HintFit), Some(M::ResetFit)),
        A::PageNext | A::PagePrev => d(Some(M::HintPage), Some(M::HintPage)),
        A::PreviewFileNext | A::PreviewFilePrev => {
            d(Some(M::HintFileJump), Some(M::PreviewFileJumpHelp))
        }
        A::TableCopy(_) => NONE,

        // Preview: diff
        #[cfg(feature = "git")]
        A::GitDiffDiscard => d(Some(M::FtDiscard), Some(M::DiscardWholeFile)),
        #[cfg(feature = "git")]
        A::CycleDiffLayout => d(Some(M::FtDiffLayout), Some(M::GitLayout)),
        #[cfg(feature = "git")]
        A::CycleDiffView => d(None, Some(M::DiffViewCycleHelp)),
        #[cfg(feature = "git")]
        A::ToggleFollowDiffScope => d(Some(M::HintFollowScope), Some(M::HintFollowScope)),

        #[cfg(feature = "git")]
        A::DiffNextChange | A::DiffPrevChange => {
            d(Some(M::HintChange), Some(M::DiffChangeJumpHelp))
        }

        // The changes hub
        #[cfg(feature = "git")]
        A::GitStage | A::GitStageAll => d(Some(M::FtStage), Some(M::StageHint)),
        #[cfg(feature = "git")]
        A::GitUnstage | A::GitUnstageAll => d(Some(M::FtUnstage), Some(M::UnstageHint)),
        #[cfg(feature = "git")]
        A::GitDiscard => d(Some(M::FtDiscard), Some(M::GitDiscardFile)),
        #[cfg(feature = "git")]
        A::GitCommit => d(Some(M::FtCommit), Some(M::GitCommit)),
        #[cfg(feature = "git")]
        A::GitWorktreeDiff => d(Some(M::FtDiffAll), Some(M::GitDiffAll)),
        #[cfg(feature = "git")]
        A::GitOpenLog => d(Some(M::FtLog), Some(M::GitLogGraph)),
        #[cfg(feature = "git")]
        A::GitOpenGraph => d(Some(M::FtGraph), Some(M::GitLogGraph)),
        #[cfg(feature = "git")]
        A::GitOpenBranches => d(Some(M::FtBranch), Some(M::GitBranches)),
        #[cfg(feature = "git")]
        A::GitLaunchTool => d(Some(M::FtTool), Some(M::GitExternalTool)),
        #[cfg(feature = "git")]
        A::GitOpenSelectedDiff => d(Some(M::HintDiff), Some(M::GitFileDiff)),

        // Log and graph
        #[cfg(feature = "git")]
        A::GitOpenDetail => d(Some(M::FtDetail), Some(M::GitDetail)),
        #[cfg(feature = "git")]
        A::GitGraphSetBase => d(Some(M::FtBase), Some(M::GraphSetBaseHelp)),
        #[cfg(feature = "git")]
        A::GitGraphClearBase => d(Some(M::FtBaseOff), Some(M::GraphClearBaseHelp)),
        #[cfg(feature = "git")]
        A::GitGraphToggleAll => d(Some(M::FtAllRevisions), Some(M::JjGraphAllRow)),
        #[cfg(feature = "git")]
        A::JjSync => d(Some(M::FtSync), Some(M::JjSyncRow)),
        #[cfg(feature = "git")]
        A::GitGraphOpenPicker => d(Some(M::FtBranch), Some(M::GraphBranchesHelp)),
        #[cfg(feature = "git")]
        A::GitGraphPickerToggle => d(Some(M::HintToggle), None),
        #[cfg(feature = "git")]
        A::GitGraphPickerAll => d(Some(M::FtAll), None),
        #[cfg(feature = "git")]
        A::GitGraphPickerCurrentOnly => d(Some(M::FtCurrentOnly), None),
        #[cfg(feature = "git")]
        A::GitGraphPickerMoveUp | A::GitGraphPickerMoveDown => d(Some(M::FtReorder), None),
        #[cfg(feature = "git")]
        A::GitGraphPickerApply => d(Some(M::FtApply), None),

        // The version-control screens
        #[cfg(feature = "git")]
        A::GitScreenNext | A::GitScreenPrev => d(Some(M::FtScreens), Some(M::GitScreensHelp)),
        #[cfg(feature = "git")]
        A::ListFilterStart => d(Some(M::HintSearch), Some(M::ListFilterHelp)),

        // Branches and worktrees
        #[cfg(feature = "git")]
        A::BranchFilterStart | A::WorktreeFilterStart => {
            d(Some(M::HintSearch), Some(M::GitFilterByName))
        }
        #[cfg(feature = "git")]
        A::BranchCheckout => d(Some(M::GitCheckout), Some(M::GitCheckout)),
        #[cfg(feature = "git")]
        A::BranchCreate => d(Some(M::FtNew), Some(M::GitNewBranch)),
        #[cfg(feature = "git")]
        A::BranchDelete => d(Some(M::WkDelete), Some(M::GitDelete)),
        #[cfg(feature = "git")]
        A::GitOpenWorktrees => d(Some(M::GitWorktreesRow), Some(M::GitWorktreesRow)),
        #[cfg(feature = "git")]
        A::WorktreeGoto => d(Some(M::FtSwitch), Some(M::GitWorktreeSwitch)),
        #[cfg(feature = "git")]
        A::WorktreeGotoNewTab => d(Some(M::HintNewTab), Some(M::OpenInNewTabHelp)),
        #[cfg(feature = "git")]
        A::WorktreeClose => d(Some(M::GitBack), None),
        #[cfg(feature = "git")]
        A::WorktreeShowChanges => d(Some(M::HintDiff), Some(M::WorktreeShowChangesHelp)),
        #[cfg(feature = "git")]
        A::WorktreeCreate => d(Some(M::FtNew), Some(M::WorktreeCreateHelp)),
        #[cfg(feature = "git")]
        A::GitCopy(_) => NONE,
        #[cfg(feature = "git")]
        A::CopyBranchName => NONE,
        #[cfg(feature = "git")]
        A::GitClose => d(Some(M::GitBack), None),

        // Menus and overlays
        A::SortSet(k) => match k {
            crate::app::SortKey::Name => d(Some(M::FtSortName), None),
            crate::app::SortKey::Size => d(Some(M::FtSortSize), None),
            crate::app::SortKey::Modified => d(Some(M::FtSortMod), None),
            crate::app::SortKey::Ext => d(Some(M::FtSortExt), None),
        },
        A::SortToggleReverse => d(Some(M::FtSortReverse), None),
        A::SortToggleDirsFirst => d(Some(M::FtSortDirs), None),
        A::ToggleTabList => d(Some(M::HintTab), Some(M::HelpTabList)),
        A::TabListClose => NONE,
        A::ToggleOutline => d(Some(M::HintOutline), Some(M::HintOutline)),
        A::BookmarkJump | A::BookmarkEdit | A::BookmarkDelete | A::BookmarkClose => NONE,
        A::InfoClose => d(Some(M::FtClose), None),
        A::ToggleTableCell => d(Some(M::HintViewCell), Some(M::TableCellViewHelp)),

        // The fixed keys' default operations
        A::Cancel => NONE,
        A::TabListActivate | A::OutlineJump => NONE,
        A::ToggleFocusedItem => d(Some(M::HintToggle), Some(M::MdTaskToggleHelp)),
    }
}

/// A label and a description as they read now in a context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpText {
    pub label: Option<Msg>,
    pub help: Option<Msg>,
}

/// The declaration of `a` as it reads now in `ctx`. This is where the labels that depend on the
/// state live; everything else is `decl`. A `None` label/help means "do not name it here" (the
/// footer and the help skip it), which is how `e` disappears while opening Office documents is
/// switched off.
pub fn text(app: &App, ctx: Context, a: Action) -> OpText {
    #[cfg(not(feature = "git"))]
    let _ = ctx;
    use Action as A;
    use Msg as M;
    let base = decl(a);
    let mut t = OpText {
        label: base.label,
        help: base.help,
    };
    match a {
        A::RequestEdit => {
            t.label = app.edit_label(M::HintEdit, M::HintOpen);
            t.help = if matches!(ctx, Context::Image | Context::Table) {
                app.edit_help_label(M::EditExternal)
            } else {
                app.edit_help_label(M::EditExternalEnv)
            };
        }
        A::CloseTabOrQuit => {
            t.label = Some(if app.tab_count() > 1 {
                M::HintCloseTab
            } else {
                M::HintQuit
            });
        }
        A::PageNext | A::PagePrev => {
            match app.tab.preview_kind {
                // A workbook's pages are its sheets.
                Some(ref k) if k.is_spreadsheet() => {
                    t.label = Some(M::HintSheet);
                    t.help = Some(M::SheetSwitchHelp);
                }
                Some(ref k) if k.is_office_document() => {
                    t.label = Some(M::HintSlide);
                    t.help = Some(M::SlideSwitchHelp);
                }
                _ => {}
            }
        }
        A::ToggleMarkdownRaw => {
            let (label, help) = if app.is_deck() {
                (app.deck_view_hint(), app.deck_view_help())
            } else if app.preview_is_diff_representation() {
                (Some(M::HintReturnToDiff), Some(M::HintReturnToDiff))
            } else if app.is_decorated_kind() {
                (
                    Some(if app.is_md_raw() {
                        M::HintRendered
                    } else {
                        M::HintRawSource
                    }),
                    Some(M::MdRawToggleHelp),
                )
            } else {
                (None, None)
            };
            t.label = label;
            t.help = help;
        }
        A::ImageReturnToDiff => {
            if !app.preview_is_diff_representation() {
                t.label = None;
                t.help = None;
            }
        }
        A::PreviewCopySelectionRef => {
            t.help = Some(if app.is_windowed() && !app.is_document() {
                M::AtRefHelp
            } else {
                M::AtRefPathHelp
            });
        }
        A::PreviewCopySelection => {
            t.label = Some(if app.preview_visual_linewise() {
                M::FtCopyLines
            } else {
                M::CopyHint
            });
        }
        A::LinkFocusNext | A::LinkFocusPrev => {
            t.help = Some(if app.is_document() {
                M::FocusDocLink
            } else {
                M::FocusMdLink
            });
        }
        A::LinkOpen => {
            t.help = Some(if app.is_document() {
                M::OpenDocLinkHint
            } else {
                M::OpenLinkHint
            });
            t.label = match app.md_focused_kind() {
                Some(MdFocus::LocalLink) => Some(M::HintOpen),
                Some(MdFocus::AnchorLink) => Some(M::HintJump),
                Some(MdFocus::ExternalLink) => Some(M::HintBrowser),
                Some(MdFocus::Task) | Some(MdFocus::Details) => Some(M::HintToggle),
                Some(MdFocus::MermaidFence) => Some(M::HintFullScreen),
                Some(MdFocus::CodeBlock) | None => None,
            };
        }
        A::OpenLinkNewTab if app.md_focused_kind() != Some(MdFocus::LocalLink) => {
            t.label = None;
        }
        #[cfg(feature = "git")]
        A::CycleDiffLayout => {
            if app.media_diff_showing_pictures() {
                t.label = Some(app.media_diff_layout_next_msg());
                t.help = Some(M::MediaLayoutCycleHelp);
            }
        }
        #[cfg(feature = "git")]
        A::CycleDiffView => {
            t.label = app.diff_view_cycle_hint();
            t.help = app.diff_view_help_hint();
        }
        #[cfg(feature = "git")]
        A::JumpNextChange | A::JumpPrevChange if ctx == Context::Diff => {
            t.label = Some(M::HintNextPrevFile);
        }
        _ => {}
    }
    t
}
