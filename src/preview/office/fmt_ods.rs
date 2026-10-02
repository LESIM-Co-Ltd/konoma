//! Format pass for ods (OpenDocument spreadsheets): which number format each cell has, translated
//! into the Excel-style format codes the number-format engine (`numfmt`) understands.
//!
//! `calamine` reads ods values but not their display styles (and it turns LibreOffice's error
//! cells into empty strings), so konoma reads `styles.xml` and `content.xml` itself:
//!
//! 1. `META-INF/manifest.xml`: any `manifest:encryption-data` means the package is encrypted.
//! 2. Styles (`styles.xml`, then the automatic styles of `content.xml`): the `number:*-style`
//!    data styles (translated to a format code) and the `style:style` cell styles that point at
//!    them through `style:data-style-name` (with `style:parent-style-name` inheritance).
//! 3. The body of `content.xml`, streamed: for each cell its style (`table:style-name`, else the
//!    row default, else the column's `table:default-cell-style-name`), repeated cells expanded
//!    with `table:number-columns-repeated` / `table:number-rows-repeated`.
//!
//! **Why this pass also guards memory.** `calamine::Ods::new` parses *every* table (hidden ones
//! too) while opening and builds a dense matrix per table over the bounding box of its non-empty
//! cells; a repeated non-empty cell is even materialised once per repetition on the way
//! (calamine issue #594 is the DoS this allows). The pass therefore *counts* (it never
//! materialises a repeat it has not budgeted): the bounding box, the number of value cells and the
//! cells calamine builds per physical row are all checked against `Limits::max_dense_cells`
//! before calamine sees the file.
//!
//! Translation table (ods -> Excel code), `n` = decimals:
//!
//! | ods                                                        | code                          |
//! |------------------------------------------------------------|-------------------------------|
//! | `number:number` places n, min-integer m, grouping          | `#,##0.00` (`0`s per m)       |
//! | `number:number` min-decimal-places k < n                   | `0.00###` (optional decimals) |
//! | `number:number` without decimal-places                     | `General`                     |
//! | `number:display-factor` 1000^k                             | k trailing commas             |
//! | `number:scientific-number`                                 | `0.00E+00`                    |
//! | `number:fraction`                                          | `# ?/?` / `# ?/8`             |
//! | `number:text` / `number:currency-symbol`                   | quoted literal                |
//! | `%` text in a `percentage-style`                           | `%` (scales by 100)           |
//! | `number:year` short / long (gengou: `e` / `ee`)            | `yy` / `yyyy`                 |
//! | `number:era` calendar gengou short / long                  | `gg` / `ggg`                  |
//! | `number:month` / textual / long                            | `m` `mm` `mmm` `mmmm`         |
//! | `number:day`, `number:day-of-week` short / long            | `d` `dd`, `ddd` `dddd`        |
//! | `number:hours` / `minutes` / `seconds` short / long        | `h` `hh` / `m` `mm` / `s` `ss`|
//! | `number:seconds decimal-places` n                          | `ss.000`                      |
//! | `truncate-on-overflow="false"` (elapsed time)              | `[hh]` `[mm]` `[ss]`          |
//! | `number:am-pm`                                             | `AM/PM`                       |
//! | `number:text-content` (text style)                         | `@`                           |
//! | `number:boolean`                                           | `General`                     |
//! | `style:map` `value()>=0` -> style A (main style = negative)| `A;main`                      |
//! | `style:map` `value()>0` + `value()<0`                      | `A;B;main`                    |
//! | other `style:map` conditions                               | `[cond]A;main`                |
//! | `fo:color` red / blue / ... on a data style                | `[Red]` / `[Blue]` ...        |
//!
//! Not translated (shown without the element): `number:quarter`, `number:week-of-year`,
//! `number:fill-character`, `number:decimal-replacement`, text styles' conditions. The ods
//! `table:null-date` (1904 date system) is not read: ods dates are ISO strings in the file.

use std::collections::HashMap;
use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::container::{self, Limits};
use super::fmt_xlsx::{keep_format_code, SheetFormats, XlsxFormats, STRING_OVERHEAD};
use super::workbook::{CellError, NumFmtRef};
use super::OfficeError;

/// Excel's own limits, which `calamine`'s ods reader also applies while expanding repeats.
const MAX_ROWS: u64 = 1_048_576;
const MAX_COLS: u64 = 16_384;
/// Most error cells remembered per sheet (an error repeated across a huge area stays empty text).
const MAX_ERRORS: usize = 100_000;
/// Safety caps on the style tables (a real file has dozens).
const MAX_STYLES: usize = 100_000;
const MAX_PARTS: usize = 256;
/// Longest `style:parent-style-name` / `style:apply-style-name` chain followed.
const MAX_DEPTH: usize = 8;

