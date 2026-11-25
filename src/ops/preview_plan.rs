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

pub fn values(
    app: &App,
    ctx: Context,
) -> Option<konoma_plugin::host::project::preview_presentation::PreviewValues> {
    use konoma_host_core::preview_plan::{footer_plan, PvFoot};
    use konoma_plugin::host::project::preview_presentation::{PreviewBinding, PreviewValues};
    let context = core_of(ctx)?;
    let state = state(app, ctx);
    let mut ops = help_order(context, &state);
    ops.extend(help_everywhere());
    for entry in footer_plan(context, &state) {
        match entry {
            PvFoot::Op(op) | PvFoot::As(op, _) => ops.push(op),
            PvFoot::Group(g, _) | PvFoot::GroupOp(g) | PvFoot::Union(g, _) => ops.extend(g.ops()),
            _ => {}
        }
    }
    let mut bindings: Vec<PreviewBinding> = Vec::new();
    for op in ops {
        if bindings.iter().any(|b| b.op == op) {
            continue;
        }
        let Some(action) = action_of(op) else {
            continue;
        };
        bindings.push(PreviewBinding {
            op,
            keys: crate::ops::keys::keys_of(app, ctx, action),
            enabled: crate::ops::gate::action_enabled(app, ctx, action),
            provided: true,
            label: repr_label(app, action).or_else(|| {
                text(app, ctx, action)
                    .label
                    .map(|m| crate::i18n::tr(app.lang, m).into())
            }),
            help: text(app, ctx, action)
                .help
                .map(|m| crate::i18n::tr(app.lang, m).into()),
            launch: crate::ops::decl::decl(action).launch,
            layers: crate::ops::keys::keys_of(app, ctx, action)
                .into_iter()
                .filter_map(|key| {
                    app.keymaps
                        .layer_of_key_on(ctx, app.screen_keys_for(ctx), key)
                        .map(|layer| (key, layer))
                })
                .collect(),
        });
    }
    Some(PreviewValues {
        context,
        state,
        bindings,
        core: crate::ops::core_help::facts_for(app, ctx),
        search_status: crate::ops::footer::search_status(app, ctx),
        change_status: crate::ops::footer::change_status(app, ctx),
        copy_code: crate::ops::footer::copy_code_token(app, ctx),
    })
}

/// Help and menus describe their underlying screen; other overlays keep their own plans.
/// In particular a project opened over Preview is a new front screen, not a Preview surface.
pub fn front_values(
    app: &App,
    ctx: Context,
) -> Option<konoma_plugin::host::project::preview_presentation::PreviewValues> {
    if app.project_active()
        || app.dialog_is_input()
        || app.dialog_is_confirm()
        || app.dialog_is_preview()
        || app.is_bookmark_list()
        || app.is_tab_list()
        || app.is_info()
        || app.is_outline()
        || app.is_table_cell_open()
        || app.is_visual()
        || app.is_filtering()
        || app.is_searching()
        || app.is_marking()
        || app.is_plugin_input()
        || !app.preview_is_diff_representation()
        || app.tab.project_return.is_none()
        || ctx != app.base_context()
    {
        return None;
    }
    values(app, ctx)
}

pub fn help_words(
    app: &App,
) -> konoma_plugin::host::project::preview_presentation::PreviewHelpWords {
    use crate::i18n::{tr, Msg};
    use konoma_plugin::host::project::preview_presentation::PreviewHelpWords;
    let word = |msg| tr(app.lang, msg).into();
    PreviewHelpWords {
        search: word(Msg::SearchHint),
        table_search: word(Msg::TableSearchHelp),
        bookmarks: word(Msg::PreviewBookmarkHint),
        zoom: word(Msg::MermaidZoomHelp),
        tasks: word(Msg::MdTaskToggleHelp),
        details: word(Msg::HintDetailsToggle),
        tool: word(Msg::GitExternalTool),
        tabs: word(Msg::HelpTabs),
    }
}
