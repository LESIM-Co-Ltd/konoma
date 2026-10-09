//! A slide of an OpenDocument presentation (odp / otp) as a drawing model ([`sd::SlideScene`]),
//! built in the same pass that reads the presentation for the text Markdown (`odp.rs`): the styles
//! are collected while the text reader walks `styles.xml` and `content.xml` ([`styles::StyleBook`],
//! hooked into `OdStyles::add`), the master pages are kept by `read_masters`, and each page's tree
//! (already in memory for the text view) is lowered to a scene right after its text is written.
//!
//! What is read (ODF 1.3 chapters 9, 10, 14, 16, 19 and LibreOffice's practice):
//!
//! * the page size (the master page's `style:page-layout`) and the background (the slide's
//!   `draw:style-name` drawing-page style, else the master page's: solid, gradient, hatch, bitmap);
//! * the master page's shapes under the slide's own; master placeholders (`presentation:class`
//!   title, outline, ..) are never drawn, the date/time, footer, page-number and header ones only
//!   when the slide's style shows them (`presentation:display-*`), with the slide's footer text
//!   (`presentation:use-footer-name` -> `presentation:footer-decl`);
//! * shapes: `draw:frame` (text box, picture, embedded object -> its stored replacement picture),
//!   `rect`, `ellipse`/`circle` (with `draw:kind`), `line`, `polyline`, `polygon`, `regular-polygon`,
//!   `path`, `connector`, `measure`, `caption`, `custom-shape`, `g`; with `draw:transform`
//!   (see [`units::box_under`]), `draw:z-index` and fills, strokes, markers, shadows ([`paint`]) and
//!   text ([`body`]).
//!
//! # Geometry
//!
//! * `svg:x/y/width/height` and `draw:transform` (SVG syntax, `rotate` in radians; see below) are
//!   composed into the model's box: the box's centre goes
//!   through the matrix, its size is scaled, the rotation is the matrix's, a negative determinant is a
//!   vertical flip. **A shear is dropped.** A group maps its children to themselves (ODF writes the
//!   children in page coordinates); a group's own transform is applied to a box of the page's size.
//! * `draw:custom-shape`: `draw:enhanced-geometry` through [`sd::odf_geom::enhanced_geometry`]; when it
//!   has no path (`draw:type="ooxml-*"` shapes that LibreOffice writes as a name only, and
//!   which LibreOffice itself draws blank) the PowerPoint preset of that name ([`sd::geom::preset`]),
//!   with `draw:modifiers` as the preset's adjust values in order; the legacy LibreOffice names
//!   without a path map through [`legacy_preset`].
//! * `draw:connector` uses its `svg:d` when it has one; otherwise a straight line (`line`),
//!   an elbow of two bends through the middle (`standard`, `lines`) or an S-curve (`curve`) between
//!   `svg:x1/y1` and `svg:x2/y2`. `draw:measure` is drawn as its line (its text is not).
//!   `draw:caption` is drawn as its rectangle (the pointer to `draw:caption-point-*` is not).
//!   `draw:page-thumbnail` (the notes page's slide picture) is not drawn. A Fontwork shape
//!   (`draw:type="fontwork-*"`, text on a warped path in LibreOffice) is drawn as plain text in the
//!   box in the shape's fill colour, without the warp outline.
//! * A `draw:transform` list applies its operations in the order written, and `rotate` is
//!   counter-clockwise ([`units::draw_transform`]); a rotated shape turns about its top-left
//!   corner before it is moved, exactly as LibreOffice writes it.
//! * Hidden slides (`presentation:visibility="hidden"`) are drawn like any other.
//!
//! # Hooks for later tasks
//!
//! [`Sb::frame_table`] (`table:table` in a frame) and [`Sb::frame_chart`] (`draw:object` that is a
//! chart) are empty; the frame's stored replacement picture is drawn meanwhile when there is one.
//!
//! # Budgets
//!
//! Items per slide ([`DocOptions::max_slide_shapes`], master shapes included), group depth
//! ([`MAX_GROUP_DEPTH`]), characters per slide ([`body::MAX_SLIDE_CHARS`]), paragraphs per text
//! body, list depth, `svg:d` size and commands ([`units::MAX_PATH_BYTES`], [`units::MAX_PATH_CMDS`]),
//! gradient stops, dash pairs, styles ([`styles::MAX_STYLES`]), resources
//! ([`styles::MAX_RESOURCES`]), style chain ([`styles::MAX_CHAIN`], cycles end it) and the
//! enhanced-geometry budgets. Going over any sets [`sd::SlideScene::truncated`] (and
//! [`Document::truncated`]); nothing is dropped silently.

