//! Number-format engine tests that pin the mutation survivors of the first audit. Expectations
//! come from Microsoft's "Number format codes" article, ECMA-376 Part 1 18.8.30/18.8.31 and
//! (for ISO 8601) ISO 8601 / ODF 1.2 18.3; cases that rest on this engine's documented design
//! choice rather than a primary source say so.

use super::numfmt::*;

fn f(code: &str, v: f64) -> String {
    format_value(code, Value::Number(v), &Options::default())
}
fn fj(code: &str, v: f64) -> String {
    let o = Options {
        date1904: false,
        locale: Locale::Ja,
        null_day: None,
    };
    format_value(code, Value::Number(v), &o)
}
fn f1904(code: &str, v: f64) -> String {
    let o = Options {
        date1904: true,
        locale: Locale::En,
        null_day: None,
    };
    format_value(code, Value::Number(v), &o)
}
fn ser(s: &str) -> f64 {
    iso_datetime_to_serial(s).unwrap()
}
/// 2026-10-02 is a Friday.
const FRI: f64 = 46297.0;

// ---- sections and conditions -------------------------------------------------------------

#[test]
fn a_single_condition_sends_everything_else_to_the_second_section() {
    // Microsoft: a conditional first section; "everything else" goes to the next section. The
    // third section only becomes "else" when *both* of the first two carry a condition
    // (SheetJS SSF, which mirrors Excel, chooses the same way).
    let c = "[>100]\"big\";\"small\";\"other\"";
    assert_eq!(f(c, 150.0), "big");
    assert_eq!(f(c, 5.0), "small");
    let both = "[>100]\"big\";[<0]\"neg\";\"other\"";
    assert_eq!(f(both, 150.0), "big");
    assert_eq!(f(both, -1.0), "-neg");
    assert_eq!(f(both, 5.0), "other");
}

#[test]
fn every_comparison_operator_includes_or_excludes_its_boundary() {
    // (code, value, expected is the first section)
    let cases: &[(&str, f64, bool)] = &[
        ("[<=5]", 5.0, true),
        ("[<=5]", 5.5, false),
        ("[<5]", 5.0, false),
        ("[<5]", 4.0, true),
        ("[>=5]", 5.0, true),
        ("[>=5]", 4.0, false),
        ("[>5]", 5.0, false),
        ("[>5]", 6.0, true),
        ("[=5]", 5.0, true),
        ("[=5]", 6.0, false),
        ("[<>5]", 5.0, false),
        ("[<>5]", 6.0, true),
    ];
    for (cond, v, first) in cases {
        let code = format!("{cond}\"yes\";\"no\"");
        let want = if *first { "yes" } else { "no" };
        assert_eq!(f(&code, *v), want, "{cond} with {v}");
    }
}

// ---- General and 15 significant digits ----------------------------------------------------

#[test]
fn fifteen_significant_digits_are_kept_not_fourteen() {
    // Excel stores and shows 15 significant digits.
    assert_eq!(f("0", 123456789012345.0), "123456789012345");
    assert_eq!(f("0", 999999999999999.0), "999999999999999");
    assert_eq!(f("0.0", 12345678901234.5), "12345678901234.5");
}

// ---- dates: range, elapsed time -----------------------------------------------------------

#[test]
fn the_date_range_ends_at_the_last_serial_of_each_system() {
    // 1900 system: 2958465 is 9999-12-31, the next serial is out of range.
    assert_eq!(f("yyyy-mm-dd", 2958465.0), "9999-12-31");
    assert_eq!(f("yyyy-mm-dd", 2958466.0), "########");
    // 1904 system: 2957003 is 9999-12-31 (1462 days earlier), the next is out of range.
    assert_eq!(f1904("yyyy-mm-dd", 2957003.0), "9999-12-31");
    assert_eq!(f1904("yyyy-mm-dd", 2957004.0), "########");
    assert_eq!(f1904("yyyy-mm-dd", 0.0), "1904-01-01");
    assert_eq!(f1904("yyyy-mm-dd", 2957002.0), "9999-12-30");
}

