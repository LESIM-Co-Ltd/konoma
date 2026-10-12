//! Format pass for xlsb (binary OOXML): which number format each cell has.
//!
//! Same job as [`super::fmt_xlsx`], over MS-XLSB records instead of XML:
//! <https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-xlsb/>
//!
//! 1. `xl/workbook.bin`: `BrtWbProp` (0x0099, bit 0 = 1904 date system) and the `BrtBundleSh`
//!    records (0x009C) giving sheet name -> relationship id, up to `BrtEndBundleShs` (0x0090).
//! 2. `xl/_rels/workbook.bin.rels`: relationship id -> part path (shared with the xlsx pass).
//! 3. `xl/styles.bin`: `BrtFmt` (0x002C: `ifmt`, format code as `XLWideString`) and, between
//!    `BrtBeginCellXFs` (0x0269) and `BrtEndCellXFs` (0x026A), the `BrtXF` records (0x002F: the
//!    `iFmt` at bytes 2..4). `cellStyleXFs` also holds `BrtXF`s and is not counted.
//! 4. each sheet part, from `BrtBeginSheetData` (0x0091): `BrtRowHdr` (0x0000) gives the row; the
//!    value-bearing cell records (0x0002..=0x000A: RK, error, bool, real, string, shared string and
//!    the four formula kinds, and 0x000B, a formula that evaluates to an error: `calamine` puts
//!    those in the range of formulas) start with `col: u32` and `iStyleRef: u24` — the cell's xf.
//! 5. `xl/sharedStrings.bin`: `BrtSSTItem` (0x0013) lengths only, for the text budget (a shared
//!    string used by many cells is copied into each).
//!
//! A record is `id` (1-2 bytes, 7 bits each), `size` (1-4 bytes, 7 bits each) and the body. Bodies
//! are never trusted: only the first [`MAX_BODY`] bytes are read (the rest is skipped), so a
//! forged size cannot make this pass allocate. A stream that ends *inside* a record is corrupt; one
//! that ends between records is just the end.

use std::collections::HashMap;
use std::io::{self, BufReader, Read};

use super::container::{self, Limits};
use super::fmt_xlsx::{
    keep_format_code, parse_rels, styles_from, OoxmlPackage, SheetFormats, STRING_OVERHEAD,
};
use super::OfficeError;

/// Most bytes of one record body that are read (the longest field we need is a format code).
const MAX_BODY: usize = 64 * 1024;
/// Safety cap on the number of `cellXfs` kept (Excel allows 64,000).
const MAX_XFS: usize = 100_000;
/// `BrtRowHdr` rows above this end the sheet, as in `calamine`.
const MAX_ROW: u32 = 0x0010_0000;

const BRT_ROW_HDR: u16 = 0x0000;
const BRT_FMT: u16 = 0x002C;
const BRT_XF: u16 = 0x002F;
const BRT_BEGIN_SHEET_DATA: u16 = 0x0091;
const BRT_END_SHEET_DATA: u16 = 0x0092;
const BRT_WB_PROP: u16 = 0x0099;
const BRT_BUNDLE_SH: u16 = 0x009C;
const BRT_END_BUNDLE_SHS: u16 = 0x0090;
const BRT_BEGIN_SST: u16 = 0x009F;
const BRT_SST_ITEM: u16 = 0x0013;
/// The last cell record `calamine` reads (`BrtFmlaError`, formulas only).
const BRT_LAST_CELL: u16 = 0x000B;
/// Longest string a cell can hold (Excel: 32,767 characters); a longer declared length is a lie
/// the reader would reject, and is charged at this size.
const MAX_CELL_CHARS: u32 = 32_767;
const BRT_BEGIN_CELL_XFS: u16 = 0x0269;
const BRT_END_CELL_XFS: u16 = 0x026A;

