use konoma_plugin::registry::{ManifestReason, PluginOrigin, Registry, RegistryError};
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

fn check(m: Manifest) -> Result<Registry, ManifestReason> {
    let mut registry = Registry::new();
    match registry.register(Arc::new(Decl(m)), PluginOrigin::User) {
        Ok(()) => Ok(registry),
        Err(RegistryError::InvalidManifest { reason, .. }) => Err(reason),
        other => panic!("{other:?}"),
    }
}

fn grouped() -> Manifest {
    Manifest::new("p", "1")
        .hint_group(HintGroup::new("stage", "stage(all)"))
        .action(Act::new("stage").hint_group("stage"))
        .action(Act::new("stage-all").hint_group("stage"))
}

#[test]
fn hint_group_unknown_is_rejected_with_action_and_group() {
    let reason = check(Manifest::new("p", "1").action(Act::new("stage").hint_group("missing")))
        .err()
        .expect("refused");
    assert_eq!(
        reason.to_string(),
        "action `stage` names unknown hint group `missing`"
    );
}

#[test]
fn hint_group_duplicate_is_rejected() {
    let reason = check(grouped().hint_group(HintGroup::new("stage", "other")))
        .err()
        .expect("refused");
    assert_eq!(
        reason.to_string(),
        "hint group `stage` is declared twice (action `stage`)"
    );
    assert!(check(
        Manifest::new("p", "1")
            .hint_group(HintGroup::new("empty", "one"))
            .hint_group(HintGroup::new("empty", "two"))
    )
    .is_err());
}

#[test]
fn hint_group_different_contexts_are_rejected() {
    for context in [None, Some("canvas")] {
        let mut m = grouped();
        m.actions[0].screen = Some("text".into());
        m.actions[1].screen = context.map(str::to_owned);
        let reason = check(m).err().expect("refused");
        assert_eq!(
            reason.to_string(),
            "action `stage-all` has a different context in hint group `stage`"
        );
    }
}

#[test]
fn hint_group_different_project_screens_are_rejected() {
    for context in [
        None,
        Some(ActContext::TreeSelection),
        Some(ActContext::Diff),
        Some(ActContext::screen("branches", Vec::<String>::new())),
    ] {
        let mut m = grouped().project(
            ProjectDecl::new()
                .marker(DetectionMarker::new(".project", MarkerKind::File))
                .screen(ProjectScreen::new("changes", "Changes"))
                .screen(ProjectScreen::new("branches", "Branches")),
        );
        m.actions[0].project_context = Some(ActContext::screen("changes", Vec::<String>::new()));
        m.actions[1].project_context = context;
        let reason = check(m).err().expect("refused");
        assert_eq!(
            reason.to_string(),
            "action `stage-all` has a different project screen in hint group `stage`"
        );
    }
}

#[test]
fn hint_groups_accept_same_context_different_row_kinds_and_empty_groups() {
    let mut m = grouped().project(
        ProjectDecl::new()
            .marker(DetectionMarker::new(".project", MarkerKind::File))
            .screen(ProjectScreen::new("changes", "Changes").row_kind("file")),
    );
    m.actions[0].project_context = Some(ActContext::screen("changes", ["file"]));
    m.actions[1].project_context = Some(ActContext::screen("changes", Vec::<String>::new()));
    let m = m.hint_group(HintGroup::new("empty", "unused"));
    let kept = check(m.clone()).unwrap();
    assert_eq!(kept.manifest("p").unwrap().hint_groups, m.hint_groups);
    for context in [None, Some("text")] {
        let mut m = grouped();
        for action in &mut m.actions {
            action.screen = context.map(str::to_owned);
        }
        check(m).unwrap();
    }
}

#[cfg(feature = "proc")]
#[test]
fn hint_groups_and_membership_round_trip() {
    use konoma_plugin::proc::{manifest_from_json, manifest_to_json};
    let m = Manifest::new("p", "1")
        .hint_group(
            HintGroup::new("stage", Tr::new("stage(all)").with("ja", "stage-ja"))
                .help(Tr::new("stage / stage all").with("ja", "help-ja")),
        )
        .action(Act::new("stage").hint_group("stage"))
        .action(Act::new("stage-all").hint_group("stage"));
    let back = manifest_from_json(&manifest_to_json(&m).unwrap()).unwrap();
    assert_eq!(back.hint_groups, m.hint_groups);
    assert_eq!(back.actions, m.actions);
    use konoma_plugin::proc::wire::{manifest_from_wire, manifest_to_wire, WManifest};
    let wire = manifest_to_wire(&m).unwrap();
    let bytes = rmp_serde::to_vec_named(&wire).unwrap();
    let decoded: WManifest = rmp_serde::from_slice(&bytes).unwrap();
    let back = manifest_from_wire(decoded).unwrap();
    assert_eq!(back.hint_groups, m.hint_groups);
    assert_eq!(back.actions, m.actions);
}

#[cfg(feature = "proc")]
#[test]
fn hint_groups_older_child_defaults() {
    use konoma_plugin::proc::{manifest_from_json, manifest_to_json};
    let json = manifest_to_json(&Manifest::new("p", "1").action(Act::new("stage"))).unwrap();
    assert!(!json.contains("hint_group"));
    let back = manifest_from_json(&json).unwrap();
    assert!(back.hint_groups.is_empty());
    assert_eq!(back.actions[0].hint_group, None);
}