#[test]
fn elapsed_time_handles_the_upper_bound_of_a_value() {
    // Values of 1e9 days and up are not representable (####), one below still is.
    assert_eq!(f("[h]:mm", 1e9 - 1.0), "23999999976:00");
    assert_eq!(f("[h]:mm", 1e9), "########");
    assert_eq!(f("[h]:mm", -0.5), "########");
}

#[test]
fn elapsed_minutes_and_seconds_use_exact_divisors() {
    assert_eq!(f("[m]", 0.5), "720");
    assert_eq!(f("[m]", 1.5), "2160");
    assert_eq!(f("[s]", 0.5), "43200");
    assert_eq!(f("[h]", 1.5), "36");
    assert_eq!(
        f("[h]:mm:ss", 1.0 + 3.0 / 24.0 + 4.0 / 1440.0 + 5.0 / 86400.0),
        "27:04:05"
    );
}

#[test]
fn elapsed_time_rounds_to_the_displayed_precision_before_splitting() {
    // Without fractional seconds the value is rounded to whole seconds first, so 59.6 s shows
    // as 60 elapsed seconds and 1:59:59.6 as 2:00:00 (Excel rounds, it does not truncate).
    assert_eq!(f("[s]", 59.6 / 86400.0), "60");
    assert_eq!(f("[m]", 7199.9999 / 86400.0), "120");
    assert_eq!(f("[h]:mm:ss", 7199.6 / 86400.0), "2:00:00");
    // With one fractional digit the granularity is 0.1 s.
    assert_eq!(f("[s].0", 59.64 / 86400.0), "59.6");
}

// ---- month vs minute -----------------------------------------------------------------------

#[test]
fn mm_directly_before_an_elapsed_seconds_field_is_minutes() {
    // Microsoft: "mm" immediately before "ss" (here the elapsed [ss]) is minutes, not a month.
    let x = 307.0 / 86400.0; // 5 min 7 s
    assert_eq!(f("mm:[ss]", x), "05:307");
    assert_eq!(f("mm:ss", x), "05:07");
    assert_eq!(f("[h]:mm", 1.0 / 24.0 + 5.0 / 1440.0), "1:05");
}

#[test]
fn mmm_is_always_a_month_name_even_between_hours_and_seconds() {
    let x = FRI + (13.0 * 3600.0 + 45.0 * 60.0) / 86400.0;
    assert_eq!(f("h:mmm:ss", x), "13:Oct:00");
    assert_eq!(f("h:mmmm:ss", x), "13:October:00");
    assert_eq!(f("h:mm:ss", x), "13:45:00");
}

// ---- AM/PM, Buddhist year ------------------------------------------------------------------

#[test]
fn am_pm_markers_follow_the_case_of_the_code() {
    // Microsoft: "AM/PM, am/pm, A/P, a/p": the marker is shown in the case it was written.
    let pm = FRI + 13.0 / 24.0;
    let am = FRI + 9.0 / 24.0;
    assert_eq!(f("h AM/PM", pm), "1 PM");
    assert_eq!(f("h am/pm", pm), "1 pm");
    assert_eq!(f("h A/P", pm), "1 P");
    assert_eq!(f("h a/p", pm), "1 p");
    assert_eq!(f("h AM/PM", am), "9 AM");
    assert_eq!(f("h am/pm", am), "9 am");
    assert_eq!(f("h A/P", am), "9 A");
    assert_eq!(f("h a/p", am), "9 a");
    // Design choice of this engine (the Microsoft table lists only the four spellings above):
    // a mixed-case "aM/PM" is not the lower-case spelling, so it shows upper case.
    assert_eq!(f("h aM/PM", pm), "1 PM");
    assert_eq!(f("h Am/Pm", pm), "1 PM");
}

#[test]
fn midnight_and_noon_are_twelve_on_a_twelve_hour_clock() {
    assert_eq!(f("h AM/PM", FRI), "12 AM");
    assert_eq!(f("h AM/PM", FRI + 0.5), "12 PM");
}

#[test]
fn buddhist_year_b_and_bb_are_two_digits_and_bbb_is_four() {
    // Buddhist era = Gregorian + 543: 2026 -> 2569.
    assert_eq!(f("b", FRI), "69");
    assert_eq!(f("bb", FRI), "69");
    assert_eq!(f("bbb", FRI), "2569");
    assert_eq!(f("bbbb", FRI), "2569");
}

