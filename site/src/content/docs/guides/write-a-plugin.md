---
title: Write a plugin
description: Build a small binary-file viewer with the Rust SDK, test it, and install it into konoma.
sidebar:
  order: 6
---

A plugin declares what it opens and returns display data; konoma draws it. This guide uses the
real `crates/konoma-sample-hexdump` sample: `*.bin` becomes offset, hex and ASCII columns, and
<kbd>w</kbd> switches 16/8 bytes per row. It reads the whole file and formats all rows on each
view, so it is a teaching example for small files.

## Start with the template

You need Rust 1.92 or later and an available `cargo generate` command. From a konoma checkout:

```sh
cargo generate --path templates/konoma-plugin --name my-hexdump
cd my-hexdump
```

The template sets `keywords = ["konoma-plugin"]` for discovery on crates.io, a version dependency
on `konoma-plugin = "0.1.0"`, and the SDK's `app` feature (standalone terminal plus protocol).
**The SDK crates are not published yet.** The version-based template needs the published SDK;
for development now, use the workspace sample below, or override `konoma-plugin` with a local
`[patch.crates-io]` pointing to the checkout's `crates/konoma-plugin`. The template test does
that offline, using the workspace's dependency lock and local mermaid patch.

Once the SDK is available (or locally overridden), inside the generated project:

```sh
cargo run -- a.bin
cargo build
konoma --plugin ./target/debug/my-hexdump a.bin
```

Use an existing `.bin` file. These commands use Cargo's default `target` directory; if you set
`CARGO_TARGET_DIR`, use its binary path instead. `cargo run -- a.bin` opens the SDK's standalone
terminal. In konoma, the plugin supplies the text and the host supplies scrolling, search,
selection and copy. <kbd>w</kbd> runs the declared `width` action; <kbd>?</kbd> shows its help.

## Manifest, open and session

Here is the sample's complete `src/lib.rs`, copied verbatim. The generated library starts with
the same code; its id/version come from its own Cargo package, so `my-hexdump` becomes the id.

```rust
use konoma_plugin::prelude::*;

pub struct Hexdump;

impl Plugin for Hexdump {
    fn manifest(&self) -> Manifest {
        Manifest::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Hex dump", ja: "16進ダンプ"))
            .bind(Bind::files(["*.bin"]))
            .shows(ViewKind::Text)
            .action(
                Act::new("width")
                    .key("w")
                    .label(t!(en: "width", ja: "幅"))
                    .help(t!(en: "Switch 8/16 bytes per row", ja: "1行のバイト数を8/16に切り替え"))
                    .footer(1),
            )
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(
                t!(en: "Expected a file", ja: "ファイルが必要です"),
            ));
        };
        Ok(Box::new(HexSession {
            bytes: std::fs::read(path)?,
            width: 16,
        }))
    }
}

struct HexSession {
    bytes: Vec<u8>,
    width: usize,
}

impl Session for HexSession {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let lines = self
            .bytes
            .chunks(self.width)
            .enumerate()
            .map(|(row, bytes)| {
                let hex = bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let ascii: String = bytes
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                format!(
                    "{:08x}  {hex:<pad$}  |{ascii}|",
                    row * self.width,
                    pad = self.width * 3 - 1
                )
            })
            .collect::<Vec<_>>();
        Ok(View::text(lines))
    }

    fn act(&mut self, action: &str, _cx: &ActCx) -> Result<Outcome, Failure> {
        if action != "width" {
            return Ok(Outcome::Unhandled);
        }
        self.width = if self.width == 16 { 8 } else { 16 };
        Ok(Outcome::Redraw)
    }
}

pub fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![Box::new(Hexdump)]
}
```

`Manifest::bind` assigns files; `shows` tells the host which display type and reserved keys apply.
Declare both. `Act` declares the action id, default key, short label, help and footer priority;
`t!` carries English and Japanese, with English fallback. The host uses this declaration for
keys, footer, help and the <kbd>Space</kbd> menu. `Session::enabled` defaults to true; override
it when an action is temporarily unavailable so the key and hint agree.

`open` rejects non-file inputs and prepares a session. The session owns file bytes and the current
row width. `view` reads that state without advancing it; resize/redraw can request a view at any
time. `View::text` returns ready-made lines, leaving drawing to the host. A short last row keeps
the ASCII column aligned, non-printable bytes become `.`, and an empty file has no lines.
`act` returns `Redraw` after changing state and `Unhandled` for unknown ids. This sample does
not ask for raw key input: konoma maps <kbd>w</kbd> to `act("width", ...)`.

## Two entry points

The sample's `src/main.rs` is exactly:

```rust
konoma_plugin::export!(@program konoma_sample_hexdump::plugins());
```

