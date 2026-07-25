use super::*;

impl App {
    /// Whether in-preview search input mode is active (intercepting keys).
    pub fn is_searching(&self) -> bool {
        self.tab.search_input.is_some()
    }

    /// The active search query (for highlighting).
    pub fn preview_search_query(&self) -> Option<&str> {
        self.tab.preview_search.as_deref()
    }

    /// The search input being edited (for the footer prompt).
    pub fn search_input(&self) -> Option<&str> {
        self.tab.search_input.as_deref()
    }

    /// Search status (current/total). Some((1-based, total)) when active and there are matches.
    pub fn search_status(&self) -> Option<(usize, usize)> {
        if self.tab.preview_search.is_some() && !self.tab.search_matches.is_empty() {
            Some((self.tab.search_idx + 1, self.tab.search_matches.len()))
        } else {
            None
        }
    }

    /// Which preview the in-preview search (`/`) runs against. Each target has its own way of
    /// locating matches and of moving to one, so `search_commit` / `jump_to_match` branch on this.
    fn search_target(&self) -> SearchTarget {
        if self.preview_win.is_some() {
            // Code/Text と `R` の生 Markdown（バイトオフセットで窓を動かす）。
            SearchTarget::Windowed
        } else if self.table_data.is_some() {
            SearchTarget::Table
        } else if self.md_cache.is_some() {
            // 装飾 Markdown / Mermaid。検索対象は**画面に出ている装飾行**であって
            // ソースではない(装飾は行を増減させるので、ソース行番号では位置を指せない)。
            SearchTarget::Markdown
        } else {
            SearchTarget::Unsupported
        }
    }

    /// Case-insensitive scan of the **decorated** Markdown lines — the same lines the renderer
    /// slices from — so a hit is always something visible on screen. Positions are
    /// `(0, logical line, byte column)`; `n`/`N` then scroll that line into view.
    ///
    /// Non-ASCII queries whose byte length changes under `to_lowercase` are skipped for that line,
    /// matching `highlight_query_in_line` so the highlight and the match list never disagree.
    fn md_search_scan(&mut self, q: &str) {
        self.tab.search_matches.clear();
        let needle = q.to_lowercase();
        let Some(c) = self.md_cache.as_ref() else {
            return;
        };
        for (li, line) in c.lines.iter().enumerate() {
            let full: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let lower = full.to_lowercase();
            if lower.len() != full.len() {
                continue;
            }
            let mut i = 0usize;
            while let Some(rel) = lower[i..].find(&needle) {
                let s = i + rel;
                self.tab.search_matches.push((0, li, s));
                i = s + needle.len().max(1);
                if i >= lower.len() {
                    break;
                }
            }
        }
    }

    /// Scroll the decorated Markdown view to a search hit on logical line `line`.
    ///
    /// A hit that is already fully on screen does not move the view (searching for something you
    /// can see should not jolt the page). Otherwise the line is placed near the top with a few
    /// lines of context above — the same affordance as `follow_scroll_to_first_change`. Note this
    /// deliberately differs from `md_focus_move`'s minimal-scroll rule: with minimal scrolling the
    /// same hit lands at the top or the bottom depending on which way you arrived, so `n` `n` `n`
    /// wrapping back around would not return to the same view.
    fn md_scroll_line_into_view(&mut self, line: usize) {
        const CONTEXT_ROWS: usize = 3;
        let (top, height) = self.md_visual_span(line);
        let vh = self.tab.preview_viewport.max(1) as usize;
        let scroll = self.tab.preview_scroll as usize;
        let fully_visible = top >= scroll && top + height <= scroll + vh;
        if fully_visible {
            return;
        }
        self.tab.preview_scroll = top.saturating_sub(CONTEXT_ROWS) as u16;
    }

