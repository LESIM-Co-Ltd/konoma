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

/// Render a LaTeX *math* expression (the content between the `$`/`$$` delimiters, without them) to a
/// standalone SVG document string. `display` selects TeX display style (block `$$…$$`) vs text style
/// (inline `$…$`), matching KaTeX. Returns `None` when the expression is empty or RaTeX cannot
/// parse/lay out/render it — the caller then falls back to the raw source. Panic-safe for the caller
/// via `catch_silent` at the worker boundary (RaTeX is pre-1.0).
pub fn latex_to_svg(latex: &str, display: bool) -> Option<String> {
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
    // A well-formed SVG always carries an `<svg` root; guard against an empty/degenerate string so a
    // downstream rasterize never sees garbage.
    svg.contains("<svg").then_some(svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn renders_simple_math_to_rasterizable_svg() {
        // The whole point: LaTeX → standalone SVG → konoma's resvg rasterizes it (no external fonts).
        for (latex, display) in [("x^2 + y^2 = z^2", true), ("\\frac{a}{b}", false)] {
            let svg = latex_to_svg(latex, display).expect("RaTeX renders basic math");
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
        assert!(latex_to_svg("", true).is_none());
        assert!(latex_to_svg("   ", false).is_none());
        // Nonsense/incomplete LaTeX must degrade to None (→ raw-text fallback), never panic.
        let _ = latex_to_svg("\\frac{", true);
        let _ = latex_to_svg("\\undefinedcmd{x}", false);
    }

    #[test]
    fn display_and_inline_styles_both_render() {
        // Same expression, both styles produce a valid SVG (display taller than inline typically).
        let d = latex_to_svg("\\sum_{i=0}^{n} i", true).expect("display");
        let i = latex_to_svg("\\sum_{i=0}^{n} i", false).expect("inline");
        assert!(d.contains("<svg") && i.contains("<svg"));
    }
}
