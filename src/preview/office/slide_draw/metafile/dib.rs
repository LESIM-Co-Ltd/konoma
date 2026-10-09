//! Device-independent bitmaps (the `BITMAPINFO` + bits pair both EMF and WMF carry) decoded to
//! RGBA, or passed through when the bits are an embedded PNG / JPEG.
//!
//! Everything is checked against the bytes really present before anything is allocated, and
//! against a pixel budget the caller owns, so a header that promises a gigapixel bitmap costs
//! nothing.

use std::io::Cursor;

use crate::preview::image::{MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE};

/// Largest single bitmap, in pixels (RGBA = 64 MiB). Far below `MAX_IMAGE_PIXELS`: a picture
/// inside a metafile is a small decoration, and a slide may hold many of them.
pub(super) const MAX_BITMAP_PIXELS: u64 = 16 * 1024 * 1024;

/// A decoded bitmap.
pub(super) struct Bitmap {
    pub w: u32,
    pub h: u32,
    /// The DIB stored its rows top to bottom (a negative height).
    pub top_down: bool,
    pub data: BitmapData,
}

pub(super) enum BitmapData {
    /// Straight (non-premultiplied) RGBA, row 0 at the top.
    Rgba(Vec<u8>),
    /// A PNG or JPEG exactly as stored (`mime`, bytes).
    Encoded(&'static str, Vec<u8>),
}

/// How the pixels are post-processed while decoding.
#[derive(Clone, Copy, Default)]
pub(super) struct Opts {
    /// 32-bit `BI_RGB`: the fourth byte is a premultiplied alpha (`AC_SRC_ALPHA`).
    pub alpha: bool,
    /// Pixels of this colour become transparent (`TransparentBlt`).
    pub key: Option<[u8; 3]>,
    /// Invert the colours (`NOTSRCCOPY`).
    pub invert: bool,
    /// The colour table holds 16-bit indices into the logical palette (`DIB_PAL_COLORS`).
    pub pal_colors: bool,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    u32_at(b, o).map(|v| v as i32)
}

const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;
const BI_JPEG: u32 = 4;
const BI_PNG: u32 = 5;
const BI_ALPHABITFIELDS: u32 = 6;

/// The parsed `BITMAPINFOHEADER` / `BITMAPCOREHEADER`.
struct Header {
    w: u32,
    h: u32,
    top_down: bool,
    bpp: u16,
    compression: u32,
    clr_used: u32,
    /// Size of the header proper (where the colour table / masks start).
    size: usize,
    core: bool,
}

fn header(bmi: &[u8]) -> Option<Header> {
    let size = u32_at(bmi, 0)? as usize;
    if size == 12 {
        let w = u16_at(bmi, 4)? as u32;
        let h = u16_at(bmi, 6)? as u32;
        let bpp = u16_at(bmi, 10)?;
        return Some(Header {
            w,
            h,
            top_down: false,
            bpp,
            compression: BI_RGB,
            clr_used: 0,
            size,
            core: true,
        });
    }
    if size < 40 || size > bmi.len() {
        return None;
    }
    let w = i32_at(bmi, 4)?;
    let h = i32_at(bmi, 8)?;
    if w <= 0 || h == 0 {
        return None;
    }
    Some(Header {
        w: w as u32,
        h: h.unsigned_abs(),
        top_down: h < 0,
        bpp: u16_at(bmi, 14)?,
        compression: u32_at(bmi, 16)?,
        clr_used: u32_at(bmi, 32)?,
        size,
        core: false,
    })
}

/// Number of colour-table entries and the byte offset where the pixel data starts when the bits
/// follow the header directly (a packed DIB).
fn layout(hd: &Header) -> (usize, usize) {
    let mut off = hd.size;
    // `BI_BITFIELDS` after a 40-byte header stores the masks right behind it (V4/V5 headers hold
    // them inside).
    if hd.size == 40 && hd.compression == BI_BITFIELDS {
        off += 12;
    } else if hd.size == 40 && hd.compression == BI_ALPHABITFIELDS {
        off += 16;
    }
    let entries = if hd.bpp <= 8 && hd.bpp > 0 {
        let max = 1usize << hd.bpp;
        if hd.clr_used != 0 {
            (hd.clr_used as usize).min(max)
        } else {
            max
        }
    } else if hd.clr_used != 0 && hd.bpp > 8 {
        hd.clr_used.min(1 << 16) as usize
    } else {
        0
    };
    let entry = if hd.core { 3 } else { 4 };
    (entries, off + entries * entry)
}

/// Splits a packed DIB (WMF) into its header and bits.
///
/// Some writers leave the colour table out of an 8-bit (or smaller) DIB and rely on the selected
/// logical palette; when what follows the header is exactly the pixel data, that is what it is.
pub(super) fn split_packed(dib: &[u8]) -> Option<(&[u8], &[u8])> {
    let hd = header(dib)?;
    let (entries, bits_off) = layout(&hd);
    let table_less = entries > 0
        && hd.compression == BI_RGB
        && bits_off - entries * if hd.core { 3 } else { 4 } <= dib.len()
        && uncompressed_len(&hd).is_some_and(|need| {
            let head = bits_off - entries * if hd.core { 3 } else { 4 };
            dib.len() - head >= need && dib.len() < bits_off + need
        });
    if table_less {
        let head = bits_off - entries * if hd.core { 3 } else { 4 };
        return Some((&dib[..head], &dib[head..]));
    }
    if bits_off > dib.len() {
        return None;
    }
    Some((&dib[..bits_off], &dib[bits_off..]))
}

/// Bytes of pixel data an uncompressed bitmap needs.
fn uncompressed_len(hd: &Header) -> Option<usize> {
    let stride = (hd.w as u64 * hd.bpp as u64).div_ceil(32) * 4;
    usize::try_from(stride.checked_mul(hd.h as u64)?).ok()
}

/// Decodes `bmi` (header, masks and colour table) with the pixel data `bits`.
///
/// `budget` is the caller's remaining pixel allowance; a bitmap over it, over the per-bitmap
/// limit or over konoma's image limits is refused (`None`) before anything is allocated.
pub(super) fn decode(bmi: &[u8], bits: &[u8], budget: &mut u64, opts: Opts) -> Option<Bitmap> {
    decode_with(bmi, bits, budget, opts, &[])
}

/// [`decode`], with the logical palette to use when the DIB carries no colour table.
pub(super) fn decode_with(
    bmi: &[u8],
    bits: &[u8],
    budget: &mut u64,
    opts: Opts,
    fallback: &[[u8; 3]],
) -> Option<Bitmap> {
    let hd = header(bmi)?;
    if hd.w == 0 || hd.h == 0 || hd.w > MAX_IMAGE_SIDE || hd.h > MAX_IMAGE_SIDE {
        return None;
    }
    let pixels = hd.w as u64 * hd.h as u64;
    if pixels > MAX_BITMAP_PIXELS || pixels > MAX_IMAGE_PIXELS || pixels > *budget {
        return None;
    }
    if hd.compression == BI_PNG || hd.compression == BI_JPEG {
        return encoded(&hd, bits, budget);
    }
    let (w, h) = (hd.w as usize, hd.h as usize);
    let (entries, _) = layout(&hd);
    let table_off = hd.size
        + if hd.size == 40 && hd.compression == BI_BITFIELDS {
            12
        } else if hd.size == 40 && hd.compression == BI_ALPHABITFIELDS {
            16
        } else {
            0
        };
    let entry = if hd.core { 3 } else { 4 };
    let palette: Vec<[u8; 3]> = if opts.pal_colors && entries > 0 {
        (0..entries)
            .map(|i| {
                let idx = u16_at(bmi, table_off + i * 2).unwrap_or(0) as usize;
                fallback.get(idx).copied().unwrap_or([0, 0, 0])
            })
            .collect()
    } else if entries > 0 && table_off + entries * entry > bmi.len() {
        // No colour table in the DIB: the selected logical palette colours it.
        fallback.to_vec()
    } else {
        (0..entries)
            .map(|i| {
                let o = table_off + i * entry;
                match bmi.get(o..o + 3) {
                    Some(c) => [c[2], c[1], c[0]],
                    None => [0, 0, 0],
                }
            })
            .collect()
    };
    let masks = bit_masks(&hd, bmi);

    let mut rgba = vec![0u8; (pixels * 4) as usize];
    match (hd.compression, hd.bpp) {
        (BI_RLE8, 8) => rle(bits, w, h, &palette, 8, hd.top_down, &mut rgba)?,
        (BI_RLE4, 4) => rle(bits, w, h, &palette, 4, hd.top_down, &mut rgba)?,
        (BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS, 1 | 4 | 8 | 16 | 24 | 32) => {
            let stride = (w as u64 * hd.bpp as u64).div_ceil(32) * 4;
            if stride.checked_mul(h as u64)? > bits.len() as u64 {
                return None;
            }
            let stride = stride as usize;
            for y in 0..h {
                let sy = if hd.top_down { y } else { h - 1 - y };
                let row = &bits[sy * stride..sy * stride + stride];
                let out = &mut rgba[y * w * 4..(y + 1) * w * 4];
                unpack_row(row, hd.bpp, &palette, &masks, hd.compression, opts, out);
            }
        }
        _ => return None,
    }
    post(&mut rgba, opts);
    *budget -= pixels;
    Some(Bitmap {
        w: hd.w,
        h: hd.h,
        top_down: hd.top_down,
        data: BitmapData::Rgba(rgba),
    })
}

fn encoded(hd: &Header, bits: &[u8], budget: &mut u64) -> Option<Bitmap> {
    // `biSizeImage` is not trusted: the format is sniffed and its own header must agree with the
    // declared size and stay inside the limits.
    let (mime, fmt) = if hd.compression == BI_PNG {
        ("image/png", image::ImageFormat::Png)
    } else {
        ("image/jpeg", image::ImageFormat::Jpeg)
    };
    let mut r = image::ImageReader::with_format(Cursor::new(bits), fmt);
    let mut lim = image::Limits::default();
    lim.max_image_width = Some(MAX_IMAGE_SIDE);
    lim.max_image_height = Some(MAX_IMAGE_SIDE);
    lim.max_alloc = Some(256 * 1024 * 1024);
    r.limits(lim);
    let (w, h) = r.into_dimensions().ok()?;
    if w as u64 * h as u64 > MAX_BITMAP_PIXELS || w as u64 * h as u64 > *budget {
        return None;
    }
    *budget -= w as u64 * h as u64;
    Some(Bitmap {
        w,
        h,
        top_down: true,
        data: BitmapData::Encoded(mime, bits.to_vec()),
    })
}

/// `(mask, shift, bits)` per channel R, G, B, A.
type Masks = [(u32, u32, u32); 4];

fn mask_info(m: u32) -> (u32, u32, u32) {
    if m == 0 {
        return (0, 0, 0);
    }
    let shift = m.trailing_zeros();
    let bits = (m >> shift).trailing_ones();
    (m, shift, bits)
}

fn bit_masks(hd: &Header, bmi: &[u8]) -> Masks {
    let explicit = hd.compression == BI_BITFIELDS || hd.compression == BI_ALPHABITFIELDS;
    let rd = |i: usize| u32_at(bmi, 40 + i * 4).unwrap_or(0);
    if explicit && hd.bpp >= 16 && (hd.size >= 52 || bmi.len() >= 52) {
        let a = if hd.compression == BI_ALPHABITFIELDS || hd.size >= 56 {
            rd(3)
        } else {
            0
        };
        return [
            mask_info(rd(0)),
            mask_info(rd(1)),
            mask_info(rd(2)),
            mask_info(a),
        ];
    }
    if hd.bpp == 16 {
        // BI_RGB 16 bpp is X1R5G5B5.
        [
            mask_info(0x7C00),
            mask_info(0x03E0),
            mask_info(0x001F),
            mask_info(0),
        ]
    } else {
        [
            mask_info(0x00FF_0000),
            mask_info(0x0000_FF00),
            mask_info(0x0000_00FF),
            mask_info(0),
        ]
    }
}

fn scale_to_8(v: u32, bits: u32) -> u8 {
    match bits {
        0 => 0,
        8 => v as u8,
        b if b > 8 => (v >> (b - 8)) as u8,
        b => ((v * 255) / ((1u32 << b) - 1)) as u8,
    }
}

fn unpack_row(
    row: &[u8],
    bpp: u16,
    palette: &[[u8; 3]],
    masks: &Masks,
    compression: u32,
    opts: Opts,
    out: &mut [u8],
) {
    let w = out.len() / 4;
    for x in 0..w {
        let px = &mut out[x * 4..x * 4 + 4];
        match bpp {
            1 | 4 | 8 => {
                let per = 8 / bpp as usize;
                let byte = row[x / per];
                let shift = 8 - bpp as usize * (x % per + 1);
                let idx = ((byte >> shift) as usize) & ((1usize << bpp) - 1);
                let c = palette.get(idx).copied().unwrap_or([0, 0, 0]);
                px.copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
            24 => {
                let o = x * 3;
                px.copy_from_slice(&[row[o + 2], row[o + 1], row[o], 255]);
            }
            16 => {
                let v = u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                let ch = |m: (u32, u32, u32)| scale_to_8((v & m.0) >> m.1, m.2);
                px.copy_from_slice(&[ch(masks[0]), ch(masks[1]), ch(masks[2]), 255]);
            }
            _ => {
                let o = x * 4;
                let v = u32::from_le_bytes([row[o], row[o + 1], row[o + 2], row[o + 3]]);
                if compression == BI_RGB {
                    let a = if opts.alpha { row[o + 3] } else { 255 };
                    px.copy_from_slice(&[row[o + 2], row[o + 1], row[o], a]);
                    if opts.alpha && a != 0 && a != 255 {
                        // Premultiplied to straight.
                        for c in px[..3].iter_mut() {
                            *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                        }
                    }
                } else {
                    let ch = |m: (u32, u32, u32)| scale_to_8((v & m.0) >> m.1, m.2);
                    let a = if masks[3].0 != 0 && opts.alpha {
                        ch(masks[3])
                    } else {
                        255
                    };
                    px.copy_from_slice(&[ch(masks[0]), ch(masks[1]), ch(masks[2]), a]);
                }
            }
        }
    }
}

/// Run-length decoding (`BI_RLE8` / `BI_RLE4`). Pixels never written stay transparent.
fn rle(
    bits: &[u8],
    w: usize,
    h: usize,
    palette: &[[u8; 3]],
    bpp: u8,
    top_down: bool,
    rgba: &mut [u8],
) -> Option<()> {
    let mut put = |x: usize, y: usize, idx: usize| {
        if x < w && y < h {
            let row = if top_down { y } else { h - 1 - y };
            let o = (row * w + x) * 4;
            let c = palette.get(idx).copied().unwrap_or([0, 0, 0]);
            rgba[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    };
    let (mut x, mut y, mut i) = (0usize, 0usize, 0usize);
    while i + 1 < bits.len() && y < h + 1 {
        let (n, v) = (bits[i] as usize, bits[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                let idx = if bpp == 8 {
                    v as usize
                } else if k % 2 == 0 {
                    (v >> 4) as usize
                } else {
                    (v & 15) as usize
                };
                put(x, y, idx);
                x += 1;
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                x += *bits.get(i)? as usize;
                y += *bits.get(i + 1)? as usize;
                i += 2;
            }
            n => {
                let n = n as usize;
                let bytes = if bpp == 8 { n } else { n.div_ceil(2) };
                let data = bits.get(i..i + bytes)?;
                for k in 0..n {
                    let idx = if bpp == 8 {
                        data[k] as usize
                    } else if k % 2 == 0 {
                        (data[k / 2] >> 4) as usize
                    } else {
                        (data[k / 2] & 15) as usize
                    };
                    put(x, y, idx);
                    x += 1;
                }
                i += bytes + (bytes & 1);
            }
        }
    }
    Some(())
}

fn post(rgba: &mut [u8], opts: Opts) {
    if opts.invert {
        for px in rgba.as_chunks_mut::<4>().0 {
            px[0] = 255 - px[0];
            px[1] = 255 - px[1];
            px[2] = 255 - px[2];
        }
    }
    if let Some(k) = opts.key {
        for px in rgba.as_chunks_mut::<4>().0 {
            // Compared after any inversion, against the colour as stored in the file.
            let c = if opts.invert {
                [255 - px[0], 255 - px[1], 255 - px[2]]
            } else {
                [px[0], px[1], px[2]]
            };
            if c == k {
                px[3] = 0;
            }
        }
    }
}

impl Bitmap {
    /// The average colour of the opaque pixels (what a pattern brush is approximated by).
    pub fn average(&self) -> Option<[u8; 3]> {
        let BitmapData::Rgba(d) = &self.data else {
            return None;
        };
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        for px in d.as_chunks::<4>().0 {
            if px[3] > 0 {
                r += px[0] as u64;
                g += px[1] as u64;
                b += px[2] as u64;
                n += 1;
            }
        }
        (n > 0).then(|| [(r / n) as u8, (g / n) as u8, (b / n) as u8])
    }

    /// The part `(x, y, w, h)` of an RGBA bitmap (clamped to the picture); `None` for an
    /// encoded bitmap or an empty part.
    pub fn crop(&self, x: i64, y: i64, w: i64, h: i64) -> Option<Bitmap> {
        let BitmapData::Rgba(d) = &self.data else {
            return None;
        };
        let x0 = x.clamp(0, self.w as i64) as usize;
        let y0 = y.clamp(0, self.h as i64) as usize;
        let x1 = x.saturating_add(w).clamp(0, self.w as i64) as usize;
        let y1 = y.saturating_add(h).clamp(0, self.h as i64) as usize;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for row in y0..y1 {
            let o = (row * self.w as usize + x0) * 4;
            out.extend_from_slice(&d[o..o + (x1 - x0) * 4]);
        }
        Some(Bitmap {
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
            top_down: true,
            data: BitmapData::Rgba(out),
        })
    }

    /// `(mime, bytes)` ready for a data URI.
    pub fn to_encoded(&self) -> Option<(&'static str, Vec<u8>)> {
        match &self.data {
            BitmapData::Encoded(m, b) => Some((m, b.clone())),
            BitmapData::Rgba(d) => {
                let mut out = Vec::new();
                let enc = image::codecs::png::PngEncoder::new_with_quality(
                    &mut out,
                    image::codecs::png::CompressionType::Fast,
                    image::codecs::png::FilterType::Sub,
                );
                image::ImageEncoder::write_image(
                    enc,
                    d,
                    self.w,
                    self.h,
                    image::ExtendedColorType::Rgba8,
                )
                .ok()?;
                Some(("image/png", out))
            }
        }
    }
}
