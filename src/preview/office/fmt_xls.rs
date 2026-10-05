//! Format pass for xls (BIFF8 inside a Compound File): which number format each cell has.
//!
//! Same job as [`super::fmt_xlsx`], over the BIFF records of the `Workbook` (or `Book`) stream:
//! <https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-xls/>
//!
//! Workbook globals (up to the first `EOF`, 0x000A):
//! - `BOF` (0x0809): the BIFF version (0x0600 = BIFF8; anything older is read as BIFF5/7, whose
//!   strings are single-byte).
//! - `FILEPASS` (0x002F) with a non-zero encryption type: encrypted ([`OfficeError::Encrypted`]).
//! - `DATEMODE` (0x0022): `1` is the 1904 date system.
//! - `FORMAT` (0x041E): `ifmt: u16` + the format code (BIFF8 `XLUnicodeString`: `cch: u16`, flags,
//!   8- or 16-bit characters). A format that is split by a `CONTINUE` is cut at the record end.
//! - `XF` (0x00E0): `ifmt` at bytes 2..4. *Every* XF record counts (style XFs included): a cell's
//!   `ixfe` indexes the whole list.
//! - `SST` (0x00FC) with its `CONTINUE`s (0x003C): only the length of each string, for the text
//!   budget (a string that many `LABELSST` cells refer to is copied into each of them).
//! - `BOUNDSHEET8` (0x0085): `lbPlyPos: u32` (where the sheet's substream starts in the stream)
//!   and the name (`ShortXLUnicodeString`).
//!
//! Each sheet substream, up to its first `EOF` (as `calamine` reads it): `NUMBER` 0x0203,
//! `LABEL` 0x0204, `RSTRING` 0x00D6, `BOOLERR` 0x0205, `LABELSST` 0x00FD, `RK` 0x027E and
//! `FORMULA` 0x0006 all start `row: u16, col: u16, ixfe: u16`; `STRING` 0x0207 (the text result of
//! the formula just before it) has no position of its own and `calamine` puts it at the last
//! `FORMULA`'s (at A1 if there was none), so it is a cell like any other here; `MULRK` 0x00BD is `row, colFirst`,
//! then `(ixfe: u16, rk: u32)` per column, then `colLast`. `DIMENSIONS` (0x0200) is only used for
//! the memory check. `CONTINUE` records (0x003C) are skipped as records of their own.
//!
//! **Memory.** `calamine::Xls::new` parses *every* sheet while opening (hidden and chart sheets
//! too) into a dense matrix over the bounding box of the cells, and `reserve`s the area a
//! `DIMENSIONS` record declares. Both are checked here, for every sheet, against
//! `Limits::max_dense_cells` *before* `calamine` sees the file. A BIFF cell can sit at row 65,535 /
//! column 65,535, so one far-flung cell would otherwise ask for 4 G cells.

use std::collections::HashMap;
use std::io::Read;

use super::container::Limits;
use super::fmt_xlsx::{
    keep_format_code, styles_from, SheetFormats, XlsxFormats, MAX_SHEETS, STRING_OVERHEAD,
};
use super::workbook::Cancel;
use super::OfficeError;

const BOF: u16 = 0x0809;
const EOF: u16 = 0x000A;
const FILEPASS: u16 = 0x002F;
const DATEMODE: u16 = 0x0022;
const FORMAT: u16 = 0x041E;
const XF: u16 = 0x00E0;
const BOUNDSHEET: u16 = 0x0085;
const DIMENSIONS: u16 = 0x0200;
const MULRK: u16 = 0x00BD;
const SST: u16 = 0x00FC;
const CONTINUE: u16 = 0x003C;
const FORMULA: u16 = 0x0006;
const FORMULA_STRING: u16 = 0x0207;
const LABEL: u16 = 0x0204;
const RSTRING: u16 = 0x00D6;
const LABELSST: u16 = 0x00FD;
/// NUMBER, LABEL, RSTRING, BOOLERR, LABELSST, RK, FORMULA: `row, col, ixfe` first.
const SIMPLE_CELLS: [u16; 7] = [0x0203, 0x0204, 0x00D6, 0x0205, 0x00FD, 0x027E, 0x0006];
/// A cancel flag is looked at every this many records of a sheet.
const CANCEL_EVERY: u32 = 8_192;
/// Most XF records kept (Excel allows 64,000 cell XFs plus the style XFs).
const MAX_XFS: usize = 200_000;

