# Contributing to konoma

Thanks for your interest! konoma is a full-screen-preview terminal file browser for
macOS (Apple Silicon, the primary target) and Linux (`x86_64`, beta). CI runs the full
clippy + test suite on both.

## Development

```bash
cargo build                 # debug build (cargo build --release for optimized)
cargo run -- /path/to/dir   # run against a directory (defaults to the current dir)
```

To verify image, SVG, video-thumbnail and PDF previews by eye, use a terminal that
speaks a graphics protocol — kitty graphics (e.g. Ghostty), iTerm2, or sixel. Anywhere
else they fall back to half-blocks, which is too coarse to judge a rendering change by.

## Before submitting a PR

The definition of done is **zero warnings and all tests green** for both feature
configurations:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo nextest run --workspace               # install once: cargo install cargo-nextest --locked
cargo nextest run --workspace --no-default-features
cargo test --workspace --doc                # nextest does not run doctests
cargo test --workspace --doc --no-default-features
```

The repository is a Cargo workspace: the `konoma` binary at the root (`src/`) plus the
host-independent engines under `crates/` — `konoma-base` (small shared utilities),
`konoma-svg` (SVG rasterising and fonts), `konoma-math-core`, `konoma-mermaid-core`,
`konoma-media-core` (PDF / video), `konoma-office-core` (Word / spreadsheet / slide
documents), `konoma-vcs-core` (git / jj), `konoma-markdown-core` (the Markdown block model
and diff), `konoma-widgets` (display parts for ratatui that do not depend on the app: windowed file
reading, kitty graphics transmit, image layout, diff rows, icon glyphs and the Markdown document
renderer), and `konoma-test-support` (shared test helpers). The cores know nothing about
the UI, the config or the i18n catalogue; the host passes in whatever text they show.
Use `--workspace` so every crate is checked, and `-p <crate>` to work on one.

- The `git` feature is on by default; `--no-default-features` drops it. Keep both building.
- Avoid panics in runtime code paths: return `Result` and add context with `anyhow`.
  Reserve `unwrap`/`expect` for self-evident init-time invariants.
- Comments are written in English — doc comments, internal `//` comments, and the
  public-facing docs (README, the documentation site) alike.
- External tools (ffmpeg, git, …) must stay optional: the app should run and
  degrade gracefully when they are absent. Prefer a pure-Rust renderer over a new
  external dependency — that is why PDF, SVG, Mermaid and LaTeX math need nothing installed.

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating,
you are expected to uphold it. To report a security issue, see [SECURITY.md](SECURITY.md).

## License

By contributing, you agree that your contributions are licensed under the MIT license.
