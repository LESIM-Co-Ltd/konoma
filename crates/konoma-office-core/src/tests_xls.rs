//! Tests for the xls format pass (`fmt_xls`). Real LibreOffice output is in `tests.rs`
//! (`real_xls_*`); the BIFF8 streams here are assembled record by record so each rule (FORMAT /
//! XF / DATEMODE / every cell record / CONTINUE / the memory checks) is pinned on its own.

use super::container::Limits;
use super::fmt_xls;
use super::tests::{
    assert_inner_does_not_panic, load, load_with, mutate, small_limits, tmp, write_cfb, xorshift,
};
use super::*;
use std::io::Write as _;

// ---------------------------------------------------------------------------------------------
// a tiny BIFF8 writer
// ---------------------------------------------------------------------------------------------

fn rec(typ: u16, data: &[u8]) -> Vec<u8> {
    let mut v = typ.to_le_bytes().to_vec();
    v.extend((data.len() as u16).to_le_bytes());
    v.extend_from_slice(data);
    v
}

fn bof(version: u16, dt: u16) -> Vec<u8> {
    let mut d = version.to_le_bytes().to_vec();
    d.extend(dt.to_le_bytes());
    d.extend([0xD3, 0x10, 0xCC, 0x07, 0, 0, 0, 0, 0x06, 0, 0, 0]);
    rec(0x0809, &d)
}

fn eof() -> Vec<u8> {
    rec(0x000A, &[])
}

/// `XLUnicodeString` (BIFF8): `cch: u16`, flags, characters.
fn xl_string(s: &str, wide: bool) -> Vec<u8> {
    let mut v = Vec::new();
    if wide {
        let units: Vec<u16> = s.encode_utf16().collect();
        v.extend((units.len() as u16).to_le_bytes());
        v.push(1);
        for u in units {
            v.extend(u.to_le_bytes());
        }
    } else {
        v.extend((s.chars().count() as u16).to_le_bytes());
        v.push(0);
        v.extend(s.chars().map(|c| c as u8));
    }
    v
}

fn format_rec(ifmt: u16, code: &str) -> Vec<u8> {
    let wide = !code.is_ascii() && code.chars().any(|c| c as u32 > 255);
    let mut d = ifmt.to_le_bytes().to_vec();
    d.extend(xl_string(code, wide));
    rec(0x041E, &d)
}

fn xf_rec(ifmt: u16) -> Vec<u8> {
    let mut d = vec![0u8; 20];
    d[2..4].copy_from_slice(&ifmt.to_le_bytes());
    rec(0x00E0, &d)
}

fn boundsheet(pos: u32, state: u8, name: &str) -> Vec<u8> {
    let mut d = pos.to_le_bytes().to_vec();
    d.push(state);
    d.push(0); // worksheet
    d.push(name.chars().count() as u8);
    let wide = name.chars().any(|c| c as u32 > 255);
    d.push(u8::from(wide));
    if wide {
        for u in name.encode_utf16() {
            d.extend(u.to_le_bytes());
        }
    } else {
        d.extend(name.chars().map(|c| c as u8));
    }
    rec(0x0085, &d)
}

fn sst(strings: &[&str]) -> Vec<u8> {
    let mut d = (strings.len() as u32).to_le_bytes().to_vec();
    d.extend((strings.len() as u32).to_le_bytes());
    for s in strings {
        d.extend(xl_string(s, false));
    }
    rec(0x00FC, &d)
}

fn dimensions(r0: u32, r1: u32, c0: u16, c1: u16) -> Vec<u8> {
    let mut d = r0.to_le_bytes().to_vec();
    d.extend(r1.to_le_bytes());
    d.extend(c0.to_le_bytes());
    d.extend(c1.to_le_bytes());
    d.extend([0, 0]);
    rec(0x0200, &d)
}

fn pos(row: u16, col: u16, ixfe: u16) -> Vec<u8> {
    let mut d = row.to_le_bytes().to_vec();
    d.extend(col.to_le_bytes());
    d.extend(ixfe.to_le_bytes());
    d
}

fn number(row: u16, col: u16, ixfe: u16, v: f64) -> Vec<u8> {
    let mut d = pos(row, col, ixfe);
    d.extend(v.to_le_bytes());
    rec(0x0203, &d)
}

/// An RK holding an integer (`value << 2 | 2`).
fn rk_int(row: u16, col: u16, ixfe: u16, v: i32) -> Vec<u8> {
    let mut d = pos(row, col, ixfe);
    d.extend(((v << 2) | 2).to_le_bytes());
    rec(0x027E, &d)
}

fn mulrk(row: u16, first: u16, cells: &[(u16, i32)]) -> Vec<u8> {
    let mut d = row.to_le_bytes().to_vec();
    d.extend(first.to_le_bytes());
    for (ixfe, v) in cells {
        d.extend(ixfe.to_le_bytes());
        d.extend(((v << 2) | 2).to_le_bytes());
    }
    d.extend((first + cells.len() as u16 - 1).to_le_bytes());
    rec(0x00BD, &d)
}

fn labelsst(row: u16, col: u16, ixfe: u16, isst: u32) -> Vec<u8> {
    let mut d = pos(row, col, ixfe);
    d.extend(isst.to_le_bytes());
    rec(0x00FD, &d)
}

fn boolerr(row: u16, col: u16, ixfe: u16, val: u8, is_err: bool) -> Vec<u8> {
    let mut d = pos(row, col, ixfe);
    d.push(val);
    d.push(u8::from(is_err));
    rec(0x0205, &d)
}

/// A FORMULA record with a numeric cached value and no tokens.
fn formula_num(row: u16, col: u16, ixfe: u16, v: f64) -> Vec<u8> {
    let mut d = pos(row, col, ixfe);
    d.extend(v.to_le_bytes());
    d.extend([0, 0]); // grbit
    d.extend([0, 0, 0, 0]); // chn
    d.extend([0, 0]); // cce
    rec(0x0006, &d)
}

struct Sheet {
    name: String,
    state: u8,
    records: Vec<Vec<u8>>,
}

fn sheet(name: &str, records: Vec<Vec<u8>>) -> Sheet {
    Sheet {
        name: name.into(),
        state: 0,
        records,
    }
}

struct Book {
    /// FORMAT / XF / DATEMODE ... records of the globals, in order.
    globals: Vec<Vec<u8>>,
    sheets: Vec<Sheet>,
}

impl Book {
    fn stream(&self) -> Vec<u8> {
        let mut head = bof(0x0600, 0x0005);
        for g in &self.globals {
            head.extend_from_slice(g);
        }
        // BOUNDSHEET records have a fixed size per name, so the positions can be computed first.
        let sheets_len: usize = self
            .sheets
            .iter()
            .map(|s| boundsheet(0, s.state, &s.name).len())
            .sum();
        let globals_len = head.len() + sheets_len + eof().len();
        let mut bodies = Vec::new();
        let mut positions = Vec::new();
        for s in &self.sheets {
            positions.push((globals_len + bodies.len()) as u32);
            bodies.extend(bof(0x0600, 0x0010));
            for r in &s.records {
                bodies.extend_from_slice(r);
            }
            bodies.extend(eof());
        }
        for (s, p) in self.sheets.iter().zip(positions) {
            head.extend(boundsheet(p, s.state, &s.name));
        }
        head.extend(eof());
        head.extend(bodies);
        head
    }

    fn write(&self, dir: &konoma_test_support::TmpDir, name: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        write_cfb(&p, &[("/Workbook", &self.stream())]);
        p
    }
}

/// The usual globals: three style XFs, then cell XFs for `ifmts` (cell xf index = 3 + i).
fn globals(formats: &[(u16, &str)], ifmts: &[u16]) -> Vec<Vec<u8>> {
    let mut g: Vec<Vec<u8>> = formats.iter().map(|(i, c)| format_rec(*i, c)).collect();
    for _ in 0..3 {
        g.push(xf_rec(0));
    }
    g.extend(ifmts.iter().map(|&i| xf_rec(i)));
    g
}

fn cell_text(wb: &Workbook, r: usize, c: usize) -> String {
    wb.sheets[0].display(r, c).to_string()
}

// ---------------------------------------------------------------------------------------------
// formats reach the display text through load_workbook
// ---------------------------------------------------------------------------------------------

#[test]
fn builtin_and_custom_formats_apply_to_every_cell_record_kind() {
    let book = Book {
        // xf 3: #,##0.00 (id 4) / xf 4: 0% (id 9) / xf 5: custom 164 `0.0%` / xf 6: date id 14
        globals: globals(&[(164, "0.0%")], &[4, 9, 164, 14]),
        sheets: vec![sheet(
            "S",
            vec![
                dimensions(0, 4, 0, 6),
                number(0, 0, 3, 1234.5),
                number(0, 1, 4, 0.256),
                number(0, 2, 5, 0.256),
                number(0, 3, 6, 46297.0),
                rk_int(1, 0, 3, 7),
                mulrk(1, 1, &[(4, 1), (5, 1), (3, 9)]),
                labelsst(2, 0, 0, 0),
                boolerr(2, 1, 0, 1, false),
                boolerr(2, 2, 0, 0x07, true),
                formula_num(3, 0, 3, 0.5),
            ],
        )],
    };
    let dir = tmp("xls_kinds");
    // The SST goes in the globals (before the BOUNDSHEETs).
    let mut book = book;
    book.globals.push(sst(&["hello"]));
    let p = book.write(&dir, "k.xls");
    let wb = load(&p).unwrap();
    let got = |r, c| cell_text(&wb, r, c);
    assert_eq!(got(0, 0), "1,234.50", "NUMBER, built-in 4");
    assert_eq!(got(0, 1), "26%", "NUMBER, built-in 9");
    assert_eq!(got(0, 2), "25.6%", "NUMBER, custom 164");
    assert_eq!(got(0, 3), "10/2/2026", "NUMBER, built-in 14 in English");
    assert_eq!(got(1, 0), "7.00", "RK");
    assert_eq!(got(1, 1), "100%", "MULRK 1");
    assert_eq!(got(1, 2), "100.0%", "MULRK 2");
    assert_eq!(got(1, 3), "9.00", "MULRK 3");
    assert_eq!(got(2, 0), "hello", "LABELSST");
    assert_eq!(got(2, 1), "TRUE", "BOOLERR bool");
    assert_eq!(got(2, 2), "#DIV/0!", "BOOLERR error");
    assert_eq!(got(3, 0), "0.50", "FORMULA numeric result");
    assert_eq!(
        wb.sheets[0].cell(2, 2).unwrap().cell_type(),
        CellType::Error
    );
}

#[test]
fn a_built_in_date_format_follows_the_locale() {
    let book = Book {
        globals: globals(&[], &[14, 22]),
        sheets: vec![sheet(
            "S",
            vec![number(0, 0, 3, 46297.0), number(0, 1, 4, 46297.5)],
        )],
    };
    let dir = tmp("xls_locale");
    let p = book.write(&dir, "l.xls");
    let en = load_with(&p, Limits::default()).unwrap();
    assert_eq!(cell_text(&en, 0, 0), "10/2/2026");
    let ja = load_workbook(
        &p,
        &LoadOptions {
            locale: Locale::Ja,
            limits: Limits::default(),
        },
    )
    .unwrap();
    assert_eq!(cell_text(&ja, 0, 0), "2026/10/2");
    assert_eq!(cell_text(&ja, 0, 1), "2026/10/2 12:00");
}

#[test]
fn wide_format_strings_and_sheet_names_are_utf16() {
    let book = Book {
        globals: globals(&[(170, "ggge\"年\"m\"月\"d\"日\"")], &[170]),
        sheets: vec![sheet("売上", vec![number(0, 0, 3, 46297.0)])],
    };
    let dir = tmp("xls_wide");
    let p = book.write(&dir, "w.xls");
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets[0].name, "売上");
    assert_eq!(cell_text(&wb, 0, 0), "令和8年10月2日");
}

#[test]
fn xf_indexes_count_style_xfs_too() {
    // ixfe 0..2 are style XFs (General); the first cell XF is ixfe 3. An ixfe past the table is
    // General, never a panic.
    let book = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![
                number(0, 0, 0, 5.0),
                number(0, 1, 3, 5.0),
                number(0, 2, 4, 5.0),
                number(0, 3, 0xFFFF, 5.0),
            ],
        )],
    };
    let dir = tmp("xls_xfidx");
    let p = book.write(&dir, "x.xls");
    let wb = load(&p).unwrap();
    let got: Vec<String> = (0..4).map(|c| cell_text(&wb, 0, c)).collect();
    assert_eq!(got, ["5", "5.00", "5", "5"]);
}

#[test]
fn a_format_record_overrides_a_built_in_id() {
    // Excel writes FORMAT records for locale-dependent built-ins; the file's definition wins.
    let book = Book {
        globals: globals(&[(14, "yyyy-mm-dd")], &[14]),
        sheets: vec![sheet("S", vec![number(0, 0, 3, 46297.0)])],
    };
    let dir = tmp("xls_override");
    let p = book.write(&dir, "o.xls");
    assert_eq!(cell_text(&load(&p).unwrap(), 0, 0), "2026-10-02");
}

#[test]
fn datemode_selects_the_1904_system() {
    let mut g = globals(&[(164, "yyyy-mm-dd")], &[164]);
    g.insert(0, rec(0x0022, &1u16.to_le_bytes()));
    let book = Book {
        globals: g,
        sheets: vec![sheet("S", vec![number(0, 0, 3, 0.0)])],
    };
    let dir = tmp("xls_1904");
    let p = book.write(&dir, "d.xls");
    let wb = load(&p).unwrap();
    assert!(wb.date1904);
    assert_eq!(cell_text(&wb, 0, 0), "1904-01-01");
    // And the same serial in the 1900 system is a different day.
    let mut g = globals(&[(164, "yyyy-mm-dd")], &[164]);
    g.insert(0, rec(0x0022, &0u16.to_le_bytes()));
    let book = Book {
        globals: g,
        sheets: vec![sheet("S", vec![number(0, 0, 3, 61.0)])],
    };
    let p = book.write(&dir, "d0.xls");
    let wb = load(&p).unwrap();
    assert!(!wb.date1904);
    assert_eq!(cell_text(&wb, 0, 0), "1900-03-01");
}

#[test]
fn continue_records_do_not_disturb_the_cells_around_them() {
    let book = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![
                number(0, 0, 3, 1.0),
                rec(0x003C, &[1, 2, 3, 4, 5, 6, 7, 8]),
                number(0, 1, 3, 2.0),
            ],
        )],
    };
    let dir = tmp("xls_continue");
    let p = book.write(&dir, "c.xls");
    let wb = load(&p).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "1.00");
    assert_eq!(cell_text(&wb, 0, 1), "2.00");
}

