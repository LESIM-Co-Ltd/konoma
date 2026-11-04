# konoma-sample-hexdump

A minimal [konoma](https://github.com/LESIM-Co-Ltd/konoma) plugin (not published).
It shows `*.bin` files as offset, hex bytes and printable ASCII (`.` for other bytes).
`w` switches between 16 and 8 bytes per row; labels and help are English and Japanese.
It reads the file into memory and builds the whole text on each view; it is a teaching sample,
not a streaming viewer for large files.

```sh
cargo build -p konoma-sample-hexdump
cargo run -p konoma-sample-hexdump -- a.bin
konoma --plugin "$CARGO_TARGET_DIR/debug/konoma-sample-hexdump" a.bin
cargo run -p konoma-plugin-test -- --file a.bin "$CARGO_TARGET_DIR/debug/konoma-sample-hexdump"
konoma plugin install konoma-sample-hexdump --path ./crates/konoma-sample-hexdump
```

Set `CARGO_TARGET_DIR` to your build directory (otherwise Cargo uses `target`).
`Hexdump` and `plugins()` live in `src/lib.rs`; `src/main.rs` uses `export!` for the
standalone terminal and child-process entry points. Installation links the library's `plugins()`
into a rebuilt host. See [Write a plugin](https://lesim-co-ltd.github.io/konoma/guides/write-a-plugin/)
and the `templates/konoma-plugin` cargo-generate template.
