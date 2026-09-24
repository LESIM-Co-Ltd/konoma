//! Pure geometry and synthetic-key helpers for the image/PDF/SVG "diff" — old and new versions of a
//! picture shown side by side or stacked (`docs/FEATURE-MEDIA-DIFF.md` §1/§4). No `App`, no I/O: the
//! actual fetch/decode lives in the worker (`app/media_diff.rs`), which calls into here only for the
//! layout math and the `media-diff://` cache-key format — the same split `preview::markdown` (pure)
//! / `app::md_diff` (App-side) already has for the Markdown block-diff.
//!
//! `#![allow(dead_code)]`: this whole module is phase A of `docs/FEATURE-MEDIA-DIFF.md` — nothing in
//! production calls into it yet (phase B wires `layout`/`media_diff_url` into `render_gitdiff`).
//! Every item here is exercised directly by this module's own tests in the meantime, mirroring the
//! precedent in `vcs::jj::base_contents`'s doc comment for the identical situation.
#![allow(dead_code)]

use ratatui::layout::Rect;

/// Which side of a media diff a rect/key describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDiffSide {
    Old,
    New,
}

/// How the two sides are arranged (`docs/FEATURE-MEDIA-DIFF.md` §1's `s` cycle / the planned
/// `[git] media_diff` setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaDiffLayout {
    #[default]
    Auto,
    Side,
    Stack,
}

/// The orientation [`layout`] actually resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDiffOrientation {
    Side,
    Stack,
}

/// One side's pane: a one-row caption at the top, and the box the picture is fit and centered in
/// below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaDiffPane {
    pub caption: Rect,
    pub image: Rect,
}

/// The full result of [`layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaDiffGeometry {
    pub orientation: MediaDiffOrientation,
    pub separator: Rect,
    pub old: MediaDiffPane,
    pub new: MediaDiffPane,
}

/// Lay the old/new panes out inside `inner`
/// (`docs/FEATURE-MEDIA-DIFF.md` §1's "並べ方"/"縮尺は両側で同じ").
///
/// - `old_px`/`new_px`: each side's natural pixel size (`None` = absent/failed — no picture to fit).
/// - `cell_px`: the terminal's font cell size in pixels (width, height) — the unit natural pixel
///   sizes are converted into cells through.
/// - `inner`: the whole diff body area to split between the two panes.
/// - `requested`: `Auto` picks whichever orientation lets the shared scale (below) come out larger,
///   tying to `Side`, and falls back to `Side` when neither side has a picture at all (nothing to
///   compare a scale by); `Side`/`Stack` are used exactly as given.
///
/// Both sides are scaled by the same factor (`docs/FEATURE-MEDIA-DIFF.md` §1: "縮尺は両側で同じ…
/// サイズの変更を見えるまま残すため"), so a half-size new image visibly renders at half the cells of
/// an unchanged old one instead of both being stretched to fill their own pane independently. The
/// shared factor never exceeds 1.0 (never enlarge past natural size) and is the tightest fit among
/// the present sides' own panes.
pub fn layout(
    old_px: Option<(u32, u32)>,
    new_px: Option<(u32, u32)>,
    cell_px: (u32, u32),
    inner: Rect,
    requested: MediaDiffLayout,
) -> MediaDiffGeometry {
    let orientation = match requested {
        MediaDiffLayout::Side => MediaDiffOrientation::Side,
        MediaDiffLayout::Stack => MediaDiffOrientation::Stack,
        MediaDiffLayout::Auto => resolve_auto(old_px, new_px, cell_px, inner),
    };
    build_geometry(old_px, new_px, cell_px, inner, orientation)
}

