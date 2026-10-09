//! Data labels: which ones a point has, their text, and where they go.

use super::layout::Rect;
use super::shapes::Out;
use super::text::{HAlign, RStyle, VAlign};
use super::{cut_chars, DataLabels, LabelPos, MAX_LABEL_CHARS};
use crate::preview::office::numfmt::{self, Locale, Options, Value};

/// The labels point `idx` of a series gets, or `None` for no label: the series' settings (or the
/// group's), overridden by the point's own.
pub(super) fn effective(
    group: Option<&DataLabels>,
    series: Option<&DataLabels>,
    idx: usize,
) -> Option<DataLabels> {
    let base = series.or(group)?;
    if base.delete {
        return None;
    }
    let mut dl = base.clone();
    if let Some((_, p)) = base.points.iter().find(|(i, _)| *i == idx) {
        if p.delete {
            return None;
        }
        let mut q = p.clone();
        // A point without its own number format, separator or style inherits the series'.
        if q.num_fmt.is_none() {
            q.num_fmt = base.num_fmt.clone();
        }
        if q.separator.is_none() {
            q.separator = base.separator.clone();
        }
        if q.pos.is_none() {
            q.pos = base.pos;
        }
        q.style = q.style.over(&base.style);
        dl = q;
    }
    if !(dl.show_value
        || dl.show_category
        || dl.show_series
        || dl.show_percent
        || dl.text.is_some())
    {
        return None;
    }
    Some(dl)
}

/// What a label can show about its point.
pub(super) struct Info<'a> {
    pub cat: Option<&'a str>,
    pub series: Option<&'a str>,
    pub value: Option<f64>,
    pub percent: Option<f64>,
    /// The series' own number format code.
    pub source_fmt: Option<&'a str>,
    pub ja: bool,
}

/// Formats a number with an Excel format code (`General` for none).
pub fn format_number(code: Option<&str>, v: f64, ja: bool) -> String {
    let opts = Options {
        locale: if ja { Locale::Ja } else { Locale::En },
        ..Options::default()
    };
    let c = numfmt::compile(code.filter(|c| !c.is_empty()).unwrap_or("General"));
    numfmt::format_compiled(&c, Value::Number(v), &opts)
}

/// The text of a label.
pub(super) fn text(dl: &DataLabels, i: &Info) -> String {
    if let Some(t) = &dl.text {
        let mut t = t.clone();
        cut_chars(&mut t, MAX_LABEL_CHARS);
        return t;
    }
    let mut parts: Vec<String> = Vec::new();
    if dl.show_series {
        if let Some(s) = i.series {
            parts.push(s.to_string());
        }
    }
    if dl.show_category {
        if let Some(c) = i.cat {
            parts.push(c.to_string());
        }
    }
    if dl.show_value {
        if let Some(v) = i.value {
            let code = dl.num_fmt.as_deref().or(i.source_fmt);
            parts.push(format_number(code, v, i.ja));
        }
    }
    if dl.show_percent {
        if let Some(p) = i.percent {
            let code = dl
                .num_fmt
                .as_deref()
                .filter(|c| c.contains('%'))
                .unwrap_or("0%");
            parts.push(format_number(Some(code), p, i.ja));
        }
    }
    let sep = dl.separator.as_deref().unwrap_or(", ");
    let mut s = parts.join(sep);
    cut_chars(&mut s, MAX_LABEL_CHARS);
    s
}

/// Draws a label on a bar (or any rectangle that grows from a base): `vertical` bars grow upwards
/// (columns), `positive` is the sign of the value.
pub(super) fn on_bar(
    o: &mut Out,
    st: &RStyle,
    s: &str,
    r: Rect,
    pos: LabelPos,
    vertical: bool,
    positive: bool,
) {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let g = 3.0;
    let pos = match pos {
        LabelPos::BestFit
        | LabelPos::Above
        | LabelPos::Below
        | LabelPos::Left
        | LabelPos::Right => LabelPos::OutsideEnd,
        p => p,
    };
    if vertical {
        // `top` is the end of the bar in the direction of the value.
        let (end_y, base_y) = if positive {
            (r.y, r.bottom())
        } else {
            (r.bottom(), r.y)
        };
        let dir = if positive { -1.0 } else { 1.0 };
        let (va_out, va_in_end, va_in_base) = if positive {
            (VAlign::Bottom, VAlign::Top, VAlign::Bottom)
        } else {
            (VAlign::Top, VAlign::Bottom, VAlign::Top)
        };
        match pos {
            LabelPos::OutsideEnd => {
                o.text(cx, end_y + dir * 2.0, HAlign::Center, va_out, s, st, 0.0)
            }
            LabelPos::InsideEnd => {
                o.text(cx, end_y - dir * 2.0, HAlign::Center, va_in_end, s, st, 0.0)
            }
            LabelPos::InsideBase => o.text(
                cx,
                base_y + dir * 2.0,
                HAlign::Center,
                va_in_base,
                s,
                st,
                0.0,
            ),
            _ => o.text(cx, cy, HAlign::Center, VAlign::Middle, s, st, 0.0),
        };
    } else {
        let (end_x, base_x) = if positive {
            (r.right(), r.x)
        } else {
            (r.x, r.right())
        };
        let dir = if positive { 1.0 } else { -1.0 };
        let (ha_out, ha_in_end, ha_in_base) = if positive {
            (HAlign::Left, HAlign::Right, HAlign::Left)
        } else {
            (HAlign::Right, HAlign::Left, HAlign::Right)
        };
        match pos {
            LabelPos::OutsideEnd => o.text(end_x + dir * g, cy, ha_out, VAlign::Middle, s, st, 0.0),
            LabelPos::InsideEnd => {
                o.text(end_x - dir * g, cy, ha_in_end, VAlign::Middle, s, st, 0.0)
            }
            LabelPos::InsideBase => {
                o.text(base_x + dir * g, cy, ha_in_base, VAlign::Middle, s, st, 0.0)
            }
            _ => o.text(cx, cy, HAlign::Center, VAlign::Middle, s, st, 0.0),
        };
    }
}

