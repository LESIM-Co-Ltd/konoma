//! The terminal handed to someone else and taken back, shared by konoma and the plugin SDK's small
//! host: the lock that orders writes to the terminal, the hand-off of the terminal to a foreground
//! child (an editor, a git tool), the escape sequence that restores the terminal on the way out,
//! and the signal thread that ends the program.
//!
//! This module owns the process-wide statics (the write lock, the foreground-child flag) so that
//! every host in the process uses the same ones: two copies would be two locks, and the exclusion
//! of a frame and an exit signal would silently break. Nothing here knows an application or a
//! renderer; the host passes in what to draw (`redraw`) and what to do on a signal (`on_signal`).

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableBracketedPaste, PopKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
    EnterAlternateScreen,
};

/// Serializes what the program writes to the terminal in one go: a frame (a kitty transmit is a
/// run of chunks that must not have anything written between them) and the deletes of canvas
/// pictures. The main thread holds it for the length of a draw and of a delete flush
/// ([`terminal_write`]); the signal thread takes it, boundedly, before it deletes pictures and
/// restores the terminal, so its escapes never land inside a transmit.
static TERMINAL_WRITE: Mutex<()> = Mutex::new(());

thread_local! {
    /// This thread holds [`TERMINAL_WRITE`] (a panic hook that runs on it must not wait for itself).
    static HOLDS_TERMINAL_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The right to write to the terminal; released on drop. Hold it for a whole draw.
pub struct TerminalWrite {
    _guard: MutexGuard<'static, ()>,
}

impl Drop for TerminalWrite {
    fn drop(&mut self) {
        HOLDS_TERMINAL_WRITE.with(|h| h.set(false));
    }
}

/// Wait for the right to write to the terminal (the main thread's draws).
pub fn terminal_write() -> TerminalWrite {
    let guard = TERMINAL_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    HOLDS_TERMINAL_WRITE.with(|h| h.set(true));
    TerminalWrite { _guard: guard }
}

/// The same, for at most `wait`, and `None` when it was not given in time -- or when this very
/// thread already holds it (a panic inside a draw): the exit path then goes on without it. A
/// draw is a few milliseconds, so a signal normally waits a frame; if the main thread is stuck the
/// exit does not wait for it for good.
pub fn terminal_write_within(wait: Duration) -> Option<TerminalWrite> {
    if HOLDS_TERMINAL_WRITE.with(std::cell::Cell::get) {
        return None;
    }
    let deadline = Instant::now() + wait;
    loop {
        match TERMINAL_WRITE.try_lock() {
            Ok(guard) => {
                HOLDS_TERMINAL_WRITE.with(|h| h.set(true));
                return Some(TerminalWrite { _guard: guard });
            }
            Err(TryLockError::Poisoned(e)) => {
                HOLDS_TERMINAL_WRITE.with(|h| h.set(true));
                return Some(TerminalWrite {
                    _guard: e.into_inner(),
                });
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// The bytes that undo what konoma turned on in the terminal: bracketed paste off, alternate
/// screen left, cursor shown (a hidden cursor would otherwise survive a signal or panic exit;
/// showing an already visible cursor is a no-op, so the normal exit path is unaffected). Raw mode is a termios setting, not an escape sequence (see
/// [`restore_terminal_quietly`]).
pub const TERMINAL_RESTORE_SEQUENCE: &[u8] = b"\x1b[?2004l\x1b[?1049l\x1b[?25h";

/// Writes the terminal-restore sequence to `w` and flushes, ignoring every error: a terminal that
/// is already gone (SIGHUP: the pty's other end is closed, every write fails with `EIO`) leaves
/// nothing to restore. Never panics and never prints (`eprintln!` itself panics on a failed
/// write, and a panic inside a panic hook aborts the process).
pub fn write_terminal_restore(w: &mut dyn Write) {
    let _ = w.write_all(TERMINAL_RESTORE_SEQUENCE);
    let _ = w.flush();
}

/// The one terminal teardown shared by every way out of the TUI (normal exit, SIGTERM / SIGHUP /
/// SIGINT, a fatal panic): raw mode off first (it has more side effects than the screen switch),
/// then the restore sequence on stdout. Every failure is dropped, nothing is printed.
pub fn restore_terminal_quietly() {
    let _ = disable_raw_mode();
    write_terminal_restore(&mut io::stdout());
}

/// True while a foreground child (`$EDITOR`, a git tool, a pager) owns the terminal.
static FOREGROUND_CHILD: AtomicBool = AtomicBool::new(false);

/// Whether a foreground child owns the terminal right now.
pub fn foreground_child_running() -> bool {
    FOREGROUND_CHILD.load(Ordering::SeqCst)
}

/// RAII marker for "a foreground child is running". The flag is cleared in `Drop`, so it comes
/// down on every way out of the wait (an error return or a panic included).
pub struct ForegroundChildGuard;

impl ForegroundChildGuard {
    pub fn new() -> Self {
        FOREGROUND_CHILD.store(true, Ordering::SeqCst);
        ForegroundChildGuard
    }
}

impl Default for ForegroundChildGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ForegroundChildGuard {
    fn drop(&mut self) {
        FOREGROUND_CHILD.store(false, Ordering::SeqCst);
    }
}

/// Whether a caught signal should be ignored. Like a shell or `system(3)` waiting for a child,
/// konoma ignores SIGINT while a foreground child runs: Ctrl-C reaches the whole foreground process
/// group, the child decides what to do with it, and exiting here would tear down the alternate
/// screen under a child that is still drawing. SIGTERM and SIGHUP still end konoma: SIGHUP means
/// the terminal is gone, and SIGTERM is an explicit request to stop; leaving without cleaning up
/// would leak the temp directory.
pub fn ignore_signal(sig: i32, child_running: bool) -> bool {
    sig == signal_hook::consts::SIGINT && child_running
}

/// Runs `on_signal(signal, foreground_child_running)` on a thread of its own for every SIGTERM,
/// SIGHUP and SIGINT the process gets; when it answers `Some(code)` the process exits with `code`
/// (the conventional `128 + signal`), `None` means the signal is ignored (see [`ignore_signal`]).
/// The hook does the host's cleanup (stop its children, remove its files, restore the terminal)
/// and keeps the right to write to the terminal until the process is gone.
#[cfg(unix)]
pub fn install_exit_signals(on_signal: impl Fn(i32, bool) -> Option<i32> + Send + 'static) {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP, SIGINT]) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("konoma-signals".into())
        .spawn(move || {
            for sig in signals.forever() {
                if let Some(code) = on_signal(sig, foreground_child_running()) {
                    std::process::exit(code);
                }
            }
        });
}

#[cfg(not(unix))]
pub fn install_exit_signals(_on_signal: impl Fn(i32, bool) -> Option<i32> + Send + 'static) {}

/// Runs `prog args` in `cwd` with the terminal inherited and waits for it. SIGINT goes to the
/// child, not to us, while it runs (see [`ignore_signal`]).
fn run_child(prog: &str, args: &[String], cwd: &Path) -> io::Result<ExitStatus> {
    let _child = ForegroundChildGuard::new();
    std::process::Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
}

/// The way back into the TUI after the child, written to `out`: begin a synchronized update, enter
/// the alternate screen, `redraw`, end the update, then pop the kitty keyboard protocol, disable
/// mouse reporting (both left by the child) and put bracketed paste back the way it was before the
/// hand-off: turned on again only when `bracketed_paste` says it was on (konoma uses it to receive
/// drag and drop), and left alone otherwise, so a host that never enabled it does not get it.
///
/// The redraw is cosmetic, so its error is dropped; `EndSynchronizedUpdate` is always sent after
/// it so the terminal never waits for the end of a sync.
fn resume<W: Write>(
    out: &mut W,
    redraw: &mut dyn FnMut() -> io::Result<()>,
    bracketed_paste: bool,
) -> io::Result<()> {
    let _ = execute!(out, BeginSynchronizedUpdate);
    execute!(out, EnterAlternateScreen)?;
    let _ = redraw();
    let _ = execute!(out, EndSynchronizedUpdate);
    let _ = execute!(out, PopKeyboardEnhancementFlags, DisableMouseCapture,);
    if bracketed_paste {
        let _ = execute!(out, EnableBracketedPaste);
    }
    Ok(())
}

/// Hands the terminal to `prog` and takes it back: suspends the TUI, runs the child
/// **synchronously** in `cwd` (stdio inherited), restores the TUI and returns the child's exit
/// status (the inner `io::Error` is a launch failure; the outer one a terminal that could not be
/// switched).
///
/// - Suspend: only raw mode is turned off. The alternate screen is **not** left (vim and friends
///   enter it themselves, and with the terminal already on it no screen switch happens, so the
///   plain screen never flashes at launch). They return to the primary screen on exit, so the
///   alternate screen is entered again on the way back.
/// - Restore: raw mode on, then [`resume`] (the switch and the redraw appear atomically through
///   synchronized output, DEC 2026; terminals that lack it ignore it and only the immediate
///   redraw takes effect).
/// - Last, the input the child left behind (terminal-query answers, stray keys) is drained for a
///   moment so the decoder is back in sync.
///
/// `bracketed_paste` is whether bracketed paste was on before the hand-off; it is turned on again
/// only then.
///
/// `redraw` must clear and draw a whole frame, and must hold [`terminal_write`] while it writes
/// (a frame is a run of chunks an exit signal must not land inside). Its error is ignored.
pub fn hand_off(
    prog: &str,
    args: &[String],
    cwd: &Path,
    bracketed_paste: bool,
    redraw: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<io::Result<ExitStatus>> {
    disable_raw_mode()?;
    let status = run_child(prog, args, cwd);
    enable_raw_mode()?;
    resume(&mut io::stdout(), redraw, bracketed_paste)?;
    while event::poll(Duration::from_millis(10)).unwrap_or(false) {
        let _ = event::read();
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    /// `TERMINAL_WRITE` is process-global, so the tests that take it must not overlap: one test
    /// holding it would make another test's "free now" or "refused for 20 ms" check meet the wrong
    /// holder. Poison is ignored so one failure does not cascade.
    static TERMINAL_TESTS: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        TERMINAL_TESTS.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A fake terminal: every write is recorded, and shared with the redraw closure so the
    /// position of the frame among the escapes can be told.
    #[derive(Clone, Default)]
    struct FakeTty(Rc<RefCell<Vec<u8>>>);

    impl Write for FakeTty {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A writer whose every write and flush fails with `EIO`, like a pty whose other end closed.
    struct DeadTty;
    impl Write for DeadTty {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(5))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from_raw_os_error(5))
        }
    }

    fn ansi(c: impl crossterm::Command) -> String {
        let mut s = String::new();
        c.write_ansi(&mut s).unwrap();
        s
    }

    /// The bytes `resume` wrote, split at the frame marker.
    fn resume_trace(redraw_result: io::Result<()>) -> String {
        resume_trace_with(redraw_result, true)
    }

    fn resume_trace_with(redraw_result: io::Result<()>, bracketed_paste: bool) -> String {
        let tty = FakeTty::default();
        let seen = tty.clone();
        let mut out = tty;
        let mut redraw = move || {
            seen.clone().write_all(b"<FRAME>").unwrap();
            redraw_result
                .as_ref()
                .map(|_| ())
                .map_err(|e| io::Error::from(e.kind()))
        };
        resume(&mut out, &mut redraw, bracketed_paste).unwrap();
        let bytes = out.0.borrow().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn the_way_back_is_sync_begin_alt_screen_frame_sync_end_then_the_input_modes() {
        let got = resume_trace(Ok(()));
        let want = [
            ansi(BeginSynchronizedUpdate),
            ansi(EnterAlternateScreen),
            "<FRAME>".to_string(),
            ansi(EndSynchronizedUpdate),
            ansi(PopKeyboardEnhancementFlags),
            ansi(DisableMouseCapture),
            ansi(EnableBracketedPaste),
        ]
        .concat();
        assert_eq!(got, want);
    }

    #[test]
    fn bracketed_paste_stays_off_when_it_was_off_before_the_hand_off() {
        let got = resume_trace_with(Ok(()), false);
        let want = [
            ansi(BeginSynchronizedUpdate),
            ansi(EnterAlternateScreen),
            "<FRAME>".to_string(),
            ansi(EndSynchronizedUpdate),
            ansi(PopKeyboardEnhancementFlags),
            ansi(DisableMouseCapture),
        ]
        .concat();
        assert_eq!(got, want);
        assert!(!got.contains("\x1b[?2004h"), "{got:?}");
        // And on: the paste mode comes back after the frame, last.
        let on = resume_trace_with(Ok(()), true);
        assert!(on.ends_with(&ansi(EnableBracketedPaste)), "{on:?}");
        assert!(on.find("<FRAME>").unwrap() < on.find("\x1b[?2004h").unwrap());
    }

    #[test]
    fn a_failed_redraw_still_ends_the_synchronized_update_and_resets_the_input_modes() {
        let got = resume_trace(Err(io::ErrorKind::BrokenPipe.into()));
        let frame = got.find("<FRAME>").unwrap();
        let end = got.find(&ansi(EndSynchronizedUpdate)).unwrap();
        assert!(frame < end, "{got:?}");
        assert!(got.ends_with(&ansi(EnableBracketedPaste)), "{got:?}");
    }

    #[test]
    fn a_terminal_that_cannot_enter_the_alternate_screen_is_an_error_before_any_frame() {
        let mut redrawn = false;
        let r = resume(
            &mut DeadTty,
            &mut || {
                redrawn = true;
                Ok(())
            },
            true,
        );
        assert!(r.is_err());
        assert!(!redrawn, "no frame is drawn when the screen switch failed");
    }

    #[test]
    fn the_foreground_flag_is_up_while_the_child_runs_and_comes_down_after() {
        let _s = serial();
        assert!(!foreground_child_running());
        let st = run_child("sh", &["-c".into(), "exit 3".into()], Path::new("/")).unwrap();
        assert_eq!(st.code(), Some(3));
        assert!(!foreground_child_running());
        // A program that cannot start is an error inside, and the flag still comes down.
        let missing = run_child("konoma-no-such-program-xyz", &[], Path::new("/"));
        assert!(missing.is_err());
        assert!(!foreground_child_running());
    }

    #[test]
    fn the_child_runs_in_the_given_directory() {
        let _s = serial();
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let st = run_child(
            "sh",
            &[
                "-c".into(),
                "test \"$(pwd -P)\" = \"$0\"".into(),
                dir.display().to_string(),
            ],
            &dir,
        )
        .unwrap();
        assert!(st.success());
    }

    #[test]
    fn sigint_is_ignored_only_while_a_foreground_child_runs() {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        assert!(ignore_signal(SIGINT, true));
        assert!(!ignore_signal(SIGINT, false));
        assert!(
            !ignore_signal(SIGTERM, true),
            "a terminate request still ends konoma"
        );
        assert!(
            !ignore_signal(SIGHUP, true),
            "a closed terminal still ends konoma"
        );
    }

    #[test]
    fn the_foreground_child_flag_comes_down_on_drop_and_on_unwind() {
        let _s = serial();
        {
            let _g = ForegroundChildGuard::new();
            assert!(foreground_child_running());
        }
        assert!(!foreground_child_running());
        let r = std::panic::catch_unwind(|| {
            let _g = ForegroundChildGuard::new();
            panic!("unwind");
        });
        assert!(r.is_err());
        assert!(!foreground_child_running());
    }

    #[test]
    fn terminal_restore_never_panics_on_a_dead_tty() {
        let r = std::panic::catch_unwind(|| write_terminal_restore(&mut DeadTty));
        assert!(r.is_ok());
    }

    #[test]
    fn terminal_restore_turns_off_paste_mode_and_leaves_the_alt_screen() {
        let mut buf = Vec::new();
        write_terminal_restore(&mut buf);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("\x1b[?2004l"), "{text:?}");
        assert!(text.contains("\x1b[?1049l"), "{text:?}");
        assert!(text.contains("\x1b[?25h"), "{text:?}");
    }

    /// The bytes match what crossterm's own commands emit (so they cannot drift from what
    /// `EnableBracketedPaste` / `EnterAlternateScreen` turned on).
    #[test]
    fn terminal_restore_sequence_matches_crossterm_commands() {
        let want = [
            ansi(crossterm::event::DisableBracketedPaste),
            ansi(crossterm::terminal::LeaveAlternateScreen),
            ansi(crossterm::cursor::Show),
        ]
        .concat();
        assert_eq!(TERMINAL_RESTORE_SEQUENCE, want.as_bytes());
    }

    /// The exit path waits for the frame being written and never lands inside it.
    #[test]
    fn the_exit_path_waits_for_the_frame_being_written_and_never_lands_inside_it() {
        let _serial = serial();
        use std::sync::mpsc::channel;
        let wire = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let (go_tx, go_rx) = channel::<()>();
        let (done_tx, done_rx) = channel::<()>();
        let w = wire.clone();
        let exit = std::thread::spawn(move || {
            go_rx.recv().unwrap();
            // The signal thread: waits for the frame, then writes its delete.
            let held = terminal_write_within(Duration::from_secs(30));
            assert!(held.is_some(), "given the terminal once the frame is done");
            w.lock().unwrap().push("delete");
            drop(held);
            done_tx.send(()).unwrap();
        });
        {
            // The main thread: a frame in chunks.
            let _frame = terminal_write();
            wire.lock().unwrap().push("chunk 1");
            go_tx.send(()).unwrap();
            // Had the exit path not waited, it would be done well within this time.
            assert!(
                done_rx.recv_timeout(Duration::from_millis(150)).is_err(),
                "the exit path wrote while a frame was being written"
            );
            wire.lock().unwrap().push("chunk 2");
        }
        done_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        exit.join().unwrap();
        assert_eq!(*wire.lock().unwrap(), ["chunk 1", "chunk 2", "delete"]);
        // A thread that holds the terminal itself (a panic inside a draw) does not wait for
        // itself, and a stuck holder is given up on after the wait.
        let _mine = terminal_write();
        assert!(terminal_write_within(Duration::from_secs(30)).is_none());
        let stuck =
            std::thread::spawn(|| terminal_write_within(Duration::from_millis(20)).is_none());
        assert!(
            stuck.join().unwrap(),
            "the exit goes on without a stuck frame"
        );
    }

    #[test]
    fn a_poisoned_lock_is_still_given_to_the_exit_path() {
        let _serial = serial();
        let _ = std::thread::spawn(|| {
            let _g = terminal_write();
            panic!("a draw panicked");
        })
        .join();
        assert!(terminal_write_within(Duration::from_millis(50)).is_some());
        // And a plain wait still gets it.
        drop(terminal_write());
    }
}
