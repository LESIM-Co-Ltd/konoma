//! Colors for the diagrams konoma draws itself.
//!
//! # Why these are not mermaid's colors
//!
//! `docs/FEATURE-MERMAID-RENDERER.md` §0-1 settles the acceptance criterion: the same source read
//! correctly *in konoma's idiom*, not a pixel match with GitHub. §4-1 then shows why colour is
//! where that bites first. konoma never paints a background — the terminal shows through
//! (`§1`) — and mermaid's light themes assume a white page underneath:
//!
//! | theme | mermaid's effective `lineColor` | on a dark terminal |
//! |---|---|---|
//! | forest | `#000000` | the edges disappear |
//! | default | `#333333` | barely visible |
//! | neutral | `#666666` | hard to read |
//!
//! (forest's `theme-forest.js` line 17 says `lineColor = 'green'`, but line 31's
//! `invert(this.background)` overwrites it unconditionally. Reading only line 17 gets this wrong.)
//!
//! So every theme here picks a line colour that survives *both* grounds, and
//! [`super::tests`] pins that as a contrast test rather than as a comment.
//!
//! # `background_paint` vs `background_ref`
//!
//! §4-2: mermaid's `background` is not a paint, it is the input to derivations like
//! `lineColor = invert(background)`. The crate konoma ships today assigns `theme.background =
//! "none"` to stop the white card, and that string then leaks into the twelve other places the
//! crate uses `background` for real colour. Splitting the two fields makes that impossible:
//! [`Theme::background_paint`] is always `none` and is the only thing that reaches a `fill`,
//! while [`Theme::background_ref`] is the colour the theme *derives* from and is what backs an
//! edge label so its text stays readable where it crosses a line.

/// One palette. Every colour is a lowercase 6-digit hex string, normalised on the way in rather
/// than at emission time — §1 notes that usvg accepts named colours and `rgba()` too, but a
/// single spelling is one less thing for a golden to be sensitive to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// The name this palette answers to, for diagnostics.
    pub name: &'static str,
    /// What is painted behind the diagram. **Always `"none"`** — see the module docs.
    pub background_paint: &'static str,
    /// The colour the palette is derived from, and the backing of an edge label. Never painted
    /// across the whole diagram.
    pub background_ref: &'static str,
    /// Interior of a node's shape.
    pub node_fill: &'static str,
    /// Outline of a node's shape.
    pub node_stroke: &'static str,
    /// Text inside a node.
    pub node_text: &'static str,
    /// The edge line itself.
    pub line: &'static str,
    /// Arrow heads, circles and crosses at the end of an edge.
    pub arrowhead: &'static str,
    /// Text of an edge label.
    pub edge_label_text: &'static str,
    /// Interior of a subgraph frame. Unused until stage 1d; carried so the palettes are complete
    /// in one place rather than grown a field at a time.
    pub cluster_fill: &'static str,
    /// Outline of a subgraph frame.
    pub cluster_stroke: &'static str,
    /// A subgraph's title. §4-1 flags this for the same reason as `lineColor`: mermaid's light
    /// themes put it at `#333`.
    pub cluster_text: &'static str,
}

/// mermaid's `dark`, which is konoma's default and the only theme tuned for a dark terminal
/// alone. Values follow the palette konoma already ships (node `#1f2020` on `#333333`), because
/// this is the one theme where the current output is not fighting the terminal.
pub const DARK: Theme = Theme {
    name: "dark",
    background_paint: "none",
    background_ref: "#333333",
    node_fill: "#1f2020",
    node_stroke: "#cccccc",
    node_text: "#cccccc",
    line: "#d3d3d3",
    arrowhead: "#d3d3d3",
    edge_label_text: "#cccccc",
    cluster_fill: "#2b2b38",
    cluster_stroke: "#8a8a8a",
    cluster_text: "#cccccc",
};

