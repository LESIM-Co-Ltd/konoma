//! The in-memory model of a spreadsheet and the loader that fills it.
//!
//! **What the UI uses**: a grid of *display strings* ([`Sheet::display`]) and, for the cell under
//! the cursor, the *detail* ([`Cell`]: raw value, type, formula, number-format reference).
//!
//! **One sheet at a time.** A [`Workbook`] always lists every visible sheet (name only), but holds
//! the cells of **one** sheet: the one being shown. Moving to another sheet loads that sheet
//! (`load_workbook_sheet`) and drops the previous one, so the memory in use is that of the
//! largest sheet shown, not of the workbook, and a hidden or never-shown sheet costs nothing.
//! Re-opening the file for each switch is cheap for xlsx/xlsb (the workbook parts, the style
//! table and the shared strings are read again; the sheet XML is streamed) — see the doc of each
//! loader for what is read.
//!
//! **Bounded reads.** xlsx (konoma's own reader, `xlsx.rs`) and xlsb (`calamine`'s streaming cell
//! reader) deliver cells one at a time and reading **stops** when a budget of [`Limits`] is
//! reached (rows, kept cells, kept text bytes), so a huge sheet shows its beginning (marked as
//! capped) instead of being refused, and no dense matrix is ever built. ods and xls cannot be
//! streamed (`calamine` reads every sheet of those into dense matrices while opening the file), so
//! for them the size is checked *before* opening and a file that is too large is refused with a
//! reason.
//!
//! **Memory design** (per sheet):
//! - Rows are sparse: `rows[r]` holds only the non-empty cells as `(col, Cell)` sorted by column,
//!   so an empty cell costs nothing and a mostly-empty wide sheet stays small.
//! - A [`Cell`] is `CellValue` (24 bytes: the tag plus an `f64` / `Box<str>` / `&'static str`),
//!   an optional display string (16 bytes; `None` when it is the same as a text cell's own text,
//!   which is the common case for strings) and a `u16` index into the workbook's deduplicated
//!   format table: 48 bytes, 56 with its column in the row vector, plus its string heap.
//! - The raw value is *not* stored as a second string: [`Cell::raw_text`] derives it on demand.
//! - A displayed (formatted) string is cut at 1,024 characters: no format of a real workbook
//!   produces more, and `mmmm` repeated in a 255-character format code would otherwise cost
//!   hundreds of bytes in every cell.
//! - Formulas are rare per cell and live in a per-sheet map, not in every `Cell`.
//! - Number formats are not stored per cell as strings: a cell carries an index into
//!   [`Workbook::formats`] (usually < 50 entries).
//!
//! Positions are always **A1-based**: a used range that starts at C5 is stored with its cells at
//! rows 4.. / columns 2.. (0-based), so the grid speaks the same addresses as Excel.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use calamine::{Data, Dimensions, Range, Reader, SheetType, SheetVisible};

use super::container::{self, Detected, Limits};
use super::fmt_xlsx::SheetFormats;
use super::numfmt;
use super::xlsx::{self, Val, Visibility};
use super::{fmt_ods, fmt_xls, fmt_xlsb};
use super::{Locale, OfficeError, SheetKind};

/// How a cell's number format is referred to. The *meaning* of a built-in number or of a custom
/// code is the job of the number-format engine ([`numfmt`]); the loader only carries the
/// reference (ods styles are translated to Excel-style codes by `fmt_ods` first).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NumFmtRef {
    /// No format (Excel's `General`).
    General,
    /// A built-in format number (`numFmtId` 1..=163 that the file does not redefine).
    Builtin(u16),
    /// A format code written in the file (`yyyy/m/d`, `#,##0.00`, `[$¥-411]#,##0`, ...).
    Custom(Box<str>),
}

/// Cell error values (`#DIV/0!` ...), kept as their display code.
pub type CellError = &'static str;

/// The raw value of a cell as the file stores it.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// An integer.
    Int(i64),
    /// A floating-point number.
    Number(f64),
    /// A string.
    Text(Box<str>),
    /// A boolean.
    Bool(bool),
    /// An error value such as `#DIV/0!`.
    Error(CellError),
    /// An error value that is not one of the fixed codes: Excel 365's `#SPILL!`, `#CALC!`,
    /// `#GETTING_DATA`, ... shown as the file wrote it.
    ErrorText(Box<str>),
    /// A date/time/duration stored as a serial number (Excel's days since 1899-12-30 / 1904).
    /// `duration` marks a `[h]:mm:ss`-style elapsed time.
    DateTime {
        /// The serial number.
        serial: f64,
        /// True for an elapsed duration rather than a point in time.
        duration: bool,
    },
    /// A date/time/duration that the file stores as an ISO 8601 string (ODS).
    DateTimeIso(Box<str>),
}

/// The type shown in the cell detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellType {
    /// Integer or floating-point number.
    Number,
    /// String.
    Text,
    /// Boolean.
    Bool,
    /// Error value.
    Error,
    /// Date, time or duration.
    DateTime,
}

/// One non-empty cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The raw value.
    pub value: CellValue,
    /// The text shown in the grid; `None` means "the same as the value's own text" (strings).
    display: Option<Box<str>>,
    /// Index into [`Workbook::formats`] (0 = `General`).
    pub fmt: u16,
}

impl Cell {
    /// The text shown in the grid.
    pub fn display(&self) -> &str {
        match (&self.display, &self.value) {
            (Some(d), _) => d,
            (None, CellValue::Text(t)) => t,
            (None, _) => "",
        }
    }

    /// The type of the value.
    pub fn cell_type(&self) -> CellType {
        match self.value {
            CellValue::Int(_) | CellValue::Number(_) => CellType::Number,
            CellValue::Text(_) => CellType::Text,
            CellValue::Bool(_) => CellType::Bool,
            CellValue::Error(_) | CellValue::ErrorText(_) => CellType::Error,
            CellValue::DateTime { .. } | CellValue::DateTimeIso(_) => CellType::DateTime,
        }
    }

    /// The raw value as text, independent of any number format (`0.1`, `45292`, `TRUE`, ...).
    pub fn raw_text(&self) -> String {
        match &self.value {
            CellValue::Int(i) => i.to_string(),
            CellValue::Number(n) => number_text(*n),
            CellValue::Text(t) => t.to_string(),
            CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
            CellValue::Error(e) => (*e).to_string(),
            CellValue::ErrorText(e) => e.to_string(),
            CellValue::DateTime { serial, .. } => number_text(*serial),
            CellValue::DateTimeIso(s) => s.to_string(),
        }
    }
}

/// An inclusive merged range, 0-based, A1-based coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeRange {
    /// First row.
    pub row0: u32,
    /// First column.
    pub col0: u32,
    /// Last row (inclusive).
    pub row1: u32,
    /// Last column (inclusive).
    pub col1: u32,
}

/// One visible sheet (its cells only when [`Sheet::loaded`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    /// The sheet name.
    pub name: String,
    /// Whether the cells of this sheet were read. Only the sheet being shown is (see the module
    /// doc); the others are listed by name.
    pub loaded: bool,
    /// Rows in the grid (from A1; includes leading empty rows). After truncation.
    pub nrows: usize,
    /// Columns in the grid (from A1). After truncation.
    pub ncols: usize,
    /// True when rows were cut off (row cap, or the sheet's cell or text budget).
    pub rows_truncated: bool,
    /// True when columns were cut off (Excel's 16,384-column maximum).
    pub cols_truncated: bool,
    /// Sparse rows: `rows[r]` is `(col, cell)` sorted by `col`; absent = empty. `rows.len() == nrows`.
    rows: Vec<Vec<(u32, Cell)>>,
    /// Formulas (without the leading `=`) by `(row, col)`.
    formulas: HashMap<(u32, u32), Box<str>>,
    /// Merged ranges as the file declares them (the value is at the top-left cell only).
    pub merges: Vec<MergeRange>,
}

