// `?` help: an overlay listing every keybinding (last resort for discoverability).
// Follows the tui-design convention: "the footer always shows only a few keys, everything else
// lives behind ?".
// Shows a bordered popup centered on the screen, clearing what's underneath with Clear. Reflects
// configured keys (copy, etc.).

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::Frame;

use crate::app::App;
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

/// Turn the sections into a Line sequence (the one look every host's help has).
fn render_sections(sections: &[HelpSection]) -> Vec<Line<'static>> {
    let view: Vec<(&str, &[(String, String)])> = sections
        .iter()
        .map(|s| (s.title.as_str(), s.rows.as_slice()))
        .collect();
    konoma_widgets::help_panel::help_lines(&view)
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
    let pairs: Vec<(KeyPress, String)> = menu
        .items
        .iter()
        .map(|i| (i.key, i.label.clone()))
        .collect();
    let shared = konoma_host_core::help::menu_section(&menu.title, prefix, &pairs)?;
    let mut sec = HelpSection::new(shared.title);
    for (keys, label) in shared.rows {
        sec = sec.row(keys, label);
    }
    Some(sec)
}

/// Turn a key into the display string every which-key menu and help row uses (`Space`,
/// `Ctrl-t`, `Enter`, `⇧Tab`). The one implementation: the footer's which-key line and the help
/// share it.
pub(crate) fn leader_key_str(kp: KeyPress) -> String {
    crate::ops::keys::help_key(kp)
}

/// The help popup's frame and the number of body rows inside it (shared with the standalone
/// app). Used by the e2e tests to know which help lines are actually on screen.
#[cfg(test)]
pub fn popup_layout(line_count: usize, area: Rect) -> (Rect, u16) {
    konoma_widgets::help_panel::popup_layout(line_count, area)
}

/// Render the full key list in a centered popup. `area` is the whole screen.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let lines = help_lines(app);
    let title = crate::ops::core_help::help_title(app);
    konoma_widgets::help_panel::render_panel(
        frame.buffer_mut(),
        area,
        lines,
        app.help_scroll,
        title,
    );
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
