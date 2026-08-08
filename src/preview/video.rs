// Video thumbnail preview: extract a single representative frame with an external tool
// (ffmpegthumbnailer preferred, falling back to ffmpeg), load that PNG, and return a DynamicImage.
// The result is put onto the app's image_src just like SVG, and from there flows through the normal
// image path (prepare_image → worker re-encode → kitty graphics) unchanged.
// **We do not "play" video inside the terminal (thumbnail only)** — real-time playback via kitty
// graphics would be CPU-prohibitive and is parser-bound to the point of breaking down in Ghostty
// (investigated 2026-06-27; see docs/AUDIT).
//
// External tools are strictly an optional dependency. If neither is present or extraction fails,
// we return None and the caller degrades to a safe fallback (a hint message) — PRD §5 ease of
// distribution, principle #3 "unsupported must fail safely".
// Tool execution runs on the media worker thread, so a blocking child process never blocks the UI.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::DynamicImage;

/// Maximum side (px) of the extracted thumbnail. Since the image path shrinks it further to terminal cells, large enough to look crisp is sufficient.
const THUMB_MAX_PX: u32 = 1024;

/// How long `ffmpegthumbnailer`/`ffmpeg` are allowed to run before being killed and treated as a
/// failure. Generous on purpose — extracting a frame from a large or slow-to-seek video can
/// legitimately take several seconds — this exists only to turn "never returns" into "eventually
/// gives up," not to police normal runtimes. Without this, a hung tool (a pathological input that
/// makes it loop forever, say) blocked `Command::status()` indefinitely: since these run on the
/// media worker thread (see the module doc comment above), that leaked the worker thread *and*
/// the child process forever, and left the busy-indicator spinner stuck on.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often [`spawn_and_wait_with_timeout`]'s poll loop re-checks the child. Small enough to
/// notice the deadline promptly without busy-polling.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Extract and return one representative frame from the video at `path`. Returns None if external tools are missing or extraction fails
/// (the caller degrades to a safe fallback with a hint).
pub fn thumbnail(path: &Path) -> Option<DynamicImage> {
    let out = temp_png_path();
    // Prefer ffmpegthumbnailer (dedicated, fast, auto-picks a representative frame); fall back to ffmpeg.
    let ok = run_ffmpegthumbnailer(path, &out) || run_ffmpeg(path, &out);
    let img = if ok {
        image::ImageReader::open(&out)
            .ok()
            .and_then(|r| r.with_guessed_format().ok())
            .and_then(|r| r.decode().ok())
    } else {
        None
    };
    let _ = std::fs::remove_file(&out); // delete the temp file right away (regardless of success)
    img
}

/// Extract with ffmpegthumbnailer. `-s` = max side px / `-q` = quality (1-10) / `-c png`. The representative frame is auto-selected
/// by default (playback position ~10%). Returns true if it succeeds and the output is non-empty.
fn run_ffmpegthumbnailer(path: &Path, out: &Path) -> bool {
    let mut cmd = Command::new("ffmpegthumbnailer");
    cmd.arg("-i")
        .arg(path)
        .arg("-o")
        .arg(out)
        .arg("-s")
        .arg(THUMB_MAX_PX.to_string())
        .arg("-q")
        .arg("8")
        .arg("-c")
        .arg("png")
        // stdin closed (not inherited): without this, ffmpegthumbnailer/ffmpeg contend with
        // crossterm for the same keypress bytes off konoma's own controlling terminal while the
        // media worker thread runs it (`Command`'s documented default, absent an explicit
        // `.stdin(...)`, is `Stdio::inherit()` — see `ffmpeg_tools_never_inherit_this_process_
        // stdin` below for how this is verified deterministically).
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    matches!(spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT), Some(s) if s.success())
        && out_is_nonempty(out)
}

/// Extract with ffmpeg. The `thumbnail` filter picks one representative frame from the first batch, and `scale` caps the max side
/// (`-2` = codec-compatible even dimensions). `-frames:v 1` = one frame / `-y` = overwrite / `-loglevel error` = quiet.
fn run_ffmpeg(path: &Path, out: &Path) -> bool {
    let vf = format!("thumbnail,scale='min({THUMB_MAX_PX},iw)':-2");
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        // `-nostdin`: ffmpeg's own documented switch for "never read stdin for interactive
        // playback controls" — belt-and-suspenders alongside `.stdin(Stdio::null())` below, which
        // is what actually prevents it from ever seeing konoma's terminal input in the first place.
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(vf)
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    matches!(spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT), Some(s) if s.success())
        && out_is_nonempty(out)
}