#[test]
fn a_cell_before_dimensions_and_unsorted_cells_still_get_their_format() {
    let book = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![
                number(2, 2, 3, 3.0),
                number(0, 1, 3, 1.0),
                number(1, 0, 3, 2.0),
            ],
        )],
    };
    let dir = tmp("xls_unsorted");
    let p = book.write(&dir, "u.xls");
    let wb = load(&p).unwrap();
    assert_eq!(cell_text(&wb, 0, 1), "1.00");
    assert_eq!(cell_text(&wb, 1, 0), "2.00");
    assert_eq!(cell_text(&wb, 2, 2), "3.00");
}

#[test]
fn the_pass_reports_what_it_read() {
    let g = globals(&[(164, "0.0%")], &[4, 164]);
    let book = Book {
        globals: g,
        sheets: vec![sheet(
            "S",
            vec![
                number(0, 0, 3, 1.0),
                number(2, 4, 4, 1.0),
                number(5, 5, 0, 1.0),
            ],
        )],
    };
    let fm = fmt_xls::parse_stream(&book.stream(), &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.value_cells, 3);
    assert_eq!(sf.bbox, Some((0, 0, 5, 5)));
    assert_eq!(sf.cells.len(), 2, "General cells are not listed");
    assert_eq!(fm.formats.len(), 3);
    assert!(!fm.date1904);
}

// ---------------------------------------------------------------------------------------------
// the memory checks (calamine builds every sheet while opening)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_far_flung_cell_is_refused_before_calamine_allocates() {
    // Row 65,535 x column 255 plus A1: a 65,536 x 256 = 16.8M-cell box > the 16M budget.
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![number(0, 0, 0, 1.0), number(65535, 255, 0, 1.0)],
        )],
    };
    let dir = tmp("xls_far");
    let p = book.write(&dir, "f.xls");
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
    // Columns beyond Excel's last (a hostile col 65,535) count in the box too.
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![number(0, 0, 0, 1.0), number(65535, 65535, 0, 1.0)],
        )],
    };
    let p = book.write(&dir, "f2.xls");
    assert!(matches!(
        load(&p),
        Err(OfficeError::TooLarge { what: "sheet area" })
    ));
}

#[test]
fn the_budget_is_exact_at_the_boundary() {
    let limits = small_limits(); // 10,000 cells
    let make = |rows: u16, cols: u16| {
        let book = Book {
            globals: globals(&[], &[]),
            sheets: vec![sheet(
                "S",
                vec![number(0, 0, 0, 1.0), number(rows - 1, cols - 1, 0, 1.0)],
            )],
        };
        fmt_xls::parse_stream(&book.stream(), &limits)
    };
    assert!(make(100, 100).is_ok(), "exactly 10,000");
    assert!(
        matches!(
            make(100, 101),
            Err(OfficeError::TooLarge { what: "sheet area" })
        ),
        "10,100"
    );
}

#[test]
fn a_hidden_sheet_counts_too() {
    let mut hidden = sheet(
        "Hidden",
        vec![number(0, 0, 0, 1.0), number(65535, 255, 0, 1.0)],
    );
    hidden.state = 1;
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![number(0, 0, 0, 1.0)]), hidden],
    };
    let dir = tmp("xls_hidden");
    let p = book.write(&dir, "h.xls");
    // calamine parses hidden sheets while opening, so a hidden bomb refuses the file.
    assert!(matches!(load(&p), Err(OfficeError::TooLarge { .. })));
}

#[test]
fn a_huge_declared_dimension_is_refused_even_with_one_cell() {
    // calamine reserves rows x cols cells from DIMENSIONS (the shape of its OOM_alloc2 case).
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![dimensions(0, 0xFFFF_FFF0, 0, 200), number(0, 0, 0, 1.0)],
        )],
    };
    let dir = tmp("xls_dims");
    let p = book.write(&dir, "d.xls");
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
    // A DIMENSIONS record of a sensible size is fine.
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![dimensions(0, 10, 0, 5), number(0, 0, 0, 1.0)],
        )],
    };
    let p = book.write(&dir, "d2.xls");
    assert!(load(&p).is_ok());
}

#[test]
fn the_cell_count_budget_is_enforced() {
    let limits = Limits {
        max_dense_cells: 5,
        ..Limits::default()
    };
    let cells: Vec<Vec<u8>> = (0..6).map(|c| number(0, c, 0, 1.0)).collect();
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", cells)],
    };
    assert_eq!(
        fmt_xls::parse_stream(&book.stream(), &limits).unwrap_err(),
        OfficeError::TooLarge {
            what: "sheet cells"
        }
    );
}

// ---------------------------------------------------------------------------------------------
// broken records
// ---------------------------------------------------------------------------------------------

fn limits() -> Limits {
    Limits::default()
}

#[test]
fn malformed_records_are_errors_not_panics() {
    let g = |extra: Vec<u8>| {
        let mut v = bof(0x0600, 5);
        v.extend(boundsheet(0, 0, "S")); // patched below
        v.extend(eof());
        let at = v.len();
        v.extend(bof(0x0600, 0x10));
        v.extend(extra);
        v.extend(eof());
        // Point the sheet at its substream.
        let bs = 4 + 20; // after the BOF record
        v[bs..bs + 4].copy_from_slice(&(at as u32).to_le_bytes());
        v
    };
    // MULRK whose declared column span does not match its body.
    let mut bad = mulrk(0, 0, &[(0, 1), (0, 2)]);
    let n = bad.len();
    bad[n - 2] = 9;
    assert!(matches!(
        fmt_xls::parse_stream(&g(bad), &limits()),
        Err(OfficeError::Corrupt(_))
    ));
    // MULRK with last < first.
    let mut bad = mulrk(0, 5, &[(0, 1)]);
    let n = bad.len();
    bad[n - 2] = 1;
    assert!(fmt_xls::parse_stream(&g(bad), &limits()).is_err());
    // A cell record too short for row/col/ixfe.
    assert!(matches!(
        fmt_xls::parse_stream(&g(rec(0x0203, &[0, 0, 0])), &limits()),
        Err(OfficeError::Corrupt(_))
    ));
    // MULRK shorter than its header.
    assert!(fmt_xls::parse_stream(&g(rec(0x00BD, &[0, 0])), &limits()).is_err());
    // DIMENSIONS of a length that exists in no BIFF version.
    assert!(fmt_xls::parse_stream(&g(rec(0x0200, &[0; 11])), &limits()).is_err());
    // A record that claims more bytes than the stream has.
    let mut cut = g(number(0, 0, 0, 1.0));
    cut.truncate(cut.len() - 6);
    assert!(fmt_xls::parse_stream(&cut, &limits()).is_err());
    // A stream that ends in the middle of a record header.
    assert!(fmt_xls::parse_stream(&[0x09, 0x08, 0x10], &limits()).is_err());
    // A sheet position past the end of the stream.
    let mut v = bof(0x0600, 5);
    v.extend(boundsheet(0x00FF_FFFF, 0, "S"));
    v.extend(eof());
    assert!(matches!(
        fmt_xls::parse_stream(&v, &limits()),
        Err(OfficeError::Corrupt(_))
    ));
    // Empty stream: no sheets, no panic.
    assert!(fmt_xls::parse_stream(&[], &limits())
        .unwrap()
        .sheets
        .is_empty());
}

#[test]
fn short_format_and_boundsheet_records_are_skipped() {
    let mut v = bof(0x0600, 5);
    v.extend(rec(0x041E, &[0x01])); // FORMAT without a code
    v.extend(rec(0x041E, &[0xA4, 0x00, 0xFF, 0xFF, 0x01])); // cch 65,535 but no characters
    v.extend(rec(0x00E0, &[0, 0])); // XF too short for ifmt
    v.extend(rec(0x0085, &[1, 2, 3])); // BOUNDSHEET too short
    v.extend(eof());
    let fm = fmt_xls::parse_stream(&v, &limits()).unwrap();
    assert!(fm.sheets.is_empty());
}

#[test]
fn filepass_with_an_encryption_type_is_encrypted() {
    let mut v = bof(0x0600, 5);
    v.extend(rec(0x002F, &[1, 0, 0, 0]));
    v.extend(eof());
    assert_eq!(
        fmt_xls::parse_stream(&v, &limits()).unwrap_err(),
        OfficeError::Encrypted
    );
    // Type 0 (XOR obfuscation off) is not encryption, as in calamine.
    let mut v = bof(0x0600, 5);
    v.extend(rec(0x002F, &[0, 0, 0, 0]));
    v.extend(eof());
    assert!(fmt_xls::parse_stream(&v, &limits()).is_ok());
}

#[test]
fn biff5_strings_are_single_byte() {
    // BIFF5/7: FORMAT is `ifmt: u16, cch: u8, bytes`; BOUNDSHEET is `pos, state, type, cch: u8, bytes`.
    let mut v = bof(0x0500, 5);
    let mut fmt = vec![0xA4, 0x00, 5];
    fmt.extend(b"0.00%");
    v.extend(rec(0x041E, &fmt));
    v.extend(xf_rec(0));
    v.extend(xf_rec(164));
    let at_bs = v.len();
    let mut bs = 0u32.to_le_bytes().to_vec();
    bs.extend([0, 0, 3]);
    bs.extend(b"Old");
    v.extend(rec(0x0085, &bs));
    v.extend(eof());
    let sheet_at = v.len();
    v.extend(bof(0x0500, 0x10));
    v.extend(number(0, 0, 1, 0.5));
    v.extend(eof());
    v[at_bs + 4..at_bs + 8].copy_from_slice(&(sheet_at as u32).to_le_bytes());
    let fm = fmt_xls::parse_stream(&v, &limits()).unwrap();
    assert!(fm.sheets.contains_key("Old"));
    assert_eq!(
        fm.formats,
        vec![NumFmtRef::General, NumFmtRef::Custom("0.00%".into())]
    );
    assert_eq!(fm.sheets["Old"].cells, vec![(0, 0, 1)]);
}

#[test]
fn every_prefix_of_a_synthetic_workbook_stream_is_handled() {
    let book = Book {
        globals: globals(&[(164, "0.0%")], &[4, 164]),
        sheets: vec![sheet(
            "S",
            vec![
                dimensions(0, 3, 0, 3),
                number(0, 0, 3, 1.0),
                mulrk(1, 0, &[(3, 1), (4, 2)]),
                rk_int(2, 0, 4, 3),
            ],
        )],
    };
    let stream = book.stream();
    let dir = tmp("xls_prefix");
    for n in 0..stream.len() {
        // The pass directly (no safety net)...
        let r = std::panic::catch_unwind(|| fmt_xls::parse_stream(&stream[..n], &limits()));
        assert!(r.is_ok(), "the pass panicked on a {n}-byte prefix");
        // ...and the whole loader on a valid compound file holding that prefix.
        let p = dir.join("p.xls");
        write_cfb(&p, &[("/Workbook", &stream[..n])]);
        let _ = assert_inner_does_not_panic(&p, &format!("stream prefix {n}"));
    }
}

#[test]
fn mutated_workbook_streams_never_panic_in_konomas_pass() {
    let book = Book {
        globals: globals(&[(164, "0.0%")], &[4, 164]),
        sheets: vec![sheet(
            "S",
            vec![
                dimensions(0, 3, 0, 3),
                number(0, 0, 3, 1.0),
                mulrk(1, 0, &[(3, 1), (4, 2)]),
                rk_int(2, 0, 4, 3),
                labelsst(3, 0, 0, 0),
            ],
        )],
    };
    let orig = book.stream();
    let mut seed = 0x1357_9BDF_2468_ACE0u64;
    for round in 0..400u64 {
        let mut b = orig.clone();
        let kind = xorshift(&mut seed);
        mutate(&mut b, &mut seed, kind);
        let r = std::panic::catch_unwind(|| fmt_xls::parse_stream(&b, &small_limits()));
        assert!(r.is_ok(), "the pass panicked in round {round}");
    }
}

#[test]
fn a_book_stream_name_is_found() {
    // Old files call the stream `Book`; calamine also tries upper-case spellings.
    let book = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet("S", vec![number(0, 0, 3, 2.0)])],
    };
    let dir = tmp("xls_bookname");
    let p = dir.join("b.xls");
    write_cfb(&p, &[("/Book", &book.stream())]);
    let wb = load(&p).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "2.00");
}

#[test]
fn a_workbook_stream_over_the_part_limit_is_refused() {
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![number(0, 0, 0, 1.0)])],
    };
    let dir = tmp("xls_big");
    let p = book.write(&dir, "b.xls");
    let r = load_with(
        &p,
        Limits {
            max_part_bytes: 50,
            ..Limits::default()
        },
    );
    assert_eq!(r.unwrap_err(), OfficeError::TooLarge { what: "entry" });
}

#[test]
fn the_real_sample_goes_through_the_pass() {
    let Some(p) = konoma_test_support::sample_path_or_skip("sample.xls") else {
        return;
    };
    let fm = fmt_xls::read(&p, &Limits::default(), None).unwrap();
    assert!(!fm.date1904);
    assert!(fm.sheets.contains_key("Sales"));
    assert!(
        fm.sheets["Sales"].cells.len() >= 6,
        "{:?}",
        fm.sheets["Sales"]
    );
    // calamine builds the hidden sheet too: it is in the map.
    assert!(fm.sheets.contains_key("Hidden"));
}

// ---------------------------------------------------------------------------------------------
// memory: the formula STRING record and the text budget
// ---------------------------------------------------------------------------------------------

/// A formula's text result (`STRING`, 0x0207): it has no position; `calamine` puts it at the last
/// FORMULA's cell, or at A1 when no FORMULA came before.
fn formula_string(s: &str) -> Vec<u8> {
    rec(0x0207, &xl_string(s, false))
}

#[test]
fn a_formula_string_with_no_formula_lands_at_a1_and_widens_the_box() {
    // One number at (60000, 60000): a 1-cell box. A STRING record before any FORMULA is put at
    // A1 by calamine, so its matrix would span 3.6 billion cells.
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![number(60_000, 60_000, 0, 1.0), formula_string("x")],
        )],
    };
    let dir = tmp("xls_string_a1");
    let p = book.write(&dir, "s.xls");
    let sf = fmt_xls::read(&p, &Limits::default(), None).unwrap_err();
    assert_eq!(sf, OfficeError::TooLarge { what: "sheet area" });
    // Without the STRING record the same sheet is fine (one cell).
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![number(60_000, 60_000, 0, 1.0)])],
    };
    let p = book.write(&dir, "n.xls");
    assert_eq!(
        fmt_xls::read(&p, &Limits::default(), None).unwrap().sheets["S"].value_cells,
        1
    );
}

