use super::*;
use konoma_plugin::keys::{self, Effect, KeyIssueKind, KeySpec};
use konoma_plugin::registry::*;
use konoma_plugin::view::ListRow;

#[test]
fn legacy_duplicate_screens_are_deliberately_refused() {
    assert_eq!(
        reason(Manifest::new("p", "1").screen("log").screen("log")),
        ManifestReason::DuplicateScreen {
            screen: "log".into()
        }
    );
}

#[test]
fn public_key_validation_and_sanitization_reject_unconfirmed_destruction() {
    let m = Manifest::new("p", "1").action(Act::new("delete").destructive());
    let issues = keys::validate(&m);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, KeyIssueKind::DestructiveWithoutConfirm);
    assert_eq!(issues[0].effect, Effect::Rejected);
    let (clean, notes) = keys::sanitize(&m);
    assert!(clean.actions.is_empty());
    assert_eq!(notes, issues);
    assert_eq!(
        issues[0].to_string(),
        "action `delete` is destructive but has no confirmation choices (rejected)"
    );
}

#[test]
fn project_screen_contexts_cannot_alias_host_contexts() {
    for id in ["tree", "canvas", "list", "diff"] {
        // d belongs to the tree, s to diff, and j is given up by a canvas.
        let m = Manifest::new("p", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new(".git", MarkerKind::Any))
                    .screen(ProjectScreen::new(id, "Screen").row_kind("row")),
            )
            .action(
                Act::new("delete")
                    .key("d")
                    .project_context(ActContext::screen(id, ["row"])),
            )
            .action(
                Act::new("stage")
                    .key("s")
                    .project_context(ActContext::screen(id, ["row"])),
            )
            .action(
                Act::new("move")
                    .key("j")
                    .project_context(ActContext::screen(id, ["row"])),
            );
        let issues = keys::validate(&m);
        assert_eq!(issues.len(), 1, "{id}: {issues:?}");
        assert_eq!(issues[0].act, "move", "{id}");
        assert!(
            matches!(&issues[0].kind, KeyIssueKind::Reserved { context, .. } if context == &format!("project:{id}")),
            "{id}: {issues:?}"
        );
        let r = check(m).unwrap();
        let acts = &r.manifest("p").unwrap().actions;
        assert_eq!(acts[0].key.as_deref(), Some("d"));
        assert_eq!(acts[1].key.as_deref(), Some("s"));
        assert_eq!(acts[2].key, None);
    }
}

#[test]
fn project_watch_paths_are_relative_bounded_and_nonempty() {
    for path in [
        "",
        "/absolute",
        "..",
        "../out",
        "inside/../../out",
        "inside/../file",
    ] {
        assert_eq!(
            reason(Manifest::new("p", "1").project(project().watch(path))),
            ManifestReason::InvalidWatchPath { index: 0 },
            "{path:?}"
        );
    }
    for path in [".", ".git/HEAD", "./sub/file", "sub//file"] {
        check(Manifest::new("p", "1").project(project().watch(path))).unwrap();
    }
    let mut p = project();
    p.watch_paths = vec![".git/HEAD".into(); MAX_WATCH_PATHS];
    check(Manifest::new("p", "1").project(p.clone())).unwrap();
    p.watch_paths.push("extra".into());
    assert_eq!(
        reason(Manifest::new("p", "1").project(p)),
        ManifestReason::TooMany {
            kind: ManifestCount::WatchPaths,
            count: MAX_WATCH_PATHS + 1,
            limit: MAX_WATCH_PATHS,
        }
    );
}

#[test]
fn project_markers_are_single_bounded_components() {
    for name in ["", ".", "..", "/absolute", "a/b", "a/", "../out", "\0"] {
        let mut p = project();
        p.markers[0].name = name.into();
        assert_eq!(
            reason(Manifest::new("p", "1").project(p)),
            ManifestReason::InvalidMarker {
                index: 0,
                limit: MAX_MARKER_BYTES
            },
            "{name:?}"
        );
    }
    for name in [
        "*.xcodeproj".to_string(),
        "a..b".into(),
        "x".repeat(MAX_MARKER_BYTES),
        format!("{}a", "界".repeat(MAX_MARKER_BYTES / 3)),
    ] {
        let mut p = project();
        p.markers[0].name = name;
        check(Manifest::new("p", "1").project(p)).unwrap();
    }
    let mut p = project();
    p.markers[0].name = "x".repeat(MAX_MARKER_BYTES + 1);
    assert_eq!(
        reason(Manifest::new("p", "1").project(p)),
        ManifestReason::InvalidMarker {
            index: 0,
            limit: MAX_MARKER_BYTES
        }
    );
}

