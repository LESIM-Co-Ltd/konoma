//! A table takes part in the ordinary block-gap protocol: it is separated from the block before it
//! and the block after it exactly the way a paragraph is. Until 2026-10 it did not (no leading blank
//! row, `pending_block_gap = false` on exit), so a table's top border sat on the previous block's
//! last row and the next block sat on its bottom border.
//!
//! The spec is stated **differentially** — "a table is separated from X exactly as a paragraph is" —
//! plus a hard "exactly one blank row" for every neighbour kind where a paragraph itself gets one, so
//! the differential cannot pass vacuously (two things that are both glued also agree). The second
//! half pins that every row-keyed feature (Tab focus, outline/anchors, code-block and mermaid
//! placements) still points at the right row once the extra row exists.

use super::md_snapshot_tests::render_case;
use super::*;

/// One rendered row's visible text.
fn text(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn rows(src: &str) -> Vec<String> {
    render_case(&Config::default(), src)
        .lines
        .iter()
        .map(text)
        .collect()
}

/// A row that is empty once the container prefixes a quote / alert / `<details>` draws are removed.
fn is_blank(row: &str) -> bool {
    row.trim_matches(|c: char| c.is_whitespace() || matches!(c, '>' | '▌' | '▏'))
        .is_empty()
}

fn is_top(row: &str) -> bool {
    row.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '>' | '▌' | '▏'))
        .starts_with('┌')
}

fn is_bottom(row: &str) -> bool {
    row.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '>' | '▌' | '▏'))
        .starts_with('└')
}

/// Blank rows directly above `rows[i]`.
fn blanks_above(rows: &[String], i: usize) -> usize {
    rows[..i].iter().rev().take_while(|r| is_blank(r)).count()
}

/// Blank rows directly below `rows[i]`.
fn blanks_below(rows: &[String], i: usize) -> usize {
    rows[i + 1..].iter().take_while(|r| is_blank(r)).count()
}

const GFM: &str = "| a | b |\n|---|---|\n| 1 | 2 |\n";
const HTML: &str = "<table><tr><td>a</td></tr></table>\n";
const PARA: &str = "PPP\n";

/// Neighbours a table can sit next to, `(name, source, separated)`. `separated` = a paragraph is
/// itself separated from it by exactly one blank row (checked below, so the flag cannot be wrong
/// silently). `math`/`img`/`alert`/`details` are the blocks whose own exit state is "glued" — a
/// paragraph is glued to them too, and the differential holds the table to the same rule.
const NEIGHBOURS: &[(&str, &str, bool)] = &[
    ("paragraph", "QQQ\n", true),
    ("h1", "# QQQ\n", true),
    ("h3", "### QQQ\n", true),
    ("bullet list", "- QQQ\n", true),
    ("ordered list", "1. QQQ\n", true),
    ("code fence", "```\nQQQ\n```\n", true),
    ("block quote", "> QQQ\n", true),
    ("rule", "---\n", true),
    ("gfm table", "| QQQ |\n|---|\n| r |\n", true),
    ("html table", "<table><tr><td>QQQ</td></tr></table>\n", true),
    // A mermaid band reserves its own blank rows (the picture goes there), so a paragraph is "not
    // one blank row" from it in either direction; the differential still holds the table to it.
    ("mermaid", "```mermaid\ngraph TD; QQQ-->B\n```\n", false),
    ("math", "$$QQQ$$\n", false),
    ("alert", "> [!NOTE]\n> QQQ\n", false),
    (
        "details",
        "<details open><summary>s</summary>\n\nQQQ\n\n</details>\n",
        false,
    ),
];