impl Sheet {
    /// The non-empty cell at `(row, col)`, if any.
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        let r = self.rows.get(row)?;
        let col = u32::try_from(col).ok()?;
        r.binary_search_by_key(&col, |&(c, _)| c)
            .ok()
            .map(|i| &r[i].1)
    }

    /// The text shown at `(row, col)` (`""` for an empty cell).
    pub fn display(&self, row: usize, col: usize) -> &str {
        self.cell(row, col).map(Cell::display).unwrap_or("")
    }

    /// The formula at `(row, col)` without the leading `=`, if the cell has one.
    pub fn formula(&self, row: usize, col: usize) -> Option<&str> {
        let key = (u32::try_from(row).ok()?, u32::try_from(col).ok()?);
        self.formulas.get(&key).map(|s| &**s)
    }

    /// The non-empty cells of one row as `(col, cell)`.
    pub fn row_cells(&self, row: usize) -> &[(u32, Cell)] {
        self.rows.get(row).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// A loaded workbook.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Workbook {
    /// The visible worksheets, in workbook order. Only the one that was asked for has cells.
    pub sheets: Vec<Sheet>,
    /// How many worksheets were hidden or very hidden (not in `sheets`).
    pub hidden_sheets: usize,
    /// The workbook uses the 1904 date system.
    pub date1904: bool,
    /// Deduplicated number formats referenced by [`Cell::fmt`]; index 0 is always `General`.
    pub formats: Vec<NumFmtRef>,
    /// Why the requested sheet could not be read although the workbook itself opened (a damaged
    /// sheet part, a hostile one). The other sheets are still listed and can be shown.
    pub sheet_error: Option<OfficeError>,
}

impl Workbook {
    /// Drops the cells of every sheet (the names stay): the sheet being left is not kept.
    pub fn unload_cells(&mut self) {
        for s in &mut self.sheets {
            if s.loaded {
                *s = Sheet {
                    name: std::mem::take(&mut s.name),
                    ..Sheet::default()
                };
            }
        }
        self.sheet_error = None;
    }

    /// The index of the sheet whose cells are loaded.
    pub fn loaded_index(&self) -> Option<usize> {
        self.sheets.iter().position(|s| s.loaded)
    }

    /// The number format of a cell.
    pub fn format_of(&self, cell: &Cell) -> &NumFmtRef {
        self.formats
            .get(usize::from(cell.fmt))
            .unwrap_or(&NumFmtRef::General)
    }
}

/// Loader options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadOptions {
    /// Locale for built-in date/currency formats (konoma's UI language).
    pub locale: Locale,
    /// Safety limits.
    pub limits: Limits,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            locale: Locale::En,
            limits: Limits::default(),
        }
    }
}

/// Context handed to [`display_text`].
#[derive(Debug, Clone, Copy)]
pub struct DisplayCtx {
    /// 1904 date system.
    pub date1904: bool,
    /// ods: days from 1970-01-01 to the sheet's null date (see `numfmt::Options::null_day`).
    pub null_day: Option<i64>,
    /// Locale.
    pub locale: Locale,
}

/// The text shown in the grid for a value with a number format: what Excel would display.
///
/// A built-in id that has no code (23..=26, 59+) falls back to `General`. An ISO date/duration
/// that cannot be converted to a serial is shown as the file wrote it.
#[cfg(test)]
pub fn display_text(value: &CellValue, fmt: &NumFmtRef, ctx: &DisplayCtx) -> String {
    display_compiled(value, &compile_fmt(fmt, ctx.locale), ctx)
}

/// The format code of a reference, parsed once.
fn compile_fmt(fmt: &NumFmtRef, locale: Locale) -> numfmt::Compiled {
    let code: &str = match fmt {
        NumFmtRef::General => "General",
        NumFmtRef::Builtin(id) => {
            numfmt::builtin_format_code(u32::from(*id), locale).unwrap_or("General")
        }
        NumFmtRef::Custom(c) => c,
    };
    numfmt::compile(code)
}

/// [`display_text`] with the format already compiled: the loader formats millions of cells with
/// a few dozen distinct formats, and parsing the format code dominated the load time.
fn display_compiled(value: &CellValue, code: &numfmt::Compiled, ctx: &DisplayCtx) -> String {
    use numfmt::Value;
    let opts = numfmt::Options {
        date1904: ctx.date1904,
        locale: ctx.locale,
        null_day: ctx.null_day,
    };
    let num = |n: f64| numfmt::format_compiled(code, Value::Number(n), &opts);
    match value {
        CellValue::Int(i) => num(*i as f64),
        CellValue::Number(n) => num(*n),
        CellValue::Text(t) => numfmt::format_compiled(code, Value::Text(t), &opts),
        CellValue::Bool(b) => numfmt::format_compiled(code, Value::Bool(*b), &opts),
        CellValue::Error(e) => numfmt::format_compiled(code, Value::Error(e), &opts),
        CellValue::ErrorText(e) => numfmt::format_compiled(code, Value::Error(e), &opts),
        CellValue::DateTime { serial, .. } => num(*serial),
        CellValue::DateTimeIso(s) => {
            let date = match ctx.null_day {
                Some(nd) => numfmt::iso_datetime_to_ods_serial(s, nd),
                None => numfmt::iso_datetime_to_serial(s),
            };
            match date.or_else(|| numfmt::iso_duration_to_serial(s)) {
                Some(n) => num(n),
                None => s.to_string(),
            }
        }
    }
}

/// Shortest round-trip text of a number (`1`, `0.1`, `1e300`), never `1.0`.
pub(crate) fn number_text(n: f64) -> String {
    if !n.is_finite() {
        return if n.is_nan() {
            "#NUM!".into()
        } else {
            "#DIV/0!".into()
        };
    }
    if n == n.trunc() && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let a = n.abs();
    if !(1e-7..1e21).contains(&a) {
        format!("{n:e}")
    } else {
        format!("{n}")
    }
}

// ---------------------------------------------------------------------------------------------
// loading
// ---------------------------------------------------------------------------------------------

/// What to read of the sheets.
#[derive(Debug, Clone, Copy)]
enum Which {
    /// The cells of one visible sheet (an index past the last is clamped to it); all the others
    /// are only listed.
    One(usize),
    /// Every visible sheet (tests: lets one call inspect a whole file).
    #[cfg(test)]
    All,
}

/// A request to stop a load that is no longer wanted. Checked by the sheet builder at every row,
/// so a superseded load of a huge sheet ends within one row of the request instead of running to
/// its budgets (the result would be thrown away). The readers of ods and xls parse the whole file
/// inside `calamine` when it is opened, where nothing can stop them: for those the app never has
/// more than one load running (see `App::spawn_workbook_job`) and this ends the cell loop.
#[derive(Clone)]
pub struct Cancel(std::sync::Arc<dyn Fn() -> bool + Send + Sync>);

impl Cancel {
    /// Cancelled once `is_cancelled` says so (a counting closure in tests).
    pub fn new(is_cancelled: impl Fn() -> bool + Send + Sync + 'static) -> Cancel {
        Cancel(std::sync::Arc::new(is_cancelled))
    }

