//! Windows Metafile (MS-WMF) records played into the GDI device context.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::rc::Rc;

use super::dib::{self, Opts};
use super::gdi::{
    make_brush, make_pen, Brush, Font, Gdi, GdiConfig, Obj, TextRun, MAX_TEXT_CHARS, P,
};
use super::{
    assemble, blend_of, colorref, decode_text, i16le, u16le, u32le, MetaSvg, MAX_OBJECTS,
    MAX_POINTS, MAX_RECORDS,
};

const PLACEABLE_KEY: u32 = 0x9AC6_CDD7;

/// A placeable header, or a standard header (`METAHEADER`: type 1 or 2, size 9, version).
pub(super) fn is_wmf(b: &[u8]) -> bool {
    if u32le(b, 0) == Some(PLACEABLE_KEY) {
        return b.len() >= 22 + 18;
    }
    matches!(u16le(b, 0), Some(1 | 2))
        && u16le(b, 2) == Some(9)
        && matches!(u16le(b, 4), Some(0x0100 | 0x0300))
}

/// The WMF object table: a created object takes the lowest free slot.
struct Table {
    slots: Vec<Option<Obj>>,
    free: BinaryHeap<Reverse<usize>>,
}

impl Table {
    /// `false` when the table is full (the object is dropped).
    fn add(&mut self, o: Obj) -> bool {
        if let Some(Reverse(i)) = self.free.pop() {
            self.slots[i] = Some(o);
            true
        } else if self.slots.len() < MAX_OBJECTS {
            self.slots.push(Some(o));
            true
        } else {
            false
        }
    }

    fn remove(&mut self, i: usize) {
        if let Some(s) = self.slots.get_mut(i) {
            if s.take().is_some() {
                self.free.push(Reverse(i));
            }
        }
    }
}

pub(super) fn convert(b: &[u8]) -> Option<MetaSvg> {
    let (mut off, bbox, inch) = if u32le(b, 0) == Some(PLACEABLE_KEY) {
        let l = i16le(b, 6)? as f64;
        let t = i16le(b, 8)? as f64;
        let r = i16le(b, 10)? as f64;
        let bt = i16le(b, 12)? as f64;
        let inch = u16le(b, 14)? as f64;
        (
            22usize,
            Some([l.min(r), t.min(bt), (r - l).abs(), (bt - t).abs()]),
            inch,
        )
    } else {
        (0usize, None, 0.0)
    };
    // Skip the METAHEADER.
    let header_words = u16le(b, off + 2)? as usize;
    off += header_words.max(9) * 2;

    let units_per_inch = if inch >= 1.0 { inch } else { 96.0 };
    let mut g = Gdi::new(GdiConfig {
        units_per_mm: (units_per_inch / 25.4, units_per_inch / 25.4),
        pxu: (units_per_inch / 96.0).clamp(0.001, 100_000.0),
    });
    // The device is the picture's frame: window -> frame unless the file maps otherwise.
    g.dc.map_mode = 8;
    g.dc.text_align = 0;
    g.dc.bk_opaque = true;
    match bbox {
        Some(bb) if bb[2] > 0.0 && bb[3] > 0.0 => {
            g.dc.win_org = (bb[0], bb[1]);
            g.dc.win_ext = (bb[2], bb[3]);
            g.dc.vp_org = (bb[0], bb[1]);
            g.dc.vp_ext = (bb[2], bb[3]);
            g.dc.vp_set = true;
        }
        _ => g.dc.vp_set = false,
    }
    g.recalc();

    let mut tbl = Table {
        slots: Vec::new(),
        free: BinaryHeap::new(),
    };
    let mut truncated = false;
    let mut count = 0usize;
    while off + 6 <= b.len() {
        let size = u32le(b, off)? as usize * 2;
        let func = u16le(b, off + 4)?;
        if size < 6 || off + size > b.len() {
            truncated = true;
            break;
        }
        count += 1;
        if count > MAX_RECORDS || g.stopped() {
            truncated = true;
            break;
        }
        let r = &b[off..off + size];
        if func == 0 {
            break;
        }
        if func != 0x0214 && func != 0x0213 {
            g.flush();
        }
        match record(&mut g, &mut tbl, func, r) {
            Some(true) => {}
            Some(false) => truncated = true,
            None => truncated = true,
        }
        off += size;
    }
    truncated |= g.truncated;

    let (view, size) = match bbox {
        Some(bb) if bb[2] > 0.0 && bb[3] > 0.0 => (
            bb,
            (bb[2] / units_per_inch * 96.0, bb[3] / units_per_inch * 96.0),
        ),
        _ => match g.view0.filter(|_| g.win_ext_set) {
            Some((o, e)) if e.0 != 0.0 && e.1 != 0.0 => (
                [o.0.min(o.0 + e.0), o.1.min(o.1 + e.1), e.0.abs(), e.1.abs()],
                (e.0.abs(), e.1.abs()),
            ),
            _ => {
                let c = g.bounds()?;
                let (w, h) = ((c[2] - c[0]).max(1.0), (c[3] - c[1]).max(1.0));
                ([c[0], c[1], w, h], (w, h))
            }
        },
    };
    let (defs, body) = g.finish();
    assemble(view, size, &defs, &body, truncated)
}