// ---- thousands separators, scaling, fractions, exponent ------------------------------------

#[test]
fn consecutive_commas_between_digits_are_thousands_separators_not_scaling() {
    assert_eq!(f("#,,0", 1234567.0), "1,234,567");
    // Commas after the last digit placeholder scale by 1000 each.
    assert_eq!(f("0,", 1234567.0), "1235");
    assert_eq!(f("0,,", 1234567.0), "1");
    assert_eq!(f("0.0,,", 1234567.0), "1.2");
}

/// The fraction p/q (q <= max_den) closest to `x`, smallest q on ties: what "up to N digits"
/// fraction codes mean.
fn nearest_fraction_error(x: f64, max_den: u32) -> f64 {
    let mut best = f64::MAX;
    for q in 1..=max_den {
        let p = (x * q as f64).round();
        best = best.min((x - p / q as f64).abs());
    }
    best
}

fn parse_fraction(s: &str) -> (f64, f64) {
    let (a, b) = s.trim().split_once('/').unwrap();
    (a.trim().parse().unwrap(), b.trim().parse().unwrap())
}

#[test]
fn a_five_digit_denominator_reaches_denominators_up_to_99999() {
    // 1/12345 is only representable with five denominator digits.
    assert_eq!(f("?????/?????", 1.0 / 12345.0), "    1/12345");
    assert_eq!(f("# ?????/?????", 1.0 / 99999.0), "     1/99999");
    // Four digits cannot: the closest 4-digit fraction is not 1/12345.
    assert_ne!(f("?\u{3f}??/?\u{3f}??", 1.0 / 12345.0).trim(), "1/12345");
}

#[test]
fn the_closest_fraction_wins_even_when_a_simpler_one_is_already_close() {
    // 1/11 is within 1e-6 of x, but a larger denominator (<= 99999) is closer; the search must
    // not stop at "close enough".
    let x = 1.0 / 11.0 + 5e-7;
    let out = f("?????/?????", x);
    let (p, q) = parse_fraction(&out);
    let err = (x - p / q).abs();
    let best = nearest_fraction_error(x, 99999);
    assert!(
        err <= best + 1e-13,
        "{out}: error {err} but the best 5-digit fraction has {best}"
    );
    assert!(
        err < 4.2e-7,
        "{out} is no closer than 1/11 (error 5e-7): {err}"
    );
    // And the simple case stays simple: exactly 1/3 stops at 1/3.
    assert_eq!(f("# ??/??", 1.0 / 3.0).trim(), "1/3");
}

#[test]
fn exponent_without_integer_placeholder_puts_the_point_first() {
    // Excel: 12345 with `.00E+00` shows .12E+05 (the mantissa is 0.12345 -> integer part blank).
    assert_eq!(f(".00E+00", 12345.0), ".12E+05");
    assert_eq!(f("0.00E+00", 12345.0), "1.23E+04");
    assert_eq!(f("##0.0E+0", 12345.0), "12.3E+3");
}

#[test]
fn builtin_43_shows_a_dash_and_blank_digit_slots_for_zero() {
    // `_(* "-"??_)`: space, dash, two `?` slots (spaces), space.
    let code = builtin_format_code(43, Locale::En).unwrap();
    assert_eq!(f(code, 0.0), " -   ");
    assert_eq!(f(code, 1234.5), " 1,234.50 ");
    assert_eq!(f(code, -1234.5), " (1,234.50)");
}

// ---- built-in formats: the whole table ------------------------------------------------------

