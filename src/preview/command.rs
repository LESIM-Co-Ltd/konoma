// External command delegation renderer.
//
// Policy (`PreviewKind::Command { path, template, render_as, detached }`):
//   - `detached = true`: spawn the child process and don't wait (a video player, etc.) — never
//     blocks the TUI. `run_detached` (this module) is called synchronously from
//     `App::start_media_load`, since `Command::spawn` returning is fast (it doesn't wait for the
//     child), unlike `run_capture` below.
//   - `render_as = Some("image")`: run on the media worker thread (`App`'s existing
//     SVG/GIF/video/PDF pipeline — see `MediaJob::Command`), then decode the produced artifact and
//     display it full-screen like any other image.
//   - anything else (`None`, `"text"`, or an unrecognized string): run on the media worker thread,
//     write the captured output to a temp file, and show it through the existing windowed
//     (less-style) text reader (`App::windowed_src` points the reader at that temp file while the
//     preview title still shows the original `path`).
// A missing/failing command never crashes konoma — it degrades to the same `[can not preview: <ext>]`
// display as an unmatched rule, with the failure reason appended (design principle #3).
//
// This module only builds argv and runs the child process; it does not know about `App`/`MediaJob`
// (kept a pure/testable leaf, like `preview::video`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

/// Splits `template` on whitespace into argv, substituting `{path}`/`{out}` **within each
/// whitespace-delimited token** (so a path containing spaces — substituted after the split — stays
/// one argv element, never re-split; no shell is invoked, so `;`/`|`/quoting in the path can't be
/// interpreted as syntax). If no token contains `{path}`, the path is appended at the end (mirrors
/// `config::EditorConfig`'s `build_argv`, which does the same for `{path}`/`{line}`). `{out}` has no
/// such fallback — an output-producing command with no `{out}` token in its template legitimately
/// means "capture stdout instead" (`run_capture`'s `uses_out=false` path).
///
/// An empty (`command = ""`) or whitespace-only template is **not** "a template with no `{path}`
/// token" — it is no command at all, and must not fall into that same append-the-path fallback: doing
/// so used to make `argv` exactly `[path]`, i.e. the previewed file itself became `argv[0]`, the
/// program `run_detached`/`run_capture` launch (bug, 2026-08 — a config typo, or an empty string
/// meant to disable a rule, executed the file being previewed instead of doing nothing). Returning an
/// empty `Vec` here instead routes to those functions' own existing `argv.split_first().context(
/// "empty command template")` `Err` — this module's own graceful-degradation path (see the module doc
/// comment) — without ever spawning a process.
pub fn build_argv(template: &str, path: &Path, out: &Path) -> Vec<String> {
    let mut argv: Vec<String> = template.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        return argv;
    }
    let p = path.to_string_lossy().to_string();
    let o = out.to_string_lossy().to_string();
    let mut path_sub = false;
    for a in argv.iter_mut() {
        if a.contains("{path}") {
            *a = a.replace("{path}", &p);
            path_sub = true;
        }
        if a.contains("{out}") {
            *a = a.replace("{out}", &o);
        }
    }
    if !path_sub {
        argv.push(p);
    }
    argv
}

// Test-only call counter for `temp_out_path`, thread-local rather than sharing the real (process-
// global) atomic counter below: `cargo test` runs tests concurrently on separate threads, and a
// process-wide counter would let *other* tests' calls (any test that runs a delegated command)
// pollute a before/after delta measured by one specific test — the same class of flake previously
// hit with a process-shared git-call counter (see `App::attempt_deferred_dispatch` tests / memory
// [[async-git-status-scan]]). Thread-local is safe even though the test harness's thread pool can
// reuse a thread across tests: a before/after delta measured *within* one test's own execution
// still isolates that test's own increments regardless of what an earlier test left on that thread.
#[cfg(test)]
thread_local! {
    static CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Number of `temp_out_path` calls on the current thread so far (test-only introspection).
#[cfg(test)]
pub(crate) fn calls_for_test() -> u64 {
    CALLS.with(|c| c.get())
}

/// Lazily creates (once per process), and returns the path to, a private `0700` (owner-only)
/// subdirectory under the system temp dir for delegated commands' `{out}` files.
/// `std::env::temp_dir()` is world-writable/world-readable on Linux (`/tmp` is mode `1777`) — a
/// delegated command's output would otherwise briefly sit at a pid-and-counter-predictable path
/// readable by any other local user on a shared box, regardless of whether *konoma itself* wrote
/// the file (the `uses_out=false` capture path, below — see also `write_private`) or an
/// arbitrary user-configured external command wrote `{out}` itself under whatever mode it happens
/// to pick (the `uses_out=true` path). Restricting the *directory* is what closes this uniformly
/// for both cases: POSIX requires execute/search permission on every ancestor directory to open a
/// file by path, so `0700` here blocks other users regardless of who created the file or what mode
/// they used for it.
fn private_temp_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("konoma-cmd-{}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            // The mode is applied atomically by `mkdir(2)` itself (masked by umask, but `0o700`
            // has no group/other bits for umask to strip), so there's no "create, then chmod" gap
            // where a wider-permission window briefly exists.
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
            // Defense-in-depth for the unlikely case the directory already existed with looser
            // permissions (e.g. a stale leftover from an earlier process that reused this pid).
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::create_dir(&dir);
        }
        dir
    })
    .clone()
}

