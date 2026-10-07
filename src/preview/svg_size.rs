// The size an SVG declares for itself, read from the root element alone.
//
// Laying out a Markdown document needs the pixel size of every picture in it, and it needs it on
// the UI thread. Asking usvg means parsing the whole file, which for a crafted one takes seconds
// (a 200 KB `<text>` stalled the UI for 16 s on a Mac with 1,000 fonts installed; the time grows with the number of fonts). The size is a function of the root `<svg>`
// element's `width`, `height` and `viewBox` only, so this reads exactly that and nothing else: one
// flat pass over the opening tag, no tree, no allocation proportional to the file.
//
// The rules are usvg's (`resolve_svg_size`), including the units, the defaults for a missing
// attribute and the viewBox aspect ratio when only one side is given, and the lengths are parsed by
// the same crate (`svgtypes`). The tests compare the two over a matrix of attribute combinations.
//
// What it will not guess at is answered `Unknown`, and the caller treats that as "size not
// available" rather than inventing one:
//   - a percentage size without a viewBox (usvg then measures the drawn content);
//   - an `em`/`ex` size when a stylesheet or a `style` attribute may set the `font-size` it is
//     measured in (usvg applies CSS to presentation properties, but not to `width`/`height`);
//   - `font-size` given by a keyword, or an entity reference in an attribute value.

use std::str::FromStr;

use svgtypes::{Length, LengthUnit, ViewBox};

use super::svg_guard::SvgFail;

/// usvg's defaults (`Options::default`).
const DPI: f32 = 96.0;
const FONT_SIZE: f32 = 12.0;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// What reading the root element yielded.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum Declared {
    /// The size usvg would report, in px (before rounding up).
    Size(f32, f32),
    /// A valid SVG whose size cannot be read from the root element alone.
    Unknown,
}

/// The attributes of the root element that decide its size.
#[derive(Default)]
struct Root<'a> {
    name: &'a str,
    width: Option<String>,
    height: Option<String>,
    view_box: Option<String>,
    font_size: Option<String>,
    style: Option<String>,
    ns: Vec<(&'a str, String)>,
    has_entity_ref: bool,
}

