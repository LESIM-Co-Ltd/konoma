//! The GDI device context both metafile formats play into: mapping modes, the world transform,
//! pen / brush / font state, paths, clipping, and the SVG each drawing call becomes.
//!
//! All geometry is written in *device* units (the units of the SVG `viewBox`): points are
//! transformed here, so arcs under a rotation stay exact (they are cubic Béziers built in logical
//! space and then mapped) and the parsers never see a matrix.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

use base64::Engine as _;

use super::dib::{Bitmap, BitmapData};
use crate::preview::office::slide_draw::fonts;
use crate::preview::office::slide_draw::svg::esc;

pub(super) type P = (f64, f64);
pub(super) type Rgb = [u8; 3];

/// Largest markup (everything except embedded image data) one metafile may write, bytes.
pub(super) const MAX_MARKUP_BYTES: usize = 8 * 1024 * 1024;
/// Largest total of embedded image data (base64), bytes.
pub(super) const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;
/// Largest path (`BEGINPATH` ... `ENDPATH`) description, bytes.
const MAX_PATH_BYTES: usize = 4 * 1024 * 1024;
/// Deepest `SAVEDC` stack that is kept.
pub(super) const MAX_SAVE_DEPTH: usize = 512;
/// Deepest clip nesting (each `INTERSECTCLIPRECT` adds a level) that is kept.
const MAX_CLIP_DEPTH: usize = 16;
/// Most embedded bitmaps in one metafile.
const MAX_IMAGES: u32 = 4096;
/// Most characters of one text record that are drawn.
pub(super) const MAX_TEXT_CHARS: usize = 2048;
/// Total pixels all bitmaps of one metafile may decode to.
pub(super) const PIXEL_BUDGET: u64 = 64 * 1024 * 1024;
/// Coordinates beyond this are clamped (a hostile transform can make anything infinite).
const COORD_LIMIT: f64 = 1.0e7;

/// A finite number with at most three decimals.
pub(super) fn n(v: f64) -> String {
    let v = if v.is_finite() {
        v.clamp(-COORD_LIMIT, COORD_LIMIT)
    } else {
        0.0
    };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn fin(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(-COORD_LIMIT, COORD_LIMIT)
    } else {
        0.0
    }
}

/// `#rrggbb`.
pub(super) fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

// ----- affine transforms ---------------------------------------------------------------------

/// `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Aff {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Aff {
    pub const ID: Aff = Aff {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn apply(&self, p: P) -> P {
        (
            fin(self.a * p.0 + self.c * p.1 + self.e),
            fin(self.b * p.0 + self.d * p.1 + self.f),
        )
    }

    /// `self` first, then `o`.
    pub fn then(&self, o: &Aff) -> Aff {
        Aff {
            a: o.a * self.a + o.c * self.b,
            b: o.b * self.a + o.d * self.b,
            c: o.a * self.c + o.c * self.d,
            d: o.b * self.c + o.d * self.d,
            e: o.a * self.e + o.c * self.f + o.e,
            f: o.b * self.e + o.d * self.f + o.f,
        }
    }

    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Length a unit step along x becomes.
    pub fn sx(&self) -> f64 {
        self.a.hypot(self.b)
    }

    /// Length a unit step along y becomes.
    pub fn sy(&self) -> f64 {
        self.c.hypot(self.d)
    }

    fn finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|v| v.is_finite() && v.abs() < 1.0e9)
    }
}

// ----- objects -------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Cap {
    Round,
    Square,
    Flat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Join {
    Round,
    Bevel,
    Miter,
}

pub(super) struct Pen {
    /// `PS_SOLID` 0 ... `PS_USERSTYLE` 7 (the low nibble of the style).
    pub style: u32,
    pub cap: Cap,
    pub join: Join,
    /// Logical units.
    pub width: f64,
    /// A cosmetic pen is one device pixel wide at any scale.
    pub cosmetic: bool,
    pub color: Rgb,
    /// `PS_USERSTYLE` dash lengths, logical units.
    pub dashes: Vec<f64>,
}

impl Pen {
    pub fn is_null(&self) -> bool {
        self.style == 5
    }

    pub fn solid(color: Rgb) -> Pen {
        Pen {
            style: 0,
            cap: Cap::Round,
            join: Join::Round,
            width: 1.0,
            cosmetic: true,
            color,
            dashes: Vec::new(),
        }
    }

    pub fn null() -> Pen {
        Pen {
            style: 5,
            ..Pen::solid([0, 0, 0])
        }
    }
}

pub(super) enum Brush {
    Null,
    Solid(Rgb),
    /// `HS_HORIZONTAL` 0 ... `HS_DIAGCROSS` 5.
    Hatch {
        style: u32,
        color: Rgb,
    },
}

pub(super) struct Font {
    /// Logical units; negative = character height, positive = cell height, 0 = default.
    pub height: i32,
    /// Tenths of a degree, counter-clockwise.
    pub escapement: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub charset: u8,
    pub pitch_family: u8,
    pub face: String,
}

impl Font {
    pub fn system() -> Font {
        Font {
            height: 0,
            escapement: 0,
            weight: 400,
            italic: false,
            underline: false,
            strike: false,
            charset: 1,
            pitch_family: 0x20,
            face: "Arial".to_string(),
        }
    }
}

/// A selectable GDI object.
pub(super) enum Obj {
    Pen(Rc<Pen>),
    Brush(Rc<Brush>),
    Font(Rc<Font>),
    /// A logical palette: it colours 8-bit bitmaps that carry no colour table.
    Palette(Rc<Vec<Rgb>>),
    /// A region or colour space: it takes a slot but nothing is drawn with it.
    Other,
}

// ----- device context ------------------------------------------------------------------------

#[derive(Clone)]
pub(super) struct Dc {
    pub world: Aff,
    pub map_mode: u32,
    pub win_org: P,
    pub win_ext: P,
    pub vp_org: P,
    pub vp_ext: P,
    /// The viewport was set explicitly (or frozen); otherwise it follows the window.
    pub vp_set: bool,
    /// Current position, logical.
    pub cur: P,
    pub pen: Rc<Pen>,
    pub brush: Rc<Brush>,
    pub font: Rc<Font>,
    pub palette: Rc<Vec<Rgb>>,
    pub text_color: Rgb,
    pub bk_color: Rgb,
    pub bk_opaque: bool,
    pub winding: bool,
    pub text_align: u32,
    pub rop2: u32,
    pub arc_ccw: bool,
    pub clips: Vec<u32>,
}

impl Dc {
    fn new() -> Dc {
        Dc {
            world: Aff::ID,
            map_mode: 1,
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            vp_set: true,
            cur: (0.0, 0.0),
            pen: Rc::new(Pen::solid([0, 0, 0])),
            brush: Rc::new(Brush::Solid([255, 255, 255])),
            font: Rc::new(Font::system()),
            palette: Rc::new(Vec::new()),
            text_color: [0, 0, 0],
            bk_color: [255, 255, 255],
            bk_opaque: true,
            winding: false,
            text_align: 0,
            rop2: 13,
            arc_ccw: true,
            clips: Vec::new(),
        }
    }
}

