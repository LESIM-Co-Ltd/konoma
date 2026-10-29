use super::*;

pub(super) use crate::preview::markdown::is_link_span;
// The post-pass lives in `konoma-widgets` (shared with the standalone app); the tests of this
// module reach its pieces by these names.
#[cfg(test)]
pub(super) use crate::preview::markdown::{
    autolink_bare_urls, collapse_links, find_bare_links, replace_emoji_shortcodes, substitute_emoji,
};

/// Scan collapsed decorated lines for Tab-cycle items (links / task checkboxes / code-block
/// headers) in document order. `targets` are the link URLs recovered by `collapse_links`, in the
/// same order the link spans appear. `fence_ords` are the **source** ordinals of the rendered
/// mermaid placements in order (from `ImagePlacement::fence_ord`) — the k-th sentinel takes the
/// k-th value. Shared by the cache build and the test-only decorate path.
/// Slugify a heading's text the way GitHub does for `#anchor` links: lowercase, drop punctuation
/// except `-`/`_`, spaces → `-` (consecutive spaces → consecutive `-`, not collapsed). Unicode
/// letters/digits are kept (CJK headings anchor too). Used to match `[x](#slug)` link targets.
pub(super) fn github_slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if c == ' ' {
            s.push('-');
        } else if c == '-' || c == '_' {
            s.push(c);
        }
    }
    s
}

