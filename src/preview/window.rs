//! less-style windowed reading (does not read the whole file).
//!
//! So that even huge files (several GB) open with constant memory and constant I/O, only the lines in the
//! currently displayed window are read via seek. **The total line count is not retained** (same as less). Scrolling moves
//! relative to "byte offset of the line start", finding the preceding/following line starts on demand. This also frees it from the u16/usize line-number limit.
//!
//! line start = the start of the file, or the byte after the preceding `\n`. When the file ends with `\n`,
//! no empty final line is created (same behavior as `str::lines()`).
//!
//! "Constant memory" also holds **per line**: a single line is only ever materialized up to
//! `MAX_LINE_BYTES` (see below). Without that cap, a file with no newlines at all (minified JS, a
//! one-line JSON log, a one-line CSV — all of which really occur) would be read into one `String`,
//! which is exactly what the windowed reader exists to avoid.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Read unit for one backward/forward scan pass.
const CHUNK: usize = 64 * 1024;

/// Maximum bytes materialized for **one** line. The rest of an over-long line is skipped (not
/// stored) up to the next `\n`, so line boundaries — and therefore every byte offset this module
/// hands out — stay exact.
///
/// Same convention as `preview::text`'s `MAX_BYTES`/`MAX_LINES` (a named cap with the reasoning
/// next to it), but on the other axis: `text` caps the *file* prefix, this caps *one line*.
///
/// Why 8 KiB (measured on this repo, release build):
///   - `read_lines` reads `count` lines per call (`count` = viewport height, ~50), so the worst
///     case in flight is `MAX_LINE_BYTES * count` ≈ 400 KiB. Uncapped, a 64 MiB one-line file
///     allocated 335 MB in a single `read_lines` (a 300 MiB line ≈ 1.6 GB) — and `read_lines` runs
///     **inside `terminal.draw`** (`app::windowed_lines`), i.e. on the UI thread (principle #4).
///   - The per-frame cost is dominated by syntax highlighting the window (~4.5 µs/byte for
///     realistic minified JS). One over-long line among normal ones — the common shape — costs
///     37 ms at 8 KiB but 77 ms at 16 KiB, so 8 KiB is the largest power of two that keeps the
///     realistic worst case inside the <60 ms draw budget.
///   - 8 KiB is ~8192 ASCII columns (~2700 CJK characters), i.e. tens of terminal widths; `h`/`l`
///     scroll 4 columns at a time, so nothing reachable by hand is lost.
///
/// The truncation is deliberately **silent** (no `…`, no marker). `preview::text` has a
/// `truncated` flag rendered as a trailing `Msg::PreviewTruncated` note, but that convention is for
/// truncation at the *end of the document*; there is no existing convention for cutting a single
/// line, and the renderer's own convention for "too wide" (`ui::table`'s `…`) is applied at draw
/// time, not baked into the data. Baking a marker in here would break the invariant the rest of
/// the app relies on — **a displayed line is a byte-exact prefix of the real line** — which is what
/// keeps the 2D caret column (clamped against the displayed line in `app::windowed_lines`, then
/// used to slice the *real* file line in `preview_selection_text`) and the search columns from
/// `find_all_matches` pointing at the right characters.
const MAX_LINE_BYTES: usize = 8 * 1024;

/// Consume bytes up to and including the next `\n` (or EOF) **without storing them**, returning how
/// many were consumed. Used to resynchronize after an over-long line was cut at `MAX_LINE_BYTES`:
/// the remainder must still be walked over, or the next read would start mid-line and every
/// subsequent line (and, in `find_all_matches`, every offset and line number) would be wrong.
/// Uses `fill_buf`/`consume`, so it works in `BufReader`-sized steps regardless of how long the
/// remainder is.
fn skip_to_newline<R: BufRead>(reader: &mut R) -> std::io::Result<u64> {
    let mut skipped = 0u64;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(skipped); // EOF: the line was the last one and is unterminated
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(i) => {
                reader.consume(i + 1);
                return Ok(skipped + i as u64 + 1);
            }
            None => {
                let n = available.len();
                reader.consume(n);
                skipped += n as u64;
            }
        }
    }
}

