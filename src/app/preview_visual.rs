use super::*;

/// The active visual selection in a windowed preview (shared with the standalone app).
pub use konoma_widgets::caret::Selection as PreviewSelection;

impl App {
    // ---- 2D caret / visual-selection copy for windowed previews -----------

    /// Whether a visual selection is active in a windowed preview (routes the PreviewTextVisual surface).
    pub fn is_preview_visual(&self) -> bool {
        self.is_windowed() && self.preview_visual_anchor.is_some()
    }

    /// Whether the active preview selection is linewise (`V`) rather than charwise (`v`). For the status chip/footer.
    pub fn preview_visual_linewise(&self) -> bool {
        self.preview_visual_linewise
    }

    /// The active selection for the render/copy: charwise (`v`) or linewise (`V`), normalized so start ≤ end.
    pub fn preview_selection(&self) -> PreviewSelection {
        let Some((al, ac)) = self.preview_visual_anchor else {
            return PreviewSelection::None;
        };
        let (cl, cc) = (self.tab.preview_cursor_line, self.tab.preview_cursor_col);
        if self.preview_visual_linewise {
            PreviewSelection::Line {
                lo: al.min(cl),
                hi: al.max(cl),
            }
        } else {
            // charwise: order by (line, col)
            let (start, end) = if (al, ac) <= (cl, cc) {
                ((al, ac), (cl, cc))
            } else {
                ((cl, cc), (al, ac))
            };
            PreviewSelection::Char { start, end }
        }
    }

    /// `v` (charwise) / `V` (linewise): start a visual selection at the current 2D caret (windowed previews only).
    pub fn preview_enter_visual(&mut self, linewise: bool) {
        if self.is_windowed() && !self.document_text_missing() {
            self.preview_visual_anchor =
                Some((self.tab.preview_cursor_line, self.tab.preview_cursor_col));
            self.preview_visual_linewise = linewise;
        }
    }

    /// Exit visual selection without copying (`v`/`V` again / Esc / q).
    pub fn preview_exit_visual(&mut self) {
        self.preview_visual_anchor = None;
    }

    /// The selected text read from the real file: whole logical lines (linewise) or an exact character range
    /// (charwise, end-inclusive). Uses the file's unwrapped/logical lines so a paste keeps the original layout.
    /// Empty when there is no path or the range is out of bounds.
    pub(super) fn preview_selection_text(&self) -> String {
        // The text the reader shows: the file itself, or — for a delegated command's output or a
        // Word document's converted Markdown — the generated temp file (`windowed_src`).
        let Some(path) = self.windowed_src().or(self.tab.preview_path.as_deref()) else {
            return String::new();
        };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return String::new(),
        };
        let s = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = s.lines().collect();
        if lines.is_empty() {
            return String::new();
        }
        let last = lines.len() - 1;
        match self.preview_selection() {
            PreviewSelection::Line { lo, hi } => {
                let end = hi.min(last);
                if lo > end {
                    String::new()
                } else {
                    lines[lo..=end].join("\n")
                }
            }
            PreviewSelection::None => {
                // not selecting → the current logical line
                let l = self.tab.preview_cursor_line.min(last);
                lines[l].to_string()
            }
            PreviewSelection::Char { start, end } => selection_char_text(&lines, start, end),
        }
    }

    /// `y`: copy the current selection (or the current line if not selecting) to the clipboard, then exit visual.
    pub fn preview_copy_selection(&mut self) {
        let text = self.preview_selection_text();
        self.preview_visual_anchor = None;
        if text.is_empty() {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        }
        self.set_clipboard_flash(&text);
    }

    /// The `@path#L12-34` reference text (Claude Code's file+line context syntax) for the current
    /// selection — or the caret line when not selecting. Line numbers are 1-based and inclusive; a single
    /// line is `#L12`. In non-windowed previews (no caret) it degrades to `@path`. None without a target.
    pub(super) fn preview_selection_ref_text(&self) -> Option<String> {
        let path = self.tab.preview_path.as_ref()?;
        let base = at_ref_text(&self.tab.open_dir, path);
        // The lines of a Word document's converted Markdown are not lines of the .docx.
        if !self.is_windowed() || self.is_document() {
            return Some(base);
        }
        let (lo, hi) = match self.preview_selection() {
            PreviewSelection::Line { lo, hi } => (lo, hi),
            PreviewSelection::Char { start, end } => (start.0, end.0),
            PreviewSelection::None => (self.tab.preview_cursor_line, self.tab.preview_cursor_line),
        };
        Some(if lo == hi {
            format!("{base}#L{}", lo + 1)
        } else {
            format!("{base}#L{}-{}", lo + 1, hi + 1)
        })
    }

    /// `Y`: copy the `@path#L…` reference of the selection/caret to the clipboard, then exit visual
    /// (paste it to Claude Code to point the conversation at this exact spot).
    pub fn preview_copy_selection_ref(&mut self) {
        let Some(text) = self.preview_selection_ref_text() else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        self.preview_visual_anchor = None;
        self.set_clipboard_flash(&text);
    }

    /// `h`/`l`: move the column caret by `dir` chars (windowed previews). The render clamps it to the line length
    /// and writes the clamp back, so overshoot is corrected on the next frame. Falls back to hscroll otherwise.
    pub fn preview_col_move(&mut self, dir: i32) {
        if self.is_windowed() {
            let mut c = self.caret();
            c.col_move(dir);
            self.tab.preview_cursor_col = c.col;
        } else {
            self.preview_hscroll(dir * 2);
        }
    }

    /// `0`: move the caret to the first column (windowed), else scroll home.
    pub fn preview_col_home(&mut self) {
        if self.is_windowed() {
            let mut c = self.caret();
            c.col_home();
            self.tab.preview_cursor_col = c.col;
        } else {
            self.preview_hscroll_home();
        }
    }

    /// `$`: move the caret to the last column (windowed). The render clamps the sentinel to the line length.
    pub fn preview_col_end(&mut self) {
        if self.is_windowed() {
            let mut c = self.caret();
            c.col_end();
            self.tab.preview_cursor_col = c.col;
        } else {
            self.preview_hscroll_end();
        }
    }
}

/// Extract the text of a charwise selection from the file's logical lines (`start`/`end` end-inclusive,
/// columns are char indices). Slicing by chars (not bytes) keeps multibyte/CJK content intact.
fn selection_char_text(lines: &[&str], start: (usize, usize), end: (usize, usize)) -> String {
    let (sl, sc) = start;
    let (el, ec) = end;
    if sl >= lines.len() {
        return String::new();
    }
    let el = el.min(lines.len() - 1);
    if sl == el {
        // Single line: [sc..=ec] (normalized so sc ≤ ec)
        lines[sl]
            .chars()
            .skip(sc)
            .take((ec + 1).saturating_sub(sc))
            .collect()
    } else {
        let mut out = String::new();
        out.push_str(&lines[sl].chars().skip(sc).collect::<String>());
        for l in &lines[sl + 1..el] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&lines[el].chars().take(ec + 1).collect::<String>());
        out
    }
}
