//! Project declarations are data until a host attaches project screens.

use konoma_plugin::registry::{ManifestReason, PluginOrigin, Registry, RegistryError};
use konoma_plugin::*;
use std::sync::Arc;

#[path = "project/chip.rs"]
mod chip;
#[path = "project/host_api.rs"]
mod host_api;
#[path = "project/presentation.rs"]
mod presentation;
#[path = "project/review.rs"]
mod review;

struct Decl(Manifest);
impl Plugin for Decl {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported("fixture"))
    }
}

fn project() -> ProjectDecl {
    ProjectDecl::new()
        .marker(DetectionMarker::new(".git", MarkerKind::Any))
        .screen(ProjectScreen::new("log", "Log").key('z').row_kind("commit"))
        .screen(
            ProjectScreen::new("branches", "Branches")
                .key('b')
                .row_kind("branch"),
        )
}
fn manifest() -> Manifest {
    Manifest::new("p", "1").project(project())
}
fn check(m: Manifest) -> Result<Registry, ManifestReason> {
    let mut r = Registry::new();
    match r.register(Arc::new(Decl(m)), PluginOrigin::User) {
        Ok(()) => Ok(r),
        Err(RegistryError::InvalidManifest { reason, .. }) => {
            assert!(r.ids().is_empty());
            assert_eq!(r.notes().len(), 1);
            Err(reason)
        }
        other => panic!("{other:?}"),
    }
}
fn reason(m: Manifest) -> ManifestReason {
    check(m).err().expect("manifest must be refused")
}

#[test]
fn project_destructive_requires_confirmation() {
    for confirmation in [None, Some(Confirmation::new("Delete?", vec![]))] {
        let mut act = Act::new("delete").destructive();
        act.confirmation = confirmation;
        assert_eq!(
            reason(manifest().action(act)),
            ManifestReason::DestructiveWithoutConfirmation {
                action: "delete".into()
            }
        );
    }
    let act = Act::new("delete")
        .key("d")
        .writes()
        .destructive()
        .project_context(ActContext::screen("branches", ["branch"]))
        .confirmation(Confirmation::new(
            "Delete?",
            vec![
                ConfirmChoice::new('y', "-d", "Delete"),
                ConfirmChoice::new('!', "-D", "Force delete"),
            ],
        ));
    let r = check(manifest().action(act.clone())).ok().unwrap();
    assert_eq!(r.manifest("p").unwrap().actions, vec![act]);
}

fn conditional_action() -> Act {
    Act::new("sync").confirmation(
        Confirmation::new("Sync?", vec![ConfirmChoice::new('y', "yes", "Yes")])
            .setting("confirm_sync"),
    )
}

#[test]
fn project_confirmation_setting_destructive_is_refused() {
    let refusal = reason(
        manifest()
            .setting(SettingDecl::new("confirm_sync", Value::Bool(true), "Ask"))
            .action(conditional_action().destructive()),
    );
    assert_eq!(
        refusal,
        ManifestReason::DestructiveConfirmationSetting {
            action: "sync".into(),
            key: "confirm_sync".into(),
        }
    );
    assert_eq!(
        refusal.to_string(),
        "destructive action `sync` cannot use confirmation setting `confirm_sync`"
    );
}

#[test]
fn project_confirmation_setting_requires_declared_boolean() {
    for default in [
        None,
        Some(Value::Str("true".into())),
        Some(Value::Int(1)),
        Some(Value::Float(1.0)),
        Some(Value::List(vec![])),
        Some(Value::Table(Default::default())),
    ] {
        let mut m = manifest().action(conditional_action());
        if let Some(default) = default {
            m = m.setting(SettingDecl::new("confirm_sync", default, "Ask"));
        }
        let refusal = reason(m);
        assert_eq!(
            refusal,
            ManifestReason::InvalidConfirmationSetting {
                action: "sync".into(),
                key: "confirm_sync".into(),
            }
        );
        assert_eq!(
            refusal.to_string(),
            "action `sync` confirmation setting `confirm_sync` must declare a boolean default"
        );
    }
}

#[test]
fn project_confirmation_setting_boolean_is_accepted() {
    for default in [false, true] {
        let action = conditional_action();
        let registry = check(
            manifest()
                .setting(SettingDecl::new(
                    "confirm_sync",
                    Value::Bool(default),
                    "Ask",
                ))
                .action(action.clone()),
        )
        .unwrap();
        assert_eq!(registry.manifest("p").unwrap().actions, vec![action]);
    }
}

