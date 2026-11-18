// Frozen from konoma output at 28ad62ce before diff presentation extraction; never regenerate from the shared implementation.
use super::*;
use crate::config::Config;
use crate::keymap::{Context, LeaderId};
use konoma_plugin::host::project::media_diff::{
    MediaBase, MediaDiffComputed, MediaDiffKind, MediaDiffOutcome, MediaDiffPicture,
    MediaDiffResult, MediaDiffSide,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Widget},
};
use serde_json::{json, Value};

fn app(case: &Value) -> (App, crate::test_support::TmpDir) {
    let dir = crate::test_support::unique_tmp("diff_presentation_agreement");
    std::fs::create_dir_all(&dir).unwrap();
    crate::test_support::init_git_repo(&dir);
    let mut config = Config::default();
    config.ui.lang = case["language"].as_str().unwrap().into();
    config.ui.keys = case["scheme"].as_str().unwrap().into();
    config.ui.wrap = case["wrap"].as_bool().unwrap();
    config.git.diff = "unified".into();
    if case["backend"] != "git" {
        config
            .plugins
            .entry("git".into())
            .or_default()
            .insert("read_only".into(), toml::Value::Boolean(true));
    }
    if case["remap"] == true {
        config.keys.surfaces.insert(
            "diff".into(),
            [
                ("s".into(), "none".into()),
                ("z".into(), "cycle_diff_layout".into()),
                (", p".into(), "toggle_follow_diff_scope".into()),
                (", f".into(), "cycle_path_style".into()),
            ]
            .into_iter()
            .collect(),
        );
    }
    let mut app = App::new(dir.to_path_buf(), config).unwrap();
    let surface = case["surface"].as_str().unwrap();
    let path = dir.join(match surface {
        "source" | "rendered" => "doc.md",
        "pending" | "ready" => "picture.pdf",
        _ => "data.bin",
    });
    app.open_git_diff(&path);
    app.tab.diff_view = if surface == "source" || surface == "summary" {
        DiffView::Source
    } else {
        DiffView::Rendered
    };
    app.diff_follow_scope = true;
    app.follow_diff_full = false;
    // Model the cached backend and its write policy without depending on an installed jj binary.
    if case["backend"] == "jj" {
        app.git_vcs = crate::vcs::VcsKind::Jj;
    }
    if surface == "source" {
        app.diff_cache = Some(DiffCache {
            path: path.clone(),
            lines: vec![crate::git::DiffLine {
                kind: crate::git::DiffLineKind::Added,
                old_no: None,
                new_no: Some(1),
                text: "new".into(),
            }],
        });
    }
    if surface == "ready" {
        app.media_diff = Default::default();
        let picture = MediaDiffPicture {
            natural_px: (20, 30),
            bytes: 100,
            page_count: Some(3),
            cache_key: dir.join("picture"),
        };
        assert!(app.media_diff.apply(
            MediaDiffResult {
                gen: 0,
                path,
                page: 1,
                raster_px: (200, 200),
                computed: MediaDiffComputed::Unavailable
            },
            |_| {
                MediaDiffOutcome::Ready {
                    kind: MediaDiffKind::Pdf,
                    base: MediaBase::Head,
                    same_bytes: false,
                    old: MediaDiffSide::Absent,
                    new: MediaDiffSide::Picture(picture),
                }
            }
        ));
        assert!(app.media_diff_showing_pictures());
        assert!(app.media_diff_can_page());
    }
    (app, dir)
}

fn host_output(app: &App) -> Value {
    let sections = crate::ops::help::sections_for(app, Context::Diff);
    let sections: Vec<_> = sections
        .into_iter()
        .map(|s| json!({"title":s.title,"rows":s.rows}))
        .collect();
    let footer = crate::ops::footer::tokens(app, Context::Diff);
    let options = crate::ops::menu::menu(
        app,
        Context::Diff,
        LeaderId::Options,
        crate::ops::menu::View::Open,
    )
    .unwrap();
    let options: Vec<_> = options
        .items
        .into_iter()
        .map(|i| json!([i.key.label(), crate::keymap::action_name(i.action), i.label]))
        .collect();
    json!({"sections":sections,"footer":footer,"options":options})
}

#[derive(serde::Deserialize)]
struct DiffReference {
    outputs: Vec<Value>,
    cases: Vec<DiffReferenceCase>,
}

#[derive(serde::Deserialize)]
struct DiffReferenceCase {
    case: Value,
    output: usize,
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("snapshots")
        .join("diff_presentation.json")
}

