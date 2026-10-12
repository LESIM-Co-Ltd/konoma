//! What the xlsx reader and the other formats' format passes share: the number-format table of
//! `xl/styles.xml` (`numFmts` + `cellXfs` -> `xf index -> index into a small deduplicated table
//! of formats`), the result types every format's pass returns ([`XlsxFormats`], [`SheetFormats`]),
//! the A1 / relationship helpers, and the XML helpers.
//!
//! The xlsx sheet itself is read by `xlsx.rs` (values, formats, formulas and merges in one pass);
//! the binary and ODF formats keep their own format passes (`fmt_xlsb`, `fmt_xls`, `fmt_ods`),
//! which return their results in these types.
//!
//! The output is deliberately compact: a deduplicated format table (typically < 50 entries) and,
//! per sheet, a sorted sparse list of `(row, col, format index)` for the cells whose format is not
//! `General` — never one string per cell.
//!
//! Safety: quick-xml never expands DTD entities (a `<!DOCTYPE ... <!ENTITY ...>` is just an
//! ignored event; an undefined `&x;` fails to unescape and we keep the raw text), and parts are
//! read through `container::part_reader` with a size cap, so neither a billion-laughs document
//! nor an oversized part can run away. Parts are *streamed*; no DOM is built.

use std::collections::HashMap;
use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::container::Limits;
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
    /// Data was left out: the pass stopped at a row past the row cap, at a reference that
    /// overflows the value reader's arithmetic, or at the sheet's cell budget, **and** a value,
    /// an inline string or a formula lies past that point (a tail of empty cells is not data).
    pub truncated: bool,
}

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

/// What the workbook-level parts of an xlsb package say: the date system, the
/// format table, which format each cell style has, and where each sheet's part is. Reading this is
/// cheap and independent of the size of any sheet: the sheets themselves are read one at a time.
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

/// Fixed cost charged to the text budget for each string the reader keeps (the `String` header
/// and allocation overhead, which is not part of its length).
pub(crate) const STRING_OVERHEAD: u64 = 32;

// ---------------------------------------------------------------------------------------------
// workbook.xml.rels
// ---------------------------------------------------------------------------------------------

/// `Id` -> normalized part path inside the zip (no leading slash).
pub(crate) fn parse_rels(src: impl BufRead) -> Result<HashMap<String, String>, OfficeError> {
    let mut rd = XmlReader::new(src);
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
    pub(crate) fn without_styles() -> Styles {
        Styles {
            formats: vec![NumFmtRef::General],
            xf_to_format: Vec::new(),
        }
    }
}

pub(crate) fn parse_styles(src: impl BufRead) -> Result<Styles, OfficeError> {
    let mut rd = XmlReader::new(src);
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
// cell references
// ---------------------------------------------------------------------------------------------

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
pub(crate) fn attr(e: &BytesStart<'_>, local: &[u8], prefixed: bool) -> Option<String> {
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
pub(crate) fn attr_bounded(e: &BytesStart<'_>, local: &[u8], max_raw: usize) -> Option<String> {
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

pub(crate) fn is_true(v: &str) -> bool {
    matches!(v.trim(), "1" | "true" | "TRUE" | "True")
}

pub(crate) fn xml_err(e: quick_xml::Error) -> OfficeError {
    if let quick_xml::Error::Io(io) = &e {
        if io.get_ref().is_some_and(|inner| inner.is::<TooDeep>()) {
            return OfficeError::TooLarge { what: "xml depth" };
        }
    }
    OfficeError::Corrupt(format!("xml: {e}"))
}

/// Most sheets a workbook may have, in any format. Excel's own UI copes with a few hundred; a
/// sheet is a tab, a name, and a place in the file. A file with millions is made to cost memory
/// (and, for xls, to make `calamine` read the same records once per sheet).
pub(crate) const MAX_SHEETS: usize = 4_096;

/// Deepest element nesting an XML part may have. Real parts are shallow: a worksheet is about 10
/// levels (`worksheet/sheetData/row/c/is/r/rPr/...`), an `extLst` or a styles part under 10, an
/// ods `content.xml` with text boxes inside frames inside text boxes about 40. The reader keeps
/// the name of every open element (8 bytes plus the name each, whatever the settings), so a
/// 215 KB part of 70 million nested `<a>` took 1.6 GB; 256 levels costs nothing and is far
/// beyond any file an application wrote.
pub(crate) const MAX_XML_DEPTH: usize = 256;

#[derive(Debug)]
struct TooDeep;

impl std::fmt::Display for TooDeep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "elements nested more than {MAX_XML_DEPTH} deep")
    }
}

impl std::error::Error for TooDeep {}

/// A [`quick_xml::Reader`] that refuses a part nested deeper than [`MAX_XML_DEPTH`]. **Every XML
/// reader of the office previews is one of these**: the depth is counted here, on every event,
/// including those inside [`XmlReader::read_to_end_into`] (which `quick_xml` runs through the same
/// open-element stack).
pub(crate) struct XmlReader<R> {
    rd: Reader<R>,
    depth: usize,
}

impl<R: BufRead> XmlReader<R> {
    pub(crate) fn new(src: R) -> XmlReader<R> {
        XmlReader {
            rd: Reader::from_reader(src),
            depth: 0,
        }
    }

    pub(crate) fn config_mut(&mut self) -> &mut quick_xml::reader::Config {
        self.rd.config_mut()
    }

    /// How many bytes of the part have been consumed so far.
    pub(crate) fn position(&self) -> u64 {
        self.rd.buffer_position()
    }

    pub(crate) fn read_event_into<'b>(
        &mut self,
        buf: &'b mut Vec<u8>,
    ) -> Result<Event<'b>, quick_xml::Error> {
        let ev = self.rd.read_event_into(buf)?;
        match ev {
            Event::Start(_) => {
                self.depth += 1;
                if self.depth > MAX_XML_DEPTH {
                    return Err(quick_xml::Error::Io(std::sync::Arc::new(
                        std::io::Error::new(std::io::ErrorKind::InvalidData, TooDeep),
                    )));
                }
            }
            Event::End(_) => self.depth = self.depth.saturating_sub(1),
            _ => {}
        }
        Ok(ev)
    }

    /// Reads up to and including the end tag named `end` that closes the element opened before
    /// (elements of the same name nested inside are counted, as `quick_xml` does; the start tag
    /// may have been followed by other events already).
    pub(crate) fn read_to_end_into(
        &mut self,
        end: quick_xml::name::QName<'_>,
        buf: &mut Vec<u8>,
    ) -> Result<(), quick_xml::Error> {
        let mut nested = 0usize;
        loop {
            buf.clear();
            match self.read_event_into(buf)? {
                Event::Start(e) if e.name() == end => nested += 1,
                Event::End(e) if e.name() == end => {
                    if nested == 0 {
                        return Ok(());
                    }
                    nested -= 1;
                }
                Event::Eof => {
                    return Err(quick_xml::errors::IllFormedError::MissingEndTag(
                        String::from_utf8_lossy(end.as_ref()).into_owned(),
                    )
                    .into())
                }
                _ => {}
            }
        }
    }
}
