//! EMF and WMF pictures (the Windows metafiles Office decks carry for pasted Visio / Excel
//! objects, OLE previews and old clip art) converted to a self-contained SVG.
//!
//! Both formats record GDI calls, so both are played into one device context ([`gdi`]): the
//! parsers ([`emf`], [`wmf`]) read records and call it; it keeps the mapping mode, the world
//! transform, the selected pen / brush / font, paths and clipping, and writes the SVG. Bitmaps go
//! through [`dib`].
//!
//! The input is never trusted. Every length is checked against the bytes really present before it
//! is used, nothing is allocated from a count in the file (object tables are maps that grow
//! as objects are created), and the work and the output are budgeted: records, objects, points per
//! record, path bytes, markup bytes, embedded image bytes, bitmap pixels, nesting of saved states
//! and clips, text length. Over a budget the rest is dropped and `truncated` is set; a record
//! whose size field is wrong stops the parse the same way. Nothing here panics on any input.
//!
//! # Approximations
//!
//! * Raster operations other than copy are approximated: `R2_NOP` draws nothing, source-AND / OR /
//!   XOR bitmaps use the SVG blend modes multiply / screen / difference, the other ROPs (and
//!   `SETROP2` modes such as XOR) draw normally.
//! * Pattern brushes (`CREATEDIBPATTERNBRUSHPT`, `CREATEMONOBRUSH`) are the bitmap's average
//!   colour (mid-grey when it cannot be decoded); hatch brushes are drawn as an 8 px pattern.
//! * Dashed pens use Windows' fixed patterns for one-pixel pens and multiples of the width for
//!   wider ones. Gaps are not filled with the background colour.
//! * A clip combined by OR / XOR with an earlier one replaces it; clips nest at most 16 deep.
//! * Text is SVG text in a substitute face: glyph widths come from the `dx` array when the file has
//!   one, otherwise from the font; `ETO_GLYPH_INDEX` text is skipped.
//! * Triangle gradient fills are the flat average colour of the three corners.
//! * EMF+ is not drawn: dual files use their GDI records, an EMF+-only file is refused.

mod dib;
mod emf;
mod gdi;
mod wmf;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_cancel;
#[cfg(test)]
mod tests_dib_mutation;
#[cfg(test)]
mod tests_emf;
#[cfg(test)]
mod tests_hostile;
#[cfg(test)]
mod tests_wmf;

use std::fmt::Write as _;

/// An SVG document made from a metafile.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaSvg {
    /// A complete `<svg>` document; its `viewBox` is the picture's frame.
    pub svg: String,
    /// The picture's natural size at 96 dpi.
    pub width_px: f64,
    pub height_px: f64,
    /// Something was dropped: a budget was reached or a record was malformed.
    pub truncated: bool,
}

/// Converts an EMF or WMF file; `None` when `bytes` is neither, is empty of any usable header, or
/// is an EMF+-only file.
#[cfg(test)]
pub fn to_svg(bytes: &[u8]) -> Option<MetaSvg> {
    to_svg_cancellable(bytes, &|| false)
}

/// [`to_svg`] that polls `cancel` every [`CANCEL_EVERY`] records and gives up (`None`) once it
/// says yes.
pub fn to_svg_cancellable(bytes: &[u8], cancel: &dyn Fn() -> bool) -> Option<MetaSvg> {
    #[cfg(test)]
    CONVERSIONS.with(|c| c.set(c.get() + 1));
    if emf::is_emf(bytes) {
        emf::convert(bytes, cancel)
    } else if wmf::is_wmf(bytes) {
        wmf::convert(bytes, cancel)
    } else {
        None
    }
}

#[cfg(test)]
thread_local! {
    /// How many conversions this thread has started (the tests count them to see that a metafile
    /// used many times in one render is converted once).
    pub(super) static CONVERSIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records between two looks at the cancellation callback.
pub(super) const CANCEL_EVERY: usize = 1024;

/// Most records one metafile may hold before the rest is ignored.
pub(super) const MAX_RECORDS: usize = 500_000;
/// Most points one record may carry.
pub(super) const MAX_POINTS: usize = 200_000;
/// Most objects alive at once.
pub(super) const MAX_OBJECTS: usize = 65_536;

pub(super) fn u16le(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}
pub(super) fn i16le(b: &[u8], o: usize) -> Option<i16> {
    u16le(b, o).map(|v| v as i16)
}
pub(super) fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}
pub(super) fn i32le(b: &[u8], o: usize) -> Option<i32> {
    u32le(b, o).map(|v| v as i32)
}
pub(super) fn f32le(b: &[u8], o: usize) -> Option<f64> {
    u32le(b, o).map(|v| f32::from_bits(v) as f64)
}

