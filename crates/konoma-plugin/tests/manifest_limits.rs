//! What registration does with the parts of a manifest that are not names: the number and the
//! length of the targets of a bind, the strings of a setting's default and of `Unavailable`.
//! Each guard is tried over the limit, on the limit and with control characters.

use std::collections::BTreeMap;
use std::sync::Arc;

use konoma_plugin::manifest::{Target, Unavailable};
use konoma_plugin::prelude::*;
use konoma_plugin::registry::{
    NoteKind, PluginOrigin, Registry, RegistryError, MAX_DEFAULT_DEPTH, MAX_DEFAULT_KEY_BYTES,
    MAX_DEFAULT_NODES, MAX_TARGETS_PER_BIND, MAX_TARGET_BYTES,
};
use konoma_plugin::text::{MAX_TEXT_CHARS, MAX_TR_LANGUAGES};

struct Stub(Manifest);

impl Plugin for Stub {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported(Tr::new("stub")))
    }
}

fn register(m: Manifest) -> (Registry, Result<(), RegistryError>) {
    let mut r = Registry::new();
    let res = r.register(Arc::new(Stub(m)), PluginOrigin::User);
    (r, res)
}

fn refused_why(m: Manifest) -> String {
    let (r, res) = register(m);
    match res {
        Err(RegistryError::InvalidManifest { why, .. }) => {
            assert!(r.ids().is_empty(), "a refused plugin is not registered");
            assert!(r
                .notes()
                .iter()
                .any(|n| matches!(n.kind, NoteKind::Refused { .. })));
            why
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn kept(m: Manifest) -> Manifest {
    let (r, res) = register(m);
    res.unwrap();
    r.manifest("p").unwrap().clone()
}

fn with_default(v: Value) -> Manifest {
    Manifest::new("p", "1").setting(SettingDecl::new("k", v, Tr::new("h")))
}

fn default_of(m: &Manifest) -> &Value {
    &m.settings[0].default
}

// ---- targets of a bind --------------------------------------------------------------------------

#[test]
fn too_many_targets_in_one_bind_refuse_the_plugin_and_exactly_the_limit_does_not() {
    let globs = |n: usize| (0..n).map(|i| format!("*.e{i}")).collect::<Vec<_>>();
    let why =
        refused_why(Manifest::new("p", "1").bind(Bind::files(globs(MAX_TARGETS_PER_BIND + 1))));
    assert!(why.contains("targets in a bind"), "{why}");
    // The count is per bind: many binds of few targets are fine (they have their own limit).
    let m = kept(Manifest::new("p", "1").bind(Bind::files(globs(MAX_TARGETS_PER_BIND))));
    assert_eq!(m.binds[0].targets.len(), MAX_TARGETS_PER_BIND);
    // The limit holds for every kind of target, not only globs.
    let names = (0..=MAX_TARGETS_PER_BIND).map(|i| format!("n{i}"));
    let why = refused_why(Manifest::new("p", "1").bind(Bind::file_names(names)));
    assert!(why.contains("targets in a bind"), "{why}");
    // The second bind is checked as well.
    let why = refused_why(
        Manifest::new("p", "1")
            .bind(Bind::files(["*.a"]))
            .bind(Bind::files(globs(MAX_TARGETS_PER_BIND + 1))),
    );
    assert!(why.contains("targets in a bind"), "{why}");
}

#[test]
fn a_target_that_is_too_long_refuses_the_plugin_and_one_on_the_limit_does_not() {
    let at = "a".repeat(MAX_TARGET_BYTES);
    let over = "a".repeat(MAX_TARGET_BYTES + 1);
    let kinds: [fn(String) -> Bind; 2] = [|s| Bind::files([s]), |s| Bind::file_names([s])];
    for mk in kinds {
        kept(Manifest::new("p", "1").bind(mk(at.clone())));
        let why = refused_why(Manifest::new("p", "1").bind(mk(over.clone())));
        assert!(why.contains("too long"), "{why}");
    }
    // A MIME type, a content type and an embed are targets too.
    for target in [
        Target::Mime(over.clone()),
        Target::Content(over.clone()),
        Target::Embed(over.clone()),
    ] {
        let mut m = Manifest::new("p", "1");
        m.binds.push(Bind::files(["*.x"]));
        m.binds[0].targets.push(target);
        assert!(refused_why(m).contains("too long"));
    }
}

#[test]
fn a_target_with_a_control_character_refuses_the_plugin_and_prints_none_of_it() {
    for bad in [
        "*.a\u{1b}[2J",
        "x\ny",
        "t\tt",
        "a\u{7f}",
        "a\u{85}",
        "a\u{10EEEE}",
    ] {
        let why = refused_why(Manifest::new("p", "1").bind(Bind::files([bad])));
        assert!(why.contains("control character"), "{bad:?}: {why}");
        assert!(
            !why.chars().any(|c| c.is_control() || c == '\u{10EEEE}'),
            "the message carries a control character: {why:?}"
        );
    }
}

// ---- the default of a setting -------------------------------------------------------------------

#[test]
fn a_string_default_is_cut_and_stripped_and_one_on_the_limit_is_untouched() {
    let at: String = "a".repeat(MAX_TEXT_CHARS);
    let m = kept(with_default(Value::Str(at.clone())));
    assert_eq!(default_of(&m), &Value::Str(at));
    let over = "b".repeat(MAX_TEXT_CHARS + 1);
    let m = kept(with_default(Value::Str(over)));
    let Value::Str(s) = default_of(&m) else {
        panic!()
    };
    assert_eq!(s.chars().count(), MAX_TEXT_CHARS);
    assert!(s.ends_with('…'));
    // Control characters other than a newline and a tab go; so does U+10EEEE.
    let m = kept(with_default(Value::Str(
        "a\u{1b}[2Jb\u{7}c\nd\te\u{7f}f\u{85}g\u{10EEEE}h".into(),
    )));
    assert_eq!(default_of(&m), &Value::Str("a[2Jbc\nd\tefgh".into()));
    // Other types are as declared.
    for v in [Value::Bool(true), Value::Int(i64::MIN), Value::Float(1.5)] {
        assert_eq!(default_of(&kept(with_default(v.clone()))), &v);
    }
}

#[test]
fn strings_inside_a_list_or_table_default_are_cleaned_and_the_keys_left_alone() {
    let dirty = "x\u{1b}y".to_string();
    let long = "z".repeat(MAX_TEXT_CHARS + 10);
    let table: BTreeMap<String, Value> = [
        ("key".to_string(), Value::Str(dirty.clone())),
        ("other".to_string(), Value::List(vec![Value::Str(long)])),
    ]
    .into();
    let m = kept(with_default(Value::List(vec![
        Value::Str(dirty),
        Value::Int(3),
        Value::Table(table),
    ])));
    let Value::List(items) = default_of(&m) else {
        panic!()
    };
    assert_eq!(items[0], Value::Str("xy".into()));
    assert_eq!(items[1], Value::Int(3));
    let Value::Table(t) = &items[2] else { panic!() };
    assert_eq!(t.get("key"), Some(&Value::Str("xy".into())));
    let Some(Value::List(inner)) = t.get("other") else {
        panic!()
    };
    let Value::Str(s) = &inner[0] else { panic!() };
    assert_eq!(s.chars().count(), MAX_TEXT_CHARS);
}

#[test]
fn a_default_with_too_many_values_refuses_the_plugin_and_one_on_the_limit_does_not() {
    // The list itself counts as one value.
    let list = |n: usize| Value::List((0..n).map(|i| Value::Int(i as i64)).collect());
    let m = kept(with_default(list(MAX_DEFAULT_NODES - 1)));
    let Value::List(l) = default_of(&m) else {
        panic!()
    };
    assert_eq!(l.len(), MAX_DEFAULT_NODES - 1);
    let why = refused_why(with_default(list(MAX_DEFAULT_NODES)));
    assert!(why.contains("setting default"), "{why}");
    // A table counts the same way, and so do the values below its entries.
    let table: BTreeMap<String, Value> = (0..MAX_DEFAULT_NODES)
        .map(|i| (format!("k{i}"), Value::Bool(true)))
        .collect();
    assert!(refused_why(with_default(Value::Table(table))).contains("setting default"));
    let nested = Value::List(vec![
        list(MAX_DEFAULT_NODES / 2),
        list(MAX_DEFAULT_NODES / 2),
    ]);
    assert!(refused_why(with_default(nested)).contains("setting default"));
}

#[test]
fn a_default_nested_too_deep_refuses_the_plugin_and_one_on_the_limit_does_not() {
    let nest = |levels: usize| {
        let mut v = Value::Int(1);
        for _ in 0..levels {
            v = Value::List(vec![v]);
        }
        v
    };
    kept(with_default(nest(MAX_DEFAULT_DEPTH)));
    let why = refused_why(with_default(nest(MAX_DEFAULT_DEPTH + 1)));
    assert!(why.contains("nested"), "{why}");
    // Tables nest the same way.
    let mut v = Value::Int(1);
    for _ in 0..=MAX_DEFAULT_DEPTH {
        v = Value::Table([("k".to_string(), v)].into());
    }
    assert!(refused_why(with_default(v)).contains("nested"));
}

// ---- the strings of `Unavailable` ---------------------------------------------------------------

fn notice_of(m: &Manifest) -> &Tr {
    let Unavailable::Notice(t) = &m.unavailable else {
        panic!("a notice")
    };
    t
}

#[test]
fn the_notice_of_unavailable_is_cut_stripped_and_limited_in_languages() {
    let mk = |t: Tr| Manifest::new("p", "1").unavailable(Unavailable::Notice(t));
    // On the limit: untouched, in every language.
    let at = "n".repeat(MAX_TEXT_CHARS);
    let m = kept(mk(Tr::new(at.clone()).ja(at.clone())));
    assert_eq!(notice_of(&m).en(), at);
    assert_eq!(notice_of(&m).get("ja"), at);
    // Over the limit.
    let m = kept(mk(Tr::new("n".repeat(MAX_TEXT_CHARS + 1))));
    assert_eq!(notice_of(&m).en().chars().count(), MAX_TEXT_CHARS);
    assert!(notice_of(&m).en().ends_with('…'));
    // Control characters, in English and in another language.
    let m = kept(mk(Tr::new("a\u{1b}[2Jb\u{10EEEE}\nc").ja("\u{7}日本\u{85}")));
    assert_eq!(notice_of(&m).en(), "a[2Jb\nc");
    assert_eq!(notice_of(&m).get("ja"), "日本");
    // More languages than the host takes: the first ones stay, the rest go.
    let mut t = Tr::new("e");
    for i in 0..(MAX_TR_LANGUAGES + 5) {
        t = t.with(format!("l{i:02}"), format!("t{i}"));
    }
    let m = kept(mk(t));
    let n = notice_of(&m);
    assert_eq!(n.get("l00"), "t0");
    assert_eq!(
        n.get(&format!("l{:02}", MAX_TR_LANGUAGES - 1)),
        format!("t{}", MAX_TR_LANGUAGES - 1)
    );
    assert_eq!(
        n.get(&format!("l{:02}", MAX_TR_LANGUAGES)),
        "e",
        "falls back to English"
    );
}

#[test]
fn the_other_unavailable_forms_carry_no_text_and_are_kept_as_declared() {
    for u in [
        Unavailable::Message,
        Unavailable::Picture,
        Unavailable::RawText { with_reason: true },
        Unavailable::RawText { with_reason: false },
    ] {
        let m = kept(Manifest::new("p", "1").unavailable(u.clone()));
        assert_eq!(m.unavailable, u);
    }
}

// ---- the boundary is in bytes, and the message never echoes the target --------------------------

#[test]
fn the_target_limit_is_in_bytes_at_a_multibyte_boundary() {
    // 'あ' is 3 bytes; 1024 = 341 * 3 + 1.
    let at: String = "あ".repeat(MAX_TARGET_BYTES / 3) + "a";
    assert_eq!(at.len(), MAX_TARGET_BYTES);
    kept(Manifest::new("p", "1").bind(Bind::files([at.clone()])));
    // One byte over, the boundary falling on a multi-byte character.
    let over = "あ".repeat(MAX_TARGET_BYTES / 3 + 1);
    assert_eq!(over.len(), MAX_TARGET_BYTES + 2);
    let why = refused_why(Manifest::new("p", "1").bind(Bind::file_names([over])));
    assert!(why.contains("too long"), "{why}");
    let one_over = "あ".repeat(MAX_TARGET_BYTES / 3) + "ab";
    assert_eq!(one_over.len(), MAX_TARGET_BYTES + 1);
    refused_why(Manifest::new("p", "1").bind(Bind::file_names([one_over])));
}

#[test]
fn a_long_brace_glob_of_many_extensions_is_accepted() {
    let exts: Vec<String> = (0..120).map(|i| format!("ext{i}")).collect();
    let glob = format!("*.{{{}}}", exts.join(","));
    assert!(
        glob.len() > 256 && glob.len() < MAX_TARGET_BYTES,
        "{}",
        glob.len()
    );
    let m = kept(Manifest::new("p", "1").bind(Bind::files([glob])));
    assert_eq!(m.binds[0].targets.len(), 1);
}

#[test]
fn a_refusal_names_the_bind_and_the_reason_but_never_the_target_text() {
    for bad in ["*.a\u{7}gnp", "x\u{1b}[2J", "q\u{10EEEE}", "y\u{85}"] {
        let why = refused_why(
            Manifest::new("p", "1")
                .bind(Bind::files(["*.ok"]))
                .bind(Bind::files([bad])),
        );
        assert!(why.contains("bind 1"), "{why}");
        assert!(why.contains("control character"), "{why}");
        assert!(!why.contains(bad), "{why:?}");
        assert!(!why.contains("gnp"), "{why:?}");
        assert!(!why.contains("*.ok"), "{why:?}");
    }
    // Too long: the byte count, not the text.
    let long = format!("zz\u{202E}{}", "q".repeat(MAX_TARGET_BYTES));
    let why = refused_why(Manifest::new("p", "1").bind(Bind::files([long.clone()])));
    assert!(
        why.contains("bind 0") && why.contains(&long.len().to_string()),
        "{why}"
    );
    assert!(!why.contains("qqq") && !why.contains('\u{202E}'), "{why:?}");
}

#[test]
fn a_table_key_of_a_default_with_a_control_character_or_too_long_refuses_the_plugin() {
    let one = |k: String| with_default(Value::Table([(k, Value::Int(1))].into()));
    for bad in ["k\u{7}ey", "k\u{1b}", "k\u{10EEEE}", "a\nb", "k\u{85}"] {
        let why = refused_why(one(bad.to_string()));
        assert!(why.contains("key"), "{why}");
        assert!(!why.contains(bad), "{why:?}");
    }
    let over = "k".repeat(MAX_DEFAULT_KEY_BYTES + 1);
    assert!(refused_why(one(over.clone())).contains("key"));
    // A key inside a nested table and a list is checked too.
    let nested = Value::List(vec![Value::Table(
        [("a\u{7}".to_string(), Value::Int(1))].into(),
    )]);
    assert!(refused_why(with_default(nested)).contains("key"));
    // On the limit it is fine, and keys are never changed: distinct keys stay distinct.
    let at = "k".repeat(MAX_DEFAULT_KEY_BYTES);
    let table: BTreeMap<String, Value> = [
        (at.clone(), Value::Int(1)),
        (format!("{}x", &at[1..]), Value::Int(2)),
        ("plain".to_string(), Value::Int(3)),
    ]
    .into();
    let m = kept(with_default(Value::Table(table.clone())));
    assert_eq!(default_of(&m), &Value::Table(table));
}
