//! konoma-native kitty graphics transmit + Unicode-placeholder rendering, with **zlib compression**.
//!
//! ratatui-image already transmits an image once and then draws it with the kitty protocol's
//! [Unicode placeholders], but it sends the pixels as **uncompressed RGBA**. For konoma's primary
//! content — screenshots, diagrams, UI mockups viewed next to an AI agent — that payload is several
//! MB and dominates the "3 seconds to show a full-screen image" latency (konoma's own CPU work is
//! ~37 ms; see `docs/PERF-IMAGE-TRANSFER-2026-07.md`). The kitty protocol supports `o=z`
//! (RFC 1950 zlib) compression of the transmitted data, which cuts that payload 2× (photos) to
//! ~50× (screenshots). ratatui-image exposes no hook to inject compression, so this module
//! re-implements just the kitty path with `o=z` added to the transmit.
//!
//! The Unicode-placeholder **rendering** (row diacritics, the id encoded in the cell foreground
//! color, the save/restore-cursor dance, per-cell skip) is a faithful port of ratatui-image
//! (`protocol/kitty.rs`, MIT) — that part is fiddly and battle-tested, so it is copied rather than
//! reinvented. The one behavioral change is the compressed transmit (`build_transmit`).
//!
//! [Unicode placeholders]: https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine as _;
use image::RgbaImage;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;

/// Force each placeholder cell to be treated as one column wide when diffing (the image data lives
/// in one cell but visually spans the row). Mirrors ratatui-image's `UNIT_WIDTH`.
const UNIT_WIDTH: CellDiffOption =
    CellDiffOption::ForcedWidth(std::num::NonZeroU16::new(1).unwrap());

/// Monotonic image-id source. Each opened image gets a fresh id so a new transmit never collides
/// with a still-referenced older one; kitty auto-removes a virtual placement once its placeholders
/// are gone, so no explicit delete is needed (same lifecycle as ratatui-image).
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// Allocate a fresh kitty image id (never 0 — kitty treats 0 as "unspecified").
pub fn next_id() -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

/// zlib-compress `raw` at a middle level (fast enough on the worker thread, near-max ratio for the
/// flat regions typical of screenshots/diagrams). Returns the RFC 1950 zlib stream `o=z` expects.
fn zlib_compress(raw: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    // Writing to a Vec never fails; keep the payload even if flush somehow errs (worst case the
    // terminal rejects it and the image simply does not show — never a crash).
    let _ = enc.write_all(raw);
    enc.finish().unwrap_or_default()
}

/// A prepared, compressed kitty image ready to transmit once and then place with Unicode
/// placeholders. Cheap to clone (holds the built escape strings).
#[derive(Clone)]
pub struct KittyImage {
    /// The full transmit escape (compressed pixels + virtual-placement header), sent on first render.
    transmit: String,
    /// Set once the transmit has been written into a frame; subsequent frames emit placeholders only.
    transmitted: std::rc::Rc<std::cell::Cell<bool>>,
    /// Display size in cells (the image was resized to exactly this many cells).
    cols: u16,
    rows: u16,
    /// The id encoded as a foreground-color SGR (`\x1b[38;2;r;g;b m`) — how a placeholder cell names
    /// its image — plus the extra high byte carried as a diacritic.
    id_color: String,
    id_extra: u16,
}

impl KittyImage {
    /// Build a compressed kitty image from an already-resized RGBA buffer (`cols`×`rows` cells).
    /// The pixel dimensions are taken from `rgba`; `cols`/`rows` are how many terminal cells it fills.
    pub fn build(rgba: &RgbaImage, cols: u16, rows: u16, id: u32, is_tmux: bool) -> Self {
        let transmit = build_transmit(rgba, id, is_tmux);
        let [id_extra, r, g, b] = id.to_be_bytes();
        Self {
            transmit,
            transmitted: std::rc::Rc::new(std::cell::Cell::new(false)),
            cols,
            rows,
            id_color: format!("\x1b[38;2;{r};{g};{b}m"),
            id_extra: u16::from(id_extra),
        }
    }

    /// Display size in cells this image was built (resized) for. Used to detect a terminal resize
    /// that changed the fit area without changing the crop (`prepare_image` then rebuilds).
    pub fn cell_size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Emit the image into `buf` at `area`: the transmit sequence once (first frame), then the
    /// Unicode-placeholder rows every frame. Faithful port of ratatui-image's kitty `render`.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let full_width = area.width.min(self.cols);
        let width_usize = usize::from(full_width);
        if width_usize == 0 {
            return;
        }

        // Transmit only on the first render of this image.
        let mut seq: Option<&str> = if self.transmitted.replace(true) {
            None
        } else {
            Some(self.transmit.as_str())
        };