/// Build the in-page anchor map (slug → decorated logical-line index) from the rendered lines, in
/// document order, with GitHub's duplicate-slug disambiguation (`slug`, `slug-1`, `slug-2`, …).
#[cfg(test)]
pub(super) fn compute_md_anchors(lines: &[Line<'static>]) -> Vec<(String, usize)> {
    compute_md_anchors_with_math(lines, &[])
}

/// The in-text LaTeX sources on each decorated line, in left-to-right order — what a heading's
/// math reservation spans stand for (`heading_text_at`). Keyed by line index.
pub(super) fn math_sources_by_line(
    images: &[crate::preview::markdown::ImagePlacement],
) -> std::collections::HashMap<usize, Vec<&str>> {
    let mut by_line: std::collections::HashMap<usize, Vec<&str>> = std::collections::HashMap::new();
    for p in images {
        if crate::preview::markdown::is_math_url(&p.url) && p.rows == 1 {
            by_line.entry(p.line).or_default().push(p.alt.as_str());
        }
    }
    by_line
}

/// [`compute_md_anchors`] for a document whose headings may hold in-text math: the slug is made from
/// the heading's text *with the `$latex$` source put back* where an expression is drawn as a picture,
/// so it is the same slug whether or not the pictures have loaded (or image mode is on at all).
pub(super) fn compute_md_anchors_with_math(
    lines: &[Line<'static>],
    images: &[crate::preview::markdown::ImagePlacement],
) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut anchors = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let math = math_sources_by_line(images);
    for i in 0..lines.len() {
        let Some(text) = crate::preview::markdown::heading_text_at(lines, i, &math) else {
            continue;
        };
        let base = github_slug(&text);
        if base.is_empty() {
            continue;
        }
        let n = counts.entry(base.clone()).or_insert(0);
        let slug = if *n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        *n += 1;
        anchors.push((slug, i));
    }
    anchors
}

pub(super) fn build_md_items(
    lines: &[Line<'static>],
    targets: &[String],
    fence_ords: &[usize],
    code_blocks: &[String],
    task_marks: &[(char, usize)],
) -> Vec<MdItem> {
    let mut items = Vec::new();
    let mut k = 0usize;
    for (li, line) in lines.iter().enumerate() {
        for span in &line.spans {
            if is_link_span(span) {
                let target = targets.get(k).cloned().unwrap_or_default();
                items.push(MdItem {
                    line: li,
                    kind: MdItemKind::Link { target },
                });
                k += 1;
            } else if crate::preview::markdown::is_task_span(span) {
                if let Some(state) =
                    crate::preview::markdown::task_span_state(span.content.as_ref())
                {
                    // Ordinal among task-checkbox sentinels seen so far — the same "ordinal from
                    // count-so-far" pattern `fence_ords`'s own lookup below uses (see that arm's own
                    // comment): `task_marks` is the model render pass's own record, in document
                    // order, of the identical checkboxes this scan is finding by sentinel span, so
                    // the Nth sentinel and `task_marks[N]` are the same checkbox by construction (one
                    // render pass, not two independent derivations) — and never absent for a document
                    // the model renderer drew, which is every document
                    // (`golden_items_match_the_live_app_across_the_corpus` pins that over the whole
                    // corpus; a short record would leave `state_at` `None` and the toggle a no-op).
                    let seen = items
                        .iter()
                        .filter(|it| matches!(it.kind, MdItemKind::Task { .. }))
                        .count();
                    let state_at = task_marks.get(seen).map(|&(_, off)| off);
                    items.push(MdItem {
                        line: li,
                        kind: MdItemKind::Task { state, state_at },
                    });
                }
            } else if crate::preview::markdown::is_code_header_span(span) {
                // Same "ordinal from count-so-far, matched against the render pass's own record"
                // shape as the task arm above — see its own comment.
                let seen = items
                    .iter()
                    .filter(|it| matches!(it.kind, MdItemKind::CodeBlock { .. }))
                    .count();
                let body = code_blocks.get(seen).cloned();
                items.push(MdItem {
                    line: li,
                    kind: MdItemKind::CodeBlock { body },
                });
            } else if crate::preview::markdown::is_mermaid_header_span(span) {
                let seen = items
                    .iter()
                    .filter(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
                    .count();
                // The ordinal is the **source order** from the placement (a running count that
                // includes loading/failed fences). Substituting the count of drawn sentinels would
                // drift from Enter's source re-extraction whenever a fence that emits no sentinel
                // upstream exists (text-degraded / still loading), opening **the wrong diagram**.
                // Only the path with no fence_ords (test compatibility) falls back to the old count.
                let ordinal = fence_ords.get(seen).copied().unwrap_or(seen);
                items.push(MdItem {
                    line: li,
                    kind: MdItemKind::MermaidFence { ordinal },
                });
            } else if crate::preview::markdown::is_details_header_span(span) {
                let ordinal = items
                    .iter()
                    .filter(|it| matches!(it.kind, MdItemKind::Details { .. }))
                    .count();
                items.push(MdItem {
                    line: li,
                    kind: MdItemKind::Details { ordinal },
                });
            }
        }
    }
    items
}

/// Derive the Tab-cycle item list from **one render pass's own outputs** — the shape both
/// production (`App::ensure_md_cache`) and the golden-snapshot harness
/// (`md_snapshot_tests::render_case`) build their items through.
///
/// Why this wrapper exists rather than each caller assembling [`build_md_items`]'s five arguments
/// itself: the two that carry *source* data — `code_blocks` (the text `y c` copies) and `task_marks`
/// (the byte the checkbox toggle writes) — were exactly the two the golden snapshot used to pass as
/// **empty slices** while production passed the render pass's real record, so every dumped
/// `CodeBlock::body`/`Task::state_at` read `None` and no golden could ever have failed on a `y c`
/// that copied the wrong block or a `Space` that flipped the wrong line. Taking
/// [`MdRenderExtras`](crate::preview::markdown::MdRenderExtras) **by reference, as the struct the
/// renderer itself returned**, means neither caller can substitute a differently-derived list
/// without hand-building a fake record; `md_snapshot_tests::golden_sections_are_the_production_records`
/// pins that nobody did.
pub(super) fn build_md_items_from_render(
    lines: &[Line<'static>],
    targets: &[String],
    images: &[crate::preview::markdown::ImagePlacement],
    extras: &crate::preview::markdown::MdRenderExtras,
) -> Vec<MdItem> {
    // The drawn mermaid placements' **source** ordinal (document order), for sentinel matching —
    // see `build_md_items`'s own mermaid arm for why the source ordinal and not a drawn count.
    let fence_ords: Vec<usize> = images.iter().filter_map(|p| p.fence_ord).collect();
    build_md_items(
        lines,
        targets,
        &fence_ords,
        &extras.code_blocks,
        &extras.tasks,
    )
}

/// Apply the focus inversion to one decorated line. `ordinal` = index of the focused marker span
/// within this line (counting link/task/code-header spans in order); `whole_line`=code-block
/// header (all spans inverted — one line, a clear focus cue). Other lines pass through untouched.
pub(super) fn invert_focused_line(
    line: &Line<'static>,
    ordinal: usize,
    whole_line: bool,
) -> Line<'static> {
    use ratatui::style::Modifier;
    let style = line.style;
    let mut seen = 0usize;
    let spans = line
        .spans
        .iter()
        .cloned()
        .map(|mut span| {
            if whole_line {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            } else if is_link_span(&span)
                || crate::preview::markdown::is_task_span(&span)
                || crate::preview::markdown::is_code_header_span(&span)
                || crate::preview::markdown::is_mermaid_header_span(&span)
                || crate::preview::markdown::is_details_header_span(&span)
            {
                if seen == ordinal {
                    span.style = span.style.add_modifier(Modifier::REVERSED);
                }
                seen += 1;
            }
            span
        })
        .collect::<Vec<_>>();
    Line::from(spans).style(style)
}
