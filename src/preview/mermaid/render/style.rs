//! Resolving `classDef` / `class` / `:::` / `style` / `linkStyle` into paint the SVG can draw.
//!
//! # Why this exists
//!
//! Every one of konoma's mermaid parsers reads these statements so they cannot become phantom
//! nodes (`docs/FEATURE-MERMAID-RENDERER.md` §2-3), but for a long time nothing downstream ever
//! looked at what they said. That is a real, reported regression against the crate konoma
//! replaced (`docs/STATUS.md`'s flowchart entry, 2026-08-28): a `linkStyle` that colour-codes a
//! flowchart's edges parsed cleanly and drew every line in the theme's ordinary colour. This
//! module is the missing "who reads it" — a small, language-neutral cascade every diagram kind
//! with `classDef`/`style`/`linkStyle` data can hand its own declarations to.
//!
//! # Why this is not `PlacedNode::series` / `PlacedEdge::series`
//!
//! `series` names an index into [`super::Theme::series`] — *which* entry of the categorical
//! palette a data point belongs to. What `classDef` writes is a literal value the source chose
//! (`fill:#f9f`), which is a different kind of fact and has to have its own field: the commit that
//! folded a second meaning onto `series` (depth, in `a88fda1`) produced a real bug, and it is
//! exactly the shape of mistake `CLAUDE.md`'s house style calls out — "意味の違う情報には別の欄を
//! 与える".
//!
//! # The cascade
//!
//! mermaid applies these in a fixed pipeline order, later stages overwriting earlier ones:
//!
//! 1. `classDef default` — every node/entity/tile in the diagram starts here.
//! 2. Each class the element carries, in the order it was applied (`:::` at the declaration, then
//!    any later `class` statement, in source order).
//! 3. The element's own `style` statement, if it has one.
//! 4. *(edges only)* `linkStyle default`, then every `linkStyle` naming this edge's index — see
//!    [`cascade_edge`] for why steps 4 stays in this fixed relative order regardless of which the
//!    author physically wrote first.
//!
//! # Validating colours
//!
//! konoma writes every colour into hand-built SVG (`svg.rs`'s module docs: "no `var(--x)`", "one
//! font family"), so a declaration usvg cannot parse is worse than a declaration konoma drops: an
//! unparsable `fill` does not fail to draw, it silently becomes black, which is invisible on
//! konoma's dark theme — the same "blank equation" failure `UiConfig::math_color` exists to avoid.
//! [`paint`] uses the identical check: parse with the same crate (`svgtypes`) usvg itself uses, and
//! fall back to leaving the theme's own colour in place rather than to a colour nobody chose.

use std::str::FromStr;

/// One shape's or line's resolved paint override — what the cascade above produced. `None` in
/// every field means "the theme decides", which is what every diagram drawn before this module
/// existed still gets: a node or edge with no [`ShapeStyle`] at all draws byte-for-byte what it
/// drew before `classDef`/`style`/`linkStyle` had anywhere to land.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShapeStyle {
    /// `fill:` — interior paint. `Some("none".into())` is a real, distinct value from `None` (no
    /// override): the source asked for *no* interior, not "use the theme's".
    pub fill: Option<String>,
    /// `stroke:`.
    pub stroke: Option<String>,
    /// `stroke-width:`, already in the px this crate's SVG is written in.
    pub stroke_width: Option<f64>,
    /// `color:` — the label's text colour. mermaid's own generated CSS never lets this reach a
    /// node's text (`docs/FEATURE-MERMAID-RENDERER.md` §4-4 records the upstream defect); konoma
    /// writes its own `<text fill="…">` rather than relying on a stylesheet, so nothing stops it
    /// from honouring the declaration konoma's author actually wrote.
    pub text: Option<String>,
    /// `stroke-dasharray:`, passed through once checked to hold nothing but the numbers, spaces,
    /// dots and commas the SVG attribute accepts.
    pub dash: Option<String>,
}

