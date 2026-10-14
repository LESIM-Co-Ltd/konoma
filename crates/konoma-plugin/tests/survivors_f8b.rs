//! Tests added after the second stage-2 mutation audit: which bindings count as "files assigned",
//! what `check_manifest` and `check_hints` (the tools a plugin author tests with) report, and the
//! kind and fit of each display type.

use std::path::Path;

use konoma_plugin::manifest::Target;
use konoma_plugin::prelude::*;
use konoma_plugin::testing::{check_hints, check_manifest, open_file, ManifestIssue};
use konoma_plugin::{Priority, Session};

struct Stub(Manifest);

impl Plugin for Stub {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Quiet))
    }
}

/// A session whose actions all work.
struct Quiet;

impl Session for Quiet {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::text(["x"]))
    }
}

fn bind(targets: Vec<Target>) -> Bind {
    Bind {
        targets,
        priority: Priority::Default,
    }
}

// ---- which bindings assign files ---------------------------------------------------------------

/// A glob, a file name and a MIME type each give the plugin files; a content type handed over by
/// another plugin and an embedded kind do not. The answer holds whichever binding or target of
/// several is the one, and a declared `shows` clears it.
#[test]
fn only_a_glob_a_name_or_a_mime_assigns_files() {
    let bare = || Manifest::new("p", "1");
    assert!(!bare().assigns_files_without_shows(), "no binding at all");
    assert!(bare()
        .bind(Bind::files(["*.x"]))
        .assigns_files_without_shows());
    assert!(bare()
        .bind(Bind::file_names(["Makefile"]))
        .assigns_files_without_shows());
    assert!(bare()
        .bind(Bind::mime("image/*"))
        .assigns_files_without_shows());
    assert!(!bare()
        .bind(Bind::content("markdown"))
        .assigns_files_without_shows());
    assert!(!bare()
        .bind(Bind::embed("math"))
        .assigns_files_without_shows());
    assert!(!bare()
        .bind(Bind::content("markdown"))
        .bind(Bind::embed("math"))
        .assigns_files_without_shows());
    // The first binding is not the one that decides, and neither is the first target.
    assert!(bare()
        .bind(Bind::content("markdown"))
        .bind(Bind::embed("math"))
        .bind(Bind::mime("text/*"))
        .assigns_files_without_shows());
    assert!(bare()
        .bind(Bind::content("markdown"))
        .bind(Bind::files(["*.x"]))
        .assigns_files_without_shows());
    for file_target in [
        Target::Glob("*.x".into()),
        Target::FileName("Makefile".into()),
        Target::Mime("text/*".into()),
    ] {
        let mixed = bind(vec![
            Target::Content("markdown".into()),
            Target::Embed("math".into()),
            file_target.clone(),
        ]);
        let m = Manifest {
            binds: vec![mixed],
            ..bare()
        };
        assert!(m.assigns_files_without_shows(), "{file_target:?} last");
        let first = bind(vec![
            file_target.clone(),
            Target::Content("markdown".into()),
        ]);
        let m = Manifest {
            binds: vec![first],
            ..bare()
        };
        assert!(m.assigns_files_without_shows(), "{file_target:?} first");
    }
    // Declaring what it shows clears it, whatever the binding.
    for b in [
        Bind::files(["*.x"]),
        Bind::file_names(["Makefile"]),
        Bind::mime("image/*"),
    ] {
        assert!(!bare()
            .bind(b)
            .shows(ViewKind::Text)
            .assigns_files_without_shows());
    }
}

// ---- check_manifest ----------------------------------------------------------------------------

fn act(id: &str, key: &str) -> Act {
    Act::new(id)
        .key(key)
        .label(id.to_string())
        .help(format!("{id} help"))
}

/// `check_manifest` reports the key problems and the files assigned without `shows`, each, and
/// nothing for a manifest that is fine.
#[test]
fn check_manifest_reports_key_problems_and_missing_shows_and_nothing_else() {
    let fine = Stub(
        Manifest::new("p", "1")
            .bind(Bind::files(["*.x"]))
            .shows(ViewKind::Text)
            .action(act("a", "w")),
    );
    assert_eq!(check_manifest(&fine), vec![]);
    // Two actions on one key, and another on a key the host keeps for itself.
    let clashing = Stub(
        Manifest::new("p", "1")
            .shows(ViewKind::Text)
            .action(act("a", "w"))
            .action(act("b", "w"))
            .action(act("c", "j")),
    );
    let issues = check_manifest(&clashing);
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues.iter().all(|i| matches!(i, ManifestIssue::Key(_))));
    // And both kinds of problem at once.
    let both = Stub(
        Manifest::new("p", "1")
            .bind(Bind::files(["*.x"]))
            .action(act("a", "j")),
    );
    let issues = check_manifest(&both);
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues.iter().any(|i| matches!(i, ManifestIssue::Key(_))));
    assert!(issues.contains(&ManifestIssue::ShowsNotDeclared));
    // Only the missing `shows`.
    let only = Stub(Manifest::new("p", "1").bind(Bind::files(["*.x"])));
    assert_eq!(check_manifest(&only), vec![ManifestIssue::ShowsNotDeclared]);
}

// ---- check_hints -------------------------------------------------------------------------------

fn report_of(m: Manifest) -> konoma_plugin::testing::HintReport {
    let p = Stub(m);
    let s = open_file(&p, Path::new("/nonexistent")).unwrap();
    check_hints(&p, s.as_ref())
}

