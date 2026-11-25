// The icon glyph tables live in the `konoma-widgets` crate; this path stays for the host. What
// stays here is the marker of the tree's repository chip, which names the host's `VcsKind`.

pub use konoma_widgets::icons::*;

/// Marker in front of the tree's repository chip.
///
/// git names a branch, so it gets the branch glyph. jj names the working-copy commit and its label
/// already opens with `@` — jj's own symbol for it — so a second marker would only repeat it.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
pub fn chip_marker(vcs: crate::vcs::VcsKind, icons: bool) -> &'static str {
    match vcs {
        crate::vcs::VcsKind::Git => branch_marker(icons),
        #[cfg(feature = "git")]
        crate::vcs::VcsKind::Jj => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::VcsKind;

    // --- chip_marker: the tree title chip's leading marker. git names a branch (gets the branch
    // glyph/label); jj's working-copy label already opens with its own `@`, so a second marker
    // would just repeat it.

    #[test]
    fn chip_marker_git_both_icon_settings() {
        assert_eq!(
            chip_marker(VcsKind::Git, true),
            "\u{2387}",
            "git+icons=true は ⎇"
        );
        assert_eq!(
            chip_marker(VcsKind::Git, false),
            "br:",
            "git+icons=false は ASCII ラベル br:"
        );
    }

    #[cfg(feature = "git")]
    #[test]
    fn chip_marker_jj_is_always_empty() {
        assert_eq!(
            chip_marker(VcsKind::Jj, true),
            "",
            "jj+icons=true はチップ記号を足さない(ラベルが既に @ で始まる)"
        );
        assert_eq!(chip_marker(VcsKind::Jj, false), "", "jj+icons=false も同様");
    }
}
