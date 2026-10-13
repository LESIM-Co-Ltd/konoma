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
        let mut m = Manifest::new("hexdump", "0.1.0")
            .bind(Bind::files(["*.bin"]))
            .shows(ViewKind::Text)
            .action(width);
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
