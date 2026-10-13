// `?` help: an overlay listing every keybinding (last resort for discoverability).
// Follows the tui-design convention: "the footer always shows only a few keys, everything else
// lives behind ?".
// Shows a bordered popup centered on the screen, clearing what's underneath with Clear. Reflects
// configured keys (copy, etc.).

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::i18n::tr;
use crate::keymap::{KeyPress, LeaderId};

/// One help section = heading + a sequence of "key notation / description" rows. Each view builds and returns this.
/// Assembled with the `HelpSection::new(title).row(key, desc).row(...)` builder.
pub struct HelpSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

impl HelpSection {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            rows: Vec::new(),
        }
    }
    pub fn row(mut self, key: impl Into<String>, desc: impl Into<String>) -> Self {
        self.rows.push((key.into(), desc.into()));
        self
    }
}

/// Help body (lines): the sections of the screen the help was opened over, generated from the
/// operations that act there and the keys they are bound to (`ops::help`).
pub fn help_lines(app: &App) -> Vec<Line<'static>> {
    render_sections(&crate::ops::help::sections(app))
}

/// Turn the sections into a Line sequence. Heading = bold underline, row = "  key (16-wide) description (dim)", with blank lines between.
fn render_sections(sections: &[HelpSection]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for sec in sections {
        if !out.is_empty() {
            out.push(Line::from(""));
        }
        out.push(Line::from(
            Span::from(sec.title.clone())
                .add_modifier(Modifier::BOLD)
                .underlined(),
        ));
        for (key, desc) in &sec.rows {
            // A key column wider than its 16 cells (`Ctrl-f / Ctrl-b  ·  PageDown / PageUp`) still
            // leaves a gap before the description.
            let pad = if key.chars().count() >= 16 { "  " } else { "" };
            out.push(Line::from(vec![
                Span::from(format!("  {key:<16}{pad}")).bold(),
                Span::from(desc.clone()).dim(),
            ]));
        }
    }
    out
}

/// Build the help section for one leader (`y`/`Space`) from the actual keymap (with config applied).
/// `prefix` is the trigger-key string to display (e.g. "y" / "Space"). Each view reuses this for the File/Copy sections.
pub fn leader_section(
    app: &App,
    ctx: crate::keymap::Context,
    id: LeaderId,
    prefix: &str,
) -> Option<HelpSection> {
    let menu = crate::ops::menu::menu(app, ctx, id, crate::ops::menu::View::Help)?;
    if menu.items.is_empty() {
        return None;
    }
    let mut sec = HelpSection::new(menu.title);
    for it in &menu.items {
        sec = sec.row(
            format!("{prefix} {}", leader_key_str(it.key)),
            it.label.clone(),
        );
    }
    Some(sec)
}

/// Turn a key into the display string every which-key menu and help row uses (`Space`,
/// `Ctrl-t`, `Enter`, `⇧Tab`). The one implementation: the footer's which-key line and the help
/// share it.
pub(crate) fn leader_key_str(kp: KeyPress) -> String {
    crate::ops::keys::help_key(kp)
}

/// Render the full key list in a centered popup. `area` is the whole screen.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let lines = help_lines(app);
    // Popup size: width caps at 66, fitted to the screen. Height is content + border, fitted to
    // the screen.
    let w = 66.min(area.width.saturating_sub(2)).max(20);
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    // Clamp the body's vertical scroll to the inner height.
    let inner_h = h.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(inner_h);
    let scroll = app.help_scroll.min(max_scroll);

    let title = tr(app.lang, crate::i18n::Msg::HelpTitle);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(ratatui::style::Color::Cyan));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup); // clear what's underneath before drawing
    frame.render_widget(para, popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Mode};
    use crate::config::Config;
    use crate::test_support::unique_tmp;

    /// Flatten help_lines into a single string (to inspect section/row text).
    fn text(app: &App) -> String {
        help_lines(app)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> (App, crate::test_support::TmpDir) {
        let dir = unique_tmp("konoma_help_lines_test");
        std::fs::create_dir_all(&dir).unwrap();
        (App::new(dir.to_path_buf(), Config::default()).unwrap(), dir)
    }

    #[test]
    fn help_is_mode_specific() {
        // Tree: shows tree operations and the git marker section; does not show preview-only
        // sections.
        let (mut a, _dir) = app();
        let tree = text(&a);
        assert!(tree.contains("Tree"), "Tree 節");
        assert!(tree.contains("Git status"), "Git 節");
        assert!(!tree.contains("Preview: image"), "画像節は出さない");
        assert!(
            !tree.contains("horizontal scroll"),
            "テキスト専用節は出さない"
        );
        // Common sections (Tabs/Copy/Global) show up in any mode.
        assert!(
            tree.contains("Tabs") && tree.contains("Copy") && tree.contains("Global"),
            "共通節"
        );

        // Text preview (a Preview that isn't an image): only the text section. Does not show
        // Tree/Git/image.
        a.tab.mode = Mode::Preview;
        let txt = text(&a);
        assert!(txt.contains("Preview: text"), "テキスト節");
        assert!(txt.contains("horizontal scroll"), "横スクロール行");
        // Detect the image section by its dedicated title (text preview also has an in-place-zoom
        // row for inline diagrams now, so "zoom" became a shared word).
        assert!(
            !txt.contains("Git status (row markers)") && !txt.contains("Preview: image"),
            "Tree/画像専用節は出さない"
        );
        // The scroll row uses arrow notation (matches the h/l/← → row, #3).
        assert!(txt.contains("j / k / ↑ ↓"), "矢印付きスクロール行");
        // The common Tabs section remains (tab operations still work during Preview).
        assert!(txt.contains("Tabs"), "Preview でも共通タブ節");
    }
}
