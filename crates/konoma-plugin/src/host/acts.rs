//! The declared actions of the open plugin, as a host offers them.
//!
//! The order a host asks in (konoma's `handle_key`): the plugin's own typing first
//! ([`super::typing`]), then a pending menu, then the fixed keys (on a canvas a fixed key runs the
//! plugin's action when it declares one on it, else nothing), then [`act_for_key`] -- the plugin's
//! layer, **before** the shared key table, which is safe because a plugin cannot declare a key of
//! layers 0 to 3 -- and last the shared table. A key found here is run only if [`gate`] says so.

use konoma_host_core::keymap::KeyPress;
use konoma_host_core::menu::{add_candidate, plain_key};

use crate::text::Tr;

/// The names of the key contexts that show a plugin's view: the plugin's layer sits on top of the
/// display type's, and nowhere else.
pub const VIEW_CONTEXTS: [&str; 5] = ["text", "document", "table", "image", "canvas"];

/// One declared action of one plugin as a host binds it. `H` is the host's handle for the action
/// (konoma: a `Copy` pair of interned ids; the standalone app: whatever names it).
#[derive(Debug, Clone)]
pub struct ActBinding<H> {
    pub act: H,
    /// The key it runs on in the plugin's layer; `None` = reachable from the `Space` menu only.
    pub key: Option<KeyPress>,
    /// The key context it lives in (`text`, `table`, ...); `None` = the plugin's main view.
    pub context: Option<String>,
    pub label: Tr,
    pub help: Tr,
    /// The footer priority (smaller first); `None` keeps it out of the footer.
    pub footer: Option<u8>,
    pub writes: bool,
}

/// The declared actions of one plugin that belong to the key context named `ctx`, in declaration
/// order. Empty for a context that does not show a plugin's view ([`VIEW_CONTEXTS`]); an action
/// with no context of its own belongs to every view context.
pub fn acts_in<'a, H>(all: &'a [ActBinding<H>], ctx: &str) -> Vec<&'a ActBinding<H>> {
    if !VIEW_CONTEXTS.contains(&ctx) {
        return Vec::new();
    }
    all.iter()
        .filter(|b| b.context.as_deref().is_none_or(|c| c == ctx))
        .collect()
}

/// The action `kp` runs: the first of `acts` (already narrowed to the context) bound to it.
pub fn act_for_key<'a, H>(acts: &[&'a ActBinding<H>], kp: KeyPress) -> Option<&'a ActBinding<H>> {
    acts.iter().copied().find(|b| b.key == Some(kp))
}

/// The actions that asked for a footer entry, by the priority they declared (a stable sort:
/// declaration order within one priority). The host places them right after the screen's
/// movement keys.
pub fn footer_order<'a, H>(acts: &[&'a ActBinding<H>]) -> Vec<&'a ActBinding<H>> {
    let mut v: Vec<_> = acts
        .iter()
        .copied()
        .filter_map(|b| b.footer.map(|p| (p, b)))
        .collect();
    v.sort_by_key(|(p, _)| *p);
    v.into_iter().map(|(_, b)| b).collect()
}

/// The footer token of an action: `<keys>:<label>`. `None` when the action has no key or does not
/// act now (`enabled`), so a hint is shown exactly while its key acts. `show_key` writes the key
/// the way the host writes keys.
pub fn footer_token<H>(
    b: &ActBinding<H>,
    enabled: bool,
    lang: &str,
    show_key: impl FnOnce(KeyPress) -> String,
) -> Option<(String, KeyPress)> {
    if !enabled {
        return None;
    }
    let key = b.key?;
    Some((format!("{}:{}", show_key(key), b.label.get(lang)), key))
}

/// The rows of the plugin's section of the `?` help: the keyed actions that act now (`enabled`),
/// each as its key and its description (the help text, else the label). The plugin's display name
/// is the section's title and comes from the host's manifest cache.
pub fn help_rows<'a, H>(
    acts: &[&'a ActBinding<H>],
    lang: &str,
    enabled: impl Fn(&ActBinding<H>) -> bool,
) -> Vec<(KeyPress, &'a str, &'a ActBinding<H>)> {
    acts.iter()
        .copied()
        .filter_map(|b| {
            let key = b.key?;
            if !enabled(b) {
                return None;
            }
            let desc = match b.help.get(lang) {
                "" => b.label.get(lang),
                h => h,
            };
            Some((key, desc, b))
        })
        .collect()
}

/// The `Space` menu candidates of the plugin: **every** declared action, keyed or not (the menu is
/// how an action that lost its key to a collision or a rebinding is still reachable), each with
/// the key the menu may use for it -- a plain character other than `q` and the space, else none --
/// in declaration order. They are the shape [`konoma_host_core::menu::build_items`] takes, after
/// the host adds its own.
pub fn menu_candidates<H: Copy + PartialEq>(acts: &[&ActBinding<H>]) -> Vec<(H, Option<KeyPress>)> {
    let mut out = Vec::new();
    for b in acts {
        add_candidate(&mut out, b.act, b.key.and_then(plain_key));
    }
    out
}

