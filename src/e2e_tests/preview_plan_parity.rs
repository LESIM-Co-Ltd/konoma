//! The description of the preview screens that lives in `konoma_host_core::preview_plan` (which
//! operations the footer plans, in what order, which the help declares, and which have a name)
//! against what konoma itself says, over every kind of preview, every focus of a decorated
//! document, both languages, both key schemes and git on and off. The `Space` menu's letters run
//! along this list, so the standalone app can give the same letters only if the list is the same.
//!
//! - The footer plan: `footer::priority` (the host's labels), against the description's
//!   `priority` with this host's labels and wording. The footer's own tokens are pinned by
//!   `help_footer_snapshot`.
//! - The help's order: the rows `help::view_rows` builds (the help itself is still drawn from them),
//!   against the description's `help_order`.
//! - The names: the operations the ungated `Space` menu holds, against `is_named`, over every
//!   operation of the description; and every non-plugin item of the menu must be one of them.

use super::canvas::{open_with, Draw, Opened, Painter};
use super::*;
use crate::i18n::Msg;
use crate::keymap::{Action, Context};
use crate::ops::{core_help, decl, footer, help, menu, preview_plan as pp};
use konoma_host_core::preview_plan as core;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Text,
    TextRaw,
    Document,
    Table,
    Image,
    Word,
    WordLoading,
}

const MD: &str = "# Title\n\nhello [go](./notes.txt) and [here](#title) and [web](https://example.com)\n\n\
- [ ] task one\n\n<details><summary>More</summary>\n\ninner\n\n</details>\n\n```rust\nfn x() {}\n```\n\n\
```mermaid\ngraph TD; A-->B\n```\n";

fn cfg_of(lang: &str, keys: &str, git: bool) -> Config {
    let mut cfg = Config::default();
    // Opening an Office document in its app is off: `e` has no label there.
    cfg.external.office_apps = !(lang == "ja" && keys == "less");
    cfg.ui.lang = lang.into();
    cfg.ui.keys = keys.into();
    cfg.external.git = git;
    cfg
}

/// A session on a file of this kind (`None` when the sample document is not in the checkout).
fn open_kind(kind: Kind, cfg: Config) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox("pp_parity");
    seed_files(&dir);
    std::fs::write(dir.join("readme.md"), MD).unwrap();
    crate::test_support::write_solid_png(&dir.join("pic.png"), 8, 8, [10, 120, 200]);
    let docx =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/office/word.docx");
    assert!(docx.exists(), "the sample document is part of the checkout");
    std::fs::copy(&docx, dir.join("word.docx")).unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 100, 60);
    if matches!(kind, Kind::Image) {
        s = s.with_picker();
    }
    if matches!(kind, Kind::WordLoading) {
        s = s.with_media();
    }
    let file = match kind {
        Kind::Text => "notes.txt",
        Kind::TextRaw | Kind::Document => "readme.md",
        Kind::Table => "data.csv",
        Kind::Image => "pic.png",
        Kind::Word | Kind::WordLoading => "word.docx",
    };
    s.select(file);
    s.enter();
    if matches!(kind, Kind::TextRaw) {
        // `R` shows the Markdown source: windowed text, no links.
        s.key('R');
    }
    (s, dir)
}

/// Operations the footer plans (so they have a name) that the screen has no key for.
const NOT_KEYED_HERE: &[&str] = &["ToggleFollowDiffScope"];

