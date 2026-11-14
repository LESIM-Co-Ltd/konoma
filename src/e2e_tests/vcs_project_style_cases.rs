// The bridge must preserve every style value, including padding and explicit Reset.
// Real repositories below separately exercise the standard plugin's conversion arms.
use ratatui::{
    buffer::{Buffer, Cell},
    style::{Color, Modifier},
};

fn bridge_cell(expected: &Cell, actual: &Cell) -> bool {
    (
        expected.symbol(),
        expected.fg,
        expected.bg,
        expected.modifier,
    ) == (actual.symbol(), actual.fg, actual.bg, actual.modifier)
}

fn draw_bridge_fixture(
    root: &std::path::Path,
    lines: Vec<konoma_plugin::style::Line>,
) -> (Sim, Buffer) {
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session, View, ViewCx, ViewReq};
    use std::sync::Arc;
    struct Fixture(Vec<konoma_plugin::style::Line>);
    impl Plugin for Fixture {
        fn manifest(&self) -> Manifest {
            konoma_vcs::Git.manifest()
        }
        fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(FixtureSession(
                konoma_vcs::Git.open(input, cx)?,
                self.0.clone(),
            )))
        }
    }
    struct FixtureSession(Box<dyn Session>, Vec<konoma_plugin::style::Line>);
    impl Session for FixtureSession {
        fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
            let real = self.0.view(req, cx)?;
            if req.project.as_ref().is_some_and(|p| p.screen == "changes") {
                return Ok(View::List(konoma_plugin::view::ListView {
                    rows: self
                        .1
                        .iter()
                        .cloned()
                        .map(|line| {
                            konoma_plugin::view::ListRow::new("style", vec![line]).selectable(false)
                        })
                        .collect(),
                    ..Default::default()
                }));
            }
            Ok(real)
        }
    }
    let height = lines.len() as u16 + 5;
    static ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let marker = crate::plugins::test_plugins::register_replacement(
        &format!(
            "vcs_port_style_bridge_{}",
            ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        Arc::new(Fixture(lines)),
    );
    let mut cfg = config("git", "en");
    cfg.plugins
        .extend(crate::plugins::test_plugins::marker_config(&marker).plugins);
    let mut sim = open(root, cfg, (80, height));
    let frame = sim.project_frame();
    assert_eq!(sim.app.project_frame().unwrap().key.screen, "changes");
    (sim, frame)
}

