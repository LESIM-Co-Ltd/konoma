//! Tests added from mutation testing of the LibreOffice automatic axis scaling (`libre_range`,
//! `libre_pass`, `decimals_of`). Every expected value was worked out by hand from the rules in the
//! documentation of `libre_range` before it was pinned.

use super::*;

fn l(y_axis: bool) -> LibreOpts {
    LibreOpts {
        y_axis,
        label_h: 18.0,
        digit_w: 9.0,
    }
}

fn o(len_px: f64, horizontal: bool) -> Opts {
    Opts {
        len_px,
        horizontal,
        percent: false,
    }
}

/// A tall vertical axis: the label-overlap loop never lowers the interval count.
fn tall(lo: f64, hi: f64, amin: Option<f64>, amax: Option<f64>, y: bool) -> (f64, f64, f64) {
    libre_range(lo, hi, amin, amax, o(1000.0, false), l(y))
}

fn near(got: (f64, f64, f64), want: (f64, f64, f64)) {
    let ok = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(
        ok(got.0, want.0) && ok(got.1, want.1) && ok(got.2, want.2),
        "{got:?} != {want:?}"
    );
}

/// A vertical axis of 192 px (six target ticks), Excel's rules.
fn excel(axis: &Axis, data: Option<(f64, f64)>) -> Scale {
    Scale::new(Some(axis), data, o(192.0, false))
}

fn ax() -> Axis {
    Axis::default()
}

#[test]
fn log_base_must_be_a_finite_number_from_2_to_1000() {
    let with = |b: f64| {
        let a = Axis {
            log_base: Some(b),
            ..ax()
        };
        excel(&a, Some((3.0, 700.0)))
    };
    assert_eq!(with(10.0).log, Some(10.0));
    assert_eq!(with(2.0).log, Some(2.0));
    assert_eq!(with(1000.0).log, Some(1000.0));
    for bad in [1.0, 1.5, 1001.0, 5000.0, f64::NAN, f64::INFINITY, -10.0] {
        assert_eq!(with(bad).log, None, "base {bad}");
    }
}

#[test]
fn non_finite_data_are_dropped_as_a_whole() {
    let none = excel(&ax(), None);
    // no data is drawn like the data 0..1
    assert_eq!(none, excel(&ax(), Some((0.0, 1.0))));
    for d in [
        (3.0, f64::NAN),
        (f64::NAN, 3.0),
        (3.0, f64::INFINITY),
        (f64::NEG_INFINITY, 3.0),
    ] {
        assert_eq!(excel(&ax(), Some(d)), none, "{d:?}");
    }
}

#[test]
fn a_log_axis_needs_a_positive_maximum() {
    let g = Axis {
        log_base: Some(10.0),
        ..ax()
    };
    assert_eq!(excel(&g, Some((3.0, 700.0))).log, Some(10.0));
    // maximum 0 or below: the log axis falls back to a linear one
    assert_eq!(excel(&g, Some((-5.0, 0.0))).log, None);
    assert_eq!(excel(&g, Some((0.0, 0.0))).log, None);
    // a minimum of 0 (or below) cannot be drawn on a log axis: the maximum's decade is used
    let s = excel(&g, Some((0.0, 700.0)));
    assert_eq!((s.min, s.max), (100.0, 1000.0));
    let s = excel(&g, Some((-4.0, 700.0)));
    assert_eq!((s.min, s.max), (100.0, 1000.0));
}

#[test]
fn percent_axis_unit_depends_on_length() {
    let p = |len: f64| {
        Scale::new(
            None,
            Some((0.2, 0.4)),
            Opts {
                percent: true,
                ..o(len, false)
            },
        )
    };
    assert_eq!(p(119.9).major, 0.2);
    assert_eq!(p(100.0).major, 0.2);
    assert_eq!(p(120.0).major, 0.1);
    assert_eq!(p(300.0).major, 0.1);
}

