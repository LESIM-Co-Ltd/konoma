//! The text of a shape (`draw:text-box`, or the paragraphs directly inside a drawing shape) as a
//! [`sd::TextBody`].
//!
//! # Where a property comes from (weakest first)
//!
//! `style:default-style` of the paragraph family, `style:default-style` of the graphic family,
//! the shape's presentation style chain (`presentation:style-name`), its graphic style chain
//! (`draw:style-name`), the shape's `draw:text-style-name` (a paragraph style), the paragraph's own
//! style chain (`text:style-name`), then the span's. In an outline placeholder
//! (`presentation:class="outline"`) the presentation style of a paragraph depends on its list
//! level: LibreOffice names the styles `<master>-outline1` .. `-outline9`, and level *n* uses
//! `outline<n>` where the shape's style chain reaches `outline1`.
//!
//! * `fo:font-size` in percent is relative to what the weaker layers say.
//! * The size, weight and style of an East-Asian or complex-script run come from the `-asian` /
//!   `-complex` properties when the run's first strong character is of that script.
//! * An *automatic* text colour (no `fo:color`, or `style:use-window-font-color`) is black on a
//!   light background and white on a dark one (the shape's fill, else the slide's background).
//!
//! # Lists
//!
//! The list style is the one named by the nearest enclosing `text:list`, else the one nested in the
//! presentation style (the outline levels' bullets). Bullet characters, their size (percent of the
//! text), colour and picture, and the numbering formats (`1`, `a`, `A`, `i`, `I` with the
//! prefix and suffix the model can express: `.`, `)`, `()`, none) are read; the indent is
//! `text:space-before + text:min-label-width` (ODF 1.1 mode) or the label alignment's margin and
//! indent (ODF 1.2 mode), the paragraph's own `fo:margin-left` / `fo:text-indent` winning when it
//! states them.
//!
//! # Not drawn
//!
//! The right margin of a paragraph, `style:line-spacing` (extra space between lines),
//! `style:line-height-at-least` (drawn as single spacing), tab stops (a tab is four spaces), the
//! underline colour and style, columns, text on a path, and `text:date` fields that have no cached
//! value.

use crate::preview::office::slide_draw as sd;
use sd::Rgba;

use super::styles::{StyleBook, View};
use super::units::{angle_deg, color, luminance, pct};
use super::*;

/// Most characters of text drawn on one slide (masters and the slide together).
pub(super) const MAX_SLIDE_CHARS: usize = 200_000;
/// Most paragraphs of one text body.
pub(super) const MAX_PARAGRAPHS: usize = 5_000;
/// Deepest list nesting followed.
pub(super) const MAX_LIST_DEPTH: usize = 9;
/// Deepest inline element nesting followed.
const MAX_INLINE_DEPTH: usize = 60;
/// How much bigger the OpenSymbol black circle is than the same character in an ordinary face.
const OPEN_SYMBOL_DOT_SCALE: f64 = 2.0;
/// A forced "page count" field is written with this marker and replaced when the deck is done.
pub(super) const PAGE_COUNT_MARK: &str = "\u{1}pagecount\u{1}";

/// The styles that apply to the text of one shape.
#[derive(Clone, Default)]
pub(super) struct Env<'a> {
    /// `presentation:style-name`.
    pub pres: Option<&'a str>,
    /// `draw:style-name`.
    pub gfx: Option<&'a str>,
    /// `draw:text-style-name` (a paragraph style).
    pub text_style: Option<&'a str>,
    /// `presentation:class`.
    pub class: Option<&'a str>,
    /// The colour of automatic text.
    pub auto: Option<Rgba>,
}

/// What an inline walk collects.
struct Inl {
    runs: Vec<sd::Run>,
    prev_space: bool,
}

fn first_strong(s: &str) -> sd::fonts::Script {
    s.chars()
        .map(sd::fonts::script_of)
        .find(|sc| *sc != sd::fonts::Script::Latin)
        .unwrap_or(sd::fonts::Script::Latin)
}

/// `"1.5cm"` etc to points.
fn pts(v: &str) -> Option<f64> {
    emu(v).map(|e| e / sd::EMU_PER_PT)
}

