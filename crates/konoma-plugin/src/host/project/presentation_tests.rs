use super::presentation::*;
use super::test_facts::{default_keymap_facts, default_keymap_motions};
use crate::host::acts::ActBinding;
use crate::{HintGroup, Tr};
use crossterm::event::KeyCode;
use konoma_host_core::keymap::KeyPress;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

fn words(ja: bool) -> ProjectPresentationWords<'static> {
    if ja {
        ProjectPresentationWords {
            language: "ja",
            screens: "画面",
            move_list: "移動",
            scroll: "スクロール",
            mark: "マーク",
            filter: "絞り込み",
            back: "戻る",
            help: "ヘルプ",
            new_tab: "別タブ",
            copy: "コピー",
            actions: "操作",
            hscroll: "横移動",
            ends: "先頭/末尾",
            diff_layout: "縦/横/Auto",
            cycle: "プロジェクトの画面を切り替える",
            pop: "画面を一つ戻るか元の表示に戻る",
            apply: "適用",
            cancel: "取消",
            nav: "移動",
            search: "検索",
            half_page: "半ページ",
            page: "ページ",
            line_ends: "行頭/行末",
            close: "閉じる",
            tool: "ツール",
        }
    } else {
        ProjectPresentationWords {
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
        }
    }
}

fn binding(
    index: usize,
    key: Option<KeyPress>,
    label: &str,
    priority: u8,
) -> ProjectPresentationBinding {
    ProjectPresentationBinding {
        binding: ActBinding {
            copy_key: None,
            hint_group: None,
            act: index,
            key,
            context: None,
            label: Tr::new(label.to_string()),
            help: Tr::new(""),
            footer: Some(priority),
            writes: false,
        },
        enabled: true,
        declared_enter: key == Some(KeyPress::key(KeyCode::Enter)),
    }
}

fn open(label: &str) -> ProjectOpenFacts {
    ProjectOpenFacts {
        label: label.into(),
        right_aliases: vec![KeyPress::ch('l'), KeyPress::key(KeyCode::Right)],
    }
}

fn build(
    facts: &ProjectPresentationFacts,
    bindings: &[ProjectPresentationBinding],
    ja: bool,
    tabs: bool,
) -> ProjectPresentation {
    ProjectPresentation::build(
        facts,
        ProjectCapabilities { can_open_tab: tabs },
        bindings,
        &words(ja),
    )
}

// Literal rows assert the host's dim footer, including the blank remainder. The expected
// buffer never calls the presentation or its fitting adapter.
#[track_caller]
fn cells(presentation: &ProjectPresentation, width: u16, reference: &str) {
    let tokens: Vec<_> = presentation
        .footer_tokens()
        .iter()
        .map(|t| t.text.clone())
        .collect();
    assert!(tokens.iter().any(|t| !t.is_empty()));
    let area = Rect::new(3, 2, width, 2);
    let mut actual = Buffer::empty(area);
    Paragraph::new(Line::from(
        Span::raw(konoma_widgets::help_panel::fit_tokens(&tokens, width)).dim(),
    ))
    .render(area, &mut actual);
    let mut expected = Buffer::empty(area);
    expected.set_string(
        area.x,
        area.y,
        reference,
        Style::default().add_modifier(Modifier::DIM),
    );
    assert_eq!(actual.area, area);
    assert_eq!(expected.area, area);
    let mut compared = 0;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let a = &actual[(x, y)];
            let e = &expected[(x, y)];
            assert_eq!(
                (a.symbol(), a.fg, a.bg, a.modifier),
                (e.symbol(), e.fg, e.bg, e.modifier),
                "cell ({x},{y}), width={width}"
            );
            compared += 1;
        }
    }
    assert_eq!(compared, usize::from(width) * 2);
    if !reference.is_empty() {
        assert!(actual.content.iter().any(|c| c.symbol() != " "));
    }
}

