//! Tests of konoma's own xlsx reader (`xlsx.rs`).
//!
//! The centre is a **differential test against `calamine`**: `calamine` still reads xlsb / xls / ods
//! and stays in the dependency graph, so here it is the oracle for the xlsx reader. The position,
//! value and formula of every cell, the sheet list (names, visibility, chart sheets), the date
//! system and the merged ranges of
//!
//! * every `testdata/office/*.xlsx` and `samples/sample.xlsx`, and
//! * a few dozen generated workbooks (every cell type, shared / inline / rich strings with
//!   phonetic runs and escapes, date / duration / number formats of every family, cells and rows
//!   without `r`, blank styled cells, shared and array formulas)
//!
//! must be identical. A difference is only ever allowed where `calamine` is the one that is wrong
//! or gives up; each such place is listed in [`known_differences`] (and has a test of its own that
//! pins both sides), so a new difference fails the test instead of being absorbed.
//!
//! The rest: the shared-formula expansion (a table, a randomised comparison with
//! `calamine::expand_shared_formula`, and the cases where konoma is right and `calamine` is not),
//! the number-format date detection, the text rules, and the safety properties (hostile input
//! never panics — this code is called *outside* `catch_silent` here — and never allocates past a
//! budget).

use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use calamine::{Data, Reader};

use super::container::Limits;
use super::tests::{deflated, mutate, read_parts, tmp, write, xorshift, Pkg};
use super::workbook::{Cancel, MergeRange, NumFmtRef};
use super::xlsx::{self, date_kind, shift_formula, DateKind, SharedStrings, Tables, Val};
use super::*;

// ---------------------------------------------------------------------------------------------
// the comparison model
// ---------------------------------------------------------------------------------------------

/// A cell value in a form both readers can be reduced to.
#[derive(Debug, Clone, PartialEq)]
enum V {
    Num(f64),
    Date(f64, bool),
    Text(String),
    Bool(bool),
    Err(String),
    Iso(String),
}

type Grid = BTreeMap<(u32, u32), V>;
type Formulas = BTreeMap<(u32, u32), String>;

/// Everything one reader says about one sheet.
#[derive(Debug, Default, PartialEq)]
struct SheetRead {
    values: Grid,
    formulas: Formulas,
    merges: Vec<(u32, u32, u32, u32)>,
}

#[derive(Debug, PartialEq)]
struct BookRead {
    date1904: bool,
    /// `(name, hidden state)` of every worksheet, in order.
    sheets: Vec<(String, u8)>,
    /// The sheet reads, in the same order; `Err` is the reader's reason for giving up on a sheet.
    reads: Vec<Result<SheetRead, String>>,
}

fn from_val(v: Val<'_>) -> V {
    match v {
        Val::Number(n) => V::Num(n),
        Val::Date { serial, duration } => V::Date(serial, duration),
        Val::Text(t) => V::Text(t.to_string()),
        Val::Bool(b) => V::Bool(b),
        Val::Error(e) => V::Err(e.to_string()),
        Val::Iso(s) => V::Iso(s.to_string()),
    }
}

fn from_data(d: &Data) -> Option<V> {
    Some(match d {
        Data::Empty => return None,
        Data::Float(f) => V::Num(*f),
        Data::Int(i) => V::Num(*i as f64),
        Data::String(s) => V::Text(s.clone()),
        Data::Bool(b) => V::Bool(*b),
        Data::DateTime(dt) => V::Date(dt.as_f64(), dt.is_duration()),
        Data::DateTimeIso(s) => V::Iso(s.clone()),
        Data::DurationIso(s) => V::Iso(s.clone()),
        Data::Error(e) => V::Err(e.to_string()),
    })
}

/// What konoma's reader says about the workbook at `path`.
fn read_ours(path: &Path) -> BookRead {
    let limits = Limits::default();
    let mut book =
        xlsx::open(path, &limits).unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
    let sheets: Vec<(String, u8)> = book
        .sheets
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                match s.visible {
                    xlsx::Visibility::Visible => 0,
                    xlsx::Visibility::Hidden => 1,
                    xlsx::Visibility::VeryHidden => 2,
                },
            )
        })
        .collect();
    let mut reads = Vec::new();
    for info in book.sheets.clone() {
        let mut r = SheetRead::default();
        let merges = book.read_sheet(&info.part, &limits, None, |c| {
            if let Some(v) = c.value {
                r.values.insert((c.row, c.col), from_val(v));
            }
            if let Some(f) = c.formula.filter(|f| !f.is_empty()) {
                r.formulas.insert((c.row, c.col), f.to_string());
            }
            true
        });
        reads.push(match merges {
            Ok(m) => {
                r.merges = m.iter().map(|m| (m.row0, m.col0, m.row1, m.col1)).collect();
                Ok(r)
            }
            Err(e) => Err(format!("{e:?}")),
        });
    }
    BookRead {
        date1904: book.date1904,
        sheets,
        reads,
    }
}

/// What `calamine` says about the same workbook.
fn read_oracle(path: &Path) -> BookRead {
    let f = BufReader::new(fs::File::open(path).unwrap());
    let mut wb: calamine::Xlsx<_> =
        calamine::Xlsx::new(f).unwrap_or_else(|e| panic!("calamine {}: {e}", path.display()));
    let metas: Vec<(String, calamine::SheetType, calamine::SheetVisible)> = wb
        .sheets_metadata()
        .iter()
        .map(|s| (s.name.clone(), s.typ, s.visible))
        .collect();
    let mut sheets = Vec::new();
    let mut reads = Vec::new();
    for (name, typ, vis) in metas {
        if typ != calamine::SheetType::WorkSheet {
            continue;
        }
        sheets.push((
            name.clone(),
            match vis {
                calamine::SheetVisible::Visible => 0,
                calamine::SheetVisible::Hidden => 1,
                calamine::SheetVisible::VeryHidden => 2,
            },
        ));
        reads.push(read_oracle_sheet(&mut wb, &name));
    }
    BookRead {
        date1904: wb.has_1904_epoch(),
        sheets,
        reads,
    }
}

fn read_oracle_sheet(
    wb: &mut calamine::Xlsx<BufReader<fs::File>>,
    name: &str,
) -> Result<SheetRead, String> {
    let mut r = SheetRead::default();
    let range = wb.worksheet_range(name).map_err(|e| e.to_string())?;
    if let Some((r0, c0)) = range.start() {
        for (i, row) in range.rows().enumerate() {
            for (j, d) in row.iter().enumerate() {
                if let Some(v) = from_data(d) {
                    r.values.insert((r0 + i as u32, c0 + j as u32), v);
                }
            }
        }
    }
    let formulas = wb.worksheet_formula(name).map_err(|e| e.to_string())?;
    if let Some((r0, c0)) = formulas.start() {
        for (i, row) in formulas.rows().enumerate() {
            for (j, f) in row.iter().enumerate() {
                if !f.is_empty() {
                    r.formulas.insert((r0 + i as u32, c0 + j as u32), f.clone());
                }
            }
        }
    }
    r.merges = wb
        .merge_cells_by_sheet_name(name)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|d| (d.start.0, d.start.1, d.end.0, d.end.1))
        .collect();
    Ok(r)
}

/// The places where the two readers are allowed to differ, as `(file, sheet, reason)`: `calamine`
/// refuses the sheet (`Err`) for a reason that is its own limitation. Anything else is a bug in
/// one of the two readers.
fn known_differences(file: &str, sheet: &str) -> Option<&'static str> {
    match (file, sheet) {
        // `calamine` knows only the seven classic error codes and rejects the whole sheet over
        // `#SPILL!` / `#CALC!` / `#FIELD!` / `#GETTING_DATA` (see `newer_error_codes_*`).
        ("newerrors.xlsx", "スピル" | "カルク" | "フィールド" | "取得中") => {
            Some("calamine rejects Excel 365 error codes")
        }
        _ => None,
    }
}

/// Compares the two reads of one file. Returns how many sheets and cells were compared.
fn compare(path: &Path) -> (usize, usize) {
    let file = path.file_name().unwrap().to_string_lossy().to_string();
    let ours = read_ours(path);
    let theirs = read_oracle(path);
    assert_eq!(ours.date1904, theirs.date1904, "{file}: date system");
    assert_eq!(ours.sheets, theirs.sheets, "{file}: sheet list");
    let (mut sheets, mut cells) = (0, 0);
    for (i, (name, _)) in ours.sheets.iter().enumerate() {
        let (o, t) = (&ours.reads[i], &theirs.reads[i]);
        match (o, t) {
            (Ok(o), Ok(t)) => {
                assert_same(&file, name, o, t);
                sheets += 1;
                cells += o.values.len();
            }
            (Ok(_), Err(why)) => {
                let reason = known_differences(&file, name).unwrap_or_else(|| {
                    panic!("{file}/{name}: calamine fails ({why}), konoma reads it")
                });
                eprintln!("known difference {file}/{name}: {reason} (calamine: {why})");
            }
            (Err(why), _) => panic!("{file}/{name}: konoma fails: {why}"),
        }
    }
    (sheets, cells)
}

fn assert_same(file: &str, sheet: &str, o: &SheetRead, t: &SheetRead) {
    let diff = |what: &str, a: Vec<String>| {
        if !a.is_empty() {
            panic!(
                "{file}/{sheet}: {what} differ ({} places), first: {:#?}",
                a.len(),
                &a[..a.len().min(5)]
            );
        }
    };
    let mut d = Vec::new();
    for k in o.values.keys().chain(t.values.keys()) {
        if o.values.get(k) != t.values.get(k) {
            d.push(format!(
                "{k:?}: konoma {:?} / calamine {:?}",
                o.values.get(k),
                t.values.get(k)
            ));
        }
    }
    d.dedup();
    diff("values", d);
    let mut d = Vec::new();
    for k in o.formulas.keys().chain(t.formulas.keys()) {
        if o.formulas.get(k) != t.formulas.get(k) {
            d.push(format!(
                "{k:?}: konoma {:?} / calamine {:?}",
                o.formulas.get(k),
                t.formulas.get(k)
            ));
        }
    }
    d.dedup();
    diff("formulas", d);
    assert_eq!(o.merges, t.merges, "{file}/{sheet}: merged ranges");
}

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/office")
        .join(name)
}

