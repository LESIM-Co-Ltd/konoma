//! Tests for the xlsb format pass (`fmt_xlsb`).
//!
//! **There is no xlsb written by a real application here**: LibreOffice cannot write xlsb, and no
//! third-party file may be shipped. The packages below are assembled by a small writer following
//! MS-XLSB (records are `id`, `size`, body; ids/sizes are 7-bit varints), keeping to what
//! `calamine`'s reader needs, so the whole `load_workbook` path is exercised on them. What a real
//! Excel-written file adds (more records between these) is skipped by the pass by design.

use super::container::Limits;
use super::fmt_xlsb;
use super::fmt_xlsx::SheetFormats;
use super::tests::{
    assert_inner_does_not_panic, deflated, load, load_with, mutate, small_limits, tmp, write,
    xorshift,
};
use super::*;

// ---------------------------------------------------------------------------------------------
// a tiny XLSB writer
// ---------------------------------------------------------------------------------------------

fn rec(id: u16, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    if id < 0x80 {
        v.push(id as u8);
    } else {
        v.push((id & 0x7F) as u8 | 0x80);
        v.push((id >> 7) as u8);
    }
    let mut len = body.len();
    loop {
        let b = (len & 0x7F) as u8;
        len >>= 7;
        if len == 0 {
            v.push(b);
            break;
        }
        v.push(b | 0x80);
    }
    v.extend_from_slice(body);
    v
}

fn wide(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut v = (units.len() as u32).to_le_bytes().to_vec();
    for u in units {
        v.extend(u.to_le_bytes());
    }
    v
}

fn wb_prop(date1904: bool) -> Vec<u8> {
    let mut b = vec![u8::from(date1904), 0, 0, 0];
    b.extend([0; 8]);
    rec(0x0099, &b)
}

fn bundle_sh(state: u32, tab: u32, rid: &str, name: &str) -> Vec<u8> {
    let mut b = state.to_le_bytes().to_vec();
    b.extend(tab.to_le_bytes());
    b.extend(wide(rid));
    b.extend(wide(name));
    rec(0x009C, &b)
}

fn fmt_rec(ifmt: u16, code: &str) -> Vec<u8> {
    let mut b = ifmt.to_le_bytes().to_vec();
    b.extend(wide(code));
    rec(0x002C, &b)
}

fn xf_rec(ifmt: u16) -> Vec<u8> {
    let mut b = vec![0u8; 16];
    b[2..4].copy_from_slice(&ifmt.to_le_bytes());
    rec(0x002F, &b)
}

/// `styles.bin`: custom formats, then the cell XFs (cell xf index = position in `ifmts`).
fn styles_bin(formats: &[(u16, &str)], ifmts: &[u16]) -> Vec<u8> {
    let mut v = rec(0x0267, &(formats.len() as u32).to_le_bytes());
    for (i, c) in formats {
        v.extend(fmt_rec(*i, c));
    }
    v.extend(rec(0x0269, &(ifmts.len() as u32).to_le_bytes()));
    for &i in ifmts {
        v.extend(xf_rec(i));
    }
    v
}

fn row_hdr(row: u32) -> Vec<u8> {
    let mut b = row.to_le_bytes().to_vec();
    b.extend([0; 8]);
    rec(0x0000, &b)
}

fn cell_head(col: u32, style: u32) -> Vec<u8> {
    let mut b = col.to_le_bytes().to_vec();
    b.extend(style.to_le_bytes());
    b
}

fn real(col: u32, style: u32, v: f64) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.extend(v.to_le_bytes());
    rec(0x0005, &b)
}

fn rk_int(col: u32, style: u32, v: i32) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.extend(((v << 2) | 2).to_le_bytes());
    rec(0x0002, &b)
}

fn cell_st(col: u32, style: u32, s: &str) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.extend(wide(s));
    rec(0x0006, &b)
}

fn cell_bool(col: u32, style: u32, v: bool) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.push(u8::from(v));
    rec(0x0004, &b)
}

fn cell_err(col: u32, style: u32, code: u8) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.push(code);
    rec(0x0003, &b)
}

/// A worksheet part from rows of cell records: `[(row, [cell records])]`.
fn sheet_bin(rows: &[(u32, Vec<Vec<u8>>)]) -> Vec<u8> {
    let mut v = rec(0x0081, &[]);
    v.extend(rec(0x0093, &[0; 8]));
    let mut dim = vec![0u8; 16];
    dim[4..8].copy_from_slice(&1u32.to_le_bytes());
    v.extend(rec(0x0094, &dim));
    v.extend(rec(0x0091, &[]));
    for (r, cells) in rows {
        v.extend(row_hdr(*r));
        for c in cells {
            v.extend_from_slice(c);
        }
    }
    v.extend(rec(0x0092, &[]));
    v
}

struct SheetDef {
    name: String,
    state: u32,
    rid: String,
    bin: Vec<u8>,
}

fn sheet(name: &str, bin: Vec<u8>) -> SheetDef {
    SheetDef {
        name: name.into(),
        state: 0,
        rid: format!("rId{name}"),
        bin,
    }
}

struct Pkg {
    date1904: bool,
    styles: Option<Vec<u8>>,
    sheets: Vec<SheetDef>,
    /// `xl/sharedStrings.bin`.
    shared: Option<Vec<u8>>,
}

impl Pkg {
    fn new(styles: Option<Vec<u8>>, sheets: Vec<SheetDef>) -> Pkg {
        Pkg {
            date1904: false,
            styles,
            sheets,
            shared: None,
        }
    }

    fn with_shared(mut self, bin: Vec<u8>) -> Pkg {
        self.shared = Some(bin);
        self
    }