impl ShapeStyle {
    /// Whether every field is still the "theme decides" default — the signal [`cascade`] and
    /// [`cascade_edge`] use to answer `None` rather than hand back a style that would draw
    /// identically to having none.
    pub fn is_empty(&self) -> bool {
        self.fill.is_none()
            && self.stroke.is_none()
            && self.stroke_width.is_none()
            && self.text.is_none()
            && self.dash.is_none()
    }

    /// Applies one `key:value` declaration, overwriting whatever this field already held — the
    /// whole of what makes a sequence of these a *cascade*. An unknown key and an unparsable value
    /// are both silently dropped: design principle #3 is "no rule ever fails a diagram", and one
    /// bad declaration (a typo, a CSS feature konoma does not draw) must cost only itself, not the
    /// picture around it.
    pub fn apply(&mut self, decl: &str) {
        let Some((key, value)) = decl.split_once(':') else {
            return;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "fill" => {
                if let Some(v) = paint(value) {
                    self.fill = Some(v);
                }
            }
            "stroke" => {
                if let Some(v) = paint(value) {
                    self.stroke = Some(v);
                }
            }
            "color" => {
                if let Some(v) = paint(value) {
                    self.text = Some(v);
                }
            }
            "stroke-width" => {
                if let Some(v) = px(value) {
                    self.stroke_width = Some(v);
                }
            }
            "stroke-dasharray" => {
                if let Some(v) = dasharray(value) {
                    self.dash = Some(v);
                }
            }
            _ => {}
        }
    }
}

/// Validates and passes through a `fill`/`stroke`/`color` value.
///
/// `none` is accepted verbatim: it is an SVG paint keyword, not a CSS `<color>`, so
/// `svgtypes::Color` has no rule for it and would otherwise reject the single most common
/// `fill` a `classDef` writes for "an outline with nothing behind it". Everything else has to
/// parse as a real colour — [`UiConfig::math_color`]'s exact convention, and for the reason its
/// own doc comment gives: usvg does not error on an unparsable paint, it draws black, and a
/// declaration that silently turns a node black is a worse failure than one silently dropped.
///
/// [`UiConfig::math_color`]: crate::config::UiConfig::math_color
fn paint(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("none") {
        return Some("none".to_string());
    }
    svgtypes::Color::from_str(value).ok()?;
    Some(value.to_string())
}

