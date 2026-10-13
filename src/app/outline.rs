use super::*;

impl App {
    // --- Heading outline overlay (`O` in a Markdown preview; the page list on a paged preview) ---
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
        let math = super::md_text::math_sources_by_line(&c.images);
        c.anchors
            .iter()
            .filter_map(|(_slug, line)| {
                let l = c.lines.get(*line)?;
                let text = crate::preview::markdown::heading_text_at(&c.lines, *line, &math)?;
                let level = crate::preview::markdown::heading_level_hint(
                    l,
                    crate::preview::markdown::row_after_heading(&c.lines, *line),
                );
                Some((level, text, *line))
            })
            .collect()
    }

    /// The page list `O` shows on this preview, or `None` when `O` is the heading outline. A
    /// preview made of pages lists them (the layer-2 `O` of the key design): the slides of a deck
    /// (the headings that start its pages, as the text shows them), the sheets of a workbook (their
    /// names) and the pages of a PDF ("Page n", with the title when the plugin reports one). The
    /// list can hold fewer than two entries: then there is nothing to choose and `O` refuses.
    pub(crate) fn page_list(&self) -> Option<Vec<String>> {
        use crate::i18n::Msg;
        if self.is_deck() {
            let Some(c) = self.md_cache.as_ref() else {
                return Some(Vec::new());
            };
            let Some(heads) = self.anchored_head_lines(c) else {
                return Some(Vec::new());
            };
            let math = super::md_text::math_sources_by_line(&c.images);
            return Some(
                heads
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        crate::preview::markdown::heading_text_at(&c.lines, *line, &math)
                            .unwrap_or_else(|| {
                                format!("{} {}", tr(self.lang, Msg::PageListPage), i + 1)
                            })
                    })
                    .collect(),
            );
        }
        let is_sheet = self
            .tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_spreadsheet);
        let is_pdf = self.shows_plugin(konoma_media::PDF_ID);
        if self.is_document() || !(is_sheet || is_pdf || self.tab.page_count.is_some()) {
            return None;
        }
        let Some(n) = self.tab.page_count else {
            return Some(Vec::new());
        };
        let titles: &[String] = match self.plugin.as_ref().map(|p| &p.layout.pages) {
            Some(Some(konoma_plugin::view::PageLayout::Separate { titles, .. })) => titles,
            _ => &[],
        };
        let word = tr(
            self.lang,
            if is_sheet {
                Msg::PageListSheet
            } else {
                Msg::PageListPage
            },
        );
        Some(
            (1..=n as usize)
                .map(|i| match titles.get(i - 1).filter(|t| !t.is_empty()) {
                    // A sheet is named by its name alone; a page by its number, then its title.
                    Some(t) if is_sheet => t.clone(),
                    Some(t) => format!("{word} {i}: {t}"),
                    None => format!("{word} {i}"),
                })
                .collect(),
        )
    }

    /// Whether `O` lists pages here and there is more than one to choose from (what the gate, the
    /// footer and the help share with the handler).
    pub fn page_list_usable(&self) -> bool {
        self.page_list().is_some_and(|l| l.len() >= 2)
    }

    /// Whether `O` is the page list on this preview (usable or not).
    pub fn is_page_list_preview(&self) -> bool {
        self.page_list().is_some()
    }

    /// The 0-based page the preview is on (for a deck, the one at the top of the view).
    fn current_page_index(&self) -> usize {
        if self.is_deck() {
            if let Some(p) = self.tab.deck_pending_slide {
                return p;
            }
            return self
                .anchored_position()
                .map_or(0, |(n, _)| n.saturating_sub(1));
        }
        (self.tab.page as usize).saturating_sub(1)
    }

    /// The rows of the open overlay: `(level, text)`, the headings of the outline or the pages.
    pub(crate) fn outline_rows(&self) -> Vec<(u8, String)> {
        match self.page_list() {
            Some(pages) => pages.into_iter().map(|t| (1, t)).collect(),
            None => self
                .md_outline()
                .into_iter()
                .map(|(level, text, _)| (level, text))
                .collect(),
        }
    }

    pub(crate) fn outline_len(&self) -> usize {
        self.outline_rows().len()
    }

    /// The row that marks where the preview is now (the page list only).
    pub(crate) fn outline_current_row(&self) -> Option<usize> {
        self.page_list().map(|_| self.current_page_index())
    }

    /// `O` on a preview of pages: open the page list at the current page.
    fn open_page_list(&mut self) {
        if !self.page_list_usable() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::PageListEmpty).into());
            return;
        }
        self.outline_sel = self.current_page_index();
        self.outline_open = true;
    }

    /// Enter in the page list: go to the selected page.
    fn page_list_jump(&mut self) {
        let sel = self.outline_sel;
        self.outline_open = false;
        if self.is_deck() {
            if !self.anchored_can_turn() {
                return;
            }
            if self.tab.deck_pending_slide.is_some() {
                let last = self.outline_len().saturating_sub(1);
                self.tab.deck_pending_slide = Some(sel.min(last));
                return;
            }
            if let Some((heads, _)) = self.anchored_view() {
                if let Some(row) = heads.get(sel) {
                    self.tab.preview_scroll = (*row).min(u16::MAX as usize) as u16;
                }
            }
        } else {
            self.goto_page(sel as u32 + 1);
        }
    }

    /// `o`: open/close the heading outline. Opening selects the heading of the current section
    /// (the last one at or above the viewport top). Flashes and stays closed if there are no
    /// headings (e.g. a non-Markdown preview or a document without any).
    pub fn toggle_outline(&mut self) {
        if self.outline_open {
            self.outline_open = false;
            return;
        }
        if self.is_page_list_preview() {
            self.open_page_list();
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
        let n = self.outline_len();
        if n == 0 {
            return;
        }
        self.outline_sel = (self.outline_sel as i32 + delta).rem_euclid(n as i32) as usize;
    }

    /// Enter in the outline: scroll the preview to the selected heading and close the overlay.
    pub fn outline_jump(&mut self) {
        if self.is_page_list_preview() {
            self.page_list_jump();
            return;
        }
        let line = self.md_outline().get(self.outline_sel).map(|(_, _, l)| *l);
        self.outline_open = false;
        if let Some(line) = line {
            let (row, _) = self.md_visual_span(line);
            self.tab.preview_scroll = row.min(u16::MAX as usize) as u16;
        }
    }
}
