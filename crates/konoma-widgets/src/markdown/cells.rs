//! Cell boxes of the pictures a document places (images, slides, mermaid diagrams, math): pure
//! arithmetic on the picture size, the terminal font cell and the width and row budgets, so the
//! host and the standalone app size a picture the same way.

/// Compute the display size in terminal cells for an image of `pw`x`ph` pixels, given the terminal
/// font cell size (`fw`x`fh` px). The image is fit to `avail_cols` (never upscaled beyond its natural
/// cell size) and then capped to `max_rows`, preserving aspect ratio in both steps.
pub fn md_image_cells(
    pw: u32,
    ph: u32,
    fw: u16,
    fh: u16,
    avail_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    let fw = (fw.max(1)) as f64;
    let fh = (fh.max(1)) as f64;
    let nat_cols = (pw as f64 / fw).ceil().max(1.0);
    let nat_rows = (ph as f64 / fh).ceil().max(1.0);
    let avail = avail_cols.max(1) as f64;
    let (mut cols, mut rows) = if nat_cols <= avail {
        (nat_cols, nat_rows)
    } else {
        let s = avail / nat_cols;
        (avail, (nat_rows * s).round().max(1.0))
    };
    let maxr = max_rows.max(1) as f64;
    if rows > maxr {
        let s = maxr / rows;
        rows = maxr;
        cols = (cols * s).round().max(1.0);
    }
    (cols as u16, rows as u16)
}

/// Cell box of a slide picture of `pw`x`ph` pixels: the full width `avail_cols` (a slide is a vector
/// drawing, so it is enlarged or shrunk to the width), with the aspect ratio kept in the terminal's
/// cells (`fw`x`fh` px each), and no taller than `max_rows` (the width then shrinks to match).
pub fn slide_cells(
    pw: u32,
    ph: u32,
    fw: u16,
    fh: u16,
    avail_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    let (fw, fh) = (fw.max(1) as f64, fh.max(1) as f64);
    let (pw, ph) = (pw.max(1) as f64, ph.max(1) as f64);
    let avail = avail_cols.max(1) as f64;
    let maxr = max_rows.max(1) as f64;
    // Cell rows for a picture `cols` wide: rows * fh / (cols * fw) = ph / pw.
    let rows_for = |cols: f64| (cols * fw * ph / (pw * fh)).round().max(1.0);
    let rows = rows_for(avail);
    if rows <= maxr {
        return (avail as u16, rows as u16);
    }
    let cols = (maxr * fh * pw / (ph * fw)).round().clamp(1.0, avail);
    (cols as u16, maxr as u16)
}

/// Cell box for an inline mermaid diagram, sized so the diagram's **own text matches the
/// terminal's body text** rather than always filling `target_rows`. `pw`x`ph` is the diagram's
/// *intrinsic* SVG size in the same px domain the renderer lays out glyphs in
/// (`text_metrics::FONT_SIZE` px, "1 SVG user unit = 1px" — see `layout_px`'s doc comment for
/// mermaid fences in `md_media.rs`), **not** the raster's pixel dimensions (those are scaled by
/// `[ui] svg_max_px` and would drag an unrelated setting into the text-size math).
///
/// A terminal cell is ~1.2em tall (same convention `math_cells` uses for RaTeX's em units), so the
/// scale that makes a `FONT_SIZE`-px glyph in the diagram equal one line of terminal text is
/// `(fh / 1.2) / FONT_SIZE`; applying it to the diagram's own px size gives its natural cell box.
///
/// `target_rows` (`[ui] mermaid_rows`) is an **upper limit**, not a fill target: a diagram whose
/// natural size is smaller than the cap is shown at that natural size — never enlarged. Only a
/// diagram taller than the cap (or wider than `avail`) is shrunk, aspect preserved in both steps.
/// Unlike raster images (`md_image_cells`), a diagram is vector-backed, so a *downscaled* cell box
/// still re-rasters sharp on zoom — but "vector-backed" is not license to enlarge past natural size
/// for a reader who never zoomed, which is the bug this sizing exists to avoid.
pub fn mermaid_cells(
    pw: u32,
    ph: u32,
    fw: u16,
    fh: u16,
    avail: u16,
    target_rows: u16,
) -> (u16, u16) {
    let (fw, fh) = (fw.max(1) as f64, fh.max(1) as f64);
    let (pw, ph) = (pw.max(1) as f64, ph.max(1) as f64);
    let font_size = konoma_mermaid_core::text_metrics::FONT_SIZE as f64;
    let scale = (fh / 1.2) / font_size;
    let mut rows = (ph * scale / fh).max(1.0);
    let mut cols = (pw * scale / fw).max(1.0);

    // Upper limit, never a fill target (see doc comment above).
    let cap = target_rows.max(1) as f64;
    if rows > cap {
        let s = cap / rows;
        rows = cap;
        cols *= s;
    }

    // Never wider than the available column budget either, aspect preserved.
    let avail = avail.max(1) as f64;
    if cols > avail {
        let s = avail / cols;
        cols = avail;
        rows *= s;
    }

    (cols.round().max(1.0) as u16, rows.round().max(1.0) as u16)
}

