//! What an SVG picture (a picture file, or a converted metafile) will cost the drawing process
//! once it is drawn, found by a flat scan of its markup: elements, path segments, the length of its
//! outlines and the area its elements paint. The picture is untrusted and the writer does not
//! interpret it (no transforms, no styles, no `use`), so every simplification here errs towards
//! costing more.
//!
//! One example the bytes of the markup say nothing about: 30 000 paths of 20 bytes, each stroked
//! 100 000 units wide, paint the whole picture 30 000 times and keep the drawing process busy for
//! seconds, where a 3 MB logo of short relative paths takes it 0.4 s.

/// The drawing cost of an SVG picture, in the units of the picture's own coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vector {
    /// Width and height of the picture (`viewBox`, else `width` / `height`, else 100 x 100).
    pub width: f64,
    pub height: f64,
    /// Elements (start tags).
    pub els: f64,
    /// Path segments.
    pub segs: f64,
    /// Length of the outlines (each segment counted up to twice the picture's longer side).
    pub len: f64,
    /// Area the elements paint, units^2: the bounding box of a path (widened by half its stroke
    /// width when stroked), the size of a rectangle, circle or ellipse, the whole picture for
    /// every other graphic element; each clipped to the picture.
    pub area: f64,
    /// Work per pixel of the filter primitives (`fe...` elements), in the units of `svg_guard`
    /// (a Gaussian blur is 12, most others 3), added up as if every primitive ran over the whole
    /// picture.
    pub fe_units: f64,
    /// The largest `feMorphology` radius, in the picture's units (its work grows with the square
    /// of the radius in device px).
    pub morph_radius: f64,
}

/// Scans `svg`; see the module documentation.
pub fn scan_vector(svg: &str) -> Vector {
    let b = svg.as_bytes();
    let mut v = Vector {
        width: 100.0,
        height: 100.0,
        ..Default::default()
    };
    let mut sized = false;
    let mut unpainted = 0usize;
    let mut i = 0;
    while let Some(lt) = b
        .get(i..)
        .and_then(|rest| rest.iter().position(|&c| c == b'<'))
        .map(|p| p + i)
    {
        let Some(&first) = b.get(lt + 1) else {
            break;
        };
        if first == b'/' {
            let close = &svg[lt + 2..tag_end(b, lt + 1).min(svg.len())];
            if is_unpainted_container(close.trim()) {
                unpainted = unpainted.saturating_sub(1);
            }
            i = lt + 1;
            continue;
        }
        if matches!(first, b'!' | b'?') {
            i = lt + 1;
            continue;
        }
        let end = tag_end(b, lt + 1);
        let tag = &svg[lt + 1..end.min(svg.len())];
        let name_len = tag
            .bytes()
            .position(|c| !(c.is_ascii_alphanumeric() || c == b':'))
            .unwrap_or(tag.len());
        let (name, attrs) = tag.split_at(name_len);
        v.els += 1.0;
        // What a clip path, mask or definition holds is not painted where it stands.
        let before = v.area;
        let container = is_unpainted_container(name) && !tag.ends_with('/');
        match name {
            "svg" if !sized => {
                sized = true;
                if let Some((w, h)) = view_size(attrs) {
                    v.width = w;
                    v.height = h;
                }
            }
            "path" => scan_path(attrs, &mut v),
            n if n.starts_with("fe") => scan_primitive(n, attrs, &mut v),
            "rect" => {
                let a = attr(attrs, "width")
                    .and_then(number)
                    .zip(attr(attrs, "height").and_then(number))
                    .map(|(w, h)| w.abs() * h.abs());
                v.area += a.unwrap_or(v.width * v.height).min(v.width * v.height);
            }
            "circle" => {
                let a = attr(attrs, "r")
                    .and_then(number)
                    .map(|r| std::f64::consts::PI * r * r);
                v.area += a.unwrap_or(v.width * v.height).min(v.width * v.height);
            }
            "ellipse" => {
                let a = attr(attrs, "rx")
                    .and_then(number)
                    .zip(attr(attrs, "ry").and_then(number))
                    .map(|(rx, ry)| std::f64::consts::PI * rx * ry);
                v.area += a.unwrap_or(v.width * v.height).min(v.width * v.height);
            }
            "text" => {
                // The ink of the characters: an em square a character and a bit less. (The
                // element is not the picture: a table of 130 cells is 130 small texts.)
                let fs = attr(attrs, "font-size")
                    .and_then(number)
                    .unwrap_or(16.0)
                    .abs();
                let chars = text_chars(&svg[(end + 1).min(svg.len())..]);
                v.area += (chars.max(1) as f64 * fs * fs * 0.7).min(v.width * v.height);
            }
            "image" | "use" | "foreignObject" => {
                let a = attr(attrs, "width")
                    .and_then(number)
                    .zip(attr(attrs, "height").and_then(number))
                    .map(|(w, h)| w.abs() * h.abs());
                v.area += a.unwrap_or(v.width * v.height).min(v.width * v.height);
            }
            // The text of a `<text>` is counted with it.
            "tspan" | "textPath" => {}
            // Groups, definitions, gradients and the like paint nothing themselves.
            "svg" | "g" | "defs" | "symbol" | "clipPath" | "mask" | "pattern" | "marker"
            | "linearGradient" | "radialGradient" | "stop" | "title" | "desc" | "metadata"
            | "style" | "filter" => {}
            _ => v.area += v.width * v.height,
        }
        if unpainted > 0 {
            v.area = before;
        }
        if container {
            unpainted += 1;
        }
        i = end + 1;
    }
    v
}

