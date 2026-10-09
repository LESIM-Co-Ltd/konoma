//! Value-axis scaling: Excel's automatic minimum, maximum and major unit ("nice numbers"), explicit
//! overrides, logarithmic axes, and the tick lists.
//!
//! # The automatic range (linear axis)
//!
//! Given the smallest and largest data value `lo`, `hi` (stacked charts pass the stacked totals):
//!
//! 1. *Zero.* If `lo >= 0` and `(hi - lo) / hi >= 1/6` the axis starts at 0. If the data sit in the
//!    top sixth of their range the axis starts at `lo - (hi - lo) / 20` instead (rounded down to
//!    a major unit). The same, mirrored, for data at or below 0. Source: Jon Peltier, "How Excel
//!    Calculates Automatic Chart Axis Limits" (peltiertech.com), measured on current Excel: "the
//!    automatic minimum is the first major unit less than or equal to `Ymin - (Ymax - Ymin) / 20`"
//!    and "the automatic maximum is the first major unit above `Ymax + (Ymax - Ymin) / 20`". (The
//!    older Microsoft knowledge-base text says `/ 2` for the minimum; Excel 2007 and later do not.)
//! 2. *Major unit.* The span being drawn is divided by the number of ticks the axis length can hold
//!    ([`target_ticks`]: one per 32 px of a vertical axis, one per 60 px of a horizontal one,
//!    between 3 and 10) and the result is rounded **up** to 1, 2 or 5 times a power of ten.
//! 3. *Maximum.* `hi + 5 % of (hi - lo)` rounded up to a multiple of the unit, so a maximum that
//!    is itself a multiple of the unit still gets a further tick above it (as in Excel: data
//!    reaching exactly 20 gives an axis up to 25 with a unit of 5). The minimum is rounded down
//!    to a multiple of the unit.
//!
//! All-equal data use the magnitude of the value as their range; no data at all gives 0..1.
//! Percent-stacked axes are fixed at 0..1 (the data are fractions) with a unit of 0.1 (0.2 when the
//! axis is short). Everything is clamped to [`super::MAX_VALUE`] first.

use super::{clean, Axis, MAX_TICKS};

/// The resolved scale of one value (or x) axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    /// Logarithmic base, when the axis is logarithmic.
    pub log: Option<f64>,
    pub reversed: bool,
    /// Major unit (linear axes); `0` on a log axis, whose major ticks are the powers of the base.
    pub major: f64,
    /// Minor unit (linear axes), `0` = none.
    pub minor: f64,
}

/// What the scaling needs to know besides the axis element and the data.
#[derive(Debug, Clone, Copy)]
pub struct Opts {
    /// Length of the axis in px.
    pub len_px: f64,
    pub horizontal: bool,
    /// Percent-stacked data: the axis is 0..1.
    pub percent: bool,
}

/// How many major intervals an axis of `len_px` should have.
pub fn target_ticks(len_px: f64, horizontal: bool) -> f64 {
    let per = if horizontal { 60.0 } else { 32.0 };
    let n = if len_px.is_finite() {
        len_px / per
    } else {
        5.0
    };
    n.clamp(3.0, 10.0)
}

/// The smallest of 1, 2, 5 times a power of ten that is at least `raw` (`raw > 0`, finite).
pub fn nice_unit(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let p = 10f64.powf(exp);
    let f = raw / p;
    // A hair of slack so that 2.0000000000000004 stays 2.
    let m = if f <= 1.000_000_1 {
        1.0
    } else if f <= 2.000_000_1 {
        2.0
    } else if f <= 5.000_000_1 {
        5.0
    } else {
        10.0
    };
    m * p
}

/// Rounds away binary noise: `0.30000000000000004` -> `0.3` (12 significant digits).
pub fn tidy(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let mag = v.abs().log10().floor();
    let p = 10f64.powf(11.0 - mag);
    if !p.is_finite() || p == 0.0 {
        return v;
    }
    let r = (v * p).round() / p;
    if r.is_finite() {
        r
    } else {
        v
    }
}

