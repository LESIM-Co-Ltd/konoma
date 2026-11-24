use konoma_plugin::registry::{PluginOrigin, Registry};
use konoma_plugin::*;
use std::sync::Arc;

struct Decl(Manifest);
impl Plugin for Decl {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported("fixture"))
    }
}

fn register(manifest: Manifest) -> Result<Registry, String> {
    let mut registry = Registry::new();
    registry
        .register(Arc::new(Decl(manifest)), PluginOrigin::User)
        .map_err(|e| e.to_string())?;
    Ok(registry)
}

fn copy_project() -> ProjectDecl {
    ProjectDecl::new()
        .marker(DetectionMarker::new(".copy-project", MarkerKind::File))
        .screen(ProjectScreen::new("log", "Log").row_kind("commit"))
        .screen(ProjectScreen::new("text", "Text"))
        .screen(ProjectScreen::new("canvas", "Canvas"))
}

#[test]
fn copy_manifest_rejects_menu_closing_keys_with_the_action_name() {
    for key in ["q", "Esc", "\u{1b}"] {
        let error = register(
            Manifest::new("p", "1").project(copy_project()).action(
                Act::new("copy-close")
                    .copy()
                    .key(key)
                    .project_context(ActContext::screen("log", ["commit"])),
            ),
        )
        .err()
        .expect("menu closing keys must reject the manifest");
        assert!(error.contains("copy-close"), "{error}");
        assert!(error.contains("one plain character"), "{error}");
        assert!(error.contains("menu closing keys"), "{error}");
    }
}

#[test]
fn copy_manifest_rejects_items_outside_project_screens() {
    for context in [
        None,
        Some(ActContext::TreeSelection),
        Some(ActContext::Diff),
    ] {
        for view in [None, Some("text"), Some("image")] {
            let mut act = Act::new("copy-outside").copy().key("t");
            act.project_context = context.clone();
            act.screen = view.map(str::to_owned);
            let error = register(Manifest::new("p", "1").project(copy_project()).action(act))
                .err()
                .expect("copy items belong to project screens");
            assert!(error.contains("copy-outside"), "{error}");
            assert!(
                error.contains("copy items belong to a project screen"),
                "{error}"
            );
        }
    }
    assert!(register(
        Manifest::new("p", "1").project(copy_project()).action(
            Act::new("copy-inside")
                .copy()
                .key("t")
                .project_context(ActContext::screen("log", ["commit"]))
        )
    )
    .is_ok());
}

#[test]
fn copy_manifest_rejects_invalid_menu_keys_naming_the_act() {
    for key in [
        None,
        Some(""),
        Some("ab"),
        Some("Ctrl-a"),
        Some("Enter"),
        Some("\n"),
        Some(" "),
    ] {
        let mut act = Act::new("copy-subject").copy();
        act.key = key.map(str::to_owned);
        let error = register(Manifest::new("p", "1").action(act))
            .err()
            .expect("invalid menu key must reject the manifest");
        assert!(error.contains("copy-subject"), "{error}");
        assert!(error.contains("copy"), "{error}");
    }
}

#[test]
fn copy_manifest_rejects_duplicate_menu_keys_in_the_same_context() {
    let item = |id: &str, context: &str| {
        Act::new(id)
            .copy()
            .key("t")
            .project_context(ActContext::screen(context, Vec::<String>::new()))
    };
    let manifest = Manifest::new("p", "1")
        .project(copy_project())
        .action(item("first", "text"))
        .action(item("second", "text"));
    let error = register(manifest).err().expect("duplicate must reject");
    assert!(
        error.contains("second") && error.contains("first"),
        "{error}"
    );
    assert!(register(
        Manifest::new("p", "1")
            .project(copy_project())
            .action(item("first", "text"))
            .action(item("second", "canvas"))
    )
    .is_ok());
    let project = ProjectDecl::new()
        .marker(DetectionMarker::new(".project", MarkerKind::File))
        .screen(
            ProjectScreen::new("log", "Log")
                .row_kind("commit")
                .row_kind("empty"),
        )
        .screen(ProjectScreen::new("detail", "Detail").pushed());
    let first = Act::new("first")
        .copy()
        .key("t")
        .project_context(ActContext::screen("log", ["commit"]));
    let second = Act::new("second")
        .copy()
        .key("t")
        .project_context(ActContext::screen("log", ["empty"]));
    let error = register(
        Manifest::new("p", "1")
            .project(project.clone())
            .action(first.clone())
            .action(second),
    )
    .err()
    .expect("one menu context even with distinct row kinds");
    assert!(error.contains("first") && error.contains("second"));
    assert!(register(
        Manifest::new("p", "1")
            .project(project)
            .action(first)
            .action(
                Act::new("second")
                    .copy()
                    .key("t")
                    .project_context(ActContext::screen("detail", Vec::<String>::new()))
            )
    )
    .is_ok());
}

#[test]
fn copy_keys_are_not_direct_keys_and_survive_reserved_and_direct_collisions() {
    let manifest = Manifest::new("p", "1")
        .project(copy_project())
        .shows(ViewKind::Text)
        .action(
            Act::new("copy-hash")
                .copy()
                .key("h")
                .project_context(ActContext::screen("log", ["commit"])),
        )
        .action(Act::new("direct-h").key("h"))
        .action(
            Act::new("copy-x")
                .copy()
                .key("x")
                .project_context(ActContext::screen("log", ["commit"])),
        )
        .action(
            Act::new("direct-x")
                .key("x")
                .project_context(ActContext::screen("log", ["commit"])),
        );
    let registry = register(manifest).unwrap();
    let actions = &registry.manifest("p").unwrap().actions;
    assert_eq!(actions[0].key.as_deref(), Some("h"));
    assert_eq!(actions[1].key, None, "direct reserved key is still dropped");
    assert_eq!(actions[2].key.as_deref(), Some("x"));
    assert_eq!(actions[3].key.as_deref(), Some("x"));
}

