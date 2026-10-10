//! The cost model of the drawing process: how long the SVG the writer produced will take it.
//!
//! The drawing process (`svg_proc`) stops a drawing that runs past [`crate::preview::svg_proc::WALL_LIMIT`]
//! (5 s) and `svg_guard` refuses one whose filter work is too large, but neither knows what the
//! writer knows: how many path segments, characters, pictures and fills the markup holds. A slide
//! far under every byte budget can still take it seconds (a path of a million `lineTo`, a few
//! thousand lines of text, a compound line). So the writer *adds up what it emits* ([`Features`])
//! and predicts the time ([`Features::predict`]); an item that would take the slide past
//! [`MAX_CHILD_MS`] is first drawn without its optional parts (effects, compound lines, dashes,
//! picture colour effects) and, if that is still too much, left out. The result says so
//! (`truncated`).
//!
//! # The model
//!
//! Time is split by how it depends on the raster size (`r` = the raster's longer side over
//! [`super::svg::MODEL_RASTER_PX`]):
//!
//! * **fixed** (does not depend on `r`): parsing and converting the markup (elements, path
//!   segments), shaping glyphs, decoding pictures;
//! * **edge** (proportional to `r`): rasterising outlines, i.e. the length of the paths in device
//!   px;
//! * **area** (proportional to `r^2`): filling and stroking, painting pictures, filters and masks.
//!
//! The coefficients are in ms of CPU time of the drawing process on the development machine
//! (Apple M-series, release build), fitted by non-negative least squares to the process's CPU time
//! on about 260 hostile and ordinary decks (the `rr1` corpus), at 1280 px and at 3000 px. CPU time
//! is what a loaded machine charges honestly; wall time on a busy machine is several times it.

/// Most CPU time, in ms at the model raster, the drawing process may be asked to spend on one slide
/// (the wall limit is 5 s; the model is fitted on a machine that is not idle, and a slide that is
/// predicted at 1.5 s has margin for a machine three times slower).
pub const MAX_CHILD_MS: f64 = 1500.0;

/// What the writer emitted, measured in device px of the **model** raster (slide px times
/// `raster_scale`), see [`Features::predict`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Features {
    /// SVG elements written (start tags), `<use>` layers of effects included.
    pub els: f64,
    /// Path segments written (`M L Q C Z`), counted where the markup holds them.
    pub segs: f64,
    /// Characters of text drawn.
    pub glyphs: f64,
    /// Sum over the runs of text of the square of their length: a very long run costs the
    /// drawing process more than its length (measured: 100 000 characters in one run take 2.8 s,
    /// 20 000 take 0.18 s).
    pub glyphs_sq: f64,
    /// Runs of text (`<tspan>`): font selection is per run.
    pub spans: f64,
    /// Pixels decoded from raster pictures (pixels times uses).
    pub pic_dec_px: f64,
    /// Encoded bytes of pictures the process decodes (bytes times uses).
    pub pic_bytes: f64,
    /// Bytes of SVG pictures the process parses (bytes times uses).
    pub svg_bytes: f64,
    /// Length of the outlines (fills and strokes), device px.
    pub edge_px: f64,
    /// Area of solid fills, device px^2.
    pub fill_px2: f64,
    /// Area of gradient, pattern and tiled fills, device px^2.
    pub grad_px2: f64,
    /// Area painted by the elements of SVG pictures (see [`Vector`]), device px^2.
    pub vec_px2: f64,
    /// Area pictures are drawn into, device px^2.
    pub pic_px2: f64,
    /// Repetitions of tiles (bbox area over tile area).
    pub tiles: f64,
    /// Filter and mask work in the units of `svg_guard`.
    pub filter_units: f64,
}

#[path = "cost_vector.rs"]
mod vector;
pub use vector::{scan_vector, Vector};

/// The predicted time of a drawing, in ms of CPU, split by its dependence on the raster size.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Predicted {
    /// Independent of the raster size.
    pub fixed: f64,
    /// Proportional to the raster's side (`r`).
    pub edge: f64,
    /// Proportional to the raster's area (`r^2`).
    pub area: f64,
}