fn load_reference(path: &std::path::Path) -> Option<Vec<Value>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "diff presentation snapshot: {} not found — skipping (published-crate build?)",
                path.display()
            );
            return None;
        }
        Err(error) => panic!(
            "inspect diff presentation snapshot {}: {error}",
            path.display()
        ),
        Ok(_) => {}
    }
    let text = std::fs::read_to_string(path).expect("read diff presentation snapshot");
    let reference: DiffReference =
        serde_json::from_str(&text).expect("parse diff presentation snapshot");
    Some(
        reference
            .cases
            .into_iter()
            .map(|case| {
                let output = reference
                    .outputs
                    .get(case.output)
                    .expect("diff presentation snapshot output index");
                json!({"case":case.case,"output":output})
            })
            .collect(),
    )
}

fn reference() -> Option<Vec<Value>> {
    load_reference(&snapshot_path())
}

fn help_lines(sections: &[konoma_host_core::help::CoreSection]) -> Vec<Line<'static>> {
    let sections: Vec<_> = sections
        .iter()
        .map(|s| (s.title.as_str(), s.rows.as_slice()))
        .collect();
    konoma_widgets::help_panel::help_lines(&sections)
}

fn cells(actual: Vec<Line<'static>>, expected: Vec<Line<'static>>, width: u16) {
    let area = Rect::new(3, 2, width, 160);
    let mut a = Buffer::empty(area);
    let mut e = Buffer::empty(area);
    Paragraph::new(actual).render(area, &mut a);
    Paragraph::new(expected).render(area, &mut e);
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
    assert_eq!(count, usize::from(width) * 160);
    assert!(a.content.iter().any(|c| c.symbol() != " "));
}

#[test]
fn diff_presentation_cells_match_host_and_sdk_adapter() {
    let Some(records) = reference() else {
        return;
    };
    assert_eq!(records.len(), 240);
    for record in records {
        let (app, _dir) = app(&record["case"]);
        assert_eq!(host_output(&app), record["output"], "{}", record["case"]);
        let values = app.diff_presentation_values();
        let keys = konoma_plugin::app::AppKeys::new(
            &app.cfg.keys,
            record["case"]["scheme"].as_str().unwrap(),
        );
        let small = keys.diff_presentation(&values);
        let (extra, menus) = crate::ops::help::diff_help_parts(&app, &small);
        let actual = small.help_sections(&extra, &menus);
        let expected: Vec<_> = record["output"]["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| konoma_host_core::help::CoreSection {
                title: s["title"].as_str().unwrap().into(),
                rows: serde_json::from_value(s["rows"].clone()).unwrap(),
            })
            .collect();
        for width in [52, 160] {
            cells(help_lines(&actual), help_lines(&expected), width);
            let actual: Vec<_> = small
                .footer_tokens()
                .iter()
                .map(|t| t.text.clone())
                .collect();
            let expected: Vec<String> =
                serde_json::from_value(record["output"]["footer"].clone()).unwrap();
            use ratatui::style::Stylize;
            cells(
                vec![Line::from(konoma_widgets::help_panel::fit_tokens(&actual, width)).dim()],
                vec![Line::from(konoma_widgets::help_panel::fit_tokens(&expected, width)).dim()],
                width,
            );
        }
        let options = crate::ops::menu::diff_options_declaration(&app, &small).unwrap();
        let options: Vec<_> = small
            .options_menu(&options)
            .into_iter()
            .map(|i| {
                json!([
                    i.key.label(),
                    crate::keymap::action_name(crate::ops::decl::diff_action(i.action).unwrap()),
                    i.label
                ])
            })
            .collect();
        assert_eq!(json!(options), record["output"]["options"]);
    }
}