/// Reads the workbook-level and per-cell format information of an xls file.
pub fn read(
    path: &std::path::Path,
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<XlsxFormats, OfficeError> {
    let stream = read_workbook_stream(path, limits)?;
    parse_stream_cancellable(&stream, limits, cancel)
}

/// The `Workbook` / `Book` stream, read through a cap on the expanded size.
fn read_workbook_stream(path: &std::path::Path, limits: &Limits) -> Result<Vec<u8>, OfficeError> {
    let mut cf =
        cfb::open(path).map_err(|e| OfficeError::Corrupt(format!("compound file: {e}")))?;
    let name = ["/Workbook", "/Book", "/WORKBOOK", "/BOOK"]
        .into_iter()
        .find(|n| cf.exists(n))
        .ok_or_else(|| OfficeError::Corrupt("missing Workbook stream".into()))?;
    let stream = cf
        .open_stream(name)
        .map_err(|e| OfficeError::Corrupt(format!("workbook stream: {e}")))?;
    let mut out = Vec::new();
    stream
        .take(limits.max_part_bytes + 1)
        .read_to_end(&mut out)
        .map_err(|e| OfficeError::Corrupt(format!("workbook stream: {e}")))?;
    if out.len() as u64 > limits.max_part_bytes {
        return Err(OfficeError::TooLarge { what: "entry" });
    }
    Ok(out)
}

/// One BIFF record.
struct Rec<'a> {
    typ: u16,
    data: &'a [u8],
}

/// The records of `stream` in order. A stream that ends inside a record yields an error item.
struct Records<'a> {
    stream: &'a [u8],
}

impl<'a> Iterator for Records<'a> {
    type Item = Result<Rec<'a>, OfficeError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.stream.is_empty() {
            return None;
        }
        if self.stream.len() < 4 {
            self.stream = &[];
            return Some(Err(OfficeError::Corrupt("truncated xls record".into())));
        }
        let typ = u16::from_le_bytes([self.stream[0], self.stream[1]]);
        let len = usize::from(u16::from_le_bytes([self.stream[2], self.stream[3]]));
        if self.stream.len() < 4 + len {
            self.stream = &[];
            return Some(Err(OfficeError::Corrupt("truncated xls record".into())));
        }
        let data = &self.stream[4..4 + len];
        self.stream = &self.stream[4 + len..];
        Some(Ok(Rec { typ, data }))
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// `cch` characters at `at`: UTF-16LE when `wide`, else one byte per character (Latin-1, which is
/// what BIFF8's "compressed" strings are; BIFF5 code pages are read as Latin-1 too). A string cut
/// short by the end of the record keeps what is there.
fn chars_at(b: &[u8], at: usize, cch: usize, wide: bool) -> String {
    let rest = b.get(at..).unwrap_or(&[]);
    if wide {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .take(cch)
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        rest.iter().take(cch).map(|&c| char::from(c)).collect()
    }
}

/// The format code of a FORMAT record body (after the `ifmt`).
fn format_code(data: &[u8], biff8: bool) -> Option<String> {
    if biff8 {
        let cch = usize::from(u16_at(data, 2)?);
        let flags = *data.get(4)?;
        Some(chars_at(data, 5, cch, flags & 1 != 0))
    } else {
        let cch = usize::from(*data.get(2)?);
        Some(chars_at(data, 3, cch, false))
    }
}

/// A `BoundSheet8` body -> `(stream position, name)`.
fn bound_sheet(data: &[u8], biff8: bool) -> Option<(usize, String)> {
    let pos = u32_at(data, 0)? as usize;
    let cch = usize::from(*data.get(6)?);
    let name = if biff8 {
        let flags = *data.get(7)?;
        chars_at(data, 8, cch, flags & 1 != 0)
    } else {
        chars_at(data, 7, cch, false)
    };
    Some((pos, name))
}

/// A cursor over the segments of an SST (the `SST` body and each `CONTINUE` body after it).
struct SstCursor<'a> {
    parts: &'a [&'a [u8]],
    seg: usize,
    pos: usize,
}

impl<'a> SstCursor<'a> {
    /// Moves to the next byte, across empty segments; `false` at the end.
    fn more(&mut self) -> bool {
        while self.seg < self.parts.len() && self.pos >= self.parts[self.seg].len() {
            self.seg += 1;
            self.pos = 0;
        }
        self.seg < self.parts.len()
    }