/// A fresh temp path for a delegated command's `{out}`, unique within this process (pid + an
/// atomic counter — no dependence on randomness/time, matching `preview::video::temp_png_path`'s
/// convention). Deliberately has **no extension**: a template that needs one appends it itself in
/// the same token (`-o {out}.png`, the documented `merman` example in config.example.toml) —
/// `run_capture`'s `uses_out=true` branch accepts either the literal `out` path or a sibling named
/// `<out's file name>.<anything>` (see `find_suffixed_out`), so a template is free to write to the
/// bare path or append its own extension.
pub fn temp_out_path() -> PathBuf {
    #[cfg(test)]
    CALLS.with(|c| c.set(c.get() + 1));
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    private_temp_dir().join(format!("out-{n}"))
}

/// Writes `data` to `path`, creating it with owner-only (`0600`) permissions **at creation time**
/// (not via a separate `set_permissions` afterward, which would leave — however briefly — a window
/// where the file exists at a wider mode). This is the one case in this module where konoma itself
/// creates the output file (`run_capture`'s `uses_out=false` branch, capturing a delegated
/// command's stdout) — unlike the `uses_out=true` case where an external command writes `{out}`
/// itself and we don't control its `open()` call at all, here we do, so this is defense-in-depth on
/// top of `private_temp_dir`'s already-`0700` parent directory.
#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}

/// Spawns `argv` and does not wait for it — `detached = true` (a video player, etc.). stdio is
/// fully detached (`null`) so the child doesn't inherit konoma's raw-mode terminal. Err only when
/// the process could not even be launched (missing binary, permission denied, ...).
pub fn run_detached(argv: &[String]) -> Result<()> {
    let (prog, rest) = argv.split_first().context("empty command template")?;
    Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch `{prog}`"))?;
    Ok(())
}

/// Runs `argv` to completion and returns the path to display:
/// - `uses_out = true` (the template contains `{out}`): the command is expected to have written
///   `out` itself, either at the literal path or — a common idiom for tools that infer their output
///   format from the extension (`ImageMagick`'s `convert in.psd {out}.png`, the documented `merman`
///   example) — at `<out>.<ext>` (see `find_suffixed_out`; the literal path wins if both exist). Err
///   if neither is found, or the one found is empty (a command that silently produced nothing).
/// - `uses_out = false`: `out` doesn't come from the command — captured stdout (capped, see below)
///   is written to `out` and that path is returned instead.
///
/// A non-zero exit is an Err whose message is the **first non-empty line of stderr** (falling back
/// to the exit status when stderr is empty) — not the command line that was run, so the actual
/// reason isn't pushed off the edge of a one-line flash (same convention as `git::git_error_message`
/// for git's own CLI errors).
///
/// stdout and stderr are each capped at `text::MAX_BYTES` while being read (shared with the
/// windowed text reader, so a delegated command's output and what actually ends up on screen never
/// disagree on how much is usable) — this is what stops a runaway/infinite-output command (`yes`,
/// a `command="cat {path}"` misconfiguration reading a device file, ...) from growing this buffer
/// without bound: once the cap is hit the child is killed rather than waiting for a natural exit
/// that may never come. Both streams are drained on background threads (this thread instead polls
/// `try_wait`, below) so a child that interleaves writes to both can't deadlock this by filling one
/// pipe's small OS buffer while only the other is drained.
///
/// Separately, the whole run is bounded by `RUN_TIMEOUT`: a command that produces *no* output at
/// all (so the cap above never triggers) and never exits on its own used to block this function —
/// and the media worker thread that calls it — forever. Past the deadline the child is killed and
/// this returns an `Err` describing the timeout, exactly like any other failure (principle #3: this
/// degrades to `[can not preview]`, it never hangs the app).
pub fn run_capture(argv: &[String], out: &Path, uses_out: bool) -> Result<PathBuf> {
    run_capture_impl(argv, out, uses_out, RUN_TIMEOUT)
}

