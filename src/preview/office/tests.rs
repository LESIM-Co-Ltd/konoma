//! Tests for the spreadsheet reading side. Synthetic packages are written with a real zip writer
//! (Deflated, so the deflate reader is exercised); real LibreOffice-written files live in
//! `samples/sample.{xlsx,ods,xls}` (see `scripts/office-samples/`, not in git).

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::container::{self, Detected, Limits};
use super::fmt_xlsx::{self, parse_a1, resolve_target, SheetFormats};
use super::workbook::{load_inner, number_text};
use super::*;
use crate::test_support::{sample_path_or_skip, unique_tmp, TmpDir};

// ---------------------------------------------------------------------------------------------
// fixture helpers
// ---------------------------------------------------------------------------------------------

pub(super) fn tmp(name: &str) -> TmpDir {
    let dir = unique_tmp(&format!("konoma_office_{name}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn zip_bytes(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut zw = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(method);
    for (name, content) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(content).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

pub(super) fn deflated(entries: &[(&str, &[u8])]) -> Vec<u8> {
    zip_bytes(entries, CompressionMethod::Deflated)
}

pub(super) fn write(dir: &TmpDir, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const RNS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// `[(name, state, rid)]`.
fn workbook_xml(date1904: bool, sheets: &[(&str, &str, &str)]) -> String {
    let mut s = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="{NS}" xmlns:r="{RNS}"><workbookPr date1904="{}"/><sheets>"#,
        if date1904 { "1" } else { "0" }
    );
    for (i, (name, state, rid)) in sheets.iter().enumerate() {
        s += &format!(
            r#"<sheet name="{name}" sheetId="{}" state="{state}" r:id="{rid}"/>"#,
            i + 1
        );
    }
    s + "</sheets></workbook>"
}

fn rels_xml(rels: &[(&str, &str)]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, target) in rels {
        s += &format!(r#"<Relationship Id="{id}" Type="x/worksheet" Target="{target}"/>"#);
    }
    s + "</Relationships>"
}

/// `numfmts`: `(id, code)`; `xfs`: numFmtId of each cellXfs entry.
fn styles_xml(numfmts: &[(u32, &str)], xfs: &[u32]) -> String {
    let mut s = format!(r#"<?xml version="1.0"?><styleSheet xmlns="{NS}">"#);
    if !numfmts.is_empty() {
        s += &format!(r#"<numFmts count="{}">"#, numfmts.len());
        for (id, code) in numfmts {
            s += &format!(r#"<numFmt numFmtId="{id}" formatCode="{code}"/>"#);
        }
        s += "</numFmts>";
    }
    // A decoy: xfs under cellStyleXfs must not be counted.
    s += r#"<cellStyleXfs count="1"><xf numFmtId="49"/></cellStyleXfs>"#;
    s += &format!(r#"<cellXfs count="{}">"#, xfs.len());
    for id in xfs {
        s += &format!(r#"<xf numFmtId="{id}" xfId="0"/>"#);
    }
    s + "</cellXfs></styleSheet>"
}

fn sheet_xml(rows: &str, after: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS}"><sheetData>{rows}</sheetData>{after}</worksheet>"#
    )
}

fn istr(r: &str, s: Option<u32>, text: &str) -> String {
    let style = s.map(|s| format!(r#" s="{s}""#)).unwrap_or_default();
    format!(r#"<c r="{r}"{style} t="inlineStr"><is><t>{text}</t></is></c>"#)
}

fn num(r: &str, s: Option<u32>, v: &str) -> String {
    let style = s.map(|s| format!(r#" s="{s}""#)).unwrap_or_default();
    format!(r#"<c r="{r}"{style}><v>{v}</v></c>"#)
}

struct SheetDef {
    name: String,
    state: String,
    rid: String,
    /// Path of the part inside the zip.
    part: String,
    /// `Target` written in the relationships file.
    target: String,
    xml: String,
}

struct Pkg {
    date1904: bool,
    sheets: Vec<SheetDef>,
    styles: Option<String>,
}

impl Pkg {
    fn new() -> Pkg {
        Pkg {
            date1904: false,
            sheets: Vec::new(),
            styles: None,
        }
    }
    fn sheet(mut self, name: &str, state: &str, xml: String) -> Pkg {
        let n = self.sheets.len() + 1;
        self.sheets.push(SheetDef {
            name: name.into(),
            state: state.into(),
            rid: format!("rId{n}"),
            part: format!("xl/worksheets/sheet{n}.xml"),
            target: format!("worksheets/sheet{n}.xml"),
            xml,
        });
        self
    }
    fn styles(mut self, xml: String) -> Pkg {
        self.styles = Some(xml);
        self
    }
    fn date1904(mut self) -> Pkg {
        self.date1904 = true;
        self
    }
    /// Use `/xl/worksheets/sheetN.xml` (absolute) targets in the rels.
    fn absolute_targets(mut self) -> Pkg {
        for s in &mut self.sheets {
            s.target = format!("/{}", s.part);
        }
        self
    }
    fn bytes(&self) -> Vec<u8> {
        let wb = workbook_xml(
            self.date1904,
            &self
                .sheets
                .iter()
                .map(|s| (s.name.as_str(), s.state.as_str(), s.rid.as_str()))
                .collect::<Vec<_>>(),
        );
        let rels = rels_xml(
            &self
                .sheets
                .iter()
                .map(|s| (s.rid.as_str(), s.target.as_str()))
                .collect::<Vec<_>>(),
        );
        let root_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
        let mut entries: Vec<(String, Vec<u8>)> = vec![
            ("_rels/.rels".into(), root_rels.as_bytes().to_vec()),
            ("xl/workbook.xml".into(), wb.into_bytes()),
            ("xl/_rels/workbook.xml.rels".into(), rels.into_bytes()),
        ];
        if let Some(st) = &self.styles {
            entries.push(("xl/styles.xml".into(), st.clone().into_bytes()));
        }
        for s in &self.sheets {
            entries.push((s.part.clone(), s.xml.clone().into_bytes()));
        }
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_slice()))
            .collect();
        deflated(&refs)
    }
    fn write(&self, dir: &TmpDir, name: &str) -> PathBuf {
        write(dir, name, &self.bytes())
    }
}

pub(super) fn load(path: &Path) -> Result<Workbook, OfficeError> {
    load_workbook(path, &LoadOptions::default())
}

pub(super) fn load_with(path: &Path, limits: Limits) -> Result<Workbook, OfficeError> {
    load_workbook(
        path,
        &LoadOptions {
            locale: Locale::En,
            limits,
        },
    )
}

pub(super) fn small_limits() -> Limits {
    Limits {
        max_file_bytes: 1 << 20,
        max_entries: 50,
        max_part_bytes: 1 << 20,
        max_total_bytes: 4 << 20,
        max_grid_cells: 1000,
        max_rows: 100,
        max_cols: 16,
        max_dense_cells: 10_000,
    }
}

// ---------------------------------------------------------------------------------------------
// small pure functions
// ---------------------------------------------------------------------------------------------

#[test]
fn sheet_kind_from_extension_is_case_insensitive_and_complete() {
    for (e, k) in [
        ("xlsx", SheetKind::Xlsx),
        ("XLSX", SheetKind::Xlsx),
        ("xlsm", SheetKind::Xlsm),
        ("xltx", SheetKind::Xltx),
        ("xltm", SheetKind::Xltm),
        ("xlsb", SheetKind::Xlsb),
        ("Xls", SheetKind::Xls),
        ("ods", SheetKind::Ods),
    ] {
        assert_eq!(SheetKind::from_ext(e), Some(k), "{e}");
    }
    for e in ["", "doc", "docx", "csv", "xlsx2", "xl", "ods "] {
        assert_eq!(SheetKind::from_ext(e), None, "{e:?}");
    }
    assert_eq!(
        SheetKind::from_path(Path::new("/a/b/Report.XLSM")),
        Some(SheetKind::Xlsm)
    );
    assert_eq!(SheetKind::from_path(Path::new("/a/b/noext")), None);
    assert_eq!(SheetKind::from_path(Path::new("/a/.xlsx")), None);
}

#[test]
fn parse_a1_table() {
    for (s, want) in [
        ("A1", Some((0, 0))),
        ("B3", Some((2, 1))),
        ("Z1", Some((0, 25))),
        ("AA1", Some((0, 26))),
        ("XFD1048576", Some((1_048_575, 16_383))),
        ("a1", Some((0, 0))),
        (" C5 ", Some((4, 2))),
        ("", None),
        ("A", None),
        ("1", None),
        ("A0", None),
        ("1A", None),
        ("A1B", None),
        ("$A$1", None),
        ("A-1", None),
        ("AAAAAAAAAAAA1", None), // column overflows u32
        ("A99999999999", None),  // row overflows u32
    ] {
        assert_eq!(parse_a1(s), want, "{s:?}");
    }
}

#[test]
fn resolve_target_relative_absolute_and_dotdot() {
    for (base, target, want) in [
        ("xl", "worksheets/sheet1.xml", "xl/worksheets/sheet1.xml"),
        (
            "xl",
            "/xl/worksheets/sheet1.xml",
            "xl/worksheets/sheet1.xml",
        ),
        ("xl", "./worksheets/sheet1.xml", "xl/worksheets/sheet1.xml"),
        ("xl", "../other/s.xml", "other/s.xml"),
        ("xl", "worksheets//s.xml", "xl/worksheets/s.xml"),
        ("xl", "../../../x.xml", "x.xml"), // cannot climb above the root
        ("xl", "/s.xml", "s.xml"),
    ] {
        assert_eq!(resolve_target(base, target), want, "{target}");
    }
}

#[test]
fn number_text_is_shortest_roundtrip() {
    assert_eq!(number_text(1.0), "1");
    assert_eq!(number_text(-3.0), "-3");
    assert_eq!(number_text(0.1), "0.1");
    assert_eq!(number_text(1234567.891), "1234567.891");
    assert_eq!(number_text(1e300), "1e300");
    assert_eq!(number_text(1e-9), "1e-9");
    assert_eq!(number_text(f64::NAN), "#NUM!");
    assert_eq!(number_text(f64::INFINITY), "#DIV/0!");
    assert_eq!(number_text(123456789012345.0), "123456789012345");
}

fn shown(v: CellValue, fmt: &NumFmtRef, date1904: bool, locale: Locale) -> String {
    display_text(&v, fmt, &DisplayCtx { date1904, locale })
}

fn custom(code: &str) -> NumFmtRef {
    NumFmtRef::Custom(code.into())
}

#[test]
fn display_text_with_general_is_plain_for_every_value_kind() {
    let g = NumFmtRef::General;
    let t = |v: CellValue| shown(v, &g, false, Locale::Ja);
    assert_eq!(t(CellValue::Empty), "");
    assert_eq!(t(CellValue::Int(-7)), "-7");
    assert_eq!(t(CellValue::Number(0.5)), "0.5");
    assert_eq!(t(CellValue::Number(0.1 + 0.2)), "0.3");
    assert_eq!(t(CellValue::Number(12345678901234.0)), "1.23457E+13");
    assert_eq!(t(CellValue::Text("あ".into())), "あ");
    assert_eq!(t(CellValue::Bool(true)), "TRUE");
    assert_eq!(t(CellValue::Bool(false)), "FALSE");
    assert_eq!(t(CellValue::Error("#N/A")), "#N/A");
    // A date with General is its serial number, as Excel shows it.
    assert_eq!(
        t(CellValue::DateTime {
            serial: 45292.0,
            duration: false
        }),
        "45292"
    );
    assert_eq!(t(CellValue::DateTimeIso("2026-10-02".into())), "46297");
    // An ISO string that is no date is shown as written.
    assert_eq!(t(CellValue::DateTimeIso("not a date".into())), "not a date");
}

#[test]
fn display_text_applies_builtin_and_custom_formats() {
    let en = Locale::En;
    let ja = Locale::Ja;
    let n = |v: f64, f: &NumFmtRef, l| shown(CellValue::Number(v), f, false, l);
    assert_eq!(n(1234.5, &NumFmtRef::Builtin(4), en), "1,234.50");
    assert_eq!(n(0.256, &NumFmtRef::Builtin(10), en), "25.60%");
    assert_eq!(n(46297.0, &NumFmtRef::Builtin(14), en), "10/2/2026");
    assert_eq!(n(46297.0, &NumFmtRef::Builtin(14), ja), "2026/10/2");
    assert_eq!(n(1500.0, &NumFmtRef::Builtin(5), en), "$1,500 ");
    assert_eq!(n(1500.0, &NumFmtRef::Builtin(5), ja), "¥1,500");
    // A built-in id that has no code is General (23..=26, 59+).
    assert_eq!(n(0.5, &NumFmtRef::Builtin(23), en), "0.5");
    assert_eq!(n(0.5, &NumFmtRef::Builtin(999), en), "0.5");
    assert_eq!(n(1234.5, &custom("[$¥-411]#,##0"), en), "¥1,235");
    assert_eq!(n(0.256, &custom("0.0%"), en), "25.6%");
    assert_eq!(n(-3.0, &custom("0;[Red]-0"), en), "-3");
    // Ints go through the same engine.
    assert_eq!(
        shown(CellValue::Int(1234567), &custom("#,##0"), false, en),
        "1,234,567"
    );
    // Dates and durations are serials.
    let d = |serial: f64, duration: bool, f: &NumFmtRef| {
        shown(CellValue::DateTime { serial, duration }, f, false, en)
    };
    assert_eq!(d(46297.0, false, &custom("yyyy-mm-dd")), "2026-10-02");
    assert_eq!(
        d(46297.0, false, &custom("ggge\\年m\\月d\\日")),
        "令和8年10月2日"
    );
    assert_eq!(d(1.5, true, &custom("[h]:mm:ss")), "36:00:00");
    assert_eq!(d(0.5, false, &custom("h:mm AM/PM")), "12:00 PM");
    // The 1904 system shifts the same serial.
    assert_eq!(
        shown(
            CellValue::DateTime {
                serial: 0.0,
                duration: false
            },
            &custom("yyyy-mm-dd"),
            true,
            en
        ),
        "1904-01-01"
    );
    // Text goes through the text section; numbers formats leave text alone.
    assert_eq!(
        shown(CellValue::Text("x".into()), &custom("@\"!\""), false, en),
        "x!"
    );
    assert_eq!(
        shown(CellValue::Text("x".into()), &custom("0.00"), false, en),
        "x"
    );
    // Booleans and errors ignore the number format.
    assert_eq!(
        shown(CellValue::Bool(true), &custom("0.00"), false, en),
        "TRUE"
    );
    assert_eq!(
        shown(CellValue::Error("#REF!"), &custom("0.00"), false, en),
        "#REF!"
    );
}

#[test]
fn display_text_converts_iso_dates_and_durations_from_ods() {
    let iso = |s: &str, f: &str| {
        shown(
            CellValue::DateTimeIso(s.into()),
            &custom(f),
            false,
            Locale::En,
        )
    };
    assert_eq!(iso("2026-10-02", "yyyy/mm/dd"), "2026/10/02");
    assert_eq!(
        iso("2026-10-02T13:05:09", "yyyy-mm-dd hh:mm:ss"),
        "2026-10-02 13:05:09"
    );
    assert_eq!(iso("PT13H05M09S", "hh:mm:ss"), "13:05:09");
    assert_eq!(iso("PT25H30M00S", "[h]:mm"), "25:30");
    assert_eq!(iso("P1DT2H", "[h]:mm"), "26:00");
    assert_eq!(iso("garbage", "yyyy"), "garbage");
}

#[test]
fn display_text_never_panics_on_extreme_numbers() {
    let fmts = [
        NumFmtRef::General,
        NumFmtRef::Builtin(14),
        NumFmtRef::Builtin(11),
        custom("0.00"),
        custom("yyyy-mm-dd"),
        custom("[h]:mm:ss"),
        custom("0.0%"),
        custom("#,##0"),
        custom("garbage ;;; [[["),
    ];
    for v in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        -0.0,
        1e300,
        -1e-300,
        2958466.0,
        2958467.0,
        -1.0,
    ] {
        for f in &fmts {
            for date1904 in [false, true] {
                let _ = shown(CellValue::Number(v), f, date1904, Locale::En);
                let _ = shown(
                    CellValue::DateTime {
                        serial: v,
                        duration: false,
                    },
                    f,
                    date1904,
                    Locale::Ja,
                );
            }
        }
    }
    let _ = shown(
        CellValue::Int(i64::MIN),
        &custom("#,##0"),
        false,
        Locale::En,
    );
    let _ = shown(
        CellValue::Int(i64::MAX),
        &NumFmtRef::General,
        false,
        Locale::En,
    );
}

#[test]
fn cell_and_value_sizes_match_the_documented_memory_budget() {
    // The module doc estimates ~48 bytes per non-empty cell; a regression here silently
    // multiplies the worst-case memory of a 4M-cell sheet.
    assert!(std::mem::size_of::<CellValue>() <= 24);
    assert!(std::mem::size_of::<Cell>() <= 48);
}

#[test]
fn office_error_display_never_panics_and_is_distinct() {
    let all = [
        OfficeError::Encrypted,
        OfficeError::TooLarge { what: "file" },
        OfficeError::Corrupt("x".into()),
        OfficeError::Unsupported,
        OfficeError::Io("y".into()),
    ];
    let texts: Vec<String> = all.iter().map(|e| e.to_string()).collect();
    for (i, a) in texts.iter().enumerate() {
        assert!(!a.is_empty());
        for b in &texts[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// format pass: workbook.xml / rels / styles / sheet
// ---------------------------------------------------------------------------------------------

#[test]
fn parse_workbook_date_system_and_sheet_ids() {
    let p = |xml: &str| fmt_xlsx::parse_workbook(xml.as_bytes()).unwrap();
    for (attr, want) in [
        (r#"date1904="1""#, true),
        (r#"date1904="true""#, true),
        (r#"date1904="0""#, false),
        (r#"date1904="false""#, false),
        ("", false),
    ] {
        let xml = format!(r#"<workbook xmlns:r="x"><workbookPr {attr}/></workbook>"#);
        assert_eq!(p(&xml).0, want, "{attr}");
    }
    // No workbookPr at all.
    assert!(!p("<workbook/>").0);
    // r:id under any prefix; a bare `id` attribute is not a relationship id; name entities.
    let (_, sheets) = p(r#"<workbook xmlns:rel="x"><sheets>
        <sheet name="A &amp; B" sheetId="1" rel:id="rId9"/>
        <sheet name="NoRid" sheetId="2" id="5"/>
        <sheet sheetId="3" rel:id="rId3"/>
        </sheets></workbook>"#);
    assert_eq!(sheets.len(), 1);
    assert_eq!(sheets[0].name, "A & B");
    assert_eq!(sheets[0].rid, "rId9");
}

#[test]
fn parse_rels_skips_external_and_resolves_both_path_styles() {
    let m = fmt_xlsx::parse_rels(
        br#"<Relationships>
        <Relationship Id="rId1" Target="worksheets/sheet1.xml"/>
        <Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/>
        <Relationship Id="rId3" Target="https://example.com/x" TargetMode="External"/>
        <Relationship Id="rId4" Target="../xl/worksheets/sheet4.xml"/>
        </Relationships>"#
            .as_slice(),
    )
    .unwrap();
    assert_eq!(m["rId1"], "xl/worksheets/sheet1.xml");
    assert_eq!(m["rId2"], "xl/worksheets/sheet2.xml");
    assert_eq!(m["rId4"], "xl/worksheets/sheet4.xml");
    assert!(!m.contains_key("rId3"));
}

#[test]
fn parse_styles_resolves_custom_builtin_and_unknown_ids() {
    let xml = styles_xml(
        &[
            (164, "yyyy/m/d"),
            (165, "0.00&quot;x&quot;"),
            (14, "dd.mm.yyyy"),
        ],
        &[0, 164, 165, 14, 9, 164, 200, 49],
    );
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    // xf 0 -> General; the cellStyleXfs decoy (49) is not an xf of cellXfs.
    assert_eq!(st.xf_to_format.len(), 8);
    let fmt = |xf: usize| &st.formats[st.xf_to_format[xf] as usize];
    assert_eq!(*fmt(0), NumFmtRef::General);
    assert_eq!(*fmt(1), NumFmtRef::Custom("yyyy/m/d".into()));
    assert_eq!(
        *fmt(2),
        NumFmtRef::Custom("0.00\"x\"".into()),
        "entities unescaped"
    );
    // A custom definition of a built-in number wins.
    assert_eq!(*fmt(3), NumFmtRef::Custom("dd.mm.yyyy".into()));
    assert_eq!(*fmt(4), NumFmtRef::Builtin(9));
    // Same code -> same table entry (deduplicated).
    assert_eq!(st.xf_to_format[1], st.xf_to_format[5]);
    // An id >= 164 that the file never defines is General, not a dangling reference.
    assert_eq!(*fmt(6), NumFmtRef::General);
    assert_eq!(*fmt(7), NumFmtRef::Builtin(49));
    assert_eq!(st.formats[0], NumFmtRef::General);
    assert!(st.formats.len() <= 6, "deduplicated: {:?}", st.formats);
}

#[test]
fn parse_styles_handles_xf_with_children_and_missing_numfmtid() {
    let xml = format!(
        r#"<styleSheet xmlns="{NS}"><cellXfs count="3">
        <xf numFmtId="9" xfId="0"><alignment horizontal="center"/><protection locked="1"/></xf>
        <xf xfId="0"/>
        <xf numFmtId="abc"/>
        </cellXfs></styleSheet>"#
    );
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(st.xf_to_format.len(), 3);
    assert_eq!(
        st.formats[st.xf_to_format[0] as usize],
        NumFmtRef::Builtin(9)
    );
    assert_eq!(st.formats[st.xf_to_format[1] as usize], NumFmtRef::General);
    assert_eq!(st.formats[st.xf_to_format[2] as usize], NumFmtRef::General);
}

#[test]
fn parse_styles_never_expands_dtd_entities() {
    // A billion-laughs shaped document: the entity must not be expanded (and must not be fatal).
    let xml = format!(
        r#"<?xml version="1.0"?><!DOCTYPE s [<!ENTITY a "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">]>
        <styleSheet xmlns="{NS}"><numFmts count="1"><numFmt numFmtId="164" formatCode="&c;"/></numFmts>
        <cellXfs count="1"><xf numFmtId="164"/></cellXfs></styleSheet>"#
    );
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    match &st.formats[st.xf_to_format[0] as usize] {
        NumFmtRef::Custom(c) => assert!(c.len() < 16, "expanded to {} bytes", c.len()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_styles_rejects_broken_xml_with_corrupt_not_panic() {
    let r =
        fmt_xlsx::parse_styles(b"<styleSheet><cellXfs><xf numFmtId=\"1\"></cellXfs>".as_slice());
    assert!(matches!(r, Err(OfficeError::Corrupt(_))), "{r:?}");
}

fn sheet_formats(rows: &str, xf_to_format: &[u16]) -> SheetFormats {
    let xml = sheet_xml(rows, "");
    fmt_xlsx::parse_sheet(xml.as_bytes(), xf_to_format, &Limits::default()).unwrap()
}

#[test]
fn parse_sheet_xf_zero_and_missing_s_are_general() {
    // xf 1 -> format 1; a cell without `s` uses xf 0 -> format 0 (General, not recorded).
    let sf = sheet_formats(
        &format!(
            r#"<row r="1">{}{}{}</row>"#,
            num("A1", None, "1"),
            num("B1", Some(1), "2"),
            num("C1", Some(0), "3")
        ),
        &[0, 1],
    );
    assert_eq!(sf.cells, vec![(0, 1, 1)]);
    assert_eq!(sf.value_cells, 3);
    assert_eq!(sf.bbox, Some((0, 0, 0, 2)));
    assert_eq!(sf.format_at(0, 1), 1);
    assert_eq!(sf.format_at(0, 0), 0);
    assert_eq!(sf.format_at(5, 5), 0);
    // A file whose xf 0 is itself a date format applies it to cells with no `s`.
    let sf = sheet_formats(
        &format!(r#"<row r="1">{}</row>"#, num("A1", None, "1")),
        &[1],
    );
    assert_eq!(sf.format_at(0, 0), 1);
}

#[test]
fn parse_sheet_cells_without_r_continue_in_their_row_and_rows_without_r_follow() {
    // Row 1 has `r`; its cells do not. Row 2 has no `r` (-> 2). Row 5 jumps; the row after it has none (-> 6).
    let rows = r#"
        <row r="1"><c s="1"><v>1</v></c><c s="1"><v>2</v></c><c r="D1" s="1"><v>3</v></c><c s="1"><v>4</v></c></row>
        <row><c s="1"><v>5</v></c></row>
        <row r="5"><c s="1"><v>6</v></c></row>
        <row><c s="1"><v>7</v></c></row>
        <row r="9"/>
        <row><c s="1"><v>8</v></c></row>"#;
    let sf = sheet_formats(rows, &[0, 1]);
    assert_eq!(
        sf.cells,
        vec![
            (0, 0, 1),
            (0, 1, 1),
            (0, 3, 1),
            (0, 4, 1),
            (1, 0, 1),
            (4, 0, 1),
            (5, 0, 1),
            (9, 0, 1)
        ]
    );
}

#[test]
fn parse_sheet_counts_only_value_bearing_cells() {
    let rows = r#"<row r="1">
        <c r="A1" s="1"/>
        <c r="B1" s="1"></c>
        <c r="C1" s="1"><f>1+1</f></c>
        <c r="D1" s="1"><v>1</v></c>
        <c r="E1" s="1" t="inlineStr"><is><t>x</t></is></c>
        </row>"#;
    let sf = sheet_formats(rows, &[0, 1]);
    assert_eq!(sf.value_cells, 2);
    assert_eq!(sf.cells, vec![(0, 3, 1), (0, 4, 1)]);
    assert_eq!(sf.bbox, Some((0, 3, 0, 4)));
}

#[test]
fn parse_sheet_drops_columns_beyond_the_maximum_and_survives_bad_refs() {
    let rows = r#"<row r="1">
        <c r="XFD1" s="1"><v>1</v></c>
        <c r="XFE1" s="1"><v>1</v></c>
        <c r="ZZZZ1" s="1"><v>1</v></c>
        <c r="??" s="1"><v>1</v></c>
        </row>"#;
    let sf = sheet_formats(rows, &[0, 1]);
    // XFD (index 16,383) is kept; XFE and ZZZZ are past the maximum; "??" falls back to the
    // sequence position (after ZZZZ, so also past the maximum).
    assert!(sf.cells.contains(&(0, 16_383, 1)));
    assert_eq!(sf.cells.len(), 1, "{:?}", sf.cells);
    assert!(sf.bbox.unwrap().3 <= 16_383);
}

#[test]
fn parse_sheet_sorts_out_of_order_cells_so_lookup_works() {
    let rows = format!(
        r#"<row r="3">{}</row><row r="1">{}{}</row>"#,
        num("A3", Some(1), "1"),
        num("C1", Some(1), "1"),
        num("A1", Some(1), "1"),
    );
    let sf = sheet_formats(&rows, &[0, 1]);
    assert_eq!(sf.cells, vec![(0, 0, 1), (0, 2, 1), (2, 0, 1)]);
    assert_eq!(sf.format_at(0, 2), 1);
    assert_eq!(sf.format_at(2, 0), 1);
    assert_eq!(sf.bbox, Some((0, 0, 2, 2)));
    assert_eq!(sf.bbox_area(), 9);
}

#[test]
fn parse_sheet_value_cell_budget_is_enforced_while_streaming() {
    let mut rows = String::new();
    for r in 1..=20 {
        rows += &format!(
            r#"<row r="{r}">{}{}</row>"#,
            num(&format!("A{r}"), None, "1"),
            num(&format!("B{r}"), None, "1")
        );
    }
    let xml = sheet_xml(&rows, "");
    let limits = Limits {
        max_dense_cells: 10,
        ..Limits::default()
    };
    let r = fmt_xlsx::parse_sheet(xml.as_bytes(), &[0], &limits);
    assert_eq!(
        r,
        Err(OfficeError::TooLarge {
            what: "sheet cells"
        })
    );
}

#[test]
fn parse_sheet_empty_and_no_cells() {
    let sf = sheet_formats("", &[0]);
    assert_eq!(sf, SheetFormats::default());
    assert_eq!(sf.bbox_area(), 0);
}

// ---------------------------------------------------------------------------------------------
// loading synthetic workbooks end to end
// ---------------------------------------------------------------------------------------------

fn basic_pkg() -> Pkg {
    let rows = format!(
        r#"<row r="1">{}{}{}</row><row r="2">{}{}{}</row>"#,
        istr("A1", None, "Name"),
        istr("B1", None, "日本語"),
        istr("C1", None, "Date"),
        istr("A2", None, "x &amp; y"),
        num("B2", Some(1), "0.256"),
        num("C2", Some(2), "46297"),
    );
    Pkg::new()
        .styles(styles_xml(
            &[(164, "0.0%"), (165, "yyyy-mm-dd")],
            &[0, 164, 165],
        ))
        .sheet("Sheet1", "visible", sheet_xml(&rows, ""))
}

#[test]
fn loads_values_types_and_format_references() {
    let dir = tmp("basic");
    let wb = load(&basic_pkg().write(&dir, "a.xlsx")).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    assert_eq!(wb.hidden_sheets, 0);
    assert!(!wb.date1904);
    let s = &wb.sheets[0];
    assert_eq!(s.name, "Sheet1");
    assert_eq!((s.nrows, s.ncols), (2, 3));
    assert!(!s.rows_truncated && !s.cols_truncated);
    assert_eq!(s.display(0, 1), "日本語");
    assert_eq!(s.display(1, 0), "x & y", "entities in text are decoded");
    let pct = s.cell(1, 1).unwrap();
    assert_eq!(pct.cell_type(), CellType::Number);
    assert_eq!(pct.raw_text(), "0.256");
    assert_eq!(*wb.format_of(pct), NumFmtRef::Custom("0.0%".into()));
    let date = s.cell(1, 2).unwrap();
    assert_eq!(
        date.cell_type(),
        CellType::DateTime,
        "calamine sees the date format"
    );
    assert_eq!(date.raw_text(), "46297");
    assert_eq!(*wb.format_of(date), NumFmtRef::Custom("yyyy-mm-dd".into()));
    let text = s.cell(0, 0).unwrap();
    assert_eq!(text.cell_type(), CellType::Text);
    assert_eq!(*wb.format_of(text), NumFmtRef::General);
    // Absent cells.
    assert!(s.cell(5, 5).is_none());
    assert_eq!(s.display(5, 5), "");
}

#[test]
fn builtin_number_format_ids_are_carried_as_builtin() {
    let rows = format!(
        r#"<row r="1">{}{}{}</row>"#,
        num("A1", Some(1), "0.5"),
        num("B1", Some(2), "46297"),
        num("C1", Some(3), "1234.5")
    );
    let dir = tmp("builtin");
    let p = Pkg::new()
        .styles(styles_xml(&[], &[0, 9, 14, 4]))
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "b.xlsx");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(*wb.format_of(s.cell(0, 0).unwrap()), NumFmtRef::Builtin(9));
    assert_eq!(*wb.format_of(s.cell(0, 1).unwrap()), NumFmtRef::Builtin(14));
    assert_eq!(*wb.format_of(s.cell(0, 2).unwrap()), NumFmtRef::Builtin(4));
    assert_eq!(s.cell(0, 1).unwrap().cell_type(), CellType::DateTime);
}

#[test]
fn works_without_a_styles_part() {
    let dir = tmp("nostyles");
    let rows = format!(r#"<row r="1">{}</row>"#, num("A1", Some(3), "1"));
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "n.xlsx");
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets[0].display(0, 0), "1");
    assert_eq!(wb.formats, vec![NumFmtRef::General]);
}

#[test]
fn used_range_not_starting_at_a1_is_placed_at_its_a1_address() {
    let rows = format!(
        r#"<row r="5">{}{}</row><row r="6">{}</row>"#,
        istr("C5", None, "top-left"),
        num("D5", Some(1), "1"),
        istr("C6", None, "below")
    );
    let dir = tmp("offset");
    let p = Pkg::new()
        .styles(styles_xml(&[(164, "0.00")], &[0, 164]))
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "o.xlsx");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(
        (s.nrows, s.ncols),
        (6, 4),
        "grid runs from A1 to the last used cell"
    );
    assert_eq!(s.display(4, 2), "top-left", "C5 is row 4, col 2 (0-based)");
    assert_eq!(s.display(5, 2), "below");
    assert_eq!(
        *wb.format_of(s.cell(4, 3).unwrap()),
        NumFmtRef::Custom("0.00".into())
    );
    assert!(s.cell(0, 0).is_none());
    assert!(s.row_cells(0).is_empty());
    assert_eq!(s.row_cells(4).len(), 2);
}

#[test]
fn rels_with_absolute_targets_resolve() {
    let dir = tmp("abs");
    let rows = format!(r#"<row r="1">{}</row>"#, istr("A1", None, "abs"));
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .absolute_targets()
        .styles(styles_xml(&[(164, "0.0")], &[0, 164]))
        .write(&dir, "abs.xlsx");
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets[0].display(0, 0), "abs");
    // The format pass resolved the absolute target too (it saw the sheet).
    let fm = fmt_xlsx::read(&p, &Limits::default()).unwrap();
    assert!(fm.sheets.contains_key("S"));
}

#[test]
fn date1904_flag_is_reported() {
    let dir = tmp("d1904");
    let rows = format!(r#"<row r="1">{}</row>"#, num("A1", Some(1), "1"));
    let mk = |d1904: bool, name: &str| {
        let mut p = Pkg::new()
            .styles(styles_xml(&[(164, "yyyy-mm-dd")], &[0, 164]))
            .sheet("S", "visible", sheet_xml(&rows, ""));
        if d1904 {
            p = p.date1904();
        }
        p.write(&dir, name)
    };
    assert!(load(&mk(true, "a.xlsx")).unwrap().date1904);
    assert!(!load(&mk(false, "b.xlsx")).unwrap().date1904);
}

#[test]
fn hidden_and_very_hidden_sheets_are_counted_not_listed_and_order_is_kept() {
    let dir = tmp("hidden");
    let one = |t: &str| sheet_xml(&format!(r#"<row r="1">{}</row>"#, istr("A1", None, t)), "");
    let p = Pkg::new()
        .sheet("First", "visible", one("1"))
        .sheet("Secret", "hidden", one("2"))
        .sheet("Deep", "veryHidden", one("3"))
        .sheet("Last", "visible", one("4"))
        .write(&dir, "h.xlsx");
    let wb = load(&p).unwrap();
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["First", "Last"]);
    assert_eq!(wb.hidden_sheets, 2);
    assert_eq!(wb.sheets[1].display(0, 0), "4");
}

#[test]
fn all_sheets_hidden_gives_an_empty_list_and_the_count() {
    let dir = tmp("allhidden");
    let p = Pkg::new()
        .sheet("A", "hidden", sheet_xml("", ""))
        .sheet("B", "veryHidden", sheet_xml("", ""))
        .write(&dir, "x.xlsx");
    let wb = load(&p).unwrap();
    assert!(wb.sheets.is_empty());
    assert_eq!(wb.hidden_sheets, 2);
}

#[test]
fn empty_sheet_is_an_empty_grid() {
    let dir = tmp("emptysheet");
    let p = Pkg::new()
        .sheet("E", "visible", sheet_xml("", ""))
        .write(&dir, "e.xlsx");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!((s.nrows, s.ncols), (0, 0));
    assert!(!s.rows_truncated);
    assert!(s.cell(0, 0).is_none());
    assert!(s.merges.is_empty());
}

#[test]
fn merged_ranges_keep_the_value_at_the_top_left_only() {
    let dir = tmp("merge");
    let rows = format!(
        r#"<row r="2">{}<c r="D2"/><c r="C3"/></row><row r="3">{}</row>"#,
        istr("C2", None, "merged"),
        num("E3", None, "5")
    );
    let after =
        r#"<mergeCells count="2"><mergeCell ref="C2:D3"/><mergeCell ref="A1:B1"/></mergeCells>"#;
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, after))
        .write(&dir, "m.xlsx");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.merges.len(), 2);
    assert!(s.merges.contains(&MergeRange {
        row0: 1,
        col0: 2,
        row1: 2,
        col1: 3
    }));
    assert!(s.merges.contains(&MergeRange {
        row0: 0,
        col0: 0,
        row1: 0,
        col1: 1
    }));
    assert_eq!(s.display(1, 2), "merged");
    assert_eq!(s.display(1, 3), "");
    assert_eq!(s.display(2, 2), "");
    assert_eq!(
        s.merge_at(2, 3),
        Some(MergeRange {
            row0: 1,
            col0: 2,
            row1: 2,
            col1: 3
        })
    );
    assert_eq!(s.merge_at(4, 4), None);
}

#[test]
fn formulas_are_kept_per_cell_without_the_equals_sign() {
    let dir = tmp("formula");
    let rows = format!(
        r#"<row r="1">{}{}<c r="C1" t="str"><f>A1&amp;"x"</f><v>1x</v></c></row>"#,
        num("A1", None, "1"),
        r#"<c r="B1"><f>SUM(A1:A1)</f><v>1</v></c>"#
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "f.xlsx");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.formula(0, 1), Some("SUM(A1:A1)"));
    assert_eq!(s.formula(0, 2), Some("A1&\"x\""));
    assert_eq!(s.formula(0, 0), None);
    assert_eq!(s.display(0, 2), "1x");
}

#[test]
fn error_and_bool_values_have_their_types() {
    let dir = tmp("errbool");
    let rows = r#"<row r="1"><c r="A1" t="e"><v>#DIV/0!</v></c><c r="B1" t="b"><v>1</v></c><c r="C1" t="b"><v>0</v></c></row>"#;
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(rows, ""))
        .write(&dir, "eb.xlsx");
    let s = &load(&p).unwrap().sheets[0];
    assert_eq!(s.cell(0, 0).unwrap().cell_type(), CellType::Error);
    assert_eq!(s.display(0, 0), "#DIV/0!");
    assert_eq!(s.cell(0, 1).unwrap().cell_type(), CellType::Bool);
    assert_eq!(s.display(0, 1), "TRUE");
    assert_eq!(s.display(0, 2), "FALSE");
}

#[test]
fn rows_are_cut_at_the_row_cap_and_flagged() {
    let dir = tmp("rowcap");
    let mut rows = String::new();
    for r in 1..=30 {
        rows += &format!(
            r#"<row r="{r}">{}</row>"#,
            num(&format!("A{r}"), None, &r.to_string())
        );
    }
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "r.xlsx");
    let limits = Limits {
        max_rows: 10,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!(s.nrows, 10);
    assert!(s.rows_truncated);
    assert!(!s.cols_truncated);
    assert_eq!(s.display(9, 0), "10");
    assert!(s.cell(10, 0).is_none());
    // Exactly at the cap is not truncated.
    let limits = Limits {
        max_rows: 30,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!(s.nrows, 30);
    assert!(!s.rows_truncated);
}

#[test]
fn columns_are_cut_at_the_column_cap_and_flagged() {
    let dir = tmp("colcap");
    let mut cells = String::new();
    for c in 0..10u32 {
        let col = (b'A' + c as u8) as char;
        cells += &num(&format!("{col}1"), None, &c.to_string());
    }
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            sheet_xml(&format!(r#"<row r="1">{cells}</row>"#), ""),
        )
        .write(&dir, "c.xlsx");
    let limits = Limits {
        max_cols: 4,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!(s.ncols, 4);
    assert!(s.cols_truncated);
    assert!(!s.rows_truncated);
    assert_eq!(s.display(0, 3), "3");
    assert!(s.cell(0, 4).is_none());
}

#[test]
fn the_cell_budget_cuts_rows_not_columns() {
    let dir = tmp("budget");
    let mut rows = String::new();
    for r in 1..=50 {
        rows += &format!(
            r#"<row r="{r}">{}{}{}{}</row>"#,
            num(&format!("A{r}"), None, "1"),
            num(&format!("B{r}"), None, "1"),
            num(&format!("C{r}"), None, "1"),
            num(&format!("D{r}"), None, "1")
        );
    }
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "bud.xlsx");
    // 4 columns, budget 40 cells -> 10 rows.
    let limits = Limits {
        max_grid_cells: 40,
        max_rows: 1000,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!((s.nrows, s.ncols), (10, 4));
    assert!(s.rows_truncated && !s.cols_truncated);
    // A budget smaller than one row still keeps one row (never an empty grid for a non-empty sheet).
    let limits = Limits {
        max_grid_cells: 1,
        max_rows: 1000,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!(s.nrows, 1);
    assert!(s.rows_truncated);
}

#[test]
fn a_sparse_far_flung_sheet_is_refused_before_calamine_can_allocate_for_it() {
    // Values at A1 and XFD1048576: calamine would build a ~17-billion-cell dense matrix.
    let dir = tmp("sparse");
    let rows = format!(
        r#"<row r="1">{}</row><row r="1048576">{}</row>"#,
        num("A1", None, "1"),
        num("XFD1048576", None, "2")
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "sparse.xlsx");
    let t = std::time::Instant::now();
    let r = load(&p);
    assert_eq!(r.unwrap_err(), OfficeError::TooLarge { what: "sheet area" });
    assert!(t.elapsed().as_secs() < 5);
}

#[test]
fn a_far_flung_hidden_sheet_does_not_block_the_visible_ones() {
    let dir = tmp("sparsehidden");
    let far = format!(
        r#"<row r="1">{}</row><row r="1048576">{}</row>"#,
        num("A1", None, "1"),
        num("XFD1048576", None, "2")
    );
    let ok = format!(r#"<row r="1">{}</row>"#, num("A1", None, "7"));
    let p = Pkg::new()
        .sheet("Visible", "visible", sheet_xml(&ok, ""))
        .sheet("Far", "hidden", sheet_xml(&far, ""))
        .write(&dir, "h.xlsx");
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    assert_eq!(wb.hidden_sheets, 1);
}

#[test]
fn a_declared_sheet_whose_part_is_missing_is_an_error_not_a_panic() {
    // workbook.xml lists a sheet whose part is missing from the zip.
    let dir = tmp("missingpart");
    let mut pkg = Pkg::new().sheet("S", "visible", sheet_xml("", ""));
    pkg.sheets[0].part = "xl/worksheets/other.xml".into(); // stored under another name than the rel says
    let r = load(&pkg.write(&dir, "m.xlsx"));
    assert!(matches!(r, Err(OfficeError::Corrupt(_))), "{r:?}");
}

// ---------------------------------------------------------------------------------------------
// limits and zip honesty
// ---------------------------------------------------------------------------------------------

/// Rewrites the declared uncompressed size of every entry (local headers and central directory).
fn forge_declared_size(mut z: Vec<u8>, declared: u32) -> Vec<u8> {
    let mut i = 0;
    while i + 30 < z.len() {
        if z[i..i + 4] == [0x50, 0x4b, 0x03, 0x04] {
            z[i + 22..i + 26].copy_from_slice(&declared.to_le_bytes());
            i += 4;
        } else if z[i..i + 4] == [0x50, 0x4b, 0x01, 0x02] {
            z[i + 24..i + 28].copy_from_slice(&declared.to_le_bytes());
            i += 4;
        } else {
            i += 1;
        }
    }
    z
}

#[test]
fn zip_does_not_stop_at_declared_size() {
    // Pins the behaviour the safety design depends on: zip 8.6 inflates past a *forged* declared
    // size (the central directory says 100 bytes; the stream holds 5 MiB). If a future zip starts
    // enforcing the declared size, this test fails and the extra counting in `container::scan_zip`
    // can be revisited.
    let real = vec![0u8; 5 << 20];
    let z = forge_declared_size(deflated(&[("a.bin", &real)]), 100);
    let mut ar = zip::ZipArchive::new(Cursor::new(z)).unwrap();
    assert_eq!(
        ar.by_index_raw(0).unwrap().size(),
        100,
        "the lie took effect"
    );
    let mut out = Vec::new();
    // Reading may or may not end in an error at EOF, but the data comes out in full first.
    let _ = ar.by_index(0).unwrap().read_to_end(&mut out);
    assert_eq!(out.len(), 5 << 20, "zip inflated past the declared size");
}

#[test]
fn a_zip_bomb_with_a_forged_size_is_stopped_by_counting_real_bytes() {
    let dir = tmp("bomb");
    let bomb = vec![0u8; 8 << 20];
    let z = forge_declared_size(
        deflated(&[("xl/workbook.xml", b"<workbook/>"), ("xl/junk.bin", &bomb)]),
        100,
    );
    let p = write(&dir, "bomb.xlsx", &z);
    let limits = Limits {
        max_part_bytes: 1 << 20,
        ..small_limits()
    };
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "entry" }
    );
    assert_eq!(
        container::inspect(&p, &limits).unwrap_err(),
        OfficeError::TooLarge { what: "entry" }
    );
}

#[test]
fn a_declared_size_over_the_limit_is_refused_without_inflating() {
    let dir = tmp("declared");
    let z = forge_declared_size(
        deflated(&[("xl/workbook.xml", b"<workbook/>")]),
        3_000_000_000,
    );
    let p = write(&dir, "d.xlsx", &z);
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "entry" }
    );
}

#[test]
fn many_medium_parts_hit_the_package_total() {
    let dir = tmp("total");
    let chunk = vec![0u8; 1 << 20];
    let z = deflated(&[
        ("xl/workbook.xml", b"<workbook/>"),
        ("a.bin", &chunk),
        ("b.bin", &chunk),
        ("c.bin", &chunk),
    ]);
    let p = write(&dir, "t.xlsx", &z);
    let limits = Limits {
        max_part_bytes: 1 << 20,
        max_total_bytes: (5 << 19) as u64, // 2.5 MiB
        ..small_limits()
    };
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "package" }
    );
    // A roomier total passes the container checks.
    let ok = Limits {
        max_total_bytes: 8 << 20,
        ..limits
    };
    assert!(container::inspect(&p, &ok).is_ok());
}

#[test]
fn too_many_entries_is_refused() {
    let dir = tmp("entries");
    let names: Vec<String> = (0..20).map(|i| format!("p{i}.xml")).collect();
    let mut es: Vec<(&str, &[u8])> = vec![("xl/workbook.xml", b"<workbook/>")];
    es.extend(names.iter().map(|n| (n.as_str(), b"x".as_slice())));
    let p = write(&dir, "e.xlsx", &deflated(&es));
    let limits = Limits {
        max_entries: 10,
        ..small_limits()
    };
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "entries" }
    );
    let limits = Limits {
        max_entries: 21,
        ..small_limits()
    };
    assert!(container::inspect(&p, &limits).is_ok());
}

#[test]
fn a_file_over_the_size_limit_is_refused_before_it_is_read() {
    let dir = tmp("big");
    let p = basic_pkg().write(&dir, "b.xlsx");
    let len = fs::metadata(&p).unwrap().len();
    let limits = Limits {
        max_file_bytes: len - 1,
        ..small_limits()
    };
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "file" }
    );
    let limits = Limits {
        max_file_bytes: len,
        ..small_limits()
    };
    assert!(load_with(&p, limits).is_ok());
}

#[test]
fn default_limits_are_the_documented_values() {
    let l = Limits::default();
    assert_eq!(l.max_file_bytes, 256 << 20);
    assert_eq!(l.max_entries, 10_000);
    assert_eq!(l.max_part_bytes, 256 << 20);
    assert_eq!(l.max_total_bytes, 1 << 30);
    assert_eq!(l.max_grid_cells, 4_000_000);
    assert_eq!(l.max_rows, crate::preview::table::MAX_ROWS);
    assert_eq!(l.max_cols, 16_384);
    assert_eq!(l.max_dense_cells, 16_000_000);
}

// ---------------------------------------------------------------------------------------------
// encryption and format detection
// ---------------------------------------------------------------------------------------------

pub(super) fn write_cfb(path: &Path, streams: &[(&str, &[u8])]) {
    let mut cf = cfb::create(path).unwrap();
    for (name, data) in streams {
        cf.create_stream(name).unwrap().write_all(data).unwrap();
    }
    cf.flush().unwrap();
}

#[test]
fn password_protected_ooxml_is_encrypted() {
    let dir = tmp("enc");
    for ext in ["xlsx", "xlsm", "xlsb", "xltx", "ods"] {
        let p = dir.join(format!("enc.{ext}"));
        write_cfb(
            &p,
            &[
                ("/EncryptionInfo", b"\x04\x00\x04\x00info"),
                ("/EncryptedPackage", b"\x10\x00\x00\x00\x00\x00\x00\x00data"),
            ],
        );
        assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted, "{ext}");
    }
    let p = dir.join("enc2.xlsx");
    write_cfb(&p, &[("/EncryptedPackage", b"x")]);
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::Encrypted,
        "package alone is enough"
    );
}

#[test]
fn a_cfb_that_is_not_a_workbook_is_unsupported() {
    let dir = tmp("doc");
    let p = dir.join("old.xls");
    write_cfb(&p, &[("/WordDocument", b"word")]);
    assert_eq!(load(&p).unwrap_err(), OfficeError::Unsupported);
}

/// A BIFF8 stream: BOF (workbook globals) then FILEPASS with a non-zero encryption type.
fn biff_filepass() -> Vec<u8> {
    let mut v = Vec::new();
    // BOF: version 0x0600, type 0x0005, build, year, flags, lowest version
    v.extend_from_slice(&[0x09, 0x08, 0x10, 0x00]);
    v.extend_from_slice(&[
        0x00, 0x06, 0x05, 0x00, 0xD3, 0x10, 0xCC, 0x07, 0, 0, 0, 0, 0x06, 0, 0, 0,
    ]);
    // FILEPASS: record 0x002F, first u16 = 1 (RC4)
    v.extend_from_slice(&[0x2F, 0x00, 0x06, 0x00, 0x01, 0x00, 0, 0, 0, 0]);
    // EOF
    v.extend_from_slice(&[0x0A, 0x00, 0x00, 0x00]);
    v
}

#[test]
fn an_xls_with_a_filepass_record_is_encrypted() {
    let dir = tmp("xlsenc");
    let p = dir.join("enc.xls");
    write_cfb(&p, &[("/Workbook", &biff_filepass())]);
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
}

#[test]
fn the_container_not_the_extension_decides_the_format() {
    let Some(xls) = sample_path_or_skip("sample.xls") else {
        return;
    };
    let Some(xlsx) = sample_path_or_skip("sample.xlsx") else {
        return;
    };
    let Some(ods) = sample_path_or_skip("sample.ods") else {
        return;
    };
    let dir = tmp("mislabeled");
    for (src, dst, want) in [
        (&xls, "really-xls.xlsx", Detected::Xls),
        (&xlsx, "really-xlsx.xls", Detected::Xlsx),
        (&ods, "really-ods.xlsx", Detected::Ods),
        (&xlsx, "really-xlsx.ods", Detected::Xlsx),
    ] {
        let p = dir.join(dst);
        fs::copy(src, &p).unwrap();
        assert_eq!(
            container::inspect(&p, &Limits::default()).unwrap(),
            want,
            "{dst}"
        );
        let wb = load(&p).unwrap_or_else(|e| panic!("{dst}: {e}"));
        assert_eq!(wb.sheets.len(), 2, "{dst}");
    }
}

#[test]
fn other_zip_packages_are_unsupported_not_corrupt() {
    let dir = tmp("docx");
    let p = write(
        &dir,
        "d.xlsx",
        &deflated(&[
            ("word/document.xml", b"<w/>"),
            ("[Content_Types].xml", b"<t/>"),
        ]),
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Unsupported);
}

#[test]
fn unknown_extension_is_unsupported_and_missing_file_is_io() {
    let dir = tmp("ext");
    let p = write(&dir, "a.txt", b"hello");
    assert_eq!(load(&p).unwrap_err(), OfficeError::Unsupported);
    let missing = dir.join("nope.xlsx");
    assert!(matches!(load(&missing), Err(OfficeError::Io(_))));
}

// ---------------------------------------------------------------------------------------------
// broken input never panics
// ---------------------------------------------------------------------------------------------

/// Runs the loader *without* the outer safety net, so a panic fails the test instead of being
/// converted into `Corrupt`.
pub(super) fn assert_inner_does_not_panic(
    path: &Path,
    what: &str,
) -> Result<Workbook, OfficeError> {
    let opts = LoadOptions {
        limits: small_limits(),
        ..LoadOptions::default()
    };
    let p = path.to_path_buf();
    match std::panic::catch_unwind(move || load_inner(&p, &opts)) {
        Ok(r) => r,
        Err(_) => panic!("the loader panicked on {what}"),
    }
}

#[test]
fn degenerate_files_are_errors() {
    let dir = tmp("degenerate");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty.xlsx", vec![]),
        ("one.xlsx", vec![b'P']),
        ("pk.xlsx", b"PK".to_vec()),
        ("pk4.xlsx", b"PK\x03\x04".to_vec()),
        (
            "eocd.xlsx",
            b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec(),
        ),
        ("text.xlsx", b"just some text, not a spreadsheet".to_vec()),
        (
            "cfbmagic.xls",
            [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1].to_vec(),
        ),
        ("cfbjunk.xls", {
            let mut v = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
            v.extend(std::iter::repeat_n(0xAB, 600));
            v
        }),
        ("zeros.ods", vec![0u8; 4096]),
    ];
    for (name, bytes) in cases {
        let p = write(&dir, name, &bytes);
        let r = assert_inner_does_not_panic(&p, name);
        assert!(r.is_err(), "{name} should be an error, got {r:?}");
        assert!(load(&p).is_err(), "{name}");
    }
}

#[test]
fn every_truncation_of_a_real_xlsx_is_handled() {
    let bytes = basic_pkg().bytes();
    let dir = tmp("trunc");
    for n in 0..bytes.len() {
        let p = write(&dir, "t.xlsx", &bytes[..n]);
        let r = assert_inner_does_not_panic(&p, &format!("truncation at {n}"));
        assert!(
            r.is_err(),
            "a {n}-byte prefix of a {}-byte zip loaded",
            bytes.len()
        );
    }
}

pub(super) fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

pub(super) fn read_parts(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut ar = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut parts = Vec::new();
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        parts.push((f.name().to_string(), b));
    }
    parts
}

pub(super) fn mutate(data: &mut Vec<u8>, seed: &mut u64, kind: u64) {
    if data.is_empty() {
        return;
    }
    let at = (xorshift(seed) as usize) % data.len();
    match kind % 4 {
        0 => data.truncate(at),
        1 => {
            for _ in 0..8 {
                let at = (xorshift(seed) as usize) % data.len();
                data[at] = xorshift(seed) as u8;
            }
        }
        2 => {
            data.drain(at..(at + 40).min(data.len()));
        }
        _ => {
            // Duplicate a slice (repeats attributes / elements).
            let dup = data[at..(at + 60).min(data.len())].to_vec();
            data.splice(at..at, dup);
        }
    }
}

#[test]
fn mutated_xml_parts_inside_a_valid_zip_never_panic_in_konomas_code() {
    // Valid zip + CRC, hostile XML: this is what reaches the format pass and calamine.
    let Some(sample) = sample_path_or_skip("sample.xlsx") else {
        return;
    };
    let parts = read_parts(&sample);
    let dir = tmp("mutate");
    let mut seed = 0x9E3779B97F4A7C15u64;
    for round in 0..150u64 {
        let mut mutated = parts.clone();
        for _ in 0..(1 + round % 3) {
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
        // konoma's own code (container scan + format pass) must never panic, uncontained.
        let inspected = std::panic::catch_unwind(|| {
            let _ = container::inspect(&p, &small_limits());
            let _ = fmt_xlsx::read(&p, &small_limits());
        });
        assert!(
            inspected.is_ok(),
            "container/format pass panicked in round {round}"
        );
        // The whole load (calamine included) is contained and must answer Ok or Err.
        let _ = load_with(&p, small_limits());
    }
}

#[test]
fn mutated_binary_xls_and_ods_are_contained() {
    let dir = tmp("mutate_bin");
    let mut seed = 0xDEADBEEFCAFEF00Du64;
    if let Some(xls) = sample_path_or_skip("sample.xls") {
        let orig = fs::read(&xls).unwrap();
        for round in 0..150u64 {
            let mut b = orig.clone();
            for _ in 0..(1 + round % 6) {
                let at = (xorshift(&mut seed) as usize) % b.len();
                b[at] = xorshift(&mut seed) as u8;
            }
            if round % 5 == 0 {
                let at = (xorshift(&mut seed) as usize) % b.len();
                b.truncate(at.max(8));
            }
            let p = write(&dir, "m.xls", &b);
            // calamine's xls reader can panic on damaged BIFF (found by this test: an arithmetic
            // overflow in xls.rs); `load_workbook` contains it, so only the public path is asserted.
            let _ = load_with(&p, small_limits());
        }
    }
    if let Some(ods) = sample_path_or_skip("sample.ods") {
        let parts = read_parts(&ods);
        for round in 0..100u64 {
            let mut m = parts.clone();
            let idx = m.iter().position(|(n, _)| n == "content.xml").unwrap();
            mutate(&mut m[idx].1, &mut seed, round);
            let refs: Vec<(&str, &[u8])> =
                m.iter().map(|(n, c)| (n.as_str(), c.as_slice())).collect();
            let p = write(&dir, "m.ods", &deflated(&refs));
            let _ = load_with(&p, small_limits());
        }
    }
}

#[test]
fn broken_sheet_xml_is_corrupt() {
    let dir = tmp("brokensheet");
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            "<worksheet><sheetData><row><c r=\"A1\"><v>1</c></row>".into(),
        )
        .write(&dir, "b.xlsx");
    let r = load(&p);
    assert!(matches!(r, Err(OfficeError::Corrupt(_))), "{r:?}");
}

#[test]
fn a_panic_inside_a_reader_is_contained() {
    // `load_workbook` wraps the whole load in `catch_silent`, the same helper the background
    // workers use; this pins the contract that a panic yields `None` (-> `Corrupt`) rather than
    // unwinding into the caller.
    let r: Option<Result<Workbook, OfficeError>> =
        crate::preview::markdown::catch_silent(|| panic!("boom"));
    assert!(r.is_none());
}

// ---------------------------------------------------------------------------------------------
// files written by a real application (LibreOffice)
// ---------------------------------------------------------------------------------------------

fn raw_f64(s: &Sheet, r: usize, c: usize) -> f64 {
    s.cell(r, c).unwrap().raw_text().parse().unwrap()
}

fn check_common_sample(wb: &Workbook, kind: &str) {
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Sales", "売上"], "{kind}");
    assert_eq!(wb.hidden_sheets, 1, "{kind}");
    let s = &wb.sheets[0];
    assert_eq!(s.display(0, 0), "Quarterly report 四半期報告", "{kind}");
    assert_eq!(s.display(1, 0), "Item 品名", "{kind}");
    assert_eq!(s.display(2, 0), "Apple りんご", "{kind}");
    assert_eq!(
        s.cell(2, 1).unwrap().cell_type(),
        CellType::Number,
        "{kind}"
    );
    assert!((raw_f64(s, 2, 1) - 1234567.891).abs() < 1e-6, "{kind}");
    assert!((raw_f64(s, 2, 2) - 1500.0).abs() < 1e-9, "{kind}");
    assert!((raw_f64(s, 2, 3) - 0.256).abs() < 1e-9, "{kind}");
    assert_eq!(
        s.cell(2, 4).unwrap().cell_type(),
        CellType::DateTime,
        "{kind}"
    );
    assert_eq!(
        s.cell(2, 5).unwrap().cell_type(),
        CellType::DateTime,
        "{kind}"
    );
    assert_eq!(s.display(3, 0), "Banana バナナ", "{kind}");
    assert_eq!(s.display(4, 0), "Total 合計", "{kind}");
    // The sheet that starts at C1 keeps A1 addresses.
    let u = &wb.sheets[1];
    assert_eq!(u.display(0, 2), "和暦の日付", "{kind}");
    assert_eq!(u.display(2, 3), "こんにちは 世界", "{kind}");
    assert!(u.cell(0, 0).is_none(), "{kind}");
    assert_eq!(
        u.cell(1, 2).unwrap().cell_type(),
        CellType::DateTime,
        "{kind}"
    );
}

#[test]
fn real_xlsx_written_by_libreoffice() {
    let Some(p) = sample_path_or_skip("sample.xlsx") else {
        return;
    };
    let wb = load(&p).unwrap();
    check_common_sample(&wb, "xlsx");
    assert!(!wb.date1904);
    let s = &wb.sheets[0];
    let f = |r, c| wb.format_of(s.cell(r, c).unwrap()).clone();
    assert_eq!(f(2, 0), NumFmtRef::General);
    assert_eq!(f(2, 1), NumFmtRef::Custom("#,##0.00".into()));
    assert_eq!(f(2, 2), NumFmtRef::Custom("[$¥-411]#,##0".into()));
    assert_eq!(f(2, 3), NumFmtRef::Custom("0.0%".into()));
    assert_eq!(f(2, 4), NumFmtRef::Custom("yyyy\\-mm\\-dd".into()));
    assert_eq!(f(2, 5), NumFmtRef::Custom("hh:mm:ss".into()));
    // Japanese era format on the second sheet.
    let u = &wb.sheets[1];
    match wb.format_of(u.cell(1, 2).unwrap()) {
        NumFmtRef::Custom(c) => assert!(c.contains("ggge") && c.contains('年'), "{c}"),
        other => panic!("{other:?}"),
    }
    // Merge, formulas, error.
    assert_eq!(
        s.merges,
        vec![MergeRange {
            row0: 0,
            col0: 0,
            row1: 0,
            col1: 2
        }]
    );
    assert_eq!(s.display(0, 1), "", "covered by the merge");
    assert_eq!(s.formula(4, 1), Some("SUM(B3:B4)"));
    assert_eq!(s.formula(4, 2), Some("SUM(C3:C4)"));
    assert_eq!(s.formula(4, 3), Some("B3/0"));
    assert_eq!(s.cell(4, 3).unwrap().cell_type(), CellType::Error);
    assert_eq!(s.display(4, 3), "#DIV/0!");
    assert!((raw_f64(s, 4, 1) - 1234609.891).abs() < 1e-6);
    assert_eq!((s.nrows, s.ncols), (5, 6));
}

#[test]
fn real_ods_written_by_libreoffice() {
    let Some(p) = sample_path_or_skip("sample.ods") else {
        return;
    };
    let wb = load(&p).unwrap();
    check_common_sample(&wb, "ods");
    let s = &wb.sheets[0];
    // calamine's ods reader turns LibreOffice's `calcext:value-type="error"` cell into an empty
    // string; the ods format pass restores the error value.
    assert_eq!(s.display(4, 3), "#DIV/0!");
    assert_eq!(s.cell(4, 3).unwrap().cell_type(), CellType::Error);
    // LibreOffice imports the source's boolean as the number 1 (no boolean style) when it saves.
    assert_eq!(s.cell(4, 4).unwrap().cell_type(), CellType::Number);
    assert_eq!(s.display(4, 4), "1");
    assert_eq!(s.formula(4, 1), Some("of:=SUM([.B3:.B4])"));
}

#[test]
fn real_xls_written_by_libreoffice() {
    let Some(p) = sample_path_or_skip("sample.xls") else {
        return;
    };
    let wb = load(&p).unwrap();
    check_common_sample(&wb, "xls");
    let s = &wb.sheets[0];
    assert_eq!(
        s.merges,
        vec![MergeRange {
            row0: 0,
            col0: 0,
            row1: 0,
            col1: 2
        }]
    );
    assert_eq!(s.cell(4, 3).unwrap().cell_type(), CellType::Error);
}

#[test]
fn real_samples_load_under_tight_limits_too() {
    // Limits small enough to cut the real sheets must truncate, not fail.
    let Some(p) = sample_path_or_skip("sample.xlsx") else {
        return;
    };
    let limits = Limits {
        max_rows: 2,
        max_cols: 3,
        ..small_limits()
    };
    let wb = load_with(&p, limits).unwrap();
    let s = &wb.sheets[0];
    assert_eq!((s.nrows, s.ncols), (2, 3));
    assert!(s.rows_truncated && s.cols_truncated);
    assert_eq!(s.display(1, 0), "Item 品名");
}

#[test]
fn locale_option_is_accepted_for_every_locale() {
    let Some(p) = sample_path_or_skip("sample.xlsx") else {
        return;
    };
    for locale in [Locale::En, Locale::Ja] {
        let wb = load_workbook(
            &p,
            &LoadOptions {
                locale,
                limits: Limits::default(),
            },
        )
        .unwrap();
        assert_eq!(wb.sheets.len(), 2);
    }
}

// ---------------------------------------------------------------------------------------------
// what the user sees: the same source, three formats, one display
// ---------------------------------------------------------------------------------------------

/// The display strings Excel / LibreOffice show for `scripts/office-samples/sample-source.fods`
/// (derived from that file: `#,##0.00`, `[$¥-411]#,##0`, `0.0%`, `yyyy-mm-dd`, `hh:mm:ss` and the
/// Japanese era format; the source's TRUE is stored by LibreOffice as the number 1 in all three
/// formats, so it is `1` here).
const SAMPLE_SALES: [[&str; 6]; 5] = [
    ["Quarterly report 四半期報告", "", "", "", "", ""],
    ["Item 品名", "Qty", "Price", "Rate", "Date", "Time"],
    [
        "Apple りんご",
        "1,234,567.89",
        "¥1,500",
        "25.6%",
        "2026-10-02",
        "13:05:09",
    ],
    [
        "Banana バナナ",
        "42.00",
        "¥980",
        "5.0%",
        "2024-02-29",
        "00:00:30",
    ],
    ["Total 合計", "1,234,609.89", "¥2,480", "#DIV/0!", "1", ""],
];

const SAMPLE_URIAGE: [[&str; 4]; 3] = [
    ["", "", "和暦の日付", "Plain text"],
    ["", "", "令和8年10月2日", "0.1"],
    ["", "", "平成1年1月8日", "こんにちは 世界"],
];

#[test]
fn the_three_real_samples_show_what_excel_shows() {
    for name in ["sample.xlsx", "sample.ods", "sample.xls"] {
        let Some(p) = sample_path_or_skip(name) else {
            continue;
        };
        // The formats in the samples are explicit, so the locale must not matter.
        for locale in [Locale::En, Locale::Ja] {
            let wb = load_workbook(
                &p,
                &LoadOptions {
                    locale,
                    limits: Limits::default(),
                },
            )
            .unwrap();
            let sales = &wb.sheets[0];
            for (r, row) in SAMPLE_SALES.iter().enumerate() {
                for (c, want) in row.iter().enumerate() {
                    assert_eq!(
                        sales.display(r, c),
                        *want,
                        "{name} {locale:?} Sales ({r},{c})"
                    );
                }
            }
            let uriage = &wb.sheets[1];
            for (r, row) in SAMPLE_URIAGE.iter().enumerate() {
                for (c, want) in row.iter().enumerate() {
                    assert_eq!(
                        uriage.display(r, c),
                        *want,
                        "{name} {locale:?} 売上 ({r},{c})"
                    );
                }
            }
        }
    }
}

#[test]
fn the_error_cell_of_every_real_sample_is_an_error_value() {
    for name in ["sample.xlsx", "sample.ods", "sample.xls"] {
        let Some(p) = sample_path_or_skip(name) else {
            continue;
        };
        let wb = load(&p).unwrap();
        let c = wb.sheets[0].cell(4, 3).unwrap();
        assert_eq!(c.cell_type(), CellType::Error, "{name}");
        assert_eq!(c.display(), "#DIV/0!", "{name}");
    }
}

#[test]
fn raw_values_stay_available_next_to_the_formatted_text() {
    // The detail view shows the raw value: formatting must not lose it.
    let Some(p) = sample_path_or_skip("sample.ods") else {
        return;
    };
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.display(2, 1), "1,234,567.89");
    assert_eq!(s.cell(2, 1).unwrap().raw_text(), "1234567.891");
    assert_eq!(s.cell(2, 3).unwrap().raw_text(), "0.256");
}