impl Predicted {
    /// The total at a raster `r` times the model's side.
    pub fn at(&self, r: f64) -> f64 {
        self.fixed + self.edge * r + self.area * r * r
    }

    /// The biggest `r` for which [`Predicted::at`] stays within `budget` ms: the positive root of
    /// `area r^2 + edge r + fixed = budget`. `f64::INFINITY` when nothing depends on the raster
    /// size, and 0 when the fixed part alone is over the budget.
    pub fn max_ratio(&self, budget: f64) -> f64 {
        let room = budget - self.fixed;
        if room.is_nan() || room <= 0.0 {
            return 0.0;
        }
        const TINY: f64 = 1e-12;
        if self.area > TINY {
            (-self.edge + (self.edge * self.edge + 4.0 * self.area * room).sqrt())
                / (2.0 * self.area)
        } else if self.edge > TINY {
            room / self.edge
        } else {
            f64::INFINITY
        }
    }
}

// ---- coefficients: ms of CPU per unit -------------------------------------------------------
//
// Fitted to the drawing process's CPU time on 264 decks at 1280 and 3000 px (see the module
// documentation), then rounded up where the fit was under what a case needed: the aim is a
// prediction that is never much below the time and is allowed to be above it (the median is
// twice the measurement; 95 % of the cases are within a factor of 5 above it, and the three
// worst cases within 1.7 below). Every constant is the time one unit of its feature costs on a
// machine that was three times oversubscribed, so an idle one takes about half.

/// What a drawing costs whatever is in it: the process's own bookkeeping.
const MS_BASE: f64 = 40.0;
/// An SVG element (parse, convert, one node in the tree).
const MS_PER_EL: f64 = 0.004;
/// A path segment (the path text and its conversion; flattening a curve).
const MS_PER_SEG: f64 = 0.0012;
/// A character of text (shaping, the glyph's outline).
const MS_PER_GLYPH: f64 = 0.0032;
/// The square of a run's length: what makes one very long run dearer than its pieces.
const MS_PER_GLYPH_SQ: f64 = 3e-7;
/// A run of text: choosing a font from its family list, laying the run out. Together with the
/// five elements a run is written as, 50 us: a page of 4500 text boxes takes 2.3 s.
const MS_PER_SPAN: f64 = 0.03;
/// A pixel of a raster picture, decoded each time it is used.
const MS_PER_DEC_PX: f64 = 5e-6;
/// A byte of an embedded picture, through base64 and the decoder, each time it is used.
const MS_PER_PIC_BYTE: f64 = 2.6e-6;
/// A byte of an SVG picture's markup, parsed each time it is used.
const MS_PER_SVG_BYTE: f64 = 5e-5;
/// A repetition of a tile.
const MS_PER_TILE: f64 = 5e-7;
/// A device pixel of outline (fill or stroke) at the model raster: rasterising long lines costs
/// 50 to 120 ns a pixel.
const MS_PER_EDGE_PX: f64 = 5e-5;
/// A device pixel of a solid fill, by the box of its shape (translucent shapes and curves cost
/// more than a rectangle's blit; the box is what the writer knows).
const MS_PER_FILL_PX2: f64 = 2.5e-6;
/// A device pixel of a gradient, pattern or tiled fill.
const MS_PER_GRAD_PX2: f64 = 4e-6;
/// A device pixel painted by the elements of an SVG picture.
const MS_PER_VEC_PX2: f64 = 2.5e-5;
/// A device pixel of a picture drawn into its box.
const MS_PER_PIC_PX2: f64 = 1e-6;
/// A unit of filter or mask work as `svg_guard` counts it (a Gaussian blur pixel is 12): 1.8 to
/// 2.2 ns for filters, more for the masks of compound lines.
const MS_PER_FILTER_UNIT: f64 = 3.4e-6;