#[test]
fn every_real_xlsx_reads_like_calamine() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/office");
    let mut files: Vec<PathBuf> = match fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "xlsx"))
            // A password-protected package is not a zip: neither reader opens it as a workbook.
            .filter(|p| {
                !p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("encrypted")
            })
            .collect(),
        Err(_) => {
            eprintln!("SKIP: testdata/office not found (excluded from the published crate)");
            Vec::new()
        }
    };
    if let Some(s) = crate::test_support::sample_path_or_skip("sample.xlsx") {
        files.push(s);
    }
    files.sort();
    let (mut n_files, mut n_sheets, mut n_cells) = (0, 0, 0);
    for f in &files {
        let (s, c) = compare(f);
        n_files += 1;
        n_sheets += s;
        n_cells += c;
    }
    eprintln!("compared {n_files} files, {n_sheets} sheets, {n_cells} cells");
    if !files.is_empty() {
        assert!(n_files >= 1 && n_cells > 0);
    }
}

// ---------------------------------------------------------------------------------------------
// generated workbooks
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        (xorshift(&mut self.0) % n as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        xorshift(&mut self.0) % 100 < pct
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// The text content of a shared / inline string item, in every shape the format has.
const STRING_ITEMS: &[&str] = &[
    "<t>plain</t>",
    "<t>日本語のテキスト</t>",
    r#"<t xml:space="preserve">  padded  </t>"#,
    "<t>  trimmed by default  </t>",
    "<t>a &amp; b &lt; c &gt; d &quot;q&quot; &apos;s&apos;</t>",
    "<r><t>rich </t></r><r><rPr><b/></rPr><t>text</t></r>",
    r#"<r><t>left </t></r><r><t xml:space="preserve"> right</t></r>"#,
    r#"<t>base</t><rPh sb="0" eb="1"><t>フリガナ</t></rPh><phoneticPr fontId="1"/>"#,
    r#"<r><t>漢字</t></r><rPh sb="0" eb="2"><t>カンジ</t></rPh>"#,
    "<t>line1&#10;line2</t>",
    "<t>cr_x000D_lf_x000A_end</t>",
    "<t>_x005F_x000D_ literal</t>",
    "<t>_x0041_ and trailing_x00</t>",
    r#"<t xml:space="preserve"> _x0020_ </t>"#,
    "<t></t>",
    "",
    "<t><![CDATA[cdata <&> text]]></t>",
    "<t>emoji 😀 text</t>",
    "<t>a&#x20;b&#65;c</t>",
    "<r><t>x</t></r><r><t></t></r><r><t>y</t></r>",
];

const NUMBERS: &[&str] = &[
    "0",
    "1",
    "-1",
    "42",
    "3.14159",
    "0.1",
    "1E+3",
    "1.5e-7",
    "123456789012345",
    "1234567890123456789",
    "-0.000001",
    ".5",
    "5.",
    "1e308",
    "100000000000000000000",
    "0.30000000000000004",
    "44927",
    "44927.5",
    "0.5",
    "60",
    "36526.75",
    "2958465",
    "-45000.25",
];

/// Custom formats of every family (date, time, elapsed, text, currency, era, fill, condition).
const CUSTOM_FORMATS: &[&str] = &[
    "yyyy-mm-dd",
    "0.00%",
    "[h]:mm:ss",
    "&quot;Y&quot;0",
    "mm:ss.0",
    "[Red]#,##0;(#,##0)",
    "h:mm AM/PM",
    "[$-411]ggge&quot;年&quot;m&quot;月&quot;d&quot;日&quot;",
    "#,##0.00_);[Red](#,##0.00)",
    "General",
    "@",
    "[mm]:ss",
    "0.0\\ &quot;d&quot;",
    "dd/mm/yy;@",
    "[&gt;=100][Magenta][s].00",
    "#,##0*y",
    "&quot;a&quot;m",
    "A/P h",
    "[Blue]\\+[h]:mm;[Red]\\-[h]:mm;[Green][h]:mm",
    "[h]",
    "ha/p\\\\m",
    "0_ ;[Red]\\-0\\ ",
    "[$-404]e&quot;\\xfc&quot;m&quot;\\xfc&quot;d&quot;\\xfc&quot;",
];

/// `numFmtId` of each style (`None`: the attribute is absent).
fn gen_xfs(rng: &mut Rng) -> (String, usize) {
    let mut xfs: Vec<Option<u32>> = vec![
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(4),
        Some(9),
        Some(10),
        Some(14),
        Some(15),
        Some(16),
        Some(17),
        Some(18),
        Some(19),
        Some(20),
        Some(21),
        Some(22),
        Some(37),
        Some(45),
        Some(46),
        Some(47),
        Some(49),
        Some(23),
        Some(200),
    ];
    let mut custom = String::new();
    for (i, code) in CUSTOM_FORMATS.iter().enumerate() {
        custom += &format!(r#"<numFmt numFmtId="{}" formatCode="{code}"/>"#, 164 + i);
        xfs.push(Some(164 + i as u32));
    }
    // A custom definition of a built-in id, in both directions.
    custom +=
        r#"<numFmt numFmtId="14" formatCode="0.00"/><numFmt numFmtId="2" formatCode="yyyy"/>"#;
    xfs.push(Some(14));
    xfs.push(Some(2));
    // Some shuffling so that the xf order is not the numeric order.
    for i in (1..xfs.len()).rev() {
        let j = rng.below(i + 1);
        xfs.swap(i, j);
    }
    let cell_xfs: String = xfs
        .iter()
        .map(|x| match x {
            Some(id) => format!(r#"<xf numFmtId="{id}" fontId="0" xfId="0"/>"#),
            None => r#"<xf fontId="0" xfId="0"><alignment horizontal="center"/></xf>"#.to_string(),
        })
        .collect();
    (
        format!(
            r#"<?xml version="1.0"?><styleSheet xmlns="{NS}"><numFmts count="{}">{custom}</numFmts><cellStyleXfs count="1"><xf numFmtId="14"/></cellStyleXfs><cellXfs count="{}">{cell_xfs}</cellXfs></styleSheet>"#,
            CUSTOM_FORMATS.len() + 2,
            xfs.len()
        ),
        xfs.len(),
    )
}

const NS: &str = super::tests::NS;

const FORMULAS: &[&str] = &[
    "A1+B1",
    "$A$1*B2",
    "SUM(A1:A10)",
    r#"IF(A1>0,"x","y")"#,
    "Sheet2!A1+1",
    "SUM($A:$A)",
    "A$1+$B2",
    "ROW()",
    "VLOOKUP(A1,$D$1:$E$20,2,FALSE)",
    r#""A1"&amp;B1"#,
    r#"COUNTIF(B:B,">"&amp;A1)"#,
    "INDEX(A1:C5,2,2)",
    "LOG10(A1)+ATAN2(B1,C1)",
    "SUM(1:1)",
    "SUM($1:3)",
    "IF(A1&lt;B1,B1-A1,A1-B1)",
    "A1:B2",
    "TaxRate*A1",
    "Q4Sales!B2",
    "AB10+XFD1048576",
    "1.5E+3*A1",
    "SUM(A1,B2,C3:D4)",
    "-A1^2",
    r#"CONCATENATE("a""b",A1)"#,
    "A1%",
    "(A1+B1)/(C1-D1)",
    "'Sheet 3'!B2+1",
];

fn push_cell_ref(col: usize, row: usize) -> String {
    let mut c = col + 1;
    let mut letters = Vec::new();
    while c > 0 {
        letters.push(b'A' + ((c - 1) % 26) as u8);
        c = (c - 1) / 26;
    }
    letters.reverse();
    format!("{}{}", String::from_utf8(letters).unwrap(), row + 1)
}

/// The XML of a generated sheet and how many shared strings it may refer to.
fn gen_sheet(rng: &mut Rng, n_strings: usize, n_xfs: usize) -> String {
    let rows = 6 + rng.below(30);
    let ncols = 2 + rng.below(8);
    // Shared formula blocks live in their own columns (J, K, L) so they never collide with the
    // random cells: `(row, col) -> xml`.
    let mut blocks: BTreeMap<(usize, usize), String> = BTreeMap::new();
    let mut si = 0;
    for _ in 0..rng.below(4) {
        let r0 = rng.below(rows);
        let k = 1 + rng.below(6);
        let two_d = rng.chance(40);
        let template = (*rng.pick(FORMULAS)).to_string();
        let cols = if two_d { 2 } else { 1 };
        let ref_attr = format!(
            "{}:{}",
            push_cell_ref(9, r0),
            push_cell_ref(9 + cols - 1, r0 + k)
        );
        for dr in 0..=k {
            for dc in 0..cols {
                let pos = (r0 + dr, 9 + dc);
                if blocks.contains_key(&pos) {
                    continue;
                }
                let r = push_cell_ref(pos.1, pos.0);
                let f = if dr == 0 && dc == 0 {
                    format!(r#"<f t="shared" ref="{ref_attr}" si="{si}">{template}</f>"#)
                } else {
                    format!(r#"<f t="shared" si="{si}"/>"#)
                };
                blocks.insert(pos, format!(r#"<c r="{r}">{f}<v>{}</v></c>"#, dr + dc));
            }
        }
        si += 1;
    }
    let mut out = String::new();
    out += &format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><dimension ref="A1:L{rows}"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="3" width="12" customWidth="1"/></cols><sheetData>"#
    );
    for r in 0..rows {
        if rng.chance(12) {
            continue; // a gap in the rows
        }
        let with_r = rng.chance(88);
        if with_r {
            out += &format!(r#"<row r="{}" spans="1:12">"#, r + 1);
        } else if rng.chance(50) {
            out += "<row>";
        } else {
            out += &format!(r#"<row ht="20" customHeight="1" r="{}">"#, r + 1);
        }
        for c in 0..ncols {
            if rng.chance(18) {
                continue; // a gap in the columns
            }
            let r_attr = if rng.chance(85) {
                format!(r#" r="{}""#, push_cell_ref(c, r))
            } else {
                String::new()
            };
            let s_attr = match rng.below(6) {
                0 => String::new(),
                1 => format!(r#" s="{}""#, n_xfs + 50), // past the style table
                _ => format!(r#" s="{}""#, rng.below(n_xfs)),
            };
            out += &gen_cell(rng, &r_attr, &s_attr, n_strings);
        }
        for (pos, xml) in blocks.range((r, 0)..(r + 1, 0)) {
            let _ = pos;
            out += xml;
        }
        out += "</row>";
    }
    out += "</sheetData>";
    // Merged ranges after the data, as every producer writes them.
    let n_merges = rng.below(4);
    if n_merges > 0 {
        out += &format!(r#"<mergeCells count="{n_merges}">"#);
        for _ in 0..n_merges {
            let (r0, c0) = (rng.below(rows), rng.below(ncols));
            let (r1, c1) = (r0 + rng.below(3), c0 + rng.below(3));
            out += &format!(
                r#"<mergeCell ref="{}:{}"/>"#,
                push_cell_ref(c0, r0),
                push_cell_ref(c1, r1)
            );
        }
        out += "</mergeCells>";
    }
    out += r#"<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><extLst><ext uri="x"><c r="A99" s="1"><v>1</v></c></ext></extLst></worksheet>"#;
    out
}

fn gen_cell(rng: &mut Rng, r: &str, s: &str, n_strings: usize) -> String {
    let num = |rng: &mut Rng| (*rng.pick(NUMBERS)).to_string();
    let formula = |rng: &mut Rng| (*rng.pick(FORMULAS)).to_string();
    let text = |rng: &mut Rng| {
        rng.pick(&[
            "text",
            "a &amp; b",
            "日本",
            "  spaced  ",
            "x&lt;y",
            "",
            "line&#10;two",
        ])
        .to_string()
    };
    match rng.below(24) {
        0..=4 => format!("<c{r}{s}><v>{}</v></c>", num(rng)),
        5 => format!(r#"<c{r}{s} t="n"><v>{}</v></c>"#, num(rng)),
        6..=8 if n_strings > 0 => {
            format!(r#"<c{r}{s} t="s"><v>{}</v></c>"#, rng.below(n_strings))
        }
        9 => format!(
            r#"<c{r}{s} t="str"><f>{}</f><v>{}</v></c>"#,
            formula(rng),
            text(rng)
        ),
        10 | 11 => {
            let item = *rng.pick(STRING_ITEMS);
            format!(r#"<c{r}{s} t="inlineStr"><is>{item}</is></c>"#)
        }
        12 => format!(
            r#"<c{r}{s} t="b"><v>{}</v></c>"#,
            rng.pick(&["0", "1", "2", "true"])
        ),
        13 => format!(
            r#"<c{r}{s} t="e"><v>{}</v></c>"#,
            rng.pick(&["#DIV/0!", "#N/A", "#NAME?", "#NULL!", "#NUM!", "#REF!", "#VALUE!"])
        ),
        14 => format!(
            r#"<c{r}{s} t="d"><v>{}</v></c>"#,
            rng.pick(&["2024-05-01T10:00:00", "2024-05-01", "1999-12-31T23:59:59.5"])
        ),
        15 => format!("<c{r}{s}/>"),
        16 => format!("<c{r}{s}></c>"),
        17 => format!("<c{r}{s}><f>{}</f></c>", formula(rng)),
        18 | 19 => format!("<c{r}{s}><f>{}</f><v>{}</v></c>", formula(rng), num(rng)),
        20 => rng
            .pick(&[
                "<c{r}{s}><v></v></c>",
                "<c{r}{s}><v/></c>",
                r#"<c{r}{s} t="s"><v/></c>"#,
                r#"<c{r}{s} t="str"><v/></c>"#,
                r#"<c{r}{s} t="inlineStr"><is/></c>"#,
                r#"<c{r}{s} t="inlineStr"><v>9</v></c>"#,
                r#"<c{r}{s}><f/></c>"#,
            ])
            .replace("{r}", r)
            .replace("{s}", s),
        21 => format!(
            r#"<c{r}{s}><f t="array" ref="A1:A3">SUM(B1:B3*C1:C3)</f><v>{}</v></c>"#,
            num(rng)
        ),
        22 => format!(
            r#"<c{r}{s} t="inlineStr"><is><t>first</t></is><v>{}</v></c>"#,
            num(rng)
        ),
        _ => format!(r#"<c{r}{s} t="str"><v>{}</v></c>"#, text(rng)),
    }
}

/// A complete package: a few worksheets (visible, hidden, very hidden), a chart sheet, styles and
/// shared strings.
fn gen_package(seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    let (styles, n_xfs) = gen_xfs(&mut rng);
    let n_strings = 5 + rng.below(40);
    let mut sst =
        format!(r#"<?xml version="1.0"?><sst xmlns="{NS}" count="99" uniqueCount="{n_strings}">"#);
    for _ in 0..n_strings {
        sst += &format!("<si>{}</si>", rng.pick(STRING_ITEMS));
    }
    sst += "</sst>";
    let d1904 = rng.chance(30);
    let sheets = [
        ("Sheet1", "visible"),
        ("Hidden", "hidden"),
        ("Chart1", "chart"),
        ("Deep", "veryHidden"),
        ("Last one", "visible"),
    ];
    let mut wb = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><workbookPr date1904="{}"/><sheets>"#,
        if d1904 { "1" } else { "0" }
    );
    let mut rels = String::from(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for (i, (name, state)) in sheets.iter().enumerate() {
        let n = i + 1;
        let st = if *state == "chart" { "visible" } else { state };
        wb += &format!(r#"<sheet name="{name}" sheetId="{n}" state="{st}" r:id="rId{n}"/>"#);
        if *state == "chart" {
            rels += &format!(
                r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet" Target="chartsheets/sheet{n}.xml"/>"#
            );
            entries.push((
                format!("xl/chartsheets/sheet{n}.xml"),
                format!(
                    r#"<?xml version="1.0"?><chartsheet xmlns="{NS}"><sheetViews/></chartsheet>"#
                )
                .into_bytes(),
            ));
        } else {
            rels += &format!(
                r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#
            );
            entries.push((
                format!("xl/worksheets/sheet{n}.xml"),
                gen_sheet(&mut rng, n_strings, n_xfs).into_bytes(),
            ));
        }
    }
    wb += "</sheets></workbook>";
    rels += r#"<Relationship Id="rId90" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId91" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#;
    let root = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let mut all: Vec<(String, Vec<u8>)> = vec![
        ("_rels/.rels".into(), root.as_bytes().to_vec()),
        ("xl/workbook.xml".into(), wb.into_bytes()),
        ("xl/_rels/workbook.xml.rels".into(), rels.into_bytes()),
        ("xl/styles.xml".into(), styles.into_bytes()),
        ("xl/sharedStrings.xml".into(), sst.into_bytes()),
    ];
    all.extend(entries);
    let refs: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    deflated(&refs)
}

#[test]
fn generated_workbooks_read_like_calamine() {
    let dir = tmp("xlsx_diff");
    let (mut sheets, mut cells, mut formulas) = (0, 0, 0);
    for seed in 1..=60u64 {
        let p = write(
            &dir,
            &format!("g{seed}.xlsx"),
            &gen_package(seed * 0x9E37_79B9),
        );
        let (s, c) = compare(&p);
        sheets += s;
        cells += c;
        formulas += read_ours(&p)
            .reads
            .iter()
            .map(|r| r.as_ref().map_or(0, |r| r.formulas.len()))
            .sum::<usize>();
    }
    eprintln!(
        "compared 60 generated workbooks: {sheets} sheets, {cells} cells, {formulas} formulas"
    );
    // The generator must actually exercise the reader (a generator that makes empty sheets would
    // make this test vacuous).
    assert!(sheets >= 150, "{sheets}");
    assert!(cells >= 2000, "{cells}");
    assert!(formulas >= 500, "{formulas}");
}

/// The generator produces what it claims to: every cell type, shared formulas whose derived cells
/// are expanded, shared strings with phonetic runs, dates and durations among the numbers.
#[test]
fn the_generated_workbooks_cover_every_kind_of_value() {
    let dir = tmp("xlsx_cover");
    let (mut num, mut date, mut dur, mut text, mut boolean, mut err, mut iso) =
        (0, 0, 0, 0, 0, 0, 0);
    let mut derived = 0;
    for seed in 1..=60u64 {
        let p = write(
            &dir,
            &format!("g{seed}.xlsx"),
            &gen_package(seed * 0x9E37_79B9),
        );
        for r in read_ours(&p).reads.into_iter().flatten() {
            for v in r.values.values() {
                match v {
                    V::Num(_) => num += 1,
                    V::Date(_, false) => date += 1,
                    V::Date(_, true) => dur += 1,
                    V::Text(_) => text += 1,
                    V::Bool(_) => boolean += 1,
                    V::Err(_) => err += 1,
                    V::Iso(_) => iso += 1,
                }
            }
            // Cells J.. hold the shared formulas; a derived cell has the template's text moved.
            derived += r.formulas.iter().filter(|((_, c), _)| *c >= 9).count();
        }
    }
    for (what, n) in [
        ("number", num),
        ("date", date),
        ("duration", dur),
        ("text", text),
        ("bool", boolean),
        ("error", err),
        ("iso date", iso),
        ("shared formula cell", derived),
    ] {
        assert!(n >= 20, "{what}: only {n}");
    }
}

#[test]
fn a_generated_workbook_reads_the_same_through_the_loader() {
    // The loader (`load_workbook`) is the production path: the same cells, shown.
    let dir = tmp("xlsx_loader");
    for seed in [3u64, 17, 29] {
        let p = write(
            &dir,
            &format!("g{seed}.xlsx"),
            &gen_package(seed * 0x9E37_79B9),
        );
        let wb = load_workbook_unguarded_for_tests(&p);
        let ours = read_ours(&p);
        assert_eq!(
            wb.sheets.len(),
            ours.sheets.iter().filter(|(_, v)| *v == 0).count()
        );
        for s in &wb.sheets {
            let idx = ours.sheets.iter().position(|(n, _)| *n == s.name).unwrap();
            let read = ours.reads[idx].as_ref().unwrap();
            for ((r, c), v) in &read.values {
                if (*c as usize) >= 16_384 || (*r as usize) >= s.nrows {
                    continue;
                }
                let cell = s
                    .cell(*r as usize, *c as usize)
                    .unwrap_or_else(|| panic!("{}: ({r},{c}) {v:?} missing", s.name));
                // The raw value agrees with the reader's.
                match (&cell.value, v) {
                    (CellValue::Number(a), V::Num(b)) => assert_eq!(a, b),
                    (CellValue::DateTime { serial, duration }, V::Date(b, d)) => {
                        assert_eq!((serial, duration), (b, d))
                    }
                    (CellValue::Text(a), V::Text(b)) => assert_eq!(&**a, b),
                    (CellValue::Bool(a), V::Bool(b)) => assert_eq!(a, b),
                    (CellValue::Error(a), V::Err(b)) => assert_eq!(a, b),
                    (CellValue::ErrorText(a), V::Err(b)) => assert_eq!(&**a, b),
                    (CellValue::DateTimeIso(a), V::Iso(b)) => assert_eq!(&**a, b),
                    (a, b) => panic!("{}: ({r},{c}) {a:?} vs {b:?}", s.name),
                }
            }
        }
    }
}

fn load_workbook_unguarded_for_tests(p: &Path) -> Workbook {
    super::workbook::load_workbook_unguarded(p, &LoadOptions::default()).unwrap()
}

// ---------------------------------------------------------------------------------------------
// where konoma and calamine differ, on purpose
// ---------------------------------------------------------------------------------------------

/// A one-sheet package with a custom sheet XML and shared strings.
fn one_sheet(dir: &crate::test_support::TmpDir, name: &str, sheet: &str) -> PathBuf {
    Pkg::new()
        .styles(super::tests::styles_xml(&[], &[0, 14]))
        .sheet("S", "visible", sheet.to_string())
        .write(dir, name)
}

fn body(rows: &str) -> String {
    super::tests::sheet_xml(rows, "")
}

#[test]
fn newer_error_codes_are_read_where_calamine_gives_up() {
    let Some(p) = Some(testdata("newerrors.xlsx")).filter(|p| p.exists()) else {
        eprintln!("SKIP: testdata/office/newerrors.xlsx not found");
        return;
    };
    let ours = read_ours(&p);
    let theirs = read_oracle(&p);
    for (i, code) in [
        (0, "#SPILL!"),
        (1, "#CALC!"),
        (2, "#FIELD!"),
        (3, "#GETTING_DATA"),
    ] {
        // calamine: the whole sheet is an error ...
        assert!(theirs.reads[i].is_err(), "sheet {i}: {:?}", theirs.reads[i]);
        // ... konoma: the error is a cell like any other and its neighbours are read.
        let r = ours.reads[i].as_ref().unwrap();
        assert_eq!(r.values[&(0, 0)], V::Text("before".into()));
        assert_eq!(r.values[&(0, 1)], V::Err(code.into()));
        assert_eq!(r.values[&(0, 2)], V::Text("after".into()));
    }
    // The sheet that has none reads the same in both.
    assert_eq!(ours.reads[4], theirs.reads[4]);
}

#[test]
fn an_error_value_is_whatever_the_file_writes() {
    let dir = tmp("xlsx_err");
    let rows = r#"<row r="1"><c r="A1" t="e"><v>#SPILL!</v></c><c r="B1" t="e"><v>#N/A</v></c><c r="C1" t="e"><v>#GETTING_DATA</v></c><c r="D1" t="e"><v>#PYTHON!</v></c><c r="E1" t="e"><v>nope</v></c></row>"#;
    let p = one_sheet(&dir, "e.xlsx", &body(rows));
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 0).unwrap();
    let s = &wb.sheets[0];
    assert!(wb.sheet_error.is_none());
    for (c, code) in ["#SPILL!", "#N/A", "#GETTING_DATA", "#PYTHON!", "nope"]
        .iter()
        .enumerate()
    {
        assert_eq!(s.display(0, c), *code);
        assert_eq!(s.cell(0, c).unwrap().cell_type(), CellType::Error);
        assert_eq!(s.cell(0, c).unwrap().raw_text(), *code);
    }
    // The classic seven keep their fixed codes.
    assert!(matches!(
        s.cell(0, 1).unwrap().value,
        CellValue::Error("#N/A")
    ));
    assert!(matches!(
        s.cell(0, 0).unwrap().value,
        CellValue::ErrorText(_)
    ));
}

/// What `calamine` makes a whole sheet fail over, or reads wrongly, and konoma reads. Both sides
/// are pinned so that a change on either side shows up.
#[test]
fn what_calamine_refuses_a_sheet_over_is_a_cell_here() {
    let dir = tmp("xlsx_lenient");
    // 1. An unknown child element of `<c>`.
    // 2. A number cell whose text is not a number (explicit `t="n"`).
    // 3. A shared string index past the table.
    // 4. An unknown cell type.
    let cases: [(&str, &str, Option<V>); 4] = [
        (
            "unknown child",
            r#"<c r="A1"><v>1</v><x14ac:foo xmlns:x14ac="u"><b/></x14ac:foo></c>"#,
            Some(V::Num(1.0)),
        ),
        (
            "bad number",
            r#"<c r="A1" t="n"><v>abc</v></c>"#,
            Some(V::Text("abc".into())),
        ),
        (
            "string index past the table",
            r#"<c r="A1" t="s"><v>9</v></c>"#,
            None,
        ),
        (
            "unknown type",
            r#"<c r="A1" t="zz"><v>x</v></c>"#,
            Some(V::Text("x".into())),
        ),
    ];
    for (i, (what, cell, want)) in cases.into_iter().enumerate() {
        let rows = format!(r#"<row r="1">{cell}<c r="B1"><v>7</v></c></row>"#);
        let p = Pkg::new()
            .sheet("S", "visible", body(&rows))
            .shared(format!(
                r#"<sst xmlns="{NS}" uniqueCount="1"><si><t>only</t></si></sst>"#
            ))
            .write(&dir, &format!("l{i}.xlsx"));
        let ours = read_ours(&p);
        let theirs = read_oracle(&p);
        assert!(
            theirs.reads[0].is_err(),
            "{what}: calamine now reads it ({:?}): drop this difference",
            theirs.reads[0]
        );
        let r = ours.reads[0]
            .as_ref()
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(r.values.get(&(0, 0)), want.as_ref(), "{what}");
        assert_eq!(
            r.values[&(0, 1)],
            V::Num(7.0),
            "{what}: the next cell is read"
        );
    }
}

/// `calamine` expands a shared formula from the top-left of its `ref`; Excel (and this reader)
/// from the cell that holds the formula text. They are the same cell in every file Excel writes.
#[test]
fn a_shared_formula_is_expanded_from_its_anchor_cell() {
    let dir = tmp("xlsx_anchor");
    // The anchor is B2 although the ref starts at A1.
    let rows = r#"<row r="2"><c r="B2"><f t="shared" ref="A1:B3" si="0">A1+1</f><v>1</v></c><c r="C2"><f t="shared" si="0"/><v>1</v></c></row>"#;
    let p = one_sheet(&dir, "a.xlsx", &body(rows));
    let ours = read_ours(&p).reads.remove(0).unwrap();
    assert_eq!(
        ours.formulas[&(1, 2)],
        "B1+1",
        "one column right of the anchor"
    );
    let theirs = read_oracle(&p).reads.remove(0).unwrap();
    assert_eq!(
        theirs.formulas[&(1, 2)],
        "C2+1",
        "calamine: two columns, one row from A1"
    );
}

#[test]
fn text_escapes_beyond_the_ascii_range_are_decoded_here() {
    // `_x4E2D_` is U+4E2D: Excel writes `_xHHHH_` for any character it escapes; calamine decodes
    // only the `_x00HH_` ones.
    let dir = tmp("xlsx_x4e2d");
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            body(
                r#"<row r="1"><c r="A1" t="inlineStr"><is><t>a_x4E2D_b_x0041_c</t></is></c></row>"#,
            ),
        )
        .write(&dir, "x.xlsx");
    let ours = read_ours(&p).reads.remove(0).unwrap();
    assert_eq!(ours.values[&(0, 0)], V::Text("a中bAc".into()));
    let theirs = read_oracle(&p).reads.remove(0).unwrap();
    assert_eq!(theirs.values[&(0, 0)], V::Text("a_x4E2D_bAc".into()));
}

#[test]
fn a_value_after_a_formula_is_kept_where_calamines_value_reader_drops_it() {
    // `<v>` before `<f>` is not the order the schema has, but a producer may write it: konoma keeps
    // the value (what the format's reader that also reads formulas does); `calamine`'s plain value
    // reader takes the `<f>` for the cell's value and loses it.
    let dir = tmp("xlsx_vf");
    let p = one_sheet(
        &dir,
        "vf.xlsx",
        &body(r#"<row r="1"><c r="A1"><v>5</v><f>1+4</f></c></row>"#),
    );
    let ours = read_ours(&p).reads.remove(0).unwrap();
    assert_eq!(ours.values[&(0, 0)], V::Num(5.0));
    assert_eq!(ours.formulas[&(0, 0)], "1+4");
    let theirs = read_oracle(&p).reads.remove(0).unwrap();
    assert!(!theirs.values.contains_key(&(0, 0)));
}

// ---------------------------------------------------------------------------------------------
// number formats: date or not
// ---------------------------------------------------------------------------------------------

fn kind_of(code: &str) -> DateKind {
    date_kind(&NumFmtRef::Custom(code.into()))
}

#[test]
fn date_kind_of_format_codes() {
    use DateKind::*;
    for (code, want) in [
        ("DD/MM/YY", Date),
        ("H:MM:SS;@", Date),
        ("#,##0\\ [$\\u20bd-46D]", Plain),
        ("m\"M\"d\"D\";@", Date),
        ("[h]:mm:ss", Duration),
        ("\"Y: \"0.00\"m\";\"Y: \"-0.00\"m\";\"Y: <num>m\";@", Plain),
        ("\"$\"#,##0_);[Red](\"$\"#,##0)", Plain),
        ("[$-404]e\"\\xfc\"m\"\\xfc\"d\"\\xfc\"", Date),
        ("0_ ;[Red]\\-0\\ ", Plain),
        ("\\Y000000", Plain),
        ("#,##0.0####\" YMD\"", Plain),
        ("[h]", Duration),
        ("[ss]", Duration),
        ("[s].000", Duration),
        ("[m]", Duration),
        ("[mm]", Duration),
        ("[Blue]\\+[h]:mm;[Red]\\-[h]:mm;[Green][h]:mm", Duration),
        ("[>=100][Magenta][s].00", Duration),
        ("[h]:mm;[=0]\\-", Duration),
        ("[>=100][Magenta].00", Plain),
        ("[>=100][Magenta]General", Plain),
        ("ha/p\\\\m", Date),
        ("#,##0.00\\ _M\"H\"_);[Red]#,##0.00\\ _M\"S\"_)", Plain),
        ("#,##0*y", Plain),
        ("0\"x\"*d", Plain),
        ("*-#,##0", Plain),
        ("*-yyyy-mm-dd", Date),
        ("General", Plain),
        ("0.00%", Plain),
        ("h:mm AM/PM", Date),
        ("yyyy\"年\"m\"月\"", Date),
        ("", Plain),
    ] {
        assert_eq!(kind_of(code), want, "{code:?}");
    }
    // Built-in ids: the fixed date / time numbers.
    for id in 0..=163u16 {
        let want = match id {
            14..=22 | 45 | 47 => Date,
            46 => Duration,
            _ => Plain,
        };
        assert_eq!(date_kind(&NumFmtRef::Builtin(id)), want, "built-in {id}");
    }
    assert_eq!(date_kind(&NumFmtRef::General), Plain);
}

/// The kind of every format code that appears in the generated workbooks and in any short string
/// of the characters that matter, against what `calamine` makes of a cell with that format
/// (`DateTime` / `TimeDelta` / `Float`).
#[test]
fn date_kind_agrees_with_calamine_for_many_format_codes() {
    let dir = tmp("xlsx_kinds");
    let alphabet = [
        'd', 'm', 'y', 'h', 's', 'a', 'p', 'A', 'P', 'M', '/', '[', ']', '"', '\\', '_', '*', ';',
        '0', '#', '.', ' ', 'e', 'g', 'G', 'x',
    ];
    let mut rng = Rng(0xC0FF_EE11);
    let mut codes: Vec<String> = CUSTOM_FORMATS
        .iter()
        .map(|c| c.replace("&quot;", "\"").replace("&gt;", ">"))
        .collect();
    for _ in 0..1500 {
        let n = 1 + rng.below(9);
        let code: String = (0..n).map(|_| *rng.pick(&alphabet)).collect();
        codes.push(code);
    }
    // 300 formats per package keep each file small.
    for (k, chunk) in codes.chunks(300).enumerate() {
        let mut numfmts = String::new();
        let mut xfs = String::new();
        let mut rows = String::from(r#"<row r="1">"#);
        for (i, code) in chunk.iter().enumerate() {
            let esc = code
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;");
            numfmts += &format!(r#"<numFmt numFmtId="{}" formatCode="{esc}"/>"#, 164 + i);
            xfs += &format!(r#"<xf numFmtId="{}"/>"#, 164 + i);
            rows += &format!(r#"<c r="{}" s="{i}"><v>1.5</v></c>"#, push_cell_ref(i, 0));
        }
        rows += "</row>";
        let styles = format!(
            r#"<styleSheet xmlns="{NS}"><numFmts count="{}">{numfmts}</numFmts><cellXfs count="{}">{xfs}</cellXfs></styleSheet>"#,
            chunk.len(),
            chunk.len()
        );
        let p = Pkg::new()
            .styles(styles)
            .sheet("S", "visible", body(&rows))
            .write(&dir, &format!("k{k}.xlsx"));
        let o = read_ours(&p).reads.remove(0).unwrap();
        let t = read_oracle(&p).reads.remove(0).unwrap();
        for (i, code) in chunk.iter().enumerate() {
            // A code that is too long is dropped by konoma's style table (shown as General).
            assert_eq!(
                o.values[&(0, i as u32)],
                t.values[&(0, i as u32)],
                "format {code:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// shared formulas
// ---------------------------------------------------------------------------------------------

fn shifted(f: &str, dr: i64, dc: i64) -> String {
    let mut out = String::new();
    shift_formula(f, dr, dc, &mut out);
    out
}

#[test]
fn shared_formula_expansion_table() {
    for (f, dr, dc, want) in [
        // relative references move, absolute ones stay
        ("A1+B1", 1, 0, "A2+B2"),
        ("A1+B1", 0, 1, "B1+C1"),
        ("A1+B1", 2, 3, "D3+E3"),
        ("$A$1+B1", 5, 5, "$A$1+G6"),
        ("$A1+B$1", 3, 3, "$A4+E$1"),
        ("A$1+$B2", 3, 3, "D$1+$B5"),
        // ranges move both ends; whole columns and rows move along their own axis only
        ("SUM(A1:A10)", 1, 1, "SUM(B2:B11)"),
        ("SUM($A$1:A10)", 2, 0, "SUM($A$1:A12)"),
        ("SUM(A:A)", 3, 1, "SUM(B:B)"),
        ("SUM($A:$A)", 3, 1, "SUM($A:$A)"),
        ("SUM(1:1)", 3, 1, "SUM(4:4)"),
        ("SUM($1:3)", 2, 0, "SUM($1:5)"),
        // names, functions, literals and numbers stay
        ("LOG10(A1)", 1, 0, "LOG10(A2)"),
        ("ATAN2(B1,C1)", 1, 0, "ATAN2(B2,C2)"),
        ("TaxRate*A1", 1, 0, "TaxRate*A2"),
        (r#"IF(A1>0,"A1","B1")"#, 1, 0, r#"IF(A2>0,"A1","B1")"#),
        (r#""say ""A1"" now"&A1"#, 1, 0, r#""say ""A1"" now"&A2"#),
        ("1.5E+3*A1", 1, 0, "1.5E+3*A2"),
        ("TRUE+A1", 1, 0, "TRUE+A2"),
        ("_xlfn.IFERROR(A1,0)", 1, 0, "_xlfn.IFERROR(A2,0)"),
        // a sheet in front: the reference moves, the sheet's name does not
        ("Sheet2!A1+1", 1, 0, "Sheet2!A2+1"),
        ("Sheet2!$A$1:B2", 1, 1, "Sheet2!$A$1:C3"),
        // a reference that would leave the grid is kept as written
        ("A1", -1, 0, "A1"),
        ("B2", 0, -2, "B2"),
        ("XFD1", 0, 1, "XFD1"),
        ("A1048576", 1, 0, "A1048576"),
        ("A1:B2", -1, 0, "A1:B2"),
        // no offset: unchanged
        ("A1+B1", 0, 0, "A1+B1"),
        // multi-byte text around the references is intact
        ("日本!A1&\"語\"", 1, 0, "日本!A2&\"語\""),
    ] {
        assert_eq!(shifted(f, dr, dc), want, "{f} by ({dr},{dc})");
    }
}

/// Where `calamine`'s tokenizer treats something that is not a cell reference as one (and moves
/// it), or stops at a function call and loses the cell before it. Both sides are pinned, so that
/// a change in either shows up (if `calamine` is fixed, the difference is simply dropped).
#[test]
fn shared_formula_expansion_where_calamine_is_wrong() {
    for (f, dr, dc, ours, theirs, why) in [
        (
            "'Q1 2024'!B2",
            0,
            1,
            "'Q1 2024'!C2",
            "'R1 2024'!C2",
            "the words of a quoted sheet name are tokens to calamine, and `Q1` is a cell reference",
        ),
        (
            "Table1[[#This Row],[Qty1]]+A1",
            0,
            1,
            "Table1[[#This Row],[Qty1]]+B1",
            "Table1[[#This Row],[QTZ1]]+B1",
            "a structured reference's column name that looks like a cell is moved by calamine",
        ),
        (
            "SUM(A1:INDEX(B:B,2))",
            1,
            0,
            "SUM(A2:INDEX(B:B,2))",
            "SUM(A1:INDEX(B:B,2))",
            "a call after a colon makes calamine leave the cell before the colon alone",
        ),
    ] {
        assert_eq!(shifted(f, dr, dc), ours, "{f}");
        let theirs_now =
            calamine::expand_shared_formula(f, (0, 0), (dr as u32, dc as u32)).unwrap();
        assert_eq!(
            theirs_now, theirs,
            "calamine on {f} ({why}): if it changed, drop the difference"
        );
    }
    // Error literals are not references for either.
    assert_eq!(shifted("IFERROR(A1,#REF!)", 0, 1), "IFERROR(B1,#REF!)");
}

/// Random formulas built from what formulas are made of; konoma and `calamine` must agree on all
/// of them (the grammar avoids the constructs in the test above).
#[test]
fn shared_formula_expansion_agrees_with_calamine_on_random_formulas() {
    let mut rng = Rng(0xFEED_FACE);
    let refs = [
        "A1",
        "B2",
        "$A$1",
        "$B2",
        "C$3",
        "AA10",
        "XFD1",
        "A1048576",
        "Z99",
        "$XFC$1048575",
        "b3",
    ];
    let ranges = [
        "A1:B2",
        "$A$1:C3",
        "A:A",
        "$B:$C",
        "1:1",
        "$2:4",
        "A1:XFD1048576",
        "AA1:AB5",
        "C$3:$D4",
    ];
    let funcs = [
        "SUM",
        "IF",
        "VLOOKUP",
        "LOG10",
        "ATAN2",
        "_xlfn.SINGLE",
        "COUNTIFS",
        "ROW",
    ];
    let ops = ["+", "-", "*", "/", "&", "=", "<>", ">=", ","];
    let literals = [
        "1",
        "2.5",
        "1E+3",
        "TRUE",
        "\"text\"",
        "\"A1 B2\"",
        "\"\"\"A1\"\"\"",
        "Sheet2!A1",
        "Data2!$B$2:C3",
        "TaxRate",
        "Total",
    ];
    let mut compared = 0;
    for _ in 0..4000 {
        let mut f = String::new();
        let n = 1 + rng.below(6);
        for k in 0..n {
            if k > 0 {
                f += rng.pick(&ops);
            }
            match rng.below(5) {
                0 => f += rng.pick(&refs),
                1 => f += rng.pick(&ranges),
                2 => {
                    f += &format!(
                        "{}({},{})",
                        rng.pick(&funcs),
                        rng.pick(&refs),
                        rng.pick(&ranges)
                    )
                }
                3 => f += rng.pick(&literals),
                _ => f += &format!("({}{}{})", rng.pick(&refs), rng.pick(&ops), rng.pick(&refs)),
            }
        }
        let dr = rng.below(2_000_000) as i64 - 1_000_000;
        let dc = rng.below(40) as i64 - 20;
        let (dr, dc) = match rng.below(3) {
            0 => (dr, dc),
            1 => (rng.below(9) as i64 - 3, rng.below(9) as i64 - 3),
            _ => (rng.below(5) as i64, rng.below(5) as i64),
        };
        let theirs = calamine::expand_shared_formula(
            &f,
            (1000, 1000),
            ((1000 + dr) as u32, (1000 + dc) as u32),
        );
        // A negative end is `u32` wrapping for calamine: only compare when it is representable.
        if 1000 + dr < 0 || 1000 + dc < 0 {
            continue;
        }
        let theirs = theirs.unwrap();
        // Upper-case column letters: calamine writes them so, konoma keeps absolute references as
        // written and writes moved ones the same way, so lower-case input is excluded below.
        if f.contains("b3") {
            continue;
        }
        assert_eq!(shifted(&f, dr, dc), theirs, "{f} by ({dr},{dc})");
        compared += 1;
    }
    assert!(compared > 1500, "{compared}");
}

#[test]
fn derived_cells_get_their_anchors_formula_in_every_direction() {
    let dir = tmp("xlsx_shared_dirs");
    let rows = r#"
        <row r="2"><c r="B2"><f t="shared" ref="B2:D4" si="0">SUM($A2:A2)+B$1</f><v>1</v></c><c r="C2"><f t="shared" si="0"/><v>2</v></c><c r="D2"><f t="shared" si="0"/><v>3</v></c></row>
        <row r="3"><c r="B3"><f t="shared" si="0"/><v>4</v></c><c r="C3"><f t="shared" si="0"/><v>5</v></c></row>
        <row r="4"><c r="D4"><f t="shared" si="0"/><v>6</v></c><c r="F4"><f t="shared" si="9"/><v>7</v></c></row>"#;
    let p = one_sheet(&dir, "d.xlsx", &body(rows));
    let ours = read_ours(&p).reads.remove(0).unwrap();
    assert_eq!(ours.formulas[&(1, 1)], "SUM($A2:A2)+B$1");
    assert_eq!(ours.formulas[&(1, 2)], "SUM($A2:B2)+C$1");
    assert_eq!(ours.formulas[&(1, 3)], "SUM($A2:C2)+D$1");
    assert_eq!(ours.formulas[&(2, 1)], "SUM($A3:A3)+B$1");
    assert_eq!(ours.formulas[&(2, 2)], "SUM($A3:B3)+C$1");
    assert_eq!(ours.formulas[&(3, 3)], "SUM($A4:C4)+D$1");
    assert!(
        !ours.formulas.contains_key(&(3, 5)),
        "a group that has no anchor has no formula"
    );
    let theirs = read_oracle(&p).reads.remove(0).unwrap();
    assert_eq!(ours.formulas, theirs.formulas);
    assert_eq!(ours.values, theirs.values);
}

#[test]
fn the_shared_formula_table_is_bounded() {
    // Every cell opens a group of its own with a long formula: the table stops growing at the
    // sheet's text budget instead of holding all of them.
    let limits = Limits {
        max_sheet_text_bytes: 4096,
        max_sheet_cells: 50,
        ..Limits::default()
    };
    let long = "A1+".repeat(100);
    let mut rows = String::new();
    for i in 0..200u32 {
        rows += &format!(
            r#"<row r="{r}"><c r="A{r}"><f t="shared" ref="A{r}:A{r2}" si="{i}">{long}A1</f></c><c r="B{r}"><f t="shared" si="{i}"/></c></row>"#,
            r = i + 1,
            r2 = i + 2
        );
    }
    let xml = body(&rows);
    let mut with_formula = 0;
    let mut total = 0;
    xlsx::parse_sheet(xml.as_bytes(), &Tables::default(), &limits, None, |c| {
        if c.col == 1 {
            total += 1;
            if c.formula.is_some() {
                with_formula += 1;
            }
        }
        true
    })
    .unwrap();
    assert_eq!(total, 200);
    assert!(with_formula > 0 && with_formula < 50, "{with_formula}");
}

// ---------------------------------------------------------------------------------------------
// values, positions and text, one rule at a time
// ---------------------------------------------------------------------------------------------

type Row = (u32, u32, u16, Option<V>, Option<String>);

/// The cells of a `<sheetData>` body as the reader reports them.
fn cells(rows: &str, tables: &Tables) -> Vec<Row> {
    let xml = body(rows);
    let mut out = Vec::new();
    xlsx::parse_sheet(xml.as_bytes(), tables, &Limits::default(), None, |c| {
        out.push((
            c.row,
            c.col,
            c.fmt,
            c.value.map(from_val),
            c.formula.map(str::to_string),
        ));
        true
    })
    .unwrap();
    out
}

fn tables_with(strings: &[&str], xfs: &[(u16, DateKind)]) -> Tables {
    Tables {
        strings: SharedStrings::from_strs(strings),
        xf_to_format: xfs.iter().map(|x| x.0).collect(),
        xf_kind: xfs.iter().map(|x| x.1).collect(),
    }
}

#[test]
fn value_types() {
    let t = tables_with(
        &["zero", "one"],
        &[
            (0, DateKind::Plain),
            (1, DateKind::Date),
            (2, DateKind::Duration),
        ],
    );
    let got = cells(
        r#"<row r="1">
        <c r="A1"><v>1.5</v></c>
        <c r="B1" s="1"><v>45000.5</v></c>
        <c r="C1" s="2"><v>1.25</v></c>
        <c r="D1" t="s"><v>1</v></c>
        <c r="E1" t="str"><v>a &amp; b</v></c>
        <c r="F1" t="b"><v>1</v></c>
        <c r="G1" t="b"><v>0</v></c>
        <c r="H1" t="e"><v>#DIV/0!</v></c>
        <c r="I1" t="d"><v>2024-05-01</v></c>
        <c r="J1" t="inlineStr"><is><t>in</t></is></c>
        <c r="K1" s="9"><v>2</v></c>
        <c r="L1"><v>abc</v></c>
        </row>"#,
        &t,
    );
    let v = |i: usize| got[i].3.clone();
    assert_eq!(v(0), Some(V::Num(1.5)));
    assert_eq!(v(1), Some(V::Date(45000.5, false)));
    assert_eq!(v(2), Some(V::Date(1.25, true)));
    assert_eq!(v(3), Some(V::Text("one".into())));
    assert_eq!(v(4), Some(V::Text("a & b".into())));
    assert_eq!(v(5), Some(V::Bool(true)));
    assert_eq!(v(6), Some(V::Bool(false)));
    assert_eq!(v(7), Some(V::Err("#DIV/0!".into())));
    assert_eq!(v(8), Some(V::Iso("2024-05-01".into())));
    assert_eq!(v(9), Some(V::Text("in".into())));
    assert_eq!(
        v(10),
        Some(V::Num(2.0)),
        "a style past the table is a plain number"
    );
    assert_eq!(
        v(11),
        Some(V::Text("abc".into())),
        "no `t`, not a number: the text"
    );
    assert_eq!(got[1].2, 1, "the format index comes from the style");
    assert_eq!(got[10].2, 0);
}

#[test]
fn rich_and_phonetic_text_trimming_and_escapes() {
    let t = Tables::default();
    let text = |item: &str| -> String {
        let rows = format!(r#"<row r="1"><c r="A1" t="inlineStr"><is>{item}</is></c></row>"#);
        match cells(&rows, &t)[0].3.clone() {
            Some(V::Text(s)) => s,
            other => panic!("{item}: {other:?}"),
        }
    };
    assert_eq!(text("<t>plain</t>"), "plain");
    assert_eq!(text("<t>  both  </t>"), "both", "trimmed unless preserved");
    assert_eq!(text(r#"<t xml:space="preserve">  both  </t>"#), "  both  ");
    assert_eq!(
        text("<r><t>a </t></r><r><t> b</t></r>"),
        "ab",
        "each run is trimmed"
    );
    assert_eq!(
        text(r#"<r><t xml:space="preserve">a </t></r><r><t xml:space="preserve"> b</t></r>"#),
        "a  b"
    );
    assert_eq!(
        text("<t>漢字</t><rPh sb=\"0\" eb=\"2\"><t>カンジ</t></rPh>"),
        "漢字",
        "furigana is not text"
    );
    assert_eq!(
        text("<r><t>a</t></r><rPh><t>x</t></rPh><r><t>b</t></r>"),
        "ab"
    );
    assert_eq!(text("<t>a_x000D_b</t>"), "a\rb");
    assert_eq!(
        text("<t>_x005F_x000D_</t>"),
        "_x000D_",
        "an escaped escape is the literal text"
    );
    assert_eq!(
        text("<t>_x00zz_ _x00</t>"),
        "_x00zz_ _x00",
        "not an escape: kept"
    );
    assert_eq!(
        text("<t>_xD800_</t>"),
        "_xD800_",
        "a surrogate is not a character: kept"
    );
    assert_eq!(text("<t>a&amp;b&#65;&#x42;&unknown;</t>"), "a&bAB&unknown;");
    assert_eq!(text("<t><![CDATA[<x>]]></t>"), "<x>");
    assert_eq!(text(""), "", "an item without text is the empty string");
    assert_eq!(text("<t>a</t><t>b</t>"), "ab");
    assert_eq!(
        text("<t>line1\r\nline2</t>"),
        "line1\nline2",
        "line ends are normalised (XML 1.0)"
    );
}

#[test]
fn positions_follow_the_files_rules() {
    let t = Tables::default();
    let got = cells(
        r#"<row r="3"><c><v>1</v></c><c><v>2</v></c><c r="E3"><v>3</v></c><c><v>4</v></c></row>
           <row><c><v>5</v></c></row>
           <row r="10"><c r="b2"><v>6</v></c></row>
           <row r="x"><c><v>7</v></c></row>"#,
        &t,
    );
    let at: Vec<(u32, u32)> = got.iter().map(|c| (c.0, c.1)).collect();
    // Row 3; the cell with a bad `r` row follows the previous row.
    assert_eq!(
        at,
        [(2, 0), (2, 1), (2, 4), (2, 5), (3, 0), (1, 1), (10, 0)]
    );
}

#[test]
fn the_last_value_child_wins_and_a_formula_alone_is_a_cell_without_value() {
    let t = Tables::default();
    let got = cells(
        r#"<row r="1">
        <c r="A1"><f>1+1</f></c>
        <c r="B1"><f>1+1</f><v>2</v></c>
        <c r="C1" t="inlineStr"><is><t>x</t></is><v>9</v></c>
        <c r="D1"><v>1</v><v>2</v></c>
        <c r="E1"><f t="shared" ref="E1:E2" si="0">E2*2</f><v>1</v></c>
        </row>"#,
        &t,
    );
    assert_eq!(got[0].3, None);
    assert_eq!(got[0].4.as_deref(), Some("1+1"));
    assert_eq!(got[1].3, Some(V::Num(2.0)));
    assert_eq!(
        got[2].3, None,
        "the `<v>` of an inline-string cell replaces what was read before"
    );
    assert_eq!(got[3].3, Some(V::Num(2.0)));
    assert_eq!(got[4].4.as_deref(), Some("E2*2"));
}

#[test]
fn only_the_cells_inside_sheet_data_are_cells_and_merges_are_found_anywhere_after_it() {
    let xml = format!(
        r#"<worksheet xmlns="{NS}"><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><mergeCells><mergeCell ref="A1:B2"/></mergeCells><extLst><ext><c r="A9"><v>1</v></c></ext></extLst></worksheet>"#
    );
    let mut n = 0;
    let merges = xlsx::parse_sheet(
        xml.as_bytes(),
        &Tables::default(),
        &Limits::default(),
        None,
        |_| {
            n += 1;
            true
        },
    )
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        merges,
        vec![MergeRange {
            row0: 0,
            col0: 0,
            row1: 1,
            col1: 1
        }]
    );
}

#[test]
fn a_chart_sheet_is_not_a_sheet_and_a_missing_part_is_corrupt() {
    let dir = tmp("xlsx_chart");
    let p = write(&dir, "g.xlsx", &gen_package(7));
    let book = xlsx::open(&p, &Limits::default()).unwrap();
    let names: Vec<&str> = book.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Sheet1", "Hidden", "Deep", "Last one"],
        "Chart1 is not listed"
    );
    let wb = super::workbook::load_workbook_unguarded(&p, &LoadOptions::default()).unwrap();
    assert_eq!(
        wb.hidden_sheets, 2,
        "a chart sheet is not a hidden worksheet"
    );
    assert_eq!(wb.sheets.len(), 2);
}

#[test]
fn parts_are_found_without_regard_to_case_or_backslashes() {
    // Some producers write `xl\worksheets\sheet1.xml`; calamine finds those too.
    let root = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let wb = format!(
        r#"<workbook xmlns="{NS}" xmlns:r="x"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let rels = r#"<Relationships xmlns="p"><Relationship Id="rId1" Type="x/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
    let sheet = body(r#"<row r="1"><c r="A1"><v>5</v></c></row>"#);
    let bytes = deflated(&[
        ("_rels/.rels", root.as_bytes()),
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("XL\\Worksheets\\Sheet1.XML", sheet.as_bytes()),
    ]);
    let dir = tmp("xlsx_backslash");
    let p = write(&dir, "b.xlsx", &bytes);
    let mut book = xlsx::open(&p, &Limits::default()).unwrap();
    let mut n = 0;
    book.read_sheet("xl/worksheets/sheet1.xml", &Limits::default(), None, |_| {
        n += 1;
        true
    })
    .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn styles_and_shared_strings_are_found_through_the_relationships() {
    // A package whose shared strings / styles have other names than the usual ones.
    let root = r#"<Relationships xmlns="p"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let wb = format!(
        r#"<workbook xmlns="{NS}" xmlns:r="x"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let rels = r#"<Relationships xmlns="p"><Relationship Id="rId1" Type="x/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="strings.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="/xl/st.xml"/></Relationships>"#;
    let sst = format!(r#"<sst xmlns="{NS}"><si><t>hello</t></si></sst>"#);
    let st = format!(
        r#"<styleSheet xmlns="{NS}"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>"#
    );
    let sheet =
        body(r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" s="1"><v>44927</v></c></row>"#);
    let bytes = deflated(&[
        ("_rels/.rels", root.as_bytes()),
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/strings.xml", sst.as_bytes()),
        ("xl/st.xml", st.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
    ]);
    let dir = tmp("xlsx_names");
    let p = write(&dir, "n.xlsx", &bytes);
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 0).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.display(0, 0), "hello");
    assert_eq!(s.display(0, 1), "1/1/2023");
    assert_eq!(s.cell(0, 1).unwrap().cell_type(), CellType::DateTime);
}

// ---------------------------------------------------------------------------------------------
// safety
// ---------------------------------------------------------------------------------------------

#[test]
fn billion_laughs_is_not_expanded() {
    let dir = tmp("xlsx_laughs");
    let dtd = r#"<!DOCTYPE s [<!ENTITY a "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">]>"#;
    let sheet = format!(
        r#"<?xml version="1.0"?>{dtd}<worksheet xmlns="{NS}"><sheetData><row r="1"><c r="A1" t="str"><v>&c;</v></c><c r="B1" t="inlineStr"><is><t>&c;</t></is></c><c r="C1"><f>&c;</f><v>1</v></c></row></sheetData></worksheet>"#
    );
    let p = one_sheet(&dir, "l.xlsx", &sheet);
    let r = read_ours(&p).reads.remove(0).unwrap();
    for c in 0..2 {
        match &r.values[&(0, c)] {
            V::Text(t) => assert_eq!(t, "&c;", "kept as written, never expanded"),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(r.formulas[&(0, 2)], "&c;");
}

#[test]
fn a_huge_value_is_gathered_only_up_to_the_text_budget() {
    // Text that arrives as many pieces (here: references) stops being gathered at the text budget
    // of 64 KiB; the builder then refuses the cell. (Plain text between two tags is one event of
    // the XML parser and is held whole; the part-size cap bounds that.)
    let limits = Limits {
        max_sheet_text_bytes: 64 * 1024,
        ..Limits::default()
    };
    let big = "&amp;".repeat(400_000);
    let xml = body(&format!(
        r#"<row r="1"><c r="A1" t="str"><v>{big}</v></c><c r="B1" t="inlineStr"><is><t>{big}</t></is></c><c r="C1"><f>{big}</f></c></row>"#
    ));
    let mut seen = Vec::new();
    xlsx::parse_sheet(xml.as_bytes(), &Tables::default(), &limits, None, |c| {
        seen.push((
            match c.value {
                Some(Val::Text(t)) => t.len(),
                _ => 0,
            },
            c.formula.map_or(0, str::len),
        ));
        true
    })
    .unwrap();
    assert_eq!(seen.len(), 3);
    for (t, f) in seen {
        assert!(t <= 64 * 1024 + 1 && f <= 64 * 1024 + 1, "{t} {f}");
    }
    // Through the loader the sheet is cut by the budget instead of holding the text.
    let dir = tmp("xlsx_hugev");
    let p = one_sheet(&dir, "h.xlsx", &xml);
    let wb = super::workbook::load_workbook_sheet(
        &p,
        &LoadOptions {
            locale: Locale::En,
            limits,
        },
        0,
    )
    .unwrap();
    assert!(wb.sheets[0].rows_truncated);
    assert!(wb.sheets[0].cell(0, 0).is_none());
}

#[test]
fn a_shared_string_table_over_the_text_limit_is_refused() {
    let dir = tmp("xlsx_sst_limit");
    let items: String = (0..2000)
        .map(|i| format!("<si><t>string number {i}</t></si>"))
        .collect();
    let p = Pkg::new()
        .sheet("S", "visible", body(""))
        .shared(format!(
            r#"<sst xmlns="{NS}" uniqueCount="2000">{items}</sst>"#
        ))
        .write(&dir, "s.xlsx");
    let tight = Limits {
        max_text_bytes: 8 * 1024,
        ..Limits::default()
    };
    assert_eq!(
        xlsx::open(&p, &tight).err(),
        Some(OfficeError::TooLarge { what: "text" })
    );
    // A forged `uniqueCount` is refused without reading the table.
    let p = Pkg::new()
        .sheet("S", "visible", body(""))
        .shared(format!(
            r#"<sst xmlns="{NS}" uniqueCount="4000000000"><si><t>x</t></si></sst>"#
        ))
        .write(&dir, "u.xlsx");
    assert_eq!(
        xlsx::open(&p, &Limits::default()).err(),
        Some(OfficeError::TooLarge { what: "text" })
    );
    assert!(xlsx::open(
        &p,
        &Limits {
            max_text_bytes: u64::MAX,
            ..Limits::default()
        }
    )
    .is_ok());
}

#[test]
fn the_shared_strings_are_kept_in_one_buffer() {
    let t = SharedStrings::from_strs(&["", "ab", "日本", ""]);
    assert_eq!(t.len(), 4);
    assert_eq!(
        (0..4).map(|i| t.get(i).unwrap()).collect::<Vec<_>>(),
        ["", "ab", "日本", ""]
    );
    assert_eq!(t.get(4), None);
}

#[test]
fn a_cancelled_read_stops_at_the_next_row() {
    let mut rows = String::new();
    for r in 1..=3000 {
        rows += &format!(r#"<row r="{r}"><c r="A{r}"><v>1</v></c></row>"#);
    }
    let xml = body(&rows);
    let cancel = Cancel::new(|| true);
    let mut n = 0;
    xlsx::parse_sheet(
        xml.as_bytes(),
        &Tables::default(),
        &Limits::default(),
        Some(&cancel),
        |_| {
            n += 1;
            true
        },
    )
    .unwrap();
    assert!(n < 1100, "{n}");
}

#[test]
fn a_sink_that_says_stop_ends_the_read() {
    let mut rows = String::new();
    for r in 1..=100 {
        rows += &format!(r#"<row r="{r}"><c r="A{r}"><v>1</v></c></row>"#);
    }
    let xml = body(&rows);
    let mut n = 0;
    xlsx::parse_sheet(
        xml.as_bytes(),
        &Tables::default(),
        &Limits::default(),
        None,
        |_| {
            n += 1;
            n < 10
        },
    )
    .unwrap();
    assert_eq!(n, 10);
}

/// Hostile and damaged input, in konoma's own code and **outside `catch_silent`**: whatever comes
/// back, nothing panics (a panic here is a failure of the test, not a contained `Corrupt`).
#[test]
fn mutated_packages_never_panic_in_the_reader() {
    let dir = tmp("xlsx_fuzz");
    let mut seed = 0x1234_5678_9ABC_DEF1u64;
    let mut ok = 0;
    let mut err = 0;
    for base in 1..=6u64 {
        let original = write(&dir, "base.xlsx", &gen_package(base * 7919));
        let parts = read_parts(&original);
        for round in 0..120 {
            let mut mutated = parts.clone();
            for _ in 0..1 + (xorshift(&mut seed) % 3) {
                let which = (xorshift(&mut seed) as usize) % mutated.len();
                let (name, data) = &mut mutated[which];
                if name.ends_with(".xml") || name.ends_with(".rels") {
                    let kind = xorshift(&mut seed);
                    mutate(data, &mut seed, kind);
                }
            }
            let refs: Vec<(&str, &[u8])> = mutated
                .iter()
                .map(|(n, c)| (n.as_str(), c.as_slice()))
                .collect();
            let p = write(&dir, "m.xlsx", &deflated(&refs));
            let limits = Limits {
                max_sheet_cells: 500,
                max_rows: 60,
                max_cols: 20,
                max_sheet_text_bytes: 1 << 16,
                max_text_bytes: 1 << 18,
                ..Limits::default()
            };
            let r = std::panic::catch_unwind(|| {
                let Ok(mut book) = xlsx::open(&p, &limits) else {
                    return false;
                };
                for s in book.sheets.clone() {
                    let _ = book.read_sheet(&s.part, &limits, None, |_| true);
                }
                true
            });
            match r {
                Ok(true) => ok += 1,
                Ok(false) => err += 1,
                Err(_) => panic!("the reader panicked: base {base}, round {round}"),
            }
        }
    }
    assert!(
        ok > 50 && err > 5,
        "ok {ok}, err {err}: the mutations should do both"
    );
}

/// Byte-level damage to a *sheet* part alone: every prefix of a small sheet, and every single
/// byte replaced by a few troublemakers.
#[test]
fn every_prefix_and_byte_flip_of_a_sheet_is_survived() {
    let sheet = body(
        r#"<row r="1"><c r="A1" s="1" t="s"><v>0</v></c><c r="B1" t="str"><f>A1&amp;"x"</f><v>a&amp;b</v></c><c r="C1" t="inlineStr"><is><r><t>a</t></r><rPh><t>b</t></rPh></is></c><c r="D1"><f t="shared" ref="D1:D2" si="0">A1+B1</f><v>1</v></c></row><row r="2"><c><f t="shared" si="0"/><v>2</v></c><c r="XFE9" t="e"><v>#N/A</v></c></row>"#,
    );
    let bytes = sheet.as_bytes();
    let tables = tables_with(&["s"], &[(0, DateKind::Plain), (1, DateKind::Date)]);
    let limits = Limits::default();
    for end in 0..=bytes.len() {
        let _ = xlsx::parse_sheet(&bytes[..end], &tables, &limits, None, |_| true);
    }
    for i in 0..bytes.len() {
        for b in [0u8, b'<', b'>', b'&', b'"', b'/', 0xFF, 0xC3] {
            let mut m = bytes.to_vec();
            m[i] = b;
            let _ = xlsx::parse_sheet(m.as_slice(), &tables, &limits, None, |_| true);
        }
    }
}

#[test]
fn absurd_row_and_column_numbers_never_index_anything() {
    let t = Tables::default();
    for rows in [
        r#"<row r="4294967295"><c r="A4294967295"><v>1</v></c></row>"#,
        r#"<row r="99999999999999999999"><c r="A1"><v>1</v></c></row>"#,
        r#"<row r="1"><c r="ZZZZZZZZZZZZZZZZZZZZ1"><v>1</v></c></row>"#,
        r#"<row r="1"><c r="A18446744073709551616"><v>1</v></c></row>"#,
        r#"<row r="0"><c r="A0"><v>1</v></c></row>"#,
        r#"<row r="-1"><c r="-1"><v>1</v></c></row>"#,
        r#"<row r="1"><c r="1A"><v>1</v></c><c r=""><v>1</v></c><c r="$A$1"><v>1</v></c></row>"#,
    ] {
        let xml = body(rows);
        let mut n = 0;
        xlsx::parse_sheet(xml.as_bytes(), &t, &Limits::default(), None, |_| {
            n += 1;
            true
        })
        .unwrap_or_else(|e| panic!("{rows}: {e:?}"));
        assert!(n >= 1, "{rows}");
    }
    // And the loader's answer for the worst of them: the rows before are shown, the sheet is cut.
    let dir = tmp("xlsx_absurd2");
    let p = one_sheet(
        &dir,
        "a.xlsx",
        &body(
            r#"<row r="1"><c r="A1"><v>1</v></c></row><row r="99999999999999999999"><c><v>1</v></c></row>"#,
        ),
    );
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 0).unwrap();
    assert!(wb.sheets[0].rows_truncated);
    assert_eq!(wb.sheets[0].display(0, 0), "1");
}

#[test]
fn an_empty_or_rootless_sheet_part_is_an_empty_sheet() {
    let t = Tables::default();
    for xml in [
        "",
        "<worksheet/>",
        r#"<chartsheet xmlns="x"/>"#,
        "<worksheet><sheetData/></worksheet>",
        "not xml at all",
    ] {
        let mut n = 0;
        let _ = xlsx::parse_sheet(xml.as_bytes(), &t, &Limits::default(), None, |_| {
            n += 1;
            true
        });
        assert_eq!(n, 0, "{xml:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// speed (run by hand: `cargo test --release -- --ignored bench_xlsx --nocapture`)
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "benchmark: set KONOMA_BENCH_XLSX to a large xlsx"]
fn bench_xlsx_load() {
    let Some(p) = std::env::var_os("KONOMA_BENCH_XLSX").map(PathBuf::from) else {
        return;
    };
    for i in 0..3 {
        let t = std::time::Instant::now();
        let wb = load_workbook_sheet(&p, &LoadOptions::default(), 0).unwrap();
        eprintln!(
            "BENCH run {i}: {:?} rows={} cols={} trunc={}",
            t.elapsed(),
            wb.sheets[0].nrows,
            wb.sheets[0].ncols,
            wb.sheets[0].rows_truncated
        );
    }
}