#[test]
fn project_duplicate_screen_is_refused() {
    let p = project().screen(ProjectScreen::new("log", "Other"));
    assert_eq!(
        reason(Manifest::new("p", "1").project(p)),
        ManifestReason::DuplicateScreen {
            screen: "log".into()
        }
    );
    assert_eq!(
        reason(manifest().screen("log")),
        ManifestReason::DuplicateScreen {
            screen: "log".into()
        }
    );
    check(manifest()).ok().unwrap();
}

#[test]
fn project_duplicate_screen_key_is_refused() {
    let p = project().screen(ProjectScreen::new("other", "Other").key('z'));
    assert_eq!(
        reason(Manifest::new("p", "1").project(p)),
        ManifestReason::DuplicateScreenKey { key: "z".into() }
    );
    check(manifest()).ok().unwrap();
}

#[test]
fn project_unknown_screen_is_refused() {
    assert_eq!(
        reason(
            manifest()
                .action(Act::new("go").project_context(ActContext::screen("absent", ["commit"])))
        ),
        ManifestReason::UnknownScreen {
            action: "go".into(),
            screen: "absent".into()
        }
    );
    assert!(matches!(
        reason(
            Manifest::new("p", "1")
                .action(Act::new("go").project_context(ActContext::screen("log", ["commit"])))
        ),
        ManifestReason::UnknownScreen { .. }
    ));
    // Legacy host key contexts remain accepted, including a plugin's own older screens.
    check(Manifest::new("p", "1").action(Act::new("go").context("custom")))
        .ok()
        .unwrap();
}

#[test]
fn project_unknown_row_kind_is_refused() {
    for kind in ["branch", "absent", "Bad\nKind"] {
        let r = reason(
            manifest().action(Act::new("go").project_context(ActContext::screen("log", [kind]))),
        );
        assert_eq!(
            r,
            ManifestReason::UnknownRowKind {
                action: "go".into(),
                screen: "log".into(),
                kind: if kind == "Bad\nKind" {
                    "?ad??ind".into()
                } else {
                    kind.into()
                }
            }
        );
        assert!(!r.to_string().chars().any(char::is_control));
    }
    check(manifest().action(Act::new("go").project_context(ActContext::screen("log", ["commit"]))))
        .ok()
        .unwrap();
}

#[test]
fn project_requires_detection_markers() {
    assert_eq!(
        reason(Manifest::new("p", "1").project(ProjectDecl::new())),
        ManifestReason::NoDetectionMarkers
    );
    check(Manifest::new("p", "1")).ok().unwrap();
    check(manifest()).ok().unwrap();
}

#[test]
fn project_names_and_counts_are_bounded() {
    use konoma_plugin::registry::{ManifestCount as C, ManifestName as N};
    for (screen, row, expected) in [
        ("Bad ID", "commit", N::ScreenName),
        ("log", "Bad ID", N::RowKind),
    ] {
        let p = ProjectDecl::new()
            .marker(DetectionMarker::new(".git", MarkerKind::Any))
            .screen(ProjectScreen::new(screen, "Log").row_kind(row));
        assert!(
            matches!(reason(Manifest::new("p", "1").project(p)), ManifestReason::InvalidName { kind, .. } if kind == expected)
        );
    }
    for (kind, n) in [(C::Screens, 65), (C::Markers, 513), (C::RowKinds, 513)] {
        let mut p = project();
        match kind {
            C::Screens => {
                p.screens = (0..n)
                    .map(|i| ProjectScreen::new(format!("s{i}"), "Screen"))
                    .collect()
            }
            C::Markers => p.markers = vec![DetectionMarker::new(".git", MarkerKind::Any); n],
            C::RowKinds => p.screens[0].row_kinds = (0..n).map(|i| format!("row{i}")).collect(),
            _ => unreachable!(),
        }
        assert!(
            matches!(reason(Manifest::new("p", "1").project(p.clone())), ManifestReason::TooMany { kind: k, count, .. } if k == kind && count == n)
        );
        match kind {
            C::Screens => {
                p.screens.pop();
            }
            C::Markers => {
                p.markers.pop();
            }
            C::RowKinds => {
                p.screens[0].row_kinds.pop();
            }
            _ => unreachable!(),
        }
        check(Manifest::new("p", "1").project(p)).ok().unwrap();
    }
}

