//! Graphic frames that hold more than a picture: charts and SmartArt.
//!
//! * **Charts** (`a:graphicData` with a `c:chart r:id`): the chart part is read through the deck
//!   reader's budgeted part reading, parsed by `chart_xml` with the slide's theme (colour elements
//!   go through the slide's colour map, the palette is `accent1` .. `accent6`, the fonts are the
//!   theme's), lowered to shapes by `slide_draw::chart` and placed in a group at the frame's box
//!   (child space `(0, 0, w, h)`). The chart part's own `c:spPr` / `c:txPr` are read by
//!   `chart_xml`. A graphic frame is never rotated or flipped by PowerPoint, so only the frame's
//!   position and size are used. A chart that does not parse (including a `cx:` chartex part that
//!   is not wrapped in `mc:AlternateContent`) draws nothing; that is not a truncation.
//!   Chartex charts (waterfall, treemap, ...) normally sit in `mc:AlternateContent` whose
//!   `mc:Choice` requires `cx1`: the alternate-content rule picks the `mc:Fallback` (a picture),
//!   because `cx*` is not among the namespaces the reader understands.
//! * **SmartArt** (`dgm:relIds`): PowerPoint saves the drawn shapes in a diagram drawing part
//!   (`dsp:drawing`), named by `dsp:dataModelExt relId` in the data model part (`r:dm`) and a
//!   relationship of the slide of type `diagramDrawing`. Its shapes are DrawingML shapes in the
//!   frame's own space; they go through the ordinary shape builder (`dsp:txXfrm` gives the text
//!   box). A diagram whose file has no drawing part (LibreOffice writes none) draws nothing; the
//!   text view still has its text. Pictures inside a drawing refer to the drawing part's own
//!   relationships, which are mapped to the builder through `Sb::extra_rels`.
//! * Embedded objects keep the ordinary path in `shapes.rs` (the picture of what they contain).

use std::collections::HashMap;

use crate::preview::office::chart_xml::{parse_chart_with, ChartEnv};
use crate::preview::office::slide_draw as sd;
use crate::preview::office::slide_draw::chart::draw_chart;
use crate::preview::office::slide_draw::Rgba;

use super::*;

/// Most elements of a SmartArt drawing part that are read (a forged one must not cost unbounded
/// memory); over it the diagram is not drawn and the scene says so.
pub(super) const MAX_DIAGRAM_NODES: usize = 100_000;
/// Most bytes of names, attributes and text of a SmartArt drawing part that are kept.
pub(super) const MAX_DIAGRAM_BYTES: usize = 4 * 1024 * 1024;

/// The text box of a SmartArt shape (`dsp:txXfrm`, in the space of the drawing) as a rectangle in
/// the shape's own box (left, top, right, bottom, EMU, before rotation and flips) and the extra
/// turn of the text (degrees): `txXfrm@rot` is **relative to the shape** (a hexagon turned by 90
/// degrees with `txXfrm rot=-90` has upright text; checked against LibreOffice's drawing of
/// PowerPoint's `smartart-rotated-text.pptx`).
///
/// The text box is placed so that, after the shape's flips and rotation are applied, its centre is
/// where `txXfrm` puts it.
pub(super) fn text_box(shape: &sd::Xfrm, tx: &sd::Xfrm) -> (sd::Rect4, f64) {
    let (scx, scy) = (shape.x + shape.w / 2.0, shape.y + shape.h / 2.0);
    let (dx, dy) = (tx.x + tx.w / 2.0 - scx, tx.y + tx.h / 2.0 - scy);
    // Undo the rotation (clockwise in a y-down plane), then the flips.
    let a = (-shape.rot_deg).to_radians();
    let (sin, cos) = a.sin_cos();
    let (mut lx, mut ly) = (dx * cos - dy * sin, dx * sin + dy * cos);
    if shape.flip_h {
        lx = -lx;
    }
    if shape.flip_v {
        ly = -ly;
    }
    // (In the shape's own box: its centre is `(w / 2, h / 2)`.)
    let (cx, cy) = (shape.w / 2.0 + lx, shape.h / 2.0 + ly);
    let rect = (
        cx - tx.w / 2.0,
        cy - tx.h / 2.0,
        cx + tx.w / 2.0,
        cy + tx.h / 2.0,
    );
    (rect, tx.rot_deg)
}

/// What parsing a whole part gave.
enum Parsed {
    Tree(Node),
    /// Over the node or byte budget.
    TooBig,
    /// Not XML.
    Bad,
}

