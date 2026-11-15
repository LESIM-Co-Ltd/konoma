//! Distinct committed, staged and jj baselines must survive every diff presentation.
use super::*;
use konoma_plugin::host::project::md_diff::{
    DiffBaseline, MdDiffKind, MdDiffOutcome, MdDiffRequest, MdDiffWorker,
};
use konoma_plugin::host::project::media_diff::{
    MediaBase, MediaDiffComputed, MediaDiffRequest, MediaDiffSideDecoded, MediaDiffWorker,
};
use konoma_plugin::host::project::DiffReadContext;
use konoma_vcs_core::vcs::{Preference, Vcs};
use std::path::Path;
use std::sync::{mpsc, Arc};

pub(super) const GIT_MD: &str = "# Git HEAD\n";
pub(super) const JJ_MD: &str = "# Jj parent\n";
const CURRENT_MD: &str = "# Jj current\n";
const INDEX_MD: &str = "# Git index\n";
const WORK_MD: &str = "# Working file\n";
pub(super) const GIT_SVG: &str =
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Git HEAD --></svg>";
pub(super) const JJ_SVG: &str =
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Jj parent --></svg>";
const CURRENT_SVG: &str =
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Jj current --></svg>";
const INDEX_SVG: &str =
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Git index --></svg>";
const WORK_SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"><!-- Working file --></svg>";

fn write_pair(root: &Path, md: &str, svg: &str) {
    std::fs::write(root.join("a.md"), md).unwrap();
    std::fs::write(root.join("b.svg"), svg).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn raster(
    _: &[u8],
    _: &Path,
    _: u32,
    _: &dyn Fn() -> bool,
) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
    Ok(image::DynamicImage::new_rgba8(1, 1))
}

fn markdown_old(outcome: &MdDiffOutcome) -> &str {
    let MdDiffOutcome::Ready { old_pre, .. } = outcome else {
        panic!("expected Markdown baseline: {outcome:?}")
    };
    old_pre
}

