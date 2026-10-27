# konoma-demo-clock

A demo [konoma](https://github.com/LESIM-Co-Ltd/konoma) plugin that runs as a **program of its own**
(not published). It shows `*.konoma-demo` files as a canvas drawn with ratatui widgets: a titled
frame, a gauge that follows the seconds since the file was opened, a small picture (where the
terminal shows pixels), and a line editor (`i` types a note, `Enter` keeps it, `Esc` leaves,
`c` clears, `Left` / `Right` change the gauge colour).

```sh
cargo build -p konoma-demo-clock
konoma --plugin target/debug/konoma-demo-clock samples/demo.konoma-demo
```

- The plugin is `Clock` (`src/lib.rs`); `src/main.rs` is `konoma_plugin::export!(..)`.
- `cargo run -p konoma-demo-clock --example clock` drives it without konoma or a terminal.
- konoma's own tests (`e2e_tests::demo_clock`) load the built program, so build it first.