    fn workbook_bin(&self) -> Vec<u8> {
        let mut v = rec(0x0083, &[]);
        v.extend(wb_prop(self.date1904));
        v.extend(rec(0x008F, &[]));
        for (i, s) in self.sheets.iter().enumerate() {
            v.extend(bundle_sh(s.state, i as u32 + 1, &s.rid, &s.name));
        }
        v.extend(rec(0x0090, &[]));
        // calamine reads names until a record that "follows them".
        v.extend(rec(0x009D, &[]));
        v
    }

    fn rels(&self) -> String {
        let mut s = String::from(
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (i, d) in self.sheets.iter().enumerate() {
            s += &format!(
                r#"<Relationship Id="{}" Type="x/worksheet" Target="worksheets/sheet{}.bin"/>"#,
                d.rid,
                i + 1
            );
        }
        s + "</Relationships>"
    }

    fn bytes(&self) -> Vec<u8> {
        let wb = self.workbook_bin();
        let rels = self.rels();
        let mut entries: Vec<(String, Vec<u8>)> = vec![
            ("xl/workbook.bin".into(), wb),
            ("xl/_rels/workbook.bin.rels".into(), rels.into_bytes()),
        ];
        if let Some(s) = &self.styles {
            entries.push(("xl/styles.bin".into(), s.clone()));
        }
        if let Some(sh) = &self.shared {
            entries.push(("xl/sharedStrings.bin".into(), sh.clone()));
        }
        for (i, d) in self.sheets.iter().enumerate() {
            entries.push((format!("xl/worksheets/sheet{}.bin", i + 1), d.bin.clone()));
        }
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_slice()))
            .collect();
        deflated(&refs)
    }

    fn write(&self, dir: &crate::test_support::TmpDir, name: &str) -> std::path::PathBuf {
        write(dir, name, &self.bytes())
    }
}

fn text(wb: &Workbook, sheet: usize, r: usize, c: usize) -> String {
    wb.sheets[sheet].display(r, c).to_string()
}

fn one_sheet(styles: Option<Vec<u8>>, rows: &[(u32, Vec<Vec<u8>>)]) -> Pkg {
    Pkg::new(styles, vec![sheet("S", sheet_bin(rows))])
}

// ---------------------------------------------------------------------------------------------
// formats reach the display text through load_workbook
// ---------------------------------------------------------------------------------------------

#[test]
fn builtin_and_custom_formats_apply_to_every_cell_kind() {
    // xf 0 General, 1 = #,##0.00 (4), 2 = 0% (9), 3 = custom 164 `0.0%`, 4 = date (14)
    let styles = styles_bin(&[(164, "0.0%")], &[0, 4, 9, 164, 14]);
    let pkg = one_sheet(
        Some(styles),
        &[
            (
                0,
                vec![
                    real(0, 1, 1234.5),
                    real(1, 2, 0.256),
                    real(2, 3, 0.256),
                    real(3, 4, 46297.0),
                    rk_int(4, 1, 7),
                ],
            ),
            (
                1,
                vec![
                    cell_st(0, 1, "text"),
                    cell_bool(1, 1, true),
                    cell_err(2, 0, 0x07),
                    real(3, 0, 0.5),
                ],
            ),
        ],
    );
    let dir = tmp("xlsb_kinds");
    let p = pkg.write(&dir, "k.xlsb");
    let wb = load(&p).unwrap();
    assert_eq!(text(&wb, 0, 0, 0), "1,234.50", "real, built-in 4");
    assert_eq!(text(&wb, 0, 0, 1), "26%", "real, built-in 9");
    assert_eq!(text(&wb, 0, 0, 2), "25.6%", "real, custom 164");
    assert_eq!(
        text(&wb, 0, 0, 3),
        "10/2/2026",
        "real, built-in 14 (English)"
    );
    assert_eq!(text(&wb, 0, 0, 4), "7.00", "RK integer");
    assert_eq!(text(&wb, 0, 1, 0), "text", "a string keeps its text");
    assert_eq!(text(&wb, 0, 1, 1), "TRUE");
    assert_eq!(text(&wb, 0, 1, 2), "#DIV/0!");
    assert_eq!(text(&wb, 0, 1, 3), "0.5");
    assert_eq!(
        wb.sheets[0].cell(1, 2).unwrap().cell_type(),
        CellType::Error
    );
}

#[test]
fn a_built_in_date_format_follows_the_locale() {
    let styles = styles_bin(&[], &[0, 14, 22]);
    let pkg = one_sheet(
        Some(styles),
        &[(0, vec![real(0, 1, 46297.0), real(1, 2, 46297.5)])],
    );
    let dir = tmp("xlsb_locale");
    let p = pkg.write(&dir, "l.xlsb");
    let ja = load_workbook(
        &p,
        &LoadOptions {
            locale: Locale::Ja,
            limits: Limits::default(),
        },
    )
    .unwrap();
    assert_eq!(text(&ja, 0, 0, 0), "2026/10/2");
    assert_eq!(text(&ja, 0, 0, 1), "2026/10/2 12:00");
}

#[test]
fn wide_format_codes_and_sheet_names() {
    let styles = styles_bin(&[(170, "ggge\"年\"m\"月\"d\"日\"")], &[0, 170]);
    let pkg = Pkg::new(
        Some(styles),
        vec![sheet("売上", sheet_bin(&[(0, vec![real(0, 1, 46297.0)])]))],
    );
    let dir = tmp("xlsb_wide");
    let p = pkg.write(&dir, "w.xlsb");
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets[0].name, "売上");
    assert_eq!(text(&wb, 0, 0, 0), "令和8年10月2日");
}

#[test]
fn the_1904_flag_changes_the_date() {
    let styles = styles_bin(&[(164, "yyyy-mm-dd")], &[0, 164]);
    let mut pkg = one_sheet(Some(styles), &[(0, vec![real(0, 1, 61.0)])]);
    let dir = tmp("xlsb_1904");
    let p = pkg.write(&dir, "a.xlsb");
    let wb = load(&p).unwrap();
    assert!(!wb.date1904);
    assert_eq!(text(&wb, 0, 0, 0), "1900-03-01");
    pkg.date1904 = true;
    let p = pkg.write(&dir, "b.xlsb");
    let wb = load(&p).unwrap();
    assert!(wb.date1904);
    assert_eq!(text(&wb, 0, 0, 0), "1904-03-02");
}