fn pt16(r: &[u8], o: usize) -> Option<P> {
    // Parameters hold y first, then x.
    Some((i16le(r, o + 2)? as f64, i16le(r, o)? as f64))
}

fn rect4(r: &[u8], o: usize) -> Option<[f64; 4]> {
    // bottom, right, top, left.
    Some([
        i16le(r, o + 6)? as f64,
        i16le(r, o + 4)? as f64,
        i16le(r, o + 2)? as f64,
        i16le(r, o)? as f64,
    ])
}

/// `Some(true)` = handled, `Some(false)` = recognised but dropped (a budget), `None` = too short.
fn record(g: &mut Gdi, tbl: &mut Table, func: u16, r: &[u8]) -> Option<bool> {
    match func {
        0x001E => g.save(),
        0x0127 => g.restore(i16le(r, 6)? as i64),
        0x0102 => g.dc.bk_opaque = u16le(r, 6)? == 2,
        0x0103 => g.set_map_mode(u16le(r, 6)? as u32),
        0x0104 => g.dc.rop2 = u16le(r, 6)? as u32,
        0x0106 => g.dc.winding = u16le(r, 6)? == 2,
        0x012E => g.dc.text_align = u16le(r, 6)? as u32,
        0x0209 => g.dc.text_color = colorref(u32le(r, 6)?),
        0x0201 => g.dc.bk_color = colorref(u32le(r, 6)?),
        0x020B => g.set_window_org(pt16(r, 6)?),
        0x020C => g.set_window_ext(pt16(r, 6)?),
        0x020D => g.set_viewport_org(pt16(r, 6)?),
        0x020E => g.set_viewport_ext(pt16(r, 6)?),
        0x020F | 0x0211 => {
            let p = pt16(r, 6)?;
            g.offset_org(func == 0x020F, p.0, p.1);
        }
        0x0410 | 0x0412 => {
            let v: Vec<f64> = (0..4)
                .map(|i| i16le(r, 6 + i * 2).map(|x| x as f64))
                .collect::<Option<_>>()?;
            // yDenom, yNum, xDenom, xNum.
            g.scale_ext(func == 0x0410, v[3], v[2], v[1], v[0]);
        }
        0x0214 => g.move_to(pt16(r, 6)?),
        0x0213 => g.line_to(pt16(r, 6)?),
        // SELECTCLIPREGION: handle 0 is the null region (no clip); a real region is not kept.
        0x012C => {
            if u16le(r, 6)? == 0 {
                g.reset_clip();
            }
        }
        0x0415 => g.exclude_clip_rect(rect4(r, 6)?),
        0x0416 => g.intersect_clip_rect(rect4(r, 6)?),
        0x0324 | 0x0325 => {
            let n = u16le(r, 6)? as usize;
            let pts = points(r, 8, n)?;
            if func == 0x0324 {
                g.polygon(&pts);
            } else {
                g.polyline(&pts, false);
            }
        }
        0x0538 => {
            let np = u16le(r, 6)? as usize;
            if np > MAX_POINTS || 8 + np * 2 > r.len() {
                return None;
            }
            let mut at = 8 + np * 2;
            let mut polys = Vec::new();
            for i in 0..np {
                let c = u16le(r, 8 + i * 2)? as usize;
                polys.push(points(r, at, c)?);
                at += c * 4;
            }
            g.poly_poly(&polys, true);
        }
        0x041B => g.rectangle(rect4(r, 6)?),
        0x061C => {
            let corner = (i16le(r, 8)? as f64, i16le(r, 6)? as f64);
            g.round_rect(rect4(r, 10)?, corner);
        }
        0x0418 => g.ellipse(rect4(r, 6)?),
        0x0817 | 0x081A | 0x0830 => {
            use super::gdi::ArcKind;
            let kind = match func {
                0x0817 => ArcKind::Arc,
                0x081A => ArcKind::Pie,
                _ => ArcKind::Chord,
            };
            g.arc(kind, rect4(r, 14)?, pt16(r, 10)?, pt16(r, 6)?);
        }
        0x0521 => {
            // TEXTOUT: length, string (even-padded), y, x.
            let n = u16le(r, 6)? as usize;
            let s = r.get(8..8usize.checked_add(n)?)?;
            let at = 8 + n + (n & 1);
            let p = pt16(r, at)?;
            text(g, s, p, None, None);
        }
        0x0A32 => {
            let p = pt16(r, 6)?;
            let n = u16le(r, 10)? as usize;
            let opts = u16le(r, 12)?;
            let mut at: usize = 14;
            let clip_rect = if opts & 6 != 0 {
                let rc = (
                    i16le(r, 14)? as f64,
                    i16le(r, 16)? as f64,
                    i16le(r, 18)? as f64,
                    i16le(r, 20)? as f64,
                );
                at += 8;
                Some([rc.0, rc.1, rc.2, rc.3])
            } else {
                None
            };
            let s = r.get(at..at.checked_add(n)?)?;
            let dx_at = at + n + (n & 1);
            let dx: Option<Vec<f64>> = (0..n)
                .map(|i| i16le(r, dx_at + i * 2).map(|v| v as f64))
                .collect();
            text(g, s, p, dx, if opts & 2 != 0 { clip_rect } else { None });
        }
        0x02FA => {
            let style = u16le(r, 6)? as u32;
            let width = i16le(r, 8)? as f64;
            let pen = make_pen(style, width, colorref(u32le(r, 12)?), false, Vec::new());
            return Some(tbl.add(Obj::Pen(Rc::new(pen))));
        }
        0x02FC => {
            let b = make_brush(
                u16le(r, 6)? as u32,
                colorref(u32le(r, 8)?),
                u16le(r, 12)? as u32,
            );
            return Some(tbl.add(Obj::Brush(Rc::new(b))));
        }
        0x02FB => {
            let face_bytes: Vec<u8> = r
                .get(24..)?
                .iter()
                .take(32)
                .copied()
                .take_while(|c| *c != 0)
                .collect();
            let charset = *r.get(19)?;
            let face: String = decode_text(&face_bytes, charset).into_iter().collect();
            let f = Font {
                height: i16le(r, 6)? as i32,
                escapement: i16le(r, 10)? as i32,
                weight: i16le(r, 14)? as i32,
                italic: *r.get(16)? != 0,
                underline: *r.get(17)? != 0,
                strike: *r.get(18)? != 0,
                charset,
                pitch_family: *r.get(23)?,
                face,
            };
            return Some(tbl.add(Obj::Font(Rc::new(f))));
        }
        0x0142 => {
            // DIBCREATEPATTERNBRUSH: style, usage, packed DIB.
            let dib_bytes = r.get(10..)?;
            let mut budget = g.pixels_left;
            let avg = dib::split_packed(dib_bytes)
                .and_then(|(bmi, bits)| {
                    dib::decode_with(bmi, bits, &mut budget, Opts::default(), &g.dc.palette)
                })
                .and_then(|bm| {
                    g.pixels_left = budget;
                    bm.average()
                })
                .unwrap_or([128, 128, 128]);
            return Some(tbl.add(Obj::Brush(Rc::new(Brush::Solid(avg)))));
        }
        // Objects that take a slot but are not drawn with: pattern brush, palette, region, bitmap.
        0x01F9 | 0x06FF | 0x02FD | 0x06FE => return Some(tbl.add(Obj::Other)),
        0x00F7 => {
            // CREATEPALETTE: version, count, then R G B flags per entry.
            let n = (u16le(r, 8)? as usize).min(1024);
            let colors: Vec<[u8; 3]> = (0..n)
                .map_while(|i| r.get(10 + i * 4..13 + i * 4).map(|c| [c[0], c[1], c[2]]))
                .collect();
            return Some(tbl.add(Obj::Palette(Rc::new(colors))));
        }
        0x012D | 0x0234 => {
            let i = u16le(r, 6)? as usize;
            if let Some(Some(o)) = tbl.slots.get(i) {
                g.select(o);
            }
        }
        0x01F0 => tbl.remove(u16le(r, 6)? as usize),
        0x061D => {
            // PATBLT: rop, height, width, y, x.
            let rop = u32le(r, 6)?;
            let (h, w, y, x) = (
                i16le(r, 10)? as f64,
                i16le(r, 12)? as f64,
                i16le(r, 14)? as f64,
                i16le(r, 16)? as f64,
            );
            rop_fill(g, [x, y, x + w, y + h], rop);
        }
        0x0940 => {
            // DIBBITBLT: rop, ySrc, xSrc, height, width, yDest, xDest, [DIB].
            let rop = u32le(r, 6)?;
            let (sy, sx, h, w, dy, dx) = (
                i16le(r, 10)? as i64,
                i16le(r, 12)? as i64,
                i16le(r, 14)? as i64,
                i16le(r, 16)? as i64,
                i16le(r, 18)? as f64,
                i16le(r, 20)? as f64,
            );
            if r.len() <= 26 {
                rop_fill(g, [dx, dy, dx + w as f64, dy + h as f64], rop);
            } else {
                blit(
                    g,
                    r.get(22..)?,
                    rop,
                    Some((sx, sy, w, h)),
                    [dx, dy, w as f64, h as f64],
                );
            }
        }
        0x0B41 => {
            // DIBSTRETCHBLT: rop, srcH, srcW, ySrc, xSrc, destH, destW, yDest, xDest, DIB.
            let rop = u32le(r, 6)?;
            let v: Vec<i64> = (0..8)
                .map(|i| i16le(r, 10 + i * 2).map(|x| x as i64))
                .collect::<Option<_>>()?;
            blit(
                g,
                r.get(26..)?,
                rop,
                Some((v[3], v[2], v[1], v[0])),
                [v[7] as f64, v[6] as f64, v[5] as f64, v[4] as f64],
            );
        }
        0x0F43 => {
            // STRETCHDIB: rop, usage, srcH, srcW, ySrc, xSrc, destH, destW, yDst, xDst, DIB.
            let rop = u32le(r, 6)?;
            let v: Vec<i64> = (0..8)
                .map(|i| i16le(r, 12 + i * 2).map(|x| x as i64))
                .collect::<Option<_>>()?;
            blit_dib_origin(
                g,
                r.get(28..)?,
                rop,
                (v[3], v[2], v[1], v[0]),
                [v[7] as f64, v[6] as f64, v[5] as f64, v[4] as f64],
            );
        }
        _ => {}
    }
    Some(true)
}