/// Reads the workbook-level parts of an xlsb package (no sheet part is touched; the shared string
/// table is only checked — `calamine` loads all of it when it opens the file).
pub fn read_package(path: &std::path::Path, limits: &Limits) -> Result<OoxmlPackage, OfficeError> {
    let mut zip = container::open_zip(path)?;
    let cap = limits.max_part_bytes;

    let (date1904, decls) = {
        let r = container::part_reader(&mut zip, "xl/workbook.bin", cap)?
            .ok_or_else(|| OfficeError::Corrupt("missing xl/workbook.bin".into()))?;
        parse_workbook(BufReader::new(r))?
    };
    let rels = match container::part_reader(&mut zip, "xl/_rels/workbook.bin.rels", cap)? {
        Some(r) => parse_rels(BufReader::new(r))?,
        None => HashMap::new(),
    };
    let styles = match container::part_reader(&mut zip, "xl/styles.bin", cap)? {
        Some(r) => parse_styles(BufReader::new(r))?,
        None => super::fmt_xlsx::styles_from(&[], &HashMap::new()),
    };
    if let Some(r) = container::part_reader(&mut zip, "xl/sharedStrings.bin", cap)? {
        check_shared_strings(BufReader::new(r), limits)?;
    }
    let sheets = decls
        .into_iter()
        .filter_map(|(name, rid)| rels.get(&rid).map(|part| (name, part.clone())))
        .collect();
    Ok(OoxmlPackage {
        date1904,
        formats: styles.formats,
        xf_to_format: styles.xf_to_format,
        sheets,
    })
}

/// Reads the cell formats of **one** sheet part (see [`super::fmt_xlsx::read_sheet`]).
pub fn read_sheet(
    path: &std::path::Path,
    part: &str,
    xf_to_format: &[u16],
    limits: &Limits,
) -> Result<SheetFormats, OfficeError> {
    let mut zip = container::open_zip(path)?;
    let reader = container::part_reader(&mut zip, part, limits.max_part_bytes)?;
    match reader {
        Some(r) => parse_sheet(BufReader::new(r), xf_to_format, limits),
        None => Ok(SheetFormats::default()),
    }
}

/// The package and every sheet's formats in one value (tests).
#[cfg(test)]
pub fn read(
    path: &std::path::Path,
    limits: &Limits,
) -> Result<super::fmt_xlsx::XlsxFormats, OfficeError> {
    let pkg = read_package(path, limits)?;
    let mut sheets = HashMap::new();
    for (name, part) in &pkg.sheets {
        sheets.insert(
            name.clone(),
            read_sheet(path, part, &pkg.xf_to_format, limits)?,
        );
    }
    Ok(super::fmt_xlsx::XlsxFormats {
        date1904: pkg.date1904,
        formats: pkg.formats,
        sheets,
    })
}

// ---------------------------------------------------------------------------------------------
// record reader
// ---------------------------------------------------------------------------------------------

struct Records<R> {
    r: R,
}

impl<R: Read> Records<R> {
    fn new(r: R) -> Records<R> {
        Records { r }
    }

    /// One byte; `None` at a clean end of stream.
    fn byte(&mut self) -> Result<Option<u8>, OfficeError> {
        let mut b = [0u8; 1];
        loop {
            match self.r.read(&mut b) {
                Ok(0) => return Ok(None),
                Ok(_) => return Ok(Some(b[0])),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(OfficeError::Corrupt(format!("xlsb record: {e}"))),
            }
        }
    }

    fn need_byte(&mut self) -> Result<u8, OfficeError> {
        self.byte()?
            .ok_or_else(|| OfficeError::Corrupt("truncated xlsb record".into()))
    }

