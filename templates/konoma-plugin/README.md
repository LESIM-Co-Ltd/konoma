# {{project-name}}

A minimal konoma hex viewer. `w` switches 8/16 bytes per row, with English/Japanese hints.
The starting implementation is the workspace's `konoma-sample-hexdump`.

This template needs the **published konoma-plugin 0.1.0 SDK** (including its dependencies).
Those crates are not published yet. Until then, develop from the konoma workspace sample,
or add a local `[patch.crates-io]` for `konoma-plugin` pointing to that checkout's
`crates/konoma-plugin`. The template test uses that override offline.

```sh
cargo run -- a.bin
cargo build
konoma --plugin ./target/debug/{{project-name}} a.bin
konoma plugin install {{project-name}} --path .
```

Use your configured target directory instead of `target` if `CARGO_TARGET_DIR` is set.
`src/lib.rs` provides `plugins()` for installation into a rebuilt host; `src/main.rs` exports
the child-process and standalone entry points. Installation runs build scripts locally and
plugins run with your permissions, without a sandbox. Review the code before trusting it.

Change the file binding, session and actions to build your own plugin. Keep
`keywords = ["konoma-plugin"]` so authors can find it on crates.io. This sample reads the
whole file and formats every row on every view; add limits before using it for huge inputs.
See [Write a plugin](https://lesim-co-ltd.github.io/konoma/guides/write-a-plugin/).