    /// Cancelled as soon as `latest` (the newest generation anyone asked for) is not `mine`.
    pub fn generation(latest: std::sync::Arc<std::sync::atomic::AtomicU64>, mine: u64) -> Cancel {
        Cancel::new(move || latest.load(std::sync::atomic::Ordering::Relaxed) != mine)
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        (self.0)()
    }
}

/// Loads a workbook: the list of its visible sheets, and the cells of the one at `sheet` (an index
/// past the last visible sheet is clamped to it). Never panics: a panic inside a reader is caught
/// and reported as [`OfficeError::Corrupt`] (for the sheet alone when the workbook itself opened).
///
/// A file that cannot be opened at all is an `Err`. A workbook that opens but whose requested
/// sheet cannot be read is an `Ok` with [`Workbook::sheet_error`] set, so the other sheets stay
/// reachable.
#[cfg(test)]
pub fn load_workbook_sheet(
    path: &Path,
    opts: &LoadOptions,
    sheet: usize,
) -> Result<Workbook, OfficeError> {
    load_workbook_sheet_cancellable(path, opts, sheet, None)
}

/// [`load_workbook_sheet`] that gives up when `cancel` says so. A cancelled load returns a
/// partial workbook (the caller is discarding it, so what is in it does not matter).
pub fn load_workbook_sheet_cancellable(
    path: &Path,
    opts: &LoadOptions,
    sheet: usize,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    load_guarded(path, opts, Which::One(sheet), cancel)
}

/// [`load_workbook_sheet`] for every visible sheet (each within its own budgets). A sheet that
/// cannot be read makes the whole call an `Err`.
#[cfg(test)]
pub fn load_workbook(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    load_guarded(path, opts, Which::All, None)
}

/// [`load_workbook`] without the outer panic net (tests: a panic must fail the test). A panic inside
/// one sheet is still caught per sheet and comes back as a `Corrupt` whose text says it crashed.
#[cfg(test)]
pub(crate) fn load_workbook_unguarded(
    path: &Path,
    opts: &LoadOptions,
) -> Result<Workbook, OfficeError> {
    load_inner(path, opts, Which::All, None)
}

fn load_guarded(
    path: &Path,
    opts: &LoadOptions,
    which: Which,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    if SheetKind::from_path(path).is_none() {
        return Err(OfficeError::Unsupported);
    }
    crate::preview::markdown::catch_silent(|| load_inner(path, opts, which, cancel)).unwrap_or_else(
        || {
            Err(OfficeError::Corrupt(
                "the reader crashed on this file".into(),
            ))
        },
    )
}

fn load_inner(
    path: &Path,
    opts: &LoadOptions,
    which: Which,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    let limits = &opts.limits;
    // The container decides the format, not the extension.
    let detected = container::inspect(path, limits)?;
    match detected {
        Detected::Xlsx => load_xlsx(path, opts, which, cancel),
        Detected::Xlsb => load_xlsb(path, opts, which, cancel),
        Detected::Ods => load_ods(path, opts, which, cancel),
        Detected::Xls => load_xls(path, opts, which, cancel),
    }
}

fn open_reader(path: &Path) -> Result<BufReader<File>, OfficeError> {
    Ok(BufReader::new(File::open(path).map_err(container::io_err)?))
}

/// xlsx / xlsm / xltx / xltm, read by konoma itself (`xlsx.rs`). **Streamed**: the workbook parts,
/// the style table and the shared strings are read when the file is opened (a shared string table
/// too large is refused there), then the requested sheet is read once, cell by cell, giving the
/// value, the format and the formula together, until a budget of [`Limits`] is reached. Sheets that
/// are not asked for are not read at all.
fn load_xlsx(
    path: &Path,
    opts: &LoadOptions,
    which: Which,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    let limits = &opts.limits;
    let mut book = xlsx::open(path, limits)?;
    let metas = book
        .sheets
        .iter()
        .map(|s| Meta {
            name: s.name.clone(),
            visible: match s.visible {
                Visibility::Visible => SheetVisible::Visible,
                Visibility::Hidden => SheetVisible::Hidden,
                Visibility::VeryHidden => SheetVisible::VeryHidden,
            },
            typ: SheetType::WorkSheet,
        })
        .collect();
    let ctx = Ctx::new(book.date1904, book.formats.clone(), *opts);
    assemble(&ctx, metas, which, cancel.as_ref(), |name| {
        let mut b = SheetBuilder::new(&ctx, name, None, cancel.as_ref());
        if let Some(part) = book.part_of(name).map(str::to_string) {
            let merges = book.read_sheet(&part, limits, cancel.as_ref(), |c| {
                b.push_xlsx(c) == Flow::Continue
            })?;
            b.set_merges(&merges);
        }
        Ok(b.finish())
    })
}

/// xlsb. Streamed like [`load_xlsx`]; `calamine` has no one-pass reader for values and formulas
/// here, so the sheet is read a second time for the formulas of the rows that were kept.
fn load_xlsb(
    path: &Path,
    opts: &LoadOptions,
    which: Which,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    let limits = &opts.limits;
    let pkg = fmt_xlsb::read_package(path, limits)?;
    let mut wb: calamine::Xlsb<_> = calamine::Xlsb::new(open_reader(path)?).map_err(map_xlsb)?;
    let metas = sheet_metas(&wb);
    let ctx = Ctx::new(pkg.date1904, pkg.formats.clone(), *opts);
    assemble(&ctx, metas, which, cancel.as_ref(), |name| {
        let sf = match pkg.part_of(name) {
            Some(part) => fmt_xlsb::read_sheet(path, part, &pkg.xf_to_format, limits)?,
            None => SheetFormats::default(),
        };
        let mut b = SheetBuilder::new(&ctx, name, Some(&sf), cancel.as_ref());
        {
            let mut rdr = wb.worksheet_cells_reader(name).map_err(map_xlsb)?;
            while let Some(c) = rdr.next_cell().map_err(map_xlsb)? {
                let (row, col) = c.get_position();
                if b.push_cell(row, col, value_from_ref(c.get_value())) == Flow::Stop {
                    break;
                }
            }
        }
        if !b.stopped_by_row_cap() {
            b.formulas_follow_values();
            let mut rdr = wb.worksheet_cells_reader(name).map_err(map_xlsb)?;
            while let Some(c) = rdr.next_formula().map_err(map_xlsb)? {
                let (row, col) = c.get_position();
                if b.push_formula(row, col, c.get_value()) == Flow::Stop {
                    break;
                }
            }
        }
        Ok(b.finish())
    })
}

/// ods. **Not streamable**: `calamine` parses every table (hidden ones too) into a dense matrix
/// when it opens the file, so `fmt_ods::read` checks the size of every table *first* and a file
/// that is too large is refused with a reason. Showing another sheet re-opens the file and takes
/// that sheet out of the parsed workbook.
fn load_ods(
    path: &Path,
    opts: &LoadOptions,
    which: Which,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    let (fm, extra) = fmt_ods::read_full(path, &opts.limits)?;
    let mut wb: calamine::Ods<_> = calamine::Ods::new(open_reader(path)?).map_err(map_ods)?;
    let metas = sheet_metas(&wb);
    let mut ctx = Ctx::new(fm.date1904, fm.formats.clone(), *opts);
    // ods dates run from the table's null date and have no 1900 leap-year bug.
    ctx.null_day = Some(extra.null_day);
    assemble(&ctx, metas, which, cancel.as_ref(), |name| {
        let mut data = wb.worksheet_range(name).map_err(map_ods)?;
        let ods = extra.sheets.get(name);
        if let Some(o) = ods {
            apply_ods_texts(&mut data, &o.texts);
        }
        let mut sheet = build_sheet(
            &ctx,
            name,
            Fetched {
                data,
                formulas: wb.worksheet_formula(name).unwrap_or_default(),
                merges: Vec::new(),
                fmts: fm.sheets.get(name),
                cancel: cancel.as_ref(),
            },
        );
        if let Some(o) = ods {
            sheet.merges = o.merges.clone();
        }
        Ok(sheet)
    })
}

