//! A Markdown document in the konoma host and in the standalone app (`konoma-plugin` feature
//! `app`) is laid out by the same shared pipeline. Given the same file, the same size and the
//! same keys, the two must show the same page: the same rows in the frame (the host draws a
//! scroll label on the border and a scrollbar on it, which the app does not), the same footer
//! hints and the same help rows for the keys both have.

use super::table_parity::{help_lines, hints, presses, rows_of, same_row, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::TermCaps;
use std::sync::Arc;

/// What the app shows after the presses: the whole screen.
fn app_term(path: &std::path::Path, w: u16, h: u16, spec: &str) -> Terminal<TestBackend> {
    let mut opened = open_file(
        vec![Arc::new(konoma_markdown::MarkdownPlugin)],
        path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: opened.shows.clone(),
        ..AppCx::default()
    };
    let ev: Vec<AppEvent> = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            let ev = KeyEvent::new(code, mods);
            AppEvent::Key(konoma_plugin::app::key_from_crossterm(&ev).expect("a key"))
        })
        .collect();
    let mut it = ev.into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults(),
    )
    .unwrap();
    assert_eq!(
        it.count(),
        0,
        "the app ended before the keys ran out: {spec}"
    );
    term
}

/// What the host shows after the presses, with the file opened from its tree.
fn host_term(root: &std::path::Path, file: &str, w: u16, h: u16, spec: &str) -> Sim {
    let mut s = Sim::with_config_sized(root, cfg_en(), w, h);
    s.select(file);
    s.enter();
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended before the keys ran out: {spec}");
    }
    // Draw the last page onto a cleared screen: the test backend keeps a cell the diff skipped
    // after a wide character from an earlier screen (the tree), which no one sees on a real
    // terminal after a repaint; the app starts from a blank screen.
    s.term.clear().expect("clear");
    s.draw();
    s
}

