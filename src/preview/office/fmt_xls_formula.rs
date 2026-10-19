//! What `calamine` 0.37 will do with the formulas of an xls file, worked out before it does it.
//!
//! `calamine` turns **every** `FORMULA` record (and the first token of every defined name) into a
//! `String` while it opens the file, and it cannot be stopped or limited. Its `parse_formula` is
//! a small stack machine over the BIFF tokens (`rgce`) that builds the text in one `String`, and
//! two things in it are not bounded by the size of the record:
//!
//! - a token of 5 bytes (`PtgName`) or 7 bytes (`PtgRef3d`) can print a defined or sheet name of
//!   up to ~765 bytes, so a record of 8 KB prints about 1 MB and a file made of such records
//!   prints about 150 times its own size;
//! - operators copy the text built so far (`split_off` / `insert`), and `PtgAttrSpace` inserts up
//!   to 255 spaces *at the start of the operand* per 4-byte token, so one record is quadratic:
//!   about 10^11 byte moves for 2,000 tokens.
//!
//! [`formula_cost`] runs the same machine over the tokens with **lengths instead of text**: it
//! mirrors every token of `parse_formula` (what it prints, how far it reads, where it errors) and
//! charges the bytes that `calamine` would move. Names and sheet names are taken at their longest
//! possible UTF-8 size, so the answer is an upper bound for the length and exact in the shape of
//! the work. The caller rewrites a formula that is too costly to a single `PtgMissArg` (`cce = 1`,
//! token `0x16`, for which `calamine` prints nothing) in the stream it hands `calamine`: the cell
//! keeps its cached value, only the formula text of the cell detail is lost.
//!
//! The table of argument counts is `FTAB_ARGC` of `calamine` 0.37 (MIT, Copyright 2016-2026
//! Johann Tuffe), which is not public.

use super::fmt_xls::{u16_at, u32_at};

/// What a formula costs `calamine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cost {
    /// Length in bytes of the text that stays (an upper bound; an unparseable formula leaves a
    /// short message instead).
    pub text: u64,
    /// The longest the text gets while it is built (transient memory).
    pub peak: u64,
    /// Bytes copied, moved or written while building it.
    pub work: u64,
    /// `calamine` gives up on it (and leaves a short message instead of the text).
    pub failed: bool,
}

/// The lookups `parse_formula` makes, as upper bounds on the UTF-8 size of each answer.
#[derive(Debug, Default, Clone)]
pub(crate) struct Env {
    /// BIFF2-5 (any version but BIFF8): references are laid out differently.
    pub pre_biff8: bool,
    /// Per `BOUNDSHEET`, in record order: size of the sheet's name.
    pub sheets: Vec<u64>,
    /// Per `NAME`, in record order: size of the defined name.
    pub names: Vec<u64>,
    /// Per `XTI` of the `EXTERNSHEET` records: `itabFirst`.
    pub xti_first: Vec<i16>,
}

impl Env {
    /// The name of the sheet `ixti` points at, through the XTI table (what `PtgRef3d` and a
    /// defined name use).
    pub(crate) fn via_xti(&self, ixti: usize) -> u64 {
        self.xti_first
            .get(ixti)
            .and_then(|&first| usize::try_from(first).ok())
            .and_then(|first| self.sheets.get(first))
            .copied()
            .unwrap_or(NO_SHEET)
    }

    /// The name of the sheet with that index (the other 3d tokens index the sheets directly).
    fn direct(&self, ixti: u16) -> u64 {
        self.sheets
            .get(usize::from(ixti))
            .copied()
            .unwrap_or(NO_SHEET)
    }
}

/// What an unparseable formula leaves in the cell detail ("Unrecognised formula for cell ...";
/// the longest of `calamine`'s messages is 110 bytes).
pub(crate) const ERROR_TEXT: u64 = 128;
/// The longest function name of `FTAB`, with room to spare.
pub(crate) const FN_NAME: u64 = 24;
/// What `#REF` (a sheet that is not there) prints.
const NO_SHEET: u64 = 4;