#[test]
fn project_enter_is_an_ordinary_action_scoped_to_each_row_kind() {
    let p = project().screen(
        ProjectScreen::new("mixed", "Mixed")
            .row_kind("file")
            .row_kind("commit"),
    );
    let actions = [
        Act::new("preview")
            .key("Enter")
            .project_context(ActContext::screen("mixed", ["file"])),
        Act::new("detail")
            .key("Enter")
            .project_context(ActContext::screen("mixed", ["commit"])),
    ];
    let m = Manifest::new("p", "1")
        .project(p)
        .action(actions[0].clone())
        .action(actions[1].clone());
    assert!(konoma_plugin::keys::validate(&m).is_empty());
    let r = check(m.clone()).ok().unwrap();
    assert_eq!(r.manifest("p").unwrap().actions, actions);
    let duplicate = m.action(
        Act::new("collision")
            .key("Enter")
            .project_context(ActContext::screen("mixed", ["file"])),
    );
    assert!(matches!(
        &konoma_plugin::keys::validate(&duplicate)[0].kind,
        konoma_plugin::keys::KeyIssueKind::Duplicate { .. }
    ));
    let tree = manifest().action(
        Act::new("stage")
            .key("s")
            .project_context(ActContext::TreeSelection),
    );
    assert!(matches!(
        &konoma_plugin::keys::validate(&tree)[0].kind,
        konoma_plugin::keys::KeyIssueKind::TreeKey
    ));
}

#[cfg(feature = "proc")]
mod wire {
    use super::*;
    use konoma_plugin::proc::{frame, wire::*};
    use konoma_plugin::view::{ListRow, ListView};
    use std::path::{Path, PathBuf};

    fn bytes<T: serde::Serialize + serde::de::DeserializeOwned>(v: &T) -> T {
        frame::decode(&frame::encode(v).unwrap()).unwrap()
    }

    #[test]
    fn project_marker_kind_round_trip() {
        for kind in [MarkerKind::Any, MarkerKind::File, MarkerKind::Directory] {
            assert_eq!(
                marker_kind_from_wire(bytes(&marker_kind_to_wire(kind))),
                kind
            );
        }
    }

    #[test]
    fn project_detection_marker_round_trip() {
        for marker in [
            DetectionMarker::new(".git", MarkerKind::Any),
            DetectionMarker::new("Package.swift", MarkerKind::File),
            DetectionMarker::new("*.xcworkspace", MarkerKind::Directory),
        ] {
            assert_eq!(
                detection_marker_from_wire(bytes(&detection_marker_to_wire(&marker))),
                marker
            );
        }
    }

    #[test]
    fn project_screen_round_trip() {
        for screen in [
            ProjectScreen::new("detail", "Details"),
            ProjectScreen::new("log", t!(en: "Log", ja: "履歴"))
                .key('l')
                .row_kind("commit")
                .row_kind("file"),
        ] {
            assert_eq!(
                project_screen_from_wire(bytes(&project_screen_to_wire(&screen))),
                screen
            );
        }
    }

    #[test]
    fn project_choice_round_trip() {
        let choice = Choice::new("main", t!(en: "Main", ja: "メイン"));
        assert_eq!(choice_from_wire(bytes(&choice_to_wire(&choice))), choice);
    }

    #[test]
    fn project_confirm_choice_round_trip() {
        for choice in [
            ConfirmChoice::new('y', "-d", t!(en: "Delete", ja: "削除")),
            ConfirmChoice::new('!', "-D", t!(en: "Force", ja: "強制削除")),
        ] {
            assert_eq!(
                confirm_choice_from_wire(bytes(&confirm_choice_to_wire(&choice))),
                choice
            );
        }
    }

    #[test]
    fn project_declaration_round_trip() {
        let p = project()
            .marker(DetectionMarker::new("Package.swift", MarkerKind::File))
            .marker(DetectionMarker::new("*.xcworkspace", MarkerKind::Directory))
            .chip(t!(en: "main", ja: "メイン"))
            .watch(".git/HEAD")
            .watch(".git/index")
            .version_control();
        assert_eq!(project_from_wire(bytes(&project_to_wire(&p))).unwrap(), p);
        let m = Manifest::new("p", "1").project(p);
        let w = manifest_to_wire(&m).unwrap();
        let json = serde_json::to_vec(&w).unwrap();
        let back = manifest_from_wire(bytes(&serde_json::from_slice::<WManifest>(&json).unwrap()))
            .unwrap();
        assert_eq!(back.project, m.project);
    }