        let row_diacritics: String =
            std::iter::repeat_n('\u{10EEEE}', width_usize.saturating_sub(1)).collect();

        // Restore the saved cursor (including its color), then walk to the row's end.
        let right = area.width - 1;
        let down = area.height - 1;
        let restore_cursor = format!("\x1b[u\x1b[{right}C\x1b[{down}B");

        // Clamp to the number of encodable placeholder rows (one diacritic per row).
        let height = area.height.min(self.rows).min(DIACRITICS.len() as u16);
        let mut symbol = String::new();
        for y in 0..height {
            symbol.clear();
            // Prepend the transmit exactly once, on the first row.
            if let Some(s) = seq.take() {
                symbol.push_str(s);
            }
            // Save cursor + fg color, then start the placeholder run: image row `y`, column 0,
            // and the id's high byte as a third diacritic.
            let _ = write!(
                symbol,
                "\x1b[s{}\u{10EEEE}{}{}{}",
                self.id_color,
                diacritic(y),
                diacritic(0),
                diacritic(self.id_extra),
            );
            symbol.push_str(&row_diacritics);
            // Mark the remaining cells of this row as skip so ratatui does not overwrite the image.
            for x in 1..full_width {
                if let Some(cell) = buf.cell_mut((area.left() + x, area.top() + y)) {
                    cell.set_diff_option(CellDiffOption::Skip);
                }
            }
            symbol.push_str(&restore_cursor);
            if let Some(cell) = buf.cell_mut((area.left(), area.top() + y)) {
                cell.set_symbol(&symbol).set_diff_option(UNIT_WIDTH);
            }
        }
    }
}

/// Build the kitty transmit escape for `img`, **zlib-compressed** (`o=z`), with a virtual
/// placement (`U=1`) so it can be positioned with Unicode placeholders.
///
/// Structure per the kitty spec: the pixels are RGBA (`f=32`), compressed as one zlib stream, then
/// base64-encoded and split into ≤4096-char chunks. The first chunk carries the control keys; every
/// chunk sets `m=1` until the last (`m=0`). Wrapped in tmux passthrough when `is_tmux`.
///
/// This is the one place that diverges from ratatui-image: the payload is compressed and the header
/// gains `o=z`.
pub fn build_transmit(img: &RgbaImage, id: u32, is_tmux: bool) -> String {
    let (w, h) = (img.width(), img.height());
    let compressed = zlib_compress(img.as_raw());
    let b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);

    let (start, escape, end) = tmux_wrap(is_tmux);

    const CHARS_PER_CHUNK: usize = 4096;
    let chunk_count = b64.len().div_ceil(CHARS_PER_CHUNK).max(1);
    let mut data = String::with_capacity(b64.len() + chunk_count * (16 + escape.len() * 2));

    let bytes = b64.as_bytes();
    for i in 0..chunk_count {
        let lo = i * CHARS_PER_CHUNK;
        let hi = (lo + CHARS_PER_CHUNK).min(b64.len());
        data.push_str(start);
        let _ = write!(data, "{escape}_Gq=2,");
        if i == 0 {
            // a=T transmit+display, U=1 virtual placement, f=32 RGBA, o=z zlib, t=d direct data.
            let _ = write!(data, "i={id},a=T,U=1,f=32,o=z,t=d,s={w},v={h},");
        }
        let more = u8::from(i + 1 < chunk_count);
        let _ = write!(data, "m={more};");
        // SAFETY: base64 output is ASCII, so any 4096-byte slice is a valid str.
        data.push_str(std::str::from_utf8(&bytes[lo..hi]).unwrap_or(""));
        let _ = write!(data, "{escape}\\");
        data.push_str(end);
    }
    data
}

/// tmux passthrough wrapping: `(start, escape, end)`. Outside tmux these are empty / bare ESC.
/// Mirrors ratatui-image's `Parser::tmux_start_escape_end`.
fn tmux_wrap(is_tmux: bool) -> (&'static str, &'static str, &'static str) {
    if is_tmux {
        ("\x1bPtmux;", "\x1b\x1b", "\x1b\\")
    } else {
        ("", "\x1b", "")
    }
}

/// The combining mark that encodes placeholder row/column index `i` (clamped to the first on
/// overflow — kitty only defines 297).
fn diacritic(i: u16) -> char {
    *DIACRITICS.get(usize::from(i)).unwrap_or(&DIACRITICS[0])
}