/// §10-3 item 6 ("辺色＝下流クラス色を1段明るく", `konoma-orthogonal` only — `route_flowchart`'s own
/// caller in `mod.rs` is the only place this is ever called): scales a colour's HSL lightness by
/// [`LIGHTEN_FACTOR`], leaving hue and saturation untouched, clamped so it never reaches pure
/// white.
///
/// # Why HSL lightness, and why this factor
///
/// The round-3 reference (`docs/mermaid-theme/handoff/round3-Konoma-Flowchart-Routing.dc.html`'s
/// `3a`) hand-picked three conversions — a class colour's `stroke:` to the edge colour flowing out
/// of it — that read as "the same colour, one step brighter": `#1f6feb`→`#58a6ff`,
/// `#2da44e`→`#3fb950`, `#d4a017`→`#d29922`. These are not a single closed-form transform (they
/// read like curated design-system tokens — GitHub Primer's own `fg`/`emphasis` pair for each hue
/// — not an algorithm anyone applied uniformly: their hue shifts by −4.5°/−8.3°/−2.9°, saturation
/// by +16.4/−7.7/−8.3 points, nothing near constant), so no formula reproduces all three exactly.
/// A single-parameter fit — multiply HSL lightness alone by a constant factor, least-squares fit
/// against the three references' own L values (52.2→67.3, 41.0→48.6, 46.1→47.8) — is the simplest
/// transform that stays inside "empirically chosen, error documented" rather than hand-coding a
/// 3-entry lookup table that would silently do nothing for a fourth colour. `LIGHTEN_FACTOR = 1.2`
/// (least-squares optimum ≈1.17, rounded to a plain number) gives L 62.6/49.2/55.3 against the
/// targets above — **errors of +4.7/+0.6/−7.5 percentage points of lightness** (blue undershoots,
/// green nearly exact, amber overshoots the least-lightened reference the most). Hue and
/// saturation are left untouched rather than also fit, since their reference deltas do not even
/// agree in sign across the three samples.
///
/// `None` for a colour [`svgtypes::Color`] cannot parse (the same permissive "leave the theme in
/// place" stance [`paint`] takes) or for the literal keyword `"none"` (has no hue to lighten).
pub fn lighten(color: &str) -> Option<String> {
    if color.eq_ignore_ascii_case("none") {
        return None;
    }
    let c = svgtypes::Color::from_str(color).ok()?;
    let (h, s, l) = rgb_to_hsl(c.red, c.green, c.blue);
    let l = (l * LIGHTEN_FACTOR).min(LIGHTEN_MAX_LIGHTNESS);
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// See [`lighten`]'s own doc for how this was fit.
const LIGHTEN_FACTOR: f64 = 1.2;
/// Never lighten all the way to white, however low the source lightness — a stroke has to stay a
/// stroke.
const LIGHTEN_MAX_LIGHTNESS: f64 = 92.0;

/// `(hue in [0, 360), saturation in [0, 100], lightness in [0, 100])` — the standard CSS/SVG HSL
/// convention, converted from 8-bit sRGB. A grey (`max == min`) has no hue or saturation at all;
/// this returns `(0.0, 0.0, lightness)` for one rather than an undefined hue, the conventional
/// choice `hsl_to_rgb`'s own "zero saturation ignores hue" branch already relies on.
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-9 {
        return (0.0, 0.0, l * 100.0);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < 1e-9 {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < 1e-9 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    ((h * 60.0).rem_euclid(360.0), s * 100.0, l * 100.0)
}

/// The inverse of [`rgb_to_hsl`], rounding each 8-bit channel to the nearest integer.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let (h, s, l) = (
        h / 360.0,
        (s / 100.0).clamp(0.0, 1.0),
        (l / 100.0).clamp(0.0, 1.0),
    );
    if s < 1e-9 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue_to_rgb = |t: f64| -> f64 {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let to_u8 = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    (
        to_u8(hue_to_rgb(h + 1.0 / 3.0)),
        to_u8(hue_to_rgb(h)),
        to_u8(hue_to_rgb(h - 1.0 / 3.0)),
    )
}

/// `stroke-width: 4px` or `stroke-width: 4` — mermaid accepts either, and every stroke width this
/// crate emits elsewhere is already a bare number, so the unit is stripped here rather than
/// carried through to `svg.rs`.
fn px(value: &str) -> Option<f64> {
    let value = value.trim();
    let value = value.strip_suffix("px").unwrap_or(value).trim();
    let w: f64 = value.parse().ok()?;
    if w.is_finite() && w >= 0.0 {
        Some(w)
    } else {
        None
    }
}

/// `stroke-dasharray: 5,5` / `5 5`.
///
/// Checked character-by-character rather than parsed as numbers, because the only thing this has
/// to prove is that the value cannot break out of the `"…"` SVG attribute it is written into —
/// what the numbers themselves mean is mermaid's business, not konoma's.
fn dasharray(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b' ' | b',' | b'.'))
    {
        return None;
    }
    Some(value.to_string())
}

