//! The shapes of a page or master page as scene items (see the module documentation of
//! [`super`] for the geometry rules).

use crate::preview::office::slide_draw as sd;
use sd::{GeomPath, Geometry, PathCmd, Pt};

use super::body::Env;
use super::styles::View;
use super::units::{box_under, number, parse_points, parse_svg_path, pct, shift, view_box, Mat};
use super::*;

/// Which of a master page's footer-line placeholders the slide shows.
#[derive(Debug, Clone, Copy)]
pub(super) struct Furniture {
    pub footer: bool,
    pub date_time: bool,
    pub page_number: bool,
    pub header: bool,
}

/// The classes of placeholders that are never drawn from a master page.
fn is_master_placeholder(class: &str) -> bool {
    !matches!(class, "header" | "footer" | "date-time" | "page-number")
}

/// A shape's own rectangle and the matrix that places it.
struct Place {
    m: Option<Mat>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Place {
    fn xfrm(&self) -> Option<sd::Xfrm> {
        match self.m {
            Some(m) => box_under(m, self.x, self.y, self.w, self.h),
            None => Some(sd::Xfrm::rect(self.x, self.y, self.w, self.h)),
        }
    }
}

/// Whether `n` holds text.
fn has_text(n: &Node, depth: usize) -> bool {
    if depth > 40 {
        return false;
    }
    n.kids.iter().any(|k| match k {
        Kid::T(t) => !t.trim().is_empty() && depth > 0,
        Kid::N(c) => match c.name.as_str() {
            "p" | "h" if c.prefix == "text" => !c.kids.is_empty() && has_any_content(c),
            "enhanced-geometry" | "image" => false,
            _ => has_text(c, depth + 1),
        },
    })
}

fn has_any_content(p: &Node) -> bool {
    p.kids.iter().any(|k| match k {
        Kid::T(t) => !t.trim().is_empty(),
        Kid::N(c) => !matches!(
            c.name.as_str(),
            "bookmark" | "bookmark-start" | "bookmark-end"
        ),
    })
}

fn pt_attr(n: &Node, q: &str) -> Option<f64> {
    qattr(n, q).and_then(emu)
}

impl<'a> Sb<'a> {
    /// Draws the shapes of a master page.
    pub(super) fn build_master(&mut self, m: &'a Node, f: &Furniture, out: &mut Vec<sd::Item>) {
        self.build_nodes(m.nodes(), 0, out, Some(f));
    }