/// ECMA-376 Part 1 18.8.30 for ids 0-22 and 37-49 (locale-independent ones), Excel's English
/// currency table for 5-8/41-44, and this engine's documented Japanese choices for ids 27-36 /
/// 50-58 (doc comment on `builtin_format_code`: ECMA gives no English meaning for them).
#[test]
fn every_builtin_format_id_maps_to_its_code() {
    let common: &[(u32, &str)] = &[
        (0, "General"),
        (1, "0"),
        (2, "0.00"),
        (3, "#,##0"),
        (4, "#,##0.00"),
        (9, "0%"),
        (10, "0.00%"),
        (11, "0.00E+00"),
        (12, "# ?/?"),
        (13, "# ??/??"),
        (15, "d-mmm-yy"),
        (16, "d-mmm"),
        (17, "mmm-yy"),
        (18, "h:mm AM/PM"),
        (19, "h:mm:ss AM/PM"),
        (20, "h:mm"),
        (21, "h:mm:ss"),
        (27, "[$-411]ge.m.d"),
        (28, "[$-411]ggge\"年\"m\"月\"d\"日\""),
        (29, "[$-411]ggge\"年\"m\"月\"d\"日\""),
        (30, "m/d/yy"),
        (31, "yyyy\"年\"m\"月\"d\"日\""),
        (32, "h\"時\"mm\"分\""),
        (33, "h\"時\"mm\"分\"ss\"秒\""),
        (34, "yyyy\"年\"m\"月\""),
        (35, "m\"月\"d\"日\""),
        (36, "[$-411]ge.m.d"),
        (37, "#,##0_);(#,##0)"),
        (38, "#,##0_);[Red](#,##0)"),
        (39, "#,##0.00_);(#,##0.00)"),
        (40, "#,##0.00_);[Red](#,##0.00)"),
        (45, "mm:ss"),
        (46, "[h]:mm:ss"),
        (47, "mmss.0"),
        (48, "##0.0E+0"),
        (49, "@"),
        (50, "[$-411]ge.m.d"),
        (51, "[$-411]ggge\"年\"m\"月\"d\"日\""),
        (52, "yyyy\"年\"m\"月\""),
        (53, "m\"月\"d\"日\""),
        (54, "[$-411]ggge\"年\"m\"月\"d\"日\""),
        (55, "yyyy\"年\"m\"月\""),
        (56, "m\"月\"d\"日\""),
        (57, "[$-411]ge.m.d"),
        (58, "[$-411]ggge\"年\"m\"月\"d\"日\""),
    ];
    for loc in [Locale::En, Locale::Ja] {
        for (id, code) in common {
            assert_eq!(
                builtin_format_code(*id, loc),
                Some(*code),
                "id {id} {loc:?}"
            );
        }
    }
    let en: &[(u32, &str)] = &[
        (5, "\"$\"#,##0_);\\(\"$\"#,##0\\)"),
        (6, "\"$\"#,##0_);[Red]\\(\"$\"#,##0\\)"),
        (7, "\"$\"#,##0.00_);\\(\"$\"#,##0.00\\)"),
        (8, "\"$\"#,##0.00_);[Red]\\(\"$\"#,##0.00\\)"),
        (14, "m/d/yyyy"),
        (22, "m/d/yyyy h:mm"),
        (41, "_(* #,##0_);_(* \\(#,##0\\);_(* \"-\"_);_(@_)"),
        (
            42,
            "_(\"$\"* #,##0_);_(\"$\"* \\(#,##0\\);_(\"$\"* \"-\"_);_(@_)",
        ),
        (43, "_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)"),
        (
            44,
            "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)",
        ),
    ];
    for (id, code) in en {
        assert_eq!(
            builtin_format_code(*id, Locale::En),
            Some(*code),
            "en id {id}"
        );
    }
    let ja: &[(u32, &str)] = &[
        (5, "\"¥\"#,##0;\"¥\"\\-#,##0"),
        (6, "\"¥\"#,##0;[Red]\"¥\"\\-#,##0"),
        (7, "\"¥\"#,##0.00;\"¥\"\\-#,##0.00"),
        (8, "\"¥\"#,##0.00;[Red]\"¥\"\\-#,##0.00"),
        (14, "yyyy/m/d"),
        (22, "yyyy/m/d h:mm"),
        (41, "_ * #,##0_ ;_ * \\-#,##0_ ;_ * \"-\"_ ;_ @_ "),
        (
            42,
            "_ \"¥\"* #,##0_ ;_ \"¥\"* \\-#,##0_ ;_ \"¥\"* \"-\"_ ;_ @_ ",
        ),
        (43, "_ * #,##0.00_ ;_ * \\-#,##0.00_ ;_ * \"-\"??_ ;_ @_ "),
        (
            44,
            "_ \"¥\"* #,##0.00_ ;_ \"¥\"* \\-#,##0.00_ ;_ \"¥\"* \"-\"??_ ;_ @_ ",
        ),
    ];
    for (id, code) in ja {
        assert_eq!(
            builtin_format_code(*id, Locale::Ja),
            Some(*code),
            "ja id {id}"
        );
    }
    // The ids Excel leaves unassigned.
    for id in [23, 24, 25, 26, 59, 60, 100, 163, u32::MAX] {
        for loc in [Locale::En, Locale::Ja] {
            assert_eq!(builtin_format_code(id, loc), None, "id {id}");
        }
    }
}

