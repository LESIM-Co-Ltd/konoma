// Whether an operation acts now: the single place this is decided.
//
// The key handler (`main::dispatch_action`), the footer and the `?` help all ask `gate`, so a hint
// is shown exactly when its key does what the label says. When an operation does not act, `Off`
// carries the message the handler used to flash to refuse (`None` = it used to do nothing).
//
// Everything here must be cheap: the footer asks every frame, and it must not touch the file system
// or run a process (design principle 4). Where the handler's own refusal depends on something
// expensive (a changed file that has since been deleted), the gate answers from the cheap cache
// and the handler stays authoritative: the gate may say `On` for a key the handler then declines,
// but never `Off` for a key that would have acted.

use crate::app::open_app::VerbTarget;
use crate::app::{App, MdFocus};
use crate::i18n::Msg;
use crate::keymap::{Action, Context};
#[cfg(feature = "git")]
use crate::preview::PreviewKind;

/// The answer of [`gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// The operation acts.
    On,
    /// It does not. The message is the reason the handler flashes (`None` = silent).
    Off(Option<Msg>),
}

impl Gate {
    pub fn is_on(self) -> bool {
        self == Gate::On
    }
}

fn when(cond: bool) -> Gate {
    if cond {
        Gate::On
    } else {
        Gate::Off(None)
    }
}

fn diff_gate(app: &App, op: konoma_plugin::host::project::diff_presentation::DiffUiOp) -> Gate {
    when(
        app.diff_presentation_state()
            .enabled(app.diff_presentation_capabilities(), op),
    )
}

/// Whether `a` acts in `ctx` right now.
pub fn action_enabled(app: &App, ctx: Context, a: Action) -> bool {
    gate(app, ctx, a).is_on()
}