/// Elements whose content is not drawn where it is written: it is a clip, a mask, a pattern, a
/// marker or a definition other elements refer to.
fn is_unpainted_container(name: &str) -> bool {
    matches!(
        name,
        "clipPath" | "mask" | "defs" | "pattern" | "marker" | "symbol"
    )
}

/// The characters of a `<text>` element whose content starts at `rest`: those outside tags up to
/// its closing tag.
fn text_chars(rest: &str) -> usize {
    let end = rest.find("</text>").unwrap_or(rest.len());
    let mut n = 0;
    let mut in_tag = false;
    for c in rest[..end].chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag && !c.is_whitespace() => n += 1,
            _ => {}
        }
    }
    n
}

/// One filter primitive: its work per pixel as `svg_guard` counts it.
fn scan_primitive(name: &str, attrs: &str, v: &mut Vector) {
    let units = match name {
        "feTurbulence" => {
            let octaves = attr(attrs, "numOctaves").and_then(number).unwrap_or(1.0);
            8.0 + 2.5 * octaves.clamp(0.0, 64.0)
        }
        "feMorphology" => {
            let r = attr(attrs, "radius")
                .map(|r| {
                    r.split(|c: char| c == ',' || c.is_whitespace())
                        .filter_map(number)
                        .fold(0.0f64, |m, x| m.max(x.abs()))
                })
                .unwrap_or(0.0);
            v.morph_radius = v.morph_radius.max(r);
            3.0
        }
        "feConvolveMatrix" => {
            let cells = attr(attrs, "order")
                .map(|o| {
                    o.split(|c: char| c == ',' || c.is_whitespace())
                        .filter_map(number)
                        .product::<f64>()
                })
                .unwrap_or(9.0);
            2.0 + cells.clamp(1.0, 4096.0)
        }
        "feDiffuseLighting" | "feSpecularLighting" => 40.0,
        "feGaussianBlur" | "feDropShadow" => 12.0,
        "feDisplacementMap" => 6.0,
        _ => 3.0,
    };
    v.fe_units += units;
}

/// The index of the `>` that ends the start tag beginning at `from`, outside quoted values.
fn tag_end(b: &[u8], from: usize) -> usize {
    let mut quote = 0u8;
    let mut j = from;
    while j < b.len() {
        let c = b[j];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return j;
        }
        j += 1;
    }
    b.len()
}

/// The value of attribute `name` in the text of a start tag.
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(p) = rest.find(name) {
        let before_ok = p == 0 || rest.as_bytes()[p - 1].is_ascii_whitespace();
        let after = &rest[p + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && after_trim.starts_with('=') {
            let v = after_trim[1..].trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let inner = &v[1..];
                return inner.find(q).map(|e| &inner[..e]);
            }
        }
        rest = after;
    }
    None
}

/// A length such as `37.795`, `10px` or `5mm` as a number (the unit is ignored: the scan only
/// needs a size to compare with).
fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')))
        .unwrap_or(s.len());
    s[..end].parse::<f64>().ok().filter(|v| v.is_finite())
}

