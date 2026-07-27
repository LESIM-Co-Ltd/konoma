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
        let Some(path) = self.tab.preview_path.clone() else {
            return;
        };
        let states = self.cfg.ui.md_task_state_chars();

        // 全文を1回だけ読む: 書込みは全文に対して行う(切り詰め接頭辞を書き戻すとファイルの
        // 末尾を消し飛ばす)。走査だけはレンダラが実際に見た範囲に合わせる(下記)。
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
        // レンダラ(`build_decorated`)は `preview::text::load` の上限(MAX_BYTES/MAX_LINES)で
        // 切り詰めた接頭辞だけを見る。スキャナが全文を見ると、切り詰め範囲より後ろにある
        // タスクまで数えてしまい、画面上の個数(rendered)と食い違って**そのファイルのトグルが
        // 全部拒否される**(5,000行超のMarkdownで再現)。上限の定義は `preview::text` の
        // ものを再利用する(ここで二重に定義しない)。前提: この接頭辞は生ファイルの先頭からの
        // バイト位置と一致するので、走査で得た (line, state_off) はそのまま `full_src` にも
        // 通用する — 書込みだけ `full_src` に対して行うことで安全に両立する。
        let (capped_lines, _) = crate::preview::text::cap_lines(full_src.as_bytes());
        let capped = capped_lines.join("\n");
        // front matter もレンダラは本文から剥がして描く(タスクとしては扱わない)。同じ規則で
        // 剥がし、除去した行数をオフセットとして見つかった行番号へ足し戻す(front matter が無い/
        // 設定 OFF なら剥がれず offset=0)。
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
        // 書込み前の照合: 個数と対象マーカーの現状態が画面表示と一致するときだけ書く。
        // `<details>` の開閉は実行時状態。描画に使ったのと同じ開閉列を渡さないと、閉じたブロックの
        // 本文まで数えてしまい個数が合わなくなる(直近の描画が md_cache に記録している)。
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
            None => states[0], // 配列外の状態は先頭へ正規化
        };
        // 状態文字 1 文字だけを置換(他バイトは不変・CRLF/末尾改行も保たれる)。全文(full_src)に
        // 対して書くので、切り詰め/front matter で見えていない範囲もそのまま保存される。
        let mut lines: Vec<String> = full_src.split('\n').map(str::to_string).collect();
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
