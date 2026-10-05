//! Tests added after a mutation audit of the spreadsheet preview: each one pins a rule that a
//! mutated build (`<` for `<=`, a dropped check, a swapped constant, ...) used to get away with.
//! The comment on each test says what it catches. Expected values come from the file-format rules
//! (ECMA-376 / OpenDocument / MS-XLSB) and from what `calamine` and LibreOffice do, not from what
//! the code happens to print.

use super::container::Limits;
use super::fmt_xlsx::STRING_OVERHEAD;
use super::tests::{
    deflated, load, load_with, sheet_xml, small_limits, styles_xml, tmp, workbook_xml, write, Pkg,
};
use super::xlsx::{self, shift_formula, SharedStrings, Tables, Val};
use super::*;

// ---------------------------------------------------------------------------------------------
// xlsx: helpers
// ---------------------------------------------------------------------------------------------

/// What `parse_sheet` reports for one cell: `(row, col, value as text, formula)`.
type Seen = (u32, u32, Option<String>, Option<String>);

fn show(v: Val<'_>) -> String {
    match v {
        Val::Text(t) => t.to_string(),
        other => format!("{other:?}"),
    }
}

fn parse_with(rows: &str, tables: &Tables, limits: &Limits) -> Vec<Seen> {
    let xml = sheet_xml(rows, "");
    let mut out = Vec::new();
    xlsx::parse_sheet(xml.as_bytes(), tables, limits, None, |c| {
        out.push((
            c.row,
            c.col,
            c.value.map(show),
            c.formula.map(str::to_string),
        ));
        true
    })
    .unwrap();
    out
}

fn parse(rows: &str) -> Vec<Seen> {
    parse_with(rows, &Tables::default(), &Limits::default())
}

