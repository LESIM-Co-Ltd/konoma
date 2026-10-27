//! Startup query of the terminal's graphics capabilities, run on the calling thread with a hard
//! deadline.
//!
//! This replaces `ratatui_image::picker::Picker::from_query_stdio` (11.x). That function asks the
//! terminal from a **helper thread** and gives up after a timeout, but the thread itself never
//! stops: it stays blocked in a `read(2)` on stdin and swallows the next bytes that arrive (the
//! user's first keys), and it later calls `disable_raw_mode()` on its own schedule. A terminal that
//! does not answer (a detached tmux, a pane without `allow-passthrough`, ...) therefore lost the
//! first key and could stall the main loop's own stdin read.
//!
//! Here nothing outlives the call: the query is written, the replies are read one byte at a time
//! with `poll(2)` bounded by an absolute deadline, and the function returns. Reading one byte at a
//! time means the read stops exactly at the end of the last reply (the Device Status Report, which
//! the query always ends with), so keys typed after it stay in the kernel's tty queue for the event
//! loop. Bytes that arrived *inside* the reply stream but are not part of a reply (keys typed while
//! the query was in flight) are handed back as `stray` so the caller can replay them.
//!
//! The query string and the reply parser are the ones of `ratatui-image` itself
//! (`cap_parser::{Parser::query, Parser::push}`), so what is asked and how it is understood cannot
//! drift; what is re-implemented here is only the I/O (above) and the decision that
//! `Picker::from_query_stdio_with_options` makes from the parsed replies and the environment.

use std::io::{self, Write};
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use ratatui_image::picker::cap_parser::{Parser, QueryStdioOptions, Response};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::FontSize;

/// How long the terminal gets to answer, measured from the moment the query is written.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_millis(2000);

/// What the environment says about the terminal, resolved once from a lookup function (so tests
/// never touch the real environment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TermEnv {
    /// Running inside tmux (the query and any later image sequences must be wrapped).
    pub is_tmux: bool,
    /// The outer terminal's protocol guessed from variables tmux passes through (only inside tmux).
    pub tmux_proto: Option<ProtocolType>,
    /// iTerm2 protocol guessed from `TERM_PROGRAM` / `LC_TERMINAL`.
    pub iterm2_proto: Option<ProtocolType>,
    /// WezTerm / Konsole: kitty graphics and sixel are left out of the query (their
    /// implementations are not usable; WezTerm is driven through iTerm2 instead).
    pub skip_kitty_and_sixel: bool,
}