/// Puts back the text `calamine` loses from an ods string cell (`<text:tab/>`, `<text:line-break/>`).
fn apply_ods_texts(data: &mut Range<Data>, runs: &[fmt_ods::TextRun]) {
    let (Some(start), Some(end)) = (data.start(), data.end()) else {
        return;
    };
    for run in runs {
        for r in run.row0..run.row0.saturating_add(run.rows) {
            for c in run.col0..run.col0.saturating_add(run.cols) {
                // Only cells the reader has: never grows the matrix.
                if r < start.0 || r > end.0 || c < start.1 || c > end.1 {
                    continue;
                }
                if matches!(data.get_value((r, c)), Some(Data::String(_))) {
                    data.set_value((r, c), Data::String(run.text.clone()));
                }
            }
        }
    }
}

/// xls. Not streamable, like [`load_ods`]: `fmt_xls::read` checks the size of every sheet first.
fn load_xls(
    path: &Path,
    opts: &LoadOptions,
    which: Which,
    cancel: Option<Cancel>,
) -> Result<Workbook, OfficeError> {
    let fm = fmt_xls::read(path, &opts.limits)?;
    let mut wb: calamine::Xls<_> = calamine::Xls::new(open_reader(path)?).map_err(map_xls)?;
    let metas = sheet_metas(&wb);
    let ctx = Ctx::new(fm.date1904, fm.formats.clone(), *opts);
    assemble(&ctx, metas, which, cancel.as_ref(), |name| {
        Ok(build_sheet(
            &ctx,
            name,
            Fetched {
                data: wb.worksheet_range(name).map_err(map_xls)?,
                formulas: wb.worksheet_formula(name).unwrap_or_default(),
                merges: wb.merge_cells_by_sheet_name(name).unwrap_or_default(),
                fmts: fm.sheets.get(name),
                cancel: cancel.as_ref(),
            },
        ))
    })
}

fn map_xlsb(e: calamine::XlsbError) -> OfficeError {
    match e {
        calamine::XlsbError::Password => OfficeError::Encrypted,
        calamine::XlsbError::Zip(z) => container::zip_err(z),
        other => OfficeError::Corrupt(format!("xlsb: {other}")),
    }
}
fn map_ods(e: calamine::OdsError) -> OfficeError {
    match e {
        calamine::OdsError::Password => OfficeError::Encrypted,
        calamine::OdsError::Zip(z) => container::zip_err(z),
        other => OfficeError::Corrupt(format!("ods: {other}")),
    }
}
fn map_xls(e: calamine::XlsError) -> OfficeError {
    match e {
        calamine::XlsError::Password => OfficeError::Encrypted,
        other => OfficeError::Corrupt(format!("xls: {other}")),
    }
}

struct Ctx {
    date1904: bool,
    /// ods only: days from 1970-01-01 to the table's null date (`None` for the other formats).
    null_day: Option<i64>,
    formats: Vec<NumFmtRef>,
    /// `formats`, each parsed once (same indices).
    compiled: Vec<numfmt::Compiled>,
    /// For a format index the table does not have.
    general: numfmt::Compiled,
    opts: LoadOptions,
}

impl Ctx {
    fn new(date1904: bool, formats: Vec<NumFmtRef>, opts: LoadOptions) -> Ctx {
        let compiled = formats
            .iter()
            .map(|f| compile_fmt(f, opts.locale))
            .collect();
        Ctx {
            date1904,
            null_day: None,
            formats,
            compiled,
            general: compile_fmt(&NumFmtRef::General, opts.locale),
            opts,
        }
    }
}

struct Meta {
    name: String,
    visible: SheetVisible,
    typ: SheetType,
}

fn sheet_metas<RS, R: Reader<RS>>(wb: &R) -> Vec<Meta>
where
    RS: std::io::Read + std::io::Seek,
{
    wb.sheets_metadata()
        .iter()
        .map(|s| Meta {
            name: s.name.clone(),
            visible: s.visible,
            typ: s.typ,
        })
        .collect()
}

/// What a dense reader (ods / xls) returns for one sheet.
struct Fetched<'a> {
    data: Range<Data>,
    formulas: Range<String>,
    merges: Vec<Dimensions>,
    fmts: Option<&'a SheetFormats>,
    cancel: Option<&'a Cancel>,
}

/// Walks the sheet list in workbook order: visible worksheets are listed by name (and loaded when
/// `which` asks for them), hidden ones are only counted. Chart/dialog/macro sheets carry no cells
/// and are skipped without being counted as hidden.
fn assemble(
    ctx: &Ctx,
    metas: Vec<Meta>,
    which: Which,
    cancel: Option<&Cancel>,
    mut load: impl FnMut(&str) -> Result<Sheet, OfficeError>,
) -> Result<Workbook, OfficeError> {
    let mut out = Workbook {
        sheets: Vec::new(),
        hidden_sheets: 0,
        date1904: ctx.date1904,
        formats: ctx.formats.clone(),
        sheet_error: None,
    };
    for m in metas {
        if m.typ != SheetType::WorkSheet {
            continue;
        }
        if m.visible != SheetVisible::Visible {
            out.hidden_sheets += 1;
            continue;
        }
        out.sheets.push(Sheet {
            name: m.name,
            ..Sheet::default()
        });
    }
    let wanted: Vec<usize> = match which {
        Which::One(_) if out.sheets.is_empty() => Vec::new(),
        Which::One(i) => vec![i.min(out.sheets.len() - 1)],
        #[cfg(test)]
        Which::All => (0..out.sheets.len()).collect(),
    };
    for i in wanted {
        // A load nobody waits for any more does not even start reading the sheet.
        if cancel.is_some_and(Cancel::is_cancelled) {
            break;
        }
        let name = out.sheets[i].name.clone();
        // A panic on a pathological sheet fails that sheet, not the workbook.
        let loaded = crate::preview::markdown::catch_silent(|| load(&name)).unwrap_or_else(|| {
            Err(OfficeError::Corrupt(
                "the reader crashed on this sheet".into(),
            ))
        });
        match (loaded, which) {
            (Ok(sheet), _) => out.sheets[i] = sheet,
            #[cfg(test)]
            (Err(e), Which::All) => return Err(e),
            (Err(e), _) => out.sheet_error = Some(e),
        }
    }
    Ok(out)
}