/// What one character of a string costs, whatever the code page. `calamine` decodes a string
/// with the encoding of the workbook's `CODEPAGE` record: a compressed (8-bit) character is
/// widened to two bytes (`b, 0`) and a 16-bit one is two bytes, and `encoding_rs` turns each
/// input byte into at most one `U+FFFD` (3 bytes of UTF-8), e.g. any byte of an ISO-2022-JP
/// stream in the wrong state. So a character is at most 6 bytes of output, an ASCII one too.
/// A surrogate pair is two characters (12 bytes).
pub(crate) const CHAR_MAX: u64 = 6;

/// The size of a string of `units` characters read from `bytes` (8-bit, or 16-bit when `wide`):
/// the worst case of any code page, [`CHAR_MAX`] per unit that is there.
pub(crate) fn text_size(bytes: &[u8], units: usize, wide: bool) -> u64 {
    let n = if wide {
        (bytes.len() / 2).min(units)
    } else {
        bytes.len().min(units)
    } as u64;
    n * CHAR_MAX
}

/// The 3d sheet index a defined name's first token (`rgce`, the token bytes of a `NAME`) points
/// at, if `calamine`'s `parse_defined_names` reads one and then puts the sheet's name in front of
/// the name's formula: `PtgRef3d`, `PtgArea3d`, `PtgRefErr3d` and `PtgAreaErr3d`, in all three
/// token classes. The index is at byte 1 in BIFF8 and at byte 11 before it.
pub(crate) fn name_sheet_ixti(rgce: &[u8], biff8: bool) -> Option<u16> {
    match rgce.first()? {
        0x3a | 0x5a | 0x7a | 0x3b | 0x5b | 0x7b | 0x3c | 0x5c | 0x7c | 0x3d | 0x5d | 0x7d => {
            u16_at(rgce, if biff8 { 1 } else { 11 })
        }
        _ => None,
    }
}

/// `FTAB_ARGC` of `calamine` 0.37: the number of arguments of each fixed-arity function.
const FTAB_ARGC: [u8; 485] = [
    255, 3, 1, 1, 255, 255, 255, 255, 1, 1, 0, 254, 255, 2, 3, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1,
    2, 3, 4, 2, 3, 1, 1, 0, 0, 255, 255, 1, 2, 3, 3, 3, 3, 3, 3, 255, 3, 2, 4, 4, 4, 4, 1, 1, 1, 5,
    5, 5, 5, 6, 3, 2, 0, 3, 3, 3, 1, 1, 1, 2, 1, 1, 1, 0, 1, 1, 1, 5, 2, 2, 3, 3, 1, 2, 0, 1, 1, 2,
    0, 1, 2, 2, 2, 0, 0, 1, 2, 1, 1, 255, 4, 4, 2, 7, 1, 1, 2, 2, 2, 4, 1, 1, 1, 1, 2, 2, 2, 1, 4,
    4, 1, 3, 1, 3, 2, 1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 2, 2, 2, 2, 1, 1, 3, 4, 5, 3, 2, 2, 2, 255,
    255, 1, 4, 5, 5, 5, 5, 1, 3, 4, 3, 1, 1, 1, 1, 1, 2, 6, 6, 255, 2, 4, 1, 0, 0, 2, 2, 3, 2, 1,
    1, 1, 4, 255, 1, 2, 1, 2, 2, 3, 1, 3, 4, 255, 255, 3, 3, 2, 1, 3, 1, 1, 0, 0, 2, 3, 3, 4, 2, 2,
    3, 3, 2, 2, 1, 1, 3, 0, 0, 5, 3, 0, 7, 0, 1, 0, 3, 255, 255, 1, 1, 1, 1, 1, 1, 3, 11, 1, 0, 2,
    3, 5, 4, 4, 1, 0, 5, 5, 1, 0, 0, 1, 2, 2, 1, 255, 1, 1, 2, 3, 3, 1, 1, 1, 2, 3, 3, 3, 2, 255,
    5, 1, 5, 4, 2, 2, 2, 3, 3, 1, 3, 3, 3, 1, 1, 2, 4, 3, 2, 4, 3, 3, 3, 4, 1, 3, 1, 3, 1, 2, 3, 3,
    4, 2, 2, 2, 2, 2, 2, 3, 2, 2, 2, 2, 2, 2, 4, 4, 255, 255, 255, 255, 255, 255, 3, 2, 2, 2, 2, 3,
    255, 2, 2, 4, 4, 3, 255, 2, 9, 2, 3, 4, 1, 1, 255, 3, 2, 1, 2, 1, 4, 3, 1, 2, 2, 4, 5, 2, 128,
    2, 1, 255, 255, 255, 255, 255, 255, 255, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 255, 255, 3, 3, 4, 2,
    1, 2, 2, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 255, 255, 4,
    1, 1, 2, 2, 2, 1, 1, 2, 2, 1, 2, 2, 2, 2, 3, 3, 6, 6, 5, 5, 5, 5, 5, 3, 3, 3, 7, 7, 2, 2, 2, 2,
    6, 6, 2, 2, 3, 4, 4, 4, 4, 4, 4, 6, 6, 8, 8, 8, 8, 2, 2, 7, 7, 8, 8, 5, 3, 3, 255, 255, 255, 2,
    4, 5, 1, 2, 128, 129, 3, 129,
];

