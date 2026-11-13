//! The real project host and the binary's uninterrupted project loop receive identical input.
use super::project_frames::compare_frames;
use super::vcs_changes::{drain, git_repo};
use super::vcs_log::{
    commit, detail_repo, empty_subject_commit, empty_subject_repo, jj, jj_repo,
    remove_commit_object, remove_head_object, repo,
};
use super::*;
use konoma_host_core::clipboard::{clear_test_clipboard, get_test_clipboard};
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::app::project::{
    observe_project_state_for_test, prepare_projects, probe_graph_plugin_for_test,
    read_snapshot_for_test, run_project_fenced_state_for_test, ProjectLoopCx, ProjectStateSnapshot,
};
use konoma_plugin::app::{key_from_crossterm, AppEvent, Next};
use konoma_plugin::host::project::ProjectContent;
use konoma_plugin::{Plugin, TermCaps};
use ratatui::{
    backend::{Backend, ClearType, WindowSize},
    buffer::{Buffer, Cell},
    layout::{Position, Rect, Size},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

include!("vcs_app_write_cases.rs");
include!("vcs_app_worktree_cases.rs");
include!("vcs_app_launch_targets.rs");
include!("vcs_app_root_jobs.rs");
include!("vcs_app_diff_cases.rs");
include!("vcs_app_media_cases.rs");
include!("vcs_app_idle_workers.rs");
include!("vcs_app_port_cases.rs");

thread_local! {
    static TALLY: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

fn report_cases() {
    TALLY.with(|t| {
        let (runs, fences) = t.get();
        eprintln!("U8b: {runs} uninterrupted sessions, {fences} frame/state/clipboard fences");
    });
}

/// Sharing the backend only lets the observer resize it while Terminal owns its mutable borrow.
struct SharedBackend(Arc<Mutex<TestBackend>>);
impl Backend for SharedBackend {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, cells: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.lock().unwrap().draw(cells)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok((0, 0).into())
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear()
    }
    fn clear_region(&mut self, region: ClearType) -> Result<(), Self::Error> {
        self.0.lock().unwrap().clear_region(region)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.0.lock().unwrap().size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.0.lock().unwrap().window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

type BeforeKey = Box<dyn Fn(&Path)>;
enum Step {
    HelpPages,
    BusyKey(KeyEvent),
    Key(KeyEvent),
    Resize(u16, u16),
    BeforeKey(BeforeKey, KeyEvent),
    Write(KeyEvent, &'static str),
    Policy(String),
}
fn key(code: KeyCode) -> Step {
    Step::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ch(c: char) -> Step {
    key(KeyCode::Char(c))
}
fn typed(text: &str) -> Vec<Step> {
    text.chars().map(ch).collect()
}

/// Each scenario process has a temporary HOME/config, including the real host's context home.
fn isolated(name: &str) -> bool {
    if std::env::var("KONOMA_VCS_APP_PARITY_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let home = konoma_test_support::unique_tmp("vcs_app_home");
    std::fs::create_dir_all(&home).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("e2e_tests::vcs_app_parity::{name}"),
            "--nocapture",
        ])
        .env("KONOMA_VCS_APP_PARITY_CHILD", name)
        .env("HOME", home.canonicalize().unwrap())
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("JJ_CONFIG", "")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+0000")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "running 1 test")
            && stdout
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed;")),
        "isolated child must run and pass exactly one test ({name}):\n{stdout}"
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}

/// Reuse the existing recipe, then place its result under the child process's temporary HOME.
fn at_home(dir: crate::test_support::TmpDir) -> std::path::PathBuf {
    let target =
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(dir.file_name().unwrap());
    std::fs::rename(&dir, &target).unwrap();
    target.canonicalize().unwrap()
}

fn config(id: &str, lang: &str, scheme: &str, path: &str, align: &str, extra: &str) -> String {
    let plugin: Arc<dyn Plugin> = match id {
        "git" => Arc::new(konoma_vcs::Git),
        "jj" => Arc::new(konoma_vcs::Jj),
        _ => unreachable!(),
    };
    let marker =
        crate::plugins::test_plugins::register_replacement(&format!("vcs_app_{id}"), plugin);
    let other = if id == "git" { "jj" } else { "git" };
    format!("[ui]\nlang='{lang}'\nkeys='{scheme}'\ntabbar='hidden'\nstatusbar='split'\ndiff_view='source'\npath_style='{path}'\ncommit_meta_align='{align}'\n[git]\ndiff='unified'\n[project]\nvcs='{id}'\n[plugins.{marker:?}]\nenabled=false\n[plugins.{other}]\nenabled=false\n{extra}")
}

pub(super) fn host_state(host: &Sim) -> ProjectStateSnapshot {
    let nav = host
        .app
        .tab
        .project_navigation
        .as_ref()
        .or(host.app.tab.project_return.as_ref());
    let content = nav
        .and_then(|n| {
            host.app
                .project_runtimes
                .get(&n.project)?
                .views
                .get(&n.frame()?.key)
        })
        .cloned();
    let menu = host.app.pending_leader.map(|menu| match menu {
        crate::keymap::LeaderId::Copy => "copy",
        crate::keymap::LeaderId::Actions => "actions",
        crate::keymap::LeaderId::Options => "options",
        _ => panic!("unexpected menu {menu:?}"),
    });
    let mut state = observe_project_state_for_test(
        nav.map(|n| &**n),
        content.as_deref(),
        host.app.project_dialog.as_ref(),
        menu,
        host.app.flash.clone(),
    );
    state.root = Some(host.app.tab.root.clone());
    state.return_to_preview = host.app.project_returns_to_preview_for_test();
    state.open_dir = Some(host.app.tab.open_dir.clone());
    state.target = host.app.project_row_target();
    state.chip = nav
        .and_then(|nav| host.app.project_runtimes.get(&nav.project))
        .and_then(|runtime| runtime.chip.clone());
    state
}

fn expected_footer(
    fixture: &serde_json::Value,
    lang: &str,
    kind: &str,
    side: &str,
    width: u16,
) -> Buffer {
    let fitted;
    let text = if let Some(text) = fixture[lang][kind][side][width.to_string()].as_str() {
        text
    } else {
        // The explicit 180-column recipes contain all tokens for these B screens.
        let recipe = fixture[lang][kind][side]["180"]
            .as_str()
            .unwrap_or_else(|| panic!("missing explicit footer: {lang}/{kind}/{side}/{width}"));
        assert!(!recipe.ends_with('…'), "the footer recipe must be complete");
        fitted = konoma_widgets::help_panel::fit_tokens(
            &recipe.split("  ").map(str::to_owned).collect::<Vec<_>>(),
            width,
        );
        &fitted
    };
    let style = |name: &str| {
        Style::default()
            .fg(fixture[name]["fg"].as_str().unwrap().parse().unwrap())
            .bg(fixture[name]["bg"].as_str().unwrap().parse().unwrap())
            .add_modifier(Modifier::from_bits_truncate(
                fixture[name]["modifiers"].as_u64().unwrap() as u16,
            ))
    };
    let mut row = Buffer::empty(Rect::new(0, 0, width, 1));
    for cell in &mut row.content {
        cell.set_symbol(fixture["blank_style"]["symbol"].as_str().unwrap())
            .set_style(style("blank_style"));
    }
    Paragraph::new(Line::raw(text).style(style("hint_style"))).render(row.area, &mut row);
    row
}

fn cells(frame: &Buffer, y: u16) -> Buffer {
    let mut row = Buffer::empty(Rect::new(0, 0, frame.area.width, 1));
    for x in 0..frame.area.width {
        row[(x, 0)] = frame[(x, y)].clone();
    }
    row
}

/// The footer's list-navigation chip follows the configured `navigate:down` aliases of the
/// text context: `D` or `J` when bound, else the default `j/k`.
fn nav_chip(config: &str) -> &'static str {
    if config.contains("D='navigate:down'") {
        "D/k/j"
    } else if config.contains("J='navigate:down'") {
        "J/k/j"
    } else {
        "j/k"
    }
}

/// Replace `{nav}` in every string of a snapshot value.
fn expand_nav(value: &serde_json::Value, nav: &str) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => s.replace("{nav}", nav).into(),
        serde_json::Value::Array(items) => items.iter().map(|v| expand_nav(v, nav)).collect(),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), expand_nav(v, nav)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        other => other.clone(),
    }
}

