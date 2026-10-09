//! The current date and time as a field shows them (`text:date` and `text:time` that are not fixed,
//! `presentation:date-time-decl` with `presentation:source="current-date"`), formatted by the
//! `number:date-style` the field names (`style:data-style-name`).
//!
//! LibreOffice re-reads such a field when it draws the page, so the stored text of a field that
//! is not fixed is stale by definition; the field shows the date of the moment the file is opened.
//! A field with `text:fixed="true"` keeps its stored text (the caller's business).
//!
//! * The moment is the LOCAL time (`localtime_r`, as LibreOffice shows it: a user in Japan sees
//!   yesterday's date between 00:00 and 09:00 if the field showed UTC). Tests set it with
//!   [`set_now_for_tests`] (the default under test is 2026-10-09 12:34:56, a Friday).
//! * Read from a `number:date-style` / `number:time-style`: `number:day`, `number:month`
//!   (`number:textual="true"` is the name), `number:year`, `number:day-of-week`, `number:hours`,
//!   `number:minutes`, `number:seconds`, `number:am-pm` and `number:text`, each with
//!   `number:style="long"` for the long form; other children (`number:era`, `number:quarter`,
//!   `number:week-of-year`) are left out. A style that does not exist gives the ISO date
//!   (`2026-10-09`). The names of months and days are English, or Japanese for
//!   `number:language="ja"`.
//! * `number:automatic-order="true"` (a date style): LibreOffice does not keep the order the style
//!   lists but the order of the style's locale (`number:language` / `number:country`; a style
//!   without them follows the system locale, measured: a deck whose own language is Italian still
//!   shows the Japanese form on a Japanese system). Day, month and year change places with the
//!   texts between them staying where they are. For Japanese LibreOffice writes its own short form
//!   `26年10月9日` (two-digit year, no padding; measured on `text-fields.odp`), which we write too.

use crate::preview::office::docx_xml::Node;

/// A moment, as the fields of a calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Moment {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    /// 0 = Monday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// The calendar fields of `secs` seconds since 1970-01-01 00:00:00 UTC.
pub(super) fn moment_of(secs: i64) -> Moment {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // (Howard Hinnant's civil-from-days.)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    Moment {
        year: if month <= 2 { y + 1 } else { y },
        month,
        day,
        weekday: (days + 3).rem_euclid(7) as u32,
        hour: (rem / 3_600) as u32,
        minute: (rem % 3_600 / 60) as u32,
        second: (rem % 60) as u32,
    }
}

#[cfg(test)]
thread_local! {
    static NOW: std::cell::Cell<Option<Moment>> = const { std::cell::Cell::new(None) };
}

/// Sets the moment [`now`] answers on this thread (tests).
#[cfg(test)]
pub(super) fn set_now_for_tests(m: Option<Moment>) {
    NOW.with(|n| n.set(m));
}

/// The moment a current-date field shows.
#[cfg(not(test))]
pub(super) fn now() -> Moment {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    moment_of(secs + local_offset(secs))
}

/// Seconds the local time zone is ahead of UTC at `secs` (0 when the system cannot say).
#[cfg(unix)]
pub(super) fn local_offset(secs: i64) -> i64 {
    // (`time_t` is 64 bits wide on every target konoma supports; where it is narrower the
    // cast wraps, which only matters 68 years from now.)
    #[allow(clippy::unnecessary_cast)]
    let t = secs as libc::time_t;
    // SAFETY: `tm` is plain data that `localtime_r` fills; both pointers are valid for the call.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return 0;
        }
        #[allow(clippy::useless_conversion)] // (`c_long` is narrower on 32-bit targets)
        i64::from(tm.tm_gmtoff)
    }
}

/// Seconds the local time zone is ahead of UTC (no time zone database off Unix: UTC).
#[cfg(all(not(unix), not(test)))]
fn local_offset(_secs: i64) -> i64 {
    0
}

/// The moment a current-date field shows (under test: fixed, so that no result depends on the day
/// the tests run).
#[cfg(test)]
pub(super) fn now() -> Moment {
    NOW.with(|n| n.get()).unwrap_or(Moment {
        year: 2026,
        month: 10,
        day: 9,
        weekday: 4,
        hour: 12,
        minute: 34,
        second: 56,
    })
}

/// A locale: lower-case language and upper-case country (either may be empty).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Locale {
    language: String,
    country: String,
}

/// The locale of a number style: its `number:language` / `number:country`, else the system's.
fn locale_of(style: &Node) -> Locale {
    match style
        .attr("language")
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        Some(l) => Locale {
            language: l.to_ascii_lowercase(),
            country: style
                .attr("country")
                .map(|c| c.trim().to_ascii_uppercase())
                .unwrap_or_default(),
        },
        None => system_locale(),
    }
}

/// The system locale as `ja-JP` / `en_US.UTF-8` says it (English without a country when unknown).
fn system_locale() -> Locale {
    #[cfg(test)]
    let tag = TEST_LOCALE.with(|l| l.borrow().clone());
    #[cfg(not(test))]
    let tag = sys_locale::get_locale();
    let tag = tag.unwrap_or_default();
    let tag = tag.split('.').next().unwrap_or("");
    let mut it = tag.split(['-', '_']);
    let language = it.next().unwrap_or("").to_ascii_lowercase();
    if language.is_empty() || language == "c" || language == "posix" {
        return Locale {
            language: "en".into(),
            country: String::new(),
        };
    }
    Locale {
        language,
        country: it.next().unwrap_or("").to_ascii_uppercase(),
    }
}

