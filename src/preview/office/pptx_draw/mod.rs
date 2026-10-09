//! A slide of a PowerPoint deck as a drawing model ([`sd::SlideScene`]), built in the same pass
//! that reads the deck for the text Markdown (`pptx.rs`, `Rd`): every part is read once, under the
//! same read budgets, and the scene is built from the trees already in memory.
//!
//! What is read (ECMA-376 Part 1, DrawingML and PresentationML):
//!
//! * the theme (colour scheme, font scheme, format scheme), the master's `p:clrMap` and the
//!   layout's and slide's `p:clrMapOvr` ([`theme`]);
//! * the background (slide `p:bg`, else layout, else master; `bgPr` fill or `bgRef` into the theme);
//! * the shapes of the master, then the layout (each only when `showMasterSp` of the layout /
//!   slide does not hide them; their placeholders are never drawn), then the slide's own, with a
//!   slide placeholder inheriting `spPr`, `bodyPr` and `lstStyle` from the layout's and the
//!   master's placeholder (found by `Inherit`, the matcher of the text view);
//! * text with the full property chain ([`text`]), fills, outlines and effects from `spPr` and the
//!   shape style ([`style`]), pictures, groups, connectors, embedded objects ([`shapes`]).
//!
//! Not drawn yet (owned by later tasks, each with one function where it plugs in): preset shapes
//! other than a rectangle, an ellipse and a straight line, and custom geometry ([`geom`]); tables,
//! charts and SmartArt (`Sb::frame_table`, `frame_chart`, `frame_diagram` in [`shapes`]).
//!
//! # Budgets
//!
//! Items per slide ([`DocOptions::max_slide_shapes`], master and layout shapes included), group
//! nesting ([`MAX_GROUP_DEPTH`]), text characters per slide ([`text::MAX_TEXT_CHARS`]), gradient
//! stops ([`style::MAX_GRAD_STOPS`]), custom dash entries ([`style::MAX_CUST_DASH`]), colour
//! transforms ([`theme::MAX_COLOR_MODS`]), adjust values of a preset, and the shapes kept of a
//! layout or master ([`MAX_PART_SHAPES`]). Going over any of them sets
//! [`sd::SlideScene::truncated`] (and [`Document::truncated`]); nothing is dropped silently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::preview::office::slide_draw as sd;
use crate::preview::office::slide_draw::Rgba;

use super::*;

mod frames;
mod geom;
mod shapes;
mod style;
mod table;
mod text;
pub(super) mod theme;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_f2_dump;
#[cfg(test)]
mod tests_f3_dump;
#[cfg(test)]
mod tests_frames;
#[cfg(test)]
mod tests_frames_dump;
#[cfg(test)]
mod tests_g1;

pub(super) use table::TableStyles;
pub(super) use theme::Theme;

use theme::{ClrMap, Colors};

/// Most shapes kept of one layout or master (a forged one must not hold unbounded memory for the
/// whole conversion).
pub(super) const MAX_PART_SHAPES: usize = 2_000;

/// How the scene builder reads further parts of the package (a chart, a SmartArt data model and
/// drawing): through the deck reader, so they are read once, under the same per-part and total
/// read budgets as every other part (a part over a budget reads as `None`, and the reader marks
/// the document truncated).
pub(super) struct Parts<'a> {
    /// The bytes of a package part, or `None` (missing, unreadable, over a budget).
    pub read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    /// The relationships of a part (empty when it has none).
    pub rels: &'a mut dyn FnMut(&str) -> HashMap<String, Rel>,
}

/// What the scene builder needs of the deck as a whole.
#[derive(Default)]
pub(super) struct DeckDraw {
    /// One scene per slide written so far (hidden ones included), in order.
    pub scenes: Vec<sd::SlideScene>,
    /// `p:defaultTextStyle` of the presentation.
    pub default_text: Option<Node>,
    /// `ppt/tableStyles.xml` (the styles a table's `a:tableStyleId` names).
    pub table_styles: Option<TableStyles>,
}

