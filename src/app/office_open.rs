//! `e` on an Office document: open it in a GUI app instead of handing the zip to `$EDITOR`.
//!
//! The chain of attempts (a Microsoft app, LibreOffice, the OS default) is built by the pure
//! [`plan`] and walked by [`run_chain`] on a **worker thread** (waiting on an exit code can take
//! seconds; design principle #4), so the UI never blocks. The process launch itself goes through a
//! [`Runner`] seam: tests install a fake and never start a real process.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::App;
use crate::i18n::{tr, Msg};

/// Which Office application family a document belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
}

/// The kind for `path`'s extension (case-insensitive), or `None` for anything that is not an Office
/// document (csv, rtf, txt... stay with `$EDITOR`).
pub(crate) fn office_kind(path: &Path) -> Option<OfficeKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "docm" | "dotx" | "dotm" | "doc" | "odt" | "ott" => Some(OfficeKind::Word),
        "xlsx" | "xlsm" | "xltx" | "xltm" | "xlsb" | "xls" | "ods" => Some(OfficeKind::Excel),
        "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" | "ppt" | "odp" | "otp" => {
            Some(OfficeKind::PowerPoint)
        }
        _ => None,
    }
}

/// What `e` does for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditTarget {
    /// The external editor (`$EDITOR` / `[editor]`).
    Editor,
    /// An Office app of this kind (the platform chain in [`plan`]).
    OfficeApp(OfficeKind),
    /// An Office document while `[external] office_apps = false`: `e` only flashes why.
    OfficeDisabled,
}

/// Which platform's chain to build. A parameter (not `cfg!`) so both chains are testable anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OfficeOs {
    MacOs,
    Other,
}

impl OfficeOs {
    pub(crate) const fn current() -> Self {
        if cfg!(target_os = "macos") {
            OfficeOs::MacOs
        } else {
            OfficeOs::Other
        }
    }
}

/// What a successful attempt opened the document with (drives the flash text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opener {
    Microsoft(OfficeKind),
    LibreOffice,
    SystemDefault,
}

/// One command line to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attempt {
    pub opener: Opener,
    pub prog: String,
    pub args: Vec<String>,
}

/// Bundle ids of the Microsoft apps for macOS. `com.microsoft.Word` / `com.microsoft.Excel` /
/// `com.microsoft.Powerpoint` (note: lowercase "p" in `Powerpoint`) are the apps' preference
/// domains = bundle ids, as used in Microsoft's own `defaults write` guidance, e.g. the Microsoft Q&A
/// answer https://learn.microsoft.com/en-us/answers/questions/5004550/how-do-i-disable-dark-mode-in-365-for-mac
fn microsoft_bundle_id(kind: OfficeKind) -> &'static str {
    match kind {
        OfficeKind::Word => "com.microsoft.Word",
        OfficeKind::Excel => "com.microsoft.Excel",
        OfficeKind::PowerPoint => "com.microsoft.Powerpoint",
    }
}

/// LibreOffice's bundle id on macOS (read from `/Applications/LibreOffice.app/Contents/Info.plist`).
const LIBREOFFICE_BUNDLE_ID: &str = "org.libreoffice.script";

/// The ordered attempts for opening `path`: macOS = `open -b <Microsoft id>`, `open -b <LibreOffice
/// id>`, `open`; elsewhere = `libreoffice`, `soffice`, `xdg-open` (Microsoft Office has no Linux
/// build). A missing app shows up as a spawn error / non-zero exit, which moves to the next attempt.
pub(crate) fn plan(os: OfficeOs, kind: OfficeKind, path: &Path) -> Vec<Attempt> {
    let p = path.to_string_lossy().into_owned();
    let mk = |opener, prog: &str, args: &[&str]| Attempt {
        opener,
        prog: prog.to_string(),
        args: args
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once(p.clone()))
            .collect(),
    };
    match os {
        OfficeOs::MacOs => vec![
            mk(
                Opener::Microsoft(kind),
                "open",
                &["-b", microsoft_bundle_id(kind)],
            ),
            mk(Opener::LibreOffice, "open", &["-b", LIBREOFFICE_BUNDLE_ID]),
            mk(Opener::SystemDefault, "open", &[]),
        ],
        OfficeOs::Other => vec![
            mk(Opener::LibreOffice, "libreoffice", &[]),
            mk(Opener::LibreOffice, "soffice", &[]),
            mk(Opener::SystemDefault, "xdg-open", &[]),
        ],
    }
}

