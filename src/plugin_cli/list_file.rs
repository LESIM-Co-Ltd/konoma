//! `plugins.toml`: the list of plugin crates `konoma plugin` builds into konoma.
//!
//! The shape follows Cargo's dependencies:
//!
//! ```toml
//! [plugins.konoma-foo]
//! version = "0.3"                 # crates.io (the default source)
//! [plugins.konoma-bar]
//! git = "https://example.org/bar" # optionally branch / rev
//! [plugins.konoma-baz]
//! path = "/abs/path"              # an absolute path or one starting with ~
//! [konoma]
//! path = "/abs/konoma"            # build against this konoma source instead of crates.io
//! ```
//!
//! A file that cannot be read or does not parse is an error and is never rewritten. A write goes to
//! a temporary file next to it and is renamed over it, so a reader never sees half a file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use toml_edit::{value, DocumentMut, Item, Table, TableLike};

/// Where one plugin crate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// crates.io, with a semver requirement (`*` when none was given).
    Crates { version: String },
    Git {
        url: String,
        branch: Option<String>,
        rev: Option<String>,
    },
    /// An absolute path or one starting with `~`.
    Path { path: String },
}

impl Source {
    /// One line for the confirmation and `list`: where the code comes from.
    pub fn describe(&self) -> String {
        match self {
            Source::Crates { version } => format!("crates.io, version {version}"),
            Source::Git { url, branch, rev } => {
                let mut s = format!("git {url}");
                if let Some(b) = branch {
                    s.push_str(&format!(", branch {b}"));
                }
                if let Some(r) = rev {
                    s.push_str(&format!(", rev {r}"));
                }
                s
            }
            Source::Path { path } => format!("path {path}"),
        }
    }

    /// Whether a later unit can ask crates.io whether a newer version exists. Git and path sources
    /// have no such thing.
    pub fn has_update_check(&self) -> bool {
        matches!(self, Source::Crates { .. })
    }
}

/// The parsed `plugins.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginList {
    /// `[konoma] path`: the konoma source to build against (development and tests).
    pub konoma_path: Option<String>,
    pub plugins: BTreeMap<String, Source>,
}

/// A crate name: what crates.io accepts (ASCII letters, digits, `-`, `_`; starts with a letter).
pub fn valid_crate_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && name.len() <= 64
}

/// `~` or `~/...` or an absolute path: the only two forms a path may take in the file.
pub fn path_form_ok(p: &str) -> bool {
    p == "~" || p.starts_with("~/") || p.starts_with('/')
}

/// Expand a leading `~` with `home`.
pub fn expand_tilde(p: &str, home: Option<&Path>) -> Result<PathBuf> {
    if p == "~" || p.starts_with("~/") {
        let Some(home) = home else {
            bail!("`{p}` starts with ~ but HOME is not set");
        };
        return Ok(if p == "~" {
            home.to_path_buf()
        } else {
            home.join(&p[2..])
        });
    }
    Ok(PathBuf::from(p))
}

fn str_key(t: &toml::Table, key: &str, ctx: &str) -> Result<Option<String>> {
    match t.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(toml::Value::String(_)) => bail!("{ctx}: `{key}` is empty"),
        Some(_) => bail!("{ctx}: `{key}` must be a string"),
    }
}

fn only_keys(t: &toml::Table, allowed: &[&str], ctx: &str) -> Result<()> {
    for k in t.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!("{ctx}: unknown key `{k}` (allowed: {})", allowed.join(", "));
        }
    }
    Ok(())
}

fn path_key(t: &toml::Table, ctx: &str) -> Result<Option<String>> {
    let Some(p) = str_key(t, "path", ctx)? else {
        return Ok(None);
    };
    if !path_form_ok(&p) {
        bail!("{ctx}: `path = \"{p}\"` must be an absolute path or start with ~");
    }
    Ok(Some(p))
}