/// Everything about one slide the builder reads.
pub(super) struct SceneInput<'a> {
    pub opts: &'a DocOptions,
    /// The slide size (EMU).
    pub size: (f64, f64),
    /// The slide's shape nodes, `p:bg`, `p:clrMapOvr` and `showMasterSp`.
    pub nodes: &'a [Node],
    pub bg: Option<&'a Node>,
    pub clr_ovr: Option<&'a Node>,
    pub show_master_sp: bool,
    /// Shapes of the slide were left unread by the loader (the shape cap or the node budget).
    pub cut: bool,
    pub rels: &'a HashMap<String, Rel>,
    pub inh: &'a Inherit,
    pub default_text: Option<&'a Node>,
    pub table_styles: Option<&'a TableStyles>,
    /// What the first slide's number field shows (`firstSlideNum`), and this slide's place.
    pub first_num: i64,
    pub number: usize,
    pub parts: Parts<'a>,
}

/// The state of building one scene.
pub(super) struct Sb<'a> {
    col: Colors<'a>,
    pub theme: &'a Theme,
    /// The master's colour map: what a chart maps its colours by unless the chart says otherwise.
    master_map: &'a ClrMap,
    /// Loads the picture at a package part into the document and returns its `office-img://` key.
    pub loader: &'a mut dyn FnMut(&str) -> Option<String>,
    /// Reads further parts (charts, SmartArt).
    pub parts: Parts<'a>,
    /// Relationships that exist only while one SmartArt drawing is built: the ids its pictures
    /// were given in place of the drawing part's own (see `frames`).
    pub extra_rels: HashMap<String, Rel>,
    /// The relationships of the slide, the layout and the master.
    pub rels: [&'a HashMap<String, Rel>; 3],
    /// The part whose shapes are being built (an index into `rels`).
    pub cur: usize,
    /// Placeholder inheritance, for the slide's own shapes only.
    pub inh: Option<&'a Inherit>,
    pub default_text: Option<&'a Node>,
    pub table_styles: Option<&'a TableStyles>,
    pub tx_styles: Option<&'a Node>,
    pub first_num: i64,
    pub slide_no: usize,
    pub size: (f64, f64),
    pub items: usize,
    pub max_items: usize,
    pub chars: usize,
    pub truncated: bool,
    /// The fill `a:grpFill` of the group being built stands for.
    pub group_fills: Vec<Option<sd::Fill>>,
    /// The child-space maps of the groups being built, innermost last.
    pub group_maps: Vec<GroupMap>,
}

/// The map from a group's child space onto its box: `p -> to + (p - off) * scale` per axis.
///
/// Members are placed through it when they are built (so their boxes, preset geometry, text box
/// and wrapping are those of the final size); only boxes are scaled, never line widths, text sizes
/// or effects, which DrawingML keeps at their own size (a scale in the drawing would multiply
/// them).
#[derive(Debug, Clone, Copy)]
pub(super) struct GroupMap {
    off: (f64, f64),
    to: (f64, f64),
    scale: (f64, f64),
}

impl GroupMap {
    /// The map of a group with child space `child_off` / `child_ext` (positive extents) onto the
    /// box `to`.
    pub(super) fn new(child_off: (f64, f64), child_ext: (f64, f64), to: &sd::Xfrm) -> GroupMap {
        GroupMap {
            off: child_off,
            to: (to.x, to.y),
            scale: (to.w / child_ext.0, to.h / child_ext.1),
        }
    }

    /// A map that leaves boxes where they are (the drawing of a SmartArt frame is in the frame's
    /// own space).
    pub(super) fn identity() -> GroupMap {
        GroupMap {
            off: (0.0, 0.0),
            to: (0.0, 0.0),
            scale: (1.0, 1.0),
        }
    }
}

