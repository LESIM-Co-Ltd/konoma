//! LaTeX math → SVG (pure Rust, via RaTeX), rasterized in-process by konoma's resvg path.
//!
//! Mirrors the mermaid/SVG handling: a math expression is turned into a *standalone* SVG (glyphs
//! embedded as `<path>` outlines via the `embed-fonts` feature, so resvg needs no external fonts),
//! and from there it flows through the exact same image pipeline (rasterize → kitty graphics). A
//! parse/layout/render failure returns `None`, and the caller degrades to the raw LaTeX text
//! (design principle #3 — never crash, never silently drop).

use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_parser::parse;
use ratex_svg::{render_to_svg, SvgOptions};
use ratex_types::math_style::MathStyle;
use resvg::usvg;

/// `math_cells` (src/app.rs) treats an SVG's intrinsic height as em units at this rate — "RaTeX
/// renders at 40 units/em" — and [`normalize_box`] targets the same convention so a normalized
/// SVG's reported em height (`intrinsic_size_bytes(..).1 / 40`) lands where `math_cells` expects.
const CANONICAL_UNITS_PER_EM: f64 = 40.0;
/// A terminal cell's height in ems, roughly (Markdown/browser math renderers assume the same).
const LINE_HEIGHT_EM: f64 = 1.2;
/// Hairline of ink padding (canonical units, ~0.025em) so a normalized box's edge never crops
/// antialiasing at the exact geometric bounding box — imperceptible, not a layout margin.
const INK_PADDING_CANONICAL: f64 = 1.0;

/// Render a LaTeX *math* expression (the content between the `$`/`$$` delimiters, without them) to a
/// standalone SVG document string. `display` selects TeX display style (block `$$…$$`) vs text style
/// (inline `$…$`), matching KaTeX. `color` is a valid SVG/CSS color the glyphs and rules are painted
/// in (RaTeX paints everything pure black by default, which is invisible on konoma's dark terminal —
/// see [`recolor`]). Returns `None` when the expression is empty or RaTeX cannot parse/lay out/render
/// it — the caller then falls back to the raw source. Panic-safe for the caller via `catch_silent` at
/// the worker boundary (RaTeX is pre-1.0).
pub fn latex_to_svg(latex: &str, display: bool, color: &str) -> Option<String> {
    let svg = render_raw(latex, display, color)?;
    // Inline math only (see `normalize_box`'s own doc comment for why display math is left as
    // RaTeX laid it out).
    let svg = if display { svg } else { normalize_box(svg) };
    // A well-formed SVG always carries an `<svg` root; guard against an empty/degenerate string so a
    // downstream rasterize never sees garbage.
    svg.contains("<svg").then_some(svg)
}

/// The part of [`latex_to_svg`] before box normalization: parse → layout → render → recolor.
/// Split out so tests can compare RaTeX's own (padded) box against [`normalize_box`]'s result —
/// production code only ever reaches this through `latex_to_svg`.
fn render_raw(latex: &str, display: bool, color: &str) -> Option<String> {
    let latex = latex.trim();
    if latex.is_empty() {
        return None;
    }
    let nodes = parse(latex).ok()?;
    if nodes.is_empty() {
        return None;
    }
    let opts = LayoutOptions {
        style: if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        },
        ..Default::default()
    };
    let root = layout(&nodes, &opts);
    let list = to_display_list(&root);
    let svg = render_to_svg(
        &list,
        &SvgOptions {
            // Standalone glyph outlines (embed-fonts) so konoma's resvg renders without KaTeX webfonts.
            embed_glyphs: true,
            ..Default::default()
        },
    );
    Some(recolor(svg, color))
}

/// Repaint the equation's glyphs and rules from RaTeX's default pure black (`rgba(0,0,0,1)`, the only
/// fill it emits) to `color`. The background stays transparent, so — exactly like the mermaid images —
/// the terminal background shows through and only the ink is colored. konoma is dark-terminal-first,
/// so the default is a light gray; black-on-transparent was the reason equations rendered *blank*
/// (the raster carried opaque ink, but it was black on a dark background = invisible). Only the black
/// fill is swapped, so an explicit `\color{…}` in the LaTeX (a non-black fill) is preserved.
fn recolor(svg: String, color: &str) -> String {
    svg.replace("rgba(0,0,0,1)", color)
}