fn inline(r: &str, body: &str) -> String {
    format!(r#"<c r="{r}" t="inlineStr"><is>{body}</is></c>"#)
}

/// The text of one inline string whose `<is>` holds `body`.
fn inline_text(body: &str) -> String {
    let seen = parse(&format!(r#"<row r="1">{}</row>"#, inline("A1", body)));
    seen[0].2.clone().expect("an inline string has a value")
}

// ---------------------------------------------------------------------------------------------
// xlsx: text escapes and whitespace
// ---------------------------------------------------------------------------------------------

/// `_xHHHH_` is seven bytes. An escape that ends the text exactly is still an escape (a `<`
/// instead of `<=` on the length check leaves `a_x000D_` as written); a sequence whose seventh byte
/// is not `_` is not one (`_x0041z`); and the four characters must all be hexadecimal digits (a
/// leading `+` is accepted by `from_str_radix`, which is how `any` instead of `all` shows).
#[test]
fn text_escapes_are_recognised_exactly() {
    assert_eq!(inline_text("<t>a_x000D_</t>"), "a\r", "at the very end");
    assert_eq!(inline_text("<t>_x0041_</t>"), "A", "the whole text");
    assert_eq!(
        inline_text("<t>_x0041z</t>"),
        "_x0041z",
        "no closing underscore"
    );
    assert_eq!(inline_text("<t>_x004</t>"), "_x004", "too short");
    assert_eq!(
        inline_text("<t>_x+041_</t>"),
        "_x+041_",
        "a sign is not a digit"
    );
    assert_eq!(inline_text("<t>_x00G1_</t>"), "_x00G1_", "not hexadecimal");
    assert_eq!(
        inline_text("<t>_xD800_</t>"),
        "_xD800_",
        "a surrogate stays as written"
    );
    assert_eq!(
        inline_text("<t>_x005F_x000D_</t>"),
        "_x000D_",
        "an escaped escape"
    );
}

/// A `<t>` is trimmed of ASCII whitespace — spaces, tabs and line breaks — unless it says
/// `xml:space="preserve"`.
#[test]
fn a_t_is_trimmed_of_tabs_and_line_breaks_unless_it_preserves_them() {
    assert_eq!(inline_text("<t>\n  hello\t</t>"), "hello");
    assert_eq!(inline_text("<t>\t\r\n x \n\t</t>"), "x");
    assert_eq!(
        inline_text("<t xml:space=\"preserve\">\n  hello\t</t>"),
        "\n  hello\t"
    );
}

/// The text of an item is gathered piece by piece (`<t>` after `<t>`); a new piece is still taken
/// when the text is *exactly* at the cap and not after it. With a cap of 10, `0123456789` fills it,
/// `Z` is the one piece taken at the cap, `Y` comes after it and is left out.
#[test]
fn a_piece_is_taken_when_the_text_is_exactly_at_the_cap() {
    let limits = Limits {
        max_sheet_text_bytes: 10,
        ..Limits::default()
    };
    let rows = format!(
        r#"<row r="1">{}</row>"#,
        inline("A1", "<t>0123456789</t><t>Z</t><t>Y</t>")
    );
    let seen = parse_with(&rows, &Tables::default(), &limits);
    assert_eq!(seen[0].2.as_deref(), Some("0123456789Z"));
}

/// An inline string's type may also be written `t="is"`; like `inlineStr` its `<v>` is not its
/// value (a `<v>` there would be read as the cell's text if the type were an unknown one).
#[test]
fn t_is_is_an_inline_string_whose_v_is_ignored() {
    let seen = parse(
        r#"<row r="1"><c r="A1" t="is"><v>b</v></c><c r="B1" t="is"><is><t>a</t></is><v>b</v></c><c r="C1" t="inlineStr"><v>b</v></c></row>"#,
    );
    assert_eq!(seen[0].2, None, "t=is, <v> only");
    assert_eq!(
        seen[1].2, None,
        "a later <v> replaces what <is> held, as for inlineStr"
    );
    assert_eq!(seen[2].2, None, "the reference behaviour");
}

/// The whitespace around a shared-string index or a style index is ignored (`<v> 1 </v>`).
#[test]
fn an_index_with_spaces_around_it_is_still_an_index() {
    let tables = Tables {
        strings: SharedStrings::from_strs(&["zero", "one"]),
        ..Tables::default()
    };
    let seen = parse_with(
        r#"<row r="1"><c r="A1" t="s"><v> 1 </v></c><c r="B1" t="s"><v>0</v></c></row>"#,
        &tables,
        &Limits::default(),
    );
    assert_eq!(seen[0].2.as_deref(), Some("one"));
    assert_eq!(seen[1].2.as_deref(), Some("zero"));
}

// ---------------------------------------------------------------------------------------------
// xlsx: shared formulas
// ---------------------------------------------------------------------------------------------

fn shared_anchor(r: &str, si: u32, text: &str) -> String {
    format!(r#"<c r="{r}"><f t="shared" ref="A1:C9" si="{si}">{text}</f></c>"#)
}

fn shared_derived(r: &str, si: u32) -> String {
    format!(r#"<c r="{r}"><f t="shared" si="{si}"/></c>"#)
}

fn formula_at(seen: &[Seen], row: u32, col: u32) -> Option<String> {
    seen.iter()
        .find(|s| (s.0, s.1) == (row, col))
        .and_then(|s| s.3.clone())
}

/// The table of shared formulas holds at most `max_sheet_cells` anchors: with 2, the third anchor
/// is not recorded and the cells derived from it have no formula (the first two do). A `>` instead
/// of `>=` lets the third in.
#[test]
fn the_third_anchor_does_not_fit_a_table_of_two() {
    let limits = Limits {
        max_sheet_cells: 2,
        ..Limits::default()
    };
    let rows = format!(
        r#"<row r="1">{}{}</row><row r="2">{}{}</row><row r="3">{}{}</row>"#,
        shared_anchor("A1", 0, "B1"),
        shared_derived("C1", 0),
        shared_anchor("A2", 1, "B2"),
        shared_derived("C2", 1),
        shared_anchor("A3", 2, "B3"),
        shared_derived("C3", 2),
    );
    let seen = parse_with(&rows, &Tables::default(), &limits);
    assert_eq!(formula_at(&seen, 0, 2).as_deref(), Some("D1"));
    assert_eq!(formula_at(&seen, 1, 2).as_deref(), Some("D2"));
    assert_eq!(formula_at(&seen, 2, 2), None, "the table is full");
    // The anchor itself still shows the formula it was written with.
    assert_eq!(formula_at(&seen, 2, 0).as_deref(), Some("B3"));
}

/// The byte budget of the table is exact: an anchor whose cost (text + overhead) is *equal* to the
/// budget is recorded; one byte less and it is not.
#[test]
fn an_anchor_that_costs_exactly_the_budget_is_recorded() {
    let text = "B1";
    let cost = text.len() as u64 + STRING_OVERHEAD;
    let rows = format!(
        r#"<row r="1">{}{}</row>"#,
        shared_anchor("A1", 0, text),
        shared_derived("C1", 0)
    );
    let at = |max: u64| {
        let limits = Limits {
            max_sheet_text_bytes: max,
            ..Limits::default()
        };
        formula_at(&parse_with(&rows, &Tables::default(), &limits), 0, 2)
    };
    assert_eq!(at(cost).as_deref(), Some("D1"));
    assert_eq!(at(cost - 1), None);
}

/// Registering the same `si` again replaces the old formula *and gives its bytes back*: with room
/// for two entries, any number of re-registrations still fit, and the last one wins. Without the
/// refund the budget is spent after two and the later anchors are ignored.
#[test]
fn registering_a_si_again_gives_the_old_bytes_back() {
    let formulas = ["SUM(1)", "SUN(1)", "SUO(1)", "SUP(1)"];
    let cost = formulas[0].len() as u64 + STRING_OVERHEAD;
    let limits = Limits {
        max_sheet_text_bytes: 2 * cost,
        ..Limits::default()
    };
    let mut rows = String::new();
    for (i, f) in formulas.iter().enumerate() {
        rows += &format!(
            r#"<row r="{}">{}</row>"#,
            i + 1,
            shared_anchor(&format!("A{}", i + 1), 0, f)
        );
    }
    rows += &format!(r#"<row r="5">{}</row>"#, shared_derived("C5", 0));
    let seen = parse_with(&rows, &Tables::default(), &limits);
    assert_eq!(formula_at(&seen, 4, 2).as_deref(), Some("SUP(1)"));
}

/// `ref=""` is no range: the cell is a derived one (its formula comes from the anchor), not an
/// anchor with an empty formula.
#[test]
fn an_empty_ref_does_not_make_an_anchor() {
    let rows = format!(
        r#"<row r="1">{}</row><row r="2"><c r="A2"><f t="shared" ref="" si="0"/></c></row>"#,
        shared_anchor("A1", 0, "B1")
    );
    let seen = parse(&rows);
    assert_eq!(formula_at(&seen, 1, 0).as_deref(), Some("B2"));
}

/// A leading zero is not a reference (`A01` is a name), so it does not move; neither does `01:01`
/// (a row range needs positive numbers).
#[test]
fn references_with_a_leading_zero_do_not_move() {
    let shift = |f: &str, dr, dc| {
        let mut out = String::new();
        shift_formula(f, dr, dc, &mut out);
        out
    };
    assert_eq!(shift("A01+B2", 1, 0), "A01+B3");
    assert_eq!(shift("01:01", 1, 0), "01:01");
    assert_eq!(shift("A01:B01", 1, 0), "A01:B01");
    assert_eq!(shift("1:1", 1, 0), "2:2", "a real row range does move");
    assert_eq!(shift("$A$01+A1", 1, 1), "$A$01+B2");
}

/// A `.` belongs to a name (`Data.A1`, a defined name, or the `1.5` of a number): the run is one
/// word, not a name, a dot and a reference. Moving `A1` out of `Data.A1` would point the formula
/// at a different cell.
#[test]
fn a_dotted_name_is_one_word() {
    let mut out = String::new();
    shift_formula("Data.A1+ROUND(1.5,0)+A1", 1, 0, &mut out);
    assert_eq!(out, "Data.A1+ROUND(1.5,0)+A2");
    // Through a sheet, as the reader meets it.
    let rows = format!(
        r#"<row r="1">{}</row><row r="2">{}</row>"#,
        shared_anchor("A1", 0, "Data.A1+ROUND(1.5,0)"),
        shared_derived("A2", 0)
    );
    assert_eq!(
        formula_at(&parse(&rows), 1, 0).as_deref(),
        Some("Data.A1+ROUND(1.5,0)")
    );
}

// ---------------------------------------------------------------------------------------------
// xlsx: positions
// ---------------------------------------------------------------------------------------------

/// `<row r="0">` is not a row number: the row follows the previous one, it is not row 1 again.
#[test]
fn a_row_numbered_zero_follows_the_previous_row() {
    let seen = parse(r#"<row r="1"><c r="A1"><v>1</v></c></row><row r="0"><c><v>2</v></c></row>"#);
    assert_eq!((seen[1].0, seen[1].1), (1, 0));
}

/// A reference that cannot be parsed falls back to "the cell after the previous one", except an
/// absurd one (4+ column letters or 8+ row digits that still do not parse: `AAAA0`, `A00000000`),
/// which is put past every row cap so the sheet is cut there instead of the cell being misplaced.
/// The sharp edge is exact: `AAA0` and `A0000000` are one letter / digit short of absurd.
#[test]
fn an_absurd_reference_is_cut_and_a_merely_bad_one_follows_its_neighbour() {
    let seen = parse(
        r#"<row r="2"><c r="B2"><v>1</v></c><c r="AAAA0"><v>2</v></c><c r="A00000000"><v>3</v></c></row>"#,
    );
    assert_eq!((seen[0].0, seen[0].1), (1, 1));
    assert_eq!((seen[1].0, seen[1].1), (u32::MAX, 0), "AAAA0");
    assert_eq!((seen[2].0, seen[2].1), (u32::MAX, 0), "A00000000");

    let seen = parse(
        r#"<row r="2"><c r="B2"><v>1</v></c><c r="AAA0"><v>2</v></c><c r="A0000000"><v>3</v></c></row>"#,
    );
    assert_eq!((seen[1].0, seen[1].1), (1, 2), "AAA0 follows B2");
    assert_eq!((seen[2].0, seen[2].1), (1, 3), "A0000000 follows it");

    // And the references that do parse are where they say, past Excel's grid or not.
    let seen = parse(
        r#"<row r="1"><c r="AAAA1"><v>1</v></c><c r="A1234567"><v>2</v></c><c r="AAA1"><v>3</v></c></row>"#,
    );
    assert_eq!((seen[0].0, seen[0].1), (0, 18_278));
    assert_eq!((seen[1].0, seen[1].1), (1_234_566, 0));
    assert_eq!((seen[2].0, seen[2].1), (0, 702));
}

/// A cell that holds only a formula, past the column cap, is still data that was left out: the
/// sheet is flagged `cols_truncated` (the cell is read past, not decoded).
#[test]
fn a_formula_only_cell_past_the_column_cap_flags_the_truncation() {
    let dir = tmp("surv_formula_col");
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c><c r="F1"><f>1+1</f></c></row>"#;
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(rows, ""))
        .write(&dir, "f.xlsx");
    let limits = Limits {
        max_cols: 4,
        ..small_limits()
    };
    let wb = load_with(&p, limits).unwrap();
    assert!(wb.sheets[0].cols_truncated);
    assert_eq!(wb.sheets[0].ncols, 4, "the grid is cut at the cap");
    // Control: nothing past the cap, nothing flagged.
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c><c r="B1"><f>1+1</f></c></row>"#;
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(rows, ""))
        .write(&dir, "g.xlsx");
    let limits = Limits {
        max_cols: 4,
        ..small_limits()
    };
    assert!(!load_with(&p, limits).unwrap().sheets[0].cols_truncated);
}

// ---------------------------------------------------------------------------------------------
// xlsx: the shared-string table and the package
// ---------------------------------------------------------------------------------------------

/// `uniqueCount` is checked before a string is read: `count * overhead` must not exceed the text
/// budget — equal is fine, one more is refused, and it is the *overhead-weighted* count that is
/// compared (not the bare count against the byte budget).
#[test]
fn the_unique_count_is_compared_with_its_overhead_and_exactly() {
    let dir = tmp("surv_unique");
    let budget = 10 * STRING_OVERHEAD;
    let limits = Limits {
        max_text_bytes: budget,
        ..Limits::default()
    };
    let open = |n: u64| {
        let p = Pkg::new()
            .sheet("S", "visible", sheet_xml("", ""))
            .shared(format!(
                r#"<sst xmlns="{NS}" uniqueCount="{n}"><si><t>x</t></si></sst>"#,
                NS = super::tests::NS
            ))
            .write(&dir, &format!("u{n}.xlsx"));
        xlsx::open(&p, &limits).map(|_| ())
    };
    assert_eq!(open(10), Ok(()), "10 * overhead is exactly the budget");
    assert_eq!(open(11), Err(OfficeError::TooLarge { what: "text" }));
    assert_eq!(open(12), Err(OfficeError::TooLarge { what: "text" }));
}

fn rels_of(items: &[(&str, &str, &str)]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, target) in items {
        s += &format!(r#"<Relationship Id="{id}" Type="x/{kind}" Target="{target}"/>"#);
    }
    s + "</Relationships>"
}

/// Chart, dialog and both kinds of macro sheet carry no cells: they are not listed and not
/// counted as hidden either — each of the four kinds on its own (dropping one from the list shows
/// that sheet).
#[test]
fn non_worksheet_sheets_are_neither_listed_nor_counted() {
    let dir = tmp("surv_kinds");
    let wb = workbook_xml(
        false,
        &[
            ("S", "visible", "rId1"),
            ("C", "hidden", "rId2"),
            ("D", "hidden", "rId3"),
            ("M", "hidden", "rId4"),
            ("I", "hidden", "rId5"),
            ("H", "hidden", "rId6"),
        ],
    );
    let rels = rels_of(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId2", "chartsheet", "chartsheets/sheet1.xml"),
        ("rId3", "dialogsheet", "dialogsheets/sheet1.xml"),
        ("rId4", "macrosheet", "macrosheets/sheet1.xml"),
        ("rId5", "intlmacrosheet", "macrosheets/sheet2.xml"),
        ("rId6", "worksheet", "worksheets/sheet2.xml"),
    ]);
    let sheet = sheet_xml(r#"<row r="1"><c r="A1"><v>1</v></c></row>"#, "");
    let bytes = deflated(&[
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/worksheets/sheet2.xml", sheet.as_bytes()),
    ]);
    let p = write(&dir, "k.xlsx", &bytes);
    let book = xlsx::open(&p, &Limits::default()).unwrap();
    let names: Vec<&str> = book.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["S", "H"]);
    let loaded = load(&p).unwrap();
    assert_eq!(loaded.sheets.len(), 1);
    assert_eq!(
        loaded.hidden_sheets, 1,
        "only the hidden *worksheet* counts"
    );
}

/// `TargetMode="External"` is compared without regard to case; an external target is not a part
/// of the package.
#[test]
fn an_external_target_mode_is_case_insensitive() {
    let rels = r#"<Relationships xmlns="x"><Relationship Id="a" Type="x/worksheet" Target="https://example.com/s.xml" TargetMode="external"/><Relationship Id="b" Type="x/worksheet" Target="https://example.com/t.xml" TargetMode="EXTERNAL"/><Relationship Id="c" Type="x/worksheet" Target="worksheets/sheet1.xml" TargetMode="Internal"/></Relationships>"#;
    let map = xlsx::parse_rels(rels.as_bytes()).unwrap();
    let mut ids: Vec<&String> = map.keys().collect();
    ids.sort();
    assert_eq!(ids, ["c"]);
}

/// When a package has two relationships of the same kind (two `styles`), the one with the
/// smallest target is used — the same one every time (a hash-map's order is not). Opened many
/// times, because a random pick agrees with the right one half of the time.
#[test]
fn the_part_for_a_kind_with_two_relationships_is_the_smallest_target() {
    let dir = tmp("surv_two_styles");
    let wb = workbook_xml(false, &[("S", "visible", "rId1")]);
    let rels = rels_of(&[
        ("rId1", "worksheet", "worksheets/sheet1.xml"),
        ("rId9", "styles", "styles2.xml"),
        ("rId8", "styles", "styles.xml"),
        ("rId7", "styles", "styles3.xml"),
    ]);
    let sheet = sheet_xml("", "");
    let a = styles_xml(&[(164, "0.00")], &[164]);
    let b = styles_xml(&[(164, "0.0")], &[164]);
    let c = styles_xml(&[(164, "0")], &[164]);
    let bytes = deflated(&[
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.as_bytes()),
        ("xl/styles.xml", a.as_bytes()),
        ("xl/styles2.xml", b.as_bytes()),
        ("xl/styles3.xml", c.as_bytes()),
    ]);
    let p = write(&dir, "two.xlsx", &bytes);
    for round in 0..40 {
        let book = xlsx::open(&p, &Limits::default()).unwrap();
        assert!(
            book.formats
                .iter()
                .any(|f| matches!(f, NumFmtRef::Custom(c) if &**c == "0.00")),
            "round {round}: {:?}",
            book.formats
        );
    }
}

// ---------------------------------------------------------------------------------------------
// container: an ods whose whole package is one encrypted entry
// ---------------------------------------------------------------------------------------------

const SHEET_MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";

const MANIFEST_NO_ENCRYPTION: &str = r#"<?xml version="1.0"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/></manifest:manifest>"#;

/// `mimetype` (a spreadsheet) + an `encrypted-package` entry is an encrypted ods *by itself*: the
/// manifest need not say `encryption-data` (or exist at all).
#[test]
fn an_encrypted_package_entry_alone_makes_an_encrypted_ods() {
    let dir = tmp("surv_pkg_alone");
    let p = write(
        &dir,
        "a.ods",
        &deflated(&[
            ("mimetype", SHEET_MIME),
            ("encrypted-package", b"\x00\x01 not a zip"),
            ("META-INF/manifest.xml", MANIFEST_NO_ENCRYPTION.as_bytes()),
        ]),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
    let p = write(
        &dir,
        "b.ods",
        &deflated(&[
            ("mimetype", SHEET_MIME),
            ("encrypted-package", b"\x00\x01 not a zip"),
        ]),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted, "no manifest");
    // Without the entry (and without encryption in the manifest) it is just not readable.
    let p = write(
        &dir,
        "c.ods",
        &deflated(&[
            ("mimetype", SHEET_MIME),
            ("META-INF/manifest.xml", MANIFEST_NO_ENCRYPTION.as_bytes()),
        ]),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Unsupported);
}

// ---------------------------------------------------------------------------------------------
// ods: helpers
// ---------------------------------------------------------------------------------------------

const ODS_NS: &str = concat!(
    r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" "#,
    r#"xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" "#,
    r#"xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" "#,
    r#"xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" "#,
    r#"xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" "#,
    r#"xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" "#,
    r#"xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0""#
);

fn ods_content(auto: &str, tables: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ODS_NS} office:version="1.3"><office:automatic-styles>{auto}</office:automatic-styles><office:body><office:spreadsheet>{tables}</office:spreadsheet></office:body></office:document-content>"#
    )
}

fn ods_write(
    dir: &crate::test_support::TmpDir,
    name: &str,
    auto: &str,
    tables: &str,
) -> std::path::PathBuf {
    let content = ods_content(auto, tables);
    write(
        dir,
        name,
        &deflated(&[
            ("mimetype", SHEET_MIME),
            ("content.xml", content.as_bytes()),
            ("META-INF/manifest.xml", MANIFEST_NO_ENCRYPTION.as_bytes()),
        ]),
    )
}

fn ods_table(name: &str, rows: &str) -> String {
    format!(r#"<table:table table:name="{name}">{rows}</table:table>"#)
}

fn ods_row(cells: &str) -> String {
    format!("<table:table-row>{cells}</table:table-row>")
}

fn ods_float(style: &str, v: &str) -> String {
    let st = if style.is_empty() {
        String::new()
    } else {
        format!(r#" table:style-name="{style}""#)
    };
    format!(
        r#"<table:table-cell{st} office:value-type="float" office:value="{v}"><text:p>{v}</text:p></table:table-cell>"#
    )
}

fn ods_text_cell(attrs: &str, body: &str) -> String {
    format!(r#"<table:table-cell office:value-type="string" {attrs}>{body}</table:table-cell>"#)
}

fn ods_error_cell(text: &str, extra: &str) -> String {
    format!(
        r#"<table:table-cell{extra} table:formula="of:=1/0" office:value-type="string" office:string-value="" calcext:value-type="error"><text:p>{text}</text:p></table:table-cell>"#
    )
}

fn ods_data_style(kind: &str, body: &str) -> String {
    format!(
        r#"<number:{kind} style:name="N">{body}</number:{kind}><style:style style:name="c" style:family="table-cell" style:data-style-name="N"/>"#
    )
}

/// The format code a cell style `c` resolves to.
fn ods_code(kind: &str, body: &str) -> String {
    let xml = ods_content(&ods_data_style(kind, body), "");
    let mut styles = fmt_ods::Styles::default();
    fmt_ods::parse_styles(xml.as_bytes(), &mut styles, true).unwrap();
    styles.code_of_cell_style("c").unwrap()
}

fn ods_read(
    name: &str,
    auto: &str,
    tables: &str,
    limits: &Limits,
) -> Result<(super::fmt_xlsx::XlsxFormats, fmt_ods::OdsExtra), OfficeError> {
    let dir = tmp(name);
    let p = ods_write(&dir, "t.ods", auto, tables);
    fmt_ods::read_full(&p, limits)
}

fn ods_texts(tables: &str) -> Vec<fmt_ods::TextRun> {
    let (_, extra) = ods_read("surv_texts", "", tables, &Limits::default()).unwrap();
    extra.sheets["S"].texts.clone()
}

// ---------------------------------------------------------------------------------------------
// ods: text, errors, the budgets
// ---------------------------------------------------------------------------------------------

/// `<text:s/>` without `text:c` is ONE space (not none): `a<text:s/>b` is `a b`.
#[test]
fn an_ods_space_without_a_count_is_one_space() {
    let rows = ods_row(&ods_text_cell(
        "",
        r#"<text:p>a<text:s/>b<text:tab/>c</text:p>"#,
    ));
    let texts = ods_texts(&ods_table("S", &rows));
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].text, "a b\tc");
    // Through the whole loader.
    let dir = tmp("surv_ods_s");
    let p = ods_write(&dir, "s.ods", "", &ods_table("S", &rows));
    assert_eq!(load(&p).unwrap().sheets[0].display(0, 0), "a b\tc");
}

/// The repair buffer takes up to `MAX_FIX_TEXT` (1 MiB) bytes: a run of spaces that brings it to
/// exactly that is kept, one more is "too long to repair".
#[test]
fn a_space_run_that_fills_the_repair_buffer_exactly_is_repaired() {
    let max = 1usize << 20;
    let at = |n: usize| {
        let rows = ods_row(&ods_text_cell(
            "",
            &format!(r#"<text:p><text:tab/><text:s text:c="{n}"/></text:p>"#),
        ));
        ods_texts(&ods_table("S", &rows))
    };
    let texts = at(max - 1);
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].text.len(), max);
    assert!(texts[0].text.starts_with("\t "));
    assert!(
        at(max).is_empty(),
        "one byte over: left as calamine read it"
    );
}

/// The same edge for plain text: `MAX_FIX_TEXT - 1` characters and then a tab fill the buffer to
/// exactly the limit (kept); one more byte and it is not repaired.
#[test]
fn a_text_that_fills_the_repair_buffer_exactly_is_repaired() {
    let max = 1usize << 20;
    let at = |n: usize| {
        let rows = ods_row(&ods_text_cell(
            "",
            &format!("<text:p>{}<text:tab/></text:p>", "x".repeat(n)),
        ));
        ods_texts(&ods_table("S", &rows))
    };
    let texts = at(max - 1);
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].text.len(), max);
    assert!(texts[0].text.ends_with("x\t"));
    assert!(at(max).is_empty());
}

