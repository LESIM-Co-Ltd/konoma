//! Best-effort sparse-index checks; the UI only receives completed observations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::{list_file::PluginList, project};
use crate::i18n::{tr, Lang, Msg};

// The SDK's public transport defaults embed its own CARGO_PKG_VERSION. Read that once, rather
// than confusing the host package version or a dependency's semver requirement with the SDK
// actually linked into this executable. This also works when konoma is used from crates.io.
static HOST_PLUGIN_VERSION: std::sync::LazyLock<Version> = std::sync::LazyLock::new(|| {
    Version::parse(&konoma_plugin::proc::client::Config::default().host_version)
        .expect("the linked SDK package version is valid semver")
});

pub(crate) struct Worker {
    rx: Option<Receiver<Snapshot>>,
    method: InstallMethod,
}

impl Worker {
    pub(crate) fn start(enabled: bool, original_exe: Option<&Path>) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
        let method = original_exe.map_or(InstallMethod::Other, |p| {
            install_method(p, home.as_deref(), cargo_home.as_deref())
        });
        let rx = if enabled {
            konoma_base::xdg::xdg_config_home()
                .zip(konoma_base::xdg::data_dir())
                .and_then(|(config, data)| {
                    start_with(
                        true,
                        &config.join("konoma"),
                        data,
                        HOST_PLUGIN_VERSION.clone(),
                        || {
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                        },
                        fetch_index,
                        spawn_worker,
                    )
                })
        } else {
            None
        };
        Self { rx, method }
    }

    /// Called only on the UI thread; preserves the current flash and appends guidance after it.
    pub(crate) fn apply(&mut self, flash: &mut Option<String>, lang: Lang) -> bool {
        let Some(rx) = self.rx.as_ref() else {
            return false;
        };
        let snapshot = match rx.try_recv() {
            Ok(snapshot) => snapshot,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                return false;
            }
        };
        self.rx = None;
        let (compatible, needs_host) = snapshot
            .plugins
            .values()
            .map(|o| o.updates.counts())
            .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
        let Some(note) = notice(compatible, needs_host, lang, self.method) else {
            return false;
        };
        *flash = Some(match flash.take() {
            Some(current) => format!("{current} / {note}"),
            None => note,
        });
        true
    }
}

fn spawn_worker(job: Box<dyn FnOnce() + Send>) -> bool {
    // Detach: exiting konoma must never wait for a slow or unreachable registry.
    std::thread::Builder::new()
        .name("plugin-updates".into())
        .spawn(job)
        .is_ok()
}

fn http_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(10)))
        .max_redirects(0)
        .build()
}

fn fetch_index(url: &str) -> Option<String> {
    let agent = ureq::Agent::new_with_config(http_config());
    agent
        .get(url)
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_to_string()
        .ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    Cargo,
    Brew,
    Apt,
    Other,
}

pub(crate) fn install_method(
    exe: &Path,
    home: Option<&Path>,
    cargo_home: Option<&Path>,
) -> InstallMethod {
    let exe = super::lexical_normal(exe);
    let cargo = cargo_home
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf);
    let default_cargo = home
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join(".cargo"));
    if cargo
        .into_iter()
        .chain(default_cargo)
        .any(|p| exe.starts_with(super::lexical_normal(&p).join("bin")))
    {
        return InstallMethod::Cargo;
    }
    let text = exe.to_string_lossy();
    if ["/Cellar/", "/homebrew/", "/linuxbrew/"]
        .iter()
        .any(|p| text.contains(p))
    {
        InstallMethod::Brew
    } else if exe.starts_with("/usr/bin") {
        InstallMethod::Apt
    } else {
        InstallMethod::Other
    }
}

