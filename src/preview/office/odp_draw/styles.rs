//! The styles of an OpenDocument presentation as the drawing needs them: `office:styles`,
//! `office:automatic-styles` and `office:font-face-decls` of `styles.xml` and `content.xml`, collected
//! while the text reader walks the same parts ([`StyleBook::add`] / [`StyleBook::add_fonts`]).
//!
//! * A `style:style` keeps the attributes of its property elements by local name (`graphic`:
//!   `style:graphic-properties` and `style:drawing-page-properties`, `para`:
//!   `style:paragraph-properties`, `text`: `style:text-properties`), the `text:list-style` that
//!   LibreOffice nests inside a graphic style, and the other `style:*-properties` for the table
//!   reader (`extra`).
//! * `style:default-style` is kept per family.
//! * Named resources: `draw:gradient`, `draw:hatch`, `draw:fill-image`, `draw:marker`,
//!   `draw:stroke-dash`, `draw:opacity`, and the named `text:list-style`s.
//! * `style:font-face` names to `svg:font-family`.
//!
//! A [`View`] is a list of styles from the weakest to the strongest (default style, parents, the
//! style itself, ...); a property is the one of the strongest layer that states it.
//!
//! # Budgets
//!
//! [`MAX_STYLES`] styles, [`MAX_RESOURCES`] of each resource kind, [`MAX_CHAIN`] styles in a
//! `style:parent-style-name` chain (a cycle ends the chain). Over a budget the thing is left out
//! and the book says so ([`StyleBook::truncated`]).

use std::collections::HashMap;

use crate::preview::office::docx_xml::{Kid, Node};

/// Most `style:style` elements kept.
pub(in crate::preview::office) const MAX_STYLES: usize = 50_000;
/// Most named resources kept of one kind (gradients, hatches, markers, dashes, ...).
pub(in crate::preview::office) const MAX_RESOURCES: usize = 4_096;
/// Longest `style:parent-style-name` chain followed (a cycle ends it too).
pub(in crate::preview::office) const MAX_CHAIN: usize = 32;
/// Most attributes kept per property element.
const MAX_PROPS: usize = 96;

/// Attributes by local name.
pub(in crate::preview::office) type Attrs = Vec<(String, String)>;

fn attrs_of(n: &Node) -> Attrs {
    n.attrs
        .iter()
        .take(MAX_PROPS)
        .map(|(k, v)| (k.rsplit(':').next().unwrap_or(k).to_string(), v.to_string()))
        .collect()
}

fn get<'a>(a: &'a Attrs, key: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// One `style:style` (or `style:default-style`).
#[derive(Debug, Clone, Default)]
pub(in crate::preview::office) struct StyleDef {
    pub parent: Option<String>,
    /// `style:graphic-properties` and `style:drawing-page-properties`.
    pub graphic: Attrs,
    /// `style:paragraph-properties`.
    pub para: Attrs,
    /// `style:text-properties`.
    pub text: Attrs,
    /// The `text:list-style` inside the style's graphic properties (LibreOffice's presentation
    /// styles carry the bullets of the outline levels this way).
    pub list: Option<Node>,
    /// The other property elements (`table-cell-properties`, `table-row-properties`, ..) by local
    /// name, for the table reader.
    pub extra: Vec<(String, Attrs)>,
}

/// The collected styles.
#[derive(Debug, Default)]
pub(in crate::preview::office) struct StyleBook {
    styles: HashMap<(String, String), StyleDef>,
    defaults: HashMap<String, StyleDef>,
    pub gradients: HashMap<String, Node>,
    pub hatches: HashMap<String, Node>,
    pub fill_images: HashMap<String, Node>,
    pub markers: HashMap<String, Node>,
    pub dashes: HashMap<String, Node>,
    pub opacities: HashMap<String, Node>,
    pub list_styles: HashMap<String, Node>,
    fonts: HashMap<String, String>,
    /// A budget cut something.
    pub truncated: bool,
}

fn put_resource(map: &mut HashMap<String, Node>, n: &Node, truncated: &mut bool) {
    let Some(name) = n.attr("name") else { return };
    if map.len() >= MAX_RESOURCES && !map.contains_key(name) {
        *truncated = true;
        return;
    }
    map.insert(name.to_string(), n.clone());
}

fn style_def(n: &Node) -> StyleDef {
    let mut d = StyleDef {
        parent: n.attr("parent-style-name").map(str::to_string),
        ..StyleDef::default()
    };
    for k in n.nodes() {
        match k.name.as_str() {
            "graphic-properties" | "drawing-page-properties" => {
                d.graphic.extend(attrs_of(k));
                if d.list.is_none() {
                    d.list = k.nodes().find(|c| c.name == "list-style").cloned();
                }
            }
            "paragraph-properties" => d.para.extend(attrs_of(k)),
            "text-properties" => d.text.extend(attrs_of(k)),
            name if name.ends_with("-properties") && d.extra.len() < 16 => {
                d.extra.push((name.to_string(), attrs_of(k)));
            }
            _ => {}
        }
    }
    d
}