#[test]
fn a_formula_string_follows_the_last_formula_cell() {
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![
                number(0, 0, 0, 1.0),
                formula_num(60_000, 60_000, 0, 1.0),
                formula_string("x"),
            ],
        )],
    };
    let dir = tmp("xls_string_after_formula");
    let p = book.write(&dir, "s.xls");
    // The FORMULA alone already spans the box; both are counted (3 cells booked).
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn sst_lengths_follow_continue_records_and_their_flag_bytes() {
    let mut body = vec![0u8; 8];
    // "abcdef": 6 chars, 8-bit flags; "abc" here, "def" as 16-bit in the CONTINUE.
    body.extend(6u16.to_le_bytes());
    body.push(0);
    body.extend(b"abc");
    let mut cont = vec![1u8]; // the continuation's own flags byte: 16-bit
    for u in "def".encode_utf16() {
        cont.extend(u.to_le_bytes());
    }
    // "xy" narrow, then a rich string (1 char, 2 runs) and a string with a 5-byte extension.
    cont.extend(2u16.to_le_bytes());
    cont.push(0);
    cont.extend(b"xy");
    cont.extend(1u16.to_le_bytes());
    cont.push(0x08);
    cont.extend(2u16.to_le_bytes());
    cont.push(b'r');
    cont.extend([0u8; 8]);
    cont.extend(1u16.to_le_bytes());
    cont.push(0x04);
    cont.extend(5u32.to_le_bytes());
    cont.push(b'e');
    cont.extend([0u8; 5]);
    let (lens, total) = fmt_xls::sst_lengths(&[&body, &cont], &Limits::default()).unwrap();
    assert_eq!(lens, vec![3 + 6, 2, 1, 1]);
    assert_eq!(total, 9 + 2 + 1 + 1 + 4 * 32);
    // A table cut in the middle of a string is corrupt, not a panic.
    let cut = &cont[..4];
    assert!(matches!(
        fmt_xls::sst_lengths(&[&body, cut], &Limits::default()),
        Err(OfficeError::Corrupt(_))
    ));
}

#[test]
fn one_long_shared_string_used_by_many_cells_is_refused_by_the_text_budget() {
    // 1,000 characters x 150 LABELSST references = 150 KB against a 100 KB budget, from a ~3 KB
    // stream (small enough for calamine's mini-stream path in the control case).
    let cells = |n: u16| (0..n).map(|r| labelsst(r, 0, 0, 0)).collect::<Vec<_>>();
    let mk = |n: u16| Book {
        globals: {
            let mut g = globals(&[], &[]);
            g.push(sst(&[&"s".repeat(1000)]));
            g
        },
        sheets: vec![sheet("S", cells(n))],
    };
    let dir = tmp("xls_amplify");
    let limits = Limits {
        max_rows: 1000,
        max_sheet_cells: 100_000,
        max_text_bytes: 100_000,
        ..small_limits()
    };
    let p = mk(150).write(&dir, "a.xls");
    assert_eq!(
        load_with(&p, limits).unwrap_err(),
        OfficeError::TooLarge { what: "text" }
    );
    let p = mk(50).write(&dir, "b.xls");
    let wb = load_with(&p, limits).unwrap();
    assert_eq!(wb.sheets[0].display(49, 0).len(), 1000);
}

#[test]
fn label_and_rstring_text_count_against_the_text_budget() {
    let limits = Limits {
        max_text_bytes: 100_000,
        ..small_limits()
    };
    let label = |r: u16, typ: u16| {
        let mut d = pos(r, 0, 0);
        d.extend(xl_string(&"l".repeat(3000), false));
        rec(typ, &d)
    };
    let dir = tmp("xls_labels");
    for typ in [0x0204u16, 0x00D6] {
        let book = Book {
            globals: globals(&[], &[]),
            sheets: vec![sheet("S", (0..50).map(|r| label(r, typ)).collect())],
        };
        let p = book.write(&dir, "l.xls");
        assert_eq!(
            load_with(&p, limits).unwrap_err(),
            OfficeError::TooLarge { what: "text" },
            "{typ:#x}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// mutation survivors: DIMENSIONS area, MULRK length, the XF cap
// ---------------------------------------------------------------------------------------------

/// A BIFF5-style DIMENSIONS record: four u16 (`rwMic`, `rwMac`, `colMic`, `colMac`) and a
/// reserved word.
fn dimensions_short(r0: u16, r1: u16, c0: u16, c1: u16) -> Vec<u8> {
    let mut d = Vec::new();
    for v in [r0, r1, c0, c1, 0] {
        d.extend(v.to_le_bytes());
    }
    rec(0x0200, &d)
}

/// Parses a book that has only a DIMENSIONS record under the given cell budget.
fn parse_with_dimensions(dim: Vec<u8>, max_dense_cells: u64) -> Result<(), OfficeError> {
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![dim])],
    };
    let limits = Limits {
        max_dense_cells,
        ..Limits::default()
    };
    fmt_xls::parse_stream(&book.stream(), &limits).map(|_| ())
}

#[test]
fn the_declared_area_is_rows_times_columns_and_the_limit_is_exact() {
    // MS-XLS 2.4.90 Dimensions: rwMic/colMic are the first row/column, rwMac/colMac are one past
    // the last, so the area is (rwMac - rwMic) x (colMac - colMic).
    let too_large = Err(OfficeError::TooLarge { what: "sheet area" });
    // Rows 0..10, columns 2..5: 10 x 3 = 30 (not 10 x 5 = 50).
    assert_eq!(parse_with_dimensions(dimensions(0, 10, 2, 5), 30), Ok(()));
    assert_eq!(
        parse_with_dimensions(dimensions(0, 10, 2, 5), 29),
        too_large
    );
    // The first row counts too: rows 4..14 are 10 rows.
    assert_eq!(parse_with_dimensions(dimensions(4, 14, 0, 3), 30), Ok(()));
    assert_eq!(
        parse_with_dimensions(dimensions(4, 14, 0, 3), 29),
        too_large
    );
    // The same shapes in the 10-byte (BIFF5/7) record.
    assert_eq!(
        parse_with_dimensions(dimensions_short(0, 10, 2, 5), 30),
        Ok(())
    );
    assert_eq!(
        parse_with_dimensions(dimensions_short(0, 10, 2, 5), 29),
        too_large
    );
}

#[test]
fn a_dimensions_record_with_nonsense_columns_falls_back_to_the_full_width() {
    // colMac < colMic (a damaged or hand-made file): the first column is taken as 0, so the area
    // is rows x colMac, and nothing underflows.
    let too_large = Err(OfficeError::TooLarge { what: "sheet area" });
    assert_eq!(parse_with_dimensions(dimensions(0, 10, 5, 2), 20), Ok(()));
    assert_eq!(
        parse_with_dimensions(dimensions(0, 10, 5, 2), 19),
        too_large
    );
    // colMic past column 255 (the BIFF8 maximum) is treated the same way.
    assert_eq!(
        parse_with_dimensions(dimensions(0, 10, 300, 400), 4000),
        Ok(())
    );
    assert_eq!(
        parse_with_dimensions(dimensions(0, 10, 300, 400), 3999),
        too_large
    );
    // No rows or no columns declared: one cell (a sheet is never "empty" to the reader).
    assert_eq!(parse_with_dimensions(dimensions(0, 0, 0, 5), 1), Ok(()));
    assert_eq!(parse_with_dimensions(dimensions(0, 5, 0, 0), 1), Ok(()));
}

#[test]
fn a_mulrk_whose_body_is_longer_than_its_declared_span_is_corrupt() {
    // Two RK cells (columns 0 and 1) but `colLast` says 0: the declared span (1 column) does not
    // match the body (2 cells). The body being too short is corrupt as well; only an exact match
    // is a MULRK.
    let corrupt = |data: Vec<u8>| {
        let book = Book {
            globals: globals(&[], &[]),
            sheets: vec![sheet("S", vec![data])],
        };
        fmt_xls::parse_stream(&book.stream(), &Limits::default())
    };
    let mut longer = mulrk(0, 0, &[(0, 1), (0, 2)]);
    let n = longer.len();
    longer[n - 2..].copy_from_slice(&0u16.to_le_bytes());
    assert!(
        matches!(corrupt(longer), Err(OfficeError::Corrupt(_))),
        "body longer than the span"
    );
    let mut shorter = mulrk(0, 0, &[(0, 1)]);
    let n = shorter.len();
    shorter[n - 2..].copy_from_slice(&1u16.to_le_bytes());
    assert!(
        matches!(corrupt(shorter), Err(OfficeError::Corrupt(_))),
        "body shorter than the span"
    );
    // The exact match is accepted.
    assert!(corrupt(mulrk(0, 3, &[(0, 1), (0, 2), (0, 3)])).is_ok());
}

#[test]
fn only_the_first_200_000_xf_records_are_kept() {
    // The 200,000th XF is kept, the 200,001st is dropped. An XF's format shows up in the format
    // table only if the record was kept (a cell can reference at most xf 65,535, so the table is
    // what shows the cap).
    let xf_bytes = |n: usize, last_ifmt: u16| {
        let mut v = bof(0x0600, 0x0005);
        for _ in 0..n - 1 {
            v.extend(xf_rec(0));
        }
        v.extend(xf_rec(last_ifmt));
        v.extend(eof());
        v
    };
    let kept = fmt_xls::parse_stream(&xf_bytes(200_000, 14), &Limits::default()).unwrap();
    assert!(
        kept.formats.contains(&NumFmtRef::Builtin(14)),
        "{:?}",
        kept.formats
    );
    let dropped = fmt_xls::parse_stream(&xf_bytes(200_001, 14), &Limits::default()).unwrap();
    assert_eq!(dropped.formats, vec![NumFmtRef::General]);
}

#[test]
fn a_format_code_over_255_characters_is_dropped_and_255_is_kept() {
    let keep = format!("0.{}", "0".repeat(253));
    let drop = format!("0.{}", "0".repeat(254));
    let book = Book {
        globals: globals(&[(164, &keep), (165, &drop)], &[164, 165]),
        sheets: vec![sheet("S", vec![number(0, 0, 3, 0.5), number(0, 1, 4, 0.5)])],
    };
    let dir = tmp("xls_longcode");
    let p = book.write(&dir, "l.xls");
    let wb = load(&p).unwrap();
    assert!(
        cell_text(&wb, 0, 0).starts_with("0.5000"),
        "the 255-character code applies"
    );
    assert_eq!(
        cell_text(&wb, 0, 1),
        "0.5",
        "256 characters is not a format Excel wrote"
    );
    assert_eq!(
        wb.formats,
        vec![NumFmtRef::General, NumFmtRef::Custom(keep.as_str().into())]
    );
}

#[test]
fn only_the_sheet_asked_for_is_taken_out_of_the_workbook() {
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![
            sheet("A", vec![number(0, 0, 3, 1.0)]),
            sheet("B", vec![number(0, 0, 3, 2.0)]),
        ],
    };
    let dir = tmp("xls_one");
    let p = book.write(&dir, "o.xls");
    let wb = load_workbook_sheet(&p, &LoadOptions::default(), 1).unwrap();
    assert_eq!(wb.loaded_index(), Some(1));
    assert_eq!(wb.sheets[1].display(0, 0), "2");
    assert!(!wb.sheets[0].loaded);
    assert_eq!(wb.sheets[0].name, "A");
}

