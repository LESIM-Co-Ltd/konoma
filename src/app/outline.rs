use super::*;

/// A page list built for the overlay, with what it was built for.
pub(crate) type BuiltPages = (PagesKey, Arc<Vec<(u8, String)>>);

#[cfg(test)]
thread_local! {
    /// Test-only: how many plugin page lists were built on this thread.
    pub(crate) static PAGE_LIST_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What a built page list of the overlay was built for (see `App::outline_rows`).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PagesKey {
    is_sheet: bool,
    count: Option<u32>,
    titles: (usize, usize),
    lang: crate::i18n::Lang,
}

/// What `O` lists on a paged preview.
enum PageListShape {
    /// The slides of a deck, named by their headings.
    Deck,
    /// The sheets of a workbook or the pages of a PDF-like plugin.
    Pages { is_sheet: bool },
}

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
        match self.page_list_shape()? {
            PageListShape::Deck => {
                let Some(c) = self.md_cache.as_ref() else {
                    return Some(Vec::new());
                };
                let Some(heads) = self.anchored_head_lines(c) else {
                    return Some(Vec::new());
                };
                let math = super::md_text::math_sources_by_line(&c.images);
                Some(
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
                )
            }
            PageListShape::Pages { is_sheet } => {
                #[cfg(test)]
                PAGE_LIST_BUILDS.with(|c| c.set(c.get() + 1));
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
        }
    }

    /// What kind of page list `O` shows here, or `None` when `O` is the heading outline. The one
    /// place that decides it: the list itself and the cheap questions below share it.
    fn page_list_shape(&self) -> Option<PageListShape> {
        if self.is_deck() {
            return Some(PageListShape::Deck);
        }
        let is_sheet = self
            .tab
            .preview_kind
            .as_ref()
            .is_some_and(PreviewKind::is_spreadsheet);
        // A picture plugin with separate pages (a PDF's pages) lists them even before the page
        // count is known: its layout says it has pages.
        let is_pdf = self.shows_media_plugin()
            && self.plugin.as_ref().is_some_and(|p| {
                matches!(
                    p.layout.pages,
                    Some(konoma_plugin::view::PageLayout::Separate { .. })
                )
            });
        if self.is_document() || !(is_sheet || is_pdf || self.tab.page_count.is_some()) {
            return None;
        }
        Some(PageListShape::Pages { is_sheet })
    }

    /// How many entries the page list has, **without building it** (the footer and the key gate
    /// ask every frame, and a plugin may declare a large page count). `None` when `O` is the
    /// heading outline.
    fn page_list_len(&self) -> Option<usize> {
        match self.page_list_shape()? {
            PageListShape::Deck => Some(
                self.md_cache
                    .as_ref()
                    .and_then(|c| self.anchored_head_lines(c))
                    .map_or(0, |heads| heads.len()),
            ),
            PageListShape::Pages { .. } => Some(self.tab.page_count.map_or(0, |n| n as usize)),
        }
    }

    /// Test-only: the page count of the preview, and a way to set it.
    #[cfg(test)]
    pub fn page_count_for_test(&self) -> Option<u32> {
        self.tab.page_count
    }

    /// Test-only: pretend the preview has `n` pages.
    #[cfg(test)]
    pub fn set_page_count_for_test(&mut self, n: Option<u32>) {
        self.tab.page_count = n;
    }

    /// Whether `O` lists pages here and there is more than one to choose from (what the gate, the
    /// footer and the help share with the handler). Decided by the count alone.
    pub fn page_list_usable(&self) -> bool {
        self.page_list_len().is_some_and(|n| n >= 2)
    }

    /// Whether `O` is the page list on this preview (usable or not).
    pub fn is_page_list_preview(&self) -> bool {
        self.page_list_shape().is_some()
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

    /// Closes the `O` overlay and drops the page list built for it.
    pub(crate) fn close_outline(&mut self) {
        self.outline_open = false;
        *self.outline_pages.borrow_mut() = None;
    }

    /// Forgets the page list built for the overlay (the layout it was built from changed).
    pub(crate) fn forget_outline_pages(&self) {
        *self.outline_pages.borrow_mut() = None;
    }

    /// The rows of the open overlay: `(level, text)`, the headings of the outline or the pages.
    /// A list of plugin pages is built once per opening of the overlay (and again when the page
    /// count, the titles or the language change), not on every frame and key.
    pub(crate) fn outline_rows(&self) -> Arc<Vec<(u8, String)>> {
        let key = self.pages_key();
        if let Some(key) = &key {
            if let Some((built_for, rows)) = self.outline_pages.borrow().as_ref() {
                if built_for == key {
                    return rows.clone();
                }
            }
        }
        let rows: Arc<Vec<(u8, String)>> = Arc::new(match self.page_list() {
            Some(pages) => pages.into_iter().map(|t| (1, t)).collect(),
            None => self
                .md_outline()
                .into_iter()
                .map(|(level, text, _)| (level, text))
                .collect(),
        });
        if let (Some(key), true) = (key, self.outline_open) {
            *self.outline_pages.borrow_mut() = Some((key, rows.clone()));
        }
        rows
    }

    /// What a built page list depends on (plugin pages only; a deck's list is bounded by its
    /// document and cheap): the kind of list, the count, the titles' allocation and the language.
    fn pages_key(&self) -> Option<PagesKey> {
        let PageListShape::Pages { is_sheet } = self.page_list_shape()? else {
            return None;
        };
        let titles: &[String] = match self.plugin.as_ref().map(|p| &p.layout.pages) {
            Some(Some(konoma_plugin::view::PageLayout::Separate { titles, .. })) => titles,
            _ => &[],
        };
        Some(PagesKey {
            is_sheet,
            count: self.tab.page_count,
            titles: (titles.as_ptr() as usize, titles.len()),
            lang: self.lang,
        })
    }

    pub(crate) fn outline_len(&self) -> usize {
        self.outline_rows().len()
    }

    /// The row that marks where the preview is now (the page list only).
    pub(crate) fn outline_current_row(&self) -> Option<usize> {
        self.is_page_list_preview()
            .then(|| self.current_page_index())
    }

    /// `O` on a preview of pages: open the page list at the current page.
    fn open_page_list(&mut self) {
        if !self.page_list_usable() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::PageListEmpty).into());
            return;
        }
        self.outline_sel = self.current_page_index();
        self.list_filter_reset(super::list_filter::ListId::Outline);
        self.forget_outline_pages();
        self.outline_open = true;
    }

    /// Enter in the page list: go to the selected page.
    fn page_list_jump(&mut self) {
        let sel = self.outline_sel;
        self.close_outline();
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
            self.close_outline();
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
        self.list_filter_reset(super::list_filter::ListId::Outline);
        self.outline_open = true;
    }

    pub fn outline_sel(&self) -> usize {
        self.outline_sel
    }

    /// Move the selection `delta` rows among the rows the `/` query leaves shown (all of them
    /// without one), wrapping at the ends.
    pub fn outline_move(&mut self, delta: i32) {
        let n = self.outline_len();
        self.outline_sel = self.overlay_step(
            super::list_filter::ListId::Outline,
            self.outline_sel,
            n,
            delta,
        );
    }

    /// `g` / `G`: the first / last row that is shown.
    pub fn outline_to_edge(&mut self, last: bool) {
        let n = self.outline_len();
        if n > 0 {
            self.outline_sel = self.overlay_edge(super::list_filter::ListId::Outline, n, last);
        }
    }

    /// Enter in the outline: scroll the preview to the selected heading and close the overlay.
    /// A query that hides the selected row (all of them) leaves nothing to go to.
    pub fn outline_jump(&mut self) {
        if !self.list_row_shown(super::list_filter::ListId::Outline, self.outline_sel) {
            return;
        }
        if self.is_page_list_preview() {
            self.page_list_jump();
            return;
        }
        let line = self.md_outline().get(self.outline_sel).map(|(_, _, l)| *l);
        self.close_outline();
        if let Some(line) = line {
            let (row, _) = self.md_visual_span(line);
            self.tab.preview_scroll = row.min(u16::MAX as usize) as u16;
        }
    }
}
