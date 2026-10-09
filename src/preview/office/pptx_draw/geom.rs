//! Shape geometry: `a:prstGeom` (preset shapes) and `a:custGeom` (custom paths).
//!
//! Both are provisional: the preset engine (`slide_draw::geom`, which evaluates the guide
//! formulas of `presetShapeDefinitions.xml`) is another task and replaces the bodies of
//! [`preset_geometry`] and [`custom_geometry`]; the callers do not change.

use crate::preview::office::slide_draw as sd;

use super::*;

/// Most adjust values (`a:gd`) read of one preset.
const MAX_ADJ: usize = 32;

/// The geometry of a preset shape `name` (`a:prstGeom@prst`) in a box of `w` x `h` EMU with the
/// adjust values `adj` (`a:gd name="adj1" fmla="val 25000"` -> `("adj1", 25000.0)`), and the text
/// rectangle the preset defines (`None`: the whole box).
///
/// PROVISIONAL. Today: `rect` -> [`sd::Geometry::Rect`], `ellipse` -> [`sd::Geometry::Ellipse`],
/// `line` and `straightConnector1` -> [`sd::Geometry::Line`], everything else -> `Rect`. The preset
/// engine (`slide_draw::geom`) replaces this body so that every preset shape gets its real outline
/// and text rectangle.
pub(super) fn preset_geometry(
    name: &str,
    adj: &[(String, f64)],
    w: f64,
    h: f64,
) -> (sd::Geometry, Option<sd::Rect4>) {
    let _ = (adj, w, h);
    let g = match name {
        "ellipse" => sd::Geometry::Ellipse,
        "line" | "straightConnector1" => sd::Geometry::Line,
        _ => sd::Geometry::Rect,
    };
    (g, None)
}

/// The geometry of an `a:custGeom` element and its text rectangle (`a:rect`).
///
/// PROVISIONAL. Today: always [`sd::Geometry::Rect`]. The preset engine (`slide_draw::geom`) will
/// evaluate the guide list (`a:gdLst`), the connection sites and the paths (`a:pathLst`) of the
/// element and replace this body.
pub(super) fn custom_geometry(_cust: &Node) -> (sd::Geometry, Option<sd::Rect4>) {
    (sd::Geometry::Rect, None)
}

/// The adjust values of an `a:avLst`: only `val N` formulas (the others are guides of the preset
/// engine's own).
pub(super) fn adjust_values(av: &Node) -> Vec<(String, f64)> {
    av.nodes()
        .filter(|g| g.name == "gd")
        .filter_map(|g| {
            let name = g.attr("name")?.to_string();
            let v = g.attr("fmla")?.trim().strip_prefix("val")?.trim();
            Some((name, v.parse::<f64>().ok().filter(|v| v.is_finite())?))
        })
        .take(MAX_ADJ)
        .collect()
}

/// The geometry a shape's `spPr` (or a placeholder's) asks for, in a box of `w` x `h` EMU: `None`
/// when it names none.
pub(super) fn geometry_of(
    sp_pr: &Node,
    w: f64,
    h: f64,
) -> Option<(sd::Geometry, Option<sd::Rect4>)> {
    if let Some(p) = sp_pr.child("prstGeom") {
        let adj = p.child("avLst").map(adjust_values).unwrap_or_default();
        return Some(preset_geometry(
            p.attr("prst").unwrap_or("rect"),
            &adj,
            w,
            h,
        ));
    }
    sp_pr.child("custGeom").map(custom_geometry)
}