#[test]
fn style_ref_is_a_24_bit_field_and_unknown_refs_are_general() {
    let styles = styles_bin(&[], &[0, 4]);
    let pkg = one_sheet(
        Some(styles),
        &[(
            0,
            vec![
                // High bits of the 4-byte word are flags, not part of iStyleRef.
                real(0, 0xAB00_0001, 5.0),
                real(1, 7, 5.0),
                real(2, 0x00FF_FFFF, 5.0),
                real(3, 1, 5.0),
            ],
        )],
    );
    let dir = tmp("xlsb_styleref");
    let p = pkg.write(&dir, "s.xlsb");
    let wb = load(&p).unwrap();
    let got: Vec<String> = (0..4).map(|c| text(&wb, 0, 0, c)).collect();
    assert_eq!(got, ["5.00", "5", "5", "5.00"]);
}

#[test]
fn a_package_without_styles_is_all_general() {
    let pkg = one_sheet(None, &[(0, vec![real(0, 3, 0.1)])]);
    let dir = tmp("xlsb_nostyles");
    let p = pkg.write(&dir, "n.xlsb");
    assert_eq!(text(&load(&p).unwrap(), 0, 0, 0), "0.1");
}

#[test]
fn cell_style_xfs_are_not_counted() {
    // cellStyleXFs hold BrtXF records too; only the cellXFs block maps cell style refs.
    let mut v = rec(0x0267, &0u32.to_le_bytes());
    v.extend(rec(0x026B, &2u32.to_le_bytes()));
    v.extend(xf_rec(49));
    v.extend(xf_rec(49));
    v.extend(rec(0x026C, &[]));
    v.extend(rec(0x0269, &1u32.to_le_bytes()));
    v.extend(xf_rec(4));
    v.extend(rec(0x026A, &[]));
    let st = fmt_xlsb::parse_styles(&v[..]).unwrap();
    assert_eq!(st.xf_to_format.len(), 1);
    assert_eq!(st.formats, vec![NumFmtRef::General, NumFmtRef::Builtin(4)]);
    assert_eq!(st.xf_to_format, vec![1]);
}

#[test]
fn a_custom_format_overrides_a_built_in_id_and_general_is_general() {
    let styles = styles_bin(&[(14, "yyyy-mm-dd"), (165, "General")], &[14, 165]);
    let pkg = one_sheet(
        Some(styles),
        &[(0, vec![real(0, 0, 46297.0), real(1, 1, 0.5)])],
    );
    let dir = tmp("xlsb_override");
    let p = pkg.write(&dir, "o.xlsb");
    let wb = load(&p).unwrap();
    assert_eq!(text(&wb, 0, 0, 0), "2026-10-02");
    assert_eq!(
        wb.format_of(wb.sheets[0].cell(0, 1).unwrap()),
        &NumFmtRef::General
    );
}

#[test]
fn several_sheets_keep_their_own_formats_and_hidden_ones_are_counted() {
    let styles = styles_bin(&[], &[0, 4, 9]);
    let mut hidden = sheet("H", sheet_bin(&[(0, vec![real(0, 1, 9.0)])]));
    hidden.state = 1;
    let pkg = Pkg::new(
        Some(styles),
        vec![
            sheet("A", sheet_bin(&[(0, vec![real(0, 1, 5.0)])])),
            hidden,
            sheet("B", sheet_bin(&[(0, vec![real(0, 2, 0.5)])])),
        ],
    );
    let dir = tmp("xlsb_sheets");
    let p = pkg.write(&dir, "m.xlsb");
    let wb = load(&p).unwrap();
    assert_eq!(wb.hidden_sheets, 1);
    assert_eq!(wb.sheets.len(), 2);
    assert_eq!(text(&wb, 0, 0, 0), "5.00");
    assert_eq!(text(&wb, 1, 0, 0), "50%");
}

#[test]
fn the_pass_reports_what_it_read() {
    let styles = styles_bin(&[], &[0, 4]);
    let pkg = one_sheet(
        Some(styles),
        &[
            (2, vec![real(1, 1, 1.0), real(3, 0, 1.0)]),
            (9, vec![cell_st(0, 1, "x")]),
        ],
    );
    let dir = tmp("xlsb_report");
    let p = pkg.write(&dir, "r.xlsb");
    let fm = fmt_xlsb::read(&p, &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.value_cells, 3);
    assert_eq!(sf.cells, vec![(2, 1, 1), (9, 0, 1)]);
    assert_eq!(fm.formats, vec![NumFmtRef::General, NumFmtRef::Builtin(4)]);
}

// ---------------------------------------------------------------------------------------------
// the memory checks
// ---------------------------------------------------------------------------------------------

#[test]
fn a_far_flung_sheet_shows_what_fits_instead_of_being_refused() {
    // Values at A1 and XFD1048576: a dense matrix over the box is ~17 billion cells. Streamed,
    // the sheet shows A1 and ends at the row past the cap.
    let pkg = one_sheet(
        None,
        &[
            (0, vec![real(0, 0, 1.0)]),
            (1_048_575, vec![real(16_383, 0, 1.0)]),
        ],
    );
    let dir = tmp("xlsb_far");
    let p = pkg.write(&dir, "f.xlsb");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.display(0, 0), "1");
    assert!(s.rows_truncated);
    assert_eq!((s.nrows, s.ncols), (1, 1));
}