impl Sb<'_> {
    /// A member's box in the child space of the group being built, as a box in the space of that
    /// group's parent; rotation and flips stay as they are.
    pub(super) fn map_xfrm(&self, x: sd::Xfrm) -> sd::Xfrm {
        let Some(m) = self.group_maps.last() else {
            return x;
        };
        let lim = |v: f64| v.clamp(-4.0e9, 4.0e9);
        sd::Xfrm {
            x: lim(m.to.0 + (x.x - m.off.0) * m.scale.0),
            y: lim(m.to.1 + (x.y - m.off.1) * m.scale.1),
            w: lim(x.w * m.scale.0).max(0.0),
            h: lim(x.h * m.scale.1).max(0.0),
            ..x
        }
    }
}

/// Builds the scene of a slide.
pub(super) fn build_scene(
    inp: SceneInput<'_>,
    loader: &mut dyn FnMut(&str) -> Option<String>,
) -> sd::SlideScene {
    let inh = inp.inh;
    let default_theme = Theme::default();
    let theme: &Theme = inh.master.theme.as_deref().unwrap_or(&default_theme);
    // The colour map: the master's, overridden by the layout's and then the slide's.
    let master_map = inh
        .master
        .clr
        .as_ref()
        .map(ClrMap::from_node)
        .unwrap_or_default();
    let layout_map = ClrMap::with_override(&master_map, inh.layout.clr.as_ref());
    let map = ClrMap::with_override(&layout_map, inp.clr_ovr);
    let mut sb = Sb {
        col: Colors { theme, map: &map },
        master_map: &master_map,
        theme,
        loader,
        parts: Parts {
            read: &mut *inp.parts.read,
            rels: &mut *inp.parts.rels,
        },
        extra_rels: HashMap::new(),
        rels: [inp.rels, &inh.layout.rels, &inh.master.rels],
        cur: 0,
        inh: None,
        default_text: inp.default_text,
        table_styles: inp.table_styles,
        tx_styles: inh.master.tx_styles.as_ref(),
        first_num: inp.first_num,
        slide_no: inp.number,
        size: inp.size,
        items: 0,
        max_items: inp.opts.max_slide_shapes,
        chars: 0,
        truncated: inp.cut || inh.layout.cut || inh.master.cut,
        group_fills: Vec::new(),
        group_maps: Vec::new(),
    };
    let mut scene = sd::SlideScene {
        width: inp.size.0,
        height: inp.size.1,
        ..sd::SlideScene::default()
    };
    scene.background = sb.background(inp.bg, inh.layout.bg.as_ref(), inh.master.bg.as_ref());
    // Back to front: the master's shapes, the layout's, the slide's.
    if inp.show_master_sp && inh.layout.show_master_sp {
        sb.cur = 2;
        sb.build_nodes(
            inh.master
                .nodes
                .iter()
                .filter(|n| !shapes::is_placeholder(n)),
            0,
            &mut scene.items,
        );
    }
    if inp.show_master_sp {
        sb.cur = 1;
        sb.build_nodes(
            inh.layout
                .nodes
                .iter()
                .filter(|n| !shapes::is_placeholder(n)),
            0,
            &mut scene.items,
        );
    }
    sb.cur = 0;
    sb.inh = Some(inh);
    sb.build_nodes(inp.nodes.iter(), 0, &mut scene.items);
    scene.truncated = sb.truncated;
    scene
}

