//! konoma's own xlsx / xlsm / xltx / xltm reader: **one pass** over a sheet gives the values, the
//! number-format index, the formulas and the merged ranges of every cell.
//!
//! Why it exists. `calamine` reads the values and konoma used to read the same sheet a second
//! time for the formats; the two readers had to be kept in step about *what a cell is*
//! (`reader_stop`), and that went wrong three times. It also refused a whole sheet over one cell
//! holding an error it does not know (Excel 365's `#SPILL!`, `#CALC!`, ...). Reading the sheet
//! once, with the rules in one place, removes both problems. `calamine` is still the reader of
//! xlsb / xls / ods, and in the tests it is the oracle this reader is compared with
//! (`tests_xlsx.rs`).
//!
//! What is read, and where the rules come from (the behaviour of `calamine` 0.36 was the
//! reference for the *values*; the code is konoma's own):
//!
//! * `xl/workbook.xml`: sheet names, `state` (visible / hidden / veryHidden) and `date1904`;
//!   `xl/_rels/workbook.xml.rels`: which part is each sheet, and its kind (only `worksheet`
//!   parts are sheets; chart / dialog / macro sheets carry no cells and are not even counted as
//!   hidden).
//! * `xl/styles.xml`: `cellXfs` -> number format (shared with the other formats, `fmt_xlsx`) and,
//!   from the same format code, whether a number is a date / a duration (`DateKind`; it decides
//!   the *type* shown in the detail line, not the display).
//! * `xl/sharedStrings.xml`: kept in one arena (`SharedStrings`), not one `String` each.
//!   Rich text runs are joined, phonetic runs (`rPh`) are dropped, `<t>` is trimmed unless
//!   `xml:space="preserve"`, and `_xHHHH_` escapes are decoded.
//! * the sheet part, streamed: cell address (`r`, or the one after the previous cell), style `s`,
//!   type `t` (`n` `s` `str` `inlineStr` `b` `e` `d`), `<v>`, `<is>`, `<f>` (a shared formula's
//!   derived cells get the formula of its anchor with relative references moved), and
//!   `<mergeCell>`.
//!
//! Where it differs from `calamine` on purpose (`tests_xlsx.rs` lists each as a known difference):
//! an error value is whatever the file says (`#SPILL!`, `#GETTING_DATA`, ...); a cell that
//! `calamine` would refuse the whole sheet over (an unknown child element, a bad number in an
//! `n` cell, a string index past the table) is skipped or shown as text; a shared formula is
//! expanded from its anchor cell (not from the top-left of its `ref`); the expansion understands
//! quoted sheet names, structured references and error literals, which `calamine`'s tokenizer
//! shifts as if they were cell references.
//!
//! Safety (all inherited from the container layer or enforced here):
//!
//! * every part is read through `container::part_reader` (capped, whatever the zip declares);
//!   the shared strings are refused beyond `Limits::max_text_bytes`;
//! * the sheet is cut by the *builder's* budgets (rows, cells, text bytes): the sink says stop and
//!   the reader stops, so nothing past the budget is allocated. The text of a `<v>` / `<f>` / `<is>`
//!   is gathered only up to the text budget when it arrives in pieces (references, many `<t>`);
//!   plain text between two tags is one parser event and is held whole, which the part-size cap
//!   bounds;
//! * rows and columns that wrap or exceed Excel's limits never index anything;
//! * quick-xml never expands DTD entities; an `&unknown;` is kept as written;
//! * the shared-formula table is bounded (count and bytes), so a hostile sheet cannot grow it
//!   without limit, and a cancelled load stops at the next row.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use quick_xml::events::{BytesRef, BytesStart, Event};
use zip::ZipArchive;

use super::container::{self, Limits};
use super::fmt_xlsx::{self, attr, is_true, parse_a1, xml_err, XmlReader, STRING_OVERHEAD};
use super::workbook::{Cancel, MergeRange, NumFmtRef};
use super::OfficeError;

/// Most merged ranges kept per sheet (a sheet of single-cell merges would otherwise be a list as
/// long as the part).
pub const MAX_MERGES: usize = 100_000;

/// Whether a number is shown as a date / time, as an elapsed duration, or as a plain number.
/// Decided from the format of its style (a cell without a style is `Plain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DateKind {
    Plain,
    Date,
    Duration,
}

/// The kind of numbers a format code produces. A custom code is a date format when its first
/// section has a date or time token outside quotes, brackets and escapes (`[h]` / `[mm]` /
/// `[ss]` alone is an elapsed time); a built-in id is one of the fixed date / time numbers.
pub(crate) fn date_kind(fmt: &NumFmtRef) -> DateKind {
    match fmt {
        NumFmtRef::General => DateKind::Plain,
        NumFmtRef::Builtin(id) => match id {
            // 27-36 and 50-58 are the East-Asian (Japanese era) dates and times.
            14..=22 | 27..=36 | 45 | 47 | 50..=58 => DateKind::Date,
            46 => DateKind::Duration,
            _ => DateKind::Plain,
        },
        NumFmtRef::Custom(code) => date_kind_of_code(code),
    }
}

