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
//! Also drawn: preset shapes and custom geometry ([`geom`]), tables ([`table`]), charts and
//! SmartArt (`frames`). Pictures and OLE objects that have no drawing of their own are drawn from
//! the picture PowerPoint stores beside them.
//!
//! # Shared drawings
//!
//! The shapes of a master and of a layout are the same for every slide that shows them, so they
//! are built once per deck ([`Shared`]) and each slide's scene holds the lists
//! ([`sd::SlideScene::underlay`]). A top-level shape that shows the slide's own number is built
//! again for every slide and takes its place between the shared stretches. A slide's
//! `p:clrMapOvr` recolours the shapes of its master and layout, so a drawing is shared only among
//! slides seen through the same colour map.
//!
//! # Budgets
//!
//! Items per slide ([`DocOptions::max_slide_shapes`], master and layout shapes included), items
//! over the whole deck ([`DocOptions::max_deck_items`]: a master's list counts once, the slides'
//! own items each) and the estimated bytes they hold ([`DocOptions::max_deck_bytes`]; the
//! per-slide copies of a master's shapes that show the slide's number have a pool of their own,
//! [`DocOptions::max_copy_bytes`], so they cannot starve the slides' content), the XML of the
//! layouts and masters kept parsed ([`DocOptions::max_master_xml`]), group nesting ([`MAX_GROUP_DEPTH`]), text characters per slide ([`text::MAX_TEXT_CHARS`]), gradient
//! stops ([`style::MAX_GRAD_STOPS`]), custom dash entries ([`style::MAX_CUST_DASH`]), colour
//! transforms ([`theme::MAX_COLOR_MODS`]), adjust values of a preset, and the shapes kept of a
//! layout or master ([`MAX_PART_SHAPES`]). Going over any of them sets
//! [`sd::SlideScene::truncated`] (and [`Document::truncated`]); nothing is dropped silently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::preview::office::slide_draw as sd;
use crate::preview::office::slide_draw::underlay::{DeckItems, PartBuilder, PartDraw, Seg};
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
#[cfg(test)]
mod tests_names;
#[cfg(test)]
mod tests_shared;

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
    /// What the slides share: the drawings of the masters and layouts, the deck's item budget.
    pub shared: Shared,
}

/// The drawings of a deck's masters and layouts, built once, and the items left to the deck.
#[derive(Default)]
pub(super) struct Shared {
    /// Per part (kind, `PartInfo::uid`): its drawing under each colour map it was built with (a
    /// slide's `p:clrMapOvr` recolours the master's and the layout's shapes too).
    parts: HashMap<(u8, u64), Vec<(ClrMap, PartDraw)>>,
    /// `None` until the first slide.
    items: Option<DeckItems>,
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
    pub shared: &'a mut Shared,
    pub parts: Parts<'a>,
}

/// The kinds of part whose drawing the slides share (a master's, a layout's).
const PART_MASTER: u8 = 0;
const PART_LAYOUT: u8 = 1;