/// The automatic linear range and major unit for data between `lo` and `hi` (see the module
/// documentation).
pub fn auto_range(lo: f64, hi: f64, ticks: f64) -> (f64, f64, f64) {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    if lo == 0.0 && hi == 0.0 {
        return (0.0, 1.0, nice_unit(1.0 / ticks));
    }
    let range = hi - lo;
    let pad = if range > 0.0 {
        0.05 * range
    } else {
        0.05 * hi.abs()
    };
    // The span the axis will cover, before it is rounded to the unit.
    let (mut amin, mut amax) = (lo, hi);
    if lo >= 0.0 {
        if hi > 0.0 && range / hi >= 1.0 / 6.0 || range == 0.0 {
            amin = 0.0;
        } else {
            amin = lo - range / 20.0;
        }
    } else if hi <= 0.0 {
        if lo < 0.0 && range / lo.abs() >= 1.0 / 6.0 || range == 0.0 {
            amax = 0.0;
        } else {
            amax = hi + range / 20.0;
        }
    }
    let top = if amax == 0.0 && hi <= 0.0 {
        0.0
    } else {
        hi + pad
    };
    let bottom = if lo < 0.0 { lo - pad } else { amin };
    let span = (top.max(amax) - bottom.min(amin)).max(f64::MIN_POSITIVE);
    let unit = nice_unit(span / ticks);
    let min = tidy((bottom.min(amin) / unit).floor() * unit);
    let mut max = tidy((top.max(amax) / unit).ceil() * unit);
    if hi <= 0.0 && amax == 0.0 {
        max = 0.0;
    }
    if max <= min {
        max = tidy(min + unit);
    }
    (min, max, unit)
}

/// What LibreOffice's automatic scaling needs besides [`Opts`].
#[derive(Debug, Clone, Copy)]
pub struct LibreOpts {
    /// The value (y) axis; an x axis of a scatter chart is not widened to zero.
    pub y_axis: bool,
    /// Height of one tick label line, px.
    pub label_h: f64,
    /// Width of one digit, px.
    pub digit_w: f64,
}

/// Most main intervals LibreOffice allows an automatic axis to start with.
const LIBRE_MAX_INTERVALS: u32 = 10;

/// The smallest of 1, 2, 5 times a power of ten that is at least `raw`.
fn libre_step(raw: f64) -> f64 {
    nice_unit(raw)
}

/// The number of decimals a tick label needs to show multiples of `step`.
fn decimals_of(step: f64) -> usize {
    (0..=8)
        .find(|d| {
            let v = step * 10f64.powi(*d);
            (v - v.round()).abs() < 1e-6 * v.abs().max(1.0)
        })
        .unwrap_or(8) as usize
}

/// LibreOffice's automatic linear range `(min, max, major unit)` for data between `lo` and `hi`
/// (`amin` / `amax`: limits the file fixes). In words, after
/// `chart2/source/view/axes/ScaleAutomatism.cxx`:
///
/// 1. Data that are all at or below zero are negated, scaled, and negated back.
/// 2. A y axis whose smallest value is positive starts at 0 when the smallest value is under
///    5/6 of the largest (or all values are equal); data that sit closer together start half
///    their span below the smallest value instead. An x axis starts at the smallest value.
/// 3. The major unit is the smallest of 1, 2, 5 times a power of ten that makes at most N
///    intervals, N being 10 at first. The axis is then rounded outward to multiples of the
///    unit, and an end that the data nearly reach (within 1/21 of the axis) gets one more unit.
/// 4. The labels must not overlap: when they would (a tick label line is taller than the
///    spacing, or a label is wider than it), N is lowered by one and the steps are redone.
fn libre_range(
    lo: f64,
    hi: f64,
    amin: Option<f64>,
    amax: Option<f64>,
    o: Opts,
    l: LibreOpts,
) -> (f64, f64, f64) {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    // Negative data: mirror (the roles of minimum and maximum swap).
    if hi <= 0.0 && lo < 0.0 {
        let (a, b, u) = libre_range(-hi, -lo, amax.map(|v| -v), amin.map(|v| -v), o, l);
        return (tidy(-b), tidy(-a), u);
    }
    let mut result = (0.0, 1.0, 1.0);
    for n in (2..=LIBRE_MAX_INTERVALS).rev() {
        result = libre_pass(lo, hi, amin, amax, f64::from(n), l);
        let (min, max, step) = result;
        let intervals = ((max - min) / step).round().max(1.0);
        let pitch = o.len_px / intervals;
        let extent = if o.horizontal {
            let d = decimals_of(step);
            let widest = format!("{:.d$}", min.abs().max(max.abs()));
            let neg = usize::from(min < 0.0);
            (widest.chars().count() + neg) as f64 * l.digit_w
        } else {
            l.label_h
        };
        if pitch >= extent || pitch.is_nan() {
            break;
        }
    }
    result
}