/// How a launch went: `Ok(Some(code))` = the process exited with `code`; `Ok(None)` = still running
/// after the wait budget (a GUI app that stays attached = launched); `Err` = could not start.
pub(crate) type Runner = Arc<dyn Fn(&Attempt) -> io::Result<Option<i32>> + Send + Sync>;

/// Walk `attempts` in order; the first that starts and does not exit non-zero wins. On total failure
/// returns the last failure's reason.
pub(crate) fn run_chain(
    attempts: &[Attempt],
    runner: &(dyn Fn(&Attempt) -> io::Result<Option<i32>> + Send + Sync),
) -> Result<Opener, String> {
    let mut last = String::from("no application to try");
    for a in attempts {
        match runner(a) {
            Ok(None) | Ok(Some(0)) => return Ok(a.opener),
            Ok(Some(code)) => last = format!("{} exited with status {code}", a.prog),
            Err(e) => last = format!("{}: {e}", a.prog),
        }
    }
    Err(last)
}

/// The exit code of a process that has finished. One killed by a signal has no code
/// (`ExitStatus::code` is `None`); that is a failed launch, not a program that is "still running",
/// so it maps to a non-zero code (the shell convention, 128 + the signal) and the chain goes on to
/// the next application.
pub(crate) fn status_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(sig)) => 128 + sig,
        // Stopped / unknown: still not a success.
        (None, None) => -1,
    }
}

/// The production runner: starts the process with all stdio detached (a TUI must not be scribbled
/// on), waits up to 3 s for an exit code (`open -b <missing app>` fails fast), and if it is still
/// running treats it as launched and hands the child to a reaper thread so no zombie is left.
#[cfg(not(test))]
pub(crate) fn real_runner() -> Runner {
    Arc::new(|a: &Attempt| {
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        let mut child = std::process::Command::new(&a.prog)
            .args(&a.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Some(status_code(status))),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(None);
                }
                Err(_) => {
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(None);
                }
            }
        }
    })
}