/// See the module comment.
pub fn gate(app: &App, ctx: Context, a: Action) -> Gate {
    use Action as A;
    if let Some(plugin) = super::decl::decl(a).writes {
        let verdict = super::write_guard::write_guard(app, plugin, true, plugin == "git");
        if !verdict.is_on() {
            return verdict;
        }
    }
    // A project screen in front covers the screen under it (`App::project_screen_covers`): only
    // the actions on an explicit allow list stay available, so a key bound by config to any other
    // action cannot reach the preview, outline, selection or marks under the screen, and the
    // keys, the footer and the help agree with the layer walk. The project screen's own keys are
    // handled before the gate is asked.
    if app.project_screen_covers() && !allowed_over_a_project_screen_in(app, ctx, a) {
        return Gate::Off(None);
    }
    match a {
        A::Navigate(
            m @ (crate::keymap::Motion::Left
            | crate::keymap::Motion::Right
            | crate::keymap::Motion::LineHome
            | crate::keymap::Motion::LineEnd),
        ) if ctx == Context::Diff => diff_gate(
            app,
            konoma_plugin::host::project::diff_presentation::DiffUiOp::Navigate(m),
        ),
        A::Navigate(
            m @ (crate::keymap::Motion::Left
            | crate::keymap::Motion::Right
            | crate::keymap::Motion::LineHome
            | crate::keymap::Motion::LineEnd),
        ) if ctx == Context::Document && !app.shows_picture_frame_plugin() => when(
            konoma_plugin::host::project::preview_presentation::horizontal_moves(
                app.cfg.ui.wrap,
                app.is_windowed(),
            ) || (m != crate::keymap::Motion::LineEnd
                && crate::ops::preview_plan::state(app, ctx).mermaid_pannable),
        ),
        // --- A plugin's own action: the session's word, the same one the footer and the help read ---
        A::Plugin(p) => gate_plugin(app, ctx, p),

        // --- Opening a cell needs a cell: an empty file has no columns, so `Enter` flashes and the
        // hint and the help row are left out. Closing the popup always acts. ---
        A::ToggleTableCell if ctx == Context::Table => {
            if app.grid().is_some_and(|g| g.ncols() > 0) {
                Gate::On
            } else {
                Gate::Off(Some(Msg::TableCellEmpty))
            }
        }

        // --- What a "page" is depends on the preview (the predicate `App::page_turn` gates on) ---
        A::PageNext | A::PagePrev if ctx == Context::Diff => diff_gate(
            app,
            konoma_plugin::host::project::diff_presentation::DiffUiOp::PageNext,
        ),
        A::PageNext | A::PagePrev => when(match app.tab.preview_kind {
            #[cfg(feature = "git")]
            Some(PreviewKind::GitDiff(_)) => app.media_diff_can_page(),
            _ => app.can_turn_pages(),
        }),

        // --- The picture's own frame (the picture failed or is not there yet) has no text: the
        // motions scroll nothing, so the footer and the help leave them out ---
        A::Navigate(_) if ctx == Context::Text && app.shows_picture_frame_plugin() => {
            Gate::Off(None)
        }

        // --- The decorated document: what the focused item lets a key do ---
        A::LinkOpen => when(matches!(
            app.md_focused_kind(),
            Some(
                MdFocus::LocalLink
                    | MdFocus::AnchorLink
                    | MdFocus::ExternalLink
                    | MdFocus::Task
                    | MdFocus::Details
                    | MdFocus::MermaidFence
            )
        )),
        A::OpenLinkNewTab => when(matches!(
            app.md_focused_kind(),
            Some(MdFocus::LocalLink | MdFocus::AnchorLink | MdFocus::ExternalLink)
        )),
        A::ToggleFocusedItem => when(app.md_focused_task() || app.md_focused_details().is_some()),
        A::CopyCodeBlock => when(app.md_focused_code()),
        // `Tab` moves between the links and checkboxes of a decorated document only.
        A::LinkFocusNext | A::LinkFocusPrev => {
            when(ctx == Context::Document && !app.document_text_missing())
        }
        // A document that has no text yet (still loading, or failed to convert) has nothing to
        // search.
        A::SearchStart | A::SearchNext | A::SearchPrev
            if matches!(ctx, Context::Text | Context::Document) =>
        {
            when(!app.document_text_missing())
        }
        A::ImageZoomIn | A::ImageZoomOut | A::ImageZoomReset => match ctx {
            // The inline diagram of a document is zoomed in place: only while one is focused.
            Context::Text | Context::Document => {
                when(app.md_focused_kind() == Some(MdFocus::MermaidFence))
            }
            _ => Gate::On,
        },
        A::CycleRepresentation => when(
            app.preview_is_diff_representation()
                || ((app.host_source_view() || app.is_deck()) && !app.document_text_missing())
                || app.repr_step().is_some(),
        ),
        A::ImageReturnToDiff => {
            when(app.preview_is_diff_representation() || app.repr_step().is_some())
        }
        // The outline lists the headings of a decorated document; anywhere else the handler
        // answers "no headings". Closing an open outline always acts.
        // On a preview of pages (a deck, a PDF, a workbook) it lists the pages instead, and only
        // when there is more than one to go to.
        A::ToggleOutline => {
            if app.is_outline() {
                Gate::On
            } else if app.is_page_list_preview() {
                if app.page_list_usable() {
                    Gate::On
                } else {
                    Gate::Off(Some(Msg::PageListEmpty))
                }
            } else if app.is_decorated_kind()
                && !app.is_raw_source()
                && !app.document_text_missing()
            {
                Gate::On
            } else {
                Gate::Off(Some(Msg::OutlineEmpty))
            }
        }
        A::PreviewEnterVisual | A::PreviewEnterVisualLine => when(app.is_windowed()),

        // Wrapping is a property of the text and the decorated document.
        A::ToggleWrap => when(matches!(ctx, Context::Text | Context::Document)),

        // On a project canvas `[` and `]` return at once with a single tab, so they do not act
        // there (the menu and the help list a key only when it acts).
        A::TabPrev | A::TabNext if matches!(ctx, Context::Canvas) && app.project_active() => {
            when(app.tab_count() > 1)
        }
        // The help words a version-control screen by its own context.
        #[cfg(feature = "git")]
        A::TabPrev | A::TabNext if matches!(ctx, Context::Screen(..)) && app.project_active() => {
            when(app.tab_count() > 1)
        }

        // --- `e` and `o` ---
        A::RequestEdit => gate_edit(app),
        A::OpenExternal => gate_open_external(app),
        // `Ctrl-t` on the changes list opens the row's file; in the tree it has always acted.
        #[cfg(feature = "git")]
        A::OpenInNewTab if matches!(ctx, Context::Screen(..)) => {
            when(matches!(app.verb_target(), VerbTarget::File(_)))
        }
        // On a project canvas `Ctrl-t` opens the row's target only while the owner marks it
        // launchable — the same cached decision `e`/`o` read, so a deleted file's row offers
        // nothing. A target that is a directory (a worktree row) keeps opening: the owner's
        // launch decision records it as existing too.
        A::OpenInNewTab if ctx == Context::Canvas && app.project_active() => {
            when(app.project_launch_target().is_some())
        }

        // --- `!`: the project's tool. The version-control screens always have a repository. ---
        // On a project canvas the base table's `!` is reachable from every screen, but the old
        // hub bound it on the changes screen alone: the other screens do nothing, silently.
        #[cfg(feature = "git")]
        A::GitLaunchTool
            if matches!(ctx, Context::Canvas)
                && app.project_active()
                && !app.project_tool_screen() =>
        {
            Gate::Off(None)
        }
        #[cfg(feature = "git")]
        A::GitLaunchTool if !matches!(ctx, Context::Screen(..)) => {
            if !app.cfg.external.git_tool {
                Gate::Off(Some(Msg::ExternalGitToolDisabled))
            } else if app.git_state_known() && app.git_branch().is_none() {
                Gate::Off(Some(Msg::NotAGitRepo))
            } else {
                Gate::On
            }
        }

        // --- The tree's version-control keys: refuse where the handler would just flash ---
        A::OpenGitView => gate_open_git_view(app),
        A::OpenGitDiffCursor => match app.tab.entries.get(app.tab.selected) {
            None => Gate::Off(None),
            Some(e) if !e.is_dir && !app.git_state_known() => Gate::On,
            Some(e) if e.is_dir => Gate::Off(Some(Msg::NotAFile)),
            Some(e) if app.git_status_of(&e.path).is_none() => Gate::Off(Some(Msg::NoChanges)),
            Some(_) => Gate::On,
        },
        A::ToggleChangedFilter => {
            if app.changed_filter() || app.git_has_changes() || !app.git_state_known() {
                Gate::On
            } else {
                Gate::Off(Some(Msg::NoChangedFiles))
            }
        }
        A::JumpNextChange | A::JumpPrevChange => {
            if ctx == Context::Tree
                && !app.changed_filter()
                && !app.git_has_changes()
                && app.git_state_known()
            {
                Gate::Off(Some(Msg::NoChangedFiles))
            } else {
                Gate::On
            }
        }

        // --- The diff screen ---
        #[cfg(feature = "git")]
        A::GitDiffDiscard if ctx == Context::Diff => diff_gate(
            app,
            konoma_plugin::host::project::diff_presentation::DiffUiOp::Discard,
        ),
        // `n` / `N` step through the change blocks of the file: only while it has some.
        #[cfg(feature = "git")]
        A::DiffNextChange | A::DiffPrevChange => diff_gate(
            app,
            konoma_plugin::host::project::diff_presentation::DiffUiOp::NextBlock,
        ),
        #[cfg(feature = "git")]
        A::CycleDiffView => diff_gate(
            app,
            konoma_plugin::host::project::diff_presentation::DiffUiOp::View,
        ),
        #[cfg(feature = "git")]
        A::ToggleFollowDiffScope => diff_gate(
            app,
            konoma_plugin::host::project::diff_presentation::DiffUiOp::FollowScope,
        ),
        A::CycleDiffLayout => {
            if ctx == Context::Diff {
                // The media diff cycles its own layout; the line diff has none while it is shown
                // as rendered blocks or as a summary of a binary file.
                diff_gate(
                    app,
                    konoma_plugin::host::project::diff_presentation::DiffUiOp::Layout,
                )
            } else {
                Gate::On
            }
        }

        // --- jj ---
        // A jj workspace is not a git worktree: konoma does not list them yet.
        #[cfg(feature = "git")]
        A::GitOpenWorktrees => {
            if matches!(ctx, Context::Screen(crate::keymap::Backend::Jj, _)) {
                Gate::Off(Some(Msg::JjWorkspacesUnlisted))
            } else {
                Gate::On
            }
        }
        // git's graph already shows every commit; only jj has a narrower default range.
        #[cfg(feature = "git")]
        A::GitGraphToggleAll => {
            if matches!(ctx, Context::Screen(crate::keymap::Backend::Git, _)) {
                Gate::Off(Some(Msg::GraphAlreadyAll))
            } else {
                Gate::On
            }
        }
        #[cfg(feature = "git")]
        A::JjSync => {
            if matches!(ctx, Context::Screen(crate::keymap::Backend::Git, _)) {
                Gate::Off(Some(Msg::JjSyncNotJj))
            } else {
                Gate::On
            }
        }

        _ => Gate::On,
    }
}