#[test]
fn vcs_shared_footer_controls_fit_individually_at_48_and_90_columns() {
    use crate::project::{ProjectDecl, ProjectScreen};
    let declaration = ProjectDecl::new()
        .version_control()
        .screen(ProjectScreen::new("changes", "Changes").row_kind("file"));
    let screen = &declaration.screens[0];
    for ja in [false, true] {
        let facts = ProjectPresentationFacts {
            content: Some(ProjectPresentationContent::List),
            filter: screen.filter,
            marks: screen.marks,
            open: Some(open(if ja { "差分" } else { "diff" })),
            row_target: true,
            ..Default::default()
        };
        let p = build(&facts, &group_bindings(false, false, false), ja, true);
        let tokens: Vec<_> = p.footer_tokens().iter().map(|t| t.text.clone()).collect();
        let first = p.controls()[0].render();
        for width in [90, 48] {
            let fitted = konoma_widgets::help_panel::fit_tokens(&tokens, width);
            assert!(
                !fitted.is_empty(),
                "narrow VCS footer must keep its first control at {width} columns"
            );
            assert!(
                fitted.starts_with(&first),
                "first VCS control at {width}: {fitted}"
            );
            assert!(
                fitted.ends_with(" …"),
                "remaining VCS controls at {width}: {fitted}"
            );
        }
    }
}

#[test]
fn project_footer_cells_preserve_tokens_order_and_spacing() {
    for ja in [false, true] {
        let label = if ja { "開く  対象" } else { "open  entry" };
        let facts = ProjectPresentationFacts {
            content: Some(ProjectPresentationContent::List),
            marks: true,
            filter: true,
            row_target: true,
            open: Some(open(label)),
            ..default_keymap_facts()
        };
        let mut bindings: Vec<_> = [
            (7, "retry-refresh"),
            (5, "retry-input"),
            (6, "retry-confirmed"),
            (2, "row-prompt"),
            (3, "conditional"),
            (4, "conditional-input"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (n, label))| {
            binding(
                i,
                Some(KeyPress {
                    code: KeyCode::F(n),
                    ctrl: true,
                }),
                label,
                1,
            )
        })
        .collect();
        bindings.push(binding(6, Some(KeyPress::key(KeyCode::Enter)), label, 1));
        bindings.push(binding(7, Some(KeyPress::ch('x')), "capture", 1));
        let p = build(&facts, &bindings, ja, true);
        if ja {
            cells(&p, 160, "Tab/Shift-Tab:画面  j/k:移動  V:マーク  /:絞り込み  h/q:戻る  ?:ヘルプ  ↵/l/→:開く  対象  Ctrl-t:別タブ  Space:操作  C-F7:retry-refresh  C-F5:retry-input …");
            cells(&p, 240, "Tab/Shift-Tab:画面  j/k:移動  V:マーク  /:絞り込み  h/q:戻る  ?:ヘルプ  ↵/l/→:開く  対象  Ctrl-t:別タブ  Space:操作  C-F7:retry-refresh  C-F5:retry-input  C-F6:retry-confirmed  C-F2:row-prompt  C-F3:conditional  C-F4:conditional-input …");
        } else {
            cells(&p, 160, "Tab/Shift-Tab:screens  j/k:move  V:mark  /:filter  h/q:back  ?:help  ↵/l/→:open  entry  Ctrl-t:new tab  Space:Actions  C-F7:retry-refresh  C-F5:retry-input …");
            cells(&p, 240, "Tab/Shift-Tab:screens  j/k:move  V:mark  /:filter  h/q:back  ?:help  ↵/l/→:open  entry  Ctrl-t:new tab  Space:Actions  C-F7:retry-refresh  C-F5:retry-input  C-F6:retry-confirmed  C-F2:row-prompt  C-F3:conditional  C-F4:conditional-input …");
        }
        cells(
            &p,
            28,
            if ja {
                "Tab/Shift-Tab:画面 …"
            } else {
                "Tab/Shift-Tab:screens …"
            },
        );
        cells(
            &p,
            80,
            if ja {
                "Tab/Shift-Tab:画面  j/k:移動  V:マーク  /:絞り込み  h/q:戻る  ?:ヘルプ …"
            } else {
                "Tab/Shift-Tab:screens  j/k:move  V:mark  /:filter  h/q:back  ?:help …"
            },
        );
        let open = p
            .footer_tokens()
            .iter()
            .find(|token| token.keys.contains(&KeyPress::key(KeyCode::Enter)))
            .unwrap();
        assert_eq!(open.ops, [PresentationOp::ProjectAction(6)]);
        assert_eq!(
            open.keys
                .iter()
                .filter(|k| k.code == KeyCode::Enter)
                .count(),
            1
        );
        assert_eq!(
            p.footer_priority(),
            &[6, 0, 1, 2, 3, 4, 5, 7].map(PresentationOp::ProjectAction)
        );
    }
    let p = build(
        &default_keymap_facts(),
        &group_bindings(false, false, false),
        false,
        true,
    );
    cells(&p, 240, "Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help  Space:Actions  C-F2:row-prompt  a:screen  z:screen-shortcut  C-F8:hint-before  s/S:stage(all)  C-F9:hint-between  u/U:unstage(all)  C-F10:hint-after");
    let mut duplicate = group_bindings(false, false, false);
    duplicate[6].binding.key = duplicate[4].binding.key;
    let p = build(&default_keymap_facts(), &duplicate, false, true);
    cells(&p,240,"Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help  Space:Actions  C-F2:row-prompt  a:screen  z:screen-shortcut  C-F8:hint-before  s:stage(all)  C-F9:hint-between  u/U:unstage(all)  C-F10:hint-after");
    let grouped = p
        .footer_tokens()
        .iter()
        .find(|t| t.text == "s:stage(all)")
        .unwrap();
    assert_eq!(grouped.keys, [KeyPress::ch('s')]);
    assert_eq!(
        grouped.ops,
        [
            PresentationOp::ProjectAction(4),
            PresentationOp::ProjectAction(6)
        ]
    );
}

