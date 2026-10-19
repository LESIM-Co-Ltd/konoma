//! A complete konoma plugin that draws with ratatui widgets, run without konoma or a terminal.
//!
//! `cargo run -p konoma-plugin-ratatui --example clock`
//!
//! The plugin itself is `konoma_plugin_ratatui::demo::Clock` (see there for what it does and the
//! practices it shows). `main` drives it with the helpers of `konoma_plugin::testing` and prints
//! what the canvas looks like.

use konoma_plugin::prelude::*;
use konoma_plugin::testing::{open_bytes, send_keys, view_canvas, CanvasText};
use konoma_plugin_ratatui::demo::Clock;

fn main() -> Result<(), Failure> {
    let (mut session, _file) = open_bytes(&Clock::default(), "a.konoma-demo", b"Hello, konoma\n")?;
    let req = ViewReq::new(34, 7);
    println!("{}\n", CanvasText::of(&view_canvas(&mut *session, &req)?));

    session.act("edit", &ActCx::bare())?;
    let typed: Vec<Key> = "hi 日本"
        .chars()
        .map(Key::char)
        .chain([Key::new(KeyCode::Enter)])
        .collect();
    send_keys(&mut *session, &typed).outcomes()?;
    let canvas = view_canvas(&mut *session, &req)?;
    println!("{}", CanvasText::of(&canvas));
    println!(
        "\ntext_input = {}, redraw_after = {:?}",
        canvas.text_input, canvas.redraw_after
    );
    Ok(())
}
