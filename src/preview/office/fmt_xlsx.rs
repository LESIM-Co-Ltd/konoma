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
use super::workbook::{CellError, MergeRange, NumFmtRef};
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
    /// Number of cells that show a value: those with a `<v>` holding text or an `<is>` (the
    /// binary formats and ods count their own value-bearing cells). A formula-only cell shows
    /// nothing and is not a kept cell, so it does not count (xlsx), and the builder's `kept`
    /// never exceeds this.
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
    /// Merged ranges (xlsx only; at most [`MAX_MERGES`]). `sheetData` comes before `mergeCells` in
    /// the part, so a sheet cut off by a limit has none: the merges that matter are in the part of
    /// the sheet that was not read.
    pub merges: Vec<MergeRange>,
    /// Data was left out: the pass stopped at a row past the row cap, at a reference that
    /// overflows the value reader's arithmetic, or at the sheet's cell budget, **and** a value,
    /// an inline string or a formula lies past that point (a tail of empty cells is not data).
    pub truncated: bool,
    /// xlsx: where the pass stopped reading (any of the three cases above): how many cells (`<c>`
    /// elements, empty ones included, as the value reader counts them) precede it. The value
    /// reader must stop after that many and never be asked for the next one, so it never reads a
    /// cell whose format was not recorded. The reader parses references with plain `u32` arithmetic
    /// (a panic in a debug build, a wrapped position in a release build), and a stop at the row
    /// cap also saves reading the rest of a million-row part.
    pub reader_stop: Option<u64>,
}

/// Most merged ranges kept per sheet (a hostile part could list millions).
pub const MAX_MERGES: usize = 100_000;

/// Longest number-format code (in characters) that is kept. Excel itself allows 255; anything longer
/// is not a format Excel wrote, and is dropped at read time (the cell is shown as `General`)
/// instead of being held in memory — a 100 MB `formatCode` attribute must not be.
pub const MAX_FORMAT_CODE_CHARS: usize = 255;

/// Whether a number-format code is short enough to keep.
pub(crate) fn keep_format_code(code: &str) -> bool {
    // `chars().take(256)` bounds the work on a huge string.
    code.chars().take(MAX_FORMAT_CODE_CHARS + 1).count() <= MAX_FORMAT_CODE_CHARS
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

    /// Refuses a workbook whose sheets together take more of the reader's dense matrices than
    /// `Limits::max_dense_cells`. The per-sheet check does not see this: `calamine` keeps **every**
    /// sheet's matrix once the file is open (hidden ones too), so a hundred sheets each just under
    /// the limit would ask for a hundred times the memory the limit stands for.
    pub(crate) fn check_total_area(total: u64, limits: &Limits) -> Result<(), OfficeError> {
        if total > limits.max_dense_cells {
            return Err(OfficeError::TooLarge { what: "sheet area" });
        }
        Ok(())
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

/// What the workbook-level parts of an OOXML package (xlsx-family or xlsb) say: the date system, the
/// format table, which format each cell style has, and where each sheet's part is. Reading this is
/// cheap and independent of the size of any sheet: the sheets themselves are read one at a time
/// ([`read_sheet`]).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct OoxmlPackage {
    /// `<workbookPr date1904="1"/>`.
    pub date1904: bool,
    /// Deduplicated formats; index 0 is always `General`.
    pub formats: Vec<NumFmtRef>,
    /// Cell style (`cellXfs` ordinal) -> index into `formats`.
    pub xf_to_format: Vec<u16>,
    /// `(sheet name, part path)` in workbook order, for the sheets whose relationship resolves.
    pub sheets: Vec<(String, String)>,
}

impl OoxmlPackage {
    /// The part holding the sheet called `name`.
    pub fn part_of(&self, name: &str) -> Option<&str> {
        self.sheets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p.as_str())
    }
}

