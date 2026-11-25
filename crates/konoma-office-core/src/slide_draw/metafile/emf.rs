//! Enhanced Metafile (MS-EMF) records played into the GDI device context.

use std::collections::HashMap;
use std::rc::Rc;

use super::dib::{self, Opts};
use super::gdi::{
    make_brush, make_pen, stock_object, Aff, ArcKind, Brush, Font, Gdi, GdiConfig, Obj, TextRun,
    MAX_TEXT_CHARS, P,
};
use super::{
    assemble, blend_of, colorref, decode_text, f32le, i16le, i32le, region_rects, u16le, u32le,
    MetaSvg, MAX_OBJECTS, MAX_POINTS, MAX_RECORDS,
};

/// `EMR_HEADER` with the ` EMF` signature.
pub(super) fn is_emf(b: &[u8]) -> bool {
    u32le(b, 0) == Some(1) && u32le(b, 40) == Some(0x464D_4520) && b.len() >= 88
}

fn rect(r: &[u8], o: usize) -> Option<[f64; 4]> {
    Some([
        i32le(r, o)? as f64,
        i32le(r, o + 4)? as f64,
        i32le(r, o + 8)? as f64,
        i32le(r, o + 12)? as f64,
    ])
}

fn pt(r: &[u8], o: usize) -> Option<P> {
    Some((i32le(r, o)? as f64, i32le(r, o + 4)? as f64))
}

fn points32(r: &[u8], o: usize, count: usize) -> Option<Vec<P>> {
    if count > MAX_POINTS || o.checked_add(count.checked_mul(8)?)? > r.len() {
        return None;
    }
    (0..count).map(|i| pt(r, o + i * 8)).collect()
}

fn points16(r: &[u8], o: usize, count: usize) -> Option<Vec<P>> {
    if count > MAX_POINTS || o.checked_add(count.checked_mul(4)?)? > r.len() {
        return None;
    }
    (0..count)
        .map(|i| Some((i16le(r, o + i * 4)? as f64, i16le(r, o + i * 4 + 2)? as f64)))
        .collect()
}

fn xform(r: &[u8], o: usize) -> Option<Aff> {
    Some(Aff {
        a: f32le(r, o)?,
        b: f32le(r, o + 4)?,
        c: f32le(r, o + 8)?,
        d: f32le(r, o + 12)?,
        e: f32le(r, o + 16)?,
        f: f32le(r, o + 20)?,
    })
}

/// `(bmi, bits)` slices of a record, `(offset, size)` pairs counted from the record's start.
fn dib_parts(
    r: &[u8],
    off_bmi: usize,
    cb_bmi: usize,
    off_bits: usize,
    cb_bits: usize,
) -> Option<(&[u8], &[u8])> {
    let bmi = r.get(off_bmi..off_bmi.checked_add(cb_bmi)?)?;
    let bits = r.get(off_bits..off_bits.checked_add(cb_bits)?)?;
    if cb_bmi < 12 {
        return None;
    }
    Some((bmi, bits))
}

struct Player {
    g: Gdi,
    objs: HashMap<u32, Obj>,
    has_plus: bool,
    truncated: bool,
}