/// A declared action of the open plugin: it must belong to this context, the session must say it
/// works (`PluginShow::enabled`), and a writing action is refused for a plugin set to
/// `read_only = true` (one guard for every plugin's writes, with the reason).
fn gate_plugin(app: &App, ctx: Context, p: crate::plugins::PluginAct) -> Gate {
    let Some(binding) = app.plugin_binding_in(ctx, p) else {
        return Gate::Off(None);
    };
    if !app.plugin_act_enabled(p) {
        return Gate::Off(None);
    }
    super::write_guard::write_guard(app, p.plugin(), binding.writes, false)
}

/// [`allowed_over_a_project_screen`] for the layer `ctx` belongs to. A project screen covers the
/// layers BEHIND it and nothing in front of it: an overlay in front (help, the bookmark list, the
/// tab list, Info) keeps its own actions, plus the application-level ones; anything else bound
/// there (a config binding to a preview action) is off, so it cannot reach the hidden screen.
pub(super) fn allowed_over_a_project_screen_in(app: &App, ctx: Context, a: Action) -> bool {
    if app.overlay_in_front_of_project(ctx) {
        return overlay_owns(app, ctx, a) || app_level_over_a_project_screen(a);
    }
    allowed_over_a_project_screen(app, a)
}

/// The actions an overlay owns, from the same sources as its footer and `Space` menu: what its
/// defaults bind (whatever `[keys]` did since), its fixed keys and the operations that have no
/// key. Config-independent, so a rebinding cannot widen it.
fn overlay_owns(app: &App, ctx: Context, a: Action) -> bool {
    // The overlay's own table (not the global one: tabs, quit, ... are the app-level part).
    app.keymaps
        .default_ops
        .get(&ctx)
        .is_some_and(|v| v.contains(&a))
        || super::keys::fixed_bindings(ctx)
            .iter()
            .any(|f| f.action == a)
        || super::menu::unkeyed(ctx).contains(&a)
}