fn points(r: &[u8], at: usize, n: usize) -> Option<Vec<P>> {
    if n > MAX_POINTS || at.checked_add(n.checked_mul(4)?)? > r.len() {
        return None;
    }
    (0..n)
        .map(|i| {
            Some((
                i16le(r, at + i * 4)? as f64,
                i16le(r, at + i * 4 + 2)? as f64,
            ))
        })
        .collect()
}

fn text(g: &mut Gdi, bytes: &[u8], p: P, dx: Option<Vec<f64>>, opaque: Option<[f64; 4]>) {
    let chars = decode_text(bytes, g.dc.font.charset);
    if chars.is_empty() && opaque.is_none() {
        return;
    }
    // Per-byte advances only line up with single-byte text.
    let dx = dx
        .filter(|d| chars.len() == bytes.len().min(MAX_TEXT_CHARS) && d.len() >= chars.len())
        .map(|d| d[..chars.len()].to_vec());
    g.text(TextRun {
        chars,
        dx,
        reference: p,
        opaque_rect: opaque,
    });
}

fn rop_fill(g: &mut Gdi, r: [f64; 4], rop: u32) {
    match rop {
        0x0000_0042 => g.fill_rect_color(r, [0, 0, 0]),
        0x00FF_0062 => g.fill_rect_color(r, [255, 255, 255]),
        0x00F0_0021 | 0x005A_0049 | 0x00FB_0A09 => g.fill_rects(&[r]),
        _ => {}
    }
}

