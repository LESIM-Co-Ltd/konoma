//! Differential tests of the formula cost model (`fmt_xls_formula`): every token class that
//! `calamine` 0.37 `parse_formula` handles is built into a formula, run through `calamine`
//! itself (over the container made from the same stream), and the cost konoma charged for it
//! (taken from the scan, so the sheet and name sizes come from the records) is held against the
//! real text. A cost under the real length is a hole in the budget; the exact pins catch a cost
//! that is too high (a constant raised) as well.

use super::*;
use crate::preview::office::fmt_xls_formula::{self as formula, Cost, ERROR_TEXT};

/// A string as the BIFF8 writer stores it: `(wide, cch, bytes)`.
type Str = (bool, usize, Vec<u8>);

fn s8(s: &str) -> Str {
    if s.chars().any(|c| c as u32 > 255) {
        let units: Vec<u16> = s.encode_utf16().collect();
        (
            true,
            units.len(),
            units.iter().flat_map(|u| u.to_le_bytes()).collect(),
        )
    } else {
        (
            false,
            s.chars().count(),
            s.chars().map(|c| c as u8).collect(),
        )
    }
}

fn bound_raw(pos: u32, s: &Str, biff5: bool) -> Vec<u8> {
    let mut d = pos.to_le_bytes().to_vec();
    d.extend([0, 0, s.1 as u8]);
    if !biff5 {
        d.push(u8::from(s.0));
    }
    d.extend_from_slice(&s.2);
    rec(0x0085, &d)
}

fn name_raw(s: &Str, rgce: &[u8], biff5: bool) -> Vec<u8> {
    let mut d = vec![0, 0, 0, s.1 as u8];
    d.extend((rgce.len() as u16).to_le_bytes());
    d.extend([0; 8]);
    if !biff5 {
        d.push(u8::from(s.0));
    }
    d.extend_from_slice(&s.2);
    d.extend_from_slice(rgce);
    rec(0x0018, &d)
}

/// A whole workbook stream (BIFF8) built from raw strings, with the formulas in one sheet.
struct Raw {
    /// BIFF5 layouts (strings without a flags byte, 3-byte references) for the whole workbook.
    biff5: bool,
    /// Every formula goes to this cell instead of one row each (use with a single formula).
    cell: Option<(u16, u16)>,
    codepage: Option<u16>,
    sheets: Vec<Str>,
    names: Vec<(Str, Vec<u8>)>,
    xti: Vec<i16>,
    formulas: Vec<Vec<u8>>,
    formula_sheet: usize,
}

impl Raw {
    /// The usual cast: an ASCII, a wide and a Latin-1 sheet name, the same for defined names, and
    /// an XTI table that also points at a sheet that is not there.
    fn standard(formulas: Vec<Vec<u8>>) -> Raw {
        Raw {
            biff5: false,
            cell: None,
            codepage: None,
            sheets: vec![s8("S"), s8("Two"), s8("シート"), s8("Ünï")],
            names: ["LongName", "two", "名前", "Ünï"]
                .into_iter()
                .map(|n| (s8(n), vec![0x1E, 1, 0]))
                .collect(),
            xti: vec![0, 1, 2, 3, 9],
            formulas,
            formula_sheet: 1,
        }
    }

    /// The BIFF5 cast: byte strings only; the sheets are told apart by the length of their names.
    fn standard5(formulas: Vec<Vec<u8>>) -> Raw {
        Raw {
            biff5: true,
            cell: None,
            codepage: None,
            sheets: vec![s8("S"), s8("Two"), s8(&"A".repeat(20)), s8("Ünï")],
            names: ["LongName", "two", "Ünï"]
                .into_iter()
                .map(|n| (s8(n), vec![0x1E, 1, 0]))
                .collect(),
            // Not an XTI table for BIFF5 (its EXTERNSHEET is laid out differently and ignored):
            // were it read as one, every 3d reference would point at sheet 0.
            xti: vec![0, 0, 0, 0],
            formulas,
            formula_sheet: 1,
        }
    }

    fn stream(&self) -> Vec<u8> {
        let version = if self.biff5 { 0x0500 } else { 0x0600 };
        let mut head = bof(version, 0x0005);
        if let Some(cp) = self.codepage {
            head.extend(rec(0x0042, &cp.to_le_bytes()));
        }
        for (n, rgce) in &self.names {
            head.extend(name_raw(n, rgce, self.biff5));
        }
        let sheets_len: usize = self
            .sheets
            .iter()
            .map(|s| bound_raw(0, s, self.biff5).len())
            .sum();
        let ext = externsheet(&self.xti);
        let globals_len = head.len() + sheets_len + ext.len() + eof().len();
        let mut bodies = Vec::new();
        let mut positions = Vec::new();
        for i in 0..self.sheets.len() {
            positions.push((globals_len + bodies.len()) as u32);
            bodies.extend(bof(version, 0x0010));
            if i == self.formula_sheet {
                for (r, f) in self.formulas.iter().enumerate() {
                    let (row, col) = self.cell.unwrap_or((r as u16, 0));
                    bodies.extend(formula_rec(row, col, 0, 1.0, f));
                }
            }
            bodies.extend(eof());
        }
        for (s, p) in self.sheets.iter().zip(positions) {
            head.extend(bound_raw(p, s, self.biff5));
        }
        head.extend(ext);
        head.extend(eof());
        head.extend(bodies);
        head
    }
}

/// What konoma charged and what `calamine` printed, per formula (in order).
struct Run {
    cost: Vec<Cost>,
    real: Vec<String>,
}

fn run(raw: &Raw) -> Run {
    let stream = raw.stream();
    let cost: Vec<Cost> = fmt_xls::formula_costs(&stream, &Limits::default())
        .unwrap()
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let container = fmt_xls::clean_container(&stream).unwrap();
    let mut wb = calamine::Xls::new(container).unwrap();
    let name = wb.sheet_names()[raw.formula_sheet].clone();
    let real: Vec<String> = wb
        .worksheet_formula(&name)
        .unwrap()
        .cells()
        .map(|(_, _, s)| s.clone())
        .collect();
    assert_eq!(cost.len(), raw.formulas.len());
    assert_eq!(real.len(), raw.formulas.len());
    Run { cost, real }
}

/// The cost of the only formula of `raw` and what `calamine` printed, or `None` where `calamine`
/// panics (it slices operands it does not check).
fn run_guarded(raw: &Raw) -> (Cost, Option<String>) {
    assert_eq!(raw.formulas.len(), 1);
    let stream = raw.stream();
    let cost = fmt_xls::formula_costs(&stream, &Limits::default()).unwrap()[0].1;
    let real = std::panic::catch_unwind(|| {
        let container = fmt_xls::clean_container(&stream).unwrap();
        let mut wb = calamine::Xls::new(container).unwrap();
        let name = wb.sheet_names()[raw.formula_sheet].clone();
        wb.worksheet_formula(&name)
            .unwrap()
            .cells()
            .next()
            .map(|(_, _, s)| s.clone())
    })
    .ok()
    .flatten();
    (cost, real)
}

// ---------------------------------------------------------------------------------------------
// token builders
// ---------------------------------------------------------------------------------------------

fn int(v: u16) -> Vec<u8> {
    [&[0x1Eu8][..], &v.to_le_bytes()].concat()
}
fn num(v: f64) -> Vec<u8> {
    [&[0x1Fu8][..], &v.to_le_bytes()].concat()
}
fn name_tok(i: u32) -> Vec<u8> {
    [&[0x23u8][..], &i.to_le_bytes()].concat()
}
fn ptg_str(cch: u8, flags: u8, bytes: &[u8]) -> Vec<u8> {
    [&[0x17u8, cch, flags][..], bytes].concat()
}
fn ref_tok(ptg: u8, row: u16, col_and_flags: u16) -> Vec<u8> {
    let mut v = vec![ptg];
    v.extend(row.to_le_bytes());
    v.extend(col_and_flags.to_le_bytes());
    v
}
fn area_tok(ptg: u8, r1: u16, r2: u16, c1: u16, c2: u16) -> Vec<u8> {
    let mut v = vec![ptg];
    for x in [r1, r2, c1, c2] {
        v.extend(x.to_le_bytes());
    }
    v
}
fn ref3d(ptg: u8, ixti: u16, row: u16, col_and_flags: u16) -> Vec<u8> {
    let mut v = vec![ptg];
    v.extend(ixti.to_le_bytes());
    v.extend(row.to_le_bytes());
    v.extend(col_and_flags.to_le_bytes());
    v
}
fn area3d(ptg: u8, ixti: u16, r1: u16, r2: u16, c1: u16, c2: u16) -> Vec<u8> {
    let mut v = vec![ptg];
    v.extend(ixti.to_le_bytes());
    for x in [r1, r2, c1, c2] {
        v.extend(x.to_le_bytes());
    }
    v
}
fn err3d(ptg: u8, ixti: u16, operand: usize) -> Vec<u8> {
    let mut v = vec![ptg];
    v.extend(ixti.to_le_bytes());
    v.extend(vec![0; operand - 2]);
    v
}
fn func(iftab: u16) -> Vec<u8> {
    [&[0x21u8][..], &iftab.to_le_bytes()].concat()
}
fn func_var(argc: u8, iftab: u16) -> Vec<u8> {
    let mut v = vec![0x22, argc];
    v.extend(iftab.to_le_bytes());
    v
}
fn attr(sub: u8, data: &[u8]) -> Vec<u8> {
    [&[0x19u8, sub][..], data].concat()
}

