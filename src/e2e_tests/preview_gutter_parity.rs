//! Compare the SDK's preview layout with real keys, App state and ui::render.
use super::*;
use konoma_plugin::host::project::md_diff::{
    DiffBaseline, MdDiffKind, MdDiffRequest, MdDiffWorker,
};
use konoma_plugin::host::project::preview_gutter::{line_gutter_marks, GutterMark};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
};

fn same_cells(label: &str, host: &Buffer, sdk: &Buffer) {
    assert_eq!(host.area, sdk.area, "{label}: Rect");
    let mut compared = 0;
    let mut nonempty = false;
    for y in host.area.y..host.area.bottom() {
        for x in host.area.x..host.area.right() {
            let (h, s) = (&host[(x, y)], &sdk[(x, y)]);
            assert_eq!(
                (h.symbol(), h.fg, h.bg, h.modifier),
                (s.symbol(), s.fg, s.bg, s.modifier),
                "{label}: cell ({x},{y})"
            );
            compared += 1;
            nonempty |= h.symbol() != " ";
        }
    }
    assert_eq!(
        compared,
        usize::from(host.area.width) * usize::from(host.area.height)
    );
    assert!(nonempty, "{label}: nonempty content");
}

fn host_body(s: &Sim) -> Buffer {
    // Bottom status layout, hidden tabs: only the last screen row is chrome.
    let area = Rect::new(0, 0, 32, 10);
    let mut out = Buffer::empty(area);
    let screen = s.term.backend().buffer();
    assert_eq!(screen.area, Rect::new(0, 0, 32, 11));
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            out[(x, y)] = screen[(x, y)].clone();
        }
    }
    out
}

