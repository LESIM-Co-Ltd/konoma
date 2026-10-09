//! Fills, outlines and effects: the `spPr` of a shape, the `a:ln`, the style lists of the theme
//! reached through `p:style` (`fillRef`, `lnRef`, `effectRef`).

use std::collections::HashMap;

use crate::preview::office::slide_draw as sd;
use crate::preview::office::slide_draw::Rgba;

use super::*;

/// Most colour stops kept of one gradient (a forged `gsLst` must not cost unbounded work).
pub(super) const MAX_GRAD_STOPS: usize = 64;
/// Most `a:ds` entries kept of one custom dash.
pub(super) const MAX_CUST_DASH: usize = 16;

/// What a fill element says.
pub(super) enum FillRes {
    Set(sd::Fill),
    /// `a:grpFill`: the fill of the enclosing group.
    Group,
}

/// The elements that set a fill.
pub(super) fn is_fill_elem(name: &str) -> bool {
    matches!(
        name,
        "noFill" | "solidFill" | "gradFill" | "blipFill" | "pattFill" | "grpFill"
    )
}

/// An integer attribute as a float.
pub(super) fn num(n: &Node, name: &str) -> Option<f64> {
    n.attr(name)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|v| v as f64)
}

/// A boolean attribute (`1` / `true`).
pub(super) fn flag(n: &Node, name: &str) -> Option<bool> {
    n.attr(name).map(|v| matches!(v.trim(), "1" | "true"))
}

/// A rectangle given by `l` `t` `r` `b` attributes in 100000ths (`srcRect`, `fillRect` ..).
fn rect4(n: &Node) -> sd::Rect4 {
    let f = |k: &str| num(n, k).unwrap_or(0.0) / 100_000.0;
    (f("l"), f("t"), f("r"), f("b"))
}

/// What an `a:ln` sets; unset properties inherit (the style's line, the placeholder's).
#[derive(Debug, Clone, Default)]
pub(super) struct LineSpec {
    pub width: Option<f64>,
    pub fill: Option<sd::Fill>,
    pub dash: Option<sd::Dash>,
    pub cap: Option<sd::Cap>,
    pub join: Option<sd::Join>,
    pub compound: Option<sd::Compound>,
    /// `Some(None)`: `type="none"` removes an inherited arrow head.
    pub head: Option<Option<sd::Arrow>>,
    pub tail: Option<Option<sd::Arrow>>,
}

impl LineSpec {
    /// Lays `over` on top of this one: what it sets replaces.
    pub(super) fn over(&mut self, over: LineSpec) {
        macro_rules! take {
            ($($f:ident),*) => { $(if over.$f.is_some() { self.$f = over.$f; })* };
        }
        take!(width, fill, dash, cap, join, compound, head, tail);
    }

    /// The line to draw (`None` when there is no visible fill).
    pub(super) fn finish(self) -> Option<sd::Line> {
        let fill = self.fill?;
        if !fill.is_visible() {
            return None;
        }
        Some(sd::Line {
            width: self.width.unwrap_or(0.0),
            fill,
            dash: self.dash.unwrap_or_default(),
            cap: self.cap.unwrap_or_default(),
            join: self.join.unwrap_or_default(),
            compound: self.compound.unwrap_or_default(),
            head: self.head.unwrap_or(None),
            tail: self.tail.unwrap_or(None),
        })
    }
}