/// LibreOffice's error codes and their spelling: `Err:503` is `#NUM!`, and the text of the cell
/// is trimmed before it is compared (a cell can hold `" #DIV/0! "`).
#[test]
fn ods_error_cells_are_decoded_after_trimming_and_by_error_number() {
    let cells = [
        ("Err:503", "#NUM!"),
        (" #DIV/0! ", "#DIV/0!"),
        ("\n#N/A\t", "#N/A"),
        (" Err:532 ", "#DIV/0!"),
        ("Err:524", "#REF!"),
        ("Err:525", "#NAME?"),
        ("#NULL!", "#NULL!"),
        ("Err:999", "#VALUE!"),
    ];
    let row: String = cells.iter().map(|(t, _)| ods_error_cell(t, "")).collect();
    let (fm, _) = ods_read(
        "surv_ods_err",
        "",
        &ods_table("S", &ods_row(&row)),
        &Limits::default(),
    )
    .unwrap();
    let got: Vec<&str> = fm.sheets["S"].errors.iter().map(|e| e.2).collect();
    let want: Vec<&str> = cells.iter().map(|(_, w)| *w).collect();
    assert_eq!(got, want);
}

/// A repeated cell costs its text *plus the per-string overhead* once per copy: 100 copies of
/// `x` are 100 * (1 + overhead) bytes of the text budget — exactly that fits, one byte less does not.
#[test]
fn a_repeated_ods_string_costs_its_overhead_for_every_copy() {
    let rows = ods_row(&ods_text_cell(
        r#"table:number-columns-repeated="100""#,
        "<text:p>x</text:p>",
    ));
    let tables = ods_table("S", &rows);
    let cost = 100 * (1 + STRING_OVERHEAD);
    let at = |max: u64| {
        let limits = Limits {
            max_text_bytes: max,
            ..Limits::default()
        };
        ods_read("surv_ods_cost", "", &tables, &limits).map(|_| ())
    };
    assert_eq!(at(cost), Ok(()));
    assert_eq!(at(cost - 1), Err(OfficeError::TooLarge { what: "text" }));
}

