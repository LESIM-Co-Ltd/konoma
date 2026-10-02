//! The in-memory model of a spreadsheet and the loader that fills it.
//!
//! **What the UI uses**: a grid of *display strings* ([`Sheet::display`]) and, for the cell under
//! the cursor, the *detail* ([`Cell`]: raw value, type, formula, number-format reference).
//!
//! **Memory design** (the cell budget in [`Limits::max_grid_cells`] bounds rows x columns over
//! the *whole workbook*, handed out to the sheets in order):
//! - Rows are sparse: `rows[r]` holds only the non-empty cells as `(col, Cell)` sorted by column,
//!   so an empty cell costs nothing and a mostly-empty wide sheet stays small.
//! - A [`Cell`] is `CellValue` (24 bytes: the tag plus an `f64` / `Box<str>` / `&'static str`),
//!   an optional display string (16 bytes; `None` when it is the same as a text cell's own text,
//!   which is the common case for strings) and a `u16` index into the workbook's deduplicated
//!   format table: ~48 bytes per non-empty cell plus its string heap. Measured (a 4M-cell sheet
//!   of numbers, release build) the whole load costs about 105-115 bytes per non-empty cell
//!   (this model, plus the reader's own dense copy that is alive while a sheet is built), so
//!   4M cells is roughly 450 MB — the worst case the budget allows.
//! - The raw value is *not* stored as a second string: [`Cell::raw_text`] derives it on demand.
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
use super::fmt_xlsx::{self, SheetFormats};
use super::numfmt;
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
            CellValue::Error(_) => CellType::Error,
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

/// One visible sheet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    /// The sheet name.
    pub name: String,
    /// Rows in the grid (from A1; includes leading empty rows). After truncation.
    pub nrows: usize,
    /// Columns in the grid (from A1). After truncation.
    pub ncols: usize,
    /// True when rows were cut off (row cap or cell budget).
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
    /// The visible worksheets, in workbook order.
    pub sheets: Vec<Sheet>,
    /// How many worksheets were hidden or very hidden (not in `sheets`).
    pub hidden_sheets: usize,
    /// The workbook uses the 1904 date system.
    pub date1904: bool,
    /// Deduplicated number formats referenced by [`Cell::fmt`]; index 0 is always `General`.
    pub formats: Vec<NumFmtRef>,
}

