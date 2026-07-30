//! Markdown task-list checkboxes: toggling the focused checkbox writes the new state back to the
//! source file — methods on `App`. This is a structured one-char edit (like a rename), not an
//! in-app editor: the write happens only after re-scanning the current file and confirming the
//! target marker still matches what is on screen, so it cannot clobber a concurrent external edit.

use super::*;

impl App {
    /// Space/Enter on a focused checkbox: cycle its state to the next entry of `ui.md_task_states`
    /// and write the single state char back to the file. On any mismatch with the file on disk
    /// (changed externally / pathological doc where render and scan disagree) nothing is written —
    /// flash + reload instead (safe fallback, principle #3).
    pub fn md_toggle_focused_task(&mut self) {
        if self.is_raw_source() {
            return; // the raw-source view is the 2D-caret surface (md_items belongs to the decorated view)
        }
        let Some(f) = self.tab.focused_item else {
            return;
        };
        let Some(MdItem {
            kind: MdItemKind::Task { state },
            ..
        }) = self.md_items.get(f)
        else {
            return;
        };
        let expected = *state;
        // Which number this checkbox is within the document (a task ordinal, excluding links).
        let ordinal = self.md_items[..=f]
            .iter()
            .filter(|it| matches!(it.kind, MdItemKind::Task { .. }))
            .count()
            .saturating_sub(1);
        let total = self
            .md_items
            .iter()
            .filter(|it| matches!(it.kind, MdItemKind::Task { .. }))
            .count();
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        let states = self.cfg.ui.md_task_state_chars();

        // Read the full text once: the write is done against the full text (writing back a
        // truncated prefix would blow away the end of the file). Only the scan is matched to the
        // range the renderer actually saw (below).
        let full_src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
                return;
            }
        };
        // The renderer (`build_decorated`) only sees the prefix truncated by `preview::text::load`'s
        // cap (MAX_BYTES/MAX_LINES). If the scanner looked at the full text, it would count tasks
        // beyond the truncated range too, disagreeing with the on-screen count (rendered) and
        // **rejecting every toggle for that file** (reproduced on a Markdown doc over 5,000 lines).
        // The cap's definition is reused from `preview::text` (not redefined here). Premise: this
        // prefix's byte positions match the raw file's from the start, so the (line, state_off)
        // obtained from the scan also holds for `full_src` — writing only against `full_src` keeps
        // both safely consistent.
        let (capped_lines, _) = crate::preview::text::cap_lines(full_src.as_bytes());
        let capped = capped_lines.join("\n");
        // The renderer also strips front matter from the body before drawing it (it isn't treated as
        // a task). Strip it with the same rule and add the removed line count back onto the found
        // line number as an offset (offset=0 if there's no front matter / the setting is OFF, so
        // nothing is stripped).
        let (front_matter, body) = if self.cfg.ui.md_frontmatter {
            crate::preview::markdown::strip_front_matter(&capped)
        } else {
            (None, capped.clone())
        };
        let line_offset = if front_matter.is_some() {
            capped.lines().count().saturating_sub(body.lines().count())
        } else {
            0
        };
        // Verify before writing: only write when the count and the target marker's current state
        // match what's shown on screen. A `<details>` block's open/closed state is runtime state —
        // without passing the same open/closed sequence used to render, the body of a closed block
        // would also be counted and the count would disagree (the most recent render records it in
        // md_cache).
        let details_states: Vec<bool> = self
            .md_cache
            .as_ref()
            .map(|c| c.details_states.clone())
            .unwrap_or_default();
        let mut locs = crate::preview::markdown::task_source_locs(&body, &states, &details_states);
        for l in &mut locs {
            l.line += line_offset;
        }
        let ok = locs.len() == total
            && locs
                .get(ordinal)
                .is_some_and(|l| norm_state(l.state) == norm_state(expected));
        if !ok {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::TaskFileChanged).into());
            self.reload_preview();
            return;
        }
        let loc = &locs[ordinal];
        let cur = loc.state;
        let next = match states
            .iter()
            .position(|s| norm_state(*s) == norm_state(cur))
        {
            Some(i) => states[(i + 1) % states.len()],
            None => states[0], // normalize an out-of-array state to the first entry
        };
        // Replace only the single state character (all other bytes are unchanged; CRLF/trailing
        // newline are also preserved). Since it writes against the full text (full_src), the range
        // that's invisible due to truncation/front matter is preserved as-is.
        let mut lines: Vec<String> = full_src.split('\n').map(str::to_string).collect();
        let Some(line) = lines.get_mut(loc.line) else {
            return; // unreachable since this comes from task_source_locs (defensive)
        };
        line.replace_range(
            loc.state_off..loc.state_off + cur.len_utf8(),
            &next.to_string(),
        );
        let new_src = lines.join("\n");
        if let Err(e) = std::fs::write(&path, new_src) {
            self.flash = Some(format!(
                "{}{e}",
                crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
            ));
            return;
        }
        // Redraw (discard md_cache). Since the item count is unchanged, focus stays on the same checkbox.
        self.reload_preview();
    }
}

/// `X` and `x` are the same checked state (GFM treats them identically).
fn norm_state(c: char) -> char {
    if c == 'X' {
        'x'
    } else {
        c
    }
}
