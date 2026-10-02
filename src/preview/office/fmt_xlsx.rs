//! Format pass for xlsx / xlsm / xltx / xltm: which number format each cell has.
//!
//! `calamine` reads values but collapses every cell's format into a 3-way `CellFormat`
//! (date / time / other) and does not expose it, so konoma reads the format of each cell itself:
//!
//! 1. `xl/workbook.xml`: sheet name -> relationship id, and `workbookPr date1904`.
//! 2. `xl/_rels/workbook.xml.rels`: relationship id -> part path (relative or `/xl/...` absolute).
//! 3. `xl/styles.xml`: `numFmts` (id -> format code) and `cellXfs` (xf index -> numFmtId); the two
//!    are folded into `xf index -> index into a small deduplicated table of formats`.
//! 4. each sheet part: `<c r="B3" s="5">` -> the xf of every value-bearing cell. A cell without
//!    `r` continues after the previous cell of its row; a row without `r` follows the previous row;
//!    a cell without `s` uses xf 0 (which is `General` unless the file says otherwise).
//!
//! The output is deliberately compact: a deduplicated format table (typically < 50 entries) and,
//! per sheet, a sorted sparse list of `(row, col, format index)` for the cells whose format is not
//! `General` — never one string per cell.
//!
//! Safety: quick-xml never expands DTD entities (a `<!DOCTYPE ... <!ENTITY ...>` is just an
//! ignored event; an undefined `&x;` fails to unescape and we keep the raw text), and parts are
//! read through `container::part_reader` with a size cap, so neither a billion-laughs document
//! nor an oversized part can run away. Sheets are *streamed*; no DOM is built.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::container::{self, Limits};
use super::workbook::{CellError, NumFmtRef};
use super::OfficeError;

/// Everything a format pass learned about the workbook. Shared by every format's pass
/// (`fmt_ods`, `fmt_xlsb`, `fmt_xls` return it too).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct XlsxFormats {
    /// `<workbookPr date1904="1"/>`.
    pub date1904: bool,
    /// Deduplicated formats; index 0 is always `General`.
    pub formats: Vec<NumFmtRef>,
    /// Per-sheet cell formats, keyed by the sheet name in `workbook.xml`.
    pub sheets: HashMap<String, SheetFormats>,
}

/// Format and extent information of one sheet part.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SheetFormats {
    /// `(row, col, format index)` of value-bearing cells whose format is not `General`, 0-based,
    /// sorted by `(row, col)`. `col` fits `u16` because columns beyond Excel's 16,384 are dropped.
    pub cells: Vec<(u32, u16, u16)>,
    /// Number of cells the value reader will put into a range: those with `<v>`, `<is>` or `<f>`
    /// (`calamine` builds a second range, of the formulas, over the cells that have a formula).
    pub value_cells: u64,
    /// Bounding box of those cells: `(min_row, min_col, max_row, max_col)`, 0-based. Columns past
    /// Excel's last are part of it: the reader does not drop them, so its dense matrix covers them.
    pub bbox: Option<(u32, u32, u32, u32)>,
    /// Bytes of text the reader and the loader will hold for this sheet: every reference to a
    /// shared string counts the string again, plus inline / formula-result strings and formulas
    /// (a shared formula's text again for each cell that derives from it).
    pub text_bytes: u64,
    /// Error values the value reader loses (ods: `calcext:value-type="error"` cells arrive as an
    /// empty string), `(row, col, code)` sorted by `(row, col)`. Empty for the other formats.
    pub errors: Vec<(u32, u32, CellError)>,
    /// Cells the value reader materialises on the way to its dense matrix, when that is more than
    /// the bounding box (ods builds one row of cells per *physical* row before expanding repeats).
    pub read_cost: u64,
}

impl SheetFormats {
    /// Area (`rows x cols`) of the bounding box of the value-bearing cells.
    pub fn bbox_area(&self) -> u64 {
        match self.bbox {
            None => 0,
            Some((r0, c0, r1, c1)) => {
                (u64::from(r1 - r0) + 1).saturating_mul(u64::from(c1 - c0) + 1)
            }
        }
    }

