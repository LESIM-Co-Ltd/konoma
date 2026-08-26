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
    /// The solid marks a state diagram is built out of: the `[*]` dots and the fork/join bars.
    ///
    /// These are *filled* shapes with nothing behind them, so §4-1's problem bites here harder
    /// than anywhere else — a black dot on a dark terminal is not a faint line, it is nothing at
    /// all. Every palette therefore points this at the same colour as [`Theme::line`], which the
    /// contrast test already holds to both grounds.
    pub state_marker: &'static str,
    /// Interior of a note's box.
    pub note_fill: &'static str,
    /// Outline of a note's box.
    pub note_stroke: &'static str,
    /// Text inside a note.
    pub note_text: &'static str,
    /// A sequence diagram's lifeline: the vertical line under a participant.
    ///
    /// Deliberately a different colour from [`Theme::line`], and drawn thinner and dashed. A
    /// lifeline is structure, not content — it runs the whole height of the diagram behind every
    /// message, and at the same weight as the arrows it turns a sequence diagram into a grid.
    pub lifeline: &'static str,
    /// Interior of an activation bar.
    pub activation_fill: &'static str,
    /// Outline of an activation bar.
    pub activation_stroke: &'static str,
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
    state_marker: "#d3d3d3",
    note_fill: "#3d3a2a",
    note_stroke: "#b8a94e",
    note_text: "#f0e6bf",
    lifeline: "#8a8a8a",
    activation_fill: "#5a5a66",
    activation_stroke: "#cccccc",
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
    state_marker: "#64748b",
    note_fill: "#fff8d5",
    note_stroke: "#b8a94e",
    note_text: "#3a3524",
    lifeline: "#94a3b8",
    activation_fill: "#dbe3ec",
    activation_stroke: "#64748b",
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
    state_marker: "#5c6b8a",
    note_fill: "#fff5ad",
    note_stroke: "#aaaa33",
    note_text: "#333333",
    lifeline: "#9aa4bd",
    activation_fill: "#dcdcf5",
    activation_stroke: "#7b88a8",
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
    state_marker: "#008000",
    note_fill: "#fff5ad",
    note_stroke: "#aaaa33",
    note_text: "#333333",
    lifeline: "#6eaa49",
    activation_fill: "#e2f3cf",
    activation_stroke: "#13540c",
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
    state_marker: "#666666",
    note_fill: "#f0f0e0",
    note_stroke: "#999999",
    note_text: "#333333",
    lifeline: "#999999",
    activation_fill: "#e0e0e0",
    activation_stroke: "#666666",
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
