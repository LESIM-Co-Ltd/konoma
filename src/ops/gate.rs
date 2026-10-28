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

/// Whether `a` acts in `ctx` right now.
pub fn action_enabled(app: &App, ctx: Context, a: Action) -> bool {
    gate(app, ctx, a).is_on()
}

/// See the module comment.
pub fn gate(app: &App, ctx: Context, a: Action) -> Gate {
    use Action as A;
    // A backend that only reads is asked politely and answers here, once, rather than each write
    // discovering it separately and failing in its own way.
    #[cfg(feature = "git")]
    if a.writes_repository() && !crate::vcs::caps(&app.tab.root).write {
        return Gate::Off(Some(Msg::VcsReadOnly));
    }
    match a {
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
        A::PageNext | A::PagePrev => when(match app.tab.preview_kind {
            #[cfg(feature = "git")]
            Some(PreviewKind::GitDiff(_)) => app.media_diff_can_page(),
            _ => app.can_turn_pages(),
        }),

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

        // --- `e` and `o` ---
        A::RequestEdit => gate_edit(app),
        A::OpenExternal => gate_open_external(app),
        // `Ctrl-t` on the changes list opens the row's file; in the tree it has always acted.
        #[cfg(feature = "git")]
        A::OpenInNewTab if matches!(ctx, Context::Screen(..)) => {
            when(matches!(app.verb_target(), VerbTarget::File(_)))
        }

        // --- `!`: the project's tool. The version-control screens always have a repository. ---
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
        // `n` / `N` step through the change blocks of the file: only while it has some.
        #[cfg(feature = "git")]
        A::DiffNextChange | A::DiffPrevChange => when(app.diff_has_change_blocks()),
        #[cfg(feature = "git")]
        A::CycleDiffView => when(app.diff_view_cycle_hint().is_some()),
        #[cfg(feature = "git")]
        A::ToggleFollowDiffScope => when(app.follow_diff_scope_msg().is_some()),
        #[cfg(feature = "git")]
        A::CycleDiffLayout => {
            if ctx == Context::Diff {
                // The media diff cycles its own layout; the line diff has none while it is shown
                // as rendered blocks or as a summary of a binary file.
                when(
                    app.media_diff_showing_pictures()
                        || (!app.diff_footer_is_media_or_summary() && !app.diff_rendered_active()),
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
    if binding.writes && app.plugin_is_read_only(p.plugin()) {
        return Gate::Off(Some(Msg::PluginReadOnly));
    }
    Gate::On
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
            if crate::app::office_open::office_kind(&p).is_some() {
                if app.cfg.external.office_apps {
                    Gate::On
                } else {
                    Gate::Off(Some(Msg::OfficeAppsDisabled))
                }
            } else if app.cfg.external.open_links {
                Gate::On
            } else {
                Gate::Off(Some(Msg::ExternalOpenLinksDisabled))
            }
        }
        VerbTarget::Dir => Gate::Off(Some(Msg::NotAFile)),
        VerbTarget::None => Gate::Off(Some(Msg::NoFileToOpenApp)),
    }
}

/// `o`: the hub opens only in a repository the backend can show (mirrors `App::open_git_view`).
fn gate_open_git_view(app: &App) -> Gate {
    let backend_is_git = app.git_vcs == crate::vcs::VcsKind::Git;
    if backend_is_git && !app.cfg.external.git {
        return Gate::Off(Some(Msg::ExternalGitDisabled));
    }
    #[cfg(feature = "git")]
    if backend_is_git && !crate::git::git_binary_available() {
        return Gate::Off(Some(Msg::GitNotInstalled));
    }
    // Until the first scan lands the cache says nothing; the handler then decides.
    if app.git_state_known() && app.git_branch().is_none() {
        return Gate::Off(Some(Msg::NotAGitRepo));
    }
    Gate::On
}