    /// Books one value-bearing cell of a binary format: counts it against the cell budget, grows
    /// the bounding box (whatever its column: the value reader's dense matrix would cover it) and
    /// remembers a non-`General` format for it (columns past Excel's last are not shown).
    pub(crate) fn note_value(
        &mut self,
        row: u32,
        col: u32,
        fmt: u16,
        limits: &Limits,
    ) -> Result<(), OfficeError> {
        self.value_cells += 1;
        if self.value_cells > limits.max_dense_cells {
            return Err(OfficeError::TooLarge {
                what: "sheet cells",
            });
        }
        self.bbox = Some(match self.bbox {
            None => (row, col, row, col),
            Some((r0, c0, r1, c1)) => (r0.min(row), c0.min(col), r1.max(row), c1.max(col)),
        });
        if fmt != 0 && (col as usize) < limits.max_cols {
            self.cells.push((row, col as u16, fmt));
        }
        Ok(())
    }

    /// Charges `bytes` of text to the sheet's budget.
    pub(crate) fn add_text(&mut self, bytes: u64, limits: &Limits) -> Result<(), OfficeError> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > limits.max_text_bytes {
            return Err(OfficeError::TooLarge { what: "text" });
        }
        Ok(())
    }

    /// The cost the value reader will pay for this sheet: the larger of the bounding box and any
    /// intermediate materialisation (`read_cost`).
    pub fn dense_cost(&self) -> u64 {
        self.bbox_area().max(self.read_cost)
    }

    /// The error value recorded at `(row, col)`, if any.
    pub fn error_at(&self, row: u32, col: u32) -> Option<CellError> {
        self.errors
            .binary_search_by_key(&(row, col), |&(r, c, _)| (r, c))
            .ok()
            .map(|i| self.errors[i].2)
    }

    /// The format index of the cell at `(row, col)` (0 = `General`).
    pub fn format_at(&self, row: u32, col: u32) -> u16 {
        let Ok(col) = u16::try_from(col) else {
            return 0;
        };
        self.cells
            .binary_search_by_key(&(row, col), |&(r, c, _)| (r, c))
            .map(|i| self.cells[i].2)
            .unwrap_or(0)
    }
}

/// Reads the workbook-level and per-cell format information of an xlsx-family package.
pub fn read(path: &std::path::Path, limits: &Limits) -> Result<XlsxFormats, OfficeError> {
    let mut zip = container::open_zip(path)?;
    let cap = limits.max_part_bytes;

    // 1. workbook.xml
    let (date1904, sheet_decls) = {
        let r = container::part_reader(&mut zip, "xl/workbook.xml", cap)?
            .ok_or_else(|| OfficeError::Corrupt("missing xl/workbook.xml".into()))?;
        parse_workbook(BufReader::new(r))?
    };

    // 2. rels
    let rels = match container::part_reader(&mut zip, "xl/_rels/workbook.xml.rels", cap)? {
        Some(r) => parse_rels(BufReader::new(r))?,
        None => HashMap::new(),
    };

    // 3. styles
    let styles = match container::part_reader(&mut zip, "xl/styles.xml", cap)? {
        Some(r) => parse_styles(BufReader::new(r))?,
        None => Styles::without_styles(),
    };

    // 4. shared strings: only their lengths (the text budget counts a string once per cell that
    //    refers to it, before `calamine` copies it that many times).
    let (shared_lens, mut text_total) =
        match container::part_reader(&mut zip, "xl/sharedStrings.xml", cap)? {
            Some(r) => parse_shared_strings(BufReader::new(r), limits)?,
            None => (Vec::new(), 0),
        };

    let mut out = XlsxFormats {
        date1904,
        formats: styles.formats,
        sheets: HashMap::new(),
    };

    // 5. every sheet part
    for decl in sheet_decls {
        let Some(part) = rels.get(&decl.rid) else {
            continue;
        };
        let Some(r) = container::part_reader(&mut zip, part, cap)? else {
            continue;
        };
        let sf = parse_sheet(
            BufReader::new(r),
            &styles.xf_to_format,
            &shared_lens,
            limits,
        )?;
        text_total = text_total.saturating_add(sf.text_bytes);
        if text_total > limits.max_text_bytes {
            return Err(OfficeError::TooLarge { what: "text" });
        }
        out.sheets.insert(decl.name, sf);
    }
    Ok(out)
}

/// Fixed cost charged to the text budget for each string the reader keeps (the `String` header
/// and allocation overhead, which is not part of its length).
pub(crate) const STRING_OVERHEAD: u64 = 32;