pub(super) fn convert(b: &[u8], cancel: &dyn Fn() -> bool) -> Option<MetaSvg> {
    let bounds = rect(b, 8)?;
    let frame = rect(b, 24)?;
    let dev = (i32le(b, 72)? as f64, i32le(b, 76)? as f64);
    let mm = (i32le(b, 80)? as f64, i32le(b, 84)? as f64);
    let ok = |v: f64| v.is_finite() && v > 0.0;
    let upm = if ok(dev.0) && ok(dev.1) && ok(mm.0) && ok(mm.1) {
        (dev.0 / mm.0, dev.1 / mm.1)
    } else {
        (96.0 / 25.4, 96.0 / 25.4)
    };
    let pxu = (upm.0 * 25.4 / 96.0).clamp(0.01, 1000.0);
    let mut p = Player {
        g: Gdi::new(GdiConfig {
            units_per_mm: upm,
            pxu,
        }),
        objs: HashMap::new(),
        has_plus: false,
        truncated: false,
    };

    let mut off = 0usize;
    let mut count = 0usize;
    while off + 8 <= b.len() {
        let (Some(ty), Some(size)) = (u32le(b, off), u32le(b, off + 4)) else {
            break;
        };
        let size = size as usize;
        if size < 8 || !size.is_multiple_of(4) || off + size > b.len() {
            p.truncated = true;
            break;
        }
        count += 1;
        if count > MAX_RECORDS || p.g.stopped() {
            p.truncated = true;
            break;
        }
        if count.is_multiple_of(super::CANCEL_EVERY) && cancel() {
            return None;
        }
        let r = &b[off..off + size];
        if ty == 14 {
            break;
        }
        // Line segments are collected across records; everything else ends them.
        if ty != 27 && ty != 54 {
            p.g.flush();
        }
        if p.record(ty, r).is_none() {
            // A record too short for its own fields.
            p.truncated = true;
        }
        off += size;
    }
    if off + 8 <= b.len() && off + u32le(b, off + 4).unwrap_or(0) as usize > b.len() {
        p.truncated = true;
    }
    if p.has_plus && p.g.drawn == 0 {
        return None;
    }

    // The picture's frame (0.01 mm), in device units; the bounds when the frame is unusable.
    let fw = frame[2] - frame[0];
    let fh = frame[3] - frame[1];
    let (view, size) = if fw > 0.0 && fh > 0.0 {
        let (sx, sy) = (upm.0 / 100.0, upm.1 / 100.0);
        (
            [frame[0] * sx, frame[1] * sy, fw * sx, fh * sy],
            (fw / 100.0 * 96.0 / 25.4, fh / 100.0 * 96.0 / 25.4),
        )
    } else if bounds[2] > bounds[0] && bounds[3] > bounds[1] {
        let (w, h) = (bounds[2] - bounds[0] + 1.0, bounds[3] - bounds[1] + 1.0);
        ([bounds[0], bounds[1], w, h], (w / pxu, h / pxu))
    } else {
        let c = p.g.bounds()?;
        let (w, h) = ((c[2] - c[0]).max(1.0), (c[3] - c[1]).max(1.0));
        ([c[0], c[1], w, h], (w / pxu, h / pxu))
    };
    let truncated = p.truncated || p.g.truncated;
    let (defs, body) = p.g.finish();
    assemble(view, size, &defs, &body, truncated)
}

impl Player {
    fn bitmap_fill(&mut self, r: &[u8], parts: Option<(&[u8], &[u8])>) -> Option<[u8; 3]> {
        let _ = r;
        let (bmi, bits) = parts?;
        let pal = self.g.dc.palette.clone();
        let bmp = dib::decode_with(bmi, bits, &mut self.g.pixels_left, Opts::default(), &pal)?;
        bmp.average()
    }

    fn create(&mut self, idx: u32, obj: Obj) {
        if idx == 0 || idx & 0x8000_0000 != 0 {
            return;
        }
        if self.objs.len() >= MAX_OBJECTS && !self.objs.contains_key(&idx) {
            self.truncated = true;
            return;
        }
        self.objs.insert(idx, obj);
    }

