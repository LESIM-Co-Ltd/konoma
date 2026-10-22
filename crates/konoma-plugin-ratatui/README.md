# konoma-plugin-ratatui

Draw a [konoma](https://github.com/LESIM-Co-Ltd/konoma) plugin's canvas with
[ratatui](https://ratatui.rs) widgets. It converts a ratatui `Buffer` into a
`konoma_plugin::CanvasView` (plain data: konoma's own ratatui is not involved).

```rust
use konoma_plugin::prelude::*;
use konoma_plugin_ratatui::ratatui::widgets::{Block, Widget};

fn view(req: &ViewReq) -> View {
    View::Canvas(konoma_plugin_ratatui::draw_canvas(req, |buf| {
        Block::bordered().title("hello").render(buf.area, buf);
    }))
}
```

- **Which ratatui:** 0.30.x. Draw with the ratatui this crate re-exports
  (`konoma_plugin_ratatui::ratatui`); each ratatui minor release needs a new version of this crate.
- There is no `Frame`: render with `Widget::render(area, buf)`. Pictures are added to the returned
  canvas itself (`CanvasImage`).
- See [`konoma-plugin`](https://crates.io/crates/konoma-plugin) for the plugin API.

The `demo` feature (a clock plugin used by konoma's `demo-canvas` feature) is **temporary and
unstable**: it is hidden from the docs, has no semver promise and is removed when konoma can load
plugins from outside.

MIT licensed.