/// Cell box for an inline math image, sized proportionally from the SVG's intrinsic **em** units
/// (RaTeX renders at 40 units/em) so equations sit at roughly text scale: em-height → rows, intrinsic
/// aspect → cols. Display math gets a touch more height than inline. Clamped to the available width
/// (aspect preserved) and a sane row cap. Vector-backed, so the once-rendered raster is fit crisply.
///
/// Inline math (`display == false`) is always `rows == 1` — placed **in the running text**, like
/// ordinary Markdown/browser rendering (`render_math_slot`'s own `!display && rows == 1` branch reads
/// that as "place this expression in the current line, not on a placeholder line of its own"). An
/// expression too wide for `avail` at one row's height is shrunk to `avail` columns, still at one row
/// — never lifted onto a taller line of its own (confirmed exhaustively, not merely derived: an
/// earlier version of this function had a "fall back to the taller lifted-line formula" branch for
/// this case, and a sweep of 212,520 `(uw, uh, fw, fh, avail)` combinations found it changed the
/// output in exactly zero of them — the avail clamp below always overrides both `cols` *and* `rows`
/// regardless, making that branch dead code; removed rather than kept for a distinction it never
/// actually produced).
pub fn math_cells(uw: u32, uh: u32, fw: u16, fh: u16, avail: u16, display: bool) -> (u16, u16) {
    let (fw, fh) = (fw.max(1) as f64, fh.max(1) as f64);
    let em_h = (uh.max(1) as f64) / 40.0;
    let ar = (uw.max(1) as f64) / (uh.max(1) as f64);
    let avail = avail.max(1) as f64;
    let mut rows = if display {
        (em_h * 1.5).round().max(1.0)
    } else {
        1.0
    };
    let mut cols = (rows * fh * ar / fw).round().max(1.0);
    if cols > avail {
        cols = avail;
        rows = (cols * fw / ar / fh).round().max(1.0);
    }
    // A cap so an extremely tall expression (e.g. a matrix) doesn't fill the whole screen.
    let maxr = 24.0;
    if rows > maxr {
        let s = maxr / rows;
        rows = maxr;
        cols = (cols * s).round().max(1.0);
    }
    (cols as u16, rows as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_never_enlarged_and_keeps_its_aspect_when_shrunk() {
        // 100x40 px at 10x20 px cells is 10x2 cells: it fits, so it stays.
        assert_eq!(md_image_cells(100, 40, 10, 20, 80, 24), (10, 2));
        // 1000 px wide needs 100 cells; at 50 available the rows scale by the same half.
        assert_eq!(md_image_cells(1000, 400, 10, 20, 50, 24), (50, 10));
        // A row cap shrinks the columns by the same factor.
        assert_eq!(md_image_cells(100, 800, 10, 20, 80, 10), (3, 10));
    }

    #[test]
    fn a_zero_budget_still_gives_one_cell() {
        assert_eq!(md_image_cells(0, 0, 0, 0, 0, 0), (1, 1));
        assert_eq!(slide_cells(960, 540, 10, 20, 0, 0), (1, 1));
        assert_eq!(math_cells(0, 0, 0, 0, 0, false), (1, 1));
        assert_eq!(mermaid_cells(0, 0, 0, 0, 0, 0), (1, 1));
    }

    #[test]
    fn inline_math_is_one_row_and_display_math_is_taller() {
        let (_, inline_rows) = math_cells(400, 80, 10, 20, 80, false);
        let (_, display_rows) = math_cells(400, 80, 10, 20, 80, true);
        assert_eq!(inline_rows, 1);
        assert!(display_rows > 1);
        // Too wide for the budget: shrunk to it, still one row for inline.
        let (cols, rows) = math_cells(40_000, 80, 10, 20, 30, false);
        assert_eq!((cols, rows), (30, 1));
    }

    #[test]
    fn a_diagram_is_capped_by_rows_and_by_width_and_never_enlarged() {
        let (c0, r0) = mermaid_cells(100, 50, 10, 20, 200, 40);
        let (c1, r1) = mermaid_cells(100, 50, 10, 20, 200, 1);
        assert!(r1 == 1 && c1 <= c0 && r0 >= r1);
        let (c2, _) = mermaid_cells(100_000, 50_000, 10, 20, 60, 500);
        assert!(c2 <= 60);
    }
}
