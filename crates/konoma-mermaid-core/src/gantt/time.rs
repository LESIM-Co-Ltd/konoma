//! The calendar a gantt chart needs, and nothing else.
//!
//! konoma has no date dependency (`Cargo.toml` carries neither `chrono` nor `time`), and a gantt
//! chart needs four things a date library would give it: read a date in the author's
//! `dateFormat`, add a duration to it, ask what day of the week it is, and print an axis tick in
//! the author's `axisFormat`. That is small enough to write, and writing it keeps the dependency
//! graph where §8 wants it.
//!
//! # Everything here is civil time
//!
//! No zone, no leap seconds, no daylight saving. mermaid's own dates are naive — `dayjs('
//! 2024-01-01', 'YYYY-MM-DD')` is midnight *somewhere*, and every arithmetic in `ganttDb` is a
//! difference between two such instants — so carrying a zone would add a way to be wrong without
//! adding a way to be right.
//!
//! # The conversion is Howard Hinnant's, unchanged
//!
//! `days_from_civil` / `civil_from_days` are the standard proleptic-Gregorian pair, valid for any
//! year, with no table and no branchy month arithmetic. They are checked here against dates a
//! person can verify by hand — the 2000 leap year, the 1900 non-leap year, month ends,
//! 29 February — rather than against konoma's own answers, which is the whole point of picking an
//! algorithm with a published definition.

/// A moment, in milliseconds since 1970-01-01T00:00:00, civil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Instant(i64);

/// A unit of duration. dayjs's `ManipulateType`, restricted to the ones `parseDuration` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// `y`
    Year,
    /// `M` — **capital**. `m` is minutes, and mixing them up moves a task by a factor of 43,200.
    Month,
    /// `w`
    Week,
    /// `d`
    Day,
    /// `h`
    Hour,
    /// `m`
    Minute,
    /// `s`
    Second,
    /// `ms`
    Millisecond,
}

const MS_PER_SECOND: i64 = 1_000;
const MS_PER_MINUTE: i64 = 60 * MS_PER_SECOND;
const MS_PER_HOUR: i64 = 60 * MS_PER_MINUTE;
/// One day, in milliseconds. Constant because this module has no daylight saving in it.
pub const MS_PER_DAY: i64 = 24 * MS_PER_HOUR;

impl Instant {
    /// From milliseconds since the epoch.
    pub fn from_millis(ms: i64) -> Instant {
        Instant(ms)
    }

    /// Milliseconds since the epoch.
    pub fn millis(self) -> i64 {
        self.0
    }

    /// From a civil date and time.
    pub fn from_civil(y: i64, m: u32, d: u32, hh: u32, mi: u32, ss: u32, ms: u32) -> Instant {
        let days = days_from_civil(y, m, d);
        Instant(
            days * MS_PER_DAY
                + hh as i64 * MS_PER_HOUR
                + mi as i64 * MS_PER_MINUTE
                + ss as i64 * MS_PER_SECOND
                + ms as i64,
        )
    }

    /// The civil date and time: year, month, day, hour, minute, second, millisecond.
    pub fn civil(self) -> (i64, u32, u32, u32, u32, u32, u32) {
        let days = self.0.div_euclid(MS_PER_DAY);
        let mut rest = self.0.rem_euclid(MS_PER_DAY);
        let (y, m, d) = civil_from_days(days);
        let hh = (rest / MS_PER_HOUR) as u32;
        rest %= MS_PER_HOUR;
        let mi = (rest / MS_PER_MINUTE) as u32;
        rest %= MS_PER_MINUTE;
        let ss = (rest / MS_PER_SECOND) as u32;
        (y, m, d, hh, mi, ss, (rest % MS_PER_SECOND) as u32)
    }

    /// Midnight at the start of this instant's day.
    pub fn start_of_day(self) -> Instant {
        Instant(self.0.div_euclid(MS_PER_DAY) * MS_PER_DAY)
    }

    /// The ISO weekday: Monday is 1, Sunday is 7. dayjs's `isoWeekday`.
    pub fn iso_weekday(self) -> u32 {
        // 1970-01-01 was a Thursday, which is 4.
        let days = self.0.div_euclid(MS_PER_DAY);
        (days + 3).rem_euclid(7) as u32 + 1
    }