/// `Auto`'s own orientation pick: build both candidate geometries and compare the shared scale each
/// would produce. A strict `>` (not `>=`) for `Stack` winning is what implements the "同点なら左右"
/// tie-break — `Side` is also what a totally degenerate `inner` (0-width or 0-height either way)
/// falls back to, since both scales come out `0.0` in that case.
fn resolve_auto(
    old_px: Option<(u32, u32)>,
    new_px: Option<(u32, u32)>,
    cell_px: (u32, u32),
    inner: Rect,
) -> MediaDiffOrientation {
    if old_px.is_none() && new_px.is_none() {
        return MediaDiffOrientation::Side; // nothing to compare a scale by at all.
    }
    let side = build_geometry(old_px, new_px, cell_px, inner, MediaDiffOrientation::Side);
    let stack = build_geometry(old_px, new_px, cell_px, inner, MediaDiffOrientation::Stack);
    let side_scale = shared_scale(side.old.image, side.new.image, cell_px, old_px, new_px);
    let stack_scale = shared_scale(stack.old.image, stack.new.image, cell_px, old_px, new_px);
    if stack_scale > side_scale {
        MediaDiffOrientation::Stack
    } else {
        MediaDiffOrientation::Side
    }
}

fn build_geometry(
    old_px: Option<(u32, u32)>,
    new_px: Option<(u32, u32)>,
    cell_px: (u32, u32),
    inner: Rect,
    orientation: MediaDiffOrientation,
) -> MediaDiffGeometry {
    let (old_pane_rect, separator, new_pane_rect) = split(inner, orientation);
    let old_image_area = image_area(old_pane_rect);
    let new_image_area = image_area(new_pane_rect);
    let scale = shared_scale(old_image_area, new_image_area, cell_px, old_px, new_px);
    let old = MediaDiffPane {
        caption: caption_row(old_pane_rect),
        image: fit_image(old_image_area, cell_px, old_px, scale),
    };
    let new = MediaDiffPane {
        caption: caption_row(new_pane_rect),
        image: fit_image(new_image_area, cell_px, new_px, scale),
    };
    MediaDiffGeometry {
        orientation,
        separator,
        old,
        new,
    }
}

/// Split `inner` into two panes with a 1-cell separator between them (a column for `Side`, a row for
/// `Stack`) — omitted (0-width/height) when `inner` is too small to fit one alongside two non-empty
/// panes (`docs/FEATURE-MEDIA-DIFF.md` §7's "極小の領域"). The two panes always partition `inner`
/// exactly (their widths/heights plus the separator's sum to `inner`'s), so nothing here can ever
/// produce a rect outside it.
fn split(inner: Rect, orientation: MediaDiffOrientation) -> (Rect, Rect, Rect) {
    match orientation {
        MediaDiffOrientation::Side => {
            let avail = inner.width;
            let sep = if avail >= 3 { 1 } else { 0 };
            let rem = avail - sep;
            let old_w = rem / 2;
            let new_w = rem - old_w;
            let old = Rect {
                x: inner.x,
                y: inner.y,
                width: old_w,
                height: inner.height,
            };
            let separator = Rect {
                x: inner.x + old_w,
                y: inner.y,
                width: sep,
                height: inner.height,
            };
            let new = Rect {
                x: inner.x + old_w + sep,
                y: inner.y,
                width: new_w,
                height: inner.height,
            };
            (old, separator, new)
        }
        MediaDiffOrientation::Stack => {
            let avail = inner.height;
            let sep = if avail >= 3 { 1 } else { 0 };
            let rem = avail - sep;
            let old_h = rem / 2;
            let new_h = rem - old_h;
            let old = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: old_h,
            };
            let separator = Rect {
                x: inner.x,
                y: inner.y + old_h,
                width: inner.width,
                height: sep,
            };
            let new = Rect {
                x: inner.x,
                y: inner.y + old_h + sep,
                width: inner.width,
                height: new_h,
            };
            (old, separator, new)
        }
    }
}

/// The top row of a pane, reserved for its caption (`docs/FEATURE-MEDIA-DIFF.md` §1: "各側の上1行が
/// 見出し"). Zero-height when the pane itself has no rows.
fn caption_row(pane: Rect) -> Rect {
    Rect {
        x: pane.x,
        y: pane.y,
        width: pane.width,
        height: pane.height.min(1),
    }
}

