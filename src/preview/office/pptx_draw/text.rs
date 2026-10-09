//! The text of a shape: `a:bodyPr` and the paragraphs of an `a:txBody`, with the full inheritance
//! of run and paragraph properties.
//!
//! # Where a property comes from
//!
//! For a run, the first of these that sets it wins:
//!
//! 1. the run's own `a:rPr`;
//! 2. the paragraph level's `a:defRPr` in, in order, the shape's own `a:lstStyle`, the layout
//!    placeholder's `a:lstStyle`, the master placeholder's `a:lstStyle`;
//! 3. the shape style's `p:style/a:fontRef` (its colour, and the minor / major font) -- the
//!    default of a shape that has a style (a text box made with a theme style), below any list
//!    style;
//! 4. the master's `p:txStyles` (`titleStyle` for title placeholders, `bodyStyle` for body-like
//!    ones, `otherStyle` for everything else, the text of a plain shape);
//! 5. the presentation's `p:defaultTextStyle`;
//! 6. the theme fonts (the minor font when nothing names one).
//!
//! Paragraph properties (`algn`, `marL`, `indent`, `lnSpc`, `spcBef`, `spcAft`, the bullet) follow
//! the same chain with the paragraph's own `a:pPr` first; each bullet property (kind, font, colour,
//! size) is looked up on its own.

use crate::preview::office::slide_draw as sd;
use crate::preview::office::slide_draw::Rgba;

use super::style::num;
use super::*;

/// Most text characters kept of one slide (over it the rest is dropped and the scene says so).
pub(super) const MAX_TEXT_CHARS: usize = 100_000;
/// Most explicit tab stops kept of one paragraph.
const MAX_TAB_STOPS: usize = 32;

/// The nodes a text body's properties are looked up in.
pub(super) struct TextChain<'a> {
    /// The `a:bodyPr` of the shape, then of the layout placeholder, then of the master's.
    pub body_pr: Vec<&'a Node>,
    /// The `a:lstStyle` of the shape (0), the layout placeholder (1), the master placeholder (2).
    pub lst: [Option<&'a Node>; 3],
    /// Which master text style applies: 0 title, 1 body, 2 other.
    pub style: usize,
    /// `p:style/a:fontRef` as a synthetic `lvl1pPr/defRPr`, if the shape has one.
    pub font_ref: Option<Node>,
}

/// A node with children (a built-up element).
fn synth(name: &str, kids: Vec<Node>) -> Node {
    Node {
        prefix: "a".into(),
        name: name.into(),
        attrs: Vec::new(),
        kids: kids.into_iter().map(Kid::N).collect(),
    }
}

impl TextChain<'_> {
    /// A `p:style/a:fontRef` as the lowest list style a shape has.
    pub(super) fn font_ref_node(style: Option<&Node>) -> Option<Node> {
        let r = style?.child("fontRef")?;
        let mut def = synth("defRPr", Vec::new());
        if let Some(c) = r.nodes().find(|c| theme::is_color_elem(&c.name)) {
            def.kids.push(Kid::N(synth("solidFill", vec![c.clone()])));
        }
        let face = match r.attr("idx") {
            Some("major") => Some("+mj-lt"),
            Some("minor") => Some("+mn-lt"),
            _ => None,
        };
        if let Some(f) = face {
            let mut latin = synth("latin", Vec::new());
            latin.attrs.push(("typeface".into(), f.into()));
            def.kids.push(Kid::N(latin));
        }
        Some(synth("lvl1pPr", vec![def]))
    }
}

/// The property nodes of paragraph level `lvl` (0-based), most specific first, each with the index
/// (0 slide, 1 layout, 2 master) of the part whose relationships its pictures refer to.
fn level_nodes<'n>(
    chain: &'n TextChain<'n>,
    tx_styles: Option<&'n Node>,
    default_text: Option<&'n Node>,
    lvl: usize,
) -> Vec<(&'n Node, usize)> {
    let key = format!("lvl{}pPr", lvl + 1);
    let mut out: Vec<(&Node, usize)> = Vec::new();
    for (i, l) in chain.lst.iter().enumerate() {
        if let Some(p) = l.and_then(|l| l.child(&key)) {
            out.push((p, i));
        }
    }
    if let Some(f) = &chain.font_ref {
        out.push((f, 0));
    }
    let style = ["titleStyle", "bodyStyle", "otherStyle"][chain.style.min(2)];
    if let Some(p) = tx_styles
        .and_then(|t| t.child(style))
        .and_then(|s| s.child(&key))
    {
        out.push((p, 2));
    }
    if let Some(p) = default_text.and_then(|t| t.child(&key)) {
        out.push((p, 0));
    }
    out
}