#[test]
fn a_table_is_separated_from_the_block_before_it_like_a_paragraph_is() {
    for (name, before, separated) in NEIGHBOURS {
        // The paragraph's own gap after `before`, the reference.
        let r = rows(&format!("{before}\n{PARA}"));
        let p = r.iter().position(|x| x.contains("PPP")).unwrap();
        let para_gap = blanks_above(&r, p);
        assert_eq!(
            para_gap == 1,
            *separated,
            "前提(段落の空け方): {name} -> {r:?}"
        );
        for (kind, table) in [("gfm", GFM), ("html", HTML)] {
            let r = rows(&format!("{before}\n{table}"));
            let t = r.iter().rposition(|x| is_top(x)).expect("table drawn");
            assert_eq!(
                blanks_above(&r, t),
                para_gap,
                "{name} の後の {kind} 表の上の空き(段落と同じはず): {r:?}"
            );
        }
    }
}

#[test]
fn a_table_is_separated_from_the_block_after_it_like_a_paragraph_is() {
    for (name, after, separated) in NEIGHBOURS {
        let r = rows(&format!("{PARA}\n{after}"));
        let p = r.iter().position(|x| x.contains("PPP")).unwrap();
        let para_gap = blanks_below(&r, p);
        assert_eq!(
            para_gap == 1,
            *separated,
            "前提(段落の空け方): {name} -> {r:?}"
        );
        for (kind, table) in [("gfm", GFM), ("html", HTML)] {
            let r = rows(&format!("{table}\n{after}"));
            let b = r.iter().position(|x| is_bottom(x)).expect("table drawn");
            assert_eq!(
                blanks_below(&r, b),
                para_gap,
                "{kind} 表の下の空き、後ろが {name}(段落と同じはず): {r:?}"
            );
        }
    }
}

/// The source has no blank line between a table and a fence (or the fence precedes the table): the
/// fence still gets its own separator row, and so does the table.
#[test]
fn a_fence_and_a_table_with_no_blank_line_in_the_source_are_still_separated() {
    let r = rows(&format!("{GFM}```\nQQQ\n```\n"));
    let b = r.iter().position(|x| is_bottom(x)).unwrap();
    assert_eq!(blanks_below(&r, b), 1, "{r:?}");
    let r = rows(&format!("```\nQQQ\n```\n{GFM}"));
    let t = r.iter().position(|x| is_top(x)).unwrap();
    assert_eq!(blanks_above(&r, t), 1, "{r:?}");
}

/// Every kind of block the table-gap rule has to coexist with, in one document, both directions at
/// once: between any two adjacent blocks there is exactly one blank row, tables included.
#[test]
fn a_document_of_mixed_blocks_has_one_blank_row_between_each_pair() {
    let src = format!(
        "# T\n\nintro\n\n{GFM}\nmid\n\n- l1\n- l2\n\n{HTML}\n```\nc\n```\n\n{GFM}\n{HTML}\n> q\n\n{GFM}\n---\n\n{GFM}\n## S\n\n{GFM}\nend\n"
    );
    let r = rows(&src);
    let tops: Vec<usize> = (0..r.len()).filter(|&i| is_top(&r[i])).collect();
    let bottoms: Vec<usize> = (0..r.len()).filter(|&i| is_bottom(&r[i])).collect();
    assert_eq!(tops.len(), 7, "{r:?}");
    for t in tops {
        assert_eq!(blanks_above(&r, t), 1, "表の上: {r:?}");
        assert!(t < 2 || !is_blank(&r[t - 2]), "空行は 1 行だけ: {r:?}");
    }
    for b in bottoms {
        assert_eq!(blanks_below(&r, b), 1, "表の下: {r:?}");
    }
}

#[test]
fn a_table_at_the_start_or_end_of_a_document_adds_no_blank_row() {
    for table in [GFM, HTML] {
        let r = rows(table);
        assert!(is_top(&r[0]), "先頭に空行が無い: {r:?}");
        assert!(is_bottom(r.last().unwrap()), "末尾に空行が無い: {r:?}");
        // …and a trailing / leading blank line in the source changes nothing.
        assert_eq!(rows(&format!("\n\n{table}\n\n")), r);
    }
}