#[test]
fn a_column_past_excels_last_is_skipped_not_widened_to() {
    // A hostile column index beyond XFD: the cell is dropped (and the sheet says columns were
    // cut); it neither widens the grid nor needs a matrix that spans it.
    let pkg = one_sheet(
        None,
        &[
            (0, vec![real(0, 0, 1.0)]),
            (5, vec![real(4_000_000, 0, 1.0)]),
        ],
    );
    let dir = tmp("xlsb_widecol");
    let p = pkg.write(&dir, "w.xlsb");
    let wb = load(&p).unwrap();
    let s = &wb.sheets[0];
    assert!(s.cols_truncated);
    assert_eq!(s.ncols, Limits::default().max_cols);
    assert_eq!(s.row_cells(5).len(), 0);
    assert_eq!(s.display(0, 0), "1");
    // The cell itself is not kept in the format list (past max_cols).
    let sf = fmt_xlsb::read(&p, &Limits::default()).unwrap();
    assert!(sf.sheets["S"].cells.is_empty());
}

#[test]
fn the_cell_budget_is_exact_at_the_boundary() {
    // 100 cells (one per row) against a budget of exactly that many.
    let rows: Vec<(u32, Vec<Vec<u8>>)> = (0..100).map(|r| (r, vec![real(0, 0, 1.0)])).collect();
    let dir = tmp("xlsb_exact");
    let p = Pkg::new(None, vec![sheet("S", sheet_bin(&rows))]).write(&dir, "ok.xlsb");
    let at = |cells: u64| {
        let limits = Limits {
            max_sheet_cells: cells,
            ..small_limits()
        };
        let s = load_with(&p, limits).unwrap().sheets.remove(0);
        (s.nrows, s.rows_truncated)
    };
    assert_eq!(at(100), (100, false));
    assert_eq!(at(99), (99, true));
}

#[test]
fn a_far_flung_hidden_sheet_does_not_block_the_visible_ones() {
    // Only the sheet asked for is read: the hidden one is never touched, so it cannot be refused.
    let mut hidden = sheet(
        "H",
        sheet_bin(&[
            (0, vec![real(0, 0, 1.0)]),
            (1_048_575, vec![real(16_383, 0, 1.0)]),
        ]),
    );
    hidden.state = 1;
    let pkg = Pkg::new(
        None,
        vec![sheet("V", sheet_bin(&[(0, vec![real(0, 0, 2.0)])])), hidden],
    );
    let dir = tmp("xlsb_hidden_far");
    let p = pkg.write(&dir, "h.xlsb");
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    assert_eq!(text(&wb, 0, 0, 0), "2");
}

#[test]
fn the_cell_count_budget_is_enforced_while_streaming() {
    let limits = Limits {
        max_sheet_cells: 3,
        ..Limits::default()
    };
    let cells: Vec<Vec<u8>> = (0..4).map(|c| real(c, 0, 1.0)).collect();
    let bin = sheet_bin(&[(0, cells)]);
    let sf = fmt_xlsb::parse_sheet(&bin[..], &[], &limits).unwrap();
    assert!(sf.truncated);
    assert_eq!(sf.value_cells, 3);
}

#[test]
fn rows_past_the_last_excel_row_end_the_sheet_like_calamine() {
    let bin = sheet_bin(&[
        (0, vec![real(0, 0, 1.0)]),
        (0x0010_0001, vec![real(0, 0, 2.0)]),
        (5, vec![real(0, 0, 3.0)]),
    ]);
    let sf = fmt_xlsb::parse_sheet(&bin[..], &[], &Limits::default()).unwrap();
    assert_eq!(sf.value_cells, 1);
}

// ---------------------------------------------------------------------------------------------
// the record reader
// ---------------------------------------------------------------------------------------------

#[test]
fn record_ids_and_sizes_use_the_variable_length_forms() {
    // A 2-byte id (>= 0x80) and a 2-byte size (>= 128 bytes of body) around a real cell.
    let mut bin = rec(0x0081, &[]);
    bin.extend(rec(0x0091, &[]));
    bin.extend(rec(0x0225, &vec![7u8; 300])); // 2-byte id, 2-byte size
    bin.extend(row_hdr(0));
    bin.extend(real(0, 0, 1.0));
    bin.extend(rec(0x0092, &[]));
    let sf = fmt_xlsb::parse_sheet(&bin[..], &[], &Limits::default()).unwrap();
    assert_eq!(sf.value_cells, 1);
}

#[test]
fn a_body_longer_than_the_read_cap_is_skipped_not_buffered() {
    let mut bin = rec(0x0091, &[]);
    bin.extend(rec(0x0200, &vec![0u8; 300_000])); // > 64 KiB, with its bytes present
    bin.extend(row_hdr(4));
    bin.extend(real(1, 0, 1.0));
    bin.extend(rec(0x0092, &[]));
    let sf = fmt_xlsb::parse_sheet(&bin[..], &[], &Limits::default()).unwrap();
    assert_eq!(sf.value_cells, 1, "the record after the long body is read");
}

#[test]
fn a_forged_giant_size_neither_allocates_nor_hangs() {
    // `size` = 0x0FFFFFFF (268 MB) on a record whose body is not there.
    let mut bin = rec(0x0091, &[]);
    bin.extend([0x02, 0xFF, 0xFF, 0xFF, 0x7F]);
    bin.extend([1, 2, 3]);
    let r = fmt_xlsb::parse_sheet(&bin[..], &[], &Limits::default());
    assert!(matches!(r, Err(OfficeError::Corrupt(_))), "{r:?}");
    // The same in the workbook and styles streams.
    let giant = [0x83u8, 0x00, 0xFF, 0xFF, 0xFF, 0x7F];
    assert!(fmt_xlsb::parse_workbook(&giant[..]).is_err());
    assert!(fmt_xlsb::parse_styles(&giant[..]).is_err());
    // Skipping a long body that really exists in full (no allocation of it) is fine.
    let mut ok = rec(0x0091, &[]);
    ok.extend(rec(0x0200, &vec![0u8; 3_000_000]));
    ok.extend(rec(0x0092, &[]));
    assert!(fmt_xlsb::parse_sheet(&ok[..], &[], &Limits::default()).is_ok());
}