/// The cost of one `FORMULA` record's `CellParsedFormula`: `data` is the record body from byte 20
/// (`cce`, then the tokens).
pub(crate) fn formula_cost(data: &[u8], env: &Env) -> Cost {
    let mut len: u64 = 0;
    let mut peak: u64 = 0;
    let mut work: u64 = 0;
    // What `calamine` makes of a formula it cannot parse is a short message (or a panic it does
    // not survive): the work done so far still counts.
    macro_rules! fail {
        () => {
            return Cost {
                text: ERROR_TEXT,
                peak,
                work,
                failed: true,
            }
        };
    }
    let Some(cce) = u16_at(data, 0).map(usize::from) else {
        fail!()
    };
    let Some(mut rgce) = data.get(2..2 + cce) else {
        fail!()
    };
    let mut stack: Vec<u64> = Vec::new();
    // Skips `n` bytes of operand; `calamine` indexes them, so a short token is a failure.
    macro_rules! adv {
        ($n:expr) => {{
            let n: usize = $n;
            if rgce.len() < n {
                fail!()
            }
            rgce = &rgce[n..];
        }};
    }
    // Prints `n` bytes at the end.
    macro_rules! push_text {
        ($n:expr) => {{
            let n: u64 = $n;
            len = len.saturating_add(n);
            work = work.saturating_add(n);
        }};
    }
    while !rgce.is_empty() {
        let ptg = rgce[0];
        rgce = &rgce[1..];
        match ptg {
            // PtgRef3d: sheet, '!', then at most `$AAA$65536`.
            0x3a | 0x5a | 0x7a => {
                let Some(ixti) = u16_at(rgce, 0) else { fail!() };
                stack.push(len);
                push_text!(env.via_xti(usize::from(ixti)) + 1 + 12);
                adv!(6);
            }
            // PtgArea3d: sheet, '!', `$AAA$65536:$AAA$65536`.
            0x3b | 0x5b | 0x7b => {
                let Some(ixti) = u16_at(rgce, 0) else { fail!() };
                stack.push(len);
                push_text!(env.direct(ixti) + 1 + 24);
                adv!(10);
            }
            // PtgRefErr3d (6 operand bytes), PtgAreaErr3d (10)
            0x3c | 0x5c | 0x7c | 0x3d | 0x5d | 0x7d => {
                let Some(ixti) = u16_at(rgce, 0) else { fail!() };
                stack.push(len);
                push_text!(env.direct(ixti) + 1 + 5);
                adv!(if ptg & 0x1F == 0x1C { 6 } else { 10 });
            }
            // PtgExp: nothing printed, four operand bytes.
            0x01 => {
                stack.push(len);
                adv!(4);
            }
            // Binary operators: the right operand is cut off and written back after the operator.
            0x03..=0x11 => {
                let Some(e2) = stack.pop() else { fail!() };
                let tail = len.saturating_sub(e2);
                // `split_off` copies the tail, then the operator (2 bytes at most) and the tail
                // are written.
                work = work
                    .saturating_add(tail.saturating_mul(2))
                    .saturating_add(2);
                len = len.saturating_add(2);
            }
            // Unary plus / minus: one character inserted before the operand.
            0x12 | 0x13 => {
                let Some(&e) = stack.last() else { fail!() };
                // The tail moves up by one, and one byte is written.
                work = work.saturating_add(len.saturating_sub(e)).saturating_add(1);
                len = len.saturating_add(1);
            }
            0x14 => push_text!(1),
            // Parentheses.
            0x15 => {
                let Some(&e) = stack.last() else { fail!() };
                work = work.saturating_add(len.saturating_sub(e));
                push_text!(2);
            }
            0x16 => stack.push(len),
            // PtgStr: quotes and the characters (at most CHAR_MAX bytes each).
            0x17 => {
                stack.push(len);
                let Some(&cch) = rgce.first() else { fail!() };
                let cch = usize::from(cch);
                let Some(&flags) = rgce.get(1) else { fail!() };
                let consumed = 1 + if flags & 1 != 0 { 2 * cch } else { cch };
                push_text!(2 + CHAR_MAX * cch as u64);
                // (`cch`, then the flags and the characters: a string cut short is a failure.)
                adv!(1 + consumed);
            }
            0x18 => adv!(5),
            // PtgAttr
            0x19 => {
                let Some(&etpg) = rgce.first() else { fail!() };
                rgce = &rgce[1..];
                match etpg {
                    0x01 | 0x02 | 0x08 | 0x20 | 0x21 => adv!(2),
                    // PtgAttrChoose
                    0x04 => {
                        let Some(n) = u16_at(rgce, 0) else { fail!() };
                        adv!(2 + 2 * (usize::from(n) + 1));
                    }
                    // PtgAttrSum: `SUM(` operand `)`, the operand cut off and written back.
                    0x10 => {
                        adv!(2);
                        let Some(&e) = stack.last() else { fail!() };
                        let tail = len.saturating_sub(e);
                        // `SUM(` and `)` are written around the tail that was cut off.
                        work = work
                            .saturating_add(tail.saturating_mul(2))
                            .saturating_add(5);
                        len = len.saturating_add(5);
                    }
                    // PtgAttrSpace: `cch` characters inserted one by one at the operand's start.
                    0x40 | 0x41 => {
                        let Some(&e) = stack.last() else { fail!() };
                        if !matches!(rgce.first(), Some(0x00..=0x06)) {
                            fail!()
                        }
                        let Some(&cch) = rgce.get(1) else { fail!() };
                        let cch = u64::from(cch);
                        let tail = len.saturating_sub(e);
                        // Insert k moves `tail + k` bytes and writes one:
                        // tail * cch + cch * (cch - 1) / 2 + cch.
                        work = work
                            .saturating_add(tail.saturating_mul(cch))
                            .saturating_add(cch * cch.saturating_sub(1) / 2)
                            .saturating_add(cch);
                        len = len.saturating_add(cch);
                        adv!(2);
                    }
                    _ => fail!(),
                }
            }
            // PtgErr
            0x1C => {
                stack.push(len);
                let Some(&err) = rgce.first() else { fail!() };
                let n = match err {
                    0x00 | 0x1D => 6,
                    0x07 | 0x0F => 7,
                    0x17 | 0x24 => 5,
                    0x2A => 4,
                    0x2B => 13,
                    _ => fail!(),
                };
                push_text!(n);
                adv!(1);
            }
            // PtgBool
            0x1D => {
                stack.push(len);
                push_text!(5);
                adv!(1);
            }
            // PtgInt
            0x1E => {
                stack.push(len);
                push_text!(5);
                adv!(2);
            }
            // PtgNum: `{}` of an f64 prints every digit (no exponent), up to ~330 bytes.
            0x1F => {
                stack.push(len);
                let Some(b) = rgce.get(..8) else { fail!() };
                let v = f64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
                let n = if v == 0.0 || (v.abs() >= 1e-5 && v.abs() < 1e16) {
                    26
                } else {
                    format!("{v}").len() as u64
                };
                push_text!(n);
                adv!(8);
            }
            // PtgArray
            0x20 | 0x40 | 0x60 => {
                stack.push(len);
                push_text!(10);
                adv!(7);
            }
            // PtgFunc, PtgFuncVar
            0x21 | 0x22 | 0x41 | 0x42 | 0x61 | 0x62 => {
                let (iftab, argc) = if matches!(ptg, 0x22 | 0x42 | 0x62) {
                    let Some(&argc) = rgce.first() else { fail!() };
                    let Some(iftab) = u16_at(rgce, 1).map(usize::from) else {
                        fail!()
                    };
                    adv!(3);
                    (iftab, usize::from(argc))
                } else {
                    let Some(iftab) = u16_at(rgce, 0).map(usize::from) else {
                        fail!()
                    };
                    let Some(&argc) = FTAB_ARGC.get(iftab) else {
                        fail!()
                    };
                    adv!(2);
                    (iftab, usize::from(argc))
                };
                // `calamine` looks the name up (and fails) for both kinds, with or without arguments.
                if iftab >= FTAB_ARGC.len() {
                    fail!()
                }
                if stack.len() < argc {
                    fail!()
                }
                if argc > 0 {
                    let args_start = stack.len() - argc;
                    let start = stack[args_start];
                    let fargs = len.saturating_sub(start);
                    // `split_off` copies the arguments, then the name, `(`, the arguments again,
                    // a comma after each and `)` are written.
                    work = work
                        .saturating_add(fargs.saturating_mul(2))
                        .saturating_add(FN_NAME + 1 + argc as u64);
                    stack.truncate(args_start);
                    stack.push(start);
                    len = start
                        .saturating_add(FN_NAME + 1)
                        .saturating_add(fargs)
                        .saturating_add(argc as u64);
                } else {
                    stack.push(len);
                    push_text!(FN_NAME + 2);
                }
            }
            // PtgName: the defined name. `iname - 1` wraps for 0, and then there is no such name.
            0x23 | 0x43 | 0x63 => {
                let Some(iname) = u32_at(rgce, 0) else {
                    fail!()
                };
                stack.push(len);
                let n = env
                    .names
                    .get((iname as usize).wrapping_sub(1))
                    .copied()
                    .unwrap_or(5);
                push_text!(n);
                adv!(4);
            }
            // PtgRef
            0x24 | 0x44 | 0x64 => {
                stack.push(len);
                push_text!(12);
                adv!(if env.pre_biff8 { 3 } else { 4 });
            }
            // PtgArea
            0x25 | 0x45 | 0x65 => {
                stack.push(len);
                push_text!(40);
                adv!(if env.pre_biff8 { 6 } else { 8 });
            }
            // PtgRefErr, PtgAreaErr
            0x2A | 0x4A | 0x6A => {
                stack.push(len);
                push_text!(5);
                adv!(if env.pre_biff8 { 3 } else { 4 });
            }
            0x2B | 0x4B | 0x6B => {
                stack.push(len);
                push_text!(5);
                adv!(if env.pre_biff8 { 6 } else { 8 });
            }
            // PtgNameX
            0x39 | 0x59 => {
                stack.push(len);
                push_text!(10);
                adv!(6);
            }
            _ => fail!(),
        }
        peak = peak.max(len);
    }
    if stack.len() == 1 {
        Cost {
            text: len,
            peak,
            work,
            failed: false,
        }
    } else {
        Cost {
            text: ERROR_TEXT,
            peak,
            work,
            failed: true,
        }
    }
}