/// Every sheet's matrix stays in memory once `calamine` has opened the file, so the budget is for
/// the workbook: sheets that are each within it can still be too many together.
#[test]
fn the_area_budget_is_for_the_whole_workbook_not_each_sheet() {
    let limits = small_limits(); // 10,000 cells
    let book = |areas: &[(u16, u16)]| Book {
        globals: globals(&[], &[]),
        sheets: areas
            .iter()
            .enumerate()
            .map(|(i, &(rows, cols))| {
                sheet(
                    &format!("S{i}"),
                    vec![number(0, 0, 0, 1.0), number(rows - 1, cols - 1, 0, 1.0)],
                )
            })
            .collect(),
    };
    let parse = |b: &Book| fmt_xls::parse_stream(&b.stream(), &limits);
    assert!(
        parse(&book(&[(100, 90)])).is_ok(),
        "one sheet under the budget"
    );
    assert!(
        parse(&book(&[(100, 50), (100, 50)])).is_ok(),
        "5,000 + 5,000 is exactly the budget"
    );
    assert_eq!(
        parse(&book(&[(100, 50), (100, 51)])).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" },
        "one cell over"
    );
    let many = book(&[(10, 100); 12]);
    assert_eq!(
        parse(&many).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" },
        "12 sheets of 1,000 cells each"
    );
    // A hidden sheet is counted like any other.
    let mut b = book(&[(100, 60), (100, 60)]);
    b.sheets[1].state = 1;
    assert_eq!(
        parse(&b).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

// ---------------------------------------------------------------------------------------------
// sheets that share records (the format pass and `calamine` read each sheet to its first EOF)
// ---------------------------------------------------------------------------------------------

/// A stream whose BOUNDSHEET records are written as given: `(position offset into the sheet
/// bodies, name)`. The bodies are `bodies` (concatenated), starting right after the globals.
fn stream_with_sheet_positions(positions: &[usize], bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut head = bof(0x0600, 0x0005);
    head.extend(xf_rec(0));
    let sheets_len: usize = positions
        .iter()
        .enumerate()
        .map(|(i, _)| boundsheet(0, 0, &format!("S{i}")).len())
        .sum();
    let globals_len = head.len() + sheets_len + eof().len();
    for (i, p) in positions.iter().enumerate() {
        head.extend(boundsheet((globals_len + p) as u32, 0, &format!("S{i}")));
    }
    head.extend(eof());
    for b in bodies {
        head.extend_from_slice(b);
    }
    head
}

fn body_with_numbers(n: usize) -> Vec<u8> {
    let mut b = bof(0x0600, 0x0010);
    for i in 0..n {
        b.extend(number(0, (i % 200) as u16, 0, 1.0));
    }
    b.extend(eof());
    b
}

#[test]
fn many_boundsheets_at_the_same_position_are_refused_not_scanned_once_each() {
    // 3,000 sheets (under the sheet cap), one body of 20,000 records: 60 million record reads
    // if each sheet is scanned.
    let body = body_with_numbers(20_000);
    let positions = vec![0usize; 3_000];
    let s = stream_with_sheet_positions(&positions, &[body]);
    let t = std::time::Instant::now();
    let r = fmt_xls::parse_stream(&s, &Limits::default());
    assert!(
        matches!(
            r,
            Err(OfficeError::Corrupt(_)) | Err(OfficeError::TooLarge { .. })
        ),
        "{r:?}"
    );
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

#[test]
fn two_sheets_at_the_same_position_are_refused() {
    let s = stream_with_sheet_positions(&[0, 0], &[body_with_numbers(3)]);
    assert!(matches!(
        fmt_xls::parse_stream(&s, &Limits::default()),
        Err(OfficeError::Corrupt(m)) if m.contains("share")
    ));
}

#[test]
fn a_sheet_that_starts_inside_another_is_refused() {
    // The second BOUNDSHEET points into the middle of the first sheet's records.
    let body = body_with_numbers(10);
    let s = stream_with_sheet_positions(&[0, bof(0x0600, 0x0010).len() + 18 * 3], &[body]);
    assert!(matches!(
        fmt_xls::parse_stream(&s, &Limits::default()),
        Err(OfficeError::Corrupt(m)) if m.contains("share")
    ));
}

#[test]
fn sheets_one_after_another_are_accepted_in_any_boundsheet_order() {
    let a = body_with_numbers(3);
    let b = body_with_numbers(4);
    let c = body_with_numbers(5);
    let (pa, pb, pc) = (0, a.len(), a.len() + b.len());
    // BOUNDSHEET order is not stream order (Excel writes them in tab order, which a file can
    // reorder); all that matters is that the bodies do not overlap.
    let s = stream_with_sheet_positions(&[pc, pa, pb], &[a, b, c]);
    let fm = fmt_xls::parse_stream(&s, &Limits::default()).unwrap();
    assert_eq!(fm.sheets.len(), 3);
}

#[test]
fn a_workbook_cannot_have_more_than_the_most_sheets() {
    // One empty sheet body, a distinct position each is impossible, so use bodies of one record.
    let bodies: Vec<Vec<u8>> = (0..4_097)
        .map(|_| {
            let mut b = bof(0x0600, 0x0010);
            b.extend(eof());
            b
        })
        .collect();
    let mut positions = Vec::new();
    let mut at = 0;
    for b in &bodies {
        positions.push(at);
        at += b.len();
    }
    let s = stream_with_sheet_positions(&positions, &bodies);
    assert_eq!(
        fmt_xls::parse_stream(&s, &Limits::default()).unwrap_err(),
        OfficeError::TooLarge { what: "sheets" }
    );
    // 4,096 is allowed.
    let s = stream_with_sheet_positions(&positions[..4_096], &bodies[..4_096]);
    assert!(fmt_xls::parse_stream(&s, &Limits::default()).is_ok());
}

#[test]
fn a_cancelled_format_pass_stops_within_a_sheet() {
    use super::workbook::Cancel;
    // A body of 40,000 records (past the check interval), cancelled from the start.
    let body = body_with_numbers(40_000);
    let s = stream_with_sheet_positions(&[0], &[body]);
    let cancel = Cancel::new(|| true);
    assert!(fmt_xls::parse_stream_cancellable(&s, &Limits::default(), Some(&cancel)).is_err());
    // Not cancelled: fine.
    let never = Cancel::new(|| false);
    assert!(fmt_xls::parse_stream_cancellable(&s, &Limits::default(), Some(&never)).is_ok());
}

#[test]
fn the_loader_refuses_the_shared_sheet_file_before_calamine_sees_it() {
    let body = body_with_numbers(20_000);
    let s = stream_with_sheet_positions(&vec![0usize; 3_000], &[body]);
    let dir = tmp("xls_shared_sheets");
    let p = dir.join("shared.xls");
    write_cfb(&p, &[("/Workbook", &s)]);
    let t = std::time::Instant::now();
    let r = load(&p);
    assert!(
        matches!(
            r,
            Err(OfficeError::Corrupt(_)) | Err(OfficeError::TooLarge { .. })
        ),
        "{r:?}"
    );
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

// ---------------------------------------------------------------------------------------------
// nested substreams (embedded charts): `calamine` 0.37 skips them and ends the sheet at its own EOF
// ---------------------------------------------------------------------------------------------

/// An embedded chart: a nested `BOF`..`EOF` substream holding `inside`.
fn chart(inside: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut v = vec![bof(0x0600, 0x0020)];
    v.extend(inside);
    v.push(eof());
    v
}

fn book_of(records: Vec<Vec<u8>>) -> Book {
    Book {
        globals: globals(&[(164, "0.0%")], &[4, 164]),
        sheets: vec![sheet("S", records)],
    }
}

#[test]
fn a_far_cell_after_an_embedded_chart_is_refused_before_calamine_runs() {
    let mut recs = vec![dimensions(0, 1, 0, 1), number(0, 0, 3, 1.0)];
    recs.extend(chart(vec![]));
    recs.push(number(65535, 255, 0, 1.0));
    let book = book_of(recs);
    assert_eq!(
        fmt_xls::parse_stream(&book.stream(), &Limits::default()).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
    let dir = tmp("xls_chart_far");
    let p = book.write(&dir, "c.xls");
    assert_eq!(
        load(&p).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn a_huge_dimensions_after_an_embedded_chart_is_refused() {
    let mut recs = vec![dimensions(0, 1, 0, 1), number(0, 0, 3, 1.0)];
    recs.extend(chart(vec![]));
    recs.push(dimensions(0, u32::MAX, 0, 255));
    assert_eq!(
        fmt_xls::parse_stream(&book_of(recs).stream(), &Limits::default()).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn records_inside_a_nested_substream_are_not_cells_and_nesting_is_tracked_by_depth() {
    // A chart inside a chart: only the depth-0 EOF ends the sheet, so the far cell after the
    // inner EOF but still inside the outer chart is skipped, and the one after both is seen.
    let mut inner = vec![bof(0x0600, 0x0020), number(60000, 200, 3, 1.0), eof()];
    inner.push(number(61000, 201, 3, 1.0));
    let mut recs = vec![number(0, 0, 3, 1.0)];
    recs.extend(chart(inner));
    let sf_book = book_of(recs.clone());
    let fm = fmt_xls::parse_stream(&sf_book.stream(), &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.value_cells, 1, "nothing inside the chart is a cell");
    assert_eq!(sf.bbox, Some((0, 0, 0, 0)));
    recs.push(number(65535, 255, 0, 1.0));
    assert_eq!(
        fmt_xls::parse_stream(&book_of(recs).stream(), &Limits::default()).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn a_chart_cache_does_not_override_the_format_of_a_real_cell() {
    // Cell xf 3 is `0.00`-ish (ifmt 4), xf 4 is `0.0%`. The chart's cached value for (0,0) has
    // xf 4; the real A1 has xf 3 and must keep its own format.
    let mut recs = vec![number(0, 0, 3, 1.0)];
    recs.extend(chart(vec![number(0, 0, 4, 9.0)]));
    let book = book_of(recs);
    let fm = fmt_xls::parse_stream(&book.stream(), &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.cells.len(), 1);
    assert_eq!(sf.value_cells, 1);
    let own = sf.format_at(0, 0);
    // Reversed order: the chart first, the cell after it.
    let mut recs = chart(vec![number(0, 0, 4, 9.0)]);
    recs.push(number(0, 0, 3, 1.0));
    let fm = fmt_xls::parse_stream(&book_of(recs).stream(), &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"].format_at(0, 0), own);
    assert_ne!(own, 0);
    // End to end: A1 shows the sheet's own format.
    let dir = tmp("xls_chart_fmt");
    let mut recs = vec![number(0, 0, 3, 1.0)];
    recs.extend(chart(vec![number(0, 0, 4, 9.0)]));
    let p = book_of(recs).write(&dir, "f.xls");
    let wb = load(&p).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "1.00");
}

#[test]
fn duplicate_records_for_one_cell_resolve_to_the_last_one() {
    // Seven records for A1, each with its own format: a lookup that lands on any duplicate other
    // than the last (a binary search may) gives a different format.
    let book = Book {
        globals: globals(&[], &[4, 5, 6, 7, 8, 9, 10]),
        sheets: vec![sheet(
            "S",
            (3..10).map(|ixfe| number(0, 0, ixfe, 1.0)).collect(),
        )],
    };
    let fm = fmt_xls::parse_stream(&book.stream(), &Limits::default()).unwrap();
    let sf = &fm.sheets["S"];
    assert_eq!(sf.cells.len(), 7);
    let last = sf.cells[6].2;
    assert!(sf.cells[..6].iter().all(|c| c.2 != last));
    assert_eq!(sf.format_at(0, 0), last);
}

#[test]
fn a_16_byte_dimensions_record_is_read_like_the_14_byte_one() {
    let mut d = Vec::new();
    d.extend(0u32.to_le_bytes());
    d.extend(10u32.to_le_bytes());
    d.extend(2u16.to_le_bytes());
    d.extend(5u16.to_le_bytes());
    d.extend([0u8; 4]);
    assert_eq!(d.len(), 16);
    let dim = rec(0x0200, &d);
    let too_large = Err(OfficeError::TooLarge { what: "sheet area" });
    assert_eq!(parse_with_dimensions(dim.clone(), 30), Ok(()));
    assert_eq!(parse_with_dimensions(dim, 29), too_large);
}

// ---------------------------------------------------------------------------------------------
// which stream of the compound file is the workbook: the one `calamine` reads
// ---------------------------------------------------------------------------------------------

/// A one-sheet book with a cell at `(row, col)` as well as A1.
fn book_with_far_cell(row: u16, col: u16) -> Book {
    Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![number(0, 0, 3, 1.0), number(row, col, 3, 2.0)],
        )],
    }
}

fn harmless_book() -> Book {
    Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet("S", vec![number(0, 0, 3, 7.0)])],
    }
}

#[test]
fn a_workbook_in_a_sub_storage_never_reaches_calamine() {
    // `calamine` would take the first directory entry named `Workbook` wherever it sits, i.e. the
    // one with the far cell. `calamine` is now given a container built from the scanned root
    // `Book` only, so the far cell is neither allocated nor refused: it is simply not there.
    let dir = tmp("xls_substorage");
    let p = dir.join("s.xls");
    let mut cf = cfb::create(&p).unwrap();
    cf.create_storage("/X").unwrap();
    cf.create_stream("/X/Workbook")
        .unwrap()
        .write_all(&book_with_far_cell(3000, 20).stream())
        .unwrap();
    cf.create_stream("/Book")
        .unwrap()
        .write_all(&harmless_book().stream())
        .unwrap();
    cf.flush().unwrap();
    drop(cf);
    let wb = load_with(&p, small_limits()).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "7.00");
}

#[test]
fn calamine_is_given_a_container_of_exactly_the_scanned_bytes() {
    // The seam: whatever the file looked like, the container is one root `Workbook` stream with
    // the scanned bytes, and nothing else.
    let stream = harmless_book().stream();
    let c = fmt_xls::clean_container(&stream).unwrap();
    let mut cf = cfb::CompoundFile::open(c).unwrap();
    let entries: Vec<String> = cf
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| e.path().display().to_string())
        .collect();
    assert_eq!(entries, ["/Workbook"]);
    let mut got = Vec::new();
    std::io::Read::read_to_end(&mut cf.open_stream("/Workbook").unwrap(), &mut got).unwrap();
    assert_eq!(got, stream);
}

#[test]
fn the_first_workbook_entry_in_the_directory_wins() {
    let dir = tmp("xls_first_entry");
    let p = dir.join("f.xls");
    let mut cf = cfb::create(&p).unwrap();
    cf.create_stream("/Workbook")
        .unwrap()
        .write_all(&harmless_book().stream())
        .unwrap();
    cf.create_storage("/X").unwrap();
    cf.create_stream("/X/Workbook")
        .unwrap()
        .write_all(&book_with_far_cell(3000, 20).stream())
        .unwrap();
    cf.flush().unwrap();
    drop(cf);
    let wb = load_with(&p, small_limits()).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "7.00");
}

/// Renames the stream entry called `victim` to `Workbook` and unlinks it from the directory tree
/// (its parent now points at the victim's only child, if any), so it is an orphan that only a flat
/// scan of the directory array finds. The directory must fit in the sector with the root entry.
fn make_orphan_workbook(bytes: &mut [u8], victim: &str) {
    const NO: u32 = 0xFFFF_FFFF;
    let name16 = |s: &str| -> Vec<u8> {
        let mut v: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        v.extend([0, 0]);
        v
    };
    let find = |bytes: &[u8], name: &str| -> usize {
        let n = name16(name);
        let hits: Vec<usize> = (0..bytes.len() - 128)
            .step_by(128)
            .filter(|&o| {
                bytes[o..o + n.len()] == n[..]
                    && bytes[o + n.len()..o + 64].iter().all(|&b| b == 0)
                    && u16::from_le_bytes([bytes[o + 64], bytes[o + 65]]) as usize == n.len()
            })
            .collect();
        assert_eq!(hits.len(), 1, "{name}: {hits:?}");
        hits[0]
    };
    let root = find(bytes, "Root Entry");
    let at = find(bytes, victim);
    assert!(at > root && (at - root) / 128 < 32);
    let idx = ((at - root) / 128) as u32;
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let (l, r) = (u32_at(bytes, at + 68), u32_at(bytes, at + 72));
    assert!(l == NO || r == NO, "the victim has two children");
    let replacement = if l == NO { r } else { l };
    for e in 0..32 {
        let o = root + e * 128;
        if o == at || o + 128 > bytes.len() {
            continue;
        }
        for field in [68, 72, 76] {
            if u32_at(bytes, o + field) == idx {
                bytes[o + field..o + field + 4].copy_from_slice(&replacement.to_le_bytes());
            }
        }
    }
    bytes[at + 68..at + 76].copy_from_slice(&[0xFF; 8]);
    let mut name = name16("Workbook");
    let len = name.len() as u16;
    name.resize(64, 0);
    bytes[at..at + 64].copy_from_slice(&name);
    bytes[at + 64..at + 66].copy_from_slice(&len.to_le_bytes());
}

#[test]
fn an_orphan_workbook_entry_before_the_real_one_is_the_one_scanned() {
    // `A` (a far cell) is created first, so it has the smaller directory index; it is then renamed
    // `Workbook` and cut out of the tree. The tree walk finds the harmless root `Workbook`;
    // `calamine`'s flat scan finds the orphan first.
    let dir = tmp("xls_orphan");
    let p = dir.join("o.xls");
    write_cfb(
        &p,
        &[
            ("/A", &book_with_far_cell(3000, 20).stream()),
            ("/Workbook", &harmless_book().stream()),
        ],
    );
    let mut bytes = std::fs::read(&p).unwrap();
    make_orphan_workbook(&mut bytes, "A");
    // The tree no longer reaches the orphan: only the real `Workbook` is left in it.
    let cf = cfb::CompoundFile::open(std::io::Cursor::new(bytes.clone())).unwrap();
    assert_eq!(cf.walk().filter(|e| e.is_stream()).count(), 1);
    drop(cf);
    std::fs::write(&p, &bytes).unwrap();
    // `calamine`'s flat scan would find the orphan; it is given the real `Workbook` only.
    let wb = load_with(&p, small_limits()).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "7.00");
}

#[test]
fn a_huge_dimensions_in_a_stream_calamine_is_never_given() {
    let dir = tmp("xls_substorage_dims");
    let p = dir.join("d.xls");
    let big = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![dimensions(0, 5000, 0, 100), number(0, 0, 3, 1.0)],
        )],
    };
    let mut cf = cfb::create(&p).unwrap();
    cf.create_storage("/X").unwrap();
    cf.create_stream("/X/Workbook")
        .unwrap()
        .write_all(&big.stream())
        .unwrap();
    cf.create_stream("/Book")
        .unwrap()
        .write_all(&harmless_book().stream())
        .unwrap();
    cf.flush().unwrap();
    drop(cf);
    let wb = load_with(&p, small_limits()).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "7.00");
}