impl TermEnv {
    pub(crate) fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let non_empty = |k: &str| get(k).is_some_and(|s| !s.is_empty());
        let is_tmux = get("TERM").is_some_and(|t| t.starts_with("tmux"))
            || get("TERM_PROGRAM").is_some_and(|t| t == "tmux");
        // Crude guess from the existence of program specific variables. kitty is not guessed from
        // KITTY_WINDOW_ID (often stale inside tmux): the reply to the query decides that.
        let tmux_proto = if is_tmux {
            [
                ("ITERM_SESSION_ID", ProtocolType::Iterm2),
                ("WEZTERM_EXECUTABLE", ProtocolType::Iterm2),
            ]
            .into_iter()
            .find(|(hint, _)| non_empty(hint))
            .map(|(_, p)| p)
        } else {
            None
        };
        const PROGRAMS: [&str; 9] = [
            "iTerm",
            "WezTerm",
            "mintty",
            "vscode",
            "Tabby",
            "Hyper",
            "rio",
            "Bobcat",
            "WarpTerminal",
        ];
        let by_program =
            get("TERM_PROGRAM").is_some_and(|p| PROGRAMS.iter().any(|n| p.contains(n)));
        let by_lc = get("LC_TERMINAL").is_some_and(|t| t.contains("iTerm"));
        Self {
            is_tmux,
            tmux_proto,
            iterm2_proto: (by_program || by_lc).then_some(ProtocolType::Iterm2),
            skip_kitty_and_sixel: non_empty("WEZTERM_EXECUTABLE") || non_empty("KONSOLE_VERSION"),
        }
    }

    /// The query to send (identical to what `from_query_stdio` writes).
    pub(crate) fn query(&self) -> String {
        let options = QueryStdioOptions {
            blacklist_protocols: if self.skip_kitty_and_sixel {
                vec![ProtocolType::Kitty, ProtocolType::Sixel]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        Parser::query(self.is_tmux, options)
    }
}

/// What came back from the terminal.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    /// The reply stream ended with the Device Status Report; these are the replies before it.
    Replied(Vec<Response>),
    /// Nothing usable before the deadline (or the input closed).
    Silent,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Reply {
    pub answer: Answer,
    /// Bytes read during the query that are not part of any escape sequence (the user's keys).
    pub stray: Vec<u8>,
}

/// Separates the user's own bytes from escape sequences in the reply stream.
#[derive(Default)]
struct StrayFilter {
    state: FilterState,
    stray: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum FilterState {
    #[default]
    Ground,
    Esc,
    Csi,
    /// OSC / APC / DCS: ends with BEL or ESC \.
    Str,
    StrEsc,
}

impl StrayFilter {
    fn push(&mut self, b: u8) {
        use FilterState::*;
        self.state = match (self.state, b) {
            (Ground | Esc, 0x1b) => Esc,
            (Ground, _) => {
                self.stray.push(b);
                Ground
            }
            (Esc, b'[') => Csi,
            (Esc, b']' | b'_' | b'P' | b'^' | b'X') => Str,
            // ESC + any other byte: a two-byte sequence (e.g. Alt+key), dropped.
            (Esc, _) => Ground,
            (Csi, 0x1b) => Esc,
            (Csi, 0x40..=0x7e) => Ground,
            (Csi, _) => Csi,
            (Str, 0x07) => Ground,
            (Str | StrEsc, 0x1b) => StrEsc,
            (Str, _) => Str,
            (StrEsc, b'\\') => Ground,
            (StrEsc, _) => Str,
        };
    }
}

/// Puts the tty's input side into non-canonical, no-echo mode for the duration of the query and
/// restores the exact previous settings on drop. (When the caller is already raw this changes
/// nothing observable.)
struct InputMode {
    fd: RawFd,
    saved: libc::termios,
}

impl InputMode {
    fn enter(fd: RawFd) -> io::Result<Self> {
        // SAFETY: `termios` is plain old data; `tcgetattr` fully initialises it on success.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `saved` is a valid out-pointer for the call.
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut t = saved;
        t.c_lflag &= !(libc::ICANON | libc::ECHO);
        t.c_cc[libc::VMIN] = 1;
        t.c_cc[libc::VTIME] = 0;
        // SAFETY: `t` is a valid termios.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, saved })
    }
}

impl Drop for InputMode {
    fn drop(&mut self) {
        // SAFETY: restores the settings read in `enter`.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
    }
}

/// Waits until `fd` is readable or `remaining` elapses. `Ok(false)` = deadline.
fn wait_readable(fd: RawFd, remaining: Duration) -> io::Result<bool> {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // Round up so a sub-millisecond remainder cannot become a busy loop of 0 ms polls.
    let ms = remaining
        .as_millis()
        .saturating_add(u128::from(
            !remaining.subsec_nanos().is_multiple_of(1_000_000),
        ))
        .min(i32::MAX as u128) as i32;
    loop {
        // SAFETY: one valid pollfd.
        let n = unsafe { libc::poll(&mut p, 1, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        // A hang-up with nothing left to read is reported as readable so the caller's read sees
        // the end of input instead of the loop waiting out the whole deadline.
        return Ok(n > 0 && p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0);
    }
}

/// Reads exactly one byte (`Ok(None)` on end of input or a hung-up terminal).
fn read_byte(fd: RawFd) -> io::Result<Option<u8>> {
    let mut b = 0u8;
    loop {
        // SAFETY: reads at most one byte into a valid one-byte buffer.
        let n = unsafe { libc::read(fd, (&mut b as *mut u8).cast(), 1) };
        match n {
            1 => return Ok(Some(b)),
            0 => return Ok(None),
            _ => {
                let e = io::Error::last_os_error();
                match e.kind() {
                    io::ErrorKind::Interrupted => {}
                    _ if e.raw_os_error() == Some(libc::EIO) => return Ok(None),
                    _ => return Err(e),
                }
            }
        }
    }
}

/// Writes `query` to `out`, then reads the terminal's replies from `in_fd` until the Device Status
/// Report that ends them or until `timeout` after the write. Never reads past that report and
/// never leaves anything running when it returns.
///
/// `Err` is for a tty that cannot be driven at all (not a terminal, write failure) — the same cases
/// in which `from_query_stdio` returned `Err`, which konoma turns into "no image backend".
pub(crate) fn query_terminal(
    in_fd: RawFd,
    out: &mut dyn Write,
    query: &str,
    timeout: Duration,
) -> io::Result<Reply> {
    let _mode = InputMode::enter(in_fd)?;
    out.write_all(query.as_bytes())?;
    out.flush()?;
    let deadline = Instant::now() + timeout;

    let mut parser = Parser::new();
    let mut filter = StrayFilter::default();
    let mut responses = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let byte = if remaining.is_zero() || !wait_readable(in_fd, remaining)? {
            None
        } else {
            read_byte(in_fd)?
        };
        let Some(b) = byte else {
            return Ok(Reply {
                answer: Answer::Silent,
                stray: filter.stray,
            });
        };
        filter.push(b);
        let mut more = parser.push(char::from(b));
        if more[..] == [Response::Status] {
            return Ok(Reply {
                answer: Answer::Replied(responses),
                stray: filter.stray,
            });
        }
        responses.append(&mut more);
    }
}

/// What to build the `Picker` from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PickerSpec {
    /// Cell size in pixels, `None` = the halfblocks default.
    pub font: Option<(u16, u16)>,
    pub proto: ProtocolType,
}

/// The decision `Picker::from_query_stdio_with_options` makes. `ioctl_font` is the cell size from
/// the tty's window size (the fallback used when the terminal does not answer the `16t` query).
pub(crate) fn decide(env: &TermEnv, answer: &Answer, ioctl_font: Option<(u16, u16)>) -> PickerSpec {
    let halfblocks = PickerSpec {
        font: None,
        proto: ProtocolType::Halfblocks,
    };
    let env_proto = env.tmux_proto.or(env.iterm2_proto);
    match answer {
        Answer::Replied(responses) if !responses.is_empty() => {
            let mut proto = None;
            let mut font = None;
            for r in responses {
                match r {
                    Response::Kitty => proto = Some(ProtocolType::Kitty),
                    Response::Sixel => {
                        if proto.is_none() {
                            proto = Some(ProtocolType::Sixel);
                        }
                    }
                    Response::CellSize(Some(wh)) => font = Some(*wh),
                    _ => {}
                }
            }
            match font.or(ioctl_font) {
                Some(font) => PickerSpec {
                    font: Some(font),
                    // The terminal's own answer is authoritative; the environment is a fallback.
                    proto: proto.or(env_proto).unwrap_or(ProtocolType::Halfblocks),
                },
                None => halfblocks,
            }
        }
        // No usable answer: still use the window size and an environment hint if there is one.
        _ => match ioctl_font {
            Some(font) => PickerSpec {
                font: Some(font),
                proto: env_proto.unwrap_or(ProtocolType::Halfblocks),
            },
            None => halfblocks,
        },
    }
}

/// Cell size from the tty's window size, `None` when the terminal reports no pixel size.
pub(crate) fn ioctl_cell_size(fd: RawFd) -> Option<(u16, u16)> {
    // SAFETY: `winsize` is plain old data; TIOCGWINSZ fills it on success.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointer for TIOCGWINSZ.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return None;
    }
    if ws.ws_xpixel == 0 || ws.ws_ypixel == 0 || ws.ws_col == 0 || ws.ws_row == 0 {
        return None;
    }
    Some((ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row))
}

pub(crate) fn build_picker(spec: PickerSpec) -> Picker {
    match spec.font {
        None => Picker::halfblocks(),
        Some((w, h)) => {
            // The only public constructor that takes a cell size; the protocol is set right after.
            #[allow(deprecated)]
            let mut p = Picker::from_fontsize(FontSize::new(w, h));
            p.set_protocol_type(spec.proto);
            p
        }
    }
}

/// Inside tmux, ask tmux to pass escape sequences through to the outer terminal (so the query and
/// images reach it). Best effort; waits for the command like `ratatui-image` does.
fn allow_tmux_passthrough() {
    let _ = std::process::Command::new("tmux")
        .args(["set", "-p", "allow-passthrough", "on"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut c| c.wait());
}

/// Replaces `Picker::from_query_stdio().ok()`: the picker (`None` when the tty cannot be queried)
/// and the bytes the user typed while the query was in flight.
pub(crate) fn picker_from_terminal() -> (Option<Picker>, Vec<u8>) {
    let env = TermEnv::from_lookup(&|k| std::env::var(k).ok());
    if env.is_tmux {
        allow_tmux_passthrough();
    }
    let mut out = io::stdout();
    match query_terminal(libc::STDIN_FILENO, &mut out, &env.query(), QUERY_TIMEOUT) {
        Ok(reply) => {
            let spec = decide(&env, &reply.answer, ioctl_cell_size(libc::STDOUT_FILENO));
            (Some(build_picker(spec)), reply.stray)
        }
        Err(_) => (None, Vec::new()),
    }
}

/// Key events for bytes that were typed during the query (plain text, Enter, Tab, Backspace,
/// Ctrl+letter). Escape sequences were already dropped by the filter.
pub(crate) fn replay_keys(stray: &[u8]) -> Vec<crossterm::event::KeyEvent> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    String::from_utf8_lossy(stray)
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' => Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            '\t' => Some(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            '\u{7f}' | '\u{8}' => Some(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            '\u{1}'..='\u{1a}' => Some(KeyEvent::new(
                KeyCode::Char((b'a' + (c as u8 - 1)) as char),
                KeyModifiers::CONTROL,
            )),
            c if c.is_control() || c == char::REPLACEMENT_CHARACTER => None,
            c => Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};

    const KITTY_OK: &str = "\x1b_Gi=31;OK\x1b\\";
    const DA1_SIXEL: &str = "\x1b[?64;4;28c";
    const CELL_8X16: &str = "\x1b[6;16;8t";
    const DSR: &str = "\x1b[0n";
    /// Waiting out silence is the thing under test; this is only how long "silence" lasts.
    const SHORT: Duration = Duration::from_millis(150);

    fn env_of(pairs: &[(&str, &str)]) -> TermEnv {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        TermEnv::from_lookup(&|k| m.get(k).cloned())
    }

    /// A pty pair. `master` plays the terminal; the slave is konoma's tty, already raw as it is
    /// when konoma queries (crossterm's raw mode is entered first).
    struct Pty {
        master: File,
        slave: File,
    }

    fn pty() -> Pty {
        let (mut m, mut s) = (0, 0);
        // SAFETY: out-pointers are valid; null name/termios/winsize are allowed.
        let r = unsafe {
            libc::openpty(
                &mut m,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(r, 0, "openpty");
        // SAFETY: fds just returned by openpty and owned by nobody else.
        let (master, slave) = unsafe { (File::from_raw_fd(m), File::from_raw_fd(s)) };
        // SAFETY: plain termios round trip on a tty we own.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            libc::tcgetattr(slave.as_raw_fd(), &mut t);
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &t);
        }
        Pty { master, slave }
    }

    impl Pty {
        fn terminal_says(&mut self, s: &str) {
            self.master.write_all(s.as_bytes()).unwrap();
        }
        /// Everything currently readable on the slave (what the event loop would get).
        fn drain_slave(&self) -> Vec<u8> {
            let fd = self.slave.as_raw_fd();
            let mut got = Vec::new();
            while wait_readable(fd, Duration::ZERO).unwrap() {
                match read_byte(fd).unwrap() {
                    Some(b) => got.push(b),
                    None => break,
                }
            }
            got
        }
        fn query(&mut self, env: &TermEnv, timeout: Duration) -> io::Result<Reply> {
            let mut out = self.slave.try_clone().unwrap();
            query_terminal(self.slave.as_raw_fd(), &mut out, &env.query(), timeout)
        }
    }

    #[test]
    fn query_written_is_the_one_ratatui_image_sends() {
        for (pairs, tmux) in [
            (vec![], false),
            (vec![("TERM", "tmux-256color")], true),
            (vec![("WEZTERM_EXECUTABLE", "/x")], false),
        ] {
            let env = env_of(&pairs);
            assert_eq!(env.is_tmux, tmux);
            let mut p = pty();
            p.terminal_says(DSR);
            p.query(&env, SHORT).unwrap();
            let mut seen = vec![0u8; env.query().len()];
            p.master.read_exact(&mut seen).unwrap();
            assert_eq!(String::from_utf8(seen).unwrap(), env.query());
        }
        let konsole = env_of(&[("KONSOLE_VERSION", "1")]).query();
        assert!(!konsole.contains("_Gi=31") && !konsole.contains("[c"));
        let plain = env_of(&[]).query();
        assert!(plain.contains("_Gi=31") && plain.contains("[c") && plain.contains("[16t"));
        assert!(env_of(&[("TERM_PROGRAM", "tmux")])
            .query()
            .starts_with("\x1bPtmux;"));
    }

    #[test]
    fn kitty_sixel_and_cell_size_replies_are_decoded() {
        let env = env_of(&[]);
        let mut p = pty();
        p.terminal_says(&format!("{KITTY_OK}{DA1_SIXEL}{CELL_8X16}{DSR}"));
        let r = p.query(&env, SHORT).unwrap();
        assert!(r.stray.is_empty());
        assert_eq!(
            decide(&env, &r.answer, None),
            PickerSpec {
                font: Some((8, 16)),
                proto: ProtocolType::Kitty
            }
        );
        // Sixel only (no kitty reply).
        let mut p = pty();
        p.terminal_says(&format!("{DA1_SIXEL}{CELL_8X16}{DSR}"));
        let r = p.query(&env, SHORT).unwrap();
        assert_eq!(decide(&env, &r.answer, None).proto, ProtocolType::Sixel);
        // DA1 without sixel (attribute 4 absent) and no cell size: the halfblocks default.
        let mut p = pty();
        p.terminal_says(&format!("\x1b[?62;22c{DSR}"));
        let r = p.query(&env, SHORT).unwrap();
        assert_eq!(
            decide(&env, &r.answer, None),
            PickerSpec {
                font: None,
                proto: ProtocolType::Halfblocks
            }
        );
    }

    #[test]
    fn silent_terminal_leaves_no_reader_and_loses_no_key() {
        let mut p = pty();
        let r = p.query(&env_of(&[]), SHORT).unwrap();
        assert_eq!(r.answer, Answer::Silent);
        assert!(r.stray.is_empty());
        // The keys typed afterwards must all be readable on the slave, in order. A leftover
        // reader would have eaten the first one (the bug this module exists for).
        p.terminal_says("jk");
        assert_eq!(p.drain_slave(), b"jk");
    }

    #[test]
    fn truncated_reply_times_out_and_later_keys_survive() {
        let env = env_of(&[]);
        let mut p = pty();
        // A reply cut off before the Device Status Report.
        p.terminal_says(&format!("{KITTY_OK}\x1b[?64;4"));
        let r = p.query(&env, SHORT).unwrap();
        assert_eq!(r.answer, Answer::Silent);
        p.terminal_says("jj");
        assert_eq!(p.drain_slave(), b"jj");
        assert_eq!(
            decide(&env, &r.answer, Some((9, 18))),
            PickerSpec {
                font: Some((9, 18)),
                proto: ProtocolType::Halfblocks
            }
        );
    }

    #[test]
    fn keys_after_the_final_report_are_left_in_the_queue() {
        let mut p = pty();
        p.terminal_says(&format!("{KITTY_OK}{CELL_8X16}{DSR}jjx"));
        let r = p.query(&env_of(&[]), SHORT).unwrap();
        assert!(matches!(r.answer, Answer::Replied(_)));
        assert!(r.stray.is_empty());
        assert_eq!(p.drain_slave(), b"jjx");
    }

    #[test]
    fn keys_mixed_into_the_replies_are_handed_back() {
        let env = env_of(&[]);
        let mut p = pty();
        // A key before the first reply, one between replies, an escape-sequence key (Up arrow),
        // and one after the report.
        p.terminal_says(&format!("j{KITTY_OK}k\x1b[A{DA1_SIXEL}{CELL_8X16}{DSR}l"));
        let r = p.query(&env, SHORT).unwrap();
        assert_eq!(r.stray, b"jk");
        assert_eq!(decide(&env, &r.answer, None).proto, ProtocolType::Kitty);
        assert_eq!(p.drain_slave(), b"l");
        assert_eq!(replay_keys(&r.stray).len(), 2);
    }

    #[test]
    fn a_reply_written_one_byte_at_a_time_is_still_decoded() {
        let env = env_of(&[]);
        let mut p = pty();
        for b in format!("{KITTY_OK}{CELL_8X16}{DSR}").bytes() {
            p.master.write_all(&[b]).unwrap();
        }
        let r = p.query(&env, SHORT).unwrap();
        assert_eq!(decide(&env, &r.answer, None).proto, ProtocolType::Kitty);
    }

    #[test]
    fn termios_is_restored_and_a_canonical_tty_works() {
        let mut p = pty();
        let fd = p.slave.as_raw_fd();
        // Make the slave canonical with echo, as a cooked tty would be.
        // SAFETY: plain termios round trip on a tty we own.
        let before = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut t);
            t.c_lflag |= libc::ICANON | libc::ECHO;
            libc::tcsetattr(fd, libc::TCSANOW, &t);
            t.c_lflag
        };
        // Without ICANON cleared the reply (no newline) would never become readable.
        p.terminal_says(&format!("{KITTY_OK}{DSR}"));
        let r = p.query(&env_of(&[]), SHORT).unwrap();
        assert!(matches!(r.answer, Answer::Replied(_)));
        // SAFETY: as above.
        let after = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut t);
            t.c_lflag
        };
        // PENDIN is set by the kernel itself when ICANON is switched off with input pending.
        let kernel_owned = libc::PENDIN;
        assert_eq!(after & !kernel_owned, before & !kernel_owned);
    }

    #[test]
    fn a_non_tty_input_is_an_error_like_before() {
        let dir = crate::test_support::unique_tmp("tq-nontty");
        std::fs::create_dir_all(&dir).unwrap();
        let f = File::create(dir.join("f")).unwrap();
        assert!(query_terminal(f.as_raw_fd(), &mut io::sink(), "q", SHORT).is_err());
    }

    #[test]
    fn a_hung_up_terminal_is_silent_without_waiting_for_the_deadline() {
        let Pty { master, slave } = pty();
        drop(master);
        let started = Instant::now();
        let r = query_terminal(
            slave.as_raw_fd(),
            &mut io::sink(),
            &env_of(&[]).query(),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(r.answer, Answer::Silent);
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    // ---- the decision, branch by branch (mirrors Picker::from_query_stdio_with_options) ----

    #[test]
    fn decide_matrix() {
        let none = env_of(&[]);
        let iterm = env_of(&[("TERM_PROGRAM", "iTerm.app")]);
        let tmux_iterm = env_of(&[("TERM", "tmux-256color"), ("ITERM_SESSION_ID", "w0t0p0")]);
        let hb = PickerSpec {
            font: None,
            proto: ProtocolType::Halfblocks,
        };
        let spec = |w, h, proto| PickerSpec {
            font: Some((w, h)),
            proto,
        };
        let replied = Answer::Replied;
        let cell = Response::CellSize(Some((8, 16)));
        // Silent: window size + env hint, else halfblocks.
        assert_eq!(decide(&none, &Answer::Silent, None), hb);
        assert_eq!(
            decide(&none, &Answer::Silent, Some((7, 14))),
            spec(7, 14, ProtocolType::Halfblocks)
        );
        assert_eq!(
            decide(&iterm, &Answer::Silent, Some((7, 14))),
            spec(7, 14, ProtocolType::Iterm2)
        );
        // An env hint without a cell size is discarded.
        assert_eq!(decide(&iterm, &Answer::Silent, None), hb);
        assert_eq!(
            decide(&tmux_iterm, &Answer::Silent, Some((7, 14))).proto,
            ProtocolType::Iterm2
        );
        // Only the status report came back: same as silent.
        assert_eq!(
            decide(&iterm, &replied(vec![]), Some((7, 14))).proto,
            ProtocolType::Iterm2
        );
        // Replies: terminal beats environment; kitty beats sixel in either order.
        let a = replied(vec![Response::Sixel, Response::Kitty, cell.clone()]);
        assert_eq!(decide(&iterm, &a, None), spec(8, 16, ProtocolType::Kitty));
        let b = replied(vec![Response::Kitty, Response::Sixel, cell.clone()]);
        assert_eq!(decide(&none, &b, None).proto, ProtocolType::Kitty);
        // Answered the graphics probe but not the cell size: window size, else the default.
        let sixel_only = replied(vec![Response::Sixel]);
        assert_eq!(
            decide(&none, &sixel_only, Some((6, 12))),
            spec(6, 12, ProtocolType::Sixel)
        );
        assert_eq!(decide(&none, &sixel_only, None), hb);
        // A cell size reply wins over the window size.
        assert_eq!(
            decide(&none, &replied(vec![cell.clone()]), Some((1, 2))),
            spec(8, 16, ProtocolType::Halfblocks)
        );
        // An unusable cell size reply does not reset a good one.
        let c = replied(vec![cell.clone(), Response::CellSize(None)]);
        assert_eq!(decide(&none, &c, None).font, Some((8, 16)));
        // Env fallback when the terminal replies with a size only.
        assert_eq!(
            decide(&iterm, &replied(vec![cell]), None),
            spec(8, 16, ProtocolType::Iterm2)
        );
    }

    #[test]
    fn env_detection() {
        assert!(!env_of(&[]).is_tmux);
        assert!(env_of(&[("TERM", "tmux")]).is_tmux);
        assert!(env_of(&[("TERM", "tmux-256color")]).is_tmux);
        assert!(!env_of(&[("TERM", "xterm-tmux")]).is_tmux);
        assert!(env_of(&[("TERM_PROGRAM", "tmux")]).is_tmux);
        assert_eq!(env_of(&[("ITERM_SESSION_ID", "x")]).tmux_proto, None);
        assert_eq!(
            env_of(&[("TERM", "tmux"), ("WEZTERM_EXECUTABLE", "x")]).tmux_proto,
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(
            env_of(&[("TERM", "tmux"), ("ITERM_SESSION_ID", "")]).tmux_proto,
            None
        );
        for p in [
            "iTerm.app",
            "WezTerm",
            "mintty",
            "vscode",
            "Tabby",
            "Hyper",
            "rio",
            "Bobcat",
            "WarpTerminal",
        ] {
            assert_eq!(
                env_of(&[("TERM_PROGRAM", p)]).iterm2_proto,
                Some(ProtocolType::Iterm2),
                "{p}"
            );
        }
        assert_eq!(
            env_of(&[("LC_TERMINAL", "iTerm2")]).iterm2_proto,
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(
            env_of(&[("TERM_PROGRAM", "Apple_Terminal")]).iterm2_proto,
            None
        );
        assert!(env_of(&[("WEZTERM_EXECUTABLE", "x")]).skip_kitty_and_sixel);
        assert!(env_of(&[("KONSOLE_VERSION", "230800")]).skip_kitty_and_sixel);
        assert!(!env_of(&[("KONSOLE_VERSION", "")]).skip_kitty_and_sixel);
    }

    #[test]
    fn stray_filter_keeps_only_bytes_outside_sequences() {
        let mut f = StrayFilter::default();
        for b in "a\x1b[?64;4cb\x1b_Gi=31;OK\x1b\\c\x1b]11;rgb:0/0/0\x07d\x1bxe\x1b[A".bytes() {
            f.push(b);
        }
        assert_eq!(f.stray, b"abcde");
    }

    #[test]
    fn replay_maps_text_and_controls() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let k = replay_keys("jあ\r\t\u{7f}\u{3}".as_bytes());
        let codes: Vec<_> = k.iter().map(|e| (e.code, e.modifiers)).collect();
        assert_eq!(
            codes,
            vec![
                (KeyCode::Char('j'), KeyModifiers::NONE),
                (KeyCode::Char('あ'), KeyModifiers::NONE),
                (KeyCode::Enter, KeyModifiers::NONE),
                (KeyCode::Tab, KeyModifiers::NONE),
                (KeyCode::Backspace, KeyModifiers::NONE),
                (KeyCode::Char('c'), KeyModifiers::CONTROL),
            ]
        );
    }

    #[test]
    fn ioctl_cell_size_reads_the_window_size() {
        let p = pty();
        let set = |xpx, ypx| {
            let ws = libc::winsize {
                ws_row: 10,
                ws_col: 40,
                ws_xpixel: xpx,
                ws_ypixel: ypx,
            };
            // SAFETY: valid winsize for TIOCSWINSZ on our own pty.
            unsafe { libc::ioctl(p.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        };
        set(400, 200);
        assert_eq!(ioctl_cell_size(p.slave.as_raw_fd()), Some((10, 20)));
        set(0, 0);
        assert_eq!(ioctl_cell_size(p.slave.as_raw_fd()), None);
    }
}