/// One top-level shape of a master or layout, built.
struct Built {
    items: Vec<sd::Item>,
    /// It shows the slide's number: it belongs to the slide it was built for.
    dep: bool,
    /// What the reader counted for it (shapes, text characters).
    count: usize,
    chars: usize,
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
    /// The reader's options (a chart's theme override is parsed under the same node budget).
    pub opts: &'a DocOptions,
    pub items: usize,
    pub max_items: usize,
    pub chars: usize,
    pub truncated: bool,
    /// A slide number field was built (the part being built shows this slide's number, so its
    /// drawing is the slide's own and cannot be shared).
    pub slide_dep: bool,
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
    /// The map of a group with child space `child_off` / `child_ext` onto the box `to`. An axis
    /// whose child extent is not a positive number (a group that states none and has no size
    /// either) cannot be scaled: its members keep their size there (scale 1) and only move with the
    /// group's offset. (A group with a zero size of its own over a real child extent is scale 0:
    /// all its members collapse onto the group's line, as PowerPoint draws it.)
    pub(super) fn new(child_off: (f64, f64), child_ext: (f64, f64), to: &sd::Xfrm) -> GroupMap {
        let axis = |size: f64, child: f64| {
            if child.is_finite() && child > 0.0 {
                let k = size / child;
                if k.is_finite() {
                    k
                } else {
                    1.0
                }
            } else {
                1.0
            }
        };
        GroupMap {
            off: child_off,
            to: (to.x, to.y),
            scale: (axis(to.w, child_ext.0), axis(to.h, child_ext.1)),
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
        opts: inp.opts,
        items: 0,
        max_items: inp.opts.max_slide_shapes,
        chars: 0,
        truncated: inp.cut || inh.layout.cut || inh.master.cut,
        slide_dep: false,
        group_fills: Vec::new(),
        group_maps: Vec::new(),
    };
    let mut scene = sd::SlideScene {
        width: inp.size.0,
        height: inp.size.1,
        ..sd::SlideScene::default()
    };
    scene.background = sb.background(inp.bg, inh.layout.bg.as_ref(), inh.master.bg.as_ref());
    // Back to front: the master's shapes, the layout's, the slide's. The first two are the same
    // drawing for every slide that shows them: built once per deck (under the colour map they are
    // seen through) and shared; only a shape that shows this slide's number is built again.
    let deck = inp.shared.items.get_or_insert_with(|| {
        DeckItems::with_bytes(
            inp.opts.max_deck_items,
            inp.opts.max_deck_bytes,
            inp.opts.max_copy_bytes,
        )
    });
    let master_shown = inp.show_master_sp && inh.layout.show_master_sp;
    for (kind, shown, part) in [
        (PART_MASTER, master_shown, &*inh.master),
        (PART_LAYOUT, inp.show_master_sp, &inh.layout),
    ] {
        if !shown {
            continue;
        }
        let cached = (part.uid != 0)
            .then(|| {
                inp.shared
                    .parts
                    .get(&(kind, part.uid))
                    .and_then(|v| v.iter().find(|(m, _)| *m == map))
                    .map(|(_, d)| d.clone())
            })
            .flatten();
        let (draw, new) = match cached {
            Some(d) => (d, false),
            None => (PartDraw::default(), true),
        };
        let draw = sb.part_segments(kind, part, draw, new, deck, &mut scene.underlay);
        if let (true, Some(d)) = (new && part.uid != 0, draw) {
            inp.shared
                .parts
                .entry((kind, part.uid))
                .or_default()
                .push((map.clone(), d));
        }
    }
    sb.cur = 0;
    sb.inh = Some(inh);
    let base_items = sb.items;
    sb.max_items = inp
        .opts
        .max_slide_shapes
        .min(base_items.saturating_add(deck.left()));
    sb.build_nodes(inp.nodes.iter(), 0, &mut scene.items);
    sd::footprint::compact(&mut scene.items);
    deck.spend(
        sb.items.saturating_sub(base_items),
        sd::footprint::items_bytes(&scene.items),
    );
    scene.truncated = sb.truncated;
    scene
}

impl Sb<'_> {
    /// Adds the drawing of a master or a layout to the slide's `underlay`, back to front.
    ///
    /// With `new` false, `draw` is what the first slide that showed the part built: its shared
    /// lists are taken as they are and the shapes that show the slide's number are built again.
    /// With `new` true the part is built shape by shape (placeholders are never drawn), within
    /// what is left of the deck's items, and the drawing to keep for the other slides is returned.
    /// The slide's counters have the part's items and characters in them after, as before.
    fn part_segments(
        &mut self,
        kind: u8,
        part: &PartInfo,
        draw: PartDraw,
        new: bool,
        deck: &mut DeckItems,
        underlay: &mut Vec<Arc<Vec<sd::Item>>>,
    ) -> Option<PartDraw> {
        let nodes: Vec<&Node> = part
            .nodes
            .iter()
            .filter(|n| !shapes::is_placeholder(n))
            .collect();
        self.cur = if kind == PART_MASTER { 2 } else { 1 };
        let cap = self.max_items;
        if !new {
            self.items += draw.count;
            self.chars += draw.chars;
            self.truncated |= draw.truncated;
            for seg in &draw.segs {
                match seg {
                    Seg::Shared(list) => underlay.push(Arc::clone(list)),
                    Seg::Own(i) => {
                        let built = self.build_one(nodes[*i], cap, deck, true);
                        if !built.items.is_empty() {
                            underlay.push(Arc::new(built.items));
                        }
                    }
                }
            }
            return None;
        }
        let was_truncated = std::mem::replace(&mut self.truncated, false);
        let mut b = PartBuilder::default();
        let mut owned: Vec<Vec<sd::Item>> = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            let built = self.build_one(n, cap, deck, false);
            if built.dep {
                b.own(i);
                owned.push(built.items);
            } else {
                b.shared(built.items, built.count, built.chars);
            }
        }
        let truncated = self.truncated;
        self.truncated |= was_truncated;
        let draw = b.finish(truncated);
        let mut own = owned.into_iter();
        for seg in &draw.segs {
            match seg {
                Seg::Shared(list) => underlay.push(Arc::clone(list)),
                Seg::Own(_) => {
                    let items = own.next().unwrap_or_default();
                    if !items.is_empty() {
                        underlay.push(Arc::new(items));
                    }
                }
            }
        }
        Some(draw)
    }

    /// Builds one top-level shape of a master or layout. The shape is the slide's own when it
    /// shows the slide's number; its items and characters are in the slide's counters, and it is
    /// charged to the deck's pool for such copies; any other shape to the deck's items.
    /// `copy`: this is a slide's further copy of a shape already built (it can only draw what is
    /// left of the copies' pool).
    fn build_one(&mut self, n: &Node, cap: usize, deck: &mut DeckItems, copy: bool) -> Built {
        let was_dep = std::mem::replace(&mut self.slide_dep, false);
        let (items0, chars0) = (self.items, self.chars);
        let room = if copy {
            deck.copies_left()
        } else {
            deck.left()
        };
        self.max_items = cap.min(self.items.saturating_add(room));
        let mut out = Vec::new();
        self.build_nodes(std::iter::once(n), 0, &mut out);
        sd::footprint::compact(&mut out);
        let dep = self.slide_dep;
        self.slide_dep = was_dep;
        let count = self.items.saturating_sub(items0);
        let mut bytes = sd::footprint::items_bytes(&out);
        if dep || copy {
            // (The first build of a shape that shows the slide's number could not know it would
            // be charged to the copies' pool: if it does not fit what is left there it is not
            // kept, and the slide says something is missing.)
            let mut kept = count;
            if !copy && (count > deck.copies_left() || bytes > deck.copies_bytes_left()) {
                out.clear();
                (kept, bytes) = (0, 0);
                self.truncated = true;
            }
            deck.spend_copies(kept, bytes);
        } else {
            deck.spend(count, bytes);
        }
        Built {
            items: out,
            dep,
            count,
            chars: self.chars.saturating_sub(chars0),
        }
    }

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