/// Reads the format information of an ods package. Errors for any table that is too large for
/// `calamine` to open safely (it parses hidden tables too).
pub fn read(path: &std::path::Path, limits: &Limits) -> Result<XlsxFormats, OfficeError> {
    let mut zip = container::open_zip(path)?;
    let cap = limits.max_part_bytes;

    if let Some(r) = container::part_reader(&mut zip, "META-INF/manifest.xml", cap)? {
        if manifest_is_encrypted(std::io::BufReader::new(r))? {
            return Err(OfficeError::Encrypted);
        }
    }

    let mut styles = Styles::default();
    if let Some(r) = container::part_reader(&mut zip, "styles.xml", cap)? {
        parse_styles(std::io::BufReader::new(r), &mut styles, false)?;
    }
    {
        let r = container::part_reader(&mut zip, "content.xml", cap)?
            .ok_or_else(|| OfficeError::Corrupt("missing content.xml".into()))?;
        parse_styles(std::io::BufReader::new(r), &mut styles, true)?;
    }

    let mut table = FormatTable::new();
    let sheets = {
        let r = container::part_reader(&mut zip, "content.xml", cap)?
            .ok_or_else(|| OfficeError::Corrupt("missing content.xml".into()))?;
        parse_body(std::io::BufReader::new(r), &styles, &mut table, limits)?
    };
    // So is the area (every table's matrix stays in memory once the file is open).
    SheetFormats::check_total_area(
        sheets
            .values()
            .map(SheetFormats::bbox_area)
            .fold(0, u64::saturating_add),
        limits,
    )?;
    // The text budget is for the whole workbook (`calamine` reads every table).
    let text: u64 = sheets.values().map(|s| s.text_bytes).sum();
    if text > limits.max_text_bytes {
        return Err(OfficeError::TooLarge { what: "text" });
    }
    Ok(XlsxFormats {
        date1904: false,
        formats: table.formats,
        sheets,
    })
}

// ---------------------------------------------------------------------------------------------
// XML helpers
// ---------------------------------------------------------------------------------------------

fn new_reader<R: BufRead>(src: R) -> Reader<R> {
    let mut rd = Reader::from_reader(src);
    // `<a/>` arrives as Start + End, so every element is handled in one place.
    rd.config_mut().expand_empty_elements = true;
    rd
}

fn xml_err(e: quick_xml::Error) -> OfficeError {
    OfficeError::Corrupt(format!("xml: {e}"))
}

/// The attribute with the exact qualified name `key` (`table:style-name`). An undefined entity
/// falls back to the raw text; nothing is ever expanded.
fn qattr(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.as_ref() == key {
            return Some(match a.normalized_value(XmlVersion::Implicit1_0) {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            });
        }
    }
    None
}

fn has_attr(e: &BytesStart<'_>, key: &[u8]) -> bool {
    e.attributes()
        .with_checks(false)
        .flatten()
        .any(|a| a.key.as_ref() == key)
}

fn attr_usize(e: &BytesStart<'_>, key: &[u8]) -> Option<usize> {
    qattr(e, key).and_then(|v| v.trim().parse().ok())
}