    /// Start a search (`/`). Supported in Code/Text (windowed) and table previews. Enters input mode.
    pub fn start_search(&mut self) {
        if matches!(self.search_target(), SearchTarget::Unsupported) {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::SearchCodeTextOnly).into());
            return;
        }
        self.tab.search_input = Some(String::new());
    }

    pub fn search_input_push(&mut self, c: char) {
        if let Some(s) = self.tab.search_input.as_mut() {
            s.push(c);
        }
    }

    pub fn search_input_backspace(&mut self) {
        if let Some(s) = self.tab.search_input.as_mut() {
            s.pop();
        }
    }

    /// Confirm input (Enter): run the query (collect all matching lines) and jump to the first match at or after the current position.
    pub fn search_commit(&mut self) {
        let q = self.tab.search_input.take().unwrap_or_default();
        if q.is_empty() {
            self.tab.preview_search = None;
            self.tab.search_matches.clear();
            self.table_search_hits.clear();
            return;
        }
        const CAP: usize = 5000;
        let target = self.search_target();
        match target {
            SearchTarget::Table => self.table_search_scan(&q),
            SearchTarget::Markdown => {
                self.table_search_hits.clear();
                self.md_search_scan(&q);
            }
            _ => {
                self.table_search_hits.clear();
                self.tab.search_matches = self
                    .preview_win
                    .as_mut()
                    .and_then(|w| w.find_all_matches(&q, CAP).ok())
                    .unwrap_or_default();
            }
        }
        self.tab.preview_search = Some(q);
        if self.tab.search_matches.is_empty() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoMatch).into());
            return;
        }
        self.tab.search_idx = match target {
            // 表: いま居るセル以降の最初の一致へ(読み順)。無ければ先頭へ巡回。
            SearchTarget::Table => {
                let (cr, cc) = (self.tab.table_cur_row, self.tab.table_cur_col);
                self.tab
                    .search_matches
                    .iter()
                    .position(|(_, r, c)| (*r, *c) >= (cr, cc))
                    .unwrap_or(0)
            }
            // 装飾 md: いま見えている先頭の論理行以降の最初の一致へ。無ければ先頭へ巡回。
            SearchTarget::Markdown => {
                let from = self.md_top_logical_line().unwrap_or(0);
                self.tab
                    .search_matches
                    .iter()
                    .position(|(_, line, _)| *line >= from)
                    .unwrap_or(0)
            }
            // 窓読み: 現在の表示位置(byte_top)以降の最初の出現へ。無ければ先頭へ巡回。
            _ => {
                let top = self.tab.preview_byte_top;
                self.tab
                    .search_matches
                    .iter()
                    .position(|(off, _, _)| *off >= top)
                    .unwrap_or(0)
            }
        };
        self.jump_to_match();
    }

    /// `n`/`N`: to the next/previous match (cyclic).
    pub fn search_next(&mut self, dir: i32) {
        if self.tab.search_matches.is_empty() {
            return;
        }
        let n = self.tab.search_matches.len() as i32;
        self.tab.search_idx = (self.tab.search_idx as i32 + dir).rem_euclid(n) as usize;
        self.jump_to_match();
    }

    /// Clear the search (Esc).
    pub fn search_clear(&mut self) {
        self.tab.preview_search = None;
        self.tab.search_input = None;
        self.tab.search_matches.clear();
        self.table_search_hits.clear();
        self.tab.search_idx = 0;
    }

    /// Bring the line of the current occurrence to the top of the display (updates the line-head byte and line number). For moves within the same line,
    /// the top does not change, and only the highlight color (orange) moves to that occurrence (the column is referenced by the render side).
    fn jump_to_match(&mut self) {
        let Some(&(off, a, b)) = self.tab.search_matches.get(self.tab.search_idx) else {
            return;
        };
        match self.search_target() {
            // 表: (行, 列) はセル座標。カーソルを移すと描画側がスクロールして追従する。
            SearchTarget::Table => {
                self.tab.table_cur_row = a;
                self.tab.table_cur_col = b;
            }
            // 装飾 md: 一致した論理行を可視域へ(装飾表示のまま=raw に切替えない)。
            SearchTarget::Markdown => self.md_scroll_line_into_view(a),
            _ => {
                self.tab.preview_byte_top = off;
                self.tab.preview_top_line = a;
            }
        }
    }
}
