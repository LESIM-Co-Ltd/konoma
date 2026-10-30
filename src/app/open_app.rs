//! The layer-3 verbs' target, and `o`: open the file in another app.
//!
//! `e` (edit) and `o` (open) act on "the file under the cursor": the selected file in the tree, the
//! file a preview or a diff shows, the row of the changes list. [`App::verb_target`] is that one
//! answer; the key handlers, the `ops::gate` check and the footer/help wording all ask it, so a
//! hint is shown exactly when the key finds something to act on.
//!
//! `o` walks the `open` verb the plugin that shows the file declares (an Office app chain for
//! Office files, the OS default for everything else) on a worker thread, like `e` does for Office
//! files (`office_open`), so the UI never blocks.

use super::office_open::{Launch, OfficeOs};
use super::*;
use crate::i18n::Msg;

/// What the verbs act on right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerbTarget {
    /// A file.
    File(PathBuf),
    /// A directory (neither verb takes one).
    Dir,
    /// Nothing under the cursor (an empty tree, a deleted file in the changes list, a screen
    /// whose rows are not files).
    None,
}

/// The answer of the full plugin resolution for one file, with what it was decided from.
#[derive(Debug)]
pub(crate) struct VerbPluginMemo {
    path: PathBuf,
    stamp: Option<(std::time::SystemTime, u64)>,
    settings: String,
    id: Option<String>,
}

/// (mtime, size) of `path`: a changed file gets a different stamp, so its answer is decided again.
fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl App {
    /// The id of the plugin that takes `path`: the one full decision (`plugins::plugin_id_of`: rules,
    /// content sniff, text probe) that `e`, `o` and the footer/help all read, so they cannot
    /// disagree.
    ///
    /// The file being previewed is not read again: the plugin its preview resolved to is the
    /// answer. Any other file is read only when its (mtime, size) or the settings differ from the
    /// last answer, so the per-frame footer costs one `stat`. `plugin_id_of` passes the preview's
    /// own gate (`is_previewable`) first, so a FIFO or device is never opened.
    pub(crate) fn verb_plugin_id(&self, path: &Path) -> Option<String> {
        if self.tab.mode == Mode::Preview && self.tab.preview_path.as_deref() == Some(path) {
            if let Some(PreviewKind::Plugin { id, .. }) = &self.tab.preview_kind {
                return Some(id.clone());
            }
        }
        let stamp = file_stamp(path);
        let disabled = self.cfg.plugin_disabled();
        let settings = format!("{:?}{:?}", self.cfg.preview.rules, disabled);
        if let Some(m) = self.verb_plugin_memo.borrow().as_ref() {
            if m.path == path && m.stamp == stamp && m.settings == settings {
                return m.id.clone();
            }
        }
        let id = crate::plugins::plugin_id_of(&self.cfg.preview.rules, &disabled, path);
        *self.verb_plugin_memo.borrow_mut() = Some(VerbPluginMemo {
            path: path.to_path_buf(),
            stamp,
            settings,
            id: id.clone(),
        });
        id
    }

    /// The `verb` the plugin that takes `path` declares (`None` = none takes it or it declares no
    /// such verb).
    pub(crate) fn declared_verb(
        &self,
        path: &Path,
        verb: konoma_plugin::Verb,
    ) -> Option<konoma_plugin::VerbDecl> {
        let id = self.verb_plugin_id(path)?;
        crate::plugins::declared_verb_of(&self.cfg.plugin_disabled(), &id, verb)
    }

    /// The file `e` and `o` act on. The changes list takes precedence over what is behind it (the
    /// hub opens over the tree and over a preview); a deleted file has nothing to open. Cheap:
    /// reads cached state only, never the file system (the footer asks every frame). What plugin
    /// takes the target is `verb_plugin_id`'s question, which stats the file and reads it only
    /// when it changed.
    pub(crate) fn verb_target(&self) -> VerbTarget {
        if self.is_git_view() {
            return match self.git_view_entries().get(self.git_view_sel()) {
                Some(e) if e.status != crate::git::FileStatus::Deleted => {
                    VerbTarget::File(e.path.clone())
                }
                _ => VerbTarget::None,
            };
        }
        // The other lists (log, graph, branches, worktrees) have rows that are not files.
        if self.in_git_view() && !self.is_git_diff_preview() {
            return VerbTarget::None;
        }
        match self.tab.mode {
            Mode::Tree => match self.tab.entries.get(self.tab.selected) {
                Some(e) if e.is_dir => VerbTarget::Dir,
                Some(e) => VerbTarget::File(e.path.clone()),
                None => VerbTarget::None,
            },
            Mode::Preview => match &self.tab.preview_path {
                Some(p) => VerbTarget::File(p.clone()),
                None => VerbTarget::None,
            },
        }
    }

    /// `o`: open the file under the cursor in another app. Office files go through the Office app
    /// chain (`[external] office_apps` switches it off); everything else through the OS default
    /// (`[external] open_links`). `[editor] ext` is `e`'s and does not reach here. Flashes why
    /// when there is nothing to open or the way is switched off.
    pub fn open_in_other_app(&mut self) {
        let path = match self.verb_target() {
            VerbTarget::File(p) => p,
            VerbTarget::Dir => {
                self.flash = Some(tr(self.lang, Msg::NotAFile).into());
                return;
            }
            VerbTarget::None => {
                self.flash = Some(tr(self.lang, Msg::NoFileToOpenApp).into());
                return;
            }
        };
        let verb = crate::plugins::open_verb_of(
            &self.cfg.plugin_disabled(),
            self.verb_plugin_id(&path).as_deref(),
        );
        let abs: PathBuf = std::path::absolute(&path).unwrap_or(path);
        let launch = konoma_plugin::launch::resolve_launch(
            konoma_plugin::Verb::Open,
            OfficeOs::current(),
            Some(&verb),
            &abs,
            &self.launch_policy(&abs),
        );
        match launch {
            Launch::Gui(attempts) => self.start_chain(attempts),
            Launch::Terminal(attempts) => self.queue_terminal_chain(&attempts, None, &abs),
            Launch::Disabled(d) => {
                self.flash = Some(konoma_plugin::host::launch::disabled_notice(self.lang, d))
            }
            // `o` never opens the editor.
            Launch::Editor { .. } => {}
        }
    }
}