    /// This instant plus `n` of `unit`. Months and years land on the same day of the month,
    /// clamped to the end of a short one — dayjs's rule, and the reason `31 January + 1M` is
    /// 28 February rather than 3 March.
    pub fn add(self, n: i64, unit: Unit) -> Instant {
        match unit {
            Unit::Millisecond => Instant(self.0 + n),
            Unit::Second => Instant(self.0 + n * MS_PER_SECOND),
            Unit::Minute => Instant(self.0 + n * MS_PER_MINUTE),
            Unit::Hour => Instant(self.0 + n * MS_PER_HOUR),
            Unit::Day => Instant(self.0 + n * MS_PER_DAY),
            Unit::Week => Instant(self.0 + n * 7 * MS_PER_DAY),
            Unit::Month | Unit::Year => {
                let months = if unit == Unit::Year { n * 12 } else { n };
                let (y, m, d, hh, mi, ss, ms) = self.civil();
                let total = y * 12 + (m as i64 - 1) + months;
                let ny = total.div_euclid(12);
                let nm = total.rem_euclid(12) as u32 + 1;
                let nd = d.min(days_in_month(ny, nm));
                Instant::from_civil(ny, nm, nd, hh, mi, ss, ms)
            }
        }
    }

    /// The same, for the fractional values `parseDuration` accepts (`1.5d`).
    ///
    /// A fractional month or year is what dayjs calls a "float add": it takes the whole part the
    /// calendar way and the remainder as a share of the month it lands in.
    pub fn add_f64(self, v: f64, unit: Unit) -> Instant {
        match unit {
            Unit::Month | Unit::Year => {
                let months = if unit == Unit::Year { v * 12.0 } else { v };
                let whole = months.trunc() as i64;
                let base = self.add(whole, Unit::Month);
                let frac = months - whole as f64;
                if frac == 0.0 {
                    return base;
                }
                let next = base.add(if frac > 0.0 { 1 } else { -1 }, Unit::Month);
                let span = (next.0 - base.0) as f64;
                Instant(base.0 + (span * frac.abs()) as i64)
            }
            _ => {
                let ms = match unit {
                    Unit::Millisecond => 1.0,
                    Unit::Second => MS_PER_SECOND as f64,
                    Unit::Minute => MS_PER_MINUTE as f64,
                    Unit::Hour => MS_PER_HOUR as f64,
                    Unit::Day => MS_PER_DAY as f64,
                    Unit::Week => (7 * MS_PER_DAY) as f64,
                    _ => unreachable!("handled above"),
                };
                Instant(self.0 + (v * ms) as i64)
            }
        }
    }
}

/// Days from 1970-01-01 to `y-m-d`, proleptic Gregorian. Howard Hinnant's `days_from_civil`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // 0..=399
    let mp = (m as i64 + 9) % 12; // March is 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // 0..=365
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
    era * 146_097 + doe - 719_468
}

/// The inverse. Howard Hinnant's `civil_from_days`.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // 0..=146096
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0..=399
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp = (5 * doy + 2) / 153; // 0..=11, March is 0
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // 1..=31
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // 1..=12
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// How many days a month has.
pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 30,
    }
}

/// The Gregorian rule: every fourth year, except every hundredth, except every four hundredth.
pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// `parseDuration`: `^(\d+(?:\.\d+)?)([Mdhmswy]|ms)$`.
///
/// Note what the character class **excludes**: there is no `D`, no `H`, no `S`, no `y` in
/// plural. `3D` is not three days upstream, and pretending it is would draw a bar where mermaid
/// draws none.
pub fn parse_duration(text: &str) -> Option<(f64, Unit)> {
    let t = text.trim();
    let digits = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (number, suffix) = t.split_at(digits);
    if number.is_empty() || number.starts_with('.') || number.ends_with('.') {
        return None;
    }
    if number.matches('.').count() > 1 {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    let unit = match suffix {
        "y" => Unit::Year,
        "M" => Unit::Month,
        "w" => Unit::Week,
        "d" => Unit::Day,
        "h" => Unit::Hour,
        "m" => Unit::Minute,
        "s" => Unit::Second,
        "ms" => Unit::Millisecond,
        _ => return None,
    };
    Some((value, unit))
}

/// Reads `text` in the author's `dateFormat`, strictly, with the fallbacks upstream has.
///
/// `dayjs(str, format, true)` first, and — this is the part that is easy to miss — a fall back to
/// `new Date(str)` when that fails, which is what makes an ISO date work in a chart whose
/// `dateFormat` says something else. Without it, mermaid's own `gantt.md` examples do not parse.
pub fn parse(text: &str, format: &str) -> Option<Instant> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let format = format.trim();
    if format == "x" {
        return text.parse::<i64>().ok().map(Instant::from_millis);
    }
    if format == "X" {
        return text
            .parse::<i64>()
            .ok()
            .map(|s| Instant::from_millis(s * 1000));
    }
    if !format.is_empty() {
        if let Some(t) = parse_strict(text, format) {
            return Some(t);
        }
    }
    // `new Date(str)`, for the shapes a browser accepts and a gantt chart actually contains.
    parse_strict(text, "YYYY-MM-DDTHH:mm:ss")
        .or_else(|| parse_strict(text, "YYYY-MM-DD HH:mm:ss"))
        .or_else(|| parse_strict(text, "YYYY-MM-DDTHH:mm"))
        .or_else(|| parse_strict(text, "YYYY-MM-DD HH:mm"))
        .or_else(|| parse_strict(text, "YYYY-MM-DD"))
}

