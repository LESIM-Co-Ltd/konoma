//! Jj follow reads run on workers; the UI consumes only root/generation-checked results.
//! Git and callers without a worker channel retain the existing synchronous path.

use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;

#[derive(Default)]
pub(super) struct State {
    tx: Option<Sender<FollowResult>>,
    generation: u64,
    read_generation: u64,
    capturing: bool,
    pending: HashSet<PathBuf>,
    landed: HashMap<PathBuf, FileResult>,
    target: Option<PathBuf>,
    pub(super) view: Option<DiffView>,
}

impl State {
    pub(super) fn invalidate_reads(&mut self) {
        self.read_generation = self.read_generation.wrapping_add(1);
        self.pending.clear();
        self.landed.clear();
    }

    pub(super) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.capturing = false;
        self.target = None;
        self.view = None;
        self.invalidate_reads();
    }

    pub(super) fn lines(&self, path: &Path) -> Option<Vec<crate::git::DiffLine>> {
        self.landed.get(path).map(|r| r.lines.clone())
    }

    pub(super) fn contents(&self, path: &Path) -> Option<Vec<u8>> {
        self.landed.get(path)?.contents.clone()
    }
}

struct FileResult {
    contents: Option<Vec<u8>>,
    lines: Vec<crate::git::DiffLine>,
}

enum Payload {
    Captured(FollowBaseline),
    File { path: PathBuf, value: FileResult },
}

/// Plain worker output. A result never retains App or tab state.
pub struct FollowResult {
    root: PathBuf,
    generation: u64,
    read_generation: u64,
    payload: Payload,
}

impl App {
    pub fn attach_follow_loader(&mut self, tx: Sender<FollowResult>) {
        self.follow_worker.tx = Some(tx);
    }

    pub(super) fn jj_follow_workers(&self) -> bool {
        self.follow_worker.tx.is_some() && self.git_vcs == crate::vcs::VcsKind::Jj
    }

    pub fn follow_work_pending(&self) -> bool {
        self.follow_worker.capturing || !self.follow_worker.pending.is_empty()
    }

    pub(super) fn start_follow_capture(&mut self) {
        self.follow_worker.cancel();
        self.follow_baseline = None;
        self.follow_worker.capturing = true;
        let generation = self.follow_worker.generation;
        let root = self.tab.root.clone();
        let tx = self
            .follow_worker
            .tx
            .clone()
            .expect("worker channel attached");
        let config = crate::vcs::WorkerConfig::capture();
        std::thread::spawn(move || {
            config.install();
            let baseline = crate::preview::markdown::compute_or_fallback(
                || capture(&root),
                || FollowBaseline {
                    dirty: HashMap::new(),
                    head: None,
                },
            );
            let _ = tx.send(FollowResult {
                root,
                generation,
                read_generation: 0,
                payload: Payload::Captured(baseline),
            });
        });
    }

    pub(super) fn request_follow_path(&mut self, path: &Path) {
        if !self.jj_follow_workers() || !self.follow_scope_valid() {
            return;
        }
        self.follow_worker.target = Some(path.to_path_buf());
        if self.follow_worker.capturing
            || self.follow_worker.landed.contains_key(path)
            || !self.follow_worker.pending.insert(path.to_path_buf())
        {
            return;
        }
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let baseline = match self.follow_baseline.as_ref() {
            Some(base) => match base.dirty.get(&key) {
                Some(Some(bytes)) => DiffBaseline::FollowSnapshot(bytes.clone()),
                Some(None) => DiffBaseline::Empty,
                None => base
                    .head
                    .as_ref()
                    .map(|sha| DiffBaseline::FollowHead { sha: sha.clone() })
                    .unwrap_or(DiffBaseline::Empty),
            },
            None => DiffBaseline::Empty,
        };
        let root = self.tab.root.clone();
        let path = path.to_path_buf();
        let generation = self.follow_worker.generation;
        let read_generation = self.follow_worker.read_generation;
        let tx = self
            .follow_worker
            .tx
            .clone()
            .expect("worker channel attached");
        let config = crate::vcs::WorkerConfig::capture();
        std::thread::spawn(move || {
            config.install();
            let value = crate::preview::markdown::compute_or_fallback(
                || read_file(&root, &path, baseline),
                || FileResult {
                    contents: None,
                    lines: Vec::new(),
                },
            );
            let _ = tx.send(FollowResult {
                root,
                generation,
                read_generation,
                payload: Payload::File { path, value },
            });
        });
    }