/// Builds a sheet from a dense range (ods / xls).
fn build_sheet(ctx: &Ctx, name: &str, f: Fetched<'_>) -> Sheet {
    let mut b = SheetBuilder::new(ctx, name, f.fmts, f.cancel);
    b.set_merges(
        &f.merges
            .iter()
            .map(|d| MergeRange {
                row0: d.start.0,
                col0: d.start.1,
                row1: d.end.0,
                col1: d.end.1,
            })
            .collect::<Vec<_>>(),
    );
    if let Some((r0, c0)) = f.data.start() {
        'rows: for (i, row) in f.data.rows().enumerate() {
            for (j, v) in row.iter().enumerate() {
                if b.push_cell(r0 + i as u32, c0 + j as u32, to_value(v)) == Flow::Stop {
                    break 'rows;
                }
            }
        }
    }
    if let Some((fr0, fc0)) = f.formulas.start() {
        'frows: for (i, row) in f.formulas.rows().enumerate() {
            for (j, formula) in row.iter().enumerate() {
                if b.push_formula(fr0 + i as u32, fc0 + j as u32, formula) == Flow::Stop {
                    break 'frows;
                }
            }
        }
    }
    b.finish()
}

/// Whether the reader should go on after a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    /// A limit was reached: nothing more is read.
    Stop,
}

/// Longest display string kept per cell, in characters. A formatted number or date is a few
/// dozen characters at most; the only way to get more is a format code that repeats a long
/// pattern (`mmmm` x 60 = 570 bytes for every cell of the column), so the text is cut here and
/// not left to cost memory in proportion to the cell count.
const MAX_DISPLAY_CHARS: usize = 1024;

/// Bytes charged to a sheet's text budget for each stored string on top of its length: the
/// allocation overhead of a `Box<str>` / formula entry. Not exact — a budget needs a proportional,
/// not a precise, measure.
const STRING_COST: u64 = 32;

/// Accumulates one sheet within its budgets and then yields the [`Sheet`].
///
/// The three budgets (all in [`Limits`], all per sheet): `max_rows`, `max_sheet_cells` (cells kept)
/// and `max_sheet_text_bytes` (the strings kept: text values, displayed strings, formulas). When
/// one is reached the sheet is marked truncated and [`Flow::Stop`] tells the reader to stop —
/// nothing past the limit is allocated, whatever the file says about its size.
struct SheetBuilder<'a> {
    ctx: &'a Ctx,
    dctx: DisplayCtx,
    fmts: Option<&'a SheetFormats>,
    sheet: Sheet,
    rows: Vec<Vec<(u32, Cell)>>,
    /// Cells kept.
    kept: u64,
    /// Text bytes charged.
    text: u64,
    /// Largest row / column index kept (value or formula).
    max_row: Option<u32>,
    max_col: Option<u32>,
    /// Stopped at a row past the row cap.
    row_cap_hit: bool,
    /// Set when the formulas are read in a second pass after a values pass that was cut short
    /// (xlsb): the last row the values reached (`None` = none), past which formulas are dropped.
    formula_row_limit: Option<Option<u32>>,
    /// Checked whenever a cell opens a new row; once it says so, nothing more is read.
    cancel: Option<&'a Cancel>,
    last_row: Option<u32>,
}

impl<'a> SheetBuilder<'a> {
    fn new(
        ctx: &'a Ctx,
        name: &str,
        fmts: Option<&'a SheetFormats>,
        cancel: Option<&'a Cancel>,
    ) -> Self {
        SheetBuilder {
            ctx,
            dctx: DisplayCtx {
                date1904: ctx.date1904,
                null_day: ctx.null_day,
                locale: ctx.opts.locale,
            },
            fmts,
            sheet: Sheet {
                name: name.to_string(),
                loaded: true,
                // The format pass stopped before the end of the sheet (the value reader was
                // stopped at the same cell).
                rows_truncated: fmts.is_some_and(|f| f.truncated),
                ..Sheet::default()
            },
            rows: Vec::new(),
            kept: 0,
            text: 0,
            max_row: None,
            max_col: None,
            row_cap_hit: false,
            formula_row_limit: None,
            cancel,
            last_row: None,
        }
    }

    /// For a second pass that reads formulas after the values: if the values were cut short, the
    /// formulas stop at the last row they reached.
    fn formulas_follow_values(&mut self) {
        if self.sheet.rows_truncated {
            self.formula_row_limit = Some(self.max_row);
        }
    }

    fn set_merges(&mut self, merges: &[MergeRange]) {
        self.sheet.merges = merges.to_vec();
    }

    /// Whether reading stopped at the row cap (nothing after it can be kept either).
    fn stopped_by_row_cap(&self) -> bool {
        self.row_cap_hit
    }

    /// Where a cell at `(row, col)` may go: `Stop` past the row cap, `Continue` (skip) past the
    /// column cap, `None` when it is in the grid.
    fn gate(&mut self, row: u32, col: u32) -> Option<Flow> {
        let limits = &self.ctx.opts.limits;
        if row as usize >= limits.max_rows {
            self.row_cap_hit = true;
            self.sheet.rows_truncated = true;
            return Some(Flow::Stop);
        }
        if col as usize >= limits.max_cols {
            self.sheet.cols_truncated = true;
            return Some(Flow::Continue);
        }
        None
    }

    fn note_extent(&mut self, row: u32, col: u32) {
        self.max_row = Some(self.max_row.map_or(row, |m| m.max(row)));
        self.max_col = Some(self.max_col.map_or(col, |m| m.max(col)));
    }

    /// Keeps a value (`None` = an empty cell: nothing to keep).
    fn push_cell(&mut self, row: u32, col: u32, value: Option<CellValue>) -> Flow {
        self.push(row, col, value, None)
    }

    /// Keeps a cell of an xlsx sheet: its value, formula and number format come together.
    fn push_xlsx(&mut self, c: xlsx::CellOut<'_>) -> Flow {
        let value = c.value.map(|v| match v {
            Val::Number(n) => CellValue::Number(n),
            Val::Date { serial, duration } => CellValue::DateTime { serial, duration },
            Val::Text(t) => CellValue::Text(t.into()),
            Val::Bool(b) => CellValue::Bool(b),
            Val::Error(e) => error_value(e),
            Val::Iso(s) => CellValue::DateTimeIso(s.into()),
        });
        self.push_with(c.row, c.col, Some(c.fmt), value, c.formula)
    }

    /// Keeps a formula (without the leading `=`; an empty one is nothing).
    fn push_formula(&mut self, row: u32, col: u32, formula: &str) -> Flow {
        // Rows after the point where the values stopped are not in the sheet.
        if let Some(limit) = self.formula_row_limit {
            if limit.is_none_or(|m| row > m) {
                return Flow::Stop;
            }
        }
        self.push(row, col, None, Some(formula))
    }

    /// Keeps what a cell holds — its value and/or its formula — **as one unit**: both fit the
    /// budgets or neither is kept (a cut never leaves a value without its formula).
    fn push(
        &mut self,
        row: u32,
        col: u32,
        value: Option<CellValue>,
        formula: Option<&str>,
    ) -> Flow {
        self.push_with(row, col, None, value, formula)
    }

