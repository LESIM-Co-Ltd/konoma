//! A `BufRead` adapter that turns invalid UTF-8 into U+FFFD exactly as `String::from_utf8_lossy`
//! does, before the bytes reach the XML reader.
//!
//! Why this exists: quick-xml 0.42 validates UTF-8 when it builds an event and fails with
//! `Error::Encoding` on a stray byte, where 0.41 handed the bytes through and the office readers
//! decoded them lossily. A part with one bad byte in a cell or a run of text must stay readable
//! (the bad byte is a replacement character, "one bad cell is not a bad sheet"), so every XML
//! reader of the office previews is built on top of this adapter (see `fmt_xlsx::XmlReader`,
//! the single place a reader is made).

use std::io::{self, BufRead, Read};

const REPLACEMENT: &str = "\u{FFFD}";

/// Wraps a byte source; what is read from it is always valid UTF-8, equal to
/// `String::from_utf8_lossy` of the whole source (also when a multi-byte sequence is split
/// across two reads of the source).
pub(crate) struct LossyUtf8<R> {
    inner: R,
    /// Converted bytes not yet consumed by the reader.
    buf: Vec<u8>,
    pos: usize,
    /// An incomplete multi-byte sequence at the end of the last chunk, waiting for its rest.
    tail: Vec<u8>,
    eof: bool,
}

impl<R: BufRead> LossyUtf8<R> {
    pub(crate) fn new(inner: R) -> Self {
        LossyUtf8 {
            inner,
            buf: Vec::new(),
            pos: 0,
            tail: Vec::new(),
            eof: false,
        }
    }
}

impl<R: BufRead> BufRead for LossyUtf8<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.pos >= self.buf.len() && !self.eof {
            self.buf.clear();
            self.pos = 0;
            // The pending tail is taken only once the read succeeded: a failed read (say
            // `Interrupted`, which callers retry) must not lose the start of a character.
            let chunk = self.inner.fill_buf()?;
            if chunk.is_empty() {
                self.eof = true;
                if !self.tail.is_empty() {
                    // A sequence cut off by the end of the input is one replacement character.
                    self.tail.clear();
                    self.buf.extend_from_slice(REPLACEMENT.as_bytes());
                }
                break;
            }
            let n = chunk.len();
            let mut data = std::mem::take(&mut self.tail);
            data.extend_from_slice(chunk);
            self.inner.consume(n);
            let mut rest: &[u8] = &data;
            loop {
                match std::str::from_utf8(rest) {
                    Ok(valid) => {
                        self.buf.extend_from_slice(valid.as_bytes());
                        break;
                    }
                    Err(e) => {
                        let (valid, after) = rest.split_at(e.valid_up_to());
                        self.buf.extend_from_slice(valid);
                        match e.error_len() {
                            Some(len) => {
                                self.buf.extend_from_slice(REPLACEMENT.as_bytes());
                                rest = &after[len..];
                            }
                            None => {
                                self.tail = after.to_vec();
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok(&self.buf[self.pos.min(self.buf.len())..])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = (self.pos + amt).min(self.buf.len());
    }
}

impl<R: BufRead> Read for LossyUtf8<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = {
            let avail = self.fill_buf()?;
            let n = avail.len().min(out.len());
            out[..n].copy_from_slice(&avail[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn lossy(bytes: &[u8], cap: usize) -> Vec<u8> {
        let mut out = Vec::new();
        LossyUtf8::new(BufReader::with_capacity(cap, bytes))
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[test]
    fn valid_input_passes_through_unchanged() {
        let s = "plain <a>日本語 \u{1F600}</a>";
        assert_eq!(lossy(s.as_bytes(), 8192), s.as_bytes());
    }

    #[test]
    fn invalid_bytes_become_what_from_utf8_lossy_makes_of_them() {
        let cases: &[&[u8]] = &[
            b"a\xFFb",
            b"a\xC3",                // cut-off sequence at the end
            b"\xE3\x81",             // two thirds of a 3-byte char at the end
            b"\xE3\x81\xFFx",        // invalid continuation
            b"\xF0\x9F\x98",         // 4-byte cut off
            b"\xC0\xAF\xED\xA0\x80", // overlong and surrogate
            b"ok\xFE\xFFok",
            b"",
        ];
        for case in cases {
            // Every chunk size, so the cut falls inside each sequence in turn.
            for cap in 1..=9 {
                assert_eq!(
                    String::from_utf8(lossy(case, cap)).unwrap(),
                    String::from_utf8_lossy(case),
                    "{case:?} read in chunks of {cap}"
                );
            }
        }
    }

    #[test]
    fn every_two_and_three_byte_combination_matches_from_utf8_lossy() {
        let alphabet: [u8; 9] = [0x41, 0x7F, 0x80, 0xBF, 0xC2, 0xE3, 0xED, 0xF0, 0xFF];
        for &a in &alphabet {
            for &b in &alphabet {
                for &c in &alphabet {
                    let v = [a, b, c];
                    for cap in [1, 2, 3, 64] {
                        assert_eq!(
                            String::from_utf8(lossy(&v, cap)).unwrap(),
                            String::from_utf8_lossy(&v),
                            "{v:?} chunks of {cap}"
                        );
                    }
                }
            }
        }
    }

    /// A source that hands out `chunks` and fails once, with `Interrupted`, before chunk `fail_at`.
    struct Flaky {
        chunks: Vec<Vec<u8>>,
        at: usize,
        fail_at: usize,
        failed: bool,
    }

    impl Read for Flaky {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            unreachable!("only fill_buf is used")
        }
    }

    impl BufRead for Flaky {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.at == self.fail_at && !self.failed {
                self.failed = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            Ok(self.chunks.get(self.at).map_or(&[][..], |c| c.as_slice()))
        }
        fn consume(&mut self, _: usize) {
            self.at += 1;
        }
    }

    #[test]
    fn a_failed_read_between_the_halves_of_a_character_loses_nothing() {
        // "a" + the first byte of a 3-byte character | the interrupted read | the rest + "b".
        let chunks = vec![b"a\xE3".to_vec(), b"\x81\x82b".to_vec()];
        let whole: Vec<u8> = chunks.concat();
        let mut out = Vec::new();
        // `read_to_end` retries `Interrupted`, as every BufRead caller may.
        LossyUtf8::new(Flaky {
            chunks,
            at: 0,
            fail_at: 1,
            failed: false,
        })
        .read_to_end(&mut out)
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            String::from_utf8_lossy(&whole)
        );
        assert_eq!(String::from_utf8_lossy(&whole), "a\u{3042}b");
    }
}