The generated main uses `my_hexdump::plugins()` instead. `export!` provides `--serve`, `--manifest`,
`--version`, `--help` and, with `app`, the file argument for the standalone terminal. Never write
unframed data to the protocol stdout; the SDK redirects ordinary prints to stderr when serving.
The public library function `plugins() -> Vec<Box<dyn Plugin>>` serves a different consumer:
`konoma plugin install` builds a host whose generated code calls `<crate>::plugins()`. Keep the
library entry as well as the program entry. Installing a Rust crate links it into the rebuilt
host; loading a program with `--plugin` uses a child process.

To try the actual sample from the checkout, using a chosen build directory:

```sh
cargo build -p konoma-sample-hexdump
cargo run -p konoma-sample-hexdump -- a.bin
konoma --plugin "$CARGO_TARGET_DIR/debug/konoma-sample-hexdump" a.bin
cargo run -p konoma-sample-hexdump -- --manifest
```

Set `CARGO_TARGET_DIR` to your build directory for these path examples (otherwise use `target`).
The development `--plugin` option is repeatable and lasts for that konoma run only.

## Test without a terminal

The sample's `tests/hexdump.rs` is copied below. `open_bytes` creates an isolated scratch file;
`open_file` exercises open on its path. `view_text` extracts the displayed lines. `send_keys`
calls raw `Session::key` directly: it does **not** resolve manifest bindings. This text plugin
leaves raw keys unhandled, so the declared-key test resolves the declaration and calls `act`.
`check_manifest` checks keys and missing `shows`; `check_hints` checks the host's derived hints.

```rust
use konoma_plugin::prelude::*;
use konoma_plugin::testing::{check_hints, check_manifest, open_bytes, send_keys, view_text};
use konoma_sample_hexdump::Hexdump;

#[test]
fn hex_lines_include_a_short_final_row() {
    let (mut session, _file) =
        open_bytes(&Hexdump, "a.bin", b"ABCDEFGHIJKLMNOP\0\x7f\xff ").unwrap();
    assert_eq!(
        view_text(&mut *session, &ViewReq::new(100, 20)).unwrap(),
        vec![
            "00000000  41 42 43 44 45 46 47 48 49 4a 4b 4c 4d 4e 4f 50  |ABCDEFGHIJKLMNOP|",
            "00000010  00 7f ff 20                                      |... |",
        ]
    );
}

#[test]
fn empty_file_has_no_rows() {
    let (mut session, _file) = open_bytes(&Hexdump, "empty.bin", b"").unwrap();
    assert!(view_text(&mut *session, &ViewReq::new(100, 20))
        .unwrap()
        .is_empty());
}

#[test]
fn width_action_toggles_and_redraws() {
    let (mut session, _file) = open_bytes(&Hexdump, "a.bin", b"ABCDEFGHIJKLMNOP").unwrap();
    let manifest = Hexdump.manifest();
    let action = manifest
        .actions
        .iter()
        .find(|a| a.key.as_deref() == Some("w"))
        .unwrap();
    assert_eq!(action.id, "width");
    let req = ViewReq::new(100, 20);
    let before = view_text(&mut *session, &req).unwrap();
    assert!(matches!(
        session.act(&action.id, &ActCx::bare()).unwrap(),
        Outcome::Redraw
    ));
    assert_eq!(
        view_text(&mut *session, &req).unwrap(),
        vec![
            "00000000  41 42 43 44 45 46 47 48  |ABCDEFGH|",
            "00000008  49 4a 4b 4c 4d 4e 4f 50  |IJKLMNOP|",
        ]
    );
    assert!(matches!(
        session.act("width", &ActCx::bare()).unwrap(),
        Outcome::Redraw
    ));
    assert_eq!(view_text(&mut *session, &req).unwrap(), before);
}

#[test]
fn unknown_action_is_unhandled() {
    let (mut session, _file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    assert!(matches!(
        session.act("unknown", &ActCx::bare()).unwrap(),
        Outcome::Unhandled
    ));
}

#[test]
fn manifest_and_hints_are_clean() {
    let (session, _file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    assert!(check_manifest(&Hexdump).is_empty());
    assert!(check_hints(&Hexdump, &*session).is_clean());
    let action = &Hexdump.manifest().actions[0];
    assert_eq!(action.label.get("en"), "width");
    assert_eq!(action.label.get("ja"), "幅");
    assert_eq!(action.help.get("en"), "Switch 8/16 bytes per row");
    assert_eq!(action.help.get("ja"), "1行のバイト数を8/16に切り替え");
}

#[test]
fn sdk_helpers_open_view_and_send_raw_keys() {
    let (mut session, file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    let mut reopened = konoma_plugin::testing::open_file(&Hexdump, file.path()).unwrap();
    assert_eq!(
        view_text(&mut *reopened, &ViewReq::new(100, 20))
            .unwrap()
            .len(),
        1
    );
    let outcomes = send_keys(&mut *session, &[Key::char('x')])
        .outcomes()
        .unwrap();
    assert!(matches!(outcomes.as_slice(), [Outcome::Unhandled]));
}
```

