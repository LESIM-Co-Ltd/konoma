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
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};

/// Splits `template` on whitespace into argv, substituting `{path}`/`{out}` **within each
/// whitespace-delimited token** (so a path containing spaces — substituted after the split — stays
/// one argv element, never re-split; no shell is invoked, so `;`/`|`/quoting in the path can't be
/// interpreted as syntax). If no token contains `{path}`, the path is appended at the end (mirrors
/// `config::EditorConfig`'s `build_argv`, which does the same for `{path}`/`{line}`). `{out}` has no
/// such fallback — an output-producing command with no `{out}` token in its template legitimately
/// means "capture stdout instead" (`run_capture`'s `uses_out=false` path).
pub fn build_argv(template: &str, path: &Path, out: &Path) -> Vec<String> {
    let p = path.to_string_lossy().to_string();
    let o = out.to_string_lossy().to_string();
    let mut argv: Vec<String> = template.split_whitespace().map(str::to_string).collect();
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
    std::env::temp_dir().join(format!("konoma-cmd-{}-{}", std::process::id(), n))
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
/// without bound: once the cap is hit we stop reading and kill the child rather than waiting for a
/// natural exit that may never come. Both streams are drained on separate threads (stdout on the
/// caller's, stderr on a helper thread) so a child that interleaves writes to both can't deadlock us
/// by filling one pipe's small OS buffer while we only drain the other.
pub fn run_capture(argv: &[String], out: &Path, uses_out: bool) -> Result<PathBuf> {
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

    let stderr_handle = std::thread::spawn(move || capped_read(&mut stderr, cap));
    let (stdout_buf, stdout_truncated) = capped_read(&mut stdout, cap);
    if stdout_truncated {
        // A runaway/infinite-output command: stop waiting for a natural exit and reap it instead.
        let _ = child.kill();
    }
    let status = child
        .wait()
        .with_context(|| format!("failed to wait for `{prog}`"))?;
    let (stderr_buf, _) = stderr_handle.join().unwrap_or_else(|_| (Vec::new(), false));

    // A capped run's exit status is meaningless (we killed it mid-flight rather than let it finish
    // naturally) — only judge success/failure when we read it to a real EOF.
    if !stdout_truncated && !status.success() {
        let msg = first_nonempty_line(&stderr_buf).unwrap_or_else(|| status.to_string());
        bail!(msg);
    }

    if uses_out {
        resolve_produced_out(out).with_context(|| format!("{} was not produced", out.display()))
    } else {
        std::fs::write(out, &stdout_buf)
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
}