/// The actions that stay available while a project screen covers the screen under it. Everything
/// else is off there, so an action added later cannot act on the hidden preview by default.
/// Written once as two parts: the application-level ones (also kept in an overlay in front of the
/// screen) and the ones that act on the project screen itself.
fn allowed_over_a_project_screen(app: &App, a: Action) -> bool {
    app_level_over_a_project_screen(a) || acts_on_the_project_screen(app, a)
}

/// The application-level part of the allow list: tab, quit, help, paste-jump and refresh
/// commands, and the screen's own motions. None of them reads the screen under (the ones that
/// would are redirected where they run: see `dispatch_action`).
fn app_level_over_a_project_screen(a: Action) -> bool {
    use Action as A;
    matches!(
        a,
        // A key the user disabled does nothing anywhere.
        A::Noop
            // The screen's own motions (it reads them through the same table).
            | A::Navigate(_)
            | A::TabNew
            | A::TabNewAfter
            | A::TabClose
            | A::CloseTabOrQuit
            | A::TabPrev
            | A::TabNext
            // The digit keys: switch to a tab.
            | A::TabGoto(_)
            | A::ToggleTabList
            | A::ToggleHelp
            | A::ToggleFollow
            | A::PasteJump
            | A::Quit
            | A::Refresh
    )
}

/// The part of the allow list that acts on the project screen: leaving it, launching from its own
/// rows, its `Space` menu controls and the project-aware old names.
fn acts_on_the_project_screen(app: &App, a: Action) -> bool {
    use Action as A;
    match a {
        A::OpenInNewTab => true,
        // Leaving the screen, and launching from its own rows.
        A::PreviewBack | A::OpenGitView | A::RequestEdit | A::OpenExternal => true,
        // A control chosen from the project screen's own `Space` menu.
        A::ProjectControl(_) => true,
        // `worktree_close` bound on a project canvas runs the project's own close.
        #[cfg(feature = "git")]
        A::WorktreeClose => true,
        // The bookmark list opens in front of the screen.
        A::MarkJump => true,
        #[cfg(feature = "git")]
        A::GitLaunchTool => true,
        // An old hub name bound on a canvas is read for the open project (never for the screen
        // under): `run_old_hub_name_in_project`.
        #[cfg(feature = "git")]
        A::GitOpenLog
        | A::GitOpenGraph
        | A::GitOpenBranches
        | A::GitOpenWorktrees
        | A::GitWorktreeDiff
        | A::GitCommit
        | A::GitStageAll
        | A::GitUnstageAll
        | A::JjSync
        | A::GitGraphToggleAll
        | A::BranchCreate
        | A::WorktreeCreate => true,
        // A plugin's action only where the plugin declares a project context (its own screen).
        A::Plugin(p) => app
            .plugin_keys
            .declaration(p)
            .is_some_and(|d| d.project_context.is_some()),
        _ => false,
    }
}