/// Every token class of `calamine`'s `parse_formula`, in the shapes that make the text longest
/// and in shorter ones. `(label, tokens)`.
fn samples() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |label: &str, rgce: Vec<u8>| out.push((label.to_string(), rgce));
    // Literals.
    add("int 65535", int(65535));
    add("int 7", int(7));
    add("int 0", int(0));
    add("bool true", vec![0x1D, 1]);
    add("bool false", vec![0x1D, 0]);
    for e in [0x00u8, 0x07, 0x0F, 0x17, 0x1D, 0x24, 0x2A, 0x2B] {
        add(&format!("err {e:#04x}"), vec![0x1C, e]);
    }
    add("err unknown", vec![0x1C, 0x55]);
    for (label, v) in [
        ("num 1.5", 1.5),
        ("num 0", 0.0),
        ("num -0", -0.0),
        ("num 12345678.9", 12345678.9),
        ("num 1/3", 1.0 / 3.0),
        ("num 1e-5", 1e-5),
        ("num 9.99e-6", 9.99e-6),
        ("num 1e15", 1e15),
        ("num 9999999999999998", 9_999_999_999_999_998.0),
        ("num 1e16", 1e16),
        ("num 1e300", 1e300),
        ("num -2.5e-300", -2.5e-300),
        ("num nan", f64::NAN),
        ("num inf", f64::INFINITY),
        ("num -inf", f64::NEG_INFINITY),
        ("num f64::MAX", f64::MAX),
        ("num min subnormal", 5e-324),
    ] {
        add(label, num(v));
    }
    // Strings: compressed ASCII, compressed with a high byte, 16-bit ASCII, 16-bit CJK.
    add("str ascii", ptg_str(3, 0, b"abc"));
    add("str empty", ptg_str(0, 0, b""));
    add("str high byte", ptg_str(2, 0, &[0xE9, b'x']));
    add("str wide ascii", ptg_str(2, 1, &[b'a', 0, b'b', 0]));
    add("str wide cjk", ptg_str(2, 1, &[0x42, 0x30, 0x44, 0x30]));
    add("str wide mixed", ptg_str(2, 1, &[b'a', 0, 0x44, 0x30]));
    add(
        "str wide surrogates",
        ptg_str(2, 1, &[0x3D, 0xD8, 0x00, 0xDE]),
    );
    add("str wide ascii with high byte", ptg_str(1, 1, &[b'a', 1]));
    add("str short body", ptg_str(5, 0, b"ab"));
    add("str no flags", vec![0x17, 1]);
    add("str no cch", vec![0x17]);
    // References (BIFF8 layout).
    add("ref abs max", ref_tok(0x24, 65534, 0x3FFF));
    add("ref rel max", ref_tok(0x24, 65534, 0xFFFF));
    add("ref a1", ref_tok(0x24, 0, 0));
    add("ref value class", ref_tok(0x44, 9, 0x00C5));
    add("ref array class", ref_tok(0x64, 99, 0x4002));
    add("area abs max", area_tok(0x25, 65535, 65535, 0x3FFF, 0x3FFF));
    add("area rel max", area_tok(0x25, 65535, 65535, 0xFFFF, 0xFFFF));
    add("area small", area_tok(0x45, 0, 1, 0x00C0, 0x00C1));
    add("area array class", area_tok(0x65, 5, 6, 1, 2));
    // Relative references (RefN / AreaN) are not handled: an error for both.
    add("refn", ref_tok(0x2C, 1, 1));
    add("refn value", ref_tok(0x4C, 1, 1));
    add("arean", area_tok(0x2D, 1, 2, 1, 2));
    add("ref err", vec![0x2A, 0, 0, 0, 0]);
    add("ref err value", vec![0x4A, 0, 0, 0, 0]);
    add("area err", vec![0x2B, 0, 0, 0, 0, 0, 0, 0, 0]);
    add("area err array", vec![0x6B, 0, 0, 0, 0, 0, 0, 0, 0]);
    // 3d references: through the XTI table (Ref3d) or the sheet index (the others), with ASCII,
    // wide and Latin-1 sheet names, and with a sheet that is not there.
    for ixti in [0u16, 1, 2, 3, 4, 99] {
        add(
            &format!("ref3d abs {ixti}"),
            ref3d(0x3A, ixti, 65534, 0x3FFF >> 2),
        );
        add(&format!("ref3d rel {ixti}"), ref3d(0x5A, ixti, 0, 0xFFFF));
        add(
            &format!("area3d {ixti}"),
            area3d(0x3B, ixti, 65535, 65535, 0x3FFF, 0x3FFF),
        );
        add(
            &format!("area3d small {ixti}"),
            area3d(0x7B, ixti, 0, 1, 0, 1),
        );
        add(&format!("ref err 3d {ixti}"), err3d(0x3C, ixti, 6));
        add(&format!("area err 3d {ixti}"), err3d(0x5D, ixti, 10));
    }
    // Defined names: ASCII, wide, Latin-1, missing (0 wraps, past the end).
    // (Name 0 wraps in `calamine`'s release build and panics in a debug one: cost only, below.)
    for i in [1u32, 2, 3, 4, 5, 99] {
        add(&format!("name {i}"), name_tok(i));
    }
    add(
        "name value class",
        [vec![0x43], 1u32.to_le_bytes().to_vec()].concat(),
    );
    add(
        "name array class",
        [vec![0x63], 3u32.to_le_bytes().to_vec()].concat(),
    );
    add("namex", vec![0x39, 0, 0, 0, 0, 0, 0]);
    add("namex value", vec![0x59, 0, 0, 0, 0, 0, 0]);
    add("exp", vec![0x01, 1, 0, 1, 0]);
    add("array", vec![0x20, 0, 0, 0, 0, 0, 0, 0]);
    add("array value", vec![0x60, 0, 0, 0, 0, 0, 0, 0]);
    add(
        "extended (skipped 5)",
        [vec![0x18, 0, 0, 0, 0, 0], int(1)].concat(),
    );
    add("miss arg", vec![0x16]);
    add(
        "miss arg then op",
        [vec![0x16], int(2), vec![0x10]].concat(),
    );
    // Operators: every binary one, two 5-digit operands.
    for ptg in 0x03u8..=0x11 {
        add(
            &format!("binary {ptg:#04x}"),
            [int(10000), int(65535), vec![ptg]].concat(),
        );
    }
    add("binary one digit", [int(1), int(2), vec![0x03]].concat());
    add(
        "binary on strings",
        [ptg_str(3, 0, b"abc"), ptg_str(3, 0, b"def"), vec![0x08]].concat(),
    );
    add("binary underflow", [int(1), vec![0x03]].concat());
    add("binary no operands", vec![0x03]);
    add("unary plus", [int(65535), vec![0x12]].concat());
    add("unary minus", [int(65535), vec![0x13]].concat());
    add("percent", [int(65535), vec![0x14]].concat());
    add("percent only", vec![0x14]);
    add("paren", [int(65535), vec![0x15]].concat());
    add("paren twice", [int(65535), vec![0x15, 0x15]].concat());
    add("paren no operand", vec![0x15]);
    add("unary no operand", vec![0x12]);
    // PtgAttr.
    for sub in [0x01u8, 0x02, 0x08, 0x20, 0x21] {
        add(
            &format!("attr skip {sub:#04x}"),
            [int(3), attr(sub, &[0, 0])].concat(),
        );
    }
    add(
        "attr choose 0",
        [int(3), attr(0x04, &[0, 0, 0, 0])].concat(),
    );
    add(
        "attr choose 2",
        [int(3), attr(0x04, &[2, 0, 1, 0, 2, 0, 3, 0])].concat(),
    );
    add("attr sum", [int(65535), attr(0x10, &[0, 0])].concat());
    add("attr sum no operand", attr(0x10, &[0, 0]));
    add(
        "attr sum after binary",
        [int(1), int(2), vec![0x03], attr(0x10, &[0, 0])].concat(),
    );
    for kind in 0u8..=7 {
        add(
            &format!("attr space {kind} x3"),
            [int(65535), attr(0x40, &[kind, 3])].concat(),
        );
    }
    add(
        "attr space 0 x0",
        [int(65535), attr(0x40, &[0, 0])].concat(),
    );
    add(
        "attr space 1 x255",
        [int(65535), attr(0x40, &[1, 255])].concat(),
    );
    add(
        "attr space 0 x255 (after)",
        [int(65535), attr(0x41, &[0, 255])].concat(),
    );
    add(
        "attr space twice",
        [int(7), attr(0x40, &[0, 200]), attr(0x41, &[2, 100])].concat(),
    );
    add(
        "attr space on the second operand",
        [int(1), int(2), attr(0x40, &[0, 10]), vec![0x03]].concat(),
    );
    add("attr space no operand", attr(0x40, &[0, 3]));
    add("attr unknown", [int(1), attr(0x77, &[0, 0])].concat());
    // Functions: fixed arity (0, 1, 2, 3), variable, a variable one with a long name.
    // FTAB: NA = 10 (0 args), ABS = 24 (1), ATAN2 = 97 (2), IF = 1 (3 here), SUM = 4 (variable).
    add("func 0 args", func(10));
    add("func 1 arg", [int(5), func(24)].concat());
    add("func 2 args", [int(5), int(6), func(97)].concat());
    add("func 3 args", [int(5), int(6), int(7), func(1)].concat());
    add("func 1 arg missing", func(24));
    add("func variable (not var token)", [int(5), func(4)].concat());
    add("func iftab out of range", [int(5), func(485)].concat());
    add("func iftab max", [int(5), func(0xFFFF)].concat());
    add("funcvar 0 args", func_var(0, 4));
    add("funcvar 1 arg", [int(5), func_var(1, 4)].concat());
    add(
        "funcvar 5 args",
        [int(5), int(5), int(5), int(5), int(5), func_var(5, 4)].concat(),
    );
    add(
        "funcvar 2 args of 65535",
        [int(65535), int(65535), func_var(2, 4)].concat(),
    );
    add(
        "funcvar more args than operands",
        [int(1), func_var(3, 4)].concat(),
    );
    add(
        "funcvar iftab out of range",
        [int(1), func_var(1, 485)].concat(),
    );
    add(
        "funcvar value class",
        [int(1), vec![0x42, 1, 4, 0]].concat(),
    );
    add(
        "funcvar array class",
        [int(1), vec![0x62, 1, 4, 0]].concat(),
    );
    add("func value class", [int(5), vec![0x41, 24, 0]].concat());
    add("func array class", [int(5), vec![0x61, 24, 0]].concat());
    // Two values left, nothing at all, unknown tokens, memory tokens.
    add("two values", [int(1), int(2)].concat());
    add("empty", vec![]);
    for ptg in [
        0x00u8, 0x02, 0x26, 0x27, 0x28, 0x29, 0x46, 0x66, 0x38, 0x77, 0xFF,
    ] {
        add(
            &format!("unhandled {ptg:#04x}"),
            [int(1), vec![ptg, 0, 0, 0, 0, 0]].concat(),
        );
    }
    out
}

