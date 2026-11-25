//! The entry points that turn mermaid source into an SVG: routing to whichever of konoma's
//! renderers owns the diagram kind, panic safety, and the start-up warm-up.
//!
//! Moved unchanged from `preview::markdown`; the host re-exports these names from there.

use konoma_base::panic::silence_panics;

/// mermaid source → SVG string, routed to whichever renderer owns that kind of diagram.
///
/// # Which renderer draws what (stage 1e)
///
/// * **`flowchart` / `graph` / `flowchart-elk` → konoma's own renderer**
///   ([`crate::render`]). Three quarters of the mermaid people write
///   (`docs/FEATURE-MERMAID-RENDERER.md` §2-1).
/// * **`stateDiagram` (stage 2), `classDiagram` and `erDiagram` (stage 3) → konoma's own
///   renderer** as well, through [`crate::render::state`],
///   [`crate::render::class`] and [`crate::render::er`].
///   With ER at 10% of what people write, §2-1's running total is 90%.
/// * **the seven data charts (stage 5) → konoma's own renderer** as well, through
///   [`crate::render::chart`]: `pie`, `xychart-beta`, `quadrantChart`,
///   `radar-beta`, `treemap-beta`, `packet-beta` and `sankey-beta`.
/// * **the eleven remaining kinds (stage 5b) → konoma's own renderer** as well: `mindmap`,
///   `kanban`, `journey`, `timeline`, `gantt`, `requirementDiagram`, `gitGraph`, the five C4
///   keywords, `block-beta`, `architecture-beta` and `zenuml`. That is all twenty-three the crate
///   ever drew, so **the crate is gone** — `cargo tree -i mermaid-rs-renderer` no longer resolves.
/// * **anything else → nothing at all.** There is no second renderer left to ask, which is the
///   point: a diagram konoma cannot read degrades to the Unicode text diagram rather than being
///   guessed at. `venn-beta` is the case §6-A records the crate answering with a lying three-box
///   diagram, and it is now refused.
///
/// The routing question is asked *before* the renderer runs
/// ([`crate::flowchart::is_flowchart`] and its twenty-two siblings) rather than
/// derived from a failure. Throughout the migration that mattered because a fallback would have
/// let the old crate paint over a bug in the new renderer — the same shape as the markdown diff
/// harness that spent a release comparing a renderer with itself. Now there is nothing to fall
/// back *to*, and the discipline still holds: a diagram konoma refuses returns `None` and the
/// caller degrades to the Unicode text diagram, which is what konoma already promises for
/// anything it cannot draw (PRD design principle #3).
///
/// The path is panic-safe, and returns `None` on failure, which is the contract in §1.
/// [`mermaid_to_svg_reason`] is the same function with the *reason* kept — §8 notes that throwing
/// those messages away is one of the things this work exists to fix.
///
/// Runs on worker threads only (layout costs a few ms per diagram).
pub fn mermaid_to_svg(code: &str, theme: &str) -> Option<String> {
    mermaid_to_svg_reason(code, theme).ok()
}

/// [`mermaid_to_svg`], with a flowchart's edges drawn in `curve` (`ui.mermaid_curve`'s raw
/// string — `"basis"` reproduces [`mermaid_to_svg`] exactly, byte for byte). Every other diagram
/// kind ignores `curve` entirely, the same as [`crate::render::render_curve`]
/// it is threaded into.
///
/// Superseded in production by [`mermaid_to_svg_flow`] — a media loader always knows
/// `[ui] mermaid_routing` too, so `App::media_load`/`md_media` call that directly rather than
/// this. Kept (curve-only, no `routing` argument) for the tests that predate `[ui]
/// mermaid_routing` and still want to say exactly that: unlike
/// [`crate::render::render_curve`] — which stays reachable through
/// [`crate::render::render`] — this one has no production caller of its own.
pub fn mermaid_to_svg_curve(code: &str, theme: &str, curve: &str) -> Option<String> {
    mermaid_to_svg_reason_curve(code, theme, curve).ok()
}

/// [`mermaid_to_svg_curve`], with a flowchart's edges routed by `routing` (`ui.mermaid_routing`'s
/// raw string — `"splines"` reproduces [`mermaid_to_svg_curve`] exactly, byte for byte). Every
/// other diagram kind ignores `routing` entirely, the same as
/// [`crate::render::render_flow`] it is threaded into. This is what
/// `App::media_load`/`md_media` actually call, since a media loader knows both settings.
pub fn mermaid_to_svg_flow(code: &str, theme: &str, curve: &str, routing: &str) -> Option<String> {
    mermaid_to_svg_reason_flow(code, theme, curve, routing).ok()
}

