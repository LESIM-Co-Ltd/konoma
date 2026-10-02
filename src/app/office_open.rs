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
        "docx" | "docm" | "dotx" | "dotm" | "doc" | "odt" => Some(OfficeKind::Word),
        "xlsx" | "xlsm" | "xltx" | "xltm" | "xlsb" | "xls" | "ods" => Some(OfficeKind::Excel),
        "pptx" | "pptm" | "ppsx" | "potx" | "ppt" | "odp" => Some(OfficeKind::PowerPoint),
        _ => None,
    }
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
                Ok(Some(status)) => return Ok(status.code()),
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

    /// Where `e` goes: an Office document without an explicit `[editor] ext` entry is opened in a GUI
    /// app (returns `true` = handled here); everything else returns `false` and takes the `$EDITOR`
    /// path unchanged.
    pub(super) fn try_open_in_office(&mut self, path: &Path) -> bool {
        let Some(kind) = office_kind(path) else {
            return false;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        // An explicit per-extension editor always wins (the user asked for it).
        if self
            .cfg
            .editor
            .ext
            .get(&ext)
            .is_some_and(|c| !c.trim().is_empty())
        {
            return false;
        }
        if !self.cfg.external.office_apps {
            self.flash = Some(tr(self.lang, Msg::OfficeAppsDisabled).into());
            return true;
        }
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
        let word = ["docx", "docm", "dotx", "dotm", "doc", "odt"];
        let excel = ["xlsx", "xlsm", "xltx", "xltm", "xlsb", "xls", "ods"];
        let ppt = ["pptx", "pptm", "ppsx", "potx", "ppt", "odp"];
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
    fn test_default_runner_never_launches() {
        let a = &mac_chain()[0];
        assert!((real_runner())(a).is_err());
    }
}