/// Spawns `cmd` and waits for it to exit, **polling** rather than blocking indefinitely
/// (`Command::status()`'s behavior) — past `timeout` the child is killed and this returns `None`,
/// exactly like "the tool failed" to every caller here (principle #3: this degrades to `[can not
/// preview]`, it never hangs the app). `timeout` is a parameter (not a call to the `TOOL_TIMEOUT`
/// constant baked in here) purely for testability: production call sites above always pass
/// `TOOL_TIMEOUT`; tests pass something short so a deliberately-hanging fixture command doesn't
/// make the test suite itself slow.
fn spawn_and_wait_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let mut child: Child = cmd.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait(); // reap; the (now-meaningless, killed) status is discarded
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return None,
        }
    }
}

/// Whether the output file exists and is non-empty (verify by the actual file, since the content can be empty even with exit code 0).
fn out_is_nonempty(out: &Path) -> bool {
    std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false)
}

/// Lazily creates (once per process), and returns the path to, a private `0700` (owner-only)
/// subdirectory under the system temp dir for this module's extracted-frame PNGs.
/// `std::env::temp_dir()` is world-writable/world-readable on Linux (`/tmp` is mode `1777`) — a
/// rendered video frame would otherwise briefly sit at a pid-and-counter-predictable path readable
/// by any other local user on a shared box, since the PNG itself is created by `ffmpeg`/
/// `ffmpegthumbnailer` (an external process we don't control) under whatever mode their own
/// default umask picks (typically `0644`). Restricting the *directory* (rather than trying to
/// `chmod` the file after an external tool writes it, which we have no reliable hook for anyway)
/// is what actually closes this: POSIX requires execute/search permission on every ancestor
/// directory to open a file by path, so `0700` here blocks other users regardless of the mode the
/// external tool used for the file itself.
fn private_temp_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("konoma-vthumb-{}", std::process::id()));
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