struct PathRec {
    d: String,
    /// Device position of the pen inside the path.
    cur: Option<P>,
}

struct Pending {
    d: String,
    last: P,
    attrs: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ArcKind {
    Arc,
    ArcTo,
    Chord,
    Pie,
}

/// One run of text, as the parsers hand it over.
pub(super) struct TextRun {
    pub chars: Vec<char>,
    /// Advance of each character, logical units.
    pub dx: Option<Vec<f64>>,
    pub reference: P,
    /// `ETO_OPAQUE` rectangle (logical l, t, r, b).
    pub opaque_rect: Option<[f64; 4]>,
}

pub(super) struct GdiConfig {
    /// Device units per millimetre (for the fixed mapping modes).
    pub units_per_mm: P,
    /// Device units in one pixel at 96 dpi (hatch pitch, minimum pen width).
    pub pxu: f64,
}

pub(super) struct Gdi {
    pub dc: Dc,
    stack: Vec<Dc>,
    ctm: Aff,
    cfg: GdiConfig,
    body: String,
    defs: String,
    img_len: usize,
    open: Vec<u32>,
    next_id: u32,
    path: Option<PathRec>,
    pending: Option<Pending>,
    hatches: HashMap<(u32, Rgb, Option<Rgb>), u32>,
    pub truncated: bool,
    pub pixels_left: u64,
    images: u32,
    /// Number of shapes, texts and bitmaps drawn.
    pub drawn: u64,
    bounds: Option<[f64; 4]>,
    /// Viewport at the first drawing call (WMF without a placeable header).
    pub view0: Option<(P, P)>,
    /// `SetWindowExt` was called at least once.
    pub win_ext_set: bool,
}

impl Gdi {
    pub fn new(cfg: GdiConfig) -> Gdi {
        let mut g = Gdi {
            dc: Dc::new(),
            stack: Vec::new(),
            ctm: Aff::ID,
            cfg,
            body: String::new(),
            defs: String::new(),
            img_len: 0,
            open: Vec::new(),
            next_id: 0,
            path: None,
            pending: None,
            hatches: HashMap::new(),
            truncated: false,
            pixels_left: PIXEL_BUDGET,
            images: 0,
            drawn: 0,
            bounds: None,
            view0: None,
            win_ext_set: false,
        };
        g.recalc();
        g
    }

    /// The markup so far is over its budget: stop reading records.
    pub fn stopped(&self) -> bool {
        self.body.len() - self.img_len + self.defs.len() > MAX_MARKUP_BYTES
            || self.img_len > MAX_IMAGE_BYTES
    }

    // ----- mapping --------------------------------------------------------------------------

    fn page_aff(&self) -> Aff {
        let dc = &self.dc;
        let upm = self.cfg.units_per_mm;
        let fixed = |mm: f64| (upm.0 * mm, -upm.1 * mm);
        let ratio = |v: f64, w: f64| {
            if w != 0.0 && v.is_finite() {
                v / w
            } else {
                1.0
            }
        };
        let (sx, sy) = match dc.map_mode {
            2 => fixed(0.1),
            3 => fixed(0.01),
            4 => fixed(0.254),
            5 => fixed(0.0254),
            6 => fixed(25.4 / 1440.0),
            7 | 8 => {
                let sx = ratio(dc.vp_ext.0, dc.win_ext.0);
                let sy = ratio(dc.vp_ext.1, dc.win_ext.1);
                if dc.map_mode == 7 {
                    // Isotropic: the smaller scale wins on both axes, keeping the signs.
                    let m = sx.abs().min(sy.abs());
                    (m * sx.signum(), m * sy.signum())
                } else {
                    (sx, sy)
                }
            }
            _ => (1.0, 1.0),
        };
        Aff {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: dc.vp_org.0 - dc.win_org.0 * sx,
            f: dc.vp_org.1 - dc.win_org.1 * sy,
        }
    }

    /// Recomputes the logical-to-device matrix after a mapping or world-transform change.
    pub fn recalc(&mut self) {
        if !self.dc.vp_set {
            self.dc.vp_org = self.dc.win_org;
            self.dc.vp_ext = self.dc.win_ext;
        }
        let m = self.dc.world.then(&self.page_aff());
        self.ctm = if m.finite() { m } else { Aff::ID };
    }

    pub fn ctm(&self) -> Aff {
        self.ctm
    }

    pub fn dev(&self, p: P) -> P {
        self.ctm.apply(p)
    }

    pub fn set_map_mode(&mut self, mode: u32) {
        if (1..=8).contains(&mode) {
            self.dc.map_mode = mode;
            self.recalc();
        }
    }

    pub fn set_window_org(&mut self, p: P) {
        self.dc.win_org = (fin(p.0), fin(p.1));
        self.recalc();
    }

    pub fn set_window_ext(&mut self, p: P) {
        self.win_ext_set = true;
        self.dc.win_ext = (fin(p.0), fin(p.1));
        self.recalc();
    }

    pub fn set_viewport_org(&mut self, p: P) {
        self.freeze_viewport();
        self.dc.vp_org = (fin(p.0), fin(p.1));
        self.recalc();
    }

    pub fn set_viewport_ext(&mut self, p: P) {
        self.freeze_viewport();
        self.dc.vp_ext = (fin(p.0), fin(p.1));
        self.recalc();
    }

    fn freeze_viewport(&mut self) {
        if !self.dc.vp_set {
            self.dc.vp_org = self.dc.win_org;
            self.dc.vp_ext = self.dc.win_ext;
            self.dc.vp_set = true;
        }
    }

    /// `ScaleWindowExt` / `ScaleViewportExt`: `ext = ext * num / den` per axis.
    pub fn scale_ext(&mut self, window: bool, nx: f64, dx: f64, ny: f64, dy: f64) {
        let sc = |e: P| {
            (
                if dx != 0.0 { e.0 * nx / dx } else { e.0 },
                if dy != 0.0 { e.1 * ny / dy } else { e.1 },
            )
        };
        if window {
            self.dc.win_ext = sc(self.dc.win_ext);
        } else {
            self.freeze_viewport();
            self.dc.vp_ext = sc(self.dc.vp_ext);
        }
        self.recalc();
    }

    pub fn offset_org(&mut self, window: bool, dx: f64, dy: f64) {
        if window {
            self.dc.win_org = (self.dc.win_org.0 + dx, self.dc.win_org.1 + dy);
        } else {
            self.freeze_viewport();
            self.dc.vp_org = (self.dc.vp_org.0 + dx, self.dc.vp_org.1 + dy);
        }
        self.recalc();
    }