    /// [`SheetBuilder::push`] with the number format given by the reader (`Some`) instead of
    /// looked up in the format pass of the sheet.
    fn push_with(
        &mut self,
        row: u32,
        col: u32,
        fmt: Option<u16>,
        value: Option<CellValue>,
        formula: Option<&str>,
    ) -> Flow {
        // One check per row: an atomic load is cheap, and a row is the unit a huge sheet is made of.
        if self.last_row != Some(row) {
            self.last_row = Some(row);
            if self.cancel.is_some_and(Cancel::is_cancelled) {
                return Flow::Stop;
            }
        }
        // An empty cell is nothing wherever it is: one past the row or column cap does not cut
        // anything off (a dense matrix and a sheet's formatted empty cells reach there too).
        let formula = formula.filter(|f| !f.is_empty());
        if value.is_none() && formula.is_none() {
            return Flow::Continue;
        }
        if let Some(flow) = self.gate(row, col) {
            return flow;
        }
        let cell = value.map(|v| self.make_cell(row, col, fmt, v));
        if cell.is_some() && self.kept >= self.ctx.opts.limits.max_sheet_cells {
            self.sheet.rows_truncated = true;
            return Flow::Stop;
        }
        let cost = cell.as_ref().map_or(0, |(_, c)| *c)
            + formula.map_or(0, |f| f.len() as u64 + STRING_COST);
        if !self.charge(cost) {
            return Flow::Stop;
        }
        self.note_extent(row, col);
        if let Some((cell, _)) = cell {
            self.kept += 1;
            let r = row as usize;
            if self.rows.len() <= r {
                self.rows.resize_with(r + 1, Vec::new);
            }
            self.rows[r].push((col, cell));
        }
        if let Some(f) = formula {
            self.sheet.formulas.insert((row, col), f.into());
        }
        Flow::Continue
    }

    /// The cell for a value, and the text bytes it costs.
    fn make_cell(&self, row: u32, col: u32, fmt: Option<u16>, mut value: CellValue) -> (Cell, u64) {
        // ods: an error cell reaches us as an empty string; the format pass knows better.
        if let Some(code) = self.fmts.and_then(|s| s.error_at(row, col)) {
            value = CellValue::Error(code);
        }
        let fmt = fmt.unwrap_or_else(|| self.fmts.map_or(0, |s| s.format_at(row, col)));
        let code = self
            .ctx
            .compiled
            .get(usize::from(fmt))
            .unwrap_or(&self.ctx.general);
        // A text cell under `General` (the common case: format 0) is shown as it is written.
        let display = if fmt == 0 && matches!(value, CellValue::Text(_)) {
            None
        } else {
            let mut shown = display_compiled(&value, code, &self.dctx);
            match &value {
                // A text cell shown as it is written keeps no second copy (and is not cut: it is
                // the value).
                CellValue::Text(t) if **t == *shown => None,
                _ => {
                    clip_display(&mut shown);
                    Some(shown.into_boxed_str())
                }
            }
        };
        let cost =
            value_text_len(&value) + display.as_ref().map_or(0, |d| d.len() as u64 + STRING_COST);
        (
            Cell {
                value,
                display,
                fmt,
            },
            cost,
        )
    }

    /// Charges `cost` bytes to the text budget; false (and the sheet marked truncated) when it
    /// does not fit.
    fn charge(&mut self, cost: u64) -> bool {
        let next = self.text.saturating_add(cost);
        if next > self.ctx.opts.limits.max_sheet_text_bytes {
            self.sheet.rows_truncated = true;
            return false;
        }
        self.text = next;
        true
    }

    fn finish(mut self) -> Sheet {
        let limits = &self.ctx.opts.limits;
        // The grid runs from A1 to the last used row / column, up to the caps.
        // (A sheet cut at the row cap with a gap before it shows its last row of data, not a
        // hundred thousand empty rows: the cut is told by `rows_truncated`.)
        let nrows = self.max_row.map_or(0, |r| r as usize + 1);
        let ncols = if self.sheet.cols_truncated {
            limits.max_cols
        } else {
            self.max_col.map_or(0, |c| c as usize + 1)
        };
        self.rows.resize_with(nrows, Vec::new);
        for row in &mut self.rows {
            // Cells arrive in file order, which is column order in any real file; a producer
            // that is not keeps its cells reachable by the binary search all the same.
            if !row.windows(2).all(|w| w[0].0 < w[1].0) {
                row.sort_by_key(|&(c, _)| c);
                // The last of two cells at one address wins (as in a dense matrix; the sort is
                // stable, so "last" is file order). One pass: removing from the middle one
                // duplicate at a time is quadratic in a row of many cells at one address.
                let mut w = 0;
                for i in 0..row.len() {
                    if i + 1 < row.len() && row[i].0 == row[i + 1].0 {
                        continue;
                    }
                    row.swap(w, i);
                    w += 1;
                }
                row.truncate(w);
            }
        }
        self.sheet.nrows = nrows;
        self.sheet.ncols = ncols;
        self.sheet.rows = self.rows;
        self.sheet
    }
}

/// Cuts a displayed string at [`MAX_DISPLAY_CHARS`] characters.
fn clip_display(s: &mut String) {
    if s.len() <= MAX_DISPLAY_CHARS {
        return;
    }
    if let Some((i, _)) = s.char_indices().nth(MAX_DISPLAY_CHARS) {
        s.truncate(i);
    }
}

/// The bytes a value's own text takes (text and ISO strings), plus the per-string overhead.
fn value_text_len(v: &CellValue) -> u64 {
    match v {
        CellValue::Text(t) | CellValue::DateTimeIso(t) | CellValue::ErrorText(t) => {
            t.len() as u64 + STRING_COST
        }
        _ => 0,
    }
}

fn to_value(d: &Data) -> Option<CellValue> {
    Some(match d {
        Data::Empty => return None,
        Data::Int(i) => CellValue::Int(*i),
        Data::Float(f) => CellValue::Number(*f),
        Data::String(s) => CellValue::Text(s.as_str().into()),
        Data::Bool(b) => CellValue::Bool(*b),
        Data::DateTime(dt) => CellValue::DateTime {
            serial: dt.as_f64(),
            duration: dt.is_duration(),
        },
        Data::DateTimeIso(s) => CellValue::DateTimeIso(s.as_str().into()),
        // An ISO 8601 duration (ODS `PT1H30M`): shown as the string, typed as date/time.
        Data::DurationIso(s) => CellValue::DateTimeIso(s.as_str().into()),
        Data::Error(e) => CellValue::Error(error_code(e)),
    })
}

/// [`to_value`] for the streaming readers' borrowing cell type.
fn value_from_ref(d: &calamine::DataRef<'_>) -> Option<CellValue> {
    use calamine::DataRef as D;
    Some(match d {
        D::Empty => return None,
        D::Int(i) => CellValue::Int(*i),
        D::Float(f) => CellValue::Number(*f),
        D::String(s) => CellValue::Text(s.as_str().into()),
        D::SharedString(s) => CellValue::Text((*s).into()),
        D::Bool(b) => CellValue::Bool(*b),
        D::DateTime(dt) => CellValue::DateTime {
            serial: dt.as_f64(),
            duration: dt.is_duration(),
        },
        D::DateTimeIso(s) | D::DurationIso(s) => CellValue::DateTimeIso(s.as_str().into()),
        D::Error(e) => CellValue::Error(error_code(e)),
    })
}

/// The value of an error cell of an xlsx sheet: one of the fixed codes, or the text as written.
fn error_value(code: &str) -> CellValue {
    const FIXED: [&str; 7] = [
        "#DIV/0!", "#N/A", "#NAME?", "#NULL!", "#NUM!", "#REF!", "#VALUE!",
    ];
    match FIXED.iter().find(|f| **f == code) {
        Some(f) => CellValue::Error(f),
        None => CellValue::ErrorText(code.into()),
    }
}