/// Resolves the `classDef default` → named classes → own-`style` cascade for one element.
///
/// `class_of` looks a class name up in whatever shape the caller's own `classDef` list is —
/// [`crate::preview::mermaid::flowchart::ClassDef`] for a flowchart or an ER diagram, a
/// `Vec<(String, Vec<String>)>` for the two chart languages that keep their own — so this stays
/// the one place the cascade order itself is written, instead of every language re-deriving it.
///
/// Returns `None` when nothing in the cascade actually set a field, which is what keeps a diagram
/// with no `classDef` at all producing exactly the [`super::PlacedNode`]/[`super::PlacedEdge`] it
/// produced before this module existed.
pub fn cascade<'a>(
    class_of: impl Fn(&str) -> Option<&'a [String]>,
    classes: &[String],
    own_styles: &[String],
) -> Option<ShapeStyle> {
    let mut style = ShapeStyle::default();
    if let Some(decls) = class_of("default") {
        for decl in decls {
            style.apply(decl);
        }
    }
    for name in classes {
        if let Some(decls) = class_of(name) {
            for decl in decls {
                style.apply(decl);
            }
        }
    }
    for decl in own_styles {
        style.apply(decl);
    }
    if style.is_empty() {
        None
    } else {
        Some(style)
    }
}

