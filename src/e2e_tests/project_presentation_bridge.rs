//! Full rectangles from the real host, projected by operation capabilities for the standalone.
use super::*;
use konoma_plugin::host::project::presentation::*;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Stylize,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use serde_json::{json, Value};

#[path = "../../crates/konoma-plugin/tests/app/project/presentation_fixture.rs"]
mod plugin;

fn fixture() -> (Sim, crate::test_support::TmpDir, Arc<Mutex<plugin::Log>>) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = sandbox("project_presentation_bridge");
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let log = Arc::new(Mutex::new(plugin::Log::default()));
    let name = format!(
        "project_presentation_bridge_{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let marker = test_plugins::register(&name, Arc::new(plugin::PresentationPlugin(log.clone())));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.external.git = false;
    let mut sim = Sim::with_config_sized(&canon(&dir), cfg, 100, 32);
    sim.app.poll_projects();
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    drain(&mut sim);
    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active());
    (sim, dir, log)
}

fn presentation(sim: &Sim) -> ProjectPresentation {
    let (host, _) = sim.app.project_presentation().unwrap();
    presentation_from(sim, host.facts_for_test())
}

fn presentation_from(sim: &Sim, facts: &ProjectPresentationFacts) -> ProjectPresentation {
    use crate::keymap::{Action, Context};
    let app = &sim.app;
    let bindings = app
        .plugin_context_bindings(Context::Canvas)
        .into_iter()
        .enumerate()
        .map(|(index, b)| ProjectPresentationBinding {
            enabled: crate::ops::action_enabled(app, Context::Canvas, Action::Plugin(b.act)),
            declared_enter: app.plugin_keys.declaration(b.act).unwrap().key.as_deref()
                == Some("Enter"),
            binding: konoma_plugin::host::acts::ActBinding {
                act: index,
                copy_key: b.copy_key,
                hint_group: b.hint_group,
                key: b.key,
                context: b.context,
                label: b.label,
                help: b.help,
                footer: b.footer,
                writes: b.writes,
            },
        })
        .collect::<Vec<_>>();
    let back = if app.tab.project_navigation.as_ref().unwrap().stack.len() > 1 {
        "back"
    } else {
        "quit"
    };
    // These two operation facts are deliberately different: no NewTab executor, and root exits.
    ProjectPresentation::build(
        facts,
        ProjectCapabilities {
            can_open_tab: false,
        },
        &bindings,
        &ProjectPresentationWords {
            language: "en",
            screens: "screens",
            move_list: "move",
            scroll: "scroll",
            mark: "mark",
            filter: "filter",
            back,
            help: "help",
            new_tab: "new tab",
            copy: "copy",
            actions: "Actions",
            hscroll: "hscroll",
            ends: "ends",
            diff_layout: "unified/split/auto",
            cycle: "Cycle project screens",
            pop: back,
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

fn cells(sim: &mut Sim, names: &[&str]) -> Value {
    let p = presentation(sim);
    let mut buffer = sim.term.backend().buffer().clone();
    let footer = Rect::new(0, 31, 100, 1);
    for x in 0..100 {
        buffer[(x, 31)].reset();
    }
    let spans = if let Some(flash) = &sim.app.flash {
        konoma_widgets::footer_line::flash_spans(flash)
    } else if names.last() == Some(&"y") || names.last() == Some(&"Space") {
        let copy = names.last() == Some(&"y");
        let items = if copy {
            p.copy_menu()
        } else {
            p.actions_menu()
        };
        konoma_widgets::footer_line::which_key_spans(
            if copy { "copy" } else { "Actions" },
            &items
                .iter()
                .map(|i| (konoma_host_core::help::help_key(i.key), i.label.clone()))
                .collect::<Vec<_>>(),
            100,
        )
    } else if sim.app.show_help {
        // Help footer is capability-independent and already rendered by konoma.
        Vec::new()
    } else {
        vec![Span::raw(konoma_widgets::help_panel::fit_tokens(
            &p.footer_tokens()
                .iter()
                .map(|t| t.text.clone())
                .collect::<Vec<_>>(),
            100,
        ))
        .dim()]
    };
    if sim.app.show_help {
        buffer = sim.term.backend().buffer().clone();
        // Re-render the underlying host without the overlay, then the project-only help.
        sim.app.show_help = false;
        sim.draw();
        let body = sim.term.backend().buffer().clone();
        for y in 1..31 {
            for x in 0..100 {
                buffer[(x, y)] = body[(x, y)].clone();
            }
        }
        sim.app.show_help = true;
        let back = if sim.app.tab.project_navigation.as_ref().unwrap().stack.len() > 1 {
            "back"
        } else {
            "quit"
        };
        let sections = p.help_sections(
            &ProjectHelpWords {
                plugin: "Presentation",
                project: "Project",
                picker: "Choose a branch",
                graph: "Git graph",
                new_tab: "Open in a new tab",
                move_list: "move",
                move_commit: "move (commit / uncommitted)",
                ends: "top / bottom",
                apply: "apply",
                cancel: "cancel",
                filter: "filter by name",
                mark: "Toggle the selected row's mark",
                search: "filter the rows by the text they show",
                refresh: "Refresh",
                screens: "Cycle project screens",
                movement: "Movement",
                back_to_changes: back,
                cycle: "Cycle project screens",
                back,
                scroll: "scroll",
                copy: "copy",
                actions: "Actions",
            },
            &[("i".into(), "Items".into())],
        );
        let lines = konoma_widgets::help_panel::help_lines(
            &sections
                .iter()
                .map(|s| (s.title.as_str(), s.rows.as_slice()))
                .collect::<Vec<_>>(),
        );
        konoma_widgets::help_panel::render_panel(
            &mut buffer,
            Rect::new(0, 0, 100, 32),
            lines,
            0,
            crate::ops::core_help::help_title(&sim.app),
        );
    } else {
        Paragraph::new(Line::from(spans)).render(footer, &mut buffer);
    }
    encode(&buffer)
}

fn encode(buffer: &Buffer) -> Value {
    assert_eq!(buffer.area, Rect::new(0, 0, 100, 32));
    let mut runs: Vec<Value> = Vec::new();
    let mut count = 0;
    let mut nonempty = 0;
    for c in &buffer.content {
        count += 1;
        nonempty += usize::from(!c.symbol().trim().is_empty());
        let value = json!([
            c.symbol(),
            format!("{:?}", c.fg),
            format!("{:?}", c.bg),
            c.modifier.bits()
        ]);
        if let Some(last) = runs.last_mut().filter(|r| r[1] == value) {
            last[0] = json!(last[0].as_u64().unwrap() + 1);
        } else {
            runs.push(json!([1, value]));
        }
    }
    assert_eq!(count, 3200);
    assert!(nonempty > 0);
    json!({"rect":[0,0,100,32],"count":count,"runs":runs})
}

#[test]
fn project_presentation_matches_frozen_host_cells_with_standalone_capabilities() {
    let sequences: &[&[&str]] = &[
        &["?"],
        &["y"],
        &["y", "c"],
        &["Space"],
        &["Space", "c"],
        &["Enter"],
        &[],
    ];
    let mut stages = Vec::new();
    for names in sequences {
        let (mut sim, _dir, log) = fixture();
        for name in *names {
            match *name {
                "Enter" => sim.enter(),
                "Space" => sim.key(' '),
                _ => sim.key(name.chars().next().unwrap()),
            };
            drain(&mut sim);
        }
        let mut stage = cells(&mut sim, names);
        stage["keys"] = json!(names);
        stage["acts"] = json!(log.lock().unwrap().acts);
        stages.push(stage);
    }
    let mut notices = Vec::new();
    for index in 0..5 {
        let (mut sim, _dir, log) = fixture();
        if index < 3 {
            log.lock().unwrap().outcome = Some(match index {
                0 => Outcome::Copy {
                    text: "alpha".into(),
                },
                1 => Outcome::Flash("visible flash".into()),
                // The four navigation executors are absent on the standalone surface. Their
                // notice is drawn through the host's Flash outcome, without running a detour.
                _ => Outcome::Flash("this action is not supported in the standalone app".into()),
            });
            sim.enter();
        } else {
            log.lock().unwrap().chip_failure = true;
            if index == 4 {
                // Another host tab retains the serial owner while the foreground no longer
                // lists it. This exercises ChipError delivery, rather than dropping its owner.
                sim.app.tab_new().unwrap();
                sim.app.tab.projects.wait_for_result();
                sim.app.poll_projects();
                drain(&mut sim);
                sim.app.tab_goto(0);
                sim.app.tab.projects.projects.clear();
            }
            for runtime in sim.app.project_runtimes.values_mut() {
                runtime.chip_dirty = true;
            }
            sim.app.poll_projects();
        }
        drain(&mut sim);
        assert_eq!(
            sim.app.flash.as_deref(),
            match index {
                0 => Some("copied: alpha"),
                1 => Some("visible flash"),
                2 => Some("this action is not supported in the standalone app"),
                3 => Some("chip failed"),
                _ => None,
            }
        );
        notices.push(cells(&mut sim, &[]));
    }
    let actual = json!({"stages":stages,"notices":notices});
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots/project_presentation.json");
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(actual, expected);
}

/// The presentation tests of konoma-plugin start from `default_keymap_motions()`, a hand copy of
/// what this host passes with no config. Pin the copy against the real host so a changed default
/// key fails here instead of leaving those tests green on stale keys.
#[test]
fn project_default_motion_keys_equal_the_plugin_test_helper_copy() {
    let (sim, _dir, _log) = fixture();
    let (host, _) = sim.app.project_presentation().unwrap();
    let facts = host.facts_for_test();
    assert!(
        matches!(facts.content, Some(ProjectPresentationContent::List)),
        "the helper copy models a list screen"
    );
    let helper = konoma_plugin::host::project::test_facts::default_keymap_motions();
    // The host also lists arrows and uppercase aliases for scrolling; the plain keys are the
    // ones the canvas hints show.
    assert_eq!(facts.motions.plain_scroll(), helper.scroll);
    assert_eq!(facts.motions.back, helper.back);
    assert_eq!(facts.motions.filter, helper.filter);
    assert_eq!(facts.motions.help, helper.help);
    // The groups the helper leaves empty (horizontal scroll, ends, half page, page, line ends,
    // diff layout) must not matter on a list screen: with only the helper's motions, the
    // controls and the footer are the same as with the host's full set.
    let mut helper_facts = facts.clone();
    helper_facts.motions = helper;
    let from_host = presentation_from(&sim, facts);
    let from_helper = presentation_from(&sim, &helper_facts);
    assert!(!from_host.controls().is_empty());
    assert_eq!(
        format!("{:?}", from_host.controls()),
        format!("{:?}", from_helper.controls())
    );
    assert_eq!(
        from_host
            .footer_tokens()
            .iter()
            .map(|t| t.text.clone())
            .collect::<Vec<_>>(),
        from_helper
            .footer_tokens()
            .iter()
            .map(|t| t.text.clone())
            .collect::<Vec<_>>()
    );
}