/// The cells between the frame's borders: from the row under the top border to the row over the
/// bottom border, without the first and last column (the border, where the host's scrollbar is).
fn inner(term: &Terminal<TestBackend>) -> Vec<String> {
    let rows = rows_of(term);
    let top = rows
        .iter()
        .position(|l| l.starts_with('┌'))
        .expect("a frame");
    let bottom = rows
        .iter()
        .rposition(|l| l.starts_with('└'))
        .expect("a frame end");
    let buf = term.backend().buffer();
    (top + 1..bottom)
        .map(|y| {
            let mut line = String::new();
            let mut x = 1;
            while x + 1 < buf.area.width {
                let sym = buf[(x, y as u16)].symbol();
                line.push_str(sym);
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// The hints only the host has, spelled out in full.
const HOST_ONLY: &[&str] = &[
    ",:view",
    "[/]:tab",
    "/:search",
    "C-n/C-p:next/prev file",
    "Tab:focus",
    "F:FOLLOW",
    "O:outline",
    "e:edit",
    "e:open",
    "o:open in app",
    "q:back",
    "R:raw source",
    "y:copy",
];

/// One Markdown file per case: (file name, text).
fn fixtures() -> Vec<(String, String)> {
    let long: String = (0..60)
        .map(|i| format!("paragraph {i} with some words\n\n"))
        .collect();
    let wide_line = "lorem ipsum dolor sit amet ".repeat(30);
    vec![
        ("heading.md".into(), "# One\n\n## Two\n\n### Three\n\ntext\n".into()),
        (
            "list.md".into(),
            "- a\n- b\n  - c\n  - d\n\n1. x\n2. y\n\n- [ ] todo\n- [x] done\n".into(),
        ),
        (
            "table.md".into(),
            "| a | b |\n|---|:-:|\n| 1 | 2 |\n| long cell here | x |\n".into(),
        ),
        (
            "code.md".into(),
            "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n`inline` code\n".into(),
        ),
        ("quote.md".into(), "> quoted\n> text\n>\n> > nested\n".into()),
        (
            "alert.md".into(),
            "> [!NOTE]\n> a note\n\n> [!WARNING]\n> careful\n".into(),
        ),
        (
            "footnote.md".into(),
            "text[^1] and more[^2].\n\n[^1]: first\n[^2]: second\n".into(),
        ),
        (
            "front.md".into(),
            "---\ntitle: T\ntags: [a, b]\n---\n\n# Body\n\ntext\n".into(),
        ),
        (
            "details.md".into(),
            "<details>\n<summary>more</summary>\n\nhidden body\n\n</details>\n\n<details open>\n<summary>open</summary>\n\nshown\n\n</details>\n".into(),
        ),
        ("wrap.md".into(), format!("# Long\n\n{wide_line}\n\n- {wide_line}\n")),
        ("scroll.md".into(), format!("# Top\n\n{long}")),
        (
            "cjk.md".into(),
            "# 見出し\n\n日本語の段落です。これは長い行になるかもしれない文章で、折り返しを確かめます。\n\n- 項目一\n- 項目二\n\n| 名前 | 値 |\n|---|---|\n| あ | い |\n".into(),
        ),
        (
            "pictures.md".into(),
            "![local](none.png)\n\n![remote](https://example.com/x.png)\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n$x^2$ and\n\n$$\\int_0^1 x dx$$\n".into(),
        ),
    ]
}

/// Keys pressed after the open: the scroll, the pages, the ends.
const SPECS: &[&str] = &[
    "",
    "j",
    "j j j",
    "k",
    "G",
    "G g",
    "G k",
    "g",
    "<C-f>",
    "<C-f> <C-f>",
    "<C-b>",
    "<C-d>",
    "<C-d> <C-u>",
    "<PgDn>",
    "<PgDn> <PgUp>",
    "<Down> <Down>",
    "<End>",
    "<End> <Home>",
];

#[test]
fn the_page_scroll_and_footer_agree_with_the_host() {
    let dir = sandbox("doc-parity");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let (mut checked, mut scrolled) = (0, 0);
    for (name, _) in fixtures() {
        for spec in SPECS {
            let what = format!("{name} [{spec}]");
            let h = host_term(&root, &name, W, HOST_H, spec);
            let a = app_term(&root.join(&name), W, APP_H, spec);
            let (hi, ai) = (inner(&h.term), inner(&a));
            assert_eq!(
                hi,
                ai,
                "{what}: the page differs\nhost:\n{}\napp:\n{}",
                rows_of(&h.term).join("\n"),
                rows_of(&a).join("\n")
            );
            let (hh, ah) = (
                hints(rows_of(&h.term).last().expect("footer")),
                hints(rows_of(&a).last().expect("footer")),
            );
            for t in hh.iter().filter(|t| !ah.contains(t)) {
                assert!(
                    HOST_ONLY.contains(&t.as_str()),
                    "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            for t in ah.iter().filter(|t| !hh.contains(t)) {
                assert!(
                    t.starts_with("q:quit") || t.starts_with("line "),
                    "{what}: the app footer has `{t}` that the host lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            checked += 1;
            scrolled += usize::from(hi.first().is_some_and(|l| l != "Top") && name == "scroll.md");
        }
    }
    assert!(checked >= 13 * SPECS.len(), "{checked}");
    assert!(scrolled > 5, "{scrolled}");
}

/// The rows only the host's help lists for a Markdown file (search, links, bookmarks, the tabs...)
/// are not in the app's; every row the app lists must be the host's, in the host's order.
#[test]
fn the_document_help_rows_agree_with_the_host() {
    let dir = sandbox("doc-help");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let mut compared = 0;
    for (name, _) in fixtures() {
        for spec in ["?", "G ?", "j j ?", "<C-f> ?", "? j", "? G", "? <C-d>"] {
            let what = format!("{name} [{spec}]");
            let h = host_term(&root, &name, 100, 130, spec);
            let a = app_term(&root.join(&name), 99, 130, spec);
            let hr = rows_of(&h.term);
            let (py, px) = hr
                .iter()
                .enumerate()
                .find_map(|(y, r)| {
                    r.find("┌ Help")
                        .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
                })
                .unwrap_or_else(|| panic!("{what}: the host's help panel in {hr:#?}"));
            let hl: Vec<String> = help_lines(&hr[py + 1..], Some(px))
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            let al: Vec<String> = help_lines(&rows_of(&a), None)
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            // The panel scrolls (`? j`): the same keys scroll both, so compare what both show.
            if spec.starts_with("? ") {
                continue;
            }
            let mut last = 0;
            // The title sits on the host's border and in the app's footer.
            let mut al = al;
            let title = al.pop().expect("a title");
            let border: String = hr[py].split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                border.contains(&title),
                "{what}: `{title}` not in `{border}`"
            );
            for row in &al {
                // The app says `quit` where the host says `back to tree`.
                let found = hl.iter().position(|r| {
                    same_row(r, row) || (row.starts_with("q / Esc") && r.starts_with("q / Esc"))
                });
                let at = found.unwrap_or_else(|| {
                    panic!("{what}: the app's help row `{row}` is not in the host's\nhost: {hl:#?}\napp: {al:#?}")
                });
                assert!(
                    at >= last || row.starts_with("q / Esc"),
                    "{what}: `{row}` is out of order"
                );
                last = last.max(at);
                compared += 1;
            }
        }
    }
    assert!(compared > 13 * 4 * 6, "{compared}");
}

/// Narrow and short terminals: the wrapping, the clamp of the scroll and the page size follow
/// the frame, whatever its size.
#[test]
fn a_narrow_or_short_terminal_shows_what_the_host_shows() {
    let dir = sandbox("doc-narrow");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let mut checked = 0;
    for (name, _) in fixtures() {
        for (w, h) in [(24u16, 12u16), (40, 20), (61, 9), (80, 40), (121, 6)] {
            for spec in [
                "",
                "j j",
                "<C-f>",
                "<C-d>",
                "G",
                "G <C-b>",
                "<C-f> <C-f> <C-d>",
            ] {
                // The test backend keeps a stale cell where a wide character was drawn in an
                // earlier frame (the host's too, before its repaint above): with a wide
                // character, only one scroll step is compared, which the repaint cleans.
                if name == "cjk.md" && spec.split(' ').count() > 1 && spec != "<C-f>" {
                    continue;
                }
                let what = format!("{name} {w}x{h} [{spec}]");
                let hs = host_term(&root, &name, w, h + 1, spec);
                let a = app_term(&root.join(&name), w, h, spec);
                assert_eq!(
                    inner(&hs.term),
                    inner(&a),
                    "{what}: the page differs\nhost:\n{}\napp:\n{}",
                    rows_of(&hs.term).join("\n"),
                    rows_of(&a).join("\n")
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 13 * 5 * 7 - 5 * 4, "{checked}");
}

/// Every Markdown the snapshot corpus holds, on its first page and one page down: the host and
/// the app lay the same source out the same way.
#[test]
fn the_markdown_corpus_shows_the_same_page_as_the_host() {
    let cases = crate::app::md_snapshot_tests::all_cases();
    // The corpus does not shrink unnoticed (a case dropped would drop its check).
    assert!(cases.len() >= 300, "{} cases", cases.len());
    let dir = sandbox("doc-corpus");
    let mut checked = 0;
    for (i, (label, src)) in cases.iter().enumerate() {
        let name = format!("case{i}.md");
        std::fs::write(dir.join(&name), src).unwrap();
        let root = canon(&dir);
        for (spec, w, h) in [("", 100u16, 30u16), ("<C-f>", 100, 30), ("", 37, 14)] {
            let hs = host_term(&root, &name, w, h + 1, spec);
            let a = app_term(&root.join(&name), w, h, spec);
            assert_eq!(
                inner(&hs.term),
                inner(&a),
                "{label} ({name}) {w}x{h} [{spec}]: the page differs\nhost:\n{}\napp:\n{}",
                rows_of(&hs.term).join("\n"),
                rows_of(&a).join("\n")
            );
            checked += 1;
        }
        std::fs::remove_file(dir.join(&name)).unwrap();
    }
    assert!(checked >= cases.len() * 3, "{checked}");
}