/// Return a temp PNG path that does not collide within the process. Made unique with pid + an atomic counter (no dependence on randomness/time).
fn temp_png_path() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    private_temp_dir().join(format!("thumb-{n}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    /// Serializes the tests in this module that mutate process-global state (`PATH`, this
    /// process's own fd 0) so they never overlap each other's window — `extracts_correct_frame_
    /// when_ffmpeg_available` probes real `ffmpeg` on `PATH`, and `ffmpeg_tools_never_inherit_
    /// this_process_stdin` temporarily shadows `ffmpeg`/`ffmpegthumbnailer` on `PATH` with fakes.
    /// Without this, the two could interleave under the test harness's default parallelism and
    /// make the ffmpeg-availability probe spuriously see the fake script instead of the real tool.
    static PATH_MUTATING_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Returns None when external tools are missing or the target is not a video (does not crash; safe fallback).
    #[test]
    fn nonexistent_or_nonvideo_returns_none() {
        assert!(thumbnail(Path::new("/no/such/video.mp4")).is_none());
    }

    /// If ffmpeg is on PATH, verify that a thumbnail can actually be extracted from a generated tiny video, down to **the extracted frame's
    /// content (color) matching the source video** (not "an image appeared" but "the correct frame appeared").
    /// Skipped on environments without ffmpeg (an optional dependency, so it does not break CI).
    #[test]
    fn extracts_correct_frame_when_ffmpeg_available() {
        let _guard = PATH_MUTATING_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let has_ffmpeg = Command::new("ffmpeg")
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !has_ffmpeg {
            eprintln!("skip: ffmpeg 不在");
            return;
        }
        // Generate a 64x64, 1-second solid-green video with lavfi and check the extracted frame's center is green.
        // `.with_extension` (not baking `.mp4` into the `unique_tmp` prefix) keeps the extension
        // at the very end of the path — ffmpeg infers the output muxer from it, so
        // `…-green.mp4_1234_5` (no trailing `.mp4`) fails to encode.
        let vid = unique_tmp("konoma-vthumb-test-green").with_extension("mp4");
        let _ = std::fs::remove_file(&vid);
        let made = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("color=c=green:s=64x64:d=1")
            .arg(&vid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(made, "テスト用動画の生成に失敗");

        let img = thumbnail(&vid).expect("ffmpeg があればサムネイルが取れるはず");
        assert!(img.width() > 0 && img.height() > 0, "サムネイル寸法が 0");
        // The center pixel is green-dominant (g > r and g > b) = the extracted frame really is from the source video.
        let rgba = img.to_rgba8();
        let px = rgba.get_pixel(rgba.width() / 2, rgba.height() / 2);
        let (r, g, b) = (px[0], px[1], px[2]);
        assert!(
            g > r && g > b && g > 60,
            "中央が緑でない(抽出フレームが元動画と不一致?): rgb=({r},{g},{b})"
        );
        std::fs::remove_file(&vid).ok();
    }

    /// Root cause: `run_ffmpegthumbnailer`/`run_ffmpeg` set stdout/stderr to `Stdio::null()` but
    /// never touch stdin, so `Command`'s documented default (`Stdio::inherit()`) hands the child
    /// *this process's own* stdin. Inside konoma that's the user's live controlling terminal, so
    /// ffmpeg/ffmpegthumbnailer — spawned from the media worker thread while the app is running —
    /// end up racing crossterm for the same keypress bytes (no `-nostdin` either).
    ///
    /// Proven deterministically and OS-independently, with **no timing/hang risk** either way:
    /// this test process's own fd 0 is redirected (via raw `dup`/`dup2`/`close` — always linked
    /// into any Unix Rust binary via libc, so no extra crate dependency is needed) to a small
    /// *regular* file holding a sentinel string. Reading a regular file can never block (unlike a
    /// live pipe/terminal), so there is no risk of this test hanging under either the buggy or the
    /// fixed behavior. Fake `ffmpeg`/`ffmpegthumbnailer` scripts are placed first on `PATH`; each
    /// just `cat`s whatever it receives on stdin into a marker file next to itself. If stdin is
    /// `/dev/null` (fixed), the marker is empty; if stdin is inherited (the bug), the marker
    /// contains the sentinel.
    #[cfg(unix)]
    #[test]
    fn ffmpeg_tools_never_inherit_this_process_stdin() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::io::AsRawFd;

        // No `libc` crate dependency needed: `dup`/`dup2`/`close` are POSIX functions always
        // linked into a Unix Rust binary (std itself is built on top of them) — declaring the FFI
        // signature ourselves is enough for the linker to resolve them.
        extern "C" {
            fn dup(fd: i32) -> i32;
            fn dup2(oldfd: i32, newfd: i32) -> i32;
            fn close(fd: i32) -> i32;
        }

        let _guard = PATH_MUTATING_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = unique_tmp("konoma_vthumb_stdin_leak_test");
        std::fs::create_dir_all(&dir).expect("create test dir");

        let sentinel_path = dir.join("sentinel.txt");
        std::fs::write(&sentinel_path, b"STDIN_LEAK_SENTINEL\n").unwrap();

        for name in ["ffmpeg", "ffmpegthumbnailer"] {
            let script = dir.join(name);
            std::fs::write(
                &script,
                format!("#!/bin/sh\ncat > \"$(dirname \"$0\")/captured_{name}.txt\"\nexit 1\n"),
            )
            .unwrap();
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.display(), orig_path);
        // SAFETY: serialized against the only other PATH-mutating test in this module via
        // `PATH_MUTATING_TESTS` above, and restored (via the `Restore` guard below) on every exit
        // path — including a failed assertion (this project disallows `panic = "abort"`, so
        // unwinding + `Drop` cleanup is reliable here).
        unsafe { std::env::set_var("PATH", &new_path) };

        let sentinel_file = std::fs::File::open(&sentinel_path).expect("open sentinel");
        // SAFETY: `dup`/`dup2` are called on valid, currently-open file descriptors; return
        // values are checked immediately below.
        let saved_stdin = unsafe { dup(0) };
        assert!(saved_stdin >= 0, "failed to save this process's fd 0");
        let rc = unsafe { dup2(sentinel_file.as_raw_fd(), 0) };
        assert_eq!(rc, 0, "failed to redirect fd 0 to the sentinel file");

        // Restores fd 0 and PATH on every exit path (normal return, `assert!` panic/unwind, or an
        // early `?`/`expect` panic) so a failing assertion never leaves this test process's own
        // stdin/PATH corrupted for whichever test runs next on this thread.
        struct Restore {
            orig_path: String,
            saved_stdin: i32,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    dup2(self.saved_stdin, 0);
                    close(self.saved_stdin);
                    std::env::set_var("PATH", &self.orig_path);
                }
            }
        }
        let _restore = Restore {
            orig_path: orig_path.clone(),
            saved_stdin,
        };

        let out = dir.join("out.png");
        let _ = run_ffmpegthumbnailer(Path::new("/dev/null"), &out);
        let _ = run_ffmpeg(Path::new("/dev/null"), &out);

        let captured_thumbnailer =
            std::fs::read(dir.join("captured_ffmpegthumbnailer.txt")).unwrap_or_default();
        let captured_ffmpeg = std::fs::read(dir.join("captured_ffmpeg.txt")).unwrap_or_default();

        assert!(
            captured_thumbnailer.is_empty(),
            "run_ffmpegthumbnailer が親プロセスの stdin を子に継承している(sentinel を読めてしまった): {:?}",
            String::from_utf8_lossy(&captured_thumbnailer)
        );
        assert!(
            captured_ffmpeg.is_empty(),
            "run_ffmpeg が親プロセスの stdin を子に継承している(sentinel を読めてしまった): {:?}",
            String::from_utf8_lossy(&captured_ffmpeg)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Root cause: `temp_png_path` built its path directly under `std::env::temp_dir()`, which is
    /// world-writable/world-readable on Linux (`/tmp` is mode `1777`) — the extracted video frame
    /// briefly sits at a pid-and-counter-predictable path, readable by any other local user on a
    /// shared box, since the PNG itself is written by `ffmpeg`/`ffmpegthumbnailer` (an external
    /// process we don't control) under whatever mode their own default umask picks (typically
    /// `0644`). Restricting the **directory** (not the file) is what actually closes this — POSIX
    /// requires execute/search permission on every ancestor directory to open a file by path, so a
    /// `0700` directory blocks other users regardless of the mode the external tool used for the
    /// file itself.
    #[cfg(unix)]
    #[test]
    fn temp_png_path_lives_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_png_path();
        let dir = path.parent().expect("temp_png_path has a parent dir");
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
            "抽出フレームの置き場が owner-only(0700) になっていない: {dir:?} mode={mode:#o}"
        );
    }

    /// Root cause: `run_ffmpegthumbnailer`/`run_ffmpeg` used a blocking `Command::status()` with
    /// no upper bound at all — a hung external tool (a pathological input that makes it loop
    /// forever, say) blocked the media worker thread permanently, leaking both the thread and the
    /// child process, and left the busy-indicator spinner stuck spinning.
    ///
    /// Confirmed for real before this fix existed (pasted into the PR/commit description, not kept
    /// as a permanent test): `Command::new("tail").arg("-f").arg("/dev/null").stdout(Stdio::null())
    /// .stderr(Stdio::null()).status()` — the exact shape `run_ffmpeg`/`run_poppler`/`run_sips`
    /// used everywhere before this fix — was still blocked after 3 real seconds waiting on a
    /// `recv_timeout`.
    ///
    /// This permanent test instead proves it deterministically and fast, via `spawn_and_wait_
    /// with_timeout` directly with a short injected timeout (production always uses the real,
    /// generous `TOOL_TIMEOUT`). The fixture (`tail -f /dev/null`) genuinely never exits on its
    /// own — unlike e.g. `sleep N`, which eventually completes regardless of whether the timeout
    /// mechanism works, and so wouldn't actually prove anything. This project avoids asserting
    /// *exact* wall-clock durations (a prior CI break — see `docs/STATUS.md`), so the bound below
    /// is a large, deliberately loose multiple of the injected timeout: it exists only so a real
    /// regression (no timeout at all) fails this test instead of hanging the whole test run.
    #[cfg(unix)]
    #[test]
    fn spawn_and_wait_with_timeout_kills_a_command_that_never_exits() {
        let mut cmd = Command::new("tail");
        cmd.arg("-f")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = Instant::now();
        let status = spawn_and_wait_with_timeout(&mut cmd, Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(
            status.is_none(),
            "ハングするコマンドが None(タイムアウト)を返さなかった: {status:?}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "タイムアウトが機能せずブロックし続けた: elapsed={elapsed:?}"
        );
    }
}
