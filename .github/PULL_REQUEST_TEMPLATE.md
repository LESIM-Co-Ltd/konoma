<!-- Thanks for contributing to konoma! Keep the summary short; the checklist matters. -->

## Summary

<!-- What does this change and why? Link any related issue (e.g. Closes #12). -->

## Checklist

- [ ] Brought up to date with the target branch, then the whole suite run once (below)
- [ ] `cargo fmt --all` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` (git feature) passes
- [ ] `cargo clippy --workspace --all-targets --no-default-features -- -D warnings` passes
- [ ] `cargo nextest run --workspace` and the doctests (`cargo test --workspace --doc`) are green, and so is `cargo nextest run --no-default-features -p konoma -p konoma-vcs-core -p konoma-widgets`
- [ ] Behavior change is covered by a test (or the reason it can't be is noted)
- [ ] `CHANGELOG.md` updated under `[Unreleased]` for user-facing changes
- [ ] Docs updated if a key, config option, or preview kind changed
      (`config.example.toml`, `CONFIGURATION.md`, `docs/KEYMAP.md`, the site)

## Manual verification

<!-- konoma is a TUI: describe what you checked in a real terminal (Ghostty for
     image/kitty-graphics previews). "Tests only" is fine for non-visual changes. -->