#[test]
fn explicit_limits_on_a_log_axis_must_be_positive() {
    let g = |min: Option<f64>, max: Option<f64>| {
        let a = Axis {
            log_base: Some(10.0),
            min,
            max,
            ..ax()
        };
        let s = excel(&a, Some((3.0, 700.0)));
        (s.min, s.max)
    };
    assert_eq!(g(None, None), (1.0, 1000.0));
    assert_eq!(g(Some(10.0), None), (10.0, 1000.0));
    assert_eq!(g(None, Some(100.0)), (1.0, 100.0));
    // zero and negative limits make no sense on a log axis: ignored
    assert_eq!(g(Some(0.0), None), (1.0, 1000.0));
    assert_eq!(g(Some(-5.0), None), (1.0, 1000.0));
    assert_eq!(g(None, Some(0.0)), (1.0, 1000.0));
    assert_eq!(g(None, Some(-5.0)), (1.0, 1000.0));
    // a minimum above the automatic maximum pushes the maximum a decade above it
    assert_eq!(g(Some(2000.0), None), (2000.0, 20000.0));
    // a maximum below the automatic minimum pulls the minimum a decade below it
    let (mn, mx) = g(None, Some(0.5));
    assert!((mn - 0.05).abs() < 1e-12 && mx == 0.5, "{mn} {mx}");
}

#[test]
fn explicit_limits_on_a_linear_axis_may_be_negative_and_win() {
    let a = |min: Option<f64>, max: Option<f64>, data: (f64, f64)| {
        let s = excel(&Axis { min, max, ..ax() }, Some(data));
        (s.min, s.max)
    };
    assert_eq!(a(None, Some(-1.0), (-8.0, -2.0)).1, -1.0);
    assert_eq!(a(Some(-3.0), None, (2.0, 8.0)).0, -3.0);
    // a maximum at or below the minimum: the side that is not fixed moves by the automatic span
    // (data 100..101 are drawn on 99.8..101.2, a span of 1.4)
    let (mn, mx) = a(Some(500.0), None, (100.0, 101.0));
    assert_eq!(mn, 500.0);
    assert!((mx - 501.4).abs() < 1e-9, "{mx}");
    let (mn, mx) = a(None, Some(-100.0), (100.0, 101.0));
    assert_eq!(mx, -100.0);
    assert!((mn + 101.4).abs() < 1e-9, "{mn}");
    // ... but a span under 1 still moves it by 1
    let (mn, mx) = a(Some(500.0), None, (100.0, 100.2));
    assert_eq!(mn, 500.0);
    assert!((mx - 501.0).abs() < 1e-9, "{mx}");
    let (mn, mx) = a(None, Some(-100.0), (100.0, 100.2));
    assert_eq!(mx, -100.0);
    assert!((mn + 101.0).abs() < 1e-9, "{mn}");
    // both fixed but not a range: the automatic limits are kept
    assert_eq!(
        a(Some(10.0), Some(5.0), (0.0, 10.0)),
        a(None, None, (0.0, 10.0))
    );
    assert_eq!(
        a(Some(7.0), Some(7.0), (0.0, 10.0)),
        a(None, None, (0.0, 10.0))
    );
}