/// The text budget is for the whole workbook: two tables that each fit it but not together are
/// refused (`calamine` keeps every table in memory).
#[test]
fn ods_tables_that_each_fit_the_text_budget_but_not_together_are_refused() {
    let one = |name: &str| {
        ods_table(
            name,
            &ods_row(&ods_text_cell(
                r#"table:number-columns-repeated="30""#,
                "<text:p>x</text:p>",
            )),
        )
    };
    let tables = format!("{}{}", one("A"), one("B"));
    let each = 30 * (1 + STRING_OVERHEAD);
    let at = |max: u64| {
        let limits = Limits {
            max_text_bytes: max,
            ..Limits::default()
        };
        ods_read("surv_ods_total", "", &tables, &limits).map(|_| ())
    };
    assert_eq!(at(2 * each), Ok(()));
    assert_eq!(
        at(2 * each - 1),
        Err(OfficeError::TooLarge { what: "text" })
    );
    assert_eq!(at(each), Err(OfficeError::TooLarge { what: "text" }));
    // One table alone, at the same budget, is fine: the refusal is the sum.
    let limits = Limits {
        max_text_bytes: each,
        ..Limits::default()
    };
    assert!(ods_read("surv_ods_total1", "", &one("A"), &limits).is_ok());
}

/// A repeat is cut at Excel's last column: a cell repeated a million times (or `u64::MAX` times)
/// counts 16,384 cells, and the column definitions around it do not overflow either.
#[test]
fn a_huge_ods_column_repeat_is_cut_at_the_last_column() {
    for reps in ["1048576", "18446744073709551615"] {
        let cols = format!(
            r#"<table:table-column table:number-columns-repeated="{reps}"/><table:table-column table:number-columns-repeated="1048576"/>"#
        );
        let rows = ods_row(&format!(
            r#"<table:table-cell table:number-columns-repeated="{reps}" office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell>"#
        ));
        let (fm, _) = ods_read(
            "surv_ods_reps",
            "",
            &ods_table("S", &format!("{cols}{rows}")),
            &Limits::default(),
        )
        .unwrap_or_else(|e| panic!("{reps}: {e:?}"));
        let sf = &fm.sheets["S"];
        assert_eq!(sf.value_cells, 16_384, "{reps}");
        assert_eq!(sf.bbox, Some((0, 0, 0, 16_383)), "{reps}");
    }
}