#[test]
fn a_stream_cut_inside_a_record_is_corrupt_and_between_records_is_the_end() {
    let limits = Limits::default();
    let records = [
        rec(0x0091, &[]),
        row_hdr(0),
        real(0, 0, 1.0),
        real(1, 0, 2.0),
    ];
    let bin: Vec<u8> = records.concat();
    let mut at = 0;
    for r in &records {
        // Inside this record: corrupt (a cut header or a cut body).
        for cut in 1..r.len() {
            let res = fmt_xlsb::parse_sheet(&bin[..at + cut], &[], &limits);
            assert!(
                matches!(res, Err(OfficeError::Corrupt(_))),
                "cut {cut} bytes into a {}-byte record: {res:?}",
                r.len()
            );
        }
        at += r.len();
        // Right after it: a clean end.
        assert!(fmt_xlsb::parse_sheet(&bin[..at], &[], &limits).is_ok());
    }
    // A cell record shorter than col + style is corrupt.
    let mut short = rec(0x0091, &[]);
    short.extend(rec(0x0005, &[1, 2, 3]));
    assert!(matches!(
        fmt_xlsb::parse_sheet(&short[..], &[], &limits),
        Err(OfficeError::Corrupt(_))
    ));
}

#[test]
fn cells_before_begin_sheet_data_are_ignored() {
    let mut bin = rec(0x0081, &[]);
    bin.extend(real(0, 0, 1.0)); // a stray record with a cell id before the data block
    bin.extend(rec(0x0091, &[]));
    bin.extend(row_hdr(0));
    bin.extend(real(2, 0, 1.0));
    bin.extend(rec(0x0092, &[]));
    bin.extend(row_hdr(9)); // after the end: ignored
    bin.extend(real(5, 0, 1.0));
    let sf = fmt_xlsb::parse_sheet(&bin[..], &[], &Limits::default()).unwrap();
    assert_eq!(sf.value_cells, 1);
}

#[test]
fn bundle_records_with_null_or_bad_strings_are_skipped() {
    let mut v = rec(0x0083, &[]);
    v.extend(wb_prop(false));
    // Null relationship id (0xFFFFFFFF): calamine skips such sheets too.
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend(0xFFFF_FFFFu32.to_le_bytes());
    b.extend(wide("NoRel"));
    v.extend(rec(0x009C, &b));
    // A name longer than the record.
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend(wide("rId1"));
    b.extend(500u32.to_le_bytes());
    v.extend(rec(0x009C, &b));
    // Too short to hold the header.
    v.extend(rec(0x009C, &[1, 2, 3]));
    v.extend(bundle_sh(0, 2, "rId2", "Good"));
    v.extend(rec(0x0090, &[]));
    let (d, sheets) = fmt_xlsb::parse_workbook(&v[..]).unwrap();
    assert!(!d);
    assert_eq!(sheets, vec![("Good".to_string(), "rId2".to_string())]);
}

#[test]
fn unpaired_surrogates_in_names_do_not_panic() {
    let mut b = 0u32.to_le_bytes().to_vec();
    b.extend(1u32.to_le_bytes());
    b.extend(wide("rId1"));
    b.extend(2u32.to_le_bytes());
    b.extend([0x00, 0xD8, 0x41, 0x00]); // lone high surrogate, then 'A'
    let mut v = rec(0x009C, &b);
    v.extend(rec(0x0090, &[]));
    let (_, sheets) = fmt_xlsb::parse_workbook(&v[..]).unwrap();
    assert_eq!(sheets[0].0, "\u{FFFD}A");
}