#[cfg(test)]
thread_local! {
    static TEST_LOCALE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Sets the system locale tag [`system_locale`] answers on this thread (tests).
#[cfg(test)]
pub(super) fn set_system_locale_for_tests(tag: Option<&str>) {
    TEST_LOCALE.with(|l| *l.borrow_mut() = tag.map(str::to_string));
}

/// Which of day (`D`), month (`M`) and year (`Y`) comes first, second, third.
fn date_order(l: &Locale) -> [char; 3] {
    match l.language.as_str() {
        "ja" | "zh" | "ko" | "hu" | "lt" | "mn" => ['Y', 'M', 'D'],
        "en" if l.country.is_empty() || l.country == "US" || l.country == "PH" => ['M', 'D', 'Y'],
        _ => ['D', 'M', 'Y'],
    }
}

/// Puts the day, month and year elements of `parts` into `order`, each slot keeping its place
/// among the texts.
fn reorder_date(parts: &mut [&Node], order: [char; 3]) {
    let kind = |n: &Node| match n.name.as_str() {
        "day" => Some('D'),
        "month" => Some('M'),
        "year" => Some('Y'),
        _ => None,
    };
    let slots: Vec<usize> = (0..parts.len())
        .filter(|&i| kind(parts[i]).is_some())
        .collect();
    // Only a style with each of the three once is reordered (anything else is the author's own).
    let mut kinds: Vec<char> = slots.iter().filter_map(|&i| kind(parts[i])).collect();
    kinds.sort_unstable();
    if kinds != ['D', 'M', 'Y'] {
        return;
    }
    let by_kind: Vec<&Node> = order
        .iter()
        .filter_map(|&c| slots.iter().map(|&i| parts[i]).find(|n| kind(n) == Some(c)))
        .collect();
    for (&i, n) in slots.iter().zip(by_kind) {
        parts[i] = n;
    }
}

const MONTHS: [&str; 12] = [
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
const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const DAYS_JA: [&str; 7] = ["月", "火", "水", "木", "金", "土", "日"];

fn two(v: u32) -> String {
    format!("{v:02}")
}

/// `m` as the date style `style` writes it (`None`: the ISO date).
pub(super) fn format(style: Option<&Node>, m: &Moment) -> String {
    let Some(style) = style else {
        return format!("{:04}-{:02}-{:02}", m.year, m.month, m.day);
    };
    let loc = locale_of(style);
    let ja = loc.language == "ja";
    let mut parts: Vec<&Node> = style.nodes().collect();
    let auto = style.attr("automatic-order").map(str::trim) == Some("true");
    if auto && style.name == "date-style" {
        if ja {
            // LibreOffice's own Japanese short form (see the module doc).
            return format!("{:02}年{}月{}日", m.year.rem_euclid(100), m.month, m.day);
        }
        reorder_date(&mut parts, date_order(&loc));
    }
    let mut out = String::new();
    for k in parts {
        let long = k.attr("style").map(str::trim) == Some("long");
        match k.name.as_str() {
            "text" => {
                let mut t = String::new();
                super::styles::text_of(k, &mut t, 0);
                out.push_str(&t);
            }
            "day" => out.push_str(&if long { two(m.day) } else { m.day.to_string() }),
            "month" => {
                if k.attr("textual").map(str::trim) == Some("true") {
                    if ja {
                        out.push_str(&format!("{}月", m.month));
                    } else {
                        let name = MONTHS[(m.month as usize - 1).min(11)];
                        out.push_str(&if long {
                            name.to_string()
                        } else {
                            name.chars().take(3).collect()
                        });
                    }
                } else {
                    out.push_str(&if long {
                        two(m.month)
                    } else {
                        m.month.to_string()
                    });
                }
            }
            "year" => out.push_str(&if long {
                format!("{:04}", m.year)
            } else {
                two(m.year.rem_euclid(100) as u32)
            }),
            "day-of-week" => {
                let i = (m.weekday as usize).min(6);
                if ja {
                    out.push_str(DAYS_JA[i]);
                    if long {
                        out.push_str("曜日");
                    }
                } else if long {
                    out.push_str(DAYS[i]);
                } else {
                    out.extend(DAYS[i].chars().take(3));
                }
            }
            "hours" => {
                // (With an AM / PM marker the hours run 1..12.)
                let h = if style.child("am-pm").is_some() {
                    match m.hour % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    m.hour
                };
                out.push_str(&if long { two(h) } else { h.to_string() });
            }
            "minutes" => out.push_str(&if long {
                two(m.minute)
            } else {
                m.minute.to_string()
            }),
            "seconds" => out.push_str(&if long {
                two(m.second)
            } else {
                m.second.to_string()
            }),
            "am-pm" => out.push_str(if m.hour < 12 { "AM" } else { "PM" }),
            _ => {}
        }
    }
    out
}
