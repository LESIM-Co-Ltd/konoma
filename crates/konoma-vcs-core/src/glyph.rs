//! The glyph of a commit-graph node.

use crate::git::NodeKind;
use crate::vcs::VcsKind;

/// Glyph for one commit-graph node. **The backend decides the [`NodeKind`];
/// this table decides the glyph.** Keeping them apart is what lets the node cell be located by
/// position rather than by matching on the character, so a second VCS can bring its own symbols
/// (jj draws `@` for the working copy, `○` for an ordinary commit, `×` for a conflict) without
/// breaking the code that paints the legend and the selected row.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn node_glyph(kind: NodeKind, vcs: VcsKind) -> char {
    use NodeKind as K;
    #[cfg(feature = "git")]
    if vcs == VcsKind::Jj {
        // jj's own symbols. A jj user reads `@` as "where I am" everywhere jj prints a graph, and
        // `◆` means immutable there — the opposite of git's "this is a merge".
        return match kind {
            K::WorkingCopy => '@',
            K::Normal | K::Merge => '\u{25cb}', // ○
            K::Immutable => '\u{25c6}',         // ◆
            K::Conflict => '\u{00d7}',          // ×
        };
    }
    let _ = vcs;
    match kind {
        // git has no glyph of its own for the working copy: its pseudo-row is drawn as an ordinary
        // node and recoloured yellow-bold afterwards.
        K::Normal | K::WorkingCopy => '\u{25cf}', // ●
        K::Merge | K::Immutable => '\u{25c6}',    // ◆
        K::Conflict => '\u{00d7}',                // ×
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- node_glyph: the backend picks the *meaning* (NodeKind), this table picks the *glyph*.
    // git and jj deliberately fold different pairs of kinds together (see the asymmetry test
    // below) — these tests spell out all 5*2=10 (kind, vcs) combinations as an outside promise
    // ("jj's working copy is @", "a conflict is x"), not a copy of the match arms, so a swapped
    // glyph breaks a test whose name says what should have been drawn.

    #[test]
    fn node_glyph_git_all_five_kinds() {
        assert_eq!(
            node_glyph(NodeKind::Normal, VcsKind::Git),
            '\u{25cf}',
            "git の通常コミットは ●"
        );
        assert_eq!(
            node_glyph(NodeKind::Merge, VcsKind::Git),
            '\u{25c6}',
            "git のマージは ◆"
        );
        assert_eq!(
            node_glyph(NodeKind::WorkingCopy, VcsKind::Git),
            '\u{25cf}',
            "git に作業コピー専用の字は無く、Normal と同じ ●(擬似行は色で後から塗り分ける)"
        );
        assert_eq!(
            node_glyph(NodeKind::Immutable, VcsKind::Git),
            '\u{25c6}',
            "git に Immutable の概念は無く、Merge と同じ ◆ に落ちる"
        );
        assert_eq!(
            node_glyph(NodeKind::Conflict, VcsKind::Git),
            '\u{00d7}',
            "衝突は ×"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn node_glyph_jj_all_five_kinds() {
        assert_eq!(
            node_glyph(NodeKind::WorkingCopy, VcsKind::Jj),
            '@',
            "jj の作業コピーは @"
        );
        assert_eq!(
            node_glyph(NodeKind::Normal, VcsKind::Jj),
            '\u{25cb}',
            "jj の通常コミットは ○"
        );
        assert_eq!(
            node_glyph(NodeKind::Merge, VcsKind::Jj),
            '\u{25cb}',
            "jj はマージを通常コミットと同じ ○ に畳む(git と違い ◆ にはしない)"
        );
        assert_eq!(
            node_glyph(NodeKind::Immutable, VcsKind::Jj),
            '\u{25c6}',
            "jj の不変コミットは ◆"
        );
        assert_eq!(
            node_glyph(NodeKind::Conflict, VcsKind::Jj),
            '\u{00d7}',
            "衝突は ×"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn git_and_jj_fold_different_kind_pairs_into_the_same_glyph() {
        // The asymmetry this table exists to protect: git has no "immutable" concept, so it
        // reuses Merge's ◆ for Immutable too — but jj *does* distinguish them (◆ is Immutable
        // only). jj instead folds Merge into Normal's ○ — but git keeps those two apart (● vs
        // ◆). Losing either distinction (e.g. by copy-pasting one backend's fallback arm into the
        // other's branch) would silently mislabel a commit's meaning.
        assert_eq!(
            node_glyph(NodeKind::Merge, VcsKind::Git),
            node_glyph(NodeKind::Immutable, VcsKind::Git),
            "git: Merge と Immutable は同じ字のはず"
        );
        assert_ne!(
            node_glyph(NodeKind::Merge, VcsKind::Jj),
            node_glyph(NodeKind::Immutable, VcsKind::Jj),
            "jj: Merge と Immutable は別の字でなければならない"
        );
        assert_eq!(
            node_glyph(NodeKind::Normal, VcsKind::Jj),
            node_glyph(NodeKind::Merge, VcsKind::Jj),
            "jj: Normal と Merge は同じ字のはず"
        );
        assert_ne!(
            node_glyph(NodeKind::Normal, VcsKind::Git),
            node_glyph(NodeKind::Merge, VcsKind::Git),
            "git: Normal と Merge は別の字でなければならない"
        );
    }
}