/// Draws a label next to a point `(x, y)` with a marker of radius `r`.
pub(super) fn at_point(o: &mut Out, st: &RStyle, s: &str, x: f64, y: f64, r: f64, pos: LabelPos) {
    let g = r + 3.0;
    match pos {
        LabelPos::Left => o.text(x - g, y, HAlign::Right, VAlign::Middle, s, st, 0.0),
        LabelPos::Above | LabelPos::OutsideEnd | LabelPos::InsideEnd => {
            o.text(x, y - g, HAlign::Center, VAlign::Bottom, s, st, 0.0)
        }
        LabelPos::Below | LabelPos::InsideBase => {
            o.text(x, y + g, HAlign::Center, VAlign::Top, s, st, 0.0)
        }
        LabelPos::Center => o.text(x, y, HAlign::Center, VAlign::Middle, s, st, 0.0),
        LabelPos::Right | LabelPos::BestFit => {
            o.text(x + g, y, HAlign::Left, VAlign::Middle, s, st, 0.0)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dl(val: bool, cat: bool, ser: bool, pct: bool) -> DataLabels {
        DataLabels {
            show_value: val,
            show_category: cat,
            show_series: ser,
            show_percent: pct,
            ..DataLabels::default()
        }
    }

    fn info() -> Info<'static> {
        Info {
            cat: Some("North"),
            series: Some("2022"),
            value: Some(1234.5),
            percent: Some(0.256),
            source_fmt: None,
            ja: false,
        }
    }

    #[test]
    fn label_parts_in_office_order() {
        assert_eq!(text(&dl(true, false, false, false), &info()), "1234.5");
        assert_eq!(
            text(&dl(true, true, true, false), &info()),
            "2022, North, 1234.5"
        );
        assert_eq!(text(&dl(false, false, false, true), &info()), "26%");
        let mut d = dl(true, true, false, false);
        d.separator = Some("\n".into());
        assert_eq!(text(&d, &info()), "North\n1234.5");
    }

    #[test]
    fn label_number_formats() {
        let mut d = dl(true, false, false, false);
        d.num_fmt = Some("#,##0".into());
        assert_eq!(text(&d, &info()), "1,235");
        d.num_fmt = Some("0.0".into());
        assert_eq!(text(&d, &info()), "1234.5");
        // The source format applies when the label has none of its own.
        let mut i = info();
        i.source_fmt = Some("0.00");
        let d = dl(true, false, false, false);
        assert_eq!(text(&d, &i), "1234.50");
        // A percent format on the label applies to the percentage.
        let mut d = dl(false, false, false, true);
        d.num_fmt = Some("0.0%".into());
        assert_eq!(text(&d, &info()), "25.6%");
    }

    #[test]
    fn custom_text_wins_and_is_cut() {
        let mut d = dl(true, false, false, false);
        d.text = Some("x".repeat(1000));
        assert_eq!(text(&d, &info()).chars().count(), MAX_LABEL_CHARS);
    }

    #[test]
    fn point_overrides() {
        let mut series = dl(true, false, false, false);
        series.num_fmt = Some("0.0".into());
        let gone = DataLabels {
            delete: true,
            ..DataLabels::default()
        };
        let mut cat = dl(false, true, false, false);
        cat.pos = None;
        series.points = vec![(1, gone), (2, cat)];
        assert!(effective(None, Some(&series), 0).unwrap().show_value);
        assert!(effective(None, Some(&series), 1).is_none());
        let p2 = effective(None, Some(&series), 2).unwrap();
        assert!(p2.show_category && !p2.show_value);
        assert_eq!(p2.num_fmt.as_deref(), Some("0.0"));
        // Nothing shown = no label; deleted series = none; group labels are inherited.
        assert!(effective(None, Some(&DataLabels::default()), 0).is_none());
        let mut d = dl(true, false, false, false);
        d.delete = true;
        assert!(effective(None, Some(&d), 0).is_none());
        let g = dl(true, false, false, false);
        assert!(effective(Some(&g), None, 5).is_some());
        assert!(effective(Some(&g), Some(&DataLabels::default()), 5).is_none());
        assert!(effective(None, None, 0).is_none());
    }
}
