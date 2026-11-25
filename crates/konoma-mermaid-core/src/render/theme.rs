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

/// The drawing decisions a palette makes **beyond colour** — the design reference's own
/// "トークン" list (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 5).
///
/// Every one of these is a number or a flag some part of [`super::svg`] used to hard-code, so a
/// palette that wants the reference look could not express it. They live in one optional struct
/// rather than as a dozen more fields on [`Theme`] for a reason that is worth stating: a palette
/// that carries `None` reaches **not one** of the branches below, so "every theme that existed
/// before this struct draws byte for byte what it drew" is visible in the type rather than only
/// in a test — the five palettes each say `tokens: None` once, and nothing else about them moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// Corner radius of an ordinary box — a flowchart's `A[…]`/`A(…)` and a state diagram's own
    /// state boxes. Replaces [`super::shapes::CORNER_RADIUS`] for those two glyphs only, so a
    /// stadium stays a pill and a class/ER box stays square.
    pub node_radius: f64,
    /// Corner radius of a frame, replacing [`super::clusters::CORNER_RADIUS`].
    pub frame_radius: f64,
    /// Whether a `subgraph` frame's interior is painted at all.
    pub cluster_filled: bool,
    /// Dash pattern of a frame's outline, replacing [`super::svg::CLUSTER_DASH`] — and applied to
    /// an ordinary (undashed) `subgraph` frame too, which is what makes a subgraph read as ground
    /// without a fill behind it.
    pub cluster_dash: &'static str,
    /// Outline of a composite state's frame (the one drawn with a title strip), which the
    /// reference draws in a different, quieter grey from a flowchart `subgraph`'s own.
    pub composite_stroke: &'static str,
    /// The 1px rule dividing a composite state's title strip from its interior.
    pub composite_rule: &'static str,
    /// Text of a composite state's strip title.
    pub composite_text: &'static str,
    /// Stroke width of an ordinary (and of a dotted) edge, replacing
    /// [`super::svg::EDGE_STROKE_WIDTH`]. A `===` edge keeps its own thick width — it means
    /// "heavier than the others", which only holds relative to whatever the others are.
    pub edge_width: f64,
    /// Dash pattern of a `-.-` edge, replacing [`super::svg::DOTTED_DASH`].
    pub dotted_dash: &'static str,
    /// Font size an edge label is drawn at.
    pub edge_label_font_size: f64,
    /// Font size a flowchart `subgraph`'s title is drawn at. A composite state's strip title keeps
    /// §10-5 S2's own `svg::STRIP_FONT_SIZE`, which the reference already agrees with.
    pub title_font_size: f64,
    /// Draw a `subgraph`'s title at the frame's top-left corner rather than centred on its top
    /// edge, the way a composite state's strip title already is.
    pub title_left_aligned: bool,
    /// Paint an edge's terminal mark and its label in the **edge's own colour** rather than in
    /// [`Theme::arrowhead`]/[`Theme::edge_label_text`] —
    /// "辺・矢尻・ラベル文字・ノード枠を同色で揃える".
    ///
    /// [`super::PlacedEdge::tip_matches_line`] already says this for a flowchart's own arrow head;
    /// stating it here as well is what extends it to a *state* diagram's arrow heads and to every
    /// edge's label text, both of which that flag does not reach.
    pub ink_follows_line: bool,
    /// Derive a class-coloured node's **fill** from its own stroke (via [`super::style::tint`])
    /// when the source named a `stroke:` but no `fill:` — the reference's "ノード塗りは薄い塗り".
    pub tint_class_fill: bool,
}

