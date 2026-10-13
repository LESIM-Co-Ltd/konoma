use konoma_plugin::keys::{
    forbidden_defaults, parse_key, reserved, validate, Effect, KeyCode, KeyError, KeyIssueKind,
    KeySpec, Layer,
};
use konoma_plugin::prelude::*;

#[test]
fn parses_the_notations() {
    assert_eq!(parse_key("w").unwrap(), KeySpec::ch('w'));
    assert_eq!(parse_key("W").unwrap(), KeySpec::ch('W'));
    assert_eq!(parse_key("Ctrl-t").unwrap(), KeySpec::ctrl('t'));
    assert_eq!(parse_key("ctrl-t").unwrap(), KeySpec::ctrl('t'));
    assert_eq!(parse_key("c-t").unwrap(), KeySpec::ctrl('t'));
    assert_eq!(parse_key("Enter").unwrap(), KeySpec::key(KeyCode::Enter));
    assert_eq!(parse_key("enter").unwrap(), KeySpec::key(KeyCode::Enter));
    assert_eq!(parse_key("Tab").unwrap(), KeySpec::key(KeyCode::Tab));
    assert_eq!(
        parse_key("Shift-Tab").unwrap(),
        KeySpec::key(KeyCode::BackTab)
    );
    assert_eq!(
        parse_key("backtab").unwrap(),
        KeySpec::key(KeyCode::BackTab)
    );
    assert_eq!(parse_key("Shift-w").unwrap(), KeySpec::ch('W'));
    assert_eq!(parse_key("Esc").unwrap(), KeySpec::key(KeyCode::Esc));
    assert_eq!(parse_key("Space").unwrap(), KeySpec::ch(' '));
    assert_eq!(parse_key("Up").unwrap(), KeySpec::key(KeyCode::Up));
    assert_eq!(parse_key("PgDn").unwrap(), KeySpec::key(KeyCode::PageDown));
    assert_eq!(parse_key("$").unwrap(), KeySpec::ch('$'));
    assert_eq!(parse_key("-").unwrap(), KeySpec::ch('-'));
}

#[test]
fn rejects_bad_keys() {
    assert_eq!(parse_key(""), Err(KeyError::Empty));
    assert!(matches!(parse_key("Ctrl-"), Err(KeyError::MissingKey(_))));
    assert!(matches!(parse_key("ab"), Err(KeyError::Unknown(_))));
    assert!(matches!(parse_key("g g"), Err(KeyError::Unknown(_))));
    assert!(matches!(parse_key("Shift-1"), Err(KeyError::Unknown(_))));
}

#[test]
fn display_round_trips() {
    for s in [
        "w",
        "W",
        "Ctrl-t",
        "Enter",
        "Tab",
        "Shift-Tab",
        "Esc",
        "Space",
        "Up",
        "Down",
        "Left",
        "Right",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Backspace",
        "Delete",
        "?",
        "$",
        "Ctrl-Space",
    ] {
        let k = parse_key(s).unwrap();
        assert_eq!(k.to_string(), s, "display of {s}");
        assert_eq!(parse_key(&k.to_string()).unwrap(), k);
    }
}

#[test]
fn reserved_layers_by_context() {
    let find = |ctx: &str, k: KeySpec| {
        reserved(ctx)
            .into_iter()
            .find(|(x, _)| *x == k)
            .map(|(_, l)| l)
    };
    assert_eq!(find("text", KeySpec::ch('Q')), Some(Layer::Global));
    assert_eq!(find("text", KeySpec::ch('j')), Some(Layer::Base));
    assert_eq!(find("text", KeySpec::ctrl('t')), Some(Layer::Base));
    assert_eq!(find("text", KeySpec::ch('e')), Some(Layer::Verb));
    assert_eq!(find("text", KeySpec::ch('J')), Some(Layer::Type));
    assert_eq!(find("tree", KeySpec::ch('J')), Some(Layer::Type));
    assert_eq!(find("text", KeySpec::ch('w')), None);
    assert_eq!(find("diff", KeySpec::ch('s')), Some(Layer::Type));
    assert_eq!(find("table", KeySpec::ch('s')), None);
    assert_eq!(find("image", KeySpec::ch('=')), Some(Layer::Type));
    assert_eq!(find("list", KeySpec::ch('=')), None);
    assert_eq!(find("list", KeySpec::ch('R')), Some(Layer::Type));
    assert_eq!(find("tree", KeySpec::ch('d')), Some(Layer::Type));
    assert_eq!(find("text", KeySpec::ch('d')), None);
    // Unknown contexts behave like a list: git's keys fit.
    for c in "susxcd".chars() {
        assert_eq!(find("git.changes", KeySpec::ch(c)), None, "{c}");
    }
    assert_eq!(find("git.changes", KeySpec::ch('N')), Some(Layer::Base));
    // No key appears twice in one table.
    let r = reserved("text");
    for (i, (a, _)) in r.iter().enumerate() {
        assert!(r.iter().skip(i + 1).all(|(b, _)| a != b), "duplicate {a}");
    }
}

