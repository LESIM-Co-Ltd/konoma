use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use konoma_plugin::manifest::Target;
use konoma_plugin::prelude::*;
use konoma_plugin::registry::{
    FileFacts, PluginOrigin, Registry, RegistryError, Resolution, UserRule,
};

struct Stub(Manifest);

impl Plugin for Stub {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported(Tr::new("stub")))
    }
}

fn stub(m: Manifest) -> Arc<dyn Plugin> {
    Arc::new(Stub(m))
}

fn resolve(
    reg: &Registry,
    rules: &[UserRule],
    path: &str,
    mime: Option<&str>,
    text: bool,
) -> Resolution {
    let m = mime.map(str::to_string);
    reg.resolve_file(
        rules,
        &FileFacts {
            path: Path::new(path),
            sniff_mime: &move || m.clone(),
            is_text: &move || text,
        },
    )
}

fn plugin(id: &str) -> Resolution {
    Resolution::Plugin {
        id: id.to_string(),
        hint: None,
    }
}

fn rule(glob: &str, plugin: &str) -> UserRule {
    UserRule {
        target: Target::Glob(glob.into()),
        plugin: plugin.into(),
        hint: None,
    }
}

#[test]
fn standard_default_binding_wins_over_text() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("img", "1").bind(Bind::files(["*.png"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(resolve(&r, &[], "/x/a.png", None, true), plugin("img"));
}

#[test]
fn glob_is_case_insensitive_and_file_name_only() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("md", "1").bind(Bind::files(["*.{md,markdown}"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(resolve(&r, &[], "/x/README.MD", None, true), plugin("md"));
    assert_eq!(resolve(&r, &[], "/x/a.markdown", None, true), plugin("md"));
    // A directory called x.md does not make y a match.
    assert_eq!(resolve(&r, &[], "/x.md/y", None, true), Resolution::Text);
}

#[test]
fn file_name_target_matches_exactly_ignoring_case() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("mk", "1").bind(Bind::file_names(["Makefile"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(resolve(&r, &[], "/x/makefile", None, true), plugin("mk"));
    assert_eq!(
        resolve(&r, &[], "/x/Makefile.bak", None, true),
        Resolution::Text
    );
}

#[test]
fn user_rules_come_first_and_carry_the_hint() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("a", "1").bind(Bind::files(["*.csv"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(stub(Manifest::new("b", "1")), PluginOrigin::Standard)
        .unwrap();
    let rules = [UserRule {
        hint: Some("tsv".into()),
        ..rule("*.csv", "b")
    }];
    assert_eq!(
        resolve(&r, &rules, "/x/a.csv", None, true),
        Resolution::Plugin {
            id: "b".into(),
            hint: Some("tsv".into())
        }
    );
}

#[test]
fn rule_to_disabled_or_unknown_plugin_is_skipped() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("a", "1").bind(Bind::files(["*.csv"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(stub(Manifest::new("b", "1")), PluginOrigin::Standard)
        .unwrap();
    r.set_enabled("b", false);
    let rules = [rule("*.csv", "b"), rule("*.csv", "ghost")];
    assert_eq!(resolve(&r, &rules, "/x/a.csv", None, true), plugin("a"));
    // The second rule is reached after the skipped first one.
    r.set_enabled("b", true);
    assert_eq!(resolve(&r, &rules, "/x/a.csv", None, true), plugin("b"));
}

#[test]
fn disabled_plugin_does_not_take_part() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("a", "1").bind(Bind::files(["*.csv"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert!(r.set_enabled("a", false));
    assert!(!r.set_enabled("nope", false));
    assert_eq!(resolve(&r, &[], "/x/a.csv", None, true), Resolution::Text);
}

#[test]
fn user_plugin_beats_standard_regardless_of_registration_order() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("std", "1").bind(Bind::files(["*.md"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("mine", "1").bind(Bind::files(["*.md"]))),
        PluginOrigin::User,
    )
    .unwrap();
    assert_eq!(resolve(&r, &[], "/x/a.md", None, true), plugin("mine"));
}

#[test]
fn registration_order_decides_within_a_layer() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("one", "1").bind(Bind::files(["*.x"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("two", "1").bind(Bind::files(["*.x"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(resolve(&r, &[], "/a.x", None, false), plugin("one"));
}

#[test]
fn option_bindings_do_not_take_part() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("raw", "1").bind(Bind::files(["*.md"]).option())),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(resolve(&r, &[], "/a.md", None, true), Resolution::Text);
}

#[test]
fn text_then_can_not_preview_with_extension() {
    let r = Registry::new();
    assert_eq!(resolve(&r, &[], "/a.txt", None, true), Resolution::Text);
    assert_eq!(
        resolve(&r, &[], "/foo.xyz", None, false),
        Resolution::CanNotPreview { ext: "xyz".into() }
    );
    assert_eq!(
        resolve(&r, &[], "/x/Makefile", None, false),
        Resolution::CanNotPreview { ext: String::new() }
    );
}

#[test]
fn mime_binding_matches_by_prefix_and_exactly() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("img", "1").bind(Bind::mime("image/*"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("pdf", "1").bind(Bind::mime("application/pdf"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(
        resolve(&r, &[], "/noext", Some("image/png"), false),
        plugin("img")
    );
    assert_eq!(
        resolve(&r, &[], "/noext", Some("application/pdf"), false),
        plugin("pdf")
    );
    assert_eq!(
        resolve(&r, &[], "/noext", Some("video/mp4"), false),
        Resolution::CanNotPreview { ext: String::new() }
    );
    assert_eq!(
        resolve(&r, &[], "/noext", None, false),
        Resolution::CanNotPreview { ext: String::new() }
    );
}

#[test]
fn content_checks_run_only_when_needed_and_at_most_once() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("a", "1").bind(Bind::mime("image/*"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("b", "1").bind(Bind::mime("audio/*"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("c", "1").bind(Bind::files(["*.q"]))),
        PluginOrigin::Standard,
    )
    .unwrap();

    let mime_calls = Cell::new(0);
    let text_calls = Cell::new(0);
    let run = |path: &str| {
        mime_calls.set(0);
        text_calls.set(0);
        r.resolve_file(
            &[],
            &FileFacts {
                path: Path::new(path),
                sniff_mime: &|| {
                    mime_calls.set(mime_calls.get() + 1);
                    Some("video/mp4".into())
                },
                is_text: &|| {
                    text_calls.set(text_calls.get() + 1);
                    false
                },
            },
        )
    };
    // The name is enough only if a glob comes before any mime binding: here the mime bindings of
    // `a` and `b` come first, so the sniff happens, but exactly once for the two of them.
    run("/x.q");
    assert_eq!((mime_calls.get(), text_calls.get()), (1, 0));
    run("/x.unknown");
    assert_eq!((mime_calls.get(), text_calls.get()), (1, 1));

    // No mime binding at all: neither check runs when a glob settles it.
    let mut r2 = Registry::new();
    r2.register(
        stub(Manifest::new("c", "1").bind(Bind::files(["*.q"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    mime_calls.set(0);
    text_calls.set(0);
    let res = r2.resolve_file(
        &[],
        &FileFacts {
            path: Path::new("/x.q"),
            sniff_mime: &|| {
                mime_calls.set(mime_calls.get() + 1);
                None
            },
            is_text: &|| {
                text_calls.set(text_calls.get() + 1);
                true
            },
        },
    );
    assert_eq!(res, plugin("c"));
    assert_eq!((mime_calls.get(), text_calls.get()), (0, 0));
}

#[test]
fn duplicate_id_is_refused_and_the_first_stays() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("a", "1").bind(Bind::files(["*.one"]))),
        PluginOrigin::Standard,
    )
    .unwrap();
    let err = r
        .register(
            stub(Manifest::new("a", "2").bind(Bind::files(["*.two"]))),
            PluginOrigin::User,
        )
        .unwrap_err();
    assert_eq!(err, RegistryError::DuplicateId("a".into()));
    assert_eq!(r.ids(), vec!["a"]);
    assert_eq!(r.manifest("a").unwrap().version, "1");
    assert_eq!(
        resolve(&r, &[], "/x.two", None, false),
        Resolution::CanNotPreview { ext: "two".into() }
    );
}

#[test]
fn invalid_glob_is_refused() {
    let mut r = Registry::new();
    let err = r
        .register(
            stub(Manifest::new("a", "1").bind(Bind::files(["[unclosed"]))),
            PluginOrigin::Standard,
        )
        .unwrap_err();
    assert!(matches!(err, RegistryError::InvalidGlob { .. }));
    assert!(r.ids().is_empty());
}

#[test]
fn content_and_embed_resolution_prefers_user_and_skips_disabled() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("md", "1").bind(Bind::content("markdown"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("mmd", "1").bind(Bind::embed("fence:mermaid"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(r.resolve_content("markdown").as_deref(), Some("md"));
    assert_eq!(r.resolve_embed("fence:mermaid").as_deref(), Some("mmd"));
    assert_eq!(r.resolve_embed("math"), None);
    r.register(
        stub(Manifest::new("mymd", "1").bind(Bind::content("markdown"))),
        PluginOrigin::User,
    )
    .unwrap();
    assert_eq!(r.resolve_content("markdown").as_deref(), Some("mymd"));
    r.set_enabled("mymd", false);
    assert_eq!(r.resolve_content("markdown").as_deref(), Some("md"));
    // Content and embed bindings never match a file.
    assert_eq!(
        resolve(&r, &[], "/markdown", None, false),
        Resolution::CanNotPreview { ext: String::new() }
    );
}

#[test]
fn option_binding_is_not_used_for_content() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("md", "1").bind(Bind::content("markdown").option())),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(r.resolve_content("markdown"), None);
}

#[test]
fn describe_lists_every_target_with_owner_and_state() {
    let mut r = Registry::new();
    r.register(
        stub(
            Manifest::new("a", "1")
                .bind(Bind::files(["*.x", "*.y"]))
                .bind(Bind::mime("image/*").option()),
        ),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.set_enabled("a", false);
    let rows = r.describe();
    assert_eq!(rows.len(), 3);
    assert!(rows
        .iter()
        .all(|row| row.plugin == "a" && !row.enabled && row.origin == PluginOrigin::Standard));
    assert_eq!(rows[2].priority, konoma_plugin::Priority::Option);
    assert!(r.plugin("a").is_some() && r.plugin("zz").is_none());
}

#[test]
fn a_content_type_is_taken_by_the_plugin_that_binds_that_type_only() {
    let mut r = Registry::new();
    r.register(
        stub(Manifest::new("md", "1").bind(Bind::content("markdown"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("rst", "1").bind(Bind::content("restructured"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(r.resolve_content("markdown").as_deref(), Some("md"));
    assert_eq!(r.resolve_content("restructured").as_deref(), Some("rst"));
    assert_eq!(r.resolve_content("textile"), None);
    // The same for embedded kinds.
    r.register(
        stub(Manifest::new("m1", "1").bind(Bind::embed("math"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    r.register(
        stub(Manifest::new("m2", "1").bind(Bind::embed("fence:mermaid"))),
        PluginOrigin::Standard,
    )
    .unwrap();
    assert_eq!(r.resolve_embed("math").as_deref(), Some("m1"));
    assert_eq!(r.resolve_embed("fence:mermaid").as_deref(), Some("m2"));
    assert_eq!(r.resolve_embed("fence:dot"), None);
}