/// [`mermaid_to_svg`], keeping why a diagram was not drawn.
///
/// The `Err` is a short English diagnostic from konoma's own parser — `unclosed \`"\` at line 3`,
/// `no such shape: \`nope\``, `not a mermaid diagram konoma can draw: the source starts with
/// \`venn-beta\``. Nothing shows it as-is today, but §8 lists throwing these away as one of the
/// things this work exists to fix, and it is what the tests here assert on instead of asserting
/// `is_none()` and learning nothing about *why*.
pub fn mermaid_to_svg_reason(code: &str, theme: &str) -> Result<String, String> {
    mermaid_to_svg_reason_curve(code, theme, "basis")
}

/// [`mermaid_to_svg_reason`] / [`mermaid_to_svg_curve`] combined.
pub fn mermaid_to_svg_reason_curve(code: &str, theme: &str, curve: &str) -> Result<String, String> {
    mermaid_to_svg_reason_flow(code, theme, curve, "splines")
}

/// [`mermaid_to_svg_reason_curve`] / [`mermaid_to_svg_flow`] combined — the one function both
/// delegate to.
pub fn mermaid_to_svg_reason_flow(
    code: &str,
    theme: &str,
    curve: &str,
    routing: &str,
) -> Result<String, String> {
    // The same byte string `mermaid_fence_url` hashes (§1), trimmed once here so both renderers
    // see identical input.
    let code = code.trim_end_matches('\n');
    if flowchart_is_ours(code) {
        render_konoma_flow(code, theme, curve, routing)
    } else if state_is_ours(code) {
        render_konoma_state(code, theme, routing)
    } else if class_is_ours(code) {
        render_konoma(code, theme, crate::render::class::render)
    } else if er_is_ours(code) {
        render_konoma(code, theme, crate::render::er::render)
    } else if sequence_is_ours(code) {
        render_konoma(code, theme, crate::render::sequence::render)
    } else if let Some(draw) = chart_renderer(code) {
        render_konoma(code, theme, draw)
    } else if let Some(draw) = stage5b_renderer(code) {
        render_konoma(code, theme, draw)
    } else {
        // Nothing left to ask. §6 makes this the *right* answer rather than a gap: the crate that
        // used to be here answered `venn-beta` with three boxes labelled "venn", "beta" and
        // "sets", which is a picture of something that does not exist.
        Err(format!(
            "not a mermaid diagram konoma can draw: the source starts with `{}`",
            crate::chart::first_word(code)
        ))
    }
}

/// Which of stage 5b's eleven renderers owns this source, if any.
///
/// One table rather than eleven `else if` arms, for the reason [`chart_renderer`] gives: the
/// answers are mutually exclusive by construction — each looks only at the keyword line and no two
/// keywords are the same — and a table makes that visible instead of leaving it to the order of a
/// chain. Panic-safe like its siblings: classification is the one step where guessing "not mine"
/// is the safe answer.
fn stage5b_renderer(code: &str) -> Option<Draw> {
    use crate as m;
    use crate::render as draw;
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let table: &[ChartArm] = &[
                (m::mindmap::is_mindmap, draw::mindmap::render),
                (m::kanban::is_kanban, draw::kanban::render),
                (m::journey::is_journey, draw::journey::render),
                (m::timeline::is_timeline, draw::timeline::render),
                (m::gantt::is_gantt, draw::gantt::render),
                (
                    m::requirement::is_requirement_diagram,
                    draw::requirement::render,
                ),
                (m::gitgraph::is_git_graph, draw::gitgraph::render),
                (m::c4::is_c4, draw::c4::render),
                (m::block::is_block_diagram, draw::block::render),
                (m::architecture::is_architecture, draw::architecture::render),
                (m::zenuml::is_zenuml, draw::zenuml::render),
            ];
            table.iter().find(|(is, _)| is(code)).map(|(_, d)| *d)
        }))
    });
    caught.unwrap_or(None)
}

/// The entry point of one of konoma's own renderers: source and theme in, SVG out.
type Draw = fn(&str, &str) -> Result<String, crate::render::RenderError>;

/// One row of the chart routing table: the predicate that claims a source, and the renderer that
/// then owns it.
type ChartArm = (fn(&str) -> bool, Draw);

/// Which of stage 5's seven chart renderers owns this source, if any.
///
/// One function rather than seven `*_is_ours` predicates: the seven answers are mutually exclusive
/// by construction (each looks at the keyword line and no two keywords are the same), so asking
/// them in one place makes that visible instead of leaving it to the order of an `else if` chain.
/// Panic-safe like its siblings, and for the same reason — classification is the one step where
/// guessing "not ours" is the safe answer, because it only ever means a diagram keeps being drawn
/// by the crate.
fn chart_renderer(code: &str) -> Option<Draw> {
    use crate::chart;
    use crate::render::chart as draw;
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let table: &[ChartArm] = &[
                (chart::pie::is_pie, draw::pie::render),
                (chart::xychart::is_xychart, draw::xychart::render),
                (chart::quadrant::is_quadrant_chart, draw::quadrant::render),
                (chart::radar::is_radar, draw::radar::render),
                (chart::treemap::is_treemap, draw::treemap::render),
                (chart::packet::is_packet, draw::packet::render),
                (chart::sankey::is_sankey, draw::sankey::render),
            ];
            table.iter().find(|(is, _)| is(code)).map(|(_, d)| *d)
        }))
    });
    caught.unwrap_or(None)
}