/// `(label, text, peak, work, real length or -1 for an error)`: what the scan charged and what
/// `calamine` printed for each sample, read off and checked against the rules in the comments of
/// `fmt_xls_formula` (the real length is `calamine`'s own, the rest is the model's).
const PINS: &[(&str, u64, u64, u64, i64)] = &[
    ("int 65535", 5, 5, 5, 5),
    ("int 7", 5, 5, 5, 1),
    ("int 0", 5, 5, 5, 1),
    ("bool true", 5, 5, 5, 4),
    ("bool false", 5, 5, 5, 5),
    ("err 0x00", 6, 6, 6, 6),
    ("err 0x07", 7, 7, 7, 7),
    ("err 0x0f", 7, 7, 7, 7),
    ("err 0x17", 5, 5, 5, 5),
    ("err 0x1d", 6, 6, 6, 6),
    ("err 0x24", 5, 5, 5, 5),
    ("err 0x2a", 4, 4, 4, 4),
    ("err 0x2b", 13, 13, 13, 13),
    ("err unknown", 128, 0, 0, -1),
    ("num 1.5", 26, 26, 26, 3),
    ("num 0", 26, 26, 26, 1),
    ("num -0", 26, 26, 26, 2),
    ("num 12345678.9", 26, 26, 26, 10),
    ("num 1/3", 26, 26, 26, 18),
    ("num 1e-5", 26, 26, 26, 7),
    ("num 9.99e-6", 10, 10, 10, 10),
    ("num 1e15", 26, 26, 26, 16),
    ("num 9999999999999998", 26, 26, 26, 16),
    ("num 1e16", 17, 17, 17, 17),
    ("num 1e300", 301, 301, 301, 301),
    ("num -2.5e-300", 304, 304, 304, 304),
    ("num nan", 3, 3, 3, 3),
    ("num inf", 3, 3, 3, 3),
    ("num -inf", 4, 4, 4, 4),
    ("num f64::MAX", 309, 309, 309, 309),
    ("num min subnormal", 326, 326, 326, 326),
    ("str ascii", 20, 20, 20, 5),
    ("str empty", 2, 2, 2, 2),
    ("str high byte", 14, 14, 14, 5),
    ("str wide ascii", 14, 14, 14, 4),
    ("str wide cjk", 14, 14, 14, 8),
    ("str wide mixed", 14, 14, 14, 6),
    ("str wide surrogates", 14, 14, 14, 6),
    ("str wide ascii with high byte", 8, 8, 8, 4),
    ("str short body", 128, 0, 32, -1),
    ("str no flags", 128, 0, 0, -1),
    ("str no cch", 128, 0, 0, -1),
    ("ref abs max", 12, 12, 12, 10),
    ("ref rel max", 12, 12, 12, 8),
    ("ref a1", 12, 12, 12, 4),
    ("ref value class", 12, 12, 12, 6),
    ("ref array class", 12, 12, 12, 5),
    ("area abs max", 40, 40, 40, 21),
    ("area rel max", 40, 40, 40, 17),
    ("area small", 40, 40, 40, 11),
    ("area array class", 40, 40, 40, 9),
    ("refn", 128, 0, 0, -1),
    ("refn value", 128, 0, 0, -1),
    ("arean", 128, 0, 0, -1),
    ("ref err", 5, 5, 5, 5),
    ("ref err value", 5, 5, 5, 5),
    ("area err", 5, 5, 5, 5),
    ("area err array", 5, 5, 5, 5),
    ("ref3d abs 0", 19, 19, 19, 12),
    ("ref3d rel 0", 19, 19, 19, 9),
    ("area3d 0", 31, 31, 31, 23),
    ("area3d small 0", 31, 31, 31, 11),
    ("ref err 3d 0", 12, 12, 12, 7),
    ("area err 3d 0", 12, 12, 12, 7),
    ("ref3d abs 1", 31, 31, 31, 14),
    ("ref3d rel 1", 31, 31, 31, 11),
    ("area3d 1", 43, 43, 43, 25),
    ("area3d small 1", 43, 43, 43, 13),
    ("ref err 3d 1", 24, 24, 24, 9),
    ("area err 3d 1", 24, 24, 24, 9),
    ("ref3d abs 2", 31, 31, 31, 20),
    ("ref3d rel 2", 31, 31, 31, 17),
    ("area3d 2", 43, 43, 43, 31),
    ("area3d small 2", 43, 43, 43, 19),
    ("ref err 3d 2", 24, 24, 24, 15),
    ("area err 3d 2", 24, 24, 24, 15),
    ("ref3d abs 3", 31, 31, 31, 16),
    ("ref3d rel 3", 31, 31, 31, 13),
    ("area3d 3", 43, 43, 43, 27),
    ("area3d small 3", 43, 43, 43, 15),
    ("ref err 3d 3", 24, 24, 24, 11),
    ("area err 3d 3", 24, 24, 24, 11),
    ("ref3d abs 4", 17, 17, 17, 15),
    ("ref3d rel 4", 17, 17, 17, 12),
    ("area3d 4", 29, 29, 29, 26),
    ("area3d small 4", 29, 29, 29, 14),
    ("ref err 3d 4", 10, 10, 10, 10),
    ("area err 3d 4", 10, 10, 10, 10),
    ("ref3d abs 99", 17, 17, 17, 15),
    ("ref3d rel 99", 17, 17, 17, 12),
    ("area3d 99", 29, 29, 29, 26),
    ("area3d small 99", 29, 29, 29, 14),
    ("ref err 3d 99", 10, 10, 10, 10),
    ("area err 3d 99", 10, 10, 10, 10),
    ("name 1", 48, 48, 48, 8),
    ("name 2", 18, 18, 18, 3),
    ("name 3", 12, 12, 12, 6),
    ("name 4", 18, 18, 18, 5),
    ("name 5", 5, 5, 5, 5),
    ("name 99", 5, 5, 5, 5),
    ("name value class", 48, 48, 48, 8),
    ("name array class", 12, 12, 12, 6),
    ("namex", 10, 10, 10, 10),
    ("namex value", 10, 10, 10, 10),
    ("exp", 0, 0, 0, 0),
    ("array", 10, 10, 10, 10),
    ("array value", 10, 10, 10, 10),
    ("extended (skipped 5)", 5, 5, 5, 1),
    ("miss arg", 0, 0, 0, 0),
    ("miss arg then op", 7, 7, 17, 2),
    ("binary 0x03", 12, 12, 22, 11),
    ("binary 0x04", 12, 12, 22, 11),
    ("binary 0x05", 12, 12, 22, 11),
    ("binary 0x06", 12, 12, 22, 11),
    ("binary 0x07", 12, 12, 22, 11),
    ("binary 0x08", 12, 12, 22, 11),
    ("binary 0x09", 12, 12, 22, 11),
    ("binary 0x0a", 12, 12, 22, 12),
    ("binary 0x0b", 12, 12, 22, 11),
    ("binary 0x0c", 12, 12, 22, 11),
    ("binary 0x0d", 12, 12, 22, 12),
    ("binary 0x0e", 12, 12, 22, 12),
    ("binary 0x0f", 12, 12, 22, 11),
    ("binary 0x10", 12, 12, 22, 11),
    ("binary 0x11", 12, 12, 22, 11),
    ("binary one digit", 12, 12, 22, 3),
    ("binary on strings", 42, 42, 82, 11),
    ("binary underflow", 128, 7, 17, -1),
    ("binary no operands", 128, 0, 0, -1),
    ("unary plus", 6, 6, 11, 6),
    ("unary minus", 6, 6, 11, 6),
    ("percent", 6, 6, 6, 6),
    ("percent only", 128, 1, 1, -1),
    ("paren", 7, 7, 12, 7),
    ("paren twice", 9, 9, 21, 9),
    ("paren no operand", 128, 0, 0, -1),
    ("unary no operand", 128, 0, 0, -1),
    ("attr skip 0x01", 5, 5, 5, 1),
    ("attr skip 0x02", 5, 5, 5, 1),
    ("attr skip 0x08", 5, 5, 5, 1),
    ("attr skip 0x20", 5, 5, 5, 1),
    ("attr skip 0x21", 5, 5, 5, 1),
    ("attr choose 0", 5, 5, 5, 1),
    ("attr choose 2", 5, 5, 5, 1),
    ("attr sum", 10, 10, 20, 10),
    ("attr sum no operand", 128, 0, 0, -1),
    ("attr sum after binary", 17, 17, 51, 8),
    ("attr space 0 x3", 8, 8, 26, 8),
    ("attr space 1 x3", 8, 8, 26, 8),
    ("attr space 2 x3", 8, 8, 26, 8),
    ("attr space 3 x3", 8, 8, 26, 8),
    ("attr space 4 x3", 8, 8, 26, 8),
    ("attr space 5 x3", 8, 8, 26, 8),
    ("attr space 6 x3", 8, 8, 26, 8),
    ("attr space 7 x3", 128, 5, 5, -1),
    ("attr space 0 x0", 5, 5, 5, 5),
    ("attr space 1 x255", 260, 260, 33920, 260),
    ("attr space 0 x255 (after)", 260, 260, 33920, 260),
    ("attr space twice", 305, 305, 46655, 301),
    ("attr space on the second operand", 22, 22, 147, 13),
    ("attr space no operand", 128, 0, 0, -1),
    ("attr unknown", 128, 5, 5, -1),
    ("func 0 args", 26, 26, 26, 4),
    ("func 1 arg", 31, 31, 41, 6),
    ("func 2 args", 37, 37, 57, 10),
    ("func 3 args", 43, 43, 73, 9),
    ("func 1 arg missing", 128, 0, 0, -1),
    ("func variable (not var token)", 128, 5, 5, -1),
    ("func iftab out of range", 128, 5, 5, -1),
    ("func iftab max", 128, 5, 5, -1),
    ("funcvar 0 args", 26, 26, 26, 5),
    ("funcvar 1 arg", 31, 31, 41, 6),
    ("funcvar 5 args", 55, 55, 105, 14),
    ("funcvar 2 args of 65535", 37, 37, 57, 16),
    ("funcvar more args than operands", 128, 5, 5, -1),
    ("funcvar iftab out of range", 128, 5, 5, -1),
    ("funcvar value class", 31, 31, 41, 6),
    ("funcvar array class", 31, 31, 41, 6),
    ("func value class", 31, 31, 41, 6),
    ("func array class", 31, 31, 41, 6),
    ("two values", 128, 10, 10, -1),
    ("empty", 128, 0, 0, -1),
    ("unhandled 0x00", 128, 5, 5, -1),
    ("unhandled 0x02", 128, 5, 5, -1),
    ("unhandled 0x26", 128, 5, 5, -1),
    ("unhandled 0x27", 128, 5, 5, -1),
    ("unhandled 0x28", 128, 5, 5, -1),
    ("unhandled 0x29", 128, 5, 5, -1),
    ("unhandled 0x46", 128, 5, 5, -1),
    ("unhandled 0x66", 128, 5, 5, -1),
    ("unhandled 0x38", 128, 5, 5, -1),
    ("unhandled 0x77", 128, 5, 5, -1),
    ("unhandled 0xff", 128, 5, 5, -1),
];

/// What the cost of one formula must be against what `calamine` printed: an error for `calamine`
/// is an error here (the message is as long as `ERROR_TEXT` says at most), anything else is
/// never charged under its real length, and the work is at least what was written.
fn assert_sound(what: &str, c: &Cost, real: &str) {
    if real.starts_with(DROPPED) {
        assert!(
            c.failed,
            "{what}: calamine gave up ({real:?}), the cost did not"
        );
        assert_eq!(c.text, ERROR_TEXT, "{what}");
        assert!(
            real.len() as u64 <= ERROR_TEXT,
            "{what}: the message ({} bytes) is longer than ERROR_TEXT",
            real.len()
        );
    } else {
        assert!(
            !c.failed,
            "{what}: parsed by calamine ({real:?}), the cost gave up"
        );
        assert!(
            c.text >= real.len() as u64,
            "{what}: charged {} for {} bytes ({real:?})",
            c.text,
            real.len()
        );
        assert!(
            c.peak >= c.text,
            "{what}: peak {} < text {}",
            c.peak,
            c.text
        );
        assert!(
            c.work >= real.len() as u64,
            "{what}: work {} < the {} bytes written",
            c.work,
            real.len()
        );
    }
}

#[test]
fn every_token_class_is_charged_at_least_what_calamine_prints_and_exactly_what_is_pinned() {
    let s = samples();
    let r = run(&Raw::standard(s.iter().map(|(_, t)| t.clone()).collect()));
    assert_eq!(
        PINS.len(),
        s.len(),
        "a sample without a pin (or the other way round)"
    );
    for (i, (label, _)) in s.iter().enumerate() {
        let (pl, text, peak, work, real_len) = PINS[i];
        assert_eq!(*label, pl, "pin {i} belongs to another sample");
        let (c, real) = (&r.cost[i], &r.real[i]);
        assert_sound(label, c, real);
        let real_len_now = if real.starts_with(DROPPED) {
            -1
        } else {
            real.len() as i64
        };
        assert_eq!(real_len_now, real_len, "{label}: calamine printed {real:?}");
        assert_eq!((c.text, c.peak, c.work), (text, peak, work), "{label}");
    }
}

