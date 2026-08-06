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
        // Verify before writing. The question that has to be answered is **not** "do the two sides
        // count the same number of checkboxes" — that is what the released build asked, and it is
        // satisfiable while the ordinals point at different checkboxes: a document that hides one
        // checkbox from the screen and reveals a different one to the raw scanner keeps the totals
        // (and, since most checkboxes are unchecked, the state characters) equal, and `Space` then
        // wrote `x` into a line the reader was not looking at, with no flash at all. Verified end to
        // end against v0.23.5.
        //
        // The question asked instead is "**is the Nth checkbox on screen the same object as the line
        // I am about to edit**". It is answered in the renderer's own coordinate system:
        //   1. scan `pre_src` — the exact text that produced what is on screen — so the Nth `TaskLoc`
        //      really is the Nth drawn checkbox (that scanner/renderer agreement, over one shared
        //      text, is what the corpus parity tests pin);
        //   2. follow `pre_origin` back to the body line that line came from — a line a pre-pass
        //      invented (the footnotes section, the tail of a `<br>` split) has no origin at all;
        //   3. require that body line, as it is on disk right now, to be byte-identical to the line
        //      on screen, which both proves the byte offset transfers verbatim and catches an
        //      external edit since the render.
        // Anything that cannot be matched refuses (principle #3) — refusing costs the reader one
        // keypress, writing to the wrong line costs them their file.
        let Some(cache) = self.md_cache.as_ref() else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::TaskFileChanged).into());
            self.reload_preview();
            return;
        };
        let drawn = crate::preview::markdown::task_source_locs(
            &cache.pre_src,
            &states,
            &cache.details_states,
        );
        let body_lines: Vec<&str> = body.lines().collect();
        // Independently of correspondence, keep refusing when the file on disk no longer holds the
        // same set of checkboxes the render read — an external editor or agent has moved on and the
        // right answer is to reload, not to write into a document that has changed. This is the
        // pre-existing staleness check; it is kept *in addition to* correspondence, not as a stand-in
        // for it. Conflating the two is precisely what let the corruption through.
        let on_disk =
            crate::preview::markdown::task_source_locs(&body, &states, &cache.details_states);
        let matched = (drawn.len() == total && on_disk.len() == total)
            .then(|| drawn.get(ordinal))
            .flatten()
            .filter(|l| norm_state(l.state) == norm_state(expected))
            .and_then(|l| {
                let origin = cache.pre_origin.get(l.line).copied().flatten()?;
                let on_screen = cache.pre_src.lines().nth(l.line)?;
                let on_disk_line = body_lines.get(origin).copied()?;
                // The write replaces exactly one character at `state_off`, so what has to hold is
                // that the bytes up to and including it are the same in both texts — not that the
                // whole line is. A pre-pass may legitimately rewrite the line's *tail* (`- [ ] a[^1]`
                // is drawn as `- [ ] a¹`, `<kbd>x</kbd>` becomes a keycap), and demanding whole-line
                // equality would refuse those perfectly ordinary checkboxes. Nothing can rewrite the
                // part before the marker: a task line is indentation, a bullet, then the brackets, so
                // there is no construct there to rewrite — and comparing the prefix explicitly means
                // the write is safe even if that ever stopped being true.
                let end = l.state_off + l.state.len_utf8();
                let (a, b) = (on_screen.get(..end)?, on_disk_line.get(..end)?);
                (a == b).then_some((origin + line_offset, l.state_off, l.state))
            });
        let Some((line_no, state_off, cur)) = matched else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::TaskFileChanged).into());
            self.reload_preview();
            return;
        };
        let loc = crate::preview::markdown::TaskLoc {
            line: line_no,
            state_off,
            state: cur,
        };
        let loc = &loc;
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
