# Contributing to konoma

Thanks for your interest! konoma is a full-screen-preview terminal file browser for
macOS on Apple Silicon.

## Development

```bash
cargo build                 # debug build (cargo build --release for optimized)
cargo run -- /path/to/dir   # run against a directory (defaults to the current dir)
```

Images, SVG, video thumbnails, and PDF previews require a terminal that supports the
kitty graphics protocol (e.g. Ghostty) for manual verification.

## Before submitting a PR

The definition of done is **zero warnings and all tests green** for both feature
configurations:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test
cargo test --no-default-features
```

- The `git` feature is on by default; `--no-default-features` drops it. Keep both building.
- Avoid panics in runtime code paths: return `Result` and add context with `anyhow`.
  Reserve `unwrap`/`expect` for self-evident init-time invariants.
- Public-facing items (doc comments, README) are written in English; internal `//`
  comments may be in Japanese.
- External tools (poppler, ffmpeg, git, …) must stay optional: the app should run and
  degrade gracefully when they are absent.

## License

By contributing, you agree that your contributions are licensed under the MIT license.
