//! A real repository driven through one uninterrupted standalone loop.
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
};

use konoma_host_core::shared::SharedConfig;
use konoma_plugin::{
    app::{
        project::{
            prepare_projects, run_project_fenced_for_test, ProjectBackend, ProjectLoopCx,
            ProjectServiceInfo, ProjectServices, ProjectSettings,
        },
        AppEvent, Next,
    },
    host::project::MatchedProject,
    prelude::*,
    TermCaps,
};
use konoma_vcs_core::git::{ChangeEntry, DiffLine, FileStatus};
use ratatui::{backend::TestBackend, Terminal};

struct Project(Arc<AtomicUsize>);
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("idle", "0")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new(".git", MarkerKind::Directory))
                    .watch(".git")
                    .screen(ProjectScreen::new("changes", "Changes").row_kind("file")),
            )
            .action(
                Act::new("diff")
                    .key("Enter")
                    .project_context(ActContext::screen("changes", ["file"])),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Project { root, .. } = input else {
            unreachable!()
        };
        Ok(Box::new(ProjectSession(root, self.0.clone())))
    }
}
struct ProjectSession(std::path::PathBuf, Arc<AtomicUsize>);
impl Session for ProjectSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Ok(View::List(konoma_plugin::view::ListView {
            rows: changes(&self.0)
                .iter()
                .map(|entry| {
                    konoma_plugin::view::ListRow::new(
                        "file",
                        vec![Line::raw(format!(
                            "{} {}",
                            entry.path.file_name().unwrap().to_string_lossy(),
                            std::fs::read_to_string(&entry.path).unwrap().trim()
                        ))],
                    )
                    .value("file.txt")
                })
                .collect(),
            ..Default::default()
        }))
    }
    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        Ok(Outcome::OpenDiff {
            path: self.0.join("file.txt"),
        })
    }
}
struct Services;
impl ProjectServices for Services {
    fn inspect(
        &self,
        p: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: p.root.clone(),
            backend: ProjectBackend::Git,
        })
    }
    fn file_diff(&self, i: &ProjectServiceInfo, p: &Path) -> Vec<DiffLine> {
        let before = git_output(&i.workdir, &["show", ":file.txt"]);
        let after = std::fs::read_to_string(p).unwrap();
        konoma_vcs_core::git::diff_contents(&before, &after)
    }
    fn changed_files(&self, i: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        changes(&i.workdir)
    }
    fn base_contents(&self, i: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        Some(git_output(&i.workdir, &["show", ":file.txt"]).into_bytes())
    }
    fn spawn_file_diff(
        self: Arc<Self>,
        request: konoma_plugin::app::diff_screen::FileDiffRequest,
    ) -> Result<konoma_plugin::app::diff_screen::FileDiffReply, Failure> {
        request.spawn(self)
    }
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "fixture git command failed");
    String::from_utf8(output.stdout).unwrap()
}

fn changes(root: &Path) -> Vec<ChangeEntry> {
    git_output(root, &["status", "--porcelain=v1"])
        .lines()
        .map(|line| ChangeEntry {
            path: root.join(&line[3..]),
            status: FileStatus::Modified,
            staged: false,
        })
        .collect()
}

