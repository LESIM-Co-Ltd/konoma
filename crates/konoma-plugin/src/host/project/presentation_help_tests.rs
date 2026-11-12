use super::presentation::*;
use super::test_facts::default_keymap_facts;
use crate::host::acts::ActBinding;
use crate::Tr;
use crossterm::event::KeyCode;
use konoma_host_core::help::CoreSection;
use konoma_host_core::keymap::KeyPress;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget},
};

fn words() -> ProjectHelpWords<'static> {
    ProjectHelpWords {
        plugin: "Example",
        project: "Project",
        picker: "Graph branches",
        graph: "Git graph",
        new_tab: "open the entry under the cursor in a new tab",
        move_list: "move",
        move_commit: "move (commit / uncommitted)",
        ends: "top/bottom",
        apply: "apply",
        cancel: "cancel",
        filter: "filter by name",
        mark: "mark",
        search: "filter the list",
        refresh: "refresh",
        screens: "cycle changes/log/graph/branches/worktrees",
        movement: "Movement",
        back_to_changes: "back to changes",
        cycle: "Cycle project screens",
        back: "Pop this screen or return to the previous view",
        scroll: "scroll",
        copy: "copy",
        actions: "Actions",
    }
}

fn binding(
    index: usize,
    key: Option<KeyPress>,
    copy: Option<char>,
    enabled: bool,
) -> ProjectPresentationBinding {
    ProjectPresentationBinding {
        binding: ActBinding {
            act: index,
            key,
            copy_key: copy.map(KeyPress::ch),
            context: None,
            hint_group: None,
            label: Tr::new(format!("item {index}")),
            help: Tr::new(""),
            footer: None,
            writes: false,
        },
        enabled,
        declared_enter: false,
    }
}

fn presentation(
    facts: &ProjectPresentationFacts,
    bindings: &[ProjectPresentationBinding],
) -> ProjectPresentation {
    ProjectPresentation::build(
        facts,
        ProjectCapabilities {
            can_open_tab: false,
        },
        bindings,
        &ProjectPresentationWords {
            language: "en",
            screens: "screens",
            move_list: "move",
            scroll: "scroll",
            mark: "mark",
            filter: "filter",
            back: "back",
            help: "help",
            new_tab: "new tab",
            copy: "copy",
            actions: "Actions",
            hscroll: "hscroll",
            ends: "ends",
            diff_layout: "unified/split/auto",
            cycle: "Cycle project screens",
            pop: "Pop this screen or return to the previous view",
            apply: "apply",
            cancel: "cancel",
            nav: "nav",
            search: "search",
            half_page: "half page",
            page: "page",
            line_ends: "line ends",
        },
    )
}

fn cells(actual: &[CoreSection], expected: &[(&str, &[(&str, &str)])]) {
    let actual_view: Vec<_> = actual
        .iter()
        .map(|s| (s.title.as_str(), s.rows.as_slice()))
        .collect();
    let expected_rows: Vec<Vec<(String, String)>> = expected
        .iter()
        .map(|(_, rows)| {
            rows.iter()
                .map(|(k, d)| (k.to_string(), d.to_string()))
                .collect()
        })
        .collect();
    let expected_view: Vec<_> = expected
        .iter()
        .zip(&expected_rows)
        .map(|((title, _), rows)| (*title, rows.as_slice()))
        .collect();
    let area = Rect::new(2, 3, 100, 32);
    let mut a = Buffer::empty(area);
    let mut e = Buffer::empty(area);
    Paragraph::new(konoma_widgets::help_panel::help_lines(&actual_view)).render(area, &mut a);
    Paragraph::new(konoma_widgets::help_panel::help_lines(&expected_view)).render(area, &mut e);
    assert_eq!(a.area, area);
    assert_eq!(e.area, area);
    let mut count = 0;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let (a, e) = (&a[(x, y)], &e[(x, y)]);
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (e.symbol(), e.fg, e.bg, e.modifier),
                "{x},{y}"
            );
            count += 1;
        }
    }
    assert_eq!(count, 3200);
    assert!(a.content.iter().any(|c| c.symbol() != " "));
}

#[test]
fn project_help_cells_preserve_uncovered_aliases() {
    for both in [false, true] {
        let facts = ProjectPresentationFacts {
            content: Some(ProjectPresentationContent::List),
            pushed: true,
            open: Some(ProjectOpenFacts {
                label: "open".into(),
                right_aliases: vec![KeyPress::ch('l'), KeyPress::key(KeyCode::Right)],
            }),
            ..default_keymap_facts()
        };
        let mut bindings = vec![binding(0, Some(KeyPress::key(KeyCode::Enter)), None, true)];
        if both {
            bindings.push(binding(1, Some(KeyPress::ch('l')), None, true));
        }
        let p = presentation(&facts, &bindings);
        let sections = p.help_sections(&words(), &[]);
        let rows: &[(&str, &str)] = if both {
            &[("Enter", "item 0"), ("l", "item 1")]
        } else {
            &[("Enter", "item 0")]
        };
        // Literal host notation, including the uncovered Right alias and the pushed-screen back row.
        cells(
            &sections,
            &[
                ("Example", rows),
                (
                    "Project",
                    &[
                        (if both { "→" } else { "l/→" }, "open"),
                        ("q / Esc", "Pop this screen or return to the previous view"),
                        ("j/k / ↑/↓", "scroll"),
                    ],
                ),
                (
                    "Actions",
                    if both {
                        &[("Space a", "item 0"), ("Space l", "item 1")]
                    } else {
                        &[("Space a", "item 0")]
                    },
                ),
            ],
        );
    }
}