#[test]
fn major_and_minor_units_follow_the_axis_and_the_tick_budget() {
    let un = |major: Option<f64>,
              minor: Option<f64>,
              min: Option<f64>,
              max: Option<f64>,
              d: (f64, f64)| {
        let s = excel(
            &Axis {
                major_unit: major,
                minor_unit: minor,
                min,
                max,
                ..ax()
            },
            Some(d),
        );
        (s.min, s.max, s.major, s.minor)
    };
    // data 0..10 on 192 px: 0..12 by 2, minor by 0.5 (a quarter for 2)
    assert_eq!(
        un(None, None, None, None, (0.0, 10.0)),
        (0.0, 12.0, 2.0, 0.5)
    );
    // a user unit wins, even one larger than the span
    assert_eq!(un(Some(3.0), None, None, None, (0.0, 10.0)).2, 3.0);
    assert_eq!(un(Some(100.0), None, None, None, (0.0, 10.0)).2, 100.0);
    // the count test uses the drawn span, not the sum of its ends (99.8..101.2: 140 units of 0.01)
    assert_eq!(un(Some(0.01), None, None, None, (100.0, 101.0)).2, 0.01);
    // too many intervals (over 200): the automatic unit instead
    assert_eq!(un(Some(0.001), None, None, None, (0.0, 10.0)).2, 2.0);
    // a user minor unit wins up to 1000 intervals
    assert_eq!(un(None, Some(0.05), None, None, (0.0, 10.0)).3, 0.05);
    assert_eq!(un(None, Some(0.012), None, None, (0.0, 10.0)).3, 0.012);
    assert_eq!(un(None, Some(0.001), None, None, (0.0, 10.0)).3, 0.5);
    // a zero or negative user unit is ignored
    assert_eq!(
        un(Some(0.0), Some(-1.0), None, None, (0.0, 10.0)),
        (0.0, 12.0, 2.0, 0.5)
    );
    // fixed limits re-derive the unit from the new span (data alone would give 2)
    assert_eq!(un(None, None, Some(-5.0), None, (0.0, 10.0)).2, 5.0);
    assert_eq!(un(None, None, None, Some(30.0), (0.0, 10.0)).2, 5.0);
}

fn lin(min: f64, max: f64, major: f64, minor: f64) -> Scale {
    Scale {
        min,
        max,
        log: None,
        reversed: false,
        major,
        minor,
    }
}

fn logs(min: f64, max: f64, base: f64) -> Scale {
    Scale {
        min,
        max,
        log: Some(base),
        reversed: false,
        major: 0.0,
        minor: 0.0,
    }
}

fn all_near(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(
            (g - w).abs() <= 1e-9 * w.abs().max(1.0),
            "{got:?} vs {want:?}"
        );
    }
}

#[test]
fn frac_places_values_on_linear_and_log_axes() {
    let s = lin(10.0, 30.0, 5.0, 1.0);
    all_near(
        &[s.frac(10.0), s.frac(20.0), s.frac(30.0), s.frac(40.0)],
        &[0.0, 0.5, 1.0, 1.5],
    );
    // reversed: counted from the far end
    let r = Scale {
        reversed: true,
        ..s.clone()
    };
    all_near(
        &[r.frac(10.0), r.frac(20.0), r.frac(30.0)],
        &[1.0, 0.5, 0.0],
    );
    // clamped far outside, and a zero-width axis gives 0
    assert_eq!(s.frac(1e30), 1e6);
    assert_eq!(s.frac(-1e30), -1e6);
    assert_eq!(lin(5.0, 5.0, 1.0, 0.2).frac(7.0), 0.0);
    // log axis: a position is a fraction of the way in decades from the real minimum
    let g = logs(10.0, 1000.0, 10.0);
    all_near(
        &[g.frac(10.0), g.frac(100.0), g.frac(1000.0)],
        &[0.0, 0.5, 1.0],
    );
    let g = logs(1.0, 1000.0, 10.0);
    all_near(&[g.frac(10.0), g.frac(100.0)], &[1.0 / 3.0, 2.0 / 3.0]);
    // zero and negative values do not exist on a log axis: far before the start
    assert_eq!(g.frac(0.0), -1e6);
    assert_eq!(g.frac(-3.0), -1e6);
    let rg = Scale {
        reversed: true,
        ..g
    };
    assert_eq!(rg.frac(0.0), 1.0 + 1e6);
}

#[test]
fn a_log_range_always_spans_at_least_one_decade() {
    let s = Scale::log_range(10.0, 100.0, 100.0);
    assert_eq!((s.min, s.max), (100.0, 1000.0));
    let s = Scale::log_range(10.0, 3.0, 700.0);
    assert_eq!((s.min, s.max), (1.0, 1000.0));
}