/// How long a delegated command is allowed to run before being killed and treated as a failure.
/// Generous on purpose — some legitimate conversions genuinely take several seconds — this exists
/// only to turn "never returns" into "eventually gives up," not to police normal runtimes.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the poll loop in `run_capture_impl` re-checks the child and the reader threads'
/// progress. Small enough that both a timeout and an output-cap kill are noticed promptly, without
/// busy-polling.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `run_capture`'s real body, parameterized on the timeout purely for testability: `run_capture`
/// itself always passes the real, generous `RUN_TIMEOUT`; tests pass something short so a
/// deliberately-hanging fixture command doesn't make the test suite itself slow (see
/// `run_capture_returns_promptly_for_a_hanging_command` below).
fn run_capture_impl(
    argv: &[String],
    out: &Path,
    uses_out: bool,
    timeout: Duration,
) -> Result<PathBuf> {
    let (prog, rest) = argv.split_first().context("empty command template")?;
    let mut child = Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch `{prog}`"))?;

    let mut stdout = child.stdout.take().expect("stdout was piped above");
    let mut stderr = child.stderr.take().expect("stderr was piped above");
    let cap = crate::preview::text::MAX_BYTES;

    // Draining stdout was previously synchronous on this thread; it now runs on a background
    // thread (matching stderr) so this thread is free to poll `try_wait` below with a deadline —
    // a blocking synchronous read can't be interrupted by a timeout on the same thread. The shared
    // flag lets this thread notice a cap-triggered truncation and kill the child immediately
    // (rather than only discovering it once the deadline arrives), preserving the prior fast-kill
    // behavior for a runaway-output command like `yes`.
    let stdout_capped = Arc::new(AtomicBool::new(false));
    let stdout_capped_writer = Arc::clone(&stdout_capped);
    let stdout_handle = std::thread::spawn(move || {
        let (buf, truncated) = capped_read(&mut stdout, cap);
        if truncated {
            stdout_capped_writer.store(true, Ordering::Release);
        }
        (buf, truncated)
    });
    let stderr_handle = std::thread::spawn(move || capped_read(&mut stderr, cap));

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if stdout_capped.load(Ordering::Acquire) {
                    // A runaway/infinite-output command: stop waiting for a natural exit and reap
                    // it instead (same trigger the old synchronous-read version killed on).
                    let _ = child.kill();
                    break child
                        .wait()
                        .with_context(|| format!("failed to wait for `{prog}`"))?;
                }
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break child
                        .wait()
                        .with_context(|| format!("failed to wait for `{prog}`"))?;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(e).context(format!("failed to wait for `{prog}`")),
        }
    };

    let (stdout_buf, stdout_truncated) =
        stdout_handle.join().unwrap_or_else(|_| (Vec::new(), false));
    let (stderr_buf, _) = stderr_handle.join().unwrap_or_else(|_| (Vec::new(), false));

    if timed_out {
        bail!("timed out after {}s without exiting", timeout.as_secs());
    }

    // A capped run's exit status is meaningless (the child was killed mid-flight rather than
    // allowed to finish naturally) — only judge success/failure when it ran to a real EOF.
    if !stdout_truncated && !status.success() {
        let msg = first_nonempty_line(&stderr_buf).unwrap_or_else(|| status.to_string());
        bail!(msg);
    }

    if uses_out {
        resolve_produced_out(out).with_context(|| format!("{} was not produced", out.display()))
    } else {
        write_private(out, &stdout_buf)
            .with_context(|| format!("failed to write {}", out.display()))?;
        Ok(out.to_path_buf())
    }
}

/// What a `uses_out=true` template actually wrote, preferring the literal `out` path over a
/// suffixed sibling (see `find_suffixed_out`) when both exist — the literal path is the documented
/// contract, so a template that honors it exactly should never be shadowed by a stale leftover
/// sharing its prefix. None if neither is present (or the one found is empty).
fn resolve_produced_out(out: &Path) -> Option<PathBuf> {
    if is_nonempty_file(out) {
        return Some(out.to_path_buf());
    }
    find_suffixed_out(out)
}

