// Built-in text loading (shared preview foundation).
//
// Responsibilities:
//   - is_probably_text: judge text vs. binary by looking at the leading bytes (NUL byte presence).
//   - load: read a file with size/line-count caps and return it split into lines.
// Zero external dependencies. Always works even in a bare environment (satisfies "works even
// without external dependencies").
// Caps are in place so huge files don't freeze the app; consider moving this to a separate thread
// later if it gets heavy.

use std::io::Read;
use std::path::Path;

/// Number of bytes read at once for detection and incremental reading.
const SNIFF_BYTES: usize = 8 * 1024;
/// Maximum bytes read for preview (to guard against huge files). `pub(crate)` so
/// `preview::command`'s captured-stdout cap can share it (a delegated command's output and the
/// windowed text reader then agree on what's actually usable, and neither buffers an unbounded
/// amount from a runaway command like `yes`) — no second definition to drift out of sync.
pub(crate) const MAX_BYTES: usize = 1024 * 1024; // 1 MiB
/// Maximum number of lines shown in preview.
const MAX_LINES: usize = 5000;

/// Peek at the start to judge whether it looks like text.
/// If it contains a NUL byte, treat it as binary (the same naive check as less/file etc.).
/// If it cannot be opened, err on the safe side and return false (= treated as binary → can not preview).
pub fn is_probably_text(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; SNIFF_BYTES];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    if n == 0 {
        return true; // treat an empty file as text
    }
    !buf[..n].contains(&0)
}

/// The loaded text content.
pub struct TextContent {
    pub lines: Vec<String>,
    /// Whether it was truncated at a limit (byte count or line count).
    pub truncated: bool,
}

/// Apply the same size caps as `load()` (`MAX_BYTES` bytes, then `MAX_LINES` lines) to an
/// already-in-memory buffer, returning the resulting prefix as lines and whether either cap fired.
/// `load()` itself calls this (after its own streaming byte-cap read, so the byte branch here is a
/// no-op for it) so the two never drift. It also lets a caller that already holds the **full** file
/// content mirror the exact prefix the render pipeline used, without re-deriving `MAX_BYTES`/
/// `MAX_LINES` itself — the Markdown checkbox/code-block source scanners (`app/md_tasks.rs`,
/// `app/md_items.rs`) need this: they must count only what is actually on screen, or a document
/// larger than the preview caps disagrees with the renderer on how many items exist and every
/// toggle/copy in the document is refused ("file changed on disk"), even ones fully on screen.
/// Byte-slicing is done on the raw bytes (never on an already-decoded `&str`), so a cut that lands
/// mid-character is safe — `from_utf8_lossy` turns it into a trailing replacement character instead
/// of panicking on a non-boundary slice.
pub fn cap_lines(bytes: &[u8]) -> (Vec<String>, bool) {
    let mut truncated = false;
    let capped: &[u8] = if bytes.len() > MAX_BYTES {
        truncated = true;
        &bytes[..MAX_BYTES]
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(capped);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        truncated = true;
    }
    (lines, truncated)
}

/// Read the file with limits and split it into lines. Non-UTF-8 bytes are converted lossily.
pub fn load(path: &Path) -> std::io::Result<TextContent> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; SNIFF_BYTES];
    let mut byte_truncated = false;
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len() + n > MAX_BYTES {
            let take = MAX_BYTES - buf.len();
            buf.extend_from_slice(&chunk[..take]);
            byte_truncated = true;
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let (lines, line_truncated) = cap_lines(&buf);
    Ok(TextContent {
        lines,
        truncated: byte_truncated || line_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("konoma_text_test_{name}"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn detects_text_and_loads_content() {
        let p = tmp("plain.txt", b"hello\nworld\n");
        assert!(is_probably_text(&p));
        let c = load(&p).unwrap();
        assert_eq!(c.lines, vec!["hello".to_string(), "world".to_string()]);
        assert!(!c.truncated);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn detects_binary_via_nul_byte() {
        let p = tmp("blob.bin", &[0x89, 0x50, 0x00, 0x01, 0x02]);
        assert!(!is_probably_text(&p));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn empty_file_is_text() {
        let p = tmp("empty", b"");
        assert!(is_probably_text(&p));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cap_lines_truncates_by_line_count() {
        let mut src = String::new();
        for i in 0..(MAX_LINES + 5) {
            src.push_str(&format!("line {i}\n"));
        }
        let (lines, truncated) = cap_lines(src.as_bytes());
        assert!(truncated);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[MAX_LINES - 1], format!("line {}", MAX_LINES - 1));
    }

    #[test]
    fn cap_lines_truncates_by_byte_count() {
        // Well under MAX_LINES but over MAX_BYTES: only the byte cap should fire.
        let line = "x".repeat(200);
        let mut src = String::new();
        while src.len() <= MAX_BYTES {
            src.push_str(&line);
            src.push('\n');
        }
        let (lines, truncated) = cap_lines(src.as_bytes());
        assert!(truncated);
        let rejoined: String = lines.join("\n");
        assert!(rejoined.len() <= MAX_BYTES);
        // Every full line kept is a byte-exact prefix of the source (no corruption at the cut).
        for l in &lines[..lines.len().saturating_sub(1)] {
            assert_eq!(l, &line);
        }
    }

    #[test]
    fn cap_lines_is_a_noop_under_both_caps() {
        let (lines, truncated) = cap_lines(b"hello\nworld\n");
        assert_eq!(lines, vec!["hello".to_string(), "world".to_string()]);
        assert!(!truncated);
    }

    #[test]
    fn load_and_cap_lines_agree_for_a_line_count_truncated_file() {
        let mut src = String::new();
        for i in 0..(MAX_LINES + 5) {
            src.push_str(&format!("line {i}\n"));
        }
        let p = tmp("cap_parity.txt", src.as_bytes());
        let loaded = load(&p).unwrap();
        let (capped, _) = cap_lines(src.as_bytes());
        assert_eq!(loaded.lines, capped);
        assert!(loaded.truncated);
        std::fs::remove_file(&p).ok();
    }
}