/// Skip to the first `>` outside quotes starting at `from`; returns the index of the `>`.
fn end_of_tag(b: &[u8], from: usize) -> Option<usize> {
    let mut quote = 0u8;
    let mut i = from;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The byte range of the root element's start tag (after the prolog), or `None` if there is none.
fn root_tag(b: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0usize;
    loop {
        let lt = b.get(i..)?.iter().position(|&c| c == b'<')? + i;
        let rest = &b[lt..];
        if rest.starts_with(b"<!--") {
            i = find(b, lt + 4, b"-->")? + 3;
        } else if rest.starts_with(b"<?") {
            i = find(b, lt + 2, b"?>")? + 2;
        } else if rest.starts_with(b"<!") {
            // DOCTYPE, possibly with an internal subset in brackets.
            let (mut j, mut bracket, mut quote) = (lt + 2, 0i32, 0u8);
            loop {
                let &c = b.get(j)?;
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else {
                    match c {
                        b'"' | b'\'' => quote = c,
                        b'[' => bracket += 1,
                        b']' => bracket -= 1,
                        b'>' if bracket <= 0 => break,
                        // A comment inside the internal subset may hold quotes and brackets.
                        b'<' if b[j..].starts_with(b"<!--") => {
                            j = find(b, j + 4, b"-->")? + 2;
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            i = j + 1;
        } else if rest.starts_with(b"</") {
            return None;
        } else {
            return Some((lt, end_of_tag(b, lt + 1)?));
        }
    }
}

/// Decode the five predefined entities and character references in an attribute value. `None`
/// for any other `&` (a DOCTYPE-defined entity: not resolved here).
fn unescape(v: &str) -> Option<String> {
    if !v.contains('&') {
        return Some(v.to_string());
    }
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let semi = after.find(';')?;
        let ent = &after[..semi];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let num = ent.strip_prefix('#')?;
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => num.parse().ok()?,
                };
                out.push(char::from_u32(code)?);
            }
        }
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Parse the start tag `tag` (from `<` to `>`).
fn parse_root(tag: &str) -> Option<Root<'_>> {
    let b = tag.as_bytes();
    let mut i = 1usize;
    let is_ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r');
    let name_start = i;
    while i < b.len() && !is_ws(b[i]) && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let mut root = Root {
        name: &tag[name_start..i],
        ..Root::default()
    };
    loop {
        while i < b.len() && is_ws(b[i]) {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' || b[i] == b'/' {
            return Some(root);
        }
        let n0 = i;
        while i < b.len() && b[i] != b'=' && !is_ws(b[i]) && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let name = &tag[n0..i];
        while i < b.len() && is_ws(b[i]) {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while i < b.len() && is_ws(b[i]) {
            i += 1;
        }
        let q = *b.get(i)?;
        if q != b'"' && q != b'\'' {
            return None;
        }
        let v0 = i + 1;
        let v1 = v0 + tag[v0..].find(q as char)?;
        let raw = &tag[v0..v1];
        i = v1 + 1;
        if raw.contains('&') && unescape(raw).is_none() {
            root.has_entity_ref = true;
        }
        let value = unescape(raw).unwrap_or_default();
        match name {
            "width" => root.width = Some(value),
            "height" => root.height = Some(value),
            "viewBox" => root.view_box = Some(value),
            "font-size" => root.font_size = Some(value),
            "style" => root.style = Some(value),
            "xmlns" => root.ns.push(("", value)),
            _ => {
                if let Some(prefix) = name.strip_prefix("xmlns:") {
                    root.ns.push((prefix, value));
                }
            }
        }
    }
}

/// Whether a `<style>` element in `text` sets a font size (what `em`/`ex` lengths are measured in).
fn stylesheet_may_resize(text: &[u8]) -> bool {
    let mut from = 0usize;
    while let Some(open) = find(text, from, b"<style") {
        let Some(gt) = end_of_tag(text, open + 6) else {
            return true;
        };
        let end = find(text, gt, b"</style").unwrap_or(text.len());
        let css = &text[gt + 1..end.max(gt + 1)];
        for prop in [&b"font-size"[..], b"font"] {
            let mut at = 0usize;
            while let Some(p) = find(css, at, prop) {
                let before = p.checked_sub(1).map(|q| css[q]);
                let after = css[p + prop.len()..]
                    .iter()
                    .copied()
                    .find(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r'));
                let boundary = !matches!(before, Some(c) if c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
                if boundary && after == Some(b':') {
                    return true;
                }
                at = p + prop.len();
            }
        }
        from = end;
    }
    false
}

fn length(v: &Option<String>) -> Option<Length> {
    Length::from_str(v.as_deref()?).ok()
}

/// usvg's `convert_user_length` for a size attribute (never a percentage here).
fn to_px(l: Length, font: f32) -> f32 {
    let n = l.number as f32;
    match l.unit {
        LengthUnit::None | LengthUnit::Px => n,
        LengthUnit::Em => n * font,
        LengthUnit::Ex => n * font / 2.0,
        LengthUnit::In => n * DPI,
        LengthUnit::Cm => n * DPI / 2.54,
        LengthUnit::Mm => n * DPI / 25.4,
        LengthUnit::Pt => n * DPI / 72.0,
        LengthUnit::Pc => n * DPI / 6.0,
        LengthUnit::Percent => n,
    }
}

/// The root element's own `font-size` in px (the base of `em`/`ex` lengths), or `None` when it is
/// a keyword this does not resolve.
fn root_font_size(attr: &Option<String>) -> Option<f32> {
    let Some(v) = attr else {
        return Some(FONT_SIZE);
    };
    let l = Length::from_str(v).ok()?;
    let n = l.number as f32;
    Some(match l.unit {
        LengthUnit::None | LengthUnit::Px => n,
        LengthUnit::Em => n * FONT_SIZE,
        LengthUnit::Ex => n * FONT_SIZE / 2.0,
        LengthUnit::In => n * DPI,
        LengthUnit::Cm => n * DPI / 2.54,
        LengthUnit::Mm => n * DPI / 25.4,
        LengthUnit::Pt => n * DPI / 72.0,
        LengthUnit::Pc => n * DPI / 6.0,
        LengthUnit::Percent => n * FONT_SIZE * 0.01,
    })
}

fn is_percent(l: Option<Length>) -> bool {
    matches!(l, Some(l) if l.unit == LengthUnit::Percent)
}

/// The size an SVG document declares, from its (already size- and depth-checked) bytes.
pub(crate) fn declared_size(bytes: &[u8]) -> Result<Declared, SvgFail> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let (lt, gt) = root_tag(bytes).ok_or(SvgFail::Invalid)?;
    let tag = std::str::from_utf8(&bytes[lt..=gt]).map_err(|_| SvgFail::Invalid)?;
    let root = parse_root(tag).ok_or(SvgFail::Invalid)?;

    // The root must be an `svg` element in the SVG namespace, or usvg refuses the file.
    let (prefix, local) = match root.name.split_once(':') {
        Some((p, l)) => (p, l),
        None => ("", root.name),
    };
    if local != "svg" {
        return Err(SvgFail::Invalid);
    }
    let declared_ns = root
        .ns
        .iter()
        .rev()
        .find(|(p, _)| *p == prefix)
        .map(|(_, v)| v.as_str());
    // usvg also accepts a root with no namespace at all; a prefix nobody declared is an XML error.
    match declared_ns {
        Some(SVG_NS) => {}
        None if prefix.is_empty() => {}
        _ => return Err(SvgFail::Invalid),
    }

    if root.has_entity_ref {
        return Ok(Declared::Unknown);
    }

    let (wl, hl) = (length(&root.width), length(&root.height));
    let vb = root
        .view_box
        .as_deref()
        .and_then(|v| ViewBox::from_str(v).ok())
        .and_then(|v| {
            let (x, y, w, h) = (v.x as f32, v.y as f32, v.w as f32, v.h as f32);
            let ok = x.is_finite()
                && y.is_finite()
                && w.is_finite()
                && h.is_finite()
                && w > 0.0
                && h > 0.0
                && (x + w).is_finite()
                && (y + h).is_finite();
            ok.then_some((w, h))
        });

    // A percentage with nothing to be a percentage of: usvg sizes the picture to its content.
    let pct_w = wl.is_none() || is_percent(wl);
    let pct_h = hl.is_none() || is_percent(hl);
    if (pct_w || pct_h) && vb.is_none() {
        return Ok(Declared::Unknown);
    }

    let uses_font_units = |l: Option<Length>| matches!(l, Some(l) if matches!(l.unit, LengthUnit::Em | LengthUnit::Ex));
    if (uses_font_units(wl) || uses_font_units(hl))
        && (root.style.as_deref().is_some_and(|s| s.contains("font"))
            || stylesheet_may_resize(bytes))
    {
        return Ok(Declared::Unknown);
    }
    let Some(font) = root_font_size(&root.font_size) else {
        return Ok(Declared::Unknown);
    };
    let (w, h) = match vb {
        Some((vw, vh)) => {
            let w = match wl {
                Some(l) if l.unit != LengthUnit::Percent => to_px(l, font),
                Some(l) => vw * (l.number as f32 / 100.0),
                None => vw,
            };
            let h = match hl {
                Some(l) if l.unit != LengthUnit::Percent => to_px(l, font),
                Some(l) => vh * (l.number as f32 / 100.0),
                None => vh,
            };
            match (wl.is_some(), hl.is_some()) {
                (true, false) => (w, vh * w / vw),
                (false, true) => (vw * h / vh, h),
                _ => (w, h),
            }
        }
        None => (to_px(wl.unwrap(), font), to_px(hl.unwrap(), font)),
    };
    if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 {
        Ok(Declared::Size(w, h))
    } else {
        Err(SvgFail::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use resvg::usvg;

    /// The size usvg itself reports for `doc`, rounded up the way konoma rounds it.
    fn usvg_size(doc: &str) -> Option<(u32, u32)> {
        let tree = usvg::Tree::from_str(doc, &usvg::Options::default()).ok()?;
        let s = tree.size();
        Some((s.width().ceil() as u32, s.height().ceil() as u32))
    }

    fn ours(doc: &str) -> Result<Option<(u32, u32)>, SvgFail> {
        Ok(match declared_size(doc.as_bytes())? {
            Declared::Size(w, h) => Some((w.ceil() as u32, h.ceil() as u32)),
            Declared::Unknown => None,
        })
    }

    fn doc(attrs: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" {attrs}><rect x="3" y="4" width="50" height="30"/></svg>"#
        )
    }

    /// Every case where this reads a size, it is the size usvg reports. Where it declines
    /// (`Unknown`) the case is one of the documented classes: a percentage without a viewBox.
    #[test]
    fn declared_size_equals_usvgs_over_a_matrix_of_attributes() {
        let lengths = [
            "", "100", "10px", "20mm", "2in", "1.5em", "3ex", "10pt", "1cm", "5pc", "50%", "100%",
            "7.25", "1e2", "0", "-5", "abc", "12 px", "  40  ",
        ];
        let boxes = [
            "",
            "0 0 200 100",
            "10 20 300 150",
            "0,0,64,64",
            "0 0 0 100",
            "0 0 -5 5",
            "0 0 1e3 5e2",
            "not a box",
        ];
        let fonts = ["", r#"font-size="20px""#, r#"font-size="2em""#];
        let (mut compared, mut declined) = (0, 0);
        for w in lengths {
            for h in lengths {
                for b in boxes {
                    for f in fonts {
                        let mut attrs = String::new();
                        if !w.is_empty() {
                            attrs += &format!(r#"width="{w}" "#);
                        }
                        if !h.is_empty() {
                            attrs += &format!(r#"height="{h}" "#);
                        }
                        if !b.is_empty() {
                            attrs += &format!(r#"viewBox="{b}" "#);
                        }
                        attrs += f;
                        let d = doc(&attrs);
                        let theirs = usvg_size(&d);
                        match ours(&d) {
                            Ok(Some(size)) => {
                                compared += 1;
                                assert_eq!(Some(size), theirs, "{attrs}");
                            }
                            Ok(None) => {
                                declined += 1;
                                // Only a percentage (or nothing, which is 100%) with no usable
                                // viewBox is left to drawing.
                                let usable_box = matches!(
                                    b,
                                    "0 0 200 100" | "10 20 300 150" | "0,0,64,64" | "0 0 1e3 5e2"
                                );
                                assert!(!usable_box, "declined with a usable viewBox: {attrs}");
                            }
                            Err(SvgFail::Invalid) => {
                                // usvg refuses it too (a zero, negative or unparsable size).
                                assert_eq!(theirs, None, "{attrs}: usvg draws it as {theirs:?}");
                            }
                            Err(other) => panic!("{attrs}: {other:?}"),
                        }
                    }
                }
            }
        }
        assert!(compared > 1000 && declined > 100, "{compared} / {declined}");
    }

    #[test]
    fn the_prolog_the_quoting_and_the_namespace_prefix_do_not_change_the_answer() {
        let cases = [
            // XML declaration, comment, DOCTYPE (with an internal subset), BOM.
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<!-- a <svg> comment -->\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\"/>",
            "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\"/>",
            "<!DOCTYPE svg [ <!ENTITY ns \"http://www.w3.org/2000/svg\"> <!-- \" --> ]>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\"/>",
            "\u{feff}<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\"/>",
            // Single quotes, spaces around `=`, newlines between attributes, `>` inside a value.
            "<svg xmlns='http://www.w3.org/2000/svg' width = '40'\n   height\t=\n\"30\" data-x=\"a>b\"/>",
            // Prefixed root.
            "<s:svg xmlns:s=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"30\"/>",
            // Character references in a value.
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"&#52;0\" height=\"&#x33;0\"/>",
            // Attribute order and unrelated attributes.
            "<svg height=\"30\" id=\"a\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"40\" xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\"/>",
        ];
        for c in cases {
            assert_eq!(
                usvg_size(c),
                Some((40, 30)),
                "usvg disagrees with the test: {c}"
            );
            assert_eq!(ours(c), Ok(Some((40, 30))), "{c}");
        }
    }

    #[test]
    fn what_is_not_an_svg_is_invalid() {
        for c in [
            "<html width=\"10\" height=\"10\"/>",
            "<svg xmlns=\"http://example.com/\" width=\"10\" height=\"10\"/>",
            "<s:svg width=\"10\" height=\"10\"/>",
            "plain text",
            "",
            "<?xml version=\"1.0\"?>",
            "</svg>",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=10 height=10/>",
        ] {
            assert_eq!(ours(c), Err(SvgFail::Invalid), "{c:?}");
            assert_eq!(usvg_size(c), None, "usvg draws {c:?}");
        }
    }

    #[test]
    fn a_root_without_a_namespace_is_sized_like_usvg_sizes_it() {
        let d = r#"<svg width="10" height="10"/>"#;
        assert_eq!(usvg_size(d), Some((10, 10)));
        assert_eq!(ours(d), Ok(Some((10, 10))));
    }

    /// usvg applies CSS to presentation properties only: `width`/`height` in a stylesheet or a
    /// `style` attribute change nothing, `font-size` does (and `em` lengths follow it).
    #[test]
    fn css_changes_the_size_only_through_font_size() {
        for d in [
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>svg { width: 77px; height: 55px }</style></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" style="width:77px;height:55px"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>.a { fill: red; stroke-width: 3px }</style><rect class="a" width="5" height="5"/></svg>"#,
        ] {
            assert_eq!(usvg_size(d), Some((10, 10)), "usvg: {d}");
            assert_eq!(ours(d), Ok(Some((10, 10))), "{d}");
        }
        // A font size set by CSS moves an `em` size: usvg says 40 x 40, so this must not say 12 x 12.
        let d = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2em" height="2em"><style>svg { font-size: 20px }</style></svg>"#;
        let theirs = usvg_size(d);
        assert_eq!(theirs, Some((40, 40)));
        assert_eq!(ours(d), Ok(None), "declined rather than answering 24 x 24");
        let d = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2em" height="2em" style="font-size:20px"/>"#;
        assert_eq!(usvg_size(d), Some((40, 40)));
        assert_eq!(ours(d), Ok(None));
        // `em` with no CSS at all is answered.
        let d = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2em" height="2em"/>"#;
        assert_eq!(ours(d), Ok(usvg_size(d)));
        assert_eq!(ours(d), Ok(Some((24, 24))));
    }

    #[test]
    fn real_files_and_konomas_own_svgs_agree_with_usvg() {
        let mut docs: Vec<(String, String)> = Vec::new();
        for p in ["samples/sample.svg", "site/public/favicon.svg"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(p);
            if let Ok(s) = std::fs::read_to_string(&path) {
                docs.push((p.to_string(), s));
            }
        }
        for (i, code) in [
            "flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C[Done]\n  B -->|no| D[Retry]",
            "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi",
            "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85",
            "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]",
        ]
        .iter()
        .enumerate()
        {
            if let Some(svg) =
                crate::preview::markdown::mermaid_to_svg_flow(code, "default", "basis", "splines")
            {
                docs.push((format!("mermaid {i}"), svg));
            }
        }
        for latex in ["E = mc^2", "\\frac{a}{b} + \\sum_{i=0}^n i^2", "x"] {
            for display in [false, true] {
                if let Some(svg) = crate::preview::math::latex_to_svg(latex, display, "#d0d0d0") {
                    docs.push((format!("math {latex} {display}"), svg));
                }
            }
        }
        assert!(docs.len() >= 8, "only {} documents", docs.len());
        for (name, d) in docs {
            assert_eq!(ours(&d), Ok(usvg_size(&d)), "{name}");
            assert!(ours(&d).unwrap().is_some(), "{name}: declined");
        }
    }

    #[test]
    fn a_root_tag_that_never_ends_or_is_huge_does_not_hang() {
        let mut d = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" ");
        d.push_str(&"a=\"1\" ".repeat(200_000));
        assert!(ours(&d).is_err());
        d.push_str("width=\"5\" height=\"5\"/>");
        let t = std::time::Instant::now();
        assert_eq!(ours(&d), Ok(Some((5, 5))));
        assert!(t.elapsed() < std::time::Duration::from_millis(500));
    }

    /// The bug this exists for: a file that took usvg six seconds to parse is sized instantly.
    #[test]
    fn a_text_heavy_svg_is_sized_without_parsing_it() {
        let d = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200"><text x="10" y="50" font-size="12">{}</text></svg>"#,
            "W".repeat(200_000)
        );
        let t = std::time::Instant::now();
        assert_eq!(ours(&d), Ok(Some((300, 200))));
        assert!(
            t.elapsed() < std::time::Duration::from_millis(50),
            "{:?}",
            t.elapsed()
        );
    }
}