fn is_nonempty_file(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
}

/// Looks in `out`'s directory for a file named `<out's file name>.<anything>` — the idiom a
/// template uses when it appends its own extension to `{out}` (`-o {out}.png`) rather than writing
/// the bare path. The `.` separator is required, not just a `starts_with` prefix match: `temp_out_path`
/// names are `konoma-cmd-<pid>-<n>`, so without the separator `…-2`'s scan would also match `…-20.png`
/// — a *different* call's leftover, mistaken for this one's own suffixed output. When more than one
/// candidate matches (a template that could write either of several extensions), the most recently
/// modified one wins. Only non-empty candidates are considered, mirroring the literal-`out` check.
fn find_suffixed_out(out: &Path) -> Option<PathBuf> {
    let dir = out.parent()?;
    let base = out.file_name()?.to_str()?;
    let prefix = format!("{base}.");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() == 0 {
            continue;
        }
        let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let is_newer = match &best {
            Some((t, _)) => mtime > *t,
            None => true,
        };
        if is_newer {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Reads from `r` until EOF or `cap` bytes, whichever comes first — the rest is discarded, not
/// buffered (that's the whole point: bounding memory against a command that never stops writing).
/// Returns `(bytes read, whether the cap was hit before EOF)`.
fn capped_read(r: &mut impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if buf.len() >= cap {
            return (buf, true);
        }
        match r.read(&mut chunk) {
            Ok(0) => return (buf, false), // EOF
            Ok(n) => {
                let room = cap - buf.len();
                let take = n.min(room);
                buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    return (buf, true); // filled the cap mid-chunk
                }
            }
            Err(_) => return (buf, false), // a read error ends the capture, not a crash
        }
    }
}