    fn byte(&mut self) -> Option<u8> {
        if !self.more() {
            return None;
        }
        let b = self.parts[self.seg][self.pos];
        self.pos += 1;
        Some(b)
    }

    fn avail(&self) -> usize {
        self.parts.get(self.seg).map_or(0, |p| p.len() - self.pos)
    }

    /// Skips `n` bytes (across segments).
    fn skip(&mut self, mut n: u64) -> Option<()> {
        while n > 0 {
            if !self.more() {
                return None;
            }
            let k = (self.avail() as u64).min(n);
            self.pos += k as usize;
            n -= k;
        }
        Some(())
    }
}

/// The length of every string of an SST in bytes (a wide character counts 2) and the budget cost
/// of the whole table. Mirrors how `calamine` reads it: a header of 8 bytes, then strings
/// (`cch: u16`, flags, optional run count / extension size, the characters, the runs, the
/// extension); where a string's characters continue in a `CONTINUE` record that record starts
/// with a fresh flags byte (its own 8/16-bit choice). A malformed table is an error, as it is for
/// `calamine`.
pub(crate) fn sst_lengths(
    parts: &[&[u8]],
    limits: &Limits,
) -> Result<(Vec<u32>, u64), OfficeError> {
    let bad = || OfficeError::Corrupt("malformed xls SST".into());
    let mut cur = SstCursor {
        parts,
        seg: 0,
        pos: 0,
    };
    cur.skip(8).ok_or_else(bad)?;
    let mut lens = Vec::new();
    let mut total: u64 = 0;
    while cur.more() {
        let cch = u64::from(u16::from_le_bytes([
            cur.byte().ok_or_else(bad)?,
            cur.byte().ok_or_else(bad)?,
        ]));
        let flags = cur.byte().ok_or_else(bad)?;
        let crun = if flags & 0x08 != 0 {
            u64::from(u16::from_le_bytes([
                cur.byte().ok_or_else(bad)?,
                cur.byte().ok_or_else(bad)?,
            ]))
        } else {
            0
        };
        let cb_ext = if flags & 0x04 != 0 {
            let mut b = [0u8; 4];
            for x in &mut b {
                *x = cur.byte().ok_or_else(bad)?;
            }
            u64::from(u32::from_le_bytes(b))
        } else {
            0
        };
        let mut wide = flags & 0x01 != 0;
        let mut need = cch;
        let mut bytes: u64 = 0;
        while need > 0 {
            if !cur.more() {
                return Err(bad());
            }
            let width = if wide { 2 } else { 1 };
            let here = ((cur.avail() / width) as u64).min(need);
            if here == 0 {
                // One stray byte of a 16-bit character at the end of a segment.
                cur.skip(cur.avail() as u64).ok_or_else(bad)?;
                continue;
            }
            cur.pos += here as usize * width;
            need -= here;
            bytes += here * width as u64;
            if need > 0 {
                // The rest continues in the next record, after its flags byte.
                cur.skip(cur.avail() as u64).ok_or_else(bad)?;
                let f = cur.byte().ok_or_else(bad)?;
                wide = f & 0x01 != 0;
            }
        }
        cur.skip(4 * crun + cb_ext).ok_or_else(bad)?;
        lens.push(u32::try_from(bytes).unwrap_or(u32::MAX));
        total = total.saturating_add(bytes + STRING_OVERHEAD);
        if total > limits.max_text_bytes {
            return Err(OfficeError::TooLarge { what: "text" });
        }
    }
    Ok((lens, total))
}

#[cfg(test)]
pub(crate) fn parse_stream(stream: &[u8], limits: &Limits) -> Result<XlsxFormats, OfficeError> {
    parse_stream_cancellable(stream, limits, None)
}