/// The routing predicate for a flowchart, made panic-safe.
///
/// Classifying reads only the header — [`preprocess`](crate::flowchart::preprocess)
/// plus the keyword scan — so a source of some other kind never reaches the flowchart grammar
/// proper. A panic in *that* much is answered with "not ours", because the cost of guessing wrong
/// in this direction is that a diagram konoma draws today stops being drawn, and §7 puts that
/// first. Guessing wrong in the other direction (a broken flowchart quietly redrawn by the old
/// crate) is the one this function exists to make impossible.
fn flowchart_is_ours(code: &str) -> bool {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::flowchart::is_flowchart(code)
        }))
    });
    caught.unwrap_or(false)
}

/// Whether this is a state diagram, made panic-safe the same way (stage 2).
fn state_is_ours(code: &str) -> bool {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::state::is_state_diagram(code)
        }))
    });
    caught.unwrap_or(false)
}

/// Whether this is a class diagram, made panic-safe the same way (stage 3).
fn class_is_ours(code: &str) -> bool {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::class::is_class_diagram(code)
        }))
    });
    caught.unwrap_or(false)
}

/// Whether this is an ER diagram, made panic-safe the same way (stage 3).
fn er_is_ours(code: &str) -> bool {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::er::is_er_diagram(code)
        }))
    });
    caught.unwrap_or(false)
}

/// Whether this is a sequence diagram, made panic-safe the same way (stage 4).
fn sequence_is_ours(code: &str) -> bool {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::sequence::is_sequence_diagram(code)
        }))
    });
    caught.unwrap_or(false)
}

/// konoma's own renderer. A failure is a failure: no second opinion from the crate.
///
/// `draw` is the entry point of whichever of konoma's renderers owns this diagram kind. Routing
/// picked it *before* either renderer ran, and that is the whole point: falling through to the
/// crate on failure would let the old renderer quietly redraw whatever the new one got wrong.
fn render_konoma(code: &str, theme: &str, draw: Draw) -> Result<String, String> {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw(code, theme)))
    });
    match caught {
        Ok(Ok(svg)) => Ok(svg),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("konoma's mermaid renderer panicked".to_string()),
    }
}

/// [`render_konoma`], for the flowchart branch alone: the one diagram kind whose own `render`
/// takes a `curve` and a `routing` (`render_flow`), so it cannot be a [`Draw`] and routed through
/// the table the other ten diagram kinds share.
fn render_konoma_flow(
    code: &str,
    theme: &str,
    curve: &str,
    routing: &str,
) -> Result<String, String> {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::render::render_flow(code, theme, curve, routing)
        }))
    });
    match caught {
        Ok(Ok(svg)) => Ok(svg),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("konoma's mermaid renderer panicked".to_string()),
    }
}

/// [`render_konoma`], for the state-diagram branch: `state::render`'s own `routing`-aware sibling
/// (`state::render_flow`, `docs/FEATURE-MERMAID-RENDERER.md` §10-5) takes a `routing` argument
/// `render`/the shared [`Draw`] table's ten other diagram kinds do not, the same reason
/// [`render_konoma_flow`] exists apart from the table for the flowchart.
fn render_konoma_state(code: &str, theme: &str, routing: &str) -> Result<String, String> {
    let caught = silence_panics(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::render::state::render_flow(code, theme, routing)
        }))
    });
    match caught {
        Ok(Ok(svg)) => Ok(svg),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("konoma's mermaid renderer panicked".to_string()),
    }
}

/// Pre-warm the renderer's lazy font work (the first call scans system fonts, tens of ms).
///
/// Called from a startup thread so the first diagram a reader opens never pays it. There is one
/// renderer now, and one thing to warm: [`crate::text_metrics`]'s shared font
/// database, which every diagram kind measures through. So a **single** diagram warms all
/// twenty-three, and the test beside this one is what says so — it used to check that two arms
/// were both reached, and that statement stopped being true when the second arm was removed
/// rather than quietly becoming vacuous.
///
/// The four sources are still four rather than one, because they exercise the four *shapes* of
/// pipeline — a layered graph, a sequence, a data chart and a banded chart — so a panic in any of
/// them shows up at startup on a background thread instead of the first time a reader opens one.
pub fn warm_mermaid() {
    let _ = mermaid_to_svg("graph LR\nA-->B", "dark");
    let _ = mermaid_to_svg("sequenceDiagram\n  A->>B: hi", "dark");
    let _ = mermaid_to_svg("pie\n  \"a\" : 1", "dark");
    let _ = mermaid_to_svg(
        "gantt\n  title G\n  section S\n  t :a1, 2024-01-01, 3d",
        "dark",
    );
}