/// `COLORREF` (0x00BBGGRR) to RGB.
pub(super) fn colorref(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// The blend mode (and whether to invert the colours) a bitmap raster operation becomes.
pub(super) fn blend_of(rop: u32) -> (Option<&'static str>, bool) {
    match rop {
        0x00EE_0086 => (Some("screen"), false),
        0x0088_00C6 => (Some("multiply"), false),
        0x0066_0046 => (Some("difference"), false),
        0x0033_0008 => (None, true),
        _ => (None, false),
    }
}

/// The text encoding of a GDI character set.
pub(super) fn charset_encoding(cs: u8) -> &'static encoding_rs::Encoding {
    use encoding_rs as e;
    match cs {
        128 => e::SHIFT_JIS,
        129 | 130 => e::EUC_KR,
        134 => e::GBK,
        136 => e::BIG5,
        161 => e::WINDOWS_1253,
        162 => e::WINDOWS_1254,
        177 => e::WINDOWS_1255,
        178 => e::WINDOWS_1256,
        186 => e::WINDOWS_1257,
        204 => e::WINDOWS_1251,
        222 => e::WINDOWS_874,
        238 => e::WINDOWS_1250,
        _ => e::WINDOWS_1252,
    }
}

/// Decodes `bytes` (single- or multi-byte text of `charset`) to characters.
pub(super) fn decode_text(bytes: &[u8], charset: u8) -> Vec<char> {
    let (s, _, _) = charset_encoding(charset).decode(bytes);
    s.chars().take(gdi::MAX_TEXT_CHARS).collect()
}

/// Rectangles of an `RGNDATA` block (header, then `RECT`s of l, t, r, b).
pub(super) fn region_rects(data: &[u8]) -> Option<Vec<[f64; 4]>> {
    let size = u32le(data, 0)? as usize;
    let count = u32le(data, 8)? as usize;
    if size < 32 || count > MAX_POINTS {
        return None;
    }
    let body = data.get(size..)?;
    let count = count.min(body.len() / 16);
    let mut v = Vec::with_capacity(count);
    for i in 0..count {
        let o = i * 16;
        v.push([
            i32le(body, o)? as f64,
            i32le(body, o + 4)? as f64,
            i32le(body, o + 8)? as f64,
            i32le(body, o + 12)? as f64,
        ]);
    }
    Some(v)
}

/// Wraps the drawing into an SVG document. `view` = `[x, y, w, h]` in device units, `size_px` =
/// the natural size at 96 dpi.
pub(super) fn assemble(
    view: [f64; 4],
    size_px: (f64, f64),
    defs: &str,
    body: &str,
    truncated: bool,
) -> Option<MetaSvg> {
    if !(view.iter().all(|v| v.is_finite()) && view[2] > 0.0 && view[3] > 0.0) {
        return None;
    }
    let (mut w, mut h) = size_px;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    // A natural size that no one could draw is brought into a sane range, keeping the ratio.
    let big = w.max(h);
    if big > 20_000.0 {
        w *= 20_000.0 / big;
        h *= 20_000.0 / big;
    }
    let small = w.min(h);
    if small < 1.0 {
        let k = 1.0 / small;
        (w, h) = ((w * k).min(20_000.0), (h * k).min(20_000.0));
    }
    let mut svg = String::with_capacity(body.len() + defs.len() + 256);
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}" height="{}" preserveAspectRatio="none">"#,
        gdi::n(view[0]),
        gdi::n(view[1]),
        gdi::n(view[2]),
        gdi::n(view[3]),
        gdi::n(w),
        gdi::n(h),
    );
    if !defs.is_empty() {
        svg.push_str("<defs>");
        svg.push_str(defs);
        svg.push_str("</defs>");
    }
    svg.push_str(body);
    svg.push_str("</svg>");
    Some(MetaSvg {
        svg,
        width_px: w,
        height_px: h,
        truncated,
    })
}