#[test]
fn the_four_stream_names_are_found_at_the_root() {
    // Whatever the spelling in the file, `calamine` gets the stream as `Workbook`.
    let dir = tmp("xls_names");
    for name in ["/Workbook", "/Book", "/WORKBOOK", "/BOOK"] {
        let p = dir.join(format!("{}.xls", name.trim_start_matches('/')));
        write_cfb(&p, &[(name, &harmless_book().stream())]);
        assert!(load(&p).is_ok(), "{name}");
    }
    let p = dir.join("none.xls");
    write_cfb(&p, &[("/Other", &harmless_book().stream())]);
    assert!(load(&p).is_err());
}

#[test]
fn small_and_large_streams_in_both_sector_sizes_are_read_byte_for_byte() {
    use cfb::Version;
    let dir = tmp("xls_cfb_sizes");
    for version in [Version::V3, Version::V4] {
        for n in [1usize, 63, 64, 65, 4095, 4096, 4097, 20_000, 150_000] {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 + i / 251) as u8).collect();
            let p = dir.join(format!("s{n}.xls"));
            let mut cf =
                cfb::CompoundFile::create_with_version(version, std::fs::File::create(&p).unwrap())
                    .unwrap();
            cf.create_stream("/Workbook")
                .unwrap()
                .write_all(&data)
                .unwrap();
            // A second stream so that the chains are not trivially contiguous.
            cf.create_stream("/Other")
                .unwrap()
                .write_all(&data)
                .unwrap();
            cf.flush().unwrap();
            drop(cf);
            let lim = |n: u64| Limits {
                max_part_bytes: n,
                ..Limits::default()
            };
            let got = fmt_xls::read_workbook_stream(&p, &lim(1 << 30)).unwrap();
            assert_eq!(got, data, "{version:?} {n}");
            assert_eq!(
                fmt_xls::read_workbook_stream(&p, &lim(n as u64 - 1)).unwrap_err(),
                OfficeError::TooLarge { what: "entry" }
            );
            // And the container rebuilt from them holds the same bytes.
            let mut cf = cfb::CompoundFile::open(fmt_xls::clean_container(&got).unwrap()).unwrap();
            let mut again = Vec::new();
            std::io::Read::read_to_end(&mut cf.open_stream("/Workbook").unwrap(), &mut again)
                .unwrap();
            assert_eq!(again, data, "{version:?} {n} rebuilt");
        }
    }
}

#[test]
fn a_looping_root_chain_with_huge_lengths_is_harmless() {
    // The root entry's (mini stream) chain loops on itself with a 4 GiB length and the header
    // claims a mini FAT: `calamine`'s own reader would append sectors until it ran out of memory.
    // The stream is under 4096 bytes so that a mini stream exists and the root really has a chain
    // to loop. Whatever the container reader here makes of it, `calamine` never sees the file.
    let dir = tmp("xls_cfb_loop");
    let p = dir.join("l.xls");
    let small = harmless_book().stream();
    assert!(small.len() < 4096, "{}", small.len());
    write_cfb(&p, &[("/Workbook", &small)]);
    let mut bytes = std::fs::read(&p).unwrap();
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let ss = if u16::from_le_bytes([bytes[30], bytes[31]]) == 9 {
        512
    } else {
        4096
    };
    let dir_start = u32_at(&bytes, 48) as usize;
    let root = ss + dir_start * ss;
    // Root entry: start sector at 116, size at 120.
    let root_start = u32_at(&bytes, root + 116) as usize;
    assert!(
        root_start < 0xFFFF_FFF0,
        "the root must own a mini stream: {root_start:#x}"
    );
    let fat_sector = u32_at(&bytes, 76) as usize;
    let at = ss + fat_sector * ss + root_start * 4;
    bytes[at..at + 4].copy_from_slice(&(root_start as u32).to_le_bytes());
    // A multiple of 64, which `cfb` insists on for the root.
    bytes[root + 120..root + 124].copy_from_slice(&0xFFFF_FFC0u32.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes()); // mini FAT sector count
    std::fs::write(&p, &bytes).unwrap();
    assert_eq!(u32_at(&std::fs::read(&p).unwrap(), at), root_start as u32);
    let t = std::time::Instant::now();
    let r = std::panic::catch_unwind(|| load_with(&p, small_limits()));
    let r = r.expect("panicked");
    assert!(t.elapsed().as_secs() < 5);
    // `cfb` follows the root chain when it reads the stream out of the mini stream and finds the
    // loop: refused as a damaged file, quickly, and never handed to `calamine`.
    assert!(
        matches!(r, Err(OfficeError::Corrupt(ref m)) if m.contains("duplicate sector")),
        "{r:?}"
    );
}

#[test]
fn a_short_last_sector_cannot_make_calamine_read_other_bytes() {
    // A file whose last sector is cut short: calamine's reader zero-pads it on later reads. It
    // never reads the file; the scanned stream is either complete or the load is refused.
    let dir = tmp("xls_cfb_short");
    let p = dir.join("t.xls");
    let big = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            (0..2000u16).map(|i| number(i, 0, 3, 1.0)).collect(),
        )],
    };
    write_cfb(&p, &[("/Workbook", &big.stream())]);
    let bytes = std::fs::read(&p).unwrap();
    for cut in [1usize, 7, 100, 300] {
        std::fs::write(&p, &bytes[..bytes.len() - cut]).unwrap();
        // A guard rather than a discriminating test: the cut lands in the padding after the
        // stream (the sectors that matter are earlier), and `cfb` gives the same bytes as before.
        // What it pins is that the load ends in the one expected outcome, never a panic or a
        // different workbook.
        let wb = load_with(&p, small_limits()).unwrap_or_else(|e| panic!("cut {cut}: {e:?}"));
        assert_eq!(cell_text(&wb, 0, 0), "1.00", "cut {cut}");
    }
}

#[test]
fn a_bom_prefixed_workbook_entry_never_reaches_calamine() {
    // `calamine` decodes entry names with a BOM-sniffing decoder, so `FF FE "Workbook"` is
    // `Workbook` to it. The names are ordered so that `cfb` accepts the file (the shorter
    // `Workbook` first); only the scanned stream is handed over, whatever the names decode to.
    let dir = tmp("xls_cfb_bom");
    let p = dir.join("b.xls");
    write_cfb(
        &p,
        &[
            ("/Workbook", &harmless_book().stream()),
            ("/ZZZZZZZZZ", &book_with_far_cell(3000, 20).stream()),
        ],
    );
    let mut bytes = std::fs::read(&p).unwrap();
    let name: Vec<u8> = "ZZZZZZZZZ"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let at = (512..bytes.len() - 128)
        .step_by(128)
        .find(|&o| bytes[o..o + 18] == name[..] && bytes[o + 18..o + 20] == [0, 0])
        .expect("the ZZZZZZZZZ entry");
    let mut n: Vec<u8> = vec![0xFF, 0xFE]; // U+FEFF
    n.extend("Workbook".encode_utf16().flat_map(u16::to_le_bytes));
    n.extend([0, 0]);
    assert_eq!(n.len(), 20);
    bytes[at..at + 20].copy_from_slice(&n);
    std::fs::write(&p, &bytes).unwrap();
    // The container reader accepts the names (so this exercises the clean-container path, not a
    // refusal) and the real `Workbook` is the one loaded: the far cell is not there.
    let wb = load_with(&p, small_limits()).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "7.00");
}

// ---------------------------------------------------------------------------------------------
// the last record at a cell wins, for the format too
// ---------------------------------------------------------------------------------------------

#[test]
fn a_later_general_record_replaces_an_earlier_format_at_the_same_cell() {
    // Cell xf 3 has a format, xf 0 is General (ixfe 0 -> format 0).
    let dated_then_general = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet("S", vec![number(0, 0, 3, 1.0), number(0, 0, 0, 2.0)])],
    };
    let fm = fmt_xls::parse_stream(&dated_then_general.stream(), &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"].format_at(0, 0), 0);
    // The other way round, the format stays.
    let general_then_dated = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet("S", vec![number(0, 0, 0, 2.0), number(0, 0, 3, 1.0)])],
    };
    let fm = fmt_xls::parse_stream(&general_then_dated.stream(), &Limits::default()).unwrap();
    assert_ne!(fm.sheets["S"].format_at(0, 0), 0);
    // And through MULRK: a RK run over a cell that had a format, and a format over a run.
    let mulrk_over = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![number(0, 1, 3, 1.0), mulrk(0, 0, &[(0, 1), (0, 2)])],
        )],
    };
    let fm = fmt_xls::parse_stream(&mulrk_over.stream(), &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"].format_at(0, 1), 0);
}

#[test]
fn an_in_order_sheet_writes_no_explicit_general_entries_and_a_return_to_a_cell_does() {
    // Row-major records, some General: no earlier record can exist, so nothing extra is stored.
    let in_order = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![
                number(0, 0, 0, 1.0),
                number(0, 1, 3, 1.0),
                number(0, 2, 0, 1.0),
                number(1, 0, 0, 1.0),
                number(1, 1, 3, 1.0),
            ],
        )],
    };
    let fm = fmt_xls::parse_stream(&in_order.stream(), &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"].cells.len(), 2, "{:?}", fm.sheets["S"].cells);
    // A (formatted), then an earlier cell B, then a General record back at A: A must be General.
    let back = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![
                number(0, 5, 3, 1.0),
                number(0, 1, 3, 1.0),
                number(0, 5, 0, 2.0),
            ],
        )],
    };
    let fm = fmt_xls::parse_stream(&back.stream(), &Limits::default()).unwrap();
    assert_eq!(fm.sheets["S"].format_at(0, 5), 0);
    assert_ne!(fm.sheets["S"].format_at(0, 1), 0);
}

#[test]
fn a_formula_string_keeps_the_format_of_its_formula() {
    let book = Book {
        globals: globals(&[], &[4]),
        sheets: vec![sheet(
            "S",
            vec![formula_num(0, 0, 3, 1.0), formula_string("text")],
        )],
    };
    let fm = fmt_xls::parse_stream(&book.stream(), &Limits::default()).unwrap();
    assert_ne!(fm.sheets["S"].format_at(0, 0), 0);
}

#[test]
fn duplicates_resolve_to_the_last_record_over_a_long_reversed_run() {
    // 100 rows in descending order (so the sort has real work to do), two records each with
    // different formats, the last one alternating: an unstable sort swaps some duplicates.
    let mut recs = Vec::new();
    let mut last_only = Vec::new();
    for r in (0..100u16).rev() {
        let (first, last) = if r % 2 == 0 { (3, 4) } else { (4, 3) };
        recs.push(number(r, 0, first, 1.0));
        recs.push(number(r, 0, last, 2.0));
        last_only.push(number(r, 0, last, 2.0));
    }
    let make = |recs: Vec<Vec<u8>>| Book {
        globals: globals(&[], &[4, 5]),
        sheets: vec![sheet("S", recs)],
    };
    let both = fmt_xls::parse_stream(&make(recs).stream(), &Limits::default()).unwrap();
    let only = fmt_xls::parse_stream(&make(last_only).stream(), &Limits::default()).unwrap();
    for r in 0..100 {
        assert_eq!(
            both.sheets["S"].format_at(r, 0),
            only.sheets["S"].format_at(r, 0),
            "row {r}"
        );
    }
    assert_ne!(
        only.sheets["S"].format_at(0, 0),
        only.sheets["S"].format_at(1, 0)
    );
}

// ---------------------------------------------------------------------------------------------
// a sheet after a sheet whose nested substream never closes
// ---------------------------------------------------------------------------------------------

#[test]
fn a_sheet_after_a_chart_without_its_eof_is_read_like_calamine_does() {
    // Sheet 1: BOF, a cell, a chart BOF that never gets its EOF. `calamine` skips from the chart
    // BOF to the end of the stream (sheet 2's BOF/EOF nest inside) and still opens sheet 2 from
    // its own BOUNDSHEET position.
    let mut body1 = bof(0x0600, 0x0010);
    body1.extend(number(0, 0, 0, 1.0));
    body1.extend(bof(0x0600, 0x0020));
    let body2 = body_with_numbers(3);
    let s = stream_with_sheet_positions(&[0, body1.len()], &[body1, body2]);
    let fm = fmt_xls::parse_stream(&s, &Limits::default()).unwrap();
    assert_eq!(fm.sheets.len(), 2);
    assert_eq!(fm.sheets["S0"].value_cells, 1);
    assert_eq!(fm.sheets["S1"].value_cells, 3);
    // End to end (calamine opens it too).
    let dir = tmp("xls_open_chart");
    let p = dir.join("c.xls");
    write_cfb(&p, &[("/Workbook", &s)]);
    let wb = load(&p).unwrap();
    assert_eq!(wb.sheets.len(), 2);
}

#[test]
fn a_far_cell_in_a_sheet_after_an_unclosed_chart_is_still_refused() {
    let mut body1 = bof(0x0600, 0x0010);
    body1.extend(bof(0x0600, 0x0020));
    let mut body2 = bof(0x0600, 0x0010);
    body2.extend(number(0, 0, 0, 1.0));
    body2.extend(number(65535, 255, 0, 1.0));
    body2.extend(eof());
    let s = stream_with_sheet_positions(&[0, body1.len()], &[body1, body2]);
    assert_eq!(
        fmt_xls::parse_stream(&s, &Limits::default()).unwrap_err(),
        OfficeError::TooLarge { what: "sheet area" }
    );
}

#[test]
fn a_sheet_pointing_into_a_closed_chart_of_another_is_refused() {
    let mut body1 = bof(0x0600, 0x0010);
    body1.extend(number(0, 0, 0, 1.0));
    let chart_at = body1.len();
    body1.extend(bof(0x0600, 0x0020));
    body1.extend(number(0, 1, 0, 1.0));
    body1.extend(eof());
    body1.extend(eof());
    let s = stream_with_sheet_positions(&[0, chart_at], &[body1]);
    assert!(matches!(
        fmt_xls::parse_stream(&s, &Limits::default()),
        Err(OfficeError::Corrupt(m)) if m.contains("share")
    ));
}

