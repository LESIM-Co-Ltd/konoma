//! Excel number-format engine: format code + cell value -> display string.
//!
//! A pure function module (std only). Spreadsheet readers drop the per-cell display format,
//! so a separate component supplies the format code string and this engine renders what
//! Excel would show. Spec sources: ECMA-376 Part 1 18.8.30 (built-in `numFmtId` table,
//! the "General" notes) and Microsoft's "Number format codes" support article.
//!
//! # Rules implemented (and the judgement calls)
//!
//! * Sections `pos;neg;zero;text`. 1 section: used for everything, a negative gets a leading
//!   `-`. 2 sections: the second is used for negatives (no sign added). 3+: positive,
//!   negative (no sign), zero. A 4th section (or a last section containing `@` when there
//!   are fewer than 4) is the text section. Conditions (`[>=100]`) override this: the first
//!   matching of section 1 / section 2 wins; with conditions on both, section 3 is the
//!   "else", otherwise section 2 is. With conditions the minus sign is kept (unverified).
//! * Rounding: Excel keeps 15 significant digits, so the value is first rounded to 15
//!   significant digits (`{:.14e}`), then rounded half away from zero on that *decimal*
//!   string. `1.005` -> `0.00` therefore gives `1.01` and `2.675` gives `2.68`, as in Excel,
//!   instead of the binary-float answer. Percent and trailing-comma scaling shift the
//!   decimal point instead of multiplying, so they add no float error.
//! * General: Excel limits "General" to 11 characters (ECMA-376: "up to 11 digits (inc.
//!   decimal point)"). Integers up to 11 digits print as is; fractions are rounded so digits
//!   plus the point fit in 11 characters (`0.123456789012` -> `0.123456789`). A value that
//!   needs 12+ integer digits, or whose exponent is below -4 (`0.00001`), switches to
//!   scientific with at most 5 mantissa decimals and a 2-digit exponent, which is what Excel
//!   shows at the default column width (`123456789012` -> `1.23457E+11`). Excel's choice
//!   really depends on the column width; this engine assumes the default.
//! * `m`/`mm` is minutes when the previous date/time token is `h`/`[h]` or the next one is
//!   `s`; otherwise the month. Time is rounded to the displayed sub-second precision (whole
//!   seconds when there is no `ss.0`), carrying into the date; minutes are truncated.
//! * 1900 system: serial 60 is the fictitious 1900-02-29, serial 0 is "1900-01-00" (Saturday).
//!   Weekday = (serial - 1) mod 7 with 1 = Sunday, which reproduces Excel's calendar bug.
//! * Language of month/day names: an explicit `[$-xxx]` wins (`411` = Japanese, any English
//!   id = English); otherwise [`Options::locale`]. `aaa`/`aaaa` are always Japanese weekdays.
//!   Japanese `mmm`/`mmmm` render as `10月`, `mmmmm` as `10` (unverified).
//! * Era tokens `g`/`gg`/`ggg`/`e`/`ee` use the Japanese eras (Meiji from 1868-09-08, as
//!   Excel does). Before Meiji `g*` is empty and `e` is the Gregorian year. `[$-404]e` is the
//!   Republic of China year.
//! * Not implemented on purpose: `[DBNum1]` digit conversion (digits stay ASCII), `*x` fill
//!   (a terminal has no cell width), colours, Thai/Buddhist numerals (only `b`/`bb` year +543).

/// A cell value to format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    Number(f64),
    Text(&'a str),
    Bool(bool),
    /// Error literal such as `#DIV/0!`, shown unchanged.
    Error(&'a str),
}

/// Display language: picks locale-dependent built-in formats and month/day names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    #[default]
    En,
    Ja,
}

/// Formatting options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// Workbook uses the 1904 date system.
    pub date1904: bool,
    pub locale: Locale,
    /// OpenDocument calendar: the day number 0 is this many days after 1970-01-01 (the
    /// spreadsheet's `table:null-date`, 1899-12-30 by default). Unlike Excel's, this calendar has
    /// no 1900 leap-year bug and runs below 0. `None` = Excel's calendar (`date1904` applies).
    pub null_day: Option<i64>,
}

/// Excel's own limit on the length of a format code. A longer code is not valid in Excel and is
/// shown as General here, so a crafted code cannot make every cell carry a huge string.
pub const MAX_CODE_CHARS: usize = 255;
/// Excel's own limit on decimal places. Placeholders after the decimal point beyond it are
/// dropped when a code is compiled.
pub const MAX_DECIMALS: usize = 30;
/// The longest text a formatted cell shows, in characters. A format applied to a cell never
/// produces much more than this (see `render_text`); the loader cuts the rest.
pub const MAX_OUTPUT_CHARS: usize = 1024;

/// A format code parsed once. Parsing a code is the expensive part of formatting a cell, and a
/// workbook has a handful of distinct codes for millions of cells, so a caller formats many cells
/// with one `Compiled` (see [`compile`]).
#[derive(Debug, Clone)]
pub struct Compiled {
    /// `None`: an empty, over-long or broken code, shown as General.
    secs: Option<Vec<Section>>,
}

/// Parses `code` once. Never panics: a broken, empty or over-long (more than
/// [`MAX_CODE_CHARS`] characters) code compiles to General.
pub fn compile(code: &str) -> Compiled {
    let secs = if code.is_empty() || code.chars().count() > MAX_CODE_CHARS {
        None
    } else {
        parse_code(code).map(|mut secs| {
            cap_decimals(&mut secs);
            secs
        })
    };
    Compiled { secs }
}

/// Drops the decimal placeholders past [`MAX_DECIMALS`] (the exponent / fraction part of a code
/// is left alone).
fn cap_decimals(secs: &mut [Section]) {
    for sec in secs.iter_mut().filter(|s| !s.date) {
        let Some(point) = sec.toks.iter().position(|t| matches!(t, Tok::Point)) else {
            continue;
        };
        let mut kept = 0;
        let mut k = point + 1;
        while k < sec.toks.len() {
            match sec.toks[k] {
                Tok::Digit(_) if kept >= MAX_DECIMALS => {
                    sec.toks.remove(k);
                    continue;
                }
                Tok::Digit(_) => kept += 1,
                Tok::Exp { .. } | Tok::Slash => break,
                _ => {}
            }
            k += 1;
        }
    }
}

/// Formats `value` with the Excel format `code`. Never panics: a broken or unsupported code
/// falls back to General. (For many cells with the same code use [`compile`]; this one-shot form
/// is what the tests use.)
#[cfg(test)]
pub fn format_value(code: &str, value: Value<'_>, opts: &Options) -> String {
    format_compiled(&compile(code), value, opts)
}

