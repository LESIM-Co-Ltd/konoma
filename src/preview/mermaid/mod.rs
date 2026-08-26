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
//!   `flowchart-elk` is drawn here, and every other kind still goes to `mermaid-rs-renderer`
//!   until the later stages take those over too (§7 — the migration must not shrink the set of
//!   diagrams konoma can draw). A flowchart this module refuses is **not** handed to the crate:
//!   it degrades to the Unicode text diagram, so a bug here stays visible instead of being
//!   painted over by the renderer it is replacing.
//! * **Stage 2** added [`state`], the `stateDiagram` language, and — this is the part worth
//!   copying — added *no layout*. It builds the same language-neutral `render::GraphSpec` a
//!   flowchart does and shares everything downstream of it.
//! * **Stage 3** added [`class`] and [`er`] the same way, which takes §2-1's running total to
//!   90% of the mermaid people actually write. Between them they needed one new idea: a node
//!   whose contents are a *table* rather than one centred label, which is
//!   [`render::panel`](render::panel). Everything else — the layout, the frames, the clipping,
//!   the curve, the viewBox, the eleven geometric invariants — is stage 1's, unchanged.

pub mod class;
pub mod er;
pub mod flowchart;
pub mod layout;
pub mod render;
pub mod state;
pub mod text_metrics;
