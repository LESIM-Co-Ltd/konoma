//! Reading geometry descriptions: `a:custGeom` nodes and the embedded preset definitions.
//!
//! The presets come from `presets/presetShapeDefinitions.xml`, the file ECMA-376 Part 1 (5th
//! edition) ships, embedded **unchanged** (see `presets/NOTICE.md` for its licence). It is parsed
//! once, on first use, into [`CustomGeomSpec`]s -- a preset is a custom geometry whose guides
//! happen to be standard.

use std::collections::HashMap;
use std::sync::OnceLock;

use quick_xml::events::Event;

use super::geom::{
    CmdSpec, ConnSpec, CustomGeomSpec, PathSpec, Xy, MAX_COMMANDS, MAX_GUIDES, MAX_PATHS,
};
use super::model::PathFill;
use crate::preview::office::docx_xml::{local_of, read_element, Budget, Node, Tree};
use crate::preview::office::fmt_xlsx::XmlReader;

/// The embedded definitions (byte-identical to the Ecma file, which starts with a BOM).
const PRESET_XML: &str = include_str!("presets/presetShapeDefinitions.xml");

/// Node and byte budget for reading one preset (the largest has about 2,000 nodes).
const PRESET_NODES: usize = 100_000;
const PRESET_BYTES: usize = 4 << 20;

type Presets = HashMap<String, CustomGeomSpec>;

fn presets() -> &'static Presets {
    static P: OnceLock<Presets> = OnceLock::new();
    P.get_or_init(load_presets)
}

pub(super) fn preset_spec(name: &str) -> Option<&'static CustomGeomSpec> {
    presets().get(name)
}

#[cfg(test)]
pub(super) fn preset_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = presets().keys().map(String::as_str).collect();
    v.sort_unstable();
    v
}

/// Presets the Ecma file lacks, and the preset each one mirrors vertically. The file defines
/// `upDownArrow` twice (two identical copies) and has no `upArrow`, which is `downArrow` turned
/// upside down: same adjust names and defaults, same guides.
const MIRRORED_PRESETS: [(&str, &str); 1] = [("upArrow", "downArrow")];

/// New guides made while mirroring, one per distinct formula.
struct Mirror {
    guides: Vec<(String, String)>,
}

impl Mirror {
    fn guide(&mut self, fmla: String) -> String {
        if let Some((n, _)) = self.guides.iter().find(|(_, f)| *f == fmla) {
            return n.clone();
        }
        let name = format!("mir{}", self.guides.len());
        self.guides.push((name.clone(), fmla));
        name
    }

    /// `Y` mirrored about the middle of the box: `t + b - Y`, or `h - Y` in a path's own space.
    fn flip_y(&mut self, y: &str, path_h: f64) -> String {
        if path_h > 0.0 {
            self.guide(format!("+- {path_h} 0 {y}"))
        } else {
            self.guide(format!("+- b t {y}"))
        }
    }

    /// `-A` for an angle operand.
    fn neg(&mut self, a: &str) -> String {
        self.guide(format!("+- 0 0 {a}"))
    }
}

/// The geometry `spec` turned upside down about the middle of its box: every y operand becomes a
/// new guide (the existing guides and the adjust values are kept, so handles still work), the
/// text rectangle swaps its top and bottom, and connection and arc angles are negated.
pub(super) fn mirrored_vertically(spec: &CustomGeomSpec) -> CustomGeomSpec {
    let mut m = Mirror { guides: Vec::new() };
    let mut out = spec.clone();
    if let Some([l, t, r, b]) = &spec.text_rect {
        out.text_rect = Some([l.clone(), m.flip_y(b, 0.0), r.clone(), m.flip_y(t, 0.0)]);
    }
    for c in &mut out.connections {
        c.ang = m.neg(&c.ang);
        c.pos.1 = m.flip_y(&c.pos.1, 0.0);
    }
    for p in &mut out.paths {
        let h = p.h;
        for cmd in &mut p.cmds {
            let mut flip = |xy: &mut Xy| xy.1 = m.flip_y(&xy.1, h);
            match cmd {
                CmdSpec::Move(a) | CmdSpec::Line(a) => flip(a),
                CmdSpec::Quad(a, b) => {
                    flip(a);
                    flip(b);
                }
                CmdSpec::Cubic(a, b, c) => {
                    flip(a);
                    flip(b);
                    flip(c);
                }
                CmdSpec::Arc { st_ang, sw_ang, .. } => {
                    *st_ang = m.neg(st_ang);
                    *sw_ang = m.neg(sw_ang);
                }
                CmdSpec::Close => {}
            }
        }
    }
    out.guides.extend(m.guides);
    out
}

/// Parses the embedded file: one child element of the root per preset. **`upDownArrow` is defined
/// twice in the Ecma file; the first definition is kept** (a later one never replaces an earlier
/// entry).
fn load_presets() -> Presets {
    let bytes = PRESET_XML.trim_start_matches('\u{feff}').as_bytes();
    let mut rd = XmlReader::new(bytes);
    let mut map = Presets::new();
    let mut buf = Vec::new();
    let mut root_seen = false;
    loop {
        buf.clear();
        let Ok(ev) = rd.read_event_into(&mut buf) else {
            break;
        };
        let (e, empty) = match ev {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof | Event::End(_) => break,
            _ => continue,
        };
        if !root_seen {
            root_seen = true;
            if empty {
                break;
            }
            continue;
        }
        let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
        let mut budget = Budget::new(PRESET_NODES, PRESET_BYTES);
        match read_element(&mut rd, &e, empty, &mut budget) {
            Ok(Tree::Ok(node)) => {
                if let Some(spec) = spec_from_node(&node) {
                    map.entry(name).or_insert(spec);
                }
            }
            Ok(Tree::TooBig) => {}
            Err(_) => break,
        }
    }
    for (name, base) in MIRRORED_PRESETS {
        if map.contains_key(name) {
            continue;
        }
        if let Some(spec) = map.get(base).map(mirrored_vertically) {
            map.insert(name.to_string(), spec);
        }
    }
    map
}