fn frames(
    host: &Buffer,
    app: &Buffer,
    states: (&ProjectStateSnapshot, &ProjectStateSnapshot),
    lang: &str,
    fixture: &serde_json::Value,
    graph_fixture: &serde_json::Value,
    help_open: bool,
) {
    let nav = graph_fixture["_nav"].as_str().unwrap_or("j/k").to_owned();
    let fixture = &expand_nav(fixture, &nav);
    let graph_fixture = &expand_nav(graph_fixture, &nav);
    let (state, host_state) = states;
    assert_eq!(host.area, app.area);
    let normal_root = state.stack.len() == 1
        && state.diff.is_none()
        && !state.filtering
        && state.menu.is_none()
        && state.prompt.is_none()
        && state.notice.is_none();
    let normal_root = normal_root && !help_open;
    let mut expected = host.clone();
    if !state.diff_actions.is_empty() && !help_open {
        let wanted = expected_diff_actions(&host_state.diff_actions, lang);
        assert_eq!(
            state.diff_actions, wanted,
            "all diff action menu entries, including clipped entries"
        );
        for (side, frame, items) in [
            ("host", host, &host_state.diff_actions),
            ("app", app, &wanted),
        ] {
            let language = if lang == "ja" {
                konoma_host_core::lang::Lang::Jp
            } else {
                konoma_host_core::lang::Lang::En
            };
            let spans = konoma_widgets::footer_line::which_key_spans(
                konoma_host_core::help_msg::CoreMsg::WkActionsTitle.tr(language),
                items,
                frame.area.width,
            );
            let mut row = Buffer::empty(Rect::new(0, 0, frame.area.width, 1));
            Paragraph::new(Line::from(spans)).render(row.area, &mut row);
            compare_frames(&row, &cells(frame, frame.area.bottom() - 1))
                .unwrap_or_else(|e| panic!("explicit {side} diff actions :1266: {e}"));
            if side == "app" {
                for x in 0..frame.area.width {
                    expected[(x, frame.area.bottom() - 1)] = row[(x, 0)].clone();
                }
            }
        }
    }
    if help_open && state.stack.len() == 1 && state.diff.is_none() {
        // §4-1:220 and §7-5:967 apply to the root return key's help row as well as its footer.
        // Render each explicit description independently; no observed app cells become expected cells.
        let screen = state.stack.last().unwrap();
        let row = &graph_fixture["root_help"][lang][screen];
        if let Some(keys) = row["key"].as_str() {
            let panel_width = konoma_widgets::help_panel::PANEL_WIDTH
                .min(host.area.width.saturating_sub(2))
                .max(20);
            let panel_x = (host.area.width.saturating_sub(panel_width)) / 2;
            let key_x = panel_x + 3;
            let desc_x = panel_x + 19;
            let width = panel_width - 20;
            for (side, frame) in [("host", host), ("app", app)] {
                // The two panels may differ in height, so each side finds its own row.
                let ys: Vec<_> = (0..frame.area.height)
                    .filter(|y| {
                        keys.chars()
                            .enumerate()
                            .all(|(i, c)| frame[(key_x + i as u16, *y)].symbol() == c.to_string())
                    })
                    .collect();
                assert_eq!(
                    ys.len(),
                    1,
                    "exactly one explicit root return help row on the {side}"
                );
                let y = ys[0];
                let mut wanted = Buffer::empty(Rect::new(0, 0, width, 1));
                Paragraph::new(
                    Line::raw(row[side].as_str().unwrap())
                        .style(Style::new().add_modifier(Modifier::DIM)),
                )
                .render(wanted.area, &mut wanted);
                let mut actual = wanted.clone();
                for x in 0..width {
                    actual[(x, 0)] = frame[(desc_x + x, y)].clone();
                }
                compare_frames(&wanted, &actual)
                    .unwrap_or_else(|e| panic!("explicit {side} root help {screen}: {e}"));
                if side == "app" {
                    for x in 0..width {
                        expected[(desc_x + x, y)] = wanted[(x, 0)].clone();
                    }
                }
            }
        }
    }
    if help_open && state.diff.is_none() {
        let app_help = state
            .canvas_help
            .as_ref()
            .expect("the standalone reports its open canvas help");
        let host_help = host_state
            .canvas_help
            .as_ref()
            .expect("the host reports its open canvas help");
        // The root return row is the one explicit row difference besides the Tabs section
        // (§4-1:220, §7-5:967); its text comes from the fixture.
        let root_row = (state.stack.len() == 1)
            .then(|| &graph_fixture["root_help"][lang][state.stack.last().unwrap()])
            .filter(|row| row["key"].is_string())
            .map(|row| {
                (
                    row["key"].as_str().unwrap().to_owned(),
                    row["host"].as_str().unwrap().to_owned(),
                    row["app"].as_str().unwrap().to_owned(),
                )
            });
        compare_canvas_help(
            &mut expected,
            (host, app),
            (host_help, app_help),
            lang,
            root_row.as_ref(),
            state.stack.last().map(String::as_str) == Some("graph_picker"),
        );
    }
    if normal_root {
        let footer_start = graph_fixture["_footer_start"].as_u64().unwrap_or(0) as u16;
        let footer_width = host.area.width.saturating_sub(footer_start);
        // FEATURE-PLUGIN-ARCHITECTURE.md §4-1:220: "ツリー・タブ・ブックマーク・フォローは無い。`q` で終了。"
        // §10-3:1266: "`Ctrl-t`（新しいタブ）" is absent; §7-5:967:
        // "root の画面の `q` は終了、重ねた画面の `q` は戻る。"
        // Only the footer differs: body/title y coordinates are still compared below.
        let screen = state.stack.last().unwrap().as_str();
        let screen = if screen == "changes" && state.backend.as_deref() == Some("jj") {
            "jj_changes"
        } else {
            screen
        };
        let kind = if state.return_to_preview {
            if screen == "jj_changes" {
                "preview_jj_project".into()
            } else {
                "preview_project".into()
            }
        } else if screen == "graph" {
            format!(
                "{}graph_{}",
                if state.backend.as_deref() == Some("jj") {
                    "jj_"
                } else {
                    ""
                },
                state
                    .selected
                    .as_ref()
                    .map_or("empty", |row| row.kind.as_str())
            )
        } else if screen == "branches" && state.backend.as_deref() == Some("jj") {
            "jj_branches".into()
        } else if state.selected.is_none() {
            format!("empty_{screen}")
        } else {
            screen.into()
        };
        let y = host.area.bottom() - 1;
        let mut graph_chrome = fixture.clone();
        if state.return_to_preview {
            // Design :1266: Ctrl-t belongs only to the host; p/q retain the Preview origin.
            let diff = read_snapshot_for_test("vcs_app_diff");
            for side in ["host", "app"] {
                graph_chrome[lang][&kind][side][footer_width.to_string()] =
                    serde_json::Value::String(konoma_widgets::help_panel::fit_tokens(
                        &diff[lang][&kind][side]
                            .as_str()
                            .unwrap()
                            .split("  ")
                            .map(str::to_owned)
                            .collect::<Vec<_>>(),
                        footer_width,
                    ));
            }
        }
        if let Some(row) = graph_fixture[lang][&kind].as_object() {
            for (side, tokens) in row {
                graph_chrome[lang][&kind][side][footer_width.to_string()] =
                    serde_json::Value::String(konoma_widgets::help_panel::fit_tokens(
                        &tokens
                            .as_str()
                            .unwrap()
                            .split("  ")
                            .map(str::to_owned)
                            .collect::<Vec<_>>(),
                        footer_width,
                    ));
            }
        }
        if let Some(path) = std::env::var_os("KONOMA_VCS_APP_PARITY_DUMP_FOOTERS") {
            // Opt-in regeneration aid: append the drawn footer of each side as
            // `lang<TAB>kind<TAB>side<TAB>width<TAB>text` to the named file and skip the
            // frozen-footer assertion, so a changed footer can be reviewed against the
            // reference text before the snapshot JSON is edited.
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("open footer dump");
            for (side, frame) in [("host", host), ("app", app)] {
                let mut text = String::new();
                let mut x = footer_start;
                while x < footer_start + footer_width {
                    let symbol = frame[(x, y)].symbol();
                    text.push_str(symbol);
                    x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
                }
                // One write per line: scenarios run in parallel processes.
                file.write_all(
                    format!(
                        "{lang}\t{kind}\t{side}\t{footer_width}\t{}\n",
                        text.trim_end()
                    )
                    .as_bytes(),
                )
                .expect("write footer dump");
            }
            for x in 0..footer_width {
                expected[(footer_start + x, y)] = app[(footer_start + x, y)].clone();
            }
        } else {
            let fixture = &graph_chrome;
            if fixture[lang][&kind]["host"][footer_width.to_string()].is_null()
                && fixture[lang][&kind]["host"]["180"].is_null()
            {
                let plain = |frame: &Buffer| -> String {
                    (0..frame.area.width)
                        .map(|x| frame[(x, y)].symbol())
                        .collect()
                };
                panic!(
                    "missing explicit footer {lang}/{kind}/{}\nhost: {:?}\napp: {:?}",
                    host.area.width,
                    plain(host),
                    plain(app)
                );
            }
            for (side, frame) in [("host", host), ("app", app)] {
                let mut actual = Buffer::empty(Rect::new(0, 0, footer_width, 1));
                for x in 0..footer_width {
                    actual[(x, 0)] = frame[(footer_start + x, y)].clone();
                }
                compare_frames(
                    &expected_footer(fixture, lang, &kind, side, footer_width),
                    &actual,
                )
                .unwrap_or_else(|e| panic!("explicit {side} footer {kind}: {e}"));
            }
            let wanted = expected_footer(fixture, lang, &kind, "app", footer_width);
            for x in 0..footer_width {
                expected[(footer_start + x, y)] = wanted[(x, 0)].clone();
            }
        }
    }
    if state.diff.as_ref().is_some_and(|d| {
        matches!(
            d.preview_context.as_deref(),
            Some("Document" | "Image" | "Text")
        )
    }) && !help_open
        && state.menu.is_none()
        && state.notice.is_none()
    {
        // fix-2 explicitly supplies absent capabilities in the standalone. Every cell of
        // either capability-dependent footer is checked before comparing the remaining frame.
        let diff = read_snapshot_for_test("vcs_app_diff");
        // Design :476: text/code search, range selection and Y are host-only;
        // design :1266: tabs, follow and file jump are absent from the standalone.
        let kind = if state.diff.as_ref().unwrap().preview_context.as_deref() == Some("Text") {
            "preview_text"
        } else if state.diff.as_ref().unwrap().preview_context.as_deref() == Some("Image") {
            "preview_image"
        } else {
            "preview_document"
        };
        let start = graph_fixture["_footer_start"].as_u64().unwrap_or(0) as u16;
        let width = host.area.width.saturating_sub(start);
        let mut chrome = fixture.clone();
        for (side, frame) in [("host", host), ("app", app)] {
            let media = read_snapshot_for_test("vcs_app_media");
            let tokens = if kind == "preview_image"
                && state
                    .diff
                    .as_ref()
                    .unwrap()
                    .path
                    .extension()
                    .is_some_and(|ext| ext == "pdf")
            {
                media["paged_preview"][lang][side].as_str().unwrap()
            } else {
                diff[lang][kind][side].as_str().unwrap()
            };
            // A binary/failed preview has no changed source line to jump to. Its footer
            // uses the same declared tokens, with the inactive change operation omitted.
            let tokens = tokens
                .split("  ")
                .filter(|token| {
                    state.diff.as_ref().unwrap().first_change.is_some()
                        || !token.starts_with("n/N:")
                })
                .collect::<Vec<_>>()
                .join("  ");
            chrome[lang]["preview"][side][width.to_string()] =
                serde_json::Value::String(konoma_widgets::help_panel::fit_tokens(
                    &tokens.split("  ").map(str::to_string).collect::<Vec<_>>(),
                    width,
                ));
            let mut actual = Buffer::empty(Rect::new(0, 0, width, 1));
            for x in 0..width {
                actual[(x, 0)] = frame[(start + x, frame.area.bottom() - 1)].clone();
            }
            compare_frames(
                &expected_footer(&chrome, lang, "preview", side, width),
                &actual,
            )
            .unwrap_or_else(|e| panic!("explicit {side} project Preview capabilities footer: {e}"));
        }
        let wanted = expected_footer(&chrome, lang, "preview", "app", width);
        for x in 0..width {
            expected[(start + x, host.area.bottom() - 1)] = wanted[(x, 0)].clone();
        }
    }
    if let Some(app_help) = state.diff.as_ref().and_then(|d| d.help.as_ref()) {
        let host_help = host_state
            .diff
            .as_ref()
            .and_then(|d| d.help.as_ref())
            .expect("host Preview help");
        compare_preview_help(
            &mut expected,
            host,
            app,
            host_help,
            app_help,
            lang,
            state.diff.as_ref().unwrap().preview_context.as_deref() == Some("Document"),
        );
        if state.media.is_some()
            && host_state.diff.as_ref().is_some_and(|d| {
                d.path.extension().is_some_and(|ext| ext == "svg")
                    && d.effective == konoma_host_core::project::DiffView::Source
                    && d.hscroll > 0
            })
        {
            let names_horizontal = host_help
                .sections
                .iter()
                .flat_map(|(_, rows)| rows)
                .any(|(keys, _)| keys.contains("h / l"));
            if !names_horizontal {
                eprintln!(
                    "Source horizontal state: {:?}",
                    host_state.diff.as_ref().map(|d| (&d.path, d.hscroll))
                );
                dump_parity_frame(
                    "host Source horizontal motion works but help omits it",
                    host,
                );
                dump_parity_frame("app Source horizontal motion works but help omits it", app);
            }
            assert!(
                names_horizontal,
                "Source x moved, so the full help must name horizontal motions"
            );
        }
    }
    compare_ported_frame(&expected, app).unwrap_or_else(|e| {
        dump_parity_frame("host whole frame", host);
        dump_parity_frame("app whole frame", app);
        panic!("whole frame: {e}")
    });
}