fn group_bindings(disabled: bool, empty: bool, remap: bool) -> Vec<ProjectPresentationBinding> {
    let mut bindings: Vec<_> = [
        (
            KeyPress {
                code: KeyCode::F(2),
                ctrl: true,
            },
            "row-prompt",
            1,
        ),
        (KeyPress::ch('a'), "screen", 1),
        (KeyPress::ch('z'), "screen-shortcut", 1),
        (
            KeyPress {
                code: KeyCode::F(8),
                ctrl: true,
            },
            "hint-before",
            0,
        ),
        (KeyPress::ch('s'), "stage", 8),
        (
            KeyPress {
                code: KeyCode::F(9),
                ctrl: true,
            },
            "hint-between",
            4,
        ),
        (KeyPress::ch(if remap { 'W' } else { 'S' }), "stage all", 2),
        (KeyPress::ch('u'), "unstage", 9),
        (KeyPress::ch('U'), "unstage all", 9),
        (
            KeyPress {
                code: KeyCode::F(10),
                ctrl: true,
            },
            "hint-after",
            10,
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (key, label, priority))| binding(i, Some(key), label, priority))
    .collect();
    for i in [4, 6, 7, 8] {
        let group = if i < 7 { "stage" } else { "unstage" };
        bindings[i].binding.hint_group = Some((
            "fixture".into(),
            HintGroup::new(group, format!("{group}(all)")),
        ));
    }
    bindings[4].enabled = !disabled && !empty;
    bindings[6].enabled = !empty;
    bindings
}

