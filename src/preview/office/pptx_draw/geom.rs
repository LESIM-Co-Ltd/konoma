//! Shape geometry: `a:prstGeom` (preset shapes) and `a:custGeom` (custom paths). Both go through
//! the guide-formula engine of `slide_draw::geom`.

use crate::preview::office::slide_draw as sd;

use super::*;

/// Most adjust values (`a:gd`) read of one preset.
const MAX_ADJ: usize = 32;

/// The geometry of a preset shape `name` (`a:prstGeom@prst`) in a box of `w` x `h` EMU with the
/// adjust values `adj` (`a:gd name="adj1" fmla="val 25000"` -> `("adj1", 25000.0)`), and the text
/// rectangle the preset defines (`None`: the whole box).
///
/// The guide formulas are evaluated by `slide_draw::geom::preset`. Three presets keep the cheap
/// exact forms of the renderer: `rect` (the box), `ellipse` (the inscribed ellipse, with the
/// preset's own text rectangle) and `line` / `straightConnector1` (a diagonal). A name that is not
/// a preset draws as the box.
pub(super) fn preset_geometry(
    name: &str,
    adj: &[(String, f64)],
    w: f64,
    h: f64,
) -> (sd::Geometry, Option<sd::Rect4>) {
    let ev = sd::geom::preset(name, adj, w, h);
    let text_rect = ev.as_ref().and_then(|g| g.text_rect);
    match name {
        "rect" => return (sd::Geometry::Rect, text_rect),
        "ellipse" => return (sd::Geometry::Ellipse, text_rect),
        "line" | "straightConnector1" => return (sd::Geometry::Line, None),
        _ => {}
    }
    match ev {
        Some(g) => (sd::Geometry::Paths(g.paths), g.text_rect),
        None => (sd::Geometry::Rect, None),
    }
}

/// The geometry of an `a:custGeom` element and its text rectangle (`a:rect`): its guide list, paths
/// and text rectangle evaluated for the box of `w` x `h` EMU. One that cannot be read (too many
/// guides) is the box.
pub(super) fn custom_geometry(cust: &Node, w: f64, h: f64) -> (sd::Geometry, Option<sd::Rect4>) {
    let g = sd::geom::custom_from_xml(cust).and_then(|spec| sd::geom::custom(&spec, w, h));
    match g {
        Some(g) => (sd::Geometry::Paths(g.paths), g.text_rect),
        None => (sd::Geometry::Rect, None),
    }
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
    sp_pr.child("custGeom").map(|c| custom_geometry(c, w, h))
}
