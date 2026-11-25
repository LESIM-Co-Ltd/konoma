//! The clock demo as a plugin program: `konoma --plugin target/debug/konoma-demo-clock
//! samples/demo.konoma-demo`. `export!` emits a `main` that serves konoma on stdin and stdout
//! (`--serve`) and answers `--manifest`, `--version` and `--help`.

konoma_plugin::export!(konoma_demo_clock::Clock::default());
