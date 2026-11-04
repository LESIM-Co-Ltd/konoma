// Golden prose copied from 7359773 (pre-fix-9): src/plugin_cli/{mod,build,list_file,watcher}.rs,
// and the already catalogued path/metadata contexts in src/i18n.rs. Never read the live catalog.
const PERMISSIONS: &str = "A plugin runs with your permissions, and its build scripts run on this machine while konoma is rebuilt.";

pub fn assert_original_english_sample(label: &str, text: &str) {
    assert!(
        !text.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c)),
        "Japanese in English sample {label}: {text}"
    );
    // The additional standard rows and disabled status are new in fix-12. Pin them separately,
    // then retain all pre-fix-9 golden assertions for the original extra-plugin output.
    let mut normalized = text.to_owned();
    if label == "[\"plugin\", \"list\"]" || matches!(label, "populated list" | "list update time") {
        let version = if label == "[\"plugin\", \"list\"]" {
            "0.1.0"
        } else {
            "not built yet"
        };
        for name in [
            "konoma-code",
            "konoma-markdown",
            "konoma-math",
            "konoma-media",
            "konoma-mermaid",
            "konoma-office",
            "konoma-table",
        ] {
            let block = format!("{name}  {version}\n    crates.io, version ^0.1.0\n");
            assert!(
                normalized.contains(&block),
                "missing standard row in {label}: {normalized}"
            );
            normalized = normalized.replacen(&block, "", 1);
        }
        assert!(
            normalized.ends_with("Plugin update checking is turned off.\n"),
            "{normalized}"
        );
        normalized.truncate(normalized.len() - "Plugin update checking is turned off.\n".len());
    }
    let text = normalized.as_str();
    let confirmation =
        format!("Plugin crate:  konoma-foo\nSource:        crates.io, version *\n{PERMISSIONS}\n");
    let list = "konoma-foo  0.3.2\n    crates.io, version *\n";
    let args = serde_json::from_str::<Vec<String>>(label).ok();
    let args = args
        .as_ref()
        .map(|args| args.iter().map(String::as_str).collect::<Vec<_>>());
    let expected: Option<String> = if let Some(args) = args {
        match args.as_slice() {
            ["plugin", "list"] => {
                assert_eq!(
                    text,
                    "No plugin crates are installed: konoma is the standard build.\n"
                );
                None
            }
            ["plugin", "clean"] => {
                assert_eq!(text, "No build cache to remove.\n");
                None
            }
            ["plugin", "install", "konoma-foo"] => {
                assert!(text.starts_with(&confirmation), "{text}");
                Some("cannot ask for confirmation: stdin is not a terminal (pass --yes to install anyway)".into())
            }
            ["plugin", "install", name @ ("konoma-foo" | "konoma-bar"), "-y"] => {
                let confirm = confirmation.replace("konoma-foo", name);
                assert!(text.contains(&confirm), "{text}");
                None
            }
            ["plugin", "install", "konoma-office", "-y"] => Some(
                "`konoma-office` is part of the standard build already; nothing to install".into(),
            ),
            ["plugin", "update" | "remove", "absent"] => {
                Some("`absent` is not installed (see `konoma plugin list`)".into())
            }
            ["plugin"] => Some("a command is needed".into()),
            ["plugin", "unknown"] => Some("unknown command `unknown`".into()),
            ["plugin", "list", "extra"] => Some("list takes no arguments".into()),
            ["plugin", "remove"] => Some("remove needs a crate name".into()),
            ["plugin", "remove", "bad name"] => Some("`bad name` is not a crate name".into()),
            ["plugin", "remove", "a", "b"] => Some("remove takes one crate name".into()),
            ["plugin", "update", "a", "b"] => Some("update takes at most one crate name".into()),
            ["plugin", "install"] => Some("install needs a crate name".into()),
            ["plugin", "install", "a", "b"] => Some("install takes one crate".into()),
            ["plugin", "install", "a@"] => Some("a version is needed after @".into()),
            ["plugin", "install", "a", "--bogus"] => Some("unknown option `--bogus`".into()),
            ["plugin", "install", "a", "--git"] => Some("--git needs a value".into()),
            ["plugin", "install", "a", "--git", "url", "--path", "/p"] => {
                Some("--git and --path are alternatives".into())
            }
            ["plugin", "install", "a@1", "--path", "/p"] => {
                Some("@version goes with crates.io; with --git or --path leave it out".into())
            }
            ["plugin", "install", "a", "--branch", "b"] => {
                Some("--branch and --rev go with --git".into())
            }
            ["plugin", "install", "a", "--git", "url", "--branch", "b", "--rev", "r"] => {
                Some("use --branch or --rev, not both".into())
            }
            ["plugin", "install", "a", "--path", "/nonexistent-konoma-fixture", "-y"] => {
                Some("Cannot resolve plugin path `/nonexistent-konoma-fixture`".into())
            }
            _ => panic!("missing pre-fix-9 golden for {label}"),
        }
    } else {
        match label {
            "question and cancellation" => { assert_eq!(text, format!("Authors:       Fixture author\n{confirmation}Install it? [y/N] Cancelled.\n")); None }
            "populated list" => { assert_eq!(text, list); None }
            "list update time" => { assert_eq!(text, format!("{list}Last checked: 1970-01-01 00:00:00 UTC\n")); None }
            "clean removal" => { assert_eq!(text, "Removed <ROOT>/xdg-data/konoma/build\n"); None }
            "last removal" => { assert_eq!(text, "Removed `konoma-foo`. No plugin crates are left; konoma is the standard build again.\n"); None }
            "cargo missing" => Some("cargo was not found (set $CARGO, or put cargo in PATH).".into()),
            "metadata failure" => Some("Could not read plugin metadata".into()),
            // These native parser/UTF-8 diagnostics were passed through by the pre-fix-9 code.
            "metadata JSON" => Some("EOF while parsing a list at line 1 column 1".into()),
            "metadata UTF-8" => Some("invalid utf-8 sequence of 1 bytes from index 0".into()),
            "build failure" => Some("`cargo build --release` failed (exit status: 101); the previous konoma is unchanged".into()),
            "build signal" => Some("`cargo build --release` failed (signal: 15 (SIGTERM)); the previous konoma is unchanged".into()),
            "missing build output" => Some("the build produced no program at <ROOT>/xdg-data/konoma/build-<PID>/target/release/konoma".into()),
            "verification failure" => Some("the new build refused to start; nothing was replaced:".into()),
            "watcher failure" => Some("plugin job watcher: cannot start watched program <ROOT>/fake/cargo".into()),
            "lock" => Some("cannot open <ROOT>/xdg-data/konoma/plugin.lock".into()),
            "saved lock" => Some("Cannot restore saved Cargo.lock; Nothing was replaced".into()),
            "record replacement" => Some("cannot replace <ROOT>/xdg-data/konoma/bin/konoma.built.toml".into()),
            "program replacement" => Some("cannot replace <ROOT>/xdg-data/konoma/bin/konoma".into()),
            "list read" => Some("cannot read <ROOT>/xdg-config/konoma/plugins.toml".into()),
            "last removal list failure" => Some("the rebuilt konoma is removed (konoma is the standard build again), but plugins.toml could not be updated and still lists `konoma-foo`.".into()),
            toml => {
                let reason = match toml {
                    "[" => "not valid TOML",
                    "unknown = 1" => "unknown table `unknown` (allowed: [plugins.<crate>], [konoma])",
                    "konoma = 1" => "[konoma] must be a table",
                    "plugins = 1" => "[plugins] must be a table of [plugins.<crate>]",
                    "[konoma]\npath = 'relative'" => "[konoma]: `path = \"relative\"` must be an absolute path or start with ~",
                    "[konoma]\nbad = 1" => "[konoma]: unknown key `bad` (allowed: path)",
                    "[konoma]\npath = ''" => "[konoma]: `path` is empty",
                    "[konoma]\npath = 1" => "[konoma]: `path` must be a string",
                    "[plugins]\n'bad name' = {}" => "[plugins.bad name]: `bad name` is not a crate name",
                    "[plugins]\na = 1" => "[plugins.a] must be a table",
                    "[plugins.a]" => "[plugins.a]: write one of `version`, `git` or `path`",
                    "[plugins.a]\nversion = '1'\npath = '/p'" => "[plugins.a]: `version`, `git` and `path` are alternatives; write only one",
                    "[plugins.a]\npath = '/p'\nbranch = 'b'" => "[plugins.a]: `branch` and `rev` go with `git`",
                    "[plugins.a]\ngit = 'url'\nbranch = 'b'\nrev = 'r'" => "[plugins.a]: write `branch` or `rev`, not both",
                    _ => panic!("missing pre-fix-9 error golden for {label}"),
                };
                Some(format!("<ROOT>/xdg-config/konoma/plugins.toml is broken: {reason}"))
            }
        }
    };
    let error = text
        .lines()
        .find_map(|line| line.strip_prefix("konoma plugin: "));
    match (expected, error) {
        (Some(expected), Some(actual)) => {
            assert!(
                actual.starts_with(&expected),
                "changed first error sentence in {label}: expected {expected:?}, got {actual:?}"
            );
            let suffix = &actual[expected.len()..];
            assert!(
                suffix.is_empty() || suffix.starts_with(": "),
                "changed first error sentence in {label}: {actual}"
            );
        }
        (None, None) => {}
        (expected, actual) => {
            panic!("wrong error presence in {label}: expected {expected:?}, got {actual:?}")
        }
    }
}