/// Reads an `a:custGeom` element into a [`CustomGeomSpec`]. `None` for any other element, or when
/// it defines more than [`MAX_GUIDES`] guides. Paths and commands over the budgets are dropped
/// and the spec says so (`truncated`).
pub fn custom_from_xml(node: &Node) -> Option<CustomGeomSpec> {
    if node.name != "custGeom" {
        return None;
    }
    spec_from_node(node)
}

fn guide_list(parent: Option<&Node>, out: &mut Vec<(String, String)>, total: &mut usize) -> bool {
    let Some(p) = parent else {
        return true;
    };
    for g in p.nodes().filter(|n| n.name == "gd") {
        *total += 1;
        if *total > MAX_GUIDES {
            return false;
        }
        out.push((
            g.attr("name").unwrap_or("").to_string(),
            g.attr("fmla").unwrap_or("").to_string(),
        ));
    }
    true
}

fn xy(n: &Node) -> Xy {
    (
        n.attr("x").unwrap_or("0").to_string(),
        n.attr("y").unwrap_or("0").to_string(),
    )
}

fn num(n: &Node, a: &str) -> f64 {
    n.attr(a)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.0)
}

fn flag(n: &Node, a: &str, default: bool) -> bool {
    match n.attr(a).map(str::trim) {
        Some("0" | "false" | "off") => false,
        Some("1" | "true" | "on") => true,
        _ => default,
    }
}

fn fill_mode(s: Option<&str>) -> PathFill {
    match s.map(str::trim) {
        Some("none") => PathFill::None,
        Some("lighten") => PathFill::Lighten,
        Some("lightenLess") => PathFill::LightenLess,
        Some("darken") => PathFill::Darken,
        Some("darkenLess") => PathFill::DarkenLess,
        _ => PathFill::Norm,
    }
}

/// Reads one geometry description node (`custGeom`, or an entry of the preset file).
fn spec_from_node(n: &Node) -> Option<CustomGeomSpec> {
    let mut spec = CustomGeomSpec::default();
    let mut total = 0usize;
    if !guide_list(n.child("avLst"), &mut spec.adjusts, &mut total)
        || !guide_list(n.child("gdLst"), &mut spec.guides, &mut total)
    {
        return None;
    }
    if let Some(r) = n.child("rect") {
        let a = |k: &str, d: &str| r.attr(k).unwrap_or(d).to_string();
        spec.text_rect = Some([a("l", "l"), a("t", "t"), a("r", "r"), a("b", "b")]);
    }
    if let Some(list) = n.child("cxnLst") {
        for c in list.nodes().filter(|c| c.name == "cxn") {
            if spec.connections.len() >= MAX_PATHS {
                spec.truncated = true;
                break;
            }
            let Some(pos) = c.child("pos") else {
                continue;
            };
            spec.connections.push(ConnSpec {
                ang: c.attr("ang").unwrap_or("0").to_string(),
                pos: xy(pos),
            });
        }
    }
    let mut cmd_budget = MAX_COMMANDS;
    if let Some(list) = n.child("pathLst") {
        'paths: for p in list.nodes().filter(|p| p.name == "path") {
            if spec.paths.len() >= MAX_PATHS {
                spec.truncated = true;
                break;
            }
            let mut ps = PathSpec {
                w: num(p, "w"),
                h: num(p, "h"),
                fill: fill_mode(p.attr("fill")),
                stroke: flag(p, "stroke", true),
                extrusion_ok: flag(p, "extrusionOk", true),
                cmds: Vec::new(),
            };
            for c in p.nodes() {
                let pts: Vec<Xy> = c
                    .nodes()
                    .filter(|q| q.name == "pt")
                    .take(3)
                    .map(xy)
                    .collect();
                let cmd = match (local_of(&c.name), pts.as_slice()) {
                    ("moveTo", [a]) => CmdSpec::Move(a.clone()),
                    ("lnTo", [a]) => CmdSpec::Line(a.clone()),
                    ("quadBezTo", [a, b]) => CmdSpec::Quad(a.clone(), b.clone()),
                    ("cubicBezTo", [a, b, d]) => CmdSpec::Cubic(a.clone(), b.clone(), d.clone()),
                    ("arcTo", _) => CmdSpec::Arc {
                        wr: c.attr("wR").unwrap_or("0").to_string(),
                        hr: c.attr("hR").unwrap_or("0").to_string(),
                        st_ang: c.attr("stAng").unwrap_or("0").to_string(),
                        sw_ang: c.attr("swAng").unwrap_or("0").to_string(),
                    },
                    ("close", _) => CmdSpec::Close,
                    // a command with the wrong number of points is malformed: skipped
                    _ => continue,
                };
                if cmd_budget == 0 {
                    spec.truncated = true;
                    spec.paths.push(ps);
                    break 'paths;
                }
                cmd_budget -= 1;
                ps.cmds.push(cmd);
            }
            spec.paths.push(ps);
        }
    }
    Some(spec)
}