/// The element tree of a whole part, within the diagram budgets.
fn parse_tree(bytes: &[u8]) -> Parsed {
    let mut budget = Budget::new(MAX_DIAGRAM_NODES, MAX_DIAGRAM_BYTES);
    let mut rd = XmlReader::new(bytes);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => (e.into_owned(), false),
            Ok(Event::Empty(e)) => (e.into_owned(), true),
            Ok(Event::Eof) | Err(_) => return Parsed::Bad,
            Ok(_) => continue,
        };
        return match read_element(&mut rd, &e, empty, &mut budget) {
            Ok(Tree::Ok(n)) => Parsed::Tree(n),
            Ok(Tree::TooBig) => Parsed::TooBig,
            Err(_) => Parsed::Bad,
        };
    }
}

/// The `c:clrMapOvr` element of a chart part, if it has one.
fn chart_clr_ovr(bytes: &[u8]) -> Option<Node> {
    if !bytes.windows(9).any(|w| w == b"clrMapOvr") {
        return None;
    }
    let mut budget = Budget::new(64, 4096);
    let mut rd = XmlReader::new(bytes);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf).ok()? {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => return None,
            _ => continue,
        };
        if e.local_name().as_ref() == b"clrMapOvr" {
            return match read_element(&mut rd, &e, empty, &mut budget) {
                Ok(Tree::Ok(n)) => Some(n),
                _ => None,
            };
        }
    }
}