    /// Reads the next record: its id, with the first [`MAX_BODY`] bytes of its body in `buf`
    /// (the rest is skipped). `None` at a clean end of stream.
    fn next(&mut self, buf: &mut Vec<u8>) -> Result<Option<u16>, OfficeError> {
        let Some(b0) = self.byte()? else {
            return Ok(None);
        };
        let id = if b0 & 0x80 != 0 {
            u16::from(b0 & 0x7F) + (u16::from(self.need_byte()? & 0x7F) << 7)
        } else {
            u16::from(b0)
        };
        let mut len: usize = 0;
        for i in 0..4 {
            let b = self.need_byte()?;
            len |= usize::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                break;
            }
        }
        let keep = len.min(MAX_BODY);
        buf.clear();
        buf.resize(keep, 0);
        self.r
            .read_exact(buf)
            .map_err(|_| OfficeError::Corrupt("truncated xlsb record".into()))?;
        let skip = (len - keep) as u64;
        if skip > 0 {
            let n = io::copy(&mut (&mut self.r).take(skip), &mut io::sink())
                .map_err(|e| OfficeError::Corrupt(format!("xlsb record: {e}")))?;
            if n != skip {
                return Err(OfficeError::Corrupt("truncated xlsb record".into()));
            }
        }
        Ok(Some(id))
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// An `XLWideString` (`u32` length in characters, then UTF-16LE) at `at`; returns the text and the
/// offset after it. `None` when the record is too short for the declared length.
fn wide_str(b: &[u8], at: usize) -> Option<(String, usize)> {
    let cch = u32_at(b, at)? as usize;
    let end = (at + 4).checked_add(cch.checked_mul(2)?)?;
    let bytes = b.get(at + 4..end)?;
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    Some((String::from_utf16_lossy(&units), end))
}

// ---------------------------------------------------------------------------------------------
// workbook.bin
// ---------------------------------------------------------------------------------------------

/// `(date1904, [(sheet name, relationship id)])`.
pub(crate) fn parse_workbook<R: Read>(
    src: R,
) -> Result<(bool, Vec<(String, String)>), OfficeError> {
    let mut rd = Records::new(src);
    let mut buf = Vec::new();
    let mut date1904 = false;
    let mut sheets = Vec::new();
    while let Some(id) = rd.next(&mut buf)? {
        match id {
            BRT_WB_PROP => date1904 = buf.first().is_some_and(|b| b & 1 != 0),
            BRT_BUNDLE_SH => {
                // hsState u32, iTabID u32, strRelID XLNullableWideString, strName XLWideString.
                let Some(cch) = u32_at(&buf, 8) else { continue };
                if cch == 0xFFFF_FFFF {
                    continue;
                }
                let Some((rid, next)) = wide_str(&buf, 8) else {
                    continue;
                };
                let Some((name, _)) = wide_str(&buf, next) else {
                    continue;
                };
                if sheets.len() >= super::fmt_xlsx::MAX_SHEETS {
                    return Err(OfficeError::TooLarge { what: "sheets" });
                }
                sheets.push((name, rid));
            }
            BRT_END_BUNDLE_SHS => break,
            _ => {}
        }
    }
    Ok((date1904, sheets))
}

// ---------------------------------------------------------------------------------------------
// styles.bin
// ---------------------------------------------------------------------------------------------

pub(crate) fn parse_styles<R: Read>(src: R) -> Result<super::fmt_xlsx::Styles, OfficeError> {
    let mut rd = Records::new(src);
    let mut buf = Vec::new();
    let mut custom: HashMap<u32, String> = HashMap::new();
    let mut xf_ids: Vec<u32> = Vec::new();
    let mut in_cell_xfs = false;
    while let Some(id) = rd.next(&mut buf)? {
        match id {
            BRT_FMT => {
                if let (Some(ifmt), Some((code, _))) = (u16_at(&buf, 0), wide_str(&buf, 2)) {
                    // A record body is read up to `MAX_BODY` (64 KiB), so the code is bounded in
                    // memory already; a code longer than Excel allows is dropped (General).
                    if keep_format_code(&code) {
                        custom.insert(u32::from(ifmt), code);
                    }
                }
            }
            BRT_BEGIN_CELL_XFS => in_cell_xfs = true,
            BRT_END_CELL_XFS => in_cell_xfs = false,
            BRT_XF if in_cell_xfs && xf_ids.len() < MAX_XFS => {
                xf_ids.push(u32::from(u16_at(&buf, 2).unwrap_or(0)));
            }
            _ => {}
        }
    }
    Ok(styles_from(&xf_ids, &custom))
}

// ---------------------------------------------------------------------------------------------
// sharedStrings.bin
// ---------------------------------------------------------------------------------------------

/// Budget bytes of a wide string of `cch` characters (UTF-16 in the file; the loader keeps UTF-8,
/// so this is between the character count and 3x it).
fn wide_cost(cch: u32) -> u64 {
    u64::from(cch.min(MAX_CELL_CHARS)) * 2
}

/// Refuses a shared-string table that `calamine` could not hold (it copies every `BrtSSTItem` into
/// a `String` when it opens the file). Counts each string plus [`STRING_OVERHEAD`] against
/// `Limits::max_text_bytes`; returns the cost of the table.
pub(crate) fn check_shared_strings<R: Read>(src: R, limits: &Limits) -> Result<u64, OfficeError> {
    let too_large = || OfficeError::TooLarge { what: "text" };
    let mut rd = Records::new(src);
    let mut buf = Vec::new();
    let mut total: u64 = 0;
    while let Some(id) = rd.next(&mut buf)? {
        match id {
            BRT_BEGIN_SST => {
                // cstTotal u32, cstUnique u32.
                let unique = u64::from(u32_at(&buf, 4).unwrap_or(0));
                if unique.saturating_mul(STRING_OVERHEAD) > limits.max_text_bytes {
                    return Err(too_large());
                }
            }
            BRT_SST_ITEM => {
                // A flags byte, then an XLWideString.
                let cch = u32_at(&buf, 1).unwrap_or(0);
                total = total.saturating_add(wide_cost(cch) + STRING_OVERHEAD);
                if total > limits.max_text_bytes {
                    return Err(too_large());
                }
            }
            _ => {}
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------------------------
// sheet part
// ---------------------------------------------------------------------------------------------

/// Streams one sheet part and records the number format of every value-bearing cell. Stops (and
/// sets [`SheetFormats::truncated`]) at the first row past `Limits::max_rows` and once the sheet
/// has `Limits::max_sheet_cells` cells; the value reader stops at the same point.
pub(crate) fn parse_sheet<R: Read>(
    src: R,
    xf_to_format: &[u16],
    limits: &Limits,
) -> Result<SheetFormats, OfficeError> {
    let mut rd = Records::new(src);
    let mut buf = Vec::new();
    let mut out = SheetFormats::default();
    let mut in_data = false;
    let mut row: u32 = 0;
    let max_row = u32::try_from(limits.max_rows).unwrap_or(u32::MAX);
    while let Some(id) = rd.next(&mut buf)? {
        match id {
            BRT_BEGIN_SHEET_DATA => in_data = true,
            BRT_END_SHEET_DATA => break,
            BRT_ROW_HDR if in_data => {
                row = u32_at(&buf, 0).unwrap_or(0);
                // Past Excel's last row the value reader ends the sheet; past the row cap we do.
                if row >= max_row || row > MAX_ROW {
                    out.truncated = true;
                    break;
                }
            }
            0x0002..=BRT_LAST_CELL if in_data => {
                // col: u32, then iStyleRef: u24 + flags (the xf of the cell).
                let (Some(col), Some(xf)) = (u32_at(&buf, 0), u32_at(&buf, 4)) else {
                    return Err(OfficeError::Corrupt("xlsb cell record too short".into()));
                };
                if out.value_cells >= limits.max_sheet_cells {
                    out.truncated = true;
                    break;
                }
                out.value_cells += 1;
                let xf = (xf & 0x00FF_FFFF) as usize;
                let fmt = xf_to_format.get(xf).copied().unwrap_or(0);
                if fmt != 0 && (col as usize) < limits.max_cols {
                    out.cells.push((row, col as u16, fmt));
                }
            }
            _ => {}
        }
    }
    out.cells.sort_unstable_by_key(|&(r, c, _)| (r, c));
    Ok(out)
}