pub(crate) fn parse_stream_cancellable(
    stream: &[u8],
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<XlsxFormats, OfficeError> {
    let mut date1904 = false;
    let mut biff8 = true;
    let mut custom: HashMap<u32, String> = HashMap::new();
    let mut xf_ids: Vec<u32> = Vec::new();
    let mut sheets: Vec<(usize, String)> = Vec::new();
    // The SST body and the CONTINUE bodies after it.
    let mut sst_parts: Vec<&[u8]> = Vec::new();
    let mut in_sst = false;

    for rec in (Records { stream }) {
        let r = rec?;
        if in_sst {
            if r.typ == CONTINUE {
                sst_parts.push(r.data);
                continue;
            }
            in_sst = false;
        }
        match r.typ {
            SST => {
                sst_parts.clear();
                sst_parts.push(r.data);
                in_sst = true;
            }
            BOF => biff8 = u16_at(r.data, 0) == Some(0x0600),
            FILEPASS if u16_at(r.data, 0).is_some_and(|t| t != 0) => {
                return Err(OfficeError::Encrypted)
            }
            DATEMODE => date1904 = u16_at(r.data, 0) == Some(1),
            FORMAT => {
                if let (Some(ifmt), Some(code)) = (u16_at(r.data, 0), format_code(r.data, biff8)) {
                    // Longer than Excel allows: not a format Excel wrote, shown as General.
                    if keep_format_code(&code) {
                        custom.insert(u32::from(ifmt), code);
                    }
                }
            }
            XF if xf_ids.len() < MAX_XFS => {
                xf_ids.push(u32::from(u16_at(r.data, 2).unwrap_or(0)));
            }
            BOUNDSHEET => {
                if let Some(s) = bound_sheet(r.data, biff8) {
                    // A sheet is at least a `BOF` and an `EOF` (16 bytes), so the stream size alone
                    // allows millions, and `calamine` parses every one before it can be cancelled.
                    if sheets.len() >= MAX_SHEETS {
                        return Err(OfficeError::TooLarge { what: "sheets" });
                    }
                    sheets.push(s);
                }
            }
            EOF => break,
            _ => {}
        }
    }

    let (sst_lens, mut text_total) = if sst_parts.is_empty() {
        (Vec::new(), 0)
    } else {
        sst_lengths(&sst_parts, limits)?
    };
    let styles = styles_from(&xf_ids, &custom);
    let mut out = XlsxFormats {
        date1904,
        formats: styles.formats,
        sheets: HashMap::new(),
    };
    let mut area_total = 0u64;
    // Each sheet is read from its position to its first EOF, and `calamine` does the same for
    // every sheet. Substreams that overlap (BOUNDSHEETs pointing at one body, or into the middle
    // of another sheet) would make the work the number of sheets times the length of the body,
    // so the sheets are taken in stream order and each must start where the previous one ended:
    // the total work is then at most the length of the stream. No real file does otherwise.
    sheets.sort_by_key(|&(pos, _)| pos);
    let mut prev_end = 0usize;
    for (pos, name) in sheets {
        if pos < prev_end {
            return Err(OfficeError::Corrupt(
                "xls sheets share the same records".into(),
            ));
        }
        let sub = stream
            .get(pos..)
            .ok_or_else(|| OfficeError::Corrupt("sheet position past the stream".into()))?;
        let (sf, used) = parse_sheet_span(sub, &styles.xf_to_format, &sst_lens, limits, cancel)?;
        prev_end = pos + used;
        text_total = text_total.saturating_add(sf.text_bytes);
        if text_total > limits.max_text_bytes {
            return Err(OfficeError::TooLarge { what: "text" });
        }
        // `calamine` builds every sheet's matrix while opening, hidden ones included.
        if sf.dense_cost() > limits.max_dense_cells {
            return Err(OfficeError::TooLarge { what: "sheet area" });
        }
        // And all the sheets together: every matrix stays in memory once the file is open.
        area_total = area_total.saturating_add(sf.bbox_area());
        SheetFormats::check_total_area(area_total, limits)?;
        out.sheets.insert(name, sf);
    }
    Ok(out)
}

/// The area a DIMENSIONS record declares (what `calamine` `reserve`s).
fn declared_area(data: &[u8]) -> Result<u64, OfficeError> {
    let bad = || OfficeError::Corrupt("xls DIMENSIONS record has an unexpected length".into());
    let (rf, rl, mut cf, cl) = match data.len() {
        10 => (
            u32::from(u16_at(data, 0).ok_or_else(bad)?),
            u32::from(u16_at(data, 2).ok_or_else(bad)?),
            u32::from(u16_at(data, 4).ok_or_else(bad)?),
            u32::from(u16_at(data, 6).ok_or_else(bad)?),
        ),
        14 => (
            u32_at(data, 0).ok_or_else(bad)?,
            u32_at(data, 4).ok_or_else(bad)?,
            u32::from(u16_at(data, 8).ok_or_else(bad)?),
            u32::from(u16_at(data, 10).ok_or_else(bad)?),
        ),
        _ => return Err(bad()),
    };
    if 0xFF < cf || cl < cf {
        cf = 0;
    }
    if rl >= 1 && cl >= 1 {
        Ok(u64::from(rl.wrapping_sub(rf)) * u64::from(cl - cf))
    } else {
        Ok(1)
    }
}

/// The bytes of a `LABEL` / `RSTRING` / formula `STRING` text of `cch` characters at `at`: what
/// the record can hold at most (a declared length that runs past the record is a lie the reader
/// stops at), 2 bytes per character when the string is 16-bit.
fn string_cost(data: &[u8], cch_at: usize, flags_at: usize, biff8: bool) -> u64 {
    let cch = usize::from(u16_at(data, cch_at).unwrap_or(0));
    let wide = biff8 && data.get(flags_at).is_some_and(|f| f & 1 != 0);
    let width = if wide { 2 } else { 1 };
    let room = data.len().saturating_sub(flags_at + 1) / width;
    (cch.min(room) * width) as u64 + STRING_OVERHEAD
}

/// The format information of one sheet substream, and how many bytes of `stream` its records
/// took (up to and including its first `EOF`).
fn parse_sheet_span(
    stream: &[u8],
    xf_to_format: &[u16],
    sst_lens: &[u32],
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<(SheetFormats, usize), OfficeError> {
    let mut out = SheetFormats::default();
    // Where `calamine` puts a formula's `STRING` result: the last FORMULA's cell, A1 before any.
    let mut formula_pos = (0u32, 0u32);
    let mut biff8 = true;
    let fmt_of = |ixfe: u16| xf_to_format.get(usize::from(ixfe)).copied().unwrap_or(0);
    let short = || OfficeError::Corrupt("xls cell record too short".into());
    let mut records = Records { stream };
    let mut seen = 0u32;
    let mut ended = false;
    for rec in records.by_ref() {
        let r = rec?;
        seen += 1;
        if seen.is_multiple_of(CANCEL_EVERY) && cancel.is_some_and(Cancel::is_cancelled) {
            return Err(OfficeError::Corrupt("cancelled".into()));
        }
        match r.typ {
            BOF => biff8 = u16_at(r.data, 0) == Some(0x0600),
            EOF => {
                ended = true;
                break;
            }
            DIMENSIONS => {
                if declared_area(r.data)? > limits.max_dense_cells {
                    return Err(OfficeError::TooLarge { what: "sheet area" });
                }
            }
            t if SIMPLE_CELLS.contains(&t) => {
                let (Some(row), Some(col), Some(ixfe)) =
                    (u16_at(r.data, 0), u16_at(r.data, 2), u16_at(r.data, 4))
                else {
                    return Err(short());
                };
                if r.typ == FORMULA {
                    formula_pos = (u32::from(row), u32::from(col));
                }
                let cost = match r.typ {
                    LABELSST => {
                        let i = u32_at(r.data, 6).map_or(0, |i| i as usize);
                        sst_lens.get(i).map_or(0, |&l| u64::from(l)) + STRING_OVERHEAD
                    }
                    LABEL | RSTRING => string_cost(r.data, 6, 8, biff8),
                    _ => 0,
                };
                out.add_text(cost, limits)?;
                out.note_value(u32::from(row), u32::from(col), fmt_of(ixfe), limits)?;
            }
            FORMULA_STRING => {
                out.add_text(string_cost(r.data, 0, 2, biff8), limits)?;
                out.note_value(formula_pos.0, formula_pos.1, 0, limits)?;
            }
            MULRK => {
                let (Some(row), Some(first)) = (u16_at(r.data, 0), u16_at(r.data, 2)) else {
                    return Err(short());
                };
                if r.data.len() < 6 {
                    return Err(short());
                }
                let last = u16_at(r.data, r.data.len() - 2).ok_or_else(short)?;
                let n = (r.data.len() - 6) / 6;
                // The declared column span must match the body, as `calamine` insists.
                if last < first || r.data.len() != 6 + 6 * (usize::from(last - first) + 1) {
                    return Err(OfficeError::Corrupt("xls MULRK length mismatch".into()));
                }
                for i in 0..n {
                    let ixfe = u16_at(r.data, 4 + 6 * i).ok_or_else(short)?;
                    out.note_value(
                        u32::from(row),
                        u32::from(first) + i as u32,
                        fmt_of(ixfe),
                        limits,
                    )?;
                }
            }
            _ => {}
        }
    }
    out.cells.sort_unstable_by_key(|&(r, c, _)| (r, c));
    let used = if ended {
        stream.len() - records.stream.len()
    } else {
        stream.len()
    };
    Ok((out, used))
}
