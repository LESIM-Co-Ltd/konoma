//! Chart text: style resolution, measuring and placing a label.

use super::super::fonts::{self, Script};
use super::super::model::*;
use super::super::text::{LINE_HEIGHT, PX_PER_PT};
use super::shapes::{e, Out};
use super::{ChartModel, TextStyle};

/// A fully resolved text style.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RStyle {
    pub size_pt: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: Rgba,
    pub font: Option<String>,
}

impl RStyle {
    pub fn size_px(&self) -> f64 {
        self.size_pt * PX_PER_PT
    }

    /// Height of one line, px.
    pub fn line_h(&self) -> f64 {
        self.size_px() * LINE_HEIGHT
    }
}

/// Resolves an element's style: the element's own, then the chart's, then the defaults given.
pub(super) fn resolve(m: &ChartModel, own: &TextStyle, def_size: f64, def_bold: bool) -> RStyle {
    let s = own.over(&m.text);
    let size = s
        .size_pt
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(def_size)
        .clamp(1.0, 200.0);
    RStyle {
        size_pt: size,
        bold: s.bold.unwrap_or(def_bold),
        italic: s.italic.unwrap_or(false),
        color: s.color.unwrap_or(Rgba::BLACK),
        font: s.font.or_else(|| m.default_font.clone()),
    }
}

/// Width of one line of text, px.
pub(super) fn line_width(st: &RStyle, text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let size = st.size_px();
    // Chunks of one script each.
    let mut total = 0.0;
    let mut chunk = String::new();
    let mut cur = Script::Latin;
    let flush = |chunk: &mut String, sc: Script, total: &mut f64| {
        if !chunk.is_empty() {
            let stack = fonts::stack_for(st.font.as_deref(), sc);
            *total += fonts::measure(&stack, st.bold, st.italic, chunk, size);
            chunk.clear();
        }
    };
    for c in text.chars() {
        let sc = match fonts::script_of(c) {
            Script::EastAsian => Script::EastAsian,
            _ => Script::Latin,
        };
        if sc != cur {
            flush(&mut chunk, cur, &mut total);
            cur = sc;
        }
        chunk.push(c);
    }
    flush(&mut chunk, cur, &mut total);
    total
}

/// Size (w, h) in px of a possibly multi-line text.
pub(super) fn measure(st: &RStyle, text: &str) -> (f64, f64) {
    let mut w = 0.0f64;
    let mut n = 0usize;
    for l in text.split('\n') {
        w = w.max(line_width(st, l));
        n += 1;
    }
    (w, n as f64 * st.line_h())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VAlign {
    Top,
    Middle,
    Bottom,
}

impl Out {
    /// Draws `text` so that the point `(x, y)` is its `ah` / `av` anchor, then rotates it by
    /// `rot_deg` (clockwise) about that point. Returns the (unrotated) box `(x, y, w, h)`.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        x: f64,
        y: f64,
        ah: HAlign,
        av: VAlign,
        text: &str,
        st: &RStyle,
        rot_deg: f64,
    ) -> (f64, f64, f64, f64) {
        let (tw, th) = measure(st, text);
        let w = tw + 6.0;
        let left = match ah {
            HAlign::Left => x,
            HAlign::Center => x - w / 2.0,
            HAlign::Right => x - w,
        };
        let top = match av {
            VAlign::Top => y,
            VAlign::Middle => y - th / 2.0,
            VAlign::Bottom => y - th,
        };
        let rect = (left, top, w, th);
        if text.is_empty() || !x.is_finite() || !y.is_finite() || !self.spend() {
            return rect;
        }
        let align = match ah {
            HAlign::Left => Align::Left,
            HAlign::Center => Align::Center,
            HAlign::Right => Align::Right,
        };
        let font = FontSpec {
            latin: st.font.as_deref().map(super::super::strings::intern),
            east_asian: st.font.as_deref().map(super::super::strings::intern),
            complex: None,
            symbol: None,
        };
        let paragraphs: Vec<Paragraph> = text
            .split('\n')
            .map(|l| Paragraph {
                align,
                end_size_pt: st.size_pt,
                runs: vec![Run {
                    text: l.to_string(),
                    size_pt: st.size_pt,
                    bold: st.bold,
                    italic: st.italic,
                    font: font.clone(),
                    fill: Fill::Solid(st.color),
                    ..Run::default()
                }],
                ..Paragraph::default()
            })
            .collect();
        let body = TextBody {
            insets: (0.0, 0.0, 0.0, 0.0),
            anchor: Anchor::Middle,
            wrap: false,
            paragraphs,
            ..TextBody::default()
        };
        let (mut cx, mut cy) = (left + w / 2.0, top + th / 2.0);
        let rot = if rot_deg.is_finite() { rot_deg } else { 0.0 };
        if rot != 0.0 {
            let (dx, dy) = (cx - x, cy - y);
            let (s, c) = rot.to_radians().sin_cos();
            cx = x + dx * c - dy * s;
            cy = y + dx * s + dy * c;
        }
        let mut xf = Xfrm::rect(e(cx - w / 2.0), e(cy - th / 2.0), e(w), e(th));
        xf.rot_deg = rot.rem_euclid(360.0);
        let mut s = ShapeItem::new(xf, Geometry::Rect);
        s.text = Some(body);
        self.push(Item::Shape(s));
        rect
    }
}