fn parse_source(name: &str, v: &toml::Value) -> Result<Source> {
    let ctx = format!("[plugins.{name}]");
    let Some(t) = v.as_table() else {
        bail!("{ctx} must be a table");
    };
    only_keys(t, &["version", "git", "branch", "rev", "path"], &ctx)?;
    let version = str_key(t, "version", &ctx)?;
    let git = str_key(t, "git", &ctx)?;
    let path = path_key(t, &ctx)?;
    let branch = str_key(t, "branch", &ctx)?;
    let rev = str_key(t, "rev", &ctx)?;
    let sources = [version.is_some(), git.is_some(), path.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if sources == 0 {
        bail!("{ctx}: write one of `version`, `git` or `path`");
    }
    if sources > 1 {
        bail!("{ctx}: `version`, `git` and `path` are alternatives; write only one");
    }
    if git.is_none() && (branch.is_some() || rev.is_some()) {
        bail!("{ctx}: `branch` and `rev` go with `git`");
    }
    if branch.is_some() && rev.is_some() {
        bail!("{ctx}: write `branch` or `rev`, not both");
    }
    Ok(if let Some(version) = version {
        Source::Crates { version }
    } else if let Some(url) = git {
        Source::Git { url, branch, rev }
    } else {
        Source::Path {
            path: path.unwrap_or_default(),
        }
    })
}

impl PluginList {
    /// Parse the text of a `plugins.toml`.
    pub fn parse(text: &str) -> Result<Self> {
        let root: toml::Table = text.parse().context("not valid TOML")?;
        for k in root.keys() {
            if k != "plugins" && k != "konoma" {
                bail!("unknown table `{k}` (allowed: [plugins.<crate>], [konoma])");
            }
        }
        let mut list = PluginList::default();
        if let Some(k) = root.get("konoma") {
            let Some(t) = k.as_table() else {
                bail!("[konoma] must be a table");
            };
            only_keys(t, &["path"], "[konoma]")?;
            list.konoma_path = path_key(t, "[konoma]")?;
        }
        if let Some(p) = root.get("plugins") {
            let Some(t) = p.as_table() else {
                bail!("[plugins] must be a table of [plugins.<crate>]");
            };
            for (name, v) in t {
                if !valid_crate_name(name) {
                    bail!("[plugins.{name}]: `{name}` is not a crate name");
                }
                list.plugins.insert(name.clone(), parse_source(name, v)?);
            }
        }
        Ok(list)
    }

    /// Read the file. A missing file is an empty list; an unreadable or broken one is an error that
    /// names the file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        Self::parse(&text).with_context(|| format!("{} is broken", path.display()))
    }

    /// The file's text, as a new file.
    #[cfg(test)]
    pub fn to_toml(&self) -> String {
        self.render(None)
    }

    /// The file's text. With `existing` (the text of the file now), only what differs from the
    /// list is changed in it: comments, blank lines, the order of entries and the layout of what
    /// stays are the user's and are kept. Text that does not parse starts a new document.
    pub fn render(&self, existing: Option<&str>) -> String {
        let mut doc = existing
            .and_then(|t| t.parse::<DocumentMut>().ok())
            .unwrap_or_default();
        let root = doc.as_table_mut();

        match (&self.konoma_path, root.get_mut("konoma")) {
            (Some(p), Some(item)) => {
                if let Some(t) = item.as_table_like_mut() {
                    set_str(t, "path", p);
                }
            }
            (Some(p), None) => {
                let mut t = Table::new();
                t.insert("path", value(p.as_str()));
                root.insert("konoma", Item::Table(t));
            }
            (None, Some(item)) => {
                if let Some(t) = item.as_table_like_mut() {
                    t.remove("path");
                }
            }
            (None, None) => {}
        }

        if root.get("plugins").is_none() && !self.plugins.is_empty() {
            let mut t = Table::new();
            // Only `[plugins.<crate>]` headers are written, not an empty `[plugins]` above them.
            t.set_implicit(true);
            root.insert("plugins", Item::Table(t));
        }
        if let Some(plugins) = root
            .get_mut("plugins")
            .and_then(|item| item.as_table_like_mut())
        {
            let gone: Vec<String> = plugins
                .iter()
                .map(|(k, _)| k.to_string())
                .filter(|k| !self.plugins.contains_key(k))
                .collect();
            for k in gone {
                plugins.remove(&k);
            }
            for (name, src) in &self.plugins {
                if plugins.get(name).is_none() {
                    plugins.insert(name, Item::Table(Table::new()));
                }
                let Some(t) = plugins
                    .get_mut(name)
                    .and_then(|item| item.as_table_like_mut())
                else {
                    continue;
                };
                // The keys of the source, and every other source key removed.
                let (version, git, branch, rev, path) = match src {
                    Source::Crates { version } => (Some(version), None, None, None, None),
                    Source::Git { url, branch, rev } => {
                        (None, Some(url), branch.as_ref(), rev.as_ref(), None)
                    }
                    Source::Path { path } => (None, None, None, None, Some(path)),
                };
                for (key, want) in [
                    ("version", version),
                    ("git", git),
                    ("branch", branch),
                    ("rev", rev),
                    ("path", path),
                ] {
                    match want {
                        Some(v) => set_str(t, key, v),
                        None => {
                            t.remove(key);
                        }
                    }
                }
            }
        }
        doc.to_string()
    }

    /// Write the file atomically (temporary file, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        let existing = std::fs::read_to_string(path).ok();
        write_atomic(path, self.render(existing.as_deref()).as_bytes())
    }
}