/// Formats `value` with an already compiled code; same result as [`format_value`].
pub fn format_compiled(code: &Compiled, value: Value<'_>, opts: &Options) -> String {
    match value {
        Value::Bool(b) => return if b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Error(e) => return e.to_string(),
        _ => {}
    }
    let secs = code.secs.as_deref();
    let Some(secs) = secs else {
        return match value {
            Value::Number(n) => general_signed(n),
            Value::Text(t) => t.to_string(),
            _ => String::new(),
        };
    };
    match value {
        Value::Text(t) => match text_section_idx(secs) {
            Some(i) => render_text(&secs[i], t),
            None => t.to_string(),
        },
        Value::Number(x) => {
            if !x.is_finite() {
                return "#NUM!".to_string();
            }
            let cnt = numeric_section_count(secs);
            if cnt == 0 {
                // Only a text section (e.g. `@` or `@" kg"`): show the number as General.
                return render_with_general(&secs[0], x);
            }
            let (idx, use_abs) = select_section(secs, cnt, x);
            render_number_section(&secs[idx], x, use_abs, opts)
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy)]
struct Cond {
    op: CmpOp,
    val: f64,
}

impl Cond {
    fn test(&self, x: f64) -> bool {
        match self.op {
            CmpOp::Lt => x < self.val,
            CmpOp::Le => x <= self.val,
            CmpOp::Gt => x > self.val,
            CmpOp::Ge => x >= self.val,
            CmpOp::Eq => x == self.val,
            CmpOp::Ne => x != self.val,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    /// Unquoted digit 1-9 (a fixed fraction denominator candidate); otherwise a literal.
    Num(char),
    /// `0`, `#` or `?`.
    Digit(char),
    Point,
    Comma,
    /// A comma that groups thousands (resolved from `Comma`).
    Group,
    /// A comma that divides by 1000 (resolved from `Comma`).
    Scale,
    Percent,
    Exp {
        plus: bool,
        upper: bool,
    },
    Slash,
    FixedDen(u32),
    At,
    General,
    Year(u8),
    Month(u8),
    Day(u8),
    Hour(u8),
    Minute(u8),
    Second(u8),
    AmPm {
        short: bool,
        lower: bool,
    },
    FracSec(u8),
    Elapsed(char, u8),
    EraG(u8),
    EraE(u8),
    JaWeekday(u8),
    BuddhistYear(u8),
}

#[derive(Debug, Clone)]
struct Section {
    toks: Vec<Tok>,
    cond: Option<Cond>,
    lang: Option<u16>,
    date: bool,
}

impl Section {
    fn has_at(&self) -> bool {
        self.toks.iter().any(|t| matches!(t, Tok::At))
    }
}

/// Splits on top-level `;` (not inside quotes, brackets, or after `\`, `_`, `*`).
fn split_sections(code: &str) -> Vec<String> {
    let ch: Vec<char> = code.chars().collect();
    let n = ch.len();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < n {
        let c = ch[i];
        match c {
            '"' => {
                cur.push(c);
                i += 1;
                while i < n {
                    cur.push(ch[i]);
                    i += 1;
                    if ch[i - 1] == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                cur.push(c);
                if i + 1 < n {
                    cur.push(ch[i + 1]);
                }
                i += 2;
            }
            '[' => {
                while i < n {
                    cur.push(ch[i]);
                    i += 1;
                    if ch[i - 1] == ']' {
                        break;
                    }
                }
            }
            ';' => {
                out.push(std::mem::take(&mut cur));
                i += 1;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    out.push(cur);
    out
}

fn parse_code(code: &str) -> Option<Vec<Section>> {
    let raw = split_sections(code);
    if raw.len() > 4 {
        return None;
    }
    raw.iter().map(|s| parse_section(s)).collect()
}

fn starts_with_ci(ch: &[char], i: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    i + p.len() <= ch.len()
        && ch[i..i + p.len()]
            .iter()
            .zip(&p)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

fn run_len(ch: &[char], i: usize) -> usize {
    let c = ch[i].to_ascii_lowercase();
    let mut k = 0;
    while i + k < ch.len() && ch[i + k].to_ascii_lowercase() == c {
        k += 1;
    }
    k
}

fn parse_cond(s: &str) -> Option<Cond> {
    let (op, rest) = if let Some(r) = s.strip_prefix("<=") {
        (CmpOp::Le, r)
    } else if let Some(r) = s.strip_prefix(">=") {
        (CmpOp::Ge, r)
    } else if let Some(r) = s.strip_prefix("<>") {
        (CmpOp::Ne, r)
    } else if let Some(r) = s.strip_prefix('<') {
        (CmpOp::Lt, r)
    } else if let Some(r) = s.strip_prefix('>') {
        (CmpOp::Gt, r)
    } else if let Some(r) = s.strip_prefix('=') {
        (CmpOp::Eq, r)
    } else {
        None?
    };
    let val: f64 = rest.trim().parse().ok()?;
    val.is_finite().then_some(Cond { op, val })
}

const COLOR_NAMES: [&str; 8] = [
    "black", "blue", "cyan", "green", "magenta", "red", "white", "yellow",
];

/// Handles the inside of `[...]`.
fn parse_bracket(
    content: &str,
    cond: &mut Option<Cond>,
    lang: &mut Option<u16>,
    toks: &mut Vec<Tok>,
) {
    if let Some(rest) = content.strip_prefix('$') {
        let (sym, loc) = match rest.split_once('-') {
            Some((s, l)) => (s, l),
            None => (rest, ""),
        };
        if !sym.is_empty() {
            toks.push(Tok::Lit(sym.to_string()));
        }
        let loc = loc.trim();
        if !loc.is_empty() {
            if loc.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(v) = u32::from_str_radix(loc, 16) {
                    *lang = Some((v & 0xFFFF) as u16);
                }
            } else if loc.to_ascii_lowercase().starts_with("ja") {
                *lang = Some(0x411);
            } else if loc.to_ascii_lowercase().starts_with("en") {
                *lang = Some(0x409);
            }
        }
        return;
    }
    let lower = content.trim().to_ascii_lowercase();
    if lower.starts_with(['<', '>', '=']) {
        if cond.is_none() {
            *cond = parse_cond(&lower);
        }
        return;
    }
    if COLOR_NAMES.contains(&lower.as_str()) || lower.starts_with("color") {
        return;
    }
    let mut it = lower.chars();
    if let Some(first) = it.next() {
        if matches!(first, 'h' | 'm' | 's') && lower.chars().all(|c| c == first) {
            toks.push(Tok::Elapsed(first, lower.len().min(2) as u8));
        }
    }
    // [DBNum1], [NatNum..] and anything unknown are skipped.
}

fn parse_section(s: &str) -> Option<Section> {
    let ch: Vec<char> = s.chars().collect();
    let n = ch.len();
    let mut toks: Vec<Tok> = Vec::new();
    let mut cond = None;
    let mut lang = None;
    let mut i = 0;
    while i < n {
        let c = ch[i];
        match c {
            '"' => {
                i += 1;
                let mut t = String::new();
                while i < n && ch[i] != '"' {
                    t.push(ch[i]);
                    i += 1;
                }
                i += 1;
                toks.push(Tok::Lit(t));
            }
            '\\' => {
                if i + 1 < n {
                    toks.push(Tok::Lit(ch[i + 1].to_string()));
                }
                i += 2;
            }
            '_' => {
                if i + 1 < n {
                    toks.push(Tok::Lit(" ".to_string()));
                }
                i += 2;
            }
            '*' => i += 2,
            '[' => {
                let close = ch[i + 1..].iter().position(|&c| c == ']')?;
                let content: String = ch[i + 1..i + 1 + close].iter().collect();
                parse_bracket(&content, &mut cond, &mut lang, &mut toks);
                i += close + 2;
            }
            '0' | '#' | '?' => {
                toks.push(Tok::Digit(c));
                i += 1;
            }
            '1'..='9' => {
                toks.push(Tok::Num(c));
                i += 1;
            }
            '.' => {
                toks.push(Tok::Point);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            '%' => {
                toks.push(Tok::Percent);
                i += 1;
            }
            '/' => {
                toks.push(Tok::Slash);
                i += 1;
            }
            '@' => {
                toks.push(Tok::At);
                i += 1;
            }
            'E' | 'e' if matches!(ch.get(i + 1), Some('+') | Some('-')) => {
                toks.push(Tok::Exp {
                    plus: ch[i + 1] == '+',
                    upper: c == 'E',
                });
                i += 2;
            }
            'G' | 'g' if starts_with_ci(&ch, i, "general") => {
                toks.push(Tok::General);
                i += 7;
            }
            'y' | 'Y' => {
                let k = run_len(&ch, i);
                toks.push(Tok::Year(if k <= 2 { 2 } else { 4 }));
                i += k;
            }
            'm' | 'M' => {
                let k = run_len(&ch, i);
                toks.push(Tok::Month(k.min(5) as u8));
                i += k;
            }
            'd' | 'D' => {
                let k = run_len(&ch, i);
                toks.push(Tok::Day(k.min(4) as u8));
                i += k;
            }
            'h' | 'H' => {
                let k = run_len(&ch, i);
                toks.push(Tok::Hour(k.min(2) as u8));
                i += k;
            }
            's' | 'S' => {
                let k = run_len(&ch, i);
                toks.push(Tok::Second(k.min(2) as u8));
                i += k;
            }
            'g' | 'G' => {
                let k = run_len(&ch, i);
                toks.push(Tok::EraG(k.min(3) as u8));
                i += k;
            }
            'e' | 'E' => {
                let k = run_len(&ch, i);
                toks.push(Tok::EraE(k.min(2) as u8));
                i += k;
            }
            'b' | 'B' => {
                let k = run_len(&ch, i);
                toks.push(Tok::BuddhistYear(if k <= 2 { 2 } else { 4 }));
                i += k;
            }
            'a' | 'A' => {
                if starts_with_ci(&ch, i, "am/pm") {
                    toks.push(Tok::AmPm {
                        short: false,
                        lower: c == 'a' && ch[i + 1] == 'm',
                    });
                    i += 5;
                } else if starts_with_ci(&ch, i, "a/p") {
                    toks.push(Tok::AmPm {
                        short: true,
                        lower: c == 'a',
                    });
                    i += 3;
                } else {
                    let k = run_len(&ch, i);
                    if k >= 3 {
                        toks.push(Tok::JaWeekday(if k >= 4 { 4 } else { 3 }));
                    } else {
                        toks.push(Tok::Lit(ch[i..i + k].iter().collect()));
                    }
                    i += k;
                }
            }
            _ => {
                toks.push(Tok::Lit(c.to_string()));
                i += 1;
            }
        }
    }
    Some(finish_section(toks, cond, lang))
}

fn is_date_tok(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Year(_)
            | Tok::Month(_)
            | Tok::Day(_)
            | Tok::Hour(_)
            | Tok::Minute(_)
            | Tok::Second(_)
            | Tok::AmPm { .. }
            | Tok::EraG(_)
            | Tok::EraE(_)
            | Tok::JaWeekday(_)
            | Tok::BuddhistYear(_)
            | Tok::Elapsed(..)
    )
}

fn finish_section(mut toks: Vec<Tok>, cond: Option<Cond>, lang: Option<u16>) -> Section {
    let date = toks.iter().any(is_date_tok);
    if date {
        finish_date(&mut toks);
    } else {
        finish_numeric(&mut toks);
    }
    for t in toks.iter_mut() {
        if let Tok::Num(c) = t {
            *t = Tok::Lit(c.to_string());
        }
    }
    Section {
        toks,
        cond,
        lang,
        date,
    }
}

fn finish_date(toks: &mut Vec<Tok>) {
    // `e-` / `e+` is an era year followed by a separator in a date code, not an exponent.
    let mut k = 0;
    while k < toks.len() {
        if let Tok::Exp { plus, .. } = toks[k] {
            toks[k] = Tok::EraE(1);
            toks.insert(k + 1, Tok::Lit(if plus { "+" } else { "-" }.to_string()));
        }
        k += 1;
    }
    // Month vs minute.
    let n = toks.len();
    for k in 0..n {
        if let Tok::Month(w) = toks[k] {
            if w > 2 {
                continue;
            }
            let prev = toks[..k].iter().rev().find(|t| is_date_tok(t));
            let next = toks[k + 1..].iter().find(|t| is_date_tok(t));
            let after_hour = matches!(prev, Some(Tok::Hour(_)) | Some(Tok::Elapsed('h', _)));
            let before_sec = matches!(next, Some(Tok::Second(_)) | Some(Tok::Elapsed('s', _)));
            if after_hour || before_sec {
                toks[k] = Tok::Minute(w);
            }
        }
    }
    // Fractional seconds: `ss.0`, `ss.00`, `[ss].00`.
    let mut out: Vec<Tok> = Vec::with_capacity(toks.len());
    let mut k = 0;
    while k < toks.len() {
        if matches!(toks[k], Tok::Point)
            && matches!(
                out.last(),
                Some(Tok::Second(_)) | Some(Tok::Elapsed('s', _))
            )
        {
            let mut z = 0;
            while matches!(toks.get(k + 1 + z), Some(Tok::Digit('0'))) {
                z += 1;
            }
            if z > 0 {
                out.push(Tok::FracSec(z.min(255) as u8));
                k += 1 + z;
                continue;
            }
        }
        out.push(toks[k].clone());
        k += 1;
    }
    // Everything else numeric-looking is a literal in a date code.
    for t in out.iter_mut() {
        match t {
            Tok::Point => *t = Tok::Lit(".".to_string()),
            Tok::Comma => *t = Tok::Lit(",".to_string()),
            Tok::Percent => *t = Tok::Lit("%".to_string()),
            Tok::Slash => *t = Tok::Lit("/".to_string()),
            Tok::Digit(c) if *c == '0' => *t = Tok::Lit("0".to_string()),
            Tok::Digit(_) | Tok::Group | Tok::Scale | Tok::General => *t = Tok::Lit(String::new()),
            _ => {}
        }
    }
    *toks = out;
}

fn finish_numeric(toks: &mut Vec<Tok>) {
    // Comma: thousands separator when more digit placeholders follow, scale otherwise.
    for k in 0..toks.len() {
        if !matches!(toks[k], Tok::Comma) {
            continue;
        }
        let digit_before = toks[..k].iter().any(|t| matches!(t, Tok::Digit(_)));
        let next = toks[k + 1..].iter().find(|t| !matches!(t, Tok::Comma));
        toks[k] = if !digit_before {
            Tok::Lit(",".to_string())
        } else if matches!(next, Some(Tok::Digit(_))) {
            Tok::Group
        } else {
            Tok::Scale
        };
    }
    // Fractions: `digits / digits` or `digits / fixed-denominator`; any other `/` is literal.
    let mut k = 0;
    while k < toks.len() {
        if matches!(toks[k], Tok::Slash) {
            let digit_before = k > 0 && matches!(toks[k - 1], Tok::Digit(_));
            // Fixed denominator: a run of unquoted digits (first one non-zero).
            let mut j = k + 1;
            let mut num = String::new();
            while let Some(t) = toks.get(j) {
                match t {
                    Tok::Num(c) => num.push(*c),
                    Tok::Digit('0') if !num.is_empty() => num.push('0'),
                    _ => break,
                }
                j += 1;
            }
            if digit_before && !num.is_empty() {
                let d: u32 = num.parse().unwrap_or(1).max(1);
                toks.splice(k + 1..j, [Tok::FixedDen(d)]);
            } else if !(digit_before && matches!(toks.get(k + 1), Some(Tok::Digit(_)))) {
                toks[k] = Tok::Lit("/".to_string());
            }
        }
        k += 1;
    }
}

// ---------------------------------------------------------------------------------------
// Section selection
// ---------------------------------------------------------------------------------------

fn text_section_idx(secs: &[Section]) -> Option<usize> {
    if secs.len() == 4 {
        Some(3)
    } else {
        (0..secs.len()).rev().find(|&i| secs[i].has_at())
    }
}

fn numeric_section_count(secs: &[Section]) -> usize {
    if secs.len() == 4 {
        3
    } else if secs.last().is_some_and(|s| s.has_at()) {
        secs.len() - 1
    } else {
        secs.len()
    }
}

/// Returns (section index, whether the sign is dropped because the section is the
/// "negative" one).
fn select_section(secs: &[Section], cnt: usize, x: f64) -> (usize, bool) {
    let any_cond = secs[..cnt].iter().any(|s| s.cond.is_some());
    if !any_cond {
        return match cnt {
            1 => (0, false),
            2 => {
                if x >= 0.0 {
                    (0, false)
                } else {
                    (1, true)
                }
            }
            _ => {
                if x > 0.0 {
                    (0, false)
                } else if x < 0.0 {
                    (1, true)
                } else {
                    (2, false)
                }
            }
        };
    }
    let m0 = secs[0].cond.is_some_and(|c| c.test(x));
    if m0 {
        (0, false)
    } else if cnt >= 2 && secs[1].cond.is_some_and(|c| c.test(x)) {
        (1, false)
    } else if cnt >= 3 && secs[0].cond.is_some() && secs[1].cond.is_some() {
        (2, false)
    } else if cnt >= 2 {
        (1, false)
    } else {
        (0, false)
    }
}

// ---------------------------------------------------------------------------------------
// Decimal helper (15 significant digits, half-away-from-zero rounding)
// ---------------------------------------------------------------------------------------

/// `0.d1d2d3... * 10^point` with 15 significant digits.
struct Dec {
    digits: Vec<u8>,
    point: i32,
}

impl Dec {
    /// `x` must be finite and non-negative.
    fn from_f64(x: f64) -> Dec {
        let s = format!("{:.14e}", x);
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let digits: Vec<u8> = mant
            .bytes()
            .filter(u8::is_ascii_digit)
            .map(|b| b - b'0')
            .collect();
        Dec {
            digits,
            point: exp + 1,
        }
    }

    fn is_zero(&self) -> bool {
        self.digits.iter().all(|&d| d == 0)
    }

    fn shifted(mut self, by: i32) -> Dec {
        self.point += by;
        self
    }

    /// Rounds half away from zero to `nd` decimals; returns (integer digits without leading
    /// zeros -- empty for zero, exactly `nd` fraction digits).
    fn to_fixed(&self, nd: usize) -> (String, String) {
        let len = self.digits.len() as i32;
        let (mut ip, mut fp): (Vec<u8>, Vec<u8>) = if self.point <= 0 {
            let mut f = vec![0u8; (-self.point) as usize];
            f.extend_from_slice(&self.digits);
            (Vec::new(), f)
        } else if self.point >= len {
            let mut i = self.digits.clone();
            i.extend(std::iter::repeat_n(0u8, (self.point - len) as usize));
            (i, Vec::new())
        } else {
            let (a, b) = self.digits.split_at(self.point as usize);
            (a.to_vec(), b.to_vec())
        };
        if fp.len() > nd {
            let up = fp[nd] >= 5;
            fp.truncate(nd);
            if up {
                let mut carry = true;
                for d in fp.iter_mut().rev() {
                    if *d == 9 {
                        *d = 0;
                    } else {
                        *d += 1;
                        carry = false;
                        break;
                    }
                }
                if carry {
                    for d in ip.iter_mut().rev() {
                        if *d == 9 {
                            *d = 0;
                        } else {
                            *d += 1;
                            carry = false;
                            break;
                        }
                    }
                    if carry {
                        ip.insert(0, 1);
                    }
                }
            }
        } else {
            fp.resize(nd, 0);
        }
        let first = ip.iter().position(|&d| d != 0).unwrap_or(ip.len());
        let ips: String = ip[first..].iter().map(|d| (b'0' + d) as char).collect();
        let fps: String = fp.iter().map(|d| (b'0' + d) as char).collect();
        (ips, fps)
    }
}

// ---------------------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------------------

fn general_signed(x: f64) -> String {
    if !x.is_finite() {
        return "#NUM!".to_string();
    }
    let body = general_abs(x.abs());
    if x < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

fn general_abs(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let d = Dec::from_f64(x);
    let e = d.point - 1;
    if !(-4..11).contains(&e) {
        return general_sci(&d);
    }
    let int_digits = if e >= 0 { e + 1 } else { 1 };
    let nd = (10 - int_digits).max(0) as usize;
    let (ip, fp) = d.to_fixed(nd);
    if ip.len() > 11 {
        return general_sci(&d);
    }
    let ip = if ip.is_empty() { "0".to_string() } else { ip };
    let fp = fp.trim_end_matches('0');
    if fp.is_empty() {
        ip
    } else {
        format!("{ip}.{fp}")
    }
}

fn general_sci(d: &Dec) -> String {
    let mut e = d.point - 1;
    let m = Dec {
        digits: d.digits.clone(),
        point: 1,
    };
    let (mut ip, mut fp) = m.to_fixed(5);
    if ip == "10" {
        ip = "1".to_string();
        fp = "00000".to_string();
        e += 1;
    }
    let fp = fp.trim_end_matches('0');
    let mant = if fp.is_empty() {
        ip
    } else {
        format!("{ip}.{fp}")
    };
    format!("{mant}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}

// ---------------------------------------------------------------------------------------
// Number rendering
// ---------------------------------------------------------------------------------------

/// A text through a text section.
///
/// A code can repeat `@` up to [`MAX_CODE_CHARS`] times, so the output is not allowed to grow with
/// the repeat count or with the length of the text: each piece is cut to the bytes that cover
/// [`MAX_OUTPUT_CHARS`] characters and the output stops once it holds that many (the caller shows
/// no more than that anyway). A section that is a lone `@` returns the text whole, so a caller
/// can tell "shown as written" (and keeps no second copy of it).
fn render_text(sec: &Section, text: &str) -> String {
    if matches!(sec.toks.as_slice(), [Tok::At]) {
        return text.to_string();
    }
    // Four bytes per char at most: this many bytes always cover the characters shown.
    const ENOUGH: usize = MAX_OUTPUT_CHARS * 4;
    let mut cut = text.len().min(ENOUGH);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let text = &text[..cut];
    let mut out = String::new();
    for t in &sec.toks {
        if out.len() >= ENOUGH {
            break;
        }
        match t {
            Tok::At => out.push_str(text),
            Tok::Lit(s) => out.push_str(s),
            _ => {}
        }
    }
    out
}

/// A text-only section applied to a number: the number as General at each `@`.
fn render_with_general(sec: &Section, x: f64) -> String {
    render_text(sec, &general_signed(x))
}

fn render_number_section(sec: &Section, x: f64, use_abs: bool, opts: &Options) -> String {
    if sec.date {
        return render_date(sec, if use_abs { x.abs() } else { x }, opts);
    }
    let neg = x < 0.0 && !use_abs;
    let ax = x.abs();
    let body = render_numeric_tokens(&sec.toks, ax);
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

fn render_numeric_tokens(toks: &[Tok], ax: f64) -> String {
    let mut out = String::new();
    if toks.iter().any(|t| matches!(t, Tok::General)) {
        for t in toks {
            match t {
                Tok::General | Tok::At => out.push_str(&general_abs(ax)),
                Tok::Lit(s) => out.push_str(s),
                Tok::Percent => out.push('%'),
                _ => {}
            }
        }
        return out;
    }
    let pct = toks.iter().filter(|t| matches!(t, Tok::Percent)).count() as i32;
    let scale = toks.iter().filter(|t| matches!(t, Tok::Scale)).count() as i32;
    let shift = 2 * pct - 3 * scale;
    if toks.iter().any(|t| matches!(t, Tok::Slash)) {
        let scaled = 10f64.powi(shift) * ax;
        render_fraction(toks, scaled, &mut out);
        return out;
    }
    let dec = Dec::from_f64(ax).shifted(shift);
    if let Some(ei) = toks.iter().position(|t| matches!(t, Tok::Exp { .. })) {
        render_sci(toks, ei, dec, &mut out);
        return out;
    }
    let nd = decimals_of(toks);
    let (ip, fp) = dec.to_fixed(nd);
    emit_plain(toks, &ip, &fp, &mut out);
    out
}

fn decimals_of(toks: &[Tok]) -> usize {
    match toks.iter().position(|t| matches!(t, Tok::Point)) {
        Some(p) => toks[p + 1..]
            .iter()
            .filter(|t| matches!(t, Tok::Digit(_)))
            .count(),
        None => 0,
    }
}

/// Emits integer/fraction digits into the placeholders of `toks` (no exponent/fraction part).
fn emit_plain(toks: &[Tok], ip: &str, fp: &str, out: &mut String) {
    let pt = toks.iter().position(|t| matches!(t, Tok::Point));
    let int_end = pt.unwrap_or(toks.len());
    let ni = toks[..int_end]
        .iter()
        .filter(|t| matches!(t, Tok::Digit(_)))
        .count();
    let group = toks.iter().any(|t| matches!(t, Tok::Group));
    let ipb = ip.as_bytes();
    let l = ipb.len();
    let last_nz = fp.bytes().rposition(|b| b != b'0');
    let fpb = fp.as_bytes();
    // Pushes the digit at position q (counted from the right); handles grouping.
    let push_cell = |q: usize, kind: Option<char>, out: &mut String, visible: &mut bool| {
        if q < l {
            out.push(ipb[l - 1 - q] as char);
            *visible = true;
        } else {
            match kind {
                Some('0') => {
                    out.push('0');
                    *visible = true;
                }
                Some('?') => out.push(' '),
                _ => {}
            }
        }
        if group && q > 0 && q.is_multiple_of(3) && *visible {
            out.push(',');
        }
    };
    let mut visible = false;
    let mut k = 0usize;
    let mut j = 0usize;
    let mut after_point = false;
    for t in toks {
        match t {
            Tok::Digit(c) if !after_point => {
                if k == 0 && l > ni {
                    for q in (ni..l).rev() {
                        push_cell(q, None, out, &mut visible);
                    }
                }
                push_cell(ni - 1 - k, Some(*c), out, &mut visible);
                k += 1;
            }
            Tok::Digit(c) => {
                let d = fpb.get(j).copied().unwrap_or(b'0') as char;
                let trailing = last_nz.is_none_or(|nz| j > nz);
                match c {
                    '0' => out.push(d),
                    '#' => {
                        if !trailing {
                            out.push(d)
                        }
                    }
                    _ => out.push(if trailing { ' ' } else { d }),
                }
                j += 1;
            }
            Tok::Point => {
                if ni == 0 && l > 0 {
                    for q in (0..l).rev() {
                        push_cell(q, None, out, &mut visible);
                    }
                }
                out.push('.');
                after_point = true;
            }
            Tok::Lit(s) => out.push_str(s),
            Tok::Percent => out.push('%'),
            _ => {}
        }
    }
}

fn render_sci(toks: &[Tok], ei: usize, dec: Dec, out: &mut String) {
    let mant_toks = &toks[..ei];
    let (plus, upper) = match toks[ei] {
        Tok::Exp { plus, upper } => (plus, upper),
        _ => (true, true),
    };
    let ni = {
        let pe = mant_toks
            .iter()
            .position(|t| matches!(t, Tok::Point))
            .unwrap_or(mant_toks.len());
        mant_toks[..pe]
            .iter()
            .filter(|t| matches!(t, Tok::Digit(_)))
            .count()
    };
    let nd = decimals_of(mant_toks);
    let exp_digits = toks[ei + 1..]
        .iter()
        .filter(|t| matches!(t, Tok::Digit(_)))
        .count()
        .max(1);
    let (mut exp, mut ip, mut fp);
    if dec.is_zero() {
        exp = 0;
        let r = dec.to_fixed(nd);
        ip = r.0;
        fp = r.1;
    } else {
        let e = dec.point - 1;
        let step = ni.max(1) as i32;
        exp = if ni == 0 {
            e + 1
        } else {
            e - e.rem_euclid(step)
        };
        let digits = dec.digits.clone();
        let point = dec.point;
        let r = Dec {
            digits: digits.clone(),
            point: point - exp,
        }
        .to_fixed(nd);
        ip = r.0;
        fp = r.1;
        let limit = if ni == 0 { 0 } else { ni };
        if ip.len() > limit {
            exp += step;
            let r = Dec {
                digits,
                point: point - exp,
            }
            .to_fixed(nd);
            ip = r.0;
            fp = r.1;
        }
    }
    emit_plain(mant_toks, &ip, &fp, out);
    out.push(if upper { 'E' } else { 'e' });
    if exp < 0 {
        out.push('-');
    } else if plus {
        out.push('+');
    }
    out.push_str(&format!("{:0w$}", exp.unsigned_abs(), w = exp_digits));
    for t in &toks[ei + 1..] {
        match t {
            Tok::Lit(s) => out.push_str(s),
            Tok::Percent => out.push('%'),
            _ => {}
        }
    }
}

fn best_fraction(frac: f64, maxden: u32) -> (u64, u32) {
    let mut best = (0u64, 1u32);
    let mut best_err = f64::MAX;
    for q in 1..=maxden {
        let p = (frac * q as f64 + 0.5).floor();
        let err = (frac - p / q as f64).abs();
        if err < best_err - 1e-15 {
            best = (p as u64, q);
            best_err = err;
            if err < 1e-12 {
                break;
            }
        }
    }
    best
}

fn render_fraction(toks: &[Tok], x: f64, out: &mut String) {
    let Some(si) = toks.iter().position(|t| matches!(t, Tok::Slash)) else {
        return;
    };
    let mut ns = si;
    while ns > 0 && matches!(toks[ns - 1], Tok::Digit(_)) {
        ns -= 1;
    }
    let num_toks = &toks[ns..si];
    let whole_toks = &toks[..ns];
    let has_whole = whole_toks.iter().any(|t| matches!(t, Tok::Digit(_)));
    let (fixed, de, den_ph) = match toks.get(si + 1) {
        Some(Tok::FixedDen(d)) => (Some(*d), si + 2, 0usize),
        _ => {
            let mut de = si + 1;
            while matches!(toks.get(de), Some(Tok::Digit(_))) {
                de += 1;
            }
            (None, de, de - (si + 1))
        }
    };
    let den_toks = &toks[si + 1..de];
    let maxden = 10u32.pow(den_ph.clamp(1, 5) as u32) - 1;
    // Clean float noise the way Excel's 15-digit storage does.
    let x15: f64 = format!("{:.14e}", x).parse().unwrap_or(x);
    let whole_f = x15.floor();
    let frac = x15 - whole_f;
    let (mut whole, p, q) = if has_whole {
        let (p, q) = match fixed {
            Some(d) => ((frac * d as f64 + 0.5).floor() as u64, d),
            None => best_fraction(frac, maxden),
        };
        (whole_f, p, q)
    } else {
        let (p, q) = match fixed {
            Some(d) => ((x15 * d as f64 + 0.5).floor() as u64, d),
            None => best_fraction(x15, maxden),
        };
        (0.0, p, q)
    };
    let (mut p, mut q) = (p, q);
    if has_whole && p >= q as u64 {
        whole += 1.0;
        p = 0;
        q = 1;
    }
    let _ = q;
    let whole_str = if has_whole {
        let s = format!("{whole:.0}");
        if whole == 0.0 && p == 0 {
            "0".to_string()
        } else if whole == 0.0 {
            String::new()
        } else {
            s
        }
    } else {
        String::new()
    };
    if has_whole {
        emit_plain(whole_toks, &whole_str, "", out);
    } else {
        for t in whole_toks {
            if let Tok::Lit(s) = t {
                out.push_str(s);
            }
        }
    }
    let den_width = match fixed {
        Some(d) => d.to_string().len(),
        None => den_toks.len(),
    };
    if has_whole && p == 0 {
        // Whole number: the fraction slots are blank.
        out.push_str(&" ".repeat(num_toks.len() + 1 + den_width));
    } else {
        let ns_str = p.to_string();
        let pad = num_toks.len().saturating_sub(ns_str.len());
        let mut left = String::new();
        for t in num_toks.iter().take(pad) {
            match t {
                Tok::Digit('0') => left.push('0'),
                Tok::Digit('?') => left.push(' '),
                _ => {}
            }
        }
        out.push_str(&left);
        out.push_str(&ns_str);
        out.push('/');
        let den_str = match fixed {
            Some(d) => d.to_string(),
            None => q.to_string(),
        };
        out.push_str(&den_str);
        if fixed.is_none() {
            let pad = den_toks.len().saturating_sub(den_str.len());
            // Right-align denominators of `?` as trailing spaces; `0` pads with zeros on the left.
            for t in den_toks.iter().rev().take(pad) {
                if matches!(t, Tok::Digit('?')) {
                    out.push(' ');
                }
            }
        }
    }
    for t in &toks[de..] {
        match t {
            Tok::Lit(s) => out.push_str(s),
            Tok::Percent => out.push('%'),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------------------

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Serial day number -> (year, month, day, weekday 0=Sunday), in the calendar `opts` selects.
fn serial_to_ymd(day: i64, opts: &Options) -> Option<(i32, u32, u32, u32)> {
    if let Some(null_day) = opts.null_day {
        // OpenDocument: plain proleptic Gregorian days from the null date, years 1..=9999.
        let days = day.checked_add(null_day)?;
        if !(-719_162..=2_932_896).contains(&days) {
            return None;
        }
        let (y, m, d) = civil_from_days(days);
        return Some((y as i32, m, d, (days + 4).rem_euclid(7) as u32));
    }
    if opts.date1904 {
        if !(0..=2_957_003).contains(&day) {
            return None;
        }
        let days = day - 24107;
        let (y, m, d) = civil_from_days(days);
        return Some((y as i32, m, d, (days + 4).rem_euclid(7) as u32));
    }
    if !(0..=2_958_465).contains(&day) {
        return None;
    }
    let wd = (day - 1).rem_euclid(7) as u32;
    match day {
        0 => Some((1900, 1, 0, wd)),
        60 => Some((1900, 2, 29, wd)),
        1..=59 => {
            let (y, m, d) = civil_from_days(day + 1 - 25569);
            Some((y as i32, m, d, wd))
        }
        _ => {
            let (y, m, d) = civil_from_days(day - 25569);
            Some((y as i32, m, d, wd))
        }
    }
}

struct Era {
    long: &'static str,
    short: &'static str,
    roman: &'static str,
    start: (i32, u32, u32),
}

const ERAS: [Era; 5] = [
    Era {
        long: "令和",
        short: "令",
        roman: "R",
        start: (2019, 5, 1),
    },
    Era {
        long: "平成",
        short: "平",
        roman: "H",
        start: (1989, 1, 8),
    },
    Era {
        long: "昭和",
        short: "昭",
        roman: "S",
        start: (1926, 12, 25),
    },
    Era {
        long: "大正",
        short: "大",
        roman: "T",
        start: (1912, 7, 30),
    },
    Era {
        long: "明治",
        short: "明",
        roman: "M",
        start: (1868, 9, 8),
    },
];

fn era_of(y: i32, m: u32, d: u32) -> Option<&'static Era> {
    ERAS.iter().find(|e| (y, m, d) >= e.start)
}

const EN_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const EN_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const JA_DAYS: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];

fn render_date(sec: &Section, x: f64, opts: &Options) -> String {
    const HASHES: &str = "########";
    // A negative serial is a date before the null date in the OpenDocument calendar (when the
    // section shows a calendar date); everywhere else it is not displayable.
    let shows_date = |sec: &Section| {
        sec.toks.iter().any(|t| {
            matches!(
                t,
                Tok::Year(_)
                    | Tok::Month(_)
                    | Tok::Day(_)
                    | Tok::EraG(_)
                    | Tok::EraE(_)
                    | Tok::JaWeekday(_)
                    | Tok::BuddhistYear(_)
            )
        })
    };
    let negative_ok = opts.null_day.is_some() && x >= -1e9 && shows_date(sec);
    if (x < 0.0 && !negative_ok) || x >= 1e9 {
        return HASHES.to_string();
    }
    let ja = match sec.lang {
        Some(0x411) => true,
        Some(l) if l & 0xFF == 0x09 => false,
        _ => opts.locale == Locale::Ja,
    };
    let roc = sec.lang == Some(0x404);
    let frac_digits = sec
        .toks
        .iter()
        .filter_map(|t| {
            if let Tok::FracSec(n) = t {
                Some(*n as u32)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);
    let gran = 10i64.pow(3 - frac_digits.min(3));
    let round_ms = |ms: i64| ((ms + gran / 2) / gran) * gran;
    let day_f = x.floor();
    let mut ms_day = round_ms(((x - day_f) * 86_400_000.0).round() as i64);
    let mut day = day_f as i64;
    if ms_day >= 86_400_000 {
        ms_day -= 86_400_000;
        day += 1;
    }
    let total_ms = round_ms((x * 86_400_000.0).round() as i64);
    let has_calendar = shows_date(sec);
    let ymd = serial_to_ymd(day, opts);
    if has_calendar && ymd.is_none() {
        return HASHES.to_string();
    }
    let (y, mo, d, wd) = ymd.unwrap_or((1900, 1, 0, 0));
    let hour = (ms_day / 3_600_000) as u32;
    let minute = ((ms_day / 60_000) % 60) as u32;
    let second = ((ms_day / 1000) % 60) as u32;
    let milli = (ms_day % 1000) as u32;
    let ampm = sec.toks.iter().any(|t| matches!(t, Tok::AmPm { .. }));
    let era = era_of(y, mo, d);
    let mut out = String::new();
    for t in &sec.toks {
        match t {
            Tok::Lit(s) => out.push_str(s),
            Tok::At => {}
            Tok::Year(2) => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            Tok::Year(_) => out.push_str(&format!("{y:04}")),
            Tok::BuddhistYear(2) => out.push_str(&format!("{:02}", (y + 543).rem_euclid(100))),
            Tok::BuddhistYear(_) => out.push_str(&format!("{:04}", y + 543)),
            Tok::Month(w) => {
                let mi = (mo.max(1) - 1) as usize;
                match w {
                    1 => out.push_str(&mo.to_string()),
                    2 => out.push_str(&format!("{mo:02}")),
                    3 | 4 => {
                        if ja {
                            out.push_str(&format!("{mo}月"));
                        } else if *w == 3 {
                            out.push_str(&EN_MONTHS[mi.min(11)][..3]);
                        } else {
                            out.push_str(EN_MONTHS[mi.min(11)]);
                        }
                    }
                    _ => {
                        if ja {
                            out.push_str(&mo.to_string());
                        } else {
                            out.push_str(&EN_MONTHS[mi.min(11)][..1]);
                        }
                    }
                }
            }
            Tok::Day(1) => out.push_str(&d.to_string()),
            Tok::Day(2) => out.push_str(&format!("{d:02}")),
            Tok::Day(w) => {
                let wi = wd as usize % 7;
                if ja {
                    out.push_str(JA_DAYS[wi]);
                    if *w >= 4 {
                        out.push_str("曜日");
                    }
                } else if *w == 3 {
                    out.push_str(&EN_DAYS[wi][..3]);
                } else {
                    out.push_str(EN_DAYS[wi]);
                }
            }
            Tok::JaWeekday(w) => {
                out.push_str(JA_DAYS[wd as usize % 7]);
                if *w >= 4 {
                    out.push_str("曜日");
                }
            }
            Tok::Hour(w) => {
                let h = if ampm {
                    match hour % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    hour
                };
                push_padded(&mut out, h as i64, *w);
            }
            Tok::Minute(w) => push_padded(&mut out, minute as i64, *w),
            Tok::Second(w) => push_padded(&mut out, second as i64, *w),
            Tok::FracSec(n) => {
                out.push('.');
                let digits = format!("{milli:03}");
                let n = *n as usize;
                if n <= 3 {
                    out.push_str(&digits[..n]);
                } else {
                    out.push_str(&digits);
                    out.push_str(&"0".repeat(n - 3));
                }
            }
            Tok::AmPm { short, lower } => {
                let pm = hour >= 12;
                let s = match (short, pm) {
                    (false, false) => "AM",
                    (false, true) => "PM",
                    (true, false) => "A",
                    (true, true) => "P",
                };
                out.push_str(&if *lower {
                    s.to_ascii_lowercase()
                } else {
                    s.to_string()
                });
            }
            Tok::Elapsed(c, w) => {
                let v = match c {
                    'h' => total_ms / 3_600_000,
                    'm' => total_ms / 60_000,
                    _ => total_ms / 1000,
                };
                push_padded(&mut out, v, *w);
            }
            Tok::EraG(w) => {
                if let Some(e) = era {
                    out.push_str(match w {
                        1 => e.roman,
                        2 => e.short,
                        _ => e.long,
                    });
                }
            }
            Tok::EraE(w) => {
                let yr = if roc {
                    y - 1911
                } else if let Some(e) = era {
                    y - e.start.0 + 1
                } else {
                    y
                };
                push_padded(&mut out, yr as i64, *w);
            }
            _ => {}
        }
    }
    out
}

fn push_padded(out: &mut String, v: i64, width: u8) {
    if width >= 2 {
        out.push_str(&format!("{v:02}"));
    } else {
        out.push_str(&v.to_string());
    }
}

// ---------------------------------------------------------------------------------------
// ISO 8601 (ODS) -> Excel serial
// ---------------------------------------------------------------------------------------

/// Converts an ODS date/time (`2026-10-02`, `2026-10-02T13:45:00`, optional fraction and
/// zone suffix, which is ignored) to a 1900-system Excel serial.
pub fn iso_datetime_to_serial(s: &str) -> Option<f64> {
    let (days, frac) = iso_datetime_parts(s)?;
    let mut serial = days + 25569;
    if serial <= 60 {
        // Excel's 1900 leap-year bug: serials before 1900-03-01 are one lower.
        serial -= 1;
    }
    Some(serial as f64 + frac)
}

/// Converts an ODS date/time to the serial of the OpenDocument calendar whose day 0 is
/// `null_day` days after 1970-01-01 (see [`Options::null_day`]): no leap-year bug, negative
/// below the null date.
pub fn iso_datetime_to_ods_serial(s: &str, null_day: i64) -> Option<f64> {
    let (days, frac) = iso_datetime_parts(s)?;
    Some(days.checked_sub(null_day)? as f64 + frac)
}

/// The day (days since 1970-01-01) of an ISO 8601 date or date-time (the time is ignored): the
/// ods `table:null-date`.
pub fn iso_days_since_1970(s: &str) -> Option<i64> {
    iso_datetime_parts(s).map(|(d, _)| d)
}

/// `(days since 1970-01-01, fraction of a day)` of an ISO 8601 date or date-time.
fn iso_datetime_parts(s: &str) -> Option<(i64, f64)> {
    let s = s.trim();
    let (date, time) = match s.split_once(['T', 't']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let (neg, date) = match date.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, date),
    };
    let mut it = date.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if neg { y.checked_neg()? } else { y };
    // A spreadsheet date is in years 1..=9999; anything else is not a date (and a huge year would
    // overflow the day count).
    if !(1..=9999).contains(&y) {
        return None;
    }
    let days = days_from_civil(y, m, d);
    let mut frac = 0.0;
    if let Some(t) = time {
        let t = t.trim_end_matches(['Z', 'z']);
        // Drop a numeric zone offset (`+09:00` / `-05:00`).
        let t = match t.find(['+', '-']) {
            Some(p) => &t[..p],
            None => t,
        };
        frac = hms_to_days(t)?;
    }
    Some((days, frac))
}

fn hms_to_days(t: &str) -> Option<f64> {
    let mut it = t.split(':');
    let h: f64 = it.next()?.parse().ok()?;
    let m: f64 = it.next().map_or(Some(0.0), |v| v.parse().ok())?;
    let sec: f64 = it.next().map_or(Some(0.0), |v| v.parse().ok())?;
    if it.next().is_some()
        || h < 0.0
        || m < 0.0
        || sec < 0.0
        || h >= 25.0
        || m >= 60.0
        || sec >= 61.0
    {
        return None;
    }
    Some((h * 3600.0 + m * 60.0 + sec) / 86400.0)
}

/// Converts an ISO 8601 duration (`PT13H45M00S`, `P1DT2H`, `PT0.5S`, `-PT1H`) to days.
/// Years and months have no fixed length, so they are rejected.
pub fn iso_duration_to_serial(s: &str) -> Option<f64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let s = s.strip_prefix('P').or_else(|| s.strip_prefix('p'))?;
    let mut total = 0.0f64;
    let mut in_time = false;
    let mut num = String::new();
    let mut any = false;
    for c in s.chars() {
        match c {
            'T' | 't' => {
                if !num.is_empty() {
                    return None;
                }
                in_time = true;
            }
            '0'..='9' | '.' => num.push(c),
            _ => {
                let v: f64 = num.parse().ok()?;
                num.clear();
                let secs = match (c.to_ascii_uppercase(), in_time) {
                    ('D', false) => v * 86400.0,
                    ('W', false) => v * 7.0 * 86400.0,
                    ('H', true) => v * 3600.0,
                    ('M', true) => v * 60.0,
                    ('S', true) => v,
                    _ => return None,
                };
                total += secs;
                any = true;
            }
        }
    }
    if !num.is_empty() || !any {
        return None;
    }
    let days = total / 86400.0;
    Some(if neg { -days } else { days })
}

// ---------------------------------------------------------------------------------------
// Built-in format table
// ---------------------------------------------------------------------------------------

/// Maps a built-in number format id (`numFmtId` / BIFF `ifmt`) to its format code.
/// Locale-dependent ids follow ECMA-376 18.8.30 and Excel's behaviour: Japanese gives `14` =
/// `yyyy/m/d`, `22` = `yyyy/m/d h:mm`, `5`-`8`/`41`-`44` with the yen sign, and the CJK
/// ids `27`-`36`, `50`-`58`. ids 27-36 and 50-58 are only ever written by East-Asian Excel, so
/// the Japanese codes are also returned for English (judgement call; the ECMA table has no
/// English meaning for them). Ids 23-26 and 59+ return `None`.
pub fn builtin_format_code(id: u32, locale: Locale) -> Option<&'static str> {
    let ja = locale == Locale::Ja;
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => {
            if ja {
                "\"¥\"#,##0;\"¥\"\\-#,##0"
            } else {
                "\"$\"#,##0_);\\(\"$\"#,##0\\)"
            }
        }
        6 => {
            if ja {
                "\"¥\"#,##0;[Red]\"¥\"\\-#,##0"
            } else {
                "\"$\"#,##0_);[Red]\\(\"$\"#,##0\\)"
            }
        }
        7 => {
            if ja {
                "\"¥\"#,##0.00;\"¥\"\\-#,##0.00"
            } else {
                "\"$\"#,##0.00_);\\(\"$\"#,##0.00\\)"
            }
        }
        8 => {
            if ja {
                "\"¥\"#,##0.00;[Red]\"¥\"\\-#,##0.00"
            } else {
                "\"$\"#,##0.00_);[Red]\\(\"$\"#,##0.00\\)"
            }
        }
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => {
            if ja {
                "yyyy/m/d"
            } else {
                "m/d/yyyy"
            }
        }
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => {
            if ja {
                "yyyy/m/d h:mm"
            } else {
                "m/d/yyyy h:mm"
            }
        }
        27 | 36 | 50 | 57 => "[$-411]ge.m.d",
        28 | 29 | 51 | 54 | 58 => "[$-411]ggge\"年\"m\"月\"d\"日\"",
        30 => "m/d/yy",
        31 => "yyyy\"年\"m\"月\"d\"日\"",
        32 => "h\"時\"mm\"分\"",
        33 => "h\"時\"mm\"分\"ss\"秒\"",
        34 | 52 | 55 => "yyyy\"年\"m\"月\"",
        35 | 53 | 56 => "m\"月\"d\"日\"",
        37 => "#,##0_);(#,##0)",
        38 => "#,##0_);[Red](#,##0)",
        39 => "#,##0.00_);(#,##0.00)",
        40 => "#,##0.00_);[Red](#,##0.00)",
        41 => {
            if ja {
                "_ * #,##0_ ;_ * \\-#,##0_ ;_ * \"-\"_ ;_ @_ "
            } else {
                "_(* #,##0_);_(* \\(#,##0\\);_(* \"-\"_);_(@_)"
            }
        }
        42 => {
            if ja {
                "_ \"¥\"* #,##0_ ;_ \"¥\"* \\-#,##0_ ;_ \"¥\"* \"-\"_ ;_ @_ "
            } else {
                "_(\"$\"* #,##0_);_(\"$\"* \\(#,##0\\);_(\"$\"* \"-\"_);_(@_)"
            }
        }
        43 => {
            if ja {
                "_ * #,##0.00_ ;_ * \\-#,##0.00_ ;_ * \"-\"??_ ;_ @_ "
            } else {
                "_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)"
            }
        }
        44 => {
            if ja {
                "_ \"¥\"* #,##0.00_ ;_ \"¥\"* \\-#,##0.00_ ;_ \"¥\"* \"-\"??_ ;_ @_ "
            } else {
                "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)"
            }
        }
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    //! Expectations come from the format-code rules (Microsoft "Number format codes" support
    //! article, ECMA-376 Part 1 18.8.30/18.8.31). Cases marked "unverified" could not be
    //! confirmed against a primary source and encode this engine's best understanding of
    //! Excel's behaviour.
    use super::*;

    fn f(code: &str, v: f64) -> String {
        format_value(code, Value::Number(v), &Options::default())
    }
    fn fj(code: &str, v: f64) -> String {
        format_value(
            code,
            Value::Number(v),
            &Options {
                date1904: false,
                locale: Locale::Ja,
                null_day: None,
            },
        )
    }
    fn f1904(code: &str, v: f64) -> String {
        format_value(
            code,
            Value::Number(v),
            &Options {
                date1904: true,
                locale: Locale::En,
                null_day: None,
            },
        )
    }
    fn ft(code: &str, t: &str) -> String {
        format_value(code, Value::Text(t), &Options::default())
    }
    fn ser(s: &str) -> f64 {
        iso_datetime_to_serial(s).unwrap()
    }
    /// 2026-10-02 (a Friday) is serial 46297; 13:45:00 is this fraction of a day.
    const D: f64 = 46297.0;
    fn t1345() -> f64 {
        (13.0 * 3600.0 + 45.0 * 60.0) / 86400.0
    }

    // ---- values that ignore the format ------------------------------------------------

    #[test]
    fn bool_and_error_ignore_format() {
        let o = Options::default();
        assert_eq!(format_value("0.00", Value::Bool(true), &o), "TRUE");
        assert_eq!(format_value("0.00", Value::Bool(false), &o), "FALSE");
        assert_eq!(format_value("yyyy", Value::Error("#DIV/0!"), &o), "#DIV/0!");
        assert_eq!(format_value("", Value::Error("#N/A"), &o), "#N/A");
    }

    #[test]
    fn non_finite_numbers_do_not_panic() {
        assert_eq!(f("0.00", f64::NAN), "#NUM!");
        assert_eq!(f("0.00", f64::INFINITY), "#NUM!");
        assert_eq!(f("General", f64::NEG_INFINITY), "#NUM!");
        assert_eq!(f("", f64::NAN), "#NUM!");
    }

    // ---- sections and signs ------------------------------------------------------------

    #[test]
    fn one_section_negative_gets_minus() {
        assert_eq!(f("0", 5.0), "5");
        assert_eq!(f("0", -5.0), "-5");
        assert_eq!(f("0.00", -1.5), "-1.50");
        assert_eq!(f("\"$\"0", -5.0), "-$5");
        assert_eq!(f("0", 0.0), "0");
    }

    #[test]
    fn two_sections_negative_has_no_automatic_sign() {
        assert_eq!(f("0;(0)", 5.0), "5");
        assert_eq!(f("0;(0)", -5.0), "(5)");
        assert_eq!(f("0;(0)", 0.0), "0");
        assert_eq!(f("0.0;-0.0", -2.0), "-2.0");
        assert_eq!(f("0;0", -2.0), "2");
    }

    #[test]
    fn three_sections_pos_neg_zero() {
        assert_eq!(f("0;-0;\"zero\"", 7.0), "7");
        assert_eq!(f("0;-0;\"zero\"", -7.0), "-7");
        assert_eq!(f("0;-0;\"zero\"", 0.0), "zero");
        assert_eq!(f("#,##0;[Red](#,##0);\"-\"", 0.0), "-");
        assert_eq!(f("#,##0;[Red](#,##0);\"-\"", -1234.0), "(1,234)");
    }

    #[test]
    fn four_sections_text() {
        let code = "0;-0;0;\"text:\"@";
        assert_eq!(ft(code, "abc"), "text:abc");
        assert_eq!(f(code, 3.0), "3");
        assert_eq!(f(code, -3.0), "-3");
        // A 4th section without `@` shows only its literal.
        assert_eq!(ft("0;0;0;\"hidden\"", "abc"), "hidden");
    }

    #[test]
    fn empty_sections_hide_everything() {
        assert_eq!(f(";;;", 5.0), "");
        assert_eq!(f(";;;", -5.0), "");
        assert_eq!(f(";;;", 0.0), "");
        assert_eq!(ft(";;;", "abc"), "");
        assert_eq!(f("0;", -5.0), "");
        assert_eq!(f("0;", 5.0), "5");
    }

    #[test]
    fn conditions() {
        let c = "[>=100]\"big\";\"small\"";
        assert_eq!(f(c, 150.0), "big");
        assert_eq!(f(c, 100.0), "big");
        assert_eq!(f(c, 50.0), "small");
        let c3 = "[<0]\"neg\";[>0]\"pos\";\"zero\"";
        // Unverified: with explicit conditions the minus sign is kept.
        assert_eq!(f(c3, -1.0), "-neg");
        assert_eq!(f(c3, 1.0), "pos");
        assert_eq!(f(c3, 0.0), "zero");
        let c4 = "[=0]\"nil\";[<>5]0.0;0.00";
        assert_eq!(f(c4, 0.0), "nil");
        assert_eq!(f(c4, 3.0), "3.0");
        assert_eq!(f(c4, 5.0), "5.00");
        let c5 = "[<=100]0;[>100]#,##0";
        assert_eq!(f(c5, 100.0), "100");
        assert_eq!(f(c5, 12345.0), "12,345");
        // Microsoft's example `[Red][<=100];[Blue][>100]` selects by value; both sections empty.
        assert_eq!(f("[Red][<=100];[Blue][>100]", 50.0), "");
        assert_eq!(f("[Red][<=100];[Blue][>100]", 500.0), "");
    }

    #[test]
    fn broken_conditions_and_brackets_fall_back() {
        assert_eq!(f("[>abc]0.00", 1.5), "1.50");
        assert_eq!(f("[unclosed0.00", 1.5), "1.5");
        assert_eq!(ft("[unclosed", "txt"), "txt");
    }

    #[test]
    fn too_many_sections_fall_back_to_general() {
        assert_eq!(f("0;0;0;0;0", 1.5), "1.5");
    }

    // ---- digit placeholders ------------------------------------------------------------

    #[test]
    fn zero_hash_question() {
        assert_eq!(f("0.00", 3.1425), "3.14");
        assert_eq!(f("#.##", 3.1), "3.1");
        assert_eq!(f("#.##", 5.0), "5.");
        assert_eq!(f("#.##", 0.0), ".");
        assert_eq!(f("0.0?", 1.5), "1.5 ");
        assert_eq!(f("0.0?", 1.25), "1.25");
        assert_eq!(f("000", 5.0), "005");
        assert_eq!(f("???", 5.0), "  5");
        assert_eq!(f("#", 0.0), "");
        assert_eq!(f("00.0", 5.0), "05.0");
        assert_eq!(f(".00", 0.5), ".50");
        assert_eq!(f("#.00", 0.5), ".50");
        assert_eq!(f("0.00", 0.5), "0.50");
        assert_eq!(f("0", 12345.0), "12345");
        assert_eq!(f("0.0", 12.34), "12.3");
        assert_eq!(f("0.000", 1e-9), "0.000");
        // Trailing `#` decimals drop zeros but not significant digits.
        assert_eq!(f("0.0##", 1.5), "1.5");
        assert_eq!(f("0.0##", 1.2345), "1.235");
    }

    #[test]
    fn literals_inside_digit_runs() {
        assert_eq!(f("000-0000", 1234567.0), "123-4567");
        assert_eq!(f("(000) 000-0000", 1234567890.0), "(123) 456-7890");
        assert_eq!(f("0-0", 12.0), "1-2");
        assert_eq!(f("0\"x\"0", 12.0), "1x2");
    }

    #[test]
    fn thousands_separator() {
        assert_eq!(f("#,##0", 1234567.0), "1,234,567");
        assert_eq!(f("#,##0", 0.0), "0");
        assert_eq!(f("#,##0", 999.0), "999");
        assert_eq!(f("#,##0", 1000.0), "1,000");
        assert_eq!(f("#,###", 0.0), "");
        assert_eq!(f("0,000", 5.0), "0,005");
        assert_eq!(f("#,##0.00", 1234.5), "1,234.50");
        assert_eq!(f("#,##0.00", 999999.999), "1,000,000.00");
        assert_eq!(f("#,##0", -1234.0), "-1,234");
        assert_eq!(f("#,##0", 1e15), "1,000,000,000,000,000");
        assert_eq!(f("0", 1234567.0), "1234567");
    }

    #[test]
    fn trailing_commas_scale_by_thousands() {
        // Microsoft: "#," shows 12000 as 12, "#,###" shows 12000 as 12,000, "0.0,," shows 12200000 as 12.2.
        assert_eq!(f("#,", 12000.0), "12");
        assert_eq!(f("#,###", 12000.0), "12,000");
        assert_eq!(f("0.0,,", 12200000.0), "12.2");
        assert_eq!(f("#,##0,", 1234567.0), "1,235");
        assert_eq!(f("0,\"K\"", 1500.0), "2K");
        assert_eq!(f("0.0,,\"M\"", 3450000.0), "3.5M");
        // Comma directly before the decimal point also scales.
        assert_eq!(f("0,.0", 12345.0), "12.3");
    }

    #[test]
    fn percent() {
        assert_eq!(f("0%", 0.125), "13%");
        assert_eq!(f("0.0%", 0.125), "12.5%");
        assert_eq!(f("0.00%", 0.5), "50.00%");
        assert_eq!(f("0%", -0.05), "-5%");
        assert_eq!(f("0%", 0.005), "1%");
        assert_eq!(f("0%", 0.285), "29%");
        assert_eq!(f("0.0%", 1.0), "100.0%");
        assert_eq!(f("0%%", 0.01), "100%%");
        assert_eq!(f("#,##0%", 12.345), "1,235%");
    }

    // ---- rounding ----------------------------------------------------------------------

    #[test]
    fn rounding_is_half_away_from_zero_on_decimal_value() {
        // Binary floats put these just below the tie; Excel keeps 15 digits and rounds up.
        assert_eq!(f("0.00", 1.005), "1.01");
        assert_eq!(f("0.00", 2.675), "2.68");
        assert_eq!(f("0.0", 0.05), "0.1");
        assert_eq!(f("0.00", 0.125), "0.13");
        assert_eq!(f("0.0", 0.25), "0.3");
        assert_eq!(f("0", 0.5), "1");
        assert_eq!(f("0", 1.5), "2");
        assert_eq!(f("0", 2.5), "3");
        assert_eq!(f("0", -0.5), "-1");
        assert_eq!(f("0", -2.5), "-3");
        assert_eq!(f("0", -1.5), "-2");
        assert_eq!(f("0.00", 0.995), "1.00");
        assert_eq!(f("0", 0.49), "0");
        assert_eq!(f("0", 9.5), "10");
        assert_eq!(f("0", 99.5), "100");
        assert_eq!(f("#,##0", 999.5), "1,000");
        assert_eq!(f("0.0", 9.95), "10.0");
        assert_eq!(f("0.00", -1.005), "-1.01");
    }

    #[test]
    fn tiny_negative_keeps_sign_unverified() {
        // Unverified: Excel is believed to show "-0.00" for -0.001 under `0.00`.
        assert_eq!(f("0.00", -0.001), "-0.00");
        assert_eq!(f("0.00;(0.00)", -0.001), "(0.00)");
    }

    // ---- scientific --------------------------------------------------------------------

    #[test]
    fn scientific() {
        assert_eq!(f("0.00E+00", 12345.6789), "1.23E+04");
        assert_eq!(f("0.00E+00", 0.000123), "1.23E-04");
        assert_eq!(f("0.00E+00", 0.0), "0.00E+00");
        assert_eq!(f("0.0E+0", 12345.0), "1.2E+4");
        assert_eq!(f("0.00E-00", 12345.0), "1.23E04");
        assert_eq!(f("0.00E-00", 0.00012), "1.20E-04");
        assert_eq!(f("0.00e+00", 12345.6789), "1.23e+04");
        assert_eq!(f("0.00E+00", 9.999), "1.00E+01");
        assert_eq!(f("0.00E+00", 1e100), "1.00E+100");
        assert_eq!(f("0.00E+00", -12345.6789), "-1.23E+04");
        assert_eq!(f("0E+00", 12345.0), "1E+04");
        assert_eq!(f("0.00E+00", 1.0), "1.00E+00");
    }

    #[test]
    fn engineering_notation_id48() {
        // ECMA-376 numFmtId 48 is `##0.0E+0`: exponent multiple of 3.
        assert_eq!(f("##0.0E+0", 12345.0), "12.3E+3");
        assert_eq!(f("##0.0E+0", 1234567.0), "1.2E+6");
        assert_eq!(f("##0.0E+0", 0.00012345), "123.5E-6");
        assert_eq!(f("##0.0E+0", 999999.0), "1.0E+6");
    }

    // ---- fractions ---------------------------------------------------------------------

    #[test]
    fn fractions() {
        assert_eq!(f("# ?/?", 3.5), "3 1/2");
        assert_eq!(f("# ?/?", 0.5), " 1/2");
        assert_eq!(f("# ?/?", 3.1425), "3 1/7");
        assert_eq!(f("# ??/??", 3.1425), "3  1/7 ");
        assert_eq!(f("?/?", 0.5), "1/2");
        assert_eq!(f("?/?", 1.5), "3/2");
        assert_eq!(f("?/?", 0.0), "0/1");
        assert_eq!(f("# ?/?", -3.5), "-3 1/2");
        assert_eq!(f("# ?/?", 0.99), "1    ");
    }

    #[test]
    fn fractions_with_fixed_denominator() {
        assert_eq!(f("?/4", 0.5), "2/4");
        assert_eq!(f("# ?/8", 2.25), "2 2/8");
        assert_eq!(f("?/100", 0.256), "26/100");
        assert_eq!(f("# ?/4", 1.9), "2    ");
    }

    #[test]
    fn whole_numbers_in_fraction_format_unverified() {
        // Unverified: padding Excel emits for the empty fraction slot and aligned `??/??`.
        assert_eq!(f("# ?/?", 3.0), "3    ");
        assert_eq!(f("# ??/??", 0.5), "  1/2 ");
    }

    // ---- literals and brackets -----------------------------------------------------------

    #[test]
    fn literals() {
        assert_eq!(f("\"$\"#,##0.00", 1234.5), "$1,234.50");
        assert_eq!(f("\\$0", 5.0), "$5");
        assert_eq!(f("0\" units\"", 5.0), "5 units");
        assert_eq!(f("$0.00", 5.0), "$5.00");
        assert_eq!(f("(0)", 5.0), "(5)");
        assert_eq!(f("0.00 \"kg\"", 2.5), "2.50 kg");
        assert_eq!(f("#,##0\"円\"", 1500.0), "1,500円");
        assert_eq!(f("+0;-0", 3.0), "+3");
        assert_eq!(f("0 : 0", 12.0), "1 : 2");
        assert_eq!(f("\"abc", 5.0), "abc");
        assert_eq!(f("\"only text\"", 5.0), "only text");
        assert_eq!(f("0!", 5.0), "5!");
    }

    #[test]
    fn underscore_and_asterisk() {
        // `_x` is one space; `*x` (fill) is dropped on a terminal.
        assert_eq!(f("0_)", 5.0), "5 ");
        assert_eq!(f("#,##0_);(#,##0)", 5.0), "5 ");
        assert_eq!(f("#,##0_);(#,##0)", -5.0), "(5)");
        assert_eq!(f("0*-", 5.0), "5");
        // `*` consumes the following character (the fill), so only `_(` and `_)` remain.
        assert_eq!(f("_(* #,##0_)", 1234.0), " 1,234 ");
        assert_eq!(f("0_", 5.0), "5");
    }

    #[test]
    fn colors_and_dbnum_are_skipped() {
        assert_eq!(f("[Red]0.00;[Blue]-0.00", -1.5), "-1.50");
        assert_eq!(f("[Red]0.00;[Blue]-0.00", 1.5), "1.50");
        assert_eq!(f("[Color10]0", 5.0), "5");
        assert_eq!(f("[DBNum1]0", 5.0), "5");
        assert_eq!(f("[DBNum2][$-411]0", 12.0), "12");
        assert_eq!(f("[BLACK]0", 5.0), "5");
    }

    #[test]
    fn currency_and_locale_brackets() {
        assert_eq!(f("[$¥-411]#,##0", 1000.0), "¥1,000");
        assert_eq!(f("[$€-407] #,##0.00", 1234.5), "€ 1,234.50");
        assert_eq!(f("[$-411]0", 5.0), "5");
        assert_eq!(f("[$-ja-JP]#,##0", 5000.0), "5,000");
        assert_eq!(f("[$$-409]#,##0.00", 3.5), "$3.50");
        assert_eq!(f("[$USD]0", 3.0), "USD3");
        assert_eq!(f("[$¥-411]#,##0;[Red]-[$¥-411]#,##0", -1000.0), "-¥1,000");
    }

    // ---- General ---------------------------------------------------------------------------

    #[test]
    fn general_integers_and_boundary() {
        assert_eq!(f("General", 0.0), "0");
        assert_eq!(f("General", 1.0), "1");
        assert_eq!(f("General", -42.0), "-42");
        assert_eq!(f("General", 12345678901.0), "12345678901"); // 11 digits
        assert_eq!(f("General", 123456789012.0), "1.23457E+11"); // 12 digits
        assert_eq!(f("General", 1e11), "1E+11");
        assert_eq!(f("General", 1e15), "1E+15");
        assert_eq!(f("General", 123456789012345.0), "1.23457E+14");
        assert_eq!(f("General", 99999999999.6), "1E+11");
        assert_eq!(f("General", 12345678901.5), "12345678902");
        assert_eq!(f("General", -123456789012.0), "-1.23457E+11");
        assert_eq!(f("General", 100.0), "100");
    }

    #[test]
    fn general_fractions_and_small_numbers() {
        assert_eq!(f("General", 1.5), "1.5");
        assert_eq!(f("General", -2.5), "-2.5");
        assert_eq!(f("General", 0.1 + 0.2), "0.3");
        assert_eq!(f("General", 1.0 / 3.0), "0.333333333");
        assert_eq!(f("General", 2.0 / 3.0), "0.666666667");
        assert_eq!(f("General", 0.123456789012), "0.123456789");
        assert_eq!(f("General", 123456.789012), "123456.789");
        assert_eq!(f("General", 1234567.891), "1234567.891");
        assert_eq!(f("General", 0.0001), "0.0001");
        assert_eq!(f("General", 0.00012345), "0.00012345");
        assert_eq!(f("General", 0.00001), "1E-05");
        assert_eq!(f("General", 0.000012345), "1.2345E-05");
        assert_eq!(f("General", 1e-10), "1E-10");
        assert_eq!(f("General", 5e-324), "4.94066E-324");
        assert_eq!(f("General", 0.9999999999999), "1");
    }

    #[test]
    fn general_is_the_fallback() {
        assert_eq!(f("", 12.5), "12.5");
        assert_eq!(f("", 123456789012.0), "1.23457E+11");
        assert_eq!(ft("", "abc"), "abc");
        assert_eq!(f("General\" x\"", 5.0), "5 x");
        assert_eq!(f("general", 5.0), "5");
        assert_eq!(f("General;General", -5.0), "5");
        assert_eq!(f("0;[Red]General", -2.5), "2.5");
    }

    // ---- text -----------------------------------------------------------------------------

    #[test]
    fn text_values() {
        assert_eq!(ft("@", "abc"), "abc");
        assert_eq!(ft("\"Name: \"@", "Bob"), "Name: Bob");
        assert_eq!(ft("0.00", "abc"), "abc");
        assert_eq!(ft("General", "abc"), "abc");
        assert_eq!(ft("0.00;0.00;0.00;@@", "ab"), "abab");
        assert_eq!(ft("yyyy/m/d", "text"), "text");
        assert_eq!(ft("@\" kg\"", "5"), "5 kg");
        assert_eq!(ft("0;@", "x"), "x");
    }

    #[test]
    fn at_with_numbers_unverified() {
        // Unverified: a number under a text-only format is shown as General.
        assert_eq!(f("@", 5.0), "5");
        assert_eq!(f("@", 1.5), "1.5");
        assert_eq!(f("@\" kg\"", 5.0), "5 kg");
        assert_eq!(f("0.00;@", 5.0), "5.00");
    }

    // ---- dates ------------------------------------------------------------------------------

    #[test]
    fn date_tokens() {
        assert_eq!(f("yyyy/m/d", D), "2026/10/2");
        assert_eq!(f("yyyy-mm-dd", D), "2026-10-02");
        assert_eq!(f("yy", D), "26");
        assert_eq!(f("y", D), "26");
        assert_eq!(f("m/d/yyyy", D), "10/2/2026");
        assert_eq!(f("d-mmm-yy", D), "2-Oct-26");
        assert_eq!(f("mmm", D), "Oct");
        assert_eq!(f("mmmm", D), "October");
        assert_eq!(f("mmmmm", D), "O");
        assert_eq!(f("ddd", D), "Fri");
        assert_eq!(f("dddd", D), "Friday");
        assert_eq!(f("d", D), "2");
        assert_eq!(f("dd", D), "02");
        assert_eq!(f("mmm-yy", D), "Oct-26");
        assert_eq!(f("YYYY/MM/DD", D), "2026/10/02");
        assert_eq!(f("yyyy\"年\"m\"月\"d\"日\"", D), "2026年10月2日");
        assert_eq!(
            f("yyyy-mm-dd\\Thh:mm:ss", D + t1345()),
            "2026-10-02T13:45:00"
        );
        assert_eq!(f("mmmm d, yyyy", 45000.0), "March 15, 2023");
        assert_eq!(f("yyyy-mm-dd", 45000.0), "2023-03-15");
        assert_eq!(f("yyyy-mm-dd", 44927.0), "2023-01-01");
        assert_eq!(f("dddd", 44927.0), "Sunday");
    }

    #[test]
    fn time_tokens_and_twelve_hour() {
        assert_eq!(f("h:mm:ss", 0.5), "12:00:00");
        assert_eq!(f("h:mm:ss", 0.75), "18:00:00");
        assert_eq!(f("hh:mm", D + t1345()), "13:45");
        assert_eq!(f("h:mm", D + 5.0 / 1440.0), "0:05");
        assert_eq!(f("h:mm AM/PM", D + t1345()), "1:45 PM");
        assert_eq!(f("hh:mm AM/PM", D + t1345()), "01:45 PM");
        assert_eq!(f("h:mm AM/PM", 0.0), "12:00 AM");
        assert_eq!(f("h:mm AM/PM", 0.5), "12:00 PM");
        assert_eq!(f("h:mm AM/PM", 0.5 + 1.0 / 1440.0), "12:01 PM");
        assert_eq!(f("h:mm A/P", 0.75), "6:00 P");
        assert_eq!(f("h:mm a/p", 0.25), "6:00 a");
        assert_eq!(f("h:mm am/pm", 0.75), "6:00 pm");
        assert_eq!(f("h:mm:ss AM/PM", 0.999988425925926), "11:59:59 PM");
        assert_eq!(f("yyyy/mm/dd hh:mm:ss", D + t1345()), "2026/10/02 13:45:00");
    }

    #[test]
    fn month_versus_minute() {
        // Minutes right after h or right before s; month otherwise.
        assert_eq!(f("mm", D), "10");
        assert_eq!(f("m", D), "10");
        assert_eq!(f("yyyy-mm", D), "2026-10");
        assert_eq!(f("mm:ss", 90.0 / 86400.0), "01:30");
        assert_eq!(f("m:ss", 90.0 / 86400.0), "1:30");
        assert_eq!(f("h:m", D + t1345() + 5.0 / 1440.0), "13:50");
        assert_eq!(f("hh:mm:ss", 3725.0 / 86400.0), "01:02:05");
        assert_eq!(f("m/d h:mm", D + t1345()), "10/2 13:45");
        assert_eq!(f("h\"時\"mm\"分\"", D + t1345()), "13時45分");
        assert_eq!(f("mmm d h:mm", D + t1345()), "Oct 2 13:45");
        assert_eq!(f("yyyy-mm-dd hh:mm", D + t1345()), "2026-10-02 13:45");
    }

    #[test]
    fn fractional_seconds() {
        assert_eq!(f("ss.0", 1.5 / 86400.0), "01.5");
        assert_eq!(f("ss.00", 1.25 / 86400.0), "01.25");
        assert_eq!(f("mm:ss.0", 61.5 / 86400.0), "01:01.5");
        assert_eq!(f("mmss.0", 61.5 / 86400.0), "0101.5"); // ECMA id 47
        assert_eq!(f("h:mm:ss.000", 0.5 + 0.123 / 86400.0), "12:00:00.123");
    }

    #[test]
    fn seconds_are_rounded_and_carry() {
        // Whole-second formats round the time to the nearest second (unverified against Excel).
        assert_eq!(f("hh:mm:ss", 0.9999999), "00:00:00");
        assert_eq!(
            f("yyyy-mm-dd hh:mm:ss", D + 0.9999999),
            "2026-10-03 00:00:00"
        );
        assert_eq!(f("hh:mm:ss", 0.5 + 0.4 / 86400.0), "12:00:00");
        assert_eq!(f("hh:mm:ss", 0.5 + 0.6 / 86400.0), "12:00:01");
    }

    #[test]
    fn elapsed_time() {
        assert_eq!(f("[h]:mm", 1.5), "36:00");
        assert_eq!(f("[h]:mm:ss", 1.0 + 1.0 / 24.0), "25:00:00");
        assert_eq!(f("[hh]:mm", 0.5), "12:00");
        assert_eq!(f("[h]:mm", 0.0), "0:00");
        // Microsoft: `[mm]:ss` shows 62:16, `[ss].00` shows 3735.80.
        assert_eq!(f("[mm]:ss", (62.0 * 60.0 + 16.0) / 86400.0), "62:16");
        assert_eq!(f("[ss].00", 3735.8 / 86400.0), "3735.80");
        assert_eq!(f("[s]", 90.0 / 86400.0), "90");
        assert_eq!(f("[m]", 90.0 / 86400.0), "1");
        assert_eq!(f("[h]:mm:ss", 46.0 / 24.0 + 5.0 / 86400.0), "46:00:05");
        assert_eq!(f("[h]:mm", -0.5), "########");
        assert_eq!(f("[h]:mm", 100000.0), "2400000:00");
    }

    #[test]
    fn excel_1900_leap_year_bug() {
        assert_eq!(f("yyyy-mm-dd", 0.0), "1900-01-00");
        assert_eq!(f("yyyy-mm-dd", 1.0), "1900-01-01");
        assert_eq!(f("yyyy-mm-dd", 31.0), "1900-01-31");
        assert_eq!(f("yyyy-mm-dd", 32.0), "1900-02-01");
        assert_eq!(f("yyyy-mm-dd", 59.0), "1900-02-28");
        assert_eq!(f("yyyy-mm-dd", 60.0), "1900-02-29");
        assert_eq!(f("yyyy-mm-dd", 61.0), "1900-03-01");
        assert_eq!(f("yyyy-mm-dd", 62.0), "1900-03-02");
        assert_eq!(f("yyyy-mm-dd", 366.0), "1900-12-31");
        assert_eq!(f("yyyy-mm-dd", 367.0), "1901-01-01");
        // Excel's weekday: serial 1 is a Sunday; serial 60 a Wednesday.
        assert_eq!(f("dddd", 0.0), "Saturday");
        assert_eq!(f("dddd", 1.0), "Sunday");
        assert_eq!(f("dddd", 59.0), "Tuesday");
        assert_eq!(f("dddd", 60.0), "Wednesday");
        assert_eq!(f("dddd", 61.0), "Thursday");
    }

    #[test]
    fn date_1904_system() {
        assert_eq!(f1904("yyyy-mm-dd", 0.0), "1904-01-01");
        assert_eq!(f1904("yyyy-mm-dd", 1.0), "1904-01-02");
        assert_eq!(f1904("yyyy-mm-dd", 1461.0), "1908-01-01");
        assert_eq!(f1904("yyyy-mm-dd", 59.0), "1904-02-29");
        assert_eq!(f1904("dddd", 0.0), "Friday");
        // 1900-system 45000 is 2023-03-15; the same date in 1904 is 1462 lower.
        assert_eq!(f1904("yyyy-mm-dd", 45000.0 - 1462.0), "2023-03-15");
        assert_eq!(f1904("yyyy-mm-dd", -1.0), "########");
    }

    #[test]
    fn invalid_dates_show_hashes() {
        assert_eq!(f("yyyy-mm-dd", -1.0), "########");
        assert_eq!(f("yyyy-mm-dd", 3_000_000.0), "########");
        assert_eq!(f("yyyy-mm-dd", 2_958_465.0), "9999-12-31");
        assert_eq!(f("yyyy-mm-dd", 1e300), "########");
    }

    // ---- Japanese -----------------------------------------------------------------------------

    #[test]
    fn japanese_names() {
        assert_eq!(fj("mmm", D), "10月");
        assert_eq!(fj("mmmm", D), "10月");
        assert_eq!(fj("mmmmm", D), "10");
        assert_eq!(fj("ddd", D), "金");
        assert_eq!(fj("dddd", D), "金曜日");
        assert_eq!(f("aaa", D), "金");
        assert_eq!(f("aaaa", D), "金曜日");
        assert_eq!(fj("aaa", 44927.0), "日");
        assert_eq!(f("aaaa", 44928.0), "月曜日");
        // An explicit locale id wins over the option.
        assert_eq!(fj("[$-409]mmm", D), "Oct");
        assert_eq!(f("[$-411]mmm", D), "10月");
        assert_eq!(f("[$-ja-JP]dddd", D), "金曜日");
        assert_eq!(
            f("[$-411]yyyy\"年\"m\"月\"d\"日\" aaaa", D),
            "2026年10月2日 金曜日"
        );
    }

    #[test]
    fn japanese_eras_and_boundaries() {
        let g = |iso: &str| f("ggge\"年\"m\"月\"d\"日\"", ser(iso));
        assert_eq!(g("2026-10-02"), "令和8年10月2日");
        assert_eq!(g("2019-05-01"), "令和1年5月1日");
        assert_eq!(g("2019-04-30"), "平成31年4月30日");
        assert_eq!(g("1989-01-08"), "平成1年1月8日");
        assert_eq!(g("1989-01-07"), "昭和64年1月7日");
        assert_eq!(g("1926-12-25"), "昭和1年12月25日");
        assert_eq!(g("1926-12-24"), "大正15年12月24日");
        assert_eq!(g("1912-07-30"), "大正1年7月30日");
        assert_eq!(g("1912-07-29"), "明治45年7月29日");
        // Excel cannot represent dates before 1900, so Meiji is only reachable from 1900.
        assert_eq!(f("ggge", 1.0), "明治33");
        assert_eq!(f("ggge", -1.0), "########");
        assert_eq!(f("ge.m.d", ser("2026-10-02")), "R8.10.2");
        assert_eq!(f("ge.m.d", ser("1989-01-07")), "S64.1.7");
        assert_eq!(f("ge.m.d", ser("1912-07-29")), "M45.7.29");
        assert_eq!(f("ge.m.d", ser("1912-07-30")), "T1.7.30");
        assert_eq!(f("gge", ser("2019-05-01")), "令1");
        assert_eq!(f("ggee", ser("2019-05-01")), "令01");
        assert_eq!(f("GGGE", ser("2019-05-01")), "令和1");
        assert_eq!(f("ggge-m-d", ser("2019-05-01")), "令和1-5-1");
    }

    #[test]
    fn roc_year_in_zh_tw_locale() {
        assert_eq!(f("[$-404]e/m/d", D), "115/10/2");
    }

    #[test]
    fn date_in_negative_and_text_sections() {
        assert_eq!(f("yyyy-mm-dd;@", D), "2026-10-02");
        assert_eq!(ft("yyyy-mm-dd;@", "x"), "x");
        // A negative value uses section 2 with its absolute value (serial 1 = 1900; unverified).
        assert_eq!(f("yyyy-mm-dd;yyyy", -1.0), "1900");
    }

    // ---- built-in formats -------------------------------------------------------------------

    #[test]
    fn builtin_table_matches_ecma() {
        use Locale::*;
        for l in [En, Ja] {
            assert_eq!(builtin_format_code(0, l), Some("General"));
            assert_eq!(builtin_format_code(1, l), Some("0"));
            assert_eq!(builtin_format_code(2, l), Some("0.00"));
            assert_eq!(builtin_format_code(3, l), Some("#,##0"));
            assert_eq!(builtin_format_code(4, l), Some("#,##0.00"));
            assert_eq!(builtin_format_code(9, l), Some("0%"));
            assert_eq!(builtin_format_code(10, l), Some("0.00%"));
            assert_eq!(builtin_format_code(11, l), Some("0.00E+00"));
            assert_eq!(builtin_format_code(12, l), Some("# ?/?"));
            assert_eq!(builtin_format_code(13, l), Some("# ??/??"));
            assert_eq!(builtin_format_code(15, l), Some("d-mmm-yy"));
            assert_eq!(builtin_format_code(16, l), Some("d-mmm"));
            assert_eq!(builtin_format_code(17, l), Some("mmm-yy"));
            assert_eq!(builtin_format_code(18, l), Some("h:mm AM/PM"));
            assert_eq!(builtin_format_code(19, l), Some("h:mm:ss AM/PM"));
            assert_eq!(builtin_format_code(20, l), Some("h:mm"));
            assert_eq!(builtin_format_code(21, l), Some("h:mm:ss"));
            assert_eq!(builtin_format_code(45, l), Some("mm:ss"));
            assert_eq!(builtin_format_code(46, l), Some("[h]:mm:ss"));
            assert_eq!(builtin_format_code(47, l), Some("mmss.0"));
            assert_eq!(builtin_format_code(48, l), Some("##0.0E+0"));
            assert_eq!(builtin_format_code(49, l), Some("@"));
            for id in [23, 24, 25, 26, 59, 60, 81, 164, 1000] {
                assert_eq!(builtin_format_code(id, l), None, "id {id}");
            }
        }
        assert_eq!(builtin_format_code(14, Locale::En), Some("m/d/yyyy"));
        assert_eq!(builtin_format_code(14, Locale::Ja), Some("yyyy/m/d"));
        assert_eq!(builtin_format_code(22, Locale::En), Some("m/d/yyyy h:mm"));
        assert_eq!(builtin_format_code(22, Locale::Ja), Some("yyyy/m/d h:mm"));
        assert_eq!(builtin_format_code(30, Locale::Ja), Some("m/d/yy"));
        assert_eq!(builtin_format_code(36, Locale::Ja), Some("[$-411]ge.m.d"));
        assert_eq!(builtin_format_code(27, Locale::En), Some("[$-411]ge.m.d"));
    }

    #[test]
    fn every_builtin_code_parses_and_formats() {
        for l in [Locale::En, Locale::Ja] {
            for id in 0..=60 {
                let Some(code) = builtin_format_code(id, l) else {
                    continue;
                };
                assert!(parse_code(code).is_some(), "id {id} {l:?}: {code}");
                let o = Options {
                    date1904: false,
                    locale: l,
                    null_day: None,
                };
                for v in [0.0, 1.5, -2.5, D + t1345(), 1234567.891] {
                    let _ = format_value(code, Value::Number(v), &o);
                }
                let _ = format_value(code, Value::Text("t"), &o);
            }
        }
    }

    #[test]
    fn builtin_numbers_render() {
        let o_ja = Options {
            date1904: false,
            locale: Locale::Ja,
            null_day: None,
        };
        let o_en = Options::default();
        let r = |id: u32, v: f64, o: &Options| {
            format_value(
                builtin_format_code(id, o.locale).unwrap(),
                Value::Number(v),
                o,
            )
        };
        assert_eq!(r(1, 1234.5, &o_en), "1235");
        assert_eq!(r(2, 1234.5, &o_en), "1234.50");
        assert_eq!(r(3, 1234.5, &o_en), "1,235");
        assert_eq!(r(4, 1234.5, &o_en), "1,234.50");
        assert_eq!(r(9, 0.256, &o_en), "26%");
        assert_eq!(r(10, 0.256, &o_en), "25.60%");
        assert_eq!(r(11, 12345.0, &o_en), "1.23E+04");
        assert_eq!(r(12, 0.5, &o_en), " 1/2");
        assert_eq!(r(14, D, &o_en), "10/2/2026");
        assert_eq!(r(14, D, &o_ja), "2026/10/2");
        assert_eq!(r(15, D, &o_en), "2-Oct-26");
        assert_eq!(r(16, D, &o_en), "2-Oct");
        assert_eq!(r(17, D, &o_en), "Oct-26");
        assert_eq!(r(18, D + t1345(), &o_en), "1:45 PM");
        assert_eq!(r(19, D + t1345(), &o_en), "1:45:00 PM");
        assert_eq!(r(20, D + t1345(), &o_en), "13:45");
        assert_eq!(r(21, D + t1345(), &o_en), "13:45:00");
        assert_eq!(r(22, D + t1345(), &o_en), "10/2/2026 13:45");
        assert_eq!(r(22, D + t1345(), &o_ja), "2026/10/2 13:45");
        assert_eq!(r(45, 90.0 / 86400.0, &o_en), "01:30");
        assert_eq!(r(46, 1.5, &o_en), "36:00:00");
        assert_eq!(r(47, 61.5 / 86400.0, &o_en), "0101.5");
        assert_eq!(r(48, 12345.0, &o_en), "12.3E+3");
        assert_eq!(r(49, 5.0, &o_en), "5");
        // 37-40: parenthesised negatives, optional red.
        assert_eq!(r(37, 1234.0, &o_en), "1,234 ");
        assert_eq!(r(37, -1234.0, &o_en), "(1,234)");
        assert_eq!(r(38, -1234.0, &o_en), "(1,234)");
        assert_eq!(r(39, 1234.5, &o_en), "1,234.50 ");
        assert_eq!(r(40, -1234.5, &o_en), "(1,234.50)");
    }

    #[test]
    fn builtin_currency_and_cjk_ids() {
        let o_ja = Options {
            date1904: false,
            locale: Locale::Ja,
            null_day: None,
        };
        let o_en = Options::default();
        let r = |id: u32, v: f64, o: &Options| {
            format_value(
                builtin_format_code(id, o.locale).unwrap(),
                Value::Number(v),
                o,
            )
        };
        assert_eq!(r(5, 1000.0, &o_ja), "¥1,000");
        assert_eq!(r(6, 1000.0, &o_ja), "¥1,000");
        assert_eq!(r(7, 1000.5, &o_ja), "¥1,000.50");
        assert_eq!(r(8, 1000.5, &o_ja), "¥1,000.50");
        assert_eq!(r(5, -1000.0, &o_ja), "¥-1,000");
        assert_eq!(r(5, 1000.0, &o_en), "$1,000 ");
        assert_eq!(r(5, -1000.0, &o_en), "($1,000)");
        assert_eq!(r(7, 1000.5, &o_en), "$1,000.50 ");
        let ids = |id: u32| r(id, D + t1345(), &o_ja);
        assert_eq!(ids(27), "R8.10.2");
        assert_eq!(ids(28), "令和8年10月2日");
        assert_eq!(ids(29), "令和8年10月2日");
        assert_eq!(ids(30), "10/2/26");
        assert_eq!(ids(31), "2026年10月2日");
        assert_eq!(ids(32), "13時45分");
        assert_eq!(ids(33), "13時45分00秒");
        assert_eq!(ids(34), "2026年10月");
        assert_eq!(ids(35), "10月2日");
        assert_eq!(ids(36), "R8.10.2");
        assert_eq!(ids(50), "R8.10.2");
        assert_eq!(ids(51), "令和8年10月2日");
        assert_eq!(ids(52), "2026年10月");
        assert_eq!(ids(53), "10月2日");
        assert_eq!(ids(54), "令和8年10月2日");
        assert_eq!(ids(55), "2026年10月");
        assert_eq!(ids(56), "10月2日");
        assert_eq!(ids(57), "R8.10.2");
        assert_eq!(ids(58), "令和8年10月2日");
        // 41-44 (accounting): digits with alignment padding; zero shows a dash.
        assert_eq!(r(41, 1234.0, &o_en).trim(), "1,234");
        assert_eq!(r(41, 0.0, &o_en).trim(), "-");
        assert_eq!(r(43, 0.0, &o_en).trim(), "-");
        assert_eq!(r(42, 1234.0, &o_en).replace(' ', ""), "$1,234");
        assert_eq!(r(44, 1234.5, &o_ja).replace(' ', ""), "¥1,234.50");
        assert_eq!(r(41, -1234.0, &o_en).replace(' ', ""), "(1,234)");
        assert_eq!(r(41, -1234.0, &o_ja).replace(' ', ""), "-1,234");
    }

    // ---- ISO 8601 -------------------------------------------------------------------------------

    #[test]
    fn iso_years_outside_1_to_9999_are_not_dates() {
        for s in [
            "99999999999999999-01-01",
            "-99999999999999999-01-01",
            "9223372036854775807-12-31T00:00:00",
            "10000-01-01",
            "0000-01-01",
            "-0001-01-01",
        ] {
            assert_eq!(iso_datetime_to_serial(s), None, "{s}");
            assert_eq!(iso_datetime_to_ods_serial(s, 0), None, "{s}");
            assert_eq!(iso_days_since_1970(s), None, "{s}");
        }
        assert!(iso_days_since_1970("0001-01-01").is_some());
        assert!(iso_days_since_1970("9999-12-31").is_some());
    }

    #[test]
    fn iso_datetime() {
        assert_eq!(iso_datetime_to_serial("2026-10-02"), Some(46297.0));
        assert_eq!(iso_datetime_to_serial("2023-03-15"), Some(45000.0));
        assert_eq!(iso_datetime_to_serial("2023-03-15T00:00:00"), Some(45000.0));
        assert_eq!(iso_datetime_to_serial("1900-03-01"), Some(61.0));
        assert_eq!(iso_datetime_to_serial("1900-02-28"), Some(59.0));
        assert_eq!(iso_datetime_to_serial("1900-01-01"), Some(1.0));
        assert_eq!(iso_datetime_to_serial("1899-12-31"), Some(0.0));
        assert_eq!(iso_datetime_to_serial("1970-01-01"), Some(25569.0));
        let v = iso_datetime_to_serial("2026-10-02T13:45:00").unwrap();
        assert!((v - (46297.0 + t1345())).abs() < 1e-9);
        let v = iso_datetime_to_serial("2026-10-02T13:45:00.500").unwrap();
        assert!((v - (46297.0 + t1345() + 0.5 / 86400.0)).abs() < 1e-9);
        assert_eq!(
            iso_datetime_to_serial("2026-10-02T13:45:00Z"),
            iso_datetime_to_serial("2026-10-02T13:45:00")
        );
        assert_eq!(
            iso_datetime_to_serial("2026-10-02T13:45:00+09:00"),
            iso_datetime_to_serial("2026-10-02T13:45:00")
        );
        assert_eq!(
            iso_datetime_to_serial("2026-10-02T13:45"),
            iso_datetime_to_serial("2026-10-02T13:45:00")
        );
        for bad in [
            "",
            "abc",
            "2026-13-01",
            "2026-10",
            "2026-10-02T25:00:00",
            "2026-10-02T13:61:00",
            "2026/10/02",
            "2026-10-02-01",
        ] {
            assert_eq!(iso_datetime_to_serial(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn iso_serial_roundtrips_through_format() {
        for (iso, shown) in [
            ("2026-10-02T13:45:00", "2026-10-02 13:45:00"),
            ("2000-02-29T23:59:59", "2000-02-29 23:59:59"),
        ] {
            assert_eq!(f("yyyy-mm-dd hh:mm:ss", ser(iso)), shown);
        }
    }

    #[test]
    fn iso_duration() {
        let day = |s: &str| iso_duration_to_serial(s).unwrap();
        assert!((day("PT13H45M00S") - t1345()).abs() < 1e-12);
        assert!((day("P1DT2H") - (1.0 + 2.0 / 24.0)).abs() < 1e-12);
        assert!((day("PT36H") - 1.5).abs() < 1e-12);
        assert!((day("PT0.5S") - 0.5 / 86400.0).abs() < 1e-15);
        assert!((day("-PT1H") + 1.0 / 24.0).abs() < 1e-12);
        assert!((day("P2D") - 2.0).abs() < 1e-12);
        assert!((day("PT1M") - 1.0 / 1440.0).abs() < 1e-12);
        for bad in ["", "P", "PT", "P1Y", "P1M", "PT5", "13:45", "PTxH", "P1H"] {
            assert_eq!(iso_duration_to_serial(bad), None, "{bad:?}");
        }
        assert_eq!(f("[h]:mm:ss", day("PT36H")), "36:00:00");
        assert_eq!(f("h:mm", day("PT13H45M")), "13:45");
    }

    // ---- robustness ------------------------------------------------------------------------------

    #[test]
    fn broken_codes_never_panic_on_edge_values() {
        let values = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            1e-300,
            5e-324,
            f64::MIN_POSITIVE,
            1e15,
            1e300,
            f64::MAX,
            f64::MIN,
            f64::NAN,
            f64::INFINITY,
            45000.123456,
            -45000.5,
            2_958_465.999999,
            1e9,
        ];
        let codes = [
            "0.00",
            "#,##0",
            "0%",
            "0.00E+00",
            "# ?/?",
            "?/100",
            "yyyy-mm-dd hh:mm:ss.000",
            "[h]:mm",
            "General",
            "@",
            "0.0,,",
            "ggge",
            "\"",
            "[",
            "]",
            "0;0;0;0;0",
            "E+",
            "0E+",
            ".",
            ",",
            "0.",
            "/",
            "# /",
            "?/",
            "/?",
            "0/0/0",
            "[$",
            "[$-",
            "0.0.0",
            "_",
            "*",
            "\\",
            "[<",
            "[>=",
            "ss.",
            "[ss].",
            "AM/PM",
            "A/P",
            "e-",
            "ee+",
            "bbbb",
            "0 0/0 0",
            "#,,,",
            "0,,,0",
            "00000000000000000000.00000000000000000000",
        ];
        for c in codes {
            for v in values {
                for o in [
                    Options::default(),
                    Options {
                        date1904: true,
                        locale: Locale::Ja,
                        null_day: None,
                    },
                ] {
                    let _ = format_value(c, Value::Number(v), &o);
                }
            }
            let _ = format_value(c, Value::Text("t"), &Options::default());
        }
    }

    #[test]
    fn exhaustive_short_codes_never_panic() {
        let alphabet = [
            "0", "#", "?", ".", ",", "%", "E+", "e-", "/", "\"", "\\", "_", "*", "[", "]", ";",
            "<", ">", "=", "$", "@", "y", "m", "d", "h", "s", "g", "e", "a", "b", "AM/PM", "A/P",
            "1", "4", " ", "(", ")", "-", ":", "Red", "$-411", "¥", "年", "General", "[h]", "[ss]",
            ".0",
        ];
        let values = [0.0, 1.5, -2.25, 45000.75, 1e300, 1e-7];
        let o = Options {
            date1904: false,
            locale: Locale::Ja,
            null_day: None,
        };
        let mut code = String::new();
        let run = |code: &str| {
            for v in values {
                let _ = format_value(code, Value::Number(v), &o);
            }
            let _ = format_value(code, Value::Text("t"), &o);
        };
        for a in alphabet {
            run(a);
            for b in alphabet {
                code.clear();
                code.push_str(a);
                code.push_str(b);
                run(&code);
                for c in alphabet {
                    code.clear();
                    code.push_str(a);
                    code.push_str(b);
                    code.push_str(c);
                    run(&code);
                }
            }
        }
        // Length 4 over a reduced alphabet of structurally interesting tokens.
        let small = [
            "0", "#", ".", ",", "E+", "/", "\"", "[", "]", ";", "m", "s", "h", "@", "%", "g", "e",
        ];
        for a in small {
            for b in small {
                for c in small {
                    for d in small {
                        code.clear();
                        for s in [a, b, c, d] {
                            code.push_str(s);
                        }
                        run(&code);
                    }
                }
            }
        }
    }

    #[test]
    fn iso_functions_never_panic() {
        let alphabet = [
            "0", "1", "9", "-", ":", "T", "P", "D", "H", "M", "S", ".", "Z", "+", "W", "Y", " ",
            "é",
        ];
        let mut s = String::new();
        for a in alphabet {
            for b in alphabet {
                for c in alphabet {
                    for d in alphabet {
                        s.clear();
                        for x in [a, b, c, d] {
                            s.push_str(x);
                        }
                        let _ = iso_datetime_to_serial(&s);
                        let _ = iso_duration_to_serial(&s);
                    }
                }
            }
        }
        for s in [
            "9999-12-31T23:59:59",
            "0000-01-01",
            "-0001-01-01",
            "99999999999999999999-01-01",
            "PT99999999999999999999H",
        ] {
            let _ = iso_datetime_to_serial(s);
            let _ = iso_duration_to_serial(s);
        }
    }
    // -----------------------------------------------------------------------------------
    // limits on crafted codes, and the compiled form
    // -----------------------------------------------------------------------------------

    #[test]
    fn a_code_over_255_characters_is_general_and_255_is_not() {
        let ok = format!("0.{}", "0".repeat(MAX_CODE_CHARS - 2));
        assert_eq!(ok.chars().count(), MAX_CODE_CHARS);
        assert_ne!(f(&ok, 1.5), f("General", 1.5));
        let long = format!("0.{}", "0".repeat(MAX_CODE_CHARS - 1));
        assert_eq!(long.chars().count(), MAX_CODE_CHARS + 1);
        assert_eq!(f(&long, 1.5), f("General", 1.5));
        // A million digits: no giant string, just General.
        let huge = format!("0.{}", "0".repeat(1_000_000));
        assert_eq!(f(&huge, 1.5), "1.5");
        assert_eq!(f(&huge, -2.0), "-2");
        // Characters, not bytes: 255 two-byte characters are within the limit.
        let wide = format!("\"{}\"0", "é".repeat(MAX_CODE_CHARS - 4));
        assert_eq!(wide.chars().count(), MAX_CODE_CHARS - 1);
        assert!(f(&wide, 1.0).ends_with('1'));
    }

    #[test]
    fn decimals_are_capped_at_30() {
        let code = format!("0.{}", "0".repeat(100));
        let out = f(&code, 1.0);
        assert_eq!(out, format!("1.{}", "0".repeat(MAX_DECIMALS)));
        // `#` and `?` placeholders count the same.
        let q = f(&format!("0.{}", "?".repeat(100)), 0.5);
        assert!(q.len() <= 2 + MAX_DECIMALS, "{q:?}");
        // The exponent digits of a scientific code are not decimals and survive the cap.
        let sci = format!("0.{}E+00", "0".repeat(100));
        let out = f(&sci, 12345.0);
        assert!(out.ends_with("E+04"), "{out}");
        assert_eq!(
            out.split_once('.')
                .unwrap()
                .1
                .split_once('E')
                .unwrap()
                .0
                .len(),
            MAX_DECIMALS
        );
        // 30 decimals themselves are still honoured.
        let thirty = format!("0.{}", "0".repeat(MAX_DECIMALS));
        assert_eq!(
            f(&thirty, 0.5),
            format!("0.5{}", "0".repeat(MAX_DECIMALS - 1))
        );
    }

    #[test]
    fn output_length_is_bounded_by_the_code_length() {
        // Whatever the code, a cell's text stays proportional to the (<= 255 char) code.
        let long_lit = format!("\"{}\"", "x".repeat(MAX_CODE_CHARS - 2));
        let codes = [
            "0".repeat(MAX_CODE_CHARS),
            format!("0.{}", "0".repeat(MAX_CODE_CHARS - 2)),
            format!("#,##0.{}", "0".repeat(MAX_CODE_CHARS - 6)),
            "?".repeat(MAX_CODE_CHARS),
            long_lit,
            "yyyy-mm-dd ".repeat(23),
            format!("0.{}E+00", "0".repeat(MAX_CODE_CHARS - 6)),
            format!("{}%", "0".repeat(MAX_CODE_CHARS - 1)),
        ];
        for code in codes {
            for v in [
                0.0,
                1.0,
                -1.5,
                123_456_789.987_654_33,
                1e300,
                1e-300,
                45000.5,
            ] {
                let out = f(&code, v);
                assert!(
                    out.chars().count() <= 4 * MAX_CODE_CHARS + 400,
                    "{} chars for a {}-char code at {v}",
                    out.chars().count(),
                    code.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_compiled_code_formats_exactly_like_format_value() {
        let codes = [
            "",
            "General",
            "0",
            "0.00",
            "#,##0.00",
            "0%",
            "0.00E+00",
            "# ?/?",
            "yyyy-mm-dd",
            "h:mm AM/PM",
            "[h]:mm:ss",
            "[>=100]0.0;[<0]\"neg\"0;0",
            "@",
            "0;-0;;@",
            "[Red]0.0",
            "\"a\"0\"b\"",
            "ggge\"年\"m\"月\"d\"日\"",
            "[$¥-411]#,##0",
            "bad[code",
            "0.0.0",
        ];
        let values = [
            Value::Number(0.0),
            Value::Number(1234.5678),
            Value::Number(-0.25),
            Value::Number(45292.75),
            Value::Number(1e21),
            Value::Number(f64::NAN),
            Value::Text("text"),
            Value::Bool(true),
            Value::Error("#N/A"),
        ];
        for code in codes {
            let c = compile(code);
            for opts in [
                Options::default(),
                Options {
                    date1904: true,
                    locale: Locale::Ja,
                    null_day: None,
                },
            ] {
                for v in values {
                    assert_eq!(
                        format_compiled(&c, v, &opts),
                        format_value(code, v, &opts),
                        "{code:?} {v:?}"
                    );
                }
            }
        }
    }
}