impl Sb<'_> {
    /// The first fill element among the children of `parent` (a shape's `spPr`, a background's
    /// `bgPr`, a table cell's `tcPr` ..). `ph` is the colour `phClr` stands for.
    pub(super) fn fill_in(
        &mut self,
        parent: &Node,
        ph: Option<Rgba>,
        rels: &HashMap<String, Rel>,
    ) -> Option<FillRes> {
        let e = parent.nodes().find(|c| is_fill_elem(&c.name))?;
        Some(self.fill_elem(e, ph, rels))
    }

    /// A fill element itself (`a:solidFill`, `a:gradFill` ..).
    pub(super) fn fill_elem(
        &mut self,
        e: &Node,
        ph: Option<Rgba>,
        rels: &HashMap<String, Rel>,
    ) -> FillRes {
        FillRes::Set(match e.name.as_str() {
            "grpFill" => return FillRes::Group,
            "solidFill" => match self.col.first_in(e, ph) {
                Some(c) => sd::Fill::Solid(c),
                None => sd::Fill::None,
            },
            "gradFill" => self.gradient(e, ph),
            "pattFill" => {
                let c = |k: &str, d: Rgba| {
                    e.child(k)
                        .and_then(|n| self.col.first_in(n, ph))
                        .unwrap_or(d)
                };
                sd::Fill::Pattern {
                    preset: e.attr("prst").unwrap_or("pct50").to_string(),
                    fg: c("fgClr", Rgba::BLACK),
                    bg: c("bgClr", Rgba::WHITE),
                }
            }
            "blipFill" => match self.image_fill(e, rels) {
                Some(i) => sd::Fill::Image(i),
                None => sd::Fill::None,
            },
            _ => sd::Fill::None,
        })
    }

    fn gradient(&mut self, g: &Node, ph: Option<Rgba>) -> sd::Fill {
        let mut stops: Vec<(f64, Rgba)> = Vec::new();
        if let Some(lst) = g.child("gsLst") {
            for gs in lst.nodes().filter(|n| n.name == "gs") {
                if stops.len() >= MAX_GRAD_STOPS {
                    self.truncated = true;
                    break;
                }
                let pos = (num(gs, "pos").unwrap_or(0.0) / 100_000.0).clamp(0.0, 1.0);
                if let Some(c) = self.col.first_in(gs, ph) {
                    stops.push((pos, c));
                }
            }
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        match stops.len() {
            0 => return sd::Fill::None,
            1 => return sd::Fill::Solid(stops[0].1),
            _ => {}
        }
        let (kind, fill_to_rect) = match g.child("path") {
            Some(p) => (
                match p.attr("path").unwrap_or("circle") {
                    "rect" => sd::GradKind::Rect,
                    "shape" => sd::GradKind::Path,
                    _ => sd::GradKind::Radial,
                },
                p.child("fillToRect")
                    .map(rect4)
                    .unwrap_or((0.5, 0.5, 0.5, 0.5)),
            ),
            None => {
                let lin = g.child("lin");
                (
                    sd::GradKind::Linear {
                        angle_deg: lin
                            .and_then(|l| num(l, "ang"))
                            .map_or(0.0, |a| a / 60_000.0),
                        scaled: lin.and_then(|l| flag(l, "scaled")).unwrap_or(false),
                    },
                    (0.5, 0.5, 0.5, 0.5),
                )
            }
        };
        sd::Fill::Gradient(sd::Gradient {
            kind,
            stops,
            fill_to_rect,
            rot_with_shape: flag(g, "rotWithShape").unwrap_or(true),
        })
    }

    /// The picture of an `a:blipFill` / `p:blipFill` with its crop and layout.
    ///
    /// Choice of picture: the `r:embed` blip (PowerPoint writes the PNG / JPEG there, with the SVG
    /// of an icon in `a:extLst/a:ext/asvg:svgBlip` beside it). The SVG is used only when the
    /// embedded one cannot be shown (missing, not a picture kind konoma loads, over a budget).
    pub(super) fn image_fill(
        &mut self,
        bf: &Node,
        rels: &HashMap<String, Rel>,
    ) -> Option<sd::ImageFill> {
        let blip = bf.child("blip")?;
        let key = self.blip_key(blip, rels)?;
        let alpha = blip
            .child("alphaModFix")
            .and_then(|a| num(a, "amt"))
            .map_or(1.0, |v| (v / 100_000.0).clamp(0.0, 1.0));
        let crop = bf.child("srcRect").map(rect4).unwrap_or_default();
        let mode = match bf.child("tile") {
            Some(t) => sd::ImageMode::Tile {
                sx: num(t, "sx").map_or(1.0, |v| v / 100_000.0),
                sy: num(t, "sy").map_or(1.0, |v| v / 100_000.0),
                tx: num(t, "tx").unwrap_or(0.0),
                ty: num(t, "ty").unwrap_or(0.0),
                align: match t.attr("algn").unwrap_or("tl") {
                    "t" => sd::RectAlign::Top,
                    "tr" => sd::RectAlign::TopRight,
                    "l" => sd::RectAlign::Left,
                    "ctr" => sd::RectAlign::Center,
                    "r" => sd::RectAlign::Right,
                    "bl" => sd::RectAlign::BottomLeft,
                    "b" => sd::RectAlign::Bottom,
                    "br" => sd::RectAlign::BottomRight,
                    _ => sd::RectAlign::TopLeft,
                },
                flip: match t.attr("flip").unwrap_or("none") {
                    "x" => sd::TileFlip::X,
                    "y" => sd::TileFlip::Y,
                    "xy" => sd::TileFlip::Xy,
                    _ => sd::TileFlip::None,
                },
            },
            None => sd::ImageMode::Stretch {
                fill_rect: bf
                    .child("stretch")
                    .and_then(|s| s.child("fillRect"))
                    .map(rect4)
                    .unwrap_or_default(),
            },
        };
        Some(sd::ImageFill {
            key,
            crop: (
                finite(crop.0),
                finite(crop.1),
                finite(crop.2),
                finite(crop.3),
            ),
            mode,
            alpha,
        })
    }

    /// The `office-img://` key of the picture a `a:blip` points at (see [`Self::image_fill`]).
    pub(super) fn blip_key(&mut self, blip: &Node, rels: &HashMap<String, Rel>) -> Option<String> {
        let extra = &self.extra_rels;
        let mut load = |rid: &str| -> Option<String> {
            let rel = rels
                .get(rid)
                .or_else(|| extra.get(rid))
                .filter(|r| !r.external)?;
            (self.loader)(&rel.target)
        };
        let embed = blip.rel_attr("embed").and_then(&mut load);
        // A metafile (EMF / WMF) as the embedded picture gives way to an SVG beside it (PowerPoint
        // writes the SVG as the sharp version of the picture).
        let is_meta = |k: &str| {
            let k = k.to_ascii_lowercase();
            k.ends_with(".emf") || k.ends_with(".wmf")
        };
        if let Some(k) = &embed {
            if !is_meta(k) {
                return embed;
            }
        }
        let svg = blip.child("extLst").and_then(|ext| {
            ext.nodes()
                .flat_map(|e| e.nodes())
                .filter(|n| n.name == "svgBlip")
                .find_map(|n| n.rel_attr("embed").and_then(&mut load))
        });
        svg.or(embed)
    }

    /// What an `a:ln` sets. `ph` is the colour `phClr` stands for.
    pub(super) fn line_spec(
        &mut self,
        ln: &Node,
        ph: Option<Rgba>,
        rels: &HashMap<String, Rel>,
    ) -> LineSpec {
        let mut s = LineSpec {
            width: num(ln, "w").map(|w| w.clamp(0.0, 1.0e9)),
            cap: ln.attr("cap").map(|c| match c {
                "rnd" => sd::Cap::Round,
                "sq" => sd::Cap::Square,
                _ => sd::Cap::Flat,
            }),
            compound: ln.attr("cmpd").map(|c| match c {
                "dbl" => sd::Compound::Dbl,
                "thickThin" => sd::Compound::ThickThin,
                "thinThick" => sd::Compound::ThinThick,
                "tri" => sd::Compound::Tri,
                _ => sd::Compound::Sng,
            }),
            ..LineSpec::default()
        };
        for c in ln.nodes() {
            match c.name.as_str() {
                n if is_fill_elem(n) => {
                    if let FillRes::Set(f) = self.fill_elem(c, ph, rels) {
                        s.fill = Some(f);
                    }
                }
                "prstDash" => s.dash = Some(preset_dash(c.attr("val").unwrap_or("solid"))),
                "custDash" => {
                    let mut pairs = Vec::new();
                    for ds in c.nodes().filter(|n| n.name == "ds") {
                        if pairs.len() >= MAX_CUST_DASH {
                            self.truncated = true;
                            break;
                        }
                        let f = |k: &str| num(ds, k).unwrap_or(100_000.0) / 100_000.0;
                        pairs.push((f("d").max(0.0), f("sp").max(0.0)));
                    }
                    s.dash = Some(if pairs.is_empty() {
                        sd::Dash::Solid
                    } else {
                        sd::Dash::Custom(pairs)
                    });
                }
                "round" => s.join = Some(sd::Join::Round),
                "bevel" => s.join = Some(sd::Join::Bevel),
                "miter" => {
                    s.join = Some(sd::Join::Miter(
                        num(c, "lim").map_or(8.0, |l| (l / 100_000.0).clamp(1.0, 100.0)),
                    ))
                }
                "headEnd" => s.head = Some(arrow(c)),
                "tailEnd" => s.tail = Some(arrow(c)),
                _ => {}
            }
        }
        s
    }

    /// The `a:ln` of a theme line style / a shape style: `lnRef idx` into the theme's line list.
    pub(super) fn style_line(&mut self, style: &Node) -> LineSpec {
        let Some(r) = style.child("lnRef") else {
            return LineSpec::default();
        };
        let ph = self.col.first_in(r, None);
        let idx = num(r, "idx").unwrap_or(0.0) as usize;
        let th = self.theme;
        match idx.checked_sub(1).and_then(|i| th.lines.get(i)) {
            Some(ln) => self.line_spec(ln, ph, &th.rels),
            None => LineSpec::default(),
        }
    }

    /// The fill of a shape style: `fillRef idx` into the theme's fill list (`1..`) or background
    /// fill list (`1001..`); `idx 0` is no fill.
    pub(super) fn style_fill(&mut self, style: &Node) -> Option<sd::Fill> {
        let r = style.child("fillRef")?;
        let ph = self.col.first_in(r, None);
        let idx = num(r, "idx").unwrap_or(0.0) as usize;
        self.theme_fill(idx, ph)
    }

    /// Entry `idx` of the theme's fill lists with `phClr` = `ph` (`None` for idx 0 or one the
    /// theme lacks).
    pub(super) fn theme_fill(&mut self, idx: usize, ph: Option<Rgba>) -> Option<sd::Fill> {
        let th = self.theme;
        let node = if idx >= 1001 {
            th.bg_fills.get(idx - 1001)
        } else {
            th.fills.get(idx.checked_sub(1)?)
        }?;
        match self.fill_elem(node, ph, &th.rels) {
            FillRes::Set(f) => Some(f),
            FillRes::Group => None,
        }
    }

    /// The effects of a shape style (`effectRef idx` into the theme's effect list).
    pub(super) fn style_effects(&self, style: &Node) -> sd::Effects {
        let Some(r) = style.child("effectRef") else {
            return sd::Effects::default();
        };
        let ph = self.col.first_in(r, None);
        let idx = num(r, "idx").unwrap_or(0.0) as usize;
        idx.checked_sub(1)
            .and_then(|i| self.theme.effects.get(i))
            .and_then(|s| s.child("effectLst"))
            .map(|l| self.effects_of(l, ph))
            .unwrap_or_default()
    }

    /// An `a:effectLst`.
    pub(super) fn effects_of(&self, lst: &Node, ph: Option<Rgba>) -> sd::Effects {
        let mut fx = sd::Effects::default();
        let color = |n: &Node, d: Rgba| self.col.first_in(n, ph).unwrap_or(d);
        let shadow = |n: &Node| sd::Shadow {
            blur_rad: num(n, "blurRad").unwrap_or(0.0).clamp(0.0, 1.0e9),
            dist: num(n, "dist").unwrap_or(0.0).clamp(-1.0e9, 1.0e9),
            dir_deg: num(n, "dir").unwrap_or(0.0) / 60_000.0,
            color: color(n, Rgba::BLACK),
            sx: num(n, "sx").map_or(1.0, |v| v / 100_000.0),
            sy: num(n, "sy").map_or(1.0, |v| v / 100_000.0),
            rot_with_shape: flag(n, "rotWithShape").unwrap_or(true),
        };
        for e in lst.nodes() {
            match e.name.as_str() {
                "outerShdw" => fx.outer_shadow = Some(shadow(e)),
                "innerShdw" => fx.inner_shadow = Some(shadow(e)),
                "glow" => {
                    fx.glow = Some(sd::Glow {
                        rad: num(e, "rad").unwrap_or(0.0).clamp(0.0, 1.0e9),
                        color: color(e, Rgba::BLACK),
                    })
                }
                "softEdge" => {
                    fx.soft_edge = Some(num(e, "rad").unwrap_or(0.0).clamp(0.0, 1.0e9));
                }
                "reflection" => {
                    let p = |k: &str, d: f64| num(e, k).map_or(d, |v| v / 100_000.0);
                    fx.reflection = Some(sd::Reflection {
                        blur_rad: num(e, "blurRad").unwrap_or(0.0).clamp(0.0, 1.0e9),
                        start_alpha: p("stA", 1.0).clamp(0.0, 1.0),
                        end_alpha: p("endA", 0.0).clamp(0.0, 1.0),
                        start_pos: p("stPos", 0.0).clamp(0.0, 1.0),
                        end_pos: p("endPos", 1.0).clamp(0.0, 1.0),
                        dist: num(e, "dist").unwrap_or(0.0).clamp(-1.0e9, 1.0e9),
                        dir_deg: num(e, "dir").unwrap_or(0.0) / 60_000.0,
                        fade_dir_deg: num(e, "fadeDir").map_or(90.0, |v| v / 60_000.0),
                        sx: p("sx", 1.0),
                        sy: p("sy", 1.0),
                    });
                }
                _ => {}
            }
        }
        fx
    }
}