fn compare_states(host: &ProjectStateSnapshot, app: &ProjectStateSnapshot) -> Result<(), String> {
    let has_help = |s: &ProjectStateSnapshot| s.diff.as_ref().is_some_and(|d| d.help.is_some());
    if has_help(host) != has_help(app) {
        return Err("Preview help visibility differs".into());
    }
    if host.canvas_help.is_some() != app.canvas_help.is_some() {
        return Err("canvas help visibility differs".into());
    }
    let mut host = host.clone();
    let mut app = app.clone();
    // The canvas help's sections and cells are checked by compare_canvas_help.
    host.canvas_help = None;
    app.canvas_help = None;
    if !host.diff_actions.is_empty() || !app.diff_actions.is_empty() {
        let lang = if host.diff_actions.iter().any(|(_, label)| label == "パス") {
            "ja"
        } else {
            "en"
        };
        let wanted = expected_diff_actions(&host.diff_actions, lang);
        if app.diff_actions != wanted {
            return Err(format!(
                "explicit diff actions: expected {wanted:?}, app {:?}",
                app.diff_actions
            ));
        }
        host.diff_actions = wanted;
    }
    // Full help contents and each rendered viewport are checked by compare_preview_help.
    if let Some(diff) = host.diff.as_mut() {
        diff.help = None;
    }
    if let Some(diff) = app.diff.as_mut() {
        diff.help = None;
    }
    if host == app {
        Ok(())
    } else {
        Err(format!("host: {host:?}\napp: {app:?}"))
    }
}

fn run(
    root: &Path,
    id: &str,
    text: &str,
    steps: Vec<Step>,
    failure: bool,
    check: impl FnMut(usize, &ProjectStateSnapshot, &Sim, &[String]),
) {
    run_roots((root, root), id, text, steps, failure, check);
}

fn run_roots(
    (root, app_root): (&Path, &Path),
    id: &str,
    text: &str,
    steps: Vec<Step>,
    failure: bool,
    check: impl FnMut(usize, &ProjectStateSnapshot, &Sim, &[String]),
) {
    run_roots_with(
        (root, app_root),
        id,
        text,
        steps,
        failure,
        RunOptions::default(),
        check,
    );
}

#[derive(Default)]
struct RunOptions {
    port_branches: bool,
    idle_workers: Option<IdleWorkers>,
    colocated: bool,
    plugin: Option<Arc<dyn Plugin>>,
    contract: bool,
    diff_entry_remap: bool,
    pictures: bool,
    media: bool,
    worktrees: bool,
    remove_switch_target: bool,
    launches: Option<Arc<Mutex<Vec<LaunchRecord>>>>,
    root_jobs: Option<RootJobCase>,
}