use std::collections::HashMap;

use crate::preview::office::slide_draw as sd;
use sd::Rgba;

use super::*;

mod body;
mod paint;
mod shapes;
pub(in crate::preview::office) mod styles;
mod units;

#[cfg(test)]
mod tests;

pub(in crate::preview::office) use styles::StyleBook;

use styles::View;

/// The key a picture that could not be loaded is drawn with: the renderer finds no such picture and
/// draws its placeholder.
pub(super) const MISSING_PICTURE: &str = "office-img://missing/picture";

/// The slide size when the master page's layout does not say (LibreOffice's 16:9 default, EMU).
pub(super) const DEFAULT_SIZE: (f64, f64) = (10_080_000.0, 5_670_000.0);

/// The footer, header and date texts of a presentation (`presentation:*-decl`), by name.
#[derive(Debug, Default)]
pub(super) struct Decls {
    pub footer: HashMap<String, String>,
    pub header: HashMap<String, String>,
    /// Fixed date/time texts (`presentation:source="fixed"`); a current-date declaration has none.
    pub date_time: HashMap<String, String>,
}

/// Most declarations of one kind kept.
pub(super) const MAX_DECLS: usize = 4_096;

impl Decls {
    /// Takes one `presentation:footer-decl` / `header-decl` / `date-time-decl` element.
    pub fn add(&mut self, n: &Node) {
        let Some(name) = n.attr("name") else { return };
        let mut text = String::new();
        styles::text_of(n, &mut text, 0);
        let text = text.trim().to_string();
        let map = match n.name.as_str() {
            "footer-decl" => &mut self.footer,
            "header-decl" => &mut self.header,
            "date-time-decl" => {
                if n.attr("source").map(str::trim) == Some("current-date") {
                    return;
                }
                &mut self.date_time
            }
            _ => return,
        };
        if map.len() < MAX_DECLS {
            map.insert(name.to_string(), text);
        }
    }
}

/// How the builder gets pictures.
pub(super) trait Media {
    /// The `office-img://` key of the picture at package part `part` (loaded once), if it can be
    /// shown.
    fn load(&mut self, part: &str) -> Option<String>;
    /// The bytes of a loaded picture.
    fn bytes(&self, key: &str) -> Option<&[u8]>;
}

impl Media for Conv<'_> {
    fn load(&mut self, part: &str) -> Option<String> {
        self.image_key_for_drawing(part)
    }

    fn bytes(&self, key: &str) -> Option<&[u8]> {
        self.images
            .iter()
            .find(|i| i.key == key)
            .map(|i| i.bytes.as_slice())
    }
}

/// What the builder needs of one slide.
pub(super) struct PageInput<'a> {
    pub book: &'a StyleBook,
    pub masters: &'a Masters,
    pub decls: &'a Decls,
    pub opts: &'a DocOptions,
    pub page: &'a Node,
    /// 1-based.
    pub number: usize,
}

/// The state of building one scene.
pub(super) struct Sb<'a> {
    pub book: &'a StyleBook,
    pub media: &'a mut dyn Media,
    pub size: (f64, f64),
    pub number: usize,
    pub items: usize,
    pub max_items: usize,
    pub chars: usize,
    pub truncated: bool,
    /// The slide's footer / header / date texts (what the master's fields show).
    pub footer: Option<String>,
    pub header: Option<String>,
    pub date_time: Option<String>,
    /// What a `text:page-name` field shows: the page's name, or `Slide N` for an unnamed page
    /// (LibreOffice's default names are `pageN`).
    pub page_name: String,
    /// The slide's background colour (for automatic text colours).
    pub page_bg: Rgba,
    /// The master page whose placeholders positions the slide's frames inherit.
    pub master_frames: Option<&'a HashMap<String, Rect>>,
    dims: HashMap<String, Option<(f64, f64)>>,
}