#[test]
fn two_consecutive_tables_have_exactly_one_blank_row_between_them() {
    for (a, b) in [(GFM, GFM), (GFM, HTML), (HTML, GFM), (HTML, HTML)] {
        let r = rows(&format!("{a}\n{b}"));
        let tops: Vec<usize> = (0..r.len()).filter(|&i| is_top(&r[i])).collect();
        assert_eq!(tops.len(), 2, "{r:?}");
        assert_eq!(blanks_above(&r, tops[1]), 1, "{r:?}");
        assert!(is_bottom(r.last().unwrap()), "{r:?}");
    }
}

/// A table inside a container is separated from its siblings in that container, and the container
/// does not grow a stray blank row at its end.
#[test]
fn a_table_inside_a_container_is_separated_from_its_siblings() {
    let table = "| a | b |\n|---|---|\n| 1 | 2 |";
    let prefixed = |p0: &str, p: &str| {
        let body = format!("PPP\n\n{table}\n\nQQQ");
        let mut out = String::new();
        for (i, l) in body.lines().enumerate() {
            out.push_str(if i == 0 { p0 } else { p });
            out.push_str(l);
            out.push('\n');
        }
        out
    };
    let html_body = "PPP\n\n<table><tr><td>a</td></tr></table>\n\nQQQ";
    let cases: Vec<(&str, String)> = vec![
        ("quote", prefixed("> ", "> ")),
        ("loose list item", {
            let mut s = String::from("- PPP\n\n  ");
            s.push_str(&table.replace('\n', "\n  "));
            s.push_str("\n\n  QQQ\n");
            s
        }),
        ("alert", format!("> [!NOTE]\n{}", prefixed("> ", "> "))),
        (
            "details",
            format!("<details open><summary>s</summary>\n\nPPP\n\n{table}\n\nQQQ\n\n</details>\n"),
        ),
        ("quote (html table)", {
            let mut s = String::new();
            for l in html_body.lines() {
                s.push_str("> ");
                s.push_str(l);
                s.push('\n');
            }
            s
        }),
    ];
    for (name, src) in cases {
        let r = rows(&src);
        let t = r
            .iter()
            .position(|x| is_top(x))
            .unwrap_or_else(|| panic!("{name}: {r:?}"));
        let b = r.iter().position(|x| is_bottom(x)).unwrap();
        assert_eq!(blanks_above(&r, t), 1, "{name} の表の上: {r:?}");
        assert_eq!(blanks_below(&r, b), 1, "{name} の表の下: {r:?}");
        let p = r.iter().position(|x| x.contains("PPP")).unwrap();
        let q = r.iter().position(|x| x.contains("QQQ")).unwrap();
        assert!(p < t && b < q, "{name}: {r:?}");
        assert!(
            !is_blank(r.last().unwrap()),
            "{name}: 末尾に余計な行: {r:?}"
        );
    }
    // A table that is the last thing in a container leaves no blank row behind it either.
    for src in [
        "> PPP\n>\n> | a |\n> |---|\n> | 1 |\n".to_string(),
        "> [!NOTE]\n> | a |\n> |---|\n> | 1 |\n".to_string(),
        "<details open><summary>s</summary>\n\n| a |\n|---|\n| 1 |\n\n</details>\n".to_string(),
    ] {
        let r = rows(&src);
        assert!(!is_blank(r.last().unwrap()), "{src:?}: {r:?}");
    }
}

/// `[ui] md_table_align`: the blank row above/below is a plain empty row; only the box is indented.
#[test]
fn an_aligned_table_keeps_its_gap_rows_empty() {
    for align in ["center", "right"] {
        let mut cfg = Config::default();
        cfg.ui.md_table_align = align.into();
        let r: Vec<String> = render_case(&cfg, &format!("PPP\n\n{GFM}\nQQQ\n"))
            .lines
            .iter()
            .map(text)
            .collect();
        let t = r.iter().position(|x| is_top(x)).unwrap();
        let b = r.iter().position(|x| is_bottom(x)).unwrap();
        assert!(r[t].starts_with(' '), "{align}: 箱が寄っている: {r:?}");
        assert_eq!(r[t - 1], "", "{align}: {r:?}");
        assert_eq!(r[b + 1], "", "{align}: {r:?}");
        assert_eq!(blanks_above(&r, t), 1, "{r:?}");
        assert_eq!(blanks_below(&r, b), 1, "{r:?}");
    }
}