fn notice(
    compatible: usize,
    needs_host: usize,
    lang: Lang,
    method: InstallMethod,
) -> Option<String> {
    let mut notes = Vec::new();
    if compatible > 0 {
        notes.push(
            tr(
                lang,
                if compatible == 1 {
                    Msg::PluginUpdateOne
                } else {
                    Msg::PluginUpdates
                },
            )
            .replace("{n}", &compatible.to_string()),
        );
    }
    if needs_host > 0 {
        let method = tr(
            lang,
            match method {
                InstallMethod::Cargo => Msg::PluginUpdateCargo,
                InstallMethod::Brew => Msg::PluginUpdateBrew,
                InstallMethod::Apt => Msg::PluginUpdateApt,
                InstallMethod::Other => Msg::PluginUpdateOther,
            },
        );
        notes.push(
            tr(
                lang,
                if needs_host == 1 {
                    Msg::PluginHostUpdateOne
                } else {
                    Msg::PluginHostUpdates
                },
            )
            .replace("{n}", &needs_host.to_string())
            .replace("{method}", method),
        );
    }
    (!notes.is_empty()).then(|| notes.join(" / "))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Snapshot {
    pub checked_at: u64,
    host_version: String,
    plugins: BTreeMap<String, Observation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Observation {
    installed: String,
    updates: Updates,
}

impl Snapshot {
    pub(crate) fn list_note(&self, name: &str, installed: Option<&String>) -> Option<String> {
        let observed = self.plugins.get(name)?;
        if installed != Some(&observed.installed) {
            return None;
        }
        if let Some(v) = &observed.updates.compatible {
            Some(format!("update available: {v}"))
        } else {
            observed
                .updates
                .needs_host
                .as_ref()
                .map(|v| format!("newer {v} needs a newer konoma"))
        }
    }

    pub(crate) fn load(data_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(data_dir.join("plugin-updates.toml")).ok()?;
        let snapshot: Self = toml::from_str(&text).ok()?;
        Version::parse(&snapshot.host_version).ok()?;
        for observed in snapshot.plugins.values() {
            Version::parse(&observed.installed).ok()?;
            for v in [&observed.updates.compatible, &observed.updates.needs_host]
                .into_iter()
                .flatten()
            {
                Version::parse(v).ok()?;
            }
        }
        Some(snapshot)
    }
}

// Both the clock and thread launcher are injected: tests can prove that a check is never even
// scheduled, and can hold the network operation behind a latch without blocking the caller.
fn start_with(
    enabled: bool,
    config_dir: &Path,
    data_dir: PathBuf,
    host: Version,
    now: impl Fn() -> u64 + Send + 'static,
    fetch: impl Fn(&str) -> Option<String> + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> bool,
) -> Option<Receiver<Snapshot>> {
    if !enabled {
        return None;
    }
    let list = PluginList::load(&config_dir.join("plugins.toml")).ok()?;
    let locked = project::locked_versions(
        &std::fs::read_to_string(data_dir.join("Cargo.lock")).unwrap_or_default(),
    );
    let targets: BTreeMap<_, _> = list
        .plugins
        .iter()
        .filter(|(_, src)| src.has_update_check())
        .filter_map(|(name, _)| Some((name.clone(), Version::parse(locked.get(name)?).ok()?)))
        .collect();
    if targets.is_empty() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    if let Some(mut cached) =
        Snapshot::load(&data_dir).filter(|s| now().saturating_sub(s.checked_at) < 86400)
    {
        cached.plugins.retain(|name, o| {
            cached.host_version == host.to_string()
                && targets
                    .get(name)
                    .is_some_and(|v| v.to_string() == o.installed)
        });
        let _ = tx.send(cached);
        return Some(rx);
    }
    spawn(Box::new(move || {
        let mut plugins = BTreeMap::new();
        for (name, installed) in targets {
            if let Some(updates) =
                fetch(&index_url(&name)).and_then(|body| inspect_index(&body, &installed, &host))
            {
                plugins.insert(
                    name,
                    Observation {
                        installed: installed.to_string(),
                        updates,
                    },
                );
            }
        }
        // A failed check must not consume the daily attempt, nor redisplay an old observation.
        if plugins.is_empty() {
            return;
        }
        let snapshot = Snapshot {
            checked_at: now(),
            host_version: host.to_string(),
            plugins,
        };
        if let Ok(text) = toml::to_string(&snapshot) {
            let _ = super::list_file::write_atomic(
                &data_dir.join("plugin-updates.toml"),
                text.as_bytes(),
            );
        }
        let _ = tx.send(snapshot);
    }))
    .then_some(rx)
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Updates {
    pub compatible: Option<String>,
    pub needs_host: Option<String>,
}

impl Updates {
    fn counts(&self) -> (usize, usize) {
        if self.compatible.is_some() {
            (1, 0)
        } else if self.needs_host.is_some() {
            (0, 1)
        } else {
            (0, 0)
        }
    }
}

#[derive(Deserialize)]
struct IndexRow {
    vers: String,
    yanked: bool,
    deps: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    package: Option<String>,
    req: String,
}

fn inspect_index(body: &str, installed: &Version, host: &Version) -> Option<Updates> {
    let mut valid = false;
    let mut compatible: Option<Version> = None;
    let mut needs_host: Option<Version> = None;
    for line in body.lines() {
        let Ok(row) = serde_json::from_str::<IndexRow>(line) else {
            continue;
        };
        let Ok(version) = Version::parse(&row.vers) else {
            continue;
        };
        let requirements = row
            .deps
            .iter()
            .filter(|d| d.package.as_deref().unwrap_or(&d.name) == "konoma-plugin")
            .map(|d| VersionReq::parse(&d.req))
            .collect::<Result<Vec<_>, _>>();
        let Ok(requirements) = requirements else {
            continue;
        };
        valid = true;
        if row.yanked || !version.pre.is_empty() || version <= *installed {
            continue;
        }
        let best = if requirements.iter().all(|r| r.matches(host)) {
            &mut compatible
        } else {
            &mut needs_host
        };
        if best.as_ref().is_none_or(|v| version > *v) {
            *best = Some(version);
        }
    }
    valid.then(|| Updates {
        compatible: compatible.map(|v| v.to_string()),
        needs_host: needs_host.map(|v| v.to_string()),
    })
}

const USER_AGENT: &str = concat!("konoma/", env!("CARGO_PKG_VERSION"));

fn index_url(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    let prefix = match name.len() {
        1 => "1".into(),
        2 => "2".into(),
        3 => format!("3/{}", &name[..1]),
        _ => format!("{}/{}", &name[..2], &name[2..4]),
    };
    format!("https://index.crates.io/{prefix}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::{unique_tmp, TmpDir};

    fn fixture() -> TmpDir {
        let d = unique_tmp("plugin-updates");
        std::fs::create_dir_all(d.join("config")).unwrap();
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::write(d.join("config/plugins.toml"), "[plugins.foo]\nversion = \"*\"\n[plugins.unbuilt]\nversion = \"*\"\n[plugins.git]\ngit = \"https://example.invalid/plugin\"\n[plugins.local]\npath = \"/example/plugin\"\n").unwrap();
        std::fs::write(d.join("data/Cargo.lock"), "[[package]]\nname = \"foo\"\nversion = \"0.3.0\"\n[[package]]\nname = \"git\"\nversion = \"0.3.0\"\n[[package]]\nname = \"local\"\nversion = \"0.3.0\"\n").unwrap();
        d
    }

    fn host() -> Version {
        Version::parse("0.1.0").unwrap()
    }

    fn check(
        d: &Path,
        now: u64,
        fetch: impl Fn(&str) -> Option<String> + Send + 'static,
    ) -> Option<Snapshot> {
        start_with(
            true,
            &d.join("config"),
            d.join("data"),
            host(),
            move || now,
            fetch,
            |job| {
                job();
                true
            },
        )?
        .recv()
        .ok()
    }

    #[test]
    fn update_daily_cache_boundary_and_only_locked_registry_targets() {
        let d = fixture();
        let snapshot = check(&d, 100, |url| {
            assert_eq!(url, "https://index.crates.io/3/f/foo");
            Some(row("0.4.0", None, false))
        })
        .unwrap();
        assert_eq!(snapshot.checked_at, 100);
        assert_eq!(snapshot.plugins.len(), 1);
        assert_eq!(snapshot.plugins["foo"].updates.counts(), (1, 0));
        let cached = start_with(
            true,
            &d.join("config"),
            d.join("data"),
            host(),
            || 100 + 86400 - 1,
            |_| panic!("no HTTP before 24h"),
            |_| panic!("no thread before 24h"),
        )
        .unwrap()
        .recv()
        .unwrap();
        assert_eq!(cached, snapshot);
        let fresh = check(&d, 100 + 86400, |_| Some(row("0.5.0", None, false))).unwrap();
        assert_eq!(fresh.checked_at, 100 + 86400);
        assert_eq!(
            fresh.plugins["foo"].updates.compatible.as_deref(),
            Some("0.5.0")
        );
        assert_eq!(Snapshot::load(&d.join("data")), Some(fresh));
    }

    #[test]
    fn update_disabled_empty_missing_unbuilt_never_spawn_or_fetch() {
        let d = fixture();
        assert!(start_with(
            false,
            &d.join("config"),
            d.join("data"),
            host(),
            || panic!("disabled clock"),
            |_| panic!("disabled HTTP"),
            |_| panic!("disabled thread")
        )
        .is_none());
        for contents in [
            None,
            Some(""),
            Some("[plugins.new]\nversion = \"*\"\n"),
            Some("[plugins.local]\npath = \"/example/plugin\"\n"),
        ] {
            if let Some(contents) = contents {
                std::fs::write(d.join("config/plugins.toml"), contents).unwrap();
            } else {
                std::fs::remove_file(d.join("config/plugins.toml")).unwrap();
            }
            assert!(start_with(
                true,
                &d.join("config"),
                d.join("data"),
                host(),
                || 0,
                |_| panic!("no HTTP"),
                |_| panic!("no thread")
            )
            .is_none());
        }
    }

    #[test]
    fn update_failures_and_corrupt_cache_are_silent_and_retryable() {
        let d = fixture();
        let old = check(&d, 100, |_| Some(row("0.3.0", None, false))).unwrap();
        for response in [None, Some("HTTP 503"), Some("broken index"), Some("")] {
            assert!(check(&d, 90000, move |_| response.map(str::to_owned)).is_none());
            assert_eq!(Snapshot::load(&d.join("data")), Some(old.clone()));
        }
        std::fs::write(d.join("data/plugin-updates.toml"), "broken = [").unwrap();
        assert!(Snapshot::load(&d.join("data")).is_none());
        assert!(check(&d, 101, |_| None).is_none());
        assert!(Snapshot::load(&d.join("data")).is_none());
        assert_eq!(
            check(&d, 102, |_| Some(row("0.3.0", None, false)))
                .unwrap()
                .checked_at,
            102
        );
    }

    #[test]
    fn update_partial_failure_keeps_successes_and_rejects_obsolete_cache() {
        let d = fixture();
        std::fs::write(
            d.join("config/plugins.toml"),
            "[plugins.foo]\nversion = \"*\"\n[plugins.bar]\nversion = \"*\"\n",
        )
        .unwrap();
        std::fs::write(d.join("data/Cargo.lock"), "[[package]]\nname = \"foo\"\nversion = \"0.3.0\"\n[[package]]\nname = \"bar\"\nversion = \"0.3.0\"\n").unwrap();
        let snapshot = check(&d, 100, |url| {
            url.ends_with("/foo").then(|| row("0.4.0", None, false))
        })
        .unwrap();
        assert_eq!(snapshot.plugins.len(), 1);
        assert!(snapshot.plugins.contains_key("foo"));
        let changed_host = start_with(
            true,
            &d.join("config"),
            d.join("data"),
            Version::parse("1.0.0").unwrap(),
            || 101,
            |_| panic!("daily limit"),
            |_| panic!("daily limit"),
        )
        .unwrap()
        .recv()
        .unwrap();
        assert!(changed_host.plugins.is_empty());
        std::fs::write(
            d.join("data/Cargo.lock"),
            "[[package]]\nname = \"foo\"\nversion = \"0.4.0\"\n",
        )
        .unwrap();
        assert!(check(&d, 101, |_| panic!("daily limit"))
            .unwrap()
            .plugins
            .is_empty());
    }

    #[test]
    fn update_install_method_boundaries() {
        for (path, cargo, expected) in [
            ("/example/.cargo/bin/konoma", None, InstallMethod::Cargo),
            (
                "/example/.cargo/bin/konoma",
                Some("/tools"),
                InstallMethod::Cargo,
            ),
            ("/tools/bin/konoma", Some("/tools"), InstallMethod::Cargo),
            ("/opt/homebrew/bin/konoma", None, InstallMethod::Brew),
            ("/opt/Cellar/konoma/1/bin/konoma", None, InstallMethod::Brew),
            ("/opt/linuxbrew/bin/konoma", None, InstallMethod::Brew),
            ("/usr/bin/konoma", None, InstallMethod::Apt),
            ("/usr/local/bin/konoma", None, InstallMethod::Other),
            ("/usr/binary/konoma", None, InstallMethod::Other),
            ("/example/.cargo/binary/konoma", None, InstallMethod::Other),
            (
                "/tools/../elsewhere/konoma",
                Some("/tools"),
                InstallMethod::Other,
            ),
            ("/opt/homebrewish/konoma", None, InstallMethod::Other),
            ("/unknown/konoma", None, InstallMethod::Other),
        ] {
            assert_eq!(
                install_method(
                    Path::new(path),
                    Some(Path::new("/example")),
                    cargo.map(Path::new)
                ),
                expected,
                "{path}"
            );
        }
        assert_eq!(
            install_method(Path::new("/bin/konoma"), None, None),
            InstallMethod::Other
        );
    }

    #[test]
    fn update_flash_i18n_counts_both_kinds_and_no_updates() {
        use crate::i18n::Lang;
        for (lang, singular, plural, host_note) in [
            (Lang::En, "1 plugin update available (`konoma plugin update`)", "2 plugin updates available (`konoma plugin update`)", "1 newer plugin version needs a newer konoma (`brew upgrade konoma`)"),
            (Lang::Jp, "プラグインの更新が 1 件あります（`konoma plugin update`）", "プラグインの更新が 2 件あります（`konoma plugin update`）", "1 件のプラグインの新しい版には konoma 本体の更新が要ります（`brew upgrade konoma`）"),
        ] {
            assert_eq!(notice(1, 0, lang, InstallMethod::Other).as_deref(), Some(singular));
            assert_eq!(notice(2, 0, lang, InstallMethod::Other).as_deref(), Some(plural));
            assert_eq!(notice(0, 1, lang, InstallMethod::Brew).as_deref(), Some(host_note));
            assert_eq!(notice(2, 1, lang, InstallMethod::Brew).unwrap(), format!("{plural} / {host_note}"));
            assert!(notice(0, 0, lang, InstallMethod::Other).is_none());
            for (method, command) in [(InstallMethod::Cargo, "cargo install konoma"), (InstallMethod::Apt, "sudo apt upgrade konoma")] {
                assert!(notice(0, 2, lang, method).unwrap().contains(command));
            }
            let other = notice(0, 2, lang, InstallMethod::Other).unwrap();
            assert!(other.contains(if lang == Lang::En { "update konoma using the method you installed it with" } else { "konoma を入れた方法で更新してください" }));
        }
    }

    #[test]
    fn update_list_uses_only_saved_matching_results_and_date() {
        let d = fixture();
        let list = PluginList::load(&d.join("config/plugins.toml")).unwrap();
        let locked = [("foo".into(), "0.3.0".into())].into();
        let plain = super::super::list_text(&list, &locked);
        assert_eq!(
            super::super::list_text_with_updates(&list, &locked, None),
            plain
        );
        for (req, expected) in [
            (None, "update available: 0.4.0"),
            (Some("^1"), "newer 0.4.0 needs a newer konoma"),
        ] {
            let snapshot = check(&d, 100, move |_| Some(row("0.4.0", req, false))).unwrap();
            let text = super::super::list_text_with_updates(&list, &locked, Some(&snapshot));
            assert!(text.contains(expected), "{text}");
            assert!(text.contains("Last checked: 1970-01-01 00:01:40 UTC"));
            let changed = [("foo".into(), "0.4.0".into())].into();
            assert!(
                !super::super::list_text_with_updates(&list, &changed, Some(&snapshot))
                    .contains(expected)
            );
            std::fs::remove_file(d.join("data/plugin-updates.toml")).unwrap();
        }
        let snapshot = check(&d, 101, |_| Some(row("0.3.0", None, false))).unwrap();
        let text = super::super::list_text_with_updates(&list, &locked, Some(&snapshot));
        assert!(!text.contains("update available") && !text.contains("needs a newer"));
        assert!(text.contains("Last checked:"));
        assert!(super::super::list_text_with_updates(
            &PluginList::default(),
            &BTreeMap::new(),
            Some(&snapshot)
        )
        .contains("Last checked:"));
    }

    #[test]
    fn update_http_configuration_has_only_product_user_agent_and_timeout() {
        let cfg = http_config();
        let ureq::config::AutoHeaderValue::Provided(value) = cfg.user_agent() else {
            panic!("explicit User-Agent required")
        };
        assert_eq!(value.as_str(), USER_AGENT);
        assert_eq!(cfg.timeouts().global, Some(Duration::from_secs(10)));
        assert_eq!(cfg.max_redirects(), 0);
        assert!(cfg.http_status_as_error());
    }

    #[test]
    fn update_sdk_version_comes_from_sdk_and_disabled_production_start_is_inert() {
        let manifest: toml::Table = include_str!("../../crates/konoma-plugin/Cargo.toml")
            .parse()
            .unwrap();
        assert_eq!(
            HOST_PLUGIN_VERSION.to_string(),
            manifest["package"]["version"].as_str().unwrap()
        );
        let mut worker = Worker::start(false, None);
        assert!(worker.rx.is_none());
        assert!(!worker.apply(&mut None, Lang::En));
    }

    #[test]
    fn update_blocked_worker_allows_startup_draw_and_keys_then_appends_flash() {
        let d = fixture();
        std::fs::write(d.join("a.txt"), "a").unwrap();
        std::fs::write(d.join("b.txt"), "b").unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let config_dir = d.join("config");
        let data_dir = d.join("data");
        let launcher = std::thread::spawn(move || {
            let rx = start_with(
                true,
                &config_dir,
                data_dir,
                host(),
                || 100,
                move |_| {
                    entered_tx.send(()).unwrap();
                    let _ = release_rx.recv();
                    Some(row("0.4.0", None, false))
                },
                spawn_worker,
            );
            started_tx.send(rx).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let rx = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("startup returns while fetch is latched")
            .unwrap();
        let mut updates = Worker {
            rx: Some(rx),
            method: InstallMethod::Other,
        };
        let cfg: crate::config::Config =
            toml::from_str("[external]\ngit = false\n[ui]\nlang = \"en\"\n").unwrap();
        let bookmarks = crate::bookmarks::Bookmarks::with_base(d.join("bookmarks"), &d);
        let mut app = crate::App::new_with_bookmarks(d.to_path_buf(), cfg, bookmarks).unwrap();
        let stale = super::super::switch::stale_notice(
            "0.31.2",
            Some(std::ffi::OsStr::new("0.32.0")),
            app.lang,
        )
        .unwrap();
        app.flash = app.startup_flash(Some(format!("config error / {stale}")), 4000);
        let before = app.flash.clone().unwrap();
        assert!(!updates.apply(&mut app.flash, app.lang));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 25)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        let selected = app.tab.selected;
        crate::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('j'),
                crossterm::event::KeyModifiers::NONE,
            ),
        )
        .unwrap();
        assert_ne!(selected, app.tab.selected);
        release_tx.send(()).unwrap();
        launcher.join().unwrap();
        // Receiving here is a test-only completion latch. The UI's apply remains nonblocking.
        let snapshot = updates
            .rx
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(snapshot).unwrap();
        updates.rx = Some(rx);
        app.flash = Some(before.clone());
        assert!(updates.apply(&mut app.flash, app.lang));
        assert_eq!(
            app.flash.as_deref(),
            Some(format!("{before} / 1 plugin update available (`konoma plugin update`)").as_str())
        );
        assert!(!updates.apply(&mut app.flash, app.lang));
        assert!(app.flash.unwrap().contains(&stale));
    }

    #[test]
    fn update_worker_emits_to_empty_flash_but_says_nothing_for_zero() {
        let d = fixture();
        let mut flash = None;
        for (version, expected) in [("0.3.0", false), ("0.4.0", true)] {
            let snapshot = check(&d, 100000, move |_| Some(row(version, None, false))).unwrap();
            let (tx, rx) = mpsc::channel();
            tx.send(snapshot).unwrap();
            let mut worker = Worker {
                rx: Some(rx),
                method: InstallMethod::Other,
            };
            assert_eq!(worker.apply(&mut flash, Lang::En), expected);
            assert_eq!(flash.is_some(), expected);
            std::fs::remove_file(d.join("data/plugin-updates.toml")).unwrap();
        }
    }

    #[test]
    fn update_sparse_prefix_and_user_agent() {
        for (name, path) in [
            ("A", "1/a"),
            ("Ab", "2/ab"),
            ("ABC", "3/a/abc"),
            ("AbCd", "ab/cd/abcd"),
            ("Konoma-Foo", "ko/no/konoma-foo"),
        ] {
            assert_eq!(index_url(name), format!("https://index.crates.io/{path}"));
        }
        assert_eq!(USER_AGENT, format!("konoma/{}", env!("CARGO_PKG_VERSION")));
    }

    fn row(version: &str, req: Option<&str>, yanked: bool) -> String {
        serde_json::json!({"vers": version, "yanked": yanked, "deps": req.map(|req| vec![serde_json::json!({"name": "konoma-plugin", "req": req})]).unwrap_or_default()}).to_string()
    }

    #[test]
    fn update_index_classifies_and_prefers_compatible_once() {
        let host = semver::Version::parse("0.1.0").unwrap();
        let installed = semver::Version::parse("0.3.0").unwrap();
        let body = [
            row("1.0.0", Some("^1"), false),
            row("0.4.0", Some("^0.1"), false),
            row("0.5.0", Some("^0.1"), false),
        ]
        .join("\n");
        let got = inspect_index(&body, &installed, &host).unwrap();
        assert_eq!(got.compatible.as_deref(), Some("0.5.0"));
        assert_eq!(got.needs_host.as_deref(), Some("1.0.0"));
        assert_eq!(got.counts(), (1, 0));
        let got = inspect_index(&row("1.0.0", Some("^1"), false), &installed, &host).unwrap();
        assert_eq!(got.counts(), (0, 1));
    }

    #[test]
    fn update_index_filters_yanked_prerelease_old_and_handles_missing_sdk() {
        let host = semver::Version::parse("0.1.0").unwrap();
        let installed = semver::Version::parse("0.3.0").unwrap();
        let body = [
            row("0.2.0", None, false),
            row("0.3.0", None, false),
            row("9.0.0", None, true),
            row("10.0.0-rc.1", None, false),
        ]
        .join("\n");
        assert_eq!(
            inspect_index(&body, &installed, &host).unwrap().counts(),
            (0, 0)
        );
        let body = format!("broken\n{}", row("0.4.0", None, false));
        assert_eq!(
            inspect_index(&body, &installed, &host)
                .unwrap()
                .compatible
                .as_deref(),
            Some("0.4.0")
        );
        for bad in [
            "",
            "broken",
            "{}",
            "{\"vers\":\"0.4.0\"}",
            "{\"vers\":\"invalid\",\"deps\":[],\"yanked\":false}",
        ] {
            assert!(inspect_index(bad, &installed, &host).is_none(), "{bad}");
        }
        let renamed = r#"{"vers":"0.4.0","yanked":false,"deps":[{"name":"sdk","package":"konoma-plugin","req":"^1"}]}"#;
        assert_eq!(
            inspect_index(renamed, &installed, &host).unwrap().counts(),
            (0, 1)
        );
        assert!(inspect_index(&row("0.4.0", Some("invalid"), false), &installed, &host).is_none());
    }
}