    #[test]
    fn project_manifest_round_trips_through_the_export_json_helpers() {
        use konoma_plugin::proc::{manifest_from_json, manifest_to_json};
        let m = manifest().action(
            Act::new("delete")
                .writes()
                .destructive()
                .project_context(ActContext::screen("branches", ["branch"]))
                .confirmation(Confirmation::new(
                    "Delete?",
                    vec![ConfirmChoice::new('!', "-D", "Force delete")],
                ))
                .text_input(TextInput::new("Name", "main"))
                .choice_input(ChoiceInput::new(
                    "Branch",
                    vec![Choice::new("main", "Main")],
                )),
        );
        let json = manifest_to_json(&m).unwrap();
        let back = manifest_from_json(&json).unwrap();
        assert_eq!(
            manifest_to_wire(&back).unwrap(),
            manifest_to_wire(&m).unwrap()
        );
        assert_eq!(back.actions, m.actions);
        assert_eq!(back.project, m.project);
    }

    #[test]
    fn project_context_round_trip() {
        for c in [
            ActContext::screen("log", ["commit", "file"]),
            ActContext::TreeSelection,
            ActContext::Diff,
        ] {
            assert_eq!(act_context_from_wire(bytes(&act_context_to_wire(&c))), c);
        }
    }

    #[test]
    fn project_confirmation_round_trip() {
        let c = Confirmation::new(
            t!(en: "Delete?", ja: "削除しますか？"),
            vec![
                ConfirmChoice::new('y', "-d", "Delete"),
                ConfirmChoice::new('!', "-D", "Force"),
            ],
        );
        let c = c.setting("confirm_sync");
        assert_eq!(c.setting.as_deref(), Some("confirm_sync"));
        assert_eq!(confirmation_from_wire(bytes(&confirmation_to_wire(&c))), c);
        let m = manifest()
            .setting(SettingDecl::new("confirm_sync", Value::Bool(true), "Ask"))
            .action(Act::new("sync").confirmation(c));
        let json = konoma_plugin::proc::manifest_to_json(&m).unwrap();
        assert_eq!(
            konoma_plugin::proc::manifest_from_json(&json)
                .unwrap()
                .actions,
            m.actions
        );
    }

    #[test]
    fn project_confirmation_older_child_has_no_setting() {
        let c = Confirmation::new("Proceed?", vec![ConfirmChoice::new('y', "yes", "Yes")]);
        assert_eq!(c.setting, None);
        let mut json = serde_json::to_value(confirmation_to_wire(&c)).unwrap();
        assert!(json.get("setting").is_none());
        json.as_object_mut().unwrap().remove("setting");
        let old: WConfirmation = serde_json::from_value(json).unwrap();
        assert_eq!(confirmation_from_wire(bytes(&old)), c);
    }

    #[test]
    fn project_text_input_round_trip() {
        let i = TextInput::new(t!(en: "Message", ja: "メッセージ"), "initial text");
        assert_eq!(text_input_from_wire(bytes(&text_input_to_wire(&i))), i);
    }

    #[test]
    fn project_choice_input_round_trip() {
        let i = ChoiceInput::new(
            "Branch",
            vec![
                Choice::new("main", "Main"),
                Choice::new("dev", "Development"),
            ],
        );
        assert_eq!(choice_input_from_wire(bytes(&choice_input_to_wire(&i))), i);
    }

    #[test]
    fn project_act_round_trip() {
        let a = Act::new("delete")
            .key("Enter")
            .writes()
            .destructive()
            .project_context(ActContext::screen("branches", ["branch"]))
            .confirmation(Confirmation::new(
                "Delete?",
                vec![ConfirmChoice::new('y', "-d", "Delete")],
            ))
            .text_input(TextInput::new("Name", "main"))
            .choice_input(ChoiceInput::new(
                "Branch",
                vec![Choice::new("main", "Main")],
            ));
        let m = manifest().action(a.clone());
        assert_eq!(
            manifest_from_wire(bytes(&manifest_to_wire(&m).unwrap()))
                .unwrap()
                .actions,
            vec![a]
        );
    }