/// konoma's `light` (the crate calls it `modern`): a slate palette for a light terminal.
pub const LIGHT: Theme = Theme {
    name: "light",
    background_paint: "none",
    background_ref: "#ffffff",
    node_fill: "#f8fafc",
    node_stroke: "#94a3b8",
    node_text: "#0f172a",
    line: "#64748b",
    arrowhead: "#64748b",
    edge_label_text: "#0f172a",
    cluster_fill: "#f1f5f9",
    cluster_stroke: "#cbd5e1",
    cluster_text: "#0f172a",
};

/// mermaid's `default`, which konoma spells `classic`. The lavender boxes are the look most
/// people recognise; only the line is moved off mermaid's `#333333`, which §4-1 measured as
/// "barely visible" once the terminal shows through.
pub const CLASSIC: Theme = Theme {
    name: "classic",
    background_paint: "none",
    background_ref: "#ffffff",
    node_fill: "#ececff",
    node_stroke: "#7b88a8",
    node_text: "#333333",
    line: "#5c6b8a",
    arrowhead: "#5c6b8a",
    edge_label_text: "#333333",
    cluster_fill: "#ffffde",
    cluster_stroke: "#aaaa33",
    cluster_text: "#333333",
};

/// mermaid's `forest`. The deliberate deviation §4-1 asks to keep: the line stays green instead
/// of the `#000000` that `invert(background)` actually produces upstream.
pub const FOREST: Theme = Theme {
    name: "forest",
    background_paint: "none",
    background_ref: "#ffffff",
    node_fill: "#cde498",
    node_stroke: "#13540c",
    node_text: "#333333",
    line: "#008000",
    arrowhead: "#008000",
    edge_label_text: "#333333",
    cluster_fill: "#cdffb2",
    cluster_stroke: "#6eaa49",
    cluster_text: "#333333",
};

/// mermaid's `neutral`: greyscale, and the one light theme whose own line colour already sits in
/// the band that reads on either ground.
pub const NEUTRAL: Theme = Theme {
    name: "neutral",
    background_paint: "none",
    background_ref: "#ffffff",
    node_fill: "#eeeeee",
    node_stroke: "#999999",
    node_text: "#333333",
    line: "#666666",
    arrowhead: "#666666",
    edge_label_text: "#333333",
    cluster_fill: "#eaeaea",
    cluster_stroke: "#999999",
    cluster_text: "#333333",
};

/// Every palette, for the tests that have to hold each one to the same rule.
pub const ALL: &[Theme] = &[DARK, LIGHT, CLASSIC, FOREST, NEUTRAL];

impl Theme {
    /// Resolves `ui.mermaid_theme`'s raw string.
    ///
    /// **An unrecognised value is `dark`, silently** — that is the contract
    /// `preview::markdown::mermaid_to_svg` has today (`docs/FEATURE-MERMAID-RENDERER.md` §1) and
    /// changing it would turn a typo in a config file into a diagram that stops rendering.
    /// mermaid's own aliases are accepted for the two themes konoma renamed.
    pub fn named(name: &str) -> Theme {
        match name {
            "light" | "modern" => LIGHT,
            "classic" | "mermaid" | "default" => CLASSIC,
            "forest" => FOREST,
            "neutral" => NEUTRAL,
            _ => DARK,
        }
    }

    /// Relative luminance (WCAG) of a `#rrggbb` string; used by the contrast tests.
    pub fn luminance(hex: &str) -> f64 {
        fn channel(c: u8) -> f64 {
            let s = c as f64 / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        let (r, g, b) = parse_hex(hex).unwrap_or((0, 0, 0));
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG contrast ratio between two `#rrggbb` strings, in `1.0..=21.0`.
    pub fn contrast(a: &str, b: &str) -> f64 {
        let (la, lb) = (Theme::luminance(a), Theme::luminance(b));
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }
}

/// Splits `#rrggbb` into bytes. `None` for anything else, which is what makes the "every colour
/// is a normalised hex" test able to fail rather than quietly pass.
pub fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let s = hex.strip_prefix('#')?;
    if s.len() != 6
        || !s
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some((
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    ))
}