/// Tests must never start a process: the default runner refuses (tests install a fake).
#[cfg(test)]
pub(crate) fn real_runner() -> Runner {
    Arc::new(|_| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
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

    /// What `e` does for `path`. The one decision `try_open_in_office` acts on and the `?` help
    /// row is worded from ([[hint-shown-iff-key-acts]]): an explicit per-extension `[editor] ext`
    /// entry always wins (the user asked for it), then the Office-app switch.
    pub(crate) fn edit_target(&self, path: &Path) -> EditTarget {
        let Some(kind) = office_kind(path) else {
            return EditTarget::Editor;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if self
            .cfg
            .editor
            .ext
            .get(&ext)
            .is_some_and(|c| !c.trim().is_empty())
        {
            return EditTarget::Editor;
        }
        if !self.cfg.external.office_apps {
            return EditTarget::OfficeDisabled;
        }
        EditTarget::OfficeApp(kind)
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
        let path = match self.tab.mode {
            crate::app::Mode::Tree => self
                .tab
                .entries
                .get(self.tab.selected)
                .filter(|e| !e.is_dir)
                .map(|e| e.path.clone()),
            crate::app::Mode::Preview => self.tab.preview_path.clone(),
        };
        match path.map(|p| self.edit_target(&p)) {
            Some(EditTarget::OfficeApp(_)) => Some(office_label),
            Some(EditTarget::OfficeDisabled) => None,
            Some(EditTarget::Editor) | None => Some(editor_label),
        }
    }

    /// Where `e` goes: an Office document without an explicit `[editor] ext` entry is opened in a GUI
    /// app (returns `true` = handled here); everything else returns `false` and takes the `$EDITOR`
    /// path unchanged.
    pub(super) fn try_open_in_office(&mut self, path: &Path) -> bool {
        let kind = match self.edit_target(path) {
            EditTarget::Editor => return false,
            EditTarget::OfficeDisabled => {
                self.flash = Some(tr(self.lang, Msg::OfficeAppsDisabled).into());
                return true;
            }
            EditTarget::OfficeApp(kind) => kind,
        };
        let abs: PathBuf = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let attempts = plan(OfficeOs::current(), kind, &abs);
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
        true
    }

    /// Report a finished attempt via flash. Returns whether anything changed (redraw).
    pub fn apply_office_open(&mut self, r: OfficeOpenResult) -> bool {
        self.flash = Some(match r.outcome {
            Ok(opener) => match opener {
                Opener::Microsoft(k) => tr(self.lang, Msg::OfficeOpenedIn).replace(
                    "{app}",
                    match k {
                        OfficeKind::Word => "Microsoft Word",
                        OfficeKind::Excel => "Microsoft Excel",
                        OfficeKind::PowerPoint => "Microsoft PowerPoint",
                    },
                ),
                Opener::LibreOffice => {
                    tr(self.lang, Msg::OfficeOpenedIn).replace("{app}", "LibreOffice")
                }
                Opener::SystemDefault => tr(self.lang, Msg::OfficeOpenedDefault).to_string(),
            },
            Err(e) => format!("{}{e}", tr(self.lang, Msg::OfficeOpenFailed)),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/x/{name}"))
    }

    #[test]
    fn kind_covers_every_listed_extension_in_any_case() {
        let word = ["docx", "docm", "dotx", "dotm", "doc", "odt", "ott"];
        let excel = ["xlsx", "xlsm", "xltx", "xltm", "xlsb", "xls", "ods"];
        let ppt = [
            "pptx", "pptm", "ppsx", "ppsm", "potx", "potm", "ppt", "odp", "otp",
        ];
        for (list, want) in [
            (&word[..], OfficeKind::Word),
            (&excel[..], OfficeKind::Excel),
            (&ppt[..], OfficeKind::PowerPoint),
        ] {
            for e in list {
                assert_eq!(office_kind(&p(&format!("a.{e}"))), Some(want), "{e}");
                assert_eq!(
                    office_kind(&p(&format!("a.{}", e.to_uppercase()))),
                    Some(want),
                    "{e} upper"
                );
            }
        }
    }

    #[test]
    fn kind_excludes_csv_rtf_txt_and_extensionless() {
        for n in [
            "a.csv",
            "a.rtf",
            "a.txt",
            "a.tsv",
            "a.md",
            "xlsx",
            "a.xlsx.bak",
            ".xlsx",
        ] {
            assert_eq!(office_kind(&p(n)), None, "{n}");
        }
    }

    #[test]
    fn plan_macos_is_microsoft_then_libreoffice_then_open() {
        let path = p("b.xlsx");
        let a = plan(OfficeOs::MacOs, OfficeKind::Excel, &path);
        let want = |opener, args: &[&str]| Attempt {
            opener,
            prog: "open".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(
            a,
            vec![
                want(
                    Opener::Microsoft(OfficeKind::Excel),
                    &["-b", "com.microsoft.Excel", "/tmp/x/b.xlsx"]
                ),
                want(
                    Opener::LibreOffice,
                    &["-b", "org.libreoffice.script", "/tmp/x/b.xlsx"]
                ),
                want(Opener::SystemDefault, &["/tmp/x/b.xlsx"]),
            ]
        );
    }

    #[test]
    fn plan_macos_bundle_ids_per_kind() {
        for (k, id) in [
            (OfficeKind::Word, "com.microsoft.Word"),
            (OfficeKind::Excel, "com.microsoft.Excel"),
            (OfficeKind::PowerPoint, "com.microsoft.Powerpoint"),
        ] {
            let a = plan(OfficeOs::MacOs, k, &p("d.x"));
            assert_eq!(a[0].args[1], id);
            assert_eq!(a[0].opener, Opener::Microsoft(k));
        }
    }

    #[test]
    fn plan_linux_has_no_microsoft_and_ends_with_xdg_open() {
        let a = plan(OfficeOs::Other, OfficeKind::Word, &p("c.docx"));
        let progs: Vec<_> = a.iter().map(|x| x.prog.as_str()).collect();
        assert_eq!(progs, ["libreoffice", "soffice", "xdg-open"]);
        assert!(a.iter().all(|x| x.args == ["/tmp/x/c.docx"]));
        assert!(!a.iter().any(|x| matches!(x.opener, Opener::Microsoft(_))));
        assert_eq!(a[2].opener, Opener::SystemDefault);
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

    #[test]
    fn chain_microsoft_present_stops_there() {
        let (r, log) = fake(|_| Ok(Some(0)));
        assert_eq!(
            run_chain(&mac_chain(), &*r),
            Ok(Opener::Microsoft(OfficeKind::Excel))
        );
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[test]
    fn chain_falls_to_libreoffice_when_microsoft_is_missing() {
        let (r, log) = fake(|a| {
            if a.args[1] == "com.microsoft.Excel" {
                Ok(Some(1))
            } else {
                Ok(None)
            }
        });
        assert_eq!(run_chain(&mac_chain(), &*r), Ok(Opener::LibreOffice));
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn chain_falls_to_default_when_both_are_missing() {
        let (r, _) = fake(|a| {
            if a.opener == Opener::SystemDefault {
                Ok(Some(0))
            } else {
                Ok(Some(1))
            }
        });
        assert_eq!(run_chain(&mac_chain(), &*r), Ok(Opener::SystemDefault));
    }

    #[test]
    fn chain_reports_the_last_failure_when_everything_fails() {
        let (r, log) = fake(|a| {
            if a.opener == Opener::SystemDefault {
                Ok(Some(3))
            } else {
                Err(io::Error::new(io::ErrorKind::NotFound, "nope"))
            }
        });
        let e = run_chain(&mac_chain(), &*r).unwrap_err();
        assert!(e.contains("open exited with status 3"), "{e}");
        assert_eq!(log.lock().unwrap().len(), 3);
    }

    #[test]
    fn chain_linux_skips_missing_binaries() {
        let (r, log) = fake(|a| match a.prog.as_str() {
            "libreoffice" => Err(io::Error::new(io::ErrorKind::NotFound, "not found")),
            "soffice" => Ok(None),
            _ => Ok(Some(0)),
        });
        let chain = plan(OfficeOs::Other, OfficeKind::Word, &p("c.docx"));
        assert_eq!(run_chain(&chain, &*r), Ok(Opener::LibreOffice));
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn empty_chain_is_an_error_not_a_panic() {
        let (r, _) = fake(|_| Ok(Some(0)));
        assert!(run_chain(&[], &*r).is_err());
    }

    #[test]
    fn a_child_killed_by_a_signal_is_a_failure_not_a_launch() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;
        // Raw wait statuses (no process is started): exit(0), exit(3), SIGKILL, SIGSEGV.
        assert_eq!(status_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(status_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(status_code(ExitStatus::from_raw(9)), 137);
        assert_eq!(status_code(ExitStatus::from_raw(11)), 139);
        // And the chain treats it as a failure and moves on to the next application.
        let attempts = mac_chain();
        let calls = std::sync::Mutex::new(0usize);
        let runner = |_: &Attempt| -> io::Result<Option<i32>> {
            let mut n = calls.lock().unwrap();
            *n += 1;
            let st = if *n == 1 {
                ExitStatus::from_raw(9) // killed
            } else {
                ExitStatus::from_raw(0)
            };
            Ok(Some(status_code(st)))
        };
        let opener = run_chain(&attempts, &runner).unwrap();
        assert_eq!(opener, attempts[1].opener);
        assert_eq!(*calls.lock().unwrap(), 2);
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
    fn an_empty_chain_says_there_was_nothing_to_try() {
        let (r, log) = fake(|_| Ok(Some(0)));
        assert_eq!(
            run_chain(&[], &*r),
            Err("no application to try".to_string())
        );
        assert!(log.lock().unwrap().is_empty());
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
        let (mut app, _d, log) = office_app("konoma_office_abs");
        let rel = Path::new("some/dir/b.xlsx");
        assert!(rel.is_relative());
        assert!(app.try_open_in_office(rel));
        let log = log.lock().unwrap();
        assert!(!log.is_empty());
        for a in log.iter() {
            let last = a.args.last().unwrap();
            assert!(Path::new(last).is_absolute(), "{a:?}");
            assert!(last.ends_with("some/dir/b.xlsx"), "{a:?}");
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