#[test]
fn the_same_tokens_in_a_biff5_workbook_are_laid_out_the_old_way_and_still_bounded() {
    // Before BIFF8 a reference has 3 (or 6) operand bytes and a 3d reference names its sheet by
    // index; `calamine` ignores the EXTERNSHEET record there and makes the XTI table the identity.
    let mut s = samples();
    let mut add = |label: &str, rgce: Vec<u8>| s.push((label.to_string(), rgce));
    add("biff5 ref", vec![0x24, 0xFF, 0xFF, 0xFF]);
    add(
        "biff5 ref then int",
        [vec![0x24, 5, 0, 7], int(1), vec![0x03]].concat(),
    );
    add("biff5 ref value class", vec![0x44, 5, 0xC0, 7]);
    add("biff5 area", vec![0x25, 1, 0, 2, 0, 3, 4]);
    add(
        "biff5 area then int",
        [vec![0x25, 1, 0, 2, 0, 3, 4], int(9), vec![0x04]].concat(),
    );
    add("biff5 ref err", vec![0x2A, 0, 0, 0]);
    add(
        "biff5 ref err then int",
        [vec![0x2A, 0, 0, 0], int(9), vec![0x04]].concat(),
    );
    add("biff5 area err", vec![0x2B, 0, 0, 0, 0, 0, 0]);
    add(
        "biff5 area err then int",
        [vec![0x2B, 0, 0, 0, 0, 0, 0], int(9), vec![0x04]].concat(),
    );
    // The identity XTI: sheet 2 has the 20-character name whatever the EXTERNSHEET says.
    for ixti in 0..5u16 {
        add(&format!("biff5 ref3d {ixti}"), ref3d(0x3A, ixti, 1, 1));
        add(
            &format!("biff5 area3d {ixti}"),
            area3d(0x3B, ixti, 1, 2, 1, 2),
        );
    }
    // One workbook per formula: where the old layout leaves `calamine` slicing past a short
    // operand it panics (for the whole book), and the cost must have given up there too.
    let mut r = Run {
        cost: Vec::new(),
        real: Vec::new(),
    };
    let mut panicked = 0;
    for (label, rgce) in &s {
        let raw = Raw::standard5(vec![rgce.clone()]);
        let (cost, real) = run_guarded(&raw);
        match real {
            Some(text) => assert_sound(&format!("{label} (biff5)"), &cost, &text),
            None => {
                panicked += 1;
                assert!(
                    cost.failed,
                    "{label} (biff5): calamine panics on it, the cost must give up"
                );
            }
        }
        r.cost.push(cost);
        r.real.push(rgce.len().to_string());
    }
    assert!(panicked > 0 && panicked < s.len() / 2, "{panicked}");
    // The ones that matter below, run together (they do not panic).
    let keep: Vec<usize> = s
        .iter()
        .enumerate()
        .filter(|(_, (l, _))| l.starts_with("biff5"))
        .map(|(i, _)| i)
        .collect();
    let r = run(&Raw::standard5(
        keep.iter().map(|&i| s[i].1.clone()).collect(),
    ));
    let s: Vec<(String, Vec<u8>)> = keep.iter().map(|&i| s[i].clone()).collect();
    let at = |label: &str| s.iter().position(|(l, _)| l == label).unwrap();
    // The operand lengths decide whether what follows a reference is read as a token: with the
    // 3-byte layout the `int` after it is one, and the formula parses.
    for label in [
        "biff5 ref then int",
        "biff5 area then int",
        "biff5 ref err then int",
        "biff5 area err then int",
    ] {
        let i = at(label);
        assert!(!r.real[i].starts_with(DROPPED), "{label}: {:?}", r.real[i]);
        assert!(!r.cost[i].failed, "{label}");
    }
    // Sheet 2 (20 characters, 6 bytes each) through the identity table, sheet 0 (1 character).
    let ref3d2 = at("biff5 ref3d 2");
    assert!(
        r.real[ref3d2].starts_with("AAAAAAAAAAAAAAAAAAAA!"),
        "{:?}",
        r.real[ref3d2]
    );
    assert_eq!(r.cost[ref3d2].text, 20 * 6 + 1 + 12);
    assert_eq!(r.cost[at("biff5 ref3d 0")].text, 6 + 1 + 12);
    assert_eq!(r.cost[at("biff5 ref3d 3")].text, 3 * 6 + 1 + 12);
    // Past the sheets: #REF.
    assert_eq!(r.cost[at("biff5 ref3d 4")].text, 4 + 1 + 12);
    assert_eq!(r.cost[at("biff5 area3d 2")].text, 20 * 6 + 1 + 24);
    // Operand sizes: ref 3 bytes, area 6.
    assert_eq!(r.cost[at("biff5 ref")].text, 12);
    assert_eq!(r.cost[at("biff5 area")].text, 40);
}

#[test]
fn every_function_id_and_argument_count_is_read_like_calamine_does() {
    // FTAB_ARGC is `calamine`'s private table: every function id, called with 0..=12 operands (as
    // a fixed-arity token and as a variable one), either parses for both or fails for both, and
    // the text is never charged under what was printed.
    let mut formulas = Vec::new();
    let mut labels = Vec::new();
    for iftab in 0..485u16 {
        for k in 0..=12usize {
            let ints: Vec<u8> = (0..k).flat_map(|_| int(1)).collect();
            formulas.push([ints.clone(), func(iftab)].concat());
            labels.push(format!("func {iftab} with {k}"));
            formulas.push([ints, func_var(k as u8, iftab)].concat());
            labels.push(format!("funcvar {iftab} with {k}"));
        }
    }
    let r = run(&Raw::standard(formulas));
    let mut longest = 0;
    for (i, label) in labels.iter().enumerate() {
        assert_sound(label, &r.cost[i], &r.real[i]);
        if label.starts_with("funcvar") && label.ends_with("with 1") {
            // "NAME(1)"
            longest = longest.max(r.real[i].len() - 3);
        }
    }
    // The longest function name of the table is what FN_NAME has to cover.
    assert_eq!(longest, 18, "CUBEMEMBERPROPERTY");
    assert!(formula::FN_NAME >= longest as u64);
    assert!(
        formula::FN_NAME < 3 * longest as u64,
        "no reason to charge much more"
    );
}