include!("kitty_diacritics.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    /// A small solid-color test image.
    fn solid(w: u32, h: u32, px: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(px))
    }

    /// Pull the base64 payload out of a (non-tmux) transmit escape and concatenate all chunks.
    fn payload_b64(transmit: &str) -> String {
        // Each chunk is `\x1b_G...;<b64>\x1b\\`. Take the text between the last `;` of the header
        // and the terminating `\x1b\\` for every chunk.
        let mut out = String::new();
        for chunk in transmit.split("\x1b_G").skip(1) {
            let after_header = &chunk[chunk.find(';').expect("chunk has a header") + 1..];
            let body = &after_header[..after_header.find('\x1b').expect("chunk terminator")];
            out.push_str(body);
        }
        out
    }

    #[test]
    fn transmit_round_trips_through_zlib() {
        // Non-solid content so compression is a real (not degenerate) round-trip.
        let mut img = RgbaImage::new(20, 10);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([(x * 11) as u8, (y * 23) as u8, (x + y) as u8, 255]);
        }
        let t = build_transmit(&img, 7, false);

        // Header sanity: RGBA, zlib, virtual placement, correct dimensions.
        assert!(t.contains("i=7,a=T,U=1,f=32,o=z,t=d,s=20,v=10,"));

        // Decode base64 → zlib-inflate → must equal the original RGBA bytes exactly.
        let b64 = payload_b64(&t);
        let compressed = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .expect("valid base64");
        let mut dec = flate2::read::ZlibDecoder::new(&compressed[..]);
        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut raw).expect("valid zlib");
        assert_eq!(raw, *img.as_raw(), "decompressed pixels match the source");
    }

    #[test]
    fn compression_shrinks_flat_images() {
        // A screenshot-like flat image should compress dramatically (the whole point).
        let img = solid(400, 300, [30, 40, 55, 255]);
        let raw_b64 = img.as_raw().len().div_ceil(3) * 4;
        let compressed_b64 = payload_b64(&build_transmit(&img, 1, false)).len();
        assert!(
            compressed_b64 * 10 < raw_b64,
            "flat image compresses >10x: {compressed_b64} vs {raw_b64}"
        );
    }

    #[test]
    fn tmux_wraps_each_chunk() {
        // Force multiple chunks with a larger image; every chunk must carry the tmux passthrough.
        let img = solid(200, 200, [1, 2, 3, 255]);
        let t = build_transmit(&img, 3, true);
        assert!(t.starts_with("\x1bPtmux;"), "tmux passthrough start");
        assert!(t.contains("\x1b\x1b_G"), "escapes are doubled inside tmux");
    }

    #[test]
    fn render_transmits_once_then_placeholders_only() {
        let img = solid(30, 20, [200, 100, 50, 255]);
        // 3 cols x 2 rows display.
        let ki = KittyImage::build(&img, 3, 2, 9, false);
        let area = Rect::new(0, 0, 3, 2);

        let mut buf = Buffer::empty(area);
        ki.render(area, &mut buf);
        let first: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(first.contains("o=z"), "first render carries the transmit");
        assert!(first.contains('\u{10EEEE}'), "placeholders are drawn");

        // A second render (same image) must NOT re-transmit — that is the whole savings.
        let mut buf2 = Buffer::empty(area);
        ki.render(area, &mut buf2);
        let second: String = buf2.content.iter().map(|c| c.symbol()).collect();
        assert!(
            !second.contains("o=z"),
            "second render is placeholders only"
        );
        assert!(second.contains('\u{10EEEE}'), "placeholders still drawn");
    }

    #[test]
    fn diacritics_table_is_complete() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(diacritic(0), DIACRITICS[0]);
        assert_eq!(diacritic(296), DIACRITICS[296]);
        assert_eq!(diacritic(999), DIACRITICS[0], "overflow clamps to first");
    }

    #[test]
    fn fresh_ids_are_nonzero_and_unique() {
        let a = next_id();
        let b = next_id();
        assert_ne!(a, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn cell_size_reports_build_dimensions() {
        // A terminal resize keeps the crop but changes the fit area; prepare_image compares this
        // against the target to know it must rebuild (regression: image kept its pre-resize size).
        let img = solid(30, 20, [10, 20, 30, 255]);
        let ki = KittyImage::build(&img, 7, 4, 1, false);
        assert_eq!(ki.cell_size(), (7, 4));
    }

    #[test]
    fn render_clamps_to_area_smaller_than_image() {
        // Area smaller than the built cell size (terminal shrank before a rebuild): must not panic,
        // and must emit at most `area` rows of placeholders.
        let img = solid(40, 40, [5, 5, 5, 255]);
        let ki = KittyImage::build(&img, 10, 8, 2, false);
        let area = Rect::new(0, 0, 3, 2); // smaller than 10x8
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 8));
        ki.render(area, &mut buf);
        // Rows outside the (small) area stay blank.
        let outside = buf.cell((0, 5)).map(|c| c.symbol().to_string());
        assert_eq!(
            outside.as_deref(),
            Some(" "),
            "no placeholder beyond the area"
        );
    }
}
