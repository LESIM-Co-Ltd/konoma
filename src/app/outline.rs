use super::*;

impl App {
    // --- 見出しアウトラインオーバーレイ (`o` in a Markdown preview) --------
    pub fn is_outline(&self) -> bool {
        self.outline_open
    }

    /// Heading outline of the current Markdown preview: `(level, text, decorated-line-index)` in
    /// document order. Built from the decorated cache, so it reflects exactly what is on screen
    /// (front matter stripped, footnotes/inline-HTML processed). Empty for non-Markdown previews.
    pub(crate) fn md_outline(&self) -> Vec<(u8, String, usize)> {
        let Some(c) = &self.md_cache else {
            return Vec::new();
        };
        c.anchors
            .iter()
            .filter_map(|(_slug, line)| {
                let l = c.lines.get(*line)?;
                let text = crate::preview::markdown::heading_text(l)?;
                let level = crate::preview::markdown::heading_level_hint(l, c.lines.get(*line + 1));
                Some((level, text, *line))
            })
            .collect()
    }

    /// `o`: open/close the heading outline. Opening selects the heading of the current section
    /// (the last one at or above the viewport top). Flashes and stays closed if there are no
    /// headings (e.g. a non-Markdown preview or a document without any).
    pub fn toggle_outline(&mut self) {
        if self.outline_open {
            self.outline_open = false;
            return;
        }
        let items = self.md_outline();
        if items.is_empty() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::OutlineEmpty).into());
            return;
        }
        // Select the current section: the last heading at or above the top of the view.
        let top = self.md_top_logical_line().unwrap_or(0);
        self.outline_sel = items
            .iter()
            .rposition(|(_, _, line)| *line <= top)
            .unwrap_or(0);
        self.outline_open = true;
    }

    pub fn outline_sel(&self) -> usize {
        self.outline_sel
    }

    pub fn outline_move(&mut self, delta: i32) {
        let n = self.md_outline().len();
        if n == 0 {
            return;
        }
        self.outline_sel = (self.outline_sel as i32 + delta).rem_euclid(n as i32) as usize;
    }

    /// Enter in the outline: scroll the preview to the selected heading and close the overlay.
    pub fn outline_jump(&mut self) {
        let line = self.md_outline().get(self.outline_sel).map(|(_, _, l)| *l);
        self.outline_open = false;
        if let Some(line) = line {
            let (row, _) = self.md_visual_span(line);
            self.tab.preview_scroll = row.min(u16::MAX as usize) as u16;
        }
    }
}