#[test]
fn linear_major_ticks() {
    all_near(&lin(0.0, 10.0, 4.0, 1.0).major_ticks(), &[0.0, 4.0, 8.0]);
    // 0.3 / 0.1 is 2.9999999999999996: the last tick must not be lost to binary noise
    all_near(
        &lin(0.0, 0.3, 0.1, 0.02).major_ticks(),
        &[0.0, 0.1, 0.2, 0.3],
    );
    assert!(lin(0.0, 10.0, 0.0, 0.0).major_ticks().is_empty());
    assert_eq!(lin(0.0, 1e6, 1.0, 0.2).major_ticks().len(), MAX_TICKS + 1);
}

#[test]
fn log_major_ticks_are_the_powers_of_the_base() {
    all_near(
        &logs(1.0, 1000.0, 10.0).major_ticks(),
        &[1.0, 10.0, 100.0, 1000.0],
    );
    // no tick beyond the maximum (the tolerance is relative, not an absolute one)
    all_near(
        &logs(1e-6, 1e-3, 10.0).major_ticks(),
        &[1e-6, 1e-5, 1e-4, 1e-3],
    );
    all_near(&logs(1.0, 8.0, 2.0).major_ticks(), &[1.0, 2.0, 4.0, 8.0]);
    // and never more than MAX_TICKS
    assert_eq!(
        logs(1.0, 2f64.powi(300), 2.0).major_ticks().len(),
        MAX_TICKS
    );
}

#[test]
fn linear_minor_ticks_skip_the_major_ones() {
    let want: Vec<f64> = (1..=20)
        .map(|i| f64::from(i) * 0.5)
        .filter(|v| ![4.0, 8.0].contains(v))
        .collect();
    // 0..10, major 4 (0 4 8), minor 0.5: the last tick, 10, is a minor one
    all_near(&lin(0.0, 10.0, 4.0, 0.5).minor_ticks(), &want[..]);
    // the majors are counted from the minimum, not from zero: 1 and 2.5 are majors
    all_near(&lin(1.0, 3.0, 1.5, 0.5).minor_ticks(), &[1.5, 2.0, 3.0]);
    // no unit, no ticks
    assert!(lin(0.0, 10.0, 4.0, 0.0).minor_ticks().is_empty());
    assert!(lin(0.0, 10.0, 0.0, 0.5).minor_ticks().is_empty());
    // a huge count is cut at 5 * MAX_TICKS (the majors 0 and 1000 are left out of 0..=1000)
    assert_eq!(
        lin(0.0, 2000.0, 1000.0, 1.0).minor_ticks().len(),
        MAX_TICKS * 5 - 1
    );
}

#[test]
fn log_minor_ticks_fill_each_decade() {
    let mut want: Vec<f64> = (2..10).map(f64::from).collect();
    want.extend((2..10).map(|k| f64::from(k) * 10.0));
    all_near(&logs(1.0, 100.0, 10.0).minor_ticks(), &want);
    // a maximum inside a decade cuts that decade (50 itself is not below 50)
    let mut want: Vec<f64> = (2..10).map(f64::from).collect();
    want.extend([20.0, 30.0, 40.0]);
    all_near(&logs(1.0, 50.0, 10.0).minor_ticks(), &want);
    // base 2 has nothing between the powers; base 4 has 2 and 3 times each
    assert!(logs(1.0, 64.0, 2.0).minor_ticks().is_empty());
    all_near(&logs(1.0, 16.0, 4.0).minor_ticks(), &[2.0, 3.0, 8.0, 12.0]);
    // at most 5 * MAX_TICKS
    assert_eq!(logs(1.0, 1e130, 10.0).minor_ticks().len(), MAX_TICKS * 5);
}

#[test]
fn the_default_minor_unit_is_a_fifth_or_for_2_a_quarter() {
    assert_eq!(default_minor(10.0), 2.0);
    assert_eq!(default_minor(5.0), 1.0);
    assert_eq!(default_minor(2.0), 0.5);
    assert_eq!(default_minor(0.2), 0.05);
    assert_eq!(default_minor(20.0), 5.0);
    // no unit, no minor unit
    assert_eq!(default_minor(0.0), 0.0);
    assert_eq!(default_minor(-4.0), 0.0);
    assert_eq!(default_minor(f64::NAN), 0.0);
}

