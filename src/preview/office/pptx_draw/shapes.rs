//! The shape tree of a slide, layout or master as scene items: shapes (`p:sp`), connectors
//! (`p:cxnSp`), pictures (`p:pic`), groups (`p:grpSp`), graphic frames (`p:graphicFrame`) and
//! `mc:AlternateContent`.

use crate::preview::office::slide_draw as sd;

use super::geom::geometry_of;
use super::style::{flag, num, FillRes};
use super::text::TextChain;
use super::*;

/// The key a picture that could not be loaded is drawn with: the renderer finds no such picture
/// and draws its placeholder.
pub(super) const MISSING_PICTURE: &str = "office-img://missing/picture";

/// Whether a shape node of a layout / master is a placeholder (those are not drawn on slides).
pub(super) fn is_placeholder(n: &Node) -> bool {
    let nv = match n.name.as_str() {
        "sp" => "nvSpPr",
        "pic" => "nvPicPr",
        "graphicFrame" => "nvGraphicFramePr",
        "cxnSp" => "nvCxnSpPr",
        _ => return false,
    };
    ph_of(n.child(nv)).is_some()
}

fn emu(v: f64) -> f64 {
    v.clamp(-4.0e9, 4.0e9)
}

/// An `a:xfrm` / `p:xfrm`: the box, rotation and flips.
pub(super) fn xfrm_of(x: &Node) -> Option<sd::Xfrm> {
    let off = x.child("off")?;
    let ext = x.child("ext")?;
    Some(sd::Xfrm {
        x: emu(num(off, "x").unwrap_or(0.0)),
        y: emu(num(off, "y").unwrap_or(0.0)),
        w: emu(num(ext, "cx").unwrap_or(0.0)).max(0.0),
        h: emu(num(ext, "cy").unwrap_or(0.0)).max(0.0),
        rot_deg: num(x, "rot").map_or(0.0, |r| r / 60_000.0),
        flip_h: flag(x, "flipH").unwrap_or(false),
        flip_v: flag(x, "flipV").unwrap_or(false),
    })
}

fn hidden(nv: Option<&Node>) -> bool {
    nv.and_then(|n| n.child("cNvPr"))
        .and_then(|c| flag(c, "hidden"))
        .unwrap_or(false)
}