#[test]
fn a_name_that_is_not_there_costs_what_ref_costs_and_the_name_index_wraps_like_calamine() {
    // Index 0 wraps to `usize::MAX` (calamine panics in a debug build, so this is the cost alone).
    let env = formula::Env {
        names: vec![40],
        ..Default::default()
    };
    let body = |rgce: &[u8]| [&(rgce.len() as u16).to_le_bytes()[..], rgce].concat();
    for (rgce, text) in [
        (name_tok(0), 5),
        (name_tok(1), 40),
        (name_tok(2), 5),
        (name_tok(u32::MAX), 5),
    ] {
        assert_eq!(
            formula::formula_cost(&body(&rgce), &env).text,
            text,
            "{rgce:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// the size of a string does not depend on the code page
// ---------------------------------------------------------------------------------------------

/// Code pages `calamine` can be given (by the CODEPAGE record): single-byte, multi-byte, ISO-2022
/// (stateful), GB18030, UTF-8, UTF-16 both ways.
const CODEPAGES: &[u16] = &[
    437, 850, 874, 932, 936, 949, 950, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258, 10000,
    20127, 28591, 28592, 50220, 50221, 50222, 51932, 51949, 54936, 65001, 1200, 1201,
];

/// A random string of `cch` characters made of what hurts a decoder most: ESC sequences, lead
/// bytes without a trail, NULs, ASCII, high bytes, UTF-16 surrogates.
fn random_str(seed: &mut u64, wide: bool) -> Str {
    let cch = (xorshift(seed) % 24) as usize + 1;
    let mut bytes = Vec::new();
    for _ in 0..cch {
        let r = xorshift(seed);
        let unit: u16 = match r % 8 {
            0 => 0x1B,
            1 => u16::from(b"$B()J@"[(r >> 8) as usize % 6]),
            2 => (r >> 8) as u16 % 0x80,
            3 => 0x80 + (r >> 8) as u16 % 0x80,
            4 => 0xD800 + (r >> 8) as u16 % 0x800,
            5 => 0,
            _ => (r >> 8) as u16,
        };
        if wide {
            bytes.extend(unit.to_le_bytes());
        } else {
            bytes.push(unit as u8);
        }
    }
    (wide, cch, bytes)
}

#[test]
fn a_string_is_charged_its_worst_case_whatever_the_code_page() {
    let mut seed = 0x1234_5678_9ABC_DEF1u64;
    let mut ran = 0;
    for &cp in CODEPAGES {
        for round in 0..12 {
            let mut raw = Raw::standard(vec![]);
            raw.codepage = Some(cp);
            // Random sheet and defined names (both widths), then formulas that print them.
            raw.sheets = vec![
                random_str(&mut seed, false),
                random_str(&mut seed, round % 2 == 0),
                random_str(&mut seed, true),
                random_str(&mut seed, false),
            ];
            raw.names = (0..3)
                .map(|k| (random_str(&mut seed, k % 2 == 1), vec![0x1E, 1, 0]))
                .collect();
            raw.xti = vec![0, 1, 2, 3];
            let mut formulas: Vec<Vec<u8>> = (1..=3).map(name_tok).collect();
            for ixti in 0..4 {
                formulas.push(ref3d(0x3A, ixti, 0, 0));
                formulas.push(area3d(0x3B, ixti, 0, 0, 0, 0));
                formulas.push(err3d(0x3C, ixti, 6));
            }
            for wide in [false, true] {
                let (_, cch, b) = random_str(&mut seed, wide);
                formulas.push(ptg_str(cch as u8, u8::from(wide), &b));
            }
            raw.formulas = formulas;
            let stream = raw.stream();
            let container = fmt_xls::clean_container(&stream).unwrap();
            let Ok(mut wb) = calamine::Xls::new(container) else {
                continue;
            };
            let cost: Vec<Cost> = fmt_xls::formula_costs(&stream, &Limits::default())
                .unwrap()
                .into_iter()
                .map(|(_, c)| c)
                .collect();
            let name = wb.sheet_names()[1].clone();
            let real: Vec<String> = wb
                .worksheet_formula(&name)
                .unwrap()
                .cells()
                .map(|(_, _, s)| s.clone())
                .collect();
            assert_eq!(real.len(), cost.len(), "cp {cp}");
            for (i, (c, r)) in cost.iter().zip(&real).enumerate() {
                assert_sound(&format!("cp {cp} round {round} formula {i}"), c, r);
            }
            ran += 1;
        }
    }
    assert!(ran > 200, "{ran} workbooks");
}

#[test]
fn worst_case_strings_under_the_awkward_code_pages_are_covered_exactly_at_the_edges() {
    // ISO-2022-JP in a two-byte mode turns every byte of a compressed string into U+FFFD (the
    // widened `x, 0` pairs are invalid), a surrogate pair is two characters, ESC is not ASCII.
    let esc_mode = |wide: bool| -> Str {
        let mut units: Vec<u16> = vec![0x1B, u16::from(b'$'), u16::from(b'B')];
        units.extend((0..20).map(|k| 0x21 + k));
        let bytes = if wide {
            units.iter().flat_map(|u| u.to_le_bytes()).collect()
        } else {
            units.iter().map(|&u| u as u8).collect()
        };
        (wide, units.len(), bytes)
    };
    for cp in [50220u16, 1252, 932] {
        let mut raw = Raw::standard(vec![name_tok(1), ref3d(0x3A, 0, 0, 0)]);
        raw.codepage = Some(cp);
        raw.names = vec![(esc_mode(false), vec![0x1E, 1, 0])];
        raw.sheets[0] = esc_mode(true);
        let r = run(&raw);
        for (i, c) in r.cost.iter().enumerate() {
            assert_sound(&format!("cp {cp} formula {i}"), c, &r.real[i]);
        }
    }
    // Sheet names with surrogate pairs: 4 bytes of input per character, up to 12 bytes out.
    let pair: Str = (true, 8, [0x3D, 0xD8, 0x00, 0xDE].repeat(4));
    let mut raw = Raw::standard(vec![ref3d(0x3A, 0, 0, 0)]);
    raw.sheets[0] = pair;
    let r = run(&raw);
    assert_sound("pairs", &r.cost[0], &r.real[0]);
    assert_eq!(
        r.cost[0].text,
        8 * 6 + 1 + 12,
        "6 per UTF-16 unit, a pair is two"
    );
}

#[test]
fn sizes_of_strings_are_per_unit_and_never_more_than_what_is_in_the_record() {
    // `text_size`: the units that are there, at the worst case each.
    assert_eq!(formula::text_size(b"abc", 3, false), 3 * formula::CHAR_MAX);
    assert_eq!(formula::text_size(b"abc", 2, false), 2 * formula::CHAR_MAX);
    assert_eq!(formula::text_size(b"abc", 9, false), 3 * formula::CHAR_MAX);
    assert_eq!(
        formula::text_size(&[1, 2, 3, 4, 5], 9, true),
        2 * formula::CHAR_MAX
    );
    assert_eq!(
        formula::text_size(&[1, 2, 3, 4, 5, 6], 2, true),
        2 * formula::CHAR_MAX
    );
    assert_eq!(formula::text_size(&[], 9, true), 0);
    assert_eq!(formula::CHAR_MAX, 6);
}

// ---------------------------------------------------------------------------------------------
// the text of an unparseable formula
// ---------------------------------------------------------------------------------------------

#[test]
fn the_message_for_a_formula_calamine_cannot_parse_fits_error_text_at_the_largest_cell() {
    let mut shapes: Vec<Vec<u8>> = vec![
        vec![0x1C, 0xFF],
        ptg_str(255, 1, &[1, 0, 2, 0, 3]),
        [ptg_str(255, 1, &[1, 0, 2, 0, 3])].concat(),
        [int(1), attr(0x40, &[255, 1])].concat(),
        [int(1), attr(0x77, &[255, 1])].concat(),
        [int(1), func(0xFFFF)].concat(),
        [int(1), func_var(255, 0xFFFF)].concat(),
        vec![0xFF],
        vec![0x03],
        vec![],
        vec![0x01, 1],
        func_var(255, 1),
    ];
    // Many values left on the stack: `InvalidFormula { stack_size: N }`.
    shapes.push((0..255).flat_map(|_| int(1)).collect());
    shapes.push((0..2000).flat_map(|_| int(1)).collect());
    let mut longest = 0;
    for rgce in shapes {
        let mut raw = Raw::standard(vec![rgce.clone()]);
        raw.cell = Some((65535, 255));
        let r = run(&raw);
        assert!(
            r.real[0].starts_with(DROPPED),
            "{rgce:02x?}: {:?}",
            r.real[0]
        );
        assert!(r.cost[0].failed && r.cost[0].text == ERROR_TEXT);
        longest = longest.max(r.real[0].len() as u64);
    }
    assert!(longest <= ERROR_TEXT, "{longest}");
    assert!(
        longest + 40 >= ERROR_TEXT,
        "ERROR_TEXT is not tight: {longest}"
    );
}

// ---------------------------------------------------------------------------------------------
// the budgets and where each comparison falls
// ---------------------------------------------------------------------------------------------

#[test]
fn the_defaults_and_the_caps_are_what_the_documentation_says() {
    let l = Limits::default();
    assert_eq!(l.max_formula_text_bytes, 128 << 20);
    assert_eq!(l.max_formula_work, 1 << 33);
    assert_eq!(fmt_xls::MAX_FORMULA_PEAK, 4 << 20);
    assert_eq!(fmt_xls::MAX_FORMULA_WORK, 1 << 27);
}

#[test]
fn every_comparison_of_the_budgets_is_exact_at_its_boundary() {
    let cost = |text, peak, work| Cost {
        text,
        peak,
        work,
        failed: false,
    };
    let big = u64::MAX;
    // The per-formula caps: at the cap passes, one over does not (each alone).
    let peak = fmt_xls::MAX_FORMULA_PEAK;
    let work = fmt_xls::MAX_FORMULA_WORK;
    assert!(!fmt_xls::is_dropped_when(cost(10, peak, 10), big, big));
    assert!(fmt_xls::is_dropped_when(cost(10, peak + 1, 10), big, big));
    assert!(!fmt_xls::is_dropped_when(cost(10, 10, work), big, big));
    assert!(fmt_xls::is_dropped_when(cost(10, 10, work + 1), big, big));
    // The workbook budgets: what is left may be used up exactly.
    assert!(!fmt_xls::is_dropped_when(cost(100, 100, 5), 100, big));
    assert!(fmt_xls::is_dropped_when(cost(100, 100, 5), 99, big));
    assert!(!fmt_xls::is_dropped_when(cost(5, 5, 100), big, 100));
    assert!(fmt_xls::is_dropped_when(cost(5, 5, 100), big, 99));
    // And what an admitted formula uses is taken off, for both budgets.
    let mut left = (300u64, 300u64);
    for _ in 0..3 {
        assert!(!fmt_xls::is_dropped_when(
            cost(100, 100, 100),
            left.0,
            left.1
        ));
        left = (left.0 - 100, left.1 - 100);
    }
    assert!(fmt_xls::is_dropped_when(
        cost(100, 100, 100),
        left.0,
        left.1
    ));
}

/// 2,741 names of 255 characters, 114 `PtgInt` and one more token: a stack that is never folded
/// (so calamine gives up on it) with a peak that is exactly `4 MiB + extra`.
fn peak_formula(last: &[u8], extra: u64) -> Vec<u8> {
    let mut rgce: Vec<u8> = (0..2741).flat_map(|_| name_tok(1)).collect();
    for _ in 0..114 {
        rgce.extend(int(1));
    }
    rgce.extend(last);
    let _ = extra;
    rgce
}

#[test]
fn a_formula_over_the_peak_cap_alone_is_dropped_and_one_at_the_cap_is_not() {
    // `#N/A` is 4 bytes: 2741 x 1530 + 114 x 5 + 4 = 4 MiB exactly; `#REF!` is 5: one over.
    for (last, over) in [(vec![0x1C, 0x2A], false), (vec![0x1C, 0x17], true)] {
        let mut raw = Raw::standard(vec![peak_formula(&last, 0)]);
        raw.names[0] = (s8(&"N".repeat(255)), vec![0x1E, 1, 0]);
        let stream = raw.stream();
        let costs = fmt_xls::formula_costs(&stream, &Limits::default()).unwrap();
        let c = costs[0].1;
        assert_eq!(c.peak, fmt_xls::MAX_FORMULA_PEAK + u64::from(over));
        // Only the peak is anywhere near a cap.
        assert!(c.work < 1 << 24 && c.text == ERROR_TEXT);
        // What calamine prints for the scanned (rewritten) stream.
        let dir = tmp("xls_peak_cap");
        let p = dir.join("p.xls");
        write_cfb(&p, &[("/Workbook", &stream)]);
        let (_, container) = fmt_xls::read_scanned(&p, &Limits::default(), None).unwrap();
        let mut wb = calamine::Xls::new(container).unwrap();
        let text = wb
            .worksheet_formula("Two")
            .unwrap()
            .cells()
            .next()
            .map(|(_, _, s)| s.clone())
            .unwrap();
        if over {
            assert!(
                text.is_empty(),
                "dropped: {:?}",
                &text[..text.len().min(40)]
            );
        } else {
            assert!(
                text.starts_with(DROPPED),
                "kept (calamine gives up on it): {text:?}"
            );
        }
    }
}

#[test]
fn a_dropped_formula_prints_nothing_and_leaves_the_record_the_same_length() {
    // The replacement is one PtgMissArg (`cce = 1`, token 0x16): an `Ok("")` for calamine, not an
    // error message; the bytes after it are the same so nothing else moves.
    let rgce: Vec<u8> = (0..3)
        .flat_map(|_| int(5))
        .chain(vec![0x03, 0x03])
        .collect();
    let book = Raw::standard(vec![rgce.clone(), rgce.clone()]);
    let stream = book.stream();
    // Each is charged 19 (three numbers and two operators of 2 bytes): room for one.
    let limits = Limits {
        max_formula_text_bytes: 19,
        ..Limits::default()
    };
    let (_, blank) = fmt_xls::scan_stream(&stream, &limits, None).unwrap();
    assert_eq!(blank.len(), 1, "the second one does not fit: {blank:?}");
    let mut patched = stream.clone();
    fmt_xls::apply_blanks(&mut patched, &blank);
    assert_eq!(patched.len(), stream.len());
    let at = match blank[0] {
        fmt_xls::Blank::Formula(at) => at,
        other => panic!("{other:?}"),
    };
    assert_eq!(&patched[at..at + 3], &[1, 0, 0x16]);
    let changed: Vec<usize> = (0..stream.len())
        .filter(|&i| stream[i] != patched[i])
        .collect();
    assert!(
        changed.iter().all(|&i| (at..at + 3).contains(&i)),
        "{changed:?}"
    );
    // calamine: the first keeps its text, the second is empty (not "Unrecognised formula").
    let container = fmt_xls::clean_container(&patched).unwrap();
    let mut wb = calamine::Xls::new(container).unwrap();
    let f: Vec<String> = wb
        .worksheet_formula("Two")
        .unwrap()
        .cells()
        .map(|(_, _, s)| s.clone())
        .collect();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0], "5+5+5");
    // (`5 5 5 + +` is the right-nested form: 5+5+5 is what the stack machine writes for it.)
    assert_eq!(f[1], "");
}

#[test]
fn a_formula_with_no_room_for_the_replacement_is_refused_not_left_to_print_an_error() {
    // A FORMULA record that stops right after `cce` (no token byte to overwrite) and is over
    // budget: the only way to keep `calamine` from printing a message per record is to refuse.
    let mut d = pos(0, 0, 0);
    d.extend(1.0f64.to_le_bytes());
    d.extend([0; 6]);
    d.extend([0, 0]); // cce = 0: no tokens at all
    let sheet_records = vec![rec(0x0006, &d)];
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", sheet_records)],
    };
    let stream = book.stream();
    // Nothing over budget: fine (calamine prints one short message).
    assert!(fmt_xls::scan_stream(&stream, &Limits::default(), None).is_ok());
    let tight = Limits {
        max_formula_text_bytes: 10,
        ..Limits::default()
    };
    assert!(matches!(
        fmt_xls::scan_stream(&stream, &tight, None),
        Err(OfficeError::TooLarge { .. })
    ));
}

// ---------------------------------------------------------------------------------------------
// defined names that start with a sheet reference
// ---------------------------------------------------------------------------------------------

/// Defined names whose first token is `first` (with the sheet index at the BIFF layout's place),
/// each charged 1,240 bytes (a 200-character sheet name, 6 bytes each, + 40) against a quarter of
/// the formula text budget that has room for exactly `room` of them: how many keep a `!`.
fn names_with_prefix(first: &[u8], biff5: bool, room: usize) -> usize {
    let mut rgce = first.to_vec();
    rgce.resize(24, 0);
    let mut raw = Raw::standard(vec![]);
    raw.biff5 = biff5;
    raw.sheets = vec![s8(&"S".repeat(200)), s8("T")];
    raw.names = (0..40)
        .map(|i| (s8(&format!("n{i}")), rgce.clone()))
        .collect();
    raw.xti = vec![0];
    raw.formula_sheet = 1;
    let stream = raw.stream();
    let limits = Limits {
        max_formula_text_bytes: 4 * 1240 * room as u64,
        ..Limits::default()
    };
    let dir = tmp("xls_name_prefix_variants");
    let p = dir.join("n.xls");
    write_cfb(&p, &[("/Workbook", &stream)]);
    let (_, container) = fmt_xls::read_scanned(&p, &limits, None).unwrap();
    let wb = calamine::Xls::new(container).unwrap();
    assert_eq!(wb.defined_names().len(), 40);
    wb.defined_names()
        .iter()
        .filter(|(_, f)| f.contains('!'))
        .count()
}

#[test]
fn every_first_token_that_prefixes_a_sheet_name_is_charged_in_both_layouts() {
    // PtgRef3d, PtgArea3d, PtgRefErr3d, PtgAreaErr3d in all three token classes, the sheet index
    // at byte 1 (BIFF8) or byte 11 (before it). The index is 0 here (through the XTI table or the
    // identity one): the 200-character sheet.
    for biff5 in [false, true] {
        let at = if biff5 { 11 } else { 1 };
        for ptg in [
            0x3Au8, 0x5A, 0x7A, 0x3B, 0x5B, 0x7B, 0x3C, 0x5C, 0x7C, 0x3D, 0x5D, 0x7D,
        ] {
            let mut tok = vec![0u8; 22];
            tok[0] = ptg;
            tok[at] = 0;
            assert_eq!(
                names_with_prefix(&tok, biff5, 7),
                7,
                "ptg {ptg:#04x} biff5={biff5}: every one of the first seven is charged"
            );
            // Where it is not a prefix token nothing is charged (the shape of a plain number).
        }
        assert_eq!(names_with_prefix(&[0x1E, 1, 0], biff5, 7), 0);
    }
}

#[test]
fn the_sheet_index_of_a_prefixed_name_is_read_at_the_right_byte() {
    // Index 1 (the one-character sheet, 6 + 40 bytes) in the right place, with the other layout's
    // place holding 0 (the big sheet): charged 46 per name, not 1,240.
    for biff5 in [false, true] {
        for ptg in [0x3Au8, 0x3C] {
            let (at, other) = if biff5 { (11, 1) } else { (1, 11) };
            let mut tok = vec![0u8; 22];
            tok[0] = ptg;
            tok[at] = 1;
            tok[other] = 0;
            // Room for 7 big ones: all 40 small ones (40 x 46 = 1,840 < 7 x 1,240) fit.
            let mut raw = Raw::standard(vec![]);
            raw.biff5 = biff5;
            raw.sheets = vec![s8(&"S".repeat(200)), s8("T")];
            raw.names = (0..40)
                .map(|i| (s8(&format!("n{i}")), tok.clone()))
                .collect();
            raw.xti = vec![0, 1];
            let stream = raw.stream();
            let limits = Limits {
                max_formula_text_bytes: 4 * 1240 * 7,
                ..Limits::default()
            };
            let blank = fmt_xls::scan_stream(&stream, &limits, None).unwrap().1;
            assert!(
                blank.is_empty(),
                "ptg {ptg:#04x} biff5={biff5}: {} dropped",
                blank.len()
            );
        }
    }
}