fn error_code(e: &calamine::CellErrorType) -> CellError {
    use calamine::CellErrorType as E;
    match e {
        E::Div0 => "#DIV/0!",
        E::NA => "#N/A",
        E::Name => "#NAME?",
        E::Null => "#NULL!",
        E::Num => "#NUM!",
        E::Ref => "#REF!",
        E::Value => "#VALUE!",
        E::GettingData => "#DATA!",
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests of the loader's own logic, below the readers: grids are built from calamine
    //! ranges directly, so each rule (sheet kinds, caps, formula cut, display text sharing, error
    //! codes) is pinned without a file.
    use super::*;
    use calamine::{Cell as RCell, CellErrorType};

    fn limits(max_rows: usize, max_cols: usize) -> Limits {
        Limits {
            max_rows,
            max_cols,
            ..Limits::default()
        }
    }

    fn ctx(limits: Limits) -> Ctx {
        Ctx::new(
            false,
            vec![NumFmtRef::General],
            LoadOptions {
                locale: Locale::En,
                limits,
            },
        )
    }

    fn range<T: calamine::CellType>(cells: Vec<(u32, u32, T)>) -> Range<T> {
        Range::from_sparse(
            cells
                .into_iter()
                .map(|(r, c, v)| RCell::new((r, c), v))
                .collect(),
        )
    }

    fn fetched(data: Range<Data>, formulas: Range<String>) -> Fetched<'static> {
        Fetched {
            data,
            formulas,
            merges: Vec::new(),
            fmts: None,
            cancel: None,
        }
    }

    fn build(limits: Limits, f: Fetched<'static>) -> Sheet {
        build_sheet(&ctx(limits), "S", f)
    }

    /// An `r x c` grid of integers.
    fn ints(r: u32, c: u32) -> Range<Data> {
        range(
            (0..r)
                .flat_map(|i| (0..c).map(move |j| (i, j, Data::Int(i64::from(i * 10 + j)))))
                .collect(),
        )
    }

    #[test]
    fn only_worksheets_are_listed_and_only_hidden_worksheets_are_counted() {
        let meta = |name: &str, typ: SheetType, visible: SheetVisible| Meta {
            name: name.into(),
            visible,
            typ,
        };
        let metas = vec![
            meta("A", SheetType::WorkSheet, SheetVisible::Visible),
            meta("Chart", SheetType::ChartSheet, SheetVisible::Visible),
            meta("Dialog", SheetType::DialogSheet, SheetVisible::Visible),
            meta("Macro", SheetType::MacroSheet, SheetVisible::Visible),
            meta("Vba", SheetType::Vba, SheetVisible::Visible),
            meta("HiddenChart", SheetType::ChartSheet, SheetVisible::Hidden),
            meta("Hidden", SheetType::WorkSheet, SheetVisible::Hidden),
            meta("VeryHidden", SheetType::WorkSheet, SheetVisible::VeryHidden),
            meta("B", SheetType::WorkSheet, SheetVisible::Visible),
        ];
        let mut fetched_names = Vec::new();
        let c = ctx(Limits::default());
        let wb = assemble(&c, metas, Which::All, None, |name| {
            fetched_names.push(name.to_string());
            Ok(build_sheet(&c, name, fetched(ints(1, 1), Range::empty())))
        })
        .unwrap();
        let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        assert_eq!(
            wb.hidden_sheets, 2,
            "chart / dialog / macro sheets are not hidden worksheets"
        );
        // The reader is never asked for a sheet that is not a visible worksheet.
        assert_eq!(fetched_names, ["A", "B"]);
    }

    #[test]
    fn columns_are_truncated_only_past_the_cap() {
        let at =
            |cols: u32, cap: usize| build(limits(100, cap), fetched(ints(2, cols), Range::empty()));
        let s = at(4, 4);
        assert_eq!(s.ncols, 4);
        assert!(!s.cols_truncated, "exactly at the cap");
        let s = at(5, 4);
        assert_eq!(s.ncols, 4);
        assert!(s.cols_truncated);
        let s = at(3, 4);
        assert_eq!(s.ncols, 3);
        assert!(!s.cols_truncated);
        assert!(s.cell(0, 3).is_none());
        // Rows behave the same way.
        let s = build(limits(2, 100), fetched(ints(2, 2), Range::empty()));
        assert_eq!(s.nrows, 2);
        assert!(!s.rows_truncated, "exactly at the row cap");
        let s = build(limits(2, 100), fetched(ints(3, 2), Range::empty()));
        assert_eq!(s.nrows, 2);
        assert!(s.rows_truncated);
    }

    #[test]
    fn formulas_past_the_grid_are_not_kept() {
        let formulas: Range<String> = range(
            (0..3u32)
                .flat_map(|r| (0..3u32).map(move |c| (r, c, format!("F{r}{c}"))))
                .collect(),
        );
        let s = build(limits(2, 2), fetched(ints(3, 3), formulas));
        assert_eq!((s.nrows, s.ncols), (2, 2));
        assert_eq!(s.formula(1, 1), Some("F11"));
        assert_eq!(s.formula(1, 2), None, "first column past the grid");
        assert_eq!(s.formula(2, 1), None, "first row past the grid");
        assert_eq!(s.formulas.len(), 4);
    }

    #[test]
    fn a_text_cell_shares_its_text_and_other_cells_store_their_display() {
        let data = range(vec![
            (0, 0, Data::String("hello".into())),
            (0, 1, Data::String("123".into())),
            (0, 2, Data::Float(1.5)),
            (0, 3, Data::Bool(true)),
            (0, 4, Data::Int(7)),
        ]);
        let s = build(Limits::default(), fetched(data, Range::empty()));
        // A text cell shown exactly as written keeps no second copy (the memory budget relies on it).
        assert!(s.cell(0, 0).unwrap().display.is_none());
        assert!(s.cell(0, 1).unwrap().display.is_none());
        assert_eq!(s.display(0, 0), "hello");
        assert_eq!(s.display(0, 1), "123");
        // Everything else keeps what the format made of it.
        assert_eq!(s.cell(0, 2).unwrap().display.as_deref(), Some("1.5"));
        assert_eq!(s.cell(0, 3).unwrap().display.as_deref(), Some("TRUE"));
        assert_eq!(s.cell(0, 4).unwrap().display.as_deref(), Some("7"));
    }

    #[test]
    fn every_error_value_has_its_excel_code() {
        use CellErrorType as E;
        let all = [
            (E::Div0, "#DIV/0!"),
            (E::NA, "#N/A"),
            (E::Name, "#NAME?"),
            (E::Null, "#NULL!"),
            (E::Num, "#NUM!"),
            (E::Ref, "#REF!"),
            (E::Value, "#VALUE!"),
            // "#GETTING_DATA" in Excel's own list; the engine shows its short form.
            (E::GettingData, "#DATA!"),
        ];
        for (e, code) in all {
            assert_eq!(
                to_value(&Data::Error(e.clone())),
                Some(CellValue::Error(code))
            );
            assert_eq!(error_code(&e), code);
        }
        assert_eq!(to_value(&Data::Empty), None);
    }

    #[test]
    fn raw_text_is_the_unformatted_value() {
        let t = |v: CellValue| {
            Cell {
                value: v,
                display: None,
                fmt: 0,
            }
            .raw_text()
        };
        assert_eq!(t(CellValue::Bool(true)), "TRUE");
        assert_eq!(t(CellValue::Bool(false)), "FALSE");
        assert_eq!(t(CellValue::Int(-4)), "-4");
        assert_eq!(t(CellValue::Number(0.25)), "0.25");
        assert_eq!(t(CellValue::Text("a b".into())), "a b");
        assert_eq!(t(CellValue::Error("#N/A")), "#N/A");
        assert_eq!(
            t(CellValue::DateTime {
                serial: 45292.5,
                duration: false
            }),
            "45292.5"
        );
        assert_eq!(t(CellValue::DateTimeIso("2024-01-01".into())), "2024-01-01");
    }

    /// Many cells at one address in one row (a hostile or sloppy producer) used to be removed one
    /// at a time from the middle of the row: quadratic. A row of 200,000 of them finishes in a
    /// fraction of a second now (the quadratic version moves ~2e10 elements), and the last one
    /// wins, as in a dense matrix.
    #[test]
    fn duplicate_cell_addresses_are_removed_in_linear_time_and_the_last_wins() {
        let c = ctx(Limits::default());
        let mut b = SheetBuilder::new(&c, "S", None, None);
        let n = 200_000i64;
        for i in 0..n {
            assert_eq!(b.push_cell(0, 3, Some(CellValue::Int(i))), Flow::Continue);
        }
        // Interleaved with a second address, so the row is out of order as well as duplicated.
        for i in 0..n {
            b.push_cell(0, 1, Some(CellValue::Int(i)));
            b.push_cell(0, 3, Some(CellValue::Int(n + i)));
        }
        let t = std::time::Instant::now();
        let s = b.finish();
        let took = t.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "{took:?}");
        let row = s.row_cells(0);
        assert_eq!(row.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(row[0].1.value, CellValue::Int(n - 1), "last at column 1");
        assert_eq!(
            row[1].1.value,
            CellValue::Int(2 * n - 1),
            "last at column 3"
        );
    }

    #[test]
    fn dedup_keeps_the_last_of_each_run_and_everything_else_in_order() {
        let c = ctx(Limits::default());
        let mut b = SheetBuilder::new(&c, "S", None, None);
        // Columns 5 (x3), 2 (x2), 9, 2 again later: sorted stably, the last of each address wins.
        for (col, v) in [(5, 0), (5, 1), (2, 2), (9, 3), (2, 4), (5, 5), (7, 6)] {
            b.push_cell(0, col, Some(CellValue::Int(v)));
        }
        let s = b.finish();
        let got: Vec<(u32, CellValue)> = s
            .row_cells(0)
            .iter()
            .map(|(c, cell)| (*c, cell.value.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (2, CellValue::Int(4)),
                (5, CellValue::Int(5)),
                (7, CellValue::Int(6)),
                (9, CellValue::Int(3)),
            ]
        );
    }

    /// An empty cell past the row or column cap cuts nothing off: a dense matrix (ods / xls) and a
    /// sheet's styled blanks reach there too.
    #[test]
    fn empty_cells_past_the_caps_are_not_a_cut() {
        let data = range(vec![
            (0, 0, Data::Int(1)),
            (1, 1, Data::Int(2)),
            // Empty cells outside the grid of 2 x 2.
            (2, 0, Data::Empty),
            (0, 2, Data::Empty),
            (5, 5, Data::Empty),
        ]);
        let s = build(limits(2, 2), fetched(data, Range::empty()));
        assert!(!s.rows_truncated, "an empty row past the cap is no cut");
        assert!(!s.cols_truncated, "an empty column past the cap is no cut");
        assert_eq!((s.nrows, s.ncols), (2, 2));
        // The same cells holding something are.
        let data = range(vec![(0, 0, Data::Int(1)), (2, 0, Data::Int(3))]);
        assert!(build(limits(2, 2), fetched(data, Range::empty())).rows_truncated);
        let data = range(vec![(0, 0, Data::Int(1)), (0, 2, Data::Int(3))]);
        assert!(build(limits(2, 2), fetched(data, Range::empty())).cols_truncated);
    }

    /// The builder asks the cancel check once per row (not per cell), and a load that is
    /// cancelled keeps nothing more.
    #[test]
    fn a_cancelled_load_stops_within_the_row_it_was_cancelled_in() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let checks = Arc::new(AtomicUsize::new(0));
        let seen = checks.clone();
        // Cancelled from the 4th check on.
        let cancel = Cancel::new(move || seen.fetch_add(1, Ordering::SeqCst) >= 3);
        let c = ctx(Limits::default());
        let mut b = SheetBuilder::new(&c, "S", None, Some(&cancel));
        let mut stopped_at = None;
        'rows: for r in 0..100u32 {
            for col in 0..5u32 {
                if b.push_cell(r, col, Some(CellValue::Int(1))) == Flow::Stop {
                    stopped_at = Some((r, col));
                    break 'rows;
                }
            }
        }
        assert_eq!(
            stopped_at,
            Some((3, 0)),
            "stopped at the first cell of row 4"
        );
        assert_eq!(checks.load(Ordering::SeqCst), 4, "one check per row");
        let s = b.finish();
        assert_eq!(s.nrows, 3, "only the three rows before it are kept");
    }

    /// The dense readers (ods / xls) loop over a finished matrix; the same check ends that loop.
    #[test]
    fn a_cancelled_dense_load_keeps_only_the_rows_before_the_cancel() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let checks = Arc::new(AtomicUsize::new(0));
        let seen = checks.clone();
        let cancel = Cancel::new(move || seen.fetch_add(1, Ordering::SeqCst) >= 5);
        let mut f = fetched(ints(50, 4), Range::empty());
        f.cancel = Some(&cancel);
        let s = build_sheet(&ctx(Limits::default()), "S", f);
        assert_eq!(s.nrows, 5);
        assert_eq!(checks.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn a_generation_cancel_fires_only_when_the_latest_generation_moved_on() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        let latest = Arc::new(AtomicU64::new(7));
        let cancel = Cancel::generation(latest.clone(), 7);
        assert!(!cancel.is_cancelled());
        latest.store(8, Ordering::Relaxed);
        assert!(cancel.is_cancelled());
        assert!(!Cancel::generation(latest, 8).is_cancelled());
    }

    #[test]
    fn assemble_does_not_read_a_sheet_once_cancelled() {
        let c = ctx(Limits::default());
        let metas = vec![Meta {
            name: "A".into(),
            visible: SheetVisible::Visible,
            typ: SheetType::WorkSheet,
        }];
        let cancel = Cancel::new(|| true);
        let mut asked = false;
        let wb = assemble(&c, metas, Which::All, Some(&cancel), |_| {
            asked = true;
            Ok(Sheet::default())
        })
        .unwrap();
        assert!(!asked, "the reader is never started");
        assert_eq!(wb.sheets.len(), 1, "the sheet list is still there");
    }

    #[test]
    fn number_text_switches_to_exponent_form_at_1e_minus_7_and_1e21() {
        // Plain decimal from 1e-7 (inclusive) up to 1e21 (exclusive), exponent form outside.
        assert_eq!(number_text(1e-6), "0.000001");
        assert_eq!(number_text(5e-7), "0.0000005");
        assert_eq!(number_text(1e-7), "0.0000001");
        assert_eq!(number_text(9.9e-8), "9.9e-8");
        assert_eq!(number_text(-5e-7), "-0.0000005");
        assert_eq!(number_text(-9.9e-8), "-9.9e-8");
        assert_eq!(number_text(1e21), "1e21");
        assert_eq!(number_text(1.5e21), "1.5e21");
        assert_eq!(
            number_text(123456789012345680000.0),
            "123456789012345680000"
        );
        // Whole numbers below 1e15 print as integers.
        assert_eq!(number_text(999_999_999_999_999.0), "999999999999999");
        assert_eq!(number_text(-0.0), "0");
    }
}