#[test]
fn preview_gutter_cells_match_host() {
    use GutterMark::{Added, Deleted, Modified};
    // Literal expectations are independent of the classification algorithm.
    let cases = [
        (
            "add.txt",
            "one\nthree\n",
            "one\ntwo\nthree\n",
            vec![(2, Added)],
        ),
        (
            "modify.txt",
            "one\nold\nthree\n",
            "one\nnew\nthree\n",
            vec![(2, Modified)],
        ),
        (
            "delete.txt",
            "one\ngone\nthree\n",
            "one\nthree\n",
            vec![(2, Deleted)],
        ),
        (
            "eof.txt",
            "one\ntwo\ngone\n",
            "one\ntwo\n",
            vec![(2, Deleted)],
        ),
        ("same.txt", "one\ntwo\n", "one\ntwo\n", vec![]),
        (
            "code.rs",
            "// one\n// old\n",
            "// one\n// new\n",
            vec![(2, Modified)],
        ),
        (
            "scroll.txt",
            "0\n1\n2\n3\n4\n5\n6\n7\n8\n9\nold\n11\n12\n13\n14\n15\n",
            "0\n1\n2\n3\n4\n5\n6\n7\n8\n9\nnew\n11\n12\n13\n14\n15\n",
            vec![(11, Modified)],
        ),
    ];
    for (name, old, new, expected) in cases {
        let dir = sandbox("preview-gutter-cells");
        init_git_repo(&dir);
        let path = dir.join(name);
        std::fs::write(&path, old).unwrap();
        run_git(&dir, &["add", name]);
        run_git(&dir, &["commit", "-qm", "baseline"]);
        std::fs::write(&path, new).unwrap();
        if name.ends_with(".rs") {
            crate::preview::code::warm_file("rs", &path);
        }
        let diff = crate::git::file_diff(&dir, &path);
        let marks = line_gutter_marks(&diff);
        assert_eq!(
            marks,
            expected.into_iter().collect(),
            "{name}: literal classification"
        );
        let mut cfg = cfg_en();
        cfg.ui.tabbar = "hidden".into();
        cfg.ui.statusbar = "bottom".into();
        cfg.ui.line_numbers = true;
        let mut s = Sim::with_config_sized(&canon(&dir), cfg, 32, 11);
        s.select(name);
        s.enter();
        if name == "scroll.txt" {
            s.key('G');
            assert!(s.app.preview_top_line() > 0, "scrolled file window");
        }
        s.term.clear().unwrap();
        s.draw();
        let first = s.app.preview_top_line();
        let raw = new
            .lines()
            .skip(first)
            .take(8)
            .collect::<Vec<_>>()
            .join("\n");
        let lines = if name.ends_with(".rs") {
            use konoma_plugin::view::LineStyler;
            konoma_code::CodeStyler::new(&path, &s.app.cfg.ui.theme.code_theme)
                .style_block(first, &raw)
                .into_iter()
                .map(konoma_plugin::ratatui_draw::into_line)
                .collect()
        } else {
            raw.lines().map(|l| Line::from(l.to_owned())).collect()
        };
        let lines = konoma_widgets::caret::apply_caret(
            lines,
            first,
            s.app.preview_caret_line(),
            0,
            konoma_widgets::caret::Selection::None,
        );
        let lines = lines
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                let mut spans = vec![Span::styled(
                    format!("{:>3} ", first + i + 1),
                    Style::new().fg(ratatui::style::Color::DarkGray),
                )];
                spans.extend(l.spans);
                Line::from(spans).style(l.style)
            })
            .collect();
        let host = host_body(&s);
        let mut sdk = Buffer::empty(host.area);
        let name_in_frame = s.app.format_path(s.app.tab.preview_path.as_ref().unwrap());
        let lens: Vec<u64> = new.split_inclusive('\n').map(|l| l.len() as u64).collect();
        konoma_plugin::app::draw_preview_lines_for_test(
            lines,
            first,
            &marks,
            &konoma_plugin::app::Display::default(),
            &mut sdk,
            host.area,
            (&name_in_frame, &lens),
        );
        same_cells(name, &host, &sdk);
        use konoma_plugin::host::project::rendered_diff::{
            GUTTER_ADDED, GUTTER_DELETED, GUTTER_MODIFIED,
        };
        for (&line, &mark) in &marks {
            if (first + 1..=first + 8).contains(&(line as usize)) {
                let cell = &sdk[(1, (line as usize - first) as u16)];
                let expected = match mark {
                    Added => ("▌", GUTTER_ADDED),
                    Modified => ("▌", GUTTER_MODIFIED),
                    Deleted => ("▔", GUTTER_DELETED),
                };
                assert_eq!(
                    (cell.symbol(), cell.fg),
                    expected,
                    "{name}: marker before line numbers"
                );
            }
        }
        let column = u16::from(!marks.is_empty());
        assert_eq!(
            sdk[(1 + column, 1)].symbol(),
            " ",
            "number gutter begins after change gutter"
        );
        if name == "same.txt" {
            assert_eq!(
                sdk[(3, 1)].symbol(),
                "1",
                "unchanged file has no change column"
            );
        }
    }

    let old = "# Title\n\nold paragraph\n\n## Keep\n\ngone paragraph\n\n### End\n";
    let new = "# Title\n\nnew paragraph\n\n## Keep\n\n### End\n\n#### Added\n";
    let wrapped_old = "# Title\n\nold paragraph\n\n## End\n";
    let wrapped_new = "# Title\n\nnew paragraph with several words and 日本語 that wraps across several rows of the narrow preview window\n\n## End\n";
    for (label, old, new, scroll) in [
        ("wrapped", wrapped_old, wrapped_new, false),
        ("wrapped-scroll", wrapped_old, wrapped_new, true),
        ("blocks", old, new, false),
        ("blocks-scroll", old, new, true),
        (
            "deleted-eof",
            "# Title\n\nparagraph\n\n## Gone\n",
            "# Title\n\nparagraph\n",
            false,
        ),
    ] {
        let dir = sandbox("preview-markdown-gutter-cells");
        init_git_repo(&dir);
        let path = dir.join("doc.md");
        std::fs::write(&path, old).unwrap();
        run_git(&dir, &["add", "doc.md"]);
        run_git(&dir, &["commit", "-qm", "baseline"]);
        std::fs::write(&path, new).unwrap();
        let mut cfg = cfg_en();
        cfg.ui.tabbar = "hidden".into();
        cfg.ui.statusbar = "bottom".into();
        let mut s = Sim::with_config_sized(&canon(&dir), cfg, 32, 11);
        s.select("doc.md");
        s.enter();
        if scroll {
            s.key('G');
        }
        s.term.clear().unwrap();
        s.draw();
        let outcome = MdDiffWorker::compute_md_diff(&MdDiffRequest {
            gen: 1,
            path: path.clone(),
            root: dir.to_path_buf(),
            kind: MdDiffKind::Gutter,
            baseline: DiffBaseline::FollowSnapshot(old.as_bytes().to_vec()),
            md_frontmatter: true,
            md_footnotes: true,
            md_inline_html: true,
        });
        let dv = konoma_plugin::view::DocumentView {
            markdown: new.into(),
            origin: konoma_plugin::input::Origin::File(path),
            resources: Default::default(),
            options: Default::default(),
            title: None,
            truncated: false,
        };
        let host = host_body(&s);
        let mut sdk = Buffer::empty(host.area);
        let name_in_frame = s.app.format_path(s.app.tab.preview_path.as_ref().unwrap());
        konoma_plugin::app::draw_preview_document_for_test(
            &dv,
            Some(&outcome),
            &konoma_plugin::app::Display::default(),
            usize::from(s.app.tab.preview_scroll),
            &mut sdk,
            host.area,
            &name_in_frame,
        );
        same_cells(label, &host, &sdk);
        let marks = s.app.md_diff_marks_for_test().unwrap();
        use crate::preview::markdown::PreviewMark;
        if label.starts_with("blocks") {
            assert_eq!(
                marks,
                vec![
                    (2..4, PreviewMark::Modified),
                    (7..8, PreviewMark::Deleted),
                    (9..11, PreviewMark::Added)
                ],
                "literal Markdown block classification"
            );
            if !scroll {
                assert_eq!(sdk[(1, 8)].symbol(), "▔", "deleted boundary anchor");
            } else {
                assert!(
                    sdk.content.iter().any(|c| c.symbol() == "▌"
                        && c.fg == konoma_plugin::host::project::rendered_diff::GUTTER_ADDED),
                    "added block is drawn"
                );
            }
        } else if label == "deleted-eof" {
            assert_eq!(
                marks,
                vec![(3..4, PreviewMark::Deleted)],
                "EOF block deletion anchor"
            );
            assert_eq!(sdk[(1, 4)].symbol(), "▔");
            assert_eq!(sdk[(2, 4)].symbol(), "p");
            assert!(
                !sdk[(2, 4)].modifier.contains(ratatui::style::Modifier::DIM),
                "deleted preview row is never dimmed"
            );
        } else {
            assert_eq!(
                marks,
                vec![(2..4, PreviewMark::Modified)],
                "wrapped block uses rendered logical rows"
            );
            if !scroll {
                assert_eq!(sdk[(1, 3)].symbol(), "▌", "paragraph separator is marked");
                assert_eq!(
                    sdk[(1, 4)].symbol(),
                    "▌",
                    "paragraph starts after the separator"
                );
                assert_eq!(
                    sdk[(1, 5)].symbol(),
                    "w",
                    "continuation row belongs to the wrapped paragraph"
                );
                assert_eq!(
                    sdk[(1, 7)].symbol(),
                    "n",
                    "paragraph spans several display rows"
                );
            } else {
                assert!(s.app.tab.preview_scroll > 0, "scroll is in display rows");
            }
        }
    }
}
