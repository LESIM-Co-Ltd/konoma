# Code highlighting samples

A set of files for checking syntax highlighting. Each one matches konoma's default `code`
rule (`*.{rs,ts,tsx,js,py,go,toml,json,sh,yaml,yml,c,cpp,h}`) and is colored by pure-Rust syntect.

| File | Kind |
|------------------|------------------------|
| `hello.rs`       | Rust |
| `script.py`      | Python |
| `app.ts`         | TypeScript |
| `util.js`        | JavaScript |
| `main.go`        | Go |
| `calc.c`         | C |
| `widget.cpp`     | C++ |
| `types.h`        | C header |
| `build.sh`       | Shell script |
| `config.toml`    | TOML config |
| `config.yaml`    | YAML config |
| `ci.yml`         | YAML config (.yml) |
| `data.json`      | JSON data |
| `large_generated.rs` | **generated Rust over 1 MB** (for testing less-style windowed reading; numbered markers) |

```sh
cargo run -- samples/code
```

`large_generated.rs` (~1.8 MB / 52512 lines) is over the threshold (256 KiB), so it opens with
**less-style windowed reading**. `j`/`Space`/`G` scroll to the end, and the `[NN%]` in the title
shows progress. Delete it if you only need it temporarily: `rm samples/code/large_generated.rs`

Open each file and confirm that keywords, strings, comments, and numbers are colored.
This README's own Markdown code fence is colored through the same syntect path.