    /// The shapes among `nodes`, appended to `out` in paint order (`draw:z-index` decides when it
    /// is there). Over the item budget or the group depth the rest is left out and the scene says
    /// so. `master` is `Some` when the nodes belong to a master page.
    pub(super) fn build_nodes(
        &mut self,
        nodes: impl Iterator<Item = &'a Node>,
        depth: usize,
        out: &mut Vec<sd::Item>,
        master: Option<&Furniture>,
    ) {
        let mut list: Vec<&'a Node> = nodes.filter(|n| n.prefix == "draw").collect();
        if list.iter().any(|n| n.attr("z-index").is_some()) {
            // Stable: shapes without a z-index keep their place relative to one another.
            let mut keyed: Vec<(i64, usize, &Node)> = list
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let z = n
                        .attr("z-index")
                        .and_then(|z| z.trim().parse::<i64>().ok())
                        .unwrap_or(i as i64);
                    (z, i, *n)
                })
                .collect();
            keyed.sort_by_key(|k| (k.0, k.1));
            list = keyed.into_iter().map(|k| k.2).collect();
        }
        for n in list {
            if self.items >= self.max_items {
                self.truncated = true;
                return;
            }
            if depth > MAX_GROUP_DEPTH {
                self.truncated = true;
                return;
            }
            match n.name.as_str() {
                "frame" => self.frame(n, out, master),
                "g" => self.group(n, depth, out, master),
                "a" => self.build_nodes(n.nodes(), depth + 1, out, master),
                name if SHAPES.contains(&name) => self.drawing(n, name, out, master),
                _ => {}
            }
        }
    }

    /// Whether a shape of a master page is drawn on the slide.
    fn master_shows(&self, n: &Node, f: Option<&Furniture>) -> bool {
        let Some(f) = f else { return true };
        match qattr(n, "presentation:class") {
            None => true,
            Some(c) if is_master_placeholder(c) => false,
            Some("header") => f.header,
            Some("footer") => f.footer,
            Some("date-time") => f.date_time,
            Some("page-number") => f.page_number,
            Some(_) => false,
        }
    }

    /// An empty placeholder is not drawn (LibreOffice shows it in the editor only).
    fn empty_placeholder(&self, n: &Node) -> bool {
        qattr(n, "presentation:placeholder").map(str::trim) == Some("true") && !has_text(n, 0)
    }

    fn look(&self, n: &'a Node) -> (View<'a>, Env<'a>) {
        let env = Env {
            pres: qattr(n, "presentation:style-name"),
            gfx: qattr(n, "draw:style-name"),
            text_style: qattr(n, "draw:text-style-name"),
            class: qattr(n, "presentation:class"),
            auto: None,
            cell: None,
        };
        let view = self.shape_view(&env);
        (view, env)
    }

    /// `svg:x` .. and `draw:transform` of a shape; a shape with no position of its own takes the
    /// master page's placeholder of the same class.
    fn place(&self, n: &Node) -> Option<Place> {
        let m = qattr(n, "draw:transform").and_then(super::units::draw_transform);
        let (mut x, mut y, mut w, mut h) = (
            pt_attr(n, "svg:x"),
            pt_attr(n, "svg:y"),
            pt_attr(n, "svg:width"),
            pt_attr(n, "svg:height"),
        );
        if let Some(class) = qattr(n, "presentation:class") {
            if let Some(r) = self.master_frames.and_then(|f| f.get(class)) {
                if x.is_none() && y.is_none() && m.is_none() {
                    x = Some(r.x as f64);
                    y = Some(r.y as f64);
                }
                w = w.or(Some(r.w as f64));
                h = h.or(Some(r.h as f64));
            }
        }
        let (w, h) = (w?.max(0.0), h?.max(0.0));
        let (x, y) = match (x, y, m.is_some()) {
            (Some(x), Some(y), _) => (x, y),
            (None, None, true) => (0.0, 0.0),
            (Some(x), None, _) => (x, 0.0),
            (None, Some(y), _) => (0.0, y),
            _ => return None,
        };
        Some(Place { m, x, y, w, h })
    }

    // -----------------------------------------------------------------------------------------
    // frames
    // -----------------------------------------------------------------------------------------

    fn frame(&mut self, n: &'a Node, out: &mut Vec<sd::Item>, master: Option<&Furniture>) {
        let class = qattr(n, "presentation:class");
        if class == Some("page") || !self.master_shows(n, master) || self.empty_placeholder(n) {
            return;
        }
        let Some(place) = self.place(n) else { return };
        let Some(xf) = place.xfrm() else { return };
        let (mut view, mut env) = self.look(n);
        let fill = self.fill(&view, place.w, place.h).unwrap_or(sd::Fill::None);
        env.auto = Some(self.auto_color(&fill));
        // A frame's style is a graphic style; the text of a text box is styled by it too.
        let line = self.line(&view);
        let effects = self.effects(&view);
        // The picture LibreOffice stores beside a table is a preview of it: the table's own task
        // draws the table, and until then the preview is not drawn as a placeholder.
        let mut picture_done = n.nodes().any(|c| c.name == "table");
        for c in n.nodes() {
            if self.items >= self.max_items {
                self.truncated = true;
                return;
            }
            match c.name.as_str() {
                "text-box" => {
                    self.items += 1;
                    let text = self.text_body(&c.kids, &env, &view);
                    if text.is_none() && !fill.is_visible() && line.is_none() {
                        self.items -= 1;
                        continue;
                    }
                    let mut s = sd::ShapeItem::new(xf, Geometry::Rect);
                    s.fill = fill.clone();
                    s.line = line.clone();
                    s.effects = effects;
                    s.text = text;
                    self.fit_text(&mut s, &view);
                    out.push(sd::Item::Shape(s));
                }
                "image" if !picture_done => {
                    self.items += 1;
                    let item = self.picture(n, &view, xf, &line, effects);
                    out.push(item);
                    picture_done = true;
                }
                "object" | "object-ole" => {
                    // A chart that is drawn replaces the picture stored beside it.
                    if self.frame_chart(n, c, xf, out) {
                        picture_done = true;
                    }
                }
                "table" => {
                    self.frame_table(n, c, xf, out);
                }
                _ => {}
            }
        }
        let _ = &mut view;
    }

    /// A table (`table:table` in a frame) as scene items (see [`super::table`]).
    fn frame_table(
        &mut self,
        frame: &'a Node,
        table: &'a Node,
        xf: sd::Xfrm,
        out: &mut Vec<sd::Item>,
    ) {
        self.table_items(frame, table, xf, out);
    }

    /// The picture at `href` when the part has no file extension (the replacement image of an
    /// embedded object), by what its bytes say it is.
    fn sniffed(&mut self, href: &str) -> Option<String> {
        let part = part_of(href)?;
        let name = part.rsplit('/').next().unwrap_or(&part);
        if name.contains('.') {
            return None;
        }
        self.media.load_sniffed(&part)
    }

    /// The picture of a frame: the first `draw:image` that can be shown (LibreOffice writes a PNG
    /// replacement after an SVG or SVM original), else the placeholder.
    fn picture(
        &mut self,
        frame: &'a Node,
        view: &View<'a>,
        mut xf: sd::Xfrm,
        line: &Option<sd::Line>,
        effects: sd::Effects,
    ) -> sd::Item {
        let mut key: Option<String> = None;
        for img in frame.nodes().filter(|c| c.name == "image") {
            if let Some(href) = img.attr("href") {
                if let Some(k) = self.image_for_href(href).or_else(|| self.sniffed(href)) {
                    key = Some(k);
                    break;
                }
            }
        }
        let shown = key.is_some();
        let key = key.unwrap_or_else(|| MISSING_PICTURE.to_string());
        let mut image = sd::ImageFill::stretch(key.clone());
        if shown {
            if let Some(c) = view.g("clip").and_then(parse_clip) {
                if let Some((nw, nh)) = self.native_emu(&key) {
                    if nw > 0.0 && nh > 0.0 {
                        image.crop = (c.3 / nw, c.0 / nh, c.1 / nw, c.2 / nh);
                    }
                }
            }
        }
        if let Some(a) = view.g("image-opacity").and_then(pct) {
            image.alpha = a.clamp(0.0, 1.0);
        }
        if let Some(m) = view.g("mirror") {
            if m.contains("horizontal") && !m.contains("on-") {
                xf.flip_h = !xf.flip_h;
            }
            if m.contains("vertical") {
                xf.flip_v = !xf.flip_v;
            }
        }
        sd::Item::Picture(sd::PictureItem {
            xfrm: xf,
            image,
            geom: Geometry::Rect,
            line: line.clone(),
            effects,
        })
    }

    // -----------------------------------------------------------------------------------------
    // groups
    // -----------------------------------------------------------------------------------------

    fn group(
        &mut self,
        n: &'a Node,
        depth: usize,
        out: &mut Vec<sd::Item>,
        master: Option<&Furniture>,
    ) {
        if depth >= MAX_GROUP_DEPTH {
            self.truncated = true;
            return;
        }
        let mut kids = Vec::new();
        self.build_nodes(n.nodes(), depth + 1, &mut kids, master);
        if kids.is_empty() {
            return;
        }
        let (w, h) = self.size;
        // The children are in page coordinates: the group maps them onto themselves, unless the
        // group has a transform of its own, which then moves the whole page-sized box.
        let m = qattr(n, "draw:transform").and_then(super::units::draw_transform);
        let xfrm = match m {
            Some(m) => match box_under(m, 0.0, 0.0, w, h) {
                Some(x) => x,
                None => return,
            },
            None => sd::Xfrm::rect(0.0, 0.0, w, h),
        };
        out.push(sd::Item::Group(sd::GroupItem {
            xfrm,
            child_off: (0.0, 0.0),
            child_ext: (w, h),
            items: kids,
        }));
    }

    // -----------------------------------------------------------------------------------------
    // drawing shapes
    // -----------------------------------------------------------------------------------------

    fn drawing(
        &mut self,
        n: &'a Node,
        name: &str,
        out: &mut Vec<sd::Item>,
        master: Option<&Furniture>,
    ) {
        if !self.master_shows(n, master) || self.empty_placeholder(n) {
            return;
        }
        let (view, mut env) = self.look(n);
        let built = match name {
            "line" | "measure" => self.line_shape(n),
            "connector" => self.connector(n),
            "caption" => self.caption_shape(n, &view),
            "rect" => self.rect_shape(n, &view).map(|s| (s, None)),
            "ellipse" | "circle" => self.ellipse_shape(n),
            "polyline" | "polygon" => self.poly_shape(n, name == "polygon"),
            "regular-polygon" => self.regular_polygon(n),
            "path" => self.path_shape(n),
            "custom-shape" => self.custom_shape(n),
            _ => None,
        };
        let Some((mut shape, extra)) = built else {
            return;
        };
        let (w, h) = (shape.xfrm.w, shape.xfrm.h);
        shape.fill = self.fill(&view, w, h).unwrap_or(sd::Fill::None);
        // Lines and open paths are not filled.
        if matches!(name, "line" | "measure" | "polyline" | "connector") {
            shape.fill = sd::Fill::None;
        }
        shape.line = self.line(&view);
        shape.effects = self.effects(&view);
        env.auto = Some(self.auto_color(&shape.fill));
        shape.text = self.text_body(&n.kids, &env, &view);
        // LibreOffice does not wrap the text of a plain rectangle, ellipse or circle (only a
        // custom shape's and a text box's follows `fo:wrap-option`): a long line overflows the
        // shape on both sides (centred) or on the right.
        if matches!(name, "rect" | "ellipse" | "circle") {
            if let Some(t) = shape.text.as_mut() {
                t.wrap = false;
            }
        }
        if name == "custom-shape" && qattr_enh(n).is_some_and(|t| t.starts_with("fontwork-")) {
            // Fontwork: the geometry is the warp the text follows, not an outline. The text is
            // drawn as ordinary text in the box, in the shape's fill colour.
            let colour = match &shape.fill {
                sd::Fill::Solid(c) => Some(*c),
                sd::Fill::Gradient(g) => g.stops.get(g.stops.len() / 2).map(|s| s.1),
                sd::Fill::Pattern { fg, .. } => Some(*fg),
                _ => None,
            };
            if let (Some(c), Some(t)) = (colour, shape.text.as_mut()) {
                for p in &mut t.paragraphs {
                    for r in &mut p.runs {
                        r.fill = sd::Fill::Solid(c);
                    }
                }
            }
            shape.fill = sd::Fill::None;
            shape.line = None;
            shape.geom = Geometry::Rect;
            shape.text_rect = None;
        }
        if !shape.fill.is_visible() && shape.line.is_none() && shape.text.is_none() {
            return;
        }
        self.fit_text(&mut shape, &view);
        // A caption's pointer is drawn under its box.
        if let Some(mut extra) = extra {
            // (The pointer is a line in the caption's own outline.)
            extra.line = shape.line.clone();
            self.items += 1;
            out.push(sd::Item::Shape(extra));
        }
        self.items += 1;
        out.push(sd::Item::Shape(shape));
    }

    /// `draw:caption`: its rectangle and the pointer to `draw:caption-point-x/y` (relative to the
    /// rectangle's top-left corner). The pointer is a straight line from the middle of the side
    /// that faces the point (the left or right side when the point is beside the rectangle, else
    /// the top or bottom one), `draw:caption-gap` away from it, to the point. Every
    /// `draw:caption-type` is drawn so (LibreOffice's angled types differ only when the point is
    /// far off the sides, which it draws as the same straight line to the point in its renderings).
    /// A point inside the rectangle has no pointer.
    fn caption_shape(
        &mut self,
        n: &Node,
        view: &View,
    ) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let rect = self.rect_shape(n, view)?;
        let place = self.place(n)?;
        let (Some(px), Some(py)) = (
            pt_attr(n, "draw:caption-point-x"),
            pt_attr(n, "draw:caption-point-y"),
        ) else {
            return Some((rect, None));
        };
        let (w, h) = (place.w, place.h);
        if (0.0..=w).contains(&px) && (0.0..=h).contains(&py) {
            return Some((rect, None));
        }
        let gap = view
            .g("caption-gap")
            .and_then(emu)
            .map_or(0.0, |g| g.clamp(0.0, 1.0e8));
        let (sx, sy) = if px < 0.0 {
            (-gap, h / 2.0)
        } else if px > w {
            (w + gap, h / 2.0)
        } else if py < 0.0 {
            (w / 2.0, -gap)
        } else {
            (w / 2.0, h + gap)
        };
        let at = |x: f64, y: f64| match place.m {
            Some(m) => super::units::apply(m, place.x + x, place.y + y),
            None => (place.x + x, place.y + y),
        };
        Some((rect, Some(line_between(at(sx, sy), at(px, py)))))
    }

    fn rect_shape(&mut self, n: &Node, _view: &View) -> Option<sd::ShapeItem> {
        let place = self.place(n)?;
        let xf = place.xfrm()?;
        let r = pt_attr(n, "draw:corner-radius")
            .or_else(|| pt_attr(n, "svg:rx"))
            .filter(|r| *r > 0.0);
        let mut s = sd::ShapeItem::new(xf, Geometry::Rect);
        if let Some(r) = r {
            let side = xf.w.min(xf.h);
            if side > 0.0 {
                let adj = (r / side * 100_000.0).clamp(0.0, 50_000.0);
                if let Some(g) =
                    sd::geom::preset("roundRect", &[("adj".to_string(), adj)], xf.w, xf.h)
                {
                    s.geom = Geometry::Paths(g.paths);
                    s.text_rect = g.text_rect;
                }
            }
        }
        Some(s)
    }

    /// `draw:ellipse` / `draw:circle`, with `draw:kind` (full, section, cut, arc).
    fn ellipse_shape(&mut self, n: &Node) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let place = self.place(n)?;
        let xf = place.xfrm()?;
        let kind = qattr(n, "draw:kind").map_or("full", str::trim);
        if kind == "full" {
            return Some((sd::ShapeItem::new(xf, Geometry::Ellipse), None));
        }
        let start = qattr(n, "draw:start-angle").and_then(number).unwrap_or(0.0);
        let end = qattr(n, "draw:end-angle").and_then(number).unwrap_or(360.0);
        // LibreOffice's angles run counter-clockwise from three o'clock; the model's run clockwise.
        let st = -start;
        let mut sweep = -((end - start).rem_euclid(360.0));
        if sweep == 0.0 {
            sweep = -360.0;
        }
        let (wr, hr) = (xf.w / 2.0, xf.h / 2.0);
        let (cx, cy) = (wr, hr);
        let t = st.to_radians();
        // the point of the ellipse at the (visual) angle `st`
        let (sx, sy) = (t.cos() * hr, t.sin() * wr);
        let norm = sx.hypot(sy).max(1e-12);
        let p0 = Pt::new(cx + sx / norm * wr, cy + sy / norm * hr);
        let mut cmds = vec![
            PathCmd::MoveTo(p0),
            PathCmd::ArcTo {
                wr,
                hr,
                st_deg: st,
                sw_deg: sweep,
            },
        ];
        let mut fill_mode = sd::PathFill::Norm;
        match kind {
            "section" => {
                cmds.push(PathCmd::LineTo(Pt::new(cx, cy)));
                cmds.push(PathCmd::Close);
            }
            "cut" => cmds.push(PathCmd::Close),
            _ => fill_mode = sd::PathFill::None,
        }
        let mut s = sd::ShapeItem::new(
            xf,
            Geometry::Paths(vec![GeomPath {
                w: 0.0,
                h: 0.0,
                fill_mode,
                stroke: true,
                cmds,
            }]),
        );
        s.text_rect = None;
        Some((s, None))
    }

    /// `draw:line` / `draw:measure`: a line between `svg:x1,y1` and `svg:x2,y2`.
    fn line_shape(&mut self, n: &Node) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let p = self.endpoints(n)?;
        Some((line_between(p.0, p.1), None))
    }

    fn endpoints(&self, n: &Node) -> Option<((f64, f64), (f64, f64))> {
        let (x1, y1, x2, y2) = (
            pt_attr(n, "svg:x1")?,
            pt_attr(n, "svg:y1")?,
            pt_attr(n, "svg:x2")?,
            pt_attr(n, "svg:y2")?,
        );
        match qattr(n, "draw:transform").and_then(super::units::draw_transform) {
            Some(m) => Some((
                super::units::apply(m, x1, y1),
                super::units::apply(m, x2, y2),
            )),
            None => Some(((x1, y1), (x2, y2))),
        }
    }

    /// `draw:connector`: its `svg:d` when it has one, else a line / elbow / curve between its ends.
    fn connector(&mut self, n: &Node) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        if qattr(n, "svg:d").is_some() {
            if let Some(r) = self.path_shape(n) {
                return Some(r);
            }
        }
        let (a, b) = self.endpoints(n)?;
        let kind = qattr(n, "draw:type").map_or("standard", str::trim);
        if kind == "line" || ((a.0 - b.0).abs() < 1.0 || (a.1 - b.1).abs() < 1.0) {
            return Some((line_between(a, b), None));
        }
        let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
        let (w, h) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
        let p = |q: (f64, f64)| Pt::new(q.0 - x0, q.1 - y0);
        let (pa, pb) = (p(a), p(b));
        let cmds = if kind == "curve" {
            let mx = (pa.x + pb.x) / 2.0;
            vec![
                PathCmd::MoveTo(pa),
                PathCmd::CubicTo(Pt::new(mx, pa.y), Pt::new(mx, pb.y), pb),
            ]
        } else if w >= h {
            let mx = (pa.x + pb.x) / 2.0;
            vec![
                PathCmd::MoveTo(pa),
                PathCmd::LineTo(Pt::new(mx, pa.y)),
                PathCmd::LineTo(Pt::new(mx, pb.y)),
                PathCmd::LineTo(pb),
            ]
        } else {
            let my = (pa.y + pb.y) / 2.0;
            vec![
                PathCmd::MoveTo(pa),
                PathCmd::LineTo(Pt::new(pa.x, my)),
                PathCmd::LineTo(Pt::new(pb.x, my)),
                PathCmd::LineTo(pb),
            ]
        };
        Some((
            sd::ShapeItem::new(
                sd::Xfrm::rect(x0, y0, w, h),
                Geometry::Paths(vec![GeomPath {
                    w: 0.0,
                    h: 0.0,
                    fill_mode: sd::PathFill::None,
                    stroke: true,
                    cmds,
                }]),
            ),
            None,
        ))
    }

    /// `draw:polyline` / `draw:polygon`: `draw:points` in the shape's `svg:viewBox`.
    fn poly_shape(
        &mut self,
        n: &Node,
        close: bool,
    ) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let place = self.place(n)?;
        let xf = place.xfrm()?;
        let (vx, vy, vw, vh) = view_box(qattr(n, "svg:viewBox")?)?;
        let mut cmds = parse_points(qattr(n, "draw:points")?, close)?;
        shift(&mut cmds, vx, vy);
        let fill_mode = if close {
            sd::PathFill::Norm
        } else {
            sd::PathFill::None
        };
        Some((
            sd::ShapeItem::new(
                xf,
                Geometry::Paths(vec![GeomPath {
                    w: vw,
                    h: vh,
                    fill_mode,
                    stroke: true,
                    cmds,
                }]),
            ),
            None,
        ))
    }

    /// `draw:regular-polygon`: `draw:corners` points on the ellipse in the box, the first at the
    /// top; a concave one (`draw:concave="true"`) alternates with the inner radius
    /// `1 - draw:sharpness`.
    fn regular_polygon(&mut self, n: &Node) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let place = self.place(n)?;
        let xf = place.xfrm()?;
        let corners = qattr(n, "draw:corners")
            .and_then(|c| c.trim().parse::<usize>().ok())
            .unwrap_or(5)
            .clamp(3, 1000);
        let concave = qattr(n, "draw:concave").map(str::trim) == Some("true");
        let sharp = qattr(n, "draw:sharpness")
            .and_then(pct)
            .unwrap_or(0.0)
            .clamp(0.0, 0.99);
        let (cx, cy, rx, ry) = (xf.w / 2.0, xf.h / 2.0, xf.w / 2.0, xf.h / 2.0);
        let mut cmds = Vec::new();
        let count = if concave { corners * 2 } else { corners };
        for i in 0..count {
            let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * i as f64 / count as f64;
            let k = if concave && i % 2 == 1 {
                1.0 - sharp
            } else {
                1.0
            };
            let p = Pt::new(cx + rx * k * a.cos(), cy + ry * k * a.sin());
            cmds.push(if i == 0 {
                PathCmd::MoveTo(p)
            } else {
                PathCmd::LineTo(p)
            });
        }
        cmds.push(PathCmd::Close);
        Some((
            sd::ShapeItem::new(
                xf,
                Geometry::Paths(vec![GeomPath {
                    w: 0.0,
                    h: 0.0,
                    fill_mode: sd::PathFill::Norm,
                    stroke: true,
                    cmds,
                }]),
            ),
            None,
        ))
    }

    /// `draw:path`: `svg:d` in `svg:viewBox`.
    fn path_shape(&mut self, n: &Node) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let place = self.place(n)?;
        let xf = place.xfrm()?;
        let (vx, vy, vw, vh) = view_box(qattr(n, "svg:viewBox")?)?;
        let (mut cmds, cut) = parse_svg_path(qattr(n, "svg:d")?)?;
        if cut {
            self.truncated = true;
        }
        shift(&mut cmds, vx, vy);
        let open = !cmds.iter().any(|c| matches!(c, PathCmd::Close));
        let _ = open;
        Some((
            sd::ShapeItem::new(
                xf,
                Geometry::Paths(vec![GeomPath {
                    w: vw,
                    h: vh,
                    fill_mode: sd::PathFill::Norm,
                    stroke: true,
                    cmds,
                }]),
            ),
            None,
        ))
    }

    /// `draw:custom-shape`: the enhanced geometry's path; without one, the preset of its
    /// `draw:type` (`ooxml-<preset>`, or a legacy LibreOffice name through
    /// [`legacy_preset`]); a shape with neither is a rectangle.
    fn custom_shape(&mut self, n: &Node) -> Option<(sd::ShapeItem, Option<sd::ShapeItem>)> {
        let place = self.place(n)?;
        let mut xf = place.xfrm()?;
        let eg_node = n.nodes().find(|c| c.name == "enhanced-geometry");
        // A shape mirrored in one direction only is turned the other way round: LibreOffice
        // applies the rotation of `draw:transform` to the mirrored geometry with its sign
        // reversed, about the same centre (checked against its renderings of a mirrored and
        // rotated arrow at several angles; mirrored in both directions it is the plain rotation).
        if let Some(eg) = eg_node {
            let mirrored = |k: &str| {
                eg.attr(k)
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
            };
            if mirrored("mirror-horizontal") != mirrored("mirror-vertical") {
                xf.rot_deg = -xf.rot_deg;
            }
        }
        let mut s = sd::ShapeItem::new(xf, Geometry::Rect);
        let Some(eg) = eg_node else {
            return Some((s, None));
        };
        if let Some((paths, text)) = sd::odf_geom::enhanced_geometry(eg, xf.w, xf.h) {
            s.geom = Geometry::Paths(paths);
            s.text_rect = text;
            return Some((s, None));
        }
        let ty = eg.attr("type").map_or("", str::trim);
        let preset = if let Some(p) = ty.strip_prefix("ooxml-") {
            Some(p)
        } else {
            legacy_preset(ty)
        };
        if let Some(name) = preset {
            let names = sd::geom::preset_adjust_names(name).unwrap_or_default();
            let mods: Vec<f64> = eg
                .attr("modifiers")
                .map(|m| {
                    m.split_whitespace()
                        .take(32)
                        .filter_map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()))
                        .collect()
                })
                .unwrap_or_default();
            let adj: Vec<(String, f64)> = names.into_iter().zip(mods).collect();
            if let Some(g) = sd::geom::preset(name, &adj, xf.w, xf.h) {
                if g.truncated {
                    self.truncated = true;
                }
                s.geom = Geometry::Paths(g.paths);
                s.text_rect = g.text_rect;
            }
        }
        Some((s, None))
    }

    // -----------------------------------------------------------------------------------------
    // text fitting
    // -----------------------------------------------------------------------------------------

    /// Shrinks the text of a shape that asks for it (`style:shrink-to-fit`) until it fits the
    /// text rectangle, and scales it to fill the shape for `draw:fit-to-size`.
    fn fit_text(&mut self, s: &mut sd::ShapeItem, view: &View) {
        let Some(body) = s.text.as_mut() else { return };
        let stretch = matches!(
            view.g("fit-to-size").map(str::trim),
            Some("true" | "all" | "proportional")
        );
        let shrink = matches!(body.autofit, sd::AutoFit::Normal { .. });
        if !(stretch || shrink) || body.vert != sd::Vert::Horz {
            return;
        }
        let (tx, ty, tw, th) = match s.text_rect {
            Some((l, t, r, b)) => (l, t, r - l, b - t),
            None => (0.0, 0.0, s.xfrm.w, s.xfrm.h),
        };
        let _ = (tx, ty);
        let w = (tw - body.insets.0 - body.insets.2) / sd::EMU_PER_PX;
        let h = (th - body.insets.1 - body.insets.3) / sd::EMU_PER_PX;
        if !(w > 1.0 && h > 1.0) {
            return;
        }
        let fits = |b: &sd::TextBody| sd::text::layout(b, w, h).content_h <= h + 0.5;
        if shrink && !stretch {
            if fits(body) {
                return;
            }
            let (mut lo, mut hi) = (0.25f64, 1.0f64);
            for _ in 0..8 {
                let mid = (lo + hi) / 2.0;
                let mut t = body.clone();
                t.autofit = sd::AutoFit::Normal {
                    font_scale: mid,
                    ln_spc_reduction: if mid < 0.9 { 0.1 } else { 0.0 },
                };
                if fits(&t) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            body.autofit = sd::AutoFit::Normal {
                font_scale: lo,
                ln_spc_reduction: if lo < 0.9 { 0.1 } else { 0.0 },
            };
        } else if stretch {
            let scaled = |k: f64| {
                let mut t = body.clone();
                for p in &mut t.paragraphs {
                    p.end_size_pt *= k;
                    for r in &mut p.runs {
                        r.size_pt *= k;
                    }
                    for sp in [&mut p.spc_before, &mut p.spc_after] {
                        if let sd::Spacing::Pts(v) = sp {
                            *v *= k;
                        }
                    }
                }
                t
            };
            let (mut lo, mut hi) = (0.05f64, 20.0f64);
            for _ in 0..12 {
                let mid = (lo * hi).sqrt();
                if fits(&scaled(mid)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            *body = scaled(lo);
            body.autofit = sd::AutoFit::None;
        }
    }
}

/// A straight line shape between two points (page coordinates).
fn line_between(a: (f64, f64), b: (f64, f64)) -> sd::ShapeItem {
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let xf = sd::Xfrm {
        x: x0,
        y: y0,
        w: (a.0 - b.0).abs(),
        h: (a.1 - b.1).abs(),
        rot_deg: 0.0,
        flip_h: a.0 > b.0,
        flip_v: a.1 > b.1,
    };
    sd::ShapeItem::new(xf, Geometry::Line)
}

/// `fo:clip="rect(top, right, bottom, left)"` as EMU lengths (top, right, bottom, left).
fn parse_clip(s: &str) -> Option<(f64, f64, f64, f64)> {
    let inner = s.trim().strip_prefix("rect(")?.strip_suffix(')')?;
    let v: Vec<f64> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| if t == "auto" { Some(0.0) } else { emu(t) })
        .collect::<Option<Vec<_>>>()?;
    (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
}

/// The `draw:type` of a custom shape's enhanced geometry.
fn qattr_enh(n: &Node) -> Option<&str> {
    n.nodes()
        .find(|c| c.name == "enhanced-geometry")
        .and_then(|e| e.attr("type"))
}