    #[test]
    fn project_input_round_trip() {
        use std::os::unix::ffi::OsStrExt;
        let dir = konoma_test_support::unique_tmp("project_wire");
        let side = Side::new(dir.as_path(), 'h');
        let root = PathBuf::from(std::ffi::OsStr::from_bytes(b"/project/\xff"));
        let input = Input::Project {
            root: root.clone(),
            marker: ".git".into(),
        };
        let w = input_to_wire(&input, &mut Enc::new(&side)).unwrap();
        let Input::Project { root: back, marker } =
            input_from_wire(bytes(&w), &mut Dec::new(&side)).unwrap()
        else {
            panic!()
        };
        assert_eq!(back, root);
        assert_eq!(marker, ".git");
        assert!(input_from_wire(
            WInput::Project {
                root: path_to_wire(Path::new("relative")),
                marker: ".git".into()
            },
            &mut Dec::new(&side)
        )
        .is_err());
    }

    #[test]
    fn project_list_round_trip() {
        let dir = konoma_test_support::unique_tmp("project_list");
        let side = Side::new(dir.as_path(), 'h');
        let mut row = ListRow::new(
            "commit",
            vec![Line::from(vec![
                Span::styled("│╲", Style::new().fg(Color::Red)),
                Span::raw(" message"),
            ])],
        );
        row.marked = true;
        row.checked = Some(false);
        row.target = Some("/project/file".into());
        row = row.name("short file name");
        assert_eq!(
            ProjectRow::from(&row).name.as_deref(),
            Some("short file name")
        );
        let expected = ListView { rows: vec![row] };
        let mut v = View::List(expected.clone());
        let w = view_to_wire(&mut v, &mut Enc::new(&side)).unwrap();
        let View::List(back) = view_from_wire(bytes(&w), &mut Dec::new(&side)).unwrap() else {
            panic!()
        };
        assert_eq!(back, expected);
    }

    #[test]
    fn project_row_name_round_trip_and_older_child_default() {
        let row = ProjectRow::new("file", "opaque").name("file.txt");
        assert_eq!(row.name.as_deref(), Some("file.txt"));
        assert_eq!(
            project_row_from_wire(bytes(&project_row_to_wire(&row))),
            row
        );
        let context = ProjectAct::new().selected(row.clone()).marked([row]);
        assert_eq!(
            project_act_from_wire(bytes(&project_act_to_wire(&context))),
            context
        );
        assert_eq!(ProjectRow::new("file", "opaque").name, None);
        let old: WProjectRow = serde_json::from_value(serde_json::json!({
            "kind": "file", "value": "opaque"
        }))
        .unwrap();
        assert_eq!(
            project_row_from_wire(old),
            ProjectRow::new("file", "opaque")
        );
        let old: WListRow = serde_json::from_value(serde_json::json!({
            "kind": "file", "cells": [], "target": null, "checked": null
        }))
        .unwrap();
        let dir = konoma_test_support::unique_tmp("project_old_row");
        let side = Side::new(dir.as_path(), 'h');
        let View::List(list) =
            view_from_wire(WView::List(vec![old]), &mut Dec::new(&side)).unwrap()
        else {
            panic!()
        };
        assert_eq!(list.rows[0].name, None);
        assert_eq!(ListRow::new("file", vec![]).name, None);
    }

    #[test]
    fn project_list_row_round_trip_keeps_marks_independent_of_checks() {
        let dir = konoma_test_support::unique_tmp("project_row");
        let side = Side::new(dir.as_path(), 'h');
        for checked in [None, Some(false), Some(true)] {
            for marked in [false, true] {
                let mut row = ListRow::new(
                    "commit",
                    vec![Line::from(vec![
                        Span::styled("│╲", Style::new().fg(Color::Red)),
                        Span::raw(" commit"),
                    ])],
                );
                row.checked = checked;
                row.marked = marked;
                row.target = Some("/project/file".into());
                let mut view = View::List(ListView {
                    rows: vec![row.clone()],
                });
                let WView::List(mut rows) = view_to_wire(&mut view, &mut Enc::new(&side)).unwrap()
                else {
                    panic!()
                };
                let wire_row: WListRow = bytes(&rows.pop().unwrap());
                let View::List(back) =
                    view_from_wire(WView::List(vec![wire_row]), &mut Dec::new(&side)).unwrap()
                else {
                    panic!()
                };
                assert_eq!(back.rows, vec![row]);
            }
        }
    }