#[test]
fn project_footer_uses_resolved_keys_and_enabled_members() {
    for (disabled, empty, remap, expected) in [
        (true,false,false,"Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help  Space:Actions  C-F2:row-prompt  a:screen  z:screen-shortcut  C-F8:hint-before  S:stage(all)  C-F9:hint-between  u/U:unstage(all)  C-F10:hint-after"),
        (false,true,false,"Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help  Space:Actions  C-F2:row-prompt  a:screen  z:screen-shortcut  C-F8:hint-before  C-F9:hint-between  u/U:unstage(all)  C-F10:hint-after"),
        (false,false,true,"Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help  Space:Actions  C-F2:row-prompt  a:screen  z:screen-shortcut  C-F8:hint-before  s/W:stage(all)  C-F9:hint-between  u/U:unstage(all)  C-F10:hint-after"),
    ] {
        cells(&build(&default_keymap_facts(), &group_bindings(disabled,empty,remap), false, true), 240, expected);
    }
    let mut unbound = group_bindings(false, false, false);
    unbound[4].binding.key = None;
    cells(&build(&default_keymap_facts(),&unbound,false,true),240,"Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help  Space:Actions  C-F2:row-prompt  a:screen  z:screen-shortcut  C-F8:hint-before  S:stage(all)  C-F9:hint-between  u/U:unstage(all)  C-F10:hint-after");
    // Both schemes resolve the footer's three motion groups to these keys. Their half/page
    // controls differ, remain in help, and are supplied by the host rather than inferred here.
    for less in [false, true] {
        let facts = ProjectPresentationFacts {
            content: Some(ProjectPresentationContent::Lines),
            motions: ProjectMotionKeys {
                scroll: vec![KeyPress::ch('Z'), KeyPress::ch('k')],
                hscroll: vec![KeyPress::ch('h'), KeyPress::ch('l')],
                ends: vec![KeyPress::ch('g'), KeyPress::ch('G')],
                half_page: vec![if less {
                    KeyPress::ch('d')
                } else {
                    KeyPress::ctrl_ch('d')
                }],
                ..default_keymap_motions()
            },
            ..Default::default()
        };
        let p = build(&facts, &[], false, true);
        cells(&p, 160, "Z/k:scroll  h/l:hscroll  g/G:ends  Tab/Shift-Tab:Cycle project screens  q/Esc:Pop this screen or return to the previous view  ?:help");
        assert_eq!(
            p.additional_text_help().unwrap(),
            [(if less { "d" } else { "Ctrl-d" }.into(), "half page".into())]
        );
        for (keys, expected, narrow) in [
            (vec![KeyPress::ch('S')], "j/k:scroll  h/l:hscroll  S:unified/split/auto  g/G:ends  Tab/Shift-Tab:Cycle project screens  q/Esc:Pop this screen or return to the previous view  ?:help", "j/k:scroll  h/l:hscroll  S:unified/split/auto  g/G:ends …"),
            (vec![], "j/k:scroll  h/l:hscroll  g/G:ends  Tab/Shift-Tab:Cycle project screens  q/Esc:Pop this screen or return to the previous view  ?:help", "j/k:scroll  h/l:hscroll  g/G:ends  Tab/Shift-Tab:Cycle project screens …"),
        ] {
            let mut diff = facts.clone();
            diff.content = Some(ProjectPresentationContent::Diff);
            diff.motions.scroll[0] = KeyPress::ch('j');
            diff.motions.diff_layout = keys;
            cells(&build(&diff, &[], false, true), 160, expected);
            cells(&build(&diff, &[], false, true), 80, narrow);
        }
    }
}