#[test]
fn idle_fence_waits_for_landing_but_not_for_live_watch() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIP: git is missing");
        return;
    }
    let parent = konoma_test_support::unique_tmp("project_idle");
    let dir = parent.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    // No identity or process configuration is needed: an index baseline is enough.
    let git = |args: &[&str]| {
        git_output(&dir, args);
    };
    git(&["init", "-q"]);
    std::fs::write(dir.join("file.txt"), "before\n").unwrap();
    git(&["add", "file.txt"]);
    std::fs::write(dir.join("file.txt"), "after\n").unwrap();
    let views = Arc::new(AtomicUsize::new(0));
    let mut prepared = prepare_projects(
        vec![Arc::new(Project(views.clone()))],
        &dir,
        TermCaps::new(),
        (
            SharedConfig::parse("[ui]\nlang='en'\ndiff_view='source'\n[git]\ndiff='unified'\n")
                .unwrap(),
            None,
        ),
        Arc::new(Services),
    )
    .unwrap_or_else(|e| panic!("{}", e.error));
    // Publish an older context first through the real bounded owner mailbox. The following
    // requested view still runs on the serial owner and reads the real repository.
    let stale = Arc::new(AtomicUsize::new(0));
    for runtime in prepared.runtimes.values_mut() {
        let (tx, rx) = mpsc::sync_channel(1);
        let owner = std::mem::replace(&mut runtime.rx, rx);
        let stale = stale.clone();
        std::thread::spawn(move || {
            let mut older = true;
            while let Ok(mut reply) = owner.recv() {
                if older && !reply.contexts.is_empty() {
                    older = false;
                    for context in reply.contexts.values_mut() {
                        context.icons = !context.icons;
                    }
                    for (_, content) in &mut reply.views {
                        *content =
                            Arc::new(konoma_plugin::host::project::ProjectContent::Lines(vec![
                                ratatui::text::Line::raw("older context"),
                            ]));
                    }
                    stale.fetch_add(1, Ordering::SeqCst);
                }
                if tx.send(reply).is_err() {
                    break;
                }
            }
        });
    }
    let watch = prepared.detection.clone();
    let watch_path = prepared.open_dir.join(".git/index");
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    let expected = konoma_plugin::app::project::read_snapshot_for_test("vcs_app_idle");
    let mut keys = [
        KeyCode::Enter,
        KeyCode::Char('R'),
        KeyCode::Char('R'),
        KeyCode::Char('q'),
    ]
    .into_iter();
    let mut frames = 0;
    let mut previous_generation = 0;
    run_project_fenced_for_test(
        &mut terminal,
        &mut prepared,
        &ProjectLoopCx::default(),
        None,
        |snapshot, frame| {
            assert!(snapshot.is_idle(), "{snapshot:?}");
            assert!(
                snapshot.drawn_generation > previous_generation,
                "frame {frames}: a fresh frame must follow state changes; previous={previous_generation} {snapshot:?}"
            );
            previous_generation = snapshot.drawn_generation;
            assert!(watch.watching(), "the resident watcher must remain alive");
            // A live handle alone is insufficient: the resident worker must acknowledge.
            watch.inject_event(Ok(notify::Event::new(notify::EventKind::Access(
                notify::event::AccessKind::Any,
            ))));
            assert_eq!(
                stale.load(Ordering::SeqCst),
                1,
                "older context arrived first"
            );
            assert!(
                views.load(Ordering::SeqCst) >= 2,
                "the asked context must land after the older reply"
            );
            assert_eq!(frame.area, ratatui::layout::Rect::new(0, 0, 40, 8));
            let stage = [0, 5, 1, 2, 3, 4][frames];
            // The frozen frame is the old hub's drawing (text rows and style runs, the whole
            // screen); a list is now inside a frame, so the cells are compared through the frame
            // rule: every row including the last, with foreground, background and modifiers.
            crate::framed::assert_frozen_rows(
                &format!("vcs_app_idle frame {frames}"),
                &frame,
                &expected["frames"][stage]["rows"],
                &expected["frames"][stage]["styles"],
            );
            frames += 1;
            if frames == 1 {
                std::fs::write(dir.join("file.txt"), "watch\n").unwrap();
                watch.inject_event_with_pending_watch_update(Ok(notify::Event::new(
                    notify::EventKind::Any,
                )
                    .add_path(watch_path.clone())));
                return Next::Timeout;
            }
            assert!(
                views.load(Ordering::SeqCst) >= 3,
                "watch update must land before capture"
            );
            keys.next()
                .map(|code| Next::Event(AppEvent::Key(Key::new(code))))
                .unwrap_or(Next::End)
        },
    )
    .unwrap();
    assert_eq!(frames, 6);
}