/// Reads the workbook-level parts of an xlsx-family package. No sheet part is touched; the shared
/// string table is only *checked* (`calamine` loads all of it when it opens the file).
pub fn read_package(path: &std::path::Path, limits: &Limits) -> Result<OoxmlPackage, OfficeError> {
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

    // 4. shared strings: `calamine` reads the whole table when it opens the file, so a table that
    //    is too large is refused here, before that.
    if let Some(r) = container::part_reader(&mut zip, "xl/sharedStrings.xml", cap)? {
        check_shared_strings(BufReader::new(r), limits)?;
    }

    let sheets = sheet_decls
        .into_iter()
        .filter_map(|d| rels.get(&d.rid).map(|part| (d.name, part.clone())))
        .collect();
    Ok(OoxmlPackage {
        date1904,
        formats: styles.formats,
        xf_to_format: styles.xf_to_format,
        sheets,
    })
}

/// Reads the cell formats of **one** sheet part (`part` from [`OoxmlPackage::sheets`]). Streams the
/// part and keeps only what is bounded by `limits`; a missing part is a sheet with no formats.
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

/// The package and every sheet's formats in one value (tests: one call inspects a whole file;
/// the loader reads sheets one at a time).
#[cfg(test)]
pub fn read(path: &std::path::Path, limits: &Limits) -> Result<XlsxFormats, OfficeError> {
    let pkg = read_package(path, limits)?;
    let mut sheets = HashMap::new();
    for (name, part) in &pkg.sheets {
        sheets.insert(
            name.clone(),
            read_sheet(path, part, &pkg.xf_to_format, limits)?,
        );
    }
    Ok(XlsxFormats {
        date1904: pkg.date1904,
        formats: pkg.formats,
        sheets,
    })
}

/// Fixed cost charged to the text budget for each string the reader keeps (the `String` header
/// and allocation overhead, which is not part of its length).
pub(crate) const STRING_OVERHEAD: u64 = 32;

/// Refuses a shared-string table that `calamine` could not hold: `calamine` copies every `<si>`
/// into a `String` when it opens the file (and reserves room for `uniqueCount` of them up front).
/// Counts each string plus [`STRING_OVERHEAD`] against `Limits::max_text_bytes`. Returns the cost
/// of the table.
pub(crate) fn check_shared_strings(src: impl BufRead, limits: &Limits) -> Result<u64, OfficeError> {
    let too_large = || OfficeError::TooLarge { what: "text" };
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
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
    Ok(total)
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
    let code = attr_bounded(e, b"formatCode", MAX_RAW_FORMAT_CODE);
    if let (Some(id), Some(code)) = (id, code) {
        if keep_format_code(&code) {
            custom.insert(id, code);
        }
    }
}

/// Longest raw (still escaped) `formatCode` attribute that is decoded at all: a numeric character
/// reference (`&#x10FFFF;`) is 10 bytes for one character, so a code of the longest kept length
/// is at most 2,550 raw bytes. A longer attribute is dropped without being copied.
const MAX_RAW_FORMAT_CODE: usize = MAX_FORMAT_CODE_CHARS * 10;

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