fn run_roots_with(
    (root, app_root): (&Path, &Path),
    id: &str,
    text: &str,
    steps: Vec<Step>,
    failure: bool,
    options: RunOptions,
    mut check: impl FnMut(usize, &ProjectStateSnapshot, &Sim, &[String]),
) {
    use crate::plugins::test_projects::{ObservedVcs, VcsCalls};
    let host_calls = Arc::new(Mutex::new(VcsCalls::default()));
    let app_calls = Arc::new(Mutex::new(VcsCalls::default()));
    let mut root_jobs = options
        .root_jobs
        .map(|case| RootJobs::new(root, app_root, case));
    for calls in [&host_calls, &app_calls] {
        calls.lock().unwrap().remove_switch_target = options.remove_switch_target;
    }
    let mut cfg: Config = toml::from_str(text).unwrap();
    let plugin = || -> Arc<dyn Plugin> {
        if let Some(plugin) = &options.plugin {
            return plugin.clone();
        }
        match id {
            "git" => Arc::new(konoma_vcs::Git),
            "jj" => Arc::new(konoma_vcs::Jj),
            _ => unreachable!(),
        }
    };
    let write_actions: Vec<_> = plugin()
        .manifest()
        .actions
        .into_iter()
        .filter(|a| a.writes)
        .map(|a| a.id)
        .collect();
    let (host_plugin, host_graph) = probe_graph_plugin_for_test(Arc::new(ObservedVcs {
        plugin: root_jobs
            .as_ref()
            .map_or_else(plugin, |jobs| jobs.host.wrap(plugin())),
        calls: host_calls.clone(),
    }));
    static PROBE_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let marker = crate::plugins::test_plugins::register_replacement(
        &format!(
            "vcs_app_probe_{}",
            PROBE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        host_plugin,
    );
    let previous = cfg
        .plugins
        .keys()
        .find(|key| key.starts_with("+test:"))
        .unwrap()
        .clone();
    let settings = cfg.plugins.remove(&previous).unwrap();
    cfg.plugins.insert(marker, settings);
    let lang = cfg.ui.lang.clone();
    let read_only = cfg
        .plugins
        .get(id)
        .and_then(|p| p.get("read_only"))
        .and_then(toml::Value::as_bool)
        == Some(true);
    let mut host = Sim::with_config_sized(root, cfg, 90, 14);
    if options.pictures {
        host = host.with_media();
    }
    if options.idle_workers.is_some() {
        host = host.with_async_md_diff();
    }
    let host_media_replies = options.idle_workers.as_ref().map(|_| {
        let (tx, rx) = std::sync::mpsc::channel();
        host.app.attach_media_diff_loader(tx);
        rx
    });
    let image_replies = options.pictures.then(|| {
        let (requests, receive_requests) = tokio::sync::mpsc::unbounded_channel();
        let (answers, mut receive_answers) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || crate::resize_worker(receive_requests, answers));
        let (publish, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Some(answer) = receive_answers.blocking_recv() {
                if publish.send(answer).is_err() {
                    break;
                }
            }
        });
        host.app
            .attach_image_backend(ratatui_image::picker::Picker::halfblocks(), requests);
        replies
    });
    if options.launches.is_some() {
        host.app.set_office_runner(Arc::new(|_| {
            Err(std::io::Error::other("test: unexpected GUI launch"))
        }));
    }
    host.app.tab.projects.wait_for_result();
    host.app.poll_projects();
    host.key('p');
    drain(&mut host);
    assert!(host.app.project_active(), "host must open the real project");
    let mut plugins: Vec<Arc<dyn Plugin>> =
        konoma_vcs::program().into_iter().map(Arc::from).collect();
    let (app_plugin, app_graph) = probe_graph_plugin_for_test(Arc::new(ObservedVcs {
        plugin: root_jobs
            .as_ref()
            .map_or_else(plugin, |jobs| jobs.app.wrap(plugin())),
        calls: app_calls.clone(),
    }));
    *plugins
        .iter_mut()
        .find(|plugin| plugin.manifest().id == id)
        .unwrap() = app_plugin;
    plugins.extend(konoma_file_plugins::plugins());
    let mut app = prepare_projects(
        plugins,
        app_root,
        TermCaps::new(),
        (SharedConfig::parse(text).unwrap(), None),
        options.idle_workers.as_ref().map_or_else(
            || {
                Arc::new(konoma_vcs::standalone::VcsServices)
                    as Arc<dyn konoma_plugin::app::project::ProjectServices>
            },
            |workers| workers.services(),
        ),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    if options.colocated {
        // This scenario owns an isolated process. Keep the ordinary detection default at auto;
        // each project diff must carry its explicit selection into its workers instead.
        crate::vcs::set_preference(crate::vcs::Preference::Auto);
    }
    assert_eq!(
        app.screen.as_ref().unwrap().project.plugin,
        id,
        "standalone backend"
    );
    assert_eq!(
        host.app
            .tab
            .project_navigation
            .as_ref()
            .unwrap()
            .project
            .plugin,
        id,
        "host backend"
    );
    assert!(host.app.tab.projects.uses_injected_events());
    assert!(app.detection.uses_injected_events());
    if let Some(jobs) = &mut root_jobs {
        jobs.attach(&mut host, &mut app);
    }
    let clipboard = app.clipboard_for_test();
    if options.media {
        app.observe_media_for_test();
    }
    clipboard.fail(failure);
    let backend = Arc::new(Mutex::new(TestBackend::new(90, 14)));
    let mut terminal = Terminal::new(SharedBackend(backend.clone())).unwrap();
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let mut graph_fixture = read_snapshot_for_test("vcs_app_graph");
    for lang in ["en", "ja"] {
        for prefix in ["", "jj_"] {
            for side in ["host", "app"] {
                let source = format!("{prefix}graph_commit");
                let target = format!("{prefix}graph_empty");
                let detail = if lang == "en" {
                    "  l/Enter:detail"
                } else {
                    "  l/Enter:詳細"
                };
                graph_fixture[lang][&target][side] = graph_fixture[lang][&source][side]
                    .as_str()
                    .unwrap()
                    .replace(detail, "")
                    .into();
            }
        }
    }
    if options.contract {
        let contract = read_snapshot_for_test("vcs_app_diff");
        for lang in ["en", "ja"] {
            for screen in CONTRACT_ROOTS {
                // The first root is the home screen (it closes); the others are pushed screens.
                let key = if screen == "one" {
                    "contract_home"
                } else {
                    "contract"
                };
                graph_fixture[lang][screen] = contract[lang][key].clone();
            }
        }
    }
    let worktree_fixture = options
        .worktrees
        .then(|| read_snapshot_for_test("vcs_app_worktrees"));
    if root != app_root || options.port_branches {
        // These are the same §4-1:220 / §7-5:967 root-footer exceptions as U8b/U8c.
        let write_fixture = read_snapshot_for_test("vcs_app_writes");
        for lang in ["en", "ja"] {
            for screen in ["branches", "changes", "empty_changes", "jj_changes"] {
                let kind = if read_only {
                    format!("readonly_{screen}")
                } else {
                    screen.into()
                };
                if !write_fixture[lang][&kind].is_null() {
                    graph_fixture[lang][screen] = write_fixture[lang][&kind].clone();
                }
            }
        }
    }
    if options.diff_entry_remap {
        let diff = read_snapshot_for_test("vcs_app_diff");
        for lang in ["en", "ja"] {
            graph_fixture[lang]["changes"] = diff[lang]["changes_remap"].clone();
        }
    }
    let mut host_writes = Vec::new();
    let mut host_launches = Vec::new();
    let app_launches = Arc::new(Mutex::new(Vec::new()));
    let launch_backend = Arc::new(Mutex::new(id.to_owned()));
    // Every scenario gives the standalone app its launch seam: the host footer always offers
    // `!:tool`, and the standalone footer offers it exactly when this seam exists. Launches are
    // only compared in the scenarios that set `options.launches`.
    let launch = Some({
        let sink = app_launches.clone();
        let backend = launch_backend.clone();
        konoma_plugin::app::LaunchCx::new(app.config.editor.clone(), &app.config.external, vec![])
            .with_hand_off(Arc::new(move |attempt, cwd, redraw| {
                sink.lock().unwrap().push(LaunchRecord::new(
                    attempt,
                    cwd,
                    &backend.lock().unwrap(),
                ));
                redraw()?;
                Ok(Some(0))
            }))
    });
    let mut count = steps.len();
    let mut steps = std::collections::VecDeque::from(steps);
    let mut step = 0;
    let mut expected_acts = Vec::<String>::new();
    let mut refresh_before = None;
    let cx = ProjectLoopCx {
        home: std::env::var_os("HOME").map(Into::into),
        launch,
    };
    // Public prepared state can be reconfigured only between runs. Ordinary scenarios still
    // use one uninterrupted run; the submit-boundary case retains its real dialog and owners.
    loop {
        let mut policy_change = None;
        let observe = |idle: konoma_plugin::app::project::ProjectIdleSnapshot,
                       frame: Buffer,
                       mut state: ProjectStateSnapshot| {
            if let Some(jobs) = &mut root_jobs {
                return jobs.observe(idle, frame, state, &mut host, &fixture, &graph_fixture);
            }
            if !idle.is_idle() {
                let workers = options
                    .idle_workers
                    .as_ref()
                    .expect("only the idle-worker scenario observes pending work");
                if matches!(steps.front(), Some(Step::BusyKey(_))) {
                    if workers.publications("source") == 1 && workers.publications("resolve") == 0 {
                        // The source reply is adopted, while classification is still held.
                        // An empty binary source is not a completed, unchanged media diff.
                        let text = frame
                            .content
                            .iter()
                            .map(|cell| cell.symbol())
                            .collect::<String>();
                        assert!(text.contains("loading…"), "unresolved media frame: {text}");
                        assert!(!text.contains("(no changes)"));
                        assert!(!state.media.as_ref().unwrap().pending);
                    }
                    for kind in workers.initial_order {
                        if workers.publications(kind) == 0 {
                            workers.release_kind(kind);
                            return Next::Timeout;
                        }
                    }
                }
                if matches!(steps.front(), Some(Step::BusyKey(_)))
                    && state.media.as_ref().is_some_and(|m| m.pending)
                {
                    if workers.publications("source") == 0 {
                        workers.release_kind("source");
                        return Next::Timeout;
                    }
                    compare_pending_media_key_frame(&mut host, &state, &frame, root, app_root);
                    let Some(Step::BusyKey(key)) = steps.pop_front() else {
                        unreachable!()
                    };
                    step += 1;
                    host.press(key.code, key.modifiers);
                    drain(&mut host);
                    workers
                        .busy_key_sent
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return Next::Event(AppEvent::Key(key_from_crossterm(&key).unwrap()));
                }
                if workers
                    .busy_key_sent
                    .load(std::sync::atomic::Ordering::SeqCst)
                    && !workers.queued.load(std::sync::atomic::Ordering::SeqCst)
                {
                    for kind in ["source", "resolve"] {
                        if workers.publications(kind) < 2 {
                            workers.release_kind(kind);
                            return Next::Timeout;
                        }
                    }
                }
                if state.media.as_ref().is_some_and(|m| m.queued.is_some()) {
                    assert!(
                        host.app.media_diff_for_test().queued().is_some(),
                        "both real media workers queue the refresh"
                    );
                    assert_eq!(
                        crate::poll_timeout(&host.app),
                        std::time::Duration::from_millis(100),
                        "the host's existing idle poll wakes queued media"
                    );
                    let (host_frame, paints) = host.project_frame_with_paints();
                    let mut host_media = host_media_snapshot(&host, &state, paints);
                    let mut app_media = state.media.clone().unwrap();
                    for (media, repo) in [(&mut host_media, root), (&mut app_media, app_root)] {
                        if let Some(want) = media.queued.as_mut() {
                            want.0 = Path::new("<repo>").join(want.0.strip_prefix(repo).unwrap());
                        }
                    }
                    if host_media != app_media {
                        dump_parity_frame("host queued media", &host_frame);
                        dump_parity_frame("app queued media", &frame);
                    }
                    assert_eq!(
                        host_media, app_media,
                        "queued media state before publication"
                    );
                    compare_pending_media_key_frame(&mut host, &state, &frame, root, app_root);
                    workers
                        .queued
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                workers.release_one();
                return Next::Timeout;
            }
            assert!(idle.is_idle());
            if let Some(workers) = &options.idle_workers {
                for _ in 0..16 {
                    if !host.app.md_diff_pending_for_current() {
                        break;
                    }
                    assert!(
                        crate::poll_timeout(&host.app) <= std::time::Duration::from_millis(100),
                        "host Markdown worker has an idle deadline"
                    );
                    host.drain_md_diff();
                }
                for _ in 0..16 {
                    if !host.app.media_diff_for_test().busy() {
                        break;
                    }
                    assert!(
                        crate::poll_timeout(&host.app) <= std::time::Duration::from_millis(100),
                        "host media worker has an idle deadline"
                    );
                    let reply = host_media_replies
                        .as_ref()
                        .unwrap()
                        .recv_timeout(ROOT_WAIT)
                        .expect("host media reply publication");
                    host.app.apply_media_diff(reply);
                    host.draw();
                }
                if state
                    .media
                    .as_ref()
                    .is_some_and(|m| m.cache.iter().any(|c| c.frame == 1))
                {
                    host.app.advance_md_gifs_if_due();
                    if host
                        .app
                        .media_cache_for_test()
                        .iter()
                        .any(|entry| entry.frames > 1 && entry.frame == 0)
                    {
                        host.app.age_md_gif_frames_for_test();
                        host.app.advance_md_gifs_if_due();
                    }
                    workers.gif.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            if options.worktrees && !host.app.project_active() {
                check_switched_tree(&mut host, &state, root, app_root);
            }
            if let Some(sink) = &options.launches {
                let identity = if options.worktrees {
                    root.parent().unwrap()
                } else {
                    root
                };
                let app_identity = if options.worktrees {
                    app_root.parent().unwrap()
                } else {
                    app_root
                };
                let normalized_host: Vec<_> = host_launches
                    .iter()
                    .map(|r: &LaunchRecord| r.normalized(identity))
                    .collect();
                let normalized_app: Vec<_> = app_launches
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|r| r.normalized(app_identity))
                    .collect();
                if normalized_host != normalized_app {
                    dump_parity_frame("host launch fence", &host.project_frame());
                    dump_parity_frame("app launch fence", &frame);
                }
                assert_eq!(
                    normalized_host, normalized_app,
                    "step {step}: launch command/target/line/backend/cwd"
                );
                *sink.lock().unwrap() = normalized_app;
                if let Some(backend) = &state.backend {
                    *launch_backend.lock().unwrap() = backend.clone();
                }
            }
            if options.pictures {
                host.settle_inline_images(
                    "project diff picture fence",
                    std::time::Duration::from_secs(30),
                );
            }
            let (mut host_frame, host_paints) = host.project_frame_with_paints();
            if let Some(replies) = &image_replies {
                for _ in 0..8 {
                    if !host.app.image_viewer.resize_pending() {
                        break;
                    }
                    let answer = replies
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .expect("project Preview resize worker reply");
                    host.app.apply_image_resize(answer);
                    host_frame = host.project_frame();
                }
                assert!(
                    !host.app.image_viewer.resize_pending(),
                    "the shown picture reaches its resize fence"
                );
            }
            let mut frame = frame;
            let mut host_snapshot = host_state(&host);
            host_snapshot.diff = host_diff_snapshot(&mut host);
            if host.app.show_help && host_snapshot.diff.is_none() {
                host_snapshot.canvas_help =
                    Some(konoma_plugin::app::project::ProjectHelpSnapshot {
                        sections: crate::ops::help::sections(&host.app)
                            .into_iter()
                            .map(|s| (s.title, s.rows))
                            .collect(),
                        scroll: usize::from(host.app.help_scroll),
                        title: crate::ops::core_help::help_title(&host.app),
                    });
            }
            if host_snapshot.diff.is_some() && host_snapshot.menu.as_deref() == Some("actions") {
                host_snapshot.diff_actions = crate::ops::menu::menu(
                    &host.app,
                    crate::keymap::Context::Diff,
                    crate::keymap::LeaderId::Actions,
                    crate::ops::menu::View::Open,
                )
                .unwrap()
                .items
                .into_iter()
                .map(|i| (konoma_host_core::help::help_key(i.key), i.label))
                .collect();
            }
            if options.media {
                host_snapshot.media = Some(host_media_snapshot(&host, &host_snapshot, host_paints));
            }
            if root != app_root {
                let host_identity = if options.worktrees {
                    root.parent().unwrap()
                } else {
                    root
                };
                let app_identity = if options.worktrees {
                    app_root.parent().unwrap()
                } else {
                    app_root
                };
                normalize_write_state(&mut host_snapshot, host_identity);
                normalize_write_state(&mut state, app_identity);
                normalize_write_frame(&mut host_frame, root);
                normalize_write_frame(&mut frame, app_root);
                if options.colocated {
                    assert_colocated_repositories(root, app_root);
                } else {
                    assert_write_repositories(root, app_root, id, step);
                }
                let writes = |calls: &Arc<Mutex<VcsCalls>>| {
                    calls
                        .lock()
                        .unwrap()
                        .acts
                        .iter()
                        .filter(|a| write_actions.contains(a))
                        .cloned()
                        .collect::<Vec<_>>()
                };
                compare_dispatched_writes(
                    &writes(&host_calls),
                    &writes(&app_calls),
                    &expected_acts,
                )
                .unwrap_or_else(|e| panic!("step {step}: {e}"));
                if let Some(((host_views, host_chips), (app_views, app_chips))) =
                    refresh_before.take()
                {
                    let host = host_calls.lock().unwrap();
                    let app = app_calls.lock().unwrap();
                    let view_counts = (host.views - host_views, app.views - app_views);
                    let chip_counts = (host.chips - host_chips, app.chips - app_chips);
                    assert_eq!(
                        view_counts.0, view_counts.1,
                        "step {step}: screen refresh count after write"
                    );
                    assert_eq!(
                        chip_counts.0, chip_counts.1,
                        "step {step}: chip refresh count after write"
                    );
                }
            }
            let picker_cursor = |snapshot: &ProjectStateSnapshot| {
                (snapshot.stack.last().map(String::as_str) == Some("graph_picker"))
                    .then_some(snapshot.cursor)
            };
            host_snapshot.graph = host_graph.snapshot(picker_cursor(&host_snapshot));
            state.graph = app_graph.snapshot(picker_cursor(&state));
            let comparison = compare_states(&host_snapshot, &state);
            comparison.unwrap_or_else(|e| {
                dump_parity_frame("host state fence", &host_frame);
                dump_parity_frame("app state fence", &frame);
                panic!("step {step}: screen/identity/filter/cursor/marks/prompt/menu/notice/graph/root/open_dir: {e}")
            });
            assert_eq!(
                host_writes,
                clipboard.writes(),
                "step {step}: every clipboard write"
            );
            if let Some(row) = &state.selected {
                assert!(
                    state
                        .rows
                        .iter()
                        .any(|(r, selectable)| r == row && *selectable),
                    "a non-selectable row was selected"
                );
            }
            if root != app_root && cells(&host_frame, 0) != cells(&frame, 0) {
                for (side, frame) in [("host", &host_frame), ("app", &frame)] {
                    eprintln!(
                        "{side} complete frame ({}x{}):",
                        frame.area.width, frame.area.height
                    );
                    for y in 0..frame.area.height {
                        let row: String = (0..frame.area.width)
                            .map(|x| frame[(x, y)].symbol())
                            .collect();
                        eprintln!("{y:02}: {row}");
                    }
                }
            }
            if let Some(fixture) = &worktree_fixture {
                let kind = if read_only { "readonly" } else { "write" };
                let target = state
                    .selected
                    .as_ref()
                    .is_some_and(|row| row.kind == "worktree");
                let row = if target {
                    "target"
                } else if state.selected.is_none() {
                    "empty"
                } else {
                    "plain"
                };
                graph_fixture[&lang]["worktrees"] = fixture[&lang][kind][row].clone();
                graph_fixture[&lang]["empty_worktrees"] = fixture[&lang][kind]["empty"].clone();
            }
            graph_fixture["_nav"] = serde_json::json!(nav_chip(text));
            graph_fixture["_footer_start"] =
                serde_json::json!(if host.app.cfg.ui.statusbar == "bottom" {
                    crate::ui::status::context_width(&host.app)
                        .saturating_add(2)
                        .min(host_frame.area.width)
                } else {
                    0
                });
            frames(
                &host_frame,
                &frame,
                (&state, &host_snapshot),
                &lang,
                &fixture,
                &graph_fixture,
                host.app.show_help,
            );
            check(step, &state, &host, &host_writes);
            let Some(mut next) = steps.pop_front() else {
                return Next::End;
            };
            if matches!(next, Step::HelpPages) {
                let help = state
                    .diff
                    .as_ref()
                    .and_then(|diff| diff.help.as_ref())
                    .expect("HelpPages requires the actual open help");
                let refs = help
                    .sections
                    .iter()
                    .map(|(title, rows)| (title.as_str(), rows.as_slice()))
                    .collect::<Vec<_>>();
                let app_lines = konoma_widgets::help_panel::help_lines(&refs).len();
                let host_lines = crate::ui::help::help_lines(&host.app).len();
                let area = frame.area;
                let keys = usize::from(
                    konoma_widgets::help_panel::max_scroll(app_lines, area)
                        .max(konoma_widgets::help_panel::max_scroll(host_lines, area)),
                ) + 1;
                assert!(keys <= 500);
                count += keys - 1;
                for _ in 1..keys {
                    steps.push_front(ch('j'));
                }
                next = ch('j');
            }
            step += 1;
            let next = match next {
                Step::BeforeKey(before, key) => {
                    before(root);
                    if root != app_root {
                        before(app_root);
                    }
                    Step::Key(key)
                }
                Step::Write(key, action) => {
                    let counts = |calls: &Arc<Mutex<VcsCalls>>| {
                        let calls = calls.lock().unwrap();
                        (calls.views, calls.chips)
                    };
                    if !options.worktrees {
                        refresh_before = Some((counts(&host_calls), counts(&app_calls)));
                    }
                    expected_acts.push(action.into());
                    Step::Key(key)
                }
                Step::Policy(text) => {
                    policy_change = Some(text);
                    return Next::End;
                }
                other => other,
            };
            let event = match next {
                Step::Resize(width, height) => {
                    host.resize(width, height);
                    backend.lock().unwrap().resize(width, height);
                    AppEvent::Resize
                }
                Step::Key(key) => {
                    clear_test_clipboard();
                    let mut press = || {
                        host.press(key.code, key.modifiers);
                        drain(&mut host);
                        if options.launches.is_some() {
                            let tool = collect_host_launches(&mut host, &mut host_launches);
                            if tool && options.media {
                                // The real standalone/host loops refresh after returning from a
                                // recorded tool handoff; no external program is started here.
                                host.app.refresh().unwrap();
                                drain(&mut host);
                            }
                        }
                    };
                    if failure {
                        App::with_clipboard_failure_for_test(press);
                    } else {
                        press();
                    }
                    if let Some(text) = get_test_clipboard() {
                        host_writes.push(text);
                    }
                    clear_test_clipboard();
                    AppEvent::Key(key_from_crossterm(&key).unwrap())
                }
                Step::BeforeKey(..)
                | Step::Write(..)
                | Step::Policy(..)
                | Step::BusyKey(..)
                | Step::HelpPages => {
                    unreachable!()
                }
            };
            Next::Event(event)
        };
        if options.root_jobs.is_some() {
            konoma_plugin::app::project::run_project_observed_for_test(
                &mut terminal,
                &mut app,
                &cx,
                observe,
            )
            .unwrap();
        } else {
            let pictures = options.pictures.then(|| {
                konoma_plugin::app::Pictures::new(
                    ratatui_image::picker::Picker::halfblocks(),
                    "git",
                    app_root,
                    1024,
                )
                .with_out(Box::new(Vec::<u8>::new()))
            });
            if let Some(workers) = &options.idle_workers {
                workers.run(&mut terminal, &mut app, &cx, pictures, observe);
            } else {
                run_project_fenced_state_for_test(&mut terminal, &mut app, &cx, pictures, observe)
                    .unwrap();
            }
        }
        let Some(text) = policy_change else { break };
        let shared = SharedConfig::parse(&text).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        for (id, settings) in config.plugins {
            host.app.cfg.plugins.insert(id, settings);
        }
        for (id, settings) in shared.plugins {
            app.config.plugins.insert(id, settings);
        }
    }
    assert_eq!(step, count, "every input must reach its fence");
    if let Some(jobs) = &root_jobs {
        assert!(
            jobs.finished,
            "one uninterrupted root loop reaches its final late-reply fence"
        );
    }
    TALLY.with(|t| {
        let (runs, fences) = t.get();
        t.set((
            runs + 1,
            fences + root_jobs.as_ref().map_or(count + 1, |jobs| jobs.fences),
        ));
    });
}

fn dump_parity_frame(side: &str, frame: &Buffer) {
    eprintln!(
        "{side} complete frame ({}x{}):",
        frame.area.width, frame.area.height
    );
    for y in 0..frame.area.height {
        let row: String = (0..frame.area.width)
            .map(|x| frame[(x, y)].symbol())
            .collect();
        eprintln!("{y:02}: {row}");
    }
}

#[test]
fn changes_log_detail_cells_and_state_match_host() {
    if isolated("changes_log_detail_cells_and_state_match_host") {
        return;
    }
    comparator_styles();
    let root = at_home(git_repo("vcs_app_changes", true));
    assert_eq!(crate::git::changed_files(&root).len(), 7);
    for lang in ["en", "ja"] {
        for scheme in ["vim", "less"] {
            for path in ["relative", "home", "full"] {
                for align in ["right", "inline"] {
                    let text = config("git", lang, scheme, path, align, "");
                    changes_case(&root, "git", &text);
                }
            }
        }
    }
    let detail = at_home(detail_repo());
    for lang in ["en", "ja"] {
        for scheme in ["vim", "less"] {
            let text = config(
                "git",
                lang,
                scheme,
                "relative",
                "right",
                "[keys.text]\nJ='navigate:down'\n",
            );
            detail_case(&detail, "git", &text, scheme);
        }
    }
    let filtered = at_home(repo(13));
    for lang in ["en", "ja"] {
        for align in ["right", "inline"] {
            let text = config("git", lang, "vim", "relative", align, "");
            let mut steps = vec![key(KeyCode::Tab), ch('G'), ch('/')];
            steps.extend(typed("作者"));
            steps.extend([
                key(KeyCode::Backspace),
                ch('者'),
                key(KeyCode::Left),
                key(KeyCode::Enter),
                key(KeyCode::Esc),
                ch('/'),
            ]);
            steps.extend(typed("__no_matching_row__"));
            steps.extend([key(KeyCode::Enter), key(KeyCode::Esc), ch('j'), ch('k')]);
            run(&filtered, "git", &text, steps, false, |_, state, _, _| {
                if state.query == "__no_matching_row__" {
                    assert_eq!(state.selected, None);
                }
            });
        }
    }
    // Neither an empty changes screen nor an unborn log has a selectable placeholder.
    for root in [at_home(repo(0)), at_home(git_repo("vcs_app_clean", false))] {
        let text = config("git", "en", "vim", "relative", "right", "");
        let steps = vec![
            ch('j'),
            ch('V'),
            key(KeyCode::Tab),
            ch('j'),
            ch('G'),
            ch('g'),
        ];
        run(&root, "git", &text, steps, false, |_, state, _, _| {
            if state.rows.iter().all(|(_, selectable)| !selectable) {
                assert_eq!(state.selected, None, "empty rows must not be selected");
            }
        });
    }
    let refresh = at_home(repo(3));
    let mut selected = None;
    let text = config("git", "en", "vim", "relative", "right", "");
    let mut steps = vec![key(KeyCode::Tab), ch('j'), ch('/')];
    steps.extend(typed("Summary"));
    steps.push(key(KeyCode::Enter));
    let before = steps.len();
    steps.push(Step::BeforeKey(
        Box::new(|p| commit(p, 3)),
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
    ));
    run(&refresh, "git", &text, steps, false, |step, state, _, _| {
        if step == before {
            selected = state.selected.clone();
        }
        if step == before + 1 {
            assert_eq!(
                state.selected, selected,
                "refresh must restore row identity, not the old index"
            );
            assert_eq!(state.cursor, 2);
        }
    });
    let big = at_home(repo(201));
    limit_case(&big, "git");
    for (root, id) in jj_cases("changes/log/detail") {
        for lang in ["en", "ja"] {
            let text = config(id, lang, "vim", "home", "inline", "");
            changes_case(&root, id, &text);
            detail_case(&root, id, &text, "vim");
        }
        let initial = backend(id).log(&root, 201).len();
        for i in initial..201 {
            if id == "git" {
                commit(&root, i);
            } else {
                super::vcs_log::jj(&root, &["new", "-m", &format!("limit {i}")]);
            }
        }
        limit_case(&root, id);
    }
    report_cases();
}

fn limit_case(root: &Path, id: &str) {
    let commits = backend(id).log(root, 201);
    assert_eq!(
        commits.len(),
        201,
        "the real backend must have the 201st commit"
    );
    let oldest = commits[200].id.clone();
    let text = config(id, "en", "vim", "relative", "right", "");
    run(
        root,
        id,
        &text,
        vec![
            key(KeyCode::Tab),
            ch('j'),
            ch('k'),
            key(KeyCode::PageDown),
            key(KeyCode::PageUp),
            ch('G'),
            ch('k'),
            ch('g'),
        ],
        false,
        |_, state, _, _| {
            if state.stack.last().map(String::as_str) == Some("log") {
                assert_eq!(state.rows.len(), 200, "log shows exactly 200 rows");
                assert!(
                    state.rows.iter().all(|(row, _)| row.value != oldest),
                    "the 201st commit is absent"
                );
            }
        },
    );
}

#[test]
fn copy_values_failures_and_cached_detail_match_host() {
    if isolated("copy_values_failures_and_cached_detail_match_host") {
        return;
    }
    comparator_styles();
    let root = at_home(detail_repo());
    for lang in ["en", "ja"] {
        copy_case(&root, "git", lang);
    }
    let empty = at_home(empty_subject_repo());
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let text = config("git", "en", "vim", "relative", "right", "");
    run(
        &empty,
        "git",
        &text,
        vec![
            key(KeyCode::Tab),
            ch('y'),
            ch('t'),
            key(KeyCode::Enter),
            ch('y'),
            ch('t'),
        ],
        false,
        |step, _, _, writes| {
            assert!(
                writes
                    .iter()
                    .all(|s| s == fixture["empty_subject"].as_str().unwrap()),
                "an empty subject is a valid copy"
            );
            if step == 6 {
                assert_eq!(
                    writes,
                    &[String::new(), String::new()],
                    "both empty-string writes must occur"
                );
            }
        },
    );
    for detail in [false, true] {
        let root = at_home(repo(1));
        let expected = metadata(&root, "git", 0).message;
        let mut steps = vec![key(KeyCode::Tab)];
        if detail {
            steps.push(key(KeyCode::Enter));
        }
        steps.extend([
            Step::BeforeKey(
                Box::new(remove_head_object),
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            ),
            ch('m'),
        ]);
        let end = steps.len();
        run(
            &root,
            "git",
            &text,
            steps,
            false,
            |step, state, _, writes| {
                if step == end {
                    if detail {
                        assert_eq!(
                            writes,
                            std::slice::from_ref(&expected),
                            "removed commit detail still copies shown metadata"
                        );
                    } else {
                        assert!(writes.is_empty());
                        assert!(state.notice.is_some(), "missing commit must report failure");
                    }
                }
            },
        );
    }
    cached_case(&at_home(repo(2)), "git");
    for (root, id) in jj_cases("copy/cache/failure") {
        for lang in ["en", "ja"] {
            copy_case(&root, id, lang);
        }
        cached_case(&root, id);
        if id == "git" {
            empty_subject_commit(&root);
        } else {
            super::vcs_log::jj(&root, &["describe", "-m", ""]);
        }
        empty_copy_case(&root, id);
        // Removing real objects makes the log's metadata read fail; cached details remain usable.
        for detail in [false, true] {
            let broken = at_home(jj_repo(id == "git" || root.join(".git").exists()));
            missing_copy_case(&broken, id, detail);
        }
    }
    report_cases();
}

fn comparator_styles() {
    let expected = Buffer::empty(Rect::new(0, 0, 2, 2));
    for field in 0..3 {
        let mut actual = expected.clone();
        match field {
            0 => actual[(1, 1)].fg = Color::Red,
            1 => actual[(1, 1)].bg = Color::Blue,
            _ => actual[(1, 1)].modifier = Modifier::BOLD,
        }
        assert!(
            compare_frames(&expected, &actual).is_err(),
            "frame comparison must detect style field {field} on the last row"
        );
    }
}

fn changes_case(root: &Path, id: &str, text: &str) {
    let mut steps = vec![
        ch('j'),
        ch('k'),
        ch('V'),
        key(KeyCode::PageDown),
        key(KeyCode::PageUp),
        ch('G'),
        ch('g'),
        ch('/'),
    ];
    steps.extend(typed("日本語"));
    steps.extend([
        key(KeyCode::Left),
        key(KeyCode::Backspace),
        key(KeyCode::Enter),
        key(KeyCode::Esc),
        ch('/'),
    ]);
    steps.extend(typed("__no_matching_row__"));
    steps.extend([
        key(KeyCode::Enter),
        key(KeyCode::Esc),
        Step::Resize(48, 9),
        Step::Resize(180, 20),
    ]);
    // Exercise the real prompt, but cancel before any repository write.
    steps.push(ch(if id == "git" { 'c' } else { 's' }));
    if id == "git" {
        steps.extend(typed("cancel me"));
        steps.push(key(KeyCode::Backspace));
    }
    steps.extend([
        key(KeyCode::Esc),
        key(KeyCode::Tab),
        ch('G'),
        ch('g'),
        key(KeyCode::Enter),
        key(KeyCode::Tab),
        ch('q'),
        key(KeyCode::BackTab),
    ]);
    let end = steps.len();
    run(root, id, text, steps, false, |step, state, _, _| {
        assert!(
            state.marked.is_empty(),
            "changes/log do not declare marks: V must preserve the host's no-op"
        );
        if state.query == "__no_matching_row__" {
            assert_eq!(state.selected, None);
        }
        if state.stack.len() > 1 {
            assert_eq!(
                state.stack.last().unwrap(),
                "detail",
                "detail is pushed, never in the tab cycle"
            );
        }
        if [end - 3, end - 2].contains(&step) {
            assert_eq!(
                state.stack.last().unwrap(),
                "detail",
                "detail must open as a pushed screen and stay there on Tab"
            );
        }
    });
}

fn empty_copy_case(root: &Path, id: &str) {
    let text = config(id, "en", "vim", "relative", "right", "");
    run(
        root,
        id,
        &text,
        vec![
            key(KeyCode::Tab),
            ch('y'),
            ch('t'),
            key(KeyCode::Enter),
            ch('y'),
            ch('t'),
        ],
        false,
        |step, _, _, writes| {
            assert!(writes.iter().all(String::is_empty));
            if step == 6 {
                assert_eq!(
                    writes,
                    &[String::new(), String::new()],
                    "both empty-string copies must be written"
                );
            }
        },
    );
}

fn missing_copy_case(root: &Path, id: &str, detail: bool) {
    let expected = metadata(root, id, 0);
    let commit = expected.id.clone();
    let mut steps = vec![key(KeyCode::Tab)];
    if detail {
        steps.push(key(KeyCode::Enter));
    }
    steps.extend([
        Step::BeforeKey(
            Box::new(move |p| remove_commit_object(p, &commit)),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        ),
        ch('m'),
    ]);
    let end = steps.len();
    let text = config(id, "en", "vim", "relative", "right", "");
    run(root, id, &text, steps, false, |step, state, _, writes| {
        if step == end {
            if detail {
                assert_eq!(
                    writes,
                    std::slice::from_ref(&expected.message),
                    "missing commit detail copies shown metadata"
                );
            } else {
                assert!(writes.is_empty());
                assert!(state.notice.is_some());
            }
        }
    });
}

fn detail_case(root: &Path, id: &str, text: &str, scheme: &str) {
    let mut steps = vec![
        key(KeyCode::Tab),
        ch('j'),
        key(KeyCode::Enter),
        key(KeyCode::Tab),
        ch('j'),
        ch('J'),
        ch('k'),
        ch('G'),
        ch('g'),
        ch('l'),
        ch('h'),
        ch('0'),
        ch('$'),
        key(KeyCode::PageDown),
        key(KeyCode::PageUp),
    ];
    for c in ['d', 'u', 'f', 'b'] {
        steps.push(Step::Key(KeyEvent::new(
            KeyCode::Char(c),
            if scheme == "less" {
                KeyModifiers::NONE
            } else {
                KeyModifiers::CONTROL
            },
        )));
    }
    steps.extend([
        Step::Resize(48, 9),
        Step::Resize(180, 20),
        ch('s'),
        ch('s'),
        ch('q'),
        key(KeyCode::BackTab),
    ]);
    let mut before_remap = 0;
    run(root, id, text, steps, false, |step, state, host, _| {
        if step == 4 {
            assert_eq!(
                state.stack.last().unwrap(),
                "detail",
                "Tab on pushed detail must do nothing"
            );
        }
        if step == 5 {
            before_remap = state.scroll;
        }
        if step == 6 && text.contains("J='navigate:down'") {
            assert_eq!(
                state.scroll,
                before_remap + 1,
                "the shared remapped key must scroll the real detail"
            );
        }
        if state.stack.last().map(String::as_str) == Some("detail") {
            assert!(matches!(
                host.app.project_content().as_deref(),
                Some(ProjectContent::Diff(_))
            ));
        }
    });
}

fn backend(id: &str) -> &'static dyn konoma_vcs_core::vcs::Vcs {
    if id == "git" {
        &konoma_vcs_core::vcs::Git
    } else {
        &konoma_vcs_core::vcs::Jj
    }
}
fn metadata(root: &Path, id: &str, index: usize) -> konoma_vcs_core::git::CommitMeta {
    let row = &backend(id).log(root, 200)[index];
    backend(id).commit_meta(root, &row.id).unwrap()
}
fn copies(meta: &konoma_vcs_core::git::CommitMeta) -> Vec<String> {
    vec![
        meta.short.clone(),
        meta.id.clone(),
        meta.message.lines().next().unwrap_or("").into(),
        meta.message.clone(),
        meta.author.clone(),
        meta.date.clone(),
    ]
}
fn copy_case(root: &Path, id: &str, lang: &str) {
    let expected = copies(&metadata(root, id, 1));
    if id == "git" && expected[3].starts_with("Subject 日本語") {
        assert!(expected[3].contains('\n') && expected[3].contains("日本語"));
    }
    let expected: Vec<_> = expected.iter().chain(&expected).cloned().collect();
    let fixture = read_snapshot_for_test("vcs_app_chrome");
    let mut steps = vec![key(KeyCode::Tab), ch('j')];
    for detail in [false, true] {
        if detail {
            steps.push(key(KeyCode::Enter));
        }
        for key in fixture["copy_keys"].as_array().unwrap() {
            steps.extend([ch('y'), ch(key.as_str().unwrap().chars().next().unwrap())]);
        }
    }
    let end = steps.len();
    let text = config(id, lang, "vim", "relative", "right", "");
    run(
        root,
        id,
        &text,
        steps,
        false,
        |step, state, host, writes| {
            if [3, 5, 7, 9, 11, 13, 16, 18, 20, 22, 24, 26].contains(&step) {
                assert_eq!(state.menu.as_deref(), Some("copy"));
            }
            assert!(writes.len() <= expected.len());
            assert_eq!(
                writes,
                &expected[..writes.len()],
                "all six copy values must describe the selected/displayed commit"
            );
            if [4, 6, 8, 10, 12, 14, 17, 19, 21, 23, 25, 27].contains(&step) {
                assert_eq!(
                    state.notice,
                    Some(konoma_host_core::copy::copied_preview_notice(
                        host.app.lang,
                        writes.last().unwrap()
                    ))
                );
            }
            if step == end {
                assert_eq!(writes.len(), 12, "all six menu items on log and detail");
            }
        },
    );
    let steps = vec![
        key(KeyCode::Tab),
        ch('y'),
        ch('m'),
        key(KeyCode::Enter),
        ch('y'),
        ch('m'),
    ];
    run(root, id, &text, steps, true, |step, state, host, writes| {
        assert!(
            writes.is_empty(),
            "failed clipboard writes must not reach either sink"
        );
        if [3, 6].contains(&step) {
            assert_eq!(
                state.notice,
                Some(konoma_host_core::copy::copy_failed_notice(
                    host.app.lang,
                    &"test clipboard failure"
                ))
            );
            assert!(state
                .notice
                .as_deref()
                .unwrap()
                .contains("test clipboard failure"));
        }
    });
}

fn cached_case(root: &Path, id: &str) {
    let ids: Vec<_> = backend(id)
        .log(root, 200)
        .iter()
        .take(2)
        .map(|r| r.id.clone())
        .collect();
    let expected = [ids[0].clone(), ids[1].clone(), ids[0].clone()];
    let text = config(id, "en", "vim", "relative", "right", "");
    let mut first = None;
    let mut steps = vec![
        key(KeyCode::Tab),
        key(KeyCode::Enter),
        ch('y'),
        ch('h'),
        ch('q'),
        ch('j'),
        key(KeyCode::Enter),
        ch('y'),
        ch('h'),
        ch('q'),
    ];
    let backend_id = id.to_owned();
    steps.push(Step::BeforeKey(
        Box::new(move |p| {
            if backend_id == "git" {
                commit(p, 20);
            } else {
                super::vcs_log::jj(p, &["new", "-m", "moved head"]);
            }
        }),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
    ));
    steps.extend([key(KeyCode::Enter), ch('y'), ch('h')]);
    run(root, id, &text, steps, false, |step, _, host, writes| {
        if step == 2 {
            first = host.app.project_content();
        }
        if step == 12 {
            assert!(
                Arc::ptr_eq(
                    first.as_ref().unwrap(),
                    &host.app.project_content().unwrap()
                ),
                "reopened A must use the cached detail"
            );
        }
        assert_eq!(
            writes,
            &expected[..writes.len()],
            "cached detail copy must use shown metadata after HEAD moved"
        );
        if step == 14 {
            assert_eq!(writes.len(), 3);
        }
    });
}

fn jj_cases(scenario: &str) -> Vec<(std::path::PathBuf, &'static str)> {
    if !crate::vcs::jj::available() {
        eprintln!("NOT RUN: {scenario}: jj is unavailable; jj-only, colocated Git and colocated jj cases were not run");
        return Vec::new();
    }
    let jj = at_home(jj_repo(false));
    let git = at_home(jj_repo(true));
    let colocated_jj = at_home(jj_repo(true));
    eprintln!("RUN: {scenario}: jj-only, colocated Git, colocated jj");
    vec![(jj, "jj"), (git, "git"), (colocated_jj, "jj")]
}

#[test]
fn graph_base_visible_order_picker_and_cells_match_host() {
    if isolated("graph_base_visible_order_picker_and_cells_match_host") {
        return;
    }
    let root = at_home(super::vcs_log::graph_picker_fixture());
    let branch_tips: std::collections::HashMap<_, _> =
        konoma_vcs_core::git::branches_by_recency(&root)
            .into_iter()
            .map(|(name, _, _)| {
                let tip = konoma_vcs_core::git::branch_tip(&root, &name).unwrap();
                (name, tip)
            })
            .collect();
    for lang in ["en", "ja"] {
        for align in ["right", "inline"] {
            let text = config(
                "git",
                lang,
                "vim",
                "relative",
                align,
                "[keys.text]\nD='navigate:down'\n",
            );
            let mut steps = vec![
                key(KeyCode::Tab),
                key(KeyCode::Tab),
                ch('D'),
                ch('j'),
                ch('k'),
            ];
            steps.extend([
                ch('/'),
                ch('日'),
                ch('本'),
                key(KeyCode::Enter),
                key(KeyCode::Esc),
            ]);
            steps.extend([
                Step::Resize(48, 8),
                ch('G'),
                ch('g'),
                Step::Resize(180, 18),
                ch('s'),
                ch('j'),
                ch('s'),
                ch('r'),
                ch('y'),
                ch('h'),
                key(KeyCode::Enter),
                ch('y'),
                ch('m'),
                ch('q'),
                ch('x'),
                ch('x'),
                ch('b'),
                ch('/'),
                ch('b'),
                ch('x'),
                ch('a'),
                ch('c'),
                ch('j'),
                ch('x'),
                ch('J'),
                ch('K'),
                ch('J'),
                key(KeyCode::Enter),
                ch('b'),
                ch('a'),
                ch('q'),
                ch('b'),
                ch('J'),
                key(KeyCode::Esc),
                ch('b'),
                ch('?'),
                ch('q'),
                ch('q'),
            ]);
            let mut applied = None;
            let mut cancelled = None;
            let mut retained_order = None;
            let mut head_selection = None;
            run(&root, "git", &text, steps, false, |step, state, _, _| {
                if state.stack.last().map(String::as_str) == Some("graph_picker") {
                    assert!(!state.filtering, "the picker has no filter");
                    assert!(state.query.is_empty());
                    let graph = state.graph.as_ref().expect("observe the real graph state");
                    assert_eq!(graph.order.len(), 4);
                    assert_eq!(graph.picker_cursor, Some(state.cursor));
                }
                assert!(
                    state
                        .rows
                        .iter()
                        .all(|(row, selectable)| row.kind != "connector" || !selectable),
                    "connector rows must never be selectable"
                );
                if step == 2 {
                    assert!(state.rows.iter().any(|(row, _)| row.kind == "connector"));
                    let mut different = state.clone();
                    different
                        .graph
                        .as_mut()
                        .unwrap()
                        .visible
                        .push("__comparison_probe__".into());
                    assert!(
                        compare_states(state, &different).is_err(),
                        "the state comparison must include the graph visible set"
                    );
                    assert_eq!(state.graph.as_ref().unwrap().visible.len(), 4);
                }
                if step == 29 {
                    head_selection = Some(state.graph.as_ref().unwrap().tentative.clone());
                }
                if step == 30 {
                    assert_eq!(
                        Some(state.graph.as_ref().unwrap().tentative.clone()),
                        head_selection,
                        "HEAD is always visible"
                    );
                }
                if step == 20 {
                    applied = state.selected.clone();
                }
                if step == 24 {
                    assert_eq!(
                        state.selected, applied,
                        "detail returns to its graph cursor"
                    );
                }
                if step == 38 {
                    let graph = state.graph.as_ref().unwrap();
                    cancelled = Some(graph.visible.clone());
                    let first = graph
                        .order
                        .iter()
                        .find(|name| graph.visible.contains(name))
                        .unwrap();
                    assert_eq!(
                        graph.label.as_ref(),
                        Some(first),
                        "reorder derives the base from the new order"
                    );
                    assert_eq!(
                        graph.base.as_ref(),
                        branch_tips.get(first),
                        "reorder updates the base commit"
                    );
                }
                if step == 41 || step == 42 {
                    assert_eq!(
                        state.graph.as_ref().unwrap().visible,
                        cancelled.clone().unwrap(),
                        "cancel discards the tentative selection"
                    );
                }
                if step == 43 {
                    retained_order = Some(state.graph.as_ref().unwrap().order.clone());
                }
                if step == 45 {
                    assert_eq!(
                        Some(state.graph.as_ref().unwrap().order.clone()),
                        retained_order,
                        "cancel retains the reordered priority"
                    );
                    assert_eq!(
                        state.graph.as_ref().unwrap().tentative,
                        cancelled.clone().unwrap(),
                        "reopening discards cancelled tentative selection"
                    );
                }
            });
        }
    }
    for lang in ["en", "ja"] {
        for (cap, priority) in [(1, false), (0, true)] {
            let extra = format!("[keys.text]\nD='navigate:down'\n[plugins.git]\ngraph_max_branches={cap}\ngraph_base_branches={}\n", if priority { "['missing-branch','priority-branch-with-a-long-name']" } else { "[]" });
            let text = config("git", lang, "vim", "relative", "right", &extra);
            run(
                &root,
                "git",
                &text,
                vec![
                    key(KeyCode::Tab),
                    key(KeyCode::Tab),
                    ch('r'),
                    ch('b'),
                    ch('q'),
                ],
                false,
                |step, state, _, _| {
                    if step == 2 || step == 3 {
                        let graph = state.graph.as_ref().unwrap();
                        assert_eq!(
                            graph.visible.len(),
                            if cap == 0 { 4 } else { cap },
                            "graph branch cap"
                        );
                        if priority {
                            assert_eq!(
                                graph.order.first().map(String::as_str),
                                Some("priority-branch-with-a-long-name")
                            );
                            assert_eq!(
                                graph.label.as_deref(),
                                Some("priority-branch-with-a-long-name")
                            );
                            assert_eq!(
                                graph.base.as_ref(),
                                branch_tips.get("priority-branch-with-a-long-name")
                            );
                        }
                    }
                },
            );
        }
    }
    let big = at_home(repo(401));
    let text = config(
        "git",
        "en",
        "vim",
        "relative",
        "right",
        "[keys.text]\nD='navigate:down'\n",
    );
    run(
        &big,
        "git",
        &text,
        vec![key(KeyCode::Tab), key(KeyCode::Tab), ch('G'), ch('g')],
        false,
        |_, state, _, _| {
            if state.stack.last().map(String::as_str) == Some("graph") {
                assert_eq!(
                    state
                        .rows
                        .iter()
                        .filter(|(row, _)| row.kind == "commit")
                        .count(),
                    400,
                    "graph commit row cap"
                );
            }
        },
    );
    report_cases();
}

#[test]
fn jj_graph_and_bookmarks_offer_only_real_actions() {
    if !crate::vcs::jj::available() {
        eprintln!(
            "NOT RUN: graph/bookmarks: jj unavailable; 8 sessions (jj-only and colocated Git/jj, en/ja) not run; 0 sessions run"
        );
    }
    if !crate::vcs::jj::available() {
        return;
    }
    if isolated("jj_graph_and_bookmarks_offer_only_real_actions") {
        return;
    }
    let root = at_home(super::vcs_log::graph_jj_fixture());
    jj(&root, &["bookmark", "create", "-r", "@-", "prior"]);
    for (case, (root, id)) in std::iter::once((root, "jj"))
        .chain(jj_cases("graph/bookmarks"))
        .enumerate()
    {
        if id == "jj" {
            super::vcs_changes::add_jj_bookmarks(&root);
        }
        for lang in ["en", "ja"] {
            let text = config(
                id,
                lang,
                "vim",
                "relative",
                "right",
                "[keys.text]\nD='navigate:down'\n",
            );
            let mut steps = vec![
                key(KeyCode::Tab),
                key(KeyCode::Tab),
                Step::Resize(180, 18),
                ch('a'),
                ch('a'),
                ch('s'),
                ch('x'),
                ch('?'),
                ch('q'),
                ch('b'),
                ch('/'),
                ch('q'),
                key(KeyCode::Tab),
                ch('j'),
                ch('g'),
                ch('y'),
                ch('n'),
                key(KeyCode::Enter),
                ch('c'),
                ch('d'),
                ch('?'),
                ch('q'),
            ];
            if id == "git" {
                steps.truncate(12);
            }
            let mut range = None;
            run(
                &root,
                id,
                &text,
                steps,
                false,
                |step, state, host, writes| {
                    if id == "jj" && state.stack.last().map(String::as_str) == Some("graph") {
                        assert!(
                            state.graph.as_ref().unwrap().base.is_none(),
                            "jj has no graph base"
                        );
                        // Explicit cells of the drawn graph screen (host and standalone share the
                        // plugin, so comparing them alone cannot see a wrong title or range).
                        let frame = host.screen();
                        assert!(
                            frame.contains("jj graph"),
                            "step {step}: jj graph title on screen: {frame}"
                        );
                        if case == 0 && step >= 3 {
                            // The fixture's default range starts at the pinned trunk; "c1" is older.
                            assert_eq!(
                                frame.contains("c1"),
                                step == 4,
                                "step {step}: only the all-revisions range shows older commits: {frame}"
                            );
                            assert!(
                                frame.contains("Next"),
                                "step {step}: the working-copy description is on screen: {frame}"
                            );
                        }
                        if step == 3 {
                            range = Some(state.rows.clone());
                        }
                        if step == 4 {
                            assert!(state.graph.as_ref().unwrap().all);
                            assert!(state.notice.is_some(), "all/range has a flash");
                            if case == 0 {
                                assert!(
                                    state.rows.len() > range.as_ref().unwrap().len(),
                                    "all expands the default jj range"
                                );
                            }
                        }
                        if step == 5 {
                            assert_eq!(Some(state.rows.clone()), range);
                        }
                    }
                    if id == "jj" && step == 17 && !state.rows.is_empty() {
                        assert_eq!(writes.last(), state.selected.as_ref().map(|row| &row.value));
                    }
                    if id == "jj" {
                        assert!(state.prompt.is_none(), "jj offers no write prompts");
                        let hints =
                            crate::ops::footer::tokens(&host.app, crate::keymap::Context::Canvas);
                        let forbidden: &[&str] = match state.stack.last().map(String::as_str) {
                            Some("graph") => &["s", "x"],
                            Some("branches") => &["c", "d", "Enter", "↵"],
                            _ => &[],
                        };
                        assert!(
                            !hints.iter().any(|hint| hint.split_once(':').is_some_and(
                                |(keys, _)| keys.split('/').any(|key| forbidden.contains(&key))
                            )),
                            "jj has no base/write hints: {hints:?}"
                        );
                    }
                },
            );
        }
    }
    report_cases();
}