#[test]
fn project_help_cells_preserve_graph_and_picker_sections() {
    let facts = ProjectPresentationFacts {
        picker: true,
        ..Default::default()
    };
    cells(
        &presentation(&facts, &[]).help_sections(&words(), &[]),
        &[(
            "Graph branches",
            &[
                ("j/k", "move"),
                ("g/G", "top/bottom"),
                ("h/Esc  ·  q", "cancel"),
            ],
        )],
    );
    for jj in [false, true] {
        let facts = ProjectPresentationFacts {
            graph_jj: Some(jj),
            ..Default::default()
        };
        let mut w = words();
        if jj {
            w.graph = "jj graph";
        }
        cells(
            &presentation(&facts, &[]).help_sections(&w, &[]),
            &[(
                if jj { "jj graph" } else { "Git graph" },
                &[
                    ("j/k / ↑/↓", "move (commit / uncommitted)"),
                    ("g/G", "top/bottom"),
                    ("/", "filter the list"),
                    ("r", "refresh"),
                    (
                        "Tab / Shift-Tab",
                        "cycle changes/log/graph/branches/worktrees",
                    ),
                    ("h/q/Esc", "back to changes"),
                ],
            )],
        );
    }
}

#[test]
fn project_menus_preserve_order_keys_and_gates() {
    let bindings = [
        binding(0, None, Some('z'), true),
        binding(1, None, Some('a'), true),
        binding(2, None, None, false),
        binding(3, None, None, true),
        binding(4, None, Some('q'), true),
    ];
    let p = presentation(&ProjectPresentationFacts::default(), &bindings);
    let copy: Vec<_> = p
        .copy_menu()
        .iter()
        .map(|i| (i.key, i.action, i.label.as_str()))
        .collect();
    assert_eq!(
        copy,
        [
            (KeyPress::ch('z'), 0, "item 0"),
            (KeyPress::ch('a'), 1, "item 1"),
            (KeyPress::ch('q'), 4, "item 4")
        ]
    );
    let actions: Vec<_> = p
        .actions_menu()
        .iter()
        .map(|i| (i.key, i.action, i.label.as_str()))
        .collect();
    assert_eq!(actions, [(KeyPress::ch('b'), 3, "item 3")]);
    assert_eq!(p.copy_action(KeyPress::ch('q')), None);
    assert_eq!(p.copy_action(KeyPress::ch('z')), Some(0));
}

#[test]
fn project_help_back_and_filter_rows_follow_the_hosts_keys() {
    let navigation_rows = |facts: &ProjectPresentationFacts| -> Vec<String> {
        let sections = presentation(facts, &[]).help_sections(&words(), &[]);
        let project = sections.iter().find(|s| s.title == "Project").unwrap();
        project.rows.iter().map(|(k, _)| k.clone()).collect()
    };
    let list = |motions: ProjectMotionKeys| ProjectPresentationFacts {
        content: Some(ProjectPresentationContent::List),
        filter: true,
        pushed: true,
        motions,
        ..Default::default()
    };
    // The default keymap: `q / Esc` leaves and `/` filters.
    let default = navigation_rows(&list(default_keymap_facts().motions));
    assert!(default.contains(&"q / Esc".to_string()), "{default:?}");
    assert!(default.contains(&"/".to_string()), "{default:?}");
    // Moved: the rows show the new keys and no stale `q` / `/`.
    let mut moved = default_keymap_facts().motions;
    moved.back = vec![KeyPress::ch('x')];
    moved.filter = vec![KeyPress::ch('f')];
    let rows = navigation_rows(&list(moved));
    assert!(rows.contains(&"x / Esc".to_string()), "{rows:?}");
    assert!(rows.contains(&"f".to_string()), "{rows:?}");
    assert!(!rows.contains(&"q / Esc".to_string()), "{rows:?}");
    assert!(!rows.contains(&"/".to_string()), "{rows:?}");
    // Unbound: only Esc is left for back, and the filter row is gone.
    let mut unbound = default_keymap_facts().motions;
    unbound.back.clear();
    unbound.filter.clear();
    let rows = navigation_rows(&list(unbound));
    assert!(rows.contains(&"Esc".to_string()), "{rows:?}");
    assert!(
        !rows.iter().any(|r| r == "/" || r.starts_with("q")),
        "{rows:?}"
    );
}