From the checkout, run the sample tests (including a real-program protocol test and template test):

```sh
cargo nextest run -p konoma-sample-hexdump
cargo run -p konoma-plugin-test -- --file a.bin "$CARGO_TARGET_DIR/debug/konoma-sample-hexdump"
```

`konoma-plugin-test` speaks the child protocol and uses konoma's normal deadlines. It probes
`--serve`; it is not a JSON manifest linter. Pass a file to exercise open/view/actions/hints/
cancellation too; without a file those checks are marked SKIP. Its library entry is
`run(&Options, Limits::default())`, used by the sample's real-program test. See the
[protocol specification](../../reference/plugin-protocol/) for exact checks and wire formats.

## Install and share

After reviewing the source, install the generated crate from its directory:

```sh
konoma plugin install my-hexdump --path .
konoma plugin list
```

For the workspace sample, run from the checkout instead:

```sh
konoma plugin install konoma-sample-hexdump --path ./crates/konoma-sample-hexdump
```

The CLI requires the crate name even with `--path`. It describes the source/authors and asks
`Install it? [y/N]`. Build scripts run on the user's machine during the rebuild, and plugins run
with the user's rights, **without a sandbox**, whether linked or started as children. Install
only code you trust. `--yes` skips the prompt; without a terminal the CLI requires it.
Installation needs the matching konoma library and SDK dependencies to be available; a local
plugin path alone does not publish those dependencies.

After publishing your own crate, users can install it by crate name (optionally `@version`).
Keep the `konoma-plugin` crates.io keyword. `konoma plugin update my-hexdump` rebuilds with a
newer compatible release; `konoma plugin remove my-hexdump` removes it and rebuilds;
`konoma plugin clean` deletes the retained build cache. `list` shows installed crates and sources.
These are author/user instructions; the tests do not install into your real configuration.

To replace a standard viewer, give your own plugin a distinct id and suitable binding/display
declarations, then turn off the standard plugin in your config. For example, a replacement
CSV viewer would disable `csv`:

```toml
[plugins.csv]
enabled = false
```

See [configuration](../../reference/configuration/) for `[plugins.<id>]`, `read_only`, plugin
settings and key overrides. A program can also be configured directly, for example:

```toml
[plugins.hex]
command = "/absolute/path/to/viewer"
args = ["--plugin", "hex"]
read_dirs = ["/absolute/path/to/assets"]
```

This example selects id `hex` from a program and permits the extra asset directory. Use absolute
paths in `args` for scripts/files because the child runs in a private working directory.
Read scopes limit what the host
reads for a plugin; they do not sandbox a plugin's own filesystem access. Do not reuse a
standard plugin id to replace it.

Rust is optional. A text-only program in another language starts with the host's Hello and its
Ack/manifest, then handles Open, View and Shutdown; it must also answer unsupported Act/Key
calls and handle Close/Cancel rather than silently ignoring requests. Follow the protocol's
[minimal loop for implementers](../../reference/plugin-protocol/#minimal-loop), including EOF
and invalid-frame handling. The manifest needs `id`, `version`, `api`, `name`, `binds`, `actions`,
`verbs`, `settings`, `screens`, `shows`, `converts`, `file_text`, `sized` and `unavailable`;
`work_group` is nullable. Empty unused declaration arrays and `shows:["Text"]` suffice for the
minimal file viewer with a file binding. Check it with `konoma-plugin-test`, then load it through
`--plugin` or `[plugins.<id>] command`; the Rust `plugins()` installation entry is for Rust crates.

To reproduce the sample without reading Rust: start with 16 bytes per row, with <kbd>w</kbd>
switching to 8 and back. Each line has the byte offset in lowercase hex, zero-padded to at least
8 digits, two spaces, lowercase two-digit hex bytes separated by one space, then enough trailing
spaces to make that hex column `width*3-1` characters wide, two more spaces, and the ASCII column
between `|` characters. Preserve bytes `0x21` through `0x7e` and the space byte `0x20`; every
other byte becomes `.`. The final short row uses the same hex-column padding without padding
inside the ASCII bars. An empty file returns no lines. A plain line becomes a `Lines` source
with one span and default span/line styles, as shown in the protocol exchange.