/// The rest of a pane below its caption row — where the picture is fit and centered.
fn image_area(pane: Rect) -> Rect {
    let caption_h = pane.height.min(1);
    Rect {
        x: pane.x,
        y: pane.y + caption_h,
        width: pane.width,
        height: pane.height - caption_h,
    }
}

/// `min(1.0, min over present sides of min(area_w/nat_w, area_h/nat_h))` — see [`layout`]'s doc
/// comment. `*_area` are each side's own image area (which can differ by a cell between the two
/// panes when an odd `inner` splits unevenly), converted to px via `cell_px`.
fn shared_scale(
    old_area: Rect,
    new_area: Rect,
    cell_px: (u32, u32),
    old_px: Option<(u32, u32)>,
    new_px: Option<(u32, u32)>,
) -> f64 {
    let mut s = 1.0_f64;
    if let Some(nat) = old_px {
        s = s.min(fit_ratio(old_area, cell_px, nat));
    }
    if let Some(nat) = new_px {
        s = s.min(fit_ratio(new_area, cell_px, nat));
    }
    s
}

/// How much `nat` (px) would need to shrink (or could grow, uncapped here — the caller clamps to
/// 1.0) to fit inside `area` (cells, converted to px via `cell_px`). `nat`'s own components are
/// floored at 1 so a degenerate 0-sized natural size can never divide by zero.
fn fit_ratio(area: Rect, cell_px: (u32, u32), nat: (u32, u32)) -> f64 {
    let area_w_px = area.width as f64 * cell_px.0.max(1) as f64;
    let area_h_px = area.height as f64 * cell_px.1.max(1) as f64;
    let nat_w = nat.0.max(1) as f64;
    let nat_h = nat.1.max(1) as f64;
    (area_w_px / nat_w).min(area_h_px / nat_h)
}

/// The picture's target rect within `area`, at the shared `scale`, centered. `None` (no picture, or
/// a zero-size `area`) becomes a zero-size rect anchored at `area`'s own origin — still inside
/// `area`, so this never panics or produces a rect outside `inner`. At least 1×1 whenever a picture
/// is present and `area` itself has room for it.
fn fit_image(area: Rect, cell_px: (u32, u32), nat: Option<(u32, u32)>, scale: f64) -> Rect {
    let (Some((nw, nh)), true) = (nat, area.width > 0 && area.height > 0) else {
        return Rect {
            x: area.x,
            y: area.y,
            width: 0,
            height: 0,
        };
    };
    let disp_w_px = nw as f64 * scale;
    let disp_h_px = nh as f64 * scale;
    let cw = ((disp_w_px / cell_px.0.max(1) as f64).round() as u16)
        .max(1)
        .min(area.width);
    let ch = ((disp_h_px / cell_px.1.max(1) as f64).round() as u16)
        .max(1)
        .min(area.height);
    Rect {
        x: area.x + (area.width - cw) / 2,
        y: area.y + (area.height - ch) / 2,
        width: cw,
        height: ch,
    }
}

/// Synthetic `md_image_cache` key for one side of a media diff — the same shape
/// `preview::markdown::math_url` uses (a scheme prefix plus an FNV-1a 64 hash formatted as hex), but
/// keyed on a caller-supplied hash of the **already-read bytes** (see [`fnv1a64`]) plus the side, the
/// PDF page, and — for a vector source (SVG/PDF) whose raster depends on the box it's fit into — the
/// raster target, so two different on-screen sizes of the identical bytes don't collide on one cached
/// raster. `raster_px` is `None` for a raw raster image (PNG/JPG/GIF), whose decode doesn't depend on
/// the target box at all.
pub fn media_diff_url(
    side: MediaDiffSide,
    bytes_hash: u64,
    page: u32,
    raster_px: Option<(u32, u32)>,
) -> String {
    let s = match side {
        MediaDiffSide::Old => 'o',
        MediaDiffSide::New => 'n',
    };
    match raster_px {
        Some((w, h)) => format!("media-diff://{s}/{bytes_hash:016x}/{page}/{w}x{h}"),
        None => format!("media-diff://{s}/{bytes_hash:016x}/{page}"),
    }
}

