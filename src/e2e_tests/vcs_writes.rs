//! Real repository writes through handle_key, through the project host.

use std::path::Path;

pub(super) fn git_output(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

pub(super) fn seed(root: &Path, changed: bool) {
    std::fs::create_dir_all(root).unwrap();
    git_output(
        root,
        &["-c", "init.defaultBranch=main", "init", "-q", "--template="],
    );
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
        ("core.quotePath", "false"),
    ] {
        git_output(root, &["config", "--local", key, value]);
    }
    std::fs::create_dir_all(root.join("sub")).unwrap();
    for file in ["a-modified", "c-deleted", "d-old", "sub/日本語.txt"] {
        // Distinct contents keep rename detection from pairing an unrelated deleted file.
        std::fs::write(root.join(file), format!("base {file}\n")).unwrap();
    }
    git_output(root, &["add", "."]);
    git_output(root, &["commit", "-qm", "seed"]);
    if changed {
        std::fs::write(root.join("a-modified"), "changed\n").unwrap();
        std::fs::write(root.join("b-new"), "new\n").unwrap();
        std::fs::remove_file(root.join("c-deleted")).unwrap();
        git_output(root, &["mv", "d-old", "d-renamed"]);
        std::fs::write(root.join("sub/日本語.txt"), "変更\n").unwrap();
    }
}

pub(super) fn seed_jj(root: &Path, colocated: bool) {
    if colocated {
        seed(root, false);
    } else {
        std::fs::create_dir_all(root).unwrap();
    }
    assert!(crate::test_support::run_jj(
        root,
        &[
            "git",
            "init",
            if colocated {
                "--colocate"
            } else {
                "--no-colocate"
            },
            "."
        ]
    ));
    std::fs::write(root.join("a-modified"), "base\n").unwrap();
    assert!(crate::test_support::run_jj(
        root,
        &["commit", "-m", "jj seed"]
    ));
    std::fs::write(root.join("a-modified"), "changed\n").unwrap();
}

fn jj_output(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("jj")
        .current_dir(root)
        .env("HOME", root)
        .env("JJ_CONFIG", "/dev/null")
        .env("JJ_USER", "Fixture")
        .env("JJ_EMAIL", "fixture@example.invalid")
        .args(["--ignore-working-copy", "--color=never"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "jj {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .replace(root.to_str().unwrap(), "<repo>")
}

pub(super) fn jj_state(root: &Path) -> Vec<String> {
    vec![
        jj_output(root, &["file", "show", "-r", "@", "a-modified"]),
        jj_output(
            root,
            &[
                "log",
                "--no-graph",
                "-r",
                "all()",
                "-T",
                "description ++ \"\\n\"",
            ],
        ),
        jj_output(
            root,
            &["op", "log", "--no-graph", "-T", "description ++ \"\\n\""],
        )
        .lines()
        .map(|line| {
            // Separate fixtures have different commit IDs; retain the operation and its order.
            match line.strip_prefix("commit ") {
                Some(id) if id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    "commit <revision>"
                }
                _ => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n"),
    ]
}