/// The relationship id (of the slide) of a diagram's drawing part: the `relId` of the
/// `dsp:dataModelExt` element of the data model part.
fn drawing_rel_id(data: &[u8]) -> Option<String> {
    let mut rd = XmlReader::new(data);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).ok()? {
            Event::Start(e) | Event::Empty(e) => {
                if e.local_name().as_ref() == b"dataModelExt" {
                    return attr(&e, b"relId", false).filter(|v| !v.is_empty());
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// Points every picture reference (`r:embed` of an `a:blip` or `asvg:svgBlip`) of a drawing at an
/// id of `extra` that stands for the drawing part's relationship; a reference that resolves to
/// nothing internal is blanked (so it can never resolve through the slide's relationships).
fn remap_blips(
    n: &mut Node,
    rels: &HashMap<String, Rel>,
    extra: &mut HashMap<String, Rel>,
    depth: usize,
) {
    if depth > 64 {
        return;
    }
    if n.name == "blip" || n.name == "svgBlip" {
        for (k, v) in n.attrs.iter_mut() {
            if crate::preview::office::docx_xml::local_of(k) != "embed" {
                continue;
            }
            match rels.get(v.as_str()).filter(|r| !r.external) {
                Some(r) => {
                    let id = format!("dgm:{v}");
                    extra.entry(id.clone()).or_insert_with(|| r.clone());
                    *v = id;
                }
                None => v.clear(),
            }
        }
    }
    for k in &mut n.kids {
        if let Kid::N(c) = k {
            remap_blips(c, rels, extra, depth + 1);
        }
    }
}

fn has_blip(n: &Node, depth: usize) -> bool {
    depth <= 64 && (n.name == "blip" || n.nodes().any(|c| has_blip(c, depth + 1)))
}

impl Sb<'_> {
    /// A chart (`c:chart`) as a group of scene items.
    pub(super) fn frame_chart(&mut self, frame: &Node, xfrm: sd::Xfrm, out: &mut Vec<sd::Item>) {
        let Some(chart) = frame
            .child("graphic")
            .and_then(|g| g.child("graphicData"))
            .and_then(|d| d.child("chart"))
        else {
            return;
        };
        if xfrm.w <= 0.0 || xfrm.h <= 0.0 {
            return;
        }
        let Some(part) = chart
            .rel_attr("id")
            .and_then(|id| self.rels_at(self.cur).get(id))
            .filter(|r| !r.external)
            .map(|r| r.target.clone())
        else {
            return;
        };
        let Some(bytes) = (self.parts.read)(&part) else {
            return;
        };
        // A chart maps its colours by the master's map, not by the slide's (or the layout's)
        // override, unless the chart part says otherwise with a `c:clrMapOvr` (LibreOffice's
        // test for `chart_pt_color_bg1.pptx`, a PowerPoint file: "bg1 is mapped in the slide to
        // dk1, but in the chart to lt1"). `c:clrMapOvr` carries the mapping in its own
        // attributes, as `p:clrMap` does.
        let chart_map = chart_clr_ovr(&bytes)
            .map(|n| ClrMap::from_node(&n))
            .unwrap_or_else(|| self.master_map.clone());
        // Pictures of the chart part resolve through its own relationships.
        let own = (self.parts.rels)(&part);
        // A chart may carry its own theme (`c:themeOverride`, the part its relationships name):
        // its colours and fonts replace the slide's theme for this chart only.
        let theme_part = own
            .values()
            .find(|r| r.kind == "themeOverride" && !r.external)
            .map(|r| r.target.clone());
        let override_theme = theme_part
            .and_then(|t| (self.parts.read)(&t))
            .map(|b| Theme::parse(&b, self.opts, HashMap::new()));
        let col = Colors {
            theme: override_theme.as_ref().unwrap_or(self.col.theme),
            map: &chart_map,
        };
        let palette: Vec<Rgba> = (1..=6)
            .filter_map(|i| col.scheme(&format!("accent{i}"), None))
            .collect();
        let resolve = move |n: &Node| col.elem(n, None);
        let font = |f: &theme::ThemeFonts| (!f.latin.is_empty()).then(|| f.latin.clone());
        let chart_theme = override_theme.as_ref().unwrap_or(self.theme);
        let pick = |own: Option<String>, slide: &theme::ThemeFonts| own.or_else(|| font(slide));
        let (minor, major) = (
            pick(font(&chart_theme.minor), &self.theme.minor),
            pick(font(&chart_theme.major), &self.theme.major),
        );
        let env = ChartEnv {
            resolve_color: &resolve,
            palette: &palette,
            minor_font: minor.as_deref(),
            major_font: major.as_deref(),
        };
        let this = std::cell::RefCell::new(&mut *self);
        let images = |bf: &Node| this.borrow_mut().image_fill(bf, &own);
        let parsed = parse_chart_with(&bytes, &env, Some(&images));
        let Ok(parsed) = parsed else {
            return;
        };
        let (mut items, cut) = draw_chart(&parsed.model, xfrm.w, xfrm.h);
        self.truncated |= cut || parsed.truncated;
        let room = self.max_items.saturating_sub(self.items);
        if items.len() > room {
            items.truncate(room);
            self.truncated = true;
        }
        self.items += items.len();
        if items.is_empty() {
            return;
        }
        out.push(sd::Item::Group(sd::GroupItem {
            xfrm: sd::Xfrm::rect(xfrm.x, xfrm.y, xfrm.w, xfrm.h),
            child_off: (0.0, 0.0),
            child_ext: (xfrm.w, xfrm.h),
            items,
        }));
    }

    /// A SmartArt diagram (`dgm:relIds`) as a group of scene items, from its saved drawing part.
    ///
    /// `depth` is the nesting depth of the frame in its own tree: the drawing's shapes continue
    /// counting from it (never restart), and a diagram frame inside a drawing is refused (a
    /// drawing never holds one; a forged drawing that points back at itself would recurse until
    /// the stack ran out).
    pub(super) fn frame_diagram(
        &mut self,
        frame: &Node,
        xfrm: sd::Xfrm,
        depth: usize,
        out: &mut Vec<sd::Item>,
    ) {
        if self.in_diagram {
            self.truncated = true;
            return;
        }
        if xfrm.w <= 0.0 || xfrm.h <= 0.0 {
            return;
        }
        let Some(ids) = frame
            .child("graphic")
            .and_then(|g| g.child("graphicData"))
            .and_then(|d| d.child("relIds"))
        else {
            return;
        };
        let rels = self.rels_at(self.cur);
        let Some(data_part) = ids
            .rel_attr("dm")
            .and_then(|id| rels.get(id))
            .filter(|r| !r.external)
            .map(|r| r.target.clone())
        else {
            return;
        };
        let Some(data) = (self.parts.read)(&data_part) else {
            return;
        };
        let Some(drawing_part) = drawing_rel_id(&data)
            .and_then(|id| rels.get(&id))
            .filter(|r| !r.external && r.kind == "diagramDrawing")
            .map(|r| r.target.clone())
        else {
            return;
        };
        drop(data);
        let Some(bytes) = (self.parts.read)(&drawing_part) else {
            return;
        };
        let mut root = match parse_tree(&bytes) {
            Parsed::Tree(n) if n.name == "drawing" => n,
            Parsed::TooBig => {
                self.truncated = true;
                return;
            }
            _ => return,
        };
        drop(bytes);
        let Some(tree) = root.kids.iter_mut().find_map(|k| match k {
            Kid::N(n) if n.name == "spTree" => Some(n),
            _ => None,
        }) else {
            return;
        };
        if has_blip(tree, 0) {
            let own = (self.parts.rels)(&drawing_part);
            let mut extra = HashMap::new();
            remap_blips(tree, &own, &mut extra, 0);
            self.extra_rels = extra;
        }
        let mut kids = Vec::new();
        // (The drawing is in the frame's own space: an enclosing group's map does not apply.)
        self.group_maps.push(GroupMap::identity());
        self.in_diagram = true;
        self.build_nodes(tree.nodes(), depth + 1, &mut kids);
        self.in_diagram = false;
        self.group_maps.pop();
        self.extra_rels.clear();
        if kids.is_empty() {
            return;
        }
        out.push(sd::Item::Group(sd::GroupItem {
            xfrm: sd::Xfrm::rect(xfrm.x, xfrm.y, xfrm.w, xfrm.h),
            child_off: (0.0, 0.0),
            child_ext: (xfrm.w, xfrm.h),
            items: kids,
        }));
    }
}