/// One pass of [`libre_range`] with at most `n` intervals.
fn libre_pass(
    lo: f64,
    hi: f64,
    amin: Option<f64>,
    amax: Option<f64>,
    n: f64,
    l: LibreOpts,
) -> (f64, f64, f64) {
    let (mut tmin, mut tmax) = (amin.unwrap_or(lo), amax.unwrap_or(hi));
    if tmax < tmin {
        std::mem::swap(&mut tmin, &mut tmax);
    }
    if amin.is_none() && tmin > 0.0 && l.y_axis {
        if tmin == tmax || tmin / tmax < 5.0 / 6.0 {
            tmin = 0.0;
        } else {
            tmin -= (tmax - tmin) / 2.0;
        }
    }
    if tmin == tmax {
        if amax.is_none() {
            tmax = if tmax == 0.0 { 1.0 } else { tmax * 2.0 };
        } else if amin.is_none() {
            tmin = if tmin == 0.0 { -1.0 } else { tmin / 2.0 };
        } else {
            tmax = tmin + 1.0;
        }
    }
    let range = tmax - tmin;
    let step = libre_step(range / n);
    let mut min = amin.unwrap_or_else(|| tidy((tmin / step).floor() * step));
    let mut max = amax.unwrap_or_else(|| tidy((tmax / step).ceil() * step));
    if max <= min {
        max = tidy(min + step);
    }
    // An end the data nearly reach gets one more unit (both tests use the rounded axis).
    let (min0, max0) = (min, max);
    let span = max0 - min0;
    if amin.is_none() && min0 != 0.0 && (max0 - lo) / span > 20.0 / 21.0 {
        min = tidy(min0 - step);
    }
    if amax.is_none() && max0 != 0.0 && (hi - min0) / span > 20.0 / 21.0 {
        max = tidy(max0 + step);
    }
    (min, max, step)
}

impl Scale {
    /// The scale of `axis` (`None` = all defaults) for data spanning `data` (`None` = no data).
    pub fn new(axis: Option<&Axis>, data: Option<(f64, f64)>, o: Opts) -> Scale {
        Scale::build(axis, data, o, None)
    }

    /// Like [`Scale::new`], but the automatic range follows LibreOffice's rules
    /// ([`libre_range`]) instead of Excel's.
    pub fn new_libre(
        axis: Option<&Axis>,
        data: Option<(f64, f64)>,
        o: Opts,
        lo: LibreOpts,
    ) -> Scale {
        Scale::build(axis, data, o, Some(lo))
    }