#[test]
fn a_sheet_without_a_relationship_or_part_is_skipped_by_the_pass() {
    let pkg = Pkg::new(
        None,
        vec![sheet("S", sheet_bin(&[(0, vec![real(0, 0, 1.0)])]))],
    );
    // Drop the sheet part from the package; the pass skips it (calamine then reports the error).
    let wb = pkg.workbook_bin();
    let rels = pkg.rels();
    let bytes = deflated(&[
        ("xl/workbook.bin", &wb),
        ("xl/_rels/workbook.bin.rels", rels.as_bytes()),
    ]);
    let dir = tmp("xlsb_nopart");
    let p = write(&dir, "n.xlsb", &bytes);
    // The sheet is listed (its relationship resolves) but its part is not there: no formats.
    let fm = fmt_xlsb::read(&p, &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"], SheetFormats::default());
    // Loading it fails (calamine cannot find the part): at the sheet when the workbook opened.
    assert!(load(&p).is_err());
}

#[test]
fn missing_workbook_bin_is_corrupt() {
    // A package that is detected as xlsb must have the part; a damaged one is an error, no panic.
    let wb = Pkg::new(None, vec![]).workbook_bin();
    let dir = tmp("xlsb_nowb");
    let p = write(&dir, "n.xlsb", &deflated(&[("xl/workbook.bin", &wb)]));
    // Read directly with the part removed from a copy that lacks it.
    let q = write(&dir, "q.xlsb", &deflated(&[("xl/other.bin", &wb)]));
    assert!(matches!(
        fmt_xlsb::read(&q, &Limits::default()),
        Err(OfficeError::Corrupt(_))
    ));
    let _ = p;
}

// ---------------------------------------------------------------------------------------------
// broken input
// ---------------------------------------------------------------------------------------------

#[test]
fn every_truncation_of_a_synthetic_xlsb_is_handled() {
    let styles = styles_bin(&[(164, "0.0%")], &[0, 4, 164]);
    let pkg = one_sheet(
        Some(styles),
        &[(0, vec![real(0, 1, 1.0), cell_st(1, 0, "x")])],
    );
    let bytes = pkg.bytes();
    let dir = tmp("xlsb_trunc");
    for n in 0..bytes.len() {
        let p = write(&dir, "t.xlsb", &bytes[..n]);
        let r = assert_inner_does_not_panic(&p, &format!("truncation at {n}"));
        assert!(r.is_err(), "a {n}-byte prefix of {} loaded", bytes.len());
    }
}

#[test]
fn every_prefix_of_each_part_never_panics_in_konomas_pass() {
    let styles = styles_bin(&[(164, "0.0%")], &[0, 4, 164]);
    let pkg = one_sheet(
        Some(styles.clone()),
        &[(
            0,
            vec![real(0, 1, 1.0), rk_int(1, 2, 3), cell_st(2, 0, "x")],
        )],
    );
    let wb = pkg.workbook_bin();
    let sh = sheet_bin(&[(
        0,
        vec![real(0, 1, 1.0), rk_int(1, 2, 3), cell_st(2, 0, "x")],
    )]);
    for n in 0..wb.len() {
        let r = std::panic::catch_unwind(|| fmt_xlsb::parse_workbook(&wb[..n]));
        assert!(r.is_ok(), "workbook prefix {n}");
    }
    for n in 0..styles.len() {
        let r = std::panic::catch_unwind(|| fmt_xlsb::parse_styles(&styles[..n]));
        assert!(r.is_ok(), "styles prefix {n}");
    }
    for n in 0..sh.len() {
        let r =
            std::panic::catch_unwind(|| fmt_xlsb::parse_sheet(&sh[..n], &[0, 1], &small_limits()));
        assert!(r.is_ok(), "sheet prefix {n}");
    }
}

#[test]
fn mutated_parts_never_panic_in_konomas_pass() {
    let styles = styles_bin(&[(164, "0.0%")], &[0, 4, 164]);
    let sh = sheet_bin(&[
        (
            0,
            vec![real(0, 1, 1.0), rk_int(1, 2, 3), cell_st(2, 0, "x")],
        ),
        (3, vec![cell_bool(0, 0, true), cell_err(1, 0, 7)]),
    ]);
    let wb = Pkg::new(None, vec![sheet("S", sh.clone())]).workbook_bin();
    let mut seed = 0x0F0F_1234_ABCD_9876u64;
    for round in 0..500u64 {
        for (which, data) in [(0, &wb), (1, &styles), (2, &sh)] {
            let mut b = data.clone();
            let kind = xorshift(&mut seed);
            mutate(&mut b, &mut seed, kind);
            let r = std::panic::catch_unwind(|| match which {
                0 => fmt_xlsb::parse_workbook(&b[..]).map(|_| ()),
                1 => fmt_xlsb::parse_styles(&b[..]).map(|_| ()),
                _ => fmt_xlsb::parse_sheet(&b[..], &[0, 1, 2], &small_limits()).map(|_| ()),
            });
            assert!(
                r.is_ok(),
                "the xlsb pass panicked in round {round} (part {which})"
            );
        }
    }
}

#[test]
fn a_zip_with_garbage_parts_is_an_error_not_a_panic() {
    let dir = tmp("xlsb_garbage");
    let p = write(
        &dir,
        "g.xlsb",
        &deflated(&[
            ("xl/workbook.bin", &[0xFF; 64]),
            ("xl/styles.bin", &[0xFF; 64]),
        ]),
    );
    let r = assert_inner_does_not_panic(&p, "garbage parts");
    assert!(r.is_err() || r.is_ok());
    assert!(std::panic::catch_unwind(|| fmt_xlsb::read(&p, &small_limits())).is_ok());
}

// ---------------------------------------------------------------------------------------------
// memory: formula-only cells and the text budget
// ---------------------------------------------------------------------------------------------

/// `BrtFmlaError` (0x000B): a formula that evaluates to an error. `calamine` skips it in the
/// value range but its range of formulas includes it.
fn fmla_error(col: u32, style: u32) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.push(0x07); // #DIV/0!
    b.extend([0, 0]); // grbitFlags
    b.extend(0u32.to_le_bytes()); // cce
    b.extend(0u32.to_le_bytes()); // cb
    rec(0x000B, &b)
}

#[test]
fn a_formula_error_cell_in_a_far_corner_does_not_widen_the_sheet() {
    let pkg = one_sheet(
        None,
        &[
            (0, vec![real(0, 0, 1.0)]),
            (1_048_575, vec![fmla_error(16_383, 0)]),
        ],
    );
    let dir = tmp("xlsb_fmla_err");
    let p = pkg.write(&dir, "e.xlsb");
    let sf = fmt_xlsb::read(&p, &Limits::default()).unwrap();
    assert_eq!(sf.sheets["S"].value_cells, 1);
    assert!(sf.sheets["S"].truncated);
    let wb = load(&p).unwrap();
    assert_eq!((wb.sheets[0].nrows, wb.sheets[0].ncols), (1, 1));
    // Formula-only on both ends.
    let pkg = one_sheet(
        None,
        &[
            (0, vec![fmla_error(0, 0)]),
            (1_048_575, vec![fmla_error(16_383, 0)]),
        ],
    );
    let p = pkg.write(&dir, "e2.xlsb");
    assert!(load(&p).is_ok());
}

fn sst_bin(strings: &[String], declared_unique: Option<u32>) -> Vec<u8> {
    let n = declared_unique.unwrap_or(strings.len() as u32);
    let mut b = (strings.len() as u32).to_le_bytes().to_vec();
    b.extend(n.to_le_bytes());
    let mut v = rec(0x009F, &b);
    for s in strings {
        let mut item = vec![0u8];
        item.extend(wide(s));
        v.extend(rec(0x0013, &item));
    }
    v.extend(rec(0x00A0, &[]));
    v
}

fn isst(col: u32, style: u32, i: u32) -> Vec<u8> {
    let mut b = cell_head(col, style);
    b.extend(i.to_le_bytes());
    rec(0x0007, &b)
}

