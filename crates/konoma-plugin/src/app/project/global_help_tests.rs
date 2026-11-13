//! The standalone project-screen help lists the Global rows (`?` and `Q`, the only layer-0 keys
//! the standalone keeps), with the effective keys, and the listed keys do what the rows say.
//! As in the host, the version-control screens (git, jj) list them; another provider's project
//! help stays as it was.
use super::*;

/// The drawn text of every line, spaces removed so wide characters compare exactly.
fn lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            let mut out = String::new();
            let mut skip = 0;
            for x in 0..buf.area.width {
                // The cell after a wide glyph holds a stale leftover, not drawn text.
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let symbol = buf[(x, y)].symbol();
                skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
                out.push_str(symbol);
            }
            out.replace(' ', "")
        })
        .collect()
}

/// The project fixture under the name of a version-control provider (`git` or `jj`).
struct Vcs(&'static str);
impl Plugin for Vcs {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.0, "0").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("changes", "Changes"))
                .screen(ProjectScreen::new("log", "Log")),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProjectSession))
    }
}

const VCS: [&str; 2] = ["git", "jj"];

fn vcs(
    name: &'static str,
    config: &str,
) -> (
    konoma_test_support::TmpDir,
    PreparedProjects,
    Terminal<TestBackend>,
) {
    setup_plugin(Arc::new(Vcs(name)), false, true, config)
}

fn big() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(110, 70)).unwrap()
}

fn config(lang: &str, keys: &str) -> String {
    format!("[ui]\nlang='{lang}'\ndiff_view='source'\nwrap=false\n{keys}")
}

/// Rows of the Global section as the standalone computes them from its effective keys.
fn global_rows(p: &PreparedProjects) -> Vec<(String, String)> {
    let rows: Vec<_> = p
        .keys
        .canvas_global_sections()
        .into_iter()
        .flat_map(|s| s.rows)
        .collect();
    assert!(
        rows.len() >= 2,
        "the Global section names both `?` and `Q`: {rows:?}"
    );
    rows
}

fn assert_help_lists(p: &mut PreparedProjects, t: &mut Terminal<TestBackend>, title: &str) {
    let rows = global_rows(p);
    drive(p, t, vec![Key::char('?')]);
    let text = lines(t);
    assert!(
        text.iter().any(|l| l.contains(title)),
        "help has the Global heading {title:?}: {text:#?}"
    );
    for (keys, desc) in rows {
        let want = format!("{keys}{desc}").replace(' ', "");
        assert!(
            text.iter().any(|l| l.contains(&want)),
            "help lists the row {keys:?} {desc:?}: {text:#?}"
        );
    }
}

#[test]
fn project_help_lists_global_rows_on_root_and_pushed_screens() {
    for name in VCS {
        for (lang, title, quit) in [
            ("en", "Global", "quit (back to tree in preview)"),
            ("ja", "共通(Global)", "終了 (プレビュー中はツリーへ戻る)"),
        ] {
            for pushed in [false, true] {
                let (_dir, mut p, _) = vcs(name, &config(lang, ""));
                let mut t = big();
                if pushed {
                    opening(&mut p, &mut t, "log");
                }
                let rows = global_rows(&p);
                assert!(
                    rows.iter().any(|(k, d)| k == "?" && !d.is_empty()),
                    "{name} {lang}: the `?` row: {rows:?}"
                );
                assert!(
                    rows.iter().any(|(k, d)| k == "Q" && d == quit),
                    "{name} {lang}: the `Q` row has the core text: {rows:?}"
                );
                assert_help_lists(&mut p, &mut t, title);
            }
        }
    }
}

#[test]
fn project_help_of_another_provider_has_no_global_rows() {
    let (_dir, mut p, _) = setup_with(false, true, &config("en", ""));
    let mut t = big();
    drive(&mut p, &mut t, vec![Key::char('?')]);
    assert!(
        lines(&t).iter().all(|l| !l.contains("Global")),
        "a generic project's help is unchanged"
    );
}

#[test]
fn project_help_global_rows_follow_a_rebound_quit() {
    for name in VCS {
        for pushed in [false, true] {
            let (_dir, mut p, _) = vcs(name, &config("en", "[keys.global]\nQ='noop'\nX='quit'\n"));
            let mut t = big();
            if pushed {
                opening(&mut p, &mut t, "log");
            }
            let rows = global_rows(&p);
            assert!(rows.iter().any(|(k, _)| k == "X"), "moved to X: {rows:?}");
            assert!(rows.iter().all(|(k, _)| k != "Q"), "Q is gone: {rows:?}");
            assert_help_lists(&mut p, &mut t, "Global");
        }
    }
}

#[test]
fn project_help_global_keys_act_as_the_rows_say() {
    for name in VCS {
        for pushed in [false, true] {
            // `?` opens the help and, pressed again, closes it.
            let (_dir, mut p, _) = vcs(name, &config("en", ""));
            let mut t = big();
            if pushed {
                opening(&mut p, &mut t, "log");
            }
            drive(&mut p, &mut t, vec![Key::char('?')]);
            assert!(lines(&t).iter().any(|l| l.contains("Global")));
            // Each drive starts a fresh loop, so both presses go in one run.
            drive(&mut p, &mut t, vec![Key::char('?'), Key::char('?')]);
            assert!(
                lines(&t).iter().all(|l| !l.contains("Global")),
                "`?` closed the help"
            );
            // `Q` ends the loop at once, with or without the help open.
            for keys in [vec![Key::char('Q')], vec![Key::char('?'), Key::char('Q')]] {
                let (_dir, mut p, _) = vcs(name, &config("en", ""));
                let mut t = big();
                if pushed {
                    opening(&mut p, &mut t, "log");
                }
                let expected = keys.len();
                let mut all = keys;
                all.push(Key::char('j'));
                assert_eq!(
                    drive(&mut p, &mut t, all).read,
                    expected,
                    "Q stops reading keys ({name}, pushed: {pushed})"
                );
            }
        }
    }
}

#[test]
fn project_rebound_quit_key_exits_the_loop() {
    let keys = config("en", "[keys.global]\nQ='noop'\nX='quit'\n");
    for name in VCS {
        let (_dir, mut p, _) = vcs(name, &keys);
        let mut t = big();
        opening(&mut p, &mut t, "log");
        let events = drive(&mut p, &mut t, vec![Key::char('X'), Key::char('j')]);
        assert_eq!(events.read, 1, "the row's key quits");
        let (_dir, mut p, _) = vcs(name, &keys);
        let mut t = big();
        opening(&mut p, &mut t, "log");
        let events = drive(&mut p, &mut t, vec![Key::char('Q'), Key::char('j')]);
        assert_eq!(events.read, 2, "the old key no longer quits");
    }
}

#[test]
fn project_quit_key_is_text_while_a_filter_is_typed() {
    let (_dir, mut p, _) = vcs("git", &config("en", ""));
    let mut t = big();
    opening(&mut p, &mut t, "log");
    let events = drive(
        &mut p,
        &mut t,
        vec![Key::char('/'), Key::char('Q'), Key::char('j')],
    );
    assert_eq!(events.read, 3, "Q went into the filter instead of quitting");
}