fn blit(g: &mut Gdi, packed: &[u8], rop: u32, src: Option<(i64, i64, i64, i64)>, dst: [f64; 4]) {
    draw(g, packed, rop, src, dst, false);
}

fn blit_dib_origin(g: &mut Gdi, packed: &[u8], rop: u32, src: (i64, i64, i64, i64), dst: [f64; 4]) {
    draw(g, packed, rop, Some(src), dst, true);
}

fn draw(
    g: &mut Gdi,
    packed: &[u8],
    rop: u32,
    src: Option<(i64, i64, i64, i64)>,
    dst: [f64; 4],
    dib_origin: bool,
) {
    let Some((bmi, bits)) = dib::split_packed(packed) else {
        return;
    };
    let (blend, invert) = blend_of(rop);
    let mut budget = g.pixels_left;
    let pal = g.dc.palette.clone();
    let Some(bmp) = dib::decode_with(
        bmi,
        bits,
        &mut budget,
        Opts {
            invert,
            ..Opts::default()
        },
        &pal,
    ) else {
        return;
    };
    g.pixels_left = budget;
    let src = src.map(|(x, y, w, h)| {
        if dib_origin && !bmp.top_down {
            (x, bmp.h as i64 - y - h, w.abs(), h.abs())
        } else {
            (x, y, w.abs(), h.abs())
        }
    });
    g.bitmap(&bmp, src, dst, blend, 1.0);
}
