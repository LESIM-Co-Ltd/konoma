// The host side of the preview screens' description (`konoma_host_core::preview_plan`): what the
// footer plans and the help declares for a preview is described there without any state; this
// reads the open file's state into a `PreviewState` and maps the description's names to this
// host's `Action`s and `Msg`s.

use konoma_host_core::keymap::CoreContext;
use konoma_host_core::preview_plan::{
    help_everywhere, help_order, Focus, PreviewOp, PreviewState, PvDyn, PvGroup,
};

use crate::app::{App, MdFocus};
use crate::keymap::{host_op_action, Action, Context, Motion};
use crate::ops::decl::{repr_label, text};

/// The state of the open file the preview screens' plans and names depend on.
pub fn state(app: &App, ctx: Context) -> PreviewState {
    let focus = match app.md_focused_kind() {
        Some(MdFocus::Task) => Some(Focus::Task),
        Some(MdFocus::Details) => Some(Focus::Details),
        Some(MdFocus::CodeBlock) => Some(Focus::CodeBlock),
        Some(MdFocus::MermaidFence) => Some(Focus::MermaidFence),
        Some(MdFocus::LocalLink) => Some(Focus::LocalLink),
        Some(MdFocus::AnchorLink) => Some(Focus::AnchorLink),
        Some(MdFocus::ExternalLink) => Some(Focus::ExternalLink),
        None => None,
    };
    PreviewState {
        text_missing: app.document_text_missing(),
        decorated: app.document_ready() && !app.is_raw_source(),
        focus,
        // hjkl/0 pan a diagram only once zoomed AND fully on screen.
        mermaid_pannable: focus == Some(Focus::MermaidFence)
            && app.fence_zoom_level() > 1.001
            && app.focused_fence_fully_visible(),
        windowed: app.is_windowed(),
        office_document: app.is_document(),
        edit_named: text(app, ctx, Action::RequestEdit).label.is_some(),
        repr_step: repr_label(app, Action::CycleRepresentation).is_some(),
        repr_text: text(app, ctx, Action::CycleRepresentation).label.is_some(),
        returns_to_diff: app.preview_is_diff_representation(),
    }
}

/// The host operation an operation of the description is (`None` for the ones this build does
/// not have: the project's tool and the diff scope need the `git` feature).
pub fn action_of(op: PreviewOp) -> Option<Action> {
    match op {
        PreviewOp::Core(a) => Some(Action::from(a)),
        PreviewOp::Host(o) => host_op_action(o),
        PreviewOp::Cancel => Some(Action::Cancel),
        PreviewOp::TabNew => Some(Action::TabNew),
        PreviewOp::ToggleTableCell => Some(Action::ToggleTableCell),
        PreviewOp::LinkOpen => Some(Action::LinkOpen),
        #[cfg(feature = "git")]
        PreviewOp::ToggleFollowDiffScope => Some(Action::ToggleFollowDiffScope),
        #[cfg(not(feature = "git"))]
        PreviewOp::ToggleFollowDiffScope => None,
    }
}

/// The operations of a footer group, as the footer plan holds them.
pub fn group(g: PvGroup) -> &'static [Action] {
    use Action as A;
    match g {
        PvGroup::Scroll => &[A::Navigate(Motion::Down), A::Navigate(Motion::Up)],
        PvGroup::Ends => &[A::Navigate(Motion::Top), A::Navigate(Motion::Bottom)],
        PvGroup::Home => &[A::Navigate(Motion::LineHome)],
        PvGroup::Hjkl => &[
            A::Navigate(Motion::Left),
            A::Navigate(Motion::Down),
            A::Navigate(Motion::Up),
            A::Navigate(Motion::Right),
        ],
        PvGroup::Tabs => &[A::TabPrev, A::TabNext],
        PvGroup::FileJump => &[A::PreviewFileNext, A::PreviewFilePrev],
        PvGroup::Zoom => &[A::ImageZoomIn, A::ImageZoomOut],
        PvGroup::Select => &[A::PreviewEnterVisual, A::PreviewEnterVisualLine],
        PvGroup::ToggleItem => &[A::ToggleFocusedItem, A::LinkOpen],
    }
}

/// The token function of a dynamic footer entry.
pub fn dyn_token(d: PvDyn) -> fn(&App, Context) -> Option<String> {
    match d {
        PvDyn::SearchStatus => crate::ops::footer::search_status,
        PvDyn::ChangeStatus => crate::ops::footer::change_status,
        PvDyn::CopyCode => crate::ops::footer::copy_code_token,
    }
}

/// The footer plan's core context of a preview screen.
pub fn core_of(ctx: Context) -> Option<CoreContext> {
    crate::ops::core_help::core_ctx(ctx)
}

/// What the help declares of a preview screen (the screen's own operations, then the ones every
/// screen lists), as host operations.
pub fn declared(app: &App, ctx: Context) -> Option<(Vec<Action>, Vec<Action>)> {
    let cctx = core_of(ctx)?;
    let st = state(app, ctx);
    let lower =
        |v: Vec<PreviewOp>| -> Vec<Action> { v.into_iter().filter_map(action_of).collect() };
    Some((lower(help_order(cctx, &st)), lower(help_everywhere())))
}