/// One `dateFormat` token.
enum Token {
    Year4,
    Year2,
    Month2,
    Month1,
    MonthShort,
    MonthLong,
    Day2,
    Day1,
    Hour24Two,
    Hour24One,
    Hour12Two,
    Hour12One,
    Minute2,
    Minute1,
    Second2,
    Second1,
    Milli3,
    Meridiem { upper: bool },
    Literal(String),
}

/// Splits a dayjs format string into tokens, longest first so `MM` never lexes as two `M`.
fn tokenise(format: &str) -> Vec<Token> {
    /// A spelling and what it means. Named so the table below reads as a table.
    type Rule = (&'static str, fn() -> Token);
    const TOKENS: &[Rule] = &[
        ("YYYY", || Token::Year4),
        ("YY", || Token::Year2),
        ("MMMM", || Token::MonthLong),
        ("MMM", || Token::MonthShort),
        ("MM", || Token::Month2),
        ("M", || Token::Month1),
        ("DD", || Token::Day2),
        ("D", || Token::Day1),
        ("HH", || Token::Hour24Two),
        ("H", || Token::Hour24One),
        ("hh", || Token::Hour12Two),
        ("h", || Token::Hour12One),
        ("mm", || Token::Minute2),
        ("m", || Token::Minute1),
        ("ss", || Token::Second2),
        ("s", || Token::Second1),
        ("SSS", || Token::Milli3),
        ("A", || Token::Meridiem { upper: true }),
        ("a", || Token::Meridiem { upper: false }),
    ];
    let mut out = Vec::new();
    let mut rest = format;
    'outer: while !rest.is_empty() {
        // `[…]` quotes a literal in dayjs.
        if let Some(after) = rest.strip_prefix('[') {
            if let Some(end) = after.find(']') {
                out.push(Token::Literal(after[..end].to_string()));
                rest = &after[end + 1..];
                continue;
            }
        }
        for (text, make) in TOKENS {
            if rest.starts_with(text) {
                out.push(make());
                rest = &rest[text.len()..];
                continue 'outer;
            }
        }
        let c = rest.chars().next().expect("not empty");
        out.push(Token::Literal(c.to_string()));
        rest = &rest[c.len_utf8()..];
    }
    out
}