    fn build(
        axis: Option<&Axis>,
        data: Option<(f64, f64)>,
        o: Opts,
        libre: Option<LibreOpts>,
    ) -> Scale {
        let ticks = target_ticks(o.len_px, o.horizontal);
        let reversed = axis.is_some_and(|a| a.reversed);
        let amin = axis.and_then(|a| a.min).and_then(clean);
        let amax = axis.and_then(|a| a.max).and_then(clean);
        let log = axis
            .and_then(|a| a.log_base)
            .filter(|b| b.is_finite() && *b >= 2.0 && *b <= 1000.0);
        let data = data.filter(|(a, b)| a.is_finite() && b.is_finite());

        let mut s = match (log, data) {
            (Some(base), Some((lo, hi))) if hi > 0.0 => Scale::log_range(base, lo, hi),
            _ if o.percent => {
                let unit = if o.len_px < 120.0 { 0.2 } else { 0.1 };
                Scale::linear(0.0, 1.0, unit)
            }
            _ => {
                let (lo, hi) = data.unwrap_or((0.0, 1.0));
                let (a, b, u) = match libre {
                    Some(l) => libre_range(lo, hi, amin, amax, o, l),
                    None => auto_range(lo, hi, ticks),
                };
                Scale::linear(a, b, u)
            }
        };
        s.reversed = reversed;
        // Explicit values win; an explicit pair that does not make a range is dropped.
        let lg = s.log.is_some();
        let (mut mn, mut mx) = (s.min, s.max);
        if let Some(v) = amin {
            if !lg || v > 0.0 {
                mn = v;
            }
        }
        if let Some(v) = amax {
            if !lg || v > 0.0 {
                mx = v;
            }
        }
        if mx <= mn {
            if amin.is_some() && amax.is_none() {
                mx = if lg {
                    mn * 10.0
                } else {
                    mn + (s.max - s.min).max(1.0)
                };
            } else if amax.is_some() && amin.is_none() {
                mn = if lg {
                    mx / 10.0
                } else {
                    mx - (s.max - s.min).max(1.0)
                };
            } else {
                mn = s.min;
                mx = s.max;
            }
        }
        s.min = mn;
        s.max = mx;
        if !lg {
            let span = s.max - s.min;
            let user = axis.and_then(|a| a.major_unit).filter(|u| *u > 0.0);
            s.major = match user {
                Some(u) if span / u <= MAX_TICKS as f64 => u,
                _ if (amin.is_some() || amax.is_some()) && libre.is_none() => {
                    nice_unit(span / ticks)
                }
                _ => s.major,
            };
            if !(s.major > 0.0 && span / s.major <= MAX_TICKS as f64) {
                s.major = nice_unit(span / ticks);
            }
            let user_minor = axis.and_then(|a| a.minor_unit).filter(|u| *u > 0.0);
            s.minor = match user_minor {
                Some(u) if span / u <= (MAX_TICKS * 5) as f64 => u,
                _ => default_minor(s.major),
            };
        }
        s
    }

    fn linear(min: f64, max: f64, major: f64) -> Scale {
        Scale {
            min,
            max,
            log: None,
            reversed: false,
            major,
            minor: default_minor(major),
        }
    }

    fn log_range(base: f64, lo: f64, hi: f64) -> Scale {
        let lo = if lo > 0.0 { lo } else { hi };
        let e0 = (lo.ln() / base.ln()).floor();
        let mut e1 = (hi.ln() / base.ln()).ceil();
        if e1 <= e0 {
            e1 = e0 + 1.0;
        }
        Scale {
            min: base.powf(e0),
            max: base.powf(e1),
            log: Some(base),
            reversed: false,
            major: 0.0,
            minor: 0.0,
        }
    }