    pub fn apply_follow(&mut self, result: FollowResult) -> bool {
        if result.root != self.tab.root
            || result.generation != self.follow_worker.generation
            || !self.follow_scope_valid()
        {
            return false;
        }
        match result.payload {
            Payload::Captured(base) => {
                self.follow_worker.capturing = false;
                self.follow_baseline = Some(base);
                self.invalidate_diff_caches();
                if let Some(path) = self.follow_worker.target.clone() {
                    self.request_follow_path(&path);
                }
            }
            Payload::File { path, value } => {
                if result.read_generation != self.follow_worker.read_generation {
                    return false;
                }
                self.follow_worker.pending.remove(&path);
                let lines = value.lines.clone();
                self.follow_worker.landed.insert(path.clone(), value);
                self.invalidate_diff_caches();
                if self.follow_mode
                    && self.follow_surface_available()
                    && !self.follow_diff_full
                    && self.follow_worker.target.as_ref() == Some(&path)
                    && self.tab.preview_path.as_ref() == Some(&path)
                    && (!lines.is_empty() || self.diff_media_capable(&path))
                {
                    let view = self
                        .is_git_diff_preview()
                        .then_some(self.tab.diff_view_choice)
                        .or(self.follow_worker.view);
                    self.open_git_diff_with(
                        &path,
                        super::git_view::DiffOpen {
                            follow_scope: true,
                            seeded_diff: Some(DiffCache {
                                path: path.clone(),
                                lines,
                            }),
                            view,
                            ..Default::default()
                        },
                    );
                }
            }
        }
        true
    }
}