/// The first non-empty, trimmed line of `bytes` (lossily decoded), or None if there isn't one.
fn first_nonempty_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    // ---- build_argv ---------------------------------------------------------------------------

    #[test]
    fn build_argv_substitutes_path_only() {
        assert_eq!(
            build_argv("cat {path}", Path::new("/a/b.txt"), Path::new("/tmp/o")),
            vec!["cat", "/a/b.txt"]
        );
    }

    #[test]
    fn build_argv_substitutes_out_only_and_still_appends_path() {
        // No {path} token in the template → the path is appended at the end (same convention as
        // `EditorConfig::build_argv` for an editor template with no {path}).
        assert_eq!(
            build_argv("tool -o {out}", Path::new("/a/b.txt"), Path::new("/tmp/o")),
            vec!["tool", "-o", "/tmp/o", "/a/b.txt"]
        );
    }

    #[test]
    fn build_argv_substitutes_both() {
        assert_eq!(
            build_argv(
                "conv {path} {out}",
                Path::new("/a/b.txt"),
                Path::new("/tmp/o")
            ),
            vec!["conv", "/a/b.txt", "/tmp/o"]
        );
    }

    #[test]
    fn build_argv_neither_token_appends_path_at_end() {
        assert_eq!(
            build_argv("cat", Path::new("/a/b.txt"), Path::new("/tmp/o")),
            vec!["cat", "/a/b.txt"]
        );
    }

    #[test]
    fn build_argv_keeps_a_space_containing_path_as_one_argv_element() {
        // The template is split BEFORE substitution, so a path containing whitespace never gets
        // re-split — this is what makes shell-less invocation safe for such paths.
        let argv = build_argv(
            "cat {path}",
            Path::new("/a/my file.txt"),
            Path::new("/tmp/o"),
        );
        assert_eq!(argv, vec!["cat", "/a/my file.txt"]);
        assert_eq!(argv.len(), 2, "空白入りパスが2要素に割れていないこと");
    }

    #[test]
    fn build_argv_handles_cjk_path() {
        assert_eq!(
            build_argv(
                "cat {path}",
                Path::new("/a/日本語.txt"),
                Path::new("/tmp/o")
            ),
            vec!["cat", "/a/日本語.txt"]
        );
    }

    #[test]
    fn build_argv_substitutes_repeated_token_in_separate_args() {
        assert_eq!(
            build_argv(
                "diff {path} {path}",
                Path::new("/a/b.txt"),
                Path::new("/tmp/o")
            ),
            vec!["diff", "/a/b.txt", "/a/b.txt"]
        );
    }

    #[test]
    fn build_argv_substitutes_repeated_token_within_one_arg() {
        // Both occurrences inside a single whitespace-delimited token are replaced too (str::replace
        // replaces every match, not just the first).
        assert_eq!(
            build_argv("cat {path}{path}", Path::new("/a"), Path::new("/tmp/o")),
            vec!["cat", "/a/a"]
        );
    }

    /// Root cause: an empty (`command = ""`) or whitespace-only (`"   "`) template used to fall
    /// through to the same "no `{path}` token was substituted" fallback as a real template like
    /// `"tool"` — `argv` starts empty, `path_sub` stays `false`, and the trailing `if !path_sub {
    /// argv.push(p); }` then made the previewed file's own path the **entire** argv, i.e. `argv[0]`
    /// — the program `run_detached`/`run_capture` launch. A config typo (or an empty string meant to
    /// "turn a rule off") would launch the previewed file as a process instead of doing nothing.
    /// `EditorConfig::build_argv` (`config::mod`) has the same `{path}`-fallback shape but is never
    /// called with an empty template (the editor always resolves to at least `vim`), so this class of
    /// bug is specific to a user-authored `command` string, which can legitimately be empty.
    ///
    /// An empty template has to mean "no command to delegate to", not "run the path" — see the
    /// end-to-end test below for the same claim proven by actually running the result.
    #[test]
    fn build_argv_empty_or_whitespace_template_produces_no_argv() {
        assert_eq!(
            build_argv("", Path::new("/a/b.sh"), Path::new("/tmp/o")),
            Vec::<String>::new(),
            "空テンプレートが argv=[プレビュー対象のパス] になっている(実行されてしまう)"
        );
        assert_eq!(
            build_argv("   ", Path::new("/a/b.sh"), Path::new("/tmp/o")),
            Vec::<String>::new(),
            "空白のみのテンプレートも同様に実行対象になってしまっている"
        );
        // A template with only unrelated tokens (no {path}/{out}) is unaffected — it should still
        // append the path (the ordinary, documented fallback), which is what makes the bug's fallout
        // dangerous: this codepath is *supposed* to append the path when the template is a real
        // command that just forgot `{path}`.
        assert_eq!(
            build_argv("cat", Path::new("/a/b.sh"), Path::new("/tmp/o")),
            vec!["cat", "/a/b.sh"]
        );
    }

    // ---- run_capture (unix-only: relies on `cat`/`true`/`false`/`yes`, no `sh -c`) ------------

    #[cfg(unix)]
    fn tmp(name: &str) -> PathBuf {
        unique_tmp(&format!("konoma_command_test_{name}"))
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_uses_out_when_the_command_writes_it() {
        let src = tmp("uses_out_src.txt");
        std::fs::write(&src, "hello from source\n").unwrap();
        let out = tmp("uses_out_dst.txt");
        let _ = std::fs::remove_file(&out);

        let argv = build_argv("cp {path} {out}", &src, &out);
        let result = run_capture(&argv, &out, true).expect("cp should succeed");
        assert_eq!(result, out);
        let content = std::fs::read_to_string(&out).unwrap();
        assert_eq!(content, "hello from source\n");

        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&out).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_writes_captured_stdout_when_out_is_unused() {
        let src = tmp("stdout_src.txt");
        std::fs::write(&src, "irrelevant").unwrap();
        let out = tmp("stdout_dst.txt");
        let _ = std::fs::remove_file(&out);

        // No {out} token → uses_out=false → stdout gets captured into `out`.
        let argv = build_argv("echo {path}", &src, &out);
        let result = run_capture(&argv, &out, false).expect("echo should succeed");
        assert_eq!(result, out);
        let content = std::fs::read_to_string(&out).unwrap();
        assert_eq!(content.trim_end(), src.to_string_lossy());

        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&out).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_reports_missing_command() {
        let out = tmp("missing_cmd_dst.txt");
        let argv = vec!["konoma-definitely-nonexistent-command-xyz123".to_string()];
        let err = run_capture(&argv, &out, false).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_reports_stderr_first_line_on_nonzero_exit() {
        let out = tmp("nonzero_dst.txt");
        // `cat` on a nonexistent file: nonzero exit, a one-line message to stderr containing the path.
        let argv = vec!["cat".to_string(), "/no/such/path/konoma-test".to_string()];
        let err = run_capture(&argv, &out, false).unwrap_err();
        assert!(
            err.to_string().contains("konoma-test") || err.to_string().contains("No such"),
            "stderr の1行目が理由として出ていない: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_reports_out_not_produced() {
        let out = tmp("not_produced_dst.txt");
        let _ = std::fs::remove_file(&out);
        // `true` exits 0 without touching `out` at all.
        let argv = vec!["true".to_string()];
        let err = run_capture(&argv, &out, true).unwrap_err();
        assert!(err.to_string().contains(&out.to_string_lossy().to_string()));
        assert!(!out.exists());
    }

    /// End-to-end proof of the empty-template fix, run exactly the way the app runs a delegated
    /// command (`build_argv` then `run_capture`) rather than just inspecting `build_argv`'s return
    /// value: the "previewed file" here is itself an executable script that leaves a marker file if
    /// it is ever actually launched. With the pre-fix `build_argv`, `argv` was `[script_path]` and
    /// `run_capture` would spawn and run it (Ok result, marker created) — previewing a file with
    /// `command = ""` executed that file. After the fix, `argv` is empty and `run_capture` fails at
    /// its very first line (`argv.split_first().context("empty command template")`) before any
    /// process is ever spawned.
    #[cfg(unix)]
    #[test]
    fn empty_template_end_to_end_never_executes_the_previewed_file() {
        use std::os::unix::fs::PermissionsExt;

        let marker = tmp("empty_template_marker.txt");
        let _ = std::fs::remove_file(&marker);
        let script = tmp("empty_template_script.sh");
        std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let out = tmp("empty_template_out.txt");
        let _ = std::fs::remove_file(&out);

        // `command = ""` — an empty template, exactly as a user could write in config.example.toml.
        let argv = build_argv("", &script, &out);
        let result = run_capture(&argv, &out, false);

        assert!(
            result.is_err(),
            "空テンプレートは「委譲するコマンドが無い」エラーになるべき: {result:?}"
        );
        assert!(
            !marker.exists(),
            "空テンプレートでプレビュー対象自身(スクリプト)が実際に実行されてしまった"
        );

        std::fs::remove_file(&script).ok();
        std::fs::remove_file(&marker).ok();
        std::fs::remove_file(&out).ok();
    }

    // ---- run_capture: `uses_out=true` also accepts a suffixed sibling (`<out>.<ext>`) -----------
    // This is the idiom documented for tools that infer their output format from the extension
    // (`merman -i {path} -o {out}.png`, config.example.toml) — the template appends its own
    // extension rather than writing the bare `out` path.

    #[cfg(unix)]
    #[test]
    fn run_capture_finds_a_suffixed_out_when_the_literal_path_is_unused() {
        let src = tmp("suffix_src.txt");
        std::fs::write(&src, "suffixed output\n").unwrap();
        let out = tmp("suffix_dst");
        let suffixed = out.with_extension("png");
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&suffixed);

        // The template appends its own extension to {out}, exactly like the documented example.
        let argv = build_argv("cp {path} {out}.png", &src, &out);
        let result = run_capture(&argv, &out, true).expect("cp should succeed");
        assert_eq!(result, suffixed, "接尾辞つき成果物のパスを返していない");
        let content = std::fs::read_to_string(&result).unwrap();
        assert_eq!(content, "suffixed output\n");

        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&suffixed).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_prefers_the_literal_out_over_a_suffixed_sibling() {
        let src = tmp("suffix_priority_src.txt");
        std::fs::write(&src, "literal wins\n").unwrap();
        let out = tmp("suffix_priority_dst");
        let suffixed = out.with_extension("png");
        let _ = std::fs::remove_file(&out);
        // A pre-existing suffixed sibling (e.g. a leftover) must not shadow a command that honors
        // the documented contract and writes the literal `out` path.
        std::fs::write(&suffixed, "stale leftover, must be ignored\n").unwrap();

        let argv = build_argv("cp {path} {out}", &src, &out);
        let result = run_capture(&argv, &out, true).expect("cp should succeed");
        assert_eq!(result, out, "リテラル out より接尾辞つきを優先してしまった");
        let content = std::fs::read_to_string(&result).unwrap();
        assert_eq!(content, "literal wins\n");

        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&out).ok();
        std::fs::remove_file(&suffixed).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_out_suffix_match_requires_a_dot_separator() {
        // A basename ending in a digit (mirroring `temp_out_path`'s `konoma-cmd-<pid>-<n>` — e.g.
        // "...-2") must not be fooled by a sibling from a *different* call whose name happens to
        // start with the same characters without the "." separator ("...-20.png" starts_with
        // "...-2" but is NOT "...-2" + "." + anything).
        let out = {
            let mut s = unique_tmp("konoma_command_test_suffix_dot").into_os_string();
            s.push("-2");
            PathBuf::from(s)
        };
        let decoy = {
            let mut s = out.clone().into_os_string();
            s.push("0.png");
            PathBuf::from(s)
        };
        let _ = std::fs::remove_file(&out);
        std::fs::write(
            &decoy,
            b"decoy belonging to a different call, must never match",
        )
        .unwrap();

        // `true` exits 0 without writing anything — neither the literal `out` nor a correctly
        // suffixed (`out` + "." + ext) sibling exists.
        let argv = vec!["true".to_string()];
        let err = run_capture(&argv, &out, true).unwrap_err();
        assert!(
            err.to_string().contains(&out.to_string_lossy().to_string()),
            "'.' 区切りを要求せず decoy を成果物として誤採用した: {err}"
        );
        assert!(!out.exists());

        std::fs::remove_file(&decoy).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_ignores_an_empty_suffixed_candidate() {
        let out = tmp("suffix_empty_dst");
        let suffixed = out.with_extension("png");
        let _ = std::fs::remove_file(&out);
        std::fs::write(&suffixed, b"").unwrap(); // 0 bytes: "silently produced nothing"

        let argv = vec!["true".to_string()];
        let err = run_capture(&argv, &out, true).unwrap_err();
        assert!(err.to_string().contains(&out.to_string_lossy().to_string()));
        assert!(!out.exists());

        std::fs::remove_file(&suffixed).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_picks_a_suffixed_candidate_when_more_than_one_matches() {
        // Two plausible extensions both get written (a template that could produce either) — the
        // most recently modified one should win. Filesystem mtime resolution/monotonicity isn't
        // guaranteed fine-grained across every platform/CI runner, so this test only asserts a
        // *deterministic, non-empty* candidate is returned (no panic/ambiguity/empty-file pick),
        // not specifically which of the two — the "most recent wins" tie-break itself is exercised
        // manually (verified locally: writing the second file ~20ms after the first reliably makes
        // it win on APFS).
        let out = tmp("suffix_multi_dst");
        let png = out.with_extension("png");
        let jpg = out.with_extension("jpg");
        let _ = std::fs::remove_file(&out);
        std::fs::write(&png, b"png candidate").unwrap();
        std::fs::write(&jpg, b"jpg candidate").unwrap();

        let argv = vec!["true".to_string()];
        let result = run_capture(&argv, &out, true).expect("a suffixed candidate should be found");
        assert!(
            result == png || result == jpg,
            "どちらの接尾辞候補でもないパスを返した: {}",
            result.display()
        );
        assert!(
            std::fs::metadata(&result)
                .map(|m| m.len() > 0)
                .unwrap_or(false),
            "空の候補を採用した"
        );

        std::fs::remove_file(&png).ok();
        std::fs::remove_file(&jpg).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_capture_truncates_runaway_output() {
        let out = tmp("runaway_dst.txt");
        let _ = std::fs::remove_file(&out);
        // `yes` never terminates on its own — this proves we stop reading (and kill it) rather
        // than hang or grow the buffer without bound.
        let argv = vec!["yes".to_string()];
        let result = run_capture(&argv, &out, false).expect("capped read still succeeds");
        assert_eq!(result, out);
        let len = std::fs::metadata(&out).unwrap().len() as usize;
        assert!(len > 0, "何も書き出せていない");
        assert!(
            len <= crate::preview::text::MAX_BYTES,
            "上限を超えて書き出された: {len}"
        );
        std::fs::remove_file(&out).ok();
    }

    /// Root cause: `temp_out_path` built its path directly under `std::env::temp_dir()`, which is
    /// world-writable/world-readable on Linux (`/tmp` is mode `1777`) — a delegated command's
    /// output briefly sits at a pid-and-counter-predictable path, readable by any other local user
    /// on a shared box. Restricting the **directory** is what closes this uniformly regardless of
    /// who wrote the file: POSIX requires execute/search permission on every ancestor directory to
    /// open a file by path, so a `0700` directory blocks other users even for the `uses_out=true`
    /// case where an arbitrary user-configured external command (not konoma) writes `{out}` itself
    /// under whatever mode it picks.
    #[cfg(unix)]
    #[test]
    fn temp_out_path_lives_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_out_path();
        let dir = path.parent().expect("temp_out_path has a parent dir");
        // The discriminating check: this must be *our own* dedicated subdirectory, not the shared
        // system temp dir directly. Checking only the resulting mode below isn't enough to prove
        // our own code sets it — on macOS, `std::env::temp_dir()` itself already happens to be
        // `0700` (a per-user `/var/folders/.../T/`), which would make a mode-only assertion pass
        // by coincidence even without this fix. Linux's `/tmp` (mode `1777`, world-writable) has no
        // such luck, which is exactly the platform this bug targets.
        assert_ne!(
            dir,
            std::env::temp_dir(),
            "システム共有の一時ディレクトリ直下に出力している(専用サブディレクトリを持っていない)"
        );
        let meta = std::fs::metadata(dir).expect("private temp dir should exist by now");
        assert!(meta.is_dir());
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "委譲コマンドの一時出力先が owner-only(0700) になっていない: {dir:?} mode={mode:#o}"
        );
    }

    /// The `uses_out=false` branch is the one case in this module where **konoma itself** creates
    /// the output file (`std::fs::write`, capturing the child's stdout) — unlike the `uses_out=true`
    /// case above where an external command writes `{out}`, here we fully control the `open()` call
    /// and so can (and should, defense-in-depth on top of the private directory) give the file
    /// itself owner-only (`0600`) permissions at creation time, not via a separate `chmod` after
    /// (which would leave a — however brief — window where the file exists at a wider mode).
    #[cfg(unix)]
    #[test]
    fn captured_stdout_output_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let out = tmp("stdout_perm_dst.txt");
        let _ = std::fs::remove_file(&out);
        let argv = vec!["echo".to_string(), "hi".to_string()];
        run_capture(&argv, &out, false).expect("echo should succeed");

        let mode = std::fs::metadata(&out).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "captured stdout の書き出し先が owner-only(0600) になっていない: mode={mode:#o}"
        );
        std::fs::remove_file(&out).ok();
    }

    /// Root cause: `run_capture` used a synchronous stdout read followed by a plain, unbounded
    /// `Child::wait()` — a command that produces *no* output at all (so the existing output cap
    /// never triggers) and never exits on its own blocked this function, and the media worker
    /// thread that calls it, forever: `App::media_loading` never cleared, and the busy-indicator
    /// spinner spun forever too.
    ///
    /// Confirmed for real before this fix existed (pasted into the PR/commit description, not kept
    /// as a permanent test): calling the pre-fix `run_capture` with `argv = ["tail", "-f",
    /// "/dev/null"]` on a background thread, then waiting on `mpsc::Receiver::recv_timeout(3s)` for
    /// a result, timed out — it was still blocked after 3 real seconds.
    ///
    /// This permanent test instead proves it deterministically and fast, via `run_capture_impl`
    /// directly with a short injected timeout (production `run_capture` always uses the real,
    /// generous `RUN_TIMEOUT`). The fixture (`tail -f /dev/null`) genuinely never exits on its
    /// own — unlike e.g. `sleep N`, which eventually completes regardless of whether the timeout
    /// mechanism works, and so wouldn't actually prove anything. This project avoids asserting
    /// *exact* wall-clock durations (a prior CI break — see `docs/STATUS.md`), so the bound below
    /// is a large, deliberately loose multiple of the injected timeout: it exists only so a real
    /// regression (no timeout at all) fails this test instead of hanging the whole test run.
    #[cfg(unix)]
    #[test]
    fn run_capture_returns_promptly_for_a_hanging_command() {
        let out = tmp("hang_dst.txt");
        let _ = std::fs::remove_file(&out);
        let argv = vec![
            "tail".to_string(),
            "-f".to_string(),
            "/dev/null".to_string(),
        ];

        let started = std::time::Instant::now();
        let result = run_capture_impl(&argv, &out, false, Duration::from_millis(200));
        let elapsed = started.elapsed();

        assert!(
            result.is_err(),
            "ハングするコマンドは失敗として返るべき: {result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "タイムアウトが機能せずブロックし続けた: elapsed={elapsed:?}"
        );
        let _ = std::fs::remove_file(&out);
    }
}
