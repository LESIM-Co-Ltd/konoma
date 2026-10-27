//! Reading a text file in windows: the lines asked for, never the whole file in memory.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// The longest line kept, in bytes. A longer line is cut (a minified file is one huge line).
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// Read one line (without its newline and a CR before it) into `out`, keeping at most
/// [`MAX_LINE_BYTES`] of it and skipping the rest of it. Returns false at the end of the file.
fn read_capped_line<R: BufRead>(r: &mut R, out: &mut Vec<u8>) -> std::io::Result<bool> {
    out.clear();
    let mut any = false;
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(any);
        }
        any = true;
        let (take, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        let room = MAX_LINE_BYTES.saturating_sub(out.len());
        let keep = (if done { take - 1 } else { take }).min(room);
        out.extend_from_slice(&buf[..keep]);
        r.consume(take);
        if done {
            if out.last() == Some(&b'\r') {
                out.pop();
            }
            return Ok(true);
        }
    }
}

/// The number of lines of a file (an unterminated last line counts; an empty file has none).
pub fn count_lines(path: &Path) -> std::io::Result<usize> {
    let mut r = BufReader::new(File::open(path)?);
    let mut n = 0;
    let mut line = Vec::new();
    while read_capped_line(&mut r, &mut line)? {
        n += 1;
    }
    Ok(n)
}

/// A line as it is drawn: lossy UTF-8, a tab as four spaces, other control characters dropped.
pub fn clean_line(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\t' {
            out.push_str("    ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// The `n` lines from line `start` (0-based), cleaned for drawing.
pub fn read_window(path: &Path, start: usize, n: usize) -> std::io::Result<Vec<String>> {
    let mut r = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    for _ in 0..start {
        if !read_capped_line(&mut r, &mut line)? {
            return Ok(Vec::new());
        }
    }
    let mut out = Vec::new();
    while out.len() < n && read_capped_line(&mut r, &mut line)? {
        out.push(clean_line(&line));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::unique_tmp;

    fn file(tag: &str, bytes: &[u8]) -> (konoma_test_support::TmpDir, std::path::PathBuf) {
        let d = unique_tmp(tag);
        std::fs::create_dir_all(d.as_path()).unwrap();
        let p = d.as_path().join("f");
        std::fs::write(&p, bytes).unwrap();
        (d, p)
    }

    #[test]
    fn lines_are_counted_with_or_without_a_final_newline() {
        for (body, n) in [
            (&b""[..], 0),
            (b"a", 1),
            (b"a\n", 1),
            (b"a\nb", 2),
            (b"a\nb\n", 2),
            (b"\n", 1),
            (b"\n\n", 2),
            (b"a\r\nb\r\n", 2),
        ] {
            let (_d, p) = file("win_count", body);
            assert_eq!(count_lines(&p).unwrap(), n, "{body:?}");
        }
    }

    #[test]
    fn the_window_is_the_lines_asked_for_and_stops_at_the_end() {
        let (_d, p) = file("win_window", b"0\n1\n2\n3\n4");
        assert_eq!(read_window(&p, 0, 2).unwrap(), vec!["0", "1"]);
        assert_eq!(read_window(&p, 3, 10).unwrap(), vec!["3", "4"]);
        assert_eq!(read_window(&p, 4, 1).unwrap(), vec!["4"]);
        assert!(read_window(&p, 5, 3).unwrap().is_empty());
        assert!(read_window(&p, 99, 3).unwrap().is_empty());
        assert!(read_window(&p, 0, 0).unwrap().is_empty());
    }

    #[test]
    fn a_line_longer_than_the_cap_is_cut_and_the_next_line_is_intact() {
        let mut body = vec![b'w'; MAX_LINE_BYTES * 3 + 5];
        body.extend_from_slice(b"\nnext\n");
        let (_d, p) = file("win_cap", &body);
        let w = read_window(&p, 0, 5).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].len(), MAX_LINE_BYTES);
        assert_eq!(w[1], "next");
        assert_eq!(count_lines(&p).unwrap(), 2);
    }

    #[test]
    fn a_line_is_drawn_clean() {
        assert_eq!(clean_line(b"a\tb"), "a    b");
        assert_eq!(clean_line(b"a\x1b[31mb\x07"), "a[31mb");
        assert_eq!(clean_line(&[b'a', 0xff, b'b']), "a\u{fffd}b");
        assert_eq!(clean_line("日本".as_bytes()), "日本");
        assert_eq!(clean_line(b""), "");
    }

    #[test]
    fn a_file_that_does_not_exist_is_an_error() {
        let d = unique_tmp("win_missing");
        std::fs::create_dir_all(d.as_path()).unwrap();
        assert!(count_lines(&d.as_path().join("x")).is_err());
        assert!(read_window(&d.as_path().join("x"), 0, 1).is_err());
    }

    #[test]
    fn the_cr_before_a_newline_is_not_part_of_the_line() {
        let mut r = std::io::Cursor::new(b"a\r\nb\r\n\r\nc\r".to_vec());
        let mut out = Vec::new();
        let mut got = Vec::new();
        while read_capped_line(&mut r, &mut out).unwrap() {
            got.push(out.clone());
        }
        // A CR at the very end of the file (no newline after it) is kept: only "\r\n" is a break.
        assert_eq!(
            got,
            vec![b"a".to_vec(), b"b".to_vec(), b"".to_vec(), b"c\r".to_vec()]
        );
    }
}