/// `e`: the target and whether opening it is possible (mirrors `App::request_edit`).
fn gate_edit(app: &App) -> Gate {
    match app.verb_target() {
        VerbTarget::File(_) => {}
        VerbTarget::Dir => return Gate::Off(Some(Msg::CannotEditDirectory)),
        VerbTarget::None => return Gate::Off(Some(Msg::NoFileToEdit)),
    }
    // `edit_label` is `None` exactly when the target is an Office document and opening those is
    // switched off: `e` then only explains that.
    if app.edit_label(Msg::HintEdit, Msg::HintOpen).is_none() {
        return Gate::Off(Some(Msg::OfficeAppsDisabled));
    }
    Gate::On
}

/// `o`: there is a file under the cursor and the way to open it is not switched off (mirrors
/// `App::open_in_other_app`). An Office document needs `[external] office_apps`, anything else
/// `[external] open_links`. A directory is not opened (no hint, a flash if the key is pressed).
/// Decided from the extension and the cached target only: the footer asks every frame.
fn gate_open_external(app: &App) -> Gate {
    match app.verb_target() {
        VerbTarget::File(p) => {
            match konoma_plugin::launch::disabled_by(
                konoma_plugin::Verb::Open,
                &p,
                &app.launch_policy(&p),
            ) {
                None => Gate::On,
                Some(konoma_plugin::launch::Disabled::OfficeApps) => {
                    Gate::Off(Some(Msg::OfficeAppsDisabled))
                }
                Some(konoma_plugin::launch::Disabled::OpenLinks) => {
                    Gate::Off(Some(Msg::ExternalOpenLinksDisabled))
                }
            }
        }
        VerbTarget::Dir => Gate::Off(Some(Msg::NotAFile)),
        VerbTarget::None => Gate::Off(Some(Msg::NoFileToOpenApp)),
    }
}

/// `p` offers only enabled projects; pending discovery can queue entry without blocking.
fn gate_open_git_view(app: &App) -> Gate {
    if app
        .tab
        .projects
        .projects
        .iter()
        .any(|p| !p.declaration.screens.is_empty() && app.project_is_enabled(&p.plugin))
    {
        return Gate::On;
    }
    let (pending, independent) = if app.tab.projects.pending() {
        crate::plugins::with_registry(&app.cfg.plugin_disabled(), |registry| {
            registry
                .ids()
                .into_iter()
                .filter(|id| registry.is_enabled(id))
                .filter_map(|id| registry.manifest(id)?.project.as_ref())
                .filter(|p| !p.screens.is_empty())
                .fold((false, false), |(_, independent), project| {
                    (true, independent || !project.version_control)
                })
        })
    } else {
        (false, false)
    };
    if independent {
        return Gate::On;
    }
    if !cfg!(feature = "git") {
        return Gate::Off(Some(Msg::NotAGitRepo));
    }
    let backend_is_git = app.git_vcs == crate::vcs::VcsKind::Git;
    if backend_is_git && !app.cfg.external.git {
        return Gate::Off(Some(Msg::ExternalGitDisabled));
    }
    #[cfg(feature = "git")]
    if backend_is_git && !crate::vcs::Git::git_binary_available() {
        return Gate::Off(Some(Msg::GitNotInstalled));
    }
    // Until the first scan lands the cache says nothing; the handler then decides.
    if app.git_state_known() && app.git_branch().is_none() {
        return Gate::Off(Some(Msg::NotAGitRepo));
    }
    if pending {
        Gate::On
    } else {
        Gate::Off(Some(Msg::NotAGitRepo))
    }
}
