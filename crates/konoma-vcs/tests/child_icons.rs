//! The `[ui] icons` setting reaches the plugin over the child-process wire: the real `konoma-vcs`
//! program is started as a child (`--plugin git --serve`), and the changes title it answers names
//! the branch with the glyph when icons are on and with `br:` when they are off.

use konoma_plugin::proc::client::{CommandLaunch, Config, Limits, ProcPlugin};
use konoma_plugin::view::ListView;
use konoma_plugin::{Input, OpenCx, Plugin, ProjectView, View, ViewCx, ViewReq};
use std::path::Path;
use std::time::Duration;

const EXE: &str = env!("CARGO_BIN_EXE_konoma-vcs");

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn changes_title(plugin: &ProcPlugin, root: &Path, icons: bool) -> String {
    let mut session = plugin
        .open(
            Input::Project {
                root: root.to_path_buf(),
                marker: ".git".into(),
            },
            &OpenCx::bare(),
        )
        .expect("the child opens the project");
    let View::List(ListView { heading, .. }) = session
        .view(
            &ViewReq::new(80, 20).project(ProjectView::new("changes").icons(icons)),
            &ViewCx::bare(),
        )
        .expect("the child answers the changes view")
    else {
        panic!("expected a list")
    };
    let heading = heading.expect("the changes list has a heading");
    format!("{}{}", heading.before.plain(), heading.after.plain())
}

#[test]
fn the_icons_setting_crosses_the_child_process_wire_into_the_changes_title() {
    if !cfg!(feature = "git") {
        return;
    }
    let dir = konoma_test_support::unique_tmp("vcs_child_icons");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "base\n").unwrap();
    git(&dir, &["add", "."]);
    git(
        &dir,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "seed",
        ],
    );
    std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
    let root = dir.canonicalize().unwrap();
    let scratch = konoma_test_support::unique_tmp("vcs_child_icons_scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let cfg = Config {
        limits: Limits {
            open_view: Duration::from_secs(20),
            start_timeout: Duration::from_secs(20),
            shutdown_grace: Duration::from_secs(10),
            rss_limit: u64::MAX,
            ..Limits::default()
        },
        scratch_root: scratch.canonicalize().unwrap(),
        host_version: "0.0.0-test".into(),
        lang: "en".into(),
    };
    let launch = CommandLaunch::new(EXE, ["--plugin", "git", "--serve"]).with_work_dir(&root);
    let plugin = ProcPlugin::start(launch, cfg).expect("the child starts");
    assert_eq!(plugin.manifest().id, "git");
    let on = changes_title(&plugin, &root, true);
    let off = changes_title(&plugin, &root, false);
    plugin.stop();
    assert!(
        on.contains("Git \u{2387} main"),
        "icons on name the branch with the glyph: {on:?}"
    );
    assert!(
        off.contains("Git br: main"),
        "icons off name the branch with `br:`: {off:?}"
    );
    assert!(
        !on.contains("br:") && !off.contains('\u{2387}'),
        "{on:?} {off:?}"
    );
}