/// The menu label of an action: its declared label in the language of the interface.
pub fn menu_label<'a, H: PartialEq>(
    acts: &[&'a ActBinding<H>],
    act: &H,
    lang: &str,
) -> Option<&'a str> {
    acts.iter()
        .find(|b| b.act == *act)
        .map(|b| b.label.get(lang))
}

/// What the open show says about whether a declared action can be run now.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShowFacts {
    /// The show on screen is this action's plugin.
    pub same_plugin: bool,
    /// The show has finished opening (it is not loading and did not fail).
    pub ready: bool,
    /// The session is at hand (no job holds it).
    pub session_present: bool,
    /// The view on screen is a canvas: a press while a job holds the session waits in line.
    pub canvas: bool,
    /// The file is being read again: a press waits for the new session.
    pub reopening: bool,
    /// The session's last answer to `Session::enabled` for this action, if it gave one.
    pub says: Option<bool>,
}

/// Whether the session says the action works now. False before the first answer and when the
/// plugin could not open the file. **While a job holds the session** the answer is the last the
/// session gave for a canvas (its presses wait in line, so the key does act, and the hint does not
/// flicker with a redraw timer's ticks), or while the file is read again; for any other display it
/// is false, because the press would be ignored.
pub fn act_enabled(f: &ShowFacts) -> bool {
    f.same_plugin
        && f.ready
        && (f.session_present || f.canvas || f.reopening)
        && f.says.unwrap_or(false)
}

/// The verdict on running a declared action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActGate {
    On,
    /// It does not act now (not in this context, or the session says no): no hint, no key.
    Off,
    /// A writing action of a plugin set to `read_only`: refused, with a reason to show.
    ReadOnly,
}