#[test]
fn free_list_keys_match_the_design() {
    let free_small = "abcdfisuwxz";
    for c in free_small.chars() {
        assert!(
            !reserved("list").iter().any(|(k, _)| *k == KeySpec::ch(c)),
            "{c} should be free"
        );
    }
    for c in "ABCDEHILMSUWXZ".chars() {
        assert!(
            !reserved("list").iter().any(|(k, _)| *k == KeySpec::ch(c)),
            "{c} should be free"
        );
    }
}

#[test]
fn forbidden_defaults_listed() {
    let f = forbidden_defaults();
    for c in "jmih[sq".chars() {
        assert!(f.contains(&KeySpec::ctrl(c)), "Ctrl-{c}");
    }
    assert_eq!(f.len(), 7);
}

fn m(acts: Vec<Act>) -> Manifest {
    acts.into_iter()
        .fold(Manifest::new("p", "1"), |m, a| m.action(a))
}

#[test]
fn clean_manifest_has_no_issues() {
    assert!(validate(&m(vec![
        Act::new("a").key("w"),
        Act::new("b").key("x"),
        Act::new("menu")
    ]))
    .is_empty());
}

#[test]
fn reserved_key_drops_the_key_and_names_the_action() {
    let issues = validate(&m(vec![
        Act::new("go").key("j"),
        Act::new("quit").key("Q"),
        Act::new("edit").key("e"),
    ]));
    assert_eq!(issues.len(), 3);
    assert_eq!(issues[0].act, "go");
    assert_eq!(issues[0].effect, Effect::KeyDropped);
    assert!(matches!(
        issues[0].kind,
        KeyIssueKind::Reserved {
            layer: Layer::Base,
            ..
        }
    ));
    assert!(matches!(
        issues[1].kind,
        KeyIssueKind::Reserved {
            layer: Layer::Global,
            ..
        }
    ));
    assert!(matches!(
        issues[2].kind,
        KeyIssueKind::Reserved {
            layer: Layer::Verb,
            ..
        }
    ));
    assert!(issues[0].to_string().contains("go"));
}

#[test]
fn context_decides_what_is_reserved() {
    // `s` is the unified/split switch in a diff but free in a table.
    assert!(validate(&m(vec![Act::new("a").key("s").screen("table")])).is_empty());
    assert_eq!(
        validate(&m(vec![Act::new("a").key("s").screen("diff")])).len(),
        1
    );
    // Without a screen the union of the display types applies, so `s` collides too.
    assert_eq!(validate(&m(vec![Act::new("a").key("s")])).len(), 1);
}

#[test]
fn duplicate_key_in_the_same_screen_loses_the_later_action() {
    let issues = validate(&m(vec![Act::new("a").key("w"), Act::new("b").key("w")]));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].act, "b");
    assert!(matches!(&issues[0].kind, KeyIssueKind::Duplicate { other, .. } if other == "a"));
    // The same key on different screens is fine.
    let ok = validate(&m(vec![
        Act::new("a").key("w").screen("p.one"),
        Act::new("b").key("w").screen("p.two"),
    ]));
    assert!(ok.is_empty());
}

#[test]
fn forbidden_and_unparseable_keys() {
    let issues = validate(&m(vec![
        Act::new("a").key("Ctrl-j"),
        Act::new("b").key("Ctrl-s"),
        Act::new("c").key("zz"),
    ]));
    assert!(matches!(issues[0].kind, KeyIssueKind::Forbidden));
    assert!(matches!(issues[1].kind, KeyIssueKind::Forbidden));
    assert!(matches!(issues[2].kind, KeyIssueKind::Unparseable(_)));
    assert!(issues.iter().all(|i| i.effect == Effect::KeyDropped));
}

#[test]
fn destructive_action_is_rejected_until_confirmation_exists() {
    let issues = validate(&m(vec![Act::new("rm").key("x").destructive()]));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, KeyIssueKind::DestructiveWithoutConfirm);
    assert_eq!(issues[0].effect, Effect::Rejected);
}

#[test]
fn duplicate_action_id_is_rejected() {
    let issues = validate(&m(vec![Act::new("a").key("w"), Act::new("a").key("x")]));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, KeyIssueKind::DuplicateActId);
    assert_eq!(issues[0].effect, Effect::Rejected);
}

#[test]
fn plugins_cannot_bind_single_keys_in_the_tree() {
    let issues = validate(&m(vec![
        Act::new("a").key("w").screen("tree"),
        Act::new("b").screen("tree"),
    ]));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].kind, KeyIssueKind::TreeKey);
}

#[test]
fn a_dropped_key_does_not_block_a_later_action() {
    // `b` collides with a reserved key, so `c` may still use `w` after `a` took it... and `b`'s
    // own key is not remembered as taken.
    let issues = validate(&m(vec![Act::new("b").key("j"), Act::new("c").key("w")]));
    assert_eq!(issues.len(), 1);
}