#[cfg(feature = "host")]
mod host {
    use super::*;
    use konoma_host_core::keymap::KeyPress;
    use konoma_plugin::host::{acts, bind::bind_actions};

    fn manifest() -> Manifest {
        Manifest::new("p", "1")
            .shows(ViewKind::Canvas)
            .hint_group(HintGroup::new("stage", "stage(all)").help("stage / stage all"))
            .hint_group(HintGroup::new("unstage", "unstage(all)"))
            .action(Act::new("before").key("x").footer(0))
            .action(Act::new("stage").key("s").footer(8).hint_group("stage"))
            .action(Act::new("between").key("v").footer(4))
            .action(Act::new("stage-all").key("S").footer(2).hint_group("stage"))
            .action(Act::new("help-only").key("h").hint_group("stage"))
            .action(Act::new("unstage").key("u").footer(9).hint_group("unstage"))
            .action(
                Act::new("unstage-all")
                    .key("U")
                    .footer(9)
                    .hint_group("unstage"),
            )
    }

    #[test]
    fn hint_group_footer_priority_keys_gating_and_rebinding() {
        let user = [("W".into(), "stage-all".into())].into();
        let (bound, notes) = bind_actions("p", &manifest(), Some(&user), str::to_owned);
        assert!(notes.is_empty(), "{notes:?}");
        let refs: Vec<_> = bound.iter().collect();
        let hints = acts::footer_hints(&refs);
        assert_eq!(
            hints.iter().map(|h| h.label("en")).collect::<Vec<_>>(),
            ["before", "stage(all)", "between", "unstage(all)"]
        );
        let stage = &hints[1];
        assert_eq!(stage.keys(|_| true), [KeyPress::ch('s'), KeyPress::ch('W')]);
        assert_eq!(stage.keys(|b| b.act == "stage-all"), [KeyPress::ch('W')]);
        assert!(stage.keys(|_| false).is_empty());
    }

    #[test]
    fn hint_group_help_order_keys_fallback_and_menu_entries() {
        let (bound, _) = bind_actions("p", &manifest(), None, str::to_owned);
        let refs: Vec<_> = bound.iter().collect();
        let rows = acts::help_rows(&refs, "en", |_| true);
        assert_eq!(
            rows.iter().map(|r| r.1).collect::<Vec<_>>(),
            ["before", "stage / stage all", "between", "unstage(all)"]
        );
        assert_eq!(
            rows[1].0,
            [KeyPress::ch('s'), KeyPress::ch('S'), KeyPress::ch('h')]
        );
        let rows = acts::help_rows(&refs, "en", |b| b.act == "stage-all");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, [KeyPress::ch('S')]);
        assert!(acts::help_rows(&refs, "en", |_| false).is_empty());
        let m = manifest();
        let (menu, _) = bind_actions("p", &m, None, |id| {
            m.actions.iter().position(|a| a.id == id).unwrap()
        });
        assert_eq!(
            acts::menu_candidates(&menu.iter().collect::<Vec<_>>()).len(),
            bound.len()
        );
    }

    #[test]
    fn hint_groups_are_scoped_to_the_plugin() {
        let (mut bound, _) = bind_actions("p", &manifest(), None, str::to_owned);
        let (other, _) = bind_actions("other", &manifest(), None, str::to_owned);
        bound.extend(other);
        let refs: Vec<_> = bound.iter().collect();
        assert_eq!(acts::footer_hints(&refs).len(), 8);
        assert_eq!(acts::help_rows(&refs, "en", |_| true).len(), 8);
    }

    #[test]
    fn hint_group_footer_ties_use_the_first_minimum_priority_member() {
        let mut m = manifest();
        m.actions[2].footer = Some(2);
        let (bound, _) = bind_actions("p", &m, None, str::to_owned);
        let refs: Vec<_> = bound.iter().collect();
        let hints = acts::footer_hints(&refs);
        assert_eq!(
            hints.iter().map(|h| h.label("en")).collect::<Vec<_>>(),
            ["before", "between", "stage(all)", "unstage(all)"]
        );
    }

    #[test]
    fn hint_group_shared_key_is_deduplicated_in_footer_and_help() {
        let (mut bound, _) = bind_actions("p", &manifest(), None, str::to_owned);
        // Exercise colliding bindings at the rendering boundary, after normal validation.
        bound[3].key = bound[1].key;
        let refs: Vec<_> = bound.iter().collect();
        let hints = acts::footer_hints(&refs);
        let stage = &hints[1];
        assert_eq!(
            stage
                .members
                .iter()
                .map(|b| b.act.as_str())
                .collect::<Vec<_>>(),
            ["stage", "stage-all"]
        );
        assert_eq!(stage.keys(|_| true), [KeyPress::ch('s')]);
        for enabled in ["stage", "stage-all"] {
            assert_eq!(stage.keys(|b| b.act == enabled), [KeyPress::ch('s')]);
        }
        assert!(stage.keys(|_| false).is_empty());
        let rows = acts::help_rows(&refs, "en", |_| true);
        assert_eq!(rows[1].0, [KeyPress::ch('s'), KeyPress::ch('h')]);
    }
}