/// One palette. Every colour is a lowercase 6-digit hex string, normalised on the way in rather
/// than at emission time — §1 notes that usvg accepts named colours and `rgba()` too, but a
/// single spelling is one less thing for a golden to be sensitive to.
#[derive(Debug, Clone, Copy, PartialEq)]
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

    // --- stage 5: the colours a data chart needs ----------------------------------------------
    /// Axis lines, a plot's frame, a radar's graticule. Structure, so quieter than [`Theme::line`]
    /// — an axis that competes with the data is an axis drawn wrong.
    pub axis: &'static str,
    /// Grid lines inside a plot. Quieter again: a reader should be able to *find* a gridline, not
    /// be unable to look past it.
    pub grid: &'static str,
    /// The categorical palette. One entry per series, wrapped when a chart has more series than
    /// the palette has colours.
    ///
    /// **Chosen so that [`Theme::series_text`] reads on every one of them**, which is what
    /// `super::tests` holds them to: a pie's percentage, a treemap's tile name and a packet
    /// field's label are all drawn *on* one of these, so a palette entry that swallows the text is
    /// not a style preference, it is a datum the reader cannot get at.
    pub series: &'static [&'static str],
    /// Text drawn on top of a [`Theme::series`] colour.
    pub series_text: &'static str,

    /// The non-colour drawing decisions this palette makes, or `None` for "every default
    /// [`super::svg`] hard-codes" — see [`Tokens`] for why this is one optional struct.
    pub tokens: Option<&'static Tokens>,
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
    axis: "#8a8a8a",
    grid: "#4a4a4a",
    series: &[
        "#4e79a7", "#4a8a42", "#b07aa1", "#9c755f", "#3f6b78", "#8c6d31", "#a8564c", "#6b6ecf",
    ],
    series_text: "#f2f2f2",
    tokens: None,
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
    axis: "#94a3b8",
    grid: "#c8d4e0",
    series: &[
        "#7ea6d8", "#8fc98a", "#d3a7cb", "#c9ab97", "#86b3bd", "#cfae6a", "#e39b93", "#a4a7e0",
    ],
    series_text: "#1a2230",
    tokens: None,
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
    axis: "#9aa4bd",
    grid: "#cfd4de",
    series: &[
        "#9fb3d9", "#a7d3a0", "#dcb6d6", "#d5bda9", "#8bc0cc", "#e0c079", "#eeaaa2", "#b3b6ea",
    ],
    series_text: "#242a36",
    tokens: None,
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
    axis: "#6eaa49",
    grid: "#c3dfae",
    series: &[
        "#8fbf6a", "#b6dd9a", "#cfe3a8", "#7aa9a0", "#c8c07a", "#a2c3d6", "#d6b48f", "#8fa8d8",
    ],
    series_text: "#1f2a17",
    tokens: None,
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
    axis: "#999999",
    grid: "#d6d6d6",
    series: &[
        "#e7e7e7", "#818181", "#c5c5c5", "#929292", "#d6d6d6", "#a3a3a3", "#b4b4b4", "#707070",
    ],
    series_text: "#1f1f1f",
    tokens: None,
};

/// [`KONOMA`]'s own non-colour half. Every number is read off the design reference's SVG
/// (`docs/render-check/zz-design-{2a,2b,2c,4a,4b,4c}-wrap.html`, the source the browser renders
/// `zz-design-*-browser.png` from) rather than from the prose token list, which rounds some of
/// them.
pub const KONOMA_TOKENS: Tokens = Tokens {
    // `rx="3"` on every node rect in `2a`/`4a`/`4c`, and on the composite frames of `4a`/`4c`.
    node_radius: 3.0,
    frame_radius: 3.0,
    // `2a`: `<rect … fill="none" stroke="#484f58" stroke-width="1" stroke-dasharray="4,3">`.
    cluster_filled: false,
    cluster_dash: "4,3",
    // `4c`: the composite frames are `stroke="#6e7681"`, their strip rule `stroke="#30363d"` and
    // their title `font-size="11" fill="#8b949e"` — all three different from a flowchart
    // `subgraph`'s own `#484f58`/`#6e7681`.
    composite_stroke: "#6e7681",
    composite_rule: "#30363d",
    composite_text: "#8b949e",
    // `2a`/`4c` draw every edge inside one `<g … stroke-width="1.5">`, and a back edge as
    // `stroke-dasharray="5,4"`.
    edge_width: 1.5,
    dotted_dash: "5,4",
    // `2a`: an edge label is `font-size="11"`, and a frame title sits top-left inside the frame
    // (`<rect x="55" y="150" …><text x="63" y="166" font-size="11">`). The title is 12 here, not
    // the drawing's 11: §10-1 item 5's own token list says "12px 相当" and konoma's body text is
    // 14 against the reference drawing's 12, so the *ratio* the reference draws is what carries
    // over, not its absolute pixel.
    edge_label_font_size: 11.0,
    title_font_size: 12.0,
    title_left_aligned: true,
    ink_follows_line: true,
    tint_class_fill: true,
};

