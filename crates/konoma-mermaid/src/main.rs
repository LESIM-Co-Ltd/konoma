//! `konoma-mermaid <file>`: the mermaid plugins as a program of their own. `export!` emits a `main` that
//! shows a given file in a terminal, serves konoma on stdin and stdout (`--serve`) and answers
//! `--manifest`, `--version` and `--help`.

konoma_plugin::export!(@program konoma_mermaid::program());
