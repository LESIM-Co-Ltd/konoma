//! Plugins the host's tests register in the registry, the way a user's plugin would be. **Not
//! part of the product**: there is no way to add a plugin to a running konoma yet (stage 3).
//!
//! The registry is built once per distinct list of disabled ids (`shared_registry`), so a test
//! that wants its own plugin puts a marker name from [`register`] into `[plugins.<marker>]
//! enabled = false` of its config (`marker_config`): the registry for that list then holds the
//! plugin, as a user plugin (it wins over the standard ones), and no other test sees it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use konoma_plugin::prelude::*;

const PREFIX: &str = "+test:";

fn table() -> &'static Mutex<BTreeMap<String, Arc<dyn Plugin>>> {
    static T: OnceLock<Mutex<BTreeMap<String, Arc<dyn Plugin>>>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// The plugin registered under the marker `id`, if it is one.
pub(crate) fn lookup(id: &str) -> Option<Arc<dyn Plugin>> {
    if !id.starts_with(PREFIX) {
        return None;
    }
    table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned()
}

/// Register `plugin` under a name of the test's own; returns the marker to put in the config.
pub(crate) fn register(name: &str, plugin: Arc<dyn Plugin>) -> String {
    let marker = format!("{PREFIX}{name}");
    table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(marker.clone(), plugin);
    marker
}

/// A config in which the registry holds the plugin registered as `marker`.
pub(crate) fn marker_config(marker: &str) -> crate::config::Config {
    let mut cfg = crate::config::Config::default();
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert(marker.to_string(), t);
    cfg
}

/// What a test hexdump plugin does differently.
#[derive(Clone, Default)]
pub(crate) struct Variant {
    /// Declare a second action on `j`, a key the host keeps for itself, and one on `x`.
    pub extra_acts: bool,
    /// `width` works only on files longer than this many bytes.
    pub gate_len: usize,
    /// Declare the `width` action as writing files.
    pub writes: bool,
    /// Counts the `act` calls that reached the session.
    pub calls: Arc<AtomicUsize>,
    /// Do not declare `shows` (the hello world as it was first written), under the id
    /// `hexdump-ns` so the declaration table of the other tests is not touched.
    pub no_shows: bool,
    /// Declare `shows(Text)` but answer with a table (under the id `hexdump-wrong`).
    pub wrong_view: bool,
    /// Keeps the `width` action on the worker until the test opens the latch, so a test that looks
    /// at the screen while the job is out does not race the worker (the host takes an answer that
    /// is already there, so a job that finishes at once is not "running" for long enough to see).
    pub hold: Option<Arc<Latch>>,
    /// Like `hold`, but for a session on a file shorter than 10 bytes (`small.bin`): a test that
    /// has two actions out at once, on two files, lets them end in the order it chooses.
    pub hold_small: Option<Arc<Latch>>,
    /// The `edit` / `open` verbs the plugin declares (none by default).
    pub verbs: Vec<konoma_plugin::VerbDecl>,
}

/// A gate a test opens by hand: [`Latch::wait`] returns once [`Latch::open`] was called (or after a
/// long bound, so a test that forgets to open it fails instead of hanging).
#[derive(Default)]
pub(crate) struct Latch {
    opened: Mutex<bool>,
    cv: std::sync::Condvar,
}

impl Latch {
    pub(crate) fn open(&self) {
        *self.opened.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.cv.notify_all();
    }

    pub(crate) fn is_open(&self) -> bool {
        *self.opened.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn wait(&self) {
        let guard = self.opened.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self
            .cv
            .wait_timeout_while(guard, std::time::Duration::from_secs(120), |open| !*open);
    }
}

/// The hello-world plugin of the design (4-10), plus what the host's tests need around it.
pub(crate) struct Hexdump(pub Variant);

impl Plugin for Hexdump {
    fn manifest(&self) -> Manifest {
        let mut width = Act::new("width")
            .key("w")
            .label(t!(en: "width", ja: "幅"))
            .help(t!(en: "Switch 8/16 bytes per row", ja: "1 行のバイト数を 8/16 で切り替え"))
            .footer(1);
        if self.0.writes {
            width = width.writes();
        }
        let id = if self.0.no_shows {
            "hexdump-ns"
        } else if self.0.wrong_view {
            "hexdump-wrong"
        } else {
            "hexdump"
        };
        let mut m = Manifest::new(id, "0.1.0")
            .bind(Bind::files(["*.bin"]))
            .action(width);
        if !self.0.no_shows {
            m = m.shows(ViewKind::Text);
        }
        for v in &self.0.verbs {
            m = m.verb(v.clone());
        }
        if self.0.extra_acts {
            m = m
                .action(
                    Act::new("jump")
                        .key("j")
                        .label(t!(en: "jump", ja: "移動"))
                        .help(t!(en: "Jump to the next block", ja: "次のブロックへ"))
                        .footer(2),
                )
                .action(
                    Act::new("dump")
                        .key("x")
                        .label(t!(en: "dump", ja: "出力"))
                        .help(t!(en: "Dump to a file", ja: "ファイルに出力")),
                );
        }
        m
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Dump {
            bytes: input.read_all()?,
            width: 16,
            v: self.0.clone(),
        }))
    }
}

struct Dump {
    bytes: Vec<u8>,
    width: usize,
    v: Variant,
}

impl Session for Dump {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        if self.v.wrong_view {
            return Ok(View::Table(konoma_plugin::view::TableView {
                grid: Arc::new(super::test_views::Cells {
                    rows: vec![vec!["a".into()]],
                    ncols: 1,
                    first_row_is_header: false,
                }),
                title: None,
                rainbow: false,
                addressed: false,
            }));
        }
        Ok(View::text(self.bytes.chunks(self.width).map(|chunk| {
            chunk
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        })))
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.v.calls.fetch_add(1, Ordering::SeqCst);
        match action {
            "width" => {
                let held = if self.bytes.len() < 10 {
                    self.v.hold_small.as_ref()
                } else {
                    self.v.hold.as_ref()
                };
                if let Some(latch) = held {
                    latch.wait();
                }
                self.width = if self.width == 16 { 8 } else { 16 };
                Ok(Outcome::Redraw)
            }
            "jump" | "dump" => Ok(Outcome::Flash(Tr::new(format!("did {action}")))),
            _ => Ok(Outcome::Unhandled),
        }
    }

    fn enabled(&self, action: &str) -> bool {
        match action {
            "width" => self.bytes.len() > self.v.gate_len,
            _ => true,
        }
    }
}
