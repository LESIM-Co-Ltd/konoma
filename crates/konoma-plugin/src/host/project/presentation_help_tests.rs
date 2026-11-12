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
            close: "close",
            tool: "tool",
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
        let facts = graph_facts(jj, vec![KeyPress::ch('q')]);
        let mut w = words();
        if jj {
            w.graph = "jj graph";
            w.screens = "cycle changes/log/graph/bookmarks";
        }
        // The v0.31.2 graph help: the graph keys in the legacy order, the leave row `h / Esc  ·  q`,
        // then the move row in its own Movement section.
        cells(
            &presentation(&facts, &[]).help_sections(&w, &[]),
            &graph_cells(jj, "h / Esc  ·  q", w.screens),
        );
    }
}

fn graph_facts(jj: bool, back: Vec<KeyPress>) -> ProjectPresentationFacts {
    let mut facts = default_keymap_facts();
    facts.graph_jj = Some(jj);
    facts.vcs_jj = Some(jj);
    facts.filter = true;
    facts.motions.ends = vec![KeyPress::ch('g'), KeyPress::ch('G')];
    facts.motions.left = vec![KeyPress::ch('h')];
    facts.motions.back = back;
    facts
}

fn graph_cells(
    jj: bool,
    leave: &'static str,
    screens: &'static str,
) -> [(&'static str, &'static [(&'static str, &'static str)]); 2] {
    // The leak keeps the expectation table `'static` like the literal tables of the other tests.
    let rows: &'static [(&'static str, &'static str)] = Box::leak(Box::new([
        ("g / G", "top/bottom"),
        ("/", "filter the list"),
        ("r", "refresh"),
        ("Tab / ⇧Tab", screens),
        (leave, "back to changes"),
    ]));
    [
        (if jj { "jj graph" } else { "Git graph" }, rows),
        ("Movement", &[("j / k", "move (commit / uncommitted)")]),
    ]
}

/// The graph help names the keys that act: moving the back key to `x` shows `x` where `q` was,
/// and the rest of the help (order, other rows) is unchanged. Tab and Shift-Tab are fixed keys of
/// the canvas (`host/project/input.rs` matches `KeyCode::Tab | KeyCode::BackTab`, and
/// `src/ops/keys.rs` declares them per context), so the cycle row cannot move.
#[test]
fn project_graph_help_shows_the_moved_back_key_in_place() {
    for jj in [false, true] {
        let facts = graph_facts(jj, vec![KeyPress::ch('x')]);
        let mut w = words();
        if jj {
            w.graph = "jj graph";
            w.screens = "cycle changes/log/graph/bookmarks";
        }
        cells(
            &presentation(&facts, &[]).help_sections(&w, &[]),
            &graph_cells(jj, "h / Esc  ·  x", w.screens),
        );
    }
}

/// The graph help follows every motion key of the host's keymap: the filter moved to `f` shows
/// `f` and not `/`; with `ends` empty there is no `g / G` row; with `left` empty the back row has
/// no `h`; with the scroll keys empty there is no `j / k` row.
#[test]
fn project_graph_help_follows_the_changed_motions() {
    let rows_of = |facts: &ProjectPresentationFacts| {
        presentation(facts, &[])
            .help_sections(&words(), &[])
            .into_iter()
            .flat_map(|s| s.rows)
            .collect::<Vec<_>>()
    };
    let has_row = |rows: &[(String, String)], description: &str| {
        rows.iter().find(|(_, d)| d == description).cloned()
    };
    let base = graph_facts(false, vec![KeyPress::ch('q')]);
    let rows = rows_of(&base);
    assert_eq!(has_row(&rows, "filter the list").unwrap().0, "/");
    assert!(has_row(&rows, "top/bottom").is_some());
    assert!(rows.iter().any(|(k, _)| k == "j / k"));
    // The filter key moved to `f`.
    let mut moved = base.clone();
    moved.motions.filter = vec![KeyPress::ch('f')];
    let rows = rows_of(&moved);
    assert_eq!(has_row(&rows, "filter the list").unwrap().0, "f");
    assert!(rows.iter().all(|(k, _)| k != "/"), "no stale `/`: {rows:?}");
    // `ends` unbound: no top/bottom row.
    let mut no_ends = base.clone();
    no_ends.motions.ends.clear();
    let rows = rows_of(&no_ends);
    assert!(has_row(&rows, "top/bottom").is_none(), "{rows:?}");
    assert!(rows.iter().all(|(k, _)| k != "g / G"), "{rows:?}");
    // `left` unbound: the back row remains, without `h`.
    let mut no_left = base.clone();
    no_left.motions.left.clear();
    let rows = rows_of(&no_left);
    let back = has_row(&rows, "back to changes").expect("the back row stays");
    assert!(!back.0.contains('h'), "back row without h: {back:?}");
    assert!(back.0.contains('q'), "back row keeps q: {back:?}");
    // The scroll keys unbound: no `j / k` row.
    let mut no_scroll = base.clone();
    no_scroll.motions.scroll.clear();
    let rows = rows_of(&no_scroll);
    assert!(
        has_row(&rows, "move (commit / uncommitted)").is_none(),
        "{rows:?}"
    );
    assert!(rows.iter().all(|(k, _)| k != "j / k"), "{rows:?}");
}

/// The cycle row of a version-control project lists the screens that project cycles: git has
/// worktrees, jj has none (and lists bookmarks instead). A project that is not version control
/// keeps the generic wording.
#[test]
fn project_cycle_help_row_lists_only_the_screens_of_the_backend() {
    let rows_of = |vcs_jj: Option<bool>| {
        let mut facts = default_keymap_facts();
        facts.content = Some(ProjectPresentationContent::List);
        facts.vcs_jj = vcs_jj;
        let mut w = words();
        w.screens = match vcs_jj {
            Some(true) => "cycle changes/log/graph/bookmarks",
            _ => "cycle changes/log/graph/branches/worktrees",
        };
        presentation(&facts, &[])
            .help_sections(&w, &[])
            .into_iter()
            .flat_map(|s| s.rows)
            .filter(|(keys, _)| keys.contains("Tab"))
            .collect::<Vec<_>>()
    };
    let row = |description: &str| vec![("Tab / ⇧Tab".to_string(), description.to_string())];
    assert_eq!(
        rows_of(Some(false)),
        row("cycle changes/log/graph/branches/worktrees")
    );
    let jj = rows_of(Some(true));
    assert_eq!(jj, row("cycle changes/log/graph/bookmarks"));
    assert!(!jj[0].1.contains("worktree"));
    assert_eq!(rows_of(None), row("Cycle project screens"));
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
