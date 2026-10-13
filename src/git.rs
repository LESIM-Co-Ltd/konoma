// Git reading (status, diffs, log, the commit-graph layout) lives in the `konoma-vcs-core` crate;
// re-exported here so every `crate::git::...` path in the host is unchanged. The core owns no
// translations: the words of the graph's "uncommitted changes" row come from `i18n::worktree_labels`.

pub use konoma_vcs_core::git::*;

#[cfg(test)]
mod tests {
    use crate::i18n::{tr, worktree_labels, Lang, Msg};
    use konoma_vcs_core::git::WorktreeLabels;

    /// The core repeats the English words as its `Default` (its own tests build graphs without a
    /// host); if the host's English table changes, this fails.
    #[test]
    fn the_cores_default_worktree_words_are_the_english_ones() {
        assert_eq!(WorktreeLabels::default(), worktree_labels(Lang::En));
    }

    #[test]
    fn the_worktree_words_handed_in_are_what_the_graph_row_always_said() {
        let en = worktree_labels(Lang::En);
        assert_eq!(en.uncommitted, "Uncommitted changes");
        assert_eq!(
            (&*en.staged, &*en.unstaged, &*en.untracked),
            ("staged", "unstaged", "untracked")
        );
        let jp = worktree_labels(Lang::Jp);
        assert_eq!(jp.uncommitted, "未コミットの変更");
        assert_eq!(
            (&*jp.staged, &*jp.unstaged, &*jp.untracked),
            ("ステージ済", "未ステージ", "未追跡")
        );
        for lang in [Lang::En, Lang::Jp] {
            assert_eq!(
                worktree_labels(lang).uncommitted,
                tr(lang, Msg::UncommittedChanges)
            );
        }
    }
}