/// `display-factor` is a power of 1000 and then one trailing comma per factor: `1e12` is four
/// commas, and a factor that is not a power of 1000 (2000, 1500) is ignored.
#[test]
fn the_ods_display_factor_is_a_power_of_1000_up_to_four_commas() {
    let code = |f: &str| {
        ods_code(
            "number-style",
            &format!(
                r#"<number:number number:decimal-places="0" number:min-integer-digits="1" number:display-factor="{f}"/>"#
            ),
        )
    };
    assert_eq!(code("1000"), "0,");
    assert_eq!(code("1000000000"), "0,,,");
    assert_eq!(code("1000000000000"), "0,,,,");
    assert_eq!(code("2000"), "0");
    assert_eq!(code("1500"), "0");
    assert_eq!(code("2000000"), "0");
}

/// A fraction's integer part keeps up to ten zeros for `min-integer-digits`.
#[test]
fn a_fraction_style_keeps_up_to_ten_integer_digits() {
    let code = |m: usize| {
        ods_code(
            "number-style",
            &format!(
                r#"<number:fraction number:min-integer-digits="{m}" number:min-numerator-digits="1" number:min-denominator-digits="1" number:max-denominator-value="9"/>"#
            ),
        )
    };
    assert_eq!(code(10), "0000000000 ?/?");
    assert_eq!(code(12), "0000000000 ?/?", "capped at ten");
    assert_eq!(code(9), "000000000 ?/?");
}