    pub fn set_world(&mut self, m: Aff) {
        self.dc.world = if m.finite() { m } else { Aff::ID };
        self.recalc();
    }

    /// `MWT_LEFTMULTIPLY` (`left = true`: `x` is applied first) / `MWT_RIGHTMULTIPLY`.
    pub fn modify_world(&mut self, x: Aff, left: bool) {
        let m = if left {
            x.then(&self.dc.world)
        } else {
            self.dc.world.then(&x)
        };
        self.set_world(m);
    }

    // ----- state ----------------------------------------------------------------------------

    pub fn save(&mut self) {
        if self.stack.len() < MAX_SAVE_DEPTH {
            self.stack.push(self.dc.clone());
        } else {
            self.truncated = true;
        }
    }

    /// `RestoreDC(n)`: negative = relative, positive = absolute (1-based).
    pub fn restore(&mut self, n: i64) {
        let len = self.stack.len() as i64;
        let idx = if n < 0 { len + n } else { n - 1 };
        if idx < 0 || idx >= len {
            return;
        }
        let dc = self.stack[idx as usize].clone();
        self.stack.truncate(idx as usize);
        // The current position and the path are not part of a saved state.
        let cur = self.dc.cur;
        self.dc = dc;
        self.dc.cur = cur;
        self.recalc();
    }

    // ----- output plumbing ------------------------------------------------------------------

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn track(&mut self, p: P) {
        match &mut self.bounds {
            Some(b) => {
                b[0] = b[0].min(p.0);
                b[1] = b[1].min(p.1);
                b[2] = b[2].max(p.0);
                b[3] = b[3].max(p.1);
            }
            None => self.bounds = Some([p.0, p.1, p.0, p.1]),
        }
    }

    /// The device bounding box of what was drawn.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        self.bounds
    }

    fn note_draw(&mut self) {
        self.drawn += 1;
        if self.view0.is_none() {
            self.freeze_viewport();
            self.view0 = Some((self.dc.vp_org, self.dc.vp_ext));
        }
    }