#[test]
fn decimals_tolerance_is_relative_and_strict() {
    // 1e-6 is off by exactly the tolerance at 0 decimals: not close enough, so 6 decimals.
    assert_eq!(decimals_of(1e-6), 6);
    // A big step is allowed an error that scales with it.
    assert_eq!(decimals_of(1_000_000.000_01), 0);
}

#[test]
fn negative_data_are_mirrored_with_their_limits() {
    // Mirrored data 8..9.8 start at zero (8/9.8 < 5/6), so the negative axis ends at zero.
    near(tall(-9.8, -8.0, None, None, true), (-11.0, 0.0, 1.0));
    // The fixed limits swap roles in the mirror: a fixed minimum of -12 is a fixed maximum of 12.
    near(tall(-9.0, -2.0, Some(-12.0), None, true), (-12.0, 0.0, 2.0));
    // ... and a fixed maximum of -1 is a fixed minimum of 1.
    near(tall(-9.0, -2.0, None, Some(-1.0), true), (-10.0, -1.0, 1.0));
    // Narrow data (98..100 mirrored) start half a span below, and the sign comes back on both ends.
    near(tall(-100.0, -98.0, None, None, true), (-100.5, -97.0, 0.5));
}

#[test]
fn y_axis_zero_rule_is_strict_at_five_sixths() {
    // 5/6 exactly is not "under" 5/6: the axis starts half a span below the minimum, not at 0.
    near(tall(5.0, 6.0, None, None, true), (4.4, 6.2, 0.2));
    // narrow data start half their span below the minimum: 50..55 -> 47.5 -> unit 1
    near(tall(50.0, 55.0, None, None, true), (47.0, 56.0, 1.0));
}

#[test]
fn a_fixed_pair_in_the_wrong_order_is_swapped() {
    near(
        tall(0.0, 0.0, Some(100.0), Some(2.0), true),
        (100.0, 110.0, 10.0),
    );
}

#[test]
fn equal_limits_widen_by_the_rule_that_matches_what_is_fixed() {
    // An x axis fixed at its maximum 8 with a minimum of 8 too: the minimum halves.
    near(tall(8.0, 8.0, None, Some(8.0), false), (4.0, 8.0, 0.5));
    // ... or goes to -1 when it is 0.
    near(tall(0.0, 0.0, None, Some(0.0), false), (-1.0, 0.0, 0.1));
    // Both ends fixed to the same value: one unit of 0.1 is made up.
    near(tall(0.0, 9.0, Some(3.0), Some(3.0), true), (3.0, 3.1, 0.1));
}

#[test]
fn x_axis_ends_get_a_unit_when_the_data_nearly_reach_them() {
    // -1.95 is within 1/21 of the axis of its rounded minimum -2: one more unit below; the maximum
    // 8 is rounded to 8 itself, which the data reach, so one more unit above.
    near(tall(-1.95, 8.0, None, None, false), (-3.0, 9.0, 1.0));
}

#[test]
fn horizontal_label_width_counts_digits_and_the_minus_sign() {
    let run = |lo: f64, hi: f64, len: f64| libre_range(lo, hi, None, None, o(len, true), l(true));
    // 0..52: labels up to "60" are 2 digits = 18 px wide; six intervals fit in 120 px (20 each) ...
    near(run(0.0, 52.0, 120.0), (0.0, 60.0, 10.0));
    // ... but not in 84 px (14 each): the unit doubles to 20.
    near(run(0.0, 52.0, 84.0), (0.0, 60.0, 20.0));
    // -50..50 needs a sign as well: "-60" is 27 px, twelve intervals of 20.8 px do not fit.
    near(run(-50.0, 50.0, 250.0), (-60.0, 60.0, 20.0));
}