#[test]
fn the_cjk_builtin_formats_render_as_the_japanese_excel_shows_them() {
    let d = ser("2026-03-05") + (9.0 * 3600.0 + 5.0 * 60.0 + 7.0) / 86400.0;
    // 32: h時mm分 (the hour is not zero padded, the minute is); 33 adds 秒.
    assert_eq!(
        f(builtin_format_code(32, Locale::Ja).unwrap(), d),
        "9時05分"
    );
    assert_eq!(
        f(builtin_format_code(33, Locale::Ja).unwrap(), d),
        "9時05分07秒"
    );
    // 34/52/55: yyyy年m月; 35/53/56: m月d日 (months and days are not zero padded).
    for id in [34, 52, 55] {
        assert_eq!(
            f(builtin_format_code(id, Locale::Ja).unwrap(), d),
            "2026年3月",
            "id {id}"
        );
    }
    for id in [35, 53, 56] {
        assert_eq!(
            f(builtin_format_code(id, Locale::Ja).unwrap(), d),
            "3月5日",
            "id {id}"
        );
    }
    assert_eq!(
        f(builtin_format_code(31, Locale::Ja).unwrap(), d),
        "2026年3月5日"
    );
}

// ---- month and day names, eras --------------------------------------------------------------

#[test]
fn all_twelve_english_month_names_full_short_and_initial() {
    let months = [
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
    for (i, name) in months.iter().enumerate() {
        let d = ser(&format!("2026-{:02}-15", i + 1));
        assert_eq!(f("mmmm", d), *name, "month {}", i + 1);
        assert_eq!(f("mmm", d), name[..3], "month {}", i + 1);
        assert_eq!(f("mmmmm", d), name[..1], "month {}", i + 1);
        assert_eq!(f("m", d), (i + 1).to_string());
        assert_eq!(f("mm", d), format!("{:02}", i + 1));
        // Japanese: N月 for mmm/mmmm, bare N for mmmmm.
        assert_eq!(fj("mmmm", d), format!("{}月", i + 1));
        assert_eq!(fj("mmm", d), format!("{}月", i + 1));
        assert_eq!(fj("mmmmm", d), (i + 1).to_string());
    }
}

#[test]
fn all_seven_weekdays_in_english_and_japanese() {
    let en = [
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
    ];
    let ja = ["金", "土", "日", "月", "火", "水", "木"];
    for k in 0..7 {
        let d = FRI + k as f64;
        assert_eq!(f("dddd", d), en[k], "day +{k}");
        assert_eq!(f("ddd", d), en[k][..3], "day +{k}");
        assert_eq!(f("aaa", d), ja[k], "day +{k}");
        assert_eq!(f("aaaa", d), format!("{}曜日", ja[k]), "day +{k}");
        assert_eq!(fj("ddd", d), ja[k], "day +{k}");
        assert_eq!(fj("dddd", d), format!("{}曜日", ja[k]), "day +{k}");
    }
}

#[test]
fn every_japanese_era_has_its_three_names_and_year_one() {
    // (first day of the era, long, short, roman)
    let eras = [
        ("2019-05-01", "令和", "令", "R"),
        ("1989-01-08", "平成", "平", "H"),
        ("1926-12-25", "昭和", "昭", "S"),
        ("1912-07-30", "大正", "大", "T"),
    ];
    for (start, long, short, roman) in eras {
        let d = ser(start);
        assert_eq!(f("ggg", d), long, "{start}");
        assert_eq!(f("gg", d), short, "{start}");
        assert_eq!(f("g", d), roman, "{start}");
        // The first year of an era is year 1 (gannen), padded to two digits with `ee`.
        assert_eq!(f("ggge", d), format!("{long}1"), "{start}");
        assert_eq!(f("ggg ee", d), format!("{long} 01"), "{start}");
    }
    // The day before each era starts belongs to the previous one.
    assert_eq!(f("ggg", ser("2019-04-30")), "平成");
    assert_eq!(f("ggg", ser("1989-01-07")), "昭和");
    assert_eq!(f("ggg", ser("1926-12-24")), "大正");
    assert_eq!(f("ggg", ser("1912-07-29")), "明治");
    // Meiji starts in 1868, before the first serial of Excel's calendar, so it shows from day 1
    // on: 1900-01-01 is Meiji 33.
    let d = ser("1900-01-01");
    assert_eq!(f("ggg", d), "明治");
    assert_eq!(f("gg", d), "明");
    assert_eq!(f("g", d), "M");
    assert_eq!(f("ggge", d), "明治33");
    // Reiwa 8 is 2026.
    assert_eq!(f("ggge", FRI), "令和8");
}

#[test]
fn an_lcid_other_than_exactly_japanese_does_not_switch_to_japanese_names() {
    // `[$-411]` is Japanese; the 16-bit LCID is kept whole, so 0xF411 is not Japanese and
    // falls back to the option locale (English here).
    assert_eq!(f("[$-F411]mmmm", FRI), "October");
    assert_eq!(f("[$-411]mmmm", FRI), "10月");
    // High words (sort ids) are dropped: 0x10411 is still Japanese.
    assert_eq!(f("[$-10411]mmmm", FRI), "10月");
    // System date / time placeholders carry no language of their own.
    assert_eq!(f("[$-F800]mmmm", FRI), "October");
    assert_eq!(fj("[$-F800]mmmm", FRI), "10月");
}

// ---- ISO 8601 (ODS) ---------------------------------------------------------------------------

#[test]
fn iso_time_fields_have_exact_upper_bounds() {
    let day = ser("2026-10-02");
    // ISO 8601 allows 24:00:00 (end of day) but not hour 25.
    assert_eq!(
        iso_datetime_to_serial("2026-10-02T24:00:00"),
        Some(day + 1.0)
    );
    assert_eq!(iso_datetime_to_serial("2026-10-02T25:00:00"), None);
    assert!(iso_datetime_to_serial("2026-10-02T23:59:59").is_some());
    // A leap second (60) is accepted, 61 is not.
    assert!(iso_datetime_to_serial("2026-10-02T13:45:60").is_some());
    assert_eq!(iso_datetime_to_serial("2026-10-02T13:45:61"), None);
    // Minutes stop at 59.
    assert!(iso_datetime_to_serial("2026-10-02T13:59:00").is_some());
    assert_eq!(iso_datetime_to_serial("2026-10-02T13:60:00"), None);
}

#[test]
fn iso_zone_offsets_of_either_sign_are_ignored() {
    let plain = iso_datetime_to_serial("2026-10-02T13:45:00").unwrap();
    for suffix in ["Z", "+09:00", "-05:00", "-0500", "+00:00"] {
        assert_eq!(
            iso_datetime_to_serial(&format!("2026-10-02T13:45:00{suffix}")),
            Some(plain),
            "{suffix}"
        );
    }
}

#[test]
fn iso_durations_weeks_and_misplaced_designators() {
    assert_eq!(iso_duration_to_serial("P1W"), Some(7.0));
    assert_eq!(iso_duration_to_serial("P2W"), Some(14.0));
    assert_eq!(iso_duration_to_serial("P1W1D"), Some(8.0));
    // A designator must follow its number: `T` right after a number is not a unit.
    assert_eq!(iso_duration_to_serial("P1T5M"), None);
    assert_eq!(iso_duration_to_serial("P1DT5"), None);
    // Months/years have no fixed length; a time unit before `T` is invalid.
    assert_eq!(iso_duration_to_serial("P1M"), None);
    assert_eq!(iso_duration_to_serial("P1Y"), None);
    assert_eq!(iso_duration_to_serial("P5M"), None);
    assert_eq!(iso_duration_to_serial("PT5M"), Some(5.0 / 1440.0));
}