const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_LONG: [&str; 12] = [
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
const DAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAYS_LONG: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Strict parsing: every token must match, and nothing may be left over.
fn parse_strict(text: &str, format: &str) -> Option<Instant> {
    let mut rest = text;
    let (mut y, mut mo, mut d) = (1970i64, 1u32, 1u32);
    let (mut hh, mut mi, mut ss, mut ms) = (0u32, 0u32, 0u32, 0u32);
    let mut pm: Option<bool> = None;
    let mut twelve_hour = false;

    for token in tokenise(format) {
        match token {
            Token::Literal(l) => rest = rest.strip_prefix(l.as_str())?,
            Token::Year4 => {
                let (v, r) = take_digits(rest, 4, 4)?;
                y = v as i64;
                rest = r;
            }
            Token::Year2 => {
                let (v, r) = take_digits(rest, 2, 2)?;
                // dayjs: 00–68 is 2000s, 69–99 is 1900s.
                y = if v <= 68 {
                    2000 + v as i64
                } else {
                    1900 + v as i64
                };
                rest = r;
            }
            Token::Month2 => {
                let (v, r) = take_digits(rest, 2, 2)?;
                mo = v;
                rest = r;
            }
            Token::Month1 => {
                let (v, r) = take_digits(rest, 1, 2)?;
                mo = v;
                rest = r;
            }
            Token::MonthShort => {
                let (v, r) = take_name(rest, &MONTHS_SHORT)?;
                mo = v as u32 + 1;
                rest = r;
            }
            Token::MonthLong => {
                let (v, r) = take_name(rest, &MONTHS_LONG)?;
                mo = v as u32 + 1;
                rest = r;
            }
            Token::Day2 => {
                let (v, r) = take_digits(rest, 2, 2)?;
                d = v;
                rest = r;
            }
            Token::Day1 => {
                let (v, r) = take_digits(rest, 1, 2)?;
                d = v;
                rest = r;
            }
            Token::Hour24Two => {
                let (v, r) = take_digits(rest, 2, 2)?;
                hh = v;
                rest = r;
            }
            Token::Hour24One => {
                let (v, r) = take_digits(rest, 1, 2)?;
                hh = v;
                rest = r;
            }
            Token::Hour12Two => {
                let (v, r) = take_digits(rest, 2, 2)?;
                hh = v;
                twelve_hour = true;
                rest = r;
            }
            Token::Hour12One => {
                let (v, r) = take_digits(rest, 1, 2)?;
                hh = v;
                twelve_hour = true;
                rest = r;
            }
            Token::Minute2 => {
                let (v, r) = take_digits(rest, 2, 2)?;
                mi = v;
                rest = r;
            }
            Token::Minute1 => {
                let (v, r) = take_digits(rest, 1, 2)?;
                mi = v;
                rest = r;
            }
            Token::Second2 => {
                let (v, r) = take_digits(rest, 2, 2)?;
                ss = v;
                rest = r;
            }
            Token::Second1 => {
                let (v, r) = take_digits(rest, 1, 2)?;
                ss = v;
                rest = r;
            }
            Token::Milli3 => {
                let (v, r) = take_digits(rest, 3, 3)?;
                ms = v;
                rest = r;
            }
            Token::Meridiem { .. } => {
                let head = rest.get(..2)?;
                pm = match head.to_ascii_uppercase().as_str() {
                    "AM" => Some(false),
                    "PM" => Some(true),
                    _ => return None,
                };
                rest = &rest[2..];
            }
        }
    }
    if !rest.is_empty() {
        return None;
    }
    // Strict means the date has to exist: `2024-02-31` is not a date.
    if mo == 0 || mo > 12 || d == 0 || d > days_in_month(y, mo) || mi > 59 || ss > 59 {
        return None;
    }
    if twelve_hour {
        if hh == 0 || hh > 12 {
            return None;
        }
        hh %= 12;
        if pm == Some(true) {
            hh += 12;
        }
    } else if hh > 23 {
        return None;
    }
    Some(Instant::from_civil(y, mo, d, hh, mi, ss, ms))
}

fn take_digits(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.chars().take(max).take_while(char::is_ascii_digit).count();
    if n < min {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn take_name<'a>(s: &'a str, names: &[&str]) -> Option<(usize, &'a str)> {
    for (i, name) in names.iter().enumerate() {
        if s.len() >= name.len() && s[..name.len()].eq_ignore_ascii_case(name) {
            return Some((i, &s[name.len()..]));
        }
    }
    None
}

/// Prints `at` with a d3 `timeFormat` specifier — what `axisFormat` is written in.
///
/// The default when the source declares none is `%Y-%m-%d`, which is d3's default for a day-scale
/// axis and what mermaid shows.
pub fn format(at: Instant, spec: &str) -> String {
    let spec = if spec.trim().is_empty() {
        "%Y-%m-%d"
    } else {
        spec.trim()
    };
    let (y, mo, d, hh, mi, ss, ms) = at.civil();
    let wd = at.iso_weekday() as usize - 1;
    let mut out = String::new();
    let mut chars = spec.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // `%-d` and `%_d` drop d3's zero and space padding.
        let mut pad = true;
        if matches!(chars.peek(), Some('-') | Some('_')) {
            pad = false;
            chars.next();
        }
        let Some(f) = chars.next() else {
            out.push('%');
            break;
        };
        match f {
            'Y' => out.push_str(&y.to_string()),
            'y' => out.push_str(&two(y.rem_euclid(100) as u32, pad)),
            'm' => out.push_str(&two(mo, pad)),
            'd' => out.push_str(&two(d, pad)),
            'e' => out.push_str(&if pad { format!("{d:2}") } else { d.to_string() }),
            'H' => out.push_str(&two(hh, pad)),
            'I' => out.push_str(&two(if hh % 12 == 0 { 12 } else { hh % 12 }, pad)),
            'M' => out.push_str(&two(mi, pad)),
            'S' => out.push_str(&two(ss, pad)),
            'L' => out.push_str(&format!("{ms:03}")),
            'p' => out.push_str(if hh < 12 { "AM" } else { "PM" }),
            'b' | 'h' => out.push_str(MONTHS_SHORT[(mo as usize - 1).min(11)]),
            'B' => out.push_str(MONTHS_LONG[(mo as usize - 1).min(11)]),
            'a' => out.push_str(DAYS_SHORT[wd]),
            'A' => out.push_str(DAYS_LONG[wd]),
            'j' => out.push_str(&format!(
                "{:03}",
                days_from_civil(y, mo, d) - days_from_civil(y, 1, 1) + 1
            )),
            'w' => out.push_str(&(at.iso_weekday() % 7).to_string()),
            '%' => out.push('%'),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

fn two(v: u32, pad: bool) -> String {
    if pad {
        format!("{v:02}")
    } else {
        v.to_string()
    }
}

/// `ganttDb.isInvalidDate` — whether a day is excluded from a task's span.
pub fn is_excluded(at: Instant, excludes: &[String], includes: &[String], weekend: &str) -> bool {
    let date_only = format(at, "%Y-%m-%d");
    if includes.contains(&date_only) {
        return false;
    }
    if excludes.iter().any(|e| e == "weekends") {
        // `WEEKEND_START_DAY = { friday: 5, saturday: 6 }`, in ISO weekdays.
        let start = if weekend == "friday" { 5 } else { 6 };
        let wd = at.iso_weekday();
        if wd == start || wd == start + 1 {
            return true;
        }
    }
    let name = DAYS_LONG[at.iso_weekday() as usize - 1].to_ascii_lowercase();
    if excludes.contains(&name) {
        return true;
    }
    excludes.contains(&date_only)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dates a person can check by hand, against the algorithm's published definition.
    #[test]
    fn the_civil_conversion_round_trips_the_awkward_dates() {
        for (y, m, d) in [
            (1970, 1, 1),
            (1969, 12, 31),
            (2000, 2, 29),
            (1900, 3, 1),
            (2024, 2, 29),
            (2023, 12, 31),
            (2100, 3, 1),
            (1, 1, 1),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        // 2000 is a leap year, 1900 is not.
        assert_eq!(
            days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 1),
            29
        );
        assert_eq!(
            days_from_civil(1900, 3, 1) - days_from_civil(1900, 2, 1),
            28
        );
    }

    #[test]
    fn the_weekday_matches_the_calendar() {
        // 1970-01-01 was a Thursday; 2024-01-01 a Monday; 2024-01-06 a Saturday.
        assert_eq!(Instant::from_civil(1970, 1, 1, 0, 0, 0, 0).iso_weekday(), 4);
        assert_eq!(Instant::from_civil(2024, 1, 1, 0, 0, 0, 0).iso_weekday(), 1);
        assert_eq!(Instant::from_civil(2024, 1, 6, 0, 0, 0, 0).iso_weekday(), 6);
        assert_eq!(Instant::from_civil(2024, 1, 7, 0, 0, 0, 0).iso_weekday(), 7);
    }

    #[test]
    fn adding_a_month_clamps_to_the_end_of_a_short_one() {
        let jan31 = Instant::from_civil(2024, 1, 31, 0, 0, 0, 0);
        assert_eq!(jan31.add(1, Unit::Month).civil().0, 2024);
        assert_eq!(jan31.add(1, Unit::Month).civil().1, 2);
        assert_eq!(
            jan31.add(1, Unit::Month).civil().2,
            29,
            "2024 is a leap year"
        );
        let jan31_2023 = Instant::from_civil(2023, 1, 31, 0, 0, 0, 0);
        assert_eq!(jan31_2023.add(1, Unit::Month).civil().2, 28);
    }

    #[test]
    fn adding_days_and_weeks_is_exact() {
        let d = Instant::from_civil(2024, 2, 27, 0, 0, 0, 0);
        assert_eq!(d.add(3, Unit::Day).civil(), (2024, 3, 1, 0, 0, 0, 0));
        assert_eq!(d.add(1, Unit::Week).civil(), (2024, 3, 5, 0, 0, 0, 0));
    }

    /// `[Mdhmswy]|ms` — and nothing else. A capital `D` is not days.
    #[test]
    fn only_the_documented_duration_suffixes_are_durations() {
        assert_eq!(parse_duration("3d"), Some((3.0, Unit::Day)));
        assert_eq!(parse_duration("1.5h"), Some((1.5, Unit::Hour)));
        assert_eq!(parse_duration("2M"), Some((2.0, Unit::Month)));
        assert_eq!(parse_duration("2m"), Some((2.0, Unit::Minute)));
        assert_eq!(parse_duration("500ms"), Some((500.0, Unit::Millisecond)));
        assert_eq!(parse_duration("3D"), None);
        assert_eq!(parse_duration("3 d"), None);
        assert_eq!(parse_duration("d"), None);
        assert_eq!(parse_duration("3days"), None);
    }

    #[test]
    fn strict_parsing_refuses_a_date_that_does_not_exist() {
        assert!(parse_strict("2024-02-30", "YYYY-MM-DD").is_none());
        assert!(parse_strict("2023-02-29", "YYYY-MM-DD").is_none());
        assert!(parse_strict("2024-02-29", "YYYY-MM-DD").is_some());
        assert!(parse_strict("2024-13-01", "YYYY-MM-DD").is_none());
        assert!(parse_strict("2024-01-01x", "YYYY-MM-DD").is_none());
    }

    #[test]
    fn the_documented_date_formats_are_read() {
        assert_eq!(
            parse("2014-01-06", "YYYY-MM-DD").map(Instant::civil),
            Some((2014, 1, 6, 0, 0, 0, 0))
        );
        assert_eq!(
            parse("2014-01-06 12:30", "YYYY-MM-DD HH:mm").map(Instant::civil),
            Some((2014, 1, 6, 12, 30, 0, 0))
        );
        assert_eq!(
            parse("06-01-2014", "DD-MM-YYYY").map(Instant::civil),
            Some((2014, 1, 6, 0, 0, 0, 0))
        );
        assert_eq!(
            parse("1700000000", "X").map(|i| i.millis()),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            parse("1700000000000", "x").map(|i| i.millis()),
            Some(1_700_000_000_000)
        );
    }

    /// The fallback upstream has and a strict-only reader does not: an ISO date parses whatever
    /// `dateFormat` says, because `getStartDate` falls through to `new Date(str)`.
    #[test]
    fn an_iso_date_parses_even_when_the_format_says_otherwise() {
        assert_eq!(
            parse("2014-01-06", "DD-MM-YYYY").map(Instant::civil),
            Some((2014, 1, 6, 0, 0, 0, 0))
        );
    }

    #[test]
    fn the_axis_formatter_prints_what_d3_prints() {
        let at = Instant::from_civil(2024, 3, 9, 14, 5, 7, 42);
        assert_eq!(format(at, "%Y-%m-%d"), "2024-03-09");
        assert_eq!(format(at, "%d/%m"), "09/03");
        assert_eq!(format(at, "%b %d"), "Mar 09");
        assert_eq!(format(at, "%B"), "March");
        assert_eq!(format(at, "%a"), "Sat");
        assert_eq!(format(at, "%A"), "Saturday");
        assert_eq!(format(at, "%H:%M:%S"), "14:05:07");
        assert_eq!(format(at, "%-d %-m"), "9 3");
        assert_eq!(format(at, "%L"), "042");
        assert_eq!(format(at, "100%%"), "100%");
        assert_eq!(format(at, ""), "2024-03-09", "the default is d3's");
    }

    #[test]
    fn weekends_are_excluded_from_the_day_the_source_names() {
        let sat = Instant::from_civil(2024, 1, 6, 0, 0, 0, 0);
        let fri = Instant::from_civil(2024, 1, 5, 0, 0, 0, 0);
        let ex = vec!["weekends".to_string()];
        assert!(is_excluded(sat, &ex, &[], "saturday"));
        assert!(!is_excluded(fri, &ex, &[], "saturday"));
        assert!(is_excluded(fri, &ex, &[], "friday"));
        // `includes` wins over `excludes`, which is the whole reason it exists.
        assert!(!is_excluded(
            sat,
            &ex,
            &["2024-01-06".to_string()],
            "saturday"
        ));
    }

    #[test]
    fn a_named_day_and_a_named_date_are_both_excludable() {
        let mon = Instant::from_civil(2024, 1, 1, 0, 0, 0, 0);
        assert!(is_excluded(mon, &["monday".to_string()], &[], "saturday"));
        assert!(is_excluded(
            mon,
            &["2024-01-01".to_string()],
            &[],
            "saturday"
        ));
        assert!(!is_excluded(
            mon,
            &["2024-01-02".to_string()],
            &[],
            "saturday"
        ));
    }
}