impl<'a> Sb<'a> {
    /// The shapes among `nodes`, appended to `out` in paint order. Over the item budget or the
    /// group depth the rest is left out and the scene says so.
    pub fn build_nodes<'n>(
        &mut self,
        nodes: impl Iterator<Item = &'n Node> + Clone,
        depth: usize,
        out: &mut Vec<sd::Item>,
    ) {
        let is_shape = |n: &Node| {
            matches!(
                n.name.as_str(),
                "sp" | "pic" | "graphicFrame" | "grpSp" | "cxnSp" | "AlternateContent"
            )
        };
        if depth > MAX_GROUP_DEPTH {
            if nodes.clone().any(is_shape) {
                self.truncated = true;
            }
            return;
        }
        for n in nodes {
            if !is_shape(n) {
                continue;
            }
            if self.items >= self.max_items {
                self.truncated = true;
                return;
            }
            self.items += 1;
            match n.name.as_str() {
                "sp" => out.extend(self.build_sp(n, false)),
                "cxnSp" => out.extend(self.build_sp(n, true)),
                "pic" => out.extend(self.build_pic(n)),
                "grpSp" => out.extend(self.build_group(n, depth)),
                "graphicFrame" => self.build_frame(n, out),
                _ => {
                    // (`AlternateContent` is a wrapper, not a shape of its own.)
                    self.items -= 1;
                    if let Some(c) = alt_content(n, PPTX_READS) {
                        self.build_nodes(c.nodes(), depth + 1, out);
                    }
                }
            }
        }
    }

    /// The shape node itself and, for a placeholder, the layout's and the master's placeholder it
    /// inherits from, each with the index of the part it lives in (0 slide, 1 layout, 2 master).
    fn ph_chain<'x>(&self, n: &'x Node, nv: &str) -> Vec<(&'x Node, usize)>
    where
        'a: 'x,
    {
        let mut chain: Vec<(&Node, usize)> = vec![(n, self.cur)];
        let (Some(inh), Some(ph)) = (self.inh, ph_of(n.child(nv))) else {
            return chain;
        };
        let key = ph_key(ph);
        let lay = inh.layout_ph(&key);
        if let Some(node) = lay.and_then(|p| inh.layout.nodes.get(p.at)) {
            chain.push((node, 1));
        }
        // (An `idx` the layout lacks inherits nothing from the master either.)
        if lay.is_some() || !key.explicit {
            if let Some(node) = inh
                .master_ph_for(&key)
                .and_then(|p| inh.master.nodes.get(p.at))
            {
                chain.push((node, 2));
            }
        }
        chain
    }

    pub(super) fn rels_at(&self, part: usize) -> &'a HashMap<String, Rel> {
        self.rels[part.min(2)]
    }

    /// A `p:sp` or `p:cxnSp`.
    pub(super) fn build_sp(&mut self, sp: &Node, connector: bool) -> Option<sd::Item> {
        let nvn = if connector { "nvCxnSpPr" } else { "nvSpPr" };
        if hidden(sp.child(nvn)) {
            return None;
        }
        let chain = self.ph_chain(sp, nvn);
        let sprs: Vec<(&Node, usize)> = chain
            .iter()
            .filter_map(|(n, i)| n.child("spPr").map(|p| (p, *i)))
            .collect();
        let xfrm = sprs
            .iter()
            .find_map(|(p, _)| p.child("xfrm"))
            .and_then(xfrm_of)?;
        let (geom, text_rect) = sprs
            .iter()
            .find(|(p, _)| p.child("prstGeom").is_some() || p.child("custGeom").is_some())
            .and_then(|(p, _)| geometry_of(p, xfrm.w, xfrm.h))
            .unwrap_or((sd::Geometry::Rect, None));
        let style = sp.child("style");

        // Fill: the first of the shape / layout / master `spPr`, else the style's.
        let mut fill: Option<sd::Fill> = None;
        for (p, i) in &sprs {
            let rels = self.rels_at(*i);
            match self.fill_in(p, None, rels) {
                Some(FillRes::Set(f)) => {
                    fill = Some(f);
                    break;
                }
                Some(FillRes::Group) => {
                    fill = Some(
                        self.group_fills
                            .last()
                            .cloned()
                            .flatten()
                            .unwrap_or_default(),
                    );
                    break;
                }
                None => {}
            }
        }
        let fill = match fill {
            Some(f) => f,
            None => style.and_then(|s| self.style_fill(s)).unwrap_or_default(),
        };

        // Line: the style's line, laid over by the master's, the layout's and the shape's `a:ln`.
        let mut line = style.map(|s| self.style_line(s)).unwrap_or_default();
        for (p, i) in sprs.iter().rev() {
            if let Some(ln) = p.child("ln") {
                let rels = self.rels_at(*i);
                let spec = self.line_spec(ln, None, rels);
                line.over(spec);
            }
        }
        let line = line.finish();

        let effects = match sprs.iter().find_map(|(p, _)| p.child("effectLst")) {
            Some(l) => self.effects_of(l, None),
            None => style.map(|s| self.style_effects(s)).unwrap_or_default(),
        };

        let mut text = if connector {
            None
        } else {
            sp.child("txBody").and_then(|tx| {
                let tc = self.text_chain(sp, &chain, nvn, style);
                self.text_body(tx, &tc)
            })
        };
        // A SmartArt shape (`dsp:sp`) carries its own text box, apart from the shape's.
        let mut text_rect = text_rect;
        if let (Some(t), Some(tx)) = (text.as_mut(), sp.child("txXfrm").and_then(xfrm_of)) {
            let (rect, turn) = super::frames::text_box(&xfrm, &tx);
            text_rect = Some(rect);
            t.rot_deg += turn;
        }

        if !fill.is_visible()
            && line.is_none()
            && text.is_none()
            && effects == sd::Effects::default()
        {
            return None;
        }
        Some(sd::Item::Shape(sd::ShapeItem {
            xfrm,
            geom,
            fill,
            line,
            text,
            text_rect,
            effects,
        }))
    }

    /// The nodes the text properties of a shape are looked up in.
    fn text_chain<'x>(
        &self,
        sp: &'x Node,
        chain: &[(&'x Node, usize)],
        nvn: &str,
        style: Option<&Node>,
    ) -> TextChain<'x> {
        let mut lst: [Option<&Node>; 3] = [None; 3];
        let mut body_pr: Vec<&Node> = Vec::new();
        for (n, i) in chain {
            if let Some(tx) = n.child("txBody") {
                if lst[*i].is_none() {
                    lst[*i] = tx.child("lstStyle");
                }
                body_pr.extend(tx.child("bodyPr"));
            }
        }
        let style_idx = match ph_of(sp.child(nvn)).map(ph_key).map(|k| k.class) {
            Some(Class::Title) => 0,
            Some(Class::Body | Class::Sub) => 1,
            _ => 2,
        };
        TextChain {
            body_pr,
            lst,
            style: style_idx,
            font_ref: TextChain::font_ref_node(style),
        }
    }

    /// A `p:pic`.
    fn build_pic(&mut self, pic: &Node) -> Option<sd::Item> {
        if hidden(pic.child("nvPicPr")) {
            return None;
        }
        let chain = self.ph_chain(pic, "nvPicPr");
        let sprs: Vec<(&Node, usize)> = chain
            .iter()
            .filter_map(|(n, i)| n.child("spPr").map(|p| (p, *i)))
            .collect();
        let xfrm = sprs
            .iter()
            .find_map(|(p, _)| p.child("xfrm"))
            .and_then(xfrm_of)?;
        let bf = pic.child("blipFill")?;
        let rels = self.rels_at(self.cur);
        // A picture that cannot be shown (linked, missing, not a kind konoma loads) is drawn as
        // the renderer's placeholder.
        let image = self
            .image_fill(bf, rels)
            .unwrap_or_else(|| sd::ImageFill::stretch(MISSING_PICTURE));
        let geom = sprs
            .iter()
            .find(|(p, _)| p.child("prstGeom").is_some() || p.child("custGeom").is_some())
            .and_then(|(p, _)| geometry_of(p, xfrm.w, xfrm.h))
            .map_or(sd::Geometry::Rect, |g| g.0);
        let style = pic.child("style");
        let mut line = style.map(|s| self.style_line(s)).unwrap_or_default();
        for (p, i) in sprs.iter().rev() {
            if let Some(ln) = p.child("ln") {
                let rels = self.rels_at(*i);
                let spec = self.line_spec(ln, None, rels);
                line.over(spec);
            }
        }
        let effects = match sprs.iter().find_map(|(p, _)| p.child("effectLst")) {
            Some(l) => self.effects_of(l, None),
            None => style.map(|s| self.style_effects(s)).unwrap_or_default(),
        };
        Some(sd::Item::Picture(sd::PictureItem {
            xfrm,
            image,
            geom,
            line: line.finish(),
            effects,
        }))
    }

    /// A `p:grpSp`.
    fn build_group(&mut self, g: &Node, depth: usize) -> Option<sd::Item> {
        if hidden(g.child("nvGrpSpPr")) {
            return None;
        }
        let gpr = g.child("grpSpPr");
        let x = gpr.and_then(|p| p.child("xfrm"));
        let (w, h) = self.size;
        // A group with no transform maps its members onto themselves.
        let xfrm = x
            .and_then(xfrm_of)
            .unwrap_or(sd::Xfrm::rect(0.0, 0.0, w, h));
        let child_off = x
            .and_then(|x| x.child("chOff"))
            .map(|o| {
                (
                    emu(num(o, "x").unwrap_or(0.0)),
                    emu(num(o, "y").unwrap_or(0.0)),
                )
            })
            .unwrap_or((xfrm.x, xfrm.y));
        let child_ext = x
            .and_then(|x| x.child("chExt"))
            .map(|e| {
                (
                    emu(num(e, "cx").unwrap_or(0.0)).max(0.0),
                    emu(num(e, "cy").unwrap_or(0.0)).max(0.0),
                )
            })
            .filter(|(cw, ch)| *cw > 0.0 && *ch > 0.0)
            .unwrap_or((xfrm.w, xfrm.h));
        // (`a:grpFill` of a member is the group's own fill.)
        let own_fill = gpr.and_then(|p| {
            let rels = self.rels_at(self.cur);
            match self.fill_in(p, None, rels) {
                Some(FillRes::Set(f)) => Some(f),
                _ => None,
            }
        });
        let inherited = self.group_fills.last().cloned().flatten();
        self.group_fills.push(own_fill.or(inherited));
        let mut kids = Vec::new();
        self.build_nodes(g.nodes(), depth + 1, &mut kids);
        self.group_fills.pop();
        (!kids.is_empty()).then_some(sd::Item::Group(sd::GroupItem {
            xfrm,
            child_off,
            child_ext,
            items: kids,
        }))
    }

    /// A `p:graphicFrame`: an embedded object's picture; tables, charts and SmartArt go to their
    /// own functions.
    fn build_frame(&mut self, f: &Node, out: &mut Vec<sd::Item>) {
        if hidden(f.child("nvGraphicFramePr")) {
            return;
        }
        let chain = self.ph_chain(f, "nvGraphicFramePr");
        let Some(xfrm) = chain
            .iter()
            .find_map(|(n, _)| n.child("xfrm"))
            .and_then(xfrm_of)
        else {
            return;
        };
        let Some(data) = f.child("graphic").and_then(|g| g.child("graphicData")) else {
            return;
        };
        let uri = data.attr("uri").unwrap_or("").to_ascii_lowercase();
        if data.child("tbl").is_some() {
            self.frame_table(f, xfrm, out);
        } else if uri.contains("/chart") || data.child("chart").is_some() {
            self.frame_chart(f, xfrm, out);
        } else if uri.ends_with("/diagram") {
            self.frame_diagram(f, xfrm, out);
        } else if let Some(rid) = find_blip(data, 0) {
            // An embedded object (`p:oleObj`) is drawn as the picture of what it contains.
            let rels = self.rels_at(self.cur);
            let key = rels
                .get(&rid)
                .filter(|r| !r.external)
                .and_then(|r| (self.loader)(&r.target))
                .unwrap_or_else(|| MISSING_PICTURE.to_string());
            out.push(sd::Item::Picture(sd::PictureItem::new(xfrm, key)));
        }
    }

    /// A table (`a:tbl`) as scene items. A later task fills this in (the cell fills, borders and
    /// text become shapes of the scene); until then a table is not drawn.
    fn frame_table(&mut self, _frame: &Node, _xfrm: sd::Xfrm, _out: &mut Vec<sd::Item>) {}
}
