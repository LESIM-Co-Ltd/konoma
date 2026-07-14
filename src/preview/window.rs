//! less-style windowed reading (does not read the whole file).
//!
//! So that even huge files (several GB) open with constant memory and constant I/O, only the lines in the
//! currently displayed window are read via seek. **The total line count is not retained** (same as less). Scrolling moves
//! relative to "byte offset of the line start", finding the preceding/following line starts on demand. This also frees it from the u16/usize line-number limit.
//!
//! line start = the start of the file, or the byte after the preceding `\n`. When the file ends with `\n`,
//! no empty final line is created (same behavior as `str::lines()`).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Read unit for one backward/forward scan pass.
const CHUNK: usize = 64 * 1024;

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
    pub fn read_lines(&mut self, top: u64, count: usize) -> std::io::Result<Vec<String>> {
        if top >= self.len || count == 0 {
            return Ok(Vec::new());
        }
        self.file.seek(SeekFrom::Start(top))?;
        let mut reader = BufReader::new(&mut self.file);
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = Vec::new();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break; // EOF
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
                return Ok((self.len, found)); // 進みきれない＝末尾(呼び出し側で clamp)
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
        // 後方へ '\n' を数える。1個目は `from` 直前の境界。(n+1)個目の次が n 行前の行頭。
        // 先頭まで尽きたら行0(オフセット0)で、戻れた行数 = 見つけた '\n' 数。
        let mut found = 0usize;
        let mut end = from; // [0, end) を後方へ走査
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
        // 末尾が \n でなければ最終行(改行なし)を 1 行として加える。
        if last != b'\n' {
            count += 1;
        }
        Ok(count)
    }

    /// Return **all occurrences** of `query` (substring match, case-insensitive) as
    /// **(line-start byte offset, 0-based line number, in-line byte column)** in ascending order (line, then column).
    /// If a line has several, return each (so `n`/`N` can move per occurrence). Reading line by line means no misses at chunk boundaries.
    /// Cut off at `cap` results (to guard against huge files).
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
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            // 行内の全出現を左から拾う。col は行頭からのバイト位置(小文字化済み文字列基準)。
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
            offset += n as u64;
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
        // 末尾バイトが \n なら、その直前の文字が最終行の一部。
        let last_byte = self.byte_at(self.len - 1)?;
        let probe = if last_byte == b'\n' {
            if self.len < 2 {
                return Ok(0); // ファイルが "\n" のみ
            }
            self.len - 2
        } else {
            self.len - 1
        };
        self.line_start_at(probe)
    }

    /// Line-start offset of the line containing byte `pos` (the byte after the preceding `\n`, or 0 if none).
    fn line_start_at(&mut self, pos: u64) -> std::io::Result<u64> {
        let mut end = pos + 1; // [0, end) に \n を後方探索
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
    use std::io::Write;
    use std::path::PathBuf;

    fn tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("konoma_window_{name}"));
        let mut f = File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn reads_window_and_strips_newlines() {
        let p = tmp("basic", b"l0\nl1\nl2\nl3\nl4\n");
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 2).unwrap(), vec!["l0", "l1"]);
        // 2 行進んだ行頭から 2 行。
        let (t2, moved) = w.advance(0, 2).unwrap();
        assert_eq!(moved, 2);
        assert_eq!(w.read_lines(t2, 2).unwrap(), vec!["l2", "l3"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn advance_retreat_report_moved_line_counts() {
        let p = tmp("moved", b"a\nb\nc\nd\ne\n"); // 5 行
        let mut w = FileWindow::open(&p).unwrap();
        let (t3, m) = w.advance(0, 3).unwrap(); // 行3(d)へ
        assert_eq!(m, 3);
        assert_eq!(w.read_lines(t3, 1).unwrap(), vec!["d"]);
        // 1 行戻る → c。
        let (b1, m1) = w.retreat(t3, 1).unwrap();
        assert_eq!(m1, 1);
        assert_eq!(w.read_lines(b1, 1).unwrap(), vec!["c"]);
        // 行3から10行戻る → 行0で頭打ち、moved=3。
        let (b0, m0) = w.retreat(t3, 10).unwrap();
        assert_eq!((b0, m0), (0, 3));
        // 総行数。
        assert_eq!(w.count_lines().unwrap(), 5);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn advance_then_retreat_roundtrip() {
        let p = tmp("rt", b"a\nbb\nccc\ndddd\ne\n");
        let mut w = FileWindow::open(&p).unwrap();
        let (t3, _) = w.advance(0, 3).unwrap(); // -> "dddd" の行頭
        assert_eq!(w.read_lines(t3, 1).unwrap(), vec!["dddd"]);
        let (back, _) = w.retreat(t3, 3).unwrap(); // 戻ると先頭
        assert_eq!(back, 0);
        let (back1, _) = w.retreat(t3, 1).unwrap(); // 1 行前 = "ccc"
        assert_eq!(w.read_lines(back1, 1).unwrap(), vec!["ccc"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn find_all_matches_returns_each_occurrence_with_column() {
        let p = tmp("find", b"alpha\nBETA beta\ngamma\nbeta2\n");
        let mut w = FileWindow::open(&p).unwrap();
        // "beta"(大小無視)の出現: line1 に2つ("BETA"@0,"beta"@5), line3 に1つ("beta2"@0)。
        let hits = w.find_all_matches("beta", 100).unwrap();
        assert_eq!(hits.len(), 3, "出現単位で3件: {hits:?}");
        // (offset, line, col)
        assert_eq!(hits[0], (6, 1, 0), "line1 の BETA(col0)");
        assert_eq!(hits[1], (6, 1, 5), "line1 の beta(col5・同一行2つ目)");
        assert_eq!(hits[2], (22, 3, 0), "line3 の beta2(col0)");
        assert_eq!(w.read_lines(hits[0].0, 1).unwrap(), vec!["BETA beta"]);
        assert_eq!(w.read_lines(hits[2].0, 1).unwrap(), vec!["beta2"]);
        // 一致なし。
        assert!(w.find_all_matches("zzz", 100).unwrap().is_empty());
        // cap で打ち切り(同一行の2つ目で頭打ち)。
        assert_eq!(w.find_all_matches("beta", 1).unwrap().len(), 1);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn no_trailing_newline_keeps_last_line() {
        let p = tmp("notrail", b"x\ny\nz"); // 末尾改行なし
        let mut w = FileWindow::open(&p).unwrap();
        let all = w.read_lines(0, 10).unwrap();
        assert_eq!(all, vec!["x", "y", "z"]);
        // 末尾ページ(2 行)。
        let top = w.last_page_top(2).unwrap();
        assert_eq!(w.read_lines(top, 2).unwrap(), vec!["y", "z"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn trailing_newline_no_phantom_empty_line() {
        let p = tmp("trail", b"x\ny\n"); // 末尾改行あり → 空行を作らない
        let mut w = FileWindow::open(&p).unwrap();
        assert_eq!(w.read_lines(0, 10).unwrap(), vec!["x", "y"]);
        let top = w.last_page_top(1).unwrap(); // 最終行 = "y"
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
}