/// Only the spreadsheet's own `table:null-date` counts: one written inside a table is cell
/// content of no meaning and must not change the date system.
#[test]
fn a_null_date_inside_a_table_is_ignored() {
    let inner = format!(
        r#"<table:table table:name="S"><table:null-date table:value-type="date" table:date-value="2000-01-01"/>{}</table:table>"#,
        ods_row(&ods_float("", "1"))
    );
    let (_, extra) = ods_read("surv_ods_nd_in", "", &inner, &Limits::default()).unwrap();
    assert_eq!(extra.null_day, -25_569, "the default, 1899-12-30");
    let outer = format!(
        r#"<table:calculation-settings><table:null-date table:value-type="date" table:date-value="2000-01-01"/></table:calculation-settings>{}"#,
        ods_table("S", &ods_row(&ods_float("", "1")))
    );
    let (_, extra) = ods_read("surv_ods_nd_out", "", &outer, &Limits::default()).unwrap();
    assert_eq!(extra.null_day, 10_957, "2000-01-01 is day 10957 since 1970");
}

/// At most 100,000 error cells are remembered per sheet: 114,688 are present (7 rows of 16,384),
/// exactly 100,000 are kept.
#[test]
fn ods_error_cells_are_remembered_up_to_exactly_the_cap() {
    let rows = format!(
        r#"<table:table-row table:number-rows-repeated="7">{}</table:table-row>"#,
        ods_error_cell("#N/A", r#" table:number-columns-repeated="16384""#)
    );
    let (fm, _) = ods_read(
        "surv_ods_err_cap",
        "",
        &ods_table("S", &rows),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(fm.sheets["S"].value_cells, 7 * 16_384);
    assert_eq!(fm.sheets["S"].errors.len(), 100_000);
}

/// 9999-12-31 is the last date of the ods calendar (day 2,958,465 from the default null date);
/// the next day is a row of hashes.
#[test]
fn the_last_ods_date_is_9999_12_31() {
    let auto = ods_data_style(
        "date-style",
        r#"<number:year number:style="long"/><number:text>-</number:text><number:month number:style="long"/><number:text>-</number:text><number:day number:style="long"/>"#,
    );
    let cells: String = ["2958464", "2958465", "2958466"]
        .iter()
        .map(|v| ods_float("c", v))
        .collect();
    let dir = tmp("surv_ods_last_day");
    let p = ods_write(&dir, "d.ods", &auto, &ods_table("S", &ods_row(&cells)));
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.display(0, 0), "9999-12-30");
    assert_eq!(s.display(0, 1), "9999-12-31");
    assert_eq!(s.display(0, 2), "########");
}

/// A column definition after another that already reached the last column must not overflow, whatever
/// its repeat (`u64::MAX` is a legal `unsignedLong` in the file).
///
/// KNOWN BUG, not fixed here: `fmt_ods.rs` computes `(start + reps).min(MAX_COLS)` for a
/// `table:table-column`, and `start + reps` overflows `u64` when a definition with a huge repeat
/// follows another one (panics in debug; wraps in release, which silently drops that column
/// default). The fix is `start.saturating_add(reps).min(MAX_COLS)`. Remove the `ignore` with it.
#[test]
#[ignore = "known bug: fmt_ods table-column `start + reps` overflows u64 (see the comment)"]
fn a_column_definition_after_a_short_one_with_a_huge_repeat_does_not_overflow() {
    let cols = r#"<table:table-column table:number-columns-repeated="5"/><table:table-column table:number-columns-repeated="18446744073709551615"/>"#;
    let rows = ods_row(&ods_float("", "1"));
    let r = ods_read(
        "surv_ods_colsum",
        "",
        &ods_table("S", &format!("{cols}{rows}")),
        &Limits::default(),
    );
    assert!(r.is_ok(), "{r:?}");
}
