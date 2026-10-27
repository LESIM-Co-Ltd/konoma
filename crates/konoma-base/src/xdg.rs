//! XDG base-directory resolution shared by the host and plugin SDK hosts.

use std::path::PathBuf;

/// Shared XDG base-dir resolution rule, used by all three of konoma's on-disk config/cache
/// locations: bookmarks/session storage here (`bookmarks::default_base`), the config file path
/// (`dirs_config_path`), and the image-cache root (the image-cache root). The rule: the XDG
/// variable (`XDG_CONFIG_HOME` / `XDG_CACHE_HOME`) wins when it is set and non-empty; otherwise
/// `$HOME/<home_suffix>`; otherwise `None`.
///
/// Before this helper existed the three call sites disagreed: the image-cache root already honored its
/// XDG variable, but this module's `default_base` and `dirs_config_path` silently ignored
/// `XDG_CONFIG_HOME` entirely — and, the more serious bug, `default_base` fell back to
/// `PathBuf::default()` (`.unwrap_or_default()`) when `HOME` was unset, which is an *empty* path:
/// `PathBuf::new().join(".config/konoma")` yields the **relative** path `.config/konoma`, so a
/// `HOME`-less launch (a systemd unit, `su` without `-`, a minimal container) silently wrote
/// `bookmarks.toml`/session files into whatever directory konoma happened to be started from
/// (`dirs_config_path` degraded more safely to `None`, since it is read-only and never
/// writes anywhere when that happens).
///
/// Takes the two env-var values as parameters instead of reading them itself, purely so it is
/// testable without mutating process-wide environment variables: `std::env::var_os` reads process
/// state shared by every test thread, so a test that called `std::env::set_var("HOME", ..)` would
/// affect every other test running concurrently — including the ~340 `App::new` call sites, each of
/// which resolves a `Bookmarks` base through this same rule on construction.
pub fn xdg_base_dir(
    xdg_value: Option<&std::ffi::OsStr>,
    home_value: Option<&std::ffi::OsStr>,
    home_suffix: &str,
) -> Option<PathBuf> {
    if let Some(x) = xdg_value {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    let home = home_value?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(home_suffix))
}

/// Config-scope base directory (`$XDG_CONFIG_HOME`, or `$HOME/.config`, or `None`). The impure,
/// real-env-reading wrapper around `xdg_base_dir`; shared with `dirs_config_path`.
pub fn xdg_config_home() -> Option<PathBuf> {
    xdg_base_dir(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        ".config",
    )
}

/// Pure core of `dirs_config_path`: given the already-resolved config-scope base (whatever
/// `xdg_config_home` decided — `$XDG_CONFIG_HOME`-based or
/// `$HOME/.config`-based, shared with bookmarks/session storage and the image cache), the config
/// file path is `<base>/konoma/config.toml`. `None` in means `None` out — unlike bookmarks/session
/// storage (bookmarks storage), this is read-only (`Config::load_reporting` never writes),
/// so "no known base" safely means "no config file to read, use defaults" with nothing to guard
/// against writing into the working directory.
pub fn dirs_config_path_from(config_home: Option<PathBuf>) -> Option<PathBuf> {
    Some(config_home?.join("konoma").join("config.toml"))
}

/// Config file path (`$XDG_CONFIG_HOME/konoma/config.toml`, or `$HOME/.config/konoma/config.toml`,
/// or `None`). Previously read only `$HOME` and silently ignored `$XDG_CONFIG_HOME` — now shares the
/// same base resolution as bookmarks storage (see that function's doc for the
/// details of the rule and why the three previously disagreed).
pub fn dirs_config_path() -> Option<PathBuf> {
    dirs_config_path_from(xdg_config_home())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn xdg_var_wins_when_non_empty() {
        assert_eq!(
            xdg_base_dir(Some(OsStr::new("/x")), Some(OsStr::new("/h")), ".config"),
            Some(PathBuf::from("/x"))
        );
    }

    #[test]
    fn empty_xdg_falls_back_to_home_suffix() {
        assert_eq!(
            xdg_base_dir(Some(OsStr::new("")), Some(OsStr::new("/h")), ".config"),
            Some(PathBuf::from("/h/.config"))
        );
        assert_eq!(
            xdg_base_dir(None, Some(OsStr::new("/h")), ".cache"),
            Some(PathBuf::from("/h/.cache"))
        );
    }

    #[test]
    fn missing_or_empty_home_is_none() {
        assert_eq!(xdg_base_dir(None, None, ".config"), None);
        assert_eq!(xdg_base_dir(None, Some(OsStr::new("")), ".config"), None);
    }

    #[test]
    fn config_path_joins_konoma_config_toml() {
        assert_eq!(
            dirs_config_path_from(Some(PathBuf::from("/b"))),
            Some(PathBuf::from("/b/konoma/config.toml"))
        );
        assert_eq!(dirs_config_path_from(None), None);
    }
}