/// Streams one sheet part and records the number format of every value-bearing cell.
///
/// Everything kept is bounded: the pass **stops reading** at the first row or cell reference past
/// `Limits::max_rows` or one that overflows `u32` (the value reader wraps such a number instead of
/// failing, so a hostile `<row r="5000000000">` lands at an arbitrary row there), and once the
/// sheet has `Limits::max_sheet_cells` value cells. Real sheets list rows in order, so what is cut
/// off is the tail, never the middle.
///
/// **What the value reader counts** (`calamine` 0.36 `xlsx/cells_reader.rs`, which reads with
/// `expand_empty_elements`): every `<c>` element inside `<sheetData>` is one returned cell, whether
/// it is `<c .../>`, `<c ...></c>`, holds only a formula or an inline string, or a value. So
/// [`SheetFormats::reader_stop`] counts *every* `<c>`, not only the ones with a value.
///
/// **Truncation is about data.** After a stop the pass keeps scanning (it records nothing) only
/// to see whether anything is left to show: a value, an inline string or a formula in a cell
/// past the stop sets [`SheetFormats::truncated`]; a tail of formatted empty cells and empty rows
/// (Excel writes those) does not.
pub(crate) fn parse_sheet(
    src: impl BufRead,
    xf_to_format: &[u16],
    limits: &Limits,
) -> Result<SheetFormats, OfficeError> {
    /// The `<c>` being read.
    #[derive(Default)]
    struct Cur {
        row: u32,
        col: u32,
        xf: u32,
        /// A `<v>` with text or an `<is>`: the cell shows something.
        has_value: bool,
    }
    let mut rd = Reader::from_reader(src);
    let mut buf = Vec::new();
    let mut out = SheetFormats::default();
    let max_col = limits.max_cols as u32;
    let max_row = u32::try_from(limits.max_rows).unwrap_or(u32::MAX);
    // 0-based position the next row / cell takes when it carries no `r`.
    let mut next_row: u32 = 0;
    let mut cur_row: u32 = 0;
    let mut next_col: u32 = 0;
    let mut cell: Option<Cur> = None;
    // `<c>` elements met so far (what the value reader returns, whatever they hold).
    let mut c_starts: u64 = 0;
    // Only what is inside `<sheetData>` is cells (the value reader stops at its end).
    let mut in_data = false;
    let mut in_v = false;
    // The reader's stop point was fixed: from here on only the question "is there data left?".
    let mut stopped = false;
    loop {
        buf.clear();
        let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(ev, Event::Empty(_));
                match e.local_name().as_ref() {
                    b"sheetData" => in_data = !is_empty,
                    b"row" if in_data && !stopped => {
                        cur_row = match attr(e, b"r", false).and_then(|v| parse_row_ref(&v)) {
                            Some(r) => r,
                            None => next_row,
                        };
                        if cur_row >= max_row {
                            out.reader_stop = Some(c_starts);
                            stopped = true;
                        }
                        next_col = 0;
                        if is_empty {
                            next_row = cur_row.saturating_add(1);
                        }
                    }
                    b"c" if in_data => {
                        if !stopped {
                            let r_attr = attr(e, b"r", false);
                            let (row, col) = match r_attr.as_deref().map(parse_a1) {
                                Some(Some(pos)) => pos,
                                // A reference too long to be a real one (it overflows `u32`,
                                // which the value reader would wrap): the rest of the sheet is
                                // not trusted.
                                Some(None) if r_attr.as_deref().is_some_and(absurd_ref) => {
                                    (max_row, 0)
                                }
                                // Short junk (`??`) just continues the row.
                                _ => (cur_row, next_col),
                            };
                            if row >= max_row {
                                out.reader_stop = Some(c_starts);
                                stopped = true;
                            } else {
                                let xf = attr(e, b"s", false)
                                    .and_then(|v| v.trim().parse::<u32>().ok())
                                    .unwrap_or(0);
                                next_col = col.saturating_add(1);
                                // Counted when it opens, empty element or not.
                                c_starts += 1;
                                if !is_empty {
                                    cell = Some(Cur {
                                        row,
                                        col,
                                        xf,
                                        has_value: false,
                                    });
                                }
                            }
                        }
                        if stopped && !is_empty {
                            cell = Some(Cur::default());
                        }
                    }
                    b"v" | b"is" | b"f" if cell.is_some() => {
                        let name = e.local_name();
                        let name = name.as_ref();
                        if name == b"v" {
                            in_v = !is_empty;
                        } else if stopped {
                            // An inline string or a formula past the stop: data not shown.
                            out.truncated = true;
                            break;
                        } else if name == b"is" {
                            if let Some(c) = cell.as_mut() {
                                c.has_value = true;
                            }
                        } else if attr(e, b"si", false)
                            .and_then(|v| v.trim().parse::<u64>().ok())
                            .is_some_and(|si| si >= limits.max_sheet_cells)
                        {
                            // The value reader keeps a table of the shared formulas by `si` and
                            // grows it to the largest one named, so an `si` in the millions asks
                            // it for gigabytes. `si` counts formula groups: it cannot honestly
                            // exceed the number of cells a sheet may hold.
                            return Err(OfficeError::TooLarge {
                                what: "sheet cells",
                            });
                        }
                    }
                    b"mergeCell" if out.merges.len() < MAX_MERGES => {
                        if let Some(m) = attr(e, b"ref", false).and_then(|v| parse_merge(&v)) {
                            out.merges.push(m);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(_) | Event::CData(_) if in_v && cell.is_some() => {
                let empty = match &ev {
                    Event::Text(t) => t.is_empty(),
                    Event::CData(t) => t.is_empty(),
                    _ => true,
                };
                if !empty {
                    if stopped {
                        out.truncated = true;
                        break;
                    }
                    if let Some(c) = cell.as_mut() {
                        c.has_value = true;
                    }
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"sheetData" => in_data = false,
                b"row" => next_row = cur_row.saturating_add(1),
                b"v" => in_v = false,
                b"c" => {
                    if let Some(c) = cell.take().filter(|c| c.has_value && !stopped) {
                        if out.value_cells >= limits.max_sheet_cells {
                            // This cell is the first one over the budget: it and what follows
                            // are not read (the reader is stopped before it).
                            out.truncated = true;
                            out.reader_stop = Some(c_starts.saturating_sub(1));
                            break;
                        }
                        out.value_cells += 1;
                        let fmt = xf_to_format.get(c.xf as usize).copied().unwrap_or(0);
                        if fmt != 0 && c.col < max_col {
                            out.cells.push((c.row, c.col as u16, fmt));
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    // Real files are in order already (the sort is then a no-op scan); a hostile or sloppy
    // producer cannot break the binary search.
    out.cells.sort_unstable_by_key(|&(r, c, _)| (r, c));
    Ok(out)
}

/// A `<row r="...">` value as a 0-based row. `None` when it is not a positive decimal number (the
/// row then follows the previous one); a number past `u32` is `u32::MAX`, beyond any row cap.
fn parse_row_ref(v: &str) -> Option<u32> {
    let v = v.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Digit by digit with saturation: `parse::<u32>` would refuse 5000000000 and the row would
    // silently fall back to "the next row".
    let n = v.bytes().fold(0u64, |n, b| {
        n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    });
    match n {
        0 => None,
        n => Some(u32::try_from(n - 1).unwrap_or(u32::MAX)),
    }
}

/// `A1:C3` (or a lone `A1`) -> the range.
fn parse_merge(v: &str) -> Option<MergeRange> {
    let (a, b) = v.split_once(':').unwrap_or((v, v));
    let (row0, col0) = parse_a1(a)?;
    let (row1, col1) = parse_a1(b)?;
    Some(MergeRange {
        row0,
        col0,
        row1,
        col1,
    })
}

/// A cell reference `parse_a1` rejected that is long enough to wrap the reader's own arithmetic
/// (4+ column letters is already past XFD; 8+ row digits is past row 1,048,576 by 100x).
fn absurd_ref(r: &str) -> bool {
    let letters = r.bytes().filter(u8::is_ascii_alphabetic).count();
    let digits = r.bytes().filter(u8::is_ascii_digit).count();
    letters >= 4 || digits >= 8
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

/// [`attr`] for an unprefixed attribute, but `None` (without decoding or copying it) when the
/// raw value is longer than `max_raw` bytes.
fn attr_bounded(e: &BytesStart<'_>, local: &[u8], max_raw: usize) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        let key = a.key;
        if key.local_name().as_ref() == local && key.prefix().is_none() {
            if a.value.len() > max_raw {
                return None;
            }
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
