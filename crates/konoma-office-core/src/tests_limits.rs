//! Limits on what a crafted file can ask of the reader that are not about cells: how deep the XML
//! may nest, how many sheets a workbook may have, and the size of a formatted text. (The cell,
//! row, text and area budgets are tested with their readers.)

use super::container::Limits;
use super::fmt_ods::{self, Styles};
use super::fmt_xlsx::{self, MAX_SHEETS, MAX_XML_DEPTH};
use super::tests::{deflated, istr, load, sheet_xml, styles_xml, tmp, write, Pkg};
use super::*;

/// `n` nested `<a>` elements.
fn nested(n: usize) -> String {
    "<a>".repeat(n) + &"</a>".repeat(n)
}

fn too_deep(e: &OfficeError) -> bool {
    *e == OfficeError::TooLarge { what: "xml depth" }
}

fn load_sheet(p: &std::path::Path) -> Workbook {
    load_workbook_sheet(p, &LoadOptions::default(), 0).unwrap()
}

// ---------------------------------------------------------------------------------------------
// element depth
// ---------------------------------------------------------------------------------------------

#[test]
fn every_xlsx_part_reader_refuses_elements_nested_past_the_limit() {
    let ok = nested(MAX_XML_DEPTH - 3);
    let deep = nested(MAX_XML_DEPTH + 1);

    // workbook.xml
    let wb = |inner: &str| format!(r#"<workbook xmlns="x"><sheets>{inner}</sheets></workbook>"#);
    assert!(xlsx::parse_workbook(wb(&ok).as_bytes()).is_ok());
    assert!(too_deep(
        &xlsx::parse_workbook(wb(&deep).as_bytes()).unwrap_err()
    ));

    // the relationships (two readers: konoma's package one and the format pass's)
    let rels = |inner: &str| format!("<Relationships>{inner}</Relationships>");
    assert!(xlsx::parse_rels(rels(&ok).as_bytes()).is_ok());
    assert!(too_deep(
        &xlsx::parse_rels(rels(&deep).as_bytes()).unwrap_err()
    ));
    assert!(fmt_xlsx::parse_rels(rels(&ok).as_bytes()).is_ok());
    assert!(too_deep(
        &fmt_xlsx::parse_rels(rels(&deep).as_bytes()).unwrap_err()
    ));

    // styles.xml
    let st = |inner: &str| format!("<styleSheet><cellXfs>{inner}</cellXfs></styleSheet>");
    assert!(fmt_xlsx::parse_styles(st(&ok).as_bytes()).is_ok());
    assert!(too_deep(
        &fmt_xlsx::parse_styles(st(&deep).as_bytes()).unwrap_err()
    ));
}

#[test]
fn a_deep_xlsx_sheet_shared_strings_or_cell_content_is_refused_by_the_loader() {
    let dir = tmp("limits_deep_xlsx");
    let deep = nested(MAX_XML_DEPTH + 10);
    let ok_row = format!(r#"<row r="1">{}</row>"#, istr("A1", None, "fine"));

    // in the sheet, among the rows
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&format!("{ok_row}{deep}"), ""))
        .write(&dir, "sheet.xlsx");
    assert!(too_deep(load_sheet(&p).sheet_error.as_ref().unwrap()));

    // inside a cell, in an element the reader only skips (`read_to_end`), and in a value
    for inner in [
        format!(r#"<c r="A1"><zzz>{deep}</zzz><v>1</v></c>"#),
        format!(r#"<c r="A1" t="inlineStr"><is><t>x</t>{deep}</is></c>"#),
        format!(r#"<c r="A1" t="inlineStr"><is>{deep}</is></c>"#),
        format!(r#"<c r="A1"><f>1{deep}</f></c>"#),
        format!(r#"<c r="A1" t="str"><v>1{deep}</v></c>"#),
        format!(r#"<c r="A1" t="n"><v>{deep}</v></c>"#),
        format!(r#"<c r="A1" t="inlineStr"><v>{deep}</v></c>"#),
    ] {
        let p = Pkg::new()
            .sheet(
                "S",
                "visible",
                sheet_xml(&format!(r#"<row r="1">{inner}</row>"#), ""),
            )
            .write(&dir, "cell.xlsx");
        let wb = load_sheet(&p);
        assert!(
            wb.sheet_error.as_ref().is_some_and(too_deep),
            "{inner:.60}: {:?}",
            wb.sheet_error
        );
    }

    // in the shared strings
    let shared = format!(r#"<sst xmlns="x"><si><t>a</t></si><si>{deep}</si></sst>"#);
    let p = Pkg::new()
        .shared(shared)
        .sheet("S", "visible", sheet_xml(&ok_row, ""))
        .write(&dir, "shared.xlsx");
    let r = load_workbook_sheet(&p, &LoadOptions::default(), 0);
    assert!(r.as_ref().is_err_and(too_deep), "{:?}", r.map(|_| ()));

    // in the styles
    let st = styles_xml(&[], &[0]).replace("</cellXfs>", &format!("{deep}</cellXfs>"));
    let p = Pkg::new()
        .styles(st)
        .sheet("S", "visible", sheet_xml(&ok_row, ""))
        .write(&dir, "styles.xlsx");
    let r = load_workbook_sheet(&p, &LoadOptions::default(), 0);
    assert!(r.as_ref().is_err_and(too_deep), "{:?}", r.map(|_| ()));

    // The same sheet with a nesting under the limit reads.
    let fine = nested(MAX_XML_DEPTH - 8);
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            sheet_xml(
                &format!(r#"{ok_row}<row r="2"><c r="A2"><zzz>{fine}</zzz><v>7</v></c></row>"#),
                "",
            ),
        )
        .write(&dir, "fine.xlsx");
    let wb = load_sheet(&p);
    assert!(wb.sheet_error.is_none(), "{:?}", wb.sheet_error);
    assert_eq!(wb.sheets[0].display(1, 0), "7");
}

/// 70 million `<a>` is 210 MB of XML; the same nesting at a size a test can afford: the reader
/// stops at the limit, long before the end of the part.
#[test]
fn a_very_deep_part_is_refused_without_being_read_to_its_end() {
    let dir = tmp("limits_very_deep");
    let deep = "<a>".repeat(2_000_000);
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            sheet_xml(
                &format!(r#"<row r="1"><c r="A1"><zzz>{deep}</zzz></c></row>"#),
                "",
            ),
        )
        .write(&dir, "very.xlsx");
    let t = std::time::Instant::now();
    assert!(too_deep(load_sheet(&p).sheet_error.as_ref().unwrap()));
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

fn ods_with(content: &str, styles: Option<&str>, manifest: &str) -> Vec<u8> {
    let mut entries: Vec<(&str, Vec<u8>)> = vec![
        (
            "mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet".to_vec(),
        ),
        ("content.xml", content.as_bytes().to_vec()),
        ("META-INF/manifest.xml", manifest.as_bytes().to_vec()),
    ];
    if let Some(s) = styles {
        entries.push(("styles.xml", s.as_bytes().to_vec()));
    }
    let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, c)| (*n, c.as_slice())).collect();
    deflated(&refs)
}

const ODS_NS: &str = concat!(
    r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" "#,
    r#"xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" "#,
    r#"xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" "#,
    r#"xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" "#,
    r#"xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0""#
);

fn ods_content(inside_cell: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><office:document-content {ODS_NS}><office:body><office:spreadsheet><table:table table:name="S"><table:table-row><table:table-cell office:value-type="string"><text:p>hi{inside_cell}</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#
    )
}

fn plain_manifest() -> String {
    format!(
        r#"<manifest:manifest {ODS_NS}><manifest:file-entry manifest:full-path="content.xml"/></manifest:manifest>"#
    )
}

#[test]
fn every_ods_part_reader_refuses_elements_nested_past_the_limit() {
    let dir = tmp("limits_deep_ods");
    let deep = nested(MAX_XML_DEPTH + 10);
    let fine = nested(MAX_XML_DEPTH - 20);

    // content.xml (read by the format pass, and again by calamine only after it passed)
    let p = write(
        &dir,
        "ok.ods",
        &ods_with(&ods_content(&fine), None, &plain_manifest()),
    );
    assert!(load(&p).is_ok());
    let p = write(
        &dir,
        "deep.ods",
        &ods_with(&ods_content(&deep), None, &plain_manifest()),
    );
    assert!(
        too_deep(&load(&p).unwrap_err()),
        "{:?}",
        load(&p).map(|_| ())
    );

    // the manifest
    let manifest = format!(r#"<manifest:manifest {ODS_NS}>{deep}</manifest:manifest>"#);
    let p = write(&dir, "mf.ods", &ods_with(&ods_content(""), None, &manifest));
    assert!(
        too_deep(&load(&p).unwrap_err()),
        "{:?}",
        load(&p).map(|_| ())
    );
    assert!(too_deep(
        &fmt_ods::manifest_is_encrypted(manifest.as_bytes()).unwrap_err()
    ));

    // styles.xml and the automatic styles
    let styles = format!(
        r#"<office:document-styles {ODS_NS}><office:styles>{deep}</office:styles></office:document-styles>"#
    );
    let mut s = Styles::default();
    assert!(too_deep(
        &fmt_ods::parse_styles(styles.as_bytes(), &mut s, false).unwrap_err()
    ));
    let p = write(
        &dir,
        "st.ods",
        &ods_with(&ods_content(""), Some(&styles), &plain_manifest()),
    );
    assert!(
        too_deep(&load(&p).unwrap_err()),
        "{:?}",
        load(&p).map(|_| ())
    );
}

// ---------------------------------------------------------------------------------------------
// number of sheets
// ---------------------------------------------------------------------------------------------

#[test]
fn a_workbook_may_have_the_most_sheets_and_no_more() {
    let sheets = |n: usize| {
        let mut s = String::from(r#"<workbook xmlns="x"><sheets>"#);
        for i in 0..n {
            s += &format!(r#"<sheet name="S{i}" sheetId="{i}" r:id="rId{i}" xmlns:r="r"/>"#);
        }
        s + "</sheets></workbook>"
    };
    assert_eq!(
        xlsx::parse_workbook(sheets(MAX_SHEETS).as_bytes())
            .unwrap()
            .1
            .len(),
        MAX_SHEETS
    );
    assert_eq!(
        xlsx::parse_workbook(sheets(MAX_SHEETS + 1).as_bytes()).unwrap_err(),
        OfficeError::TooLarge { what: "sheets" }
    );
}

#[test]
fn an_ods_may_have_the_most_tables_and_no_more() {
    let dir = tmp("limits_ods_tables");
    let tables = |n: usize| {
        (0..n)
            // The same name over and over: the cap counts tables, not distinct names.
            .map(|_| r#"<table:table table:name="S"/>"#)
            .collect::<String>()
    };
    let content = |n: usize| {
        format!(
            r#"<?xml version="1.0"?><office:document-content {ODS_NS}><office:body><office:spreadsheet>{}</office:spreadsheet></office:body></office:document-content>"#,
            tables(n)
        )
    };
    let p = write(
        &dir,
        "many.ods",
        &ods_with(&content(MAX_SHEETS + 1), None, &plain_manifest()),
    );
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "sheets" }
    );
    let p = write(
        &dir,
        "max.ods",
        &ods_with(&content(MAX_SHEETS), None, &plain_manifest()),
    );
    assert!(!matches!(
        load(&p),
        Err(OfficeError::TooLarge { what: "sheets" })
    ));
}

#[test]
fn a_cancelled_ods_format_pass_gives_up_within_the_body() {
    let dir = tmp("limits_ods_cancel");
    // 6,000 rows of one cell: well past the 8,192-event check interval.
    let rows: String = (0..6_000)
        .map(|_| {
            r#"<table:table-row><table:table-cell office:value-type="float" office:value="1"/></table:table-row>"#
        })
        .collect();
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content {ODS_NS}><office:body><office:spreadsheet><table:table table:name="S">{rows}</table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    let p = write(&dir, "c.ods", &ods_with(&content, None, &plain_manifest()));
    let limits = Limits::default();
    let cancelled = workbook::Cancel::new(|| true);
    assert!(fmt_ods::read_full_cancellable(&p, &limits, Some(&cancelled)).is_err());
    let never = workbook::Cancel::new(|| false);
    assert!(fmt_ods::read_full_cancellable(&p, &limits, Some(&never)).is_ok());
    assert!(fmt_ods::read_full_cancellable(&p, &limits, None).is_ok());
}