/// The value of the attribute with the qualified name `q` (`draw:style-name`): `Node::attr` matches
/// the local name only, and `draw:style-name` / `presentation:style-name` share theirs.
pub(super) fn qattr<'n>(n: &'n Node, q: &str) -> Option<&'n str> {
    n.attrs
        .iter()
        .find(|(k, _)| k == q)
        .map(|(_, v)| v.as_str())
}

/// Builds the scene of a slide.
pub(super) fn build_scene(inp: PageInput<'_>, media: &mut dyn Media) -> sd::SlideScene {
    let page = inp.page;
    let master_name = page.attr("master-page-name");
    let master = master_name.and_then(|m| inp.masters.pages.get(m));
    let size = master_name
        .and_then(|m| inp.masters.size.get(m))
        .map_or(DEFAULT_SIZE, |r| (r.w as f64, r.h as f64));
    let mut sb = Sb {
        book: inp.book,
        media,
        size,
        number: inp.number,
        items: 0,
        max_items: inp.opts.max_slide_shapes,
        chars: 0,
        truncated: false,
        footer: None,
        header: None,
        date_time: None,
        page_name: page_display_name(page, inp.number),
        page_bg: Rgba::WHITE,
        master_frames: master_name.and_then(|m| inp.masters.frames.get(m)),
        dims: HashMap::new(),
    };
    let mut scene = sd::SlideScene {
        width: size.0,
        height: size.1,
        ..sd::SlideScene::default()
    };

    // The slide's style, then the master page's.
    let mut page_view = View::with_default(inp.book, "graphic");
    if let Some(s) = qattr(page, "draw:style-name") {
        page_view.push_chain(inp.book, "drawing-page", s);
    }
    let mut master_view = View::with_default(inp.book, "graphic");
    if let Some(s) = master.and_then(|m| qattr(m, "draw:style-name")) {
        master_view.push_chain(inp.book, "drawing-page", s);
    }
    let truthy = |v: Option<&str>, default: bool| v.map_or(default, |s| s.trim() == "true");
    let bg_visible = truthy(page_view.g("background-visible"), true);
    let objects_visible = truthy(page_view.g("background-objects-visible"), true);
    scene.background = if bg_visible {
        let own = sb.fill(&page_view, size.0, size.1);
        let fill = match own {
            Some(f) => Some(f),
            None => sb.fill(&master_view, size.0, size.1),
        };
        match fill {
            Some(sd::Fill::None) | None => sd::Fill::Solid(Rgba::WHITE),
            Some(f) => f,
        }
    } else {
        sd::Fill::Solid(Rgba::WHITE)
    };
    sb.page_bg = match &scene.background {
        sd::Fill::Solid(c) => *c,
        sd::Fill::Gradient(g) => g.stops.first().map_or(Rgba::WHITE, |s| s.1),
        _ => Rgba::WHITE,
    };

    // Footer, header and date texts of this slide.
    // (ODF 1.2 puts the references on the page element, LibreOffice's older files on its style.)
    let decl = |key: &str, map: &HashMap<String, String>| {
        qattr(page, &format!("presentation:{key}"))
            .or_else(|| page_view.g(key))
            .and_then(|n| map.get(n.trim()))
            .cloned()
    };
    sb.footer = decl("use-footer-name", &inp.decls.footer);
    sb.header = decl("use-header-name", &inp.decls.header);
    sb.date_time = decl("use-date-time-name", &inp.decls.date_time);
    let show = |key: &str| truthy(page_view.g(key), true);
    let flags = shapes::Furniture {
        footer: show("display-footer"),
        date_time: show("display-date-time"),
        page_number: show("display-page-number"),
        header: show("display-header"),
    };

    // A background picture placed once goes over the background colour, under everything.
    if bg_visible {
        let v = if page_view.g("fill").is_some() {
            &page_view
        } else {
            &master_view
        };
        if let Some(p) = sb.single_background_picture(v) {
            scene.background = sd::Fill::Solid(Rgba::WHITE);
            scene.items.push(sd::Item::Picture(p));
        }
    }
    // Back to front: the master page's shapes, then the slide's.
    if objects_visible {
        if let Some(m) = master {
            sb.build_master(m, &flags, &mut scene.items);
        }
    }
    sb.build_nodes(page.nodes(), 0, &mut scene.items, None);
    scene.truncated = sb.truncated;
    scene
}