/// The font size of the view in points (percent sizes are relative to the weaker layers).
fn size_pt(v: &View, key: &str, start: f64) -> f64 {
    let mut cur = start;
    for l in v.layers() {
        if let Some((_, val)) = l.text.iter().find(|(k, _)| k == key) {
            if let Some(p) = pct(val) {
                cur *= p;
            } else if let Some(p) = pts(val) {
                cur = p;
            }
        }
    }
    cur.clamp(0.5, 4000.0)
}

fn transform_text(t: &str, how: &str) -> String {
    match how {
        "lowercase" => t.to_lowercase(),
        "capitalize" => {
            let mut out = String::with_capacity(t.len());
            let mut up = true;
            for c in t.chars() {
                if up && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                    up = false;
                } else {
                    out.push(c);
                    up = !c.is_alphanumeric() && c != '\'';
                }
            }
            out
        }
        _ => t.to_string(),
    }
}

impl<'a> Sb<'a> {
    /// The shape-level view (no paragraph yet).
    pub(super) fn shape_view(&self, env: &Env<'a>) -> View<'a> {
        let mut v = View::with_default(self.book, "paragraph");
        if let Some(d) = self.book.default_style("graphic") {
            v.push(d);
        }
        if let Some(p) = env.pres {
            v.push_chain(self.book, "presentation", p);
        }
        if let Some(g) = env.gfx {
            v.push_chain(self.book, "graphic", g);
        }
        v
    }

    /// The view of paragraphs at list level `level` (0-based) of a shape.
    fn level_view(&self, env: &Env<'a>, level: usize) -> View<'a> {
        let book: &'a StyleBook = self.book;
        let mut v = View::with_default(book, "paragraph");
        if let Some(d) = book.default_style("graphic") {
            v.push(d);
        }
        let mut done = false;
        if env.class == Some("outline") {
            if let Some(p) = env.pres {
                let chain = book.chain("presentation", p);
                if let Some(idx) = chain.iter().position(|(n, _)| n.ends_with("outline1")) {
                    let stem = &chain[idx].0[..chain[idx].0.len() - 1];
                    let target = format!("{stem}{}", (level + 1).min(9));
                    if book.style("presentation", &target).is_some() {
                        v.push_chain(book, "presentation", &target);
                        // What the shape's own (automatic) styles say wins over the level's.
                        for (_, d) in chain[..idx].iter().rev() {
                            v.push(d);
                        }
                        done = true;
                    }
                }
            }
        }
        if !done {
            if let Some(p) = env.pres {
                v.push_chain(book, "presentation", p);
            }
        }
        if let Some(g) = env.gfx {
            v.push_chain(book, "graphic", g);
        }
        if let Some(t) = env.text_style {
            v.push_chain(book, "paragraph", t);
        }
        v
    }

    /// The text body of the paragraphs among `kids`; `None` when there is no text at all.
    ///
    /// `shape` is the shape-level view for the body properties; `lists` is the list style nested
    /// in the shape's presentation style.
    pub(super) fn text_body(
        &mut self,
        kids: &'a [Kid],
        env: &Env<'a>,
        shape: &View<'a>,
    ) -> Option<sd::TextBody> {
        let mut paras: Vec<sd::Paragraph> = Vec::new();
        let base_list = shape.list();
        self.blocks(kids, env, base_list, None, 0, false, &mut paras);
        if paras.is_empty() || paras.iter().all(|p| p.runs.is_empty()) && paras.len() == 1 {
            // A single empty paragraph carries no text; an empty text box draws nothing.
            if paras.iter().all(|p| p.runs.is_empty()) {
                return None;
            }
        }
        let mut body = sd::TextBody {
            paragraphs: paras,
            ..sd::TextBody::default()
        };
        self.body_props(&mut body, shape);
        Some(body)
    }

    /// The body properties (`style:graphic-properties`) of a shape.
    fn body_props(&self, body: &mut sd::TextBody, v: &View) {
        // LibreOffice's own text distances (0.25 cm sideways, 0.125 cm above and below) hold when
        // no style states them.
        let pad = |k: &str, whole: Option<&str>, default: f64| -> f64 {
            v.g(k)
                .and_then(emu)
                .or_else(|| whole.and_then(emu))
                .unwrap_or(default)
                .clamp(0.0, 1.0e8)
        };
        let all = v.g("padding");
        body.insets = (
            pad("padding-left", all, 90_000.0),
            pad("padding-top", all, 45_000.0),
            pad("padding-right", all, 90_000.0),
            pad("padding-bottom", all, 45_000.0),
        );
        body.anchor = match v.g("textarea-vertical-align").map_or("top", str::trim) {
            "middle" => sd::Anchor::Middle,
            "bottom" => sd::Anchor::Bottom,
            _ => sd::Anchor::Top,
        };
        body.anchor_ctr = v.g("textarea-horizontal-align").map(str::trim) == Some("center");
        body.wrap = v.g("wrap-option").map(str::trim) != Some("no-wrap");
        let truthy = |s: Option<&str>| matches!(s.map(str::trim), Some("true"));
        body.autofit = if truthy(v.g("shrink-to-fit")) {
            sd::AutoFit::Normal {
                font_scale: 1.0,
                ln_spc_reduction: 0.0,
            }
        } else if truthy(v.g("auto-grow-height")) {
            sd::AutoFit::Shape
        } else {
            sd::AutoFit::None
        };
        body.vert = match v.p("writing-mode").map(str::trim) {
            Some("tb-rl" | "tb" | "tb-lr") => sd::Vert::EaVert,
            _ => sd::Vert::Horz,
        };
    }

    /// Walks block-level children (`text:p`, `text:h`, `text:list`, ...).
    #[allow(clippy::too_many_arguments)]
    fn blocks(
        &mut self,
        kids: &'a [Kid],
        env: &Env<'a>,
        list: Option<&'a Node>,
        outer_list: Option<&'a Node>,
        depth: usize,
        header: bool,
        out: &mut Vec<sd::Paragraph>,
    ) {
        let _ = outer_list;
        for k in kids {
            let Kid::N(n) = k else { continue };
            if out.len() >= MAX_PARAGRAPHS || self.chars >= MAX_SLIDE_CHARS {
                self.truncated = true;
                return;
            }
            match (n.prefix.as_str(), n.name.as_str()) {
                ("text", "p" | "h") => {
                    let p = self.paragraph(n, env, list, depth, header);
                    out.push(p);
                }
                ("text", "list") => {
                    if depth >= MAX_LIST_DEPTH {
                        self.truncated = true;
                        continue;
                    }
                    let style = n
                        .attr("style-name")
                        .and_then(|s| self.book.list_styles.get(s))
                        .or(list);
                    for it in n.nodes() {
                        let hdr = it.name == "list-header";
                        if it.name == "list-item" || hdr {
                            self.blocks(&it.kids, env, style, list, depth + 1, hdr, out);
                        }
                    }
                }
                ("text", "section" | "index-body") => {
                    self.blocks(&n.kids, env, list, outer_list, depth, header, out);
                }
                _ => {}
            }
        }
    }

    fn paragraph(
        &mut self,
        n: &'a Node,
        env: &Env<'a>,
        list: Option<&'a Node>,
        depth: usize,
        header: bool,
    ) -> sd::Paragraph {
        let level = depth.saturating_sub(1);
        let in_list = depth > 0;
        let mut pv = self.level_view(env, level);
        if let Some(s) = n.attr("style-name") {
            pv.push_chain(self.book, "paragraph", s);
        }
        let mut para = sd::Paragraph {
            level: level.min(8) as u8,
            ..sd::Paragraph::default()
        };
        let rtl = matches!(pv.p("writing-mode").map(str::trim), Some("rl-tb" | "rl"));
        para.rtl = rtl;
        para.align = match pv.p("text-align").map_or("start", str::trim) {
            "center" => sd::Align::Center,
            "end" => {
                if rtl {
                    sd::Align::Left
                } else {
                    sd::Align::Right
                }
            }
            "right" => sd::Align::Right,
            "left" => sd::Align::Left,
            "justify" | "justified" => sd::Align::Justify,
            _ => sd::Align::Left,
        };
        // Indents.
        let own_left = pv.p("margin-left").and_then(emu);
        let own_indent = pv.p("text-indent").and_then(emu);
        let (mut mar_l, mut indent) = (own_left.unwrap_or(0.0), own_indent.unwrap_or(0.0));
        let mut bullet = None;
        let lvl_node = if in_list {
            list.and_then(|l| level_node(l, level + 1))
        } else {
            None
        };
        if let Some(ln) = lvl_node {
            let props = ln.child("list-level-properties");
            let align = props.and_then(|p| p.child("list-level-label-alignment"));
            if let Some(a) = align {
                let ml = a.attr("margin-left").and_then(emu).unwrap_or(0.0);
                let ti = a.attr("text-indent").and_then(emu).unwrap_or(0.0);
                mar_l = own_left.unwrap_or(ml);
                indent = own_indent.unwrap_or(ti);
            } else if let Some(p) = props {
                let before = p.attr("space-before").and_then(emu).unwrap_or(0.0);
                let label = p.attr("min-label-width").and_then(emu).unwrap_or(0.0);
                mar_l = before + label + own_left.unwrap_or(0.0);
                indent = -label;
            }
            if !header {
                bullet = self.bullet(ln, &pv);
            }
        }
        para.mar_l = mar_l.clamp(-1.0e8, 1.0e8);
        para.indent = indent.clamp(-1.0e8, 1.0e8);
        para.bullet = bullet;
        let sp = |k: &str| {
            pv.p(k)
                .and_then(pts)
                .map_or(sd::Spacing::Pts(0.0), |p| sd::Spacing::Pts(p.max(0.0)))
        };
        para.spc_before = sp("margin-top");
        para.spc_after = sp("margin-bottom");
        para.line_spacing = match pv.p("line-height").map(str::trim) {
            Some(s) if s.ends_with('%') => {
                sd::Spacing::Pct(pct(s).unwrap_or(1.0).clamp(0.05, 20.0))
            }
            Some(s) => match pts(s) {
                Some(p) if p > 0.0 => sd::Spacing::Pts(p),
                _ => sd::Spacing::Pct(1.0),
            },
            None => sd::Spacing::Pct(1.0),
        };
        para.end_size_pt = size_pt(&pv, "font-size", 18.0);
        let mut inl = Inl {
            runs: Vec::new(),
            prev_space: true,
        };
        self.inline(&n.kids, &pv, env, &mut inl, 0);
        // A trailing space of the paragraph is not text.
        if let Some(last) = inl.runs.last_mut() {
            if last.kind == sd::RunKind::Text && last.text.ends_with(' ') && last.text.len() > 1 {
                last.text.pop();
            }
        }
        para.runs = inl.runs;
        para
    }

    /// The bullet a list level style defines.
    fn bullet(&mut self, ln: &'a Node, pv: &View) -> Option<sd::Bullet> {
        let tp = ln.child("text-properties");
        let colour = tp.and_then(|t| {
            if t.attr("use-window-font-color").map(str::trim) == Some("true")
                && t.attr("color").is_none()
            {
                None
            } else {
                t.attr("color").and_then(color)
            }
        });
        let size = match tp.and_then(|t| t.attr("font-size")) {
            Some(s) => match pct(s) {
                Some(p) => sd::BulletSize::Pct(p.clamp(0.05, 10.0)),
                None => pts(s).map_or(sd::BulletSize::FollowText, sd::BulletSize::Pts),
            },
            None => sd::BulletSize::FollowText,
        };
        let _ = pv;
        // OpenSymbol's characters are Unicode already, but its black circle is far bigger than the
        // same character in a text face (LibreOffice draws "45 %" of it as a clearly visible dot):
        // the size is scaled to look the same.
        let open_symbol = tp
            .and_then(|t| t.attr("font-family"))
            .is_some_and(|f| f.contains("OpenSymbol"));
        let size = match size {
            sd::BulletSize::Pct(p) if open_symbol && ln.attr("bullet-char") == Some("\u{25CF}") => {
                sd::BulletSize::Pct((p * OPEN_SYMBOL_DOT_SCALE).min(10.0))
            }
            other => other,
        };
        let font = None;
        match ln.name.as_str() {
            "list-level-style-bullet" => {
                let ch = ln.attr("bullet-char").unwrap_or("\u{2022}");
                if ch.trim().is_empty() {
                    return None;
                }
                Some(sd::Bullet {
                    kind: sd::BulletKind::Char(ch.chars().take(4).collect()),
                    font,
                    color: colour,
                    size,
                })
            }
            "list-level-style-number" => {
                let fmt = ln.attr("num-format").unwrap_or("1").trim();
                if fmt.is_empty() {
                    return None;
                }
                let prefix = ln.attr("num-prefix").unwrap_or("").trim();
                let suffix = ln.attr("num-suffix").unwrap_or("").trim();
                let base = match fmt {
                    "a" => "alphaLc",
                    "A" => "alphaUc",
                    "i" => "romanLc",
                    "I" => "romanUc",
                    _ => "arabic",
                };
                let tail = match (prefix, suffix) {
                    ("(", ")") => "ParenBoth",
                    (_, ")") => "ParenR",
                    (_, ".") => "Period",
                    _ => "Plain",
                };
                let start = ln
                    .attr("start-value")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or(1)
                    .min(999_999);
                Some(sd::Bullet {
                    kind: sd::BulletKind::AutoNum {
                        scheme: format!("{base}{tail}"),
                        start,
                    },
                    font,
                    color: colour,
                    size,
                })
            }
            "list-level-style-image" => {
                let href = ln.attr("href")?;
                let key = self.image_for_href(href)?;
                // The picture's size is `fo:height` of the level's properties.
                let h = ln
                    .child("list-level-properties")
                    .and_then(|p| p.attr("height"))
                    .and_then(emu)
                    .filter(|h| *h > 0.0);
                Some(sd::Bullet {
                    kind: sd::BulletKind::Picture(sd::ImageFill::stretch(key)),
                    font,
                    color: None,
                    size: h.map_or(sd::BulletSize::FollowText, |h| {
                        sd::BulletSize::Pts(h / sd::EMU_PER_PT)
                    }),
                })
            }
            _ => None,
        }
    }

    /// The runs of inline content.
    fn inline(&mut self, kids: &[Kid], pv: &View<'a>, env: &Env<'a>, inl: &mut Inl, depth: usize) {
        if depth > MAX_INLINE_DEPTH {
            self.truncated = true;
            return;
        }
        for k in kids {
            if self.chars >= MAX_SLIDE_CHARS {
                self.truncated = true;
                return;
            }
            match k {
                Kid::T(t) => self.text_run(t, pv, env, inl, false),
                Kid::N(n) => {
                    if n.prefix == "text" {
                        match n.name.as_str() {
                            "s" => {
                                let c = n
                                    .attr("c")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                                    .unwrap_or(1)
                                    .min(1000);
                                self.text_run(&" ".repeat(c), pv, env, inl, true);
                            }
                            "tab" => self.text_run("    ", pv, env, inl, true),
                            "line-break" => {
                                let mut r = self.run_of(pv, env, "");
                                r.kind = sd::RunKind::LineBreak;
                                inl.runs.push(r);
                                inl.prev_space = true;
                            }
                            "span" => {
                                let mut sv = pv.clone();
                                if let Some(s) = n.attr("style-name") {
                                    sv.push_chain(self.book, "text", s);
                                }
                                self.inline(&n.kids, &sv, env, inl, depth + 1);
                            }
                            "page-number" => {
                                let delta = match n.attr("select-page").map(str::trim) {
                                    Some("previous") => -1,
                                    Some("next") => 1,
                                    _ => 0,
                                };
                                let no = (self.number as i64 + delta).max(0);
                                self.field(no.to_string(), pv, env, inl);
                            }
                            "page-count" => self.field(PAGE_COUNT_MARK.to_string(), pv, env, inl),
                            "page-name" => {
                                let name = self.page_name.clone();
                                self.field(name, pv, env, inl);
                            }
                            "note"
                            | "annotation"
                            | "annotation-end"
                            | "bookmark"
                            | "bookmark-start"
                            | "bookmark-end"
                            | "bookmark-ref"
                            | "reference-mark"
                            | "reference-mark-start"
                            | "reference-mark-end"
                            | "soft-page-break"
                            | "ruby-text"
                            | "hidden-paragraph"
                            | "hidden-text"
                            | "tracked-changes"
                            | "change"
                            | "change-start"
                            | "change-end"
                            | "toc-mark"
                            | "alphabetical-index-mark" => {}
                            "date" | "time" | "file-name" | "title" | "subject" | "author-name"
                            | "author-initials" | "initial-creator" | "creator" | "description"
                            | "keywords" | "modification-date" | "modification-time"
                            | "creation-date" | "creation-time" | "print-date" | "print-time"
                            | "editing-cycles" | "editing-duration" | "sender-firstname"
                            | "sender-lastname" | "sender-company" | "variable-set"
                            | "variable-get" | "user-defined" | "text-input" | "drop-down"
                            | "sequence" | "expression" | "chapter" | "word-count"
                            | "paragraph-count" | "character-count" | "image-count"
                            | "table-count" | "object-count" | "page-variable-get"
                            | "conditional-text" => {
                                let mut t = String::new();
                                super::styles::text_of(n, &mut t, 0);
                                if !t.trim().is_empty() {
                                    self.field(t, pv, env, inl);
                                }
                            }
                            _ => self.inline(&n.kids, pv, env, inl, depth + 1),
                        }
                    } else if n.prefix == "presentation" {
                        let t = match n.name.as_str() {
                            "footer" => self.footer.clone(),
                            "header" => self.header.clone(),
                            "date-time" => self.date_time.clone(),
                            _ => None,
                        };
                        if let Some(t) = t {
                            self.field(t, pv, env, inl);
                        }
                    } else if matches!(n.name.as_str(), "annotation" | "annotation-end") {
                        // office:annotation
                    } else {
                        self.inline(&n.kids, pv, env, inl, depth + 1);
                    }
                }
            }
        }
    }

    fn field(&mut self, text: String, pv: &View<'a>, env: &Env<'a>, inl: &mut Inl) {
        self.chars += text.len();
        let mut r = self.run_of(pv, env, &text);
        r.text = text;
        r.kind = sd::RunKind::Field;
        inl.runs.push(r);
        inl.prev_space = false;
    }

    /// Text, with ODF's white-space collapsing (a run of white space is one space, none at the start
    /// of a paragraph); `literal` text (`text:s`, `text:tab`) is kept as it is.
    fn text_run(&mut self, t: &str, pv: &View<'a>, env: &Env<'a>, inl: &mut Inl, literal: bool) {
        let mut s = String::with_capacity(t.len());
        if literal {
            s.push_str(t);
            inl.prev_space = false;
        } else {
            for c in t.chars() {
                if c.is_whitespace() {
                    if !inl.prev_space {
                        s.push(' ');
                        inl.prev_space = true;
                    }
                } else {
                    s.push(c);
                    inl.prev_space = false;
                }
            }
        }
        if s.is_empty() {
            return;
        }
        if pv.t("display").map(str::trim) == Some("none") {
            return;
        }
        self.chars += s.len();
        if self.chars > MAX_SLIDE_CHARS {
            self.truncated = true;
            let keep = MAX_SLIDE_CHARS.saturating_sub(self.chars - s.len());
            s = s.chars().take(keep).collect();
            if s.is_empty() {
                return;
            }
        }
        let mut r = self.run_of(pv, env, &s);
        if let Some(how) = pv.t("text-transform") {
            s = transform_text(&s, how.trim());
        }
        r.text = s;
        inl.runs.push(r);
    }

    /// A run with the properties of `pv` (its text is set by the caller).
    fn run_of(&self, pv: &View<'a>, env: &Env<'a>, sample: &str) -> sd::Run {
        let book = self.book;
        let script = first_strong(sample);
        let (size_key, weight_key, style_key) = match script {
            sd::fonts::Script::EastAsian => {
                ("font-size-asian", "font-weight-asian", "font-style-asian")
            }
            sd::fonts::Script::Complex => (
                "font-size-complex",
                "font-weight-complex",
                "font-style-complex",
            ),
            _ => ("font-size", "font-weight", "font-style"),
        };
        let mut r = sd::Run {
            size_pt: size_pt(pv, size_key, 18.0),
            ..sd::Run::default()
        };
        let bold = |s: &str| match s.trim() {
            "bold" => true,
            "normal" => false,
            n => n.parse::<u32>().is_ok_and(|w| w >= 600),
        };
        r.bold = pv
            .t(weight_key)
            .or_else(|| pv.t("font-weight"))
            .is_some_and(bold);
        r.italic = pv
            .t(style_key)
            .or_else(|| pv.t("font-style"))
            .is_some_and(|s| matches!(s.trim(), "italic" | "oblique"));
        // Fonts: `style:font-name` (declared) or `fo:font-family`, per script.
        let fam = |name: &str, family: &str| -> Option<String> {
            pv.t(name)
                .map(|n| book.font_family(n.trim()))
                .or_else(|| pv.t(family).map(super::styles::clean_family))
                .filter(|s| !s.is_empty())
        };
        r.font = sd::FontSpec {
            latin: fam("font-name", "font-family"),
            east_asian: fam("font-name-asian", "font-family-asian"),
            complex: fam("font-name-complex", "font-family-complex"),
            symbol: None,
        };
        let line = |style: &str, ty: &str| -> Option<bool> {
            // Some(true): a line; Some(false): none; None: not stated (a weaker layer decides)
            let s = pv.t(style).map(str::trim);
            let t = pv.t(ty).map(str::trim);
            match (s, t) {
                (Some("none"), _) | (None, Some("none")) => Some(false),
                (None, None) => None,
                _ => Some(true),
            }
        };
        r.underline = match line("text-underline-style", "text-underline-type") {
            Some(true) => {
                if pv.t("text-underline-type").map(str::trim) == Some("double") {
                    sd::Underline::Double
                } else {
                    sd::Underline::Single
                }
            }
            _ => sd::Underline::None,
        };
        r.strike = match line("text-line-through-style", "text-line-through-type") {
            Some(true) => {
                if pv.t("text-line-through-type").map(str::trim) == Some("double") {
                    sd::Strike::Double
                } else {
                    sd::Strike::Single
                }
            }
            _ => sd::Strike::None,
        };
        let auto = env.auto.unwrap_or(Rgba::BLACK);
        let colour = match pv.t("color").and_then(color) {
            Some(c) if pv.t("use-window-font-color").map(str::trim) != Some("true") => c,
            Some(c) => c,
            None => auto,
        };
        r.fill = sd::Fill::Solid(colour);
        r.highlight = pv.t("background-color").and_then(color);
        r.baseline_pct = match pv
            .t("text-position")
            .and_then(|p| p.split_whitespace().next())
        {
            Some("super") => 33.0,
            Some("sub") => -33.0,
            Some(p) => pct(p).map_or(0.0, |v| (v * 100.0).clamp(-100.0, 100.0)),
            None => 0.0,
        };
        r.spacing_pt = pv
            .t("letter-spacing")
            .and_then(|s| {
                if s.trim() == "normal" {
                    Some(0.0)
                } else {
                    pts(s)
                }
            })
            .unwrap_or(0.0)
            .clamp(-50.0, 100.0);
        r.caps = match (
            pv.t("text-transform").map(str::trim),
            pv.t("font-variant").map(str::trim),
        ) {
            (Some("uppercase"), _) => sd::Caps::All,
            (_, Some("small-caps")) => sd::Caps::Small,
            _ => sd::Caps::None,
        };
        let _ = angle_deg;
        r
    }

    /// The colour of automatic text on a fill (or on the slide when the fill paints nothing).
    pub(super) fn auto_color(&self, fill: &sd::Fill) -> Rgba {
        let probe = match fill {
            sd::Fill::Solid(c) if c.a > 0.5 => Some(*c),
            sd::Fill::Gradient(g) => g.stops.first().map(|s| s.1).filter(|c| c.a > 0.5),
            sd::Fill::Pattern { bg, .. } if bg.a > 0.5 => Some(*bg),
            _ => None,
        };
        let bg = probe.unwrap_or(self.page_bg);
        if luminance(bg) < 0.5 {
            Rgba::WHITE
        } else {
            Rgba::BLACK
        }
    }
}

/// The level `lvl` (1-based) among the children of a `text:list-style`.
fn level_node(list: &Node, lvl: usize) -> Option<&Node> {
    list.nodes().find(|n| {
        n.name.starts_with("list-level-style")
            && n.attr("level").and_then(|l| l.trim().parse::<usize>().ok()) == Some(lvl)
    })
}