/// Shrink an **inline** equation's SVG box down to (approximately) its own ink, so it draws at the
/// same size as the surrounding text instead of at RaTeX's own layout box.
///
/// # Why this exists
///
/// `math_cells` (src/app.rs) always reserves exactly **one terminal row** for an inline expression,
/// then the whole SVG box — ink plus RaTeX's ascent/descent padding — is scaled to fill that one
/// row. RaTeX's layout box carries generous padding (room for accents, deep subscripts, tall
/// delimiters — few expressions actually use), so the row ends up mostly padding and the glyphs
/// draw small. Worse, the amount of padding varies per expression (a bare `x` is ~57% ink by area,
/// `E = mc^2` ~34%), so different equations in the same document draw at *different* sizes relative
/// to each other, not just relative to the text (measured on real renders: `x` at 96% of the
/// surrounding text's size, `E = mc^2` at 68%).
///
/// # The rule
///
/// The box height becomes `max(1.2em, ink height)`, with the ink centered inside it:
/// - Ink at most 1.2em tall (`x`, `E = mc^2`, `H_2O`, most single-line expressions) gets a box of
///   exactly one line — it now draws at the surrounding text's size, and so does every other
///   expression whose ink fits, closing the "different sizes for different equations" gap.
/// - A taller expression (`\frac{..}{..}`, `\sum`, a matrix) keeps a box equal to its own ink: a
///   terminal row's height is fixed and there's nowhere else to put the rest, but it is at least
///   scaled by the *same* rule as everything else, rather than by however much padding RaTeX
///   happened to lay out around it.
///
/// The box width is tightened to the ink's own width, too: the horizontal whitespace in RaTeX's
/// layout box is redundant with the ordinary Markdown space around `$…$`, and stacked on top of it
/// read as an oversized gap between the equation and its surrounding words.
///
/// # Display math is untouched
///
/// Unlike inline, a display expression's row count is *proportional* to its box height
/// (`math_cells`: `rows = round(em_h * 1.5)`), not pinned to one row. Padding then cancels out of
/// the apparent glyph size — displayed ink height works out to `(ink_em / 40) * 1.5 * fh` regardless
/// of how much padding surrounds it in the box, because both the row count and the ink's share of
/// each row scale with the same box height. So display math was never the "text-scale" bug this
/// fixes; normalizing it would only trim the blank rows framing an already-independent line, a
/// cosmetic change with no reported problem behind it — left alone.
///
/// # Failure mode
///
/// Returns `svg` byte-for-byte unchanged if it can't be parsed, its header isn't the shape
/// `ratex_svg::render_to_svg` always emits, or the computed geometry is degenerate — never panics,
/// never produces a broken SVG (design principle #3).
fn normalize_box(svg: String) -> String {
    let Some((declared_w, declared_h)) = parse_declared_size(&svg) else {
        return svg;
    };
    if !(declared_w > 0.0 && declared_h > 0.0) {
        return svg;
    }
    let opt = usvg::Options {
        fontdb: crate::preview::svg::shared_fontdb(),
        ..Default::default()
    };
    let Ok(tree) = usvg::Tree::from_str(&svg, &opt) else {
        return svg;
    };
    let size = tree.size();
    let (canvas_w, canvas_h) = (size.width() as f64, size.height() as f64);
    if !(canvas_w.is_finite() && canvas_h.is_finite() && canvas_w > 0.0 && canvas_h > 0.0) {
        return svg;
    }
    // How declared (viewBox / path-coordinate) units map to the canvas units `tree.size()` — and
    // `math_cells`'s "40 units/em" — use. ratex_svg always declares `width`/`height` in `pt`, so
    // this is normally the fixed 96/72 pt→px factor; derived rather than hardcoded so a future
    // change of declared unit keeps this correct (averaging both axes for a stray rounding hedge —
    // it's a uniform, non-distorting conversion, so both axes agree).
    let k = ((canvas_w / declared_w) + (canvas_h / declared_h)) / 2.0;
    if !(k.is_finite() && k > 0.0) {
        return svg;
    }

    // Ink bounding box, already in *canonical* (canvas, `k`-scaled) coordinates — usvg's `abs_*`
    // boxes are in the tree's canvas space, the same space `tree.size()` reports (verified against
    // a real render: RaTeX's `x` glyph path data tops out at coordinate 30.88, while its reported
    // `abs_bounding_box` right edge is 41.17 = 30.88 * k — the box is already canvas-scaled, *not*
    // in the raw `<path d="…">` units, so it must not be multiplied by `k` again below).
    // `abs_stroke_bounding_box` is the superset of `abs_bounding_box` that also covers stroked
    // geometry (rules render as filled rects here, so in practice the two are identical — measured
    // — but the superset is the safe one to crop by).
    let bbox = tree.root().abs_stroke_bounding_box();
    let (ink_x0_c, ink_y0_c, ink_w_c, ink_h_c) = (
        bbox.x() as f64,
        bbox.y() as f64,
        bbox.width() as f64,
        bbox.height() as f64,
    );
    if !(ink_w_c.is_finite() && ink_h_c.is_finite() && ink_w_c > 0.0 && ink_h_c > 0.0) {
        return svg;
    }

    // From here, everything is in canonical units to match `math_cells`'s "1em = 40 canonical
    // units", then converted back to declared units (÷ k) to write the new viewBox (the `<path>`
    // data itself stays in declared units, untouched).
    let line_height_c = LINE_HEIGHT_EM * CANONICAL_UNITS_PER_EM;
    let new_h_c = (ink_h_c + 2.0 * INK_PADDING_CANONICAL).max(line_height_c);
    let vpad_c = (new_h_c - ink_h_c) / 2.0;
    let new_w_c = ink_w_c + 2.0 * INK_PADDING_CANONICAL;
    let hpad_c = INK_PADDING_CANONICAL;

    let new_x0 = (ink_x0_c - hpad_c) / k;
    let new_y0 = (ink_y0_c - vpad_c) / k;
    let new_w = new_w_c / k;
    let new_h = new_h_c / k;
    if !(new_w.is_finite() && new_h.is_finite() && new_w > 0.0 && new_h > 0.0) {
        return svg;
    }

    rewrite_svg_box(&svg, new_x0, new_y0, new_w, new_h).unwrap_or(svg)
}

