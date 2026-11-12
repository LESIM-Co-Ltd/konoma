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
const PENDING_FRAME_7: &str = "                                                                                                                                                  JJ   HELP   sort: name ↑  path:rel\n┌ jj @ puqqprxs 次4の1日 本 語u Summary  (1) ─────────────────┌ Help  (?/Esc to close, j/k to scroll) ─────────────────────────┐────────────────────────────────────────────────────────┐\n│  A vcs_log_jj_78344_1/changed.txt                      │jj changes (p)                                                  │                                                        │\n│                                                        │  g / G           top / bottom                                  │                                                        │\n│                                                        │  l / Enter       file diff                                     │                                                        │\n│                                                        │  d               diff of everything against @-                 │                                                        │\n│                                                        │  b               bookmarks                                     │                                                        │\n│                                                        │  s               let jj snapshot the working copy (confirm)    │                                                        │\n│                                                        │  Ctrl-t          open the entry under the cursor in a new tab  │                                                        │\n│                                                        │  /               filter the rows by the text they show         │                                                        │\n│                                                        │  r               refresh listing & git status                  │                                                        │\n│                                                        │  Tab / ⇧Tab      cycle changes/log/graph/bookmarks             │                                                        │\n│                                                        │  !               external jj tool (lazyjj)                     │                                                        │\n│                                                        │  h / Esc  ·  q   close jj view                                 │                                                        │\n│                                                        │                                                                │                                                        │\n│                                                        │Movement                                                        │                                                        │\n│                                                        │  j / k           move                                          │                                                        │\n│                                                        │                                                                │                                                        │\n│                                                        │Launch                                                          │                                                        │\n│                                                        │  e               edit in external editor ($EDITOR / config)    │                                                        │\n│                                                        │  o               open in another app (OS default / Office)     │                                                        │\n│                                                        │                                                                │                                                        │\n│                                                        │Actions                                                         │                                                        │\n│                                                        │  Space l         diff                                          │                                                        │\n│                                                        │  Space d         diff-all                                      │                                                        │\n│                                                        │  Space a         log                                           │                                                        │\n│                                                        │  Space g         graph                                         │                                                        │\n│                                                        │  Space b         bookmarks                                     │                                                        │\n│                                                        │  Space s         sync                                          │                                                        │\n│                                                        │  Space !         tool                                          │                                                        │\n│                                                        │  Space /         search                                        │                                                        │\n│                                                        │  Space f         next screen                                   │                                                        │\n│                                                        │  Space i         previous screen                               │                                                        │\n│                                                        │  Space h         close                                         │                                                        │\n│                                                        │  Space e         edit                                          │                                                        │\n│                                                        │  Space o         open in app                                   │                                                        │\n│                                                        │  Space j         open in a new tab                             │                                                        │\n│                                                        │  Space r         refresh                                       │                                                        │\n└────────────────────────────────────────────────────────└────────────────────────────────────────────────────────────────┘────────────────────────────────────────────────────────┘\nj/k:scroll  g/G:top/bottom  ?/q/Esc:close";
const PENDING_FRAME_8: &str = "                                                                                                                                                           Project   HELP   path:rel\n p:Changes  Log  Graph  b:Bookmarks\n  A vcs_log_jj_78344_1/changed.txt\n\n\n\n\n\n\n\n                                                         ┌ Help  (?/Esc to close, j/k to scroll) ─────────────────────────┐\n                                                         │jj                                                              │\n                                                         │  s               let jj snapshot the working copy (confirm)    │\n                                                         │  Enter           Open the file diff                            │\n                                                         │                                                                │\n                                                         │Project                                                         │\n                                                         │  Ctrl-t          open the entry under the cursor in a new tab  │\n                                                         │  l/→             diff                                          │\n                                                         │  Tab / Shift-Tab Cycle project screens                         │\n                                                         │  q / Esc         Pop this screen or return to the previous view│\n                                                         │  j/k / ↑/↓       scroll                                        │\n                                                         │  /               filter by name                                │\n                                                         │  p               Changes                                       │\n                                                         │  b               Bookmarks                                     │\n                                                         │                                                                │\n                                                         │Actions                                                         │\n                                                         │  Space a         diff                                          │\n                                                         │  Space s         sync                                          │\n                                                         └────────────────────────────────────────────────────────────────┘\n\n\n\n\n\n\n\n\n\n\nj/k:scroll  g/G:top/bottom  ?/q/Esc:close";
const PENDING_FRAME_9: &str = "                                                                                                                                                 GIT   HELP   sort: name ↑  path:rel\n┌ Git ⎇ master  (7) ─────────────────────────────────────┌ Help  (?/Esc to close, j/k to scroll) ─────────────────────────┐────────────────────────────────────────────────────────┐\n│● M konoma_e2e_vcs_literal_capture_67812_0/a-staged.txt │Git changes (p)                                                 │                                                        │\n│  M konoma_e2e_vcs_literal_capture_67812_0/b-unstaged.tx│  G               top / bottom                                  │                                                        │\n│  D konoma_e2e_vcs_literal_capture_67812_0/c-deleted.txt│  s / S           stage / stage all                             │                                                        │\n│● R konoma_e2e_vcs_literal_capture_67812_0/d-renamed.txt│  u / U           unstage / unstage all                         │                                                        │\n│  U konoma_e2e_vcs_literal_capture_67812_0/e-untracked.t│  x               discard file (confirm)                        │                                                        │\n│● A konoma_e2e_vcs_literal_capture_67812_0/f-added.txt  │  c               commit (staged)                               │                                                        │\n│  M konoma_e2e_vcs_literal_capture_67812_0/日 本 語 .txt   │  Enter           file diff                                     │                                                        │\n│                                                        │  d               diff of all changes (worktree)                │                                                        │\n│                                                        │  l / g           log / graph                                   │                                                        │\n│                                                        │  b               branches                                      │                                                        │\n│                                                        │  w               worktrees                                     │                                                        │\n│                                                        │  Ctrl-t          open the entry under the cursor in a new tab  │                                                        │\n│                                                        │  /               filter the rows by the text they show         │                                                        │\n│                                                        │  r               refresh listing & git status                  │                                                        │\n│                                                        │  Tab / ⇧Tab      cycle changes/log/graph/branches/worktrees    │                                                        │\n│                                                        │  !               external git tool (lazygit)                   │                                                        │\n│                                                        │  h / Esc  ·  q   close git view                                │                                                        │\n│                                                        │                                                                │                                                        │\n│                                                        │Movement                                                        │                                                        │\n│                                                        │  j / k           move                                          │                                                        │\n│                                                        │                                                                │                                                        │\n│                                                        │Launch                                                          │                                                        │\n│                                                        │  e               edit in external editor ($EDITOR / config)    │                                                        │\n│                                                        │  o               open in another app (OS default / Office)     │                                                        │\n│                                                        │                                                                │                                                        │\n│                                                        │Actions                                                         │                                                        │\n│                                                        │  Space s         stage the file                                │                                                        │\n│                                                        │  Space S         stage all                                     │                                                        │\n│                                                        │  Space u         unstage the file                              │                                                        │\n│                                                        │  Space U         unstage all                                   │                                                        │\n│                                                        │  Space x         discard                                       │                                                        │\n│                                                        │  Space c         commit                                        │                                                        │\n│                                                        │  Space a         diff                                          │                                                        │\n│                                                        │  Space d         diff-all                                      │                                                        │\n│                                                        │  Space l         log                                           │                                                        │\n│                                                        │  Space g         graph                                         │                                                        │\n└────────────────────────────────────────────────────────└────────────────────────────────────────────────────────────────┘────────────────────────────────────────────────────────┘\nj/k:scroll  g/G:top/bottom  ?/q/Esc:close";
const PENDING_FRAME_10: &str = "                                                                                                                                                           Project   HELP   path:rel\n p:Changes  Log  Graph  b:Branches  w:Worktrees\n● M konoma_e2e_vcs_literal_capture_67812_0/a-staged.txt\n  M konoma_e2e_vcs_literal_capture_67812_0/b-unstaged.txt\n  D konoma_e2e_vcs_literal_capture_67812_0/c-deleted.txt\n● R konoma_e2e_vcs_literal_capture_67812_0/d-renamed.txt\n  U konoma_e2e_vcs_literal_capture_67812_0/e-untracked.tx┌ Help  (?/Esc to close, j/k to scroll) ─────────────────────────┐\n● A konoma_e2e_vcs_literal_capture_67812_0/f-added.txt   │Git                                                             │\n  M konoma_e2e_vcs_literal_capture_67812_0/日 本 語 .txt    │  s / S           stage / stage all                             │\n                                                         │  u / U           unstage / unstage all                         │\n                                                         │  x               discard file (confirm)                        │\n                                                         │  c               commit (staged)                               │\n                                                         │  Enter           Open the file diff                            │\n                                                         │                                                                │\n                                                         │Project                                                         │\n                                                         │  Ctrl-t          open the entry under the cursor in a new tab  │\n                                                         │  l/→             diff                                          │\n                                                         │  Tab / Shift-Tab Cycle project screens                         │\n                                                         │  q / Esc         Pop this screen or return to the previous view│\n                                                         │  j/k / ↑/↓       scroll                                        │\n                                                         │  /               filter by name                                │\n                                                         │  p               Changes                                       │\n                                                         │  b               Branches                                      │\n                                                         │  w               Worktrees                                     │\n                                                         │                                                                │\n                                                         │Actions                                                         │\n                                                         │  Space a         diff                                          │\n                                                         │  Space s         stage(all)                                    │\n                                                         │  Space S         stage(all)                                    │\n                                                         │  Space u         unstage(all)                                  │\n                                                         │  Space U         unstage(all)                                  │\n                                                         │  Space x         discard                                       │\n                                                         │  Space c         commit                                        │\n                                                         └────────────────────────────────────────────────────────────────┘\n\n\n\n\n\nj/k:scroll  g/G:top/bottom  ?/q/Esc:close";
const PENDING_BEHAVIORS: &[PendingBehavior] = &[
    PendingBehavior { old: "e2e_space_menu_follows_the_footer_order_and_labels_in_git", assertion: "space:log:min-three", marker: "PENDING_DECISION:project-space-shared-operations", literal: "want.len() >= 3; detail, search, screens and back are declared operations", old_frame: CAP_SPACE_LOG_OLD, new_frame: CAP_SPACE_LOG_NEW },
    PendingBehavior { old: "changes_hub_help_shows_jj_section_and_hides_git_write_keys", assertion: "help:jj:changes-label", marker: "PENDING_DECISION:project-help-wording", literal: "jj changes (p)", old_frame: PENDING_FRAME_7, new_frame: PENDING_FRAME_8 },
    PendingBehavior { old: "changes_hub_help_shows_jj_section_and_hides_git_write_keys", assertion: "help:jj:close-view", marker: "PENDING_DECISION:project-help-wording", literal: "close jj view", old_frame: PENDING_FRAME_7, new_frame: PENDING_FRAME_8 },
    PendingBehavior { old: "changes_hub_help_shows_jj_section_and_hides_git_write_keys", assertion: "help:jj:diff-all", marker: "PENDING_DECISION:project-help-wording", literal: "diff of everything against @-", old_frame: PENDING_FRAME_7, new_frame: PENDING_FRAME_8 },
    PendingBehavior { old: "changes_hub_help_shows_jj_section_and_hides_git_write_keys", assertion: "help:jj:bookmarks", marker: "PENDING_DECISION:project-help-wording", literal: "bookmarks", old_frame: PENDING_FRAME_7, new_frame: PENDING_FRAME_8 },
    PendingBehavior { old: "changes_hub_help_shows_jj_section_and_hides_git_write_keys", assertion: "help:jj:external-tool", marker: "PENDING_DECISION:project-help-wording", literal: "external jj tool (lazyjj)", old_frame: PENDING_FRAME_7, new_frame: PENDING_FRAME_8 },
    PendingBehavior { old: "changes_hub_help_shows_jj_section_and_hides_git_write_keys", assertion: "help:git:worktree", marker: "PENDING_DECISION:project-help-wording", literal: "ghelp.contains(\"worktree\")", old_frame: PENDING_FRAME_9, new_frame: PENDING_FRAME_10 },
];
#[test]
fn pending_behavior_decisions_keep_old_literals_and_both_frames() {
    let expected = [
        ("e2e_space_menu_follows_the_footer_order_and_labels_in_git","space:log:min-three","PENDING_DECISION:project-space-shared-operations","want.len() >= 3; detail, search, screens and back are declared operations"),
        ("changes_hub_help_shows_jj_section_and_hides_git_write_keys","help:jj:changes-label","PENDING_DECISION:project-help-wording","jj changes (p)"),
        ("changes_hub_help_shows_jj_section_and_hides_git_write_keys","help:jj:close-view","PENDING_DECISION:project-help-wording","close jj view"),
        ("changes_hub_help_shows_jj_section_and_hides_git_write_keys","help:jj:diff-all","PENDING_DECISION:project-help-wording","diff of everything against @-"),
        ("changes_hub_help_shows_jj_section_and_hides_git_write_keys","help:jj:bookmarks","PENDING_DECISION:project-help-wording","bookmarks"),
        ("changes_hub_help_shows_jj_section_and_hides_git_write_keys","help:jj:external-tool","PENDING_DECISION:project-help-wording","external jj tool (lazyjj)"),
        ("changes_hub_help_shows_jj_section_and_hides_git_write_keys","help:git:worktree","PENDING_DECISION:project-help-wording","ghelp.contains(\"worktree\")"),
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
        let jj = p.assertion.contains("jj");
        let a = p.assertion;
        let pick = |jj_mode: &'static str, git_mode: &'static str| if jj { jj_mode } else { git_mode };
        match p.old {
            "e2e_space_menu_follows_the_footer_order_and_labels_in_git" => ("GIT LOG", "Project"),
            "changes_hub_help_shows_jj_section_and_hides_git_write_keys" => {
                (pick("JJ HELP", "GIT HELP"), "Project HELP")
            }
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
            ("changes_hub_help_shows_jj_section_and_hides_git_write_keys", "help:jj:changes-label") => {
                (&["jj @", "jj changes (p)"], &["b:Bookmarks", "p Changes"])
            }
            ("changes_hub_help_shows_jj_section_and_hides_git_write_keys", "help:jj:close-view") => {
                (&["jj @", "close jj view"], &["b:Bookmarks", "Pop this screen or return"])
            }
            ("changes_hub_help_shows_jj_section_and_hides_git_write_keys", "help:jj:diff-all") => (
                &["jj @", "diff of everything against @-"],
                &["b:Bookmarks", "Space a diff"],
            ),
            ("changes_hub_help_shows_jj_section_and_hides_git_write_keys", "help:jj:bookmarks") => {
                (&["jj @", "b bookmarks"], &["b:Bookmarks", "b Bookmarks"])
            }
            ("changes_hub_help_shows_jj_section_and_hides_git_write_keys", "help:jj:external-tool") => (
                &["jj @", "external jj tool (lazyjj)"],
                &["b:Bookmarks", "Space s sync"],
            ),
            ("changes_hub_help_shows_jj_section_and_hides_git_write_keys", _) => {
                (&["Git ⎇ master (7)", "w worktrees"], &["w:Worktrees", "w Worktrees"])
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
