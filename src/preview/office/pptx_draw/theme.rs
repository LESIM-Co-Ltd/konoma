//! The theme of a deck (`a:theme`), the colour map (`p:clrMap` / `p:clrMapOvr`) and DrawingML colours.
//!
//! A colour element (`srgbClr`, `scrgbClr`, `hslClr`, `sysClr`, `prstClr`, `schemeClr`) with its
//! transforms becomes an [`Rgba`]; a `schemeClr` goes through the colour map (`bg1` -> `lt1` ..)
//! to the theme's colour scheme, and `phClr` is the colour the referencing style supplies
//! (`p:style/a:fillRef` and friends).

use std::collections::HashMap;

use super::*;
use crate::preview::office::slide_draw::color::{
    apply_mods, hsl_to_rgb, linear_to_srgb, preset_color, system_color, ColorMod,
};
use crate::preview::office::slide_draw::Rgba;

/// Most transforms read from one colour element (a forged element must not cost unbounded work).
pub(super) const MAX_COLOR_MODS: usize = 32;

/// The fonts of one half of a font scheme (`a:majorFont` / `a:minorFont`).
#[derive(Debug, Clone, Default)]
pub(super) struct ThemeFonts {
    pub latin: String,
    pub ea: String,
    pub cs: String,
    /// `a:font script="Jpan" typeface="..."`: the font for a script when `ea` / `cs` name none.
    pub scripts: Vec<(String, String)>,
}

/// What a deck's theme gives: colours, fonts and the format scheme's style lists.
#[derive(Debug, Clone)]
pub(in super::super) struct Theme {
    /// `dk1`, `lt1`, `dk2`, `lt2`, `accent1`..`accent6`, `hlink`, `folHlink`.
    pub colors: Vec<(String, Rgba)>,
    pub(super) major: ThemeFonts,
    pub(super) minor: ThemeFonts,
    /// `a:fillStyleLst`: the fills `fillRef idx=1..` point at.
    pub fills: Vec<Node>,
    /// `a:lnStyleLst`: the `a:ln` elements `lnRef idx=1..` point at.
    pub lines: Vec<Node>,
    /// `a:effectStyleLst`: the `a:effectStyle` elements `effectRef idx=1..` point at.
    pub effects: Vec<Node>,
    /// `a:bgFillStyleLst`: the fills `fillRef idx=1001..` and `bgRef idx=1001..` point at.
    pub bg_fills: Vec<Node>,
    /// The relationships of the theme part (a picture fill of the format scheme).
    pub rels: HashMap<String, Rel>,
}

impl Default for Theme {
    /// The Office 2007-2010 theme, used when a deck has no readable theme.
    fn default() -> Theme {
        let c = |n: &str, v: u32| {
            (
                n.to_string(),
                Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8),
            )
        };
        Theme {
            colors: vec![
                c("dk1", 0x000000),
                c("lt1", 0xFFFFFF),
                c("dk2", 0x1F497D),
                c("lt2", 0xEEECE1),
                c("accent1", 0x4F81BD),
                c("accent2", 0xC0504D),
                c("accent3", 0x9BBB59),
                c("accent4", 0x8064A2),
                c("accent5", 0x4BACC6),
                c("accent6", 0xF79646),
                c("hlink", 0x0000FF),
                c("folHlink", 0x800080),
            ],
            major: ThemeFonts::default(),
            minor: ThemeFonts::default(),
            fills: Vec::new(),
            lines: Vec::new(),
            effects: Vec::new(),
            bg_fills: Vec::new(),
            rels: HashMap::new(),
        }
    }
}

impl Theme {
    pub(super) fn color(&self, name: &str) -> Option<Rgba> {
        self.colors.iter().find(|(n, _)| n == name).map(|(_, c)| *c)
    }