#[test]
fn many_sheets_over_one_unclosed_body_are_refused_by_the_visit_budget() {
    // Each sheet is a nested substream that never closes, 20,000 records long; 3,000 of them at
    // one position would be 60 million record visits.
    let mut body = bof(0x0600, 0x0010);
    body.extend(bof(0x0600, 0x0020));
    for i in 0..20_000 {
        body.extend(number(0, (i % 200) as u16, 0, 1.0));
    }
    let s = stream_with_sheet_positions(&vec![0usize; 3_000], &[body]);
    let t = std::time::Instant::now();
    let r = fmt_xls::parse_stream(&s, &Limits::default());
    assert!(
        matches!(&r, Err(OfficeError::Corrupt(m)) if m.contains("share")),
        "{r:?}"
    );
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
}

// ---------------------------------------------------------------------------------------------
// the compound-file front door (`container::open_cfb` / `check_cfb_fat`)
// ---------------------------------------------------------------------------------------------

/// A reader that counts the bytes handed out, to pin that the check reads O(file length).
struct Counting<R> {
    inner: R,
    bytes: u64,
}
impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes += n as u64;
        Ok(n)
    }
}
impl<R: std::io::Seek> std::io::Seek for Counting<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

const FREE: u32 = 0xFFFF_FFFF;
const EOC: u32 = 0xFFFF_FFFE;

/// A version-3 compound file made only of a header and `difat_sectors` DIFAT sectors chained in
/// order; `entry(n)` gives the n-th FAT sector id listed by the chained sectors. Nothing else in
/// it is valid: the check must refuse it from the DIFAT alone.
fn difat_only_file(header_ids: &[u32], difat_sectors: u32, entry: impl Fn(u64) -> u32) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    h[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    h[26..28].copy_from_slice(&3u16.to_le_bytes());
    h[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    h[30..32].copy_from_slice(&9u16.to_le_bytes());
    h[32..34].copy_from_slice(&6u16.to_le_bytes());
    h[68..72].copy_from_slice(&(if difat_sectors > 0 { 0 } else { EOC }).to_le_bytes());
    h[72..76].copy_from_slice(&difat_sectors.to_le_bytes());
    for i in 0..109 {
        let id = header_ids.get(i).copied().unwrap_or(FREE);
        h[76 + 4 * i..80 + 4 * i].copy_from_slice(&id.to_le_bytes());
    }
    let mut n = 0u64;
    for k in 0..difat_sectors {
        for _ in 0..127 {
            h.extend(entry(n).to_le_bytes());
            n += 1;
        }
        let next = if k + 1 < difat_sectors { k + 1 } else { EOC };
        h.extend(next.to_le_bytes());
    }
    h
}

/// 109 different, valid FAT sector ids for the header (a file of 100+ sectors).
fn header_ids() -> Vec<u32> {
    (10..119).collect()
}

fn set_num_fat_sectors(bytes: &mut [u8], n: u32) {
    bytes[44..48].copy_from_slice(&n.to_le_bytes());
}

fn check_bytes(bytes: Vec<u8>) -> (Result<(), OfficeError>, u64, u64) {
    let len = bytes.len() as u64;
    let mut r = Counting {
        inner: std::io::Cursor::new(bytes),
        bytes: 0,
    };
    let res = container::check_cfb_fat(&mut r, len);
    (res, r.bytes, len)
}

#[test]
fn a_difat_that_lists_one_fat_sector_over_and_over_is_refused_without_the_blowup() {
    // Every sector of the file is a DIFAT sector naming sector 1: `cfb` would read 128 entries
    // for each of the ~38,000 ids (127 times the file) before noticing anything.
    let bytes = difat_only_file(&header_ids(), 300, |_| 1);
    let (res, read, len) = check_bytes(bytes.clone());
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
    assert!(
        read <= 2 * len,
        "the check read {read} bytes of a {len}-byte file"
    );
    // Through the real entry point, from a file: refused, never handed to `cfb`.
    let dir = tmp("cfb_blowup");
    let p = dir.join("b.xls");
    std::fs::write(&p, &bytes).unwrap();
    // The refusal is ours (the message says so), i.e. it came before `cfb` was called.
    assert!(
        matches!(container::open_cfb(&p), Err(OfficeError::Corrupt(ref m)) if m.contains("listed twice")),
    );
    assert!(matches!(
        container::inspect(&p, &Limits::default()),
        Err(OfficeError::Corrupt(_))
    ));
    assert!(matches!(
        fmt_xls::read_workbook_stream(&p, &Limits::default()),
        Err(OfficeError::Corrupt(_))
    ));
}

#[test]
fn a_difat_naming_every_sector_once_is_bounded_by_the_file_and_not_refused() {
    // 300 sectors, 300 different FAT ids (109 in the header): `cfb`'s FAT would be 300 x 512 bytes, i.e. the size of
    // the file, so this is not a blow-up and the check lets it through (whether `cfb` likes the
    // rest is its business). The repeats that make the blow-up are refused ("twice"), see above.
    let ids: Vec<u32> = (0..109).collect();
    let bytes = difat_only_file(&ids, 300, |n| if n < 191 { 109 + n as u32 } else { FREE });
    let (res, read, len) = check_bytes(bytes);
    assert!(res.is_ok(), "{res:?}");
    assert!(read <= 2 * len);
}

#[test]
fn an_over_allocated_fat_still_passes_the_check() {
    // 10 sectors listing 5 FAT sectors (a writer that reserved FAT sectors it did not fill): the
    // number of ids is under the number of sectors, which is all that bounds `cfb`'s FAT.
    let (res, _, _) = check_bytes(difat_only_file(&[1, 2, 3, 4, 5], 10, |_| FREE));
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn stale_ids_after_the_first_free_header_slot_are_ignored_like_cfb_does() {
    let (res, _, _) = check_bytes(difat_only_file(&[1, FREE, 1, 9_999], 10, |_| FREE));
    assert!(res.is_ok(), "{res:?}");
    // ... and the slots before it still count.
    let (res, _, _) = check_bytes(difat_only_file(&[1, 1, FREE], 10, |_| FREE));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
}

#[test]
fn zero_padding_in_a_chained_difat_sector_is_accepted_but_zeros_cfb_keeps_are_not() {
    // [0, 0, ...] with nothing after them and the header's count at 0: padding, stripped like
    // `cfb` does (down to the header's 109 slots, all of them FREE here).
    let (res, read, len) = check_bytes(difat_only_file(&[], 4, |_| 0));
    assert!(res.is_ok(), "{res:?}");
    assert!(read <= 2 * len);
    // A zero followed by a real id is FAT sector 0 and that sector.
    let (res, _, _) = check_bytes(difat_only_file(&header_ids(), 200, |n| match n {
        0 => 0,
        1 => 5,
        _ => FREE,
    }));
    assert!(res.is_ok(), "{res:?}");
    // Two zeros followed by an id list sector 0 twice.
    let (res, _, _) = check_bytes(difat_only_file(&header_ids(), 200, |n| match n {
        0 | 1 => 0,
        2 => 5,
        _ => FREE,
    }));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
}

#[test]
fn zeros_that_cfb_does_not_strip_are_fat_sector_zero_listed_again_shape_a() {
    // The header claims 0xFFFFFFFF FAT sectors, so `cfb` never strips the zero padding and reads
    // sector 0 as a FAT sector once per zero (128 FAT entries each: 127 x the file).
    let mut bytes = difat_only_file(&header_ids(), 300, |_| 0);
    set_num_fat_sectors(&mut bytes, 0xFFFF_FFFF);
    let (res, read, len) = check_bytes(bytes);
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
    assert!(read <= 2 * len);
}

#[test]
fn zeros_that_cfb_does_not_strip_are_fat_sector_zero_listed_again_shape_b() {
    // No header trick: the last slot of the chain is FREE, so `cfb` finds no trailing zero to
    // strip, and only then removes the trailing FREE. Every zero before it stays.
    let total = 300 * 127;
    let mut bytes = difat_only_file(
        &header_ids(),
        300,
        |n| if n == total - 1 { FREE } else { 0 },
    );
    let (res, _, _) = check_bytes(bytes.clone());
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
    // The same with an accurate-looking count: still refused (the count only matters for the
    // zeros at the very end).
    set_num_fat_sectors(&mut bytes, 110);
    let (res, _, _) = check_bytes(bytes);
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
}

/// The list the check evaluates, for a header + DIFAT layout.
fn fat_list(bytes: Vec<u8>) -> Result<Vec<u32>, OfficeError> {
    let len = bytes.len() as u64;
    container::cfb_fat_sector_list(&mut std::io::Cursor::new(bytes), len).map(|(l, _)| l)
}

#[test]
fn the_fat_list_is_what_cfb_builds_in_its_order_with_its_stripping_rules() {
    let hdr = header_ids();
    let with = |nfat: u32, entry: &dyn Fn(u64) -> u32| {
        let mut b = difat_only_file(&hdr, 2, entry);
        set_num_fat_sectors(&mut b, nfat);
        fat_list(b).unwrap()
    };
    let hdr_and = |extra: &[u32]| [hdr.clone(), extra.to_vec()].concat();
    // Padding zeros are stripped down to the header's count (110: one id from the chain).
    assert_eq!(with(110, &|n| if n == 0 { 77 } else { 0 }), hdr_and(&[77]));
    // A count under 109 changes nothing: the header's 109 slots are never stripped.
    assert_eq!(with(3, &|n| if n == 0 { 77 } else { 0 }), hdr_and(&[77]));
    // The count stops the stripping where it says: 112 entries, the last of them a zero.
    assert_eq!(
        with(112, &|n| match n {
            0 => 70,
            1 => 71,
            _ => 0,
        }),
        hdr_and(&[70, 71, 0])
    );
    // A count that is too big (or a lie) strips nothing: every zero stays (2 x 127 of them).
    let l = with(0xFFFF_FFFF, &|n| if n == 0 { 77 } else { 0 });
    assert_eq!(l.len(), 109 + 254);
    assert_eq!(l.iter().filter(|&&x| x == 0).count(), 253);
    // A FREE at the very end blocks the zero stripping and is then removed itself.
    let l = with(110, &|n| match n {
        0 => 77,
        253 => FREE,
        _ => 0,
    });
    assert_eq!(l.len(), 109 + 253);
    assert_eq!(l.iter().filter(|&&x| x == 0).count(), 252);
    // FREE entries before the end stay (and `check_cfb_fat` refuses them: past the end).
    let l = with(0, &|n| match n {
        0 => 77,
        1 => FREE,
        2 => 78,
        _ => FREE,
    });
    assert_eq!(l.len(), 109 + 3);
    assert_eq!(l[109 + 1], FREE);
    // The header: the first FREE slot ends it even when ids follow.
    let b = difat_only_file(&[5, 6, FREE, 7], 2, |_| FREE);
    assert_eq!(fat_list(b).unwrap(), vec![5, 6]);
    // The order is the header's 109 slots (FREE ones included), then each chained sector's in
    // chain order.
    let b = difat_only_file(&[5, 6], 2, |n| if n < 2 { 20 + n as u32 } else { FREE });
    let l = fat_list(b).unwrap();
    assert_eq!(l.len(), 109 + 2);
    assert_eq!(l[..2], [5, 6]);
    assert!(l[2..109].iter().all(|&x| x == FREE));
    assert_eq!(l[109..], [20, 21]);
}

#[test]
fn a_free_slot_in_the_middle_of_the_fat_list_is_refused() {
    // `cfb` would read the FAT sectors before it and then fail on this one.
    let (res, _, _) = check_bytes(difat_only_file(&[], 8, |n| match n {
        0 => 0,
        1 => 5,
        _ => FREE,
    }));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("past the end")),
        "{res:?}"
    );
}

#[test]
fn the_fat_list_of_real_files_is_what_the_writer_wrote_zero_padded_or_not() {
    // Parity with `cfb`: files cfb itself wrote (and opens), at sizes around the DIFAT limits,
    // padded with FREE as written and then with zeros (cfb issue 41): the list is the 109 header
    // slots plus the used DIFAT entries, and cfb still reads the stream back.
    let dir = tmp("cfb_fat_list_parity");
    let le = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    for (i, size) in [6_900_000usize, 7_100_000, 7_300_000, 7_450_000, 15_500_000]
        .into_iter()
        .enumerate()
    {
        let p = dir.join(format!("p{i}.xls"));
        let mut cf = cfb::CompoundFile::create_with_version(
            cfb::Version::V3,
            std::fs::File::create(&p).unwrap(),
        )
        .unwrap();
        cf.create_stream("/Workbook")
            .unwrap()
            .write_all(&vec![0xABu8; size])
            .unwrap();
        cf.flush().unwrap();
        drop(cf);
        let bytes = std::fs::read(&p).unwrap();
        let nfat = le(&bytes, 44) as usize;
        // Expected: header ids, then the chained entries that are not FREE, in order.
        let mut expected: Vec<u32> = (0..109)
            .map(|k| le(&bytes, 76 + 4 * k))
            .take_while(|&x| x != FREE)
            .collect();
        let mut next = le(&bytes, 68);
        let mut sectors = Vec::new();
        while next != EOC && next != FREE {
            sectors.push(next as usize);
            let at = (next as usize + 1) * 512;
            expected.extend(
                (0..127)
                    .map(|k| le(&bytes, at + 4 * k))
                    .filter(|&x| x != FREE),
            );
            next = le(&bytes, at + 508);
        }
        assert_eq!(expected.len(), nfat, "size {size}");
        assert_eq!(
            fat_list(bytes.clone()).unwrap(),
            expected,
            "written by cfb, size {size}"
        );
        let mut padded = bytes.clone();
        for &sec in &sectors {
            for k in 0..127 {
                let at = (sec + 1) * 512 + 4 * k;
                if le(&padded, at) == FREE {
                    padded[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
                }
            }
        }
        assert_eq!(
            fat_list(padded.clone()).unwrap(),
            expected,
            "zero padded, size {size}"
        );
        let q = dir.join(format!("q{i}.xls"));
        std::fs::write(&q, &padded).unwrap();
        let mut cf = container::open_cfb(&q).unwrap();
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut cf.open_stream("/Workbook").unwrap(), &mut out).unwrap();
        assert_eq!(out.len(), size);
        assert!(out.iter().all(|&b| b == 0xAB));
    }
}

#[test]
fn a_long_difat_chain_of_free_sectors_is_read_once_and_bounded_by_the_file() {
    let (res, read, len) = check_bytes(difat_only_file(&[], 300, |_| FREE));
    assert!(res.is_ok(), "{res:?}");
    assert!(read <= 2 * len, "read {read} of {len}");
}

#[test]
fn a_difat_chain_that_leaves_the_file_is_refused() {
    for next in [9_999u32, 0xFFFF_FFFB, 0xFFFF_FFFC] {
        let mut bytes = difat_only_file(&[], 2, |_| FREE);
        // The last DIFAT sector (sector 1) points past the end / to a reserved value.
        let at = 512 + 512 + 508;
        bytes[at..at + 4].copy_from_slice(&next.to_le_bytes());
        let (res, _, _) = check_bytes(bytes);
        assert!(
            matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("DIFAT sector past")),
            "{next:#x}: {res:?}"
        );
    }
}

