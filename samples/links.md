# Link navigation demo

`Tab` / `⇧Tab` focuses a link, `Enter` opens it.
(URL = browser / local file = konoma preview / directory = jump there)

## Local links (open inside konoma)

- Code: [hello.rs](./code/hello.rs) / [config.toml](./code/config.toml)
- Text: [japanese.txt](./japanese.txt)
- Directory: [code/](./code)
- Another Markdown file: [markdown.md](./markdown.md)

## External links (open in the browser)

- [ratatui](https://ratatui.rs)
- [Rust](https://www.rust-lang.org)
- Autolink: <https://github.com>

## Links inside a table

Links in a cell show only their label, and `Tab`/`Enter` work just like paragraph links.

| Kind | Link | Note |
|------|------|------|
| local | [hello.rs](./code/hello.rs) | opens inside konoma |
| external | [ratatui](https://ratatui.rs) | opens in the browser |
| with title | [japanese.txt](./japanese.txt "Japanese sample") | the title is not mixed into the URL |
| many in one cell | [markdown.md](./markdown.md) and [code/](./code) | focus them in order |
| CJK mix | 日本語 before [README](./README.md) after | column alignment stays intact |

## A broken link (shows an error)

- [missing](./does-not-exist.md)