#[test]
fn project_screen_keys_reuse_the_list_reserved_table() {
    for key in ['\n', '\u{1b}', 'q', '1', '9', 'j', '?'] {
        let mut p = project();
        p.screens[0].key = Some(key);
        assert_eq!(
            reason(Manifest::new("p", "1").project(p)),
            ManifestReason::InvalidScreenKey {
                key: key.escape_default().to_string()
            }
        );
    }
    for (key, _) in keys::reserved("list") {
        if key.ctrl {
            continue;
        }
        if let konoma_plugin::keys::KeyName::Char(c) = key.code {
            let mut p = project();
            p.screens[0].key = Some(c);
            // A duplicate key is also refused before the reserved-key check.
            assert!(
                check(Manifest::new("p", "1").project(p)).is_err(),
                "{key:?}"
            );
        }
    }
    assert!(!keys::reserved("list")
        .iter()
        .any(|(k, _)| *k == KeySpec::ch('z')));
    check(manifest()).unwrap();
}

fn confirmation(choices: Vec<ConfirmChoice>) -> Manifest {
    Manifest::new("p", "1").action(Act::new("go").confirmation(Confirmation::new("Go?", choices)))
}

#[test]
fn project_confirmation_keys_are_nonempty_unique_and_safe() {
    assert_eq!(
        reason(confirmation(vec![])),
        ManifestReason::EmptyConfirmation {
            action: "go".into()
        }
    );
    assert_eq!(
        reason(confirmation(vec![
            ConfirmChoice::new('y', "a", "A"),
            ConfirmChoice::new('y', "b", "B")
        ])),
        ManifestReason::DuplicateConfirmationKey {
            action: "go".into(),
            key: "y".into()
        }
    );
    for key in ['\n', '\u{1b}', 'q', '1', '9', '?'] {
        assert_eq!(
            reason(confirmation(vec![ConfirmChoice::new(key, "a", "A")])),
            ManifestReason::InvalidConfirmationKey {
                action: "go".into(),
                key: key.escape_default().to_string()
            }
        );
    }
    for (key, layer) in keys::confirmation_reserved() {
        assert!(keys::reserved("global").contains(&(key, layer)));
        assert!(
            layer == keys::Layer::Global
                || key == KeySpec::ch('q')
                || key == KeySpec::key(keys::KeyName::Esc)
        );
    }
    check(confirmation(vec![
        ConfirmChoice::new('y', "-d", "Delete"),
        ConfirmChoice::new('!', "-D", "Force delete"),
    ]))
    .unwrap();
}

#[test]
fn project_prompt_counts_values_and_all_label_languages_are_bounded() {
    let choice = Choice::new(
        "x".repeat(MAX_CHOICE_VALUE_BYTES),
        "x".repeat(konoma_plugin::text::MAX_TEXT_CHARS),
    );
    let choices = |choices| {
        Manifest::new("p", "1")
            .action(Act::new("go").choice_input(ChoiceInput::new("Pick", choices)))
    };
    check(choices(vec![choice.clone(); MAX_CHOICES])).unwrap();
    assert_eq!(
        reason(choices(vec![choice; MAX_CHOICES + 1])),
        ManifestReason::TooMany {
            kind: ManifestCount::Choices,
            count: MAX_CHOICES + 1,
            limit: MAX_CHOICES
        }
    );
    let mut c = (0..MAX_CHOICES)
        .map(|i| ConfirmChoice::new(char::from_u32(0x400 + i as u32).unwrap(), "v", "V"))
        .collect::<Vec<_>>();
    check(confirmation(c.clone())).unwrap();
    c.push(ConfirmChoice::new('z', "v", "V"));
    assert_eq!(
        reason(confirmation(c)),
        ManifestReason::TooMany {
            kind: ManifestCount::Choices,
            count: MAX_CHOICES + 1,
            limit: MAX_CHOICES
        }
    );
    for confirm in [false, true] {
        let make = |c: Choice| {
            if confirm {
                let mut k = ConfirmChoice::new('y', "", "");
                k.choice = c;
                confirmation(vec![k])
            } else {
                choices(vec![c])
            }
        };
        assert_eq!(
            reason(make(Choice::new(
                "x".repeat(MAX_CHOICE_VALUE_BYTES + 1),
                "V"
            ))),
            ManifestReason::ChoiceValueTooLong {
                action: "go".into(),
                limit: MAX_CHOICE_VALUE_BYTES
            }
        );
        for lang in ["en", "ja", "fr"] {
            assert_eq!(
                reason(make(Choice::new(
                    "v",
                    Tr::new("V").with(lang, "界".repeat(konoma_plugin::text::MAX_TEXT_CHARS + 1))
                ))),
                ManifestReason::ChoiceLabelTooLong {
                    action: "go".into(),
                    limit: konoma_plugin::text::MAX_TEXT_CHARS
                }
            );
        }
    }
}