// ---- row-keyed features follow the extra row ----

/// Document with something row-keyed on each side of two tables.
const FEATURES: &str = "\
[first](https://x.test/1)

| h |
|---|
| c |

## Anchor Heading

```rust
let code_after_table = 1;
```

- [ ] task after table

| h2 |
|---|
| c2 |

[last](https://x.test/2)
";

fn row_of(r: &crate::app::md_snapshot_tests::CaseRender, i: usize) -> String {
    text(&r.lines[i])
}

/// Tab focus: every item (links, the checkbox, the code block) points at the row that shows it.
#[test]
fn tab_focus_items_point_at_their_own_rows_around_tables() {
    let r = render_case(&Config::default(), FEATURES);
    // The premise: the extra gap rows exist, so the rows below really did move.
    let all: Vec<String> = r.lines.iter().map(text).collect();
    for t in (0..all.len()).filter(|&i| is_top(&all[i])) {
        assert_eq!(blanks_above(&all, t), 1, "{all:?}");
    }
    let by_kind = |pred: &dyn Fn(&MdItemKind) -> bool| -> Vec<usize> {
        r.items
            .iter()
            .filter(|it| pred(&it.kind))
            .map(|it| it.line)
            .collect()
    };
    let links = by_kind(&|k| matches!(k, MdItemKind::Link { .. }));
    assert_eq!(
        links.len(),
        2,
        "{:?}",
        r.lines.iter().map(text).collect::<Vec<_>>()
    );
    assert!(
        row_of(&r, links[0]).contains("first"),
        "{}",
        row_of(&r, links[0])
    );
    assert!(
        row_of(&r, links[1]).contains("last"),
        "{}",
        row_of(&r, links[1])
    );
    let tasks = by_kind(&|k| matches!(k, MdItemKind::Task { .. }));
    assert_eq!(tasks.len(), 1);
    assert!(row_of(&r, tasks[0]).contains("task after table"));
    let code = by_kind(&|k| matches!(k, MdItemKind::CodeBlock { .. }));
    assert_eq!(code.len(), 1);
    assert!(
        row_of(&r, code[0]).contains("rust"),
        "{}",
        row_of(&r, code[0])
    );
}

/// Outline `o` / `#anchor` jumps (both read `compute_md_anchors`' row).
#[test]
fn heading_anchors_point_at_the_heading_row_after_a_table() {
    let r = render_case(&Config::default(), FEATURES);
    let (slug, line) = r.anchors.first().expect("an anchor").clone();
    assert_eq!(slug, "anchor-heading");
    assert!(
        row_of(&r, line).contains("Anchor Heading"),
        "{}",
        row_of(&r, line)
    );
}

/// Block images (a mermaid band) after a table: the placement starts on the row right under the
/// band's caption, wherever the table's extra gap row pushed it. (A mermaid fence is glued to what
/// precedes it, a paragraph included — that is the band's own, unchanged rule, not the table's.)
#[test]
fn a_placement_after_a_table_starts_under_its_own_caption() {
    let src = format!(
        "{GFM}\n```mermaid\ngraph TD; A-->B\n```\n\n{GFM}\n```mermaid\ngraph TD; C-->D\n```\n"
    );
    let r = render_case(&Config::default(), &src);
    assert_eq!(r.images.len(), 2);
    let all: Vec<String> = r.lines.iter().map(text).collect();
    for p in &r.images {
        assert!(
            all[p.line - 1].contains("mermaid"),
            "配置 {p:?} の直前: {all:?}"
        );
        assert!(is_bottom(&all[p.line - 2]), "配置 {p:?} の直前: {all:?}");
    }
    // The second table sits one blank row under the first band, like any other block would.
    let second = all.iter().rposition(|x| is_top(x)).unwrap();
    assert_eq!(
        blanks_above(&all, second),
        r.images[0].rows as usize + 1,
        "{all:?}"
    );
}
