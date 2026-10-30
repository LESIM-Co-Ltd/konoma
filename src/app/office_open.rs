//! `e` on an Office document: open it in a GUI app instead of handing the zip to `$EDITOR`.
//!
//! The chain of attempts (a Microsoft app, LibreOffice, the OS default) is built by the pure
//! [`plan`] and walked by [`run_chain`] on a **worker thread** (waiting on an exit code can take
//! seconds; design principle #4), so the UI never blocks. The process launch itself goes through a
//! [`Runner`] seam: tests install a fake and never start a real process.

use std::path::{Path, PathBuf};

use super::App;
use crate::i18n::Msg;

// The pure chain (what to try, in which order, and how a launch is judged) is shared with the
// standalone plugin app; only the App side stays here.
#[cfg(test)]
pub(crate) use konoma_plugin::launch::{attempts_for_verb, plan, OfficeKind};
pub(crate) use konoma_plugin::launch::{
    office_kind, run_chain, Attempt, Launch, LaunchPolicy, OfficeOs, Opener, Runner,
};

/// What `e` does for a file, as the tests word it (a view of [`App::edit_launch`]; the footer, the
/// help and the handler read `host::launch::edit_word` of the same launch).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditTarget {
    /// The external editor (`$EDITOR` / `[editor]`).
    Editor,
    /// An Office app of this kind (the platform chain in [`plan`]).
    OfficeApp(OfficeKind),
    /// A GUI program chain the plugin that shows the file declares for `edit` (not an Office
    /// document).
    DeclaredApp,
    /// An Office document while `[external] office_apps = false`: `e` only flashes why.
    OfficeDisabled,
}

/// The runner the host installs. Tests must never start a process: it refuses there (tests install
/// a fake). This `cfg(test)` seam lives in the host on purpose: one in the shared crate would not
/// apply to the host's tests and they would really launch `open -b ...`.
#[cfg(not(test))]
pub(crate) fn real_runner() -> Runner {
    konoma_plugin::launch::spawn_runner()
}

/// Tests must never start a process: the default runner refuses (tests install a fake).
#[cfg(test)]
pub(crate) fn real_runner() -> Runner {
    std::sync::Arc::new(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "process launching is disabled in tests",
        ))
    })
}

/// A finished background attempt, applied by the run loop.
pub struct OfficeOpenResult {
    outcome: Result<Opener, String>,
}

impl App {
    /// Wire the channel the worker reports on (called by `main`). Without it the chain runs inline.
    pub fn attach_office_opener(&mut self, tx: std::sync::mpsc::Sender<OfficeOpenResult>) {
        self.office_tx = Some(tx);
    }

    /// Replace the process runner (tests).
    #[cfg(test)]
    pub(crate) fn set_office_runner(&mut self, r: Runner) {
        self.office_runner = r;
    }

    /// The settings the launch verbs read, for `path`.
    pub(crate) fn launch_policy(&self, path: &Path) -> LaunchPolicy {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        LaunchPolicy {
            editor_ext_explicit: self
                .cfg
                .editor
                .ext
                .get(&ext)
                .is_some_and(|c| !c.trim().is_empty()),
            office_apps: self.cfg.external.office_apps,
            open_links: self.cfg.external.open_links,
        }
    }

    /// Where `e` goes for `path`: the shared [`konoma_plugin::launch::resolve_launch`] over the
    /// `edit` verb the plugin that shows the file declares, `[editor] ext` and `[external]`. The
    /// declaration comes from the same memoized full resolution `o` uses (`App::verb_plugin_id`),
    /// and `e` acts on the same answer the hint is worded from ([[hint-shown-iff-key-acts]]).
    pub(crate) fn edit_launch(&self, path: &Path) -> Launch {
        let abs: PathBuf = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let decl = self.declared_verb(&abs, konoma_plugin::Verb::Edit);
        konoma_plugin::launch::resolve_launch(
            konoma_plugin::Verb::Edit,
            OfficeOs::current(),
            decl.as_ref(),
            &abs,
            &self.launch_policy(path),
        )
    }