#[test]
fn standalone_help_has_no_host_tabs_or_follow_rows() {
    use konoma_plugin::host::project::diff_presentation::{
        DiffHelpSection, DiffPresentation, DiffUiOp,
    };
    let Some(records) = reference() else {
        return;
    };
    for record in records {
        let (app, _dir) = app(&record["case"]);
        let mut values = app.diff_presentation_values();
        let full = DiffPresentation::build(
            &values.state,
            values.capabilities,
            &values.bindings,
            &values.words,
        );
        let (extra, menus) = crate::ops::help::diff_help_parts(&app, &full);
        values.capabilities.global = false;
        values.capabilities.tabs = false;
        values.capabilities.follow = false;
        let small = DiffPresentation::build(
            &values.state,
            values.capabilities,
            &values.bindings,
            &values.words,
        );
        let filtered = small.help_rows_with(&extra);
        assert!(!filtered
            .iter()
            .any(|r| matches!(r.section, DiffHelpSection::Global | DiffHelpSection::Tabs)));
        let is_host = |op: &DiffUiOp| {
            matches!(
                op,
                DiffUiOp::Global(_) | DiffUiOp::Tab(_) | DiffUiOp::Follow | DiffUiOp::FollowScope
            )
        };
        assert!(!filtered.iter().flat_map(|r| &r.ops).any(is_host));
        let expected: Vec<_> = full
            .help_rows_with(&extra)
            .into_iter()
            .filter(|r| {
                !matches!(r.section, DiffHelpSection::Global | DiffHelpSection::Tabs)
                    && !r.ops.iter().any(is_host)
            })
            .collect();
        assert_eq!(filtered, expected);
        let sections = small.help_sections(&extra, &menus);
        assert!(sections.iter().all(|s| ![
            values.words.global.as_str(),
            values.words.tabs.as_str()
        ]
        .contains(&s.title.as_str())));
        let mut removed: Vec<_> = full
            .help_rows_with(&extra)
            .into_iter()
            .filter(|row| row.ops.iter().any(is_host))
            .map(|row| (row.keys_text, row.description))
            .collect();
        for menu in &menus {
            removed.extend(
                menu.items
                    .iter()
                    .filter(|item| is_host(&item.action))
                    .map(|item| {
                        (
                            format!(
                                "{} {}",
                                menu.prefix,
                                konoma_host_core::help::help_key(item.key)
                            ),
                            item.label.clone(),
                        )
                    }),
            );
        }
        // Expected text comes from the pre-extraction host fixture; capability differences
        // remove only the rows explicitly identified above by operation ID.
        let expected: Vec<_> = record["output"]["sections"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|section| {
                let title = section["title"].as_str().unwrap();
                if title == values.words.global || title == values.words.tabs {
                    return None;
                }
                let mut rows: Vec<(String, String)> =
                    serde_json::from_value(section["rows"].clone()).unwrap();
                rows.retain(|row| !removed.contains(row));
                (!rows.is_empty()).then(|| konoma_host_core::help::CoreSection {
                    title: title.into(),
                    rows,
                })
            })
            .collect();
        for width in [52, 160] {
            cells(help_lines(&sections), help_lines(&expected), width);
        }
        assert_eq!(
            small
                .footer_tokens()
                .iter()
                .map(|t| (&t.text, &t.keys, &t.ops))
                .collect::<Vec<_>>(),
            full.footer_tokens()
                .iter()
                .map(|t| (&t.text, &t.keys, &t.ops))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            small
                .action_priority()
                .iter()
                .map(|p| (p.op, p.word))
                .collect::<Vec<_>>(),
            full.action_priority()
                .iter()
                .filter(|p| !is_host(&p.op))
                .map(|p| (p.op, p.word))
                .collect::<Vec<_>>()
        );
        let options = crate::ops::menu::diff_options_declaration(&app, &small).unwrap();
        assert_eq!(
            small.options_menu(&options),
            full.options_menu(&options)
                .into_iter()
                .filter(|i| !is_host(&i.action))
                .collect::<Vec<_>>()
        );
        assert!(!small
            .options_menu(&options)
            .iter()
            .any(|i| is_host(&i.action)));
    }
}

#[test]
fn diff_presentation_is_built_once_per_frame() {
    use konoma_plugin::host::project::diff_presentation::diff_presentation_build_count;
    use ratatui::{backend::TestBackend, Terminal};
    let Some(records) = reference() else {
        return;
    };
    for record in records.into_iter().filter(|r| {
        r["case"]["language"] == "en"
            && r["case"]["scheme"] == "vim"
            && r["case"]["remap"] == false
            && r["case"]["wrap"] == true
            && r["case"]["backend"] == "git"
    }) {
        for (help, leader) in [
            (false, None),
            (true, None),
            (false, Some(LeaderId::Options)),
            (false, Some(LeaderId::Actions)),
            (false, Some(LeaderId::Copy)),
        ] {
            let (mut app, _dir) = app(&record["case"]);
            app.show_help = help;
            app.pending_leader = leader;
            app.flash = None;
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            let before = diff_presentation_build_count();
            terminal
                .draw(|frame| crate::ui::render(frame, &mut app))
                .unwrap();
            assert_eq!(
                diff_presentation_build_count() - before,
                1,
                "{}, help={help}, leader={leader:?}",
                record["case"]
            );
        }
    }
}

#[test]
fn diff_snapshot_missing_file_is_skipped() {
    let dir = crate::test_support::unique_tmp("diff_snapshot_missing");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(load_reference(&dir.join("missing.json")).is_none());
}

#[test]
#[should_panic(expected = "read diff presentation snapshot")]
fn diff_snapshot_present_but_unreadable_fails() {
    let dir = crate::test_support::unique_tmp("diff_snapshot_unreadable");
    std::fs::create_dir_all(&dir).unwrap();
    load_reference(&dir);
}

#[test]
#[should_panic(expected = "parse diff presentation snapshot")]
fn diff_snapshot_present_but_malformed_fails() {
    let dir = crate::test_support::unique_tmp("diff_snapshot_malformed");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("malformed.json");
    std::fs::write(&path, "{").unwrap();
    load_reference(&path);
}