impl StyleBook {
    /// Takes the children of an `office:styles` or `office:automatic-styles` element. Names are
    /// not unique across the two parts in principle; the later one wins (LibreOffice prefixes the
    /// names of the master page's automatic styles, so they do not clash in practice).
    pub fn add(&mut self, container: &Node) {
        for n in container.nodes() {
            match n.name.as_str() {
                "style" => {
                    let (Some(fam), Some(name)) = (n.attr("family"), n.attr("name")) else {
                        continue;
                    };
                    let key = (fam.to_string(), name.to_string());
                    if self.styles.len() >= MAX_STYLES && !self.styles.contains_key(&key) {
                        self.truncated = true;
                        continue;
                    }
                    self.styles.insert(key, style_def(n));
                }
                "default-style" => {
                    if let Some(fam) = n.attr("family") {
                        if self.defaults.len() < 64 {
                            self.defaults.insert(fam.to_string(), style_def(n));
                        }
                    }
                }
                "gradient" => put_resource(&mut self.gradients, n, &mut self.truncated),
                "hatch" => put_resource(&mut self.hatches, n, &mut self.truncated),
                "fill-image" => put_resource(&mut self.fill_images, n, &mut self.truncated),
                "marker" => put_resource(&mut self.markers, n, &mut self.truncated),
                "stroke-dash" => put_resource(&mut self.dashes, n, &mut self.truncated),
                "opacity" => put_resource(&mut self.opacities, n, &mut self.truncated),
                "list-style" => put_resource(&mut self.list_styles, n, &mut self.truncated),
                _ => {}
            }
        }
    }

    /// Takes an `office:font-face-decls`.
    pub fn add_fonts(&mut self, decls: &Node) {
        for f in decls.nodes().filter(|n| n.name == "font-face") {
            let Some(name) = f.attr("name") else { continue };
            if self.fonts.len() >= MAX_RESOURCES && !self.fonts.contains_key(name) {
                self.truncated = true;
                continue;
            }
            if let Some(fam) = f.attr("font-family") {
                self.fonts.insert(name.to_string(), clean_family(fam));
            }
        }
    }

    /// The family a `style:font-name` stands for (the name itself when it has no declaration).
    pub fn font_family(&self, name: &str) -> String {
        self.fonts
            .get(name)
            .cloned()
            .unwrap_or_else(|| clean_family(name))
    }

    pub fn style(&self, family: &str, name: &str) -> Option<&StyleDef> {
        self.styles.get(&(family.to_string(), name.to_string()))
    }

    pub fn default_style(&self, family: &str) -> Option<&StyleDef> {
        self.defaults.get(family)
    }

    /// The styles of the chain starting at `name`, most derived first.
    pub fn chain(&self, family: &str, name: &str) -> Vec<(&str, &StyleDef)> {
        let mut out: Vec<(&str, &StyleDef)> = Vec::new();
        let mut cur = self
            .styles
            .get_key_value(&(family.to_string(), name.to_string()));
        while let Some(((_, k), d)) = cur {
            if out.len() >= MAX_CHAIN || out.iter().any(|(o, _)| *o == k.as_str()) {
                break;
            }
            out.push((k.as_str(), d));
            cur = d.parent.as_deref().and_then(|p| {
                self.styles
                    .get_key_value(&(family.to_string(), p.to_string()))
            });
        }
        out
    }
}

/// `'Liberation Sans'` -> `Liberation Sans` (`svg:font-family` may quote a name or list several;
/// the first is kept).
pub(in crate::preview::office) fn clean_family(s: &str) -> String {
    let first = s.split(',').next().unwrap_or(s);
    first
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .trim()
        .to_string()
}

/// A list of styles, weakest first.
#[derive(Debug, Clone, Default)]
pub(in crate::preview::office) struct View<'a> {
    layers: Vec<&'a StyleDef>,
}

impl<'a> View<'a> {
    /// The default style of `family` as the weakest layer.
    pub fn with_default(book: &'a StyleBook, family: &str) -> View<'a> {
        let mut v = View::default();
        if let Some(d) = book.default_style(family) {
            v.layers.push(d);
        }
        v
    }

    /// Adds the chain of style `name` (parents first) as the strongest layers.
    pub fn push_chain(&mut self, book: &'a StyleBook, family: &str, name: &str) {
        for (_, d) in book.chain(family, name).into_iter().rev() {
            self.layers.push(d);
        }
    }

    /// Adds one style as the strongest layer.
    pub fn push(&mut self, d: &'a StyleDef) {
        self.layers.push(d);
    }

    pub fn layers(&self) -> &[&'a StyleDef] {
        &self.layers
    }

    fn find(&self, pick: impl Fn(&StyleDef) -> &Attrs, key: &str) -> Option<&'a str> {
        self.layers.iter().rev().find_map(|l| get(pick(l), key))
    }

    /// A property of the graphic / drawing-page properties.
    pub fn g(&self, key: &str) -> Option<&'a str> {
        self.find(|d| &d.graphic, key)
    }

    /// A property of the paragraph properties.
    pub fn p(&self, key: &str) -> Option<&'a str> {
        self.find(|d| &d.para, key)
    }

    /// A property of the text properties.
    pub fn t(&self, key: &str) -> Option<&'a str> {
        self.find(|d| &d.text, key)
    }

    /// The strongest `text:list-style` a layer carries.
    pub fn list(&self) -> Option<&'a Node> {
        self.layers.iter().rev().find_map(|l| l.list.as_ref())
    }
}

/// All the text under `n`.
pub(in crate::preview::office) fn text_of(n: &Node, out: &mut String, depth: usize) {
    if depth > 32 || out.len() > 4096 {
        return;
    }
    for k in &n.kids {
        match k {
            Kid::T(t) => out.push_str(t),
            Kid::N(c) => text_of(c, out, depth + 1),
        }
    }
}