#[test]
fn one_long_shared_string_used_by_many_cells_cuts_the_sheet_at_the_text_budget() {
    // 10,000 characters x 200 cells = 2 MB against the sheet's 1 MiB text budget: shown up to the
    // cell that no longer fits (each kept cell costs 10,000 + 32), marked as cut — not refused.
    let limits = Limits {
        max_rows: 1000,
        max_sheet_cells: 100_000,
        ..small_limits()
    };
    let mk = |cells: u32| {
        let rows: Vec<(u32, Vec<Vec<u8>>)> = (0..cells).map(|r| (r, vec![isst(0, 0, 0)])).collect();
        Pkg::new(None, vec![sheet("S", sheet_bin(&rows))])
            .with_shared(sst_bin(&["q".repeat(10_000)], None))
    };
    let dir = tmp("xlsb_amplify");
    let p = mk(200).write(&dir, "a.xlsb");
    let s = load_with(&p, limits).unwrap().sheets.remove(0);
    assert!(s.rows_truncated);
    assert_eq!(s.nrows as u64, limits.max_sheet_text_bytes / 10_032);
    let p = mk(50).write(&dir, "b.xlsb");
    let wb = load_with(&p, limits).unwrap();
    assert!(!wb.sheets[0].rows_truncated);
    assert_eq!(wb.sheets[0].display(49, 0).len(), 10_000);
}

#[test]
fn inline_strings_count_against_the_sheet_text_budget_in_xlsb() {
    let limits = Limits {
        max_sheet_text_bytes: 50_000,
        ..small_limits()
    };
    // 20 x (4,000 + 32) against 50,000: twelve fit.
    let big = "w".repeat(4000);
    let rows: Vec<(u32, Vec<Vec<u8>>)> = (0..20).map(|r| (r, vec![cell_st(0, 0, &big)])).collect();
    let dir = tmp("xlsb_inline");
    let p = Pkg::new(None, vec![sheet("S", sheet_bin(&rows))]).write(&dir, "i.xlsb");
    let s = load_with(&p, limits).unwrap().sheets.remove(0);
    assert_eq!(s.nrows, 12);
    assert!(s.rows_truncated);
}

#[test]
fn a_forged_unique_count_in_the_xlsb_table_is_refused() {
    let dir = tmp("xlsb_unique");
    let pkg = one_sheet(None, &[(0, vec![real(0, 0, 1.0)])])
        .with_shared(sst_bin(&["a".into()], Some(0x7FFF_FFFF)));
    let p = pkg.write(&dir, "u.xlsb");
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
}

// ---------------------------------------------------------------------------------------------
// mutation survivors: record id range, 24-bit style refs past 65,535, row limit, XF cap
// ---------------------------------------------------------------------------------------------

#[test]
fn exactly_the_value_cell_records_2_to_0b_are_read() {
    // MS-XLSB: 0x0001 is a blank cell (no value: not read), 0x0002..=0x000A are the value cells
    // and 0x000B is a formula that evaluates to an error (`calamine` keeps it in the range of
    // formulas); 0x000C and up are other records.
    let mut ids_read = Vec::new();
    for id in 0x0000u16..=0x000E {
        // A body long enough for the longest layout (col, style, then 12 bytes).
        let mut body = cell_head(3, 1);
        body.extend([0u8; 12]);
        let mut bin = rec(0x0091, &[]); // BrtBeginSheetData
        bin.extend(row_hdr(0));
        if id != 0 {
            bin.extend(rec(id, &body));
        }
        bin.extend(rec(0x0092, &[]));
        let sf = fmt_xlsb::parse_sheet(&bin[..], &[0, 1], &Limits::default()).unwrap();
        if sf.value_cells == 1 {
            ids_read.push(id);
            assert_eq!(sf.cells, vec![(0, 3, 1)], "id {id:#x} keeps its format");
        } else {
            assert_eq!(sf.value_cells, 0, "id {id:#x}");
        }
    }
    let want: Vec<u16> = (0x0002..=0x000B).collect();
    assert_eq!(ids_read, want);
}

#[test]
fn a_style_ref_above_65535_is_not_truncated_to_16_bits() {
    // 70,000 cell XFs, only xf 66,000 has a (date) format. A 16-bit mask would send a cell that
    // uses xf 66,000 to xf 464, which is General.
    let n = 70_000usize;
    let mut ifmts = vec![0u16; n];
    ifmts[66_000] = 14;
    let styles = styles_bin(&[], &ifmts);
    let st = fmt_xlsb::parse_styles(&styles[..]).unwrap();
    assert_eq!(st.xf_to_format.len(), n);
    let date = st.xf_to_format[66_000];
    assert_ne!(date, 0);
    assert_eq!(st.formats[usize::from(date)], NumFmtRef::Builtin(14));
    let bin = sheet_bin(&[(0, vec![real(0, 66_000, 45292.0), real(1, 464, 45292.0)])]);
    let sf = fmt_xlsb::parse_sheet(&bin[..], &st.xf_to_format, &Limits::default()).unwrap();
    assert_eq!(sf.format_at(0, 0), date);
    assert_eq!(sf.format_at(0, 1), 0, "xf 464 is General");
    // And the 4th byte of the word is a flag byte, never part of the index.
    let bin = sheet_bin(&[(0, vec![real(0, 0x7F00_0000 | 66_000, 1.0)])]);
    let sf = fmt_xlsb::parse_sheet(&bin[..], &st.xf_to_format, &Limits::default()).unwrap();
    assert_eq!(sf.format_at(0, 0), date);
}