#[cfg(feature = "proc")]
#[test]
fn copy_wire_manifest_round_trip_and_old_json() {
    use konoma_plugin::proc::{frame, wire::*};
    let manifest = Manifest::new("p", "1")
        .action(Act::new("copy-text").copy().key("t"))
        .action(Act::new("direct").key("x"));
    let wire = manifest_to_wire(&manifest).unwrap();
    assert_eq!(manifest_protocol(&wire), 2);
    let mut json = serde_json::to_value(&wire).unwrap();
    assert_eq!(json["actions"][0]["copy"], true);
    assert!(
        json["actions"][1].get("copy").is_none(),
        "legacy bytes stay unchanged"
    );
    let decoded: WManifest = frame::decode(&frame::encode(&wire).unwrap()).unwrap();
    let back = manifest_from_wire(decoded).unwrap();
    assert_eq!(back.actions, manifest.actions);
    json["actions"][0].as_object_mut().unwrap().remove("copy");
    let old = manifest_from_wire(serde_json::from_value(json).unwrap()).unwrap();
    assert!(!old.actions[0].copy);
    assert!(!old.actions[1].copy);
    assert_eq!(manifest_protocol(&manifest_to_wire(&old).unwrap()), 1);
}

#[cfg(feature = "proc")]
#[test]
fn copy_wire_outcome_round_trip_empty_multiline_and_old_json() {
    use konoma_plugin::proc::{frame, wire::*};
    for text in ["", "Subject\n\n本文 日本語"] {
        let outcome = Outcome::Copy { text: text.into() };
        let wire = outcome_to_wire(&outcome);
        assert_eq!(outcome_protocol(&wire), 2);
        let bytes = frame::encode(&wire).unwrap();
        assert_eq!(
            outcome_from_wire_checked(frame::decode(&bytes).unwrap()).unwrap(),
            outcome
        );
        let json = serde_json::to_string(&wire).unwrap();
        assert_eq!(
            outcome_from_wire_checked(serde_json::from_str(&json).unwrap()).unwrap(),
            outcome
        );
    }
    for (json, expected) in [
        ("\"Unhandled\"", Outcome::Unhandled),
        ("\"Redraw\"", Outcome::Redraw),
        ("\"Reload\"", Outcome::Reload),
    ] {
        assert_eq!(
            outcome_from_wire_checked(serde_json::from_str(json).unwrap()).unwrap(),
            expected
        );
    }
    let flash = Outcome::Flash(Tr::new("nothing").with("ja", "なし"));
    let old_json = r#"{"Flash":{"en":"nothing","others":[["ja","なし"]]}}"#;
    assert_eq!(
        outcome_from_wire_checked(serde_json::from_str(old_json).unwrap()).unwrap(),
        flash
    );
}

#[cfg(feature = "host")]
fn bindings() -> Vec<konoma_plugin::host::acts::ActBinding<&'static str>> {
    let manifest = Manifest::new("p", "1")
        .shows(ViewKind::Text)
        .action(
            Act::new("copy-text")
                .copy()
                .key("t")
                .label(Tr::new("subject").with("ja", "件名"))
                .footer(0),
        )
        .action(Act::new("direct").key("x").footer(1));
    konoma_plugin::host::bind::bind_actions("p", &manifest, None, |id| {
        if id == "direct" {
            "direct"
        } else {
            "copy-text"
        }
    })
    .0
}

#[cfg(feature = "host")]
#[test]
fn copy_binding_has_no_direct_key_or_direct_help_row() {
    use konoma_plugin::host::acts::*;
    let bindings = bindings();
    let here = acts_in(&bindings, "text");
    assert_eq!(bindings[0].key, None);
    assert!(act_for_key(&here, konoma_host_core::keymap::KeyPress::ch('t')).is_none());
    let rows = help_rows(&here, "en", |_| true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2.act, "direct");
}

#[cfg(feature = "host")]
#[test]
fn copy_items_are_absent_from_space_candidates() {
    use konoma_plugin::host::acts::*;
    let bindings = bindings();
    let here = acts_in(&bindings, "text");
    assert_eq!(
        menu_candidates(&here),
        vec![("direct", Some(konoma_host_core::keymap::KeyPress::ch('x')))]
    );
}

#[cfg(feature = "host")]
#[test]
fn copy_items_have_no_footer_even_if_a_priority_was_declared() {
    use konoma_plugin::host::acts::*;
    let bindings = bindings();
    let here = acts_in(&bindings, "text");
    assert_eq!(
        footer_order(&here)
            .iter()
            .map(|b| b.act)
            .collect::<Vec<_>>(),
        ["direct"]
    );
    assert_eq!(
        footer_hints(&here)
            .iter()
            .map(|h| h.members[0].act)
            .collect::<Vec<_>>(),
        ["direct"]
    );
}

#[cfg(feature = "host")]
#[test]
fn copy_menu_keys_are_unaffected_by_direct_key_overrides() {
    use konoma_host_core::keymap::KeyPress;
    let manifest = Manifest::new("p", "1")
        .action(Act::new("copy").copy().key("t"))
        .action(Act::new("direct").key("t"));
    let user = [
        ("t".to_string(), "noop".to_string()),
        ("x".to_string(), "direct".to_string()),
    ]
    .into();
    let (bindings, notes) =
        konoma_plugin::host::bind::bind_actions("p", &manifest, Some(&user), str::to_owned);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(bindings[0].copy_key, Some(KeyPress::ch('t')));
    assert_eq!(bindings[0].key, None);
    assert_eq!(bindings[1].key, Some(KeyPress::ch('x')));
}