    /// Reads a theme part. A damaged or partial part gives the defaults for what is missing.
    pub(in super::super) fn parse(
        bytes: &[u8],
        opts: &DocOptions,
        rels: HashMap<String, Rel>,
    ) -> Theme {
        let mut t = Theme {
            rels,
            ..Theme::default()
        };
        let mut rd = XmlReader::new(bytes);
        let mut buf = Vec::new();
        let mut budget = Budget::new(opts.max_block_nodes.min(100_000), 1 << 20);
        let (mut got_clr, mut got_font, mut got_fmt) = (false, false, false);
        loop {
            buf.clear();
            let (e, empty) = match rd.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => (e.into_owned(), false),
                Ok(Event::Empty(e)) => (e.into_owned(), true),
                Ok(Event::Eof) | Err(_) => break,
                _ => continue,
            };
            let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
            let want = match name.as_str() {
                "clrScheme" => !got_clr,
                "fontScheme" => !got_font,
                "fmtScheme" => !got_fmt,
                _ => false,
            };
            if !want {
                continue;
            }
            let Ok(Tree::Ok(n)) = read_element(&mut rd, &e, empty, &mut budget) else {
                break;
            };
            match name.as_str() {
                "clrScheme" => {
                    got_clr = true;
                    t.read_colors(&n);
                }
                "fontScheme" => {
                    got_font = true;
                    t.major = read_fonts(n.child("majorFont"));
                    t.minor = read_fonts(n.child("minorFont"));
                }
                _ => {
                    got_fmt = true;
                    let list = |k: &str| -> Vec<Node> {
                        n.child(k)
                            .map(|l| l.nodes().take(16).cloned().collect())
                            .unwrap_or_default()
                    };
                    t.fills = list("fillStyleLst");
                    t.lines = list("lnStyleLst");
                    t.effects = list("effectStyleLst");
                    t.bg_fills = list("bgFillStyleLst");
                }
            }
        }
        t
    }

    fn read_colors(&mut self, scheme: &Node) {
        for c in scheme.nodes() {
            let name = c.name.as_str();
            if !matches!(
                name,
                "dk1"
                    | "lt1"
                    | "dk2"
                    | "lt2"
                    | "accent1"
                    | "accent2"
                    | "accent3"
                    | "accent4"
                    | "accent5"
                    | "accent6"
                    | "hlink"
                    | "folHlink"
            ) {
                continue;
            }
            let Some(elem) = c.nodes().next() else {
                continue;
            };
            // (A theme colour is a plain colour: no scheme reference to resolve.)
            let Some(rgba) = color_elem(elem, &|_| None) else {
                continue;
            };
            match self.colors.iter_mut().find(|(n, _)| n == name) {
                Some(slot) => slot.1 = rgba,
                None => self.colors.push((name.to_string(), rgba)),
            }
        }
    }
}