#[test]
fn a_partial_last_sector_counts_as_a_sector_and_a_truncated_difat_sector_is_refused() {
    // Sector 1 is only 100 bytes long: it still exists for the FAT list (floor division would
    // call id 1 "past the end").
    let mut bytes = difat_only_file(&[1], 1, |_| FREE);
    bytes.extend([0u8; 100]);
    let (res, _, _) = check_bytes(bytes);
    assert!(res.is_ok(), "{res:?}");
    // The DIFAT sector itself cut short: refused as truncated (floor division would call it "past
    // the end").
    let mut bytes = difat_only_file(&[], 1, |_| FREE);
    bytes.truncate(512 + 412);
    let (res, _, _) = check_bytes(bytes);
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("truncated DIFAT")),
        "{res:?}"
    );
}

#[test]
fn a_real_size_file_with_a_zero_padded_difat_opens() {
    // The shape of `cfb` issue 41 at a real size: 110-116 FAT sectors (a file of about 7 MB) need
    // exactly one DIFAT sector, whose unused tail some writers fill with zeros instead of FREE.
    let dir = tmp("cfb_zero_padded_difat");
    let p = dir.join("z.xls");
    let mut cf = cfb::CompoundFile::create_with_version(
        cfb::Version::V3,
        std::fs::File::create(&p).unwrap(),
    )
    .unwrap();
    {
        let mut s = cf.create_stream("/Workbook").unwrap();
        s.write_all(&vec![0xABu8; 7_100_000]).unwrap();
    }
    cf.flush().unwrap();
    drop(cf);
    let mut bytes = std::fs::read(&p).unwrap();
    let le = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let difat_at = (le(&bytes, 68) as usize + 1) * 512;
    assert_eq!(le(&bytes, 72), 1, "one DIFAT sector");
    let mut used = 0;
    for i in 0..127 {
        let at = difat_at + 4 * i;
        if le(&bytes, at) == FREE {
            bytes[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        } else {
            used += 1;
        }
    }
    let fat_sectors = 109 + used;
    assert!(
        (110..=116).contains(&fat_sectors),
        "{fat_sectors} FAT sectors: adjust the stream size"
    );
    std::fs::write(&p, &bytes).unwrap();
    let mut cf = container::open_cfb(&p).unwrap();
    let mut n = 0;
    std::io::Read::read_to_end(&mut cf.open_stream("/Workbook").unwrap(), &mut vec![])
        .map(|k| n = k)
        .unwrap();
    assert_eq!(n, 7_100_000);
}

#[test]
fn a_repeated_id_in_the_header_difat_is_refused() {
    let (res, _, _) = check_bytes(difat_only_file(&[1, 2, 1], 3, |_| FREE));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
}

#[test]
fn a_fat_id_past_the_end_of_the_file_is_refused() {
    let (res, _, _) = check_bytes(difat_only_file(&[1, 9_999], 3, |_| FREE));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("past the end")),
        "{res:?}"
    );
    let (res, _, _) = check_bytes(difat_only_file(
        &[],
        3,
        |n| if n == 5 { 77_777 } else { FREE },
    ));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("past the end")),
        "{res:?}"
    );
}

#[test]
fn a_difat_chain_that_loops_is_refused() {
    let mut bytes = difat_only_file(&[], 2, |_| FREE);
    // Point the last DIFAT sector (sector 1) back at sector 0.
    let at = 512 + 512 + 508;
    bytes[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
    let (res, _, _) = check_bytes(bytes);
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("loops")),
        "{res:?}"
    );
}

#[test]
fn a_truncated_or_odd_header_is_refused_not_panicked_on() {
    let (res, _, _) = check_bytes(vec![0u8; 100]);
    assert!(matches!(res, Err(OfficeError::Corrupt(_))));
    let mut bytes = difat_only_file(&[], 0, |_| FREE);
    bytes[30] = 7;
    let (res, _, _) = check_bytes(bytes);
    assert!(matches!(res, Err(OfficeError::Corrupt(_))));
}

#[test]
fn every_real_compound_file_in_testdata_still_opens() {
    let dir = konoma_test_support::repo_root().join("testdata/office");
    let mut n = 0;
    for name in [
        "date1904.xls",
        "empty.xls",
        "encrypted.xls",
        "formats.xls",
        "formulas.xls",
        "layout.xls",
        "objects.xls",
        "pivot.xls",
        "sheets.xls",
        "encrypted.xlsx",
    ] {
        let p = dir.join(name);
        container::open_cfb(&p).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        n += 1;
    }
    assert_eq!(n, 10);
}

#[test]
fn a_big_valid_file_with_a_chained_difat_still_opens() {
    // More than 109 FAT sectors (109 x 128 sectors of 512 bytes = 7 MiB) needs real DIFAT
    // sectors: 9 MiB of stream does it.
    let dir = tmp("cfb_big_difat");
    let p = dir.join("big.xls");
    let mut cf = cfb::CompoundFile::create_with_version(
        cfb::Version::V3,
        std::fs::File::create(&p).unwrap(),
    )
    .unwrap();
    let chunk = vec![0xABu8; 1 << 20];
    {
        let mut s = cf.create_stream("/Workbook").unwrap();
        for _ in 0..9 {
            s.write_all(&chunk).unwrap();
        }
    }
    cf.flush().unwrap();
    drop(cf);
    let len = std::fs::metadata(&p).unwrap().len();
    let mut f = std::fs::File::open(&p).unwrap();
    let head_difat_count = {
        let mut h = [0u8; 76];
        std::io::Read::read_exact(&mut f, &mut h).unwrap();
        u32::from_le_bytes([h[72], h[73], h[74], h[75]])
    };
    assert!(
        head_difat_count >= 1,
        "the fixture must chain DIFAT sectors"
    );
    let mut cf = container::open_cfb(&p).unwrap();
    let mut n = 0;
    std::io::Read::read_to_end(&mut cf.open_stream("/Workbook").unwrap(), &mut vec![])
        .map(|k| n = k)
        .unwrap();
    assert_eq!(n, 9 << 20);
    assert!(len > 9 << 20);
}

#[test]
fn the_xls_stream_cap_is_the_part_cap_and_names_it() {
    let dir = tmp("xls_stream_cap");
    let p = dir.join("c.xls");
    write_cfb(&p, &[("Workbook", &harmless_book().stream())]);
    let n = harmless_book().stream().len() as u64;
    let lim = Limits {
        max_part_bytes: n - 1,
        ..Limits::default()
    };
    assert!(matches!(
        fmt_xls::read_workbook_stream(&p, &lim),
        Err(OfficeError::TooLarge { what: "entry" })
    ));
    let lim = Limits {
        max_part_bytes: n,
        ..Limits::default()
    };
    assert_eq!(
        fmt_xls::read_workbook_stream(&p, &lim).unwrap().len() as u64,
        n
    );
}

// ---------------------------------------------------------------------------------------------
// formula text: what `calamine` prints is counted first (`fmt_xls_formula`) and a formula that
// would cost too much is handed over without tokens
// ---------------------------------------------------------------------------------------------

use super::fmt_xls_formula;
use calamine::Reader as _;

/// A FORMULA record with a numeric cached value and the given tokens (`cce` is added).
fn formula_rec(row: u16, col: u16, ixfe: u16, v: f64, rgce: &[u8]) -> Vec<u8> {
    let mut d = pos(row, col, ixfe);
    d.extend(v.to_le_bytes());
    d.extend([0, 0]); // grbit
    d.extend([0, 0, 0, 0]); // chn
    d.extend((rgce.len() as u16).to_le_bytes());
    d.extend_from_slice(rgce);
    rec(0x0006, &d)
}

/// A NAME record: a defined name (ASCII) whose formula is `rgce`.
fn name_rec(name: &str, rgce: &[u8]) -> Vec<u8> {
    let mut d = vec![0, 0, 0, name.len() as u8];
    d.extend((rgce.len() as u16).to_le_bytes());
    d.extend([0; 4]); // ixals, itab
    d.extend([0; 4]); // menu, description, help, status text lengths
    d.push(0); // 8-bit characters
    d.extend(name.bytes());
    d.extend_from_slice(rgce);
    rec(0x0018, &d)
}

/// An EXTERNSHEET record: one XTI per `itabFirst`.
fn externsheet(firsts: &[i16]) -> Vec<u8> {
    let mut d = (firsts.len() as u16).to_le_bytes().to_vec();
    for f in firsts {
        d.extend(0u16.to_le_bytes());
        d.extend(f.to_le_bytes());
        d.extend(f.to_le_bytes());
    }
    rec(0x0017, &d)
}

/// The container `calamine` is given for this book, and its formulas by sheet: `(row, col, text)`.
fn scanned_formulas(book: &Book, limits: &Limits) -> Vec<(u32, u32, String)> {
    let dir = tmp("xls_formula_scan");
    let p = book.write(&dir, "f.xls");
    let (_, container) = fmt_xls::read_scanned(&p, limits, None).unwrap();
    let mut wb = calamine::Xls::new(container).unwrap();
    let name = wb.sheet_names()[0].clone();
    wb.worksheet_formula(&name)
        .unwrap()
        .cells()
        .map(|(r, c, s)| (r as u32, c as u32, s.clone()))
        .collect()
}

const DROPPED: &str = "Unrecognised formula";

/// What `calamine` prints for a formula the guard took the tokens off: nothing (a lone
/// `PtgMissArg`), not an error message.
fn was_dropped(text: &str) -> bool {
    text.is_empty()
}

/// `N + N + N ...` (left to right: PtgName, then PtgName and PtgAdd for each further one).
fn name_chain(names: usize) -> Vec<u8> {
    let mut rgce = vec![0x23, 1, 0, 0, 0];
    for _ in 1..names {
        rgce.extend([0x23, 1, 0, 0, 0, 0x03]);
    }
    rgce
}

#[test]
fn a_chain_of_long_names_prints_a_megabyte_per_record_and_is_dropped_over_the_budget() {
    // 1,366 x PtgName (5 bytes) of a 255-byte name: an 8 KB record prints 348 KB, and a file of
    // such records 40 times its own size. Budget for two of them.
    let long = "N".repeat(255);
    let rgce = name_chain(1366);
    assert!(rgce.len() < 8200, "{}", rgce.len());
    assert_eq!(rgce.len(), 1366 * 5 + 1365);
    let values: Vec<Vec<u8>> = (0..6u16)
        .map(|r| formula_rec(r, 0, 0, f64::from(r) + 1.0, &rgce))
        .collect();
    let book = Book {
        globals: {
            let mut g = globals(&[], &[]);
            g.push(name_rec(&long, &[0x1E, 1, 0]));
            g
        },
        sheets: vec![sheet("S", values)],
    };
    let printed = 1366 * 255 + 1365;
    // The bound counts the worst case of any code page for every character of the name and two
    // bytes for every operator (`<=`, `<>`): room for two formulas.
    let bound = 1366 * 255 * 6 + 1365 * 2;
    let tight = Limits {
        max_formula_text_bytes: 2 * bound as u64 + 10,
        ..Limits::default()
    };
    let f = scanned_formulas(&book, &tight);
    assert_eq!(f.len(), 6, "every cell still has a formula entry");
    for (r, _, text) in &f {
        if *r < 2 {
            assert_eq!(
                text.len(),
                printed,
                "row {r} keeps its text: {}",
                &text[..text.len().min(100)]
            );
        } else {
            assert!(was_dropped(text), "row {r}: {} bytes", text.len());
        }
    }
    // Over the budget the cached values are all still there.
    let dir = tmp("xls_formula_names_values");
    let p = book.write(&dir, "v.xls");
    let wb = load_with(
        &p,
        Limits {
            max_formula_text_bytes: 2 * bound as u64 + 10,
            ..small_limits()
        },
    )
    .unwrap();
    for r in 0..6 {
        assert_eq!(cell_text(&wb, r, 0), (r + 1).to_string(), "row {r}");
    }
    // With room for all of them nothing is dropped (so the budget is what dropped the rest).
    let roomy = scanned_formulas(&book, &Limits::default());
    assert!(roomy.iter().all(|(_, _, t)| t.len() == printed));
}

#[test]
fn a_right_nested_chain_is_quadratic_and_dropped_by_the_per_formula_cap() {
    // PtgName x n, then PtgAdd x (n - 1): every add copies everything to its right.
    let mut rgce = Vec::new();
    for _ in 0..1366 {
        rgce.extend([0x23, 1, 0, 0, 0]);
    }
    rgce.extend(std::iter::repeat_n(0x03, 1365));
    let env = fmt_xls_formula::Env {
        names: vec![255],
        ..Default::default()
    };
    let body = [&(rgce.len() as u16).to_le_bytes()[..], &rgce].concat();
    let cost = fmt_xls_formula::formula_cost(&body, &env);
    assert!(cost.work > 400_000_000, "{cost:?}");
    let book = Book {
        globals: {
            let mut g = globals(&[], &[]);
            g.push(name_rec(&"N".repeat(255), &[0x1E, 1, 0]));
            g
        },
        sheets: vec![sheet("S", vec![formula_rec(0, 0, 0, 1.0, &rgce)])],
    };
    let f = scanned_formulas(&book, &Limits::default());
    assert!(was_dropped(&f[0].2), "{}", f[0].2.len());
}

#[test]
fn spaces_inserted_at_the_start_of_an_operand_are_counted_and_the_formula_dropped() {
    // One PtgInt, then 2,000 x PtgAttrSpace(255 spaces): each insert moves the whole text, about
    // 1.3 x 10^11 byte moves for an 8 KB record. `calamine` would be at it for minutes.
    let mut rgce = vec![0x1E, 1, 0];
    for _ in 0..2000 {
        rgce.extend([0x19, 0x40, 0x00, 255]);
    }
    let env = fmt_xls_formula::Env::default();
    let cost = fmt_xls_formula::formula_cost(
        &[&(rgce.len() as u16).to_le_bytes()[..], &rgce].concat(),
        &env,
    );
    assert!(cost.work > 10_000_000_000, "{cost:?}");
    assert_eq!(cost.text, 5 + 2000 * 255);
    // A mild one passes and keeps its text (the spaces in front of the value).
    let mild = vec![0x1E, 1, 0, 0x19, 0x40, 0x00, 3];
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet(
            "S",
            vec![
                formula_rec(0, 0, 0, 1.0, &rgce),
                formula_rec(1, 0, 0, 2.0, &mild),
            ],
        )],
    };
    let t = std::time::Instant::now();
    let f = scanned_formulas(&book, &Limits::default());
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    assert_eq!(f.len(), 2);
    assert!(
        was_dropped(&f[0].2),
        "{:?}",
        &f[0].2[..f[0].2.len().min(40)]
    );
    assert_eq!(f[1].2, "   1");
    // The cached values of both are intact.
    let dir = tmp("xls_formula_space_values");
    let p = book.write(&dir, "s.xls");
    let wb = load(&p).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "1");
    assert_eq!(cell_text(&wb, 1, 0), "2");
}