/// A NAME record that is nothing but its formula: `cce` is the whole body.
fn name_that_is_all_formula() -> Vec<u8> {
    let mut d = vec![0u8; 24];
    d[0] = 0x3A;
    d[1] = 0; // ixti 0
    d[3] = 0; // cch 0
    d[4..6].copy_from_slice(&24u16.to_le_bytes());
    rec(0x0018, &d)
}

#[test]
fn a_name_record_whose_formula_is_the_whole_body_or_empty_is_handled() {
    let mut raw = Raw::standard(vec![]);
    raw.sheets = vec![s8(&"S".repeat(200)), s8("T")];
    raw.xti = vec![0];
    let mut stream = raw.stream();
    // Splice a NAME record into the globals (right before the first BOUNDSHEET) and fix the
    // sheet positions by the added length.
    let bs = 0x0085u16.to_le_bytes();
    let at = stream
        .windows(2)
        .position(|w| w == bs)
        .expect("a BOUNDSHEET record");
    let extra = name_that_is_all_formula();
    // The empty one: cce = 0.
    let empty = rec(0x0018, &{
        let mut d = vec![0u8; 15];
        d[3] = 0;
        d
    });
    let spliced: Vec<u8> = [extra.clone(), empty.clone()].concat();
    stream.splice(at..at, spliced.iter().copied());
    for (i, w) in stream.clone().windows(2).enumerate() {
        if w == bs && i >= at + spliced.len() {
            let p =
                u32::from_le_bytes(stream[i + 4..i + 8].try_into().unwrap()) + spliced.len() as u32;
            stream[i + 4..i + 8].copy_from_slice(&p.to_le_bytes());
        }
    }
    // With no room for the prefix the one whose formula is the whole record is rewritten (cce
    // zeroed) and the empty one is left alone, without a panic.
    let tight = Limits {
        max_formula_text_bytes: 8,
        ..Limits::default()
    };
    let (_, blank) = fmt_xls::scan_stream(&stream, &tight, None).unwrap();
    assert_eq!(blank.len(), 1, "{blank:?}");
    let fmt_xls::Blank::Name(cce_at) = blank[0] else {
        panic!("{blank:?}")
    };
    assert_eq!(cce_at, at + 4 + 4);
    assert_eq!(&stream[cce_at..cce_at + 2], &24u16.to_le_bytes());
    // And with room, nothing is rewritten.
    let (_, none) = fmt_xls::scan_stream(&stream, &Limits::default(), None).unwrap();
    assert!(none.is_empty());
}

#[test]
fn name_sizes_are_read_from_the_right_bytes_in_both_layouts() {
    // The size of a defined name (what `PtgName` prints): the flags byte at 14 and the characters
    // from 15 in BIFF8, the characters from 14 before it. Pinned through formula costs.
    for biff5 in [false, true] {
        let mut raw = if biff5 {
            Raw::standard5(vec![name_tok(1), name_tok(2)])
        } else {
            Raw::standard(vec![name_tok(1), name_tok(2)])
        };
        raw.names = vec![
            (s8("Seven77"), vec![0x1E, 1, 0]),
            (s8("x"), vec![0x1E, 1, 0]),
        ];
        let r = run(&raw);
        assert_eq!(r.real[0], "Seven77");
        assert_eq!(r.cost[0].text, 7 * formula::CHAR_MAX, "biff5={biff5}");
        assert_eq!(r.cost[1].text, formula::CHAR_MAX, "biff5={biff5}");
    }
}

#[test]
fn the_name_budget_is_exact_at_its_boundary() {
    // 10 prefixes of 1,240 each in exactly a quarter of 4 x 12,400: the tenth is the last one.
    assert_eq!(names_with_prefix(&[0x3A, 0, 0], false, 10), 10);
    assert_eq!(names_with_prefix(&[0x3A, 0, 0], false, 9), 9);
    assert_eq!(names_with_prefix(&[0x3A, 0, 0], false, 0), 0);
}

// ---------------------------------------------------------------------------------------------
// EXTERNSHEET, the identity table and the version of the sheets
// ---------------------------------------------------------------------------------------------

#[test]
fn an_externsheet_in_a_biff8_workbook_maps_xti_to_sheets_and_counts_and_offsets_are_exact() {
    // XTI 0 -> sheet 2 (the 20-character one), XTI 1 -> sheet 0, in the 6-byte structs after the
    // count. A count smaller than the structs present cuts the table; a bigger one is bounded by
    // the data.
    let mut raw = Raw::standard(vec![
        ref3d(0x3A, 0, 0, 0),
        ref3d(0x3A, 1, 0, 0),
        ref3d(0x3A, 2, 0, 0),
    ]);
    raw.sheets = vec![s8("S"), s8("Two"), s8(&"A".repeat(20)), s8("Ünï")];
    raw.xti = vec![2, 0, 3];
    let r = run(&raw);
    assert_eq!(r.real[0], "AAAAAAAAAAAAAAAAAAAA!A1");
    assert_eq!(r.cost[0].text, 20 * 6 + 1 + 12);
    assert_eq!(r.cost[1].text, 6 + 1 + 12);
    assert_eq!(r.cost[2].text, 3 * 6 + 1 + 12);
    for (i, c) in r.cost.iter().enumerate() {
        assert_sound(&format!("xti {i}"), c, &r.real[i]);
    }
    // The count says 2: the third struct is not an XTI (calamine's `take(cxti)`).
    let stream = raw.stream();
    let at = stream
        .windows(4)
        .position(|w| w == [0x17, 0x00, 20, 0])
        .expect("EXTERNSHEET");
    let mut cut = stream.clone();
    cut[at + 4..at + 6].copy_from_slice(&2u16.to_le_bytes());
    let costs = fmt_xls::formula_costs(&cut, &Limits::default()).unwrap();
    assert_eq!(costs[2].1.text, 4 + 1 + 12, "xti 2 is not there: #REF");
    assert_eq!(costs[0].1.text, 20 * 6 + 1 + 12);
    // A count that is too big takes what is there.
    let mut big = stream.clone();
    big[at + 4..at + 6].copy_from_slice(&200u16.to_le_bytes());
    let costs = fmt_xls::formula_costs(&big, &Limits::default()).unwrap();
    assert_eq!(costs[2].1.text, 3 * 6 + 1 + 12);
    // An XTI that points at itabFirst (bytes 2..4), not at the first two bytes (the book index).
    let mut other = stream.clone();
    other[at + 6..at + 8].copy_from_slice(&3u16.to_le_bytes()); // supbook of XTI 0
    let costs = fmt_xls::formula_costs(&other, &Limits::default()).unwrap();
    assert_eq!(costs[0].1.text, 20 * 6 + 1 + 12);
}

#[test]
fn a_negative_xti_or_one_past_the_sheets_is_ref() {
    let mut raw = Raw::standard(vec![ref3d(0x3A, 0, 0, 0), ref3d(0x3A, 1, 0, 0)]);
    raw.xti = vec![-1, 4];
    let r = run(&raw);
    for i in 0..2 {
        assert_eq!(r.cost[i].text, 4 + 1 + 12);
        assert!(r.real[i].starts_with("#REF!"), "{:?}", r.real[i]);
    }
}

#[test]
fn the_globals_version_decides_how_every_sheet_is_read_even_for_costs() {
    // A BIFF5 workbook: a LABEL's characters start at byte 8 with no flags byte, so bit 0 of the
    // first character is not a "16-bit" flag. The same text costs the same whatever that bit is.
    let label = |first: u8| {
        let mut d = pos(0, 0, 0);
        d.extend(10u16.to_le_bytes());
        d.push(first);
        d.extend(b"abcdefghi");
        rec(0x0204, &d)
    };
    let cost = |first: u8| {
        let book = Book {
            globals: globals(&[], &[]),
            sheets: vec![sheet("S", vec![label(first)])],
        };
        let mut stream = book.stream();
        // Globals' BOF (the first record) says BIFF5; so does the sheet's.
        stream[4..6].copy_from_slice(&0x0500u16.to_le_bytes());
        let f = fmt_xls::parse_stream(&stream, &Limits::default()).unwrap();
        f.sheets.values().next().unwrap().text_bytes
    };
    assert_eq!(
        cost(0x02),
        cost(0x01),
        "bit 0 of the first character is not a flag in BIFF5"
    );
    // In BIFF8 it is: the odd one is read as 16-bit, and costs a different amount.
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![label(0x01)])],
    };
    let a = fmt_xls::parse_stream(&book.stream(), &Limits::default())
        .unwrap()
        .sheets["S"]
        .text_bytes;
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![label(0x02)])],
    };
    let b = fmt_xls::parse_stream(&book.stream(), &Limits::default())
        .unwrap()
        .sheets["S"]
        .text_bytes;
    assert_ne!(a, b);
}

#[test]
fn a_short_bof_is_read_as_biff8_of_a_worksheet() {
    // No version, no document type: the defaults are BIFF8 (the version) and not a BIFF5 chart
    // (the type).
    assert!(fmt_xls::bof_is_biff8_for_test(&[]));
    assert!(fmt_xls::bof_is_biff8_for_test(&[0, 0]));
    assert!(fmt_xls::bof_is_biff8_for_test(&[0, 6]));
    assert!(!fmt_xls::bof_is_biff8_for_test(&[0, 5]));
    assert!(!fmt_xls::bof_is_biff8_for_test(&[0, 0, 0, 0x10]));
    assert!(fmt_xls::bof_is_biff8_for_test(&[0, 0, 5, 0]));
}

#[test]
fn the_visit_budget_is_two_passes_over_the_stream_and_a_little_more() {
    assert_eq!(fmt_xls::visit_budget(0), 16);
    assert_eq!(fmt_xls::visit_budget(3), 16);
    assert_eq!(fmt_xls::visit_budget(4), 18);
    assert_eq!(fmt_xls::visit_budget(4000), 2016);
}

#[test]
fn the_visit_budget_is_exact_at_its_boundary() {
    // Three sheets that never close (a nested BOF without its EOF) all read to the end of the
    // stream, so every empty record after the last of them is visited three times while the
    // budget grows by two per record: the surplus can be stepped one by one (and the length of the
    // last record, in bytes, moves the budget by two per four bytes). The visits are counted here
    // and held against the budget of the stream's length: at the budget the workbook is read, one
    // visit more and it is refused.
    let mut found = (false, false);
    for pad in 0..8usize {
        for after in 0..400usize {
            let (stream, visits) = book_with_open_sheets(3, after, pad);
            let budget = fmt_xls::visit_budget(stream.len());
            let r = fmt_xls::parse_stream(&stream, &Limits::default());
            if visits == budget {
                assert!(r.is_ok(), "exactly at the budget: {r:?}");
                found.0 = true;
            } else if visits == budget + 1 {
                assert!(
                    matches!(&r, Err(OfficeError::Corrupt(m)) if m.contains("share")),
                    "one over: {r:?}"
                );
                found.1 = true;
            }
        }
    }
    assert_eq!(
        found,
        (true, true),
        "the search must reach both sides of the boundary"
    );
}