#[test]
fn project_footer_capability_follows_selected_target() {
    for ja in [false, true] {
        for (target, tabs) in [(false, true), (true, false), (false, false)] {
            let facts = ProjectPresentationFacts {
                row_target: target,
                ..default_keymap_facts()
            };
            let p = build(&facts, &[], ja, tabs);
            assert!(p.new_tab_control().is_none());
            cells(
                &p,
                80,
                if ja {
                    "Tab/Shift-Tab:画面  j/k:スクロール  q:戻る  ?:ヘルプ"
                } else {
                    "Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help"
                },
            );
        }
        let facts = ProjectPresentationFacts {
            content: Some(ProjectPresentationContent::List),
            filter: true,
            open: Some(open(if ja { "差分" } else { "diff" })),
            row_target: true,
            ..default_keymap_facts()
        };
        let mut bindings = vec![
            binding(0, Some(KeyPress::ch('s')), "stage", 1),
            binding(1, Some(KeyPress::ch('S')), "stage all", 1),
            binding(2, Some(KeyPress::ch('u')), "unstage", 1),
            binding(3, Some(KeyPress::ch('U')), "unstage all", 1),
            binding(4, Some(KeyPress::ch('x')), "discard", 1),
            binding(5, Some(KeyPress::ch('c')), "commit", 1),
        ];
        for (indices, id, en, jp) in [
            ([0, 1], "stage", "stage(all)", "ステージ(全)"),
            ([2, 3], "unstage", "unstage(all)", "解除(全)"),
        ] {
            for i in indices {
                bindings[i].binding.hint_group =
                    Some(("fixture".into(), HintGroup::new(id, Tr::new(en).ja(jp))));
            }
        }
        bindings[4].binding.label = Tr::new("discard").ja("破棄");
        bindings[5].binding.label = Tr::new("commit").ja("コミット");
        let p = build(&facts, &bindings, ja, true);
        assert!(p.new_tab_control().is_some());
        cells(
            &p,
            160,
            if ja {
                "Tab/Shift-Tab:画面  j/k:移動  /:絞り込み  h/q:戻る  ?:ヘルプ  ↵/l/→:差分  Ctrl-t:別タブ  Space:操作  s/S:ステージ(全)  u/U:解除(全)  x:破棄  c:コミット"
            } else {
                "Tab/Shift-Tab:screens  j/k:move  /:filter  h/q:back  ?:help  ↵/l/→:diff  Ctrl-t:new tab  Space:Actions  s/S:stage(all)  u/U:unstage(all)  x:discard  c:commit"
            },
        );
    }
}

#[test]
fn picker_and_graph_footer_cells_match_reference() {
    for ja in [false, true] {
        let mut bindings = vec![
            binding(0, Some(KeyPress::ch('x')), "toggle", 1),
            binding(1, Some(KeyPress::ch('J')), "up", 2),
            binding(2, Some(KeyPress::ch('K')), "down", 2),
            binding(3, Some(KeyPress::ch('a')), "all", 3),
            binding(4, Some(KeyPress::ch('c')), "current only", 4),
            binding(5, Some(KeyPress::key(KeyCode::Enter)), "duplicate Enter", 0),
        ];
        for i in [1, 2] {
            bindings[i].binding.hint_group = Some((
                "fixture".into(),
                HintGroup::new("reorder", Tr::new("reorder").ja("並替")),
            ));
        }
        bindings[0].binding.label = Tr::new("toggle").ja("トグル");
        bindings[3].binding.label = Tr::new("all").ja("全部");
        bindings[4].binding.label = Tr::new("current only").ja("現在のみ");
        let mut facts = ProjectPresentationFacts {
            content: Some(ProjectPresentationContent::List),
            picker: true,
            open: Some(open("apply")),
            ..Default::default()
        };
        let p = build(&facts, &bindings, ja, true);
        cells(
            &p,
            80,
            if ja {
                "x:トグル  J/K:並替  a:全部  c:現在のみ  l/Enter:適用  h/q:取消"
            } else {
                "x:toggle  J/K:reorder  a:all  c:current only  l/Enter:apply  h/q:cancel"
            },
        );
        cells(
            &p,
            28,
            if ja {
                "x:トグル  J/K:並替  a:全部 …"
            } else {
                "x:toggle  J/K:reorder …"
            },
        );
        facts.picker = false;
        facts.graph_jj = Some(false);
        facts.graph_scroll = vec![KeyPress::ch('j'), KeyPress::ch('k')];
        facts.open = Some(open(if ja { "詳細" } else { "detail" }));
        let graph = vec![
            binding(
                0,
                Some(KeyPress::ch('s')),
                if ja { "基準" } else { "base" },
                1,
            ),
            binding(
                1,
                Some(KeyPress::ch('x')),
                if ja { "基準解除" } else { "base off" },
                2,
            ),
            binding(
                2,
                Some(KeyPress::ch('b')),
                if ja { "ブランチ" } else { "branches" },
                3,
            ),
            bindings[5].clone(),
        ];
        let p = build(&facts, &graph, ja, true);
        cells(
            &p,
            160,
            if ja {
                "j/k:移動  l/Enter:詳細  s:基準  x:基準解除  b:ブランチ  /:検索  Tab/⇧Tab:画面  h/q/Esc:戻る"
            } else {
                "j/k:nav  l/Enter:detail  s:base  x:base off  b:branches  /:search  Tab/⇧Tab:screens  h/q/Esc:back"
            },
        );
        cells(
            &p,
            28,
            if ja {
                "j/k:移動  l/Enter:詳細 …"
            } else {
                "j/k:nav  l/Enter:detail …"
            },
        );
        facts.graph_jj = Some(true);
        facts.open = None;
        cells(
            &build(&facts, &[], ja, true),
            80,
            if ja {
                "j/k:移動  /:検索  Tab/⇧Tab:画面  h/q/Esc:戻る"
            } else {
                "j/k:nav  /:search  Tab/⇧Tab:screens  h/q/Esc:back"
            },
        );
    }
}

