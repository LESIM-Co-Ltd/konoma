//! `e` (edit) and `o` (open in another app) in the app, as konoma does them.
//!
//! The decision is the shared [`crate::launch::resolve_launch`] over the verb the plugin that
//! shows the file declares, `[editor] ext` and `[external]`; the words (footer, help, notices) are
//! the shared ones in [`crate::host::launch`]. What differs from the host is only who runs the
//! process, and every way of starting one goes through a seam a test replaces:
//!
//! - [`LaunchCx::runner`] starts the programs of a GUI chain (on a worker thread in the app);
//! - [`LaunchCx::hand_off`] gives the terminal to a program (the editor, a terminal chain) and
//!   takes it back.
//!
//! [`LaunchCx::new`] starts both as refusals, so nothing starts a process unless the program
//! asks for it ([`LaunchCx::live`]).

use std::io;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use konoma_host_core::editor::EditorConfig;
use konoma_host_core::lang::Lang;

use crate::host::launch::{
    chain_notice, disabled_notice, edit_line, edit_word, editor_failed_notice, EditWord,
};
use crate::launch::{
    disabled_by, resolve_launch, run_chain, walk_chain, with_line, Attempt, Launch, LaunchPolicy,
    OfficeOs, Opener, Runner,
};
use crate::manifest::{LaunchMode, Launcher, Verb, VerbDecl};

/// Gives the terminal to the program of an attempt in the directory, runs it, takes the terminal
/// back and returns its exit code. The closure draws the screen again once the terminal is back.
pub type HandOff = Arc<
    dyn Fn(&Attempt, &Path, &mut dyn FnMut() -> io::Result<()>) -> io::Result<Option<i32>>
        + Send
        + Sync,
>;

/// A seam that refuses: nothing is started unless the program installs a real one.
fn refusing() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "starting programs is not enabled",
    )
}

/// What `e` and `o` need to run in the app.
#[derive(Clone)]
pub struct LaunchCx {
    /// `[editor]`: the editor and its per-extension rules.
    pub editor: EditorConfig,
    /// `[external] office_apps`.
    pub office_apps: bool,
    /// `[external] open_links`.
    pub open_links: bool,
    /// The verbs the plugin that shows the file declares ([`crate::Manifest::verbs`]). After a
    /// `Reopen` the plugin it opened in declares them instead.
    pub verbs: Vec<VerbDecl>,
    /// Starts the programs of a GUI chain.
    pub runner: Runner,
    /// Gives the terminal to a program.
    pub hand_off: HandOff,
    /// A GUI chain is walked on a worker thread (the UI never waits for it). Off, it is walked
    /// at once, which is what a test wants.
    pub worker: bool,
    /// The platform the chains are built for.
    pub os: OfficeOs,
}

impl std::fmt::Debug for LaunchCx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchCx")
            .field("office_apps", &self.office_apps)
            .field("open_links", &self.open_links)
            .field("verbs", &self.verbs)
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}

impl LaunchCx {
    /// The settings and the declared verbs, with no way to start a process.
    pub fn new(
        editor: EditorConfig,
        external: &konoma_host_core::shared::SharedExternal,
        verbs: Vec<VerbDecl>,
    ) -> LaunchCx {
        LaunchCx {
            editor,
            office_apps: external.office_apps,
            open_links: external.open_links,
            verbs,
            runner: Arc::new(|_| Err(refusing())),
            hand_off: Arc::new(|_, _, _| Err(refusing())),
            worker: false,
            os: OfficeOs::current(),
        }
    }

    /// The same with the real ways to start a process: a worker thread for a GUI chain and the
    /// terminal given over to the editor.
    pub fn live(mut self) -> LaunchCx {
        self.runner = crate::launch::spawn_runner();
        self.hand_off = Arc::new(|a, cwd, redraw| {
            konoma_host_core::term::hand_off(&a.prog, &a.args, cwd, true, redraw)?
                .map(|s| Some(crate::launch::status_code(s)))
        });
        self.worker = true;
        self
    }

    /// The same with `runner` starting the programs of a GUI chain.
    pub fn with_runner(mut self, runner: Runner) -> LaunchCx {
        self.runner = runner;
        self
    }

    /// The same with `hand_off` giving the terminal over.
    pub fn with_hand_off(mut self, hand_off: HandOff) -> LaunchCx {
        self.hand_off = hand_off;
        self
    }

    /// The same with the platform the chains are built for.
    pub fn with_os(mut self, os: OfficeOs) -> LaunchCx {
        self.os = os;
        self
    }

    /// The settings the verbs read for `path`.
    pub fn policy(&self, path: &Path) -> LaunchPolicy {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        LaunchPolicy {
            editor_ext_explicit: self
                .editor
                .ext
                .get(&ext)
                .is_some_and(|c| !c.trim().is_empty()),
            office_apps: self.office_apps,
            open_links: self.open_links,
        }
    }

    /// Where `verb` goes for `path`, over the declarations `verbs`. `o` with no declaration opens
    /// with the OS default, as konoma's does; `e` with none opens the editor.
    pub fn resolve(&self, verbs: &[VerbDecl], verb: Verb, path: &Path) -> Launch {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let declared = verbs.iter().find(|v| v.verb == verb).cloned();
        let decl = match (verb, declared) {
            (_, Some(d)) => Some(d),
            (Verb::Open, None) => Some(
                VerbDecl::new(Verb::Open)
                    .launcher(Launcher::OsDefault)
                    .mode(LaunchMode::Gui),
            ),
            (_, None) => None,
        };
        resolve_launch(verb, self.os, decl.as_ref(), &abs, &self.policy(&abs))
    }

