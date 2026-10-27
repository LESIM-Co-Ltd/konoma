//! The chain of applications a launch verb walks, and the runner that starts them.
//!
//! Shared by konoma (`e` on an Office document, `o`) and the standalone plugin app, so both open a
//! file the same way. The chain is built by the pure [`plan`] / [`attempts_for_verb`] and walked by
//! [`run_chain`]; the process launch itself goes through a [`Runner`] seam. [`spawn_runner`] is the
//! real one. A host that must never start a process in its own tests installs its own refusing
//! runner instead of calling it (a `cfg(test)` here would not reach the host's tests).

use std::io;
use std::path::Path;
use std::sync::Arc;

use crate::manifest::{Launcher, VerbDecl};

/// Which Office application family a document belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
}

/// The kind for `path`'s extension (case-insensitive), or `None` for anything that is not an Office
/// document (csv, rtf, txt... stay with `$EDITOR`).
pub fn office_kind(path: &Path) -> Option<OfficeKind> {
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

/// Which platform's chain to build. A parameter (not `cfg!`) so both chains are testable anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeOs {
    MacOs,
    Other,
}

impl OfficeOs {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            OfficeOs::MacOs
        } else {
            OfficeOs::Other
        }
    }
}

/// What a successful attempt opened the document with (drives the flash text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    Microsoft(OfficeKind),
    LibreOffice,
    SystemDefault,
    /// An application a plugin's own `open` verb names (neither an Office app nor the OS default).
    Declared,
}

/// One command line to try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub opener: Opener,
    pub prog: String,
    pub args: Vec<String>,
}

/// Bundle ids of the Microsoft apps for macOS. `com.microsoft.Word` / `com.microsoft.Excel` /
/// `com.microsoft.Powerpoint` (note: lowercase "p" in `Powerpoint`) are the apps' preference
/// domains = bundle ids, as used in Microsoft's own `defaults write` guidance, e.g. the Microsoft Q&A
/// answer https://learn.microsoft.com/en-us/answers/questions/5004550/how-do-i-disable-dark-mode-in-365-for-mac
pub fn microsoft_bundle_id(kind: OfficeKind) -> &'static str {
    match kind {
        OfficeKind::Word => "com.microsoft.Word",
        OfficeKind::Excel => "com.microsoft.Excel",
        OfficeKind::PowerPoint => "com.microsoft.Powerpoint",
    }
}

/// LibreOffice's bundle id on macOS (read from `/Applications/LibreOffice.app/Contents/Info.plist`).
pub const LIBREOFFICE_BUNDLE_ID: &str = "org.libreoffice.script";