impl Features {
    /// `self` plus `o` scaled by `k` (the layers an effect draws a shape in).
    pub fn add_scaled(&mut self, o: &Features, k: f64) {
        self.els += o.els * k;
        self.segs += o.segs * k;
        self.glyphs += o.glyphs * k;
        self.glyphs_sq += o.glyphs_sq * k;
        self.spans += o.spans * k;
        self.pic_dec_px += o.pic_dec_px * k;
        self.pic_bytes += o.pic_bytes * k;
        self.svg_bytes += o.svg_bytes * k;
        self.edge_px += o.edge_px * k;
        self.fill_px2 += o.fill_px2 * k;
        self.grad_px2 += o.grad_px2 * k;
        self.vec_px2 += o.vec_px2 * k;
        self.pic_px2 += o.pic_px2 * k;
        self.tiles += o.tiles * k;
        self.filter_units += o.filter_units * k;
    }

    /// The predicted time of drawing what these features describe.
    pub fn predict(&self) -> Predicted {
        Predicted {
            fixed: MS_BASE
                + self.els * MS_PER_EL
                + self.segs * MS_PER_SEG
                + self.glyphs * MS_PER_GLYPH
                + self.glyphs_sq * MS_PER_GLYPH_SQ
                + self.spans * MS_PER_SPAN
                + self.pic_dec_px * MS_PER_DEC_PX
                + self.pic_bytes * MS_PER_PIC_BYTE
                + self.svg_bytes * MS_PER_SVG_BYTE
                + self.tiles * MS_PER_TILE,
            edge: self.edge_px * MS_PER_EDGE_PX,
            area: self.fill_px2 * MS_PER_FILL_PX2
                + self.grad_px2 * MS_PER_GRAD_PX2
                + self.vec_px2 * MS_PER_VEC_PX2
                + self.pic_px2 * MS_PER_PIC_PX2
                + self.filter_units * MS_PER_FILTER_UNIT,
        }
    }
}

/// What a scan of markup found.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scan {
    pub els: f64,
    pub segs: f64,
    pub glyphs: f64,
    pub glyphs_sq: f64,
    pub spans: f64,
}