    /// Opens / closes the clip groups so the open ones match the current clip.
    fn sync_clips(&mut self) {
        let want = self.dc.clips.clone();
        let common = self
            .open
            .iter()
            .zip(want.iter())
            .take_while(|(a, b)| a == b)
            .count();
        while self.open.len() > common {
            self.body.push_str("</g>");
            self.open.pop();
        }
        for id in &want[common..] {
            let _ = write!(self.body, r#"<g clip-path="url(#c{id})">"#);
            self.open.push(*id);
        }
    }

    /// Emits any line segments still being collected.
    pub fn flush(&mut self) {
        if let Some(p) = self.pending.take() {
            self.sync_clips();
            let _ = write!(self.body, r#"<path d="{}" fill="none"{}/>"#, p.d, p.attrs);
        }
    }

    /// Closes everything and returns `(defs, body)`.
    pub fn finish(mut self) -> (String, String) {
        self.flush();
        while self.open.pop().is_some() {
            self.body.push_str("</g>");
        }
        (self.defs, self.body)
    }

    // ----- paint ----------------------------------------------------------------------------

    fn dev_pen_width(&self, pen: &Pen) -> f64 {
        let px = self.cfg.pxu;
        if pen.cosmetic {
            return px;
        }
        let s = self.ctm.det().abs().sqrt();
        (pen.width.abs() * s).max(px).min(1.0e6)
    }

    fn stroke_attrs(&self) -> Option<String> {
        let pen = &self.dc.pen;
        if pen.is_null() || self.dc.rop2 == 11 {
            return None;
        }
        let w = self.dev_pen_width(pen);
        let mut s = format!(r#" stroke="{}" stroke-width="{}""#, hex(pen.color), n(w));
        let cap = match pen.cap {
            Cap::Round => "round",
            Cap::Square => "square",
            Cap::Flat => "butt",
        };
        let join = match pen.join {
            Join::Round => "round",
            Join::Bevel => "bevel",
            Join::Miter => "miter",
        };
        if pen.cap != Cap::Flat || pen.join != Join::Miter {
            // Cosmetic pens in GDI have flat caps and round joins; with the single-pixel widths
            // involved the difference is invisible, so the pen's own values are used.
            let _ = write!(s, r#" stroke-linecap="{cap}" stroke-linejoin="{join}""#);
        }
        if pen.join == Join::Miter {
            let _ = write!(s, r#" stroke-miterlimit="10""#);
        }
        if let Some(d) = self.dash_array(pen, w) {
            let _ = write!(s, r#" stroke-dasharray="{d}""#);
        }
        Some(s)
    }

    /// Dash lengths: Windows' fixed patterns for single-pixel pens, multiples of the width for
    /// wider ones (the patterns GDI really uses for geometric pens are not documented).
    fn dash_array(&self, pen: &Pen, w: f64) -> Option<String> {
        let px = self.cfg.pxu;
        let pat: Vec<f64> = match pen.style {
            1..=4 if pen.cosmetic || w <= px * 1.5 => match pen.style {
                1 => vec![18.0, 6.0],
                2 => vec![3.0, 3.0],
                3 => vec![9.0, 6.0, 3.0, 6.0],
                _ => vec![9.0, 3.0, 3.0, 3.0, 3.0, 3.0],
            }
            .into_iter()
            .map(|v| v * px)
            .collect(),
            1 => vec![3.0 * w, w],
            2 => vec![w, w],
            3 => vec![3.0 * w, w, w, w],
            4 => vec![3.0 * w, w, w, w, w, w],
            7 => {
                let s = self.ctm.det().abs().sqrt();
                let v: Vec<f64> = pen
                    .dashes
                    .iter()
                    .take(16)
                    .map(|d| d.abs() * if pen.cosmetic { px } else { s })
                    .collect();
                if v.iter().all(|d| *d <= 0.0) {
                    return None;
                }
                v
            }
            8 => vec![px, px],
            _ => return None,
        };
        let pat: Vec<f64> = pat.into_iter().map(|v| v.max(0.0001)).collect();
        // An odd list repeats itself in SVG; GDI's user styles alternate on/off the same way.
        Some(pat.iter().map(|v| n(*v)).collect::<Vec<_>>().join(" "))
    }

    fn hatch_pattern(&mut self, style: u32, fg: Rgb, bg: Option<Rgb>) -> u32 {
        if let Some(id) = self.hatches.get(&(style, fg, bg)) {
            return *id;
        }
        let id = self.id();
        self.hatches.insert((style, fg, bg), id);
        let u = self.cfg.pxu;
        let s = 8.0 * u;
        let mut g = String::new();
        if let Some(bg) = bg {
            let _ = write!(
                g,
                r#"<rect width="{}" height="{}" fill="{}"/>"#,
                n(s),
                n(s),
                hex(bg)
            );
        }
        let h = s / 2.0;
        let mut d = String::new();
        if matches!(style, 0 | 4) {
            let _ = write!(d, "M0 {}H{}", n(h), n(s));
        }
        if matches!(style, 1 | 4) {
            let _ = write!(d, "M{} 0V{}", n(h), n(s));
        }
        if matches!(style, 2 | 5) {
            let _ = write!(d, "M0 0L{} {}", n(s), n(s));
        }
        if matches!(style, 3 | 5) {
            let _ = write!(d, "M0 {}L{} 0", n(s), n(s));
        }
        let _ = write!(
            g,
            r#"<path d="{d}" fill="none" stroke="{}" stroke-width="{}"/>"#,
            hex(fg),
            n(u)
        );
        let _ = write!(
            self.defs,
            r#"<pattern id="h{id}" patternUnits="userSpaceOnUse" width="{}" height="{}">{g}</pattern>"#,
            n(s),
            n(s)
        );
        id
    }

    fn fill_attrs(&mut self) -> Option<String> {
        if self.dc.rop2 == 11 {
            return None;
        }
        let rule = if self.dc.winding {
            "nonzero"
        } else {
            "evenodd"
        };
        let brush = self.dc.brush.clone();
        match &*brush {
            Brush::Null => None,
            Brush::Solid(c) => Some(format!(r#" fill="{}" fill-rule="{rule}""#, hex(*c))),
            Brush::Hatch { style, color } => {
                let bg = self.dc.bk_opaque.then_some(self.dc.bk_color);
                let id = self.hatch_pattern(*style, *color, bg);
                Some(format!(r##" fill="url(#h{id})" fill-rule="{rule}""##))
            }
        }
    }

    // ----- paths and shapes -----------------------------------------------------------------

    pub fn begin_path(&mut self) {
        self.flush();
        self.path = Some(PathRec {
            d: String::new(),
            cur: None,
        });
    }

    pub fn end_path(&mut self) {
        // The recorded path stays available for the fill / stroke / clip records that follow.
    }

    pub fn abort_path(&mut self) {
        self.path = None;
    }

    pub fn close_figure(&mut self) {
        if let Some(p) = &mut self.path {
            if p.d.len() < MAX_PATH_BYTES && !p.d.is_empty() {
                p.d.push('Z');
            }
            p.cur = None;
        }
    }

    fn take_path(&mut self) -> Option<String> {
        self.path.take().map(|p| p.d).filter(|d| !d.is_empty())
    }

    /// `FILLPATH` / `STROKEPATH` / `STROKEANDFILLPATH`.
    pub fn paint_path(&mut self, fill: bool, stroke: bool) {
        self.flush();
        let Some(d) = self.take_path() else { return };
        self.paint(&d, fill, stroke);
    }

    /// Writes a path element with the current brush / pen.
    fn paint(&mut self, d: &str, fill: bool, stroke: bool) {
        let f = if fill { self.fill_attrs() } else { None };
        let s = if stroke { self.stroke_attrs() } else { None };
        if f.is_none() && s.is_none() {
            return;
        }
        self.note_draw();
        self.sync_clips();
        let _ = write!(self.body, r#"<path d="{d}""#);
        match &f {
            Some(f) => self.body.push_str(f),
            None => self.body.push_str(r#" fill="none""#),
        }
        if let Some(s) = &s {
            self.body.push_str(s);
        }
        self.body.push_str("/>");
    }

    /// Adds a finished fragment: into the path being recorded, or as its own element.
    fn put(&mut self, d: String, last: Option<P>, closed: bool) {
        if let Some(p) = &mut self.path {
            if p.d.len() + d.len() > MAX_PATH_BYTES {
                self.truncated = true;
                return;
            }
            p.d.push_str(&d);
            p.cur = last;
            return;
        }
        self.flush();
        self.paint(&d, closed, true);
    }

    fn same(a: P, b: P) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    /// `M` unless the path being recorded already is at `from`.
    fn start(&self, d: &mut String, from: P, continues: bool) {
        let cont = continues
            && self
                .path
                .as_ref()
                .and_then(|p| p.cur)
                .is_some_and(|c| Self::same(c, from));
        if !cont {
            let _ = write!(d, "M{} {}", n(from.0), n(from.1));
        }
    }

    fn dpt(&mut self, p: P) -> P {
        let q = self.dev(p);
        self.track(q);
        q
    }

    pub fn move_to(&mut self, p: P) {
        self.dc.cur = p;
        if self.path.is_some() {
            // A new figure starts at the next drawing call.
            if let Some(pr) = &mut self.path {
                pr.cur = None;
            }
        }
    }

    pub fn line_to(&mut self, p: P) {
        let from = self.dc.cur;
        self.dc.cur = p;
        if self.dc.rop2 == 11 {
            return;
        }
        let a = self.dpt(from);
        let b = self.dpt(p);
        if self.path.is_some() {
            let mut d = String::new();
            self.start(&mut d, a, true);
            let _ = write!(d, "L{} {}", n(b.0), n(b.1));
            self.put(d, Some(b), false);
            return;
        }
        let Some(attrs) = self.stroke_attrs() else {
            return;
        };
        if let Some(pd) = &mut self.pending {
            if Self::same(pd.last, a) && pd.attrs == attrs && pd.d.len() < 65536 {
                let _ = write!(pd.d, "L{} {}", n(b.0), n(b.1));
                pd.last = b;
                return;
            }
        }
        self.flush();
        self.note_draw();
        self.pending = Some(Pending {
            d: format!("M{} {}L{} {}", n(a.0), n(a.1), n(b.0), n(b.1)),
            last: b,
            attrs,
        });
    }

    /// `POLYLINE` (`to = false`) / `POLYLINETO`.
    pub fn polyline(&mut self, pts: &[P], to: bool) {
        if pts.is_empty() || (!to && pts.len() < 2) {
            return;
        }
        let mut d = String::new();
        let mut last = None;
        let mut iter = pts.iter();
        if to {
            let c = self.dpt(self.dc.cur);
            self.start(&mut d, c, true);
        } else {
            let a = self.dpt(*iter.next().unwrap_or(&(0.0, 0.0)));
            let _ = write!(d, "M{} {}", n(a.0), n(a.1));
        }
        for p in iter {
            let q = self.dpt(*p);
            let _ = write!(d, "L{} {}", n(q.0), n(q.1));
            last = Some(q);
        }
        if let Some(p) = pts.last() {
            self.dc.cur = *p;
        }
        self.put(d, last, false);
    }

    pub fn polygon(&mut self, pts: &[P]) {
        if pts.len() < 2 {
            return;
        }
        let mut d = String::new();
        self.poly_subpath(&mut d, pts, true);
        self.put(d, None, true);
    }

    fn poly_subpath(&mut self, d: &mut String, pts: &[P], close: bool) {
        for (i, p) in pts.iter().enumerate() {
            let q = self.dpt(*p);
            let _ = write!(d, "{}{} {}", if i == 0 { "M" } else { "L" }, n(q.0), n(q.1));
        }
        if close {
            d.push('Z');
        }
    }

    /// `POLYPOLYGON` (closed, filled) / `POLYPOLYLINE`.
    pub fn poly_poly(&mut self, polys: &[Vec<P>], closed: bool) {
        let mut d = String::new();
        for p in polys {
            if p.len() >= 2 {
                self.poly_subpath(&mut d, p, closed);
            }
        }
        if !d.is_empty() {
            self.put(d, None, closed);
        }
    }

    /// `POLYBEZIER` (`to = false`: first point is the start) / `POLYBEZIERTO`.
    pub fn poly_bezier(&mut self, pts: &[P], to: bool) {
        let mut d = String::new();
        let mut rest = pts;
        if to {
            let c = self.dpt(self.dc.cur);
            self.start(&mut d, c, true);
        } else {
            let Some((first, r)) = pts.split_first() else {
                return;
            };
            let a = self.dpt(*first);
            let _ = write!(d, "M{} {}", n(a.0), n(a.1));
            rest = r;
        }
        let mut last = None;
        for c in rest.as_chunks::<3>().0 {
            let (p1, p2, p3) = (self.dpt(c[0]), self.dpt(c[1]), self.dpt(c[2]));
            let _ = write!(
                d,
                "C{} {} {} {} {} {}",
                n(p1.0),
                n(p1.1),
                n(p2.0),
                n(p2.1),
                n(p3.0),
                n(p3.1)
            );
            last = Some(p3);
            self.dc.cur = c[2];
        }
        if last.is_some() {
            self.put(d, last, false);
        }
    }

    fn norm_rect(r: [f64; 4]) -> [f64; 4] {
        [
            r[0].min(r[2]),
            r[1].min(r[3]),
            r[0].max(r[2]),
            r[1].max(r[3]),
        ]
    }

    pub fn rectangle(&mut self, r: [f64; 4]) {
        let [l, t, rr, b] = Self::norm_rect(r);
        self.poly_closed(&[(l, t), (rr, t), (rr, b), (l, b)]);
    }

    fn poly_closed(&mut self, pts: &[P]) {
        let mut d = String::new();
        self.poly_subpath(&mut d, pts, true);
        self.put(d, None, true);
    }

    /// Cubic Béziers for the arc `t0 .. t0 + sweep` of an ellipse, appended to `d` (the start point
    /// is not written).
    fn arc_curves(&mut self, d: &mut String, c: P, rx: f64, ry: f64, t0: f64, sweep: f64) -> P {
        let segs = ((sweep.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 4);
        let da = sweep / segs as f64;
        let k = 4.0 / 3.0 * (da / 4.0).tan();
        let mut end = c;
        for i in 0..segs {
            let a = t0 + da * i as f64;
            let b = a + da;
            let p1 = (
                c.0 + rx * (a.cos() - k * a.sin()),
                c.1 + ry * (a.sin() + k * a.cos()),
            );
            let p2 = (
                c.0 + rx * (b.cos() + k * b.sin()),
                c.1 + ry * (b.sin() - k * b.cos()),
            );
            let p3 = (c.0 + rx * b.cos(), c.1 + ry * b.sin());
            let (q1, q2, q3) = (self.dpt(p1), self.dpt(p2), self.dpt(p3));
            let _ = write!(
                d,
                "C{} {} {} {} {} {}",
                n(q1.0),
                n(q1.1),
                n(q2.0),
                n(q2.1),
                n(q3.0),
                n(q3.1)
            );
            end = p3;
        }
        end
    }

    pub fn ellipse(&mut self, r: [f64; 4]) {
        let [l, t, rr, b] = Self::norm_rect(r);
        let (rx, ry) = ((rr - l) / 2.0, (b - t) / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let c = ((l + rr) / 2.0, (t + b) / 2.0);
        let s = self.dpt((c.0 + rx, c.1));
        let mut d = format!("M{} {}", n(s.0), n(s.1));
        self.arc_curves(&mut d, c, rx, ry, 0.0, std::f64::consts::TAU);
        d.push('Z');
        self.put(d, None, true);
    }

    pub fn round_rect(&mut self, r: [f64; 4], corner: P) {
        let [l, t, rr, b] = Self::norm_rect(r);
        let rx = (corner.0.abs() / 2.0).min((rr - l) / 2.0);
        let ry = (corner.1.abs() / 2.0).min((b - t) / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            self.rectangle(r);
            return;
        }
        let q = std::f64::consts::FRAC_PI_2;
        let mut d = String::new();
        let s = self.dpt((l + rx, t));
        let _ = write!(d, "M{} {}", n(s.0), n(s.1));
        let steps: [(P, P, f64); 4] = [
            ((rr - rx, t), (rr - rx, t + ry), -q),
            ((rr, b - ry), (rr - rx, b - ry), 0.0),
            ((l + rx, b), (l + rx, b - ry), q),
            ((l, t + ry), (l + rx, t + ry), 2.0 * q),
        ];
        for (line_to, centre, t0) in steps {
            let p = self.dpt(line_to);
            let _ = write!(d, "L{} {}", n(p.0), n(p.1));
            self.arc_curves(&mut d, centre, rx, ry, t0, q);
        }
        d.push('Z');
        self.put(d, None, true);
    }

    /// `ARC` / `ARCTO` / `CHORD` / `PIE` within box `r`, from the ray towards `start` to the ray
    /// towards `end`, in the current arc direction.
    pub fn arc(&mut self, kind: ArcKind, r: [f64; 4], start: P, end: P) {
        let [l, t, rr, b] = Self::norm_rect(r);
        let (rx, ry) = ((rr - l) / 2.0, (b - t) / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let c = ((l + rr) / 2.0, (t + b) / 2.0);
        let ang = |p: P| ((p.1 - c.1) / ry).atan2((p.0 - c.0) / rx);
        let (t0, t1) = (ang(start), ang(end));
        let tau = std::f64::consts::TAU;
        // Increasing t turns clockwise on screen when the mapping keeps orientation.
        let dir = if self.dc.arc_ccw == (self.ctm.det() > 0.0) {
            -1.0
        } else {
            1.0
        };
        let mut sweep = (t1 - t0) * dir;
        sweep = sweep.rem_euclid(tau);
        if sweep < 1e-9 {
            sweep = tau;
        }
        let sweep = sweep * dir;
        let a = (c.0 + rx * t0.cos(), c.1 + ry * t0.sin());
        let a_dev = self.dpt(a);
        let mut d = String::new();
        match kind {
            ArcKind::ArcTo => {
                let from = self.dpt(self.dc.cur);
                self.start(&mut d, from, true);
                let _ = write!(d, "L{} {}", n(a_dev.0), n(a_dev.1));
            }
            ArcKind::Pie => {
                let cd = self.dpt(c);
                let _ = write!(d, "M{} {}L{} {}", n(cd.0), n(cd.1), n(a_dev.0), n(a_dev.1));
            }
            _ => {
                let _ = write!(d, "M{} {}", n(a_dev.0), n(a_dev.1));
            }
        }
        let e = self.arc_curves(&mut d, c, rx, ry, t0, sweep);
        let e_dev = self.dpt(e);
        match kind {
            ArcKind::Arc => self.put(d, Some(e_dev), false),
            ArcKind::ArcTo => {
                self.dc.cur = e;
                self.put(d, Some(e_dev), false);
            }
            ArcKind::Chord | ArcKind::Pie => {
                d.push('Z');
                self.put(d, None, true);
            }
        }
    }

    /// `ANGLEARC`: a line to the circle, then the arc (degrees counter-clockwise on screen).
    pub fn angle_arc(&mut self, centre: P, radius: f64, start_deg: f64, sweep_deg: f64) {
        if !(radius.is_finite() && start_deg.is_finite() && sweep_deg.is_finite()) || radius <= 0.0
        {
            return;
        }
        let s = self.ctm.det().abs().sqrt();
        let r = radius * s;
        let c = self.dpt(centre);
        let t0 = -start_deg.to_radians();
        let sweep = (-sweep_deg).clamp(-360.0, 360.0).to_radians();
        let p0 = (c.0 + r * t0.cos(), c.1 + r * t0.sin());
        let mut d = String::new();
        let from = self.dpt(self.dc.cur);
        self.start(&mut d, from, true);
        let _ = write!(d, "L{} {}", n(p0.0), n(p0.1));
        // The curves are built in device space: a circle stays a circle.
        let saved = self.ctm;
        self.ctm = Aff::ID;
        let e = self.arc_curves(&mut d, c, r, r, t0, sweep);
        self.ctm = saved;
        self.track(e);
        // Back to logical for the pen position.
        let inv = self.invert(e);
        self.dc.cur = inv;
        self.put(d, Some(e), false);
    }

    fn invert(&self, p: P) -> P {
        let m = self.ctm;
        let det = m.det();
        if det.abs() < 1e-12 {
            return self.dc.cur;
        }
        let (x, y) = (p.0 - m.e, p.1 - m.f);
        ((m.d * x - m.c * y) / det, (-m.b * x + m.a * y) / det)
    }

    // ----- clipping -------------------------------------------------------------------------

    pub fn reset_clip(&mut self) {
        self.dc.clips.clear();
    }

    /// Intersects the clip with `d` (a device path); `evenodd` for rectangles-with-holes.
    fn clip_with(&mut self, d: &str, evenodd: bool, replace: bool) {
        let id = self.id();
        let rule = if evenodd {
            r#" clip-rule="evenodd""#
        } else {
            ""
        };
        let _ = write!(
            self.defs,
            r#"<clipPath id="c{id}"><path d="{d}"{rule}/></clipPath>"#
        );
        if replace {
            self.dc.clips.clear();
        }
        if self.dc.clips.len() >= MAX_CLIP_DEPTH {
            self.dc.clips.pop();
            self.truncated = true;
        }
        self.dc.clips.push(id);
    }

    fn rect_path(&self, r: [f64; 4]) -> String {
        let [l, t, rr, b] = Self::norm_rect(r);
        let mut d = String::new();
        for (i, p) in [(l, t), (rr, t), (rr, b), (l, b)].iter().enumerate() {
            let q = self.dev(*p);
            let _ = write!(d, "{}{} {}", if i == 0 { "M" } else { "L" }, n(q.0), n(q.1));
        }
        d.push('Z');
        d
    }

    pub fn intersect_clip_rect(&mut self, r: [f64; 4]) {
        self.flush();
        let d = self.rect_path(r);
        self.clip_with(&d, false, false);
    }

    pub fn exclude_clip_rect(&mut self, r: [f64; 4]) {
        self.flush();
        let hole = self.rect_path(r);
        let big = format!("M-{c} -{c}H{c}V{c}H-{c}Z{hole}", c = n(COORD_LIMIT / 2.0));
        self.clip_with(&big, true, false);
    }

    /// `SelectClipPath` / `ExtSelectClipRgn` with a region mode: 1 = AND, 2 = OR, 3 = XOR,
    /// 4 = DIFF, 5 = COPY. `d` is a device path.
    pub fn clip_mode(&mut self, d: &str, mode: u32, evenodd: bool) {
        self.flush();
        match mode {
            4 => {
                let big = format!("M-{c} -{c}H{c}V{c}H-{c}Z{d}", c = n(COORD_LIMIT / 2.0));
                self.clip_with(&big, true, false);
            }
            // The union and the exclusive-or with an earlier clip are not built: the new shape
            // replaces it.
            2 | 3 | 5 => self.clip_with(d, evenodd, true),
            _ => self.clip_with(d, evenodd, false),
        }
    }

    /// `SELECTCLIPPATH`.
    pub fn select_clip_path(&mut self, mode: u32) {
        let evenodd = !self.dc.winding;
        if let Some(d) = self.take_path() {
            self.clip_mode(&d, mode, evenodd);
        }
    }

    /// A region made of logical rectangles as a device path.
    pub fn rects_path(&self, rects: &[[f64; 4]]) -> String {
        rects.iter().map(|r| self.rect_path(*r)).collect()
    }

    // ----- fills without a shape record -----------------------------------------------------

    /// Fills logical rectangles with the current brush (`PaintRgn`, `BitBlt` with `PATCOPY`).
    pub fn fill_rects(&mut self, rects: &[[f64; 4]]) {
        self.flush();
        let d = self.rects_path(rects);
        for r in rects {
            let [l, t, rr, b] = Self::norm_rect(*r);
            for p in [(l, t), (rr, b)] {
                self.dpt(p);
            }
        }
        self.paint(&d, true, false);
    }

    /// Fills a device-space path with the current brush.
    pub fn fill_device_path(&mut self, d: &str) {
        self.flush();
        self.paint(d, true, false);
    }

    /// Fills a logical rectangle with a plain colour.
    pub fn fill_rect_color(&mut self, r: [f64; 4], c: Rgb) {
        self.flush();
        let d = self.rect_path(r);
        let [l, t, rr, b] = Self::norm_rect(r);
        for p in [(l, t), (rr, b)] {
            self.dpt(p);
        }
        self.note_draw();
        self.sync_clips();
        let _ = write!(self.body, r#"<path d="{d}" fill="{}"/>"#, hex(c));
    }

    /// Fills a logical rectangle with a linear gradient.
    pub fn gradient_rect(&mut self, r: [f64; 4], from: Rgb, to: Rgb, horizontal: bool) {
        self.flush();
        let d = self.rect_path(r);
        let [l, t, rr, b] = Self::norm_rect(r);
        let (p0, p1) = (self.dpt((l, t)), self.dpt((rr, b)));
        let id = self.id();
        let (x2, y2) = if horizontal {
            (p1.0, p0.1)
        } else {
            (p0.0, p1.1)
        };
        let _ = write!(
            self.defs,
            r#"<linearGradient id="g{id}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
            n(p0.0),
            n(p0.1),
            n(x2),
            n(y2),
            hex(from),
            hex(to)
        );
        self.note_draw();
        self.sync_clips();
        let _ = write!(self.body, r##"<path d="{d}" fill="url(#g{id})"/>"##);
    }

    // ----- text -----------------------------------------------------------------------------

    pub fn text(&mut self, run: TextRun) {
        self.flush();
        // `ETO_OPAQUE` fills its rectangle with the background colour, with or without text.
        if let Some(rc) = run.opaque_rect {
            let bk = self.dc.bk_color;
            self.fill_rect_color(rc, bk);
        }
        if run.chars.is_empty() {
            return;
        }
        let font = self.dc.font.clone();
        let m = self.ctm;
        let em_logical = match font.height {
            0 => 16.0,
            h if h < 0 => -(h as f64),
            h => h as f64 * 0.9,
        };
        let em = (em_logical * m.sy()).clamp(0.01, 1.0e5);
        let sx = m.sx();
        let est: f64 = run
            .chars
            .iter()
            .map(|c| match *c {
                ' ' => 0.28,
                c if fonts::script_of(c) == fonts::Script::EastAsian => 1.0,
                _ => 0.55,
            })
            .sum::<f64>()
            * em;
        let width = match &run.dx {
            Some(dx) => dx.iter().sum::<f64>().abs() * sx,
            None => est,
        };
        let align = self.dc.text_align;
        let origin = if align & 1 != 0 {
            self.dc.cur
        } else {
            run.reference
        };
        let r = self.dpt(origin);
        let hoff = match align & 6 {
            6 => -width / 2.0,
            2 => -width,
            _ => 0.0,
        };
        let voff = match align & 24 {
            24 => 0.0,
            8 => -0.21 * em,
            _ => 0.9 * em,
        };
        let angle = m.b.atan2(m.a).to_degrees() - font.escapement as f64 / 10.0;
        let angle = if angle.is_finite() { angle } else { 0.0 };

        self.note_draw();
        self.sync_clips();
        let tf = format!("translate({} {}) rotate({})", n(r.0), n(r.1), n(angle));
        let mut s = format!(r#"<g transform="{tf}">"#);
        let stack = {
            let script = run
                .chars
                .iter()
                .map(|c| fonts::script_of(*c))
                .find(|sc| *sc != fonts::Script::Latin)
                .unwrap_or(fonts::Script::Latin);
            let face = font.face.trim();
            let mut st = fonts::stack_for(if face.is_empty() { None } else { Some(face) }, script);
            let generic = match font.pitch_family & 0xF0 {
                0x10 => "serif",
                0x30 => "monospace",
                _ => "sans-serif",
            };
            if !st.iter().any(|f| f == generic) {
                st.push(generic.to_string());
            }
            st
        };
        let mut attrs = format!(
            r#" font-family="{}" font-size="{}" fill="{}""#,
            esc(&fonts::css_family(&stack)),
            n(em),
            hex(self.dc.text_color)
        );
        let weight = ((font.weight.clamp(100, 900) + 50) / 100) * 100;
        if weight != 400 && font.weight > 0 {
            let _ = write!(attrs, r#" font-weight="{weight}""#);
        }
        if font.italic {
            attrs.push_str(r#" font-style="italic""#);
        }
        match (font.underline, font.strike) {
            (true, true) => attrs.push_str(r#" text-decoration="underline line-through""#),
            (true, false) => attrs.push_str(r#" text-decoration="underline""#),
            (false, true) => attrs.push_str(r#" text-decoration="line-through""#),
            _ => {}
        }
        let text: String = run
            .chars
            .iter()
            .map(|c| match *c {
                c if (c as u32) < 0x20 || matches!(c as u32, 0x7F..=0x9F | 0xFFFE | 0xFFFF) => ' ',
                c => c,
            })
            .collect();
        let xs = match &run.dx {
            Some(dx) if dx.len() == run.chars.len() => {
                let mut x = hoff;
                let mut v = Vec::with_capacity(dx.len());
                for d in dx {
                    v.push(n(x));
                    x += d * sx;
                }
                v.join(" ")
            }
            _ => {
                match align & 6 {
                    6 => attrs.push_str(r#" text-anchor="middle""#),
                    2 => attrs.push_str(r#" text-anchor="end""#),
                    _ => {}
                }
                "0".to_string()
            }
        };
        let _ = write!(
            s,
            r#"<text xml:space="preserve" x="{xs}" y="{}"{attrs}>{}</text></g>"#,
            n(voff),
            esc(&text)
        );
        self.body.push_str(&s);
        if align & 1 != 0 {
            // The pen position moves by the width of the text, along x.
            let adv = if sx > 0.0 { width / sx } else { 0.0 };
            self.dc.cur.0 += adv;
        }
    }

    // ----- bitmaps --------------------------------------------------------------------------

    /// Draws `bmp` (or the part `src` of it) into the logical rectangle `dst` (x, y, w, h; a
    /// negative extent mirrors).
    pub fn bitmap(
        &mut self,
        bmp: &Bitmap,
        src: Option<(i64, i64, i64, i64)>,
        dst: [f64; 4],
        blend: Option<&'static str>,
        opacity: f64,
    ) {
        self.flush();
        if self.images >= MAX_IMAGES {
            self.truncated = true;
            return;
        }
        let (sw, sh) = (bmp.w as i64, bmp.h as i64);
        let (sx, sy, cw, ch) = src.unwrap_or((0, 0, sw, sh));
        // An empty or inverted source rectangle: the whole bitmap.
        let (sx, sy, cw, ch) = if cw <= 0 || ch <= 0 {
            (0, 0, sw, sh)
        } else {
            (sx, sy, cw, ch)
        };
        let whole = sx <= 0 && sy <= 0 && sx + cw >= sw && sy + ch >= sh;
        let (cropped, view): (Option<Bitmap>, (f64, f64, f64, f64)) = if whole {
            (None, (0.0, 0.0, sw as f64, sh as f64))
        } else if let Some(c) = bmp.crop(sx, sy, cw, ch) {
            let (w, h) = (c.w as f64, c.h as f64);
            (Some(c), (0.0, 0.0, w, h))
        } else if matches!(bmp.data, BitmapData::Rgba(_)) {
            return;
        } else {
            (None, (sx as f64, sy as f64, cw as f64, ch as f64))
        };
        let b = cropped.as_ref().unwrap_or(bmp);
        let Some((mime, bytes)) = b.to_encoded() else {
            return;
        };
        let uri_len = bytes.len() / 3 * 4 + 64;
        if self.img_len + uri_len > MAX_IMAGE_BYTES {
            self.truncated = true;
            return;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let [dx, dy, dw, dh] = dst;
        if !(dx.is_finite() && dy.is_finite() && dw.is_finite() && dh.is_finite()) {
            return;
        }
        // Local frame: the destination rectangle, normalised; mirrored by a flip about its centre.
        let (x0, y0) = (dx.min(dx + dw), dy.min(dy + dh));
        let (w, h) = (dw.abs(), dh.abs());
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let mut local = Aff::ID;
        if dw < 0.0 {
            local = local.then(&Aff {
                a: -1.0,
                e: 2.0 * x0 + w,
                ..Aff::ID
            });
        }
        if dh < 0.0 {
            local = local.then(&Aff {
                d: -1.0,
                f: 2.0 * y0 + h,
                ..Aff::ID
            });
        }
        let m = local.then(&self.ctm);
        // The picture is stretched so that `view` fills the rectangle.
        let (kx, ky) = (w / view.2, h / view.3);
        let (ix, iy) = (x0 - view.0 * kx, y0 - view.1 * ky);
        let (iw, ih) = (b.w as f64 * kx, b.h as f64 * ky);
        for p in [(x0, y0), (x0 + w, y0 + h)] {
            self.dpt(p);
        }
        self.note_draw();
        self.sync_clips();
        let mut attrs = String::new();
        let clip = if cropped.is_none() && !whole {
            let id = self.id();
            let _ = write!(
                self.defs,
                r#"<clipPath id="i{id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#,
                n(x0),
                n(y0),
                n(w),
                n(h)
            );
            format!(r##" clip-path="url(#i{id})""##)
        } else {
            String::new()
        };
        if let Some(bm) = blend {
            let _ = write!(attrs, r#" style="mix-blend-mode:{bm}""#);
        }
        if opacity < 1.0 {
            let _ = write!(attrs, r#" opacity="{}""#, n(opacity.clamp(0.0, 1.0)));
        }
        let _ = write!(
            self.body,
            r#"<image transform="matrix({} {} {} {} {} {})" x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none"{clip}{attrs} href="data:{mime};base64,{b64}"/>"#,
            n(m.a),
            n(m.b),
            n(m.c),
            n(m.d),
            n(m.e),
            n(m.f),
            n(ix),
            n(iy),
            n(iw),
            n(ih),
        );
        self.img_len += b64.len();
        self.images += 1;
    }
}

// ----- object constructors shared by the parsers ---------------------------------------------

/// A pen from its GDI fields. `ext` = created by `ExtCreatePen` (the style carries the type, cap
/// and join bits); `CreatePen` pens wider than one unit are always solid.
pub(super) fn make_pen(style: u32, width: f64, color: Rgb, ext: bool, dashes: Vec<f64>) -> Pen {
    let base = style & 0xF;
    let (cosmetic, base) = if ext {
        (style & 0xF_0000 == 0, base)
    } else if width > 1.0 {
        (false, if (1..=4).contains(&base) { 0 } else { base })
    } else {
        (true, base)
    };
    Pen {
        style: base,
        cap: match style & 0xF00 {
            0x100 => Cap::Square,
            0x200 => Cap::Flat,
            _ => Cap::Round,
        },
        join: match style & 0xF000 {
            0x1000 => Join::Bevel,
            0x2000 => Join::Miter,
            _ => Join::Round,
        },
        width: if width.is_finite() { width } else { 1.0 },
        cosmetic,
        color,
        dashes,
    }
}

/// A brush from `LOGBRUSH` fields. Pattern brushes (`BS_PATTERN` and the DIB kinds) have no
/// bitmap here: they are a mid-grey; the DIB ones are approximated by the caller.
pub(super) fn make_brush(style: u32, color: Rgb, hatch: u32) -> Brush {
    match style {
        0 | 4 => Brush::Solid(color),
        1 => Brush::Null,
        2 => Brush::Hatch {
            style: hatch.min(5),
            color,
        },
        _ => Brush::Solid([128, 128, 128]),
    }
}

/// The stock brush / pen / font object `n` (`GetStockObject`), if it is one.
pub(super) fn stock_object(n: u32) -> Option<Obj> {
    Some(match n {
        0 => Obj::Brush(Rc::new(Brush::Solid([255, 255, 255]))),
        1 => Obj::Brush(Rc::new(Brush::Solid([192, 192, 192]))),
        2 => Obj::Brush(Rc::new(Brush::Solid([128, 128, 128]))),
        3 => Obj::Brush(Rc::new(Brush::Solid([64, 64, 64]))),
        4 => Obj::Brush(Rc::new(Brush::Solid([0, 0, 0]))),
        5 => Obj::Brush(Rc::new(Brush::Null)),
        6 => Obj::Pen(Rc::new(Pen::solid([255, 255, 255]))),
        7 => Obj::Pen(Rc::new(Pen::solid([0, 0, 0]))),
        8 => Obj::Pen(Rc::new(Pen::null())),
        10 | 11 | 16 => Obj::Font(Rc::new(Font {
            pitch_family: 0x31,
            face: "Courier New".to_string(),
            ..Font::system()
        })),
        12..=14 | 17 => Obj::Font(Rc::new(Font::system())),
        18 => Obj::Brush(Rc::new(Brush::Solid([255, 255, 255]))),
        19 => Obj::Pen(Rc::new(Pen::solid([0, 0, 0]))),
        _ => return None,
    })
}

impl Gdi {
    pub fn select(&mut self, o: &Obj) {
        self.flush();
        match o {
            Obj::Pen(p) => self.dc.pen = p.clone(),
            Obj::Brush(b) => self.dc.brush = b.clone(),
            Obj::Font(f) => self.dc.font = f.clone(),
            Obj::Palette(p) => self.dc.palette = p.clone(),
            Obj::Other => {}
        }
    }

    /// A region of device-space rectangles as a path.
    pub fn device_rects_path(rects: &[[f64; 4]]) -> String {
        let mut d = String::new();
        for r in rects {
            let [l, t, rr, b] = Self::norm_rect(*r);
            let _ = write!(d, "M{} {}H{}V{}H{}Z", n(l), n(t), n(rr), n(b), n(l));
        }
        d
    }

    /// Fills a logical polygon with a plain colour and no outline.
    pub fn fill_polygon_color(&mut self, pts: &[P], c: Rgb) {
        self.flush();
        let brush = std::mem::replace(&mut self.dc.brush, Rc::new(Brush::Solid(c)));
        let pen = std::mem::replace(&mut self.dc.pen, Rc::new(Pen::null()));
        self.polygon(pts);
        self.dc.brush = brush;
        self.dc.pen = pen;
    }
}