fn date_kind_of_code(code: &str) -> DateKind {
    let mut escaped = false; // after `\`, `_` or `*`: the next character is a literal
    let mut quoted = false;
    let mut depth = 0u8; // `[` nesting: colours, conditions, locale, elapsed time
    let mut am_pm = false; // inside an `AM/PM` / `A/P` marker
    let mut elapsed = false; // just after `[` + `h` / `m` / `s`: `[h]` is a duration
    let mut prev = ' ';
    for c in code.chars() {
        if escaped {
            escaped = false;
        } else if matches!(c, '_' | '\\' | '*') {
            escaped = true;
        } else if quoted {
            if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        } else if c == ';' {
            return DateKind::Plain;
        } else if c == '[' {
            depth = depth.saturating_add(1);
        } else if c == ']' && depth == 1 && elapsed {
            return DateKind::Duration;
        } else if c == ']' {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && !am_pm && matches!(c, 'a' | 'A') {
            am_pm = true;
        } else if depth == 0
            && if am_pm {
                // the rest of `AM/PM` / `A/P`
                matches!(c, 'p' | 'm' | '/' | 'P' | 'M')
            } else {
                matches!(c, 'd' | 'm' | 'h' | 'y' | 's' | 'D' | 'M' | 'H' | 'Y' | 'S')
            }
        {
            return DateKind::Date;
        } else if elapsed && c.eq_ignore_ascii_case(&prev) {
            // `[hh]`, `[mm]`: still an elapsed-time bracket
        } else {
            elapsed = prev == '[' && matches!(c, 'm' | 'h' | 's' | 'M' | 'H' | 'S');
        }
        prev = c;
    }
    DateKind::Plain
}

// ---------------------------------------------------------------------------------------------
// the package
// ---------------------------------------------------------------------------------------------

/// A worksheet's visibility, as `workbook.xml` declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Visibility {
    Visible,
    Hidden,
    VeryHidden,
}

/// One worksheet of the workbook (chart / dialog / macro sheets are not listed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SheetInfo {
    pub name: String,
    /// The part inside the zip.
    pub part: String,
    pub visible: Visibility,
}

/// What the sheet reader needs from the workbook-level parts.
#[derive(Debug, Default)]
pub(crate) struct Tables {
    pub strings: SharedStrings,
    /// `cellXfs` index -> index into the format table.
    pub xf_to_format: Vec<u16>,
    /// `cellXfs` index -> what kind of number a cell with that style holds.
    pub xf_kind: Vec<DateKind>,
}

/// An opened xlsx package.
pub(crate) struct XlsxBook {
    zip: ZipArchive<BufReader<File>>,
    /// Zip entry names by lower-cased, `/`-separated name: parts are found without regard to case
    /// or `\` separators (some producers write them).
    names: HashMap<String, String>,
    pub date1904: bool,
    pub sheets: Vec<SheetInfo>,
    /// The deduplicated number formats the cells refer to (index 0 is General).
    pub formats: Vec<NumFmtRef>,
    tables: Tables,
}

fn norm_name(n: &str) -> String {
    n.replace('\\', "/").to_ascii_lowercase()
}

/// A buffered reader over the part `p` (found without regard to case or `\\` separators), capped
/// at `cap` expanded bytes. `None` when the package has no such part.
fn read_part<'z>(
    zip: &'z mut ZipArchive<BufReader<File>>,
    names: &HashMap<String, String>,
    p: &str,
    cap: u64,
) -> Result<Option<BufReader<impl Read + 'z>>, OfficeError> {
    let name = names.get(&norm_name(p)).map_or(p, String::as_str);
    Ok(container::part_reader(zip, name, cap)?.map(|r| BufReader::with_capacity(64 * 1024, r)))
}

/// Opens the package: workbook, relationships, styles and shared strings are read; sheets are not.
pub(crate) fn open(path: &Path, limits: &Limits) -> Result<XlsxBook, OfficeError> {
    let mut zip = container::open_zip(path)?;
    let names: HashMap<String, String> = zip
        .file_names()
        .map(|n| (norm_name(n), n.to_string()))
        .collect();
    let cap = limits.max_part_bytes;

    let (date1904, decls) = {
        let r = read_part(&mut zip, &names, "xl/workbook.xml", cap)?
            .ok_or_else(|| OfficeError::Corrupt("missing xl/workbook.xml".into()))?;
        parse_workbook(r)?
    };
    let rels = match read_part(&mut zip, &names, "xl/_rels/workbook.xml.rels", cap)? {
        Some(r) => parse_rels(r)?,
        None => HashMap::new(),
    };
    let rel_part = |suffix: &str, default: &str| -> String {
        let mut found: Vec<&Rel> = rels.values().filter(|r| r.kind == suffix).collect();
        found.sort_by(|a, b| a.target.cmp(&b.target));
        found
            .first()
            .map_or_else(|| default.to_string(), |r| r.target.clone())
    };

    let styles_part = rel_part("styles", "xl/styles.xml");
    let styles = match read_part(&mut zip, &names, &styles_part, cap)? {
        Some(r) => fmt_xlsx::parse_styles(r)?,
        None => fmt_xlsx::Styles::without_styles(),
    };
    let strings_part = rel_part("sharedStrings", "xl/sharedStrings.xml");
    let strings = match read_part(&mut zip, &names, &strings_part, cap)? {
        Some(r) => read_shared_strings(r, limits)?,
        None => SharedStrings::default(),
    };

    let kinds: Vec<DateKind> = styles.formats.iter().map(date_kind).collect();
    let xf_kind = styles
        .xf_to_format
        .iter()
        .map(|&f| {
            kinds
                .get(usize::from(f))
                .copied()
                .unwrap_or(DateKind::Plain)
        })
        .collect();

    let sheets = decls
        .into_iter()
        .filter_map(|d| {
            let rel = rels.get(&d.rid)?;
            // Chart, dialog and macro sheets carry no cells: not sheets here (and not counted as
            // hidden ones either).
            (!matches!(
                rel.kind.as_str(),
                "chartsheet" | "dialogsheet" | "macrosheet" | "intlmacrosheet"
            ))
            .then(|| SheetInfo {
                name: d.name,
                part: rel.target.clone(),
                visible: d.visible,
            })
        })
        .collect();
    Ok(XlsxBook {
        zip,
        names,
        date1904,
        sheets,
        formats: styles.formats,
        tables: Tables {
            strings,
            xf_to_format: styles.xf_to_format,
            xf_kind,
        },
    })
}