fn capture(root: &Path) -> FollowBaseline {
    let head = crate::vcs::current_point(root);
    let mut dirty = HashMap::new();
    let mut total = 0usize;
    let mut paths: Vec<_> = crate::vcs::statuses(root)
        .into_keys()
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    for path in paths {
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        let size = std::fs::metadata(&path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let content = if size > FOLLOW_BASELINE_FILE_CAP
            || total.saturating_add(size) > FOLLOW_BASELINE_TOTAL_CAP
        {
            None
        } else {
            std::fs::read(&path).ok()
        };
        if let Some(bytes) = &content {
            total = total.saturating_add(bytes.len());
        }
        dirty.insert(key, content);
    }
    FollowBaseline { dirty, head }
}

fn read_file(root: &Path, path: &Path, baseline: DiffBaseline) -> FileResult {
    let contents = match baseline {
        DiffBaseline::FollowSnapshot(bytes) => Some(bytes),
        // Keep the existing absent/error-to-empty contract for both backends.
        DiffBaseline::FollowHead { sha } => {
            Some(crate::vcs::contents_at(root, &sha, path).unwrap_or_default())
        }
        DiffBaseline::Vcs => crate::vcs::base_contents(root, path),
        DiffBaseline::Empty => None,
    };
    let lines = contents
        .as_ref()
        .and_then(|old| {
            let meta = std::fs::metadata(path).ok()?;
            if !meta.is_file() || meta.len() > FOLLOW_BASELINE_FILE_CAP as u64 {
                return None;
            }
            let current = std::fs::read(path).ok()?;
            if current.len() > FOLLOW_BASELINE_FILE_CAP {
                return None;
            }
            Some(crate::vcs::diff_contents(
                std::str::from_utf8(old).ok()?,
                std::str::from_utf8(&current).ok()?,
            ))
        })
        .unwrap_or_else(|| crate::vcs::file_diff(root, path));
    FileResult { contents, lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{jj_scratch_seeded, run_jj, unique_tmp};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Duration;

    fn fixture(name: &str) -> Option<(App, Receiver<FollowResult>, crate::test_support::TmpDir)> {
        if !crate::vcs::jj::available() {
            eprintln!("skipping: jj unavailable");
            return None;
        }
        let dir = jj_scratch_seeded(name).expect("jj fixture");
        std::fs::write(dir.join("a.txt"), b"one\n").unwrap();
        assert!(run_jj(&dir, &["status"]));
        let root = dir.canonicalize().unwrap();
        let mut app = App::new(root, crate::config::Config::default()).unwrap();
        let (tx, rx) = channel();
        app.attach_follow_loader(tx);
        Some((app, rx, dir))
    }

    fn receive(rx: &Receiver<FollowResult>) -> FollowResult {
        rx.recv_timeout(Duration::from_secs(60))
            .expect("follow worker answers")
    }

    #[test]
    fn jj_follow_pinned_point_and_clean_file_diff_run_on_workers() {
        let Some((mut app, rx, _dir)) = fixture("jj_follow_worker_clean") else {
            return;
        };
        let path = app.tab.root.join("a.txt");
        let (_, reads) = crate::vcs::count_point_read_calls(|| app.toggle_follow());
        assert_eq!(reads, 0, "F must not query jj's point on the UI thread");
        assert!(
            app.follow_baseline.is_none(),
            "capture is pending until the UI applies its result"
        );
        assert!(app.apply_follow(receive(&rx)));
        let point = app.follow_baseline.as_ref().unwrap().head.clone().unwrap();
        assert!(
            app.follow_baseline.as_ref().unwrap().dirty.is_empty(),
            "the file was clean at F"
        );
        let before = crate::vcs::contents_at(&app.tab.root, &point, &path).unwrap();
        std::fs::write(&path, b"after\n").unwrap();
        let (_, reads) = crate::vcs::count_point_read_calls(|| {
            assert!(app.follow_note_change(&path));
            app.follow_jump(&path);
            assert!(app.follow_baseline_contents(&path).is_none());
            assert!(app.compute_gitdiff_lines(&path, true).is_empty());
        });
        assert_eq!(
            reads, 0,
            "a pending clean-file baseline never runs jj file show on the UI"
        );
        let result = receive(&rx);
        assert!(app.apply_follow(result));
        assert_eq!(app.follow_baseline_contents(&path), Some(before.clone()));
        let expected = crate::vcs::diff_contents(std::str::from_utf8(&before).unwrap(), "after\n");
        let tuples = |lines: Vec<crate::git::DiffLine>| {
            lines
                .into_iter()
                .map(|l| (l.kind, l.old_no, l.new_no, l.text))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            tuples(app.compute_gitdiff_lines(&path, true)),
            tuples(expected)
        );
        assert!(
            app.is_git_diff_preview(),
            "the landed result replaces the pending preview"
        );
    }

    #[test]
    fn jj_follow_worker_rejects_results_from_old_roots_or_sessions() {
        for file_result in [false, true] {
            for restart in [false, true] {
                let Some((mut app, rx, _dir)) = fixture("jj_follow_worker_stale") else {
                    return;
                };
                app.toggle_follow();
                let mut held = receive(&rx);
                if file_result {
                    assert!(app.apply_follow(held));
                    let path = app.tab.root.join("a.txt");
                    app.request_follow_path(&path);
                    held = receive(&rx);
                }
                if restart {
                    app.toggle_follow();
                    app.toggle_follow();
                } else {
                    let plain = unique_tmp("jj_follow_worker_other_root");
                    std::fs::create_dir_all(&plain).unwrap();
                    app.jump_to_dir(plain.to_path_buf());
                }
                assert!(
                    !app.apply_follow(held),
                    "stale file={file_result} restart={restart}"
                );
                assert!(app.follow_worker.landed.is_empty());
                if restart {
                    assert!(app.apply_follow(receive(&rx)));
                }
            }
        }
    }

    #[test]
    fn jj_follow_worker_keeps_binary_media_in_the_diff() {
        let Some((mut app, rx, _dir)) = fixture("jj_follow_worker_media") else {
            return;
        };
        app.toggle_follow();
        assert!(app.apply_follow(receive(&rx)));
        let path = app.tab.root.join("new.png");
        crate::test_support::write_solid_png(&path, 4, 4, [30, 90, 120]);
        assert!(app.follow_note_change(&path));
        app.refresh_fs_changed(false, std::slice::from_ref(&path))
            .unwrap();
        app.follow_jump(&path);
        assert!(app.apply_follow(receive(&rx)));
        assert!(
            app.is_git_diff_preview(),
            "an empty binary line diff still opens the media diff"
        );
        assert!(app.diff_media_active());
    }

    #[test]
    fn jj_follow_worker_does_not_interrupt_a_paused_surface() {
        let Some((mut app, rx, _dir)) = fixture("jj_follow_worker_paused") else {
            return;
        };
        app.toggle_follow();
        assert!(app.apply_follow(receive(&rx)));
        let path = app.tab.root.join("a.txt");
        std::fs::write(&path, b"after\n").unwrap();
        assert!(app.follow_note_change(&path));
        app.follow_jump(&path);
        let held = receive(&rx);
        app.open_git_view();
        assert!(app.is_git_view());
        assert!(app.apply_follow(held));
        assert!(
            app.is_git_view(),
            "a late result must not replace a paused list with a diff"
        );
    }
}
