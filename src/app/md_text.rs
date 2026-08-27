use super::*;

/// Whether a span is a Markdown link (blue underline). Since konoma's StyleSheet.link()=underline+blue draws them,
/// these two conditions decide it (code/headings/rules have different colors, so no false positives).
pub(super) fn is_link_span(span: &Span<'static>) -> bool {
    use ratatui::style::{Color, Modifier};
    span.style.add_modifier.contains(Modifier::UNDERLINED) && span.style.fg == Some(Color::Blue)
}

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
pub(super) fn compute_md_anchors(lines: &[Line<'static>]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    let mut anchors = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(text) = crate::preview::markdown::heading_text(line) else {
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

/// Fold the accompanying URL of the "label (URL)" that konoma's own renderer emits, leaving **only the label** in link style (blue underline,
/// with a leading link icon when `icons=true`). The URLs are collected in order into `targets` and returned (hidden destinations).
/// Pattern: `[label]` `" ("` `[URL(blue underline)]` `")"`. Spans that do not match are passed through unchanged.
pub(super) fn collapse_links(
    lines: Vec<Line<'static>>,
    icons: bool,
) -> (Vec<Line<'static>>, Vec<String>) {
    use ratatui::style::{Color, Modifier, Style};
    let link_style = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    let mut out = Vec::with_capacity(lines.len());
    let mut targets = Vec::new();
    for line in lines {
        let style = line.style;
        let spans = line.spans;
        let n = spans.len();
        let mut new: Vec<Span<'static>> = Vec::with_capacity(n);
        let mut i = 0;
        while i < n {
            // A table-cell-originated "label + hidden target" pair (generated by
            // markdown::render_table): leave the label as-is (doesn't change its width = no icon
            // either, to preserve the table's column alignment), remove the hidden span, and
            // collect the URL into `targets` (Tab/Enter work the same as a normal link).
            if i + 1 < n
                && is_link_span(&spans[i])
                && crate::preview::markdown::is_hidden_link_target(&spans[i + 1])
            {
                targets.push(spans[i + 1].content.to_string());
                new.push(spans[i].clone());
                i += 2;
                continue;
            }
            let is_link_pattern = i + 2 < n
                && spans[i + 1].content.as_ref() == " ("
                && is_link_span(&spans[i + 2])
                && spans.get(i + 3).is_some_and(|s| s.content.starts_with(')'));
            if is_link_pattern {
                let label = spans[i].content.as_ref();
                targets.push(spans[i + 2].content.to_string());
                let text = if icons {
                    format!("{} {label}", crate::ui::icons::link_icon())
                } else {
                    label.to_string()
                };
                new.push(Span::styled(text, link_style));
                // Keep any characters (punctuation, etc.) that follow the closing paren ")".
                let closer = spans[i + 3].content.as_ref();
                if closer.len() > 1 {
                    new.push(Span::styled(closer[1..].to_string(), spans[i + 3].style));
                }
                i += 4;
            } else {
                new.push(spans[i].clone());
                i += 1;
            }
        }
        out.push(Line::from(new).style(style));
    }
    (out, targets)
}

/// Trim trailing punctuation from a matched URL run, GFM-style: drop a trailing run of
/// `? ! . , : * _ ~ ' "` and any `)` that has no matching `(` inside the run (iteratively).
fn trim_url_end(s: &str) -> &str {
    let mut end = s.len();
    loop {
        let sub = &s[..end];
        let Some(last) = sub.chars().last() else {
            break;
        };
        if "?!.,:*_~'\"".contains(last) {
            end -= last.len_utf8();
            continue;
        }
        if last == ')' && sub.matches(')').count() > sub.matches('(').count() {
            end -= 1;
            continue;
        }
        break;
    }
    &s[..end]
}

/// A character that ends a bare-URL run: whitespace, `<`, or CJK/fullwidth punctuation. The last set
/// matters for konoma's CJK audience — Japanese text often abuts a URL with no ASCII space
/// (`https://x.example、と`), and GFM's "stop only at whitespace/`<`" would swallow the trailing
/// characters into the link. Stopping at these keeps the URL clean.
fn is_url_stop(c: char) -> bool {
    c.is_whitespace()
        || c == '<'
        || matches!(
            c,
            '、' | '。'
                | '，'
                | '．'
                | '！'
                | '？'
                | '；'
                | '：'
                | '（'
                | '）'
                | '「'
                | '」'
                | '『'
                | '』'
                | '【'
                | '】'
                | '〈'
                | '〉'
                | '《'
                | '》'
                | '…'
                | '・'
                | '〜'
                | '”'
                | '“'
        )
}

/// Match a bare URL autolink at the start of `rest` (`http://…`, `https://…`, or `www.…`). Returns
/// the matched byte length and the target to open (`www.` gains an `http://` prefix). The run stops
/// at whitespace / `<` / CJK punctuation (`is_url_stop`), then trailing punctuation is trimmed. None
/// if `rest` does not start a URL.
fn match_bare_url(rest: &str) -> Option<(usize, String)> {
    let is_www = rest.starts_with("www.");
    if !(is_www || rest.starts_with("http://") || rest.starts_with("https://")) {
        return None;
    }
    let run_end = rest.find(is_url_stop).unwrap_or(rest.len());
    let run = trim_url_end(&rest[..run_end]);
    // Need a host after the scheme (or after `www.` a further alphanumeric char).
    let valid = if is_www {
        run.len() > 4 && run[4..].contains(|c: char| c.is_ascii_alphanumeric())
    } else {
        run.find("://").is_some_and(|p| run.len() > p + 3)
    };
    if !valid {
        return None;
    }
    let target = if is_www {
        format!("http://{run}")
    } else {
        run.to_string()
    };
    Some((run.len(), target))
}

fn is_email_local(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}
fn is_email_domain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

/// Match a bare email autolink at the start of `rest` (`local@domain.tld`). Returns the matched byte
/// length and a `mailto:` target. Requires a non-empty local part, an `@`, and a domain with a dot
/// whose last label is ≥2 ASCII letters. Trailing `.`/`-` are trimmed from the domain.
fn match_bare_email(rest: &str) -> Option<(usize, String)> {
    let local_len: usize = rest
        .chars()
        .take_while(|&c| is_email_local(c))
        .map(char::len_utf8)
        .sum();
    if local_len == 0 || !rest[local_len..].starts_with('@') {
        return None;
    }
    let after_at = &rest[local_len + 1..];
    let mut dom_len: usize = after_at
        .chars()
        .take_while(|&c| is_email_domain(c))
        .map(char::len_utf8)
        .sum();
    // Trim trailing '.'/'-' from the domain.
    while dom_len > 0 && matches!(after_at.as_bytes()[dom_len - 1], b'.' | b'-') {
        dom_len -= 1;
    }
    let domain = &after_at[..dom_len];
    let tld_ok = domain
        .rsplit_once('.')
        .is_some_and(|(_, tld)| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()));
    if !tld_ok {
        return None;
    }
    let end = local_len + 1 + dom_len;
    Some((end, format!("mailto:{}", &rest[..end])))
}

/// Find bare autolinkable spans (GFM autolink literals) in `text`: `http(s)://…`, `www.…`, and
/// `local@domain.tld` emails. Returns `(start_byte, end_byte, target)` in document order,
/// non-overlapping. A pragmatic subset of GFM §6.9: a match may start only at the beginning or after
/// a non-alphanumeric byte, the run stops at whitespace/`<`, and trailing punctuation (plus
/// unbalanced `)`) is trimmed. `www.` gets an `http://` prefix; emails a `mailto:` prefix.
pub(super) fn find_bare_links(text: &str) -> Vec<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0usize;
    while i < n {
        let boundary_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if boundary_ok {
            let rest = &text[i..];
            if let Some((len, target)) = match_bare_url(rest).or_else(|| match_bare_email(rest)) {
                out.push((i, i + len, target));
                i += len.max(1);
                continue;
            }
        }
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// Post-pass over collapsed lines: turn bare URLs/emails in plain-text spans into link spans, and
/// interleave their targets into `targets` in document order (so `build_md_items` pairs each link
/// span with the right URL). Skips spans that already carry a link, a background (inline code / code
/// fence content), or a konoma sentinel (task / code-header / mermaid / hidden-target) — matching
/// GitHub, which never auto-links inside code.
pub(super) fn autolink_bare_urls(
    lines: Vec<Line<'static>>,
    in_targets: Vec<String>,
) -> (Vec<Line<'static>>, Vec<String>) {
    use ratatui::style::{Color, Modifier, Style};
    let link_style = Style::new()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);
    let mut out_lines = Vec::with_capacity(lines.len());
    let mut out_targets = Vec::new();
    let mut ti = 0usize;
    for line in lines {
        // Code-block lines never auto-link (a URL in a fence stays literal). Keep as-is; for any
        // existing link span (none in practice) carry its target through 1:1 so the k-th link span
        // in `out_lines` still maps to `out_targets[k]` — no drift if a code line ever holds a link.
        if crate::preview::markdown::is_code_line(&line) {
            for span in &line.spans {
                if is_link_span(span) {
                    out_targets.push(in_targets.get(ti).cloned().unwrap_or_default());
                    ti += 1;
                }
            }
            out_lines.push(line);
            continue;
        }
        let style = line.style;
        let mut new: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
        for span in line.spans {
            // Existing link span → keep as-is; it owns the next input target (preserve order).
            if is_link_span(&span) {
                out_targets.push(in_targets.get(ti).cloned().unwrap_or_default());
                ti += 1;
                new.push(span);
                continue;
            }
            // Never auto-link inside code (inline code / bg-filled code) or over a konoma sentinel.
            if span.style.bg.is_some()
                || crate::preview::markdown::is_inline_code_span(&span)
                || crate::preview::markdown::is_task_span(&span)
                || crate::preview::markdown::is_code_header_span(&span)
                || crate::preview::markdown::is_mermaid_header_span(&span)
                || crate::preview::markdown::is_hidden_link_target(&span)
                || crate::preview::markdown::is_inline_math_reservation_span(&span)
            {
                new.push(span);
                continue;
            }
            let text = span.content.into_owned();
            let matches = find_bare_links(&text);
            if matches.is_empty() {
                new.push(Span::styled(text, span.style));
                continue;
            }
            let mut pos = 0usize;
            for (s, e, url) in matches {
                if s > pos {
                    new.push(Span::styled(text[pos..s].to_string(), span.style));
                }
                new.push(Span::styled(text[s..e].to_string(), link_style));
                out_targets.push(url);
                pos = e;
            }
            if pos < text.len() {
                new.push(Span::styled(text[pos..].to_string(), span.style));
            }
        }
        out_lines.push(Line::from(new).style(style));
    }
    (out_lines, out_targets)
}

/// Replace `:shortcode:` runs in `text` with Unicode emoji (gemoji names, e.g. `:rocket:` → 🚀).
/// Returns `None` when nothing changed (so the caller reuses the original span untouched). A
/// shortcode with no Unicode mapping (GitHub-custom like `:shipit:`) and non-shortcode `:` (times,
/// ratios) are left exactly as they were.
pub(super) fn replace_emoji_shortcodes(text: &str) -> Option<String> {
    if !text.contains(':') {
        return None;
    }
    let mut out = String::new();
    let mut changed = false;
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find(':') {
            let code = &after[..end];
            let looks_like_code = !code.is_empty()
                && code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'));
            if looks_like_code {
                if let Some(e) = emojis::get_by_shortcode(code) {
                    out.push_str(e.as_str());
                    changed = true;
                    rest = &after[end + 1..];
                    continue;
                }
            }
            // Not a known shortcode: keep this `:` and resume scanning right after it.
            out.push(':');
            rest = after;
        } else {
            out.push(':');
            out.push_str(after);
            rest = "";
        }
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// Post-pass over decorated Markdown lines: convert `:shortcode:` emoji to Unicode in plain-text
/// spans. Skips spans that carry a background (inline code / code fence), a link, or a konoma
/// sentinel (task / code-header / mermaid / hidden-target) — matching GitHub, which never converts
/// shortcodes inside code, and keeping Tab-item spans byte-stable.
pub(super) fn substitute_emoji(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            // Code-block lines keep their `:shortcode:` literal (a fence is verbatim).
            if crate::preview::markdown::is_code_line(&line) {
                return line;
            }
            let style = line.style;
            let spans = line
                .spans
                .into_iter()
                .map(|span| {
                    if span.style.bg.is_some()
                        || is_link_span(&span)
                        || crate::preview::markdown::is_inline_code_span(&span)
                        || crate::preview::markdown::is_task_span(&span)
                        || crate::preview::markdown::is_code_header_span(&span)
                        || crate::preview::markdown::is_mermaid_header_span(&span)
                        || crate::preview::markdown::is_hidden_link_target(&span)
                        || crate::preview::markdown::is_inline_math_reservation_span(&span)
                    {
                        return span;
                    }
                    match replace_emoji_shortcodes(span.content.as_ref()) {
                        Some(new) => Span::styled(new, span.style),
                        None => span,
                    }
                })
                .collect::<Vec<_>>();
            Line::from(spans).style(style)
        })
        .collect()
}
