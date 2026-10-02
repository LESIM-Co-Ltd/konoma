//! Tests for the xls format pass (`fmt_xls`). Real LibreOffice output is in `tests.rs`
//! (`real_xls_*`); the BIFF8 streams here are assembled record by record so each rule (FORMAT /
//! XF / DATEMODE / every cell record / CONTINUE / the memory checks) is pinned on its own.

use super::container::Limits;
use super::fmt_xls;
use super::tests::{
    assert_inner_does_not_panic, load, load_with, mutate, small_limits, tmp, write_cfb, xorshift,
};
use super::*;

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

    fn write(&self, dir: &crate::test_support::TmpDir, name: &str) -> std::path::PathBuf {
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
    let Some(p) = crate::test_support::sample_path_or_skip("sample.xls") else {
        return;
    };
    let fm = fmt_xls::read(&p, &Limits::default()).unwrap();
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