impl Sb<'_> {
    /// The background fill: the slide's `p:bg`, else the layout's, else the master's; the
    /// background colour `bg1` when none says anything.
    fn background(
        &mut self,
        slide: Option<&Node>,
        layout: Option<&Node>,
        master: Option<&Node>,
    ) -> sd::Fill {
        for (bg, part) in [(slide, 0usize), (layout, 1), (master, 2)] {
            let Some(bg) = bg else { continue };
            let rels = self.rels[part];
            if let Some(pr) = bg.child("bgPr") {
                if let Some(style::FillRes::Set(f)) = self.fill_in(pr, None, rels) {
                    return f;
                }
            }
            if let Some(r) = bg.child("bgRef") {
                let ph = self.col.first_in(r, None);
                let idx = style::num(r, "idx").unwrap_or(0.0) as usize;
                if let Some(f) = self.theme_fill(idx, ph) {
                    return f;
                }
            }
        }
        self.col
            .scheme("bg1", None)
            .map_or(sd::Fill::Solid(Rgba::WHITE), sd::Fill::Solid)
    }
}

// ---------------------------------------------------------------------------------------------
// the picture view of the deck
// ---------------------------------------------------------------------------------------------

/// Counts the decks converted in this process (see [`slide_keys`]).
static DECK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The picture URLs of the `count` slides of one converted deck: `office-img://<12 hex>/slide-<n>.svg`
/// (`n` from 1). The 12 hex digits differ for every call in this process (the call number goes
/// through an odd multiplier and an xor with a per-process salt: both are bijections of 48 bits),
/// so two decks, or two loads of one file, never share a cached picture.
pub(in crate::preview::office::docx) fn slide_keys(count: usize) -> Vec<String> {
    static SALT: OnceLock<u64> = OnceLock::new();
    let salt = *SALT.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        nanos ^ (u64::from(std::process::id()) << 32)
    });
    let n = DECK_COUNTER.fetch_add(1, Ordering::Relaxed);
    let id = (n.wrapping_mul(0x9E37_79B9_7F4B_7C15) ^ salt) & 0xFFFF_FFFF_FFFF;
    (1..=count)
        .map(|i| format!("office-img://{id:012x}/slide-{i}.svg"))
        .collect()
}

/// A presentation's picture view as Markdown, derived from its text Markdown: for each slide the
/// same level-2 heading line as in `markdown` (the very text), a blank line, `![<alt>](<key>)`, a
/// blank line, and the slide's notes exactly as `markdown` has them (the quote block that ends
/// the slide's section). Shared with the OpenDocument reader.
///
/// `slides` and `keys` are in the order of the slides. Returns the empty string (the app then
/// shows `markdown`) when the Markdown does not hold exactly one level-2 heading per slide, which
/// the readers guarantee it does.
pub(in crate::preview::office::docx) fn picture_markdown(
    markdown: &str,
    slides: &[SlideInfo],
    keys: &[String],
    lang: crate::i18n::Lang,
) -> String {
    if slides.len() != keys.len() {
        return String::new();
    }
    let lines: Vec<&str> = markdown.split('\n').collect();
    let heads: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("## "))
        .map(|(i, _)| i)
        .collect();
    if heads.len() != slides.len() {
        return String::new();
    }
    let notes_start = format!("> **{}**", tr(lang, Msg::SlideNotesLabel));
    let mut out = String::new();
    for (k, (&h, slide)) in heads.iter().zip(slides).enumerate() {
        let end = heads.get(k + 1).copied().unwrap_or(lines.len());
        let section = &lines[h + 1..end];
        // The notes are the quote block that starts a paragraph and runs to the section's end.
        let notes = section
            .iter()
            .enumerate()
            .rev()
            .find(|(i, l)| {
                l.starts_with(&notes_start) && (*i == 0 || section[*i - 1].trim().is_empty())
            })
            .map(|(i, _)| section[i..].join("\n").trim_end().to_string());
        let mut alt = format!("{} {}", tr(lang, Msg::SlideHeading), slide.number);
        if !slide.title.is_empty() {
            alt.push_str(": ");
            alt.push_str(&slide.title);
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(lines[h]);
        out.push_str("\n\n");
        out.push_str(&format!("![{}]({})", md_alt(&alt), keys[k]));
        if let Some(n) = notes {
            out.push_str("\n\n");
            out.push_str(&n);
        }
    }
    out
}