fn read_fonts(n: Option<&Node>) -> ThemeFonts {
    let Some(n) = n else {
        return ThemeFonts::default();
    };
    let face = |k: &str| {
        n.child(k)
            .and_then(|f| f.attr("typeface"))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    ThemeFonts {
        latin: face("latin"),
        ea: face("ea"),
        cs: face("cs"),
        scripts: n
            .nodes()
            .filter(|f| f.name == "font")
            .filter_map(|f| {
                Some((
                    f.attr("script")?.to_string(),
                    f.attr("typeface")?.trim().to_string(),
                ))
            })
            .take(64)
            .collect(),
    }
}

/// `p:clrMap`: which theme colour each of `bg1`, `tx1`, `bg2`, `tx2`, `accent1`.. stands for.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ClrMap(Vec<(String, String)>);

impl Default for ClrMap {
    fn default() -> ClrMap {
        let pairs = [
            ("bg1", "lt1"),
            ("tx1", "dk1"),
            ("bg2", "lt2"),
            ("tx2", "dk2"),
            ("accent1", "accent1"),
            ("accent2", "accent2"),
            ("accent3", "accent3"),
            ("accent4", "accent4"),
            ("accent5", "accent5"),
            ("accent6", "accent6"),
            ("hlink", "hlink"),
            ("folHlink", "folHlink"),
        ];
        ClrMap(
            pairs
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        )
    }
}

impl ClrMap {
    /// From a `p:clrMap` / `a:overrideClrMapping` element (attributes missing keep the default).
    pub(super) fn from_node(n: &Node) -> ClrMap {
        let mut m = ClrMap::default();
        for (k, v) in &n.attrs {
            let k = crate::preview::office::docx_xml::local_of(k);
            if let Some(slot) = m.0.iter_mut().find(|(n, _)| n == k) {
                slot.1 = v.trim().to_string();
            }
        }
        m
    }

    /// The map a `p:clrMapOvr` element asks for: `a:overrideClrMapping` replaces `base`,
    /// `a:masterClrMapping` (or nothing) keeps it.
    pub(super) fn with_override(base: &ClrMap, ovr: Option<&Node>) -> ClrMap {
        match ovr.and_then(|o| o.child("overrideClrMapping")) {
            Some(o) => ClrMap::from_node(o),
            None => base.clone(),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Resolves colours against a theme and a colour map.
#[derive(Clone, Copy)]
pub(super) struct Colors<'a> {
    pub theme: &'a Theme,
    pub map: &'a ClrMap,
}

impl Colors<'_> {
    /// A `schemeClr` value: `phClr` is `ph`; `bg1` / `tx1` / `accentN` .. go through the colour
    /// map; `dk1` / `lt1` .. name the theme colour directly.
    pub(super) fn scheme(&self, val: &str, ph: Option<Rgba>) -> Option<Rgba> {
        if val == "phClr" {
            return ph;
        }
        let name = self.map.get(val).unwrap_or(val);
        self.theme.color(name)
    }

    /// The colour a colour element (`a:srgbClr`, `a:schemeClr` ..) stands for, transforms applied.
    pub(super) fn elem(&self, n: &Node, ph: Option<Rgba>) -> Option<Rgba> {
        color_elem(n, &|v| self.scheme(v, ph))
    }

    /// The colour of the first colour element among the children of `parent`.
    pub(super) fn first_in(&self, parent: &Node, ph: Option<Rgba>) -> Option<Rgba> {
        parent
            .nodes()
            .find(|c| is_color_elem(&c.name))
            .and_then(|c| self.elem(c, ph))
    }
}

pub(super) fn is_color_elem(name: &str) -> bool {
    matches!(
        name,
        "srgbClr" | "scrgbClr" | "hslClr" | "sysClr" | "prstClr" | "schemeClr"
    )
}

fn int(n: &Node, name: &str) -> Option<f64> {
    n.attr(name)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|v| v as f64)
}

/// A colour element with its transforms. `scheme` resolves a `schemeClr` value.
fn color_elem(n: &Node, scheme: &dyn Fn(&str) -> Option<Rgba>) -> Option<Rgba> {
    let base = match n.name.as_str() {
        "srgbClr" => Rgba::from_hex(n.attr("val")?.trim())?,
        "scrgbClr" => {
            // Linear-light channels in 1000ths of a percent.
            let ch = |k: &str| {
                let v = (int(n, k).unwrap_or(0.0) / 100_000.0).clamp(0.0, 1.0);
                (linear_to_srgb(v).clamp(0.0, 1.0) * 255.0).round() as u8
            };
            Rgba::rgb(ch("r"), ch("g"), ch("b"))
        }
        "hslClr" => {
            let h = int(n, "hue").unwrap_or(0.0) / 60_000.0;
            let s = int(n, "sat").unwrap_or(0.0) / 100_000.0;
            let l = int(n, "lum").unwrap_or(0.0) / 100_000.0;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            Rgba::rgb(r, g, b)
        }
        "sysClr" => n
            .attr("lastClr")
            .and_then(|v| Rgba::from_hex(v.trim()))
            .or_else(|| system_color(n.attr("val")?))?,
        "prstClr" => preset_color(n.attr("val")?)?,
        "schemeClr" => scheme(n.attr("val")?.trim())?,
        _ => return None,
    };
    Some(apply_mods(base, &color_mods(n)))
}

/// The transforms among the children of a colour element, in order.
pub(super) fn color_mods(n: &Node) -> Vec<ColorMod> {
    let mut out = Vec::new();
    for m in n.nodes() {
        if out.len() >= MAX_COLOR_MODS {
            break;
        }
        let Some(v) = int(m, "val") else {
            if m.name == "comp" {
                out.push(ColorMod::Comp);
            } else if m.name == "inv" {
                out.push(ColorMod::Inv);
            } else if m.name == "gray" {
                out.push(ColorMod::Gray);
            } else if m.name == "gamma" {
                out.push(ColorMod::Gamma);
            } else if m.name == "invGamma" {
                out.push(ColorMod::InvGamma);
            }
            continue;
        };
        // Percentages are in 1000ths of a percent, angles in 60000ths of a degree.
        let p = v / 100_000.0;
        let d = v / 60_000.0;
        out.push(match m.name.as_str() {
            "tint" => ColorMod::Tint(p),
            "shade" => ColorMod::Shade(p),
            "lum" => ColorMod::Lum(p),
            "lumMod" => ColorMod::LumMod(p),
            "lumOff" => ColorMod::LumOff(p),
            "sat" => ColorMod::Sat(p),
            "satMod" => ColorMod::SatMod(p),
            "satOff" => ColorMod::SatOff(p),
            "hue" => ColorMod::Hue(d),
            "hueMod" => ColorMod::HueMod(p),
            "hueOff" => ColorMod::HueOff(d),
            "alpha" => ColorMod::Alpha(p),
            "alphaMod" => ColorMod::AlphaMod(p),
            "alphaOff" => ColorMod::AlphaOff(p),
            "red" => ColorMod::Red(p),
            "redMod" => ColorMod::RedMod(p),
            "redOff" => ColorMod::RedOff(p),
            "green" => ColorMod::Green(p),
            "greenMod" => ColorMod::GreenMod(p),
            "greenOff" => ColorMod::GreenOff(p),
            "blue" => ColorMod::Blue(p),
            "blueMod" => ColorMod::BlueMod(p),
            "blueOff" => ColorMod::BlueOff(p),
            _ => continue,
        });
    }
    out
}