/// Read the `<svg>` root's declared `width="{n}pt"` / `height="{n}pt"` (the shape
/// `ratex_svg::render_to_svg` always emits — see its own `<svg xmlns="…" viewBox="0 0 {w} {h}"
/// width="{w}pt" height="{h}pt">` template). `None` if that shape isn't found (a future ratex_svg
/// version changed it, or `svg` isn't well-formed) — the caller then leaves the SVG unnormalized
/// rather than guess.
fn parse_declared_size(svg: &str) -> Option<(f64, f64)> {
    let w = parse_quoted_number(svg, "width=\"", "pt\"")?;
    let h = parse_quoted_number(svg, "height=\"", "pt\"")?;
    Some((w, h))
}

/// The number between the first `svg.find(prefix)` and the following `suffix`, e.g.
/// `parse_quoted_number(r#"width="12.5pt""#, "width=\"", "pt\"")` → `Some(12.5)`.
fn parse_quoted_number(svg: &str, prefix: &str, suffix: &str) -> Option<f64> {
    let start = svg.find(prefix)? + prefix.len();
    let rest = &svg[start..];
    let end = rest.find(suffix)?;
    rest[..end].parse::<f64>().ok()
}

/// Rewrite the `<svg>` root's `viewBox`/`width`/`height` to `(x0, y0, w, h)` (declared units),
/// leaving the `<path>` body — and everything else — byte-for-byte as RaTeX emitted it. `None` if
/// the expected `viewBox="…"` / `width="…pt"` / `height="…pt"` attributes aren't found.
fn rewrite_svg_box(svg: &str, x0: f64, y0: f64, w: f64, h: f64) -> Option<String> {
    let svg = replace_quoted(
        svg,
        "viewBox=\"",
        "\"",
        &format!("{x0:.4} {y0:.4} {w:.4} {h:.4}"),
    )?;
    let svg = replace_quoted(&svg, "width=\"", "pt\"", &format!("{w:.4}"))?;
    let svg = replace_quoted(&svg, "height=\"", "pt\"", &format!("{h:.4}"))?;
    Some(svg)
}