/// Whether the action may run. `binding` is the action looked up in the context on screen.
pub fn gate<H>(binding: Option<&ActBinding<H>>, enabled: bool, read_only: bool) -> ActGate {
    let Some(b) = binding else {
        return ActGate::Off;
    };
    if !enabled {
        return ActGate::Off;
    }
    if b.writes && read_only {
        return ActGate::ReadOnly;
    }
    ActGate::On
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn b(
        act: &'static str,
        key: Option<KeyPress>,
        context: Option<&str>,
        footer: Option<u8>,
    ) -> ActBinding<&'static str> {
        ActBinding {
            act,
            key,
            context: context.map(str::to_string),
            label: Tr::new(act).with("ja", format!("{act}-ja")),
            help: Tr::new(""),
            footer,
            writes: false,
        }
    }

    fn all() -> Vec<ActBinding<&'static str>> {
        vec![
            b("width", Some(KeyPress::ch('w')), None, Some(2)),
            b("rows", Some(KeyPress::ch('r')), Some("table"), Some(1)),
            b("dump", None, None, None),
            b("again", Some(KeyPress::ch('x')), None, Some(1)),
        ]
    }

    fn names(v: &[&ActBinding<&'static str>]) -> Vec<&'static str> {
        v.iter().map(|b| b.act).collect()
    }

    #[test]
    fn a_context_that_shows_no_plugin_view_has_no_actions() {
        let all = all();
        for ctx in ["tree", "help", "image-pages", "", "Canvas"] {
            assert!(acts_in(&all, ctx).is_empty(), "{ctx}");
        }
        for ctx in VIEW_CONTEXTS {
            assert!(!acts_in(&all, ctx).is_empty(), "{ctx}");
        }
    }

    #[test]
    fn an_action_belongs_to_its_context_or_to_all_when_it_names_none() {
        let all = all();
        assert_eq!(
            names(&acts_in(&all, "table")),
            ["width", "rows", "dump", "again"]
        );
        assert_eq!(names(&acts_in(&all, "canvas")), ["width", "dump", "again"]);
        assert_eq!(names(&acts_in(&all, "text")), ["width", "dump", "again"]);
    }

    #[test]
    fn a_key_runs_the_action_bound_to_it_in_the_context_on_screen() {
        let all = all();
        let table = acts_in(&all, "table");
        assert_eq!(
            act_for_key(&table, KeyPress::ch('r')).map(|b| b.act),
            Some("rows")
        );
        let canvas = acts_in(&all, "canvas");
        assert!(act_for_key(&canvas, KeyPress::ch('r')).is_none());
        assert!(act_for_key(&table, KeyPress::ch('z')).is_none());
        // The control bit is part of the key.
        assert!(act_for_key(&table, KeyPress::ctrl_ch('w')).is_none());
        assert!(act_for_key(&table, KeyPress::key(KeyCode::Enter)).is_none());
    }

    #[test]
    fn the_first_binding_wins_when_two_share_a_key() {
        let all = vec![
            b("one", Some(KeyPress::ch('k')), None, None),
            b("two", Some(KeyPress::ch('k')), None, None),
        ];
        let v = acts_in(&all, "text");
        assert_eq!(
            act_for_key(&v, KeyPress::ch('k')).map(|b| b.act),
            Some("one")
        );
    }

    #[test]
    fn the_footer_takes_only_declared_entries_by_priority_then_declaration_order() {
        let all = all();
        let v = acts_in(&all, "table");
        assert_eq!(names(&footer_order(&v)), ["rows", "again", "width"]);
        assert_eq!(
            names(&footer_order(&acts_in(&all, "text"))),
            ["again", "width"]
        );
    }

    #[test]
    fn a_footer_token_needs_a_key_and_an_action_that_acts() {
        let all = all();
        let show = |k: KeyPress| k.label();
        let t = footer_token(&all[0], true, "en", show).unwrap();
        assert_eq!(t, ("w:width".to_string(), KeyPress::ch('w')));
        let ja = footer_token(&all[0], true, "ja", |k| k.label()).unwrap();
        assert_eq!(ja.0, "w:width-ja");
        assert_eq!(footer_token(&all[0], false, "en", |k| k.label()), None);
        assert_eq!(footer_token(&all[2], true, "en", |k| k.label()), None);
    }

    #[test]
    fn the_help_lists_keyed_actions_that_act_with_help_else_label() {
        let mut all = all();
        all[0].help = Tr::new("Change the width").with("ja", "幅を変える");
        let v = acts_in(&all, "table");
        let rows = help_rows(&v, "en", |b| b.act != "again");
        let got: Vec<_> = rows.iter().map(|(k, d, _)| (k.label(), *d)).collect();
        assert_eq!(
            got,
            [
                ("w".to_string(), "Change the width"),
                ("r".to_string(), "rows")
            ],
            "the keyless one and the disabled one are left out"
        );
        let ja = help_rows(&v, "ja", |_| true);
        assert_eq!(ja[0].1, "幅を変える");
        assert_eq!(
            ja[1].1, "rows-ja",
            "no help text: the label of the language"
        );
    }

    #[test]
    fn the_menu_has_every_action_and_only_plain_keys() {
        let all = vec![
            b("plain", Some(KeyPress::ch('w')), None, None),
            b("ctrl", Some(KeyPress::ctrl_ch('w')), None, None),
            b("q", Some(KeyPress::ch('q')), None, None),
            b("space", Some(KeyPress::ch(' ')), None, None),
            b("enter", Some(KeyPress::key(KeyCode::Enter)), None, None),
            b("none", None, None, None),
        ];
        let c = menu_candidates(&acts_in(&all, "text"));
        assert_eq!(
            c,
            vec![
                ("plain", Some(KeyPress::ch('w'))),
                ("ctrl", None),
                ("q", None),
                ("space", None),
                ("enter", None),
                ("none", None),
            ]
        );
    }

    #[test]
    fn the_menu_follows_the_context_and_names_by_the_label() {
        let all = all();
        let table = acts_in(&all, "table");
        let canvas = acts_in(&all, "canvas");
        assert_eq!(menu_candidates(&table).len(), 4);
        assert_eq!(menu_candidates(&canvas).len(), 3);
        assert_eq!(menu_label(&table, &"rows", "ja"), Some("rows-ja"));
        assert_eq!(menu_label(&canvas, &"rows", "en"), None);
    }

    #[test]
    fn an_action_acts_only_when_the_session_says_so_in_a_state_that_can_run_it() {
        let ok = ShowFacts {
            same_plugin: true,
            ready: true,
            session_present: true,
            canvas: false,
            reopening: false,
            says: Some(true),
        };
        assert!(act_enabled(&ok));
        assert!(!act_enabled(&ShowFacts::default()));
        assert!(!act_enabled(&ShowFacts {
            same_plugin: false,
            ..ok
        }));
        assert!(!act_enabled(&ShowFacts { ready: false, ..ok }));
        assert!(!act_enabled(&ShowFacts {
            says: Some(false),
            ..ok
        }));
        assert!(
            !act_enabled(&ShowFacts { says: None, ..ok }),
            "no answer yet"
        );
    }

    #[test]
    fn while_a_job_holds_the_session_only_a_canvas_or_a_reload_keeps_the_hint() {
        let out = ShowFacts {
            same_plugin: true,
            ready: true,
            session_present: false,
            canvas: false,
            reopening: false,
            says: Some(true),
        };
        assert!(!act_enabled(&out), "another display ignores the press");
        assert!(act_enabled(&ShowFacts {
            canvas: true,
            ..out
        }));
        assert!(act_enabled(&ShowFacts {
            reopening: true,
            ..out
        }));
    }

    #[test]
    fn a_missing_binding_or_a_disabled_action_is_off_and_a_write_to_a_read_only_plugin_is_refused()
    {
        let mut all = all();
        all[0].writes = true;
        assert_eq!(gate::<&str>(None, true, false), ActGate::Off);
        assert_eq!(gate(Some(&all[0]), false, false), ActGate::Off);
        assert_eq!(gate(Some(&all[0]), true, false), ActGate::On);
        assert_eq!(gate(Some(&all[0]), true, true), ActGate::ReadOnly);
        // A reading action is not touched by `read_only`.
        assert_eq!(gate(Some(&all[1]), true, true), ActGate::On);
        // Off wins over the refusal: a hint that does not act is not explained.
        assert_eq!(gate(Some(&all[0]), false, true), ActGate::Off);
    }
}
