//! konoma's own mermaid renderer.
//!
//! Being built in stages; see `docs/FEATURE-MERMAID-RENDERER.md` §7.
//!
//! * **Stage 0** laid down the shared groundwork: [`text_metrics`], which measures text the
//!   way resvg will actually draw it, plus the instrument that proves the two agree.
//! * **Stage 1a** vendored the layout engine into [`layout`] — a copy of the `dagre` crate,
//!   carried in `src/` rather than taken as a dependency so konoma can fix it in a published
//!   build. It arrives with the 30-graph cross-validation baseline recorded from the real
//!   dagre.js, which is this renderer's only external source of truth.
//! * **Stage 1b** added [`flowchart`], which reads mermaid's flowchart language — three quarters
//!   of the mermaid people actually write — into a structural model. It parses and stops: no
//!   measuring, no layout, no SVG.
//! * **Stage 1c** added [`render`], which is where the three of them meet and a picture comes
//!   out: labels measured by [`text_metrics`], boxes sized by mermaid's own shape formulas,
//!   positions from [`layout`], and then the part that is konoma's alone — clipping each edge to
//!   the shape it touches, curving it, and putting its label back on the middle of the line.
//! * **Stage 1d** framed subgraphs.
//! * **Stage 1e** put it on the drawing path. `preview::markdown::mermaid_to_svg` now asks
//!   [`flowchart::is_flowchart`] which renderer owns a diagram: a `flowchart` / `graph` /
//!   `flowchart-elk` is drawn here. At the time, every other kind still went to
//!   `mermaid-rs-renderer`; a flowchart this module refused was **not** handed to it, so a bug
//!   here stayed visible instead of being painted over by the renderer it was replacing. Stage 5b
//!   finished the job and removed that crate, and the discipline outlived it: a diagram konoma
//!   refuses degrades to the Unicode text diagram.
//! * **Stage 2** added [`state`], the `stateDiagram` language, and — this is the part worth
//!   copying — added *no layout*. It builds the same language-neutral `render::GraphSpec` a
//!   flowchart does and shares everything downstream of it.
//! * **Stage 3** added [`class`] and [`er`] the same way, which takes §2-1's running total to
//!   90% of the mermaid people actually write. Between them they needed one new idea: a node
//!   whose contents are a *table* rather than one centred label, which is
//!   [`render::panel`](render::panel). Everything else — the layout, the frames, the clipping,
//!   the curve, the viewBox, the eleven geometric invariants — is stage 1's, unchanged.
//! * **Stage 4** added [`sequence`], and with it the first language that does **not** go through
//!   `lay_out_spec`: a sequence diagram is not a layered graph, so it walks its own events and
//!   builds a `Diagram` in one pass. Everything below layout is still shared.
//! * **Stage 5** added [`chart`] — seven data charts (`pie`, `xychart-beta`, `quadrantChart`,
//!   `radar-beta`, `treemap-beta`, `packet-beta`, `sankey-beta`), none of which is a graph either.
//!   They needed seven [`render::shapes::Glyph`] variants, three
//!   [`render::shapes::Mark`]s and a categorical palette, and **no second SVG writer and no new
//!   `Diagram` field**: a chart's marks are nodes and its lines are edges, so `svg::emit` and
//!   `normalise` are the ones stage 1 wrote.

pub mod architecture;
pub mod block;
pub mod c4;
pub mod chart;
pub mod class;
pub mod er;
pub mod flowchart;
pub mod gantt;
pub mod gitgraph;
pub mod journey;
pub mod kanban;
pub mod layout;
pub mod mindmap;
pub mod render;
pub mod requirement;
pub mod sequence;
pub mod state;
pub mod text_metrics;
pub mod timeline;
pub mod zenuml;