/// An action that is shown needs both its label and its help text: either one missing is reported,
/// and an action with both is not.
#[test]
fn an_action_shown_without_a_label_or_without_help_is_reported() {
    let base = || Manifest::new("p", "1").shows(ViewKind::Text);
    let no_label = Act::new("nl").label("").help("help");
    let no_help = Act::new("nh").label("Label").help("");
    let neither = Act::new("nn").label("");
    let both = act("ok", "w");
    let r = report_of(
        base()
            .action(no_label)
            .action(no_help)
            .action(neither)
            .action(both),
    );
    assert_eq!(r.unlabelled, ["nl", "nh", "nn"]);
    assert!(r.shown_while_disabled.is_empty());
    assert!(!r.is_clean());
    assert!(report_of(base().action(act("ok", "w"))).is_clean());
}

/// The report reads the manifest the way the host registers it: an action declared twice is one
/// action (the first), so the second declaration's missing label is not a problem of the shown one.
#[test]
fn check_hints_reads_the_manifest_as_it_is_registered() {
    let r = report_of(
        Manifest::new("p", "1")
            .shows(ViewKind::Text)
            .action(act("a", "w"))
            .action(Act::new("a").key("x")),
    );
    assert!(r.unlabelled.is_empty(), "{:?}", r.unlabelled);
    assert_eq!(r.key_issues.len(), 1, "the duplicate is still reported");
    // A destructive action is not registered, so nothing is shown for it.
    let r = report_of(
        Manifest::new("p", "1")
            .shows(ViewKind::Text)
            .action(Act::new("d").key("x").destructive()),
    );
    assert!(r.unlabelled.is_empty(), "{:?}", r.unlabelled);
}

// ---- the display type of each view, and which declarations it fits ------------------------------

mod views {
    use std::sync::Arc;

    use konoma_plugin::view::{
        DiffView, DocOptions, DocumentView, ImageData, ImageView, ListView, MessageView, Reopen,
        TableView,
    };
    use konoma_plugin::{Origin, Resources, View, ViewKind};

    struct NoCells;

    impl konoma_plugin::view::Grid for NoCells {
        fn nrows(&self) -> usize {
            0
        }
        fn ncols(&self) -> usize {
            0
        }
        fn header(&self, _: usize) -> Option<String> {
            None
        }
        fn cell(&self, _: usize, _: usize) -> String {
            String::new()
        }
    }

    const ALL: [ViewKind; 6] = [
        ViewKind::Text,
        ViewKind::Document,
        ViewKind::Table,
        ViewKind::Image,
        ViewKind::Diff,
        ViewKind::List,
    ];

    fn every_view() -> Vec<(&'static str, View, Option<ViewKind>)> {
        vec![
            ("text", View::text(["x"]), Some(ViewKind::Text)),
            (
                "document",
                View::Document(DocumentView {
                    markdown: String::new(),
                    origin: Origin::File("x".into()),
                    resources: Resources::new(),
                    options: DocOptions::default(),
                    title: None,
                    truncated: false,
                }),
                Some(ViewKind::Document),
            ),
            (
                "reopen",
                View::Reopen(Reopen {
                    content_type: "markdown".into(),
                    text: String::new(),
                    resources: Resources::new(),
                    origin: Origin::File("x".into()),
                    truncated: false,
                }),
                Some(ViewKind::Document),
            ),
            (
                "table",
                View::Table(TableView {
                    grid: Arc::new(NoCells),
                    title: None,
                    rainbow: false,
                    addressed: false,
                }),
                Some(ViewKind::Table),
            ),
            (
                "image",
                View::Image(ImageView::new(ImageData::Encoded(vec![0u8].into()))),
                Some(ViewKind::Image),
            ),
            (
                "list",
                View::List(ListView { rows: Vec::new() }),
                Some(ViewKind::List),
            ),
            (
                "diff",
                View::Diff(DiffView { hunks: Vec::new() }),
                Some(ViewKind::Diff),
            ),
            ("message", View::Message(MessageView::default()), None),
        ]
    }

    /// Each view is the display type it says, and a message is none.
    #[test]
    fn every_view_has_its_own_kind() {
        for (name, view, kind) in every_view() {
            assert_eq!(view.kind(), kind, "{name}");
        }
    }

    /// A plugin may return a view of a type it declared and no other, with the two conventions: a
    /// message fits whatever is declared (also nothing), and a text view also fits a document or
    /// a picture plugin (the source of a document, the text form of a diagram).
    #[test]
    fn a_view_fits_only_what_the_plugin_declared() {
        for (name, view, kind) in every_view() {
            for declared in ALL {
                let expected = match (kind, declared) {
                    (None, _) => true,
                    (Some(ViewKind::Text), d) => {
                        matches!(d, ViewKind::Text | ViewKind::Document | ViewKind::Image)
                    }
                    (Some(k), d) => k == d,
                };
                assert_eq!(
                    view.fits(&[declared]),
                    expected,
                    "{name} against {declared:?}"
                );
                // And among others: the declaration is a list, any entry may be the one.
                let others: Vec<ViewKind> = ALL.into_iter().filter(|k| *k != declared).collect();
                let mut with = others.clone();
                with.push(declared);
                let without = view.fits(&others);
                let with_it = view.fits(&with);
                assert_eq!(with_it, expected || without, "{name} among {declared:?}");
            }
            assert_eq!(view.fits(&[]), kind.is_none(), "{name} against nothing");
            assert!(view.fits(&ALL), "{name} fits when everything is declared");
        }
    }
}