impl Workbook {
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
    };
    let num = |n: f64| numfmt::format_compiled(code, Value::Number(n), &opts);
    match value {
        CellValue::Int(i) => num(*i as f64),
        CellValue::Number(n) => num(*n),
        CellValue::Text(t) => numfmt::format_compiled(code, Value::Text(t), &opts),
        CellValue::Bool(b) => numfmt::format_compiled(code, Value::Bool(*b), &opts),
        CellValue::Error(e) => numfmt::format_compiled(code, Value::Error(e), &opts),
        CellValue::DateTime { serial, .. } => num(*serial),
        CellValue::DateTimeIso(s) => {
            match numfmt::iso_datetime_to_serial(s).or_else(|| numfmt::iso_duration_to_serial(s)) {
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

/// Loads a workbook. Never panics: a panic inside a reader is caught and reported as
/// [`OfficeError::Corrupt`].
pub fn load_workbook(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    if SheetKind::from_path(path).is_none() {
        return Err(OfficeError::Unsupported);
    }
    crate::preview::markdown::catch_silent(|| load_inner(path, opts)).unwrap_or_else(|| {
        Err(OfficeError::Corrupt(
            "the reader crashed on this file".into(),
        ))
    })
}

pub(crate) fn load_inner(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    let limits = &opts.limits;
    // The container decides the format, not the extension.
    let detected = container::inspect(path, limits)?;
    match detected {
        Detected::Xlsx => load_xlsx(path, opts),
        Detected::Xlsb => load_xlsb(path, opts),
        Detected::Ods => load_ods(path, opts),
        Detected::Xls => load_xls(path, opts),
    }
}

fn open_reader(path: &Path) -> Result<BufReader<File>, OfficeError> {
    Ok(BufReader::new(File::open(path).map_err(container::io_err)?))
}

fn load_xlsx(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    let limits = &opts.limits;
    // Our own pass first: it also tells how big each sheet's value box is, before calamine
    // allocates a dense matrix for it.
    let fm = fmt_xlsx::read(path, limits)?;
    let mut wb: calamine::Xlsx<_> = calamine::Xlsx::new(open_reader(path)?).map_err(map_xlsx)?;
    let metas = sheet_metas(&wb);
    let ctx = Ctx::new(fm.date1904, fm.formats.clone(), *opts);
    assemble(&ctx, metas, |name| {
        let sf = fm.sheets.get(name);
        if let Some(sf) = sf {
            if sf.dense_cost() > limits.max_dense_cells {
                return Err(OfficeError::TooLarge { what: "sheet area" });
            }
        }
        let data = wb.worksheet_range(name).map_err(map_xlsx)?;
        let formulas = wb.worksheet_formula(name).unwrap_or_default();
        let merges = wb.merge_cells_by_sheet_name(name).unwrap_or_default();
        Ok(Fetched {
            data,
            formulas,
            merges,
            fmts: sf,
        })
    })
}

fn load_xlsb(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    let limits = &opts.limits;
    // Our own pass first (formats, 1904, and the size of each sheet's value box): calamine fills
    // a dense matrix over that box and an OOM abort cannot be caught.
    let fm = fmt_xlsb::read(path, limits)?;
    let mut wb: calamine::Xlsb<_> = calamine::Xlsb::new(open_reader(path)?).map_err(map_xlsb)?;
    let metas = sheet_metas(&wb);
    let ctx = Ctx::new(fm.date1904, fm.formats.clone(), *opts);
    assemble(&ctx, metas, |name| {
        let sf = fm.sheets.get(name);
        if sf.is_some_and(|sf| sf.dense_cost() > limits.max_dense_cells) {
            return Err(OfficeError::TooLarge { what: "sheet area" });
        }
        Ok(Fetched {
            data: wb.worksheet_range(name).map_err(map_xlsb)?,
            formulas: wb.worksheet_formula(name).unwrap_or_default(),
            merges: Vec::new(),
            fmts: sf,
        })
    })
}

fn load_ods(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    // Our own pass first: it detects encryption, budgets every table (calamine parses them all
    // while opening) and reads the styles, so it must run before `Ods::new`.
    let fm = fmt_ods::read(path, &opts.limits)?;
    let mut wb: calamine::Ods<_> = calamine::Ods::new(open_reader(path)?).map_err(map_ods)?;
    let metas = sheet_metas(&wb);
    let ctx = Ctx::new(fm.date1904, fm.formats.clone(), *opts);
    assemble(&ctx, metas, |name| {
        Ok(Fetched {
            data: wb.worksheet_range(name).map_err(map_ods)?,
            formulas: wb.worksheet_formula(name).unwrap_or_default(),
            merges: Vec::new(),
            fmts: fm.sheets.get(name),
        })
    })
}

fn load_xls(path: &Path, opts: &LoadOptions) -> Result<Workbook, OfficeError> {
    // Our own pass first: encryption, formats, and the size of *every* sheet's value box (calamine
    // builds all of them while opening).
    let fm = fmt_xls::read(path, &opts.limits)?;
    let mut wb: calamine::Xls<_> = calamine::Xls::new(open_reader(path)?).map_err(map_xls)?;
    let metas = sheet_metas(&wb);
    let ctx = Ctx::new(fm.date1904, fm.formats.clone(), *opts);
    assemble(&ctx, metas, |name| {
        Ok(Fetched {
            data: wb.worksheet_range(name).map_err(map_xls)?,
            formulas: wb.worksheet_formula(name).unwrap_or_default(),
            merges: wb.merge_cells_by_sheet_name(name).unwrap_or_default(),
            fmts: fm.sheets.get(name),
        })
    })
}

fn map_xlsx(e: calamine::XlsxError) -> OfficeError {
    match e {
        calamine::XlsxError::Password => OfficeError::Encrypted,
        calamine::XlsxError::Zip(z) => container::zip_err(z),
        other => OfficeError::Corrupt(format!("xlsx: {other}")),
    }
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

/// What one sheet's reader call returns.
struct Fetched<'a> {
    data: Range<Data>,
    formulas: Range<String>,
    merges: Vec<Dimensions>,
    fmts: Option<&'a SheetFormats>,
}

/// Walks the sheet list in workbook order; hidden sheets are only counted. Chart/dialog/macro
/// sheets carry no cells and are skipped without being counted as hidden.
fn assemble<'a>(
    ctx: &Ctx,
    metas: Vec<Meta>,
    mut fetch: impl FnMut(&str) -> Result<Fetched<'a>, OfficeError>,
) -> Result<Workbook, OfficeError> {
    let mut out = Workbook {
        sheets: Vec::new(),
        hidden_sheets: 0,
        date1904: ctx.date1904,
        formats: ctx.formats.clone(),
    };
    // One grid-cell budget for the whole workbook, handed out in sheet order: ten sheets of the
    // per-sheet maximum would otherwise cost ten times the memory the limit promises.
    let mut cells_left = ctx.opts.limits.max_grid_cells;
    for m in metas {
        if m.typ != SheetType::WorkSheet {
            continue;
        }
        if m.visible != SheetVisible::Visible {
            out.hidden_sheets += 1;
            continue;
        }
        let f = fetch(&m.name)?;
        out.sheets
            .push(build_sheet(ctx, m.name, f, &mut cells_left));
    }
    Ok(out)
}

fn build_sheet(ctx: &Ctx, name: String, f: Fetched<'_>, cells_left: &mut u64) -> Sheet {
    let limits = &ctx.opts.limits;
    let dctx = DisplayCtx {
        date1904: ctx.date1904,
        locale: ctx.opts.locale,
    };
    let mut sheet = Sheet {
        name,
        merges: f
            .merges
            .iter()
            .map(|d| MergeRange {
                row0: d.start.0,
                col0: d.start.1,
                row1: d.end.0,
                col1: d.end.1,
            })
            .collect(),
        ..Sheet::default()
    };
    let (Some((r0, c0)), Some((r1, c1))) = (f.data.start(), f.data.end()) else {
        return sheet;
    };

    // Grid extent from A1 to the last used row/column, then the caps.
    let abs_rows = u64::from(r1) + 1;
    let abs_cols = u64::from(c1) + 1;
    let ncols = abs_cols.min(limits.max_cols as u64) as usize;
    sheet.cols_truncated = abs_cols > limits.max_cols as u64;
    // Rows this sheet may take from what is left of the workbook's budget. A sheet always gets
    // one row while any budget is left; once it is gone later sheets are empty (and flagged).
    let budget_rows = match *cells_left {
        0 => 0,
        left => (left / ncols.max(1) as u64).max(1),
    };
    let row_cap = (limits.max_rows as u64).min(budget_rows);
    let nrows = abs_rows.min(row_cap) as usize;
    sheet.rows_truncated = abs_rows > row_cap;
    sheet.ncols = ncols;
    sheet.nrows = nrows;
    *cells_left = cells_left.saturating_sub(nrows as u64 * ncols as u64);
    sheet.rows = vec![Vec::new(); nrows];

    for (i, row) in f.data.rows().enumerate() {
        let abs_row = r0 as usize + i;
        if abs_row >= nrows {
            break;
        }
        let out_row = &mut sheet.rows[abs_row];
        for (j, v) in row.iter().enumerate() {
            let abs_col = c0 as usize + j;
            if abs_col >= ncols {
                break;
            }
            let Some(mut value) = to_value(v) else {
                continue;
            };
            // ods: an error cell reaches us as an empty string; the format pass knows better.
            if let Some(code) = f
                .fmts
                .and_then(|s| s.error_at(abs_row as u32, abs_col as u32))
            {
                value = CellValue::Error(code);
            }
            let fmt = f
                .fmts
                .map(|s| s.format_at(abs_row as u32, abs_col as u32))
                .unwrap_or(0);
            let code = ctx.compiled.get(usize::from(fmt)).unwrap_or(&ctx.general);
            let shown = display_compiled(&value, code, &dctx);
            let display = match &value {
                CellValue::Text(t) if **t == *shown => None,
                _ => Some(shown.into_boxed_str()),
            };
            out_row.push((
                abs_col as u32,
                Cell {
                    value,
                    display,
                    fmt,
                },
            ));
        }
    }

    if let Some((fr0, fc0)) = f.formulas.start() {
        for (i, row) in f.formulas.rows().enumerate() {
            let abs_row = fr0 as usize + i;
            if abs_row >= nrows {
                break;
            }
            for (j, formula) in row.iter().enumerate() {
                let abs_col = fc0 as usize + j;
                if abs_col >= ncols {
                    break;
                }
                if !formula.is_empty() {
                    sheet
                        .formulas
                        .insert((abs_row as u32, abs_col as u32), formula.as_str().into());
                }
            }
        }
    }
    sheet
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
