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
use super::workbook::{load_workbook_unguarded, number_text};
use super::xlsx;
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

pub(super) const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub(super) const RNS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// `[(name, state, rid)]`.
pub(super) fn workbook_xml(date1904: bool, sheets: &[(&str, &str, &str)]) -> String {
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

pub(super) fn rels_xml(rels: &[(&str, &str)]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, target) in rels {
        s += &format!(r#"<Relationship Id="{id}" Type="x/worksheet" Target="{target}"/>"#);
    }
    s + "</Relationships>"
}

/// `numfmts`: `(id, code)`; `xfs`: numFmtId of each cellXfs entry.
pub(super) fn styles_xml(numfmts: &[(u32, &str)], xfs: &[u32]) -> String {
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

pub(super) fn sheet_xml(rows: &str, after: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS}"><sheetData>{rows}</sheetData>{after}</worksheet>"#
    )
}

pub(super) fn istr(r: &str, s: Option<u32>, text: &str) -> String {
    let style = s.map(|s| format!(r#" s="{s}""#)).unwrap_or_default();
    format!(r#"<c r="{r}"{style} t="inlineStr"><is><t>{text}</t></is></c>"#)
}

pub(super) fn num(r: &str, s: Option<u32>, v: &str) -> String {
    let style = s.map(|s| format!(r#" s="{s}""#)).unwrap_or_default();
    format!(r#"<c r="{r}"{style}><v>{v}</v></c>"#)
}

pub(super) struct SheetDef {
    pub(super) name: String,
    pub(super) state: String,
    pub(super) rid: String,
    /// Path of the part inside the zip.
    pub(super) part: String,
    /// `Target` written in the relationships file.
    pub(super) target: String,
    pub(super) xml: String,
}

pub(super) struct Pkg {
    pub(super) date1904: bool,
    pub(super) sheets: Vec<SheetDef>,
    pub(super) styles: Option<String>,
    /// `xl/sharedStrings.xml`.
    pub(super) shared: Option<String>,
}

impl Pkg {
    pub(super) fn new() -> Pkg {
        Pkg {
            date1904: false,
            sheets: Vec::new(),
            styles: None,
            shared: None,
        }
    }
    pub(super) fn shared(mut self, xml: String) -> Pkg {
        self.shared = Some(xml);
        self
    }
    pub(super) fn sheet(mut self, name: &str, state: &str, xml: String) -> Pkg {
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
    pub(super) fn styles(mut self, xml: String) -> Pkg {
        self.styles = Some(xml);
        self
    }
    pub(super) fn date1904(mut self) -> Pkg {
        self.date1904 = true;
        self
    }
    /// Use `/xl/worksheets/sheetN.xml` (absolute) targets in the rels.
    pub(super) fn absolute_targets(mut self) -> Pkg {
        for s in &mut self.sheets {
            s.target = format!("/{}", s.part);
        }
        self
    }
    pub(super) fn bytes(&self) -> Vec<u8> {
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
        if let Some(sh) = &self.shared {
            entries.push(("xl/sharedStrings.xml".into(), sh.clone().into_bytes()));
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
    pub(super) fn write(&self, dir: &TmpDir, name: &str) -> PathBuf {
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
        max_sheet_cells: 1000,
        max_rows: 100,
        max_cols: 16,
        max_sheet_text_bytes: 1 << 20,
        max_dense_cells: 10_000,
        max_text_bytes: 1 << 20,
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
    display_text(
        &v,
        fmt,
        &DisplayCtx {
            date1904,
            null_day: None,
            locale,
        },
    )
}

fn custom(code: &str) -> NumFmtRef {
    NumFmtRef::Custom(code.into())
}

#[test]
fn display_text_with_general_is_plain_for_every_value_kind() {
    let g = NumFmtRef::General;
    let t = |v: CellValue| shown(v, &g, false, Locale::Ja);
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
    let p = |xml: &str| xlsx::parse_workbook(xml.as_bytes()).unwrap();
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

/// A sheet read through the real loader. `xfs` are the `numFmtId` of each cell style: with
/// `[0, 14]`, a cell with `s="1"` has format index 1 (a date) and shows as one.
fn read_xml(xml: &str, xfs: &[u32], limits: Limits) -> Sheet {
    let dir = tmp("readxml");
    let p = Pkg::new()
        .styles(styles_xml(&[], xfs))
        .sheet("S", "visible", xml.to_string())
        .write(&dir, "r.xlsx");
    load_with(&p, limits).unwrap().sheets.remove(0)
}

fn read_rows(rows: &str, limits: Limits) -> Sheet {
    read_xml(&sheet_xml(rows, ""), &[0, 14], limits)
}

/// `(row, col, format index)` of every kept cell whose format is not General.
fn formatted(s: &Sheet) -> Vec<(u32, u32, u16)> {
    let mut out = Vec::new();
    for r in 0..s.nrows {
        for (c, cell) in s.row_cells(r) {
            if cell.fmt != 0 {
                out.push((r as u32, *c, cell.fmt));
            }
        }
    }
    out
}

/// How many cells the sheet keeps.
fn kept(s: &Sheet) -> usize {
    (0..s.nrows).map(|r| s.row_cells(r).len()).sum()
}

#[test]
fn xf_zero_and_missing_s_are_general() {
    // xf 1 -> format 1; a cell without `s` uses xf 0 -> format 0 (General).
    let s = read_rows(
        &format!(
            r#"<row r="1">{}{}{}</row>"#,
            num("A1", None, "1"),
            num("B1", Some(1), "2"),
            num("C1", Some(0), "3")
        ),
        Limits::default(),
    );
    assert_eq!(formatted(&s), vec![(0, 1, 1)]);
    assert_eq!(kept(&s), 3);
    assert_eq!(s.cell(0, 1).unwrap().fmt, 1);
    assert_eq!(s.cell(0, 0).unwrap().fmt, 0);
    assert!(s.cell(5, 5).is_none());
    // A file whose xf 0 is itself a date format applies it to cells with no `s`.
    let s = read_xml(
        &sheet_xml(&format!(r#"<row r="1">{}</row>"#, num("A1", None, "1")), ""),
        &[14],
        Limits::default(),
    );
    assert_eq!(s.cell(0, 0).unwrap().fmt, 1);
}

#[test]
fn cells_without_r_continue_in_their_row_and_rows_without_r_follow() {
    // Row 1 has `r`; its cells do not. Row 2 has no `r` (-> 2). Row 5 jumps; the row after it has none (-> 6).
    let rows = r#"
        <row r="1"><c s="1"><v>1</v></c><c s="1"><v>2</v></c><c r="D1" s="1"><v>3</v></c><c s="1"><v>4</v></c></row>
        <row><c s="1"><v>5</v></c></row>
        <row r="5"><c s="1"><v>6</v></c></row>
        <row><c s="1"><v>7</v></c></row>
        <row r="9"/>
        <row><c s="1"><v>8</v></c></row>"#;
    let s = read_rows(rows, Limits::default());
    assert_eq!(
        formatted(&s),
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
fn only_value_bearing_cells_are_kept() {
    let rows = r#"<row r="1">
        <c r="A1" s="1"/>
        <c r="B1" s="1"></c>
        <c r="C1" s="1"><f>1+1</f></c>
        <c r="D1" s="1"><v>1</v></c>
        <c r="E1" s="1" t="inlineStr"><is><t>x</t></is></c>
        </row>"#;
    let s = read_rows(rows, Limits::default());
    // Only a `<v>` with text and an `<is>` show something. A formula-only cell shows nothing, so it
    // is not a cell (its formula is kept in the formula map).
    assert_eq!(kept(&s), 2);
    assert_eq!(formatted(&s), vec![(0, 3, 1), (0, 4, 1)]);
    assert_eq!(s.formula(0, 2), Some("1+1"));
}

#[test]
fn cells_beyond_the_maximum_column_are_dropped_and_bad_refs_survive() {
    let rows = r#"<row r="1">
        <c r="XFD1" s="1"><v>1</v></c>
        <c r="XFE1" s="1"><v>1</v></c>
        <c r="ZZZZ1" s="1"><v>1</v></c>
        <c r="??" s="1"><v>1</v></c>
        </row>"#;
    let s = read_rows(rows, Limits::default());
    // XFD (index 16,383) is kept with its format; XFE and ZZZZ are past the maximum so the cells
    // are skipped; "??" falls back to the sequence position (after ZZZZ), also past it.
    assert_eq!(formatted(&s), vec![(0, 16_383, 1)]);
    assert_eq!(kept(&s), 1);
    assert!(s.cols_truncated);
    assert!(!s.rows_truncated);
}

#[test]
fn out_of_order_cells_are_sorted_so_lookup_works() {
    let rows = format!(
        r#"<row r="3">{}</row><row r="1">{}{}</row>"#,
        num("A3", Some(1), "1"),
        num("C1", Some(1), "1"),
        num("A1", Some(1), "1"),
    );
    let s = read_rows(&rows, Limits::default());
    assert_eq!(formatted(&s), vec![(0, 0, 1), (0, 2, 1), (2, 0, 1)]);
    assert_eq!(s.cell(0, 2).unwrap().fmt, 1);
    assert_eq!(s.cell(2, 0).unwrap().fmt, 1);
}

#[test]
fn the_cell_budget_is_enforced_while_streaming() {
    let mut rows = String::new();
    for r in 1..=20 {
        rows += &format!(
            r#"<row r="{r}">{}{}</row>"#,
            num(&format!("A{r}"), None, "1"),
            num(&format!("B{r}"), None, "1")
        );
    }
    let limits = Limits {
        max_sheet_cells: 10,
        ..Limits::default()
    };
    // The sheet has 40 cells: the reader stops at the 11th and the sheet says so; nothing past it
    // is kept.
    let s = read_rows(&rows, limits);
    assert!(s.rows_truncated);
    assert_eq!(kept(&s), 10);
}

#[test]
fn an_empty_sheet_has_no_cells() {
    let s = read_rows("", Limits::default());
    assert!(s.loaded);
    assert_eq!((s.nrows, s.ncols), (0, 0));
    assert!(!s.rows_truncated && !s.cols_truncated);
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
    // The reader resolved the absolute target too (it saw the sheet).
    let book = xlsx::open(&p, &Limits::default()).unwrap();
    assert_eq!(book.part_of("S"), Some("xl/worksheets/sheet1.xml"));
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
        max_sheet_cells: 40,
        max_rows: 1000,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!((s.nrows, s.ncols), (10, 4));
    assert!(s.rows_truncated && !s.cols_truncated);
    // A budget smaller than one row still keeps one row (never an empty grid for a non-empty sheet).
    let limits = Limits {
        max_sheet_cells: 1,
        max_rows: 1000,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!(s.nrows, 1);
    assert!(s.rows_truncated);
}

#[test]
fn a_sparse_far_flung_sheet_shows_what_fits_instead_of_being_refused() {
    // Values at A1 and XFD1048576. The old reader built a dense matrix over the bounding box
    // (~17 billion cells, so the file had to be refused); the streamed reader reads A1, meets a
    // row past the row cap and stops: the sheet is shown, capped.
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
    let wb = load(&p).unwrap();
    assert!(t.elapsed().as_secs() < 5);
    let s = &wb.sheets[0];
    assert_eq!(s.display(0, 0), "1");
    assert!(s.rows_truncated, "the far cell is past the row cap");
    // The grid is what was kept, never the box the file describes.
    assert_eq!((s.nrows, s.ncols), (1, 1));
}

#[test]
fn a_format_code_over_255_characters_is_dropped_and_255_is_kept() {
    let keep = format!("0.{}", "0".repeat(253));
    let drop = format!("0.{}", "0".repeat(254));
    let xml = styles_xml(&[(164, &keep), (165, &drop)], &[164, 165]);
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(
        st.formats,
        vec![NumFmtRef::General, NumFmtRef::Custom(keep.as_str().into())]
    );
    assert_eq!(st.xf_to_format, vec![1, 0]);
    // Counted in characters, not bytes: 255 two-byte characters are kept.
    let wide = "é".repeat(255);
    let xml = styles_xml(&[(164, &wide)], &[164]);
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(st.formats.len(), 2);
    // A code written with escapes is measured after decoding: 255 `&amp;` are 255 characters.
    let esc = "&amp;".repeat(255);
    let xml = styles_xml(&[(164, &esc)], &[164]);
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(st.formats.len(), 2, "1,275 raw bytes, 255 characters");
    // End to end.
    let dir = tmp("xlsx_longcode");
    let rows = format!(
        r#"<row r="1">{}{}</row>"#,
        num("A1", Some(1), "0.5"),
        num("B1", Some(2), "0.5")
    );
    let p = Pkg::new()
        .styles(styles_xml(&[(164, &keep), (165, &drop)], &[0, 164, 165]))
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "l.xlsx");
    let wb = load(&p).unwrap();
    assert!(
        wb.sheets[0].display(0, 0).starts_with("0.5000"),
        "the kept code applies"
    );
    assert_eq!(wb.sheets[0].display(0, 1), "0.5");
    assert_eq!(
        wb.formats.len(),
        2,
        "the long code is not in the workbook's table"
    );
}

#[test]
fn a_huge_format_code_is_dropped_without_being_decoded() {
    // A 4 MB attribute: far past the 2,550 raw bytes a kept code can take, so it is skipped
    // before being unescaped or copied; the styles part still parses and the rest is intact.
    let huge = "9".repeat(4 << 20);
    let xml = format!(
        r#"<?xml version="1.0"?><styleSheet xmlns="{NS}"><numFmts count="2">
        <numFmt numFmtId="164" formatCode="{huge}"/><numFmt numFmtId="165" formatCode="0.0"/></numFmts>
        <cellXfs count="2"><xf numFmtId="164"/><xf numFmtId="165"/></cellXfs></styleSheet>"#
    );
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(
        st.formats,
        vec![NumFmtRef::General, NumFmtRef::Custom("0.0".into())]
    );
    assert_eq!(st.xf_to_format, vec![0, 1]);
}

#[test]
fn a_row_reference_past_u32_saturates_instead_of_falling_back_to_the_next_row() {
    // `parse::<u32>` fails on 5000000000 and a reader that treated that as "the next row" would
    // show the cell at row 2; the reference is a row far past any cap and must read as such.
    let xml = sheet_xml(
        &format!(
            r#"<row r="1">{}</row><row r="5000000000">{}</row>"#,
            num("A1", Some(1), "1"),
            num("A5000000000", Some(1), "2")
        ),
        "",
    );
    let s = read_xml(&xml, &[0, 14], Limits::default());
    assert!(s.rows_truncated);
    assert_eq!(formatted(&s), vec![(0, 0, 1)]);
    assert_eq!(s.nrows, 1);
}

#[test]
fn the_reader_stops_at_the_first_row_past_the_cap() {
    let rows: String = (1..=10)
        .map(|r| {
            format!(
                r#"<row r="{r}">{}{}</row>"#,
                num(&format!("A{r}"), Some(1), "1"),
                num(&format!("B{r}"), Some(1), "1")
            )
        })
        .collect();
    let limits = Limits {
        max_rows: 4,
        ..Limits::default()
    };
    let s = read_rows(&rows, limits);
    assert!(s.rows_truncated);
    assert_eq!(s.nrows, 4);
    assert_eq!(formatted(&s).len(), 8, "rows 1-4: eight cells");
}

#[test]
fn merged_ranges_are_read_and_bounded() {
    let after = r#"<mergeCells count="3"><mergeCell ref="A1:B2"/><mergeCell ref="C3"/><mergeCell ref="??"/></mergeCells>"#;
    let xml = sheet_xml(
        &format!(r#"<row r="1">{}</row>"#, num("A1", None, "1")),
        after,
    );
    let s = read_xml(&xml, &[0], Limits::default());
    assert_eq!(
        s.merges,
        vec![
            MergeRange {
                row0: 0,
                col0: 0,
                row1: 1,
                col1: 1
            },
            MergeRange {
                row0: 2,
                col0: 2,
                row1: 2,
                col1: 2
            },
        ]
    );
    // And through the loader.
    let dir = tmp("xlsx_merges");
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            sheet_xml(
                &format!(r#"<row r="1">{}</row>"#, num("A1", None, "1")),
                after,
            ),
        )
        .write(&dir, "m.xlsx");
    assert_eq!(load(&p).unwrap().sheets[0].merges.len(), 2);
    // At most MAX_MERGES are kept.
    let many: String = (0..xlsx::MAX_MERGES + 50)
        .map(|i| format!(r#"<mergeCell ref="A{}"/>"#, i + 1))
        .collect();
    let xml = sheet_xml("", &format!("<mergeCells>{many}</mergeCells>"));
    let s = read_xml(&xml, &[0], Limits::default());
    assert_eq!(s.merges.len(), xlsx::MAX_MERGES);
}

#[test]
fn only_the_sheet_asked_for_is_read_and_the_index_is_clamped() {
    let dir = tmp("xlsx_one");
    let cell = |t: &str| format!(r#"<row r="1">{}</row>"#, istr("A1", None, t));
    let p = Pkg::new()
        .sheet("A", "visible", sheet_xml(&cell("a-text"), ""))
        .sheet("Hidden", "hidden", sheet_xml(&cell("h-text"), ""))
        .sheet("B", "visible", sheet_xml(&cell("b-text"), ""))
        .sheet("C", "visible", sheet_xml(&cell("c-text"), ""))
        .write(&dir, "o.xlsx");
    let one = |i: usize| load_workbook_sheet(&p, &LoadOptions::default(), i).unwrap();
    for (i, want) in [(0, "a-text"), (1, "b-text"), (2, "c-text"), (50, "c-text")] {
        let wb = one(i);
        let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            ["A", "B", "C"],
            "visible sheets listed by name, hidden counted"
        );
        assert_eq!(wb.hidden_sheets, 1);
        let k = i.min(2);
        assert_eq!(wb.loaded_index(), Some(k));
        assert_eq!(wb.sheets.iter().filter(|s| s.loaded).count(), 1);
        assert_eq!(wb.sheets[k].display(0, 0), want);
        // The sheets that were not asked for are the placeholders, not empty grids that look read.
        for (j, s) in wb.sheets.iter().enumerate().filter(|&(j, _)| j != k) {
            assert!(!s.loaded && s.nrows == 0 && s.cell(0, 0).is_none(), "{j}");
        }
    }
    // A workbook with no visible sheet lists none and loads none, whatever is asked.
    let p = Pkg::new()
        .sheet("H", "hidden", sheet_xml(&cell("x"), ""))
        .write(&dir, "n.xlsx");
    let wb = one_of(&p, 3);
    assert!(wb.sheets.is_empty() && wb.loaded_index().is_none() && wb.sheet_error.is_none());
    assert_eq!(wb.hidden_sheets, 1);
}

fn one_of(p: &Path, i: usize) -> Workbook {
    load_workbook_sheet(p, &LoadOptions::default(), i).unwrap()
}

#[test]
fn a_damaged_sheet_is_a_sheet_error_not_a_workbook_error() {
    let dir = tmp("xlsx_badsheet");
    let good = format!(r#"<row r="1">{}</row>"#, istr("A1", None, "fine"));
    let p = Pkg::new()
        .sheet("Good", "visible", sheet_xml(&good, ""))
        .sheet(
            "Bad",
            "visible",
            sheet_xml(r#"<row r="1"><c r="A1"><v>1</v></c></row></nope>"#, ""),
        )
        .write(&dir, "b.xlsx");
    let wb = one_of(&p, 1);
    assert_eq!(wb.sheets.len(), 2);
    assert!(wb.loaded_index().is_none());
    assert!(
        matches!(wb.sheet_error, Some(OfficeError::Corrupt(_))),
        "{:?}",
        wb.sheet_error
    );
    let wb = one_of(&p, 0);
    assert_eq!(wb.sheets[0].display(0, 0), "fine");
    assert!(wb.sheet_error.is_none());
    // Asking for every sheet at once (tests only) is an error: nothing is silently skipped.
    assert!(matches!(load(&p), Err(OfficeError::Corrupt(_))));
}

#[test]
fn unload_cells_keeps_the_names_and_drops_the_cells_and_the_error() {
    let dir = tmp("xlsx_unload");
    let cell = |t: &str| format!(r#"<row r="1">{}</row>"#, istr("A1", None, t));
    let p = Pkg::new()
        .sheet("A", "visible", sheet_xml(&cell("a"), ""))
        .sheet("B", "visible", sheet_xml(&cell("b"), ""))
        .write(&dir, "u.xlsx");
    let mut wb = one_of(&p, 1);
    wb.sheet_error = Some(OfficeError::Encrypted);
    wb.unload_cells();
    assert!(wb.loaded_index().is_none() && wb.sheet_error.is_none());
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["A", "B"]);
    assert!(wb.sheets.iter().all(|s| s.nrows == 0));
}

#[test]
fn a_cell_that_arrives_out_of_order_or_twice_is_sorted_and_the_last_one_wins() {
    let dir = tmp("xlsx_order");
    let rows = format!(
        r#"<row r="1">{}{}{}</row>"#,
        num("C1", None, "3"),
        num("A1", None, "1"),
        num("C1", None, "30"),
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "o.xlsx");
    let s = &load(&p).unwrap().sheets[0];
    assert_eq!(
        s.row_cells(0).iter().map(|&(c, _)| c).collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(s.display(0, 0), "1");
    assert_eq!(s.display(0, 2), "30");
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
// memory: what calamine materialises must be inside the checked box and budgets
// ---------------------------------------------------------------------------------------------

/// `small_limits` with room for parts of a few MB (the text budget stays 1 MiB).
fn roomy_parts() -> Limits {
    Limits {
        max_file_bytes: 8 << 20,
        max_part_bytes: 8 << 20,
        max_total_bytes: 32 << 20,
        ..small_limits()
    }
}

fn formula_cell(r: &str) -> String {
    format!(r#"<c r="{r}"><f>1</f></c>"#)
}

fn shared_xml(strings: &[String]) -> String {
    let mut s = format!(
        r#"<?xml version="1.0"?><sst xmlns="{NS}" count="{n}" uniqueCount="{n}">"#,
        n = strings.len()
    );
    for t in strings {
        s += &format!("<si><t>{t}</t></si>");
    }
    s + "</sst>"
}

fn shared_ref(r: &str) -> String {
    format!(r#"<c r="{r}" t="s"><v>0</v></c>"#)
}

#[test]
fn formula_only_cells_in_far_corners_do_not_widen_the_sheet() {
    // The old reader built the range of formulas over the bounding box of the formula cells
    // (~550 GB for these two). Streamed, the formula at A1 is kept and the far one is past the
    // row cap, where the reading stops.
    let dir = tmp("xlsx_formula_far");
    let rows = format!(
        r#"<row r="1">{}</row><row r="1048576">{}</row>"#,
        formula_cell("A1"),
        formula_cell("XFD1048576")
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "f.xlsx");
    let t = std::time::Instant::now();
    let wb = load(&p).unwrap();
    assert!(t.elapsed().as_secs() < 5);
    let s = &wb.sheets[0];
    assert_eq!(s.formula(0, 0), Some("1"));
    assert!(s.rows_truncated);
    assert_eq!((s.nrows, s.ncols), (1, 1));
    // One value and one formula-only cell apart: same.
    let rows = format!(
        r#"<row r="1">{}</row><row r="1048576">{}</row>"#,
        num("A1", None, "1"),
        formula_cell("XFD1048576")
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "g.xlsx");
    let s = &load(&p).unwrap().sheets[0];
    assert_eq!(s.display(0, 0), "1");
    assert_eq!((s.nrows, s.ncols), (1, 1));
}

#[test]
fn a_shared_formula_cell_far_away_is_never_reached() {
    let dir = tmp("xlsx_shared_formula_far");
    let far = r#"<row r="1048576"><c r="XFD1048576"><f t="shared" si="0"/></c></row>"#;
    // With its anchor before it, the far cell is a formula past the row cap: data left out.
    let rows = format!(
        r#"<row r="1"><c r="A1"><f t="shared" ref="A1:XFD1048576" si="0">1+1</f><v>2</v></c></row>{far}"#,
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "s.xlsx");
    let s = &load(&p).unwrap().sheets[0];
    assert_eq!((s.nrows, s.ncols), (1, 1));
    assert!(s.rows_truncated);
    // Without an anchor the cell has no formula and no value: nothing is left out.
    let rows = format!(r#"<row r="1">{}</row>{far}"#, num("A1", None, "1"));
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "t.xlsx");
    let s = &load(&p).unwrap().sheets[0];
    assert_eq!((s.nrows, s.ncols), (1, 1));
    assert!(!s.rows_truncated);
}

#[test]
fn columns_past_xfd_are_skipped_and_a_far_row_ends_the_sheet() {
    // `ZZZ1` is past the column cap: its cell is dropped and the sheet says columns were cut.
    // `ZZZ1048576` is past the row cap, where the reading stops.
    let dir = tmp("xlsx_zzz");
    let rows = format!(
        r#"<row r="1">{}{}</row><row r="1048576">{}</row>"#,
        num("A1", None, "1"),
        num("ZZZ1", None, "9"),
        num("ZZZ1048576", None, "2")
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "z.xlsx");
    let s = &load_with(&p, small_limits()).unwrap().sheets[0];
    assert_eq!(s.display(0, 0), "1");
    assert!(s.cols_truncated && s.rows_truncated);
    assert_eq!(s.ncols, 16, "the column cap of `small_limits`");
    assert_eq!(
        s.row_cells(0).len(),
        1,
        "the cell past the column cap is gone"
    );
}

/// The properties of a hostile reference that matter, whichever way it is written: the sheet
/// loads (the rows before it are shown), it says it was cut, and **the value reader is never asked
/// to parse the reference** — `calamine` does that with plain `u32` arithmetic that panics in a
/// debug build and wraps in a release build (to an arbitrary row: a `<row r="5000000000">` landed
/// at row ~705 million, which sized its dense matrix at ~22 GB). The tests run in a debug build, so
/// a reader that *was* asked would panic and the sheet would come back as a `Corrupt`.
#[test]
fn a_reference_that_would_overflow_the_readers_arithmetic_is_never_handed_to_it() {
    let dir = tmp("xlsx_absurd_ref");
    let cases: Vec<(&str, String)> = vec![
        // The cell reference overflows.
        (
            "cell letters",
            format!(
                r#"<row r="1">{}{}</row>"#,
                num("A1", None, "1"),
                num("ZZZZZZZZZZ1", None, "2")
            ),
        ),
        (
            "cell digits",
            format!(
                r#"<row r="1">{}{}</row>"#,
                num("A1", None, "1"),
                num("A99999999999", None, "2")
            ),
        ),
        (
            "cell letters 2",
            format!(
                r#"<row r="1">{}{}</row>"#,
                num("A1", None, "1"),
                num("AAAAAAAA5", None, "2")
            ),
        ),
        // The row reference overflows `u32` (5,000,000,000; and 4,294,967,297, which wraps to row 1).
        (
            "row 5e9",
            format!(
                r#"<row r="1">{}</row><row r="5000000000">{}</row>"#,
                num("A1", None, "1"),
                num("A5000000000", None, "2")
            ),
        ),
        (
            "row 5e9 cell without r",
            format!(
                r#"<row r="1">{}</row><row r="5000000000"><c><v>2</v></c></row>"#,
                num("A1", None, "1"),
            ),
        ),
        (
            "row wraps to 1",
            format!(
                r#"<row r="1">{}</row><row r="4294967297"><c r="A4294967297"><v>2</v></c></row>"#,
                num("A1", None, "1"),
            ),
        ),
        // Merely past the row cap (a perfectly valid file).
        (
            "row past the cap",
            format!(
                r#"<row r="1">{}</row><row r="101">{}</row>"#,
                num("A1", None, "1"),
                num("A101", None, "2")
            ),
        ),
    ];
    for (what, rows) in cases {
        let p = Pkg::new()
            .sheet("S", "visible", sheet_xml(&rows, ""))
            .write(&dir, "r.xlsx");
        let wb = load_with(&p, small_limits()).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        let s = &wb.sheets[0];
        assert_eq!(s.display(0, 0), "1", "{what}");
        assert!(s.rows_truncated, "{what}");
        assert_eq!(
            (s.nrows, s.ncols),
            (1, 1),
            "{what}: nothing past the bad reference"
        );
    }
}

/// The old value reader (`calamine`) grew a table to the largest `si` it met (`si="4000000000"` =
/// ~160 GB), so such a sheet had to be refused. konoma's reader keys the shared formulas by `si`
/// in a bounded map: a forged index costs one entry, and the sheet loads.
#[test]
fn a_forged_shared_formula_index_costs_one_table_entry() {
    let dir = tmp("xlsx_si");
    let rows = r#"<row r="1"><c r="A1"><f t="shared" ref="A1:A2" si="4000000000">1+1</f><v>2</v></c><c r="B1"><f t="shared" si="4000000000"/><v>3</v></c></row>"#;
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(rows, ""))
        .write(&dir, "si.xlsx");
    let s = &load(&p).unwrap().sheets[0];
    assert_eq!(s.formula(0, 0), Some("1+1"));
    assert_eq!(
        s.formula(0, 1),
        Some("1+1"),
        "same group, no reference to move"
    );
    // An index past u32 is no group: an ordinary formula.
    let rows = r#"<row r="1"><c r="A1"><f t="shared" ref="A1:A2" si="3">1+1</f><v>2</v></c></row>"#;
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(rows, ""))
        .write(&dir, "si_ok.xlsx");
    assert!(load(&p).is_ok());
}

/// A workbook with one shared string of `len` bytes referenced by `refs` cells.
fn shared_ref_pkg(len: usize, refs: usize) -> Pkg {
    let rows: String = (1..=refs)
        .map(|i| format!(r#"<row r="{i}">{}</row>"#, shared_ref(&format!("A{i}"))))
        .collect();
    Pkg::new()
        .shared(shared_xml(&["x".repeat(len)]))
        .sheet("S", "visible", sheet_xml(&rows, ""))
}

#[test]
fn one_long_shared_string_used_by_many_cells_cuts_the_sheet_at_the_text_budget() {
    // 10 KB x 200 cells = 2 MB of text from a ~12 KB file (the sheet's text budget here is
    // 1 MiB). In the real limits this is the 100 KB -> 1.2 GB amplification. The sheet is shown up
    // to the cell that no longer fits, and says it was cut — it is not refused.
    let dir = tmp("xlsx_amplify");
    let limits = Limits {
        max_rows: 1000,
        max_sheet_cells: 100_000,
        ..small_limits()
    };
    let p = shared_ref_pkg(10_000, 200).write(&dir, "a.xlsx");
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert!(s.rows_truncated);
    // Each kept cell costs 10,000 + 32 bytes: 1 MiB holds 104 of them.
    let budget = limits.max_sheet_text_bytes;
    assert_eq!(s.nrows as u64, budget / 10_032);
    assert_eq!(s.display(s.nrows - 1, 0).len(), 10_000);
    // The same string used by few cells loads whole.
    let p = shared_ref_pkg(10_000, 50).write(&dir, "b.xlsx");
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert!(!s.rows_truncated);
    assert_eq!(s.display(49, 0).len(), 10_000);
}

#[test]
fn the_text_budget_boundary_is_exact_for_shared_strings() {
    // Each kept reference costs len + 32 against the sheet's text budget (the table itself is
    // checked separately, before the file is opened, against `max_text_bytes`).
    let dir = tmp("xlsx_text_boundary");
    let len = 1000usize;
    let mk = |budget| Limits {
        max_sheet_text_bytes: budget,
        ..small_limits()
    };
    let p = shared_ref_pkg(len, 3).write(&dir, "e.xlsx");
    let rows_at = |budget| {
        let s = &load_with(&p, mk(budget)).unwrap().sheets[0];
        (s.nrows, s.rows_truncated)
    };
    let one = len as u64 + 32;
    assert_eq!(rows_at(3 * one), (3, false), "exactly fits");
    assert_eq!(
        rows_at(3 * one - 1),
        (2, true),
        "one byte short of the third"
    );
    assert_eq!(rows_at(2 * one), (2, true));
    assert_eq!(rows_at(one - 1), (0, true), "not even the first");
}

#[test]
fn the_shared_strings_table_is_checked_against_its_own_limit() {
    // The table is the one thing `calamine` loads whole when it opens the file: it is refused
    // before opening when it is over `max_text_bytes` (table cost = len + 32 per string), whatever
    // the sheets hold.
    let dir = tmp("xlsx_sst_boundary");
    let len = 1000usize;
    let p = shared_ref_pkg(len, 1).write(&dir, "t.xlsx");
    let mk = |budget| Limits {
        max_text_bytes: budget,
        ..small_limits()
    };
    let exact = len as u64 + 32;
    assert!(load_with(&p, mk(exact)).is_ok());
    assert_eq!(
        load_with(&p, mk(exact - 1)).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
}

#[test]
fn a_shared_strings_table_that_alone_exceeds_the_budget_is_refused() {
    let dir = tmp("xlsx_sst_alone");
    let p = Pkg::new()
        .shared(shared_xml(&["y".repeat(2 << 20)]))
        .sheet("S", "visible", sheet_xml(&num("A1", None, "1"), ""))
        .write(&dir, "t.xlsx");
    assert_eq!(
        load_with(&p, roomy_parts()).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
}

#[test]
fn a_forged_unique_count_is_refused_before_calamine_reserves_for_it() {
    // calamine does `strings.reserve(uniqueCount)`: 4 billion x 24 bytes.
    let dir = tmp("xlsx_unique_count");
    let sst = format!(
        r#"<?xml version="1.0"?><sst xmlns="{NS}" count="1" uniqueCount="4000000000"><si><t>a</t></si></sst>"#
    );
    let p = Pkg::new()
        .shared(sst)
        .sheet("S", "visible", sheet_xml(&num("A1", None, "1"), ""))
        .write(&dir, "u.xlsx");
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
}

#[test]
fn inline_strings_and_formula_text_count_against_the_sheet_text_budget() {
    let dir = tmp("xlsx_inline_text");
    let limits = Limits {
        max_rows: 1000,
        max_sheet_cells: 100_000,
        ..roomy_parts()
    };
    let big = "z".repeat(10_000);
    let rows: String = (1..=200)
        .map(|i| {
            format!(
                r#"<row r="{i}">{}</row>"#,
                istr(&format!("A{i}"), None, &big)
            )
        })
        .collect();
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "i.xlsx");
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert!(s.rows_truncated);
    assert_eq!(s.nrows as u64, limits.max_sheet_text_bytes / 10_032);
    // The text result of a formula (`t="str"`) is text too; a long numeric `<v>` is not.
    let rows: String = (1..=200)
        .map(|i| format!(r#"<row r="{i}"><c r="A{i}" t="str"><f>1</f><v>{big}</v></c></row>"#))
        .collect();
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "s.xlsx");
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert!(s.rows_truncated);
    assert!(s.nrows < 200);
    // A shared formula: one master with 2 KB of text, 600 cells derived from it (each gets its
    // own copy of the translated text in the reader and in the loader): 600 x 2 KB is over the
    // 1 MiB budget, so the sheet is cut where the formulas stop fitting.
    let master = format!(
        r#"<c r="A1"><f t="shared" ref="A1:A600" si="0">{}</f><v>1</v></c>"#,
        "1+".repeat(1000) + "1"
    );
    let derived: String = (2..=600)
        .map(|i| format!(r#"<row r="{i}"><c r="A{i}"><f t="shared" si="0"/><v>1</v></c></row>"#))
        .collect();
    let rows = format!(r#"<row r="1">{master}</row>{derived}"#);
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "f.xlsx");
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert!(s.rows_truncated);
    assert!(s.nrows > 100 && s.nrows < 600, "{}", s.nrows);
    assert!(s.formula(s.nrows - 1, 0).is_some());
}

#[test]
fn dxfs_number_formats_do_not_leak_into_the_cell_formats() {
    // The same id 164 is defined in `<numFmts>` (the cell format) and, differently, in `<dxfs>`
    // (a conditional-formatting override). Only the first is the file's format table.
    let xml = format!(
        r#"<?xml version="1.0"?><styleSheet xmlns="{NS}">
        <dxfs count="1"><dxf><numFmt numFmtId="164" formatCode="0%"/></dxf></dxfs>
        <numFmts count="1"><numFmt numFmtId="164" formatCode="0.00"/></numFmts>
        <cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="164"/></cellXfs></styleSheet>"#
    );
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(st.formats[usize::from(st.xf_to_format[1])], custom("0.00"));
    // And with `<dxfs>` after `<cellXfs>` and no `<numFmts>`: id 164 is then undefined.
    let xml = format!(
        r#"<?xml version="1.0"?><styleSheet xmlns="{NS}">
        <cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="164"/></cellXfs>
        <dxfs count="1"><dxf><numFmt numFmtId="164" formatCode="0%"/></dxf></dxfs></styleSheet>"#
    );
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(
        st.formats[usize::from(st.xf_to_format[1])],
        NumFmtRef::General
    );
}

fn column_sheet(rows: usize) -> String {
    let r: String = (1..=rows)
        .map(|i| format!(r#"<row r="{i}">{}</row>"#, num(&format!("A{i}"), None, "1")))
        .collect();
    sheet_xml(&r, "")
}

#[test]
fn the_cell_budget_is_per_sheet_so_every_sheet_of_a_big_workbook_is_shown() {
    // The old budget was one for the whole workbook (handed out in sheet order: the third of
    // three big sheets came out empty). Now each sheet has its own, and only the one on screen
    // is held anyway.
    let dir = tmp("xlsx_wb_budget");
    let limits = Limits {
        max_rows: 1000,
        max_sheet_cells: 1000,
        ..small_limits()
    };
    let p = Pkg::new()
        .sheet("A", "visible", column_sheet(600))
        .sheet("B", "visible", column_sheet(600))
        .sheet("C", "visible", column_sheet(600))
        .write(&dir, "wb.xlsx");
    let wb = load_with(&p, limits).unwrap();
    let rows: Vec<usize> = wb.sheets.iter().map(|s| s.nrows).collect();
    assert_eq!(rows, vec![600, 600, 600]);
    assert!(wb.sheets.iter().all(|s| !s.rows_truncated));
    // A sheet over its own budget is cut, the others are not.
    let p = Pkg::new()
        .sheet("A", "visible", column_sheet(300))
        .sheet("B", "visible", column_sheet(1500))
        .sheet("C", "visible", column_sheet(300))
        .write(&dir, "ok.xlsx");
    let wb = load_with(&p, limits).unwrap();
    let cut: Vec<(usize, bool)> = wb
        .sheets
        .iter()
        .map(|s| (s.nrows, s.rows_truncated))
        .collect();
    assert_eq!(cut, vec![(300, false), (1000, true), (300, false)]);
}

#[test]
fn a_hidden_sheets_size_never_refuses_the_workbook() {
    // The old format pass parsed hidden sheets too (it parsed every sheet) and refused the whole
    // workbook for the size of one nobody can see. Only the sheet asked for is read now.
    let dir = tmp("xlsx_hidden_big");
    let huge: String = (1..=1000)
        .map(|r| {
            format!(
                r#"<row r="{r}">{}</row>"#,
                istr(&format!("A{r}"), None, &"h".repeat(5000))
            )
        })
        .collect();
    let far = format!(
        r#"<row r="1">{}</row><row r="1048576">{}</row>"#,
        num("A1", None, "1"),
        num("XFD1048576", None, "2")
    );
    let ok = format!(r#"<row r="1">{}</row>"#, num("A1", None, "7"));
    let p = Pkg::new()
        .sheet("Visible", "visible", sheet_xml(&ok, ""))
        .sheet("HiddenText", "hidden", sheet_xml(&huge, ""))
        .sheet("HiddenFar", "veryHidden", sheet_xml(&far, ""))
        .write(&dir, "h.xlsx");
    let limits = Limits {
        max_sheet_text_bytes: 1000,
        max_sheet_cells: 10,
        ..roomy_parts()
    };
    let wb = load_with(&p, limits).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    assert_eq!(wb.hidden_sheets, 2);
    assert_eq!(wb.sheets[0].display(0, 0), "7");
    // One sheet only, as the application asks for it: the other sheet's cells are not even read.
    let one = load_workbook_sheet(
        &p,
        &LoadOptions {
            limits,
            ..LoadOptions::default()
        },
        0,
    )
    .unwrap();
    assert_eq!(one.sheets[0].display(0, 0), "7");
}

#[test]
fn a_text_cell_shown_through_a_repeating_format_is_cut_at_the_display_cap() {
    // A text format of 255 `@` repeats the text 255 times; a 32,767-character cell would then
    // display 8 MB. A displayed string is cut at 1,024 characters, and what is kept is what the
    // budget counts.
    let dir = tmp("xlsx_display_cap");
    let code = "@".repeat(255);
    let rows = format!(
        r#"<row r="1">{}{}</row>"#,
        istr("A1", Some(1), "hello"),
        istr("B1", Some(1), &"x".repeat(30_000)),
    );
    let p = Pkg::new()
        .styles(styles_xml(&[(164, &code)], &[0, 164]))
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "cap.xlsx");
    let limits = Limits {
        max_sheet_text_bytes: 1 << 20,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!(
        s.display(0, 0).chars().count(),
        1024,
        "5 x 255 = 1,275 characters, cut"
    );
    assert!(s.display(0, 0).starts_with("hellohello"));
    assert_eq!(s.display(0, 1).chars().count(), 1024);
    // The raw value is whole.
    assert_eq!(s.cell(0, 1).unwrap().raw_text().len(), 30_000);
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
    assert_eq!(l.max_sheet_cells, 4_000_000);
    assert_eq!(l.max_rows, crate::preview::table::MAX_ROWS);
    assert_eq!(l.max_cols, 16_384);
    assert_eq!(l.max_sheet_text_bytes, 128 << 20);
    assert_eq!(l.max_dense_cells, 8_000_000);
    assert_eq!(l.max_text_bytes, 256 << 20);
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
    match std::panic::catch_unwind(move || load_workbook_unguarded(&p, &opts)) {
        // A panic inside one sheet is caught per sheet (the workbook stays usable) and reported
        // as this `Corrupt`; for a test it is still a panic.
        Ok(Err(OfficeError::Corrupt(m))) if m.contains("crashed") => {
            panic!("the loader panicked on {what}: {m}")
        }
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
        // konoma's own code (container scan + xlsx reader) must never panic, uncontained.
        let inspected = std::panic::catch_unwind(|| {
            let _ = container::inspect(&p, &small_limits());
            if let Ok(mut book) = xlsx::open(&p, &small_limits()) {
                for sheet in book.sheets.clone() {
                    let _ = book.read_sheet(&sheet.part, &small_limits(), None, |_| true);
                }
            }
        });
        assert!(
            inspected.is_ok(),
            "container/format pass panicked in round {round}"
        );
        // The whole load is contained and must answer Ok or Err.
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

// ---------------------------------------------------------------------------------------------
// mutation survivors: container, xlsx styles and budgets
// ---------------------------------------------------------------------------------------------

/// Sets bit 0 of the general-purpose flag of every entry (local headers and central directory):
/// "the file is encrypted" (ZIP APPNOTE 4.4.4).
fn set_encrypted_flag(mut z: Vec<u8>) -> Vec<u8> {
    let mut i = 0;
    while i + 30 < z.len() {
        if z[i..i + 4] == [0x50, 0x4b, 0x03, 0x04] {
            z[i + 6] |= 1;
            i += 4;
        } else if z[i..i + 4] == [0x50, 0x4b, 0x01, 0x02] {
            z[i + 8] |= 1;
            i += 4;
        } else {
            i += 1;
        }
    }
    z
}

#[test]
fn the_package_total_limit_is_exact() {
    let dir = tmp("totalexact");
    // 11 + 100 expanded bytes.
    let p = write(
        &dir,
        "t.xlsx",
        &deflated(&[("xl/workbook.xml", b"<workbook/>"), ("a.bin", &[7u8; 100])]),
    );
    let at = |total: u64| {
        container::inspect(
            &p,
            &Limits {
                max_total_bytes: total,
                ..small_limits()
            },
        )
    };
    assert_eq!(at(111), Ok(Detected::Xlsx));
    assert_eq!(at(110), Err(OfficeError::TooLarge { what: "package" }));
    assert_eq!(at(1000), Ok(Detected::Xlsx));
}

#[test]
fn an_entry_with_the_encrypted_flag_is_encrypted_even_when_it_is_also_too_large() {
    // The flag is checked before the size, so a password-protected part is reported as such and
    // not as "too large".
    let dir = tmp("zipflag");
    let z = set_encrypted_flag(deflated(&[("xl/workbook.xml", &[b'x'; 4096])]));
    let p = write(&dir, "e.xlsx", &z);
    let tiny = Limits {
        max_part_bytes: 1000,
        ..small_limits()
    };
    assert_eq!(
        container::inspect(&p, &tiny).unwrap_err(),
        OfficeError::Encrypted
    );
    assert_eq!(load(&p).unwrap_err(), OfficeError::Encrypted);
    // Without the flag the same package is just too large for the tiny limit.
    let plain = write(
        &dir,
        "p.xlsx",
        &deflated(&[("xl/workbook.xml", &[b'x'; 4096])]),
    );
    assert_eq!(
        container::inspect(&plain, &tiny).unwrap_err(),
        OfficeError::TooLarge { what: "entry" }
    );
}

#[test]
fn zip_errors_map_to_office_errors() {
    use zip::result::ZipError as Z;
    let enc = OfficeError::Encrypted;
    assert_eq!(container::zip_err(Z::InvalidPassword), enc);
    assert_eq!(
        container::zip_err(Z::UnsupportedArchive(Z::PASSWORD_REQUIRED)),
        enc
    );
    // The match on the message ignores case.
    assert_eq!(
        container::zip_err(Z::UnsupportedArchive("PASSWORD needed")),
        enc
    );
    // Any other unsupported feature is "not ours", not "encrypted".
    assert_eq!(
        container::zip_err(Z::UnsupportedArchive("multi-disk archive")),
        OfficeError::Unsupported
    );
    assert_eq!(
        container::zip_err(Z::UnsupportedArchive("x")),
        OfficeError::Unsupported
    );
    // Damage is corruption.
    assert!(matches!(
        container::zip_err(Z::FileNotFound),
        OfficeError::Corrupt(_)
    ));
    assert!(matches!(
        container::zip_err(Z::InvalidArchive("bad".into())),
        OfficeError::Corrupt(_)
    ));
    assert!(matches!(
        container::zip_err(Z::Io(std::io::Error::other("boom"))),
        OfficeError::Corrupt(_)
    ));
    assert!(matches!(
        container::io_err(std::io::Error::other("boom")),
        OfficeError::Io(_)
    ));
}

#[test]
fn a_zip_is_detected_by_its_marker_parts() {
    let dir = tmp("detect");
    let case = |name: &str, entries: &[(&str, &[u8])]| {
        let p = write(&dir, name, &deflated(entries));
        container::inspect(&p, &Limits::default())
    };
    assert_eq!(
        case("a.zip", &[("xl/workbook.xml", b"x")]),
        Ok(Detected::Xlsx)
    );
    assert_eq!(
        case("b.zip", &[("xl/workbook.bin", b"x")]),
        Ok(Detected::Xlsb)
    );
    // OpenDocument needs both the `mimetype` entry and `content.xml`.
    assert_eq!(
        case("c.zip", &[("mimetype", b"x"), ("content.xml", b"x")]),
        Ok(Detected::Ods)
    );
    assert_eq!(
        case("d.zip", &[("content.xml", b"x")]),
        Err(OfficeError::Unsupported)
    );
    assert_eq!(
        case("e.zip", &[("mimetype", b"x")]),
        Err(OfficeError::Unsupported)
    );
    assert_eq!(
        case("f.zip", &[("other", b"x")]),
        Err(OfficeError::Unsupported)
    );
    // The xlsx marker wins over ods ones.
    assert_eq!(
        case(
            "g.zip",
            &[
                ("xl/workbook.xml", b"x"),
                ("mimetype", b"x"),
                ("content.xml", b"x")
            ]
        ),
        Ok(Detected::Xlsx)
    );
}

#[test]
fn a_chartsheet_in_an_xlsx_is_neither_listed_nor_counted_as_hidden() {
    // The sheet kind comes from the relationship type (`.../chartsheet`); a chart sheet has no
    // cells and must not reach the grid.
    let dir = tmp("chartsheet");
    let wb = workbook_xml(
        false,
        &[
            ("Data", "visible", "rId1"),
            ("Chart1", "visible", "rId2"),
            ("More", "visible", "rId3"),
            ("HiddenChart", "hidden", "rId4"),
        ],
    );
    let rel = |id: &str, ty: &str, target: &str| {
        format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{ty}" Target="{target}"/>"#
        )
    };
    let rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}{}{}{}</Relationships>"#,
        rel("rId1", "worksheet", "worksheets/sheet1.xml"),
        rel("rId2", "chartsheet", "chartsheets/sheet1.xml"),
        rel("rId3", "worksheet", "worksheets/sheet2.xml"),
        rel("rId4", "chartsheet", "chartsheets/sheet2.xml"),
    );
    let root_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let one = |t: &str| sheet_xml(&format!(r#"<row r="1">{}</row>"#, istr("A1", None, t)), "");
    let chart = format!(r#"<?xml version="1.0"?><chartsheet xmlns="{NS}"/>"#);
    let z = deflated(&[
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", wb.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", one("first").as_bytes()),
        ("xl/worksheets/sheet2.xml", one("second").as_bytes()),
        ("xl/chartsheets/sheet1.xml", chart.as_bytes()),
        ("xl/chartsheets/sheet2.xml", chart.as_bytes()),
    ]);
    let p = write(&dir, "chart.xlsx", &z);
    let wb = load(&p).unwrap();
    let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Data", "More"]);
    assert_eq!(
        wb.hidden_sheets, 0,
        "a chart sheet is not a hidden worksheet"
    );
    assert_eq!(wb.sheets[1].display(0, 0), "second");
}

#[test]
fn the_bounding_box_of_an_xlsx_sheet_is_not_limited_because_it_is_never_built() {
    // Two values at A1 and C3 (a 3 x 3 box) with `max_dense_cells` of 1: xlsx is streamed, so
    // the limit that guards the dense matrices of ods / xls does not apply to it.
    let dir = tmp("denseexact");
    let rows = format!(
        r#"<row r="1">{}</row><row r="3">{}</row>"#,
        num("A1", None, "1"),
        num("C3", None, "2")
    );
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "d.xlsx");
    let wb = load_with(
        &p,
        Limits {
            max_dense_cells: 1,
            ..small_limits()
        },
    )
    .unwrap();
    assert_eq!((wb.sheets[0].nrows, wb.sheets[0].ncols), (3, 3));
    assert_eq!(wb.sheets[0].display(2, 2), "2");
}

#[test]
fn the_value_cell_limit_is_exact() {
    let rows: String = (1..=10)
        .map(|r| format!(r#"<row r="{r}">{}</row>"#, num(&format!("A{r}"), None, "1")))
        .collect();
    let at = |n: u64| {
        read_rows(
            &rows,
            Limits {
                max_sheet_cells: n,
                ..Limits::default()
            },
        )
    };
    let exact = at(10);
    assert_eq!(kept(&exact), 10);
    assert!(
        !exact.rows_truncated,
        "10 cells in a budget of 10 is not cut"
    );
    let over = at(9);
    assert_eq!(kept(&over), 9);
    assert!(over.rows_truncated);
}

#[test]
fn note_value_of_the_binary_formats_has_exact_limits() {
    // Columns: only those inside max_cols keep their format (the cell is still in the box).
    let limits = Limits {
        max_cols: 4,
        max_dense_cells: 3,
        ..Limits::default()
    };
    let mut sf = SheetFormats::default();
    sf.note_value(0, 3, 7, &limits).unwrap(); // last shown column
    sf.note_value(0, 4, 7, &limits).unwrap(); // first column past the maximum
    sf.note_value(1, 0, 0, &limits).unwrap(); // General is never recorded
    assert_eq!(sf.cells, vec![(0, 3, 7)]);
    assert_eq!(sf.bbox, Some((0, 0, 1, 4)));
    assert_eq!(sf.value_cells, 3);
    // The 4th value cell is one over a budget of 3.
    assert_eq!(
        sf.note_value(2, 0, 0, &limits),
        Err(OfficeError::TooLarge {
            what: "sheet cells"
        })
    );
}

#[test]
fn the_last_builtin_number_format_id_is_163() {
    // ECMA-376 18.8.30: ids below 164 are built in, custom formats start at 164.
    let xml = styles_xml(&[], &[162, 163, 164, 165]);
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    let fmt = |xf: usize| st.formats[st.xf_to_format[xf] as usize].clone();
    assert_eq!(fmt(0), NumFmtRef::Builtin(162));
    assert_eq!(fmt(1), NumFmtRef::Builtin(163));
    assert_eq!(fmt(2), NumFmtRef::General, "undefined 164 is General");
    assert_eq!(fmt(3), NumFmtRef::General);
}

#[test]
fn a_custom_general_code_is_general_whatever_its_spacing_or_case() {
    // LibreOffice writes `General` as a custom code; padding and case do not matter.
    for code in ["General", "GENERAL", "general", " General ", "\tgeneral\n"] {
        let xml = styles_xml(&[(164, code)], &[164]);
        let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
        assert_eq!(
            st.formats[st.xf_to_format[0] as usize],
            NumFmtRef::General,
            "{code:?}"
        );
    }
    // But a code that merely contains the word is a code.
    let xml = styles_xml(&[(164, "General\\ 0")], &[164]);
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert!(matches!(
        st.formats[st.xf_to_format[0] as usize],
        NumFmtRef::Custom(_)
    ));
}

#[test]
fn when_a_numfmt_id_is_defined_twice_the_later_definition_wins() {
    // Not specified by ECMA-376 (ids are meant to be unique); this engine keeps the last one it
    // read, as a map insert does, and the choice is pinned here so it cannot change silently.
    let xml = styles_xml(&[(164, "0.0"), (164, "0.000")], &[164]);
    let st = fmt_xlsx::parse_styles(xml.as_bytes()).unwrap();
    assert_eq!(
        st.formats[st.xf_to_format[0] as usize],
        NumFmtRef::Custom("0.000".into())
    );
}

#[test]
fn the_last_column_of_the_cap_keeps_its_format_and_the_first_past_it_does_not() {
    // Columns A..D with a cap of 4: D (index 3) is shown, E (index 4) is past the cap.
    let rows = r#"<row r="1"><c r="D1" s="1"><v>1</v></c><c r="E1" s="1"><v>1</v></c></row>"#;
    let s = read_rows(
        rows,
        Limits {
            max_cols: 4,
            ..Limits::default()
        },
    );
    assert_eq!(formatted(&s), vec![(0, 3, 1)]);
}

#[test]
fn a_grid_exactly_max_cols_wide_is_not_truncated() {
    let dir = tmp("colexact");
    let cells: String = (0..5u32)
        .map(|c| num(&format!("{}1", (b'A' + c as u8) as char), None, "1"))
        .collect();
    let p = Pkg::new()
        .sheet(
            "S",
            "visible",
            sheet_xml(&format!(r#"<row r="1">{cells}</row>"#), ""),
        )
        .write(&dir, "c.xlsx");
    let at = |cols: usize| {
        load_with(
            &p,
            Limits {
                max_cols: cols,
                ..small_limits()
            },
        )
        .unwrap()
    };
    let s = &at(5).sheets[0];
    assert_eq!(s.ncols, 5);
    assert!(!s.cols_truncated, "exactly at the cap is not truncated");
    let s = &at(4).sheets[0];
    assert_eq!(s.ncols, 4);
    assert!(s.cols_truncated);
}

#[test]
fn formulas_outside_the_cut_grid_are_dropped() {
    // A 3 x 3 sheet where every cell has a formula, cut to 2 x 2: the formula of a row or column
    // that is cut off is not kept (the first column / row past the grid included).
    let dir = tmp("formulacut");
    let rows: String = (1..=3)
        .map(|r| {
            let cells: String = ["A", "B", "C"]
                .iter()
                .map(|c| format!(r#"<c r="{c}{r}"><f>{c}{r}+1</f><v>1</v></c>"#))
                .collect();
            format!(r#"<row r="{r}">{cells}</row>"#)
        })
        .collect();
    let p = Pkg::new()
        .sheet("S", "visible", sheet_xml(&rows, ""))
        .write(&dir, "f.xlsx");
    let limits = Limits {
        max_rows: 2,
        max_cols: 2,
        ..small_limits()
    };
    let s = &load_with(&p, limits).unwrap().sheets[0];
    assert_eq!((s.nrows, s.ncols), (2, 2));
    assert_eq!(s.formula(1, 1), Some("B2+1"));
    assert_eq!(s.formula(0, 2), None, "first column past the cap");
    assert_eq!(s.formula(2, 0), None, "first row past the cap");
    assert_eq!(s.formula(2, 2), None);
    assert_eq!(s.formula(5, 5), None);
    // Uncut, all nine are there.
    let s = &load_with(&p, small_limits()).unwrap().sheets[0];
    assert_eq!(s.formula(2, 2), Some("C3+1"));
}

// ---------------------------------------------------------------------------------------------
// the stop point of the value reader, and what counts as "cut off"
// ---------------------------------------------------------------------------------------------

fn single_sheet(dir: &TmpDir, file: &str, rows: &str) -> PathBuf {
    Pkg::new()
        .styles(styles_xml(&[], &[0, 14]))
        .sheet("S", "visible", sheet_xml(rows, ""))
        .write(dir, file)
}

fn read_rows_capped(rows: &str, max_rows: usize) -> Sheet {
    read_rows(
        rows,
        Limits {
            max_rows,
            ..Limits::default()
        },
    )
}

/// The sheet the stop-point tests share: every shape of `<c>` the value reader returns (empty
/// element, empty pair, formula only, inline string, value, no `r`), an empty `<row/>`, and then
/// data past the row cap of 4.
fn mixed_cells_rows() -> String {
    [
        r#"<row r="1"><c r="A1" s="1"/><c r="B1" s="1"></c><c r="C1"><v>5</v></c><c r="D1" t="inlineStr"><is><t>x</t></is></c><c r="E1"><f>1+1</f></c></row>"#,
        r#"<row r="2"><c r="A2" s="1"/><c r="B2" s="1"/><c r="C2" s="1"/></row>"#,
        r#"<row r="3"/>"#,
        r#"<row r="4"><c s="1"><v>1</v></c><c s="1"/></row>"#,
        r#"<row r="5"><c r="A5" s="1"/><c r="B5"><v>9</v></c></row>"#,
    ]
    .concat()
}

/// Rows above the cap are shown with their formats whatever shapes of `<c>` they hold (empty
/// element, empty pair, formula only, inline string, value, no `r`), and the sheet says it is cut
/// when a later row holds a value.
#[test]
fn the_rows_above_the_cap_are_shown_with_their_formats() {
    let rows = mixed_cells_rows();
    let dir = tmp("stop_vs_calamine");
    let p = single_sheet(&dir, "m.xlsx", &rows);
    let wb = load_with(
        &p,
        Limits {
            max_rows: 4,
            ..Limits::default()
        },
    )
    .unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.nrows, 4);
    assert_eq!(s.display(0, 2), "5");
    // (xf 1 is a date format: the cell with no `r` is read, and shown with its format.)
    assert_eq!(s.display(3, 0), "1/1/1900");
    assert!(
        s.rows_truncated,
        "B5 holds a value that is not shown: cut off"
    );
}

#[test]
fn every_shape_of_cell_before_the_cap_is_read_and_the_data_after_it_is_a_cut() {
    // Each shape alone, with the cap right before row 3: whatever the shape, the sheet keeps rows
    // 1-2 and reports the value of row 3 as cut off.
    let shapes = [
        r#"<c r="A1" s="1"/>"#,
        r#"<c r="A1" s="1"></c>"#,
        r#"<c r="A1"><f>1+1</f></c>"#,
        r#"<c r="A1" t="inlineStr"><is><t>x</t></is></c>"#,
        r#"<c r="A1"><v>3</v></c>"#,
        r#"<c r="A1"><f>1</f><v>3</v></c>"#,
        r#"<c r="A1"><v></v></c>"#,
        r#"<c r="A1"><v/></c>"#,
        r#"<c s="1"/>"#,
    ];
    for shape in shapes {
        let rows = format!(
            r#"<row r="1">{shape}{shape}</row><row r="2">{shape}</row><row r="3"><c r="A3"><v>1</v></c></row>"#
        );
        let s = read_rows_capped(&rows, 2);
        assert!(s.rows_truncated, "row 3 has a value: {shape}");
        assert!(s.nrows <= 2, "{shape}");
        assert!(s.cell(2, 0).is_none(), "{shape}");
    }
}

/// Formatted blank cells and empty rows past the cap are not data: nothing is cut off, so no
/// `(capped)` marker.
#[test]
fn a_tail_of_formatted_blanks_and_empty_rows_is_not_a_cut() {
    let mut rows = String::new();
    for r in 1..=3 {
        rows += &format!(r#"<row r="{r}">{}</row>"#, num(&format!("A{r}"), None, "1"));
    }
    for r in 4..=50 {
        rows += &format!(r#"<row r="{r}"><c r="A{r}" s="1"/><c r="B{r}" s="1"></c></row>"#);
    }
    rows += r#"<row r="51"/><row r="52" ht="15"/>"#;
    let s = read_rows_capped(&rows, 3);
    assert!(!s.rows_truncated, "nothing past the cap holds data");
    assert_eq!(s.nrows, 3);
    // The same tail with one value in it is a cut.
    let with_data = rows.replace(
        r#"<c r="B30" s="1"></c>"#,
        r#"<c r="B30" s="1"><v>7</v></c>"#,
    );
    assert!(read_rows_capped(&with_data, 3).rows_truncated);
    // A formula, an inline string and a `<v>` with text each count as data; an empty `<v>` not.
    for (cell, data) in [
        (r#"<c r="B30"><f>1+1</f></c>"#, true),
        (r#"<c r="B30" t="inlineStr"><is><t>x</t></is></c>"#, true),
        (r#"<c r="B30"><v>7</v></c>"#, true),
        (r#"<c r="B30"><v></v></c>"#, false),
        (r#"<c r="B30"><v/></c>"#, false),
        (r#"<c r="B30" s="1"/>"#, false),
    ] {
        let r = rows.replace(r#"<c r="B30" s="1"></c>"#, cell);
        assert_eq!(read_rows_capped(&r, 3).rows_truncated, data, "{cell}");
    }
}

/// The report that started this: a sheet where every row ends in styled blanks. The display has
/// to reach the row cap.
#[test]
fn styled_blanks_do_not_stop_the_display_before_the_row_cap() {
    let rows: String = (1..=200)
        .map(|r| {
            format!(
                r#"<row r="{r}">{}<c r="B{r}" s="1"/><c r="C{r}" s="1"/></row>"#,
                num(&format!("A{r}"), None, &r.to_string())
            )
        })
        .collect();
    let dir = tmp("blanks_cap");
    let p = single_sheet(&dir, "b.xlsx", &rows);
    let wb = load_with(
        &p,
        Limits {
            max_rows: 100,
            ..Limits::default()
        },
    )
    .unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.nrows, 100);
    assert_eq!(
        s.display(99, 0),
        "100",
        "the last row under the cap is read"
    );
    assert!(s.rows_truncated, "rows 101.. do hold values");
}

/// A formula-only cell is not a kept cell, so a sheet with many of them must not use up the cell
/// budget: the value cells after them are still shown, with their format.
#[test]
fn formula_only_cells_do_not_use_up_the_cell_budget() {
    let mut rows = String::new();
    for r in 1..=30 {
        rows += &format!(r#"<row r="{r}"><c r="A{r}" s="1"><f>1+1</f></c></row>"#);
    }
    for r in 31..=35 {
        rows += &format!(
            r#"<row r="{r}">{}</row>"#,
            num(&format!("A{r}"), Some(1), "46297")
        );
    }
    let limits = Limits {
        max_sheet_cells: 10,
        ..Limits::default()
    };
    let s = read_rows(&rows, limits);
    assert!(!s.rows_truncated);
    assert_eq!(kept(&s), 5);
    assert_eq!(formatted(&s).len(), 5, "the five dates keep their format");
    assert_ne!(s.display(34, 0), "46297", "formatted, not General");
    assert_eq!(s.formula(0, 0), Some("1+1"), "the formulas are still kept");
}

/// When the cell budget does cut the sheet, exactly the budget is kept and every kept cell is
/// shown with its format.
#[test]
fn the_cell_budget_cuts_the_sheet_and_every_kept_cell_has_its_format() {
    let mut rows = String::new();
    for r in 1..=20 {
        rows += &format!(
            r#"<row r="{r}">{}{}<c r="C{r}" s="1"/></row>"#,
            num(&format!("A{r}"), Some(1), "46297"),
            num(&format!("B{r}"), Some(1), "46297")
        );
    }
    let limits = Limits {
        max_sheet_cells: 10,
        ..Limits::default()
    };
    let dir = tmp("budget_cut");
    let p = single_sheet(&dir, "c.xlsx", &rows);
    let wb = load_with(&p, limits).unwrap();
    let s = &wb.sheets[0];
    assert!(s.rows_truncated);
    assert_eq!(kept(s), 10);
    for r in 0..s.nrows {
        for (c, cell) in s.row_cells(r) {
            assert_ne!(cell.display(), "46297", "({r},{c}) shown with its format");
        }
    }
}

/// `<c>` elements outside `<sheetData>` (an extension list after it) are not cells.
#[test]
fn cells_outside_sheet_data_are_not_cells() {
    let xml = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS}"><sheetData><row r="1">{}</row></sheetData><extLst><ext><c r="A9" s="1"><v>1</v></c><c r="B9" s="1"><v>1</v></c></ext></extLst></worksheet>"#,
        num("A1", Some(1), "1")
    );
    let s = read_xml(&xml, &[0, 14], Limits::default());
    assert_eq!(kept(&s), 1);
    assert_eq!(formatted(&s), vec![(0, 0, 1)]);
    // An empty `<sheetData/>` is no data either.
    let xml = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS}"><sheetData/><c r="A1" s="1"><v>1</v></c></worksheet>"#
    );
    let s = read_xml(&xml, &[0, 14], Limits::default());
    assert_eq!(kept(&s), 0);
}

/// An absurd reference is a stop like a row past the cap, and it is a cut only when the cell holds
/// data.
#[test]
fn an_absurd_reference_stops_the_reader_and_cuts_only_when_data_holds() {
    let with_value = format!(
        r#"<row r="1">{}<c r="ZZZZZZZZZZZ1" s="1"><v>1</v></c></row>"#,
        num("A1", Some(1), "1")
    );
    let s = read_rows_capped(&with_value, 100);
    assert_eq!(kept(&s), 1);
    assert!(s.rows_truncated);
    let blank = format!(
        r#"<row r="1">{}<c r="ZZZZZZZZZZZ1" s="1"/></row>"#,
        num("A1", Some(1), "1")
    );
    let s = read_rows_capped(&blank, 100);
    assert_eq!(kept(&s), 1);
    assert!(!s.rows_truncated);
}

/// A real xlsx read with a cancel that fires after a few rows: the sheet comes back cut short
/// (the result is thrown away by the caller), and a cancel that never fires changes nothing.
#[test]
fn a_cancelled_xlsx_load_stops_early_and_an_uncancelled_one_is_complete() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let rows: String = (1..=500)
        .map(|r| format!(r#"<row r="{r}">{}</row>"#, num(&format!("A{r}"), None, "1")))
        .collect();
    let dir = tmp("cancel_xlsx");
    let p = single_sheet(&dir, "c.xlsx", &rows);
    let opts = LoadOptions::default();
    let full = workbook::load_workbook_sheet_cancellable(&p, &opts, 0, None).unwrap();
    assert_eq!(full.sheets[0].nrows, 500);
    let never = Cancel::new(|| false);
    let same = workbook::load_workbook_sheet_cancellable(&p, &opts, 0, Some(never)).unwrap();
    assert_eq!(same.sheets[0].nrows, 500);
    let seen = Arc::new(AtomicUsize::new(0));
    let s2 = seen.clone();
    let cancel = Cancel::new(move || s2.fetch_add(1, Ordering::SeqCst) >= 10);
    let cut = workbook::load_workbook_sheet_cancellable(&p, &opts, 0, Some(cancel)).unwrap();
    // One check before the sheet is read, then one per row: the 11th check is the 10th row's.
    assert_eq!(cut.sheets[0].nrows, 9, "stopped at the 10th row");
    assert_eq!(
        seen.load(Ordering::SeqCst),
        11,
        "not asked again once it said stop"
    );
    // Already cancelled before the sheet is read: the sheet list, no cells.
    let cut = workbook::load_workbook_sheet_cancellable(&p, &opts, 0, Some(Cancel::new(|| true)))
        .unwrap();
    assert_eq!(cut.sheets.len(), 1);
    assert!(!cut.sheets[0].loaded);
}

// ---------------------------------------------------------------------------------------------
// what the sheet keeps
// ---------------------------------------------------------------------------------------------

#[test]
fn an_entity_reference_alone_in_v_is_a_value_for_a_string_cell_and_data_past_a_cap() {
    let limits = Limits {
        max_rows: 1,
        ..Limits::default()
    };
    // `&gt;` arrives as a `GeneralRef`, not as text.
    let s = read_xml(
        &sheet_xml(
            r#"<row r="1"><c r="A1" s="1" t="str"><v>&gt;</v></c></row>"#,
            "",
        ),
        &[0, 14],
        limits,
    );
    assert_eq!(kept(&s), 1);
    assert_eq!(s.display(0, 0), ">");
    assert_eq!(formatted(&s), vec![(0, 0, 1)]);
    assert!(!s.rows_truncated);
    // The same cell past the row cap is data left out.
    let s = read_xml(
        &sheet_xml(
            r#"<row r="1"><c r="A1" t="str"><v>x</v></c></row><row r="2"><c r="A2" t="str"><v>&amp;</v></c></row>"#,
            "",
        ),
        &[0],
        limits,
    );
    assert!(s.rows_truncated);
    assert_eq!(kept(&s), 1);
}

#[test]
fn a_cell_is_kept_when_it_holds_a_value() {
    let count = |cell: &str| {
        kept(&read_rows(
            &format!(r#"<row r="1">{cell}</row>"#),
            Limits::default(),
        ))
    };
    // A string cell holds a string even when it is empty; an inline string too.
    assert_eq!(count(r#"<c r="A1" t="str"><v/></c>"#), 1);
    assert_eq!(count(r#"<c r="A1" t="inlineStr"><is/></c>"#), 1);
    // `<v>` of an inline string cell is ignored; a plain cell needs text first.
    assert_eq!(count(r#"<c r="A1" t="inlineStr"><v>5</v></c>"#), 0);
    assert_eq!(count(r#"<c r="A1"><v>&amp;</v></c>"#), 0);
    assert_eq!(count(r#"<c r="A1"><v/></c>"#), 0);
    assert_eq!(count(r#"<c r="A1"><f/></c>"#), 0);
    assert_eq!(count(r#"<c r="A1"><v>1</v></c>"#), 1);
    // The last value child wins.
    assert_eq!(
        count(r#"<c r="A1" t="inlineStr"><is><t>x</t></is><v>1</v></c>"#),
        0
    );
}

#[test]
fn a_cell_past_the_column_cap_does_not_use_the_cell_budget() {
    let limits = Limits {
        max_cols: 2,
        max_sheet_cells: 2,
        ..Limits::default()
    };
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c><c r="C1"><v>1</v></c><c r="D1"><v>1</v></c><c r="B1"><v>1</v></c></row>"#;
    let s = read_rows(rows, limits);
    assert_eq!(kept(&s), 2);
    assert!(!s.rows_truncated);
    assert!(s.cols_truncated);
}

#[test]
fn an_empty_formula_or_inline_string_past_the_cap_is_judged_like_a_value() {
    let past = |cell: &str| {
        read_rows_capped(
            &format!(r#"<row r="1"><c r="A1"><v>1</v></c></row><row r="2">{cell}</row>"#),
            1,
        )
        .rows_truncated
    };
    // An empty formula is nothing to keep; one with text is.
    assert!(!past(r#"<c r="A2"><f/></c>"#));
    assert!(!past(r#"<c r="A2"><f></f></c>"#));
    assert!(past(r#"<c r="A2"><f>1+1</f></c>"#));
    assert!(past(r#"<c r="A2"><f>A1&gt;0</f></c>"#));
    // A `<v>` an inline-string cell ignores is nothing.
    assert!(!past(r#"<c r="A2" t="inlineStr"><v>5</v></c>"#));
    assert!(past(r#"<c r="A2" t="inlineStr"><is><t>x</t></is></c>"#));
}

#[test]
fn rows_without_cells_are_checked_for_cancellation_too() {
    // 6000 rows that hold no cell (the builder is never asked), then a value: a cancelled load
    // ends within a thousand rows, an uncancelled one reaches the value.
    let mut rows = String::new();
    for r in 1..6000 {
        rows += &format!(r#"<row r="{r}"/>"#);
    }
    rows += r#"<row r="6000"><c r="A6000"><v>9</v></c></row>"#;
    let xml = sheet_xml(&rows, "");
    let limits = Limits::default();
    let tables = xlsx::Tables::default();
    let run = |cancel: Option<&workbook::Cancel>| {
        let mut cells = 0;
        xlsx::parse_sheet(xml.as_bytes(), &tables, &limits, cancel, |_| {
            cells += 1;
            true
        })
        .unwrap();
        cells
    };
    assert_eq!(run(None), 1, "the value at the end is found");
    let yes = workbook::Cancel::new(|| true);
    assert_eq!(run(Some(&yes)), 0, "a cancelled read stops early");
    let no = workbook::Cancel::new(|| false);
    assert_eq!(run(Some(&no)), 1);
}

#[test]
fn the_last_format_at_one_address_wins() {
    let cell = |s: u32| format!(r#"<c r="A1" s="{s}"><v>1</v></c>"#);
    let xf = [0, 14, 15];
    let sheet = |row: String| read_xml(&sheet_xml(&row, ""), &xf, Limits::default());
    // A later General cell replaces an earlier formatted one, and the reverse.
    let s = sheet(format!(r#"<row r="1">{}{}</row>"#, cell(1), cell(0)));
    assert_eq!(formatted(&s), vec![]);
    assert_eq!(kept(&s), 1);
    let s = sheet(format!(r#"<row r="1">{}{}</row>"#, cell(0), cell(1)));
    assert_eq!(formatted(&s), vec![(0, 0, 1)]);
    // Many cells at one address among others: whatever the sort does, the last in the file wins.
    let mut row = String::new();
    for i in 0..300u32 {
        row += &format!(r#"<c r="A1" s="{}"><v>1</v></c>"#, 1 + i % 2);
        row += &format!(r#"<c r="B1" s="{}"><v>1</v></c>"#, 2 - i % 2);
    }
    let s = sheet(format!(r#"<row r="1">{row}</row>"#));
    // The last A1 has i = 299: s = 2; the last B1: s = 1.
    assert_eq!(formatted(&s), vec![(0, 0, 2), (0, 1, 1)]);
}