#[test]
fn vcs_port_style_bridge_preserves_every_legacy_color_and_modifier() {
    if isolated("vcs_port_style_bridge_preserves_every_legacy_color_and_modifier") {
        return;
    }
    use konoma_plugin::style::{Color as C, Line, Span, Style};
    let root = super::vcs_log::repo(1);
    let colors = [
        (C::Reset, Color::Reset),
        (C::Red, Color::Red),
        (C::Green, Color::Green),
        (C::Yellow, Color::Yellow),
        (C::Blue, Color::Blue),
        (C::Magenta, Color::Magenta),
        (C::Cyan, Color::Cyan),
        (C::White, Color::White),
        (C::LightRed, Color::LightRed),
        (C::LightGreen, Color::LightGreen),
        (C::LightYellow, Color::LightYellow),
    ];
    let mut lines = Vec::new();
    let mut expected = Vec::new();
    for (sdk, legacy) in colors {
        for bold in [false, true] {
            let style = Style::new().fg(sdk);
            lines.push(Line::from(Span::styled(
                "X ",
                if bold { style.bold() } else { style },
            )));
            expected.push((
                legacy,
                if bold {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            ));
        }
    }
    lines.extend([
        Line::from(Span::raw("X ")),
        Line::from(Span::styled("X ", Style::new().bold())),
    ]);
    expected.extend([
        (Color::Reset, Modifier::empty()),
        (Color::Reset, Modifier::BOLD),
    ]);
    let (_sim, frame) = draw_bridge_fixture(&root, lines);
    let (bx, by) = body_origin();
    for (row, (color, modifiers)) in expected.into_iter().enumerate() {
        for x in 0..2 {
            let mut wanted = Cell::default();
            wanted.set_symbol(if x == 0 { "X" } else { " " });
            wanted.fg = color;
            wanted.modifier = modifiers;
            assert!(
                bridge_cell(&wanted, &frame[(x + bx, row as u16 + by)]),
                "palette row={row}, x={x}"
            );
        }
    }
    let cell = &frame[(bx, by)];
    let mut wrong = cell.clone();
    wrong.modifier ^= Modifier::BOLD;
    assert!(
        !bridge_cell(cell, &wrong),
        "style bridge must compare the modifier field"
    );
    wrong = cell.clone();
    wrong.fg = Color::Blue;
    assert!(
        !bridge_cell(cell, &wrong),
        "style bridge must compare the foreground"
    );
    wrong = cell.clone();
    wrong.bg = Color::Red;
    assert!(
        !bridge_cell(cell, &wrong),
        "style bridge must compare the background"
    );
}

#[test]
fn vcs_port_graph_legacy_glyphs_and_padding_styles_reach_the_bridge() {
    if isolated("vcs_port_graph_legacy_glyphs_and_padding_styles_reach_the_bridge") {
        return;
    }
    use konoma_plugin::style::{Color as C, Line, Span, Style};
    use konoma_vcs_core::style::VcsColor;
    let root = super::vcs_log::repo(1);
    for vcs in [crate::vcs::VcsKind::Git, crate::vcs::VcsKind::Jj] {
        for worktree in [false, true] {
            let fixture = konoma_vcs_core::testing::graph_style_fixture(vcs, worktree);
            let legacy = match (vcs, worktree) {
                (crate::vcs::VcsKind::Git, false) => vec![
                    ("◆  ", "Wnn"),
                    ("├─┐", "WCC"),
                    ("● │", "WrC"),
                    ("│ ●", "WrC"),
                    ("├─┘", "WCC"),
                    ("●  ", "Wnn"),
                ],
                (crate::vcs::VcsKind::Jj, false) => vec![
                    ("○  ", "Wnn"),
                    ("├─┐", "WCC"),
                    ("○ │", "WrC"),
                    ("│ ○", "WrC"),
                    ("├─┘", "WCC"),
                    ("○  ", "Wnn"),
                ],
                (crate::vcs::VcsKind::Git, true) => vec![
                    ("│ ●  ", "WrYnn"),
                    ("│ ◆  ", "WrCnn"),
                    ("│ ├─┐", "WrCGG"),
                    ("├─┘ │", "WCCrG"),
                    ("●   │", "WrrrG"),
                    ("│   ●", "WrrrG"),
                    ("├───┘", "WGGGG"),
                    ("●    ", "Wnnnn"),
                ],
                (crate::vcs::VcsKind::Jj, true) => vec![
                    ("│ @  ", "WrYnn"),
                    ("│ ○  ", "WrCnn"),
                    ("│ ├─┐", "WrCGG"),
                    ("├─┘ │", "WCCrG"),
                    ("○   │", "WrrrG"),
                    ("│   ○", "WrrrG"),
                    ("├───┘", "WGGGG"),
                    ("○    ", "Wnnnn"),
                ],
            };
            // This is bridge input, not a second VCS renderer: the independent literal above
            // still verifies the core DAG and its unset/Reset distinction before drawing it.
            let lines = fixture
                .iter()
                .map(|row| {
                    Line::from(
                        row.graph
                            .iter()
                            .map(|(text, style)| {
                                let mut mapped = Style::new();
                                if let Some(color) = style.fg {
                                    mapped = mapped.fg(match color {
                                        VcsColor::White => C::White,
                                        VcsColor::Cyan => C::Cyan,
                                        VcsColor::Green => C::Green,
                                        VcsColor::Yellow => C::Yellow,
                                        VcsColor::Reset => C::Reset,
                                        _ => panic!("unexpected fixture color"),
                                    });
                                }
                                if style.bold {
                                    mapped = mapped.bold();
                                }
                                Span::styled(text, mapped)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(lines.len(), legacy.len());
            for (line, (text, styles)) in lines.iter().zip(&legacy) {
                assert_eq!(line.plain(), *text);
                assert_eq!(text.chars().count(), styles.len());
                for (span, code) in line.spans.iter().zip(styles.chars()) {
                    let expected = match code {
                        'W' => Style::new().fg(C::White),
                        'C' => Style::new().fg(C::Cyan),
                        'G' => Style::new().fg(C::Green),
                        'Y' => Style::new().fg(C::Yellow).bold(),
                        'r' => Style::new().fg(C::Reset),
                        'n' => Style::new(),
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        span.style, expected,
                        "unset and explicit Reset must remain distinct"
                    );
                }
            }
            let (_sim, frame) = draw_bridge_fixture(&root, lines);
            let (bx, by) = body_origin();
            for (y, (text, styles)) in legacy.into_iter().enumerate() {
                for (x, (ch, code)) in text.chars().zip(styles.chars()).enumerate() {
                    let mut wanted = Cell::default();
                    wanted.set_symbol(&ch.to_string());
                    wanted.fg = match code {
                        'W' => Color::White,
                        'C' => Color::Cyan,
                        'G' => Color::Green,
                        'Y' => Color::Yellow,
                        _ => Color::Reset,
                    };
                    if code == 'Y' {
                        wanted.modifier = Modifier::BOLD;
                    }
                    assert!(
                        bridge_cell(&wanted, &frame[(x as u16 + bx, y as u16 + by)]),
                        "{vcs:?}/{worktree}/{x},{y}"
                    );
                }
            }
        }
    }
}

#[test]
fn vcs_port_real_status_conversion_and_drawn_colors() {
    if isolated("vcs_port_real_status_conversion_and_drawn_colors") {
        return;
    }
    use crate::test_support::run_git;
    let root = super::vcs_changes::git_repo("vcs_real_status_palette", true);
    std::fs::remove_file(root.join("b-unstaged.txt")).unwrap();
    std::os::unix::fs::symlink("a-staged.txt", root.join("b-unstaged.txt")).unwrap();
    std::fs::write(root.join("g-conflict.txt"), "conflicted\n").unwrap();
    let repo = konoma_vcs_core::git2::Repository::open(&root).unwrap();
    let blobs = ["base\n", "ours\n", "theirs\n"].map(|s| repo.blob(s.as_bytes()).unwrap());
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .current_dir(&root)
        .args(["update-index", "--index-info"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let text = blobs
        .iter()
        .enumerate()
        .map(|(i, oid)| format!("100644 {oid} {}\tg-conflict.txt\n", i + 1))
        .collect::<String>();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(
        child.wait_with_output().unwrap().status.success(),
        "conflict fixture index"
    );
    run_git(&root, &["config", "core.symlinks", "true"]);
    let mut sim = open(&root, config("git", "en"), (180, 24));
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("list required")
    };
    let frame = sim.project_frame();
    for (name, marker, color, sdk) in [
        (
            "a-staged.txt",
            "M",
            Color::Yellow,
            konoma_plugin::style::Color::Yellow,
        ),
        (
            "b-unstaged.txt",
            "T",
            Color::Yellow,
            konoma_plugin::style::Color::Yellow,
        ),
        (
            "c-deleted.txt",
            "D",
            Color::Red,
            konoma_plugin::style::Color::Red,
        ),
        (
            "d-renamed.txt",
            "R",
            Color::Cyan,
            konoma_plugin::style::Color::Cyan,
        ),
        (
            "e-untracked.txt",
            "U",
            Color::LightGreen,
            konoma_plugin::style::Color::LightGreen,
        ),
        (
            "f-added.txt",
            "A",
            Color::Green,
            konoma_plugin::style::Color::Green,
        ),
        (
            "g-conflict.txt",
            "!",
            Color::LightRed,
            konoma_plugin::style::Color::LightRed,
        ),
    ] {
        let (row, entry) = list
            .rows
            .iter()
            .enumerate()
            .find(|(_, r)| r.name.as_deref() == Some(name))
            .unwrap();
        assert_eq!(
            entry.cells[0].spans[1].text,
            format!("{marker} "),
            "status marker {name}"
        );
        assert_eq!(
            entry.cells[0].spans[1].style.fg,
            Some(sdk),
            "standard plugin status conversion {name}"
        );
        let (bx, by) = body_origin();
        for x in [2, 3] {
            let cell = &frame[(x + bx, row as u16 + by)];
            assert_eq!(cell.fg, color, "drawn status foreground {name}/{x}");
            assert_eq!(cell.bg, Color::Reset);
            assert_eq!(
                cell.modifier,
                if row == 0 {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                }
            );
        }
    }
}

fn real_graph_style_repository() -> crate::test_support::TmpDir {
    let root = super::vcs_log::repo(1);
    let repo = konoma_vcs_core::git2::Repository::open(&root).unwrap();
    let r = repo.head().unwrap().peel_to_commit().unwrap();
    let tree = r.tree().unwrap();
    let make = |reference: &str,
                message: &str,
                parents: &[&konoma_vcs_core::git2::Commit<'_>],
                minute: i64| {
        let sig = konoma_vcs_core::git2::Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &konoma_vcs_core::git2::Time::new(1_577_836_800 + minute * 60, 0),
        )
        .unwrap();
        let oid = repo
            .commit(Some(reference), &sig, &sig, message, &tree, parents)
            .unwrap();
        repo.find_commit(oid).unwrap()
    };
    let b = make("refs/heads/base", "B", &[&r], 2);
    let f = make("refs/heads/topic", "F", &[&r], 1);
    make("refs/heads/feature", "M", &[&b, &f], 3);
    let original = repo.head().unwrap().name().unwrap().to_owned();
    repo.set_head("refs/heads/feature").unwrap();
    repo.find_reference(&original).unwrap().delete().unwrap();
    drop((b, f, tree, r));
    drop(repo);
    root
}

#[test]
fn vcs_port_real_graph_palette_checks_each_conversion_arm() {
    if isolated("vcs_port_real_graph_palette_checks_each_conversion_arm") {
        return;
    }
    use konoma_plugin::style::Color as S;
    use konoma_vcs_core::style::VcsColor as C;
    let root = super::vcs_log::repo(1);
    let repo = konoma_vcs_core::git2::Repository::open(&root).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let tree = parent.tree().unwrap();
    for lane in 0..14 {
        let signature = konoma_vcs_core::git2::Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &konoma_vcs_core::git2::Time::new(1_577_836_860 + lane * 60, 0),
        )
        .unwrap();
        repo.commit(
            Some(&format!("refs/heads/lane-{lane:02}")),
            &signature,
            &signature,
            &format!("lane {lane}"),
            &tree,
            &[&parent],
        )
        .unwrap();
    }
    repo.set_head("refs/heads/lane-13").unwrap();
    let map = |c| match c {
        C::Reset => S::Reset,
        C::Red => S::Red,
        C::Green => S::Green,
        C::Yellow => S::Yellow,
        C::Blue => S::Blue,
        C::Magenta => S::Magenta,
        C::Cyan => S::Cyan,
        C::White => S::White,
        C::LightRed => S::LightRed,
        C::LightGreen => S::LightGreen,
        C::LightYellow => S::LightYellow,
    };
    let mut cfg = config("git", "en");
    cfg.plugins
        .entry("git".into())
        .or_default()
        .insert("graph_max_branches".into(), toml::Value::Integer(0));
    let mut sim = open(&root, cfg, (180, 60));
    screen(&mut sim, "graph");
    let expected = crate::git::graph_with_base(
        &root,
        None,
        &konoma_vcs_core::git::WorktreeLabels::default(),
        None,
    );
    let list = real_list(&sim);
    assert_eq!(list.rows.len(), expected.len());
    let mut seen = std::collections::HashSet::new();
    for (row, core) in list.rows.iter().zip(&expected) {
        for (actual, (text, style)) in row.cells[0].spans.iter().zip(&core.graph) {
            assert_eq!(&actual.text, text, "palette graph glyph");
            assert_eq!(
                actual.style.fg,
                style.fg.map(map),
                "palette graph conversion {}",
                core.subject
            );
            assert_eq!(
                actual
                    .style
                    .add
                    .contains(konoma_plugin::style::Modifier::BOLD),
                style.bold
            );
            if let Some(fg) = actual.style.fg {
                seen.insert(format!("{fg:?}"));
            }
        }
    }
    for color in [
        "White",
        "Cyan",
        "Green",
        "Magenta",
        "Blue",
        "LightRed",
        "LightYellow",
        "Reset",
    ] {
        assert!(
            seen.contains(color),
            "independent fixture must exercise {color}"
        );
    }
}

#[test]
fn vcs_port_right_metadata_literal_padding_ellipsis_and_narrow_fallback() {
    if isolated("vcs_port_right_metadata_literal_padding_ellipsis_and_narrow_fallback") {
        return;
    }
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session, View, ViewCx, ViewReq};
    #[derive(Clone, Copy)]
    struct Fixture(&'static str, &'static str);
    impl Plugin for Fixture {
        fn manifest(&self) -> Manifest {
            konoma_vcs::Git.manifest()
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(*self))
        }
    }
    impl Session for Fixture {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            let (left, right) = (self.0, self.1);
            Ok(View::List(konoma_plugin::view::ListView {
                rows: vec![konoma_plugin::view::ListRow::new(
                    "file",
                    vec![konoma_plugin::style::Line::raw(left)],
                )
                .selectable(false)
                .right(konoma_plugin::style::Line::raw(right))],
                ..Default::default()
            }))
        }
    }
    let root = super::vcs_log::repo(0);
    for width in [20u16, 12, 5] {
        let (left, right) = if width == 20 {
            ("abc", "date")
        } else if width == 12 {
            ("abcdefghijklmnopqrstuvwxyz", "date")
        } else {
            ("abcdef", "longmeta")
        };
        let token = crate::plugins::test_plugins::register_replacement(
            &format!("vcs_right_literal_{width}"),
            std::sync::Arc::new(Fixture(left, right)),
        );
        let mut cfg = config("git", "en");
        cfg.plugins
            .extend(crate::plugins::test_plugins::marker_config(&token).plugins);
        let mut sim = open(&root, cfg, (width + 2, 8));
        let frame = sim.project_frame();
        let (bx, by) = body_origin();
        let text = (0..width)
            .map(|x| frame[(x + bx, by)].symbol())
            .collect::<String>();
        // The column is the frame's inside; the right edge of the frame follows it. Measure the
        // whole row up to that edge, so overflow past the column (instead of elision) would
        // overwrite the edge and be visible.
        let row = text.clone();
        let past_column = frame[(width + bx, by)].symbol().to_string();
        assert_eq!(past_column, "│", "width {width}: drawn past the column: {row:?}");
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(row.trim_end()),
            row.trim_end().chars().count(),
            "width {width}: unexpected wide cells: {row:?}"
        );
        if width == 20 {
            assert!(text.starts_with("abc"));
            assert!(text.ends_with("date"));
            assert_eq!(unicode_width::UnicodeWidthStr::width(text.as_str()), 20);
            assert_eq!(text, format!("abc{}date", " ".repeat(13)));
        } else if width == 12 {
            assert!(text.contains('…'));
            assert!(text.ends_with("date"));
            assert!(unicode_width::UnicodeWidthStr::width(text.as_str()) <= 12);
            assert_eq!(text, "abcdef… date");
        } else {
            assert!(!row.contains("longmeta"), "meta must be dropped: {row:?}");
            assert!(!row.contains("long"), "meta must be dropped: {row:?}");
            assert!(unicode_width::UnicodeWidthStr::width(text.as_str()) <= 5);
            assert!(row.starts_with("abcd…"), "left side ellipsized at the tail: {row:?}");
        }
    }
}

#[test]
fn vcs_port_real_graph_conversion_legend_and_legacy_cells() {
    if isolated("vcs_port_real_graph_conversion_legend_and_legacy_cells") {
        return;
    }
    let root = real_graph_style_repository();
    for worktree in [false, true] {
        if worktree {
            std::fs::write(root.join("dirty"), "new\n").unwrap();
        }
        let mut cfg = config("git", "en");
        let settings = cfg.plugins.entry("git".into()).or_default();
        settings.insert("graph_max_branches".into(), toml::Value::Integer(0));
        let mut sim = open(&root, cfg, (180, 28));
        screen(&mut sim, "graph");
        if worktree {
            // Pin B without changing the recency order of the three legend entries.
            let repo = konoma_vcs_core::git2::Repository::open(&root).unwrap();
            let base = repo
                .find_reference("refs/heads/base")
                .unwrap()
                .target()
                .unwrap()
                .to_string();
            for _ in 0..16 {
                if selected_value(&sim) == base {
                    break;
                }
                keys(&mut sim, "j");
            }
            assert_eq!(selected_value(&sim), base, "style fixture selects B");
            keys(&mut sim, "sg");
        }
        let content = sim.app.project_content().unwrap();
        let ProjectContent::List(list) = content.as_ref() else {
            panic!("list required")
        };
        // Git's topo-order emits F before B. The synthetic bridge above keeps the old
        // fixture order; this independent repository also exercises the plugin mapper.
        let legacy = if worktree {
            vec![
                ("│ ●  ", "WrYnn"),
                ("│ ◆  ", "WrCnn"),
                ("│ ├─┐", "WrCGG"),
                ("│ │ ●", "WrCrG"),
                ("├─┘ │", "WCCrG"),
                ("●   │", "WrrrG"),
                ("├───┘", "WGGGG"),
                ("●    ", "Wnnnn"),
            ]
        } else {
            vec![
                ("◆  ", "Wnn"),
                ("├─┐", "WCC"),
                ("│ ●", "WrC"),
                ("● │", "WrC"),
                ("├─┘", "WCC"),
                ("●  ", "Wnn"),
            ]
        };
        assert_eq!(list.rows.len(), legacy.len());
        let frame = sim.project_frame();
        let (bx, by) = body_origin();
        for (y, (text, codes)) in legacy.iter().enumerate() {
            let spans = &list.rows[y].cells[0].spans;
            for (x, (ch, code)) in text.chars().zip(codes.chars()).enumerate() {
                let sdk = match code {
                    'W' => Some(konoma_plugin::style::Color::White),
                    'C' => Some(konoma_plugin::style::Color::Cyan),
                    'G' => Some(konoma_plugin::style::Color::Green),
                    'Y' => Some(konoma_plugin::style::Color::Yellow),
                    'r' => Some(konoma_plugin::style::Color::Reset),
                    'n' => None,
                    _ => unreachable!(),
                };
                assert_eq!(
                    spans[x].text,
                    ch.to_string(),
                    "plugin graph glyph {worktree}/{x},{y}"
                );
                assert_eq!(
                    spans[x].style.fg, sdk,
                    "plugin graph color {worktree}/{x},{y}"
                );
                assert_eq!(
                    spans[x]
                        .style
                        .add
                        .contains(konoma_plugin::style::Modifier::BOLD),
                    code == 'Y',
                    "plugin graph bold {worktree}/{x},{y}"
                );
                let mut expected = Cell::default();
                expected.set_symbol(&ch.to_string());
                expected.fg = match code {
                    'W' => Color::White,
                    'C' => Color::Cyan,
                    'G' => Color::Green,
                    'Y' => Color::Yellow,
                    _ => Color::Reset,
                };
                expected.modifier = (if code == 'Y' {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }) | if y == 0 {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                };
                assert!(
                    bridge_cell(&expected, &frame[(x as u16 + bx, y as u16 + by)]),
                    "drawn graph cell {worktree}/{x},{y}"
                );
            }
        }
        if worktree {
            let chips = &list.bottom.as_ref().unwrap().chips;
            assert_eq!(
                chips.iter().map(|line| line.plain()).collect::<Vec<_>>(),
                ["⎇ ● feature", "⌖ ● base", "● topic"],
                "legend order, branch names, HEAD and base markers"
            );
            for (name, expected) in [
                ("feature", konoma_plugin::style::Color::Cyan),
                ("base", konoma_plugin::style::Color::White),
                ("topic", konoma_plugin::style::Color::Green),
            ] {
                let chip = chips
                    .iter()
                    .find(|line| line.plain().ends_with(name))
                    .unwrap();
                assert_eq!(
                    chip.spans.iter().find(|s| s.text == "●").unwrap().style.fg,
                    Some(expected),
                    "legend node color {name}"
                );
            }
        }
    }
}