/// A workbook with `k` sheets, each `BOF` then a nested `BOF` (never closed); after the last one
/// `after` empty records and one of `pad` bytes. Returns the stream and how many records the
/// sheets visit together (each reads from its own start to the end).
fn book_with_open_sheets(k: usize, after: usize, pad: usize) -> (Vec<u8>, u64) {
    let open = || [bof(0x0600, 0x0010), bof(0x0600, 0x0020)].concat();
    let mut tail = (0..after)
        .flat_map(|_| rec(0x0001, &[]))
        .collect::<Vec<u8>>();
    tail.extend(rec(0x0001, &vec![0u8; pad]));
    let mut head = bof(0x0600, 0x0005);
    let names: Vec<String> = (0..k).map(|i| format!("S{i}")).collect();
    let globals_len = head.len()
        + names
            .iter()
            .map(|n| boundsheet(0, 0, n).len())
            .sum::<usize>()
        + eof().len();
    for (i, n) in names.iter().enumerate() {
        head.extend(boundsheet((globals_len + i * open().len()) as u32, 0, n));
    }
    head.extend(eof());
    let mut stream = head;
    let starts: Vec<usize> = (0..k).map(|i| stream.len() + i * open().len()).collect();
    for _ in 0..k {
        stream.extend(open());
    }
    stream.extend(&tail);
    let count = |bytes: &[u8]| -> u64 {
        let mut n = 0;
        let mut s = bytes;
        while s.len() >= 4 {
            let len = usize::from(u16::from_le_bytes([s[2], s[3]]));
            s = &s[4 + len..];
            n += 1;
        }
        n
    };
    let visits = starts.iter().map(|&p| count(&stream[p..])).sum();
    (stream, visits)
}

// ---------------------------------------------------------------------------------------------
// the xls records through the cost model, at random: the work against a reference machine
// ---------------------------------------------------------------------------------------------

/// A plain re-implementation of the formula machine for the tokens a random generator makes
/// (ints, names, binary operators, unary signs, percent, parentheses, SUM, spaces), working on
/// real strings and counting the bytes it really moves: what `calamine` does, without its lookups.
/// Returns the text and the moved bytes (insertions move the tail, `split_off` + write back
/// copies the tail twice, every pushed byte is written once).
fn reference_machine(tokens: &[Tok], names: &[&str]) -> Option<(String, u64)> {
    let mut s = String::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut moved = 0u64;
    for t in tokens {
        match t {
            Tok::Int(v) => {
                stack.push(s.len());
                let x = v.to_string();
                moved += x.len() as u64;
                s.push_str(&x);
            }
            Tok::Name(i) => {
                stack.push(s.len());
                let n = names.get(i.wrapping_sub(1)).copied().unwrap_or("#REF!");
                moved += n.len() as u64;
                s.push_str(n);
            }
            Tok::Bin(op) => {
                let e2 = stack.pop()?;
                let tail = s.split_off(e2);
                moved += 2 * tail.len() as u64 + op.len() as u64;
                s.push_str(op);
                s.push_str(&tail);
            }
            Tok::Sign(c) => {
                let &e = stack.last()?;
                moved += (s.len() - e) as u64 + 1;
                s.insert(e, *c);
            }
            Tok::Percent => {
                s.push('%');
                moved += 1;
            }
            Tok::Paren => {
                let &e = stack.last()?;
                moved += (s.len() - e) as u64 + 1;
                s.insert(e, '(');
                s.push(')');
                moved += 1;
            }
            Tok::Sum => {
                let &e = stack.last()?;
                let tail = s.split_off(e);
                moved += 2 * tail.len() as u64 + 5;
                s.push_str("SUM(");
                s.push_str(&tail);
                s.push(')');
            }
            Tok::Space(n) => {
                let &e = stack.last()?;
                for _ in 0..*n {
                    moved += (s.len() - e) as u64 + 1;
                    s.insert(e, ' ');
                }
            }
        }
    }
    (stack.len() == 1).then_some((s, moved))
}

#[derive(Clone, Debug)]
enum Tok {
    Int(u16),
    Name(usize),
    Bin(&'static str),
    Sign(char),
    Percent,
    Paren,
    Sum,
    Space(u8),
}

fn tok_bytes(t: &Tok) -> Vec<u8> {
    match t {
        Tok::Int(v) => int(*v),
        Tok::Name(i) => name_tok(*i as u32),
        Tok::Bin(op) => {
            let ptg = [
                "+", "-", "*", "/", "^", "&", "<", "<=", "=", ">", ">=", "<>", " ", ",", ":",
            ]
            .iter()
            .position(|x| x == op)
            .unwrap() as u8
                + 3;
            vec![ptg]
        }
        Tok::Sign(c) => vec![if *c == '+' { 0x12 } else { 0x13 }],
        Tok::Percent => vec![0x14],
        Tok::Paren => vec![0x15],
        Tok::Sum => attr(0x10, &[0, 0]),
        Tok::Space(n) => attr(0x40, &[0, *n]),
    }
}

#[test]
fn random_formulas_are_charged_at_least_what_a_reference_machine_moves_and_prints() {
    let ops = [
        "+", "-", "*", "/", "^", "&", "<", "<=", "=", ">", ">=", "<>", " ", ",", ":",
    ];
    let names = ["LongName", "two", "名前", "Ünï"];
    let mut seed = 0xDEAD_BEEF_CAFE_F00Du64;
    let mut formulas: Vec<Vec<u8>> = Vec::new();
    let mut expect: Vec<(String, u64)> = Vec::new();
    while formulas.len() < 600 {
        // Build in valid order: operands push, operators fold.
        let mut toks: Vec<Tok> = Vec::new();
        let mut depth = 0usize;
        for _ in 0..(xorshift(&mut seed) % 30 + 1) {
            let r = xorshift(&mut seed);
            match (r % 9, depth) {
                (0 | 1, _) => {
                    toks.push(Tok::Int((r >> 8) as u16));
                    depth += 1;
                }
                (2, _) => {
                    toks.push(Tok::Name((r >> 8) as usize % 6));
                    depth += 1;
                }
                (3, d) if d >= 2 => {
                    toks.push(Tok::Bin(ops[(r >> 8) as usize % ops.len()]));
                    depth -= 1;
                }
                (4, d) if d >= 1 => toks.push(Tok::Sign(if r & 0x100 == 0 { '+' } else { '-' })),
                (5, d) if d >= 1 => toks.push(Tok::Percent),
                (6, d) if d >= 1 => toks.push(Tok::Paren),
                (7, d) if d >= 1 => toks.push(Tok::Sum),
                (8, d) if d >= 1 => toks.push(Tok::Space((r >> 8) as u8)),
                _ => {}
            }
        }
        // Name 0 wraps (a debug-build panic in calamine): keep to 1..=5.
        for t in &mut toks {
            if let Tok::Name(0) = t {
                *t = Tok::Name(1);
            }
        }
        while depth > 1 {
            toks.push(Tok::Bin(ops[(xorshift(&mut seed) as usize) % ops.len()]));
            depth -= 1;
        }
        if depth == 0 {
            continue;
        }
        let Some((text, moved)) = reference_machine(&toks, &names) else {
            continue;
        };
        formulas.push(toks.iter().flat_map(tok_bytes).collect());
        expect.push((text, moved));
    }
    let mut raw = Raw::standard(formulas);
    raw.names = names.iter().map(|n| (s8(n), vec![0x1E, 1, 0])).collect();
    let r = run(&raw);
    for (i, (text, moved)) in expect.iter().enumerate() {
        // The reference machine is `calamine`'s, and agrees with it.
        assert_eq!(&r.real[i], text, "formula {i}");
        let c = &r.cost[i];
        assert!(
            c.text >= text.len() as u64,
            "{i}: text {} < {}",
            c.text,
            text.len()
        );
        assert!(
            c.work >= *moved,
            "{i}: work {} < {} bytes really moved ({text:?})",
            c.work,
            moved
        );
    }
}

// ---------------------------------------------------------------------------------------------
// the compound-file front door
// ---------------------------------------------------------------------------------------------

fn header_with(ids: &[u32], first_difat: u32) -> Vec<u8> {
    let mut b = difat_only_file(ids, 0, |_| FREE);
    b[68..72].copy_from_slice(&first_difat.to_le_bytes());
    // 120 sectors so every id used here is in range.
    b.extend(vec![0u8; 512 * 120]);
    b
}

#[test]
fn the_109th_header_slot_and_the_last_slot_of_a_difat_sector_are_checked_too() {
    let mut ids: Vec<u32> = (10..119).collect();
    assert_eq!(ids.len(), 109);
    assert!(check_bytes(header_with(&ids, EOC)).0.is_ok());
    // Repeated in the very last header slot.
    ids[108] = ids[0];
    let (res, _, _) = check_bytes(header_with(&ids, EOC));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("twice")),
        "{res:?}"
    );
    // Out of range in the very last header slot.
    ids[108] = 5_000;
    let (res, _, _) = check_bytes(header_with(&ids, EOC));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("past the end")),
        "{res:?}"
    );
    // The last id slot of a chained DIFAT sector (index 126; the 128th word is the next
    // pointer): 126 good ids before it, then a repeated one / one out of range.
    let ids: Vec<u32> = (10..119).collect();
    for (value, what) in [(10u32, "twice"), (5_000, "past the end")] {
        let (res, _, _) = check_bytes(difat_only_file(&ids, 300, |n| {
            if n == 126 {
                value
            } else if n < 126 {
                119 + n as u32
            } else {
                FREE
            }
        }));
        assert!(
            matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains(what)),
            "{value}: {res:?}"
        );
    }
    // And a good one in that slot is accepted (the same layout, a fresh id).
    let (res, _, _) = check_bytes(difat_only_file(&ids, 300, |n| {
        if n <= 126 {
            119 + n as u32
        } else {
            FREE
        }
    }));
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn a_difat_start_of_free_means_there_is_no_chain() {
    let ids: Vec<u32> = (10..119).collect();
    let (res, _, _) = check_bytes(header_with(&ids, FREE));
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn the_sector_count_excludes_the_header_and_an_id_equal_to_it_is_past_the_end() {
    // 120 sectors after the header: ids 0..=119 exist, 120 does not.
    let mut ids: Vec<u32> = (10..119).collect();
    ids[108] = 119;
    assert!(check_bytes(header_with(&ids, EOC)).0.is_ok());
    ids[108] = 120;
    let (res, _, _) = check_bytes(header_with(&ids, EOC));
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("past the end")),
        "{res:?}"
    );
    // A DIFAT sector id at the boundary is past the end too.
    let mut b = difat_only_file(&[], 0, |_| FREE);
    b.extend(vec![0u8; 512 * 120]);
    // Sector 119 is a DIFAT sector of zeros (stripped padding) that ends the chain.
    b[512 + 119 * 512 + 508..512 + 119 * 512 + 512].copy_from_slice(&EOC.to_le_bytes());
    b[68..72].copy_from_slice(&120u32.to_le_bytes());
    let (res, _, _) = check_bytes(b.clone());
    assert!(
        matches!(res, Err(OfficeError::Corrupt(ref m)) if m.contains("DIFAT sector past")),
        "{res:?}"
    );
    b[68..72].copy_from_slice(&119u32.to_le_bytes());
    assert!(check_bytes(b).0.is_ok());
}

#[test]
fn a_part_reader_by_index_stops_exactly_at_its_cap() {
    use std::io::Read as _;
    let dir = tmp("part_reader_at");
    let p = dir.join("z.zip");
    {
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        zw.start_file("a/b.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zw.write_all(&[b'x'; 100]).unwrap();
        zw.finish().unwrap();
    }
    let mut z = container::open_zip(&p).unwrap();
    let idx = container::entry_index(&z)["a/b.txt"];
    for (cap, want) in [(40u64, 40usize), (99, 99), (100, 100), (101, 100), (0, 0)] {
        let mut out = Vec::new();
        container::part_reader_at(&mut z, idx, cap)
            .unwrap()
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out.len(), want, "cap {cap}");
    }
}