fn attr_true(e: &BytesStart<'_>, key: &[u8]) -> bool {
    qattr(e, key).is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

/// Appends the character data of a text / entity-reference event to `out`.
fn push_text(ev: &Event<'_>, out: &mut String) {
    match ev {
        Event::Text(t) => {
            if let Ok(s) = t.xml10_content() {
                out.push_str(&s);
            }
        }
        Event::GeneralRef(r) => {
            if let Ok(name) = r.decode() {
                match &*name {
                    "lt" => out.push('<'),
                    "gt" => out.push('>'),
                    "amp" => out.push('&'),
                    "apos" => out.push('\''),
                    "quot" => out.push('"'),
                    _ => {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            out.push(c);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// manifest (encryption)
// ---------------------------------------------------------------------------------------------

pub(crate) fn manifest_is_encrypted(src: impl BufRead) -> Result<bool, OfficeError> {
    let mut rd = new_reader(src);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) if e.local_name().as_ref() == b"encryption-data" => return Ok(true),
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// data styles -> Excel format codes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Percentage,
    Currency,
    Date,
    Time,
    Boolean,
    Text,
}

/// One element of a data style, already translated.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// Literal text (quoted when emitted).
    Lit(String),
    /// A ready format-code token (`yyyy`, `#,##0.00`, `@`, ...).
    Code(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Condition {
    /// `>=`, `>`, `<=`, `<`, `=`, `<>`.
    op: &'static str,
    /// The number as written.
    value: String,
}

#[derive(Debug, Clone)]
struct DataStyle {
    kind: Kind,
    toks: Vec<Tok>,
    maps: Vec<(Condition, String)>,
    color: Option<&'static str>,
}

#[derive(Debug, Clone, Default)]
struct CellStyle {
    parent: Option<String>,
    data_style: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Styles {
    data: HashMap<String, DataStyle>,
    cells: HashMap<String, CellStyle>,
}

impl Styles {
    /// The data-style name of a cell style, following `style:parent-style-name`.
    fn data_style_of(&self, cell_style: &str) -> Option<&str> {
        let mut name = cell_style;
        for _ in 0..MAX_DEPTH {
            let cs = self.cells.get(name)?;
            if let Some(d) = &cs.data_style {
                return Some(d);
            }
            name = cs.parent.as_deref()?;
        }
        None
    }

    /// The Excel format code of a cell style, `None` when it has no data style.
    pub(crate) fn code_of_cell_style(&self, cell_style: &str) -> Option<String> {
        let ds = self.data_style_of(cell_style)?;
        self.code_of_data_style(ds, 0)
    }

    fn code_of_data_style(&self, name: &str, depth: usize) -> Option<String> {
        let ds = self.data.get(name)?;
        let main = section(ds);
        if depth >= MAX_DEPTH || ds.maps.is_empty() {
            return Some(main);
        }
        // Conditional sections (negative numbers in red, ...): the applied style of each map.
        let applied: Vec<(&Condition, String)> = ds
            .maps
            .iter()
            .take(2)
            .filter_map(|(c, n)| Some((c, self.code_of_data_style(n, depth + 1)?)))
            .collect();
        Some(combine(&main, &applied))
    }
}

/// The sections of a main style and its `style:map`s.
fn combine(main: &str, maps: &[(&Condition, String)]) -> String {
    let is = |c: &Condition, op: &str, v: &str| c.op == op && c.value == v;
    match maps {
        [] => main.to_string(),
        [(c, a)] if is(c, ">=", "0") => format!("{a};{main}"),
        [(c, a)] if is(c, "<", "0") => format!("{main};{a}"),
        [(c, a)] => format!("[{}{}]{a};{main}", c.op, c.value),
        [(c1, a), (c2, b)] if is(c1, ">", "0") && is(c2, "<", "0") => format!("{a};{b};{main}"),
        [(c1, a), (c2, b)] if is(c1, ">=", "0") && is(c2, "<", "0") => format!("{a};{b}"),
        [(c1, a), (c2, b)] => format!(
            "[{}{}]{a};[{}{}]{b};{main}",
            c1.op, c1.value, c2.op, c2.value
        ),
        _ => main.to_string(),
    }
}

/// One section of a format code from a data style (no maps).
fn section(ds: &DataStyle) -> String {
    let mut out = String::new();
    if let Some(c) = ds.color {
        out.push('[');
        out.push_str(c);
        out.push(']');
    }
    let mut body = String::new();
    for t in &ds.toks {
        match t {
            Tok::Code(c) => body.push_str(c),
            Tok::Lit(s) => push_literal(&mut body, s, ds.kind),
        }
    }
    if body.is_empty() {
        body.push_str("General");
    }
    out.push_str(&body);
    out
}

/// Appends `s` as format-code literal text. Safe separators stay bare; everything else is quoted
/// (`"` itself is backslash-escaped). In a percentage style a `%` is the percent *operator*
/// (scales by 100); in a date/time style `,` is plain text (in a number it is the thousands mark).
fn push_literal(out: &mut String, s: &str, kind: Kind) {
    let percent = kind == Kind::Percentage;
    let bare_comma = matches!(kind, Kind::Date | Kind::Time);
    let mut quoted = false;
    let close = |out: &mut String, quoted: &mut bool| {
        if *quoted {
            out.push('"');
            *quoted = false;
        }
    };
    for ch in s.chars() {
        if percent && ch == '%' {
            close(out, &mut quoted);
            out.push('%');
        } else if matches!(ch, ' ' | '-' | '/' | ':' | '(' | ')' | '+') || (bare_comma && ch == ',')
        {
            close(out, &mut quoted);
            out.push(ch);
        } else if ch == '"' {
            close(out, &mut quoted);
            out.push_str("\\\"");
        } else if ch.is_control() {
            // Line breaks and the like cannot live in a one-line format code.
        } else {
            if !quoted {
                out.push('"');
                quoted = true;
            }
            out.push(ch);
        }
    }
    close(out, &mut quoted);
}

/// `value()>=0` -> `>=` / `0`. Anything that is not a plain numeric comparison is `None`.
fn parse_condition(s: &str) -> Option<Condition> {
    let rest: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let rest = rest.strip_prefix("value()")?;
    let (op, num) = [">=", "<=", "!=", "<>", ">", "<", "="]
        .iter()
        .find_map(|op| rest.strip_prefix(op).map(|n| (*op, n)))?;
    num.parse::<f64>().ok()?;
    Some(Condition {
        op: match op {
            "!=" | "<>" => "<>",
            ">=" => ">=",
            "<=" => "<=",
            ">" => ">",
            "<" => "<",
            _ => "=",
        },
        value: num.to_string(),
    })
}

fn excel_color(hex: &str) -> Option<&'static str> {
    Some(match hex.trim().to_ascii_lowercase().as_str() {
        "#ff0000" => "Red",
        "#0000ff" => "Blue",
        "#00ff00" => "Green",
        "#ffff00" => "Yellow",
        "#ff00ff" => "Magenta",
        "#00ffff" => "Cyan",
        "#000000" => "Black",
        "#ffffff" => "White",
        _ => return None,
    })
}

fn number_token(e: &BytesStart<'_>) -> String {
    let decimals = attr_usize(e, b"number:decimal-places");
    let min_dec = attr_usize(e, b"number:min-decimal-places");
    let min_int = attr_usize(e, b"number:min-integer-digits").unwrap_or(1);
    let grouping = attr_true(e, b"number:grouping");
    let (dec, opt) = match (decimals, min_dec) {
        (None, None) => {
            // No decimal-places: the style is "as many as needed".
            return if grouping {
                "#,##0.##########".into()
            } else {
                "General".into()
            };
        }
        (Some(d), None) => (d, 0),
        (None, Some(m)) => (m, 0),
        (Some(d), Some(m)) => (m.min(d), d.saturating_sub(m)),
    };
    let dec = dec.min(30);
    let opt = opt.min(30 - dec);
    let width = min_int.clamp(1, 30);
    let digits = if grouping { width.max(4) } else { width };
    let mut int = String::new();
    for i in 0..digits {
        // `i` counts from the left; the last `width` positions are required zeros.
        int.push(if digits - i <= width { '0' } else { '#' });
    }
    if grouping {
        let mut grouped = String::new();
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (digits - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        int = grouped;
    }
    let mut out = int;
    if dec + opt > 0 {
        out.push('.');
        out.push_str(&"0".repeat(dec));
        out.push_str(&"#".repeat(opt));
    }
    // `number:display-factor` 1000^k scales down: one trailing comma per factor of 1000.
    if let Some(f) = qattr(e, b"number:display-factor").and_then(|v| v.trim().parse::<u64>().ok()) {
        let (mut n, mut commas) = (f, 0);
        while n >= 1000 && n % 1000 == 0 && commas < 4 {
            n /= 1000;
            commas += 1;
        }
        if n == 1 {
            out.push_str(&",".repeat(commas));
        }
    }
    out
}

fn scientific_token(e: &BytesStart<'_>) -> String {
    let dec = attr_usize(e, b"number:decimal-places").unwrap_or(2).min(30);
    let min_int = attr_usize(e, b"number:min-integer-digits")
        .unwrap_or(1)
        .clamp(1, 30);
    let exp = attr_usize(e, b"number:min-exponent-digits")
        .unwrap_or(2)
        .clamp(1, 5);
    let mut out = "0".repeat(min_int);
    if dec > 0 {
        out.push('.');
        out.push_str(&"0".repeat(dec));
    }
    out.push_str("E+");
    out.push_str(&"0".repeat(exp));
    out
}

fn fraction_token(e: &BytesStart<'_>) -> String {
    let min_int = attr_usize(e, b"number:min-integer-digits").unwrap_or(0);
    let num = attr_usize(e, b"number:min-numerator-digits")
        .unwrap_or(1)
        .clamp(1, 10);
    let den = attr_usize(e, b"number:min-denominator-digits")
        .unwrap_or(1)
        .clamp(1, 10);
    let mut out = String::new();
    if min_int > 0 {
        out.push_str("# ");
    }
    out.push_str(&"?".repeat(num));
    out.push('/');
    match attr_usize(e, b"number:denominator-value").filter(|&d| d > 0) {
        Some(d) => out.push_str(&d.to_string()),
        None => out.push_str(&"?".repeat(den)),
    }
    out
}

/// A date/time element -> its code token. `None` for elements that have no Excel equivalent.
fn date_token(name: &[u8], e: &BytesStart<'_>) -> Option<String> {
    let long = qattr(e, b"number:style").is_some_and(|s| s == "long");
    let gengou = qattr(e, b"number:calendar").is_some_and(|c| c == "gengou");
    let elapsed = qattr(e, b"number:truncate-on-overflow").is_some_and(|v| v == "false");
    let pick = |s: &str, l: &str| if long { l } else { s }.to_string();
    let unit = |s: &str, l: &str| {
        let t = pick(s, l);
        if elapsed {
            format!("[{t}]")
        } else {
            t
        }
    };
    Some(match name {
        b"number:year" => {
            if gengou {
                pick("e", "ee")
            } else {
                pick("yy", "yyyy")
            }
        }
        b"number:era" => pick("gg", "ggg"),
        b"number:month" => {
            if attr_true(e, b"number:textual") {
                pick("mmm", "mmmm")
            } else {
                pick("m", "mm")
            }
        }
        b"number:day" => pick("d", "dd"),
        b"number:day-of-week" => pick("ddd", "dddd"),
        b"number:hours" => unit("h", "hh"),
        b"number:minutes" => unit("m", "mm"),
        b"number:seconds" => {
            let mut s = unit("s", "ss");
            if let Some(d) = attr_usize(e, b"number:decimal-places").filter(|&d| d > 0) {
                s.push('.');
                s.push_str(&"0".repeat(d.min(6)));
            }
            s
        }
        b"number:am-pm" => "AM/PM".to_string(),
        _ => return None,
    })
}

fn kind_of(name: &[u8]) -> Option<Kind> {
    Some(match name {
        b"number:number-style" => Kind::Number,
        b"number:percentage-style" => Kind::Percentage,
        b"number:currency-style" => Kind::Currency,
        b"number:date-style" => Kind::Date,
        b"number:time-style" => Kind::Time,
        b"number:boolean-style" => Kind::Boolean,
        b"number:text-style" => Kind::Text,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// styles.xml / the automatic styles of content.xml
// ---------------------------------------------------------------------------------------------

/// Reads data styles and cell styles from a `styles.xml` or `content.xml`. With `stop_at_body`
/// the scan ends where the document body starts (content.xml: styles precede the body).
pub(crate) fn parse_styles(
    src: impl BufRead,
    out: &mut Styles,
    stop_at_body: bool,
) -> Result<(), OfficeError> {
    let mut rd = new_reader(src);
    let mut buf = Vec::new();
    let mut cur: Option<(String, DataStyle)> = None;
    // Text collected for `number:text` / `number:currency-symbol`.
    let mut text: Option<String> = None;
    loop {
        buf.clear();
        let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
        match &ev {
            Event::Start(e) => {
                let name = e.name();
                let name = name.as_ref();
                if name == b"office:body" && stop_at_body {
                    break;
                }
                if let Some(kind) = kind_of(name) {
                    if let Some(n) = qattr(e, b"style:name") {
                        cur = Some((
                            n,
                            DataStyle {
                                kind,
                                toks: Vec::new(),
                                maps: Vec::new(),
                                color: None,
                            },
                        ));
                    }
                    continue;
                }
                if name == b"style:style" {
                    if qattr(e, b"style:family").is_some_and(|f| f == "table-cell")
                        && out.cells.len() < MAX_STYLES
                    {
                        if let Some(n) = qattr(e, b"style:name") {
                            out.cells.insert(
                                n,
                                CellStyle {
                                    parent: qattr(e, b"style:parent-style-name"),
                                    data_style: qattr(e, b"style:data-style-name"),
                                },
                            );
                        }
                    }
                    continue;
                }
                let Some((_, ds)) = cur.as_mut() else {
                    continue;
                };
                match name {
                    b"style:map" => {
                        if let (Some(c), Some(a)) = (
                            qattr(e, b"style:condition")
                                .as_deref()
                                .and_then(parse_condition),
                            qattr(e, b"style:apply-style-name"),
                        ) {
                            if ds.maps.len() < 8 {
                                ds.maps.push((c, a));
                            }
                        }
                    }
                    b"style:text-properties" => {
                        if let Some(c) = qattr(e, b"fo:color").as_deref().and_then(excel_color) {
                            ds.color = Some(c);
                        }
                    }
                    b"number:text" | b"number:currency-symbol" => text = Some(String::new()),
                    _ if ds.toks.len() >= MAX_PARTS => {}
                    b"number:number" => ds.toks.push(Tok::Code(number_token(e))),
                    b"number:scientific-number" => ds.toks.push(Tok::Code(scientific_token(e))),
                    b"number:fraction" => ds.toks.push(Tok::Code(fraction_token(e))),
                    b"number:text-content" => ds.toks.push(Tok::Code("@".into())),
                    b"number:boolean" => ds.toks.push(Tok::Code("General".into())),
                    _ => {
                        if let Some(t) = date_token(name, e) {
                            ds.toks.push(Tok::Code(t));
                        }
                    }
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => {
                if let Some(t) = text.as_mut() {
                    if t.len() < 1024 {
                        push_text(&ev, t);
                    }
                }
            }
            Event::End(e) => {
                let name = e.name();
                let name = name.as_ref();
                if name == b"number:text" || name == b"number:currency-symbol" {
                    if let (Some(t), Some((_, ds))) = (text.take(), cur.as_mut()) {
                        if !t.is_empty() && ds.toks.len() < MAX_PARTS {
                            ds.toks.push(Tok::Lit(t));
                        }
                    }
                } else if kind_of(name).is_some() {
                    if let Some((n, ds)) = cur.take() {
                        if out.data.len() < MAX_STYLES {
                            out.data.insert(n, ds);
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// the body: cells
// ---------------------------------------------------------------------------------------------

/// The deduplicated format table (index 0 = General).
struct FormatTable {
    formats: Vec<NumFmtRef>,
    index: HashMap<NumFmtRef, u16>,
}

impl FormatTable {
    fn new() -> FormatTable {
        let mut index = HashMap::new();
        index.insert(NumFmtRef::General, 0);
        FormatTable {
            formats: vec![NumFmtRef::General],
            index,
        }
    }

    fn intern_code(&mut self, code: &str) -> u16 {
        // Longer than Excel allows: not a format Excel wrote, shown as General (and never kept).
        if !keep_format_code(code) {
            return 0;
        }
        let fmt = if code.trim().eq_ignore_ascii_case("general") {
            NumFmtRef::General
        } else {
            NumFmtRef::Custom(code.into())
        };
        if let Some(&i) = self.index.get(&fmt) {
            return i;
        }
        match u16::try_from(self.formats.len()) {
            Ok(i) => {
                self.formats.push(fmt.clone());
                self.index.insert(fmt, i);
                i
            }
            Err(_) => 0,
        }
    }
}

/// Resolves cell-style names to format indexes (memoised: a sheet has few distinct styles).
struct StyleResolver<'a> {
    styles: &'a Styles,
    table: &'a mut FormatTable,
    memo: HashMap<String, u16>,
}

impl StyleResolver<'_> {
    /// The format index of a cell style (0 when it has no data style).
    fn resolve(&mut self, style: &str) -> u16 {
        if let Some(&i) = self.memo.get(style) {
            return i;
        }
        let i = match self.styles.code_of_cell_style(style) {
            Some(code) => self.table.intern_code(&code),
            None => 0,
        };
        self.memo.insert(style.to_string(), i);
        i
    }
}

/// The value kind of a cell without any style, for the default look LibreOffice would give it.
fn default_code(value_type: &str, date_value: Option<&str>) -> Option<&'static str> {
    match value_type {
        "date" => Some(if date_value.is_some_and(|d| d.contains(['T', 't'])) {
            "yyyy-mm-dd hh:mm:ss"
        } else {
            "yyyy-mm-dd"
        }),
        "time" => Some("[hh]:mm:ss"),
        "percentage" => Some("0.00%"),
        _ => None,
    }
}

/// The `table:table` being read.
struct TableState {
    name: String,
    out: SheetFormats,
    /// `(end column exclusive, default cell style)` per `table:table-column` run.
    col_defaults: Vec<(u64, Option<String>)>,
    /// Rows consumed so far (repeats included, capped like calamine).
    row_cursor: u64,
    // The row being read.
    row_default: Option<String>,
    row_reps: u64,
    row_width: u64,
    col_cursor: u64,
    cell: Option<CellState>,
}

/// The cell being read: everything is decided at its start tag; an error cell's text is
/// collected until its end tag.
struct CellState {
    reps: u64,
    start_col: u64,
    style: Option<String>,
    value_type: String,
    date_value: Option<String>,
    nonempty: bool,
    is_error: bool,
    text: String,
    /// Bytes of text the cell holds (its `string-value`, its paragraphs, its formula), for the
    /// text budget: a repeated cell holds them once per repetition.
    text_len: u64,
}

fn parse_body(
    src: impl BufRead,
    styles: &Styles,
    table: &mut FormatTable,
    limits: &Limits,
) -> Result<HashMap<String, SheetFormats>, OfficeError> {
    let mut rd = new_reader(src);
    let mut buf = Vec::new();
    let mut sheets: HashMap<String, SheetFormats> = HashMap::new();
    let mut cur: Option<TableState> = None;
    // Depth of `table:table` elements nested inside the current one (ignored wholesale).
    let mut nested = 0usize;
    let mut resolver_memo: HashMap<String, u16> = HashMap::new();
    let max = limits.max_dense_cells;
    loop {
        buf.clear();
        let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
        match &ev {
            Event::Start(e) => {
                let name = e.name();
                let name = name.as_ref();
                if name == b"table:table" {
                    if cur.is_some() {
                        nested += 1;
                    } else if let Some(n) = qattr(e, b"table:name") {
                        cur = Some(TableState {
                            name: n,
                            out: SheetFormats::default(),
                            col_defaults: Vec::new(),
                            row_cursor: 0,
                            row_default: None,
                            row_reps: 1,
                            row_width: 0,
                            col_cursor: 0,
                            cell: None,
                        });
                    }
                    continue;
                }
                let Some(t) = cur.as_mut() else { continue };
                if nested > 0 {
                    continue;
                }
                match name {
                    b"table:table-column" => {
                        let reps = rep_attr(e, b"table:number-columns-repeated");
                        let start = t.col_defaults.last().map_or(0, |d| d.0);
                        let end = (start + reps).min(MAX_COLS);
                        if end > start {
                            t.col_defaults
                                .push((end, qattr(e, b"table:default-cell-style-name")));
                        }
                    }
                    b"table:table-row" => {
                        let reps = rep_attr(e, b"table:number-rows-repeated");
                        // Like calamine: the repeats of all rows together stop at Excel's last row.
                        t.row_reps = reps.min(MAX_ROWS.saturating_sub(t.row_cursor));
                        t.row_default = qattr(e, b"table:default-cell-style-name");
                        t.row_width = 0;
                        t.col_cursor = 0;
                    }
                    b"table:table-cell" | b"table:covered-table-cell" => {
                        let reps = rep_attr(e, b"table:number-columns-repeated");
                        let start_col = t.col_cursor;
                        let reps = reps.min(MAX_COLS.saturating_sub(start_col));
                        t.col_cursor = start_col + reps;
                        let value_type = qattr(e, b"office:value-type").unwrap_or_default();
                        let date_value = qattr(e, b"office:date-value");
                        let has_value = [
                            &b"office:value"[..],
                            b"office:string-value",
                            b"office:date-value",
                            b"office:time-value",
                            b"office:boolean-value",
                        ]
                        .iter()
                        .any(|k| has_attr(e, k))
                            || value_type == "string";
                        let formula_len = qattr(e, b"table:formula").map_or(0, |f| f.len() as u64);
                        let has_formula = formula_len > 0;
                        let value_type_is_text = matches!(value_type.as_str(), "string" | "");
                        let string_len =
                            qattr(e, b"office:string-value").map_or(0, |v| v.len() as u64);
                        t.cell = Some(CellState {
                            reps,
                            start_col,
                            style: qattr(e, b"table:style-name"),
                            value_type,
                            date_value,
                            nonempty: (has_value || has_formula) && reps > 0,
                            is_error: qattr(e, b"calcext:value-type").is_some_and(|v| v == "error"),
                            text: String::new(),
                            // Only strings keep their text (numbers and dates are read from
                            // attributes); every cell keeps its formula.
                            text_len: formula_len + if value_type_is_text { string_len } else { 0 },
                        });
                    }
                    _ => {}
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => {
                if nested == 0 {
                    if let Some(c) = cur.as_mut().and_then(|t| t.cell.as_mut()) {
                        if matches!(c.value_type.as_str(), "string" | "") {
                            c.text_len += match &ev {
                                Event::Text(t) => t.len() as u64,
                                _ => 1,
                            };
                        }
                        if c.is_error && c.text.len() < 256 {
                            push_text(&ev, &mut c.text);
                        }
                    }
                }
            }
            Event::End(e) => {
                let name = e.name();
                let name = name.as_ref();
                if name == b"table:table" {
                    if nested > 0 {
                        nested -= 1;
                    } else if let Some(mut t) = cur.take() {
                        // Real files are in order already; a hostile one cannot break lookups.
                        t.out.cells.sort_unstable_by_key(|&(r, c, _)| (r, c));
                        t.out.errors.sort_unstable_by_key(|&(r, c, _)| (r, c));
                        sheets.insert(t.name, t.out);
                    }
                    continue;
                }
                let Some(t) = cur.as_mut() else { continue };
                if nested > 0 {
                    continue;
                }
                match name {
                    b"table:table-cell" | b"table:covered-table-cell" => {
                        if let Some(c) = t.cell.take() {
                            let mut res = StyleResolver {
                                styles,
                                table,
                                memo: std::mem::take(&mut resolver_memo),
                            };
                            let r = finish_cell(t, c, &mut res, limits);
                            resolver_memo = res.memo;
                            r?;
                        }
                    }
                    b"table:table-row" => {
                        t.row_cursor += t.row_reps;
                        // calamine builds one row of cells per physical row, repeats or not.
                        t.out.read_cost = t.out.read_cost.saturating_add(t.row_width);
                        if t.out.read_cost > max {
                            return Err(OfficeError::TooLarge { what: "sheet area" });
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sheets)
}

/// `number-*-repeated`, at least 1 (a malformed value counts as 1).
fn rep_attr(e: &BytesStart<'_>, key: &[u8]) -> u64 {
    qattr(e, key)
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(1)
        .max(1)
}

/// Books one finished cell: the budget first (arithmetically, before anything is expanded), then
/// its format and error value for every repetition.
fn finish_cell(
    t: &mut TableState,
    c: CellState,
    res: &mut StyleResolver<'_>,
    limits: &Limits,
) -> Result<(), OfficeError> {
    let max = limits.max_dense_cells;
    if !c.nonempty {
        return Ok(());
    }
    let row0 = t.row_cursor;
    let rows = t.row_reps;
    let col0 = c.start_col;
    t.row_width = t.row_width.max(col0 + c.reps);
    if rows == 0 {
        return Ok(());
    }

    let count = rows.saturating_mul(c.reps);
    if c.text_len > 0 {
        // Every repetition is a copy of the text in the reader and again in the loader.
        let cost = (c.text_len + STRING_OVERHEAD).saturating_mul(count);
        t.out.add_text(cost, limits)?;
    }
    t.out.value_cells = t.out.value_cells.saturating_add(count);
    if t.out.value_cells > max {
        return Err(OfficeError::TooLarge {
            what: "sheet cells",
        });
    }
    let (r1, c1) = (row0 + rows - 1, col0 + c.reps - 1);
    t.out.bbox = Some(match t.out.bbox {
        None => (row0 as u32, col0 as u32, r1 as u32, c1 as u32),
        Some((a, b, c_, d)) => (
            a.min(row0 as u32),
            b.min(col0 as u32),
            c_.max(r1 as u32),
            d.max(c1 as u32),
        ),
    });
    if t.out.bbox_area() > max {
        return Err(OfficeError::TooLarge { what: "sheet area" });
    }

    // The error value: remembered per repetition (capped).
    let error = c.is_error.then(|| error_code(&c.text));
    // Formats: cell style, else the row default, else the column default.
    let own_style = c.style.as_deref().or(t.row_default.as_deref());
    let own_fmt = own_style.map(|s| res.resolve(s));
    for dc in 0..c.reps {
        let col = col0 + dc;
        let (fmt, styled) = match own_fmt {
            Some(f) => (f, true),
            None => {
                let i = t.col_defaults.partition_point(|d| d.0 <= col);
                match t.col_defaults.get(i).and_then(|d| d.1.as_deref()) {
                    Some(s) => (res.resolve(s), true),
                    None => (0, false),
                }
            }
        };
        let fmt = if fmt == 0 && !styled {
            default_code(&c.value_type, c.date_value.as_deref())
                .map_or(0, |code| res.table.intern_code(code))
        } else {
            fmt
        };
        for dr in 0..rows {
            let row = (row0 + dr) as u32;
            if fmt != 0 {
                t.out.cells.push((row, col as u16, fmt));
            }
            if let Some(code) = error {
                if t.out.errors.len() < MAX_ERRORS {
                    t.out.errors.push((row, col as u32, code));
                }
            }
        }
    }
    Ok(())
}

/// The Excel error code for the text LibreOffice shows in an error cell (`#DIV/0!`, `Err:532`).
fn error_code(text: &str) -> CellError {
    match text.trim() {
        "#DIV/0!" | "Err:532" => "#DIV/0!",
        "#N/A" => "#N/A",
        "#NAME?" | "Err:525" => "#NAME?",
        "#NULL!" => "#NULL!",
        "#NUM!" | "Err:503" => "#NUM!",
        "#REF!" | "Err:524" => "#REF!",
        _ => "#VALUE!",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_style_with_maps(n: usize) -> Styles {
        let maps: String = (0..n)
            .map(|i| {
                format!(
                    r#"<style:map style:condition="value()&gt;{i}" style:apply-style-name="X"/>"#
                )
            })
            .collect();
        let xml = format!(
            r#"<r xmlns:number="urn:n" xmlns:style="urn:s"><number:number-style style:name="N">{maps}</number:number-style></r>"#
        );
        let mut st = Styles::default();
        parse_styles(xml.as_bytes(), &mut st, false).unwrap();
        st
    }

    #[test]
    fn a_data_style_keeps_at_most_eight_maps() {
        // The code only ever uses two, so the cap is about memory: a hostile style cannot grow
        // without bound.
        assert_eq!(data_style_with_maps(3).data["N"].maps.len(), 3);
        assert_eq!(data_style_with_maps(8).data["N"].maps.len(), 8);
        assert_eq!(data_style_with_maps(9).data["N"].maps.len(), 8);
        assert_eq!(data_style_with_maps(500).data["N"].maps.len(), 8);
    }
}