    /// Where `v` sits on the axis, `0` at the start and `1` at the end of the axis as drawn
    /// (a reversed axis counts from its far end). Not clamped (but finite).
    pub fn frac(&self, v: f64) -> f64 {
        let f = match self.log {
            Some(_) => {
                if v <= 0.0 {
                    -1e6
                } else {
                    (v.ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
                }
            }
            None => (v - self.min) / (self.max - self.min),
        };
        let f = if f.is_finite() {
            f.clamp(-1e6, 1e6)
        } else {
            0.0
        };
        if self.reversed {
            1.0 - f
        } else {
            f
        }
    }

    /// The major tick values, in ascending order, at most [`MAX_TICKS`].
    pub fn major_ticks(&self) -> Vec<f64> {
        let mut out = Vec::new();
        match self.log {
            Some(base) => {
                let mut v = self.min;
                while v <= self.max * 1.000_001 && out.len() < MAX_TICKS {
                    out.push(tidy(v));
                    v *= base;
                }
            }
            None => {
                if self.major <= 0.0 {
                    return out;
                }
                let n = ((self.max - self.min) / self.major + 1e-9).floor() as usize;
                for i in 0..=n.min(MAX_TICKS) {
                    out.push(tidy(self.min + i as f64 * self.major));
                }
            }
        }
        out
    }

    /// The minor tick values (those that are not major ones), at most `5 * MAX_TICKS`.
    pub fn minor_ticks(&self) -> Vec<f64> {
        let mut out = Vec::new();
        match self.log {
            Some(base) => {
                let mut p = self.min;
                while p < self.max * 0.999_999 && out.len() < MAX_TICKS * 5 {
                    let mut k = 2.0;
                    while k < base && out.len() < MAX_TICKS * 5 {
                        let v = p * k;
                        if v < self.max {
                            out.push(v);
                        }
                        k += 1.0;
                    }
                    p *= base;
                }
            }
            None => {
                if self.minor <= 0.0 || self.major <= 0.0 {
                    return out;
                }
                let n = ((self.max - self.min) / self.minor + 1e-9).floor() as usize;
                for i in 0..=n.min(MAX_TICKS * 5) {
                    let v = tidy(self.min + i as f64 * self.minor);
                    let r = (v - self.min) / self.major;
                    if (r - r.round()).abs() > 1e-6 {
                        out.push(v);
                    }
                }
            }
        }
        out
    }
}

/// Excel's automatic minor unit: a fifth of the major unit (a quarter for 2).
fn default_minor(major: f64) -> f64 {
    if major.is_nan() || major <= 0.0 {
        return 0.0;
    }
    let p = 10f64.powf(major.log10().floor());
    let m = major / p;
    if (m - 2.0).abs() < 1e-6 {
        major / 4.0
    } else {
        major / 5.0
    }
}

/// The data range a category-less list of values spans, ignoring blanks.
#[cfg(test)]
pub fn span(values: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let mut r: Option<(f64, f64)> = None;
    for v in values {
        r = Some(match r {
            None => (v, v),
            Some((a, b)) => (a.min(v), b.max(v)),
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto(lo: f64, hi: f64) -> (f64, f64, f64) {
        auto_range(lo, hi, 6.0)
    }

    #[test]
    fn nice_units() {
        assert_eq!(nice_unit(0.9), 1.0);
        assert_eq!(nice_unit(1.0), 1.0);
        assert_eq!(nice_unit(1.5), 2.0);
        assert_eq!(nice_unit(2.0), 2.0);
        assert_eq!(nice_unit(3.0), 5.0);
        assert_eq!(nice_unit(5.0), 5.0);
        assert_eq!(nice_unit(5.1), 10.0);
        assert_eq!(nice_unit(0.03), 0.05);
        assert_eq!(nice_unit(0.0), 1.0);
        assert_eq!(nice_unit(f64::NAN), 1.0);
        assert_eq!(nice_unit(-3.0), 1.0);
        assert_eq!(nice_unit(1234.0), 2000.0);
    }

    #[test]
    fn excel_examples() {
        // The reference: 0..21 -> 0..25 by 5; stacked sums up to 52 -> 0..60 by 10.
        assert_eq!(auto(6.0, 21.0), (0.0, 25.0, 5.0));
        assert_eq!(auto(0.0, 52.0), (0.0, 60.0, 10.0));
        // A maximum on a unit boundary still gets a tick above it.
        let (_, max, _) = auto(0.0, 20.0);
        assert!(max > 20.0, "{max}");
        assert_eq!(auto(0.0, 100.0).1, 120.0);
        assert_eq!(auto(0.0, 1.0), (0.0, 1.2, 0.2));
    }

    #[test]
    fn negative_and_mixed() {
        let (mn, mx, u) = auto(-30.0, 40.0);
        assert!(mn <= -30.0 && mx >= 40.0 && u > 0.0);
        assert_eq!(mn % u, 0.0);
        assert_eq!(mx % u, 0.0);
        let (mn, mx, _) = auto(-50.0, -10.0);
        assert_eq!(mx, 0.0);
        assert!(mn <= -50.0);
        // Data in the top sixth: the axis does not start at 0.
        let (mn, mx, _) = auto(100.0, 102.0);
        assert!(mn > 90.0 && mn <= 100.0, "{mn}");
        assert!(mx >= 102.0);
    }

    #[test]
    fn tiny_equal_and_empty() {
        let (mn, mx, u) = auto(0.0, 0.0);
        assert_eq!((mn, mx), (0.0, 1.0));
        assert!(u > 0.0);
        let (mn, mx, _) = auto(5.0, 5.0);
        assert_eq!(mn, 0.0);
        assert!((5.0..=10.0).contains(&mx), "{mx}");
        let (mn, mx, _) = auto(-5.0, -5.0);
        assert_eq!(mx, 0.0);
        assert!(mn <= -5.0);
        let (mn, mx, u) = auto(0.001, 0.004);
        assert_eq!(mn, 0.0);
        assert!((0.004..=0.01).contains(&mx), "{mx} {u}");
        // Every result is a proper range.
        for (a, b) in [
            (1e-9, 2e-9),
            (0.0, 1e-12),
            (-1e-12, 1e-12),
            (1e14, 1.0001e14),
        ] {
            let (mn, mx, u) = auto(a, b);
            assert!(mn < mx && u > 0.0, "{a} {b}: {mn} {mx} {u}");
        }
    }

    #[test]
    fn huge_values() {
        let s = Scale::new(
            None,
            Some((0.0, 1e308)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        // The caller clamps; the scale itself must still not produce NaN or infinity.
        assert!(s.min.is_finite() && s.max.is_finite() && s.major.is_finite());
        assert!(s.frac(1e300).is_finite());
        let s = Scale::new(
            None,
            Some((0.0, super::super::MAX_VALUE)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert!(s.max >= super::super::MAX_VALUE);
        assert!(s.major_ticks().len() <= MAX_TICKS + 1);
    }

    #[test]
    fn explicit_overrides() {
        let a = Axis {
            min: Some(10.0),
            max: Some(50.0),
            major_unit: Some(10.0),
            ..Axis::default()
        };
        let s = Scale::new(
            Some(&a),
            Some((0.0, 100.0)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert_eq!((s.min, s.max, s.major), (10.0, 50.0, 10.0));
        assert_eq!(s.major_ticks(), vec![10.0, 20.0, 30.0, 40.0, 50.0]);
        // min above max is dropped.
        let a = Axis {
            min: Some(50.0),
            max: Some(10.0),
            ..Axis::default()
        };
        let s = Scale::new(
            Some(&a),
            Some((0.0, 20.0)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert!(s.max > s.min);
        // A unit that would give thousands of ticks is ignored.
        let a = Axis {
            major_unit: Some(1e-9),
            ..Axis::default()
        };
        let s = Scale::new(
            Some(&a),
            Some((0.0, 20.0)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert!(s.major_ticks().len() <= MAX_TICKS + 1);
        // Only a max: the min stays automatic.
        let a = Axis {
            max: Some(30.0),
            ..Axis::default()
        };
        let s = Scale::new(
            Some(&a),
            Some((0.0, 20.0)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert_eq!((s.min, s.max), (0.0, 30.0));
    }

    #[test]
    fn percent_axis() {
        let s = Scale::new(
            None,
            Some((0.0, 1.0)),
            Opts {
                len_px: 300.0,
                horizontal: false,
                percent: true,
            },
        );
        assert_eq!((s.min, s.max, s.major), (0.0, 1.0, 0.1));
        assert_eq!(s.major_ticks().len(), 11);
    }

    #[test]
    fn log_axis() {
        let a = Axis {
            log_base: Some(10.0),
            ..Axis::default()
        };
        let s = Scale::new(
            Some(&a),
            Some((3.0, 4000.0)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert_eq!((s.min, s.max), (1.0, 10000.0));
        assert_eq!(s.major_ticks(), vec![1.0, 10.0, 100.0, 1000.0, 10000.0]);
        assert!((s.frac(100.0) - 0.5).abs() < 1e-9);
        // Non-positive values sit below the axis.
        assert!(s.frac(0.0) < 0.0);
        assert!(s.frac(-5.0) < 0.0);
        // Only non-positive data: falls back to a linear axis.
        let s = Scale::new(
            Some(&a),
            Some((-5.0, -1.0)),
            Opts {
                len_px: 200.0,
                horizontal: false,
                percent: false,
            },
        );
        assert!(s.log.is_none());
        let m = s.minor_ticks();
        assert!(m.len() < 1000);
    }

    #[test]
    fn frac_and_reverse() {
        let s = Scale::linear(0.0, 10.0, 5.0);
        assert_eq!(s.frac(0.0), 0.0);
        assert_eq!(s.frac(10.0), 1.0);
        assert_eq!(s.frac(2.5), 0.25);
        let r = Scale {
            reversed: true,
            ..s.clone()
        };
        assert_eq!(r.frac(0.0), 1.0);
        assert_eq!(r.frac(10.0), 0.0);
        assert!(s.frac(f64::NAN).is_finite());
        assert!(s.frac(1e300).is_finite());
    }

    #[test]
    fn tidy_noise() {
        assert_eq!(tidy(0.1 + 0.2), 0.3);
        assert_eq!(tidy(0.0), 0.0);
        assert_eq!(tidy(-0.1 - 0.2), -0.3);
        assert!(tidy(f64::NAN).is_nan());
        assert_eq!(tidy(1e-300), 1e-300);
    }

    #[test]
    fn tick_lists() {
        let s = Scale::linear(0.0, 1.0, 0.1);
        let t = s.major_ticks();
        assert_eq!(t.len(), 11);
        assert_eq!(t[3], 0.3);
        let m = Scale::linear(0.0, 10.0, 5.0).minor_ticks();
        assert_eq!(m, vec![1.0, 2.0, 3.0, 4.0, 6.0, 7.0, 8.0, 9.0]);
        assert_eq!(default_minor(2.0), 0.5);
        assert_eq!(default_minor(0.0), 0.0);
    }

    #[test]
    fn target_tick_counts() {
        assert_eq!(target_ticks(10.0, false), 3.0);
        assert_eq!(target_ticks(10_000.0, false), 10.0);
        assert!((target_ticks(170.0, false) - 5.3125).abs() < 1e-9);
        assert_eq!(target_ticks(f64::NAN, true), 5.0);
    }

    #[test]
    fn span_of_values() {
        assert_eq!(span([3.0, -1.0, 7.0].into_iter()), Some((-1.0, 7.0)));
        assert_eq!(span(std::iter::empty()), None);
    }

    fn lo(lo: f64, hi: f64, y_axis: bool, len_px: f64, horizontal: bool) -> (f64, f64, f64) {
        libre_range(
            lo,
            hi,
            None,
            None,
            Opts {
                len_px,
                horizontal,
                percent: false,
            },
            LibreOpts {
                y_axis,
                label_h: 18.0,
                digit_w: 9.0,
            },
        )
    }

    #[test]
    fn libre_examples_of_the_measured_charts() {
        // The self-made scatter chart: y values 1..9.8 on an axis too short for 11 labels.
        assert_eq!(lo(1.0, 9.8, true, 169.0, false), (0.0, 12.0, 2.0));
        // ... on a tall axis the unit stays 1 (the end is nudged because the data reach it).
        assert_eq!(lo(1.0, 9.8, true, 600.0, false), (0.0, 11.0, 1.0));
        // x values 1..5: not widened to zero, half-unit steps, one more unit at both ends.
        assert_eq!(lo(1.0, 5.0, false, 367.0, true), (0.5, 5.5, 0.5));
        // columns up to 21 / 52
        assert_eq!(lo(6.0, 21.0, true, 150.0, false), (0.0, 25.0, 5.0));
        assert_eq!(lo(0.0, 52.0, true, 150.0, false), (0.0, 60.0, 10.0));
    }

    #[test]
    fn libre_y_axis_rules() {
        // wide data start at zero, narrow data (min above 5/6 of max) half a span below the min
        assert_eq!(lo(10.0, 100.0, true, 400.0, false).0, 0.0);
        let (mn, _, u) = lo(100.0, 105.0, true, 400.0, false);
        assert!(mn < 100.0 && mn > 96.0 && u > 0.0, "{mn} {u}");
        // all values equal: the axis starts at zero and goes past the value
        let (mn, mx, _) = lo(5.0, 5.0, true, 400.0, false);
        assert_eq!(mn, 0.0);
        assert!(mx > 5.0, "{mx}");
        // ... an x axis (not widened to zero) doubles the maximum instead
        let (mn, mx, _) = lo(5.0, 5.0, false, 400.0, true);
        assert!(mn <= 5.0 && mx >= 10.0, "{mn} {mx}");
        let (mn, mx, _) = lo(0.0, 0.0, true, 400.0, false);
        assert!(mn <= 0.0 && mx >= 1.0, "{mn} {mx}");
        // an x axis is not widened to zero
        assert!(lo(100.0, 200.0, false, 400.0, true).0 >= 90.0);
        // negative data mirror the positive ones
        let (mn, mx, u) = lo(-9.8, -1.0, true, 169.0, false);
        assert_eq!((mn, mx, u), (-12.0, 0.0, 2.0));
        // mixed signs: rounded outward to multiples of the unit
        let (mn, mx, u) = lo(-3.0, 7.0, true, 400.0, false);
        assert!(mn <= -3.0 && mx >= 7.0 && (mn / u).fract() == 0.0 && (mx / u).fract() == 0.0);
    }

    #[test]
    fn libre_fixed_limits_are_kept() {
        let o = Opts {
            len_px: 300.0,
            horizontal: false,
            percent: false,
        };
        let l = LibreOpts {
            y_axis: true,
            label_h: 18.0,
            digit_w: 9.0,
        };
        let (mn, mx, u) = libre_range(0.0, 7.0, Some(2.0), Some(9.0), o, l);
        assert_eq!((mn, mx), (2.0, 9.0));
        assert!(u > 0.0);
        let (mn, mx, _) = libre_range(0.0, 7.0, None, Some(8.0), o, l);
        assert_eq!((mn, mx), (0.0, 8.0));
        // fixed values flow through Scale::new_libre too
        let axis = Axis {
            min: Some(1.0),
            max: Some(11.0),
            ..Axis::default()
        };
        let s = Scale::new_libre(Some(&axis), Some((2.0, 8.0)), o, l);
        assert_eq!((s.min, s.max), (1.0, 11.0));
        assert!(s.major > 0.0);
        // percent and logarithmic axes keep the shared rules
        let p = Scale::new_libre(None, Some((0.2, 0.4)), Opts { percent: true, ..o }, l);
        assert_eq!((p.min, p.max), (0.0, 1.0));
        let g = Axis {
            log_base: Some(10.0),
            ..Axis::default()
        };
        let s = Scale::new_libre(Some(&g), Some((3.0, 700.0)), o, l);
        assert_eq!((s.min, s.max), (1.0, 1000.0));
    }

    #[test]
    fn libre_decimals_of_a_step() {
        assert_eq!(decimals_of(1.0), 0);
        assert_eq!(decimals_of(0.5), 1);
        assert_eq!(decimals_of(0.25), 2);
        assert_eq!(decimals_of(0.2), 1);
    }
}