fn finite(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(-100.0, 100.0)
    } else {
        0.0
    }
}

fn preset_dash(v: &str) -> sd::Dash {
    match v {
        "dot" => sd::Dash::Dot,
        "dash" => sd::Dash::Dash,
        "lgDash" => sd::Dash::LgDash,
        "dashDot" => sd::Dash::DashDot,
        "lgDashDot" => sd::Dash::LgDashDot,
        "lgDashDotDot" => sd::Dash::LgDashDotDot,
        "sysDash" => sd::Dash::SysDash,
        "sysDot" => sd::Dash::SysDot,
        "sysDashDot" => sd::Dash::SysDashDot,
        "sysDashDotDot" => sd::Dash::SysDashDotDot,
        _ => sd::Dash::Solid,
    }
}

/// `a:headEnd` / `a:tailEnd`: `None` for `type="none"` (or an unknown type).
fn arrow(n: &Node) -> Option<sd::Arrow> {
    let kind = match n.attr("type").unwrap_or("none") {
        "triangle" => sd::ArrowKind::Triangle,
        "stealth" => sd::ArrowKind::Stealth,
        "diamond" => sd::ArrowKind::Diamond,
        "oval" => sd::ArrowKind::Oval,
        "arrow" => sd::ArrowKind::Arrow,
        _ => return None,
    };
    let size = |k: &str| match n.attr(k).unwrap_or("med") {
        "sm" => sd::ArrowSize::Sm,
        "lg" => sd::ArrowSize::Lg,
        _ => sd::ArrowSize::Med,
    };
    Some(sd::Arrow {
        kind,
        w: size("w"),
        len: size("len"),
    })
}
