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
//! Each sheet substream, up to its own `EOF` (nested `BOF`..`EOF` substreams such as embedded
//! charts are skipped, as `calamine` 0.37 does): `NUMBER` 0x0203,
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
use std::io::{Read, Write};

use super::container::Limits;
use super::fmt_xls_formula::{self as formula, Env};
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
const NAME: u16 = 0x0018;
const EXTERNSHEET: u16 = 0x0017;
/// One formula whose text would get longer than this while it is built (it is nearly always under
/// a few KB: a record holds at most 8 KB of tokens) is not given to `calamine`.
pub(super) const MAX_FORMULA_PEAK: u64 = 4 << 20;
/// One formula that would need more byte moves than this (a few tens of milliseconds) likewise.
pub(super) const MAX_FORMULA_WORK: u64 = 1 << 27;
/// NUMBER, LABEL, RSTRING, BOOLERR, LABELSST, RK, FORMULA: `row, col, ixfe` first.
const SIMPLE_CELLS: [u16; 7] = [0x0203, 0x0204, 0x00D6, 0x0205, 0x00FD, 0x027E, 0x0006];
/// A cancel flag is looked at every this many records of a sheet.
const CANCEL_EVERY: u32 = 8_192;
/// Most XF records kept (Excel allows 64,000 cell XFs plus the style XFs).
const MAX_XFS: usize = 200_000;

/// Whether a `BOF` body makes `calamine` read the stream as BIFF8: it maps the version word like
/// this (`0x0200`, `0x0002`, `0x0007`, `0x0300`, `0x0400` and `0x0500` are older; version `0` is BIFF5 for a
/// BIFF5 for the document type `0x1000`, else BIFF8; anything unknown is BIFF8).
fn bof_is_biff8(data: &[u8]) -> bool {
    let version = u16_at(data, 0).unwrap_or(0x0600);
    let dt = u16_at(data, 2).unwrap_or(0);
    match version {
        0x0200 | 0x0002 | 0x0007 | 0x0300 | 0x0400 | 0x0500 => false,
        0 => dt != 0x1000,
        _ => true,
    }
}

#[cfg(test)]
pub(super) fn bof_is_biff8_for_test(data: &[u8]) -> bool {
    bof_is_biff8(data)
}