/// The name a `text:page-name` field shows for a page.
fn page_display_name(page: &Node, number: usize) -> String {
    let named = qattr(page, "draw:name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .filter(|n| {
            !(n.strip_prefix("page")
                .is_some_and(|t| !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit())))
        });
    match named {
        Some(n) => n.chars().take(200).collect(),
        None => format!("Slide {number}"),
    }
}

/// Replaces the page-count marker of the fields in `scenes` with `total`.
pub(super) fn patch_page_count(scenes: &mut [sd::SlideScene], total: usize) {
    fn items(list: &mut [sd::Item], total: &str) {
        for it in list {
            match it {
                sd::Item::Shape(s) => {
                    if let Some(t) = &mut s.text {
                        for p in &mut t.paragraphs {
                            for r in &mut p.runs {
                                if r.kind == sd::RunKind::Field
                                    && r.text.contains(body::PAGE_COUNT_MARK)
                                {
                                    r.text = r.text.replace(body::PAGE_COUNT_MARK, total);
                                }
                            }
                        }
                    }
                }
                sd::Item::Group(g) => items(&mut g.items, total),
                sd::Item::Picture(_) => {}
            }
        }
    }
    let t = total.to_string();
    for s in scenes {
        items(&mut s.items, &t);
    }
}

/// An empty scene of the given size, for a slide that could not be read.
pub(super) fn empty_scene(size: (f64, f64)) -> sd::SlideScene {
    sd::SlideScene {
        width: size.0,
        height: size.1,
        background: sd::Fill::Solid(Rgba::WHITE),
        truncated: true,
        ..sd::SlideScene::default()
    }
}

/// The slide size for a master page name, as the scene builder takes it.
pub(super) fn size_of(masters: &Masters, master: Option<&str>) -> (f64, f64) {
    master
        .and_then(|m| masters.size.get(m))
        .map_or(DEFAULT_SIZE, |r| (r.w as f64, r.h as f64))
}

impl Sb<'_> {
    /// The picture at `href` (an ODF `xlink:href`, `Pictures/x.png`) as a model key; `None` for an
    /// external link or anything that cannot be shown.
    pub(super) fn image_for_href(&mut self, href: &str) -> Option<String> {
        let part = part_of(href)?;
        self.media.load(&part)
    }

    /// The size of a loaded picture in EMU at its own resolution (PNG `pHYs`, JPEG JFIF density;
    /// 96 dpi without either). `None` for a picture that is not a raster this can read.
    pub(super) fn image_dims(&mut self, key: &str) -> Option<(u32, u32)> {
        self.native_size(key).map(|(w, h, _)| (w as u32, h as u32))
    }

    /// (width px, height px, dpi) of a picture.
    fn native_size(&mut self, key: &str) -> Option<(f64, f64, f64)> {
        if let Some(Some((w, h))) = self.dims.get(key) {
            let _ = (w, h);
        }
        let bytes = self.media.bytes(key)?;
        let (w, h) = image_dimensions(bytes)?;
        let dpi = image_dpi(bytes).unwrap_or(96.0);
        Some((f64::from(w), f64::from(h), dpi))
    }

    /// The picture's own size in EMU (pixels over its dpi).
    pub(super) fn native_emu(&mut self, key: &str) -> Option<(f64, f64)> {
        if let Some(c) = self.dims.get(key) {
            return *c;
        }
        let v = self.native_size(key).map(|(w, h, dpi)| {
            let per = 914_400.0 / dpi.clamp(1.0, 4800.0);
            (w * per, h * per)
        });
        if self.dims.len() < 4096 {
            self.dims.insert(key.to_string(), v);
        }
        v
    }
}