#[test]
fn project_footer_back_filter_and_help_follow_the_hosts_keys() {
    let list = |motions: ProjectMotionKeys| ProjectPresentationFacts {
        content: Some(ProjectPresentationContent::List),
        filter: true,
        motions,
        ..Default::default()
    };
    // Default keymap, loading screen and list.
    cells(
        &build(&default_keymap_facts(), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:scroll  q:back  ?:help",
    );
    cells(
        &build(&list(default_keymap_motions()), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:move  /:filter  h/q:back  ?:help",
    );
    // Back moved to `x`: the footer shows `h/x:back` (the list's `Left` key stays) and no `q:back`.
    let mut moved = default_keymap_motions();
    moved.back = vec![KeyPress::ch('x')];
    moved.filter = vec![KeyPress::ch('f')];
    moved.help = vec![KeyPress::ch('H')];
    cells(
        &build(&list(moved), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:move  f:filter  h/x:back  H:help",
    );
    // Each control is hidden when its key is unbound.
    let mut no_back = default_keymap_motions();
    no_back.back.clear();
    cells(
        &build(&list(no_back), &[], false, true),
        80,
        // `Left` still goes back on a list, so its token stays when only `back` is cleared.
        "Tab/Shift-Tab:screens  j/k:move  /:filter  h:back  ?:help",
    );
    let mut no_filter = default_keymap_motions();
    no_filter.filter.clear();
    cells(
        &build(&list(no_filter), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:move  h/q:back  ?:help",
    );
    let mut no_help = default_keymap_motions();
    no_help.help.clear();
    cells(
        &build(&list(no_help), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:move  /:filter  h/q:back",
    );
}

/// On a list the scheme's `Left` key goes back like `q`, so the back token names it; a rebound
/// `Left` moves it, a cleared one drops it, and a text or diff screen (where it pans) never names
/// it in the back token.
#[test]
fn project_footer_generic_list_back_token_names_the_left_key() {
    let facts = |content, left: Vec<KeyPress>| {
        let mut motions = default_keymap_motions();
        motions.left = left;
        ProjectPresentationFacts {
            content: Some(content),
            filter: true,
            motions,
            ..Default::default()
        }
    };
    let list = ProjectPresentationContent::List;
    cells(
        &build(&facts(list, vec![KeyPress::ch('h')]), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:move  /:filter  h/q:back  ?:help",
    );
    cells(
        &build(&facts(list, vec![KeyPress::ch('Z')]), &[], true, true),
        80,
        "Tab/Shift-Tab:画面  j/k:移動  /:絞り込み  Z/q:戻る  ?:ヘルプ",
    );
    cells(
        &build(&facts(list, Vec::new()), &[], false, true),
        80,
        "Tab/Shift-Tab:screens  j/k:move  /:filter  q:back  ?:help",
    );
    // `h` pans a text or diff screen: it is not a way back there.
    for content in [
        ProjectPresentationContent::Lines,
        ProjectPresentationContent::Diff,
    ] {
        let presentation = build(&facts(content, vec![KeyPress::ch('h')]), &[], false, true);
        let footer: String = presentation
            .footer_tokens()
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        assert!(!footer.contains("h/q"), "{footer}");
    }
}