const OUTSIDE_THE_DESCRIPTION: &[&str] = &[
    "Canvas: CyclePathStyle",
    "Document: ImageZoomIn",
    "Document: ImageZoomOut",
    "Document: ImageZoomReset",
    "Document: ToggleFocusedItem",
    "Document: ToggleWrap",
    "Text: CyclePathStyle",
    "Text: CycleRepresentation",
    "Text: GitLaunchTool",
    "Text: ImageZoomIn",
    "Text: ImageZoomOut",
    "Text: ImageZoomReset",
    "Text: LinkFocusNext",
    "Text: LinkFocusPrev",
    "Text: OpenGitView",
    "Text: PageNext",
    "Text: PagePrev",
    "Text: PreviewCopySelectionRef",
    "Text: PreviewEnterVisual",
    "Text: PreviewEnterVisualLine",
    "Text: Refresh",
    "Text: SearchNext",
    "Text: SearchPrev",
    "Text: SearchStart",
    "Text: ToggleOutline",
    "Text: ToggleWrap",
];

#[derive(Default)]
struct Counts {
    not_keyed: usize,
    outside: BTreeSet<String>,
    sessions: usize,
    plan_entries: usize,
    help_entries: usize,
    named_ops: usize,
    menu_items: usize,
    focuses: BTreeSet<String>,
    states: BTreeSet<String>,
}

fn universe(ctx: Context, st: &core::PreviewState) -> Vec<(core::PreviewOp, Action)> {
    let cctx = pp::core_of(ctx).unwrap();
    let mut ops: Vec<core::PreviewOp> = core::help_order(cctx, st);
    ops.extend(core::help_everywhere());
    let facts = konoma_host_core::help::HelpFacts::default();
    ops.extend(
        core::priority(cctx, st, &facts, |_| Some(()), |_| ())
            .into_iter()
            .map(|(o, _)| o),
    );
    let mut seen = BTreeSet::new();
    ops.into_iter()
        .filter_map(|o| pp::action_of(o).map(|a| (o, a)))
        .filter(|(_, a)| seen.insert(format!("{a:?}")))
        .collect()
}

#[track_caller]
fn compare(s: &Sim, what: &str, c: &mut Counts) {
    let app = &s.app;
    let ctx = app.base_context();
    let cctx = pp::core_of(ctx).expect("a preview screen");
    let st = pp::state(app, ctx);
    c.sessions += 1;
    c.states.insert(format!("{ctx:?} {st:?}"));
    if let Some(f) = st.focus {
        c.focuses.insert(format!("{f:?}"));
    }
    let tag = format!("{what} {ctx:?} {st:?}");

    // 1. The footer plan.
    let facts = core_help::facts_for(app, ctx);
    let want: Vec<(Action, Option<Msg>)> = footer::priority(app, ctx)
        .into_iter()
        .filter(|(a, _)| !matches!(a, Action::Plugin(_)))
        .collect();
    let got: Vec<(Action, Option<Msg>)> = core::priority(
        cctx,
        &st,
        &facts,
        |o| pp::action_of(o).and_then(|a| decl::text(app, ctx, a).label),
        Msg::from_core,
    )
    .into_iter()
    .filter_map(|(o, l)| pp::action_of(o).map(|a| (a, l)))
    .collect();
    assert_eq!(got, want, "footer plan: {tag}");
    c.plan_entries += want.len();

    // 2. The help's declared order.
    let (own, everywhere) = help::declared_groups_by_rows(app, ctx);
    let (own2, everywhere2) = pp::declared(app, ctx).unwrap();
    assert_eq!(own2, own, "help own rows: {tag}");
    assert_eq!(everywhere2, everywhere, "help global rows: {tag}");
    c.help_entries += own.len() + everywhere.len();

    // 3. Which operations are named, and that nothing else is a candidate.
    let uni = universe(ctx, &st);
    let in_menu: BTreeSet<String> = menu::declared_actions_menu(app, ctx)
        .into_iter()
        .filter(|i| !matches!(i.action, Action::Plugin(_)))
        .map(|i| format!("{:?}", i.action))
        .collect();
    c.menu_items += in_menu.len();
    for (o, a) in &uni {
        let listed = in_menu.contains(&format!("{a:?}"));
        let named = core::is_named(*o, &st);
        if NOT_KEYED_HERE.contains(&format!("{a:?}").as_str()) {
            // Named, but no key of this screen reaches it, so the menu has no candidate for it
            // (the menu's list of candidates is the keymap's, which is not in the description).
            assert!(!listed, "{a:?} is a candidate after all: {tag}");
            c.not_keyed += 1;
        } else {
            assert_eq!(listed, named, "named: {o:?} / {a:?}: {tag}");
        }
        c.named_ops += 1;
    }
    let known: BTreeSet<String> = uni.iter().map(|(_, a)| format!("{a:?}")).collect();
    for m in &in_menu {
        if !known.contains(m) {
            c.outside.insert(format!("{ctx:?}: {m}"));
        }
    }
}