/// Whether an inline-image URL is a synthetic media-diff key (never a real file on disk) — the
/// `media-diff://` counterpart of `preview::markdown::is_mermaid_fence_url`/`is_math_url`.
/// `preview::markdown::is_synthetic_md_url` includes this.
pub fn is_media_diff_url(url: &str) -> bool {
    url.starts_with("media-diff://")
}

/// FNV-1a 64, byte-for-byte the same constants `preview::markdown::math_url` hashes its input with
/// (see that function's own doc comment) — the media-diff worker (`app/media_diff.rs`) hashes each
/// side's raw bytes with this to build a content-addressed [`media_diff_url`] key: an edited file
/// therefore gets a fresh key, and an unchanged one reuses its cached raster across redraws.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    // ---- orientation ----

    #[test]
    fn auto_picks_stack_for_a_wide_image() {
        let inner = rect(0, 0, 80, 40);
        let cell_px = (8, 16);
        let g = layout(
            Some((2000, 200)),
            Some((2000, 200)),
            cell_px,
            inner,
            MediaDiffLayout::Auto,
        );
        assert_eq!(g.orientation, MediaDiffOrientation::Stack);
    }

    #[test]
    fn auto_picks_side_for_a_tall_image() {
        let inner = rect(0, 0, 80, 40);
        let cell_px = (8, 16);
        let g = layout(
            Some((200, 2000)),
            Some((200, 2000)),
            cell_px,
            inner,
            MediaDiffLayout::Auto,
        );
        assert_eq!(g.orientation, MediaDiffOrientation::Side);
    }

    #[test]
    fn auto_ties_to_side() {
        // A perfectly square inner + square images: both orientations must produce the identical
        // scale (each pane is exactly half of a square split either way) — the tie-break is `Side`.
        let inner = rect(0, 0, 80, 80);
        let cell_px = (1, 1); // square cells so the geometry is exactly symmetric under transpose.
        let g = layout(
            Some((100, 100)),
            Some((100, 100)),
            cell_px,
            inner,
            MediaDiffLayout::Auto,
        );
        assert_eq!(g.orientation, MediaDiffOrientation::Side);
    }

    #[test]
    fn auto_with_no_pictures_at_all_is_side() {
        let inner = rect(0, 0, 80, 40);
        let g = layout(None, None, (8, 16), inner, MediaDiffLayout::Auto);
        assert_eq!(g.orientation, MediaDiffOrientation::Side);
    }

    #[test]
    fn requested_side_and_stack_are_honored_regardless_of_fit() {
        // Even when a wide image would make Auto pick Stack, an explicit request is honored as-is.
        let inner = rect(0, 0, 80, 40);
        let cell_px = (8, 16);
        let g = layout(
            Some((2000, 200)),
            Some((2000, 200)),
            cell_px,
            inner,
            MediaDiffLayout::Side,
        );
        assert_eq!(g.orientation, MediaDiffOrientation::Side);
        let g = layout(
            Some((200, 2000)),
            Some((200, 2000)),
            cell_px,
            inner,
            MediaDiffLayout::Stack,
        );
        assert_eq!(g.orientation, MediaDiffOrientation::Stack);
    }

    // ---- mutation-proving: a no-op orientation pick must fail this ----
    #[test]
    fn auto_orientation_actually_differs_from_always_side() {
        // If `resolve_auto` degenerated into "always Side" (a no-op), this specific wide-image case
        // (which auto_picks_stack_for_a_wide_image above pins as Stack) would silently start
        // matching a hypothetical "always Side" implementation instead. Kept as its own test (not
        // just relying on the assertion above) precisely so a regression here reads as "the auto
        // pick itself broke", not "some unrelated wide-image expectation broke".
        let inner = rect(0, 0, 80, 40);
        let cell_px = (8, 16);
        let auto = layout(
            Some((2000, 200)),
            Some((2000, 200)),
            cell_px,
            inner,
            MediaDiffLayout::Auto,
        );
        let forced_side = layout(
            Some((2000, 200)),
            Some((2000, 200)),
            cell_px,
            inner,
            MediaDiffLayout::Side,
        );
        assert_ne!(auto.orientation, forced_side.orientation);
    }

    // ---- shared scale ----

    #[test]
    fn common_scale_is_shared_a_half_size_new_image_renders_at_half_the_cells() {
        let inner = rect(0, 0, 200, 100);
        let cell_px = (8, 16);
        let g = layout(
            Some((400, 200)),
            Some((200, 100)),
            cell_px,
            inner,
            MediaDiffLayout::Side,
        );
        assert_eq!(g.new.image.width, g.old.image.width / 2);
        assert_eq!(g.new.image.height, g.old.image.height / 2);
    }

    #[test]
    fn never_enlarges_a_tiny_icon_stays_natural_size() {
        let inner = rect(0, 0, 200, 100);
        let cell_px = (8, 16);
        // A 8x8px icon in 8x16px cells is under 1 cell either way; the natural size in cells is
        // ceil(8/8)=1 x ceil(8/16)=1 — the whole pane is far bigger, so nothing should upscale it.
        let g = layout(
            Some((8, 8)),
            Some((8, 8)),
            cell_px,
            inner,
            MediaDiffLayout::Side,
        );
        assert_eq!(g.old.image.width, 1);
        assert_eq!(g.old.image.height, 1);
        assert_eq!(g.new.image.width, 1);
        assert_eq!(g.new.image.height, 1);
    }

    // ---- mutation-proving: a no-op scale (always 1.0) must fail this ----
    #[test]
    fn scale_actually_shrinks_when_the_image_does_not_fit() {
        // A wide (non-square) pane, square cells, and a square source: this combination is what
        // discriminates a genuine shared scale from an "always 1.0, just clamp per axis" no-op — the
        // no-op would independently clamp width to 19 and height to 9, producing a *distorted*
        // 19x9 rect that fills the pane; a real shrink-to-fit scales both axes by the same factor
        // (bound by the tighter, height axis) and stays square, narrower than the pane's own width.
        let inner = rect(0, 0, 40, 10);
        let cell_px = (10, 10);
        let g = layout(
            Some((100_000, 100_000)),
            None,
            cell_px,
            inner,
            MediaDiffLayout::Side,
        );
        let pane_area = image_area(split(inner, MediaDiffOrientation::Side).0);
        assert!(g.old.image.width <= pane_area.width);
        assert!(g.old.image.height <= pane_area.height);
        assert!(
            g.old.image.width < pane_area.width,
            "clamp-only の no-op なら幅も領域いっぱい(19)になるはず: got {}",
            g.old.image.width
        );
        // With a genuine shared scale, width and height shrink by the *same* factor, so the aspect
        // ratio (1:1 source, square cells) is preserved.
        assert_eq!(g.old.image.width, g.old.image.height);
    }

    // ---- absence / degeneracy ----

    #[test]
    fn one_side_absent_the_other_still_lays_out() {
        let inner = rect(0, 0, 200, 100);
        let g = layout(
            None,
            Some((400, 200)),
            (8, 16),
            inner,
            MediaDiffLayout::Side,
        );
        assert_eq!(g.old.image.width, 0);
        assert_eq!(g.old.image.height, 0);
        assert!(g.new.image.width > 0 && g.new.image.height > 0);
    }

    #[test]
    fn both_sides_absent_no_panic_and_zero_size_images() {
        let inner = rect(0, 0, 200, 100);
        let g = layout(None, None, (8, 16), inner, MediaDiffLayout::Side);
        assert_eq!(g.old.image.width, 0);
        assert_eq!(g.old.image.height, 0);
        assert_eq!(g.new.image.width, 0);
        assert_eq!(g.new.image.height, 0);
    }

    #[test]
    fn zero_size_inner_no_panic() {
        let inner = rect(5, 5, 0, 0);
        for l in [
            MediaDiffLayout::Auto,
            MediaDiffLayout::Side,
            MediaDiffLayout::Stack,
        ] {
            let g = layout(Some((100, 100)), Some((100, 100)), (8, 16), inner, l);
            assert_eq!(g.old.image.width, 0);
            assert_eq!(g.new.image.width, 0);
        }
    }

    #[test]
    fn zero_size_natural_image_no_panic() {
        let inner = rect(0, 0, 40, 20);
        // A pathological 0x0 natural size (should never come from a real decoder, but must not
        // divide by zero / panic if it ever does).
        let g = layout(Some((0, 0)), None, (8, 16), inner, MediaDiffLayout::Side);
        assert!(g.old.image.width >= 1);
        assert!(g.old.image.height >= 1);
    }

    // ---- captions / separator / bounds ----

    #[test]
    fn caption_is_the_top_row_of_each_pane() {
        let inner = rect(0, 0, 80, 40);
        let g = layout(
            Some((100, 100)),
            Some((100, 100)),
            (8, 16),
            inner,
            MediaDiffLayout::Side,
        );
        assert_eq!(g.old.caption.height, 1);
        assert_eq!(g.old.caption.y, inner.y);
        assert_eq!(g.new.caption.height, 1);
        assert_eq!(g.new.caption.y, inner.y);
        // The image *area* (what the picture is centered within, not the picture's own rect — it
        // may be smaller and centered inside it) starts right below the caption.
        let (old_pane, _, new_pane) = split(inner, MediaDiffOrientation::Side);
        let old_area = image_area(old_pane);
        let new_area = image_area(new_pane);
        assert_eq!(old_area.y, g.old.caption.y + 1);
        assert_eq!(new_area.y, g.new.caption.y + 1);
        // The picture itself stays within that area (centered, not necessarily flush with its top).
        assert!(g.old.image.y >= old_area.y);
        assert!(g.old.image.y + g.old.image.height <= old_area.y + old_area.height);
    }

    #[test]
    fn separator_is_one_cell_between_the_panes_for_side() {
        let inner = rect(0, 0, 81, 40);
        let g = layout(
            Some((100, 100)),
            Some((100, 100)),
            (8, 16),
            inner,
            MediaDiffLayout::Side,
        );
        assert_eq!(g.separator.width, 1);
        assert_eq!(g.separator.height, inner.height);
        // Panes + separator exactly cover `inner` with no overlap and no gap.
        let (old_pane, sep, new_pane) = split(inner, MediaDiffOrientation::Side);
        assert_eq!(old_pane.width + sep.width + new_pane.width, inner.width);
        assert_eq!(sep.x, old_pane.x + old_pane.width);
        assert_eq!(new_pane.x, sep.x + sep.width);
    }

    #[test]
    fn separator_is_one_cell_between_the_panes_for_stack() {
        let inner = rect(0, 0, 40, 41);
        let (old_pane, sep, new_pane) = split(inner, MediaDiffOrientation::Stack);
        assert_eq!(sep.height, 1);
        assert_eq!(sep.width, inner.width);
        assert_eq!(old_pane.height + sep.height + new_pane.height, inner.height);
        assert_eq!(sep.y, old_pane.y + old_pane.height);
        assert_eq!(new_pane.y, sep.y + sep.height);
    }

    /// Property-style sweep: for a range of `inner` sizes and images, every rect `layout` produces
    /// stays inside `inner` (§10's "rects never exceed inner").
    #[test]
    fn every_rect_stays_within_inner_across_many_sizes() {
        fn within(outer: Rect, r: Rect) -> bool {
            r.x >= outer.x
                && r.y >= outer.y
                && r.x + r.width <= outer.x + outer.width
                && r.y + r.height <= outer.y + outer.height
        }
        for w in [0u16, 1, 2, 3, 4, 7, 15, 40, 121] {
            for h in [0u16, 1, 2, 3, 4, 7, 15, 40, 121] {
                let inner = rect(2, 3, w, h); // nonzero origin: also proves no absolute-origin bug.
                for old in [None, Some((3u32, 500)), Some((900, 3))] {
                    for new in [None, Some((10u32, 10)), Some((5000, 4000))] {
                        for l in [
                            MediaDiffLayout::Auto,
                            MediaDiffLayout::Side,
                            MediaDiffLayout::Stack,
                        ] {
                            let g = layout(old, new, (8, 16), inner, l);
                            assert!(
                                within(inner, g.separator),
                                "{w}x{h} separator escaped inner"
                            );
                            assert!(within(inner, g.old.caption), "{w}x{h} old caption escaped");
                            assert!(within(inner, g.old.image), "{w}x{h} old image escaped");
                            assert!(within(inner, g.new.caption), "{w}x{h} new caption escaped");
                            assert!(within(inner, g.new.image), "{w}x{h} new image escaped");
                        }
                    }
                }
            }
        }
    }

    // ---- keys ----

    #[test]
    fn media_diff_url_has_the_scheme_and_distinguishes_side_page_and_raster() {
        let h = fnv1a64(b"hello");
        let a = media_diff_url(MediaDiffSide::Old, h, 1, None);
        let b = media_diff_url(MediaDiffSide::New, h, 1, None);
        let c = media_diff_url(MediaDiffSide::Old, h, 2, None);
        let d = media_diff_url(MediaDiffSide::Old, h, 1, Some((100, 200)));
        let e = media_diff_url(MediaDiffSide::Old, h, 1, Some((50, 60)));
        assert!(a.starts_with("media-diff://"));
        // Every one of side/page/raster changing must change the key.
        assert_ne!(a, b, "side must change the key");
        assert_ne!(a, c, "page must change the key");
        assert_ne!(a, d, "adding a raster target must change the key");
        assert_ne!(d, e, "a different raster target must change the key");
        for url in [&a, &b, &c, &d, &e] {
            assert!(is_media_diff_url(url));
        }
    }

    #[test]
    fn media_diff_url_is_content_addressed() {
        let url_a = media_diff_url(MediaDiffSide::Old, fnv1a64(b"content A"), 1, None);
        let url_a2 = media_diff_url(MediaDiffSide::Old, fnv1a64(b"content A"), 1, None);
        let url_b = media_diff_url(MediaDiffSide::Old, fnv1a64(b"content B"), 1, None);
        assert_eq!(url_a, url_a2, "identical bytes reuse the same key");
        assert_ne!(url_b, url_a, "different bytes get a different key");
    }

    #[test]
    fn is_media_diff_url_rejects_other_synthetic_and_real_urls() {
        assert!(!is_media_diff_url("figure.png"));
        assert!(!is_media_diff_url("math://i0123456789abcdef"));
        assert!(!is_media_diff_url("mermaid-fence://0123456789abcdef"));
    }

    #[test]
    fn fnv1a64_matches_math_urls_own_hash() {
        // `math_url` formats as "math://{d|i}{hash:016x}" — the 16 trailing hex chars are the exact
        // same FNV-1a 64 this module claims to share the constants with.
        let latex = "E = mc^2";
        let url = crate::preview::markdown::math_url(latex, false);
        let expected_hex = &url["math://i".len()..];
        assert_eq!(format!("{:016x}", fnv1a64(latex.as_bytes())), expected_hex);
    }
}