/// Reads the workbook-level and per-cell format information of an xls file.
#[cfg(test)]
pub fn read(
    path: &std::path::Path,
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<XlsxFormats, OfficeError> {
    read_scanned(path, limits, cancel).map(|(fm, _)| fm)
}

/// Like [`read`], and also hands back the container `calamine` must be given: a fresh compound
/// file holding the scanned bytes and nothing else. `calamine` has its own, different rules for
/// picking a stream out of a compound file (flat directory scan, mini-stream walks, BOM-aware
/// names, zero-padded short sectors), so instead of imitating them every one of them is made
/// irrelevant: it can only ever read the bytes this pass has just checked.
pub fn read_scanned(
    path: &std::path::Path,
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<(XlsxFormats, std::io::Cursor<Vec<u8>>), OfficeError> {
    let mut stream = read_workbook_stream(path, limits)?;
    let (fm, blank) = scan_stream(&stream, limits, cancel)?;
    apply_blanks(&mut stream, &blank);
    // Memory, for a stream of N bytes (N <= `max_part_bytes`, 256 MiB, as before this check
    // existed). While scanning: the stream (N, allocated at its real size) plus the container
    // built from it (about 1.01 N, pre-sized so the writer never doubles it). The stream is
    // dropped when this function returns, so `calamine` starts with only the container alive and
    // then holds about 3 N at its peak: the container (its reader), its own copy of the sectors it
    // touches (N, up to 2 N of capacity while that vector grows) and the stream it extracts (N, up
    // to 2 N likewise). That is the same order as before this pass handed `calamine` a rebuilt
    // container (it copied the file's sectors and extracted the stream the same way), plus the
    // container itself; the dense cells (`max_dense_cells`, ~800 MB) come on top of both.
    let container = clean_container(&stream)?;
    Ok((fm, container))
}

/// Rewrites what [`scan_stream`] marked: a formula `calamine` would spend too much on (see
/// `fmt_xls_formula`) becomes a single `PtgMissArg` (`cce = 1`, token `0x16`), for which
/// `calamine` prints nothing (an empty `Ok`; `cce = 0` would make it build an error message of
/// ~80 bytes per formula instead, outside every budget); the cell keeps its cached value (other
/// bytes of the record). A defined name's `cce` is zeroed: its formula is then "empty".
pub(crate) fn apply_blanks(stream: &mut [u8], blank: &[Blank]) {
    for b in blank {
        match *b {
            Blank::Formula(at) => {
                stream[at] = 1;
                stream[at + 1] = 0;
                stream[at + 2] = 0x16;
            }
            Blank::Name(at) => {
                stream[at] = 0;
                stream[at + 1] = 0;
            }
        }
    }
}

/// A new compound file (version 3: 512-byte sectors, what Excel writes) with a single root-level `Workbook` stream holding `stream`.
pub fn clean_container(stream: &[u8]) -> Result<std::io::Cursor<Vec<u8>>, OfficeError> {
    let err = |e: std::io::Error| OfficeError::Corrupt(format!("compound file: {e}"));
    // The container is the stream plus its FAT (1/128), the directory and the header: reserve
    // that up front so the writer never has to double (and briefly hold 2x) the buffer.
    let reserve = stream.len() + stream.len() / 64 + 64 * 1024;
    let mut cf = cfb::CompoundFile::create_with_version(
        cfb::Version::V3,
        std::io::Cursor::new(Vec::with_capacity(reserve)),
    )
    .map_err(err)?;
    cf.create_stream("/Workbook")
        .map_err(err)?
        .write_all(stream)
        .map_err(err)?;
    cf.flush().map_err(err)?;
    // `calamine` reads from the current position, not from the start.
    let mut c = cf.into_inner();
    // Give back any slack (the reserve above is an estimate): the container lives for as long as
    // `calamine` reads from it.
    c.get_mut().shrink_to_fit();
    c.set_position(0);
    Ok(c)
}

/// The root-level `Workbook` / `Book` stream, read through a cap on the expanded size. Which
/// stream is picked no longer has to match `calamine`'s choice: `calamine` is given only a copy
/// of these very bytes (see [`read_scanned`]).
pub fn read_workbook_stream(
    path: &std::path::Path,
    limits: &Limits,
) -> Result<Vec<u8>, OfficeError> {
    let mut cf = super::container::open_cfb(path)?;
    let name = ["/Workbook", "/Book", "/WORKBOOK", "/BOOK"]
        .into_iter()
        .find(|n| cf.exists(n))
        .ok_or_else(|| OfficeError::Corrupt("missing Workbook stream".into()))?;
    let stream = cf
        .open_stream(name)
        .map_err(|e| OfficeError::Corrupt(format!("workbook stream: {e}")))?;
    let cap = limits.max_part_bytes;
    // Sized by the directory's length, but never beyond the file itself (a forged length cannot
    // make a small file reserve a large buffer): the buffer is then exact and never doubles.
    let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut out = Vec::with_capacity(stream.len().min(file_len).min(cap) as usize + 1);
    stream
        .take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|e| OfficeError::Corrupt(format!("workbook stream: {e}")))?;
    if out.len() as u64 > cap {
        return Err(OfficeError::TooLarge { what: "entry" });
    }
    out.shrink_to_fit();
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

pub(super) fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub(super) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
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

/// The size of a sheet's name as `calamine` will hold it (worst case, see
/// [`formula::text_size`]), from a `BoundSheet8` body.
fn sheet_name_size(data: &[u8], biff8: bool) -> u64 {
    let cch = usize::from(data.get(6).copied().unwrap_or(0));
    if biff8 {
        let wide = data.get(7).is_some_and(|f| f & 1 != 0);
        formula::text_size(data.get(8..).unwrap_or(&[]), cch, wide)
    } else {
        formula::text_size(data.get(7..).unwrap_or(&[]), cch, false)
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

#[cfg(test)]
pub(crate) fn parse_stream_cancellable(
    stream: &[u8],
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<XlsxFormats, OfficeError> {
    scan_stream(stream, limits, cancel).map(|(fm, _)| fm)
}

/// A place in the stream to rewrite (see [`apply_blanks`]): the offset of the `cce` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blank {
    /// A `FORMULA` record's `cce` (it is followed by at least one byte of the record).
    Formula(usize),
    /// A `NAME` record's `cce`.
    Name(usize),
}

/// The formulas `calamine` is not to print (see `fmt_xls_formula`) and the budgets they are
/// charged to.
pub(crate) struct FormulaGuard {
    /// Whether the workbook globals' last `BOF` made `calamine` read BIFF8; it uses that one
    /// for every sheet too.
    biff8: bool,
    env: Env,
    text_left: u64,
    work_left: u64,
    /// Where the sheet being scanned starts in the stream.
    base: usize,
    pub(crate) blank: Vec<Blank>,
    /// What each admitted or dropped formula was charged, with its cell (tests only).
    #[cfg(test)]
    pub(crate) trace: FormulaTrace,
    #[cfg(test)]
    pub(crate) pos: (u32, u32),
}

impl FormulaGuard {
    /// Charges a `FORMULA` record's tokens (`body` = the record from byte 20, at `cce_at` in the
    /// current sheet) to the budgets, or marks the formula for removal. A formula that is over
    /// budget but has no byte after its `cce` for the replacement token cannot be made empty (it
    /// would still print an error message): the file is refused.
    fn admit(&mut self, cce_at: usize, body: &[u8]) -> Result<(), OfficeError> {
        let c = formula::formula_cost(body, &self.env);
        #[cfg(test)]
        self.trace.push((self.pos, c));
        self.charge(cce_at, body.len(), c)
    }

    /// The budget decisions of [`Self::admit`] for a cost already worked out.
    fn charge(
        &mut self,
        cce_at: usize,
        body_len: usize,
        c: formula::Cost,
    ) -> Result<(), OfficeError> {
        if c.peak > MAX_FORMULA_PEAK
            || c.work > MAX_FORMULA_WORK
            || c.text > self.text_left
            || c.work > self.work_left
        {
            if body_len < 3 {
                return Err(OfficeError::TooLarge {
                    what: "formula text",
                });
            }
            self.blank.push(Blank::Formula(self.base + cce_at));
        } else {
            self.text_left -= c.text;
            self.work_left -= c.work;
        }
        Ok(())
    }
}

/// Whether a formula of this cost, charged to budgets with this much left, is dropped (tests:
/// the boundaries of every comparison).
#[cfg(test)]
pub(super) fn is_dropped_when(c: formula::Cost, text_left: u64, work_left: u64) -> bool {
    let mut g = FormulaGuard {
        biff8: true,
        env: Env::default(),
        text_left,
        work_left,
        base: 0,
        blank: Vec::new(),
        trace: Vec::new(),
        pos: (0, 0),
    };
    g.charge(0, 3, c).unwrap();
    !g.blank.is_empty()
}

/// The most records all the sheets together may visit: one pass over a stream is `len / 4`
/// records at most, and a sheet that never closes lets the next ones read the same records again.
pub(super) fn visit_budget(stream_len: usize) -> u64 {
    (stream_len as u64 / 4) * 2 + 16
}

/// Scans the stream like [`parse_stream_cancellable`] and also says which formulas to drop.
pub(crate) fn scan_stream(
    stream: &[u8],
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<(XlsxFormats, Vec<Blank>), OfficeError> {
    scan_guarded(stream, limits, cancel).map(|(fm, g)| (fm, g.blank))
}

/// What each formula was charged, with its cell, in record order (tests).
#[cfg(test)]
pub(crate) type FormulaTrace = Vec<((u32, u32), formula::Cost)>;

/// What each formula of the stream is charged, as `(row, col, cost)` in record order (tests).
#[cfg(test)]
pub(crate) fn formula_costs(stream: &[u8], limits: &Limits) -> Result<FormulaTrace, OfficeError> {
    scan_guarded(stream, limits, None).map(|(_, g)| g.trace)
}

fn scan_guarded(
    stream: &[u8],
    limits: &Limits,
    cancel: Option<&Cancel>,
) -> Result<(XlsxFormats, FormulaGuard), OfficeError> {
    let mut date1904 = false;
    let mut biff8 = true;
    let mut env = Env::default();
    // Defined names whose formula starts with a sheet reference: where their `cce` is, and the
    // XTI index (`calamine` prints the sheet's name in front of the name's formula).
    let mut name_prefix: Vec<(usize, usize)> = Vec::new();
    let offset_of = |data: &[u8]| data.as_ptr() as usize - stream.as_ptr() as usize;
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
            BOF => biff8 = bof_is_biff8(r.data),
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
                    env.sheets.push(sheet_name_size(r.data, biff8));
                    sheets.push(s);
                }
            }
            NAME => {
                let cch = usize::from(r.data.get(3).copied().unwrap_or(0));
                env.names.push(if biff8 {
                    r.data.get(14).map_or(0, |&fl| {
                        formula::text_size(r.data.get(15..).unwrap_or(&[]), cch, fl & 1 != 0)
                    })
                } else {
                    formula::text_size(r.data.get(14..).unwrap_or(&[]), cch, false)
                });
                if let Some(cce) = u16_at(r.data, 4).map(usize::from) {
                    if cce <= r.data.len() {
                        let rgce = &r.data[r.data.len() - cce..];
                        if let Some(i) = formula::name_sheet_ixti(rgce, biff8) {
                            name_prefix.push((offset_of(r.data) + 4, usize::from(i)));
                        }
                    }
                }
            }
            EXTERNSHEET if biff8 => {
                let n = usize::from(u16_at(r.data, 0).unwrap_or(0));
                if let Some(rest) = r.data.get(2..) {
                    env.xti_first.extend(
                        rest.as_chunks::<6>()
                            .0
                            .iter()
                            .take(n)
                            .map(|x| i16::from_le_bytes([x[2], x[3]])),
                    );
                }
            }
            EOF => break,
            _ => {}
        }
    }

    // Before BIFF8 a formula names its sheets by index: `calamine` makes the XTI table the identity.
    env.pre_biff8 = !biff8;
    // (`EXTERNSHEET` is not read before BIFF8, so the table is empty here.)
    if !biff8 {
        env.xti_first = (0..env.sheets.len())
            .map_while(|i| i16::try_from(i).ok())
            .collect();
    }
    let mut guard = FormulaGuard {
        biff8,
        text_left: limits.max_formula_text_bytes,
        work_left: limits.max_formula_work,
        base: 0,
        blank: Vec::new(),
        #[cfg(test)]
        trace: Vec::new(),
        #[cfg(test)]
        pos: (0, 0),
        env,
    };
    // The prefixes of defined names share a quarter of the formula text budget.
    let mut name_budget = limits.max_formula_text_bytes / 4;
    for (cce_at, ixti) in name_prefix {
        let cost = guard.env.via_xti(ixti) + 40;
        if cost > name_budget {
            guard.blank.push(Blank::Name(cce_at));
        } else {
            name_budget -= cost;
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
    // Each sheet is read from its position to its own EOF, and `calamine` does the same for
    // every sheet. Substreams that overlap (BOUNDSHEETs pointing at one body, or into the middle
    // of another sheet) would make the work the number of sheets times the length of the body,
    // so the sheets are taken in stream order and each must start where the previous *closed*
    // sheet ended: the total work is then at most the length of the stream. No real file does
    // otherwise.
    // A sheet that never closes (a nested substream without its EOF) runs to the end of the
    // stream, and `calamine` still opens the sheets after it, each from its own BOUNDSHEET
    // position; so such a sheet does not move `prev_end`, and the records visited over all the
    // sheets are capped instead (one pass over the stream is `len / 4` records at most).
    sheets.sort_by_key(|&(pos, _)| pos);
    let mut prev_end = 0usize;
    let mut visited = 0u64;
    let visit_budget = visit_budget(stream.len());
    for (pos, name) in sheets {
        if pos < prev_end {
            return Err(OfficeError::Corrupt(
                "xls sheets share the same records".into(),
            ));
        }
        let sub = stream
            .get(pos..)
            .ok_or_else(|| OfficeError::Corrupt("sheet position past the stream".into()))?;
        guard.base = pos;
        let (sf, used, ended, seen) = parse_sheet_span(
            sub,
            &styles.xf_to_format,
            &sst_lens,
            limits,
            cancel,
            &mut guard,
        )?;
        if ended {
            prev_end = pos + used;
        }
        visited += u64::from(seen);
        if visited > visit_budget {
            return Err(OfficeError::Corrupt(
                "xls sheets share the same records".into(),
            ));
        }
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
    Ok((out, guard))
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
        14 | 16 => (
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
/// took (up to and including its own depth-0 `EOF`; nested substreams are skipped), whether it
/// closed with that `EOF`, and how many records were visited.
fn parse_sheet_span(
    stream: &[u8],
    xf_to_format: &[u16],
    sst_lens: &[u32],
    limits: &Limits,
    cancel: Option<&Cancel>,
    guard: &mut FormulaGuard,
) -> Result<(SheetFormats, usize, bool, u32), OfficeError> {
    let mut out = SheetFormats::default();
    // Where `calamine` puts a formula's `STRING` result: the last FORMULA's cell, A1 before any.
    let mut formula_pos = (0u32, 0u32);
    // `calamine` reads every sheet with the version of the workbook globals' `BOF`, not its own.
    let biff8 = guard.biff8;
    let fmt_of = |ixfe: u16| xf_to_format.get(usize::from(ixfe)).copied().unwrap_or(0);
    let short = || OfficeError::Corrupt("xls cell record too short".into());
    let mut records = Records { stream };
    let mut seen = 0u32;
    let mut ended = false;
    // Walk the sheet exactly as `calamine` 0.37 does: the first BOF is the sheet's own, every
    // later BOF opens a nested substream (an embedded chart, a macro sheet) whose records are
    // skipped up to its matching EOF, and the sheet ends at its own depth-0 EOF.
    let mut seen_sheet_bof = false;
    let mut depth = 0usize;
    // The greatest cell position seen: a General record at or before it may replace an earlier
    // record's format (the last record wins, as in `calamine`'s matrix).
    let mut last_pos: Option<(u32, u16)> = None;
    for rec in records.by_ref() {
        let r = rec?;
        seen += 1;
        if seen.is_multiple_of(CANCEL_EVERY) && cancel.is_some_and(Cancel::is_cancelled) {
            return Err(OfficeError::Corrupt("cancelled".into()));
        }
        if depth > 0 {
            match r.typ {
                BOF => depth += 1,
                EOF => depth -= 1,
                _ => {}
            }
            continue;
        }
        match r.typ {
            BOF if seen_sheet_bof => depth += 1,
            BOF => seen_sheet_bof = true,
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
                    if r.data.len() >= 20 {
                        let cce_at = r.data.as_ptr() as usize - stream.as_ptr() as usize + 20;
                        #[cfg(test)]
                        {
                            guard.pos = formula_pos;
                        }
                        guard.admit(cce_at, &r.data[20..])?;
                    }
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
                out.keep_last_format(
                    &mut last_pos,
                    u32::from(row),
                    u32::from(col),
                    fmt_of(ixfe),
                    limits,
                );
            }
            FORMULA_STRING => {
                out.add_text(string_cost(r.data, 0, 2, biff8), limits)?;
                // The text result of the formula before it: the FORMULA's own format stays.
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
                    let col = u32::from(first) + i as u32;
                    out.note_value(u32::from(row), col, fmt_of(ixfe), limits)?;
                    out.keep_last_format(&mut last_pos, u32::from(row), col, fmt_of(ixfe), limits);
                }
            }
            _ => {}
        }
    }
    out.cells.sort_by_key(|&(r, c, _)| (r, c));
    let used = if ended {
        stream.len() - records.stream.len()
    } else {
        stream.len()
    };
    Ok((out, used, ended, seen))
}