// ---------------------------------------------------------------------------------------------
// the format table of a sheet (fmt_xlsx.rs)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_column_past_u16_never_lands_on_another_cell_of_the_format_table() {
    let limits = Limits {
        max_cols: 1 << 20,
        ..Limits::default()
    };
    let mut sf = fmt_xlsx::SheetFormats::default();
    let mut last = None;
    // 70,000 would wrap to 4,464.
    sf.note_value(0, 70_000, 5, &limits).unwrap();
    sf.keep_last_format(&mut last, 0, 70_000, 5, &limits);
    sf.note_value(0, 70_000, 0, &limits).unwrap();
    sf.keep_last_format(&mut last, 0, 70_000, 0, &limits);
    assert_eq!(sf.format_at(0, 4_464), 0);
    assert!(sf.cells.is_empty(), "{:?}", sf.cells);
    assert_eq!(last, None);
    // The last column of `u16` still is a cell.
    sf.note_value(0, 65_535, 5, &limits).unwrap();
    sf.keep_last_format(&mut last, 0, 65_535, 5, &limits);
    assert_eq!(sf.format_at(0, 65_535), 5);
}

#[test]
fn the_last_position_is_remembered_for_every_record_not_only_the_first() {
    // (0,0) fmt 5; (5,0) fmt 6; (3,0) fmt 5; then a General record at (3,0): the greatest position
    // is (5,0), so the General one is at or before it and must be written down explicitly.
    let limits = Limits::default();
    let mut sf = fmt_xlsx::SheetFormats::default();
    let mut last = None;
    for (r, c, f) in [(0u32, 0u32, 5u16), (5, 0, 6), (3, 0, 5), (3, 0, 0)] {
        sf.note_value(r, c, f, &limits).unwrap();
        sf.keep_last_format(&mut last, r, c, f, &limits);
    }
    assert_eq!(last, Some((5, 0)));
    sf.cells.sort_by_key(|&(r, c, _)| (r, c));
    assert_eq!(sf.format_at(3, 0), 0, "the last record at a cell wins");
    assert_eq!(sf.format_at(5, 0), 6);
    assert_eq!(sf.format_at(0, 0), 5);
    // An equal position does not move `last` and a smaller one never lowers it.
    sf.keep_last_format(&mut last, 5, 0, 6, &limits);
    sf.keep_last_format(&mut last, 1, 0, 6, &limits);
    assert_eq!(last, Some((5, 0)));
}

// ---------------------------------------------------------------------------------------------
// shapes that single mutations of the model used to get away with
// ---------------------------------------------------------------------------------------------

#[test]
fn more_shapes_are_mirrored_one_by_one() {
    let mut s: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |label: &str, rgce: Vec<u8>| s.push((label.to_string(), rgce));
    // An unknown PtgAttr subtype with nothing after it: `calamine` gives up, nothing else would.
    add("attr unknown alone", [int(1), vec![0x19, 0x77]].concat());
    add(
        "attr space at the end",
        [int(1), vec![0x19, 0x40, 0x00]].concat(),
    );
    add(
        "attr space type 7 at the end",
        [int(1), vec![0x19, 0x40, 0x07, 0]].concat(),
    );
    // A string one byte short of what it says (compressed and 16-bit): an error for both.
    add("str compressed one short", vec![0x17, 3, 0, b'a', b'b']);
    add("str compressed exact", vec![0x17, 3, 0, b'a', b'b', b'c']);
    add("str wide one short", vec![0x17, 2, 1, b'a', 0, b'b']);
    add("str wide exact", vec![0x17, 2, 1, b'a', 0, b'b', 0]);
    // Every class of every 3d token (the sheet is named by the XTI table or by index).
    for (ptg, operand) in [
        (0x3Au8, 6usize),
        (0x5A, 6),
        (0x7A, 6),
        (0x3B, 10),
        (0x5B, 10),
        (0x7B, 10),
        (0x3C, 6),
        (0x5C, 6),
        (0x7C, 6),
        (0x3D, 10),
        (0x5D, 10),
        (0x7D, 10),
    ] {
        for ixti in [0u16, 1, 2] {
            add(&format!("3d {ptg:#04x} {ixti}"), err3d(ptg, ixti, operand));
            add(
                &format!("3d {ptg:#04x} {ixti} then int"),
                [err3d(ptg, ixti, operand), int(1), vec![0x03]].concat(),
            );
        }
    }
    // One workbook each (calamine panics on the one with a missing operand). The XTI table is not
    // the identity: XTI 0 -> sheet 2, 1 -> 0, 2 -> 3.
    let mut r = Run {
        cost: Vec::new(),
        real: Vec::new(),
    };
    for (label, rgce) in &s {
        let mut raw = Raw::standard(vec![rgce.clone()]);
        raw.xti = vec![2, 0, 3];
        let (cost, real) = run_guarded(&raw);
        match &real {
            Some(text) => assert_sound(label, &cost, text),
            None => assert!(
                cost.failed,
                "{label}: calamine panics, the cost must give up"
            ),
        }
        r.cost.push(cost);
        r.real.push(real.unwrap_or_else(|| DROPPED.to_string()));
    }
    let at = |label: &str| s.iter().position(|(l, _)| l == label).unwrap();
    for l in [
        "attr unknown alone",
        "str compressed one short",
        "str wide one short",
        "attr space at the end",
        "attr space type 7 at the end",
    ] {
        assert!(r.cost[at(l)].failed, "{l}");
        assert!(
            r.real[at(l)].starts_with(DROPPED),
            "{l}: {:?}",
            r.real[at(l)]
        );
    }
    for l in ["str compressed exact", "str wide exact"] {
        assert!(!r.cost[at(l)].failed, "{l}");
    }
    // Sheets: S(1), Two(3), シート(3), Ünï(3), each at 6 bytes a character. The XTI tokens
    // (Ref3d) go through XTI 0 -> sheet 2; the others index the sheets directly.
    let sheet = [6u64, 18, 18, 18];
    for (ptg, operand_text) in [(0x3Au8, 12u64), (0x5A, 12), (0x7A, 12)] {
        for ixti in [0usize, 1, 2] {
            let want = [18u64, 6, 18][ixti] + 1 + operand_text;
            assert_eq!(
                r.cost[at(&format!("3d {ptg:#04x} {ixti}"))].text,
                want,
                "{ptg:#04x}"
            );
        }
    }
    for (ptg, operand_text) in [
        (0x3Bu8, 24u64),
        (0x5B, 24),
        (0x7B, 24),
        (0x3C, 5),
        (0x5C, 5),
        (0x7C, 5),
        (0x3D, 5),
        (0x5D, 5),
        (0x7D, 5),
    ] {
        for ixti in [0usize, 1, 2] {
            let want = sheet[ixti] + 1 + operand_text;
            assert_eq!(
                r.cost[at(&format!("3d {ptg:#04x} {ixti}"))].text,
                want,
                "{ptg:#04x} {ixti}"
            );
        }
    }
}

#[test]
fn tokens_past_cce_are_not_read_or_charged() {
    // `calamine` reads `rgce[2..2 + cce]`: bytes after that belong to nothing. Here an int and
    // then bytes that would be an error as tokens.
    let env = formula::Env::default();
    let mut body = 3u16.to_le_bytes().to_vec();
    body.extend(int(7));
    body.extend([0xFF, 0xFF, 0xFF]);
    let c = formula::formula_cost(&body, &env);
    assert!(!c.failed);
    assert_eq!((c.text, c.peak, c.work), (5, 5, 5));
    // And through a record: calamine prints the same.
    let mut d = pos(0, 0, 0);
    d.extend(1.0f64.to_le_bytes());
    d.extend([0; 6]);
    d.extend_from_slice(&body);
    let book = Book {
        globals: globals(&[], &[]),
        sheets: vec![sheet("S", vec![rec(0x0006, &d)])],
    };
    let stream = book.stream();
    let container = fmt_xls::clean_container(&stream).unwrap();
    let mut wb = calamine::Xls::new(container).unwrap();
    let f: Vec<String> = wb
        .worksheet_formula("S")
        .unwrap()
        .cells()
        .map(|(_, _, s)| s.clone())
        .collect();
    assert_eq!(f, vec!["7".to_string()]);
}

#[test]
fn a_truncated_old_style_name_is_charged_for_every_byte_calamine_decodes() {
    // BIFF5: the characters start at byte 14 and `calamine` takes as many as are there (the
    // formula bytes that follow are part of what is left). cch says 200, 9 bytes are there.
    let mut d = vec![0, 0, 0, 200];
    d.extend(3u16.to_le_bytes()); // cce
    d.extend([0; 8]);
    d.extend(b"abcdef");
    d.extend([0x1E, 1, 0]);
    let mut raw = Raw::standard5(vec![name_tok(1)]);
    raw.names = vec![];
    let mut stream = raw.stream();
    let at = stream
        .windows(2)
        .position(|w| w == 0x0085u16.to_le_bytes())
        .unwrap();
    let extra = rec(0x0018, &d);
    stream.splice(at..at, extra.iter().copied());
    for i in at + extra.len()..stream.len().saturating_sub(1) {
        if stream[i..i + 2] == 0x0085u16.to_le_bytes() && i + 8 <= stream.len() {
            // A BOUNDSHEET: its position moves by the record that was added.
            let p = u32::from_le_bytes(stream[i + 4..i + 8].try_into().unwrap());
            if (p as usize) < stream.len() {
                stream[i + 4..i + 8].copy_from_slice(&(p + extra.len() as u32).to_le_bytes());
            }
        }
    }
    let costs = fmt_xls::formula_costs(&stream, &Limits::default()).unwrap();
    assert_eq!(costs.len(), 1);
    assert_eq!(costs[0].1.text, 9 * formula::CHAR_MAX);
}

#[test]
fn a_wide_sheet_name_cut_short_by_its_record_counts_the_characters_that_are_there() {
    // cch 10, 16-bit characters, only 4 of them in the record.
    let mut raw = Raw::standard(vec![ref3d(0x3A, 0, 0, 0)]);
    raw.sheets[0] = (true, 10, vec![b'a', 0, b'b', 0, b'c', 0, b'd', 0]);
    raw.xti = vec![0];
    let stream = raw.stream();
    let costs = fmt_xls::formula_costs(&stream, &Limits::default()).unwrap();
    assert_eq!(costs[0].1.text, 4 * formula::CHAR_MAX + 1 + 12);
}

#[test]
fn the_last_column_the_format_table_holds_is_the_limit_for_the_last_position_too() {
    let limits = Limits::default();
    let max = limits.max_cols as u32;
    let mut sf = fmt_xlsx::SheetFormats::default();
    let mut last = None;
    // A cell at the first column that is not shown: not a cell for the table, not a position.
    sf.note_value(0, max, 5, &limits).unwrap();
    sf.keep_last_format(&mut last, 0, max, 5, &limits);
    sf.keep_last_format(&mut last, 0, max, 0, &limits);
    assert!(sf.cells.is_empty(), "{:?}", sf.cells);
    assert_eq!(last, None);
    // The column before it is.
    sf.note_value(0, max - 1, 5, &limits).unwrap();
    sf.keep_last_format(&mut last, 0, max - 1, 5, &limits);
    assert_eq!(last, Some((0, max as u16 - 1)));
    assert_eq!(sf.format_at(0, max - 1), 5);
}

#[test]
fn the_zero_stripping_never_eats_the_header_slots_even_at_exactly_109_entries() {
    // The 109th header slot is sector 0 (a real FAT sector): with the header's count under 109
    // and nothing chained, the list stays 109 long (`cfb`: "don't remove sectors from the header
    // DIFAT").
    let mut ids: Vec<u32> = (10..119).collect();
    ids[108] = 0;
    let mut b = header_with(&ids, EOC);
    set_num_fat_sectors(&mut b, 3);
    let l = fat_list(b).unwrap();
    assert_eq!(l.len(), 109);
    assert_eq!(l[108], 0);
    // With one chained sector the zeros are the chain's padding and go; the header's last slot
    // (the 109th entry) is the one that is never removed.
    let mut b = difat_only_file(&ids, 1, |_| 0);
    set_num_fat_sectors(&mut b, 3);
    let l = fat_list(b).unwrap();
    assert_eq!(l.len(), 109);
    assert_eq!(l[108], 0);
}