fn view_size(attrs: &str) -> Option<(f64, f64)> {
    if let Some(vb) = attr(attrs, "viewBox") {
        let n: Vec<f64> = vb
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .filter_map(number)
            .collect();
        if n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 {
            return Some((n[2], n[3]));
        }
    }
    let w = attr(attrs, "width").and_then(number)?;
    let h = attr(attrs, "height").and_then(number)?;
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// One `<path>`: its segments, outline length and painted area.
fn scan_path(attrs: &str, v: &mut Vector) {
    let cap = 2.0 * v.width.max(v.height);
    let m = attr(attrs, "d")
        .map(|d| path_metrics(d, cap))
        .unwrap_or_default();
    v.segs += m.segs;
    v.len += m.len;
    let stroked = attr(attrs, "stroke").is_some_and(|s| s.trim() != "none");
    // Without a `fill` attribute a path is filled; a style attribute may say anything: both count.
    let styled = attr(attrs, "style").is_some();
    let filled = styled || attr(attrs, "fill").is_none_or(|f| f.trim() != "none");
    let sw = if stroked || styled {
        attr(attrs, "stroke-width")
            .and_then(number)
            .unwrap_or(1.0)
            .abs()
    } else {
        0.0
    };
    if m.segs == 0.0 {
        return;
    }
    let grow = if stroked || styled { sw / 2.0 } else { 0.0 };
    let (x0, x1) = ((m.x0 - grow).max(0.0), (m.x1 + grow).min(v.width));
    let (y0, y1) = ((m.y0 - grow).max(0.0), (m.y1 + grow).min(v.height));
    if (filled || stroked || styled) && x1 > x0 && y1 > y0 {
        v.area += (x1 - x0) * (y1 - y0);
    }
}

#[derive(Default, Clone, Copy)]
struct PathMetrics {
    segs: f64,
    len: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

/// Segments, length and bounding box of path data `d`. Relative commands are treated as the
/// absolute ones from the current point; arcs as one line to their end point; numbers that do not
/// parse end the scan. Each segment's length counts up to `cap`.
fn path_metrics(d: &str, cap: f64) -> PathMetrics {
    let b = d.as_bytes();
    let mut m = PathMetrics {
        x0: f64::MAX,
        y0: f64::MAX,
        x1: f64::MIN,
        y1: f64::MIN,
        ..Default::default()
    };
    let (mut cx, mut cy, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut cmd = b'M';
    let mut args: Vec<f64> = Vec::with_capacity(8);
    let mut i = 0;
    let see = |m: &mut PathMetrics, x: f64, y: f64| {
        m.x0 = m.x0.min(x);
        m.y0 = m.y0.min(y);
        m.x1 = m.x1.max(x);
        m.y1 = m.y1.max(y);
    };
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            cmd = c;
            args.clear();
            i += 1;
            if matches!(cmd, b'Z' | b'z') {
                m.segs += 1.0;
                m.len += (cx - sx).hypot(cy - sy).min(cap);
                cx = sx;
                cy = sy;
            }
            continue;
        }
        if c == b',' || c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // A number: sign, digits, one dot, an exponent.
        let start = i;
        if matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let mut dot = false;
        while i < b.len() {
            match b[i] {
                b'0'..=b'9' => i += 1,
                b'.' if !dot => {
                    dot = true;
                    i += 1;
                }
                b'e' | b'E' => {
                    i += 1;
                    if i < b.len() && matches!(b[i], b'+' | b'-') {
                        i += 1;
                    }
                }
                _ => break,
            }
        }
        let Some(n) = d[start..i].parse::<f64>().ok().filter(|n| n.is_finite()) else {
            break;
        };
        args.push(n);
        let need = match cmd.to_ascii_uppercase() {
            b'M' | b'L' | b'T' => 2,
            b'H' | b'V' => 1,
            b'C' => 6,
            b'S' | b'Q' => 4,
            b'A' => 7,
            _ => break,
        };
        if args.len() < need {
            continue;
        }
        let rel = cmd.is_ascii_lowercase();
        let (nx, ny) = match cmd.to_ascii_uppercase() {
            b'H' => (if rel { cx + args[0] } else { args[0] }, cy),
            b'V' => (cx, if rel { cy + args[0] } else { args[0] }),
            _ => {
                let (ex, ey) = (args[need - 2], args[need - 1]);
                if rel {
                    (cx + ex, cy + ey)
                } else {
                    (ex, ey)
                }
            }
        };
        // Control points count towards the box and the length of the polygon they make. A moveto
        // draws nothing: its point is in the box and the pen jumps.
        let moves = cmd.eq_ignore_ascii_case(&b'M');
        let mut prev = (cx, cy);
        let n_pts = match cmd.to_ascii_uppercase() {
            b'C' => 3,
            b'S' | b'Q' => 2,
            _ => 1,
        };
        for k in 0..n_pts {
            let (px, py) = if k == n_pts - 1 {
                (nx, ny)
            } else {
                let (ax, ay) = (args[2 * k], args[2 * k + 1]);
                if rel {
                    (cx + ax, cy + ay)
                } else {
                    (ax, ay)
                }
            };
            if !moves {
                m.len += (px - prev.0).hypot(py - prev.1).min(cap);
            }
            see(&mut m, px, py);
            prev = (px, py);
        }
        m.segs += 1.0;
        if moves {
            sx = nx;
            sy = ny;
            // Further pairs after a moveto are linetos.
            cmd = if rel { b'l' } else { b'L' };
        }
        cx = nx;
        cy = ny;
        args.clear();
    }
    if m.segs == 0.0 || m.x0 > m.x1 {
        return PathMetrics::default();
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_comes_from_the_view_box_then_width_and_height_then_a_default() {
        let a =
            scan_vector(r#"<svg xmlns="x" viewBox="0 0 10 20" width="500" height="900"></svg>"#);
        assert_eq!((a.width, a.height), (10.0, 20.0));
        let b = scan_vector(r#"<svg width="37.795px" height="5"><g/></svg>"#);
        assert_eq!((b.width, b.height), (37.795, 5.0));
        let c = scan_vector("<svg></svg>");
        assert_eq!((c.width, c.height), (100.0, 100.0));
        // A broken view box falls through to the next source.
        let d = scan_vector(r#"<svg viewBox="0 0 0 5" width="8" height="9"/>"#);
        assert_eq!((d.width, d.height), (8.0, 9.0));
    }

    #[test]
    fn a_path_costs_its_segments_length_and_box() {
        let v = scan_vector(
            r##"<svg viewBox="0 0 100 100"><path d="M10 10 L50 10 L50 40 Z" fill="#000"/></svg>"##,
        );
        assert_eq!(v.els, 2.0);
        assert_eq!(v.segs, 4.0);
        // 40 + 30 + 50 (the closing line)
        assert!((v.len - 120.0).abs() < 1e-9, "{}", v.len);
        assert!((v.area - 40.0 * 30.0).abs() < 1e-9, "{}", v.area);
    }

    #[test]
    fn a_wide_stroke_paints_the_whole_picture_however_short_the_path() {
        let thin = scan_vector(
            r##"<svg viewBox="0 0 10 10"><path d="M0 1L9 5" fill="none" stroke="#f00" stroke-width="1"/></svg>"##,
        );
        let fat = scan_vector(
            r##"<svg viewBox="0 0 10 10"><path d="M0 1L9 5" fill="none" stroke="#f00" stroke-width="100000"/></svg>"##,
        );
        assert!(thin.area < 50.0, "{}", thin.area);
        assert_eq!(fat.area, 100.0, "clipped to the picture");
        // Without a stroke the width does not count.
        let none = scan_vector(
            r#"<svg viewBox="0 0 10 10"><path d="M0 1L9 5" fill="none" stroke-width="100000"/></svg>"#,
        );
        assert_eq!(none.area, 0.0);
    }

    #[test]
    fn relative_commands_and_implicit_repeats_are_followed() {
        let v = scan_vector(
            r#"<svg viewBox="0 0 100 100"><path d="m10 10 l10 0 0 10 h-5 v5 c1 1 2 2 3 3z"/></svg>"#,
        );
        // m, l, implicit l, h, v, c, z
        assert_eq!(v.segs, 7.0);
        assert!(v.len > 30.0 && v.len < 100.0, "{}", v.len);
    }

    #[test]
    fn a_segment_far_longer_than_the_picture_counts_up_to_twice_its_side() {
        let v = scan_vector(r#"<svg viewBox="0 0 10 10"><path d="M0 0 L1000000 0"/></svg>"#);
        assert_eq!(v.len, 20.0);
    }

    #[test]
    fn other_graphic_elements_paint_the_whole_picture_and_containers_nothing() {
        let v = scan_vector(
            r#"<svg viewBox="0 0 10 10"><defs><linearGradient id="g"><stop/></linearGradient></defs><g><text/><image/></g></svg>"#,
        );
        assert_eq!(v.area, 200.0);
        assert_eq!(v.els, 7.0);
    }

    #[test]
    fn rectangles_circles_and_ellipses_paint_their_own_size_up_to_the_picture() {
        let v = scan_vector(
            r#"<svg viewBox="0 0 10 10"><rect width="2" height="3"/><circle r="1"/><ellipse rx="2" ry="1"/></svg>"#,
        );
        let want = 6.0 + std::f64::consts::PI + 2.0 * std::f64::consts::PI;
        assert!((v.area - want).abs() < 1e-9, "{}", v.area);
        // a rectangle bigger than the picture, and one whose size is not given
        let v = scan_vector(
            r#"<svg viewBox="0 0 10 10"><rect width="500" height="500"/><rect/></svg>"#,
        );
        assert_eq!(v.area, 200.0);
        let v = scan_vector(r#"<svg viewBox="0 0 10 10"><circle r="x"/></svg>"#);
        assert_eq!(v.area, 100.0);
    }

    #[test]
    fn filter_primitives_add_up_their_work_per_pixel_and_paint_nothing_themselves() {
        let v = scan_vector(
            r#"<svg viewBox="0 0 10 10"><filter id="f"><feGaussianBlur stdDeviation="3"/><feOffset dx="1"/><feTurbulence numOctaves="4"/><feMorphology radius="5 2"/><feDiffuseLighting/></filter></svg>"#,
        );
        assert_eq!(v.fe_units, 12.0 + 3.0 + (8.0 + 10.0) + 3.0 + 40.0);
        assert_eq!(v.morph_radius, 5.0);
        assert_eq!(v.area, 0.0);
        // convolution by its matrix size, and unreadable numbers fall back to something
        let v = scan_vector(r#"<svg><feConvolveMatrix order="5"/><feConvolveMatrix/></svg>"#);
        assert_eq!(v.fe_units, 2.0 + 5.0 + 2.0 + 9.0);
    }

    #[test]
    fn text_paints_its_characters_not_the_picture_and_images_their_own_size() {
        // 130 cells of a table: a picture of 1000 x 1000 units
        let mut s = String::from(r#"<svg viewBox="0 0 1000 1000">"#);
        for _ in 0..130 {
            s.push_str(r#"<g><text font-size="40" x="0" y="0">ab <tspan>cd</tspan></text></g>"#);
        }
        s.push_str(r#"<image width="100" height="50"/><use/></svg>"#);
        let v = scan_vector(&s);
        let want = 130.0 * 4.0 * 40.0 * 40.0 * 0.7 + 5000.0 + 1e6;
        assert!((v.area - want).abs() < 1.0, "{} against {want}", v.area);
        // text bigger than the picture is the picture; an empty text is one character
        let v = scan_vector(r#"<svg viewBox="0 0 10 10"><text font-size="500">abc</text></svg>"#);
        assert_eq!(v.area, 100.0);
        let v = scan_vector(r#"<svg viewBox="0 0 100 100"><text font-size="10"></text></svg>"#);
        assert_eq!(v.area, 70.0);
        // an unclosed text counts what follows it, without running past the end
        let v = scan_vector(r#"<svg viewBox="0 0 100 100"><text font-size="1">ab"#);
        assert_eq!(v.area, 1.4);
    }

    #[test]
    fn what_a_clip_path_or_definition_holds_is_not_painted_where_it_stands() {
        let v = scan_vector(
            r#"<svg viewBox="0 0 100 100"><defs><clipPath id="c"><path d="M0 0H100V100H0Z"/></clipPath></defs><clipPath id="d"><rect/></clipPath><g clip-path="url(#c)"><path d="M0 0H10V10H0Z"/></g><mask><text>x</text></mask><use width="5" height="5"/></svg>"#,
        );
        // the clip's rectangle and the mask's text are not painted; the path and the use are
        assert_eq!(v.area, 100.0 + 25.0);
        // the clip still costs its elements and segments
        assert_eq!(v.segs, 10.0);
        // self-closing containers hold nothing and do not hide what follows
        let v =
            scan_vector(r#"<svg viewBox="0 0 10 10"><defs/><text font-size="1">a</text></svg>"#);
        assert!((v.area - 0.7).abs() < 1e-9);
    }

    #[test]
    fn markup_that_is_not_svg_does_not_panic() {
        for s in [
            "",
            "<",
            "<<<<",
            "<path d=",
            "<path d=\"M",
            "<path d=\"M1e9999 4 L\"/>",
            "<path d=\"M--1 2\"/>",
            "<svg viewBox=\"a b c d\"",
            "<path d=\"M 1 2 3\" stroke=\"x\" stroke-width=\"nan\"/>",
            "\u{0}<!-- <path d=\"M0 0\"> -->",
        ] {
            let v = scan_vector(s);
            assert!(v.len.is_finite() && v.area.is_finite(), "{s:?}");
        }
    }

    #[test]
    fn thirty_thousand_fat_strokes_cost_thirty_thousand_pictures() {
        let mut s = String::from(r#"<svg viewBox="0 0 10 10">"#);
        for i in 0..30_000 {
            s.push_str(&format!(
                r##"<path d="M{} 1L99 50" fill="none" stroke="#f00" stroke-width="100000"/>"##,
                i % 40
            ));
        }
        s.push_str("</svg>");
        let v = scan_vector(&s);
        assert_eq!(v.area, 30_000.0 * 100.0);
    }
}