/// The ordered attempts for opening `path`: macOS = `open -b <Microsoft id>`, `open -b <LibreOffice
/// id>`, `open`; elsewhere = `libreoffice`, `soffice`, `xdg-open` (Microsoft Office has no Linux
/// build). A missing app shows up as a spawn error / non-zero exit, which moves to the next attempt.
pub fn plan(os: OfficeOs, kind: OfficeKind, path: &Path) -> Vec<Attempt> {
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

/// The attempts for a plugin's `open` verb on `path`, in the order the verb declares them. The
/// launchers that belong to another platform are left out: a macOS bundle id is reached on macOS
/// only, and where a chain names bundle ids the command names after them are the other
/// platforms' way to the same apps. For the Office plugins' chain this is exactly [`plan`].
/// `kind` names the Office family of `path` (it decides which bundle id is "Microsoft").
pub fn attempts_for_verb(
    os: OfficeOs,
    verb: &VerbDecl,
    kind: Option<OfficeKind>,
    path: &Path,
) -> Vec<Attempt> {
    let p = path.to_string_lossy().into_owned();
    let has_bundle = verb
        .chain
        .iter()
        .any(|l| matches!(l, Launcher::MacApp { .. }));
    let mut out: Vec<Attempt> = Vec::new();
    for l in &verb.chain {
        match l {
            Launcher::MacApp { bundle_id } if os == OfficeOs::MacOs => {
                let opener = match kind {
                    Some(k) if *bundle_id == microsoft_bundle_id(k) => Opener::Microsoft(k),
                    _ if bundle_id == LIBREOFFICE_BUNDLE_ID => Opener::LibreOffice,
                    _ => Opener::Declared,
                };
                out.push(Attempt {
                    opener,
                    prog: "open".to_string(),
                    args: vec!["-b".to_string(), bundle_id.clone(), p.clone()],
                });
            }
            Launcher::Command(argv) if !(os == OfficeOs::MacOs && has_bundle) => {
                let Some((prog, rest)) = argv.split_first() else {
                    continue;
                };
                let opener = if matches!(prog.as_str(), "libreoffice" | "soffice") {
                    Opener::LibreOffice
                } else {
                    Opener::Declared
                };
                out.push(Attempt {
                    opener,
                    prog: prog.clone(),
                    args: rest
                        .iter()
                        .cloned()
                        .chain(std::iter::once(p.clone()))
                        .collect(),
                });
            }
            Launcher::OsDefault => out.push(Attempt {
                opener: Opener::SystemDefault,
                prog: match os {
                    OfficeOs::MacOs => "open",
                    OfficeOs::Other => "xdg-open",
                }
                .to_string(),
                args: vec![p.clone()],
            }),
            // The editor is `e`'s; a bundle id elsewhere than macOS and the other platform's
            // commands are not for this platform.
            _ => {}
        }
    }
    out
}

/// How a launch went: `Ok(Some(code))` = the process exited with `code`; `Ok(None)` = still running
/// after the wait budget (a GUI app that stays attached = launched); `Err` = could not start.
pub type Runner = Arc<dyn Fn(&Attempt) -> io::Result<Option<i32>> + Send + Sync>;

/// Walk `attempts` in order; the first that starts and does not exit non-zero wins. On total failure
/// returns the last failure's reason.
pub fn run_chain(
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
pub fn status_code(status: std::process::ExitStatus) -> i32 {
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
pub fn spawn_runner() -> Runner {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
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

    #[test]
    fn an_empty_chain_says_there_was_nothing_to_try() {
        let (r, log) = fake(|_| Ok(Some(0)));
        assert_eq!(
            run_chain(&[], &*r),
            Err("no application to try".to_string())
        );
        assert!(log.lock().unwrap().is_empty());
    }

    // ---- attempts_for_verb ----

    fn verb(chain: Vec<Launcher>) -> VerbDecl {
        chain
            .into_iter()
            .fold(VerbDecl::new(crate::manifest::Verb::Open), |v, l| {
                v.launcher(l)
            })
    }

    fn mac(id: &str) -> Launcher {
        Launcher::MacApp {
            bundle_id: id.into(),
        }
    }

    fn cmd(argv: &[&str]) -> Launcher {
        Launcher::Command(argv.iter().map(|s| s.to_string()).collect())
    }

    fn office_verb() -> VerbDecl {
        verb(vec![
            mac("com.microsoft.Excel"),
            mac(LIBREOFFICE_BUNDLE_ID),
            cmd(&["libreoffice", "--view"]),
            cmd(&["soffice"]),
            Launcher::OsDefault,
        ])
    }

    #[test]
    fn a_verb_chain_on_macos_uses_the_bundle_ids_and_leaves_the_commands_out() {
        let got = attempts_for_verb(
            OfficeOs::MacOs,
            &office_verb(),
            Some(OfficeKind::Excel),
            &p("b.xlsx"),
        );
        assert_eq!(got, plan(OfficeOs::MacOs, OfficeKind::Excel, &p("b.xlsx")));
    }

    #[test]
    fn a_verb_chain_elsewhere_keeps_the_commands_and_drops_the_bundle_ids() {
        let got = attempts_for_verb(
            OfficeOs::Other,
            &office_verb(),
            Some(OfficeKind::Excel),
            &p("b.xlsx"),
        );
        let line: Vec<_> = got
            .iter()
            .map(|a| (a.opener, a.prog.as_str(), a.args.clone()))
            .collect();
        assert_eq!(
            line,
            vec![
                (
                    Opener::LibreOffice,
                    "libreoffice",
                    vec!["--view".to_string(), "/tmp/x/b.xlsx".to_string()]
                ),
                (
                    Opener::LibreOffice,
                    "soffice",
                    vec!["/tmp/x/b.xlsx".to_string()]
                ),
                (
                    Opener::SystemDefault,
                    "xdg-open",
                    vec!["/tmp/x/b.xlsx".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn commands_alone_are_tried_on_macos_too_and_a_declared_app_is_named_declared() {
        let v = verb(vec![mac("com.example.Viewer"), cmd(&["viewer", "-x"])]);
        let got = attempts_for_verb(OfficeOs::MacOs, &v, None, &p("a.bin"));
        assert_eq!(
            got.len(),
            1,
            "with a bundle id, the commands are the other platforms'"
        );
        assert_eq!(got[0].opener, Opener::Declared);
        assert_eq!(got[0].args, ["-b", "com.example.Viewer", "/tmp/x/a.bin"]);
        let only = verb(vec![cmd(&["viewer", "-x"])]);
        let got = attempts_for_verb(OfficeOs::MacOs, &only, None, &p("a.bin"));
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].opener, got[0].prog.as_str()),
            (Opener::Declared, "viewer")
        );
        assert_eq!(got[0].args, ["-x", "/tmp/x/a.bin"]);
    }

    #[test]
    fn the_microsoft_bundle_id_is_microsoft_only_for_the_kind_of_the_file() {
        let v = verb(vec![mac("com.microsoft.Word")]);
        let word = attempts_for_verb(OfficeOs::MacOs, &v, Some(OfficeKind::Word), &p("a.docx"));
        assert_eq!(word[0].opener, Opener::Microsoft(OfficeKind::Word));
        let excel = attempts_for_verb(OfficeOs::MacOs, &v, Some(OfficeKind::Excel), &p("a.docx"));
        assert_eq!(excel[0].opener, Opener::Declared);
        let none = attempts_for_verb(OfficeOs::MacOs, &v, None, &p("a.docx"));
        assert_eq!(none[0].opener, Opener::Declared);
    }

    #[test]
    fn the_editor_an_empty_command_and_an_empty_chain_give_nothing_to_try() {
        let v = verb(vec![Launcher::Editor, Launcher::Command(Vec::new())]);
        for os in [OfficeOs::MacOs, OfficeOs::Other] {
            assert!(attempts_for_verb(os, &v, None, &p("a")).is_empty());
            assert!(attempts_for_verb(os, &verb(Vec::new()), None, &p("a")).is_empty());
        }
    }

    #[test]
    fn the_os_default_is_open_on_macos_and_xdg_open_elsewhere() {
        let v = verb(vec![Launcher::OsDefault]);
        let m = attempts_for_verb(OfficeOs::MacOs, &v, None, &p("a"));
        let o = attempts_for_verb(OfficeOs::Other, &v, None, &p("a"));
        assert_eq!(
            (m[0].prog.as_str(), m[0].opener),
            ("open", Opener::SystemDefault)
        );
        assert_eq!(
            (o[0].prog.as_str(), o[0].opener),
            ("xdg-open", Opener::SystemDefault)
        );
    }
}
