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

// ---------------------------------------------------------------------------------------------
// The reserved tables, pinned key by key (a key dropped from one string of `reserved` must fail).
// ---------------------------------------------------------------------------------------------

/// `layer:key` for every reserved key of a context, sorted.
fn table(ctx: &str) -> Vec<String> {
    let mut v: Vec<String> = reserved(ctx)
        .into_iter()
        .map(|(k, l)| format!("{l:?}:{k}"))
        .collect();
    v.sort();
    v
}

fn expected(groups: &[(&str, &str)]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for (layer, keys) in groups {
        for k in keys.split(' ').filter(|k| !k.is_empty()) {
            v.push(format!("{layer}:{k}"));
        }
    }
    v.sort();
    v
}

/// Layers 0 to 3 and the page keys: the part every context shares.
fn shared() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Global", "? Q F P t T [ ] 1 2 3 4 5 6 7 8 9"),
        (
            "Base",
            "j k h l g G 0 $ q / n N v V y Y r m ' , Space Ctrl-d Ctrl-u Ctrl-f Ctrl-b Ctrl-n Ctrl-p Ctrl-t Up Down Left Right Esc Enter",
        ),
        ("Verb", "e o p !"),
        ("Type", "J K O"),
    ]
}

fn with(extra: &[(&'static str, &'static str)]) -> Vec<String> {
    let mut g = shared();
    g.extend_from_slice(extra);
    expected(&g)
}

#[test]
fn the_global_table_is_exactly_layers_zero_to_three_and_the_page_keys() {
    assert_eq!(table("global"), with(&[]));
}

#[test]
fn every_context_table_is_exactly_the_shared_part_plus_its_own_layer_two() {
    assert_eq!(table("tree"), with(&[("Type", "d C a A . i")]));
    for ctx in ["text", "document"] {
        assert_eq!(
            table(ctx),
            with(&[("Type", "Tab Shift-Tab R + - = f")]),
            "{ctx}"
        );
    }
    assert_eq!(table("image"), with(&[("Type", "R + - =")]));
    assert_eq!(table("table"), with(&[("Type", "R")]));
    assert_eq!(table("diff"), with(&[("Type", "s R")]));
    // "list" and every unknown name (a plugin's screen, `git.changes`).
    for ctx in ["list", "git.changes", "my.screen", ""] {
        assert_eq!(table(ctx), with(&[("Type", "R")]), "{ctx}");
    }
}

#[test]
fn no_context_table_lists_a_key_twice() {
    for ctx in [
        "global", "tree", "text", "document", "table", "image", "diff", "list",
    ] {
        let r = reserved(ctx);
        for (i, (a, _)) in r.iter().enumerate() {
            assert!(
                r.iter().skip(i + 1).all(|(b, _)| a != b),
                "{ctx}: {a} twice"
            );
        }
    }
}

#[test]
fn the_notation_aliases_parse_to_one_key_each() {
    for s in ["pagedown", "PgDn", "pagedn", "PAGEDN"] {
        assert_eq!(
            parse_key(s).unwrap(),
            KeySpec::key(KeyCode::PageDown),
            "{s}"
        );
    }
    for s in ["pageup", "PgUp"] {
        assert_eq!(parse_key(s).unwrap(), KeySpec::key(KeyCode::PageUp), "{s}");
    }
    for s in ["esc", "Escape", "ESCAPE"] {
        assert_eq!(parse_key(s).unwrap(), KeySpec::key(KeyCode::Esc), "{s}");
    }
    for s in ["enter", "Return"] {
        assert_eq!(parse_key(s).unwrap(), KeySpec::key(KeyCode::Enter), "{s}");
    }
    for s in ["delete", "Del"] {
        assert_eq!(parse_key(s).unwrap(), KeySpec::key(KeyCode::Delete), "{s}");
    }
    assert_eq!(parse_key("F1").unwrap(), KeySpec::key(KeyCode::F(1)));
    assert_eq!(parse_key("f12").unwrap(), KeySpec::key(KeyCode::F(12)));
    assert!(parse_key("F13").is_err());
    assert!(parse_key("F0").is_err());
}

#[test]
fn ctrl_keeps_its_modifier_on_shift_tab_and_shift_letters() {
    assert_eq!(
        parse_key("Ctrl-Shift-Tab").unwrap(),
        KeySpec {
            code: KeyCode::BackTab,
            ctrl: true
        }
    );
    assert_eq!(
        parse_key("c-shift-tab").unwrap(),
        KeySpec {
            code: KeyCode::BackTab,
            ctrl: true
        }
    );
    assert_eq!(
        parse_key("Ctrl-Shift-w").unwrap(),
        KeySpec {
            code: KeyCode::Char('W'),
            ctrl: true
        }
    );
    assert_eq!(
        parse_key("Shift-Tab").unwrap(),
        KeySpec::key(KeyCode::BackTab)
    );
    assert_eq!(
        parse_key("Ctrl-Tab").unwrap(),
        KeySpec {
            code: KeyCode::Tab,
            ctrl: true
        }
    );
}

#[test]
fn a_repeated_action_id_reports_only_the_repetition() {
    // The later declaration is rejected and nothing else is said about it: its own key (reserved
    // here) is not examined, and it does not take the key `w` the first one holds.
    let issues = validate(&m(vec![
        Act::new("a").key("w"),
        Act::new("a").key("j"),
        Act::new("a").key("w"),
    ]));
    assert_eq!(issues.len(), 2);
    assert!(issues
        .iter()
        .all(|i| i.kind == KeyIssueKind::DuplicateActId && i.effect == Effect::Rejected));
    // The repetition's own key (`y2` does not even parse) is not examined; `c` collides with the
    // first `a` on `x`.
    let issues = validate(&m(vec![
        Act::new("a").key("x"),
        Act::new("a").key("y2"),
        Act::new("c").key("x"),
    ]));
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].kind, KeyIssueKind::DuplicateActId);
    assert!(matches!(&issues[1].kind, KeyIssueKind::Duplicate { other, .. } if other == "a"));
}