impl XlsxBook {
    /// The part of the worksheet named `name`.
    pub(crate) fn part_of(&self, name: &str) -> Option<&str> {
        self.sheets
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.part.as_str())
    }

    /// Reads one sheet part, calling `sink` for every cell in file order until it returns `false`
    /// (a budget is spent). Returns the merged ranges found up to there. A declared sheet whose part
    /// is missing from the package is corrupt.
    pub(crate) fn read_sheet(
        &mut self,
        part: &str,
        limits: &Limits,
        cancel: Option<&Cancel>,
        sink: impl FnMut(CellOut<'_>) -> bool,
    ) -> Result<Vec<MergeRange>, OfficeError> {
        let actual = self
            .names
            .get(&norm_name(part))
            .cloned()
            .unwrap_or_else(|| part.to_string());
        let Some(r) = container::part_reader(&mut self.zip, &actual, limits.max_part_bytes)? else {
            return Err(OfficeError::Corrupt(format!("missing sheet part {part}")));
        };
        parse_sheet(
            BufReader::with_capacity(64 * 1024, r),
            &self.tables,
            limits,
            cancel,
            sink,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// workbook.xml and its relationships
// ---------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub(crate) struct SheetDecl {
    pub name: String,
    pub rid: String,
    pub visible: Visibility,
}

/// `(date1904, sheets)` of `workbook.xml`. A `<sheet>` needs a name and a relationship id (an
/// attribute with a namespace prefix: `r:id`).
pub(crate) fn parse_workbook(src: impl BufRead) -> Result<(bool, Vec<SheetDecl>), OfficeError> {
    let mut rd = XmlReader::new(src);
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
                    let rid = attr(&e, b"id", true);
                    let visible = match attr(&e, b"state", false).as_deref() {
                        Some("hidden") => Visibility::Hidden,
                        Some("veryHidden") => Visibility::VeryHidden,
                        _ => Visibility::Visible,
                    };
                    if let (Some(name), Some(rid)) = (name, rid) {
                        if sheets.len() >= fmt_xlsx::MAX_SHEETS {
                            return Err(OfficeError::TooLarge { what: "sheets" });
                        }
                        sheets.push(SheetDecl { name, rid, visible });
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

/// One internal relationship of the workbook.
#[derive(Debug, PartialEq)]
pub(crate) struct Rel {
    /// The part, resolved against `xl/` (`worksheets/sheet1.xml` and `/xl/worksheets/sheet1.xml`
    /// are the same).
    pub target: String,
    /// The last segment of the relationship type (`worksheet`, `chartsheet`, `styles`, ...).
    pub kind: String,
}

/// Internal relationships by id (external targets are not parts of the package).
pub(crate) fn parse_rels(src: impl BufRead) -> Result<HashMap<String, Rel>, OfficeError> {
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
                    let kind = attr(&e, b"Type", false)
                        .and_then(|t| t.rsplit('/').next().map(str::to_string))
                        .unwrap_or_default();
                    map.insert(
                        id,
                        Rel {
                            target: fmt_xlsx::resolve_target("xl", &target),
                            kind,
                        },
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

// ---------------------------------------------------------------------------------------------
// text: shared strings, inline strings, entities
// ---------------------------------------------------------------------------------------------

/// The shared string table: all text in one buffer, and where each string ends. A string costs
/// its text plus 4 bytes, against 24 bytes + a heap block for a `String`.
#[derive(Debug, Default)]
pub(crate) struct SharedStrings {
    data: String,
    ends: Vec<u32>,
}

impl SharedStrings {
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    pub(crate) fn get(&self, i: usize) -> Option<&str> {
        let end = *self.ends.get(i)? as usize;
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        self.data.get(start..end)
    }

    #[cfg(test)]
    pub(crate) fn from_strs(items: &[&str]) -> SharedStrings {
        let mut s = SharedStrings::default();
        for i in items {
            s.data.push_str(i);
            s.ends.push(s.data.len() as u32);
        }
        s
    }
}

/// Scratch buffers of the sheet reader, reused for every cell.
#[derive(Default)]
struct Scratch {
    /// Event buffers (the cell body loop, text gathering, skipping).
    ev: Vec<u8>,
    ev2: Vec<u8>,
    ev3: Vec<u8>,
    /// Text of the `<v>` of a string / date / error cell.
    v: String,
    /// Text of an inline string.
    inline: String,
    /// The formula as written / the expanded one.
    f: String,
    x: String,
    /// One `<t>` before it is trimmed and unescaped.
    piece: String,
}

/// Appends an entity or character reference. An unknown entity is kept as written (never
/// expanded: a DTD `<!ENTITY>` is not honoured).
fn push_ref(e: &BytesRef<'_>, out: &mut String) {
    let Ok(name) = e.decode() else {
        return;
    };
    if let Some(s) = quick_xml::escape::resolve_xml_entity(&name) {
        out.push_str(s);
    } else if let Ok(Some(c)) = e.resolve_char_ref() {
        out.push(c);
    } else {
        out.push('&');
        out.push_str(&name);
        out.push(';');
    }
}

/// Gathers the text of the element named `closing` (whose start was just read) into `out`: text,
/// CDATA and references. Never appends past `cap` bytes of `out` (a text event is cut at the cap
/// rather than added whole, so one huge event cannot grow `out` far beyond it); the rest is still
/// read. A byte that is not valid UTF-8 is a replacement character, so one bad cell is not a
/// bad sheet.
fn read_text_into<R: BufRead>(
    rd: &mut XmlReader<R>,
    closing: &[u8],
    buf: &mut Vec<u8>,
    out: &mut String,
    cap: usize,
) -> Result<(), OfficeError> {
    loop {
        buf.clear();
        match rd.read_event_into(buf).map_err(xml_err)? {
            Event::Text(t) => {
                if out.len() < cap {
                    match t.xml10_content() {
                        Ok(s) => push_capped(out, &s, cap),
                        Err(_) => push_capped(out, &String::from_utf8_lossy(&t), cap),
                    }
                }
            }
            Event::CData(t) => {
                if out.len() < cap {
                    match t.xml10_content() {
                        Ok(s) => push_capped(out, &s, cap),
                        Err(_) => push_capped(out, &String::from_utf8_lossy(&t), cap),
                    }
                }
            }
            Event::GeneralRef(e) => {
                if out.len() < cap {
                    push_ref(&e, out);
                }
            }
            Event::End(e) if e.name().as_ref() == closing => return Ok(()),
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

/// Appends as much of `s` as keeps `out` within `cap` bytes, cutting at a character boundary.
fn push_capped(out: &mut String, s: &str, cap: usize) {
    let room = cap.saturating_sub(out.len());
    if s.len() <= room {
        out.push_str(s);
        return;
    }
    let mut end = room;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&s[..end]);
}

/// Reads a string item (`<si>` of the shared strings, `<is>` of an inline string) into `out`: the
/// text of every `<t>` that is not phonetic, in order. Each `<t>` is trimmed of ASCII whitespace
/// unless it says `xml:space="preserve"`, and `_xHHHH_` escapes are decoded.
fn read_rich<R: BufRead>(
    rd: &mut XmlReader<R>,
    closing: &[u8],
    bufs: (&mut Vec<u8>, &mut Vec<u8>),
    piece: &mut String,
    out: &mut String,
    cap: usize,
) -> Result<(), OfficeError> {
    let (ev, ev2) = bufs;
    let mut in_phonetic = false;
    loop {
        ev.clear();
        match rd.read_event_into(ev).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"rPh" => in_phonetic = true,
                b"t" if !in_phonetic => {
                    let preserve =
                        e.attributes().with_checks(false).flatten().any(|a| {
                            a.key.as_ref() == b"xml:space" && a.value.as_ref() == b"preserve"
                        });
                    piece.clear();
                    read_text_into(rd, e.name().as_ref(), ev2, piece, cap)?;
                    let text = if preserve {
                        piece.as_str()
                    } else {
                        piece.trim_matches([' ', '\t', '\r', '\n'])
                    };
                    if out.len() <= cap {
                        push_unescaped(text, out);
                    }
                }
                _ => {}
            },
            Event::End(e) => {
                if e.name().as_ref() == closing {
                    return Ok(());
                }
                if e.local_name().as_ref() == b"rPh" {
                    in_phonetic = false;
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

/// Appends `s` with Excel's `_xHHHH_` escapes decoded (`_x000D_` is a carriage return; the text
/// `_x000D_` itself is written `_x005F_x000D_`). A sequence that is not an escape, or names a
/// surrogate, is kept as written.
fn push_unescaped(s: &str, out: &mut String) {
    if !s.contains("_x") {
        out.push_str(s);
        return;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        if b[i] == b'_' && i + 7 <= b.len() && b[i + 1] == b'x' && b[i + 6] == b'_' {
            let hex = &s[i + 2..i + 6];
            if hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                if let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                    out.push_str(&s[start..i]);
                    out.push(c);
                    i += 7;
                    start = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&s[start..]);
}

/// Reads the shared string table. A table whose strings add up past `Limits::max_text_bytes` is
/// refused (the same bound as for the binary formats), counting a fixed overhead per string.
fn read_shared_strings(src: impl BufRead, limits: &Limits) -> Result<SharedStrings, OfficeError> {
    let too_large = || OfficeError::TooLarge { what: "text" };
    let cap = usize::try_from(limits.max_text_bytes).unwrap_or(usize::MAX);
    let mut rd = XmlReader::new(src);
    rd.config_mut().expand_empty_elements = true;
    let (mut ev, mut ev2, mut piece) = (Vec::new(), Vec::new(), String::new());
    let mut buf = Vec::new();
    let mut table = SharedStrings::default();
    let mut total: u64 = 0;
    let mut item = String::new();
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
                b"si" => {
                    item.clear();
                    read_rich(
                        &mut rd,
                        e.name().as_ref(),
                        (&mut ev, &mut ev2),
                        &mut piece,
                        &mut item,
                        cap,
                    )?;
                    total = total
                        .saturating_add(item.len() as u64)
                        .saturating_add(STRING_OVERHEAD);
                    if total > limits.max_text_bytes {
                        return Err(too_large());
                    }
                    table.data.push_str(&item);
                    table
                        .ends
                        .push(u32::try_from(table.data.len()).map_err(|_| too_large())?);
                }
                _ => {}
            },
            Event::End(e) if e.local_name().as_ref() == b"sst" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(table)
}

// ---------------------------------------------------------------------------------------------
// the sheet
// ---------------------------------------------------------------------------------------------

/// A cell value as the file holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Val<'a> {
    /// A plain integer literal that fits `i64`, kept exact (an `f64` loses the end of 17+ digits).
    Int(i64),
    Number(f64),
    /// A number whose style is a date / time (`duration`: an elapsed time).
    Date {
        serial: f64,
        duration: bool,
    },
    Text(&'a str),
    Bool(bool),
    /// An error value such as `#DIV/0!` or `#SPILL!`: whatever the file wrote.
    Error(&'a str),
    /// A `t="d"` cell: an ISO 8601 date.
    Iso(&'a str),
}

/// One `<c>` of a sheet, with everything it holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CellOut<'a> {
    pub row: u32,
    pub col: u32,
    /// Index into the format table (0 = General).
    pub fmt: u16,
    /// `None`: nothing to show (a blank styled cell, a formula without a cached value).
    pub value: Option<Val<'a>>,
    /// The formula without the leading `=`; a shared formula's derived cell has it expanded.
    pub formula: Option<&'a str>,
}

/// The value of a cell while it is being read; text lives in the scratch buffers.
#[derive(Debug, Clone, Copy)]
enum Pv {
    Empty,
    Int(i64),
    Num(f64),
    Date(f64, bool),
    Bool(bool),
    Shared(usize),
    /// Text in `Scratch::v` (a `str` cell).
    Str,
    /// Text in `Scratch::v` (an `e` cell).
    Err,
    /// Text in `Scratch::v` (a `d` cell).
    Iso,
    /// Text in `Scratch::inline`.
    Inline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum T {
    Number,
    Shared,
    Bool,
    Error,
    Str,
    Iso,
    Inline,
}

fn type_of(t: Option<&[u8]>) -> T {
    match t {
        None | Some(b"n") => T::Number,
        Some(b"s") => T::Shared,
        Some(b"b") => T::Bool,
        Some(b"e") => T::Error,
        Some(b"d") => T::Iso,
        Some(b"inlineStr" | b"is") => T::Inline,
        // `str` and anything unknown: the text, as it is.
        Some(_) => T::Str,
    }
}

/// The anchor of a shared formula.
struct Master {
    text: Box<str>,
    row: u32,
    col: u32,
}

/// The shared formulas of one sheet by `si`, bounded in count and in bytes.
struct Masters {
    map: HashMap<u32, Master>,
    bytes: u64,
    max_count: usize,
    max_bytes: u64,
}

impl Masters {
    fn insert(&mut self, si: u32, text: &str, row: u32, col: u32) {
        let cost = text.len() as u64 + STRING_OVERHEAD;
        if self.map.len() >= self.max_count || self.bytes.saturating_add(cost) > self.max_bytes {
            return;
        }
        if let Some(old) = self.map.insert(
            si,
            Master {
                text: text.into(),
                row,
                col,
            },
        ) {
            self.bytes = self
                .bytes
                .saturating_sub(old.text.len() as u64 + STRING_OVERHEAD);
        }
        self.bytes += cost;
    }
}

fn parse_uint(b: &[u8]) -> Option<usize> {
    let b = b.trim_ascii();
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    b.iter().try_fold(0usize, |n, d| {
        n.checked_mul(10)?.checked_add(usize::from(d - b'0'))
    })
}

/// A `<row r>` value as a 0-based row. `None` when it is not a positive decimal number (the row
/// then follows the previous one); a number past `u32` saturates, beyond any row cap.
fn parse_row_ref(v: &[u8]) -> Option<u32> {
    let v = v.trim_ascii();
    if v.is_empty() || !v.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // Digit by digit with saturation: `parse::<u32>` would refuse 5000000000 and the row would
    // silently fall back to "the next row".
    let n = v.iter().fold(0u64, |n, b| {
        n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    });
    match n {
        0 => None,
        n => Some(u32::try_from(n - 1).unwrap_or(u32::MAX)),
    }
}

/// A reference `parse_a1` rejected that is long enough to wrap arithmetic (4+ column letters is
/// already past XFD; 8+ row digits is past row 1,048,576 by 100x).
fn absurd_ref(r: &[u8]) -> bool {
    let letters = r.iter().filter(|b| b.is_ascii_alphabetic()).count();
    let digits = r.iter().filter(|b| b.is_ascii_digit()).count();
    letters >= 4 || digits >= 8
}

fn parse_merge(v: &str) -> Option<MergeRange> {
    let (a, b) = v.split_once(':').unwrap_or((v, v));
    let (row0, col0) = parse_a1(a)?;
    let (row1, col1) = parse_a1(b)?;
    // `B2:A1` names the same rectangle as `A1:B2`.
    Some(MergeRange {
        row0: row0.min(row1),
        col0: col0.min(col1),
        row1: row0.max(row1),
        col1: col0.max(col1),
    })
}

/// Reads a sheet part, calling `sink` for every cell of `<sheetData>` in file order until it
/// returns `false`. Returns the merged ranges seen up to the end (or up to the stop).
///
/// A cell's position is its `r` attribute, or the cell after the previous one of its row; a row
/// without `r` follows the previous row. An unparsable `r` falls back to that sequence, except an
/// absurd one (4+ column letters, 8+ row digits), which is placed past every row cap so that the
/// builder cuts the sheet there instead of misplacing the cell.
pub(crate) fn parse_sheet(
    src: impl BufRead,
    tables: &Tables,
    limits: &Limits,
    cancel: Option<&Cancel>,
    mut sink: impl FnMut(CellOut<'_>) -> bool,
) -> Result<Vec<MergeRange>, OfficeError> {
    let cap = usize::try_from(limits.max_sheet_text_bytes).unwrap_or(usize::MAX);
    let mut rd = XmlReader::new(src);
    rd.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    let mut sc = Scratch::default();
    let mut masters = Masters {
        map: HashMap::new(),
        bytes: 0,
        max_count: usize::try_from(limits.max_sheet_cells).unwrap_or(usize::MAX),
        max_bytes: limits.max_sheet_text_bytes,
    };
    let mut merges = Vec::new();
    let mut in_data = false;
    let mut next_row: u32 = 0;
    let mut cur_row: u32 = 0;
    let mut next_col: u32 = 0;
    let mut rows_seen: u32 = 0;
    let mut cells_seen: u32 = 0;
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"sheetData" => in_data = true,
                b"row" if in_data => {
                    cur_row = row_attr(&e).unwrap_or(next_row);
                    next_col = 0;
                    // The builder asks `cancel` per row that holds cells; this covers rows that
                    // hold none.
                    rows_seen = rows_seen.wrapping_add(1);
                    if rows_seen.is_multiple_of(1024) && cancel.is_some_and(Cancel::is_cancelled) {
                        break;
                    }
                }
                b"c" if in_data => {
                    let a = cell_attrs(&e);
                    let (row, col) = match a.r.as_deref() {
                        Some(raw) => match std::str::from_utf8(raw).ok().and_then(parse_a1) {
                            Some(pos) => pos,
                            None if absurd_ref(raw) => (u32::MAX, 0),
                            None => (cur_row, next_col),
                        },
                        None => (cur_row, next_col),
                    };
                    next_col = col.saturating_add(1);
                    // A row of millions of cells is one row: look at `cancel` within it too.
                    cells_seen = cells_seen.wrapping_add(1);
                    if cells_seen.is_multiple_of(1024) && cancel.is_some_and(Cancel::is_cancelled) {
                        break;
                    }
                    let t = type_of(a.t.as_deref());
                    let kind =
                        a.s.as_deref()
                            .map(|s| {
                                let id = parse_uint(s).unwrap_or(0);
                                tables.xf_kind.get(id).copied().unwrap_or(DateKind::Plain)
                            })
                            .unwrap_or(DateKind::Plain);
                    let xf = a.s.as_deref().and_then(parse_uint).unwrap_or(0);
                    let fmt = tables.xf_to_format.get(xf).copied().unwrap_or(0);
                    let (pv, formula) = read_cell(
                        &mut rd,
                        &mut sc,
                        tables,
                        &mut masters,
                        (row, col),
                        t,
                        kind,
                        cap,
                        col as usize >= limits.max_cols,
                    )?;
                    let formula = match formula {
                        Formula::None => None,
                        Formula::Written => Some(sc.f.as_str()),
                        Formula::Expanded => Some(sc.x.as_str()),
                    };
                    let value = match pv {
                        Pv::Empty => None,
                        Pv::Int(i) => Some(Val::Int(i)),
                        Pv::Num(n) => Some(Val::Number(n)),
                        Pv::Date(serial, duration) => Some(Val::Date { serial, duration }),
                        Pv::Bool(b) => Some(Val::Bool(b)),
                        Pv::Shared(i) => tables.strings.get(i).map(Val::Text),
                        Pv::Str => Some(Val::Text(&sc.v)),
                        Pv::Err => Some(Val::Error(&sc.v)),
                        Pv::Iso => Some(Val::Iso(&sc.v)),
                        Pv::Inline => Some(Val::Text(&sc.inline)),
                    };
                    if !sink(CellOut {
                        row,
                        col,
                        fmt,
                        value,
                        formula,
                    }) {
                        break;
                    }
                }
                b"mergeCell" if merges.len() < MAX_MERGES => {
                    if let Some(m) = attr(&e, b"ref", false).and_then(|v| parse_merge(&v)) {
                        merges.push(m);
                    }
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"sheetData" => in_data = false,
                b"row" if in_data => next_row = cur_row.saturating_add(1),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(merges)
}

fn row_attr(e: &BytesStart<'_>) -> Option<u32> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == b"r")
        .and_then(|a| parse_row_ref(&a.value))
}

struct CellAttrs<'a> {
    r: Option<std::borrow::Cow<'a, [u8]>>,
    s: Option<std::borrow::Cow<'a, [u8]>>,
    t: Option<std::borrow::Cow<'a, [u8]>>,
}

fn cell_attrs<'a>(e: &'a BytesStart<'_>) -> CellAttrs<'a> {
    let mut out = CellAttrs {
        r: None,
        s: None,
        t: None,
    };
    for a in e.attributes().with_checks(false).flatten() {
        match a.key.as_ref() {
            b"r" => out.r = Some(a.value),
            b"s" => out.s = Some(a.value),
            b"t" => out.t = Some(a.value),
            _ => {}
        }
    }
    out
}

/// Where a cell's formula text is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Formula {
    None,
    /// In `Scratch::f`, as written.
    Written,
    /// In `Scratch::x`: a derived shared formula, expanded.
    Expanded,
}

/// Reads the body of a `<c>` up to its end. The last value-bearing child wins (`<v>` after
/// `<is>` replaces it, as the format's readers do); a child that is none of `<f>`, `<v>`, `<is>`
/// is skipped. A truncated file ends the cell where it ends.
#[allow(clippy::too_many_arguments)]
fn read_cell<R: BufRead>(
    rd: &mut XmlReader<R>,
    sc: &mut Scratch,
    tables: &Tables,
    masters: &mut Masters,
    pos: (u32, u32),
    t: T,
    kind: DateKind,
    cap: usize,
    skip: bool,
) -> Result<(Pv, Formula), OfficeError> {
    let mut pv = Pv::Empty;
    let mut formula = Formula::None;
    // A cell the grid will drop (past the column cap) is only read past: nothing in it is
    // decoded, expanded or kept. `had` records that it held something, which is all the builder
    // needs to note the truncation.
    let mut had = false;
    loop {
        sc.ev.clear();
        match rd.read_event_into(&mut sc.ev).map_err(xml_err)? {
            Event::Start(e) if skip => {
                had |= matches!(e.local_name().as_ref(), b"f" | b"v" | b"is");
                rd.read_to_end_into(e.name(), &mut sc.ev2)
                    .map_err(xml_err)?;
            }
            Event::Start(e) => match e.local_name().as_ref() {
                b"f" => {
                    let (shared, si, has_ref) = formula_attrs(&e);
                    sc.f.clear();
                    read_text_into(rd, e.name().as_ref(), &mut sc.ev2, &mut sc.f, cap)?;
                    formula = Formula::Written;
                    if shared {
                        if let Some(si) = si {
                            if has_ref {
                                masters.insert(si, &sc.f, pos.0, pos.1);
                            } else if let Some(m) = masters.map.get(&si) {
                                let dr = i64::from(pos.0) - i64::from(m.row);
                                let dc = i64::from(pos.1) - i64::from(m.col);
                                shift_formula(&m.text, dr, dc, &mut sc.x);
                                formula = Formula::Expanded;
                            } else {
                                // A derived cell whose anchor has not been seen: no formula.
                                formula = Formula::None;
                            }
                        }
                    }
                }
                b"is" => {
                    sc.inline.clear();
                    read_rich(
                        rd,
                        e.name().as_ref(),
                        (&mut sc.ev2, &mut sc.ev3),
                        &mut sc.piece,
                        &mut sc.inline,
                        cap,
                    )?;
                    pv = Pv::Inline;
                }
                b"v" => {
                    pv = read_v(
                        rd,
                        e.name().as_ref(),
                        (&mut sc.ev2, &mut sc.ev3),
                        &mut sc.v,
                        (t, kind),
                        tables,
                        cap,
                    )?
                }
                _ => {
                    rd.read_to_end_into(e.name(), &mut sc.ev2)
                        .map_err(xml_err)?;
                }
            },
            Event::End(e) if e.local_name().as_ref() == b"c" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    if skip {
        sc.v.clear();
        return Ok((if had { Pv::Str } else { Pv::Empty }, Formula::None));
    }
    Ok((pv, formula))
}

fn formula_attrs(e: &BytesStart<'_>) -> (bool, Option<u32>, bool) {
    let mut shared = false;
    let mut si = None;
    let mut has_ref = false;
    for a in e.attributes().with_checks(false).flatten() {
        match a.key.as_ref() {
            b"t" => shared = a.value.as_ref() == b"shared",
            b"si" => si = parse_uint(&a.value).and_then(|n| u32::try_from(n).ok()),
            b"ref" => has_ref = !a.value.is_empty(),
            _ => {}
        }
    }
    (shared, si, has_ref)
}

/// Reads a `<v>`. For the types that are a number or an index (`n` `s` `b` `e`) the value is the
/// first text of the element and nothing else (an entity or CDATA first, or no text, is no value);
/// for the types that are text (`str`, `d`, anything else) it is all the text. The `<v>` of an
/// inline string is redundant and ignored (it also replaces a value read before it).
fn read_v<R: BufRead>(
    rd: &mut XmlReader<R>,
    closing: &[u8],
    bufs: (&mut Vec<u8>, &mut Vec<u8>),
    text: &mut String,
    (t, kind): (T, DateKind),
    tables: &Tables,
    cap: usize,
) -> Result<Pv, OfficeError> {
    let (ev, ev2) = bufs;
    match t {
        T::Inline => {
            rd.read_to_end_into(quick_xml::name::QName(closing), ev)
                .map_err(xml_err)?;
            Ok(Pv::Empty)
        }
        T::Str | T::Iso => {
            text.clear();
            read_text_into(rd, closing, ev, text, cap)?;
            Ok(if t == T::Iso { Pv::Iso } else { Pv::Str })
        }
        T::Number | T::Shared | T::Bool | T::Error => {
            ev.clear();
            let pv = match rd.read_event_into(ev).map_err(xml_err)? {
                Event::Text(raw) => plain_value(&raw, t, kind, tables, text),
                Event::End(e) if e.name().as_ref() == closing => return Ok(Pv::Empty),
                Event::Eof => return Ok(Pv::Empty),
                _ => Pv::Empty,
            };
            rd.read_to_end_into(quick_xml::name::QName(closing), ev2)
                .map_err(xml_err)?;
            Ok(pv)
        }
    }
}

/// The value of a number / index / boolean / error `<v>` from its raw text.
fn plain_value(raw: &[u8], t: T, kind: DateKind, tables: &Tables, text: &mut String) -> Pv {
    match t {
        T::Shared => match parse_uint(raw) {
            Some(i) if i < tables.strings.len() => Pv::Shared(i),
            _ => Pv::Empty,
        },
        T::Bool => Pv::Bool(raw != b"0"),
        T::Error => {
            text.clear();
            text.push_str(&String::from_utf8_lossy(raw));
            Pv::Err
        }
        _ => {
            if raw.is_empty() {
                return Pv::Empty;
            }
            let lit = std::str::from_utf8(raw).ok();
            // A plain integer literal that fits `i64` stays exact (as it is read by calamine).
            if kind == DateKind::Plain {
                if let Some(i) = lit.and_then(parse_int_literal) {
                    return Pv::Int(i);
                }
            }
            // `f64::from_str` also takes `NaN`, `inf` and `infinity`, and overflows to infinity:
            // none of those is a number in a sheet.
            match lit
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
            {
                Some(n) => match kind {
                    DateKind::Plain => Pv::Num(n),
                    DateKind::Date => Pv::Date(n, false),
                    DateKind::Duration => Pv::Date(n, true),
                },
                None => {
                    // Not a number: shown as the text it is.
                    text.clear();
                    text.push_str(&String::from_utf8_lossy(raw));
                    Pv::Str
                }
            }
        }
    }
}

/// An optionally signed run of ASCII digits that fits `i64`.
fn parse_int_literal(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

// ---------------------------------------------------------------------------------------------
// shared formulas
// ---------------------------------------------------------------------------------------------

const MAX_COL: i64 = 16_384;
const MAX_ROW: i64 = 1_048_576;

/// A parsed reference part.
#[derive(Clone, Copy)]
struct Ref {
    kind: RefKind,
    col: i64,
    row: i64,
    abs_col: bool,
    abs_row: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    Cell,
    Col,
    Row,
}

/// `A1`, `$A$1`, `A$1`, `$A1`, `A` / `$A` (a column) or `5` / `$5` (a row), within Excel's grid.
fn parse_ref(s: &str) -> Option<Ref> {
    let b = s.as_bytes();
    let mut i = 0;
    let abs_col = b.first() == Some(&b'$');
    if abs_col {
        i += 1;
    }
    let l0 = i;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    let letters = &b[l0..i];
    if letters.len() > 3 {
        return None;
    }
    let abs_row = i < b.len() && b[i] == b'$';
    if abs_row {
        i += 1;
    }
    let d0 = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &b[d0..i];
    if i != b.len() || digits.len() > 8 {
        return None;
    }
    let col = letters.iter().fold(0i64, |n, c| {
        n * 26 + i64::from(c.to_ascii_uppercase() - b'A') + 1
    });
    let row = digits
        .iter()
        .fold(0i64, |n, d| n * 10 + i64::from(d - b'0'));
    let (kind, abs_col, abs_row) = match (letters.is_empty(), digits.is_empty()) {
        (false, false) => (RefKind::Cell, abs_col, abs_row),
        // `$A` / `A`: the `$` before the digits does not exist; `abs_row` was read as a `$` after
        // the letters, which for a column alone is a syntax error.
        (false, true) if !abs_row => (RefKind::Col, abs_col, false),
        // `$5` / `5`: the one `$` seen before the digits is the row's.
        (true, false) if !abs_row => (RefKind::Row, false, abs_col),
        _ => return None,
    };
    let ok_col = (1..=MAX_COL).contains(&col);
    let ok_row = (1..=MAX_ROW).contains(&row);
    let valid = match kind {
        RefKind::Cell => ok_col && ok_row && digits[0] != b'0',
        RefKind::Col => ok_col,
        RefKind::Row => ok_row && digits[0] != b'0',
    };
    valid.then_some(Ref {
        kind,
        col,
        row,
        abs_col,
        abs_row,
    })
}

impl Ref {
    /// The reference moved by `(dr, dc)`; absolute parts stay. `None` when it leaves the grid.
    fn shifted(self, dr: i64, dc: i64) -> Option<Ref> {
        let col = if self.abs_col {
            self.col
        } else {
            self.col.saturating_add(dc)
        };
        let row = if self.abs_row {
            self.row
        } else {
            self.row.saturating_add(dr)
        };
        let ok_col = (1..=MAX_COL).contains(&col);
        let ok_row = (1..=MAX_ROW).contains(&row);
        let ok = match self.kind {
            RefKind::Cell => ok_col && ok_row,
            // A column range does not move down, a row range does not move across.
            RefKind::Col => ok_col,
            RefKind::Row => ok_row,
        };
        ok.then_some(Ref { col, row, ..self })
    }

    fn write(self, out: &mut String) {
        if self.kind != RefKind::Row {
            if self.abs_col {
                out.push('$');
            }
            let mut letters = [0u8; 4];
            let mut n = self.col;
            let mut len = 0;
            while n > 0 {
                letters[len] = b'A' + ((n - 1) % 26) as u8;
                len += 1;
                n = (n - 1) / 26;
            }
            for k in (0..len).rev() {
                out.push(letters[k] as char);
            }
        }
        if self.kind != RefKind::Col {
            if self.abs_row {
                out.push('$');
            }
            out.push_str(&self.row.to_string());
        }
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'$' | b'_' | b'.' | b':' | b'\\' | b'?') || b >= 0x80
}

/// The formula of a cell derived from a shared formula's anchor, `(dr, dc)` cells away: every
/// relative cell / column / row reference moves, absolute ones (`$A$1`) stay.
///
/// Left as written: string literals, quoted sheet names (`'Q1 2024'!A1` — the sheet name is not a
/// reference), structured references (`Table1[[#This Row],[Qty]]`), error literals (`#REF!`),
/// function and defined names, and any reference that would leave Excel's grid. A range moves both
/// ends (`A1:B2`, `A:A`, `1:1`); a name that merely follows a colon (`A1:INDEX(...)`) does not
/// stop the cell before it from moving.
pub(crate) fn shift_formula(f: &str, dr: i64, dc: i64, out: &mut String) {
    out.clear();
    if dr == 0 && dc == 0 {
        out.push_str(f);
        return;
    }
    let b = f.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                // A quoted run; a doubled quote inside is an escaped one.
                let start = i;
                i += 1;
                while i < n {
                    if b[i] == c {
                        if i + 1 < n && b[i + 1] == c {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.push_str(&f[start..i]);
            }
            b'[' => {
                let start = i;
                let mut depth = 0usize;
                while i < n {
                    match b[i] {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                out.push_str(&f[start..i]);
            }
            c if is_word(c) => {
                let start = i;
                while i < n && is_word(b[i]) {
                    i += 1;
                }
                let is_call = b.get(i) == Some(&b'(');
                shift_token(&f[start..i], is_call, dr, dc, out);
            }
            _ => {
                // Any other ASCII byte (operators, separators); a non-ASCII byte is a word byte.
                out.push(c as char);
                i += 1;
            }
        }
    }
}

/// Moves the references in one run of word characters (`A1`, `$B$2:C3`, `Sheet1`, `A:A`, ...).
fn shift_token(tok: &str, is_call: bool, dr: i64, dc: i64, out: &mut String) {
    let parts: Vec<&str> = tok.split(':').collect();
    if parts.len() == 2 && !is_call {
        if let (Some(a), Some(z)) = (parse_ref(parts[0]), parse_ref(parts[1])) {
            if a.kind == z.kind {
                match (a.shifted(dr, dc), z.shifted(dr, dc)) {
                    (Some(a), Some(z)) => {
                        a.write(out);
                        out.push(':');
                        z.write(out);
                    }
                    _ => out.push_str(tok),
                }
                return;
            }
        }
    }
    let last = parts.len() - 1;
    for (k, part) in parts.iter().enumerate() {
        if k > 0 {
            out.push(':');
        }
        let callee = is_call && k == last;
        match parse_ref(part) {
            Some(r) if r.kind == RefKind::Cell && !callee => match r.shifted(dr, dc) {
                Some(r) => r.write(out),
                None => out.push_str(part),
            },
            _ => out.push_str(part),
        }
    }
}