    /// What `e` does for `path` (test view of [`App::edit_launch`]).
    #[cfg(test)]
    pub(crate) fn edit_target(&self, path: &Path) -> EditTarget {
        match self.edit_launch(path) {
            Launch::Editor { .. } | Launch::Terminal(_) => EditTarget::Editor,
            Launch::Disabled(_) => EditTarget::OfficeDisabled,
            Launch::Gui(_) => match office_kind(path) {
                Some(kind) => EditTarget::OfficeApp(kind),
                None => EditTarget::DeclaredApp,
            },
        }
    }

    /// The help row for `e` as it acts *now*: the label to show (`editor_label` when it opens an
    /// editor), or `None` when the key only explains that opening Office documents is switched off
    /// (a row would promise something it does not do). The target is the selected file in the
    /// tree, the shown file in a preview.
    pub fn edit_help_label(&self, editor_label: Msg) -> Option<Msg> {
        self.edit_label(editor_label, Msg::EditInOfficeApp)
    }

    /// What `e` is called now, for any surface (help row or footer hint): `editor_label` when it
    /// opens an editor, `office_label` when it opens an Office app, `None` when it only explains
    /// that Office documents are switched off.
    pub fn edit_label(&self, editor_label: Msg, office_label: Msg) -> Option<Msg> {
        let path = match self.verb_target() {
            super::open_app::VerbTarget::File(p) => Some(p),
            _ => None,
        };
        use konoma_plugin::host::launch::{edit_word, EditWord};
        match path.map(|p| edit_word(&self.edit_launch(&p))) {
            Some(EditWord::App) => Some(office_label),
            Some(EditWord::Off) => None,
            Some(EditWord::Editor) | None => Some(editor_label),
        }
    }

    /// Where `e` goes (see [`App::edit_launch`]): a chain of GUI programs the file's plugin
    /// declares is walked from a worker (returns `true` = handled here); everything that opens the
    /// editor returns `false` and takes the `$EDITOR` path unchanged.
    pub(super) fn try_open_in_office(&mut self, path: &Path) -> bool {
        match self.edit_launch(path) {
            // The terminal hand-over of a declared chain is not wired in the host yet; the
            // editor path is the safe landing.
            Launch::Editor { .. } | Launch::Terminal(_) => false,
            Launch::Disabled(d) => {
                self.flash = Some(konoma_plugin::host::launch::disabled_notice(self.lang, d));
                true
            }
            Launch::Gui(attempts) => {
                self.start_chain(attempts);
                true
            }
        }
    }

    /// Walk `attempts` on a worker (the chain waits on exit codes, which can take seconds; the UI
    /// never blocks) and report the outcome through the channel `main` attached. Without a channel
    /// (the tests) the chain runs inline through the installed [`Runner`].
    pub(super) fn start_chain(&mut self, attempts: Vec<Attempt>) {
        let runner = self.office_runner.clone();
        match self.office_tx.clone() {
            Some(tx) => {
                std::thread::spawn(move || {
                    let outcome = run_chain(&attempts, &*runner);
                    let _ = tx.send(OfficeOpenResult { outcome });
                });
            }
            None => {
                let outcome = run_chain(&attempts, &*runner);
                self.apply_office_open(OfficeOpenResult { outcome });
            }
        }
    }

    /// Report a finished attempt via flash. Returns whether anything changed (redraw).
    pub fn apply_office_open(&mut self, r: OfficeOpenResult) -> bool {
        // The wording is the shared one (the standalone app tells the same).
        self.flash = Some(konoma_plugin::host::launch::chain_notice(
            self.lang, &r.outcome,
        ));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Arc;
    use std::sync::Mutex;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/x/{name}"))
    }

    /// A fake runner: `script(prog, args)` decides; every call is recorded.
    fn fake(
        script: impl Fn(&Attempt) -> io::Result<Option<i32>> + Send + Sync + 'static,
    ) -> (Runner, Arc<Mutex<Vec<Attempt>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        let r: Runner = Arc::new(move |a: &Attempt| {
            l.lock().unwrap().push(a.clone());
            script(a)
        });
        (r, log)
    }