/// Replace the text strictly between the first `prefix` and the following `suffix` with
/// `new_inner`, keeping `prefix`/`suffix` themselves and everything outside that span untouched.
fn replace_quoted(svg: &str, prefix: &str, suffix: &str, new_inner: &str) -> Option<String> {
    let start = svg.find(prefix)? + prefix.len();
    let rest = &svg[start..];
    let end = rest.find(suffix)?;
    let mut out = String::with_capacity(svg.len());
    out.push_str(&svg[..start]);
    out.push_str(new_inner);
    out.push_str(&rest[end..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn renders_simple_math_to_rasterizable_svg() {
        // The whole point: LaTeX → standalone SVG → konoma's resvg rasterizes it (no external fonts).
        for (latex, display) in [("x^2 + y^2 = z^2", true), ("\\frac{a}{b}", false)] {
            let svg = latex_to_svg(latex, display, "#d0d0d0").expect("RaTeX renders basic math");
            assert!(svg.contains("<svg"), "has an <svg> root: {latex}");
            // embed-fonts emits vector <path> glyph outlines (not <text> needing KaTeX webfonts).
            assert!(
                svg.contains("<path") || svg.contains("<image"),
                "standalone glyphs (path/image), not webfont <text>: {latex}"
            );
            let img =
                crate::preview::svg::rasterize_bytes(svg.as_bytes(), Path::new("m.svg"), 1600)
                    .expect("resvg rasterizes the standalone SVG");
            assert!(
                img.width() > 0 && img.height() > 0,
                "non-empty raster: {latex}"
            );
            // The raster must carry actual ink (opaque glyph pixels), not be a blank/transparent box.
            use image::GenericImageView;
            let ink = img.pixels().filter(|(_, _, p)| p.0[3] > 16).count();
            assert!(
                ink > 50,
                "equation raster has visible ink: {latex} (ink={ink})"
            );
        }
    }

    #[test]
    fn empty_or_garbage_returns_none_not_panic() {
        assert!(latex_to_svg("", true, "#d0d0d0").is_none());
        assert!(latex_to_svg("   ", false, "#d0d0d0").is_none());
        // Nonsense/incomplete LaTeX must degrade to None (→ raw-text fallback), never panic.
        let _ = latex_to_svg("\\frac{", true, "#d0d0d0");
        let _ = latex_to_svg("\\undefinedcmd{x}", false, "#d0d0d0");
    }

    #[test]
    fn display_and_inline_styles_both_render() {
        // Same expression, both styles produce a valid SVG (display taller than inline typically).
        let d = latex_to_svg("\\sum_{i=0}^{n} i", true, "#d0d0d0").expect("display");
        let i = latex_to_svg("\\sum_{i=0}^{n} i", false, "#d0d0d0").expect("inline");
        assert!(d.contains("<svg") && i.contains("<svg"));
    }

    /// Regression 2026-07-20: RaTeX paints every glyph pure black (`rgba(0,0,0,1)`), so on konoma's
    /// dark terminal black-on-dark is **invisible** (the reason the reserved slot showed an image yet
    /// looked blank). Repaint with `color` and mechanically verify that no black remains in the output,
    /// the specified color is applied, and opaque ink actually exists.
    #[test]
    fn recolors_glyphs_away_from_invisible_black() {
        use image::GenericImageView;
        let svg = latex_to_svg("E = mc^2", false, "#d0d0d0").expect("renders");
        assert!(
            !svg.contains("rgba(0,0,0,1)"),
            "不可視の純黒フィルが残っていない"
        );
        assert!(svg.contains("#d0d0d0"), "指定色でグリフを塗る");
        // The raster's opaque pixels are the specified light color, not black (0,0,0).
        let img = crate::preview::svg::rasterize_bytes(svg.as_bytes(), Path::new("m.svg"), 1024)
            .expect("rasterizes");
        let opaque_non_black = img
            .pixels()
            .filter(|(_, _, p)| p.0[3] > 200 && (p.0[0] > 40 || p.0[1] > 40 || p.0[2] > 40))
            .count();
        assert!(
            opaque_non_black > 50,
            "端末背景に映える明色インクが実在する (non_black={opaque_non_black})"
        );
    }

    /// The ink's own height, in em (canonical units / 40 — `math_cells`'s convention), read
    /// straight from usvg's bounding box. A second, independent measurement from the one
    /// `normalize_box` itself uses production-side, so a test that compares against it is checking
    /// the *box* against the *ink*, not re-deriving the same number twice.
    fn ink_em_height(svg: &str) -> f64 {
        let opt = usvg::Options {
            fontdb: crate::preview::svg::shared_fontdb(),
            ..Default::default()
        };
        let tree = usvg::Tree::from_str(svg, &opt).expect("normalized SVG must still parse");
        let bbox = tree.root().abs_stroke_bounding_box();
        bbox.height() as f64 / CANONICAL_UNITS_PER_EM
    }

    /// Box height in em, via the same `intrinsic_size_bytes` path `math_cells` reads in production
    /// (so a test failure here is a test failure `math_cells` would also feel).
    fn box_em_height(svg: &str) -> f64 {
        let (_, h) =
            crate::preview::svg::intrinsic_size_bytes(svg.as_bytes()).expect("intrinsic size");
        h as f64 / CANONICAL_UNITS_PER_EM
    }

    /// Regression for the reported bug: an inline expression whose ink is shorter than one line
    /// (`x`, `H_2O`, …) used to draw at whatever fraction of the row RaTeX's own padded layout box
    /// happened to leave for it (measured on real renders: `x` 96%, `E = mc^2` 68% of the
    /// surrounding text — not equal, so equations in the same document drew at different sizes).
    /// After normalization every such expression's box is the same ~1.2em line height, so they all
    /// draw at the same, full text size.
    #[test]
    fn small_ink_clamps_to_one_line_height_regardless_of_how_little_ink_there_is() {
        for latex in ["x", "H_2O", "E = mc^2", "\\alpha"] {
            let raw = render_raw(latex, false, "#d0d0d0").expect("renders");
            let ink_em = ink_em_height(&raw);
            assert!(
                ink_em < LINE_HEIGHT_EM,
                "test fixture assumption: {latex}'s ink ({ink_em:.3}em) must be under one line \
                 for this test to be exercising the clamp branch"
            );
            let normalized = latex_to_svg(latex, false, "#d0d0d0").expect("renders");
            let box_em = box_em_height(&normalized);
            assert!(
                (LINE_HEIGHT_EM..LINE_HEIGHT_EM + 0.1).contains(&box_em),
                "{latex}: box should sit at ~one line height ({LINE_HEIGHT_EM}em), not the ink's \
                 own ({ink_em:.3}em) or RaTeX's original padded box: got {box_em:.3}em"
            );
        }
    }

    /// The other side of the rule: an expression whose ink already exceeds one line
    /// (`\frac`, `\sum`, a tall radical) can't be squeezed to 1.2em without literally cropping it,
    /// so the box ties to the ink instead — tighter than RaTeX's own box, but not shrunk below the
    /// content itself.
    #[test]
    fn tall_ink_ties_the_box_to_itself_instead_of_ratexs_own_padding() {
        for latex in ["\\frac{a}{b}", "\\sum_{i=0}^{n} i", "\\int_0^1 x dx"] {
            let raw = render_raw(latex, false, "#d0d0d0").expect("renders");
            let raw_box_em = box_em_height(&raw);
            let ink_em = ink_em_height(&raw);
            assert!(
                ink_em > LINE_HEIGHT_EM,
                "test fixture assumption: {latex}'s ink ({ink_em:.3}em) must exceed one line for \
                 this test to be exercising the ink-tied branch"
            );
            let normalized = latex_to_svg(latex, false, "#d0d0d0").expect("renders");
            let box_em = box_em_height(&normalized);
            assert!(
                box_em > LINE_HEIGHT_EM,
                "{latex}: box ({box_em:.3}em) should stay above one line — the content genuinely \
                 doesn't fit in 1.2em"
            );
            assert!(
                (box_em - ink_em).abs() < 0.15,
                "{latex}: box ({box_em:.3}em) should sit close to the ink's own height \
                 ({ink_em:.3}em), not RaTeX's padded layout box"
            );
            assert!(
                box_em < raw_box_em - 0.1,
                "{latex}: normalized box ({box_em:.3}em) should be meaningfully tighter than \
                 RaTeX's own padded box ({raw_box_em:.3}em)"
            );
        }
    }

    /// Whichever branch the height rule takes, the ink itself must never be cropped — a hairline
    /// of padding always separates it from the raster's edge.
    #[test]
    fn normalization_never_crops_the_ink() {
        use image::GenericImageView;
        for latex in [
            "x",
            "E = mc^2",
            "H_2O",
            "\\frac{a}{b}",
            "\\sum_{i=0}^{n} i",
            "\\int_0^1 x dx",
            "\\sqrt{2}",
            "\\alpha",
            "\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}",
        ] {
            let svg = latex_to_svg(latex, false, "#d0d0d0").expect("renders");
            let img = crate::preview::svg::rasterize_bytes(svg.as_bytes(), Path::new("m.svg"), 800)
                .expect("rasterizes");
            let (w, h) = img.dimensions();
            let mut min_x = w;
            let mut min_y = h;
            let mut max_x = 0i64;
            let mut max_y = 0i64;
            let mut any_ink = false;
            for (x, y, p) in img.pixels() {
                if p.0[3] > 16 {
                    any_ink = true;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x as i64);
                    max_y = max_y.max(y as i64);
                }
            }
            assert!(any_ink, "{latex}: raster must carry visible ink");
            assert!(
                min_x > 0 && min_y > 0 && (max_x as u32) < w - 1 && (max_y as u32) < h - 1,
                "{latex}: ink must not touch the raster's edge (clipped): \
                 bbox=({min_x},{min_y})-({max_x},{max_y}) of {w}x{h}"
            );
        }
    }

    /// The horizontal whitespace RaTeX's layout box carries is redundant with the ordinary
    /// Markdown space around `$…$`; normalization tightens it away.
    #[test]
    fn normalization_tightens_the_horizontal_margin_too() {
        let raw = render_raw("x", false, "#d0d0d0").expect("renders");
        let (raw_w, _) = crate::preview::svg::intrinsic_size_bytes(raw.as_bytes()).unwrap();
        let normalized = latex_to_svg("x", false, "#d0d0d0").expect("renders");
        let (norm_w, _) = crate::preview::svg::intrinsic_size_bytes(normalized.as_bytes()).unwrap();
        assert!(
            norm_w < raw_w,
            "normalized width ({norm_w}) should be tighter than RaTeX's own ({raw_w})"
        );
    }

    /// `normalize_box` is inline-only (see its own doc comment's "Display math is untouched"
    /// section for the math behind why): a display expression's SVG must come out of
    /// `latex_to_svg` byte-for-byte identical to what `render_raw` produced, with the sole
    /// exception of `recolor`'s black→`color` swap (which `render_raw` already applies).
    #[test]
    fn display_math_box_is_left_exactly_as_ratex_laid_it_out() {
        for latex in ["\\sum_{i=0}^{n} i", "x", "\\frac{a}{b}"] {
            let raw = render_raw(latex, true, "#d0d0d0").expect("renders");
            let normalized = latex_to_svg(latex, true, "#d0d0d0").expect("renders");
            assert_eq!(
                raw, normalized,
                "{latex}: display math must not be touched by box normalization"
            );
        }
    }
}
