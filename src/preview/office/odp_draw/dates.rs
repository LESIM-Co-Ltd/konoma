//! The current date and time as a field shows them (`text:date` and `text:time` that are not fixed,
//! `presentation:date-time-decl` with `presentation:source="current-date"`), formatted by the
//! `number:date-style` the field names (`style:data-style-name`).
//!
//! LibreOffice re-reads such a field when it draws the page, so the stored text of a field that
//! is not fixed is stale by definition; the field shows the date of the moment the file is opened.
//! A field with `text:fixed="true"` keeps its stored text (the caller's business).
//!
//! * The moment is UTC (the program has no time zone database). Tests set it with
//!   [`set_now_for_tests`] (the default under test is 2026-10-09 12:34:56, a Friday).
//! * Read from a `number:date-style` / `number:time-style`: `number:day`, `number:month`
//!   (`number:textual="true"` is the name), `number:year`, `number:day-of-week`, `number:hours`,
//!   `number:minutes`, `number:seconds`, `number:am-pm` and `number:text`, each with
//!   `number:style="long"` for the long form; other children (`number:era`, `number:quarter`,
//!   `number:week-of-year`) are left out. A style that does not exist gives the ISO date
//!   (`2026-10-09`). The names of months and days are English, or Japanese for
//!   `number:language="ja"`.

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
    moment_of(secs)
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
    let ja = style
        .attr("language")
        .is_some_and(|l| l.trim().eq_ignore_ascii_case("ja"));
    let mut out = String::new();
    for k in style.nodes() {
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