/// An edge's own cascade: the classes it carries (`class <edgeId> name`, in applied order), then
/// its `linkStyle` layer.
///
/// # Why this is not [`cascade`] plus `linkStyle`
///
/// [`cascade`] starts every element at `classDef default`, because mermaid applies `default` to
/// **every node** automatically. An edge is not a node: `classDef default` has no such standing
/// rule for edges — an edge's baseline is `linkStyle default`, a declaration of its own — so
/// reusing [`cascade`] here would leak a node class onto every edge in the diagram whenever the
/// source happened to declare a `classDef default` for its boxes. This function starts an edge at
/// nothing instead, and named classes reach it only the way mermaid lets them: `class <edgeId>
/// name` naming the edge explicitly.
///
/// # Why `linkStyle default` is applied before an indexed one regardless of source order
///
/// The pipeline is fixed: every `linkStyle default` statement is applied — in the order the source
/// wrote more than one of them, so the last still wins among themselves — and only *then* every
/// `linkStyle <indices>` that names this edge, again in source order. An author who writes
/// `linkStyle 0 stroke:red` before `linkStyle default stroke:blue` still gets a red edge 0:
/// `default` sets the diagram's baseline and an index is always the more specific instruction,
/// which is the same reasoning a stylesheet's specificity uses, applied by hand because konoma's
/// SVG carries no stylesheet at all.
pub fn cascade_edge<'a>(
    class_of: impl Fn(&str) -> Option<&'a [String]>,
    classes: &[String],
    link_default: impl Iterator<Item = &'a [String]>,
    link_indexed: impl Iterator<Item = &'a [String]>,
) -> Option<ShapeStyle> {
    let mut style = ShapeStyle::default();
    for name in classes {
        if let Some(decls) = class_of(name) {
            for decl in decls {
                style.apply(decl);
            }
        }
    }
    for decls in link_default {
        for decl in decls {
            style.apply(decl);
        }
    }
    for decls in link_indexed {
        for decl in decls {
            style.apply(decl);
        }
    }
    if style.is_empty() {
        None
    } else {
        Some(style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`rgb_to_hsl`] round-trips through [`hsl_to_rgb`] back to the same 8-bit channels for every
    /// hue octant and every corner of the RGB cube (including the achromatic ones, where hue is
    /// undefined and `rgb_to_hsl` picks `0.0`) — the conversion pair [`lighten`] depends on, pinned
    /// on its own before that function's own approximate-by-construction reference test below.
    #[test]
    fn hsl_round_trips_every_corner_and_octant() {
        let cases: &[(u8, u8, u8)] = &[
            (0, 0, 0),
            (255, 255, 255),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (255, 255, 0),
            (0, 255, 255),
            (255, 0, 255),
            (31, 111, 235), // #1f6feb, one of `lighten`'s own reference colours
            (212, 160, 23), // #d4a017, another
        ];
        for &(r, g, b) in cases {
            let (h, s, l) = rgb_to_hsl(r, g, b);
            let (r2, g2, b2) = hsl_to_rgb(h, s, l);
            assert_eq!(
                (r, g, b),
                (r2, g2, b2),
                "hsl({h},{s},{l}) must round-trip back to rgb({r},{g},{b})"
            );
        }
    }

    #[test]
    fn lighten_leaves_none_and_an_unparsable_colour_alone() {
        assert_eq!(lighten("none"), None, "\"none\" has no hue to lighten");
        assert_eq!(
            lighten("NONE"),
            None,
            "the keyword check is case-insensitive"
        );
        assert_eq!(lighten("notacolor"), None);
    }

    #[test]
    fn lighten_never_reaches_pure_white() {
        // Already very light, so a naive `L * 1.2` would overshoot past 100.
        let lightened = lighten("#f5f5f5").expect("a light grey still has a hue-adjacent value");
        let (_, _, l) = rgb_to_hsl_hex(&lightened);
        // 8-bit RGB quantisation (`hsl_to_rgb`'s own `.round()`) can round the clamped L back up by
        // a fraction of a percentage point once re-measured through `rgb_to_hsl` — 0.5 is generous
        // slack for that, not a loosened clamp.
        assert!(
            l <= LIGHTEN_MAX_LIGHTNESS + 0.5,
            "L={l} must stay close to the {LIGHTEN_MAX_LIGHTNESS} clamp"
        );
    }

    /// [`lighten`]'s own doc: an empirically fit single-parameter transform, not an exact
    /// reproduction of Design's 3 hand-picked reference conversions. Pins the actual output (not
    /// the reference target) so a future change to `LIGHTEN_FACTOR`/`LIGHTEN_MAX_LIGHTNESS` has to
    /// touch this test deliberately, and separately asserts the *documented* error against each
    /// reference stays within the bound that doc states (a regression that silently changed the
    /// fit — not just a deliberate retune — would show up as a widening gap here).
    #[test]
    fn lighten_approximates_the_three_reference_conversions_within_its_documented_error() {
        let reference = [
            ("#1f6feb", "#58a6ff"),
            ("#2da44e", "#3fb950"),
            ("#d4a017", "#d29922"),
        ];
        for (source, target) in reference {
            let got = lighten(source).unwrap_or_else(|| panic!("{source} must parse"));
            let (_, _, l_got) = rgb_to_hsl_hex(&got);
            let (_, _, l_target) = rgb_to_hsl_hex(target);
            let error = (l_got - l_target).abs();
            assert!(
                error <= 8.0,
                "{source}->{got} (target {target}): lightness error {error} exceeds the \
                 documented ~7.5pt worst case"
            );
        }
        // Pin the exact output too, so a silent change in the fit is caught even when it happens
        // to stay within the 8pt tolerance above.
        assert_eq!(lighten("#1f6feb").as_deref(), Some("#508eef"));
        assert_eq!(lighten("#2da44e").as_deref(), Some("#36c55e"));
        assert_eq!(lighten("#d4a017").as_deref(), Some("#e9b631"));
    }

    /// Test-only helper: hex string straight to HSL, for asserting against `lighten`'s own output
    /// without re-deriving the RGB parse this module already does.
    fn rgb_to_hsl_hex(hex: &str) -> (f64, f64, f64) {
        let c = svgtypes::Color::from_str(hex).unwrap_or_else(|_| panic!("{hex} must parse"));
        rgb_to_hsl(c.red, c.green, c.blue)
    }

    #[test]
    fn an_unknown_color_is_dropped_not_substituted_with_black() {
        let mut s = ShapeStyle::default();
        s.apply("fill:notacolor");
        assert_eq!(s.fill, None);
    }

    #[test]
    fn none_is_a_real_fill_distinct_from_no_override() {
        let mut s = ShapeStyle::default();
        s.apply("fill:none");
        assert_eq!(s.fill.as_deref(), Some("none"));
    }

    #[test]
    fn a_hex_color_passes_through_unchanged() {
        let mut s = ShapeStyle::default();
        s.apply("fill:#f9f");
        assert_eq!(s.fill.as_deref(), Some("#f9f"));
    }

    #[test]
    fn stroke_width_sheds_its_px_suffix() {
        let mut s = ShapeStyle::default();
        s.apply("stroke-width:4px");
        assert_eq!(s.stroke_width, Some(4.0));
    }

    #[test]
    fn a_negative_stroke_width_is_dropped() {
        let mut s = ShapeStyle::default();
        s.apply("stroke-width:-2px");
        assert_eq!(s.stroke_width, None);
    }

    #[test]
    fn dasharray_rejects_anything_that_could_break_out_of_the_attribute() {
        let mut s = ShapeStyle::default();
        s.apply("stroke-dasharray:5,5\" onload=\"evil()");
        assert_eq!(s.dash, None);
    }

    #[test]
    fn later_declarations_overwrite_earlier_ones() {
        let mut s = ShapeStyle::default();
        s.apply("fill:#111111");
        s.apply("fill:#222222");
        assert_eq!(s.fill.as_deref(), Some("#222222"));
    }

    #[test]
    fn cascade_order_is_default_then_classes_then_own_style() {
        let defs: Vec<(&str, Vec<String>)> = vec![
            ("default", vec!["fill:#111111".to_string()]),
            ("hot", vec!["fill:#222222".to_string()]),
        ];
        let class_of = |name: &str| {
            defs.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| s.as_slice())
        };
        let classes = vec!["hot".to_string()];
        let own = vec!["fill:#333333".to_string()];
        let style = cascade(class_of, &classes, &own).unwrap();
        assert_eq!(style.fill.as_deref(), Some("#333333"));

        let style = cascade(class_of, &classes, &[]).unwrap();
        assert_eq!(style.fill.as_deref(), Some("#222222"));

        let style = cascade(class_of, &[], &[]).unwrap();
        assert_eq!(style.fill.as_deref(), Some("#111111"));
    }

    #[test]
    fn no_classdef_at_all_resolves_to_none() {
        let class_of = |_: &str| None;
        assert_eq!(cascade(class_of, &[], &[]), None);
    }

    /// `classDef default` styles every *node* automatically ([`cascade_order_is_default_then_classes_then_own_style`]
    /// above states that for [`cascade`]). An edge is not a node, and mermaid has no "every edge
    /// gets `classDef default`" rule — an edge's baseline is `linkStyle default`, not `classDef
    /// default`. A `cascade_edge` that reused [`cascade`] verbatim would leak a `classDef default`
    /// meant for boxes onto every line in the diagram; this is the regression a coordinator's own
    /// mutation pass on the golden corpus caught (`docs/STATUS.md`'s flowchart entry).
    #[test]
    fn classdef_default_does_not_leak_onto_an_edge_with_no_class_or_linkstyle() {
        let defs: Vec<(&str, Vec<String>)> = vec![("default", vec!["stroke:#556".to_string()])];
        let class_of = |name: &str| {
            defs.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| s.as_slice())
        };
        let style = cascade_edge(class_of, &[], std::iter::empty(), std::iter::empty());
        assert_eq!(
            style, None,
            "an edge named by neither `class` nor `linkStyle` must draw in the theme's own colour"
        );
    }

    /// A `class <edgeId> name` statement — mermaid's real mechanism for putting a `classDef` onto
    /// an edge — still reaches [`cascade_edge`], unlike the automatic `default`.
    #[test]
    fn a_class_statement_naming_an_edge_still_reaches_it() {
        let defs: Vec<(&str, Vec<String>)> = vec![("hot", vec!["stroke:#a00".to_string()])];
        let class_of = |name: &str| {
            defs.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| s.as_slice())
        };
        let classes = vec!["hot".to_string()];
        let style = cascade_edge(class_of, &classes, std::iter::empty(), std::iter::empty())
            .expect("the edge's own class must still resolve");
        assert_eq!(style.stroke.as_deref(), Some("#a00"));
    }
}