    /// Plays one record; `None` when it is shorter than its type needs.
    fn record(&mut self, ty: u32, r: &[u8]) -> Option<()> {
        let g = &mut self.g;
        match ty {
            // --- polygons and polylines (32-bit points)
            2..=6 => {
                let n = u32le(r, 24)? as usize;
                let pts = points32(r, 28, n)?;
                poly(g, ty, &pts);
            }
            85..=89 => {
                let n = u32le(r, 24)? as usize;
                let pts = points16(r, 28, n)?;
                // 16-bit types map to the 32-bit ones: 85->2, 86->3, 87->4, 88->5, 89->6.
                poly(g, ty - 83, &pts);
            }
            7 | 8 | 90 | 91 => {
                let np = u32le(r, 24)? as usize;
                let total = u32le(r, 28)? as usize;
                if np > MAX_POINTS || total > MAX_POINTS || 32 + np * 4 > r.len() {
                    return None;
                }
                let wide = ty < 90;
                let step = if wide { 8 } else { 4 };
                let pts_off = 32 + np * 4;
                if pts_off + total * step > r.len() {
                    return None;
                }
                let mut polys = Vec::with_capacity(np.min(1024));
                let mut idx = 0usize;
                for i in 0..np {
                    let c = u32le(r, 32 + i * 4)? as usize;
                    if c > total.saturating_sub(idx) {
                        return None;
                    }
                    let v: Vec<P> = (0..c)
                        .map(|k| {
                            let o = pts_off + (idx + k) * step;
                            if wide {
                                pt(r, o)
                            } else {
                                Some((i16le(r, o)? as f64, i16le(r, o + 2)? as f64))
                            }
                        })
                        .collect::<Option<_>>()?;
                    idx += c;
                    polys.push(v);
                }
                g.poly_poly(&polys, ty == 8 || ty == 91);
            }
            56 | 92 => {
                // POLYDRAW: points then a type byte each.
                let n = u32le(r, 24)? as usize;
                let wide = ty == 56;
                let step = if wide { 8 } else { 4 };
                if n > MAX_POINTS || 28 + n * step + n > r.len() {
                    return None;
                }
                let pts = if wide {
                    points32(r, 28, n)?
                } else {
                    points16(r, 28, n)?
                };
                let types = &r[28 + n * step..28 + n * step + n];
                poly_draw(g, &pts, types);
            }
            // --- mapping
            9 => g.set_window_ext(pt(r, 8)?),
            10 => g.set_window_org(pt(r, 8)?),
            11 => g.set_viewport_ext(pt(r, 8)?),
            12 => g.set_viewport_org(pt(r, 8)?),
            17 => g.set_map_mode(u32le(r, 8)?),
            31 | 32 => {
                let v: Vec<f64> = (0..4)
                    .map(|i| i32le(r, 8 + i * 4).map(|x| x as f64))
                    .collect::<Option<_>>()?;
                g.scale_ext(ty == 32, v[0], v[1], v[2], v[3]);
            }
            35 => g.set_world(xform(r, 8)?),
            36 => match u32le(r, 32)? {
                1 => g.set_world(Aff::ID),
                2 => g.modify_world(xform(r, 8)?, true),
                3 => g.modify_world(xform(r, 8)?, false),
                4 => g.set_world(xform(r, 8)?),
                _ => {}
            },
            // --- state
            18 => g.dc.bk_opaque = u32le(r, 8)? == 2,
            19 => g.dc.winding = u32le(r, 8)? == 2,
            20 => g.dc.rop2 = u32le(r, 8)?,
            22 => g.dc.text_align = u32le(r, 8)?,
            24 => g.dc.text_color = colorref(u32le(r, 8)?),
            25 => g.dc.bk_color = colorref(u32le(r, 8)?),
            57 => g.dc.arc_ccw = u32le(r, 8)? != 2,
            33 => g.save(),
            34 => g.restore(i32le(r, 8)? as i64),
            // --- objects
            37 => {
                let idx = u32le(r, 8)?;
                if idx & 0x8000_0000 != 0 {
                    if let Some(o) = stock_object(idx & 0x7FFF_FFFF) {
                        g.select(&o);
                    }
                } else if let Some(o) = self.objs.get(&idx) {
                    g.select(o);
                }
            }
            40 => {
                self.objs.remove(&u32le(r, 8)?);
            }
            38 => {
                let idx = u32le(r, 8)?;
                let style = u32le(r, 12)?;
                let width = i32le(r, 16)? as f64;
                let pen = make_pen(style, width, colorref(u32le(r, 24)?), false, Vec::new());
                self.create(idx, Obj::Pen(Rc::new(pen)));
            }
            95 => {
                let idx = u32le(r, 8)?;
                let style = u32le(r, 28)?;
                let width = u32le(r, 32)? as f64;
                let color = colorref(u32le(r, 40)?);
                let n = (u32le(r, 48)? as usize).min(16);
                let dashes: Vec<f64> = if style & 0xF == 7 {
                    (0..n)
                        .filter_map(|i| u32le(r, 52 + i * 4).map(|v| v as f64))
                        .collect()
                } else {
                    Vec::new()
                };
                let brush_style = u32le(r, 36)?;
                // A pen with a null brush draws nothing.
                let mut pen = make_pen(style, width, color, true, dashes);
                if brush_style == 1 {
                    pen.style = 5;
                }
                self.create(idx, Obj::Pen(Rc::new(pen)));
            }
            39 => {
                let idx = u32le(r, 8)?;
                let b = make_brush(u32le(r, 12)?, colorref(u32le(r, 16)?), u32le(r, 20)?);
                self.create(idx, Obj::Brush(Rc::new(b)));
            }
            94 | 93 => {
                let idx = u32le(r, 8)?;
                let parts = dib_parts(
                    r,
                    u32le(r, 16)? as usize,
                    u32le(r, 20)? as usize,
                    u32le(r, 24)? as usize,
                    u32le(r, 28)? as usize,
                );
                let c = self.bitmap_fill(r, parts).unwrap_or([128, 128, 128]);
                self.create(idx, Obj::Brush(Rc::new(Brush::Solid(c))));
            }
            82 => {
                let idx = u32le(r, 8)?;
                let mut face = String::new();
                let units: Vec<u16> = (0..32)
                    .map_while(|i| u16le(r, 40 + i * 2))
                    .take_while(|c| *c != 0)
                    .collect();
                face.extend(char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')));
                let f = Font {
                    height: i32le(r, 12)?,
                    escapement: i32le(r, 20)?,
                    weight: i32le(r, 28)?,
                    italic: *r.get(32)? != 0,
                    underline: *r.get(33)? != 0,
                    strike: *r.get(34)? != 0,
                    charset: *r.get(35)?,
                    pitch_family: *r.get(39)?,
                    face,
                };
                self.create(idx, Obj::Font(Rc::new(f)));
            }
            49 => {
                // CREATEPALETTE: version, count, then R G B flags per entry.
                let idx = u32le(r, 8)?;
                let n = (u16le(r, 14)? as usize).min(1024);
                let colors: Vec<[u8; 3]> = (0..n)
                    .map_while(|i| r.get(16 + i * 4..19 + i * 4).map(|c| [c[0], c[1], c[2]]))
                    .collect();
                self.create(idx, Obj::Palette(Rc::new(colors)));
            }
            48 => {
                if let Some(o) = self.objs.get(&u32le(r, 8)?) {
                    g.select(o);
                }
            }
            99 | 122 => {
                let idx = u32le(r, 8)?;
                self.create(idx, Obj::Other);
            }
            // --- positions and lines
            27 => g.move_to(pt(r, 8)?),
            54 => g.line_to(pt(r, 8)?),
            // --- shapes
            43 => g.rectangle(rect(r, 8)?),
            44 => g.round_rect(rect(r, 8)?, pt(r, 24)?),
            42 => g.ellipse(rect(r, 8)?),
            45 | 46 | 47 | 55 => {
                let kind = match ty {
                    45 => ArcKind::Arc,
                    46 => ArcKind::Chord,
                    47 => ArcKind::Pie,
                    _ => ArcKind::ArcTo,
                };
                g.arc(kind, rect(r, 8)?, pt(r, 24)?, pt(r, 32)?);
            }
            41 => g.angle_arc(
                pt(r, 8)?,
                u32le(r, 16)? as f64,
                f32le(r, 20)?,
                f32le(r, 24)?,
            ),
            15 => {
                let p = pt(r, 8)?;
                g.fill_rect_color([p.0, p.1, p.0 + 1.0, p.1 + 1.0], colorref(u32le(r, 16)?));
            }
            // --- paths
            59 => g.begin_path(),
            60 => g.end_path(),
            61 => g.close_figure(),
            62 => g.paint_path(true, false),
            63 => g.paint_path(true, true),
            64 => g.paint_path(false, true),
            68 => g.abort_path(),
            67 => g.select_clip_path(u32le(r, 8)?),
            // --- clipping
            29 => g.exclude_clip_rect(rect(r, 8)?),
            30 => g.intersect_clip_rect(rect(r, 8)?),
            75 => {
                let cb = u32le(r, 8)? as usize;
                let mode = u32le(r, 12)?;
                let data = r.get(16..16usize.checked_add(cb)?)?;
                match region_rects(data) {
                    Some(rects) if cb > 0 && !rects.is_empty() => {
                        let d = Gdi::device_rects_path(&rects);
                        g.clip_mode(&d, mode, false);
                    }
                    // No data (or an empty region with COPY): the default clip.
                    _ if mode == 5 || cb == 0 => g.reset_clip(),
                    _ => {}
                }
            }
            71 | 74 => {
                let cb = u32le(r, 24)? as usize;
                let (brush_at, data_at): (Option<usize>, usize) =
                    if ty == 71 { (Some(28), 32) } else { (None, 28) };
                let data = r.get(data_at..data_at.checked_add(cb)?)?;
                if let Some(rects) = region_rects(data) {
                    let saved = g.dc.brush.clone();
                    if let Some(at) = brush_at {
                        let idx = u32le(r, at)?;
                        if let Some(Obj::Brush(b)) = self.objs.get(&idx) {
                            g.dc.brush = b.clone();
                        }
                    }
                    // Region rectangles are device units; the path helper takes logical ones.
                    let d = Gdi::device_rects_path(&rects);
                    g.fill_device_path(&d);
                    g.dc.brush = saved;
                }
            }
            // --- text
            84 | 83 => {
                self.text(ty == 84, r)?;
            }
            // --- bitmaps
            76 | 77 => {
                // BITBLT / STRETCHBLT.
                let (dx, dy, dw, dh) = (i32le(r, 24)?, i32le(r, 28)?, i32le(r, 32)?, i32le(r, 36)?);
                let rop = u32le(r, 40)?;
                let (sx, sy) = (i32le(r, 44)?, i32le(r, 48)?);
                let cb_bmi = u32le(r, 88)? as usize;
                let dst = [dx as f64, dy as f64, dw as f64, dh as f64];
                if cb_bmi == 0 {
                    g_rop_fill(g, dst, rop);
                    return Some(());
                }
                let parts = dib_parts(
                    r,
                    u32le(r, 84)? as usize,
                    cb_bmi,
                    u32le(r, 92)? as usize,
                    u32le(r, 96)? as usize,
                )?;
                let (sw, sh) = if ty == 77 {
                    (i32le(r, 100)? as i64, i32le(r, 104)? as i64)
                } else {
                    (dw as i64, dh as i64)
                };
                let (blend, invert) = blend_of(rop);
                let opts = Opts {
                    invert,
                    pal_colors: u32le(r, 80)? == 1,
                    ..Opts::default()
                };
                self.blit(
                    parts,
                    opts,
                    Some((sx as i64, sy as i64, sw.abs(), sh.abs())),
                    dst,
                    blend,
                    1.0,
                    false,
                );
            }
            81 => {
                // STRETCHDIBITS.
                let (dx, dy) = (i32le(r, 24)?, i32le(r, 28)?);
                let (sx, sy, sw, sh) = (i32le(r, 32)?, i32le(r, 36)?, i32le(r, 40)?, i32le(r, 44)?);
                let parts = dib_parts(
                    r,
                    u32le(r, 48)? as usize,
                    u32le(r, 52)? as usize,
                    u32le(r, 56)? as usize,
                    u32le(r, 60)? as usize,
                )?;
                let rop = u32le(r, 68)?;
                let (dw, dh) = (i32le(r, 72)?, i32le(r, 76)?);
                let (blend, invert) = blend_of(rop);
                let opts = Opts {
                    invert,
                    pal_colors: u32le(r, 64)? == 1,
                    ..Opts::default()
                };
                self.blit(
                    parts,
                    opts,
                    Some((sx as i64, sy as i64, sw as i64, sh as i64)),
                    [dx as f64, dy as f64, dw as f64, dh as f64],
                    blend,
                    1.0,
                    true,
                );
            }
            80 => {
                // SETDIBITSTODEVICE.
                let (dx, dy) = (i32le(r, 24)?, i32le(r, 28)?);
                let (sx, sy, sw, sh) = (i32le(r, 32)?, i32le(r, 36)?, i32le(r, 40)?, i32le(r, 44)?);
                let parts = dib_parts(
                    r,
                    u32le(r, 48)? as usize,
                    u32le(r, 52)? as usize,
                    u32le(r, 56)? as usize,
                    u32le(r, 60)? as usize,
                )?;
                self.blit(
                    parts,
                    Opts {
                        pal_colors: u32le(r, 64)? == 1,
                        ..Opts::default()
                    },
                    Some((sx as i64, sy as i64, sw as i64, sh as i64)),
                    [dx as f64, dy as f64, sw as f64, sh as f64],
                    None,
                    1.0,
                    true,
                );
            }
            114 | 116 => {
                // ALPHABLEND / TRANSPARENTBLT.
                let (dx, dy, dw, dh) = (i32le(r, 24)?, i32le(r, 28)?, i32le(r, 32)?, i32le(r, 36)?);
                let (sx, sy) = (i32le(r, 44)?, i32le(r, 48)?);
                let parts = dib_parts(
                    r,
                    u32le(r, 84)? as usize,
                    u32le(r, 88)? as usize,
                    u32le(r, 92)? as usize,
                    u32le(r, 96)? as usize,
                )?;
                let (sw, sh) = (i32le(r, 100)? as i64, i32le(r, 104)? as i64);
                let mut opts = Opts {
                    pal_colors: u32le(r, 80)? == 1,
                    ..Opts::default()
                };
                let mut opacity = 1.0;
                if ty == 114 {
                    opacity = *r.get(42)? as f64 / 255.0;
                    opts.alpha = *r.get(43)? == 1;
                } else {
                    opts.key = Some(colorref(u32le(r, 40)?));
                }
                self.blit(
                    parts,
                    opts,
                    Some((sx as i64, sy as i64, sw, sh)),
                    [dx as f64, dy as f64, dw as f64, dh as f64],
                    None,
                    opacity,
                    false,
                );
            }
            118 => self.gradient(r)?,
            // --- comments: EMF+ is noticed, not drawn
            70 => {
                let n = u32le(r, 8)? as usize;
                if n >= 4 && r.get(12..16) == Some(b"EMF+") {
                    self.has_plus = true;
                }
            }
            _ => {}
        }
        Some(())
    }

    #[allow(clippy::too_many_arguments)]
    fn blit(
        &mut self,
        parts: (&[u8], &[u8]),
        opts: Opts,
        src: Option<(i64, i64, i64, i64)>,
        dst: [f64; 4],
        blend: Option<&'static str>,
        opacity: f64,
        dib_origin: bool,
    ) {
        let pal = self.g.dc.palette.clone();
        let Some(bmp) = dib::decode_with(parts.0, parts.1, &mut self.g.pixels_left, opts, &pal)
        else {
            return;
        };
        // `ySrc` of the DIB functions counts from the bottom row of a bottom-up bitmap.
        let src = src.map(|(x, y, w, h)| {
            if dib_origin && !bmp.top_down {
                (x, bmp.h as i64 - y - h, w, h)
            } else {
                (x, y, w, h)
            }
        });
        self.g.bitmap(&bmp, src, dst, blend, opacity);
    }

    fn text(&mut self, wide: bool, r: &[u8]) -> Option<()> {
        let reference = pt(r, 36)?;
        let n = u32le(r, 44)? as usize;
        let off = u32le(r, 48)? as usize;
        let options = u32le(r, 52)?;
        let off_dx = u32le(r, 72)? as usize;
        let opaque = (options & 0x2 != 0).then(|| rect(r, 56)).flatten();
        if n == 0 || options & 0x10 != 0 {
            // No glyphs to draw (or none that can be): the opaque rectangle still is.
            if opaque.is_some() {
                self.g.text(TextRun {
                    chars: Vec::new(),
                    dx: None,
                    reference,
                    opaque_rect: opaque,
                });
            }
            return Some(());
        }
        let n = n.min(MAX_TEXT_CHARS * 2);
        let unit = if wide { 2 } else { 1 };
        let raw = r.get(off..off.checked_add(n.checked_mul(unit)?)?)?;
        let stride = if options & 0x2000 != 0 { 2 } else { 1 };
        let dx_at = |i: usize| -> Option<f64> {
            if off_dx == 0 {
                return None;
            }
            i32le(r, off_dx.checked_add(i.checked_mul(4 * stride)?)?).map(|v| v as f64)
        };
        let mut chars = Vec::new();
        let mut dx: Vec<f64> = Vec::new();
        let mut have_dx = off_dx != 0 && dx_at(n - 1).is_some();
        if wide {
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            let mut i = 0;
            while i < units.len() && chars.len() < MAX_TEXT_CHARS {
                let u = units[i];
                let (c, used) = if (0xD800..0xDC00).contains(&u)
                    && i + 1 < units.len()
                    && (0xDC00..0xE000).contains(&units[i + 1])
                {
                    let cp =
                        0x10000 + (((u as u32) - 0xD800) << 10) + (units[i + 1] as u32 - 0xDC00);
                    (char::from_u32(cp).unwrap_or('\u{fffd}'), 2)
                } else {
                    (char::from_u32(u as u32).unwrap_or('\u{fffd}'), 1)
                };
                chars.push(c);
                if have_dx {
                    let w: f64 = (0..used).filter_map(|k| dx_at(i + k)).sum();
                    dx.push(w);
                }
                i += used;
            }
        } else {
            chars = decode_text(raw, self.g.dc.font.charset);
            if have_dx && chars.len() == n.min(MAX_TEXT_CHARS * 2) {
                dx = (0..chars.len()).filter_map(dx_at).collect();
            } else {
                have_dx = false;
            }
        }
        self.g.text(TextRun {
            dx: (have_dx && dx.len() == chars.len()).then_some(dx),
            chars,
            reference,
            opaque_rect: opaque,
        });
        Some(())
    }

    fn gradient(&mut self, r: &[u8]) -> Option<()> {
        let nver = u32le(r, 24)? as usize;
        let nelem = u32le(r, 28)? as usize;
        let mode = u32le(r, 32)?;
        if nver > MAX_POINTS || nelem > MAX_POINTS || 36 + nver * 16 > r.len() {
            return None;
        }
        let vert = |i: usize| -> Option<(P, [u8; 3])> {
            let o = 36 + i * 16;
            if i >= nver {
                return None;
            }
            Some((
                pt(r, o)?,
                [
                    (u16le(r, o + 8)? >> 8) as u8,
                    (u16le(r, o + 10)? >> 8) as u8,
                    (u16le(r, o + 12)? >> 8) as u8,
                ],
            ))
        };
        let base = 36 + nver * 16;
        let size = if mode == 2 { 12 } else { 8 };
        if base + nelem * size > r.len() {
            return None;
        }
        for k in 0..nelem {
            let o = base + k * size;
            if mode == 2 {
                let (a, b, c) = (
                    vert(u32le(r, o)? as usize)?,
                    vert(u32le(r, o + 4)? as usize)?,
                    vert(u32le(r, o + 8)? as usize)?,
                );
                let avg = |i: usize| ((a.1[i] as u32 + b.1[i] as u32 + c.1[i] as u32) / 3) as u8;
                self.g
                    .fill_polygon_color(&[a.0, b.0, c.0], [avg(0), avg(1), avg(2)]);
            } else {
                let (ul, lr) = (
                    vert(u32le(r, o)? as usize)?,
                    vert(u32le(r, o + 4)? as usize)?,
                );
                self.g
                    .gradient_rect([ul.0 .0, ul.0 .1, lr.0 .0, lr.0 .1], ul.1, lr.1, mode == 0);
            }
        }
        Some(())
    }
}

/// The 32-bit polygon record types: 2 bezier, 3 polygon, 4 polyline, 5 bezier-to, 6 polyline-to.
fn poly(g: &mut Gdi, ty: u32, pts: &[P]) {
    match ty {
        2 => g.poly_bezier(pts, false),
        3 => g.polygon(pts),
        4 => g.polyline(pts, false),
        5 => g.poly_bezier(pts, true),
        _ => g.polyline(pts, true),
    }
}

/// `POLYDRAW`: point types `PT_MOVETO` 6, `PT_LINETO` 2, `PT_BEZIERTO` 4, all with 1 = close.
fn poly_draw(g: &mut Gdi, pts: &[P], types: &[u8]) {
    let mut i = 0;
    while i < pts.len() {
        match types[i] & !1 {
            6 => {
                g.move_to(pts[i]);
                i += 1;
            }
            2 => {
                g.line_to(pts[i]);
                if types[i] & 1 != 0 {
                    g.close_figure();
                }
                i += 1;
            }
            4 if i + 2 < pts.len() => {
                g.poly_bezier(&pts[i..i + 3], true);
                if types[i + 2] & 1 != 0 {
                    g.close_figure();
                }
                i += 3;
            }
            _ => i += 1,
        }
    }
}

/// A bitmap record without a source bitmap: the raster operation alone decides the colour.
fn g_rop_fill(g: &mut Gdi, dst: [f64; 4], rop: u32) {
    let r = [dst[0], dst[1], dst[0] + dst[2], dst[1] + dst[3]];
    match rop {
        0x0000_0042 => g.fill_rect_color(r, [0, 0, 0]),
        0x00FF_0062 => g.fill_rect_color(r, [255, 255, 255]),
        // PATCOPY and the pattern ROPs that paint it.
        0x00F0_0021 | 0x005A_0049 | 0x00FB_0A09 => g.fill_rects(&[r]),
        _ => {}
    }
}