/// Spacing from an `lnSpc` / `spcBef` / `spcAft` element.
fn spacing(n: &Node) -> Option<sd::Spacing> {
    if let Some(p) = n.child("spcPct") {
        return Some(sd::Spacing::Pct(num(p, "val")? / 100_000.0));
    }
    n.child("spcPts")
        .and_then(|p| num(p, "val"))
        .map(|v| sd::Spacing::Pts(v / 100.0))
}

fn emu(v: f64) -> f64 {
    v.clamp(-1.0e9, 1.0e9)
}

impl Sb<'_> {
    /// The text of a shape: `None` when it shows no text at all.
    pub(super) fn text_body(&mut self, tx: &Node, chain: &TextChain<'_>) -> Option<sd::TextBody> {
        let (body, shown) = self.text_body_raw(tx, chain);
        shown.then_some(body)
    }

    /// The text of a shape and whether any of it is visible. A body with no visible text still has
    /// paragraphs with an end-of-paragraph size (an empty table cell is as high as one line of it).
    pub(super) fn text_body_raw(
        &mut self,
        tx: &Node,
        chain: &TextChain<'_>,
    ) -> (sd::TextBody, bool) {
        let bp = |name: &str| chain.body_pr.iter().find_map(|b| b.attr(name));
        let ins = |name: &str, d: f64| {
            bp(name)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map_or(d, |v| emu(v as f64))
        };
        let autofit = chain
            .body_pr
            .iter()
            .find_map(|b| {
                b.nodes().find_map(|c| match c.name.as_str() {
                    "noAutofit" => Some(sd::AutoFit::None),
                    "spAutoFit" => Some(sd::AutoFit::Shape),
                    "normAutofit" => Some(sd::AutoFit::Normal {
                        font_scale: num(c, "fontScale")
                            .map_or(1.0, |v| (v / 100_000.0).clamp(0.01, 1.0)),
                        ln_spc_reduction: num(c, "lnSpcReduction")
                            .map_or(0.0, |v| (v / 100_000.0).clamp(0.0, 1.0)),
                    }),
                    _ => None,
                })
            })
            .unwrap_or(sd::AutoFit::None);
        let mut body = sd::TextBody {
            insets: (
                ins("lIns", 91_440.0),
                ins("tIns", 45_720.0),
                ins("rIns", 91_440.0),
                ins("bIns", 45_720.0),
            ),
            anchor: match bp("anchor") {
                Some("ctr") => sd::Anchor::Middle,
                Some("b") => sd::Anchor::Bottom,
                _ => sd::Anchor::Top,
            },
            anchor_ctr: bp("anchorCtr").is_some_and(|v| matches!(v.trim(), "1" | "true")),
            wrap: bp("wrap") != Some("none"),
            vert: match bp("vert") {
                Some("vert" | "mongolianVert") => sd::Vert::Vert,
                Some("vert270") => sd::Vert::Vert270,
                Some("eaVert" | "wordArtVert" | "wordArtVertRtl") => sd::Vert::EaVert,
                _ => sd::Vert::Horz,
            },
            autofit,
            rot_deg: bp("rot")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map_or(0.0, |v| v as f64 / 60_000.0),
            upright: bp("upright").is_some_and(|v| matches!(v.trim(), "1" | "true")),
            columns: bp("numCol")
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(1)
                .clamp(1, 16),
            paragraphs: Vec::new(),
        };
        let mut shown = false;
        for p in tx.nodes().filter(|n| n.name == "p") {
            if self.chars >= MAX_TEXT_CHARS {
                self.truncated = true;
                break;
            }
            let para = self.paragraph(p, chain);
            shown |= para
                .runs
                .iter()
                .any(|r| r.kind != sd::RunKind::LineBreak && !r.text.trim().is_empty());
            body.paragraphs.push(para);
        }
        (body, shown)
    }

    fn paragraph(&mut self, p: &Node, chain: &TextChain<'_>) -> sd::Paragraph {
        let ppr = p.child("pPr");
        let lvl = ppr
            .and_then(|n| num(n, "lvl"))
            .map_or(0, |v| v.clamp(0.0, 8.0) as usize);
        let tx_styles = self.tx_styles;
        let default_text = self.default_text;
        let levels = level_nodes(chain, tx_styles, default_text, lvl);
        // Paragraph-level lookup list: the paragraph's own `pPr`, then the levels.
        let mut plist: Vec<(&Node, usize)> = Vec::with_capacity(levels.len() + 1);
        if let Some(n) = ppr {
            plist.push((n, 0));
        }
        plist.extend(levels.iter().copied());
        let pattr = |name: &str| plist.iter().find_map(|(n, _)| n.attr(name));
        let pnum = |name: &str| pattr(name).and_then(|v| v.trim().parse::<i64>().ok());
        let pspace = |name: &str| {
            plist
                .iter()
                .find_map(|(n, _)| n.child(name).and_then(spacing))
        };

        let defs: Vec<&Node> = plist
            .iter()
            .filter_map(|(n, _)| n.child("defRPr"))
            .collect();
        let mut para = sd::Paragraph {
            align: match pattr("algn") {
                Some("ctr") => sd::Align::Center,
                Some("r") => sd::Align::Right,
                Some("just" | "justLow") => sd::Align::Justify,
                Some("dist" | "thaiDist") => sd::Align::Distributed,
                _ => sd::Align::Left,
            },
            level: lvl as u8,
            mar_l: emu(pnum("marL").unwrap_or(0) as f64),
            indent: emu(pnum("indent").unwrap_or(0) as f64),
            spc_before: pspace("spcBef").unwrap_or(sd::Spacing::Pts(0.0)),
            spc_after: pspace("spcAft").unwrap_or(sd::Spacing::Pts(0.0)),
            line_spacing: pspace("lnSpc").unwrap_or(sd::Spacing::Pct(1.0)),
            rtl: pattr("rtl").is_some_and(|v| matches!(v.trim(), "1" | "true")),
            def_tab: pnum("defTabSz")
                .filter(|v| *v > 0)
                .map_or(sd::DEFAULT_TAB_EMU, |v| emu(v as f64)),
            tabs: plist
                .iter()
                .find_map(|(n, _)| n.child("tabLst"))
                .map(|l| {
                    l.nodes()
                        .filter(|t| t.name == "tab")
                        .take(MAX_TAB_STOPS)
                        .filter_map(|t| {
                            Some(sd::TabStop {
                                pos: emu(t.attr("pos")?.trim().parse::<i64>().ok()? as f64),
                                align: match t.attr("algn") {
                                    Some("ctr") => sd::TabAlign::Center,
                                    Some("r") => sd::TabAlign::Right,
                                    Some("dec") => sd::TabAlign::Decimal,
                                    _ => sd::TabAlign::Left,
                                },
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ..sd::Paragraph::default()
        };
        para.bullet = self.bullet(&plist);

        for n in p.nodes() {
            match n.name.as_str() {
                "r" | "fld" => {
                    let rpr = n.child("rPr");
                    let mut rprs: Vec<&Node> = Vec::with_capacity(defs.len() + 1);
                    rprs.extend(rpr);
                    rprs.extend(defs.iter().copied());
                    let mut run = self.run_props(&rprs);
                    if rpr.is_some_and(|r| r.child("hlinkClick").is_some()) {
                        // A hyperlink takes the theme's link colour and an underline unless the
                        // run states a colour of its own.
                        let explicit = rpr.is_some_and(|r| {
                            r.nodes()
                                .any(|c| matches!(c.name.as_str(), "solidFill" | "gradFill"))
                        });
                        if !explicit {
                            if let Some(c) = self.col.scheme("hlink", None) {
                                run.fill = sd::Fill::Solid(c);
                            }
                        }
                        run.underline = sd::Underline::Single;
                    }
                    self.add_text_runs(n, run, &mut para.runs);
                }
                "br" => {
                    let mut rprs: Vec<&Node> = Vec::new();
                    rprs.extend(n.child("rPr"));
                    rprs.extend(defs.iter().copied());
                    let mut run = self.run_props(&rprs);
                    run.kind = sd::RunKind::LineBreak;
                    para.runs.push(run);
                }
                _ => {}
            }
        }
        let mut rprs: Vec<&Node> = Vec::new();
        rprs.extend(p.child("endParaRPr"));
        rprs.extend(defs.iter().copied());
        para.end_size_pt = self.run_props(&rprs).size_pt;
        para
    }

    /// The text runs of an `a:r` / `a:fld` (a vertical tab inside the text is a line break).
    fn add_text_runs(&mut self, n: &Node, proto: sd::Run, out: &mut Vec<sd::Run>) {
        let is_field = n.name == "fld";
        let slidenum = is_field && n.attr("type").is_some_and(|t| t.trim() == "slidenum");
        let kind = if is_field {
            sd::RunKind::Field
        } else {
            sd::RunKind::Text
        };
        if slidenum {
            self.slide_dep = true;
            let num = self.first_num.saturating_add(self.slide_no as i64 - 1);
            self.chars += 8;
            out.push(sd::Run {
                text: num.to_string(),
                kind,
                ..proto
            });
            return;
        }
        for t in n.nodes().filter(|c| c.name == "t") {
            let text = t.text();
            for (i, part) in text.split('\u{B}').enumerate() {
                if i > 0 {
                    out.push(sd::Run {
                        kind: sd::RunKind::LineBreak,
                        text: String::new(),
                        ..proto.clone()
                    });
                }
                // Tabs stay (the layout moves to the next tab stop); the text view's cleaning
                // would turn them into spaces.
                let part = part.split('\t').map(clean).collect::<Vec<_>>().join("\t");
                if part.is_empty() {
                    continue;
                }
                if self.chars.saturating_add(part.chars().count()) > MAX_TEXT_CHARS {
                    self.truncated = true;
                    self.chars = MAX_TEXT_CHARS;
                    return;
                }
                self.chars += part.chars().count();
                out.push(sd::Run {
                    text: part,
                    kind,
                    ..proto.clone()
                });
            }
        }
    }

    /// The run properties from a lookup list (the run's `rPr` first, then the level `defRPr`s).
    fn run_props(&self, rprs: &[&Node]) -> sd::Run {
        let attr = |name: &str| rprs.iter().find_map(|n| n.attr(name));
        let on = |name: &str| attr(name).is_some_and(|v| matches!(v.trim(), "1" | "true"));
        let size = attr("sz")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map_or(18.0, |v| (v / 100.0).clamp(0.5, 4000.0));
        let lang = attr("lang").map(|l| l.trim().to_string());
        let font = self.font_spec(rprs, lang.as_deref());
        sd::Run {
            text: String::new(),
            kind: sd::RunKind::Text,
            font,
            size_pt: size,
            bold: on("b"),
            italic: on("i"),
            underline: match attr("u") {
                None | Some("none") => sd::Underline::None,
                Some("dbl") => sd::Underline::Double,
                Some(_) => sd::Underline::Single,
            },
            strike: match attr("strike") {
                Some("sngStrike") => sd::Strike::Single,
                Some("dblStrike") => sd::Strike::Double,
                _ => sd::Strike::None,
            },
            fill: self.run_fill(rprs),
            highlight: rprs
                .iter()
                .find_map(|n| n.child("highlight"))
                .and_then(|h| self.col.first_in(h, None)),
            baseline_pct: attr("baseline")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map_or(0.0, |v| (v / 1000.0).clamp(-100.0, 100.0)),
            spacing_pt: attr("spc")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map_or(0.0, |v| (v / 100.0).clamp(-1000.0, 1000.0)),
            caps: match attr("cap") {
                Some("all") => sd::Caps::All,
                Some("small") => sd::Caps::Small,
                _ => sd::Caps::None,
            },
            lang,
        }
    }

    /// The text colour: the first lookup node with a fill; the default is the text colour `tx1`.
    fn run_fill(&self, rprs: &[&Node]) -> sd::Fill {
        for n in rprs {
            let Some(f) = n.nodes().find(|c| {
                matches!(
                    c.name.as_str(),
                    "noFill" | "solidFill" | "gradFill" | "pattFill"
                )
            }) else {
                continue;
            };
            return match f.name.as_str() {
                "noFill" => sd::Fill::None,
                "solidFill" => self
                    .col
                    .first_in(f, None)
                    .map_or(sd::Fill::None, sd::Fill::Solid),
                "pattFill" => f
                    .child("fgClr")
                    .and_then(|c| self.col.first_in(c, None))
                    .map_or(sd::Fill::None, sd::Fill::Solid),
                // A gradient on text is drawn with its first colour.
                _ => f
                    .child("gsLst")
                    .and_then(|l| l.child("gs"))
                    .and_then(|gs| self.col.first_in(gs, None))
                    .map_or(sd::Fill::None, sd::Fill::Solid),
            };
        }
        self.col
            .scheme("tx1", None)
            .map_or(sd::Fill::Solid(Rgba::BLACK), sd::Fill::Solid)
    }

    /// A typeface name with a theme reference (`+mj-lt`, `+mn-ea` ..) replaced by the theme's font;
    /// `None` when the theme names none for it.
    fn resolve_typeface(&self, tf: &str) -> Option<String> {
        let th = self.theme;
        let (fonts, kind) = match tf.get(..4) {
            Some("+mj-") => (&th.major, tf.get(4..)?),
            Some("+mn-") => (&th.minor, tf.get(4..)?),
            _ => return Some(tf.to_string()),
        };
        let f = match kind {
            "lt" => &fonts.latin,
            "ea" => &fonts.ea,
            "cs" => &fonts.cs,
            _ => return None,
        };
        (!f.is_empty()).then(|| f.clone())
    }

    /// The typefaces of a run: the first named one, theme references (`+mj-lt` ..) resolved.
    fn font_spec(&self, rprs: &[&Node], lang: Option<&str>) -> sd::FontSpec {
        let th = self.theme;
        let named = |kind: &str| {
            rprs.iter()
                .filter_map(|n| n.child(kind))
                .filter_map(|f| f.attr("typeface"))
                .map(str::trim)
                .find(|t| !t.is_empty())
        };
        let resolve = |tf: &str| self.resolve_typeface(tf);
        let by_script = |fonts: &theme::ThemeFonts, scripts: &[&str]| -> Option<String> {
            scripts.iter().find_map(|s| {
                fonts
                    .scripts
                    .iter()
                    .find(|(k, v)| k == s && !v.is_empty())
                    .map(|(_, v)| v.clone())
            })
        };
        let latin = named("latin")
            .and_then(resolve)
            .or_else(|| Some(th.minor.latin.clone()).filter(|f| !f.is_empty()));
        let script: &[&str] = match lang.map(|l| l.to_ascii_lowercase()) {
            Some(l) if l.starts_with("ja") => &["Jpan"],
            Some(l) if l.starts_with("ko") => &["Hang"],
            Some(l)
                if l.starts_with("zh-tw") || l.starts_with("zh-hk") || l.starts_with("zh-mo") =>
            {
                &["Hant"]
            }
            Some(l) if l.starts_with("zh") => &["Hans"],
            _ => &[],
        };
        let east_asian = named("ea")
            .and_then(resolve)
            .or_else(|| {
                if named("ea").is_none() {
                    Some(th.minor.ea.clone()).filter(|f| !f.is_empty())
                } else {
                    None
                }
            })
            .or_else(|| by_script(&th.minor, script));
        let complex = named("cs")
            .and_then(resolve)
            .or_else(|| Some(th.minor.cs.clone()).filter(|f| !f.is_empty()));
        sd::FontSpec {
            latin,
            east_asian,
            complex,
            symbol: named("sym").map(str::to_string),
        }
    }

    /// The bullet of a paragraph: each property (kind, font, colour, size) is the first the lookup
    /// list sets; `None` when the kind is `buNone` or nothing sets one.
    fn bullet(&mut self, plist: &[(&Node, usize)]) -> Option<sd::Bullet> {
        let find = |names: &[&str]| -> Option<(&Node, usize)> {
            plist.iter().find_map(|(n, i)| {
                n.nodes()
                    .find(|c| names.contains(&c.name.as_str()))
                    .map(|c| (c, *i))
            })
        };
        let (k, part) = find(&["buNone", "buChar", "buAutoNum", "buBlip"])?;
        let kind = match k.name.as_str() {
            "buChar" => {
                let ch = k.attr("char")?;
                if ch.is_empty() {
                    return None;
                }
                sd::BulletKind::Char(ch.chars().take(4).collect())
            }
            "buAutoNum" => sd::BulletKind::AutoNum {
                scheme: k.attr("type").unwrap_or("arabicPeriod").to_string(),
                start: num(k, "startAt").map_or(1, |v| v.clamp(1.0, 999_999.0) as u32),
            },
            "buBlip" => {
                let rels = self.rels[part.min(2)];
                let blip = k.child("blip")?;
                let key = self.blip_key(blip, rels)?;
                sd::BulletKind::Picture(sd::ImageFill::stretch(key))
            }
            _ => return None,
        };
        let font = match find(&["buFont", "buFontTx"]) {
            Some((f, _)) if f.name == "buFont" => f
                .attr("typeface")
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .and_then(|t| self.resolve_typeface(t))
                .map(|t| sd::FontSpec {
                    latin: Some(t),
                    ..sd::FontSpec::default()
                }),
            _ => None,
        };
        let color = match find(&["buClr", "buClrTx"]) {
            Some((c, _)) if c.name == "buClr" => self.col.first_in(c, None),
            _ => None,
        };
        let size = match find(&["buSzPct", "buSzPts", "buSzTx"]) {
            Some((s, _)) if s.name == "buSzPct" => {
                sd::BulletSize::Pct(num(s, "val").map_or(1.0, |v| (v / 100_000.0).clamp(0.05, 4.0)))
            }
            Some((s, _)) if s.name == "buSzPts" => {
                sd::BulletSize::Pts(num(s, "val").map_or(12.0, |v| (v / 100.0).clamp(1.0, 1000.0)))
            }
            _ => sd::BulletSize::FollowText,
        };
        Some(sd::Bullet {
            kind,
            font,
            color,
            size,
        })
    }
}
