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
            return; // raw ソース表示は 2D キャレット面(md_items は装飾表示のもの)
        }
        let Some(f) = self.focused_item else { return };
        let Some(MdItem {
            kind: MdItemKind::Task { state },
            ..
        }) = self.md_items.get(f)
        else {
            return;
        };
        let expected = *state;
        // このチェックボックスが文書内で何個目か(リンクを除いたタスク序数)。
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
        let Some(path) = self.preview_path.clone() else {
            return;
        };
        let states = self.cfg.ui.md_task_state_chars();

        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.flash = Some(format!(
                    "{}{e}",
                    crate::i18n::tr(self.lang, crate::i18n::Msg::OperationFailed)
                ));
                return;
            }
        };
        // 書込み前の照合: 個数と対象マーカーの現状態が画面表示と一致するときだけ書く。
        let locs = crate::preview::markdown::task_source_locs(&src, &states);
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
            None => states[0], // 配列外の状態は先頭へ正規化
        };
        // 状態文字 1 文字だけを置換(他バイトは不変・CRLF/末尾改行も保たれる)。
        let mut lines: Vec<String> = src.split('\n').map(str::to_string).collect();
        let Some(line) = lines.get_mut(loc.line) else {
            return; // task_source_locs 由来なので到達しない(防御)
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
        // 再描画(md_cache 破棄)。アイテム数は不変なのでフォーカスは同じチェックボックスに残る。
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