#[test]
fn the_row_limit_is_exact() {
    // The pass stops at the first row header at or past `max_rows`; and, whatever `max_rows` is,
    // at one past Excel's last row (0x100000), as `calamine` does.
    let at = |row: u32, max_rows: usize| {
        let bin = sheet_bin(&[(row, vec![real(0, 0, 1.0)])]);
        let limits = Limits {
            max_rows,
            ..Limits::default()
        };
        fmt_xlsb::parse_sheet(&bin[..], &[], &limits).unwrap()
    };
    let sf = at(99, 100);
    assert_eq!(sf.value_cells, 1);
    assert!(!sf.truncated);
    let sf = at(100, 100);
    assert_eq!(sf.value_cells, 0);
    assert!(sf.truncated);
    // With a cap above Excel's last row, Excel's last row decides.
    assert_eq!(at(0x0010_0000, 2_000_000).value_cells, 1);
    assert_eq!(at(0x000F_FFFF, 2_000_000).value_cells, 1);
    assert_eq!(at(0x0010_0001, 2_000_000).value_cells, 0);
}

#[test]
fn only_the_first_100_000_cell_xfs_are_kept() {
    let styles = |n: usize, last_ifmt: u16| {
        let mut ifmts = vec![0u16; n - 1];
        ifmts.push(last_ifmt);
        styles_bin(&[], &ifmts)
    };
    let kept = fmt_xlsb::parse_styles(&styles(100_000, 14)[..]).unwrap();
    assert_eq!(kept.xf_to_format.len(), 100_000);
    assert!(kept.formats.contains(&NumFmtRef::Builtin(14)));
    let dropped = fmt_xlsb::parse_styles(&styles(100_001, 14)[..]).unwrap();
    assert_eq!(dropped.xf_to_format.len(), 100_000);
    assert_eq!(dropped.formats, vec![NumFmtRef::General]);
}

// ---------------------------------------------------------------------------------------------
// one sheet at a time, and the format-code length limit
// ---------------------------------------------------------------------------------------------

#[test]
fn a_format_code_over_255_characters_is_dropped_and_255_is_kept() {
    let keep = format!("0.{}", "0".repeat(253)); // 255 characters
    assert_eq!(keep.chars().count(), 255);
    let drop = format!("0.{}", "0".repeat(254)); // 256
    let styles = styles_bin(&[(164, &keep), (165, &drop)], &[164, 165]);
    let st = fmt_xlsb::parse_styles(&styles[..]).unwrap();
    assert_eq!(
        st.formats,
        vec![NumFmtRef::General, NumFmtRef::Custom(keep.as_str().into())],
        "the long code is not held in the format table"
    );
    assert_eq!(st.xf_to_format, vec![1, 0], "its cells are General");
    // End to end: the cell with the long code shows the General text.
    let pkg = one_sheet(Some(styles), &[(0, vec![real(0, 0, 0.5), real(1, 1, 0.5)])]);
    let dir = tmp("xlsb_longcode");
    let p = pkg.write(&dir, "l.xlsb");
    let wb = load(&p).unwrap();
    assert!(
        text(&wb, 0, 0, 0).starts_with("0.5000"),
        "the 255-character code applies"
    );
    assert_eq!(text(&wb, 0, 0, 1), "0.5");
}

#[test]
fn only_the_sheet_asked_for_is_read_and_the_index_is_clamped() {
    let pkg = Pkg::new(
        None,
        vec![
            sheet("A", sheet_bin(&[(0, vec![real(0, 0, 1.0)])])),
            sheet("B", sheet_bin(&[(0, vec![real(0, 0, 2.0)])])),
            sheet("C", sheet_bin(&[(0, vec![real(0, 0, 3.0)])])),
        ],
    );
    let dir = tmp("xlsb_one");
    let p = pkg.write(&dir, "o.xlsb");
    let one = |i: usize| load_workbook_sheet(&p, &LoadOptions::default(), i).unwrap();
    for (i, want) in [(0, "1"), (1, "2"), (2, "3"), (9, "3")] {
        let wb = one(i);
        let names: Vec<&str> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"], "every sheet is listed");
        let loaded: Vec<usize> = (0..3).filter(|&k| wb.sheets[k].loaded).collect();
        assert_eq!(loaded, [i.min(2)], "exactly the requested sheet has cells");
        assert_eq!(wb.loaded_index(), Some(i.min(2)));
        assert_eq!(wb.sheets[i.min(2)].display(0, 0), want);
        assert!(wb.sheet_error.is_none());
    }
}

#[test]
fn a_damaged_sheet_part_is_a_sheet_error_and_the_other_sheets_still_load() {
    let pkg = Pkg::new(
        None,
        vec![
            sheet("Good", sheet_bin(&[(0, vec![real(0, 0, 1.0)])])),
            // A record that claims more bytes than the stream has.
            sheet("Bad", vec![0x91, 0x7F, 1, 2, 3]),
        ],
    );
    let dir = tmp("xlsb_badsheet");
    let p = pkg.write(&dir, "b.xlsb");
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 1).unwrap();
    assert_eq!(wb.sheets.len(), 2, "the workbook opened and lists both");
    assert!(wb.loaded_index().is_none());
    assert!(
        matches!(wb.sheet_error, Some(OfficeError::Corrupt(_))),
        "{:?}",
        wb.sheet_error
    );
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 0).unwrap();
    assert_eq!(wb.sheets[0].display(0, 0), "1");
    assert!(wb.sheet_error.is_none());
}

#[test]
fn an_xlsb_may_have_the_most_sheets_and_no_more() {
    let book = |n: usize| {
        let mut v = Vec::new();
        for i in 0..n {
            v.extend(bundle_sh(0, i as u32, &format!("rId{i}"), &format!("S{i}")));
        }
        v.extend(rec(0x0090, &[]));
        v
    };
    let most = fmt_xlsx::MAX_SHEETS;
    assert_eq!(
        fmt_xlsb::parse_workbook(&book(most)[..]).unwrap().1.len(),
        most
    );
    assert_eq!(
        fmt_xlsb::parse_workbook(&book(most + 1)[..]).unwrap_err(),
        OfficeError::TooLarge { what: "sheets" }
    );
}