/// The pixel size of a raster picture, from its header only.
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let r = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    r.into_dimensions().ok().filter(|(w, h)| *w > 0 && *h > 0)
}

/// The resolution a PNG (`pHYs`) or JPEG (JFIF `APP0`) states; `None` without one.
fn image_dpi(b: &[u8]) -> Option<f64> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        let mut i = 8;
        while i + 8 <= b.len() {
            let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            let kind = &b[i + 4..i + 8];
            if kind == b"pHYs" && len >= 9 && i + 8 + 9 <= b.len() {
                let d = &b[i + 8..];
                let ppu = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                return (d[8] == 1 && ppu > 0).then(|| f64::from(ppu) * 0.0254);
            }
            if kind == b"IDAT" {
                return None;
            }
            i = i.checked_add(12)?.checked_add(len)?;
        }
        return None;
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 4 <= b.len() && b[i] == 0xFF {
            let marker = b[i + 1];
            let len = usize::from(b[i + 2]) << 8 | usize::from(b[i + 3]);
            if marker == 0xE0 && len >= 14 && i + 4 + 12 <= b.len() && &b[i + 4..i + 9] == b"JFIF\0"
            {
                let units = b[i + 11];
                let dx = f64::from(u16::from_be_bytes([b[i + 12], b[i + 13]]));
                return match units {
                    1 if dx > 0.0 => Some(dx),
                    2 if dx > 0.0 => Some(dx * 2.54),
                    _ => None,
                };
            }
            if marker == 0xDA {
                return None;
            }
            i += 2 + len;
        }
    }
    None
}

/// The PowerPoint preset a LibreOffice legacy shape name stands for (used when
/// `draw:enhanced-geometry` has no path). Only the shapes LibreOffice names itself; the `ooxml-*`
/// names are the presets' own. A name not in the table has no preset.
pub(super) fn legacy_preset(name: &str) -> Option<&'static str> {
    Some(match name {
        "rectangle" | "round-rectangle" => {
            if name == "rectangle" {
                "rect"
            } else {
                "roundRect"
            }
        }
        "ellipse" => "ellipse",
        "diamond" => "diamond",
        "isosceles-triangle" => "triangle",
        "right-triangle" => "rtTriangle",
        "parallelogram" => "parallelogram",
        "trapezoid" => "trapezoid",
        "hexagon" => "hexagon",
        "octagon" => "octagon",
        "pentagon" | "regular-pentagon" => "pentagon",
        "cross" => "plus",
        "ring" => "donut",
        "can" => "can",
        "cube" => "cube",
        "paper" => "foldedCorner",
        "star4" => "star4",
        "star5" => "star5",
        "star6" => "star6",
        "star8" => "star8",
        "star12" => "star12",
        "star24" => "star24",
        "heart" => "heart",
        "sun" => "sun",
        "moon" => "moon",
        "cloud" => "cloud",
        "lightning-bolt" => "lightningBolt",
        "smiley" => "smileyFace",
        "forbidden" => "noSmoking",
        "bevel" => "bevel",
        "frame" => "frame",
        "right-arrow" => "rightArrow",
        "left-arrow" => "leftArrow",
        "up-arrow" => "upArrow",
        "down-arrow" => "downArrow",
        "left-right-arrow" => "leftRightArrow",
        "up-down-arrow" => "upDownArrow",
        "chevron" => "chevron",
        "left-bracket" => "leftBracket",
        "right-bracket" => "rightBracket",
        "left-brace" => "leftBrace",
        "right-brace" => "rightBrace",
        "flowchart-process" => "flowChartProcess",
        "flowchart-decision" => "flowChartDecision",
        "flowchart-terminator" => "flowChartTerminator",
        "flowchart-document" => "flowChartDocument",
        "flowchart-connector" => "flowChartConnector",
        "rectangular-callout" => "wedgeRectCallout",
        "round-rectangular-callout" => "wedgeRoundRectCallout",
        "round-callout" => "wedgeEllipseCallout",
        _ => return None,
    })
}

#[cfg(test)]
mod dump;