#[test]
fn the_work_budget_is_shared_by_all_formulas_of_the_workbook() {
    // Each formula is under the per-formula cap but ten of them are over the shared budget.
    let mut rgce = vec![0x1E, 1, 0];
    for _ in 0..40 {
        rgce.extend([0x19, 0x40, 0x00, 255]);
    }
    let one = fmt_xls_formula::formula_cost(
        &[&(rgce.len() as u16).to_le_bytes()[..], &rgce].concat(),
        &fmt_xls_formula::Env::default(),
    );
    assert!(one.work > 1_000_000 && one.work < 1 << 27, "{one:?}");
    let recs: Vec<Vec<u8>> = (0..10u16)
        .map(|r| formula_rec(r, 0, 0, 1.0, &rgce))
        .collect();
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", recs)],
    };
    let limits = Limits {
        max_formula_work: one.work * 4 + 1,
        ..Limits::default()
    };
    let f = scanned_formulas(&book, &limits);
    let kept = f.iter().filter(|(_, _, t)| !was_dropped(t)).count();
    assert_eq!(
        kept,
        4,
        "{:?}",
        f.iter().map(|x| x.2.len()).collect::<Vec<_>>()
    );
}

/// Token sequences with names, sheets and every kind of operand, for the property checks.
fn formula_token_samples() -> Vec<Vec<u8>> {
    let int = |v: u16| [&[0x1Eu8][..], &v.to_le_bytes()].concat();
    let name = |i: u32| [&[0x23u8][..], &i.to_le_bytes()].concat();
    let ref3d = |ixti: u16| {
        let mut v = vec![0x3A];
        v.extend(ixti.to_le_bytes());
        v.extend([4, 0, 0x02, 0xC0]);
        v
    };
    let area3d = |ixti: u16| {
        let mut v = vec![0x3B];
        v.extend(ixti.to_le_bytes());
        v.extend([1, 0, 9, 0, 2, 0, 5, 0]);
        v
    };
    let func1 = |iftab: u16| [&[0x21u8][..], &iftab.to_le_bytes()].concat(); // ABS-like, fixed arity
    let num = |v: f64| [&[0x1Fu8][..], &v.to_le_bytes()].concat();
    let mut out = vec![
        int(1),
        [int(1), int(2), vec![0x03]].concat(),
        [int(1), int(2), vec![0x0E]].concat(),
        [int(7), vec![0x12]].concat(),
        [int(7), vec![0x13, 0x14]].concat(),
        [int(7), vec![0x15]].concat(),
        [name(1), name(1), vec![0x03]].concat(),
        [name(2), name(3), vec![0x03]].concat(), // missing names
        ref3d(0),
        ref3d(1),
        ref3d(99),
        area3d(0),
        area3d(1),
        area3d(99),
        [vec![0x3C, 0, 0, 0, 0, 0, 0]].concat(),
        [vec![0x3D, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]].concat(),
        [int(4), func1(6)].concat(),               // an arity-1 function
        [int(4), int(5), func1(1)].concat(),       // wrong arity for IF: error path
        [int(9), vec![0x19, 0x10, 0, 0]].concat(), // PtgAttrSum
        [int(9), vec![0x19, 0x40, 0x00, 4]].concat(),
        [int(9), vec![0x19, 0x41, 0x01, 2]].concat(),
        [int(9), vec![0x19, 0x40, 0x09, 2]].concat(), // bad space kind: error path
        [num(1.5)].concat(),
        [num(1e300)].concat(),
        [num(-2.5e-300)].concat(),
        [num(f64::NAN)].concat(),
        vec![0x17, 3, 0, b'a', b'b', b'c'],
        vec![0x17, 2, 1, b'a', 0, 0x4E, 0x00],
        vec![0x1D, 1],
        vec![0x1C, 0x07],
        vec![0x1C, 0x2B],
        vec![0x1C, 0x55], // unknown error code: error path
        vec![0x24, 3, 0, 5, 0xC0],
        vec![0x25, 1, 0, 9, 0, 2, 0, 0x05, 0xC0],
        vec![0x2A, 0, 0, 0, 0],
        vec![0x2B, 0, 0, 0, 0, 0, 0, 0, 0],
        vec![0x01, 0, 0, 0, 0],
        vec![0x20, 0, 0, 0, 0, 0, 0, 0],
        vec![0x39, 0, 0, 0, 0, 0, 0],
        vec![0x03],                // stack underflow: error path
        vec![0x77],                // unknown token: error path
        [int(1), int(2)].concat(), // two values left: error path
    ];
    // A variable-arity function call: SUM(1, 2, 3).
    let mut sum = [int(1), int(2), int(3)].concat();
    sum.extend([0x22, 3, 4, 0]);
    out.push(sum);
    out
}

/// The `Env` of the book used by the property checks (see `property_book`).
fn property_env() -> fmt_xls_formula::Env {
    fmt_xls_formula::Env {
        pre_biff8: false,
        sheets: vec![1, 3],
        names: vec![8, 3],
        xti_first: vec![0, 1],
    }
}

fn property_book(formulas: &[Vec<u8>]) -> Book {
    Book {
        globals: {
            let mut g = globals(&[], &[]);
            g.push(name_rec("LongName", &[0x1E, 1, 0]));
            g.push(name_rec("two", &[0x1E, 1, 0]));
            g.push(externsheet(&[0, 1]));
            g
        },
        sheets: vec![sheet("S", vec![]), {
            let mut s = sheet("Two", vec![]);
            s.records = formulas
                .iter()
                .enumerate()
                .map(|(i, f)| formula_rec(i as u16, 0, 0, 1.0, f))
                .collect();
            s
        }],
    }
}

#[test]
fn the_cost_is_an_upper_bound_of_what_calamine_prints_and_mirrors_where_it_gives_up() {
    let samples = formula_token_samples();
    let book = property_book(&samples);
    let stream = book.stream();
    let container = fmt_xls::clean_container(&stream).unwrap();
    let mut wb = calamine::Xls::new(container).unwrap();
    let printed: Vec<(usize, String)> = wb
        .worksheet_formula("Two")
        .unwrap()
        .cells()
        .map(|(r, _, s)| (r, s.clone()))
        .collect();
    assert_eq!(printed.len(), samples.len());
    let env = property_env();
    let mut errors = 0;
    for (r, text) in printed {
        let rgce = &samples[r];
        let body = [&(rgce.len() as u16).to_le_bytes()[..], rgce].concat();
        let cost = fmt_xls_formula::formula_cost(&body, &env);
        if text.starts_with(DROPPED) {
            errors += 1;
            assert_eq!(
                cost.text,
                fmt_xls_formula::ERROR_TEXT,
                "sample {r} is an error for calamine: {rgce:02x?}"
            );
        } else {
            assert!(
                cost.text >= text.len() as u64,
                "sample {r}: bound {} < printed {} ({text})",
                cost.text,
                text.len()
            );
            assert!(
                cost.text != fmt_xls_formula::ERROR_TEXT,
                "sample {r} was parsed: {text}"
            );
            assert!(cost.peak >= cost.text);
        }
    }
    assert!(errors >= 6, "the error paths are in the samples: {errors}");
}

#[test]
fn random_tokens_never_panic_the_cost_and_it_bounds_what_calamine_prints() {
    let samples = formula_token_samples();
    let env = property_env();
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut checked = 0;
    for round in 0..300 {
        // Glue a few samples together and flip some bytes.
        let mut rgce = Vec::new();
        for _ in 0..(xorshift(&mut seed) % 4 + 1) {
            rgce.extend(&samples[(xorshift(&mut seed) as usize) % samples.len()]);
        }
        for _ in 0..(xorshift(&mut seed) % 3) {
            let at = (xorshift(&mut seed) as usize) % rgce.len();
            rgce[at] = xorshift(&mut seed) as u8;
        }
        let body = [&(rgce.len() as u16).to_le_bytes()[..], &rgce].concat();
        let cost = fmt_xls_formula::formula_cost(&body, &env);
        let book = property_book(std::slice::from_ref(&rgce));
        let stream = book.stream();
        let Ok(container) = fmt_xls::clean_container(&stream) else {
            continue;
        };
        let got = std::panic::catch_unwind(|| {
            let mut wb = calamine::Xls::new(container).ok()?;
            wb.worksheet_formula("Two")
                .ok()
                .map(|r| r.cells().map(|(_, _, s)| s.clone()).collect::<Vec<_>>())
        });
        // `calamine` indexes short operands and panics: the cost gave up there too (it never panics).
        if let Ok(Some(texts)) = got {
            for t in texts {
                if !t.starts_with(DROPPED) {
                    assert!(cost.text >= t.len() as u64, "round {round}: {rgce:02x?}");
                    checked += 1;
                } else {
                    assert_eq!(
                        cost.text,
                        fmt_xls_formula::ERROR_TEXT,
                        "round {round}: {rgce:02x?}"
                    );
                }
            }
        }
    }
    assert!(checked > 20, "{checked}");
}

#[test]
fn every_real_xls_fixture_reads_the_same_with_and_without_the_formula_guard() {
    // The formulas, values and sheet names `calamine` gives for a fixture are the same whether it
    // reads the file itself or the container made from the scanned stream (nothing is dropped).
    let dir = konoma_test_support::repo_root().join("testdata/office");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) != Some("xls") {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let Ok(mut own) = calamine::open_workbook::<calamine::Xls<_>, _>(&p) else {
            continue; // encrypted.xls
        };
        let Ok((_, container)) = fmt_xls::read_scanned(&p, &Limits::default(), None) else {
            panic!("{name}: the pass refuses what calamine opens");
        };
        let mut scanned = calamine::Xls::new(container).unwrap();
        assert_eq!(own.sheet_names(), scanned.sheet_names(), "{name}");
        for s in own.sheet_names() {
            assert_eq!(
                own.worksheet_range(&s).unwrap(),
                scanned.worksheet_range(&s).unwrap(),
                "{name}/{s} values"
            );
            let (a, b) = (
                own.worksheet_formula(&s).unwrap(),
                scanned.worksheet_formula(&s).unwrap(),
            );
            assert_eq!(a, b, "{name}/{s} formulas");
            n += a.cells().count();
        }
        assert_eq!(own.defined_names(), scanned.defined_names(), "{name} names");
    }
    assert!(n > 5, "the fixtures have formulas: {n}");
}

#[test]
fn defined_names_with_a_sheet_prefix_share_a_quarter_of_the_text_budget() {
    // 100 names whose formula starts with a reference to a sheet with a 200-byte name: `calamine`
    // prints the sheet's name in front of each. The names stay (PtgName indexes them), only the
    // prefix of those over the budget is dropped.
    let sheet_name = "S".repeat(200);
    let mut ref3d = vec![0x3A, 0, 0];
    ref3d.extend([0, 0, 0, 0]);
    let mut g = globals(&[], &[]);
    g.push(externsheet(&[0]));
    for i in 0..100 {
        g.push(name_rec(&format!("n{i}"), &ref3d));
    }
    let book = Book {
        globals: g,
        sheets: vec![sheet(&sheet_name, vec![number(0, 0, 0, 1.0)])],
    };
    let dir = tmp("xls_name_prefix");
    let p = book.write(&dir, "n.xls");
    // 1,200 (6 per character) + 40 per prefix; a quarter of 4 x 1,240 x 10 allows ten.
    let limits = Limits {
        max_formula_text_bytes: 4 * 1240 * 10,
        ..Limits::default()
    };
    let (_, container) = fmt_xls::read_scanned(&p, &limits, None).unwrap();
    let wb = calamine::Xls::new(container).unwrap();
    let names = wb.defined_names();
    assert_eq!(names.len(), 100);
    let with_prefix = names.iter().filter(|(_, f)| f.contains('!')).count();
    assert_eq!(with_prefix, 10);
    assert!(names.iter().all(|(n, _)| n.starts_with('n')));
}

#[test]
fn the_biff_version_is_mapped_like_calamine_does() {
    for (version, dt, biff8) in [
        (0x0600u16, 0x0005u16, true),
        (0x0500, 0x0005, false),
        (0x0400, 0x0005, false),
        (0x0300, 0x0005, false),
        (0x0200, 0x0005, false),
        (0x0002, 0x0005, false),
        (0x0007, 0x0005, false),
        (0x0000, 0x0005, true),
        (0x0000, 0x1000, false),
        (0x0700, 0x0005, true),
        (0x1234, 0x0005, true),
    ] {
        let mut d = version.to_le_bytes().to_vec();
        d.extend(dt.to_le_bytes());
        assert_eq!(
            fmt_xls::bof_is_biff8_for_test(&d),
            biff8,
            "{version:#x}/{dt:#x}"
        );
    }
}

#[test]
fn version_zero_globals_are_read_as_biff8_and_sheets_use_the_globals_version() {
    // Globals whose BOF says version 0 are BIFF8 for `calamine` (unless the type is 0x1000): the
    // FORMAT record must be read in the BIFF8 layout.
    let book = Book {
        globals: globals(&[(164, "0.0%")], &[164]),
        sheets: vec![sheet("S", vec![number(0, 0, 3, 0.256)])],
    };
    let mut stream = book.stream();
    stream[4..6].copy_from_slice(&0u16.to_le_bytes());
    let dir = tmp("xls_bof_zero");
    let p = dir.join("z.xls");
    write_cfb(&p, &[("/Workbook", &stream)]);
    let wb = load(&p).unwrap();
    assert_eq!(cell_text(&wb, 0, 0), "25.6%");
    // A sheet whose own BOF says BIFF5 is still read as the globals say (BIFF8): the cost of a
    // wide LABEL string is the same as without the patch.
    let label = |s: &str| {
        let mut d = pos(0, 0, 0);
        d.extend(xl_string(s, true));
        rec(0x0204, &d)
    };
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![label("まっさらな文字列です")])],
    };
    let plain = book.stream();
    let mut patched = plain.clone();
    // The sheet's BOF is the record after the globals' EOF: find its version word.
    let at = plain
        .windows(4)
        .rposition(|w| w == [0x09, 0x08, 0x10, 0x00])
        .unwrap();
    patched[at + 4..at + 6].copy_from_slice(&0x0500u16.to_le_bytes());
    let a = fmt_xls::parse_stream(&plain, &Limits::default()).unwrap();
    let b = fmt_xls::parse_stream(&patched, &Limits::default()).unwrap();
    assert_eq!(a.sheets["S"].text_bytes, b.sheets["S"].text_bytes);
    assert!(a.sheets["S"].text_bytes >= 20);
}

#[cfg(test)]
mod cost;