    fn mac_chain() -> Vec<Attempt> {
        plan(OfficeOs::MacOs, OfficeKind::Excel, &p("b.xlsx"))
    }

    // ---- the App side: flash text, the editor-vs-Office decision, the launched command line ----

    use crate::config::Config;
    use crate::i18n::Lang;
    use crate::test_support::unique_tmp;

    /// An English-UI App rooted in a fresh sandbox, with a recording runner (no process starts).
    fn office_app(name: &str) -> (App, crate::test_support::TmpDir, Arc<Mutex<Vec<Attempt>>>) {
        let dir = unique_tmp(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = Config::default();
        cfg.ui.lang = "en".into();
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        let (r, log) = fake(|_| Ok(Some(0)));
        app.set_office_runner(r);
        (app, dir, log)
    }

    fn done(outcome: Result<Opener, String>) -> OfficeOpenResult {
        OfficeOpenResult { outcome }
    }

    #[test]
    fn flash_names_the_exact_application_that_opened_the_document() {
        let (mut app, _d, _) = office_app("konoma_office_flash_names");
        for (opener, want) in [
            (
                Opener::Microsoft(OfficeKind::Word),
                "opened in Microsoft Word",
            ),
            (
                Opener::Microsoft(OfficeKind::Excel),
                "opened in Microsoft Excel",
            ),
            (
                Opener::Microsoft(OfficeKind::PowerPoint),
                "opened in Microsoft PowerPoint",
            ),
            (Opener::LibreOffice, "opened in LibreOffice"),
            (Opener::SystemDefault, "opened with the default app"),
        ] {
            app.flash = None;
            assert!(
                app.apply_office_open(done(Ok(opener))),
                "{opener:?}: redraw"
            );
            assert_eq!(app.flash.as_deref(), Some(want), "{opener:?}");
        }
    }

    #[test]
    fn a_failed_launch_flashes_the_reason_after_the_prefix() {
        let (mut app, _d, _) = office_app("konoma_office_flash_fail");
        app.flash = None;
        assert!(app.apply_office_open(done(Err("xdg-open: not found".into()))));
        assert_eq!(
            app.flash.as_deref(),
            Some("could not open: xdg-open: not found")
        );
    }

    #[test]
    fn flash_is_localised() {
        let (mut app, _d, _) = office_app("konoma_office_flash_ja");
        app.lang = Lang::Jp;
        app.apply_office_open(done(Ok(Opener::LibreOffice)));
        let ja = app.flash.clone().unwrap();
        assert!(ja.contains("LibreOffice"), "{ja}");
        assert_ne!(
            ja, "opened in LibreOffice",
            "the Japanese UI has its own wording"
        );
    }

    #[test]
    fn a_failed_run_reports_why_through_the_flash_end_to_end() {
        let (mut app, d, _) = office_app("konoma_office_fail_flash");
        let book = d.join("b.xlsx");
        std::fs::write(&book, b"x").unwrap();
        let (r, _) = fake(|a| {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} gone", a.prog),
            ))
        });
        app.set_office_runner(r);
        assert!(app.try_open_in_office(&book));
        let shown = app.flash.clone().unwrap();
        // The last attempt of the chain is the OS default; its reason is part of the message.
        let last = plan(OfficeOs::current(), OfficeKind::Excel, &book)
            .pop()
            .unwrap();
        assert_eq!(
            shown,
            format!("could not open: {}: {} gone", last.prog, last.prog)
        );
    }

    #[test]
    fn a_blank_editor_ext_entry_means_not_configured() {
        let (mut app, d, log) = office_app("konoma_office_blank_ext");
        let book = d.join("b.xlsx");
        std::fs::write(&book, b"x").unwrap();
        for blank in ["", " ", "  \t "] {
            app.cfg.editor.ext.insert("xlsx".into(), blank.into());
            assert_eq!(
                app.edit_target(&book),
                EditTarget::OfficeApp(OfficeKind::Excel),
                "{blank:?}"
            );
        }
        app.cfg.editor.ext.insert("xlsx".into(), "  ".into());
        assert!(app.try_open_in_office(&book));
        assert!(!log.lock().unwrap().is_empty(), "the Office chain ran");
        // A real command is "configured".
        app.cfg.editor.ext.insert("xlsx".into(), "myeditor".into());
        assert_eq!(app.edit_target(&book), EditTarget::Editor);
    }

    #[test]
    fn editor_ext_is_matched_case_insensitively_on_the_file_extension() {
        let (mut app, d, log) = office_app("konoma_office_ext_case");
        app.cfg.editor.ext.insert("xlsx".into(), "myeditor".into());
        for name in ["BOOK.XLSX", "Book.XlSx", "book.xlsx"] {
            let p = d.join(name);
            // A real file: the decision passes the preview's gate, which a missing path does not.
            std::fs::write(&p, b"x").unwrap();
            assert_eq!(app.edit_target(&p), EditTarget::Editor, "{name}");
            assert!(
                !app.try_open_in_office(&p),
                "{name}: left to the editor path"
            );
        }
        assert!(log.lock().unwrap().is_empty());
        // Without the entry the upper-case name is an Office document like any other.
        app.cfg.editor.ext.clear();
        assert_eq!(
            app.edit_target(&d.join("BOOK.XLSX")),
            EditTarget::OfficeApp(OfficeKind::Excel)
        );
    }

    #[test]
    fn a_relative_path_is_handed_to_the_application_as_an_absolute_one() {
        let (mut app, d, log) = office_app("konoma_office_abs");
        // A relative spelling of a real file (the gate needs it to exist): up out of the working
        // directory, then down the temporary directory's absolute path.
        std::fs::write(d.join("b.xlsx"), b"x").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let mut rel = PathBuf::new();
        for _ in cwd.components().skip(1) {
            rel.push("..");
        }
        rel.push(d.join("b.xlsx").strip_prefix("/").unwrap());
        let rel = rel.as_path();
        assert!(rel.is_relative());
        assert!(app.try_open_in_office(rel));
        let log = log.lock().unwrap();
        assert!(!log.is_empty());
        for a in log.iter() {
            let last = a.args.last().unwrap();
            assert!(Path::new(last).is_absolute(), "{a:?}");
            assert!(last.ends_with(rel.to_str().unwrap()), "{a:?}");
        }
    }

    #[test]
    fn e_in_the_bookmark_list_opens_an_office_file_in_the_app_not_the_editor() {
        let (mut app, d, log) = office_app("konoma_office_bm_edit");
        let base = unique_tmp("konoma_office_bm_base");
        let book = d.join("b.xlsx");
        std::fs::write(&book, b"x").unwrap();
        std::fs::write(d.join("n.txt"), b"x").unwrap();
        app.bookmarks =
            crate::bookmarks::Bookmarks::with_base(base.to_path_buf(), &app.tab.open_dir);
        app.rebuild_tree().unwrap();
        let idx = |app: &App, n: &str| {
            app.tab
                .entries
                .iter()
                .position(|e| e.path.ends_with(n))
                .unwrap()
        };
        for (mark, file) in [('a', "b.xlsx"), ('b', "n.txt")] {
            app.tab.selected = idx(&app, file);
            app.start_mark_set();
            app.mark_input(mark);
        }
        // The Office file: the chain runs, the editor is not asked for.
        app.open_bookmark_list();
        app.bookmark_list_edit();
        assert!(!app.is_bookmark_list());
        assert!(
            app.take_pending_edit().is_none(),
            "no editor for a workbook"
        );
        assert!(!log.lock().unwrap().is_empty(), "the Office chain ran");
        assert!(
            app.flash.clone().unwrap().contains("opened"),
            "{:?}",
            app.flash
        );
        // A plain file still goes to the editor.
        let before = log.lock().unwrap().len();
        app.open_bookmark_list();
        app.bookmark_list_move(1);
        app.bookmark_list_edit();
        assert_eq!(
            app.take_pending_edit().map(|(p, _)| p).as_deref(),
            Some(d.join("n.txt").as_path())
        );
        assert_eq!(log.lock().unwrap().len(), before);
    }

    #[test]
    fn test_default_runner_never_launches() {
        let a = &mac_chain()[0];
        assert!((real_runner())(a).is_err());
    }
}