    fn outcome(o: Outcome) {
        assert_eq!(
            outcome_from_wire_checked(bytes(&outcome_to_wire(&o))).unwrap(),
            o
        );
    }
    #[test]
    fn project_push_screen_round_trip() {
        outcome(Outcome::PushScreen {
            screen: "detail".into(),
            selection: Some("commit-id".into()),
        });
        outcome(Outcome::PushScreen {
            screen: "detail".into(),
            selection: None,
        });
    }
    #[test]
    fn project_open_file_round_trip() {
        outcome(Outcome::OpenFile {
            path: "/project/file".into(),
            line: Some(12),
        });
        outcome(Outcome::OpenFile {
            path: "/project/file".into(),
            line: None,
        });
    }
    #[test]
    fn project_open_diff_round_trip() {
        outcome(Outcome::OpenDiff {
            path: "/project/file".into(),
        });
    }
    #[test]
    fn project_switch_root_round_trip() {
        outcome(Outcome::SwitchRoot {
            root: "/project/worktree".into(),
        });
    }
    #[test]
    fn project_open_tab_round_trip() {
        outcome(Outcome::OpenTab {
            path: "/project/worktree".into(),
        });
    }

    #[test]
    fn project_navigation_rejects_bad_paths_and_lines() {
        for path in ["relative", "/bad\0path"] {
            for o in [
                WOutcome::OpenFile {
                    path: path_to_wire(Path::new(path)),
                    line: None,
                },
                WOutcome::OpenDiff {
                    path: path_to_wire(Path::new(path)),
                },
                WOutcome::SwitchRoot {
                    root: path_to_wire(Path::new(path)),
                },
                WOutcome::OpenTab {
                    path: path_to_wire(Path::new(path)),
                },
            ] {
                assert!(outcome_from_wire_checked(o).is_err());
            }
        }
        assert!(outcome_from_wire_checked(WOutcome::OpenFile {
            path: path_to_wire(Path::new("/file")),
            line: Some(0)
        })
        .is_err());
    }

