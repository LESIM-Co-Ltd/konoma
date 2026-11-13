// Unresolved contracts retain the old literal and both diagnostic frames.
struct PendingBehavior {
    old: &'static str,
    assertion: &'static str,
    marker: &'static str,
    literal: &'static str,
    old_frame: &'static str,
    new_frame: &'static str,
}

include!("vcs_port_pending_frames.rs");
const PENDING_BEHAVIORS: &[PendingBehavior] = &[
    PendingBehavior { old: "e2e_space_menu_follows_the_footer_order_and_labels_in_git", assertion: "space:log:min-three", marker: "PENDING_DECISION:project-space-shared-operations", literal: "want.len() >= 3; detail, search, screens and back are declared operations", old_frame: CAP_SPACE_LOG_OLD, new_frame: CAP_SPACE_LOG_NEW },
];
#[test]
fn pending_behavior_decisions_keep_old_literals_and_both_frames() {
    let expected = [
        ("e2e_space_menu_follows_the_footer_order_and_labels_in_git","space:log:min-three","PENDING_DECISION:project-space-shared-operations","want.len() >= 3; detail, search, screens and back are declared operations"),
    ];
    let actual = PENDING_BEHAVIORS
        .iter()
        .map(|p| (p.old, p.assertion, p.marker, p.literal))
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "pending behavior set and old literals are fixed"
    );
    // Structural check: each frame pair shows the screen its row names. The old hub prints its
    // mode label first (`GIT LOG`, `JJ HELP`, ...), the `p` path prints `Project` (+ `HELP`).
    fn first_line(frame: &str) -> String {
        frame
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    fn expected_modes(p: &PendingBehavior) -> (&'static str, &'static str) {
        let a = p.assertion;
        match p.old {
            "e2e_space_menu_follows_the_footer_order_and_labels_in_git" => ("GIT LOG", "Project"),
            other => panic!("no expected screen for {other}/{a}"),
        }
    }
    for p in PENDING_BEHAVIORS {
        let (old_mode, new_mode) = expected_modes(p);
        let (old_head, new_head) = (first_line(p.old_frame), first_line(p.new_frame));
        assert!(
            old_head.starts_with(old_mode),
            "{}/{}: the old frame shows `{old_head}`, expected `{old_mode}`",
            p.old,
            p.assertion
        );
        let new_ok = if new_mode == "ja" {
            !new_head.is_empty() && !new_head.contains("JJ") && !new_head.contains("GIT")
        } else if new_mode == "Project" {
            new_head.starts_with("Project") && !new_head.contains("HELP")
        } else {
            new_head.starts_with(new_mode)
        };
        assert!(
            new_ok,
            "{}/{}: the new frame shows `{new_head}`, expected `{new_mode}`",
            p.old, p.assertion
        );
    }
    // Content check per row: each frame must show words of the screen its row names (the old
    // frame the old hub's words, the new frame the `p` path's), so two rows' frames swapped
    // fail even when both show the same mode label.
    fn screen_words(p: &PendingBehavior) -> (&'static [&'static str], &'static [&'static str]) {
        match (p.old, p.assertion) {
            ("e2e_space_menu_follows_the_footer_order_and_labels_in_git", _) => {
                (&["Git log", "Actions ▸ l:detail"], &["Actions ▸ a:detail", "w:Worktrees"])
            }
            (other, a) => panic!("no screen words for {other}/{a}"),
        }
    }
    for p in PENDING_BEHAVIORS {
        let (old_words, new_words) = screen_words(p);
        // The dump pads cells with runs of spaces; compare with the runs squeezed.
        let squeeze = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let (old_text, new_text) = (squeeze(p.old_frame), squeeze(p.new_frame));
        for word in old_words {
            assert!(
                old_text.contains(word),
                "{}/{}: the old frame lacks `{word}`",
                p.old,
                p.assertion
            );
        }
        for word in new_words {
            assert!(
                new_text.contains(word),
                "{}/{}: the new frame lacks `{word}`",
                p.old,
                p.assertion
            );
        }
    }
    for (frame, want) in [
        (CAP_SPACE_LOG_NEW, "Actions ▸"),
        (CAP_SPACE_LOG_OLD, "Actions ▸"),
    ] {
        assert!(frame.contains(want), "frame lacks `{want}`");
    }
    for p in PENDING_BEHAVIORS {
        assert!(
            !p.old_frame.is_empty()
                && !p.new_frame.is_empty()
                && !p.new_frame.contains("pending capture"),
            "both frames for {}/{}",
            p.old,
            p.assertion
        );
    }
    let actual: BTreeSet<_> = port_map()
        .into_iter()
        .filter(|(_, _, n)| {
            n.starts_with("PENDING_DECISION:")
                && !matches!(
                    *n,
                    "PENDING_DECISION:title-count" | "PENDING_DECISION:title-label"
                )
        })
        .collect();
    let expected: BTreeSet<_> = PENDING_BEHAVIORS
        .iter()
        .map(|p| (p.old, p.assertion.to_owned(), p.marker))
        .collect();
    assert_eq!(
        actual, expected,
        "every unresolved behavior remains in the ledger"
    );
}
