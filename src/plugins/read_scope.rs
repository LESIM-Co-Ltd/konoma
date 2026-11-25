//! The folders the host reads files from on behalf of a plugin (`[plugins.<id>] read_dirs`).
//!
//! Only plugins the user added are limited; the standard ones read as they always did. The scope
//! is the tab root plus the folder of the previewed file, widened by `read_dirs`. The previewed
//! file itself is never refused (see `konoma_plugin::canvas_pictures::files::open_picture_file`).

use konoma_plugin::ReadScope;
use std::path::{Path, PathBuf};

/// What is wrong with one `read_dirs` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadDirsProblem {
    /// The value is not a list of strings.
    NotAList,
    /// An element of the list is not a string.
    NotAString,
    /// The entry is neither absolute nor `~`-prefixed (rejected).
    Relative(String),
    /// The entry names a folder that does not exist (ignored).
    Missing(String),
}

/// The parsed `read_dirs`: the folders to add and what to tell the user.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ReadDirs {
    pub(crate) dirs: Vec<PathBuf>,
    pub(crate) problems: Vec<ReadDirsProblem>,
}

/// Expand a leading `~` / `~/` with `home`. `None` when the entry is not absolute and not
/// `~`-prefixed (or `~` is used with no home to expand it with). Pure: the caller gives `home`.
fn expand(entry: &str, home: Option<&Path>) -> Option<PathBuf> {
    if entry == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = entry.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    let p = Path::new(entry);
    p.is_absolute().then(|| p.to_path_buf())
}

/// Read a `read_dirs` value. `home` is where `~` points (a parameter so tests never touch the
/// real home). A missing folder is reported but still returned out of `dirs`.
pub(crate) fn parse_read_dirs(value: &toml::Value, home: Option<&Path>) -> ReadDirs {
    let mut out = ReadDirs::default();
    let toml::Value::Array(items) = value else {
        out.problems.push(ReadDirsProblem::NotAList);
        return out;
    };
    for item in items {
        let toml::Value::String(s) = item else {
            out.problems.push(ReadDirsProblem::NotAString);
            continue;
        };
        match expand(s, home) {
            None => out.problems.push(ReadDirsProblem::Relative(s.clone())),
            Some(p) if !p.is_dir() => out.problems.push(ReadDirsProblem::Missing(s.clone())),
            Some(p) => out.dirs.push(p),
        }
    }
    out
}

/// Whether plugin `id` is one of the host's own (a standard plugin, or one of the three internal
/// ones: plain text, the external command, the unsupported notice). Those are not limited: they
/// are konoma's own code, and the command plugin names the private file its output went to.
///
/// Decided by **how the plugin was registered** (`registry`), never by its id: the profile table
/// is keyed by id, so a plugin the user added under a standard id in a registry that does not
/// hold the standard one would otherwise read as the standard plugin. An id the registry does not
/// hold is not host-owned (limited).
pub(crate) fn is_host_owned(registry: &konoma_plugin::registry::Registry, id: &str) -> bool {
    registry.origin(id) == Some(konoma_plugin::registry::PluginOrigin::Standard)
}

