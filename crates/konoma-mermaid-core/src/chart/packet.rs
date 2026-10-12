//! mermaid's `packet-beta` language.
//!
//! Read from `packet.langium`, `parser.ts` and `db.ts` at tag `mermaid@11.17.2`.
//!
//! ```text
//! PacketBlock: ( start=INT ('-' end=INT)? | '+' bits=INT ) ':' label=STRING EOL;
//! ```
//!
//! The grammar is three lines and the meaning is all in `populate`:
//!
//! * **Fields must be contiguous.** `start ??= lastBit + 1`, and then
//!   `if (start !== lastBit + 1) throw`. A packet with a hole in it is refused, not drawn with a
//!   gap — which is right: a gap that looks like a gap and a gap that looks like a mistake are
//!   the same picture.
//! * **`+n` means "the next n bits"**, so a whole packet can be written without ever naming an
//!   index, and mixing the two forms is legal as long as they agree.
//! * **`end < start` is an error**, and so is `+0` ("Cannot have a zero bit field").
//! * **A field that crosses a row boundary is split into two**, one per row, both keeping the
//!   label. The row is `bitsPerRow` wide — 32 by default. That split is what makes a 64-bit field
//!   readable, and it is why [`Field::start`] is not always where the author wrote it.
//!
//! What konoma does not take from upstream is the *drawing*: the crate konoma is replacing lays
//! every field out at the same width in one row, so a 32-bit field and a 16-bit one look alike,
//! which is the one thing a packet diagram exists to show.

use super::{find_header, first_word, read_quoted, Envelope, ParseError, Preamble, TitleSyntax};

/// The keywords, longest first.
pub const KEYWORDS: &[&str] = &["packet-beta", "packet"];

/// The name used in messages.
pub const KEYWORD: &str = "packet";

/// How many bits a row holds. mermaid's `packet.bitsPerRow`, schema default 32.
pub const BITS_PER_ROW: u32 = 32;

/// Upstream's ceiling on how many fields it will lay out.
pub const MAX_FIELDS: usize = 10_000;

/// How many rows konoma will draw.
///
/// Upstream has no such limit and will happily accept `0-4294967295`, whose row split is 134
/// million rectangles — the arithmetic overflows before the drawing gets a chance to hang.
/// 128 rows is 4,096 bits, which is longer than any real packet header and still a picture; past
/// that a reader is better served by the source than by a wall of rectangles.
pub const MAX_ROWS: u32 = 128;

/// One drawn field: a run of bits inside a single row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// First bit, inclusive. Absolute, not relative to the row.
    pub start: u32,
    /// Last bit, inclusive.
    pub end: u32,
    /// The quoted label.
    pub label: String,
}

impl Field {
    /// How many bits wide it is.
    pub fn bits(&self) -> u32 {
        self.end - self.start + 1
    }
}

/// A parsed packet diagram: rows of fields, already split at the row boundaries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Packet {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// One entry per row, each holding the fields that fall in it.
    pub rows: Vec<Vec<Field>>,
}

/// Whether this source is a packet diagram.
pub fn is_packet(src: &str) -> bool {
    find_header(src, KEYWORDS).is_some()
}

/// Parses a mermaid packet diagram.
pub fn parse(src: &str) -> Result<Packet, ParseError> {
    let Some(source) = find_header(src, KEYWORDS) else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };
    let mut env = Envelope::new(source.front_matter_title);
    // The blocks as written, before the row split.
    let mut blocks: Vec<Field> = Vec::new();
    let mut last_bit: i64 = -1;

    let mut feed: Vec<(usize, String)> = Vec::new();
    if !source.header_rest.is_empty() {
        feed.push((source.header_index + 1, source.header_rest.clone()));
    }
    for (i, line) in source
        .lines
        .iter()
        .enumerate()
        .skip(source.header_index + 1)
    {
        feed.push((i + 1, line.clone()));
    }

    for (number, line) in feed {
        if line.trim().is_empty() {
            continue;
        }
        if env.read(&line, TitleSyntax::Terminal) {
            continue;
        }
        let t = line.trim();
        let Some((head, tail)) = t.split_once(':') else {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: t.to_string(),
            });
        };
        let chars: Vec<char> = tail.trim().chars().collect();
        let Some((label, used)) = read_quoted(&chars, 0) else {
            return Err(ParseError::Invalid {
                line: number,
                message: "a field's label must be quoted".to_string(),
            });
        };
        if !chars[used..].iter().collect::<String>().trim().is_empty() {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: chars[used..].iter().collect::<String>().trim().to_string(),
            });
        }

        let head = head.trim();
        let (start, end) = if let Some(bits) = head.strip_prefix('+') {
            let bits = integer(bits.trim(), number)?;
            if bits == 0 {
                return Err(ParseError::Invalid {
                    line: number,
                    message: format!("packet block {} cannot be a zero bit field", last_bit + 1),
                });
            }
            let start = (last_bit + 1) as u32;
            (start, start + bits - 1)
        } else {
            match head.split_once('-') {
                Some((a, b)) => {
                    let start = integer(a.trim(), number)?;
                    let end = integer(b.trim(), number)?;
                    if end < start {
                        return Err(ParseError::Invalid {
                            line: number,
                            message: format!(
                                "packet block {start} - {end} is invalid; end must not be before start"
                            ),
                        });
                    }
                    (start, end)
                }
                None => {
                    let start = integer(head, number)?;
                    (start, start)
                }
            }
        };
        if end / BITS_PER_ROW >= MAX_ROWS {
            return Err(ParseError::Invalid {
                line: number,
                message: format!(
                    "bit {end} is past the {} bits a packet may be drawn across",
                    MAX_ROWS * BITS_PER_ROW
                ),
            });
        }
        if start as i64 != last_bit + 1 {
            return Err(ParseError::Invalid {
                line: number,
                message: format!(
                    "packet block {start} - {end} is not contiguous; it should start at {}",
                    last_bit + 1
                ),
            });
        }
        last_bit = end as i64;
        blocks.push(Field { start, end, label });
        if blocks.len() > MAX_FIELDS {
            return Err(ParseError::Invalid {
                line: number,
                message: format!("a packet may hold at most {MAX_FIELDS} fields"),
            });
        }
    }

    let mut packet = Packet {
        preamble: env.preamble,
        rows: Vec::new(),
    };
    if blocks.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "field",
        });
    }
    // The row split. A field that reaches past the end of its row is cut there and continues in
    // the next one, keeping its label — `getNextFittingBlock`.
    for block in blocks {
        let mut start = block.start;
        while start <= block.end {
            let row = (start / BITS_PER_ROW) as usize;
            let row_end = (row as u32 + 1) * BITS_PER_ROW - 1;
            let end = block.end.min(row_end);
            while packet.rows.len() <= row {
                packet.rows.push(Vec::new());
            }
            packet.rows[row].push(Field {
                start,
                end,
                label: block.label.clone(),
            });
            start = end + 1;
        }
    }
    Ok(packet)
}

fn integer(text: &str, line: usize) -> Result<u32, ParseError> {
    // `INT` in `common.langium` is `/0|[1-9][0-9]*(?!\.)/`: no sign, no leading zero.
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::BadNumber {
            line,
            text: text.to_string(),
            why: "a bit index must be a whole number",
        });
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(ParseError::BadNumber {
            line,
            text: text.to_string(),
            why: "a bit index must not have a leading zero",
        });
    }
    text.parse::<u32>().map_err(|_| ParseError::BadNumber {
        line,
        text: text.to_string(),
        why: "that bit index is too large",
    })
}