#[test]
fn the_description_is_what_konoma_says() {
    let mut c = Counts::default();
    for lang in ["en", "ja"] {
        for keys in ["vim", "less"] {
            for git in [true, false] {
                for kind in [
                    Kind::Text,
                    Kind::TextRaw,
                    Kind::Document,
                    Kind::Table,
                    Kind::Image,
                    Kind::Word,
                    Kind::WordLoading,
                ] {
                    let (mut s, _dir) = open_kind(kind, cfg_of(lang, keys, git));
                    let what = format!("{kind:?} {lang} {keys} git={git}");
                    compare(&s, &what, &mut c);
                    if matches!(kind, Kind::Document | Kind::Word) {
                        // Walk the focus through every item, and zoom a diagram.
                        for i in 0..12 {
                            s.tab();
                            compare(&s, &format!("{what} tab {i}"), &mut c);
                            if s.app.md_focused_kind() == Some(crate::app::MdFocus::MermaidFence) {
                                for _ in 0..3 {
                                    s.key('+');
                                    compare(&s, &format!("{what} zoom"), &mut c);
                                }
                            }
                        }
                    }
                }
            }
        }
        let Opened { s, _dir, .. } = open_with(
            "pp_canvas",
            Painter::new("pp_canvas", Draw::Pattern { cols: 20, rows: 6 }).with_acts(),
            90,
            26,
            lang,
        );
        compare(&s, &format!("canvas {lang}"), &mut c);
    }
    // The comparison is real: every kind of screen, every focus, and a body of entries.
    for f in [
        "Task",
        "Details",
        "CodeBlock",
        "LocalLink",
        "AnchorLink",
        "ExternalLink",
    ] {
        assert!(
            c.focuses.contains(f),
            "focus {f} never reached: {:?}",
            c.focuses
        );
    }
    // The menu's candidates come from the keymap, which binds more than the footer and the help
    // list (a loading document declares almost nothing but its keys still exist; a canvas has
    // `CyclePathStyle` from the shared layers). They keep their place after the described ones.
    // Pinned: a new one is a decision (it changes the letters the menu hands out).
    let outside: Vec<&str> = c.outside.iter().map(String::as_str).collect();
    assert_eq!(outside, OUTSIDE_THE_DESCRIPTION);
    assert!(c.not_keyed >= 1, "the exception list is never used");
    assert!(c.sessions >= 150, "{} sessions", c.sessions);
    assert!(c.plan_entries >= 3000, "{} plan entries", c.plan_entries);
    assert!(c.help_entries >= 5000, "{} help entries", c.help_entries);
    assert!(c.named_ops >= 3000, "{} named checks", c.named_ops);
    assert!(c.menu_items >= 2000, "{} menu items", c.menu_items);
    for ctx in ["Text", "Document", "Image", "Table", "Canvas"] {
        assert!(
            c.states.iter().any(|s| s.starts_with(ctx)),
            "{ctx} never seen"
        );
    }
    assert!(
        c.states.iter().any(|s| s.contains("text_missing: true")),
        "no loading document"
    );
    assert!(
        c.states.iter().any(|s| s.contains("office_document: true")),
        "no Word text"
    );
    assert!(
        c.states.iter().any(|s| s.contains("windowed: true")),
        "no windowed text"
    );
}
