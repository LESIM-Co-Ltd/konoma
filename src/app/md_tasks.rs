//! Markdown task-list checkboxes: toggling the focused checkbox writes the new state back to the
//! source file — methods on `App`. This is a structured one-char edit (like a rename), not an
//! in-app editor: the write happens only after re-scanning the current file and confirming the
//! target marker still matches what is on screen, so it cannot clobber a concurrent external edit.

use super::*;

impl App {
    /// Space/Enter on a focused checkbox: cycle its state to the next entry of `ui.md_task_states`
    /// and write the single state char back to the file. On any mismatch with the file on disk
    /// (changed externally) nothing is written — flash + reload instead (safe fallback, principle #3).
    ///
    /// The final step of the `md-block-walk` migration: `MdItemKind::Task::state_at` is the model
    /// render pass's own byte offset for *this exact checkbox*, into `MdCache::pre_src` — pushed by
    /// `render_item` at the moment it drew this checkbox's own marker (`render::RenderOut::tasks`),
    /// or, for the rare legacy fallback, resolved once (by ordinal, no document-wide count) when
    /// `app::md_render::build_decorated` built the cache — see that function's own doc comment. Either
    /// way, there is no second, independent re-scan of `pre_src` left to reconcile by count against
    /// what is on screen (what the old `task_source_locs`-based "drawn" scan here used to be): the
    /// **position** this function starts from is not a re-derivation, it is the render pass's own
    /// record — the count-based correspondence bug this closed (v0.23.5 — a document could hide one
    /// checkbox from the screen and reveal a different one to a re-scan of the file while both totals
    /// and state characters stayed equal, silently writing the wrong line) cannot recur, because there
    /// is no second count left to *agree by coincidence*.
    ///
    /// What **is** still verified, at write time, is genuinely position-based, not count-based: the
    /// external-edit safety net principle #3 calls for.
    ///   1. `state_at` → `(line, byte offset within that line)` in `pre_src` (`byte_to_line_offset`);
    ///   2. follow `pre_origin` back to the body line that line came from — a line a pre-pass invented
    ///      (the footnotes section, the tail of a `<br>` split) has no origin at all;
    ///   3. require that body line, **read fresh from disk right now**, to be byte-identical up to
    ///      and including the state char, which both proves the byte offset transfers verbatim and
    ///      catches an external edit since the render.
    ///
    /// Anything that cannot be matched refuses (principle #3) — refusing costs the reader one
    /// keypress, writing to the wrong line costs them their file.
    pub fn md_toggle_focused_task(&mut self) {
        if self.is_raw_source() {
            return; // the raw-source view is the 2D-caret surface (md_items belongs to the decorated view)
        }
        let Some(f) = self.tab.focused_item else {
            return;
        };
        // `state_at: None` only for the vanishingly rare legacy-fallback item this position could not
        // be resolved for at cache-build time (see `MdItemKind::Task`'s own doc comment) — a quiet
        // no-op, the same as focusing nothing: never write against a position that isn't known.
        let Some(MdItem {
            kind:
                MdItemKind::Task {
                    state_at: Some(byte_off),
                    ..
                },
            ..
        }) = self.md_items.get(f)
        else {
            return;
        };
        let byte_off = *byte_off;
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        let states = self.cfg.ui.md_task_state_chars();

        // Read the full text once: the write is done against the full text (writing back a
        // truncated prefix would blow away the end of the file). Only the verification below is
        // matched to the range the renderer actually saw.
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
        // cap (MAX_BYTES/MAX_LINES). Premise: this prefix's byte positions match the raw file's from
        // the start, so the (line, state_off) obtained below also holds for `full_src` — writing only
        // against `full_src` keeps both safely consistent.
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
        let Some(cache) = self.md_cache.as_ref() else {
            self.flash = Some(crate::i18n::tr(self.lang, crate::i18n::Msg::TaskFileChanged).into());
            self.reload_preview();
            return;
        };
        let matched = crate::preview::markdown::byte_to_line_offset(&cache.pre_src, byte_off)
            .and_then(|(line, off)| {
                let on_screen = cache.pre_src.split('\n').nth(line)?;
                // The char this render pass recorded at `byte_off` — always `md_items[f]`'s own
                // `state` by construction (same cache, same build), read fresh off `pre_src` rather
                // than trusted blind, so a `Writer`/scanner bug that ever let the two drift would show
                // up as a refusal here, not a write to the wrong byte.
                let cur = on_screen.get(off..)?.chars().next()?;
                let origin = cache.pre_origin.get(line).copied().flatten()?;
                let body_lines: Vec<&str> = body.lines().collect();
                let on_disk_line = body_lines.get(origin).copied()?;
                // The write replaces exactly one character at `off`, so what has to hold is that the
                // bytes up to and including it are the same in both texts — not that the whole line
                // is. A pre-pass may legitimately rewrite the line's *tail* (`- [ ] a[^1]` is drawn as
                // `- [ ] a¹`, `<kbd>x</kbd>` becomes a keycap), and demanding whole-line equality would
                // refuse those perfectly ordinary checkboxes. Nothing can rewrite the part before the
                // marker: a task line is indentation, a bullet, then the brackets, so there is no
                // construct there to rewrite — and comparing the prefix explicitly means the write is
                // safe even if that ever stopped being true.
                let end = off + cur.len_utf8();
                let (a, b) = (on_screen.get(..end)?, on_disk_line.get(..end)?);
                (a == b).then_some((origin + line_offset, off, cur))
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
            return; // unreachable: `loc.line` came from `full_src.split('\n')` itself (defensive)
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