/// The user's home, for `~` (the `HOME` variable; tests give their own to the pure functions).
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
thread_local! {
    /// How many times `scope_for` ran on this thread (test seam: a redraw must not rebuild).
    pub(crate) static SCOPE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The scope for plugin `id`: unrestricted for a standard plugin; otherwise the tab `root`, the
/// folder of `shown` (as opened and as resolved), and the plugin's `read_dirs`. Pure in `home`.
pub(crate) fn scope_for(
    standard: bool,
    table: Option<&toml::Table>,
    root: &Path,
    shown: &Path,
    home: Option<&Path>,
) -> ReadScope {
    #[cfg(test)]
    SCOPE_BUILDS.with(|c| c.set(c.get() + 1));
    if standard {
        return ReadScope::unrestricted();
    }
    let mut dirs = vec![root.to_path_buf()];
    // The previewed file's folder, both where it was opened (where a link sits) and, when the
    // file is a symbolic link, where it really is (its target's parent): a plugin names the
    // sibling files of the real file (an SVG's `base_dir`), so both count as "its folder".
    if let Some(d) = shown.parent() {
        dirs.push(d.to_path_buf());
    }
    if let Some(d) = std::fs::canonicalize(shown)
        .ok()
        .and_then(|r| r.parent().map(Path::to_path_buf))
    {
        dirs.push(d);
    }
    if let Some(v) = table.and_then(|t| t.get("read_dirs")) {
        dirs.extend(parse_read_dirs(v, home).dirs);
    }
    ReadScope::within(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use toml::Value;

    fn list(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| Value::String((*s).into())).collect())
    }

    /// A plugin that names `id`, for the origin tests.
    struct Named(&'static str);
    impl konoma_plugin::Plugin for Named {
        fn manifest(&self) -> konoma_plugin::Manifest {
            konoma_plugin::Manifest::new(self.0, "0.1.0").shows(konoma_plugin::ViewKind::Text)
        }
        fn open(
            &self,
            _: konoma_plugin::Input,
            _: &konoma_plugin::OpenCx,
        ) -> Result<Box<dyn konoma_plugin::Session>, konoma_plugin::Failure> {
            Err(konoma_plugin::Failure::unsupported(
                konoma_plugin::t!(en: "no"),
            ))
        }
    }

    #[test]
    fn host_owned_follows_the_origin_not_the_id() {
        use konoma_plugin::registry::{PluginOrigin, Registry};
        // A user plugin under a standard id in a registry that does not hold the standard one is
        // registered (nothing refuses it there) and the profile table still says "standard" for
        // that id -- it must stay limited.
        let mut r = Registry::new();
        crate::plugins::register(
            &mut r,
            std::sync::Arc::new(Named("markdown")),
            PluginOrigin::User,
        )
        .unwrap();
        assert!(crate::plugins::profile_of("markdown").standard);
        assert!(!is_host_owned(&r, "markdown"), "origin User: limited");
        // An id the registry does not hold is limited too.
        assert!(!is_host_owned(&r, "nobody"));
        // The ones konoma registers itself are not limited (standard and internal).
        let own = crate::plugins::build_registry(&[]);
        for id in ["markdown", "image", "text", "command", "can-not-preview"] {
            assert!(is_host_owned(&own, id), "{id}");
        }
    }

    #[test]
    fn a_user_plugin_cannot_take_a_standard_id_in_the_real_registry() {
        use konoma_plugin::registry::{PluginOrigin, RegistryError};
        // The standard plugins are registered first and a second plugin with the same id is
        // refused (registry.rs `DuplicateId`), even with the standard one switched off.
        for disabled in [vec![], vec!["markdown".to_string()]] {
            let mut r = crate::plugins::build_registry(&disabled);
            let got = crate::plugins::register(
                &mut r,
                std::sync::Arc::new(Named("markdown")),
                PluginOrigin::User,
            );
            assert!(matches!(got, Err(RegistryError::DuplicateId(_))), "{got:?}");
            assert_eq!(r.origin("markdown"), Some(PluginOrigin::Standard));
            assert!(is_host_owned(&r, "markdown"));
        }
    }

    fn mkdirs(root: &Path, names: &[&str]) {
        for n in names {
            std::fs::create_dir_all(root.join(n)).unwrap();
        }
    }

    #[test]
    fn an_absolute_existing_entry_is_taken() {
        let dir = unique_tmp("rs_abs");
        mkdirs(&dir, &["a"]);
        let a = dir.join("a");
        let got = parse_read_dirs(&list(&[a.to_str().unwrap()]), None);
        assert_eq!(got.dirs, vec![a]);
        assert!(got.problems.is_empty());
    }

    #[test]
    fn tilde_expands_to_the_given_home_never_the_real_one() {
        let home = unique_tmp("rs_home");
        mkdirs(&home, &["pics"]);
        let got = parse_read_dirs(&list(&["~/pics", "~"]), Some(&home));
        assert_eq!(got.dirs, vec![home.join("pics"), home.to_path_buf()]);
        assert!(got.problems.is_empty(), "{:?}", got.problems);
        // Without a home `~` cannot be expanded: refused like a relative entry.
        let none = parse_read_dirs(&list(&["~/pics"]), None);
        assert!(none.dirs.is_empty());
        assert_eq!(
            none.problems,
            vec![ReadDirsProblem::Relative("~/pics".into())]
        );
    }

    #[test]
    fn a_relative_entry_is_rejected_with_a_problem() {
        for bad in ["pics", "./pics", "../pics", "~pics", "~user/pics", ""] {
            let got = parse_read_dirs(&list(&[bad]), Some(Path::new("/h")));
            assert!(got.dirs.is_empty(), "{bad:?} taken");
            assert_eq!(
                got.problems,
                vec![ReadDirsProblem::Relative(bad.into())],
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_missing_entry_is_a_problem_and_not_taken() {
        let dir = unique_tmp("rs_missing");
        mkdirs(&dir, &["there"]);
        let gone = dir.join("gone");
        let got = parse_read_dirs(
            &list(&[gone.to_str().unwrap(), dir.join("there").to_str().unwrap()]),
            None,
        );
        assert_eq!(got.dirs, vec![dir.join("there")]);
        assert_eq!(
            got.problems,
            vec![ReadDirsProblem::Missing(gone.to_str().unwrap().into())]
        );
        // A file is not a folder.
        std::fs::write(dir.join("f"), b"x").unwrap();
        let f = dir.join("f");
        let got = parse_read_dirs(&list(&[f.to_str().unwrap()]), None);
        assert!(got.dirs.is_empty());
        assert_eq!(got.problems.len(), 1);
    }

    #[test]
    fn a_value_that_is_not_a_list_of_strings_is_reported() {
        let got = parse_read_dirs(&Value::String("/tmp".into()), None);
        assert_eq!(got.problems, vec![ReadDirsProblem::NotAList]);
        let got = parse_read_dirs(&Value::Array(vec![Value::Integer(3)]), None);
        assert_eq!(got.problems, vec![ReadDirsProblem::NotAString]);
        assert!(got.dirs.is_empty());
    }

    fn table(dirs: &[&Path]) -> toml::Table {
        let mut t = toml::Table::new();
        t.insert(
            "read_dirs".into(),
            Value::Array(
                dirs.iter()
                    .map(|d| Value::String(d.to_str().unwrap().into()))
                    .collect(),
            ),
        );
        t
    }

    #[test]
    fn a_standard_plugin_is_unrestricted_and_a_user_plugin_is_not() {
        let dir = unique_tmp("rs_for");
        mkdirs(&dir, &["root", "pv"]);
        let (root, shown) = (dir.join("root"), dir.join("pv").join("a.x"));
        assert!(!scope_for(true, None, &root, &shown, None).is_restricted());
        let user = scope_for(false, None, &root, &shown, None);
        assert!(user.is_restricted());
        let real = |p: PathBuf| std::fs::canonicalize(p).unwrap();
        assert_eq!(
            user.dirs().unwrap(),
            &[real(root.clone()), real(dir.join("pv"))][..]
        );
    }

    #[test]
    fn the_previewed_files_folder_holds_as_written_and_as_resolved_through_a_link() {
        let dir = unique_tmp("rs_for_link");
        mkdirs(&dir, &["root", "pv", "real", "other"]);
        std::fs::write(dir.join("real/t.svg"), b"x").unwrap();
        std::fs::write(dir.join("real/sibling.png"), b"x").unwrap();
        std::fs::write(dir.join("pv/own.png"), b"x").unwrap();
        std::fs::write(dir.join("other/no.png"), b"x").unwrap();
        let link = dir.join("pv/l.svg");
        std::os::unix::fs::symlink(dir.join("real/t.svg"), &link).unwrap();
        let s = scope_for(false, None, &dir.join("root"), &link, None);
        // Where the link sits.
        assert!(s.allows(&dir.join("pv/own.png")), "folder as written");
        // Where the file really is (what an SVG's `base_dir` names).
        assert!(
            s.allows(&dir.join("real/sibling.png")),
            "folder as resolved"
        );
        assert!(s.allows(&dir.join("real")));
        // Nothing else opened up.
        assert!(!s.allows(&dir.join("other/no.png")));
    }

    #[test]
    fn read_dirs_is_added_to_the_default_scope() {
        let dir = unique_tmp("rs_for_wide");
        mkdirs(&dir, &["root", "pv", "extra"]);
        let (root, shown) = (dir.join("root"), dir.join("pv").join("a.x"));
        let t = table(&[&dir.join("extra")]);
        let s = scope_for(false, Some(&t), &root, &shown, None);
        assert!(s.allows(&dir.join("extra")));
        assert!(s.allows(&root));
        // Without the key the extra folder is outside.
        let s = scope_for(false, None, &root, &shown, None);
        assert!(!s.allows(&dir.join("extra")));
    }

    #[test]
    fn a_relative_or_missing_read_dirs_entry_widens_nothing() {
        let dir = unique_tmp("rs_for_bad");
        mkdirs(&dir, &["root", "pv", "extra"]);
        let (root, shown) = (dir.join("root"), dir.join("pv").join("a.x"));
        let mut t = toml::Table::new();
        t.insert(
            "read_dirs".into(),
            list(&["extra", dir.join("nope").to_str().unwrap()]),
        );
        let s = scope_for(false, Some(&t), &root, &shown, None);
        assert!(!s.allows(&dir.join("extra")));
        assert_eq!(
            s.dirs().unwrap().len(),
            2,
            "only root and the preview folder"
        );
    }

    #[test]
    fn a_scope_follows_the_tilde_to_the_fake_home() {
        let dir = unique_tmp("rs_for_home");
        mkdirs(&dir, &["root", "home/pics"]);
        let home = dir.join("home");
        let mut t = toml::Table::new();
        t.insert("read_dirs".into(), list(&["~/pics"]));
        let s = scope_for(
            false,
            Some(&t),
            &dir.join("root"),
            &dir.join("root/a.x"),
            Some(&home),
        );
        assert!(s.allows(&home.join("pics")));
        assert!(!s.allows(&home));
    }

    #[test]
    fn read_dirs_never_reaches_the_plugins_own_settings() {
        let dir = unique_tmp("rs_settings");
        mkdirs(&dir, &["x"]);
        let mut t = table(&[&dir.join("x")]);
        t.insert("wrap".into(), Value::Boolean(true));
        let settings = crate::plugins::settings_of(&t);
        assert!(settings.get("read_dirs").is_none());
        assert!(settings.get("wrap").is_some());
        assert!(crate::plugins::HOST_PLUGIN_KEYS.contains(&"read_dirs"));
    }

    #[test]
    fn startup_warnings_name_the_plugin_and_the_entry() {
        use crate::i18n::Lang;
        let mut tables = crate::plugins::PluginTables::new();
        let mut t = toml::Table::new();
        t.insert(
            "read_dirs".into(),
            list(&["relative/dir", "/definitely/not/here"]),
        );
        tables.insert(konoma_markdown::MARKDOWN_ID.to_string(), t);
        for lang in [Lang::En, Lang::Jp] {
            let msgs = crate::plugins::table_problems(&tables, lang);
            let all = msgs.join("\n");
            assert!(all.contains("relative/dir"), "{all}");
            assert!(all.contains("/definitely/not/here"), "{all}");
            assert!(all.contains("read_dirs"), "{all}");
            assert!(all.contains(konoma_markdown::MARKDOWN_ID), "{all}");
            assert_eq!(msgs.len(), 2, "{all}");
        }
        // And the key is not reported as an unknown setting.
        let all = crate::plugins::table_problems(&tables, Lang::En).join("\n");
        assert!(!all.contains("unknown setting"), "{all}");
    }

    #[test]
    fn a_good_read_dirs_gives_no_warning() {
        use crate::i18n::Lang;
        let dir = unique_tmp("rs_nowarn");
        mkdirs(&dir, &["ok"]);
        let mut tables = crate::plugins::PluginTables::new();
        tables.insert(
            konoma_markdown::MARKDOWN_ID.to_string(),
            table(&[&dir.join("ok")]),
        );
        assert!(crate::plugins::table_problems(&tables, Lang::En).is_empty());
    }
}