/// Counts what `markup` holds: elements (start tags), path segments in `d="..."` attributes,
/// `<tspan>` runs and the characters of their text. The writer's own output only (no comments, no
/// CDATA, attribute values never contain `<`).
pub fn scan(markup: &str) -> Scan {
    let b = markup.as_bytes();
    let mut s = Scan::default();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'<' => {
                match b.get(i + 1) {
                    Some(b'/') | Some(b'!') | Some(b'?') | None => {}
                    Some(_) => {
                        s.els += 1.0;
                        if b[i + 1..].starts_with(b"tspan") {
                            s.spans += 1.0;
                            // The text of the run: from the end of the start tag to `</tspan>`.
                            let tag_end = b[i..]
                                .iter()
                                .position(|&c| c == b'>')
                                .map_or(b.len(), |p| i + p);
                            let close = find(b, tag_end, b"</tspan>").unwrap_or(b.len());
                            let n = markup[(tag_end + 1).min(markup.len())
                                ..close.max(tag_end + 1).min(markup.len())]
                                .chars()
                                .count() as f64;
                            s.glyphs += n;
                            s.glyphs_sq += n * n;
                        }
                    }
                }
                i += 1;
            }
            b' ' if b[i + 1..].starts_with(b"d=\"") => {
                i += 4;
                while i < b.len() && b[i] != b'"' {
                    if matches!(b[i], b'M' | b'L' | b'Q' | b'C' | b'Z') {
                        s.segs += 1.0;
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    s
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_counts_elements_segments_runs_and_characters() {
        let m = r##"<g transform="translate(1 2)"><path d="M0 0 L10 0 L10 10 Z" fill="#000"/><text xml:space="preserve"><tspan x="1" y="2" font-family="A">héllo</tspan><tspan x="9" y="2">ab</tspan></text><clipPath id="c1"><path d="M0 0C1 1 2 2 3 3Q4 4 5 5"/></clipPath></g>"##;
        let s = scan(m);
        // g, path, text, tspan, tspan, clipPath, path
        assert_eq!(s.els, 7.0);
        assert_eq!(s.segs, 4.0 + 3.0);
        assert_eq!(s.spans, 2.0);
        assert_eq!(s.glyphs, 7.0, "characters, not bytes");
    }

    #[test]
    fn scan_ignores_closing_tags_comments_and_other_attributes_ending_in_d() {
        let s =
            scan(r#"<!-- c --><?x?><rect id="d1" width="1"/></g><image id="im" href="data:x"/>"#);
        assert_eq!(s.els, 2.0);
        assert_eq!(s.segs, 0.0, "`id=\"` is not a path's `d=\"`");
        assert_eq!(scan("").els, 0.0);
        assert_eq!(scan("<").els, 0.0);
        assert_eq!(
            scan("<tspan").spans,
            1.0,
            "an unfinished tag does not run past the end"
        );
        assert_eq!(scan("<tspan x=\"1\">no end").glyphs, 6.0);
    }

    #[test]
    fn predicted_time_adds_up_by_how_it_grows_with_the_raster() {
        let p = Predicted {
            fixed: 100.0,
            edge: 50.0,
            area: 10.0,
        };
        assert_eq!(p.at(1.0), 160.0);
        assert_eq!(p.at(2.0), 100.0 + 100.0 + 40.0);
        assert_eq!(p.at(0.0), 100.0);
    }

    #[test]
    fn max_ratio_is_the_root_of_the_budget_and_monotonic() {
        let p = Predicted {
            fixed: 100.0,
            edge: 50.0,
            area: 10.0,
        };
        let r = p.max_ratio(1000.0);
        assert!((p.at(r) - 1000.0).abs() < 1e-6, "{r}");
        assert!(p.max_ratio(2000.0) > r);
        // Only an edge part, only an area part, neither.
        let e = Predicted {
            fixed: 0.0,
            edge: 100.0,
            area: 0.0,
        };
        assert_eq!(e.max_ratio(300.0), 3.0);
        let a = Predicted {
            fixed: 0.0,
            edge: 0.0,
            area: 100.0,
        };
        assert!((a.max_ratio(400.0) - 2.0).abs() < 1e-9);
        assert!(Predicted::default().max_ratio(1.0).is_infinite());
        // The fixed part alone over the budget: no raster at all (the caller floors it).
        assert_eq!(p.max_ratio(100.0), 0.0);
        assert_eq!(p.max_ratio(50.0), 0.0);
        assert_eq!(p.max_ratio(f64::NAN), 0.0);
    }

    #[test]
    fn features_add_scaled_covers_every_field() {
        let one = Features {
            els: 1.0,
            segs: 1.0,
            glyphs: 1.0,
            glyphs_sq: 1.0,
            spans: 1.0,
            pic_dec_px: 1.0,
            pic_bytes: 1.0,
            svg_bytes: 1.0,
            edge_px: 1.0,
            fill_px2: 1.0,
            grad_px2: 1.0,
            vec_px2: 1.0,
            pic_px2: 1.0,
            tiles: 1.0,
            filter_units: 1.0,
        };
        let mut f = Features::default();
        f.add_scaled(&one, 3.0);
        f.add_scaled(&one, 2.0);
        let five = Features {
            els: 5.0,
            segs: 5.0,
            glyphs: 5.0,
            glyphs_sq: 5.0,
            spans: 5.0,
            pic_dec_px: 5.0,
            pic_bytes: 5.0,
            svg_bytes: 5.0,
            edge_px: 5.0,
            fill_px2: 5.0,
            grad_px2: 5.0,
            vec_px2: 5.0,
            pic_px2: 5.0,
            tiles: 5.0,
            filter_units: 5.0,
        };
        assert_eq!(f, five);
    }
}