/// Reader that windows over a file. Holds a `File` and seeks on each request to read.
pub struct FileWindow {
    file: File,
    len: u64,
    /// `(count, top)` memo for `last_page_top`. `len` is fixed for this instance (the preview
    /// reopens the window when the file changes), so the answer varies only with `count` —
    /// without this, every frame paid an EOF seek + backward scan just for the scroll clamp.
    last_page: Option<(usize, u64)>,
}

impl FileWindow {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            len,
            last_page: None,
        })
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    /// Read up to `count` lines from `top` (line-start byte) and return them as strings with newlines (\n/\r) removed.
    /// Non-UTF-8 bytes are converted lossily. If there are fewer lines before EOF, return only those.
    ///
    /// Each line is cut at `MAX_LINE_BYTES` (silently — see that constant). A cut line is a
    /// byte-exact prefix of the real line, and the rest of it is skipped rather than kept, so the
    /// following entries are still the following *lines*.
    pub fn read_lines(&mut self, top: u64, count: usize) -> std::io::Result<Vec<String>> {
        if top >= self.len || count == 0 {
            return Ok(Vec::new());
        }
        self.file.seek(SeekFrom::Start(top))?;
        let mut reader = BufReader::new(&mut self.file);
        let mut out = Vec::with_capacity(count);
        // Reused across lines: its capacity settles at the longest line seen (≤ MAX_LINE_BYTES)
        // instead of being regrown from empty for each of `count` lines, every frame.
        let mut buf = Vec::new();
        for _ in 0..count {
            buf.clear();
            // `Take` bounds this one line's read; the wrapper is dropped each iteration, so the
            // allowance resets per line.
            let n = (&mut reader)
                .take(MAX_LINE_BYTES as u64)
                .read_until(b'\n', &mut buf)?;
            if n == 0 {
                break; // EOF
            }
            if !buf.ends_with(b"\n") {
                // Either the line was cut at the cap, or this is an unterminated final line. In the
                // first case we must walk over the remainder to stay aligned; in the second the
                // skip immediately hits EOF and does nothing.
                skip_to_newline(&mut reader)?;
            }
            while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                buf.pop();
            }
            out.push(String::from_utf8_lossy(&buf).into_owned());
        }
        Ok(out)
    }

    /// Return the line-start offset `n` lines forward from `from` (a line start), and the number of lines actually advanced.
    /// Does not go past EOF (returns `len` if it would). Also returns the actual lines moved for line-number tracking.
    pub fn advance(&mut self, from: u64, n: usize) -> std::io::Result<(u64, usize)> {
        if n == 0 || from >= self.len {
            return Ok((from.min(self.len), 0));
        }
        self.file.seek(SeekFrom::Start(from))?;
        let mut pos = from;
        let mut found = 0usize;
        let mut buf = [0u8; CHUNK];
        loop {
            let read = self.file.read(&mut buf)?;
            if read == 0 {
                return Ok((self.len, found)); // couldn't advance the full amount = EOF (caller clamps)
            }
            for (i, &b) in buf[..read].iter().enumerate() {
                if b == b'\n' {
                    found += 1;
                    if found == n {
                        return Ok(((pos + i as u64 + 1).min(self.len), found));
                    }
                }
            }
            pos += read as u64;
        }
    }

    /// Return the line-start offset `n` lines back from `from` (a line start), and the number of lines actually moved back.
    /// Does not go below the start (0). For a line start `from` (>0), the byte just before it, `from-1`, is always `\n`. Counting from there,
    /// the byte after the (n+1)-th `\n` is the answer (= the line start n lines earlier). If not found, the file start 0.
    pub fn retreat(&mut self, from: u64, n: usize) -> std::io::Result<(u64, usize)> {
        if n == 0 || from == 0 {
            return Ok((0, 0));
        }
        // Count '\n' going backward. The 1st one is the boundary just before `from`. The one after
        // the (n+1)-th is the line start n lines earlier. If it runs out before the start, that's
        // line 0 (offset 0), and the number of lines moved back = the number of '\n' found.
        let mut found = 0usize;
        let mut end = from; // scan backward over [0, end)
        let mut buf = vec![0u8; CHUNK];
        while end > 0 {
            let chunk_len = (CHUNK as u64).min(end) as usize;
            let start = end - chunk_len as u64;
            self.file.seek(SeekFrom::Start(start))?;
            let slice = &mut buf[..chunk_len];
            self.file.read_exact(slice)?;
            for i in (0..chunk_len).rev() {
                if slice[i] == b'\n' {
                    found += 1;
                    if found == n + 1 {
                        return Ok((start + i as u64 + 1, n));
                    }
                }
            }
            end = start;
        }
        Ok((0, found))
    }

    /// Total number of displayed lines (a trailing-newline empty line is not counted). Call only when needed for `G` and line-number display (scans the whole file).
    pub fn count_lines(&mut self) -> std::io::Result<usize> {
        if self.len == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut count = 0usize;
        let mut last = 0u8;
        let mut buf = [0u8; CHUNK];
        loop {
            let read = self.file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            for &b in &buf[..read] {
                if b == b'\n' {
                    count += 1;
                }
            }
            last = buf[read - 1];
        }
        // If the last byte isn't \n, count the final (unterminated) line as one more line.
        if last != b'\n' {
            count += 1;
        }
        Ok(count)
    }

    /// Return **all occurrences** of `query` (substring match, case-insensitive) as
    /// **(line-start byte offset, 0-based line number, in-line byte column)** in ascending order (line, then column).
    /// If a line has several, return each (so `n`/`N` can move per occurrence). Reading line by line means no misses at chunk boundaries.
    /// Cut off at `cap` results (to guard against huge files).
    ///
    /// **Known limitation:** each line is only searched up to `MAX_LINE_BYTES` (the same cut
    /// `read_lines` applies), so an occurrence past that point in an over-long line is not
    /// reported — including one that straddles the boundary. This is deliberate: those bytes are
    /// not displayable either, so reporting them would produce a match `n`/`N` could never show.
    pub fn find_all_matches(
        &mut self,
        query: &str,
        cap: usize,
    ) -> std::io::Result<Vec<(u64, usize, usize)>> {
        let q = query.to_lowercase();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(&mut self.file);
        let mut offset = 0u64;
        let mut line_idx = 0usize;
        let mut out = Vec::new();
        let mut buf = Vec::new();
        'outer: loop {
            buf.clear();
            let n = (&mut reader)
                .take(MAX_LINE_BYTES as u64)
                .read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            // Bytes of this line beyond the cap are skipped, not searched — but they must still be
            // counted, since `offset` is the *real* byte offset of the line start (it is handed to
            // `read_lines`, so being off by the skipped amount would show the wrong line).
            let skipped = if buf.ends_with(b"\n") {
                0
            } else {
                skip_to_newline(&mut reader)?
            };
            // Pick up every occurrence in the line, left to right. `col` is the byte position from
            // the line start (relative to the lowercased string).
            let line = String::from_utf8_lossy(&buf).to_lowercase();
            let mut from = 0usize;
            while let Some(rel) = line[from..].find(&q) {
                let col = from + rel;
                out.push((offset, line_idx, col));
                if out.len() >= cap {
                    break 'outer;
                }
                from = col + q.len();
            }
            offset += n as u64 + skipped;
            line_idx += 1;
        }
        Ok(out)
    }

    /// Line-start offset for displaying the last `count` lines (for `G` and the per-frame scroll
    /// clamp). Memoized per `count` — see `last_page`.
    pub fn last_page_top(&mut self, count: usize) -> std::io::Result<u64> {
        if let Some((c, top)) = self.last_page {
            if c == count {
                return Ok(top);
            }
        }
        let last = self.last_line_start()?;
        let top = if count <= 1 {
            last
        } else {
            self.retreat(last, count - 1)?.0
        };
        self.last_page = Some((count, top));
        Ok(top)
    }

    /// Line-start offset of the final (displayed) line. When the file ends with `\n`, no empty final line is created.
    fn last_line_start(&mut self) -> std::io::Result<u64> {
        if self.len == 0 {
            return Ok(0);
        }
        // If the last byte is \n, the character just before it belongs to the final line.
        let last_byte = self.byte_at(self.len - 1)?;
        let probe = if last_byte == b'\n' {
            if self.len < 2 {
                return Ok(0); // the file is just "\n"
            }
            self.len - 2
        } else {
            self.len - 1
        };
        self.line_start_at(probe)
    }

    /// Line-start offset of the line containing byte `pos` (the byte after the preceding `\n`, or 0 if none).
    fn line_start_at(&mut self, pos: u64) -> std::io::Result<u64> {
        let mut end = pos + 1; // search backward for \n over [0, end)
        let mut buf = vec![0u8; CHUNK];
        while end > 0 {
            let chunk_len = (CHUNK as u64).min(end) as usize;
            let start = end - chunk_len as u64;
            self.file.seek(SeekFrom::Start(start))?;
            let slice = &mut buf[..chunk_len];
            self.file.read_exact(slice)?;
            for i in (0..chunk_len).rev() {
                if slice[i] == b'\n' {
                    return Ok(start + i as u64 + 1);
                }
            }
            end = start;
        }
        Ok(0)
    }

    fn byte_at(&mut self, pos: u64) -> std::io::Result<u8> {
        self.file.seek(SeekFrom::Start(pos))?;
        let mut b = [0u8; 1];
        self.file.read_exact(&mut b)?;
        Ok(b[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use std::io::Write;
    use std::path::PathBuf;

    fn tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let p = unique_tmp(&format!("konoma_window_{name}"));
        let mut f = File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn reads_window_and_strips_newlines() {
        let p = tmp("basic", b"l0\nl1\nl2\nl3\nl4\n");
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 2).unwrap(), vec!["l0", "l1"]);
        // 2 lines from the line start reached by advancing 2 lines.
        let (t2, moved) = w.advance(0, 2).unwrap();
        assert_eq!(moved, 2);
        assert_eq!(w.read_lines(t2, 2).unwrap(), vec!["l2", "l3"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn advance_retreat_report_moved_line_counts() {
        let p = tmp("moved", b"a\nb\nc\nd\ne\n"); // 5 lines
        let mut w = FileWindow::open(&p).unwrap();
        let (t3, m) = w.advance(0, 3).unwrap(); // to line 3 (d)
        assert_eq!(m, 3);
        assert_eq!(w.read_lines(t3, 1).unwrap(), vec!["d"]);
        // Move back 1 line → c.
        let (b1, m1) = w.retreat(t3, 1).unwrap();
        assert_eq!(m1, 1);
        assert_eq!(w.read_lines(b1, 1).unwrap(), vec!["c"]);
        // Move back 10 lines from line 3 → capped at line 0, moved=3.
        let (b0, m0) = w.retreat(t3, 10).unwrap();
        assert_eq!((b0, m0), (0, 3));
        // Total line count.
        assert_eq!(w.count_lines().unwrap(), 5);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn advance_then_retreat_roundtrip() {
        let p = tmp("rt", b"a\nbb\nccc\ndddd\ne\n");
        let mut w = FileWindow::open(&p).unwrap();
        let (t3, _) = w.advance(0, 3).unwrap(); // -> the line start of "dddd"
        assert_eq!(w.read_lines(t3, 1).unwrap(), vec!["dddd"]);
        let (back, _) = w.retreat(t3, 3).unwrap(); // moving back reaches the start
        assert_eq!(back, 0);
        let (back1, _) = w.retreat(t3, 1).unwrap(); // 1 line earlier = "ccc"
        assert_eq!(w.read_lines(back1, 1).unwrap(), vec!["ccc"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn find_all_matches_returns_each_occurrence_with_column() {
        let p = tmp("find", b"alpha\nBETA beta\ngamma\nbeta2\n");
        let mut w = FileWindow::open(&p).unwrap();
        // Occurrences of "beta" (case-insensitive): 2 on line1 ("BETA"@0, "beta"@5), 1 on line3 ("beta2"@0).
        let hits = w.find_all_matches("beta", 100).unwrap();
        assert_eq!(hits.len(), 3, "出現単位で3件: {hits:?}");
        // (offset, line, col)
        assert_eq!(hits[0], (6, 1, 0), "line1 の BETA(col0)");
        assert_eq!(hits[1], (6, 1, 5), "line1 の beta(col5・同一行2つ目)");
        assert_eq!(hits[2], (22, 3, 0), "line3 の beta2(col0)");
        assert_eq!(w.read_lines(hits[0].0, 1).unwrap(), vec!["BETA beta"]);
        assert_eq!(w.read_lines(hits[2].0, 1).unwrap(), vec!["beta2"]);
        // No match.
        assert!(w.find_all_matches("zzz", 100).unwrap().is_empty());
        // Cut off by cap (capped at the 2nd occurrence on the same line).
        assert_eq!(w.find_all_matches("beta", 1).unwrap().len(), 1);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn no_trailing_newline_keeps_last_line() {
        let p = tmp("notrail", b"x\ny\nz"); // no trailing newline
        let mut w = FileWindow::open(&p).unwrap();
        let all = w.read_lines(0, 10).unwrap();
        assert_eq!(all, vec!["x", "y", "z"]);
        // Last page (2 lines).
        let top = w.last_page_top(2).unwrap();
        assert_eq!(w.read_lines(top, 2).unwrap(), vec!["y", "z"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn trailing_newline_no_phantom_empty_line() {
        let p = tmp("trail", b"x\ny\n"); // has a trailing newline → doesn't create an empty line
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 10).unwrap(), vec!["x", "y"]);
        let top = w.last_page_top(1).unwrap(); // last line = "y"
        assert_eq!(w.read_lines(top, 1).unwrap(), vec!["y"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn empty_lines_in_middle_are_preserved() {
        let p = tmp("blanks", b"a\n\n\nb\n");
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 4).unwrap(), vec!["a", "", "", "b"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn last_page_top_clamps_when_file_shorter_than_page() {
        let p = tmp("short", b"only\n");
        let mut w = FileWindow::open(&p).unwrap();
        let top = w.last_page_top(10).unwrap();
        assert_eq!(top, 0);
        assert_eq!(w.read_lines(top, 10).unwrap(), vec!["only"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn utf8_multibyte_lossy_safe() {
        let p = tmp("utf8", "あ\nいい\nうううx\n".as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 3).unwrap(), vec!["あ", "いい", "うううx"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn read_past_eof_returns_empty() {
        let p = tmp("eof", b"a\nb\n");
        let mut w = FileWindow::open(&p).unwrap();
        let beyond = w.len();
        assert!(w.read_lines(beyond, 5).unwrap().is_empty());
        std::fs::remove_file(&p).ok();
    }

    // -----------------------------------------------------------------------------------------
    // MAX_LINE_BYTES: a single line is capped, the remainder is *skipped* (not kept), so every
    // line boundary — and therefore every offset this module hands out — stays exact.
    //
    // The axes below are deliberately over-covered (a cut that loses sync doesn't crash, it
    // silently shows the wrong text from the wrong place, which is the hardest kind to notice):
    // cap boundary ±1, consecutive over-long lines, unterminated last line, CRLF straddling the
    // cut, a multi-byte character split by the cut, degenerate files, cross-checks against
    // `advance`/`retreat`/`count_lines`, `find_all_matches` offsets, and a byte-identical
    // non-regression check for ordinary files.
    // -----------------------------------------------------------------------------------------

    /// The invariant every capped read must satisfy: the displayed line is the real line's byte
    /// prefix (lossily decoded), never a marker-decorated or re-encoded string.
    fn expected_capped(real_line: &str) -> String {
        let b = real_line.as_bytes();
        String::from_utf8_lossy(&b[..b.len().min(MAX_LINE_BYTES)]).into_owned()
    }

    #[test]
    fn long_line_is_capped_and_the_next_line_is_still_correct() {
        let long = "x".repeat(MAX_LINE_BYTES + 500);
        let body = format!("{long}\nsecond\nthird\n");
        let p = tmp("cap_basic", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        let lines = w.read_lines(0, 5).unwrap();
        assert_eq!(
            lines,
            vec![
                expected_capped(&long),
                "second".to_string(),
                "third".to_string()
            ],
            "上限で切った後、続く行が「次の行」のままであること"
        );
        assert_eq!(
            lines[0].len(),
            MAX_LINE_BYTES,
            "1行は MAX_LINE_BYTES で切られる"
        );
        assert!(
            lines[0].bytes().all(|b| b == b'x'),
            "切り詰めは本文のバイト列そのまま(マーカーを混ぜない)"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn cap_boundary_off_by_one_is_exact() {
        // A line of exactly MAX_LINE_BYTES is NOT data-loss (its \n just falls outside the
        // allowance, so the skip path runs and must consume exactly that \n).
        for extra in [-1i64, 0, 1] {
            let n = (MAX_LINE_BYTES as i64 + extra) as usize;
            let long = "a".repeat(n);
            let body = format!("{long}\nNEXT\n");
            let p = tmp(&format!("cap_edge_{extra}"), body.as_bytes());
            let mut w = FileWindow::open(&p).unwrap();
            let lines = w.read_lines(0, 3).unwrap();
            assert_eq!(
                lines,
                vec![expected_capped(&long), "NEXT".to_string()],
                "境界 {n} バイトの行で行境界がずれた"
            );
            assert_eq!(lines[0].len(), n.min(MAX_LINE_BYTES));
            // The offset side must agree with the text side.
            let (t1, moved) = w.advance(0, 1).unwrap();
            assert_eq!(moved, 1);
            assert_eq!(t1, n as u64 + 1, "次の行の開始オフセット");
            assert_eq!(w.read_lines(t1, 1).unwrap(), vec!["NEXT"]);
            std::fs::remove_file(&p).ok();
        }
    }

    #[test]
    fn consecutive_long_lines_stay_aligned() {
        let a = "a".repeat(MAX_LINE_BYTES + 7);
        let b = "b".repeat(MAX_LINE_BYTES * 2);
        let c = "c".repeat(MAX_LINE_BYTES + 1);
        let body = format!("{a}\n{b}\n{c}\nend\n");
        let p = tmp("cap_consecutive", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(
            w.read_lines(0, 10).unwrap(),
            vec![
                expected_capped(&a),
                expected_capped(&b),
                expected_capped(&c),
                "end".to_string()
            ],
            "長い行が連続しても1行ずつ正しく進む"
        );
        assert_eq!(w.count_lines().unwrap(), 4);
        // Reading from the middle (the window's usual entry point) lands on the right line too.
        let (t2, _) = w.advance(0, 2).unwrap();
        assert_eq!(t2, (a.len() + 1 + b.len() + 1) as u64);
        assert_eq!(
            w.read_lines(t2, 2).unwrap(),
            vec![expected_capped(&c), "end".to_string()]
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn unterminated_long_final_line() {
        // The pathological real-world shape: a file that is ONE long line with no trailing newline
        // (minified JS / a one-line JSON log).
        let only = "z".repeat(MAX_LINE_BYTES * 3 + 11);
        let p = tmp("cap_only_line", only.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 50).unwrap(), vec![expected_capped(&only)]);
        assert_eq!(w.count_lines().unwrap(), 1);
        assert_eq!(w.last_page_top(10).unwrap(), 0);
        std::fs::remove_file(&p).ok();

        // …and the same shape as the *last* line after normal ones.
        let body = format!("head\n{only}");
        let p2 = tmp("cap_tail_line", body.as_bytes());
        let mut w2 = FileWindow::open(&p2).unwrap();
        assert_eq!(
            w2.read_lines(0, 5).unwrap(),
            vec!["head".to_string(), expected_capped(&only)]
        );
        let top = w2.last_page_top(1).unwrap();
        assert_eq!(top, 5, "最終行の開始オフセット");
        assert_eq!(w2.read_lines(top, 1).unwrap(), vec![expected_capped(&only)]);
        std::fs::remove_file(&p2).ok();
    }

    #[test]
    fn crlf_across_the_cap_boundary() {
        // Three placements of \r\n relative to the cut: \r inside the allowance, \r exactly at it,
        // and both past it. None may leak a \r into the text or eat the following line.
        for extra in [-1i64, 0, 5] {
            let n = (MAX_LINE_BYTES as i64 + extra) as usize;
            let long = "a".repeat(n);
            let body = format!("{long}\r\nNEXT\r\ntail\r\n");
            let p = tmp(&format!("cap_crlf_{extra}"), body.as_bytes());
            let mut w = FileWindow::open(&p).unwrap();
            let lines = w.read_lines(0, 3).unwrap();
            assert_eq!(
                lines,
                vec![
                    expected_capped(&long),
                    "NEXT".to_string(),
                    "tail".to_string()
                ],
                "CRLF が上限境界に跨る({n} バイト)"
            );
            assert!(
                !lines[0].contains('\r'),
                "改行文字が本文に混ざってはいけない"
            );
            std::fs::remove_file(&p).ok();
        }
    }

    #[test]
    fn multibyte_split_at_cap_is_lossy_not_panic() {
        // "あ" is 3 bytes and MAX_LINE_BYTES is not a multiple of 3, so the cut lands mid-character:
        // it must degrade to U+FFFD (from_utf8_lossy), never panic on a non-boundary slice.
        let long: String = "あ".repeat(MAX_LINE_BYTES); // far more bytes than the cap
        let body = format!("{long}\nあとの行\n");
        let p = tmp("cap_multibyte", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        let lines = w.read_lines(0, 2).unwrap();
        assert_eq!(
            lines,
            vec![expected_capped(&long), "あとの行".to_string()],
            "マルチバイトの分断は lossy 変換で処理し、次の行は無傷"
        );
        if !MAX_LINE_BYTES.is_multiple_of(3) {
            assert!(
                lines[0].ends_with('\u{FFFD}'),
                "文字境界でない位置で切れたら置換文字になる"
            );
        }
        assert_eq!(
            lines[0].chars().filter(|c| *c == 'あ').count(),
            MAX_LINE_BYTES / 3,
            "完全な文字はすべて残る"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn degenerate_files_are_unchanged_by_the_cap() {
        for (name, bytes, want) in [
            ("empty", &b""[..], vec![]),
            ("one_byte", &b"a"[..], vec!["a".to_string()]),
            ("just_newline", &b"\n"[..], vec![String::new()]),
            (
                "blank_lines",
                &b"\n\n\n"[..],
                vec![String::new(), String::new(), String::new()],
            ),
        ] {
            let p = tmp(name, bytes);
            let mut w = FileWindow::open(&p).unwrap();
            assert_eq!(w.read_lines(0, 10).unwrap(), want, "{name}");
            std::fs::remove_file(&p).ok();
        }
    }

    #[test]
    fn advance_retreat_and_read_lines_agree_across_long_lines() {
        let long = "L".repeat(MAX_LINE_BYTES + 3);
        let body = format!("a\n{long}\nb\n{long}\nc\n");
        let p = tmp("cap_offsets", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        // Walk forward line by line and check the offset against the string layout each step,
        // then read at that offset (the two must describe the same line).
        let starts: Vec<u64> = {
            let mut v = vec![0u64];
            let mut acc = 0u64;
            for l in ["a", &long, "b", &long] {
                acc += l.len() as u64 + 1;
                v.push(acc);
            }
            v
        };
        let expect = [
            "a".to_string(),
            expected_capped(&long),
            "b".to_string(),
            expected_capped(&long),
            "c".to_string(),
        ];
        for (i, want_start) in starts.iter().enumerate() {
            let (got, moved) = w.advance(0, i).unwrap();
            assert_eq!(got, *want_start, "advance(0,{i}) のオフセット");
            assert_eq!(moved, i);
            assert_eq!(w.read_lines(got, 1).unwrap(), vec![expect[i].clone()]);
        }
        // Backward too (retreat is chunk-scanning and untouched by the cap, but it must still
        // agree with the capped text side).
        let (back, moved) = w.retreat(starts[4], 2).unwrap();
        assert_eq!((back, moved), (starts[2], 2));
        assert_eq!(w.read_lines(back, 1).unwrap(), vec!["b".to_string()]);
        assert_eq!(w.count_lines().unwrap(), 5);
        assert_eq!(
            w.read_lines(0, 10).unwrap(),
            expect.to_vec(),
            "一括読みも1行ずつ読みと一致"
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn find_all_matches_keeps_offsets_exact_across_long_lines() {
        let long = "q".repeat(MAX_LINE_BYTES * 2);
        let body = format!("NEEDLE one\n{long}\nNEEDLE two\n{long}\nlast NEEDLE\n");
        let p = tmp("cap_find_offsets", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        let hits = w.find_all_matches("needle", 100).unwrap();
        let l0 = 11u64; // "NEEDLE one\n"
        let l1 = l0 + long.len() as u64 + 1;
        let l2 = l1 + 11; // "NEEDLE two\n"
        let l3 = l2 + long.len() as u64 + 1;
        assert_eq!(
            hits,
            vec![(0, 0, 0), (l1, 2, 0), (l3, 4, 5)],
            "長い行を跨いでも行番号とオフセットが正確"
        );
        // The offsets are usable: reading at each one shows the matching line.
        for (off, _, _) in &hits {
            assert!(
                w.read_lines(*off, 1).unwrap()[0]
                    .to_lowercase()
                    .contains("needle"),
                "オフセット {off} は一致行の先頭を指す"
            );
        }
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn find_all_matches_is_capped_per_line_like_read_lines() {
        // Documented limitation: an occurrence past MAX_LINE_BYTES (or straddling it) is not
        // reported, because those bytes are not displayable either.
        let inside_col = MAX_LINE_BYTES - 20;
        let mut line0 = "x".repeat(inside_col);
        line0.push_str("NEEDLE"); // fully inside the cap
        line0.push_str(&"y".repeat(200));
        line0.push_str("NEEDLE"); // far beyond the cap
        let body = format!("{line0}\nNEEDLE tail\n");
        let p = tmp("cap_find_limit", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        let hits = w.find_all_matches("needle", 100).unwrap();
        assert_eq!(
            hits,
            vec![(0, 0, inside_col), (line0.len() as u64 + 1, 1, 0)],
            "上限内の一致だけが報告され、次行のオフセットは正確なまま"
        );

        // A match that straddles the cut is not reported either.
        let mut straddle = "x".repeat(MAX_LINE_BYTES - 3);
        straddle.push_str("NEEDLE");
        let body2 = format!("{straddle}\nplain\n");
        let p2 = tmp("cap_find_straddle", body2.as_bytes());
        let mut w2 = FileWindow::open(&p2).unwrap();
        assert!(
            w2.find_all_matches("needle", 100).unwrap().is_empty(),
            "境界を跨ぐ一致は(表示もできないので)報告しない"
        );
        assert_eq!(
            w2.read_lines(1 + straddle.len() as u64, 1).unwrap(),
            vec!["plain"]
        );
        std::fs::remove_file(&p).ok();
        std::fs::remove_file(&p2).ok();
    }

    #[test]
    fn ordinary_files_are_byte_identical_under_the_cap() {
        // Non-regression: for every line shorter than the cap, the reader must behave exactly as
        // `str::lines()` — the cap must be invisible for normal content.
        let mut body = String::new();
        for i in 0..200 {
            body.push_str(&format!(
                "line {i} — 日本語 mixed\tcontent {}\n",
                "z".repeat(i)
            ));
        }
        body.push_str(&"w".repeat(MAX_LINE_BYTES - 1)); // longest allowed, unterminated
        let p = tmp("cap_noregress", body.as_bytes());
        let mut w = FileWindow::open(&p).unwrap();
        let want: Vec<String> = body.lines().map(|s| s.to_string()).collect();
        assert_eq!(w.read_lines(0, 1000).unwrap(), want);
        assert_eq!(w.count_lines().unwrap(), want.len());
        // Windowed reads at every line start agree with the same slice of `str::lines()`.
        let mut top = 0u64;
        for i in 0..want.len() {
            assert_eq!(
                w.read_lines(top, 3).unwrap(),
                want[i..(i + 3).min(want.len())]
            );
            top = w.advance(top, 1).unwrap().0;
        }
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn read_lines_allocation_does_not_scale_with_line_length() {
        // Tight, module-local companion to `mem_tests::windowed_read_is_bounded_per_line`
        // (which owns the scaling check): a single call must stay within a small multiple of the
        // cap no matter how long the line is. Uncapped this allocated ~5.2x the line length.
        let mut body = vec![b'x'; MAX_LINE_BYTES * 64];
        body.push(b'\n');
        body.extend_from_slice(b"tail\n");
        let p = tmp("cap_alloc", &body);
        let mut w = FileWindow::open(&p).unwrap();
        let used = crate::mem_tests::allocated_by(|| {
            let lines = w.read_lines(0, 50).unwrap();
            std::hint::black_box(&lines);
        });
        assert!(
            used < (MAX_LINE_BYTES * 8) as u64,
            "read_lines の確保が行長に比例している: {used} バイト(上限 {MAX_LINE_BYTES} の8倍未満のはず)"
        );
        std::fs::remove_file(&p).ok();
    }
}