/// Set `key` to the string `val`. A value that is already right is not touched, and a changed one
/// keeps the comment and spacing around it.
fn set_str(t: &mut dyn TableLike, key: &str, val: &str) {
    match t.get_mut(key) {
        Some(item) if item.as_str() == Some(val) => {}
        Some(item) => {
            let mut new = toml_edit::Value::from(val);
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            t.insert(key, value(val));
        }
    }
}

/// Write `bytes` to `path` through a temporary file in the same directory and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(path, bytes, |p, b| std::fs::write(p, b))
}

fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&Path, &[u8]) -> std::io::Result<()>,
) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        name.unwrap_or_else(|| "file".into()),
        std::process::id()
    ));
    struct TempPath(std::path::PathBuf);
    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let tmp = TempPath(tmp);
    write(&tmp.0, bytes).with_context(|| format!("cannot write {}", tmp.0.display()))?;
    std::fs::rename(&tmp.0, path).with_context(|| format!("cannot replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::{unique_tmp, TmpDir};

    fn tmp(prefix: &str) -> TmpDir {
        let d = unique_tmp(prefix);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn parse(t: &str) -> Result<PluginList> {
        PluginList::parse(t)
    }

    fn err(t: &str) -> String {
        format!("{:#}", parse(t).unwrap_err())
    }

    #[test]
    fn a_partial_write_failure_removes_the_atomic_temp_file() {
        let d = tmp("atomic-partial-write");
        let target = d.join("plugins.toml");
        std::fs::write(&target, "old").unwrap();
        let result = write_atomic_with(&target, b"new", |path, _| {
            std::fs::write(path, b"partial")?;
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        let files: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(files, [std::ffi::OsString::from("plugins.toml")]);
    }

    #[test]
    fn the_three_sources_and_the_konoma_path_parse() {
        let l = parse(
            r#"
[konoma]
path = "~/src/konoma"
[plugins.konoma-foo]
version = "0.3"
[plugins.konoma-bar]
git = "https://example.org/bar"
branch = "main"
[plugins.konoma-qux]
git = "https://example.org/qux"
rev = "abc123"
[plugins.konoma-baz]
path = "/abs/baz"
"#,
        )
        .unwrap();
        assert_eq!(l.konoma_path.as_deref(), Some("~/src/konoma"));
        assert_eq!(
            l.plugins["konoma-foo"],
            Source::Crates {
                version: "0.3".into()
            }
        );
        assert_eq!(
            l.plugins["konoma-bar"],
            Source::Git {
                url: "https://example.org/bar".into(),
                branch: Some("main".into()),
                rev: None
            }
        );
        assert_eq!(
            l.plugins["konoma-qux"],
            Source::Git {
                url: "https://example.org/qux".into(),
                branch: None,
                rev: Some("abc123".into())
            }
        );
        assert_eq!(
            l.plugins["konoma-baz"],
            Source::Path {
                path: "/abs/baz".into()
            }
        );
    }

    #[test]
    fn an_empty_file_is_an_empty_list() {
        assert_eq!(parse("").unwrap(), PluginList::default());
        assert_eq!(parse("[plugins]\n").unwrap(), PluginList::default());
    }

    #[test]
    fn relative_and_odd_paths_are_refused() {
        for p in ["rel/dir", "./x", "../x", "~user/x", ""] {
            let e = err(&format!("[plugins.konoma-a]\npath = \"{p}\"\n"));
            assert!(e.contains("[plugins.konoma-a]"), "{p}: {e}");
        }
        let e = err("[konoma]\npath = \"rel\"\n");
        assert!(e.contains("[konoma]") && e.contains("absolute"), "{e}");
        assert!(parse("[plugins.konoma-a]\npath = \"~\"\n").is_ok());
        assert!(parse("[plugins.konoma-a]\npath = \"~/x\"\n").is_ok());
    }

    #[test]
    fn broken_shapes_are_errors_naming_the_table() {
        for (text, needle) in [
            ("[plugins.konoma-a]\n", "one of"),
            (
                "[plugins.konoma-a]\nversion = \"1\"\ngit = \"u\"\n",
                "alternatives",
            ),
            (
                "[plugins.konoma-a]\nversion = \"1\"\nrev = \"x\"\n",
                "`git`",
            ),
            (
                "[plugins.konoma-a]\ngit = \"u\"\nbranch = \"b\"\nrev = \"r\"\n",
                "not both",
            ),
            ("[plugins.konoma-a]\nversion = 3\n", "string"),
            ("[plugins.konoma-a]\nversion = \"\"\n", "empty"),
            ("[plugins.konoma-a]\nbogus = 1\nversion = \"1\"\n", "bogus"),
            ("plugins = 3\n", "[plugins]"),
            ("[plugins]\nkonoma-a = \"1\"\n", "must be a table"),
            ("[plugins.\"a b\"]\nversion = \"1\"\n", "crate name"),
            ("[plugins.\"-a\"]\nversion = \"1\"\n", "crate name"),
            ("[other]\nx = 1\n", "unknown table"),
            ("[konoma]\nversion = \"1\"\n", "version"),
            ("this is = = not toml", "TOML"),
        ] {
            let e = err(text);
            assert!(e.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn crate_names_follow_crates_io() {
        for ok in ["konoma-foo", "a", "a_b-1", "A9"] {
            assert!(valid_crate_name(ok), "{ok}");
        }
        for bad in ["", "-a", "1a", "a b", "a/b", "a.b", "é"] {
            assert!(!valid_crate_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_text_it_writes_reads_back_the_same() {
        let mut l = PluginList {
            konoma_path: Some("/k".into()),
            ..Default::default()
        };
        l.plugins.insert(
            "konoma-a".into(),
            Source::Crates {
                version: "^1.2".into(),
            },
        );
        l.plugins.insert(
            "konoma-b".into(),
            Source::Git {
                url: "https://x/\"y\"".into(),
                branch: Some("dev".into()),
                rev: None,
            },
        );
        l.plugins
            .insert("konoma-c".into(), Source::Path { path: "~/c".into() });
        assert_eq!(parse(&l.to_toml()).unwrap(), l);
        assert_eq!(
            parse(&PluginList::default().to_toml()).unwrap(),
            PluginList::default()
        );
    }

    #[test]
    fn load_of_a_missing_file_is_empty_and_of_a_broken_one_an_error() {
        let dir = tmp("plist-load");
        let f = dir.as_path().join("plugins.toml");
        assert_eq!(PluginList::load(&f).unwrap(), PluginList::default());
        std::fs::write(&f, "[plugins.x]\n").unwrap();
        let e = format!("{:#}", PluginList::load(&f).unwrap_err());
        assert!(e.contains("plugins.toml") && e.contains("broken"), "{e}");
    }

    #[test]
    fn a_broken_file_is_left_untouched_by_the_read() {
        let dir = tmp("plist-untouched");
        let f = dir.as_path().join("plugins.toml");
        let broken = "# mine\n[plugins.konoma-a]\npath = \"rel\"\n";
        std::fs::write(&f, broken).unwrap();
        assert!(PluginList::load(&f).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), broken);
    }

    #[test]
    fn save_replaces_the_file_through_a_temporary_one() {
        let dir = tmp("plist-save");
        let f = dir.as_path().join("sub").join("plugins.toml");
        let mut l = PluginList::default();
        l.plugins.insert(
            "konoma-a".into(),
            Source::Crates {
                version: "1".into(),
            },
        );
        l.save(&f).unwrap();
        assert_eq!(PluginList::load(&f).unwrap(), l);
        l.plugins.clear();
        l.save(&f).unwrap();
        assert_eq!(PluginList::load(&f).unwrap(), l);
        let left: Vec<_> = std::fs::read_dir(f.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["plugins.toml"], "no temporary file is left");
    }

    #[test]
    fn a_failed_rename_leaves_the_old_file_and_no_temporary() {
        let dir = tmp("plist-save-fail");
        // The target is a non-empty directory: the rename cannot replace it.
        let target = dir.as_path().join("plugins.toml");
        std::fs::create_dir_all(target.join("inner")).unwrap();
        assert!(write_atomic(&target, b"x").is_err());
        assert!(target.join("inner").is_dir());
        let names: Vec<_> = std::fs::read_dir(dir.as_path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["plugins.toml"]);
    }

    const HAND_WRITTEN: &str = "# my plugins\n\n[konoma]   # dev tree\npath = \"/k\"\n\n# office tools\n[plugins.konoma-zeta]\nversion = \"1\"   # pinned\n\n[plugins.konoma-alpha]\ngit = \"https://x/a\"\nbranch = \"main\"\n";

    #[test]
    fn rewriting_keeps_comments_blank_lines_and_the_order_of_entries() {
        let mut l = parse(HAND_WRITTEN).unwrap();
        // Unchanged list: the text comes back byte for byte.
        assert_eq!(l.render(Some(HAND_WRITTEN)), HAND_WRITTEN);
        // Adding appends; zeta stays above alpha although the map is sorted.
        l.plugins.insert(
            "konoma-new".into(),
            Source::Crates {
                version: "0.3".into(),
            },
        );
        let added = l.render(Some(HAND_WRITTEN));
        assert!(added.starts_with(HAND_WRITTEN), "{added}");
        assert!(added.contains("[plugins.konoma-new]\nversion = \"0.3\""));
        assert_eq!(parse(&added).unwrap(), l);
        // Removing takes the entry and leaves the rest.
        l.plugins.remove("konoma-zeta");
        let removed = l.render(Some(&added));
        assert!(!removed.contains("zeta"), "{removed}");
        assert!(removed.contains("# my plugins") && removed.contains("# dev tree"));
        assert!(removed.contains("[plugins.konoma-alpha]") && removed.contains("branch"));
        assert_eq!(parse(&removed).unwrap(), l);
    }

    #[test]
    fn a_changed_source_swaps_its_keys_and_keeps_the_comment_after_the_value() {
        let text = "[plugins.konoma-a]\nversion = \"1\"   # pinned\n";
        let mut l = parse(text).unwrap();
        l.plugins.insert(
            "konoma-a".into(),
            Source::Crates {
                version: "2".into(),
            },
        );
        assert_eq!(
            l.render(Some(text)),
            "[plugins.konoma-a]\nversion = \"2\"   # pinned\n"
        );
        l.plugins.insert(
            "konoma-a".into(),
            Source::Git {
                url: "https://x/a".into(),
                branch: None,
                rev: Some("abc".into()),
            },
        );
        let swapped = l.render(Some(text));
        assert!(!swapped.contains("version"), "{swapped}");
        assert_eq!(parse(&swapped).unwrap(), l);
    }

    #[test]
    fn the_inline_form_is_edited_in_place_too() {
        let text = "plugins = { konoma-a = { version = \"1\" } }\n";
        let mut l = parse(text).unwrap();
        l.plugins
            .insert("konoma-b".into(), Source::Path { path: "/b".into() });
        l.plugins.remove("konoma-a");
        let out = l.render(Some(text));
        assert_eq!(parse(&out).unwrap(), l, "{out}");
        assert!(!out.contains("konoma-a"), "{out}");
        // Text that is not TOML cannot be edited: a new document, which still reads back.
        assert_eq!(parse(&l.render(Some("= = ="))).unwrap(), l);
    }

    #[test]
    fn a_list_that_becomes_empty_writes_a_file_that_reads_back_empty() {
        // A comment above an entry belongs to that entry and goes with it; the others stay.
        let text =
            "# keep me\n[konoma]\npath = \"/k\"\n# about a\n[plugins.konoma-a]\nversion = \"1\"\n";
        let mut l = parse(text).unwrap();
        l.plugins.clear();
        let out = l.render(Some(text));
        assert!(
            out.contains("# keep me") && !out.contains("# about a"),
            "{out}"
        );
        assert_eq!(parse(&out).unwrap(), l);
    }

    #[test]
    fn save_goes_through_the_file_that_is_there() {
        let d = tmp("save-keeps");
        let f = d.join("plugins.toml");
        std::fs::write(&f, HAND_WRITTEN).unwrap();
        let mut l = PluginList::load(&f).unwrap();
        l.plugins.remove("konoma-alpha");
        l.save(&f).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(
            text.contains("# my plugins") && text.contains("# pinned"),
            "{text}"
        );
        assert!(!text.contains("alpha"), "{text}");
    }

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/h");
        assert_eq!(expand_tilde("~", Some(home)).unwrap(), PathBuf::from("/h"));
        assert_eq!(
            expand_tilde("~/a/b", Some(home)).unwrap(),
            PathBuf::from("/h/a/b")
        );
        assert_eq!(expand_tilde("/x/y", None).unwrap(), PathBuf::from("/x/y"));
        assert!(expand_tilde("~/a", None).is_err());
    }

    #[test]
    fn describe_names_the_origin() {
        assert!(Source::Crates {
            version: "0.3".into()
        }
        .describe()
        .contains("crates.io"));
        let g = Source::Git {
            url: "u".into(),
            branch: Some("b".into()),
            rev: None,
        };
        assert_eq!(g.describe(), "git u, branch b");
        assert!(!g.has_update_check());
        assert!(!Source::Path { path: "/p".into() }.has_update_check());
        assert!(Source::Crates {
            version: "1".into()
        }
        .has_update_check());
    }
}