#[test]
fn project_row_builders_keep_existing_defaults_and_fields() {
    let row = ListRow::new("file", vec![]);
    assert!(!row.marked);
    assert_eq!(row.checked, None);
    assert_eq!(row.target, None);
    let row = row.target("/file").checked(false).marked(true);
    assert!(row.marked);
    assert_eq!(row.checked, Some(false));
    assert_eq!(row.target.as_deref(), Some(std::path::Path::new("/file")));
}

#[cfg(feature = "proc")]
#[test]
fn project_server_refusals_report_the_actual_versions_and_catch_manifest_panics() {
    use konoma_plugin::proc::wire::path_to_wire;
    use konoma_plugin::proc::{
        frame,
        proto::{Hello, HelloReply},
        serve_io, ServeOptions,
    };
    struct Server {
        project: bool,
        panic: bool,
    }
    impl Plugin for Server {
        fn manifest(&self) -> Manifest {
            assert!(!self.panic, "manifest boom");
            if self.project {
                manifest()
            } else {
                Manifest::new("p", "1")
            }
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported("unused"))
        }
    }
    for (project, panic, expected, text) in [
        (
            false,
            false,
            vec![1, 2],
            "the host speaks protocol versions [99], this plugin speaks protocol versions [1, 2]",
        ),
        (
            true,
            false,
            vec![2],
            "the host speaks protocol versions [99], this plugin speaks protocol versions [2]",
        ),
        (false, true, vec![], "the plugin crashed"),
    ] {
        let mut input = Vec::new();
        frame::send(
            &mut input,
            &Hello {
                versions: vec![99],
                host_version: "test".into(),
                side_dir: path_to_wire(std::path::Path::new("/unused")),
                lang: "en".into(),
            },
        )
        .unwrap();
        let mut output = Vec::new();
        serve_io(
            Server { project, panic },
            std::io::Cursor::new(input),
            &mut output,
            &ServeOptions::default(),
        )
        .unwrap();
        let HelloReply::Refused { supported, reason } =
            frame::recv(&mut std::io::Cursor::new(output))
                .unwrap()
                .unwrap()
        else {
            panic!("expected refusal")
        };
        assert_eq!(supported, expected);
        assert_eq!(reason, text);
    }
}

#[cfg(feature = "proc")]
#[test]
fn infallible_wire_decoder_documents_its_lossy_contract() {
    use konoma_plugin::proc::wire::{
        outcome_from_wire, outcome_from_wire_checked, path_to_wire, WOutcome,
    };
    let bad = WOutcome::OpenFile {
        path: path_to_wire(std::path::Path::new("relative")),
        line: None,
    };
    assert!(outcome_from_wire_checked(bad.clone()).is_err());
    assert_eq!(outcome_from_wire(bad), Outcome::Unhandled);
    // This public compatibility helper must warn readers before they choose it for a transport.
    let source = include_str!("../../src/proc/wire.rs");
    let docs = source
        .split("pub fn outcome_from_wire(")
        .next()
        .unwrap()
        .rsplit("\n\n")
        .next()
        .unwrap();
    assert!(docs.contains("Malformed navigation data becomes `Unhandled`"));
    assert!(docs.contains("[`outcome_from_wire_checked`]"));
}
