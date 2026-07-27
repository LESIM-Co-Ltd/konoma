use super::*;

impl App {
    // ---- タブ (FR-5) ----

    /// Snapshot the current tree working state.
    pub(super) fn snapshot_tab(&self) -> PerTab {
        self.tab.clone()
    }

    /// Save the current working state to the active tab.
    pub(super) fn save_active(&mut self) {
        // このタブのメディアはこの後 clear_image で捨てられる。1枠だけ退避しておき、戻ってきた時に
        // デコード/ラスタライズ/外部ツール起動をやり直さずに済ませる。
        self.stash_media_cache();
        let snap = self.snapshot_tab();
        self.tabs[self.active_tab] = snap;
    }

    /// Load the active tab's snapshot into the working fields.
    /// **Also restores that tab's mode/preview** (does not drop to Tree). Images are restored by reloading.
    /// The snapshot is **moved** out of the slot (no deep clone of entries/filter_pool/git vectors):
    /// while a tab is active its slot is never read (tab_label/tab_root use the live App fields)
    /// and every switch path calls save_active before the slot is needed again.
    fn load_active(&mut self) {
        let mut t = std::mem::take(&mut self.tabs[self.active_tab]);
        // preview_path/preview_kind are read below (table-hits check, media block, setup_windowed,
        // load_table) before the bulk `self.tab = t` restore at the end of this function, so they
        // are restored early here (mirroring `self.tab.md_raw = t.md_raw;` further down).
        self.tab.preview_path = t.preview_path.clone();
        self.tab.preview_kind = t.preview_kind.clone();
        // root/open_dir/entries/selected/show_hidden/tree_viewport/mode/preview_scroll/
        // preview_hscroll/preview_viewport/preview_byte_top/preview_top_line/selection/visual_anchor/
        // tree_filter/filter_input/filter_pool/changed_filter/preview_search/search_input/search_idx:
        // pure per-tab copies with no read anywhere below before `self.tab = t` restores the whole
        // PerTab bundle at the end of this function — nothing in between reads them, so no individual
        // line is needed here.
        // グラフのブランチ可視ピッカー(モーダル)はタブ跨ぎで持ち越さない: 復元時は閉じておく。
        // (git オーバーレイ本体・グラフ装飾状態は `self.tab = t` で後段まとめて復元される。)
        self.git_graph_picker = false;
        self.git_graph_picker_sel = 0;
        self.git_graph_picker_set.clear();
        self.git_graph_reordered = false;
        // 見出しアウトラインオーバーレイもタブ跨ぎで持ち越さない。
        self.outline_open = false;
        // <details> の開閉状態も文書ごと=タブ跨ぎで持ち越さない。
        self.details_open.clear();
        // `table_search_hits` は `search_matches` から導出される描画用の集合。PerTab には持たず
        // (二重管理を避ける)、復元した(まだ `t` に載ったままの)search_matches から作り直す
        // (`self.tab` 自体は末尾の `self.tab = t` まで前タブの値のままなので、ここでは `t` を
        // 直接読む=借用のみで `t` は消費しない)。表以外は空にする — さもないと別タブの一致セル
        // 座標が居残り、表 renderer(`table_cell_is_hit` を無条件参照)が誤って強調する。
        self.table_search_hits = if matches!(
            self.tab.preview_kind,
            Some(PreviewKind::Table { .. }) | Some(PreviewKind::Archive { .. })
        ) {
            t.search_matches.iter().map(|&(_, r, c)| (r, c)).collect()
        } else {
            std::collections::HashSet::new()
        };
        // 装飾キャッシュは持ち越さない (decorated_lines が再生成)。
        self.md_cache = None;
        // 復元した diff プレビューはフォロー由来の印を持ち越さない(セッションはタブ横断の概念でない)。
        self.diff_follow_scope = false;
        // 画像は重い状態(protocol/元画像/GIFフレーム)を持ち越さず、画像系プレビューなら再読込で復元する。
        // (clear_image は tab.image_center/tab.pdf_page/tab.pdf_pages も既定値へ戻すが、この関数の
        // 最後で `self.tab = t` が復元値を上書きするので、間で誰も読まない限り無害。)
        self.clear_image();
        if let (Some(kind), Some(path)) =
            (self.tab.preview_kind.clone(), self.tab.preview_path.clone())
        {
            // Mermaid(.mmd 画像モード)/全画面フェンスも媒体復元の対象(kind_loads_media)。
            // 漏れると clear_image 後に誰も start_media_load を呼ばず、reload_media_if_changed も
            // mtime 一致で早期 return するため、タブ復帰でテキスト図/偽エラーに劣化していた。
            if self.kind_loads_media(&kind) {
                // SVG/動画サムネ/GIF は別スレッドで読み込み開始(set_* は zoom/center を触らない)。
                // 保存値を即セットしておけば、後から結果が届いても復元したズーム/中心が保たれる。
                // GIF は先頭フレームから再生。
                // PDF は保存ページを start_media_load の前に戻す(その世代でそのページをラスタライズする)。
                // `t.pdf_page` をその場でクランプしておく: これから読む restore_media_cache の
                // 引数にも、末尾の `self.tab = t` が運ぶ最終値にも同じクランプ後の値を使わせる。
                t.pdf_page = t.pdf_page.max(1);
                // pdf_pages: 変換無しの純コピー。ここでは読まないので末尾の一括復元に任せる。
                // 直前に見ていたタブへ戻る等、同じファイル・同じ mtime・同じページなら、退避した
                // デコード済み画像をそのまま使う(PDF/動画の外部ツール起動やラスタライズをやり直さない)。
                let reused = self.restore_media_cache(&path, t.pdf_page);
                if !reused {
                    self.start_media_load(&kind, &path);
                }
                self.tab.image_zoom = t.image_zoom;
                // image_center: 変換無しの純コピー。ここでは読まないので末尾の一括復元に任せる。
                self.image_crop = None;
                if reused {
                    // 復元は apply_payload を通らないので、そこで走るはずのシャープ再ラスタが
                    // 起動しない。再ラスタ中にタブを離れた SVG/mermaid がボケたまま戻るのを防ぐ
                    // (ズーム復元後に呼ぶ＝必要密度は復元後の値で判定される)。
                    self.maybe_sharpen_vector();
                }
            }
        }
        // raw ソース表示状態を復元してから windowed を張り直す(raw md は窓読みにするため順序が重要)。
        // setup_windowed が is_raw_source 経由で読むので、末尾の一括復元を待たずここで写す
        // (Copy なので t は消費されず、末尾の `self.tab = t` にも同じ値がそのまま乗る)。
        self.tab.md_raw = t.md_raw;
        // Tab フォーカス/インライン図のズーム/全画面復帰情報はこのタブの保存値へ(前タブの値を
        // 別文書に適用しない)。md_items は次描画の ensure_md_cache がこのタブの文書で再構築し、
        // focused_item はその時に範囲へクランプされる(それまで旧文書の items を参照しないよう空に)。
        // (focused_item/fence_zoom/fence_center/fence_return はここでは読まれないので末尾の
        // `self.tab = t` に任せる。)
        self.md_items.clear();
        // 大きい Code/Text(＋raw の Markdown/Mermaid)なら ウィンドウ読みリーダを張り直す(byte_top は末尾で復元される)。
        self.setup_windowed();
        // windowed プレビューの 2D キャレットは末尾の `self.tab = t` で復元される(選択は持ち越さない)。
        // 範囲は次描画/移動でクランプされる。
        self.preview_visual_anchor = None;
        self.preview_visual_linewise = false;
        // CSV/TSV テーブルは本体を再パースし、保存済みカーソル/スクロールを復元してクランプする。
        self.load_table();
        self.tab = t;
        self.clamp_table_cursor();

        // 切替でアクティブになったタブを**ディスクから再読み込み**する。ファイル監視はアクティブな
        // root しか見ていない(rewatch)ため、裏に居た間の外部変更(ファイルの作成/削除/リネーム・git
        // 状態・変更ガター/diff)がスナップショットのまま古い。fs イベント時と同じ refresh_fs 経路を
        // 通して、ツリー再構築・git status・diff/gutter キャッシュ・変更フィルタ・git ビュー・プレビュー
        // 再読込を一括で追従させる。`false`=重い ignore セット(gitignore)は作り直さずキャッシュ保持
        // (root が変われば次の描画の refresh_git_if_needed が workdir 単位キャッシュで面倒を見る＝
        // 巨大 repo の性能対策[[git-watch-feedback-loop-perf]]を壊さない)。プレビューの表示位置は
        // reload_preview が保持する(スクロール/ズーム/テーブルカーソル)。
        // タブは背面に居た間 fs 監視の対象外なので、そのタブの git status は外部変更を取りこぼして
        // いる可能性がある(同一 repo の別タブなら workdir 単位キャッシュで流用され陳腐化が居残る)。
        // 切替=再検証の節目として dirty を立て、次の描画の refresh_git_if_needed に取り直させる
        // (back_to_tree と同型。h/l 連打は dirty を立てないので最適化は保たれる)。
        self.git_status_dirty = true;
        // プレビュー本体は上で既にディスクから組み直しているので、ここでは**再読込しない**
        // (従来は refresh_fs → reload_preview が同じ仕事を繰り返し、CSV タブは全行パースが2回走っていた)。
        let _ = self.refresh_fs_after_tab_switch();
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    /// Display name of tab `i`. While showing Tree, the **root directory name**; while showing Preview/image, etc.,
    /// the **preview target's file name**. The active tab references the latest working state (App fields),
    /// while inactive tabs reference the snapshot (`PerTab`) (since saving happens only on switch).
    pub fn tab_label(&self, i: usize) -> String {
        // (mode, root, preview_path) をアクティブ/非アクティブで使い分ける。
        let (mode, root, preview) = if i == self.active_tab {
            (
                self.tab.mode,
                self.tab.root.as_path(),
                self.tab.preview_path.as_deref(),
            )
        } else if let Some(t) = self.tabs.get(i) {
            (t.mode, t.root.as_path(), t.preview_path.as_deref())
        } else {
            return String::new();
        };
        let path = match mode {
            Mode::Preview => preview.unwrap_or(root), // 念のため preview 無しは root へ
            Mode::Tree => root,
        };
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }

    /// New tab. Create another tree context starting from the current root and switch to it.
    /// The new tab starts in Tree (since it has no preview target). The source tab's preview is
    /// preserved by `save_active`, so it is not lost.
    pub fn tab_new(&mut self) -> Result<()> {
        self.save_active();
        // 見出しアウトラインオーバーレイは新タブへ持ち越さない(空タブ上の空オーバーレイを防ぐ)。
        self.outline_open = false;
        let root = self.tab.root.clone();
        self.tab.open_dir = root.clone();
        self.tab.root = root;
        self.tab.selected = 0;
        self.tab.entries.clear();
        self.rebuild_tree()?;
        // snapshot 前に Tree へリセットする (snapshot がプレビュー状態も取り込むため順序が重要)。
        self.tab.mode = Mode::Tree;
        self.clear_image();
        self.tab.preview_path = None;
        self.tab.preview_kind = None;
        self.tab.preview_scroll = 0;
        self.tab.preview_hscroll = 0;
        self.tab.preview_byte_top = 0;
        self.tab.preview_top_line = 0;
        self.preview_win = None;
        self.win_cache = None;
        self.preview_total_lines = None;
        self.md_cache = None;
        // 新規タブは md フォーカス/インライン図ズームも素の状態から(PerTab 複製対象)。
        self.md_items.clear();
        self.tab.focused_item = None;
        self.tab.fence_zoom = 1.0;
        self.tab.fence_center = (0.5, 0.5);
        self.tab.fence_return = None;
        // 新規タブは git オーバーレイ無しの素の Tree から始める(現タブの git 状態は save_active で保持済み)。
        self.tab.git_view = false;
        self.tab.git_view_sel = 0;
        self.tab.git_view_entries.clear();
        self.tab.came_from_git_view = false;
        self.tab.git_log = None;
        self.tab.git_log_sel = 0;
        self.tab.git_detail = None;
        self.tab.git_detail_meta = None;
        self.tab.git_detail_title = None;
        self.tab.git_detail_scroll = 0;
        self.tab.git_detail_hscroll = 0;
        self.tab.git_detail_viewport = 0;
        self.tab.git_detail_total = 0;
        self.tab.git_branches = None;
        self.tab.git_branch_sel = 0;
        self.tab.git_branch_filter.clear();
        self.tab.git_branch_filtering = false;
        self.tab.git_graph = None;
        self.tab.git_graph_sel = 0;
        // 新規タブはグラフの装飾状態(基準/凡例/表示ブランチ/優先順)とピッカーも素から始める
        // (現タブのこれらは直前の save_active で PerTab に保持済み)。次に開くとき再導出される。
        self.tab.git_graph_base = None;
        self.tab.git_graph_base_label = None;
        self.tab.git_graph_visible.clear();
        self.tab.git_graph_legend.clear();
        self.tab.git_graph_hidden = 0;
        self.tab.git_graph_order.clear();
        self.git_graph_picker = false;
        self.git_graph_picker_sel = 0;
        self.git_graph_picker_set.clear();
        self.git_graph_reordered = false;
        // 新規タブは選択 / 絞り込み / プレビュー検索を空から始める
        // (現タブのこれらは直前の save_active で PerTab に保持済み)。
        self.tab.selection.clear();
        self.tab.visual_anchor = None;
        self.clear_filter_state();
        self.search_clear();
        self.tabs.push(self.snapshot_tab());
        self.active_tab = self.tabs.len() - 1;
        // タブ集合が変わる節目でセッションを保存する(異常終了でも直近のタブ構成が残る)。
        self.save_session();
        Ok(())
    }

    /// `Ctrl-t` in the tree: open the entry under the cursor in a new (foreground) tab, leaving the
    /// current tab untouched. A file opens as a preview; a directory becomes the new tab's root
    /// (mirroring `l`). No-op when the tree is empty.
    pub fn tab_new_from_selection(&mut self) -> Result<()> {
        let Some(entry) = self.tab.entries.get(self.tab.selected).cloned() else {
            return Ok(());
        };
        self.tab_new()?; // fresh Tree tab at the current root, now active
        if entry.is_dir {
            // Descend into the folder in the new tab (same as `l`); root changes, so drop stale state.
            self.clear_for_root_change();
            self.tab.root = entry.path;
            self.tab.entries.clear();
            self.tab.selected = 0;
            self.rebuild_tree()?;
        } else {
            // Reveal the file (so returning with `q` lands on it), then preview it.
            let _ = self.reveal_path_deep(&entry.path);
            self.enter_preview(&entry.path);
        }
        self.save_session();
        Ok(())
    }

    /// Close the active tab. The last one is not closed.
    pub fn tab_close(&mut self) {
        if self.tabs.len() <= 1 {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::CantCloseLastTab).into());
            return;
        }
        // 閉じるタブのメディアはもう誰も戻ってこない。1枠のキャッシュが握り続けると、
        // どのタブとも無関係な画像が常駐したままになる。
        self.drop_media_cache_for_active();
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.load_active();
        self.save_session();
    }

    /// Move tabs relatively (dir: +1=next / -1=previous). Wraps at the ends.
    pub fn tab_cycle(&mut self, dir: i32) {
        self.tab_list = false; // 一覧から [/] で切替えた場合も一覧は用済み
        if self.tabs.len() <= 1 {
            return;
        }
        self.save_active();
        let n = self.tabs.len() as i32;
        self.active_tab = (self.active_tab as i32 + dir).rem_euclid(n) as usize;
        self.load_active();
        self.save_session();
    }

    /// Select a tab by number (0-based). Out-of-range or the same tab is ignored.
    pub fn tab_goto(&mut self, i: usize) {
        self.tab_list = false; // 一覧(1-9/Enter)からの切替で一覧を閉じる
        if i >= self.tabs.len() || i == self.active_tab {
            return;
        }
        self.save_active();
        self.active_tab = i;
        self.load_active();
        self.save_session();
    }

    // --- タブ一覧オーバーレイ (`T`) ----------------------------------------
    pub fn is_tab_list(&self) -> bool {
        self.tab_list
    }
    /// `T`: open/close the tab list. Opening puts the selection on the active tab.
    pub fn toggle_tab_list(&mut self) {
        self.tab_list = !self.tab_list;
        if self.tab_list {
            self.tab_list_sel = self.active_tab;
        }
    }
    pub fn tab_list_sel(&self) -> usize {
        self.tab_list_sel
    }
    pub fn tab_list_move(&mut self, delta: i32) {
        let n = self.tabs.len();
        if n == 0 {
            return;
        }
        self.tab_list_sel = (self.tab_list_sel as i32 + delta).rem_euclid(n as i32) as usize;
    }
    /// Enter in the list: switch to the selected tab (closing the list; same tab = just close).
    pub fn tab_list_activate(&mut self) {
        let i = self.tab_list_sel;
        self.tab_list = false;
        self.tab_goto(i);
    }
    /// `w` in the list: close the **selected** tab (the list stays open, like the bookmark list).
    /// Closing the active tab switches to a neighbor; the last tab refuses with a flash.
    pub fn tab_list_close_selected(&mut self) {
        if self.tabs.len() <= 1 {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::CantCloseLastTab).into());
            return;
        }
        let i = self.tab_list_sel;
        if i == self.active_tab {
            self.tab_close();
        } else {
            // 閉じる(非アクティブ)タブのメディアをキャッシュが握っていたら手放す。
            if let Some(t) = self.tabs.get(i) {
                let closing = t.preview_path.clone();
                if matches!((&self.media_cache, &closing), (Some(c), Some(p)) if &c.path == p) {
                    self.media_cache = None;
                }
            }
            self.tabs.remove(i);
            if self.active_tab > i {
                self.active_tab -= 1;
            }
        }
        self.tab_list_sel = self.tab_list_sel.min(self.tabs.len() - 1);
        self.save_session();
    }
    /// Root directory of tab `i` (for the tab list's path column).
    pub fn tab_root(&self, i: usize) -> std::path::PathBuf {
        if i == self.active_tab {
            self.tab.root.clone()
        } else {
            self.tabs
                .get(i)
                .map(|t| t.root.clone())
                .unwrap_or_else(|| self.tab.root.clone())
        }
    }
}