    /// What the keys, the footer and the help say now: one answer for all three.
    pub fn facts(&self, verbs: &[VerbDecl], path: &Path) -> LaunchFacts {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        LaunchFacts {
            edit: Some(edit_word(&self.resolve(verbs, Verb::Edit, &abs))),
            open: disabled_by(Verb::Open, &abs, &self.policy(&abs)).is_none(),
        }
    }
}

/// What `e` and `o` do on the screen now. The default is "nothing": a program that gave the loop
/// no [`LaunchCx`] has neither key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaunchFacts {
    /// How `e` is named (`None`: the program has no `e`).
    pub edit: Option<EditWord>,
    /// `o` opens the file.
    pub open: bool,
}

impl LaunchFacts {
    /// `e` starts something (it is not absent and does not only explain).
    pub fn edit_acts(&self) -> bool {
        matches!(self.edit, Some(EditWord::Editor | EditWord::App))
    }
}

/// What a launch left for the loop to do.
#[derive(Debug, Default)]
pub(super) struct Done {
    /// The notice to show.
    pub notice: Option<String>,
    /// The file is read again (the program may have rewritten it).
    pub reload: bool,
    /// The terminal was given over: what was drawn on it must be drawn again.
    pub handed: bool,
}

/// The attempt the editor's command line is, in the shape a chain walks.
fn editor_attempt(argv: &[String]) -> Option<Attempt> {
    let (prog, args) = argv.split_first()?;
    Some(Attempt {
        opener: Opener::SystemDefault,
        prog: prog.clone(),
        args: args.to_vec(),
    })
}

/// A GUI chain: on a worker (its notice arrives on `tx`) or at once.
fn gui(
    lc: &LaunchCx,
    attempts: Vec<Attempt>,
    lang: Lang,
    tx: &Sender<Result<Opener, String>>,
) -> Done {
    if lc.worker {
        let (runner, tx) = (lc.runner.clone(), tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(run_chain(&attempts, &*runner));
        });
        Done::default()
    } else {
        Done {
            notice: Some(chain_notice(lang, &run_chain(&attempts, &*lc.runner))),
            ..Done::default()
        }
    }
}

/// A chain that takes the terminal over, walked like the editor is.
fn terminal(
    lc: &LaunchCx,
    attempts: &[Attempt],
    path: &Path,
    lang: Lang,
    redraw: &mut dyn FnMut() -> io::Result<()>,
) -> Done {
    let cwd = path.parent().unwrap_or(path);
    let outcome = walk_chain(attempts, &mut |a| (lc.hand_off)(a, cwd, redraw));
    match outcome {
        Ok(_) => Done {
            reload: true,
            handed: true,
            ..Done::default()
        },
        Err(e) => Done {
            notice: Some(editor_failed_notice(lang, &e)),
            handed: true,
            ..Done::default()
        },
    }
}

/// `e` on `path`, the caret on `caret` (the file line on screen), over the declarations `verbs`.
pub(super) fn edit(
    lc: &LaunchCx,
    verbs: &[VerbDecl],
    path: &Path,
    caret: Option<usize>,
    lang: Lang,
    tx: &Sender<Result<Opener, String>>,
    redraw: &mut dyn FnMut() -> io::Result<()>,
) -> Done {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let launch = lc.resolve(verbs, Verb::Edit, &abs);
    let line = edit_line(&launch, caret);
    match launch {
        Launch::Editor { .. } => {
            let argv = lc.editor.resolve(&abs, line);
            let Some(attempt) = editor_attempt(&argv) else {
                return Done::default();
            };
            let cwd = abs.parent().unwrap_or(&abs);
            match (lc.hand_off)(&attempt, cwd, redraw) {
                Ok(_) => Done {
                    reload: true,
                    handed: true,
                    ..Done::default()
                },
                Err(e) => Done {
                    notice: Some(editor_failed_notice(lang, &e.to_string())),
                    handed: true,
                    ..Done::default()
                },
            }
        }
        Launch::Terminal(attempts) => terminal(lc, &with_line(&attempts, line), &abs, lang, redraw),
        Launch::Gui(attempts) => gui(lc, attempts, lang, tx),
        Launch::Disabled(d) => Done {
            notice: Some(disabled_notice(lang, d)),
            ..Done::default()
        },
    }
}

/// `o` on `path`, over the declarations `verbs`.
pub(super) fn open(
    lc: &LaunchCx,
    verbs: &[VerbDecl],
    path: &Path,
    lang: Lang,
    tx: &Sender<Result<Opener, String>>,
    redraw: &mut dyn FnMut() -> io::Result<()>,
) -> Done {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match lc.resolve(verbs, Verb::Open, &abs) {
        Launch::Gui(attempts) => gui(lc, attempts, lang, tx),
        Launch::Terminal(attempts) => terminal(lc, &with_line(&attempts, None), &abs, lang, redraw),
        Launch::Disabled(d) => Done {
            notice: Some(disabled_notice(lang, d)),
            ..Done::default()
        },
        // `o` never opens the editor.
        Launch::Editor { .. } => Done::default(),
    }
}
