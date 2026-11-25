//! Calendar formatting of Unix epoch seconds (UTC), with no date dependency.

/// Convert UNIX epoch seconds to "YYYY-MM-DD HH:MM:SS UTC" (calendar math with no extra dependency, Howard Hinnant's method).
pub fn format_epoch_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days: days since 1970-01-01 → calendar date.
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// Short date-time for column display, "YYYY-MM-DD HH:MM" (UTC, no seconds/label).
pub fn format_epoch_short(secs: u64) -> String {
    format_epoch_utc(secs)[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formatters() {
        assert_eq!(format_epoch_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_epoch_utc(1577836800), "2020-01-01 00:00:00 UTC");
        assert_eq!(
            format_epoch_utc(1718323200 + 45296),
            "2024-06-14 12:34:56 UTC"
        );
        assert_eq!(format_epoch_short(1718323200 + 45296), "2024-06-14 12:34");
    }
}