#[test]
fn selected_backend_drives_raw_rendered_preview_and_media() {
    if !crate::vcs::jj::available() {
        return;
    }
    const CHILD: &str = "KONOMA_SELECTED_BACKEND_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = sandbox("selected_backend_home");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "e2e_tests::selected_backend::selected_backend_drives_raw_rendered_preview_and_media", "--nocapture"])
            .env(CHILD, "1")
            .env("HOME", home.as_path())
            .env("XDG_CONFIG_HOME", home.as_path())
            .env("JJ_CONFIG", "")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let dir = selected_backend_repo();
    assert_eq!(
        konoma_vcs_core::vcs::Jj
            .contents_at(&dir, "@", &dir.join("a.md"))
            .unwrap(),
        CURRENT_MD.as_bytes()
    );
    assert_eq!(
        konoma_vcs_core::vcs::Jj
            .contents_at(&dir, "@", &dir.join("b.svg"))
            .unwrap(),
        CURRENT_SVG.as_bytes()
    );

    let mut checked = 0;
    for (id, backend, md, svg, base) in [
        ("git", Preference::Git, GIT_MD, GIT_SVG, MediaBase::Head),
        ("jj", Preference::Jj, JJ_MD, JJ_SVG, MediaBase::JjParent),
    ] {
        let context = DiffReadContext::new(backend);
        // Pin the ordinary rule to the other backend. Explicit requests must win even on
        // synchronous fallbacks, and must not alter the caller's detection rule.
        let opposite = if id == "git" {
            crate::vcs::Preference::Jj
        } else {
            crate::vcs::Preference::Git
        };
        crate::vcs::set_preference_for_test(Some(opposite));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::vcs::set_preference_for_test(None);
            }
        }
        let _reset = Reset;

        let cfg = vcs_changes::plugin_config(id, &format!("selected_diff_backend_{id}"), "en");
        let mut sim = vcs_changes::project_sim(&dir, id, cfg, 100, 20);
        sim = sim.with_async_md_diff();
        sim.enter();
        vcs_changes::drain(&mut sim);
        assert_eq!(sim.app.tab.preview_path, Some(dir.join("a.md")));
        let raw = sim.app.git_diff_lines();
        assert!(
            raw.iter().any(|l| l.text == md.trim_end()),
            "{id}: raw selected baseline: {raw:?}"
        );
        assert!(
            !raw.iter().any(|l| l.text == INDEX_MD.trim_end()),
            "raw must not read index"
        );
        for kind in [MdDiffKind::Rendered, MdDiffKind::Gutter] {
            sim.key('R');
            let result = sim
                .md_diff_rx
                .as_ref()
                .unwrap()
                .recv_timeout(DRAIN_WAIT)
                .unwrap();
            assert_eq!(result.kind, kind);
            assert_eq!(
                markdown_old(&result.outcome),
                md,
                "{id}: host {kind:?} selected baseline"
            );
            assert!(sim.app.apply_md_diff(result));
            sim.draw();
            checked += 1;
        }

        // Retargeting a project diff carries the choice into the host's media dispatcher.
        sim.key('R');
        sim.ctrl('n');
        assert_eq!(sim.app.tab.preview_path, Some(dir.join("b.svg")));
        let (tx, rx) = mpsc::channel();
        sim.app.attach_media_diff_loader(tx);
        sim.app.poll_media_diff(&dir.join("b.svg"), 1, (10, 10));
        let result = rx.recv_timeout(DRAIN_WAIT).unwrap();
        let MediaDiffComputed::Ready {
            base: actual_base,
            old: MediaDiffSideDecoded::Picture(old),
            ..
        } = result.computed
        else {
            panic!("expected host media baseline")
        };
        assert_eq!(actual_base, base, "{id}: host media base label");
        assert_eq!(
            old.svg.as_deref().unwrap().as_slice(),
            svg.as_bytes(),
            "{id}: host media old bytes"
        );
        checked += 1;

        // The shared workers also serve standalone and project-host-only callers.
        for asynchronous in [false, true] {
            let mut worker = MdDiffWorker::default().with_read_context(context);
            let (tx, rx) = mpsc::channel();
            if asynchronous {
                worker.attach(tx);
            }
            let result = worker
                .kick(MdDiffRequest {
                    gen: 0,
                    path: dir.join("a.md"),
                    root: dir.to_path_buf(),
                    kind: MdDiffKind::Rendered,
                    baseline: DiffBaseline::Vcs,
                    md_frontmatter: false,
                    md_footnotes: false,
                    md_inline_html: false,
                })
                .unwrap_or_else(|| rx.recv_timeout(DRAIN_WAIT).unwrap());
            assert_eq!(
                markdown_old(&result.outcome),
                md,
                "{id}: worker selected baseline"
            );
            let mut media = MediaDiffWorker::default().with_read_context(context);
            let (tx, rx) = mpsc::channel();
            if asynchronous {
                media.attach(tx);
            }
            let request = |baseline| MediaDiffRequest {
                gen: 0,
                path: dir.join("b.svg"),
                root: dir.to_path_buf(),
                baseline,
                page: 1,
                raster_px: (10, 10),
                classify: Arc::new(|_, _| konoma_host_core::project::DiffTarget::Svg),
                rasterize_svg: raster,
                cancel: Arc::default(),
            };
            let result = media
                .dispatch(request(DiffBaseline::Vcs))
                .unwrap_or_else(|| rx.recv_timeout(DRAIN_WAIT).unwrap());
            let MediaDiffComputed::Ready {
                base: actual_base,
                old: MediaDiffSideDecoded::Picture(old),
                ..
            } = result.computed
            else {
                panic!("expected decoded media")
            };
            assert_eq!(actual_base, base, "{id}: media base label");
            assert_eq!(
                old.svg.as_deref().unwrap().as_slice(),
                svg.as_bytes(),
                "{id}: media old bytes"
            );
            checked += 2;

            // Context chooses only Vcs. Follow's missing-head/empty fallback keeps detection.
            let expected = if id == "git" { JJ_SVG } else { GIT_SVG };
            for baseline in [
                DiffBaseline::Empty,
                DiffBaseline::FollowHead {
                    sha: "missing-point".into(),
                },
                DiffBaseline::FollowSnapshot(WORK_SVG.as_bytes().to_vec()),
            ] {
                let mut fallback = MediaDiffWorker::default().with_read_context(context);
                let result = fallback.dispatch(request(baseline.clone())).unwrap();
                let MediaDiffComputed::Ready {
                    base,
                    old: MediaDiffSideDecoded::Picture(old),
                    ..
                } = result.computed
                else {
                    panic!("expected fallback media")
                };
                let expected = if matches!(baseline, DiffBaseline::FollowSnapshot(_)) {
                    assert_eq!(base, MediaBase::FollowStart);
                    WORK_SVG
                } else {
                    expected
                };
                assert_eq!(
                    old.svg.as_deref().unwrap().as_slice(),
                    expected.as_bytes(),
                    "follow fallback keeps detection"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 26,
        "both backends and every presentation must run: {checked}"
    );
}

pub(super) fn selected_backend_repo() -> crate::test_support::TmpDir {
    selected_backend_repo_variant(false, false)
}

pub(super) fn selected_backend_repo_with_code() -> crate::test_support::TmpDir {
    selected_backend_repo_variant(true, false)
}

pub(super) fn selected_backend_repo_with_changed_sets() -> crate::test_support::TmpDir {
    selected_backend_repo_variant(false, true)
}

fn selected_backend_repo_variant(code: bool, changed_sets: bool) -> crate::test_support::TmpDir {
    let dir = sandbox("selected_diff_backend").into_canonical().unwrap();
    let write = |md, svg| {
        write_pair(&dir, md, svg);
        if changed_sets {
            std::fs::write(
                dir.join("c-git-only.md"),
                if md == GIT_MD { GIT_MD } else { WORK_MD },
            )
            .unwrap();
            std::fs::write(
                dir.join("d-jj-only.md"),
                if md == JJ_MD { JJ_MD } else { WORK_MD },
            )
            .unwrap();
            std::fs::write(
                dir.join("e-git-only.svg"),
                if md == GIT_MD { GIT_SVG } else { WORK_SVG },
            )
            .unwrap();
        }
        if code {
            let value = match md {
                GIT_MD => "// Git HEAD\nlet stable = 1;\n// Working file\n",
                JJ_MD => "// Working file\nlet stable = 1;\n// Jj parent\n",
                WORK_MD => "// Working file\nlet stable = 1;\n// Working file\n",
                _ => "// Current/index\nlet stable = 1;\n// Current/index\n",
            };
            std::fs::write(dir.join("c.rs"), value).unwrap();
            std::fs::write(dir.join("d.txt"), value).unwrap();
        }
    };
    init_git_repo(&dir);
    write(GIT_MD, GIT_SVG);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-qm", "seed"]);
    assert!(crate::test_support::run_jj(
        &dir,
        &["git", "init", "--colocate", "."]
    ));
    write(JJ_MD, JJ_SVG);
    assert!(crate::test_support::run_jj(
        &dir,
        &["commit", "-m", "parent"]
    ));
    write(CURRENT_MD, CURRENT_SVG);
    assert!(crate::test_support::run_jj(
        &dir,
        &["describe", "-m", "current"]
    ));
    write(GIT_MD, GIT_SVG);
    git(
        &dir,
        if changed_sets {
            &[
                "add",
                "a.md",
                "b.svg",
                "c-git-only.md",
                "d-jj-only.md",
                "e-git-only.svg",
            ]
        } else if code {
            &["add", "a.md", "b.svg", "c.rs", "d.txt"]
        } else {
            &["add", "a.md", "b.svg"]
        },
    );
    git(&dir, &["commit", "-qm", "git head"]);
    write(INDEX_MD, INDEX_SVG);
    git(
        &dir,
        if changed_sets {
            &[
                "add",
                "a.md",
                "b.svg",
                "c-git-only.md",
                "d-jj-only.md",
                "e-git-only.svg",
            ]
        } else if code {
            &["add", "a.md", "b.svg", "c.rs", "d.txt"]
        } else {
            &["add", "a.md", "b.svg"]
        },
    );
    write(WORK_MD, WORK_SVG);
    dir
}