/// konoma's own look: the palette Claude Design drew konoma's diagrams in
/// (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 5's token list, and the design SVGs
/// [`KONOMA_TOKENS`] cites).
///
/// **Not a `[ui] mermaid_theme` value.** It is the palette half of `[ui] mermaid_routing =
/// "konoma-orthogonal"`, and [`Theme::for_routing`] is the only thing that ever selects it — see
/// that function for why the mode owns its own colours.
///
/// A dark-terminal palette like [`DARK`], but built out of the reference's own greys rather than
/// mermaid's: a near-black node fill under a mid grey outline, one bright text colour, and a
/// single muted grey for every line, arrowhead and marker.
pub const KONOMA: Theme = Theme {
    name: "konoma",
    background_paint: "none",
    // The page the reference is drawn on (`<svg style="background: #0d1117">`). Never painted —
    // it is what an edge label's patch is filled with so the words stay readable on the line.
    background_ref: "#0d1117",
    node_fill: "#161b22",
    node_stroke: "#8b949e",
    node_text: "#e6edf3",
    line: "#8b949e",
    arrowhead: "#8b949e",
    edge_label_text: "#8b949e",
    // Never painted (`KONOMA_TOKENS::cluster_filled` is false); carried so the palette is complete
    // and so the "every colour is a normalised hex" test has something real to check.
    cluster_fill: "#161b22",
    cluster_stroke: "#484f58",
    cluster_text: "#6e7681",
    state_marker: "#8b949e",
    // The reference has no notes, no lifelines and no charts. These keep the reference's own
    // family — GitHub-dark surfaces under `node_text` — with the amber the design already uses for
    // its third accent marking a note as an aside.
    note_fill: "#1c2128",
    note_stroke: "#d29922",
    note_text: "#e6edf3",
    lifeline: "#484f58",
    activation_fill: "#30363d",
    activation_stroke: "#8b949e",
    axis: "#484f58",
    grid: "#21262d",
    // The three accents the design names (`#58a6ff` blue, `#3fb950` green, `#d29922` amber), then
    // the two more its `2b`/`2c` sources add (`#a371f7` purple, `#f85149` red), then three further
    // GitHub-dark hues so a chart with more than five series still tells them apart.
    series: &[
        "#58a6ff", "#3fb950", "#d29922", "#a371f7", "#f85149", "#39c5cf", "#db61a2", "#a5d6ff",
    ],
    series_text: "#0d1117",
    tokens: Some(&KONOMA_TOKENS),
};

/// Every `[ui] mermaid_theme` value, for the tests that have to hold each one to the same rule.
pub const ALL: &[Theme] = &[DARK, LIGHT, CLASSIC, FOREST, NEUTRAL];

/// [`ALL`] plus [`KONOMA`], which is not a theme value but is still a palette and still has to
/// answer to every rule a palette answers to (normalised hex, contrast on both grounds).
pub const EVERY_PALETTE: &[Theme] = &[DARK, LIGHT, CLASSIC, FOREST, NEUTRAL, KONOMA];

impl Theme {
    /// The `i`th series colour, wrapping when a chart has more series than the palette has
    /// colours. Wrapping rather than fading out: eight is already more series than a terminal-sized
    /// chart can be read at, and a ninth drawn in the first colour is at least a colour.
    pub fn series(&self, i: usize) -> &'static str {
        self.series[i % self.series.len()]
    }

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

    /// The palette a flowchart or a state diagram draws in, given `ui.mermaid_theme`'s raw string
    /// and the routing already resolved for it.
    ///
    /// **`Routing::Orthogonal` is always [`KONOMA`], whatever the theme says.** `konoma-orthogonal`
    /// is not a router with a palette bolted on, it is one designed picture
    /// (`docs/FEATURE-MERMAID-RENDERER.md` §10 — the reference drawings the mode was built from are
    /// *drawn in these colours*), so a mermaid palette underneath it would draw something the
    /// design never describes. `ui.mermaid_theme` is inert there for exactly the reason
    /// `ui.mermaid_curve` already is: the mode answers that question itself.
    ///
    /// `Routing::Splines` is [`named`](Theme::named), unchanged — which is every diagram konoma has
    /// ever drawn by default, byte for byte.
    pub fn for_routing(name: &str, routing: super::Routing) -> Theme {
        match routing {
            super::Routing::Orthogonal => KONOMA,
            super::Routing::Splines => Theme::named(name),
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
