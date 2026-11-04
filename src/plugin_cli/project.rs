//! The Cargo project `konoma plugin` builds: konoma as a library plus the listed plugin crates,
//! and a `main` that runs them. Also the reading of its `Cargo.lock` (the versions `list` shows).

use std::collections::BTreeMap;
use std::path::Path;

use crate::i18n::{tr, tr_format, Lang, Msg};
use anyhow::{Context, Result};

use super::list_file::{expand_tilde_lang, PluginList, Source};

/// The name of the generated package (it is never published).
pub const PACKAGE: &str = "konoma-with-plugins";

/// The crates of the standard build. They are linked in already; installing one by name would only
/// repeat their ids.
pub const STANDARD_CRATES: &[&str] = &[
    "konoma",
    "konoma-plugin",
    "konoma-code",
    "konoma-markdown",
    "konoma-math",
    "konoma-media",
    "konoma-mermaid",
    "konoma-office",
    "konoma-table",
];

/// What the generated project consists of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub cargo_toml: String,
    pub main_rs: String,
}

/// The library name of a crate: its name with `-` written as `_`.
pub fn lib_name(crate_name: &str) -> String {
    crate_name.replace('-', "_")
}

/// Read authors from the resolved direct dependency, never a same-named transitive package.
pub fn metadata_authors_lang(text: &str, name: &str, lang: Lang) -> Result<Vec<String>> {
    let metadata: serde_json::Value =
        serde_json::from_str(text).map_err(|error: serde_json::Error| {
            if lang == Lang::En {
                anyhow::Error::new(error)
            } else {
                anyhow::anyhow!(tr_format(
                    lang,
                    Msg::PluginMetadataJson,
                    &[
                        ("line", &error.line().to_string()),
                        ("column", &error.column().to_string())
                    ]
                ))
            }
        })?;
    let resolved = &metadata["resolve"];
    let root = resolved["root"]
        .as_str()
        .context(tr(lang, Msg::PluginMetadataNoRoot))?;
    let node = resolved["nodes"]
        .as_array()
        .context(tr(lang, Msg::PluginMetadataNoNodes))?
        .iter()
        .find(|n| n["id"].as_str() == Some(root))
        .context(tr(lang, Msg::PluginMetadataRootMissing))?;
    let direct = node["deps"]
        .as_array()
        .context(tr(lang, Msg::PluginMetadataNoDeps))?;
    let package = metadata["packages"]
        .as_array()
        .context(tr(lang, Msg::PluginMetadataNoPackages))?
        .iter()
        .find(|p| p["name"].as_str() == Some(name) && direct.iter().any(|d| d["pkg"] == p["id"]))
        .context(tr(lang, Msg::PluginMetadataNoPackage))?;
    Ok(package["authors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str())
        .map(|a| {
            a.split('<')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .filter(|word| !word.contains('@'))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|a| !a.is_empty())
        .collect())
}

fn dependency(src: &Source, home: Option<&Path>, lang: Lang) -> Result<toml::Value> {
    let mut t = toml::Table::new();
    match src {
        Source::Crates { version } => {
            t.insert("version".into(), version.clone().into());
        }
        Source::Git { url, branch, rev } => {
            t.insert("git".into(), url.clone().into());
            if let Some(b) = branch {
                t.insert("branch".into(), b.clone().into());
            }
            if let Some(r) = rev {
                t.insert("rev".into(), r.clone().into());
            }
        }
        Source::Path { path } => {
            let p = expand_tilde_lang(path, home, lang)?;
            t.insert("path".into(), p.to_string_lossy().into_owned().into());
        }
    }
    Ok(t.into())
}

/// Generate the project for `list`, against konoma `konoma_version` from crates.io (pinned with
/// `=`, so the library is exactly the one the running binary was built from) or against the
/// source `[konoma] path` names.
pub fn generate_lang(
    list: &PluginList,
    konoma_version: &str,
    home: Option<&Path>,
    lang: Lang,
) -> Result<Project> {
    let mut deps = toml::Table::new();
    let mut konoma = toml::Table::new();
    match &list.konoma_path {
        Some(p) => {
            let p = expand_tilde_lang(p, home, lang)?;
            konoma.insert("path".into(), p.to_string_lossy().into_owned().into());
        }
        None => {
            konoma.insert("version".into(), format!("={konoma_version}").into());
        }
    }
    deps.insert("konoma".into(), konoma.into());
    for (name, src) in &list.plugins {
        deps.insert(
            name.clone(),
            dependency(src, home, lang).with_context(|| format!("[plugins.{name}]"))?,
        );
    }

    let mut package = toml::Table::new();
    package.insert("name".into(), PACKAGE.into());
    package.insert("version".into(), "0.0.0".into());
    package.insert("edition".into(), "2021".into());
    package.insert("publish".into(), false.into());
    let mut bin = toml::Table::new();
    bin.insert("name".into(), "konoma".into());
    bin.insert("path".into(), "src/main.rs".into());
    let mut root = toml::Table::new();
    root.insert("package".into(), package.into());
    root.insert("bin".into(), vec![toml::Value::from(bin)].into());
    root.insert("dependencies".into(), deps.into());
    // An empty `[workspace]` makes this project its own workspace, so a build inside some other
    // Cargo workspace (a development tree, a test) does not join it.
    root.insert("workspace".into(), toml::Table::new().into());
    let cargo_toml = format!(
        "# Generated by `konoma plugin`; rebuilt on every install, update and remove.\n{}",
        toml::to_string(&root)
            .with_context(|| tr(lang, Msg::PluginCannotWrite).replace("{path}", "Cargo.toml"))?
    );

    let mut main_rs = String::from(
        "// Generated by `konoma plugin`.\nfn main() {\n    let result = konoma::Host::builder()\n        .standard_plugins()\n",
    );
    for name in list.plugins.keys() {
        main_rs.push_str(&format!(
            "        .plugins({}::plugins())\n",
            lib_name(name)
        ));
    }
    main_rs.push_str(
        "        .run();\n    if let Err(e) = result {\n        eprintln!(\"konoma: {e:#}\");\n        std::process::exit(1);\n    }\n}\n",
    );
    Ok(Project {
        cargo_toml,
        main_rs,
    })
}

/// Resolve the generated package's direct dependencies and konoma's standard plugin edges.
pub fn locked_versions(lock_text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(t) = lock_text.parse::<toml::Table>() else {
        return out;
    };
    let Some(pkgs) = t.get("package").and_then(|p| p.as_array()) else {
        return out;
    };
    let field = |p: &toml::Value, key: &str| p.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let find = |edge: &str| {
        let mut parts = edge.split_whitespace();
        let name = parts.next()?;
        let version = parts.next();
        let source = parts.next().map(|s| s.trim_matches(['(', ')']));
        let mut matches = pkgs.iter().filter(|p| {
            field(p, "name").as_deref() == Some(name)
                && version.is_none_or(|v| field(p, "version").as_deref() == Some(v))
                && source.is_none_or(|s| field(p, "source").as_deref() == Some(s))
        });
        let p = matches.next()?;
        matches.next().is_none().then_some(p)
    };
    let edges = |p: &toml::Value| -> Vec<String> {
        p.get("dependencies")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    if let Some(root) = find(PACKAGE).or_else(|| find("konoma")) {
        let mut selected = edges(root);
        if let Some(konoma) = selected
            .iter()
            .find_map(|e| find(e).filter(|p| field(p, "name").as_deref() == Some("konoma")))
        {
            selected.extend(
                edges(konoma).into_iter().filter(|e| {
                    STANDARD_CRATES.contains(&e.split_whitespace().next().unwrap_or(""))
                }),
            );
        }
        for edge in selected {
            if let Some(p) = find(&edge) {
                if let (Some(n), Some(v)) = (field(p, "name"), field(p, "version")) {
                    out.insert(n, v);
                }
            }
        }
        // The library workspace's own lock has konoma as its root.
        if field(root, "name").as_deref() == Some("konoma") {
            if let Some(v) = field(root, "version") {
                out.insert("konoma".into(), v);
            }
        }
    } else {
        // Older incomplete records can only identify packages with an unambiguous name.
        for p in pkgs {
            if let Some(n) = field(p, "name") {
                if let Some(v) = find(&n).and_then(|p| field(p, "version")) {
                    out.insert(n, v);
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub fn generate(list: &PluginList, version: &str, home: Option<&Path>) -> Result<Project> {
    generate_lang(list, version, home, Lang::En)
}
#[cfg(test)]
pub fn metadata_authors(text: &str, name: &str) -> Result<Vec<String>> {
    metadata_authors_lang(text, name, Lang::En)
}
#[cfg(test)]
mod tests {
    #[test]
    fn metadata_error_language_walk() {
        use crate::i18n::Lang;
        for lang in [Lang::Jp, Lang::En] {
            for (json, expected) in [
                ("[", "EOF while parsing a list at line 1 column 1"),
                ("{}", "metadata has no root"),
                (r#"{"resolve":{"root":"r"}}"#, "metadata has no nodes"),
                (
                    r#"{"resolve":{"root":"r","nodes":[]}}"#,
                    "metadata root is missing",
                ),
                (
                    r#"{"resolve":{"root":"r","nodes":[{"id":"r"}]}}"#,
                    "metadata has no direct dependencies",
                ),
                (
                    r#"{"resolve":{"root":"r","nodes":[{"id":"r","deps":[]}]}}"#,
                    "metadata has no packages",
                ),
                (
                    r#"{"resolve":{"root":"r","nodes":[{"id":"r","deps":[]}]},"packages":[]}"#,
                    "metadata does not contain the requested direct dependency",
                ),
            ] {
                let error = super::metadata_authors_lang(json, "konoma-foo", lang).unwrap_err();
                if lang == Lang::Jp {
                    crate::test_support::plugin_language::assert_japanese_plugin_prose(
                        "metadata",
                        &format!("{error:#}"),
                    );
                } else {
                    crate::test_support::plugin_english_lower::assert_english_context(
                        "metadata",
                        &format!("{error:#}"),
                        expected,
                    );
                    assert_eq!(error.to_string(), expected);
                }
            }
            let list = super::PluginList {
                konoma_path: Some("~/fixture".into()),
                ..Default::default()
            };
            let error = super::generate_lang(&list, "0.31.2", None, lang).unwrap_err();
            if lang == Lang::Jp {
                crate::test_support::plugin_language::assert_japanese_plugin_prose(
                    "project",
                    &format!("{error:#}"),
                );
            } else {
                assert_eq!(
                    error.to_string(),
                    "`~/fixture` starts with ~ but HOME is not set"
                );
                crate::test_support::plugin_english_lower::assert_english_context(
                    "project",
                    &format!("{error:#}"),
                    "`~/fixture` starts with ~ but HOME is not set",
                );
            }
        }
    }
    mod environment {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/environment.rs"
        ));
    }

    #[test]
    fn generated_main_dispatches_svg_before_noisy_plugin_factories() {
        use crate::test_support::command::CommandOutput;
        use std::io::Write;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Duration;
        let env = environment::Environment::new("noisy-generated-main");
        // Reuse the offline workspace build and its exact library artifacts. The linked binary
        // lives beside those artifacts; only its small source and isolated app state use TmpDir.
        let build = Command::new(env!("CARGO"))
            .args([
                "build",
                "--offline",
                "-p",
                "konoma",
                "--lib",
                "--message-format=json",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .supervised_output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let artifact = |name: &str| {
            String::from_utf8_lossy(&build.stdout)
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|message| {
                    message["reason"] == "compiler-artifact" && message["target"]["name"] == name
                })
                .flat_map(|message| message["filenames"].as_array().unwrap().clone())
                .filter_map(|filename| filename.as_str().map(std::path::PathBuf::from))
                .find(|filename| filename.extension().is_some_and(|ext| ext == "rlib"))
                .unwrap()
        };
        let konoma = artifact("konoma");
        let sdk = artifact("konoma_plugin");
        let deps = sdk.parent().unwrap();
        struct Binary(std::path::PathBuf);
        impl Drop for Binary {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let binary = Binary(deps.join(env.root.file_name().unwrap()));
        let mut plugins = super::PluginList::default();
        plugins.plugins.insert(
            "noisy".into(),
            super::Source::Crates {
                version: "0.1".into(),
            },
        );
        let mut source = super::generate(&plugins, env!("CARGO_PKG_VERSION"), None)
            .unwrap()
            .main_rs;
        source.push_str(
            r#"
mod noisy {
    use konoma_plugin::prelude::*;
    struct Noisy;
    impl Plugin for Noisy {
        fn manifest(&self) -> Manifest {
            println!("manifest noise");
            Manifest::new("noisy", "0.1.0")
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported("fixture"))
        }
    }
    pub fn plugins() -> Vec<Box<dyn Plugin>> {
        println!("factory noise");
        vec![Box::new(Noisy)]
    }
}
"#,
        );
        let path = env.root.join("main.rs");
        std::fs::write(&path, source).unwrap();
        let compile = Command::new("rustc")
            .args(["--edition=2021", "--crate-name", "noisy_generated_main"])
            .arg(&path)
            .arg("-o")
            .arg(&binary.0)
            .arg("--extern")
            .arg(format!("konoma={}", konoma.display()))
            .arg("--extern")
            .arg(format!("konoma_plugin={}", sdk.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .supervised_output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let mut control = Command::new(&binary.0);
        env.configure(&mut control);
        let control = control.arg("--plugins").supervised_output().unwrap();
        assert!(control.status.success(), "{control:?}");
        let control = String::from_utf8_lossy(&control.stdout);
        assert!(control.contains("factory noise") && control.contains("manifest noise"));
        let mut command = Command::new(&binary.0);
        env.configure(&mut command);
        let mut child = konoma_plugin::proc::process::ChildGuard::new(
            command
                .arg("--internal-svg-render")
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8"/></svg>"#;
        let mut request = b"KSV2".to_vec();
        request.extend_from_slice(&8u32.to_le_bytes());
        request.extend_from_slice(&0u32.to_le_bytes());
        request.extend_from_slice(&u32::MAX.to_le_bytes());
        request.extend_from_slice(&(svg.len() as u64).to_le_bytes());
        request.extend_from_slice(svg);
        child.stdin.take().unwrap().write_all(&request).unwrap();
        let output = konoma_plugin::proc::process::collect_output(
            &mut child,
            || false,
            Some(Duration::from_secs(30)),
            1 << 20,
            Duration::ZERO,
        )
        .unwrap()
        .output;
        assert!(output.status.success(), "{output:?}");
        assert!(
            output.stdout.starts_with(b"KSRDYKSR1\0"),
            "plugin output corrupted the drawing protocol: {:?}",
            output.stdout
        );
        assert_eq!(output.stdout.len(), 5 + 13 + 8 * 8 * 4);
    }
    use super::*;

    fn list(toml: &str) -> PluginList {
        PluginList::parse(toml).unwrap()
    }

    fn parsed(p: &Project) -> toml::Table {
        p.cargo_toml
            .parse()
            .expect("the generated Cargo.toml parses")
    }

    #[test]
    fn standard_build_with_no_plugins() {
        let p = generate(&PluginList::default(), "0.31.2", None).unwrap();
        let t = parsed(&p);
        assert_eq!(t["package"]["name"].as_str(), Some(PACKAGE));
        assert_eq!(t["package"]["publish"].as_bool(), Some(false));
        assert_eq!(t["bin"][0]["name"].as_str(), Some("konoma"));
        assert_eq!(
            t["dependencies"]["konoma"]["version"].as_str(),
            Some("=0.31.2")
        );
        assert_eq!(t["dependencies"].as_table().unwrap().len(), 1);
        assert!(t.contains_key("workspace"));
        assert!(!p.main_rs.contains(".plugins("));
        assert!(p.main_rs.contains(".standard_plugins()"));
        assert!(p.main_rs.contains(".run()"));
    }

    #[test]
    fn each_source_becomes_a_dependency_and_a_plugins_call() {
        let l = list(
            r#"
[plugins.konoma-foo]
version = "0.3"
[plugins.konoma-bar]
git = "https://example.org/bar"
branch = "main"
[plugins.konoma-baz]
path = "~/baz"
[plugins.konoma-qux]
git = "https://example.org/qux"
rev = "abc"
"#,
        );
        let p = generate(&l, "1.2.3", Some(Path::new("/home/u"))).unwrap();
        let d = &parsed(&p)["dependencies"];
        assert_eq!(d["konoma-foo"]["version"].as_str(), Some("0.3"));
        assert_eq!(
            d["konoma-bar"]["git"].as_str(),
            Some("https://example.org/bar")
        );
        assert_eq!(d["konoma-bar"]["branch"].as_str(), Some("main"));
        assert_eq!(d["konoma-baz"]["path"].as_str(), Some("/home/u/baz"));
        assert_eq!(d["konoma-qux"]["rev"].as_str(), Some("abc"));
        for n in ["konoma_foo", "konoma_bar", "konoma_baz", "konoma_qux"] {
            assert!(
                p.main_rs.contains(&format!(".plugins({n}::plugins())")),
                "{n}: {}",
                p.main_rs
            );
        }
        assert_eq!(p.main_rs.matches(".plugins(").count(), 4);
        let body = p.main_rs.split("fn main() {").nth(1).unwrap();
        assert!(body
            .trim_start()
            .starts_with("let result = konoma::Host::builder()"));
    }

    #[test]
    fn konoma_path_replaces_the_pinned_version() {
        let l = list("[konoma]\npath = \"/src/konoma\"\n");
        let t = parsed(&generate(&l, "0.31.2", None).unwrap());
        assert_eq!(
            t["dependencies"]["konoma"]["path"].as_str(),
            Some("/src/konoma")
        );
        assert!(t["dependencies"]["konoma"].get("version").is_none());
    }

    #[test]
    fn a_tilde_without_home_is_an_error_naming_the_plugin() {
        let l = list("[plugins.konoma-baz]\npath = \"~/baz\"\n");
        let e = format!("{:#}", generate(&l, "1.0.0", None).unwrap_err());
        assert!(e.contains("konoma-baz") && e.contains("HOME"), "{e}");
        let l = list("[konoma]\npath = \"~/k\"\n");
        assert!(generate(&l, "1.0.0", None).is_err());
    }

    #[test]
    fn values_with_quotes_cannot_break_out_of_the_toml() {
        let mut l = PluginList::default();
        l.plugins.insert(
            "konoma-x".into(),
            Source::Git {
                url: "u\"\n[evil]\nx = \"1".into(),
                branch: None,
                rev: None,
            },
        );
        let t = parsed(&generate(&l, "1.0.0", None).unwrap());
        assert!(!t.contains_key("evil"));
        assert!(t["dependencies"]["konoma-x"]["git"]
            .as_str()
            .unwrap()
            .contains("[evil]"));
    }

    #[test]
    fn the_main_reports_a_failed_run_and_exits_nonzero() {
        let p = generate(&PluginList::default(), "1.0.0", None).unwrap();
        assert!(p.main_rs.contains("eprintln!(\"konoma: {e:#}\")"));
        assert!(p.main_rs.contains("std::process::exit(1)"));
    }

    #[test]
    fn lib_names_replace_dashes() {
        assert_eq!(lib_name("konoma-foo-bar"), "konoma_foo_bar");
        assert_eq!(lib_name("plain"), "plain");
    }

    #[test]
    fn metadata_authors_follow_the_direct_package_identity() {
        let metadata = serde_json::json!({
            "packages": [
                {"id": "direct", "name": "foo", "authors": ["Fixture author"]},
                {"id": "transitive", "name": "foo", "authors": ["Another author"]}
            ],
            "resolve": {"root": "root", "nodes": [{"id": "root", "deps": [{"pkg": "direct"}]}]}
        });
        assert_eq!(
            metadata_authors(&metadata.to_string(), "foo").unwrap(),
            ["Fixture author"]
        );
        assert!(metadata_authors(&metadata.to_string(), "absent").is_err());
    }

    #[test]
    fn locked_versions_follow_direct_edges_instead_of_the_last_package() {
        let lock = r#"
[[package]]
name = "konoma-with-plugins"
version = "0.0.0"
dependencies = ["konoma", "foo 0.3.0"]
[[package]]
name = "konoma"
version = "0.31.2"
dependencies = ["konoma-office 0.1.0"]
[[package]]
name = "foo"
version = "0.3.0"
[[package]]
name = "foo"
version = "0.9.0"
[[package]]
name = "konoma-office"
version = "0.1.0"
[[package]]
name = "konoma-office"
version = "0.2.0"
"#;
        let versions = locked_versions(lock);
        assert_eq!(versions["foo"], "0.3.0");
        assert_eq!(versions["konoma-office"], "0.1.0");
    }

    #[test]
    fn locked_versions_reads_the_packages() {
        let lock = "version = 4\n\n[[package]]\nname = \"konoma-foo\"\nversion = \"0.3.2\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n";
        let v = locked_versions(lock);
        assert_eq!(v["konoma-foo"], "0.3.2");
        assert_eq!(v["serde"], "1.0.0");
        assert!(locked_versions("not = = toml").is_empty());
        assert!(locked_versions("").is_empty());
    }

    #[test]
    fn the_standard_crates_include_every_crate_with_a_program() {
        for c in [
            "konoma-code",
            "konoma-markdown",
            "konoma-math",
            "konoma-media",
            "konoma-mermaid",
            "konoma-office",
            "konoma-table",
        ] {
            assert!(STANDARD_CRATES.contains(&c), "{c}");
        }
    }
}