    #[test]
    fn project_protocol_keeps_old_bytes_and_defaults() {
        let old = manifest_to_wire(&Manifest::new("p", "1").action(Act::new("old"))).unwrap();
        assert_eq!(manifest_protocol(&old), 1);
        let json = serde_json::to_value(&old).unwrap();
        assert!(json.get("project").is_none());
        for field in [
            "project_context",
            "confirmation",
            "text_input",
            "choice_input",
        ] {
            assert!(json["actions"][0].get(field).is_none());
        }
        assert_eq!(
            manifest_to_wire(&manifest_from_wire(bytes(&old)).unwrap()).unwrap(),
            old
        );
        let p = manifest_to_wire(&manifest()).unwrap();
        assert_eq!(manifest_protocol(&p), 2);
        assert!(require_protocol(1, manifest_protocol(&p)).is_err());
        require_protocol(2, manifest_protocol(&p)).unwrap();
        let old_row: WListRow =
            serde_json::from_str(r#"{"cells":[],"kind":"file","target":null,"checked":true}"#)
                .unwrap();
        assert!(!old_row.marked);
        assert!(serde_json::to_value(&old_row)
            .unwrap()
            .get("marked")
            .is_none());
    }

    struct Served {
        project: bool,
    }

    #[test]
    fn project_client_does_not_send_project_input_to_a_protocol_one_peer() {
        use konoma_plugin::proc::client::{Config, ProcPlugin, Spawned};
        use konoma_plugin::proc::proto::{Hello, HelloReply, ToChild};
        use std::os::unix::net::UnixStream;
        let dir = konoma_test_support::unique_tmp("project_client_v1");
        std::fs::create_dir_all(dir.as_path()).unwrap();
        let threads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = threads.clone();
        let launch = move || {
            let (host, mut child) = UnixStream::pair()?;
            let kill = host.try_clone()?;
            let thread = std::thread::spawn(move || {
                let hello: Hello = frame::recv(&mut child).unwrap().unwrap();
                assert!(hello.versions.contains(&1));
                frame::send(
                    &mut child,
                    &HelloReply::Ack {
                        version: 1,
                        manifest: manifest_to_wire(&Manifest::new("legacy", "1")).unwrap(),
                    },
                )
                .unwrap();
                // Rejection happens before Open is sent; the next message must be shutdown.
                let message: ToChild = frame::recv(&mut child).unwrap().unwrap();
                assert!(matches!(message, ToChild::Shutdown));
            });
            started.lock().unwrap().push(thread);
            Ok(Spawned {
                reader: Box::new(host.try_clone()?),
                writer: Box::new(host),
                stderr: None,
                pid: None,
                kill: Box::new(move || {
                    let _ = kill.shutdown(std::net::Shutdown::Both);
                }),
            })
        };
        let config = Config {
            scratch_root: dir.as_path().to_path_buf(),
            ..Config::default()
        };
        let plugin = ProcPlugin::start(launch, config).unwrap();
        let failure = match plugin.open(
            Input::Project {
                root: "/project".into(),
                marker: ".git".into(),
            },
            &OpenCx::bare(),
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("protocol one must not receive project input"),
        };
        assert_eq!(failure.kind, FailureKind::Corrupt);
        assert_eq!(
            failure.detail.as_deref(),
            Some("project data requires protocol 2, negotiated 1")
        );
        plugin.stop();
        drop(plugin);
        for thread in threads.lock().unwrap().drain(..) {
            thread.join().unwrap();
        }
    }

    #[test]
    fn project_client_transports_input_view_and_every_outcome() {
        use konoma_plugin::proc::client::{Config, ProcPlugin, Spawned};
        use konoma_plugin::proc::{serve_io, ServeOptions};
        use std::os::unix::net::UnixStream;
        let dir = konoma_test_support::unique_tmp("project_client");
        std::fs::create_dir_all(dir.as_path()).unwrap();
        let threads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = threads.clone();
        let launch = move || {
            let (host, child) = UnixStream::pair()?;
            let reader = child.try_clone()?;
            let kill = host.try_clone()?;
            let thread = std::thread::spawn(move || {
                serve_io(
                    Served { project: true },
                    reader,
                    child,
                    &ServeOptions::default(),
                )
            });
            started.lock().unwrap().push(thread);
            Ok(Spawned {
                reader: Box::new(host.try_clone()?),
                writer: Box::new(host),
                stderr: None,
                pid: None,
                kill: Box::new(move || {
                    let _ = kill.shutdown(std::net::Shutdown::Both);
                }),
            })
        };
        let config = Config {
            scratch_root: dir.as_path().to_path_buf(),
            ..Config::default()
        };
        let plugin = ProcPlugin::start(launch, config).unwrap();
        assert_eq!(plugin.manifest().project, Some(project()));
        let mut session = plugin
            .open(
                Input::Project {
                    root: "/project".into(),
                    marker: ".git".into(),
                },
                &OpenCx::bare(),
            )
            .unwrap();
        for action in ["push", "file", "diff", "root", "tab"] {
            assert_eq!(
                session.act(action, &ActCx::bare()).unwrap(),
                Served { project: true }
                    .act(action, &ActCx::bare())
                    .unwrap()
            );
        }
        assert!(
            matches!(session.view(&ViewReq::new(80, 24), &ViewCx::bare()).unwrap(), View::List(l) if l.rows[0].marked)
        );
        drop(session);
        plugin.stop();
        drop(plugin);
        for thread in threads.lock().unwrap().drain(..) {
            thread.join().unwrap().unwrap();
        }
    }
    impl Plugin for Served {
        fn manifest(&self) -> Manifest {
            if self.project {
                manifest()
            } else {
                Manifest::new("p", "1")
            }
        }
        fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            match input {
                Input::Project { root, marker }
                    if root == Path::new("/project") && marker == ".git" =>
                {
                    Ok(Box::new(Served { project: true }))
                }
                Input::File { .. } => Ok(Box::new(Served { project: false })),
                _ => Err(Failure::unsupported("unexpected input")),
            }
        }
    }
    impl Session for Served {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            let mut row = ListRow::new("commit", vec![Line::raw("commit")]);
            row.marked = true;
            Ok(View::List(ListView { rows: vec![row] }))
        }
        fn act(&mut self, a: &str, _: &ActCx) -> Result<Outcome, Failure> {
            Ok(match a {
                "push" => Outcome::PushScreen {
                    screen: "log".into(),
                    selection: Some("id".into()),
                },
                "file" => Outcome::OpenFile {
                    path: "/project/file".into(),
                    line: Some(2),
                },
                "diff" => Outcome::OpenDiff {
                    path: "/project/file".into(),
                },
                "root" => Outcome::SwitchRoot {
                    root: "/project/next".into(),
                },
                "tab" => Outcome::OpenTab {
                    path: "/project/next".into(),
                },
                _ => Outcome::Unhandled,
            })
        }
    }

    #[test]
    fn project_serve_negotiation_and_messages() {
        use konoma_plugin::proc::{proto::*, serve_io, ServeOptions};
        use std::os::unix::net::UnixStream;
        for (offered, project_plugin, expected) in [
            (vec![1, 2], true, Some(2)),
            (vec![1], true, None),
            (vec![1], false, Some(1)),
        ] {
            let dir = konoma_test_support::unique_tmp("project_serve");
            std::fs::create_dir_all(dir.as_path()).unwrap();
            let (mut host, child) = UnixStream::pair().unwrap();
            let reader = child.try_clone().unwrap();
            let thread = std::thread::spawn(move || {
                serve_io(
                    Served {
                        project: project_plugin,
                    },
                    reader,
                    child,
                    &ServeOptions::default(),
                )
            });
            frame::send(
                &mut host,
                &Hello {
                    versions: offered,
                    host_version: "test".into(),
                    side_dir: path_to_wire(dir.as_path()),
                    lang: "en".into(),
                },
            )
            .unwrap();
            let reply: HelloReply = frame::recv(&mut host).unwrap().unwrap();
            let Some(version) = expected else {
                assert!(
                    matches!(reply, HelloReply::Refused { supported, .. } if supported == vec![2])
                );
                drop(host);
                thread.join().unwrap().unwrap();
                continue;
            };
            assert!(matches!(reply, HelloReply::Ack { version: v, .. } if v == version));
            let cx = cx_to_wire(&CxSource {
                settings: Settings::empty(),
                lang: "en",
                term: TermCaps::new(),
                external: false,
                read_scope: &ReadScope::unrestricted(),
            })
            .unwrap();
            let input = WInput::Project {
                root: path_to_wire(Path::new("/project")),
                marker: ".git".into(),
            };
            frame::send(
                &mut host,
                &ToChild::Open {
                    sid: 1,
                    rid: 1,
                    input,
                    cx: cx.clone(),
                },
            )
            .unwrap();
            let opened: FromChild = frame::recv(&mut host).unwrap().unwrap();
            if version == 1 {
                assert!(matches!(opened.answer, Answer::Err(_)));
                frame::send(
                    &mut host,
                    &ToChild::Open {
                        sid: 1,
                        rid: 2,
                        input: WInput::File {
                            path: path_to_wire(Path::new("/project/file")),
                            hint: None,
                        },
                        cx: cx.clone(),
                    },
                )
                .unwrap();
                let opened: FromChild = frame::recv(&mut host).unwrap().unwrap();
                assert!(matches!(opened.answer, Answer::Ok(Reply::Opened { .. })));
            } else {
                assert!(matches!(opened.answer, Answer::Ok(Reply::Opened { .. })));
            }
            for (i, action) in ["push", "file", "diff", "root", "tab"]
                .into_iter()
                .enumerate()
            {
                frame::send(
                    &mut host,
                    &ToChild::Act {
                        sid: 1,
                        rid: 3 + i as u64,
                        action: action.into(),
                        cx: cx.clone(),
                    },
                )
                .unwrap();
                let reply: FromChild = frame::recv(&mut host).unwrap().unwrap();
                if version == 1 {
                    assert!(matches!(reply.answer, Answer::Err(_)));
                } else {
                    let Answer::Ok(Reply::Acted { outcome, .. }) = reply.answer else {
                        panic!()
                    };
                    let expected = Served { project: true }
                        .act(action, &ActCx::bare())
                        .unwrap();
                    assert_eq!(outcome_from_wire_checked(outcome).unwrap(), expected);
                }
            }
            frame::send(
                &mut host,
                &ToChild::View {
                    sid: 1,
                    rid: 10,
                    req: req_to_wire(&ViewReq::new(80, 24)),
                    cx,
                },
            )
            .unwrap();
            let reply: FromChild = frame::recv(&mut host).unwrap().unwrap();
            if version == 1 {
                assert!(matches!(reply.answer, Answer::Err(_)));
            } else {
                assert!(
                    matches!(reply.answer, Answer::Ok(Reply::Viewed { view: WView::List(rows), .. }) if rows[0].marked)
                );
            }
            frame::send(&mut host, &ToChild::Shutdown).unwrap();
            drop(host);
            thread.join().unwrap().unwrap();
        }
    }
}