/// `xl/sharedStrings.xml` -> the byte length of each `<si>` (in order, as `calamine` indexes
/// them) and the budget cost of the whole table. A table that alone exceeds the text budget, or
/// that announces more strings than the budget could hold, is `TooLarge` (`calamine` reserves
/// room for `uniqueCount` strings up front).
pub(crate) fn parse_shared_strings(
    src: impl BufRead,
    limits: &Limits,
) -> Result<(Vec<u32>, u64), OfficeError> {
    let too_large = || OfficeError::TooLarge { what: "text" };
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut lens: Vec<u32> = Vec::new();
    let mut total: u64 = 0;
    let mut cur: Option<u64> = None;
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"sst" => {
                    let unique = attr(&e, b"uniqueCount", false)
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .unwrap_or(0);
                    if unique.saturating_mul(STRING_OVERHEAD) > limits.max_text_bytes {
                        return Err(too_large());
                    }
                }
                b"si" => cur = Some(0),
                _ => {}
            },
            Event::Text(t) => {
                if let Some(n) = cur.as_mut() {
                    *n += t.len() as u64;
                }
            }
            Event::CData(t) => {
                if let Some(n) = cur.as_mut() {
                    *n += t.len() as u64;
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"si" => {
                if let Some(n) = cur.take() {
                    lens.push(u32::try_from(n).unwrap_or(u32::MAX));
                    total = total.saturating_add(n + STRING_OVERHEAD);
                    if total > limits.max_text_bytes {
                        return Err(too_large());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((lens, total))
}

// ---------------------------------------------------------------------------------------------
// workbook.xml
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub(crate) struct SheetDecl {
    pub name: String,
    pub rid: String,
}

pub(crate) fn parse_workbook(src: impl BufRead) -> Result<(bool, Vec<SheetDecl>), OfficeError> {
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut date1904 = false;
    let mut sheets = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"workbookPr" => {
                    if let Some(v) = attr(&e, b"date1904", false) {
                        date1904 = is_true(&v);
                    }
                }
                b"sheet" => {
                    let name = attr(&e, b"name", false);
                    // `r:id` — any prefix, but it must have one (a bare `id` is something else).
                    let rid = attr(&e, b"id", true);
                    if let (Some(name), Some(rid)) = (name, rid) {
                        sheets.push(SheetDecl { name, rid });
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((date1904, sheets))
}

// ---------------------------------------------------------------------------------------------
// workbook.xml.rels
// ---------------------------------------------------------------------------------------------

/// `Id` -> normalized part path inside the zip (no leading slash).
pub(crate) fn parse_rels(src: impl BufRead) -> Result<HashMap<String, String>, OfficeError> {
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut map = HashMap::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                if attr(&e, b"TargetMode", false)
                    .is_some_and(|m| m.eq_ignore_ascii_case("External"))
                {
                    continue;
                }
                if let (Some(id), Some(target)) =
                    (attr(&e, b"Id", false), attr(&e, b"Target", false))
                {
                    map.insert(id, resolve_target("xl", &target));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// Resolves a relationship target against the directory of the part that owns the `.rels`
/// (`base`, no trailing slash): `worksheets/sheet1.xml` -> `xl/worksheets/sheet1.xml`,
/// `/xl/worksheets/sheet1.xml` -> `xl/worksheets/sheet1.xml`, `../x.xml` climbs one level.
pub(crate) fn resolve_target(base: &str, target: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let rest = match target.strip_prefix('/') {
        Some(abs) => abs,
        None => {
            parts.extend(base.split('/').filter(|s| !s.is_empty()));
            target
        }
    };
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

// ---------------------------------------------------------------------------------------------
// styles.xml
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub(crate) struct Styles {
    /// Deduplicated format table (index 0 = General).
    pub formats: Vec<NumFmtRef>,
    /// `cellXfs` ordinal -> index into `formats`.
    pub xf_to_format: Vec<u16>,
}

impl Styles {
    /// A package with no `styles.xml`: every cell is `General`.
    fn without_styles() -> Styles {
        Styles {
            formats: vec![NumFmtRef::General],
            xf_to_format: Vec::new(),
        }
    }
}

pub(crate) fn parse_styles(src: impl BufRead) -> Result<Styles, OfficeError> {
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut custom: HashMap<u32, String> = HashMap::new();
    let mut xf_ids: Vec<u32> = Vec::new();
    let mut in_cell_xfs = false;
    // Only `<numFmts>` defines the file's formats: `<dxfs>` (conditional formatting) carries
    // `<numFmt>` elements of its own whose ids are unrelated to the cell formats.
    let mut in_num_fmts = false;
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"cellXfs" => in_cell_xfs = true,
                b"numFmts" => in_num_fmts = true,
                b"xf" if in_cell_xfs => xf_ids.push(xf_num_fmt_id(&e)),
                b"numFmt" if in_num_fmts => note_num_fmt(&e, &mut custom),
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"xf" if in_cell_xfs => xf_ids.push(xf_num_fmt_id(&e)),
                b"numFmt" if in_num_fmts => note_num_fmt(&e, &mut custom),
                _ => {}
            },
            Event::End(e) if e.local_name().as_ref() == b"cellXfs" => in_cell_xfs = false,
            Event::End(e) if e.local_name().as_ref() == b"numFmts" => in_num_fmts = false,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(styles_from(&xf_ids, &custom))
}

/// Folds `xf index -> numFmtId` and the file's custom format table into the compact
/// [`Styles`] (shared by the binary formats, which read the same two tables from records).
pub(crate) fn styles_from(xf_ids: &[u32], custom: &HashMap<u32, String>) -> Styles {
    let mut formats = vec![NumFmtRef::General];
    let mut index: HashMap<NumFmtRef, u16> = HashMap::new();
    index.insert(NumFmtRef::General, 0);
    let mut xf_to_format = Vec::with_capacity(xf_ids.len());
    for &id in xf_ids {
        let fmt = resolve_num_fmt(id, custom);
        let idx = match index.get(&fmt) {
            Some(&i) => i,
            None => match u16::try_from(formats.len()) {
                Ok(i) => {
                    formats.push(fmt.clone());
                    index.insert(fmt, i);
                    i
                }
                // More than 65,535 distinct formats cannot happen in a real file (Excel caps
                // styles far below); degrade to General rather than fail.
                Err(_) => 0,
            },
        };
        xf_to_format.push(idx);
    }
    Styles {
        formats,
        xf_to_format,
    }
}

fn xf_num_fmt_id(e: &BytesStart<'_>) -> u32 {
    attr(e, b"numFmtId", false)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

fn note_num_fmt(e: &BytesStart<'_>, custom: &mut HashMap<u32, String>) {
    let id = attr(e, b"numFmtId", false).and_then(|v| v.trim().parse::<u32>().ok());
    let code = attr(e, b"formatCode", false);
    if let (Some(id), Some(code)) = (id, code) {
        custom.insert(id, code);
    }
}

/// A `numFmtId` + the file's custom table -> the format reference. A custom definition wins over
/// a built-in id of the same number; an id that is neither defined nor built-in is `General`.
fn resolve_num_fmt(id: u32, custom: &HashMap<u32, String>) -> NumFmtRef {
    if let Some(code) = custom.get(&id) {
        // LibreOffice writes `General` as a custom code (id 164); it is the same as no format.
        if code.trim().eq_ignore_ascii_case("general") {
            return NumFmtRef::General;
        }
        return NumFmtRef::Custom(code.as_str().into());
    }
    match id {
        0 => NumFmtRef::General,
        1..=163 => NumFmtRef::Builtin(id as u16),
        _ => NumFmtRef::General,
    }
}

// ---------------------------------------------------------------------------------------------
// sheet XML
// ---------------------------------------------------------------------------------------------

pub(crate) fn parse_sheet(
    src: impl BufRead,
    xf_to_format: &[u16],
    shared_lens: &[u32],
    limits: &Limits,
) -> Result<SheetFormats, OfficeError> {
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut out = SheetFormats::default();
    let max_col = limits.max_cols as u32;
    // 0-based position the next row / cell takes when it carries no `r`.
    let mut next_row: u32 = 0;
    let mut cur_row: u32 = 0;
    let mut next_col: u32 = 0;
    // The `<c>` being read: (row, col, xf, has_value).
    let mut cell: Option<(u32, u32, u32, bool)> = None;
    // Text accounting. `shared`: the cell's `t="s"` (its `<v>` is an index into the shared
    // strings). `v_index`: that index, being read. `f_len` / `f_shared`: the formula being read.
    let mut shared = false;
    // The cell's `<v>` is text (`t="str"` formula result, `"e"` error, `"d"` ISO date), not a number.
    let mut text_v = false;
    let (mut in_v, mut in_is, mut in_f) = (false, false, false);
    let mut v_index: Option<u64> = None;
    let mut f_len: u64 = 0;
    let mut f_shared: Option<usize> = None;
    // Text length of each shared formula's master, by `si`, for the cells that derive from it.
    let mut shared_formula_len: HashMap<usize, u64> = HashMap::new();
    loop {
        buf.clear();
        let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(ev, Event::Empty(_));
                match e.local_name().as_ref() {
                    b"row" => {
                        cur_row = attr(e, b"r", false)
                            .and_then(|v| v.trim().parse::<u32>().ok())
                            .filter(|&r| r >= 1)
                            .map(|r| r - 1)
                            .unwrap_or(next_row);
                        next_col = 0;
                        if is_empty {
                            next_row = cur_row.saturating_add(1);
                        }
                    }
                    b"c" => {
                        let r_attr = attr(e, b"r", false);
                        let (row, col) = match r_attr.as_deref().map(parse_a1) {
                            Some(Some(pos)) => pos,
                            // A reference that is long enough to overflow the reader's own
                            // arithmetic (it wraps in release builds): its dense matrix could be
                            // anything, so refuse it. Short junk (`??`) just continues the row.
                            Some(None) if r_attr.as_deref().is_some_and(absurd_ref) => {
                                return Err(OfficeError::TooLarge { what: "sheet area" });
                            }
                            _ => (cur_row, next_col),
                        };
                        let xf = attr(e, b"s", false)
                            .and_then(|v| v.trim().parse::<u32>().ok())
                            .unwrap_or(0);
                        let t = attr(e, b"t", false);
                        shared = t.as_deref() == Some("s");
                        text_v = matches!(t.as_deref(), Some("str" | "e" | "d"));
                        next_col = col.saturating_add(1);
                        if is_empty {
                            // `<c .../>` has no value: nothing to record.
                        } else {
                            cell = Some((row, col, xf, false));
                        }
                    }
                    // Only a value-bearing child makes the cell show something; a formula makes
                    // the reader keep it too (in the range of formulas).
                    b"v" | b"is" | b"f" => {
                        let name = e.local_name();
                        let name = name.as_ref();
                        if let Some(c) = cell.as_mut() {
                            c.3 = true;
                        }
                        match name {
                            b"v" => {
                                in_v = !is_empty;
                                v_index = None;
                            }
                            b"is" => in_is = !is_empty,
                            _ => {
                                f_len = 0;
                                f_shared = if attr(e, b"t", false).is_some_and(|t| t == "shared") {
                                    attr(e, b"si", false).and_then(|v| v.trim().parse().ok())
                                } else {
                                    None
                                };
                                if is_empty {
                                    // `<f t="shared" si="3"/>`: derived from the master's text.
                                    finish_formula(&mut out, f_shared, 0, &mut shared_formula_len);
                                } else {
                                    in_f = true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(ref t) => {
                let n = t.len() as u64;
                if in_is || in_f {
                    if in_f {
                        f_len += n;
                    } else {
                        out.text_bytes += n;
                    }
                } else if in_v {
                    if shared {
                        // Digits of the shared-string index.
                        let mut idx = v_index.unwrap_or(0);
                        for b in t.iter().filter(|b| b.is_ascii_digit()) {
                            idx = idx.saturating_mul(10).saturating_add(u64::from(b - b'0'));
                        }
                        v_index = Some(idx);
                    } else if text_v {
                        out.text_bytes += n;
                    }
                }
            }
            Event::CData(ref t) => {
                // Inline text, or the text of a text-valued `<v>` (a shared one is an index).
                if in_is || (in_v && text_v) {
                    out.text_bytes += t.len() as u64;
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"row" => next_row = cur_row.saturating_add(1),
                b"v" => {
                    in_v = false;
                    if let Some(i) = v_index.take().filter(|_| shared) {
                        let len = usize::try_from(i)
                            .ok()
                            .and_then(|i| shared_lens.get(i))
                            .copied()
                            .unwrap_or(0);
                        out.text_bytes += u64::from(len) + STRING_OVERHEAD;
                    }
                }
                b"is" => {
                    in_is = false;
                    out.text_bytes += STRING_OVERHEAD;
                }
                b"f" => {
                    in_f = false;
                    finish_formula(&mut out, f_shared, f_len, &mut shared_formula_len);
                }
                b"c" => {
                    if let Some((row, col, xf, true)) = cell.take() {
                        record(&mut out, row, col, xf, xf_to_format, max_col, limits)?;
                    }
                    cell = None;
                    shared = false;
                    text_v = false;
                    if out.text_bytes > limits.max_text_bytes {
                        return Err(OfficeError::TooLarge { what: "text" });
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if out.text_bytes > limits.max_text_bytes {
        return Err(OfficeError::TooLarge { what: "text" });
    }
    // Real files are in order already (the sort is then a no-op scan); a hostile or sloppy
    // producer cannot break the binary search.
    out.cells.sort_unstable_by_key(|&(r, c, _)| (r, c));
    Ok(out)
}

/// A cell reference `parse_a1` rejected that is long enough to wrap the reader's own arithmetic
/// (4+ column letters is already past XFD; 8+ row digits is past row 1,048,576 by 100x).
fn absurd_ref(r: &str) -> bool {
    let letters = r.bytes().filter(u8::is_ascii_alphabetic).count();
    let digits = r.bytes().filter(u8::is_ascii_digit).count();
    letters >= 4 || digits >= 8
}

/// Books the text of a finished formula: its own length, or (a shared-formula cell with no text)
/// that of its master. A master (`ref` is not tracked: any `<f t="shared">` with text) is
/// remembered for the cells that follow.
fn finish_formula(
    out: &mut SheetFormats,
    shared: Option<usize>,
    own: u64,
    masters: &mut HashMap<usize, u64>,
) {
    let len = match shared {
        Some(si) if own == 0 => masters.get(&si).copied().unwrap_or(0),
        Some(si) => {
            // Bounded by the part size, but a hostile file could name millions of `si`.
            if masters.len() < 1 << 20 {
                masters.insert(si, own);
            }
            own
        }
        None => own,
    };
    out.text_bytes += len + STRING_OVERHEAD;
}

pub(crate) fn record(
    out: &mut SheetFormats,
    row: u32,
    col: u32,
    xf: u32,
    xf_to_format: &[u16],
    max_col: u32,
    limits: &Limits,
) -> Result<(), OfficeError> {
    // The bounding box and the count include columns beyond Excel's last: the reader keeps such a
    // cell, so its dense matrix covers it (`A1` and `ZZZZ1048576` would ask for ~500 GB). Only the
    // format of a cell that is not shown is dropped.
    out.value_cells += 1;
    if out.value_cells > limits.max_dense_cells {
        return Err(OfficeError::TooLarge {
            what: "sheet cells",
        });
    }
    out.bbox = Some(match out.bbox {
        None => (row, col, row, col),
        Some((r0, c0, r1, c1)) => (r0.min(row), c0.min(col), r1.max(row), c1.max(col)),
    });
    let fmt = xf_to_format.get(xf as usize).copied().unwrap_or(0);
    if fmt != 0 && col < max_col {
        out.cells.push((row, col as u16, fmt));
    }
    Ok(())
}

/// `B3` -> `(row 2, col 1)` (0-based). `None` for anything that is not a plain A1 reference.
pub(crate) fn parse_a1(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || !letters.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut col: u32 = 0;
    for b in letters.bytes() {
        col = col
            .checked_mul(26)?
            .checked_add(u32::from(b.to_ascii_uppercase() - b'A') + 1)?;
    }
    let row: u32 = digits.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// The value of the attribute whose *local* name is `local` (`prefixed`: it must carry a
/// namespace prefix, e.g. `r:id`; otherwise it must not). Entity references that do not resolve
/// (an undefined `&x;`) fall back to the raw text — never expanded.
fn attr(e: &BytesStart<'_>, local: &[u8], prefixed: bool) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        let key = a.key;
        if key.local_name().as_ref() == local && key.prefix().is_some() == prefixed {
            return Some(match a.normalized_value(XmlVersion::Implicit1_0) {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            });
        }
    }
    None
}

fn is_true(v: &str) -> bool {
    matches!(v.trim(), "1" | "true" | "TRUE" | "True")
}

fn xml_err(e: quick_xml::Error) -> OfficeError {
    OfficeError::Corrupt(format!("xml: {e}"))
}
