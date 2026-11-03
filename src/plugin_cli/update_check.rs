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

    /// Called only on the UI thread; uses the footer width cached by startup or the last draw.
    pub(crate) fn apply(&mut self, app: &mut crate::App) -> bool {
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
        let Some(note) = notice(compatible, needs_host, app.lang, self.method) else {
            return false;
        };
        app.append_flash_notice(note);
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
        let saved: CheckTime = toml::from_str(&text).ok()?;
        Some(Self {
            checked_at: saved.checked_at,
            host_version: String::new(),
            plugins: BTreeMap::new(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CheckTime {
    checked_at: u64,
}

fn standard_targets() -> BTreeMap<String, (Version, VersionReq)> {
    let manifest: toml::Table = include_str!("../../Cargo.toml")
        .parse()
        .expect("package manifest");
    [
        ("konoma-code", konoma_code::code_manifest().version),
        (
            "konoma-markdown",
            konoma_markdown::markdown_manifest().version,
        ),
        ("konoma-math", konoma_math::math_manifest().version),
        ("konoma-media", konoma_media::image_manifest().version),
        ("konoma-mermaid", konoma_mermaid::mermaid_manifest().version),
        (
            "konoma-office",
            konoma_office::spreadsheet_manifest().version,
        ),
        ("konoma-table", konoma_table::csv_manifest().version),
    ]
    .into_iter()
    .filter_map(|(name, version)| {
        Some((
            name.into(),
            (
                Version::parse(&version).ok()?,
                VersionReq::parse(manifest["dependencies"][name]["version"].as_str()?).ok()?,
            ),
        ))
    })
    .collect()
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
    let config_dir = config_dir.to_path_buf();
    let (tx, rx) = mpsc::channel();
    spawn(Box::new(move || {
        // A separate, stable lock file protects the attempt claim across concurrent starts.
        // Release it before HTTP: the timestamp, including failures, enforces the daily limit.
        if std::fs::create_dir_all(&data_dir).is_err() {
            return;
        }
        let Ok(claim) = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(data_dir.join("plugin-updates.lock"))
        else {
            return;
        };
        if claim.try_lock().is_err() {
            return;
        }
        let checked_at = now();
        if let Some(saved) =
            Snapshot::load(&data_dir).filter(|s| checked_at.saturating_sub(s.checked_at) < 86400)
        {
            let _ = tx.send(saved);
            return;
        }
        let Ok(list) = PluginList::load(&config_dir.join("plugins.toml")) else {
            return;
        };
        let lock_text = std::fs::read_to_string(data_dir.join("Cargo.lock"));
        if lock_text
            .as_ref()
            .is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
        {
            return;
        }
        let locked = project::locked_versions(lock_text.as_deref().unwrap_or_default());
        let mut targets: BTreeMap<_, _> = list
            .plugins
            .iter()
            .filter_map(|(name, source)| {
                let super::list_file::Source::Crates { version } = source else {
                    return None;
                };
                Some((
                    name.clone(),
                    (
                        Version::parse(locked.get(name)?).ok()?,
                        VersionReq::parse(version).ok()?,
                    ),
                ))
            })
            .collect();
        for (name, (compiled, req)) in standard_targets() {
            let installed = if lock_text.is_ok() {
                locked.get(&name).and_then(|v| Version::parse(v).ok())
            } else {
                Some(compiled)
            };
            if let Some(installed) = installed {
                targets.insert(name, (installed, req));
            }
        }
        if targets.is_empty() {
            return;
        }
        let Ok(text) = toml::to_string(&CheckTime { checked_at }) else {
            return;
        };
        if super::list_file::write_atomic(&data_dir.join("plugin-updates.toml"), text.as_bytes())
            .is_err()
        {
            return;
        }
        drop(claim);
        let mut plugins = BTreeMap::new();
        for (name, (installed, requirement)) in targets {
            if let Some(updates) = fetch(&index_url(&name))
                .and_then(|body| inspect_index_in_range(&body, &installed, &host, &requirement))
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
        if plugins.is_empty() {
            return;
        }
        let snapshot = Snapshot {
            checked_at,
            host_version: host.to_string(),
            plugins,
        };
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

#[cfg(test)]
fn inspect_index(body: &str, installed: &Version, host: &Version) -> Option<Updates> {
    inspect_index_in_range(body, installed, host, &VersionReq::STAR)
}

fn inspect_index_in_range(
    body: &str,
    installed: &Version,
    host: &Version,
    requirement: &VersionReq,
) -> Option<Updates> {
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
        if row.yanked
            || !version.pre.is_empty()
            || version <= *installed
            || !requirement.matches(&version)
        {
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

    fn flash_app(d: &Path, lang: Lang) -> crate::App {
        let cfg =
            toml::from_str("[external]\ngit = false\n[plugins.markdown]\nunknown = 1\n").unwrap();
        let bookmarks = crate::bookmarks::Bookmarks::with_base(d.join("bookmarks"), d);
        let mut app = crate::App::new_with_bookmarks(d.to_path_buf(), cfg, bookmarks).unwrap();
        app.lang = lang;
        app
    }

    fn deliver_latched(app: &mut crate::App, snapshot: Snapshot, width: usize) {
        app.refit_notice_flash(width);
        let (release, wait) = mpsc::channel();
        let (done, received) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let sender = std::thread::spawn(move || {
            wait.recv().unwrap();
            tx.send(snapshot).unwrap();
            done.send(()).unwrap();
        });
        let mut worker = Worker {
            rx: Some(rx),
            method: InstallMethod::Other,
        };
        let before = app.flash.clone();
        assert!(!worker.apply(app));
        assert_eq!(app.flash, before);
        release.send(()).unwrap();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        sender.join().unwrap();
        assert!(worker.apply(app));
    }

    #[test]
    fn update_late_flash_refits_folded_startup_counts_and_current_width() {
        use unicode_width::UnicodeWidthStr;
        let d = fixture();
        let snapshot = check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
        for lang in [Lang::En, Lang::Jp] {
            for (initial, current) in [(45, 45), (45, 4000), (4000, 45)] {
                let mut app = flash_app(&d, lang);
                let error = if lang == Lang::En {
                    "config error"
                } else {
                    "設定エラー"
                };
                let mut all = vec![error.to_string()];
                all.extend(app.cfg.plugin_report(lang));
                app.set_startup_flash(Some(error.into()), initial);
                if initial == 45 {
                    assert!(app
                        .flash
                        .as_ref()
                        .unwrap()
                        .contains(&tr(lang, Msg::KeymapMore).replace("{n}", "1")));
                }
                all.push(notice(1, 0, lang, InstallMethod::Other).unwrap());
                deliver_latched(&mut app, snapshot.clone(), current);
                let expected = if current == 45 {
                    format!("{error} {}", tr(lang, Msg::KeymapMore).replace("{n}", "2"))
                } else {
                    all.join(" / ")
                };
                assert_eq!(app.flash.as_deref(), Some(expected.as_str()));
                assert!(app.flash.as_ref().unwrap().width() <= current);
                assert!(app.flash.as_ref().unwrap().starts_with(error));
            }
        }
    }

    #[test]
    fn update_late_flash_without_startup_is_bounded_in_both_languages() {
        use unicode_width::UnicodeWidthStr;
        let d = fixture();
        let snapshot = check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
        for lang in [Lang::En, Lang::Jp] {
            for width in [0, 1, 2, 12, 45, 4000] {
                let mut app = flash_app(&d, lang);
                app.set_startup_flash(Some("obsolete startup error".into()), 4000);
                app.flash = None;
                deliver_latched(&mut app, snapshot.clone(), width);
                let flash = app.flash.as_ref().unwrap();
                assert!(flash.width() <= width, "{width}: {flash}");
                assert!(!flash.contains("obsolete"));
                assert_eq!(
                    flash,
                    &crate::ui::status::truncate_display(
                        &notice(1, 0, lang, InstallMethod::Other).unwrap(),
                        width
                    )
                );
            }
        }
    }

    #[test]
    fn update_late_flash_preserves_replacement_without_reviving_startup() {
        use unicode_width::UnicodeWidthStr;
        let d = fixture();
        let snapshot = check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
        for lang in [Lang::En, Lang::Jp] {
            let mut app = flash_app(&d, lang);
            app.set_startup_flash(Some("obsolete startup error".into()), 45);
            let current = if lang == Lang::En {
                "copy failed"
            } else {
                "コピー失敗"
            };
            app.flash = Some(current.into());
            deliver_latched(&mut app, snapshot.clone(), 45);
            assert_eq!(
                app.flash,
                Some(format!(
                    "{current} {}",
                    tr(lang, Msg::KeymapMore).replace("{n}", "1")
                ))
            );
            let flash = app.flash.as_ref().unwrap();
            assert!(flash.width() <= 45);
            assert!(flash.starts_with(current));
            assert!(!flash.contains("obsolete"));
        }
    }

    #[test]
    fn update_late_flash_uses_current_footer_area_across_resizes() {
        use unicode_width::UnicodeWidthStr;
        let d = fixture();
        let snapshot = check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
        for lang in [Lang::En, Lang::Jp] {
            for layout in ["split", "bottom"] {
                let mut app = flash_app(&d, lang);
                app.cfg.ui.statusbar = layout.into();
                let mut all = vec!["config error".to_string()];
                all.extend(app.cfg.plugin_report(lang));
                let narrow = (all.join(" / ").width()
                    + 1
                    + tr(lang, Msg::KeymapMore).replace("{n}", "1").width()
                    + 2) as u16;
                all.push(notice(1, 0, lang, InstallMethod::Other).unwrap());
                app.set_startup_flash(Some("config error".into()), 400);
                deliver_latched(&mut app, snapshot.clone(), 400);
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(400, 12)).unwrap();
                for cols in [narrow, 400] {
                    terminal.backend_mut().resize(cols, 12);
                    terminal
                        .draw(|frame| crate::ui::render(frame, &mut app))
                        .unwrap();
                    let available = if layout == "split" {
                        cols
                    } else {
                        cols.saturating_sub(crate::ui::status::context_width(&app) + 2)
                    };
                    let expected = if cols == 400 {
                        all.join(" / ")
                    } else if layout == "split" {
                        format!(
                            "{} {}",
                            all[..2].join(" / "),
                            tr(lang, Msg::KeymapMore).replace("{n}", "1")
                        )
                    } else {
                        format!(
                            "config error {}",
                            tr(lang, Msg::KeymapMore).replace("{n}", "2")
                        )
                    };
                    assert_eq!(
                        app.flash.as_deref(),
                        Some(expected.as_str()),
                        "{layout}, {cols}"
                    );
                    assert!(app.flash.as_ref().unwrap().width() <= usize::from(available));
                }
                app.flash = None;
                terminal
                    .draw(|frame| crate::ui::render(frame, &mut app))
                    .unwrap();
                assert!(app.flash.is_none());
            }
        }
    }

    #[test]
    fn update_late_flash_long_current_keeps_count_inside_width() {
        use unicode_width::UnicodeWidthStr;
        let d = fixture();
        let snapshot = check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
        for lang in [Lang::En, Lang::Jp] {
            let mut app = flash_app(&d, lang);
            app.flash = Some(format!("error: {}", "長い説明".repeat(30)));
            deliver_latched(&mut app, snapshot.clone(), 45);
            let flash = app.flash.as_ref().unwrap();
            assert!(flash.width() <= 45);
            assert!(flash.starts_with("error: "));
            assert!(flash.ends_with(&tr(lang, Msg::KeymapMore).replace("{n}", "1")));
        }
    }

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

    #[test]
    fn update_standard_build_without_a_list_checks_standard_crates() {
        let d = fixture();
        std::fs::remove_file(d.join("config/plugins.toml")).unwrap();
        std::fs::remove_file(d.join("data/Cargo.lock")).unwrap();
        let snapshot = check(&d, 100, |url| {
            url.ends_with("/konoma-office")
                .then(|| row("0.1.1", Some("^0.1"), false))
        })
        .expect("the standard build has updatable plugins");
        assert_eq!(
            snapshot.plugins["konoma-office"]
                .updates
                .compatible
                .as_deref(),
            Some("0.1.1")
        );
    }

    #[test]
    fn update_file_reads_are_deferred_to_the_worker() {
        let d = fixture();
        let path = d.join("config/plugins.toml");
        std::fs::remove_file(&path).unwrap();
        let cpath = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: this test owns the FIFO and the string is NUL-terminated.
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        let config = d.join("config");
        let data = d.join("data");
        let (tx, rx) = mpsc::channel();
        let launcher = std::thread::spawn(move || {
            let mut queued = None;
            let result = start_with(
                true,
                &config,
                data,
                host(),
                || 100,
                |_| None,
                |job| {
                    queued = Some(job);
                    true
                },
            );
            tx.send((result.is_some(), queued)).unwrap();
        });
        let returned = rx.recv_timeout(Duration::from_secs(2));
        // Release an old implementation blocked on the caller; the watchdog cannot leak it.
        if returned.is_err() {
            std::fs::write(&path, "").unwrap();
        }
        launcher.join().unwrap();
        assert!(
            returned
                .expect("startup must return before any file read")
                .0
        );
    }

    #[test]
    fn update_respects_the_configured_semver_range() {
        for requirement in ["=0.3.0", "^0.3"] {
            let d = fixture();
            std::fs::write(
                d.join("config/plugins.toml"),
                format!("[plugins.foo]\nversion = {requirement:?}\n"),
            )
            .unwrap();
            let snapshot = check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
            assert_eq!(
                snapshot.plugins["foo"].updates.counts(),
                (0, 0),
                "{requirement}"
            );
        }
    }

    #[test]
    fn update_failed_attempt_consumes_the_daily_limit() {
        let d = fixture();
        assert!(check(&d, 100, |_| None).is_none());
        let snapshot = check(&d, 101, |_| panic!("a failed attempt still counts"));
        assert!(snapshot.is_none_or(|s| s.plugins.is_empty()));
    }

    #[test]
    fn update_concurrent_starts_share_one_attempt() {
        let d = fixture();
        let (entered, entering) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let first = start_with(
            true,
            &d.join("config"),
            d.join("data"),
            host(),
            || 100,
            move |_| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                None
            },
            spawn_worker,
        )
        .unwrap();
        entering.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check(&d, 100, |_| {
                panic!("another startup already claimed today's attempt")
            })
        }));
        release.send(()).unwrap();
        assert!(first.recv_timeout(Duration::from_secs(5)).is_err());
        assert!(second.unwrap().is_none_or(|s| s.plugins.is_empty()));
    }

    #[test]
    fn update_saves_only_the_attempt_timestamp() {
        let d = fixture();
        check(&d, 100, |_| Some(row("0.4.0", None, false))).unwrap();
        let saved: toml::Table = std::fs::read_to_string(d.join("data/plugin-updates.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(saved.len(), 1, "{saved:?}");
        assert_eq!(saved["checked_at"].as_integer(), Some(100));
    }

    #[test]
    fn update_claim_is_exclusive_before_reading_the_clock() {
        let d = fixture();
        let held = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(d.join("data/plugin-updates.lock"))
            .unwrap();
        held.lock().unwrap();
        let rx = start_with(
            true,
            &d.join("config"),
            d.join("data"),
            host(),
            || panic!("another startup owns the attempt claim"),
            |_| panic!("no HTTP"),
            |job| {
                job();
                true
            },
        )
        .unwrap();
        assert!(rx.recv().is_err());
        drop(held);
        assert!(check(&d, 100, |_| Some(row("0.4.0", None, false))).is_some());
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
            |job| {
                job();
                true
            },
        )
        .unwrap()
        .recv()
        .unwrap();
        assert_eq!(cached.checked_at, snapshot.checked_at);
        assert!(
            cached.plugins.is_empty(),
            "observations are never persisted"
        );
        let fresh = check(&d, 100 + 86400, |_| Some(row("0.5.0", None, false))).unwrap();
        assert_eq!(fresh.checked_at, 100 + 86400);
        assert_eq!(
            fresh.plugins["foo"].updates.compatible.as_deref(),
            Some("0.5.0")
        );
        assert_eq!(
            Snapshot::load(&d.join("data")).unwrap().checked_at,
            fresh.checked_at
        );
        assert!(Snapshot::load(&d.join("data")).unwrap().plugins.is_empty());
    }

    #[test]
    fn update_disabled_never_spawns_and_unbuilt_targets_never_fetch() {
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
                |job| {
                    job();
                    true
                }
            )
            .unwrap()
            .recv()
            .is_err());
        }
    }

    #[test]
    fn update_failures_are_silent_and_corrupt_timestamps_are_replaced() {
        for response in [None, Some("HTTP 503"), Some("broken index"), Some("")] {
            let d = fixture();
            assert!(check(&d, 90000, move |_| response.map(str::to_owned)).is_none());
            let saved = Snapshot::load(&d.join("data")).unwrap();
            assert_eq!(saved.checked_at, 90000);
            assert!(saved.plugins.is_empty());
            assert!(check(&d, 90001, |_| panic!("failed attempts count"))
                .unwrap()
                .plugins
                .is_empty());
        }
        let d = fixture();
        std::fs::write(d.join("data/plugin-updates.toml"), "broken = [").unwrap();
        assert!(Snapshot::load(&d.join("data")).is_none());
        assert!(check(&d, 101, |_| None).is_none());
        assert_eq!(Snapshot::load(&d.join("data")).unwrap().checked_at, 101);
        assert_eq!(
            check(&d, 102, |_| Some(row("0.3.0", None, false)))
                .unwrap()
                .checked_at,
            101
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
            |job| {
                job();
                true
            },
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
        let d = fixture();
        assert!(!worker.apply(&mut flash_app(&d, Lang::En)));
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
        app.set_startup_flash(Some(format!("config error / {stale}")), 4000);
        let before = app.flash.clone().unwrap();
        assert!(!updates.apply(&mut app));
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
        app.refit_notice_flash(4000);
        app.flash = Some(before.clone());
        assert!(updates.apply(&mut app));
        assert_eq!(
            app.flash.as_deref(),
            Some(format!("{before} / 1 plugin update available (`konoma plugin update`)").as_str())
        );
        assert!(!updates.apply(&mut app));
        assert!(app.flash.unwrap().contains(&stale));
    }

    #[test]
    fn update_worker_emits_to_empty_flash_but_says_nothing_for_zero() {
        let d = fixture();
        let mut app = flash_app(&d, Lang::En);
        for (version, expected) in [("0.3.0", false), ("0.4.0", true)] {
            let snapshot = check(&d, 100000, move |_| Some(row(version, None, false))).unwrap();
            let (tx, rx) = mpsc::channel();
            tx.send(snapshot).unwrap();
            let mut worker = Worker {
                rx: Some(rx),
                method: InstallMethod::Other,
            };
            assert_eq!(worker.apply(&mut app), expected);
            assert_eq!(app.flash.is_some(), expected);
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
