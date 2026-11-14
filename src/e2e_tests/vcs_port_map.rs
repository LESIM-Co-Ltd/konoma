//! Partial U9b1 ledger. Name existence is not a proof of assertion strength or mutation coverage.
//! Unlisted tests and adapter-sensitive style guards must not be removed using this ledger.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

include!("vcs_port_inventory.rs");

const PICKER_STEPS: [&str; 11] = [
    "step-0-b",
    "step-1-x",
    "step-2-j",
    "step-3-x",
    "step-4-a",
    "step-5-c",
    "step-6-J",
    "step-7-K",
    "step-8-G",
    "step-9-k",
    "step-10-g",
];

const CHANGES_TITLE_TEST: &str =
    "e2e_vcs_changes_title_matches_hub_for_icons_languages_filters_and_branch_states";
const TITLE_WORDS_TEST: &str =
    "e2e_git_titles_changes_log_branches_and_jj_labels_come_from_the_plugin";
const PICKER_OPEN_TEST: &str =
    "e2e_vcs_graph_picker_opens_over_shared_branch_key_and_matches_cells";
const PICKER_STEPS_TEST: &str =
    "e2e_vcs_graph_picker_toggles_head_all_current_reorders_and_moves_like_legacy";
const PICKER_TITLE_TEST: &str = "vcs_port_picker_title_counts_equal_old_hub_at_every_step";
const PICKER_JJ_TEST: &str =
    "e2e_vcs_graph_picker_jj_git_branch_source_empty_and_colocated_and_hints";

/// The 71 title rows: every title the old hub drew is an ordinary assertion in the named new
/// test (the old hub's top edge compared cell for cell, plus the words checked in place).
fn title_rows() -> Vec<(&'static str, String, &'static str)> {
    let mut rows: Vec<(&'static str, String, &'static str)> = vec![
        (
            "e2e_slash_filters_the_changes_list_and_the_operations_follow_the_row",
            "title:filtered-count:one".into(),
            CHANGES_TITLE_TEST,
        ),
        (
            "e2e_slash_filter_moves_over_the_shown_rows_only",
            "title:filtered-count:zero".into(),
            CHANGES_TITLE_TEST,
        ),
        (
            "e2e_git_hub_to_log_shows_commit_subject",
            "title:git-log-label".into(),
            TITLE_WORDS_TEST,
        ),
        (
            "e2e_git_graph_shows_commit_nodes",
            "title:git-graph-label".into(),
            "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
        ),
    ];
    for lang in ["en", "ja"] {
        rows.push((
            "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
            format!("title:jj-graph-label:{lang}"),
            "e2e_vcs_graph_jj_top_edge_literal_in_both_languages",
        ));
    }
    for icons in ["icons-off", "icons-on"] {
        for step in ["initial", "branch-added"] {
            rows.push((
                PICKER_OPEN_TEST,
                format!("title:picker-count:{icons}:{step}"),
                PICKER_TITLE_TEST,
            ));
        }
    }
    for lang in ["en", "ja"] {
        for icons in ["icons-off", "icons-on"] {
            for step in PICKER_STEPS {
                rows.push((
                    PICKER_STEPS_TEST,
                    format!("title:picker-count:{lang}:{icons}:{step}"),
                    PICKER_TITLE_TEST,
                ));
            }
        }
    }
    for kind in ["jj", "colocated-jj"] {
        for lang in ["en", "ja"] {
            rows.push((
                PICKER_JJ_TEST,
                format!("title:picker-count:{kind}:{lang}"),
                PICKER_TITLE_TEST,
            ));
        }
    }
    for id in [
        "title:changes:glyph-on",
        "title:changes:branch-on",
        "title:changes:glyph-off",
        "title:changes:ascii-off",
        "title:changes:branch-off",
    ] {
        rows.push((
            "changes_and_log_titles_gate_branch_marker_by_icons",
            id.into(),
            CHANGES_TITLE_TEST,
        ));
    }
    for id in [
        "title:log:glyph-on",
        "title:log:glyph-off",
        "title:log:ascii-off",
    ] {
        rows.push((
            "changes_and_log_titles_gate_branch_marker_by_icons",
            id.into(),
            TITLE_WORDS_TEST,
        ));
    }
    rows.extend([
        (
            "git_view_renders_marker_and_filename",
            "title:changes-git-label".into(),
            CHANGES_TITLE_TEST,
        ),
        (
            "git_log_render_shows_commit_summary",
            "title:log-git-label".into(),
            TITLE_WORDS_TEST,
        ),
        (
            "git_branches_render_shows_current_marker",
            "title:branches-git-label".into(),
            "e2e_vcs_branches_top_edge_literal_title_count_and_tabs_at_two_widths",
        ),
        (
            "jj_bookmarks_render_shows_bookmarks_title_and_never_a_current_marker",
            "title:jj-bookmarks-label".into(),
            "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
        ),
        (
            "jj_bookmarks_render_shows_bookmarks_title_and_never_a_current_marker",
            "title:jj:not-git-branches".into(),
            "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
        ),
    ]);
    rows
}

// Each group expands to (old_test, assertion_id, new_test). The case dimensions are part
// of the stable id: a cell comparison always means every symbol, fg, bg and modifier.
type Group = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static str,
);
const GROUPS: &[Group] = &[
    (
        "vcs_color_mapping_preserves_every_legacy_color",
        &["palette:Red", "palette:Yellow", "palette:LightGreen"],
        &["real-status"],
        "vcs_port_real_status_conversion_and_drawn_colors",
    ),
    (
        "e2e_ui_commit_meta_align_right_vs_inline_moves_author_column",
        &["author:right-column-greater-than-inline"],
        &["git"],
        "vcs_port_tree_keys_refresh_and_meta_alignment_literals",
    ),
    (
        "e2e_git_graph_set_and_clear_base",
        &["graph:open", "base:set-selected", "base:clear"],
        &["git"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_git_graph_toggle_all_differs_between_git_and_jj",
        &[
            "git:a:no-flash",
            "git:a:full-cells-unchanged",
            "git:a:list-state-unchanged",
            "git:a:selection-unchanged",
            "git:a:no-footer",
            "git:a:no-help",
            "git:a:no-space",
            "git:a:host-navigation-state-unchanged",
            "git:a:space-actions-lack-graph-toggle",
            "jj:a:all-notice",
            "jj:a:default-notice",
            "jj:a:expanded",
            "jj:a:restored",
        ],
        &["en", "ja"],
        "vcs_port_git_a_is_inert_and_jj_a_keeps_literal_notices",
    ),
    (
        "e2e_git_graph_set_base_keeps_selection_then_clear",
        &["base:set-keeps-selected-id", "base:clear-keeps-selected-id"],
        &["git"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_the_tree_s_and_o_and_p_changes",
        &[
            "tree:s:no-leader",
            "tree:o:stays-tree",
            "p:inactive-outside-a-repository",
            "p:path-style-unchanged",
        ],
        &["git"],
        "vcs_port_tree_keys_refresh_and_meta_alignment_literals",
    ),
    (
        "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
        &[
            "graph:flash:literal-select-copy-base-detail",
            "graph:picker:flash:literal-head-toggle",
        ],
        &["git", "en", "ja"],
        "vcs_port_graph_flash_texts_are_literal_in_both_languages",
    ),
    (
        "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
        &["config:old-ui-names:graph-and-log"],
        &["git"],
        "vcs_port_old_ui_names_reach_the_new_graph_and_log_through_the_real_config_path",
    ),
    (
        "e2e_git_hub_to_log_shows_commit_subject",
        &["log:q-returns-to-origin"],
        &["git"],
        "vcs_port_q_returns_to_origin_and_l_g_are_list_keys_on_log_and_graph",
    ),
    (
        "e2e_git_graph_shows_commit_nodes",
        &["graph:q-returns-to-origin"],
        &["git"],
        "vcs_port_q_returns_to_origin_and_l_g_are_list_keys_on_log_and_graph",
    ),
    (
        "e2e_the_hub_g_and_l_are_the_list_keys_not_the_log_and_the_graph",
        &["l-g:list-keys-on-log-and-graph"],
        &["git"],
        "vcs_port_q_returns_to_origin_and_l_g_are_list_keys_on_log_and_graph",
    ),
    (
        "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
        &["graph:filter-rows-per-query-and-short-height-scroll"],
        &["git"],
        "vcs_port_graph_filter_rows_per_query_and_short_height_scroll",
    ),
    (
        "e2e_slash_filters_the_changes_list_and_the_operations_follow_the_row",
        &["filter:input-line-opens-and-closes-on-every-screen"],
        &["git"],
        "vcs_port_filter_input_line_opens_and_closes_and_dirty_bottom_row_base_pin",
    ),
    (
        "e2e_graph_x_clears_the_base_and_zero_does_not",
        &["base:dirty-bottom-row-pin"],
        &["git"],
        "vcs_port_filter_input_line_opens_and_closes_and_dirty_bottom_row_base_pin",
    ),
    (
        "e2e_space_menu_follows_the_footer_order_and_labels_in_git",
        &[
            "space:declared-prefix",
            "space:open-prefix",
            "space:footer-label-or-menu-label",
            "space:shorter-than-help",
            "space:max-label-26",
        ],
        &["worktrees"],
        "vcs_port_copy_contexts_and_space_footer_order",
    ),
    (
        "e2e_vcs_project_footer_log_changes_and_worktrees_have_one_entry_per_key",
        &["footer:one-enter", "footer:no-duplicate-chords"],
        &["worktrees"],
        "vcs_port_log_and_changes_help_filter_marks_and_footer_priority",
    ),
    (
        "e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order",
        &["unborn:open-then-tab-empty-log"],
        &["en", "ja"],
        "vcs_port_unborn_after_open_then_tab_shows_the_empty_log",
    ),
    (
        "e2e_vcs_branches_copy_menu_clipboard_footer_and_help_match_legacy_acts",
        &["footer:enter-token-per-row"],
        &["git"],
        "vcs_port_branch_footer_enter_token_per_row",
    ),
    (
        "e2e_r_reads_the_lists_again",
        &["log:r:first-id-delta", "branches:r:external-ref-present"],
        &["git"],
        "vcs_port_tree_keys_refresh_and_meta_alignment_literals",
    ),
    (
        "vcs_color_mapping_preserves_every_legacy_color",
        &[
            "palette:every-legacy-color",
            "palette:bold",
            "palette:glyph",
        ],
        &["real-git-fourteen-lanes"],
        "vcs_port_real_graph_palette_checks_each_conversion_arm",
    ),
    (
        "commit_header_shows_full_multiline_message",
        &[
            "header:short",
            "header:author",
            "header:date",
            "header:subject-bold",
            "header:blank-line",
            "header:body-one",
            "header:body-two",
            "header:separator-darkgray",
        ],
        &["git"],
        "vcs_port_full_commit_header_literal_lines_and_styles",
    ),
    (
        "right_meta_aligns_pads_and_degrades",
        &[
            "right:left-prefix",
            "right:date-suffix",
            "right:padding-exact-width",
        ],
        &["width20"],
        "vcs_port_right_metadata_literal_padding_ellipsis_and_narrow_fallback",
    ),
    (
        "right_meta_aligns_pads_and_degrades",
        &["right:ellipsis", "right:date-suffix", "right:width-bound"],
        &["width12"],
        "vcs_port_right_metadata_literal_padding_ellipsis_and_narrow_fallback",
    ),
    (
        "right_meta_aligns_pads_and_degrades",
        &["right:metadata-omitted", "right:width-bound"],
        &["width5"],
        "vcs_port_right_metadata_literal_padding_ellipsis_and_narrow_fallback",
    ),
    (
        "graph_picker_renders_panel_title_and_branches",
        &["picker:title", "picker:trunk", "picker:feature-x"],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "graph_title_gates_base_marker_without_duplicating_base_word",
        &["base:word-count-one", "base:glyph-count"],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "graph_title_gates_base_marker_without_duplicating_base_word",
        &[
            "base:whole-frame-word-count-one",
            "base:whole-frame-glyph-count",
        ],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "graph_legend_head_and_base_markers_are_spaced_and_gated_by_icons",
        &[
            "legend:head-marker-spaced",
            "legend:base-marker-spaced",
            "legend:ascii-prefixes",
            "legend:no-glued-markers",
        ],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "graph_picker_marker_column_gated_by_icons_and_alignment_preserved",
        &[
            "picker:marker-kind",
            "picker:trunk-column",
            "picker:feature-column",
        ],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "changes_hub_help_shows_jj_section_and_hides_git_write_keys",
        &[
            "help:jj:sync-literal",
            "help:jj:no-stage",
            "help:jj:no-unstage",
            "help:jj:no-commit",
            "help:jj:no-worktree",
        ],
        &["en", "ja"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "changes_hub_help_shows_jj_section_and_hides_git_write_keys",
        &["help:git:stage", "help:git:unstage", "help:git:commit"],
        &["en"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "changes_hub_help_shows_jj_section_and_hides_git_write_keys",
        &["help:git:no-jj-heading"],
        &["en", "ja"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "changes_hub_help_shows_jj_section_and_hides_git_write_keys",
        &[
            "help:jj:changes-label",
            "help:jj:close-view",
            "help:jj:diff-all",
            "help:jj:bookmarks",
            "help:jj:external-tool",
            "help:git:worktree",
        ],
        &["en", "ja"],
        "vcs_port_jj_help_has_no_git_write_words_and_git_help_has_them",
    ),
    (
        "graph_help_shows_jj_section_and_all_revisions_row",
        &[
            "help:graph:exact-section-label",
            "help:graph:all-revisions",
            "help:graph:no-base-in-jj",
            "help:graph:base-in-git",
        ],
        &["en", "ja"],
        "vcs_port_graph_picker_and_backend_help_exact_rows",
    ),
    (
        "e2e_vcs_graph_picker_footer_and_help_match_legacy_in_both_languages",
        &[
            "picker:footer-exact-order-labels",
            "picker:help-exact-key-labels",
            "picker:help-exact-descriptions",
            "picker:no-extra-rows",
        ],
        &["en:git,jj,colocated", "ja:git,jj,colocated"],
        "vcs_port_graph_picker_and_backend_help_exact_rows",
    ),
    (
        "e2e_vcs_graph_help_and_footer_follow_legacy_labels_and_priorities_in_both_languages",
        &[
            "graph:footer-exact-order-labels",
            "graph:help-exact-descriptions",
            "graph:help-section-title",
            "graph:no-extra-rows",
        ],
        &["en:git,jj,colocated", "ja:git,jj,colocated"],
        "vcs_port_graph_picker_and_backend_help_exact_rows",
    ),
    (
        "e2e_vcs_changes_git_enter_opens_same_file_and_head_baseline_even_when_staged",
        &[
            "file:staged-fixture",
            "file:preview-path",
            "file:head-baseline-all-lines",
            "file:worktree-not-index",
        ],
        &["Enter", "l", "Right", "remapped-Z"],
        "vcs_port_file_head_baseline_right_remaps_and_diff_layout",
    ),
    (
        "e2e_vcs_changes_jj_and_colocated_body_cell_parity_and_enter_parent_baseline",
        &[
            "changes:full-cells-state",
            "changes:selectable-count",
            "enter:path",
            "enter:parent-baseline-all-lines",
        ],
        &["bare:changed,empty", "colocated:changed,empty"],
        "vcs_port_jj_changes_fixture_matrix_cells_and_parent_baseline",
    ),
    (
        "e2e_vcs_project_list_right_opens_changes_like_enter_and_respects_remaps",
        &[
            "open:diff-preview",
            "open:path",
            "remap:l-noop",
            "open:all-lines",
        ],
        &["Enter", "l", "Right", "remapped-Z"],
        "vcs_port_file_head_baseline_right_remaps_and_diff_layout",
    ),
    (
        "e2e_vcs_detail_layout_remap_uses_diff_context_for_detail_changes_and_file",
        &[
            "layout:s-noop",
            "layout:S-split",
            "layout:diff-context",
            "layout:footer-S",
            "layout:help-S",
        ],
        &["git.detail", "text"],
        "vcs_port_file_head_baseline_right_remaps_and_diff_layout",
    ),
    (
        "e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order",
        &[
            "unborn:no-selection",
            "unborn:one-disabled-row",
            "unborn:literal-no-commits",
            "unborn:dim-cell",
            "unborn:Enter-no-push",
            "tabs:fixed-root-order",
        ],
        &["en", "ja"],
        "vcs_port_unborn_both_languages_fixed_tabs_and_empty_row",
    ),
    (
        "e2e_vcs_worktrees_footer_preserves_legacy_action_priority_and_labels",
        &["footer:exact-shared-action-order-and-labels"],
        &["en", "ja"],
        "vcs_port_worktree_footer_shared_priority_literal",
    ),
    (
        "e2e_y_opens_a_different_menu_in_each_context",
        &[
            "copy:leader-kind",
            "copy:escape-clears",
            "copy:pairwise-distinct-key-sets",
        ],
        &["Tree", "Table", "Branches", "Log"],
        "vcs_port_copy_contexts_and_space_footer_order",
    ),
    (
        "e2e_space_menu_follows_the_footer_order_and_labels_in_git",
        &[
            "space:declared-prefix",
            "space:open-prefix",
            "space:footer-label-or-menu-label",
            "space:shorter-than-help",
            "space:max-label-26",
        ],
        &["changes", "log", "graph", "branches", "file-diff"],
        "vcs_port_copy_contexts_and_space_footer_order",
    ),
    (
        "e2e_space_menu_follows_the_footer_order_and_labels_in_git",
        &["space:log:min-three"],
        &["git", "jj"],
        "vcs_port_project_space_shared_operations",
    ),
    (
        "e2e_diff_navigation_and_back_work_without_a_repository",
        &[
            "diff:host-opens-without-repo",
            "diff:context",
            "motion:j",
            "motion:k",
            "motion:page-down",
            "motion:page-up",
            "motion:half-down",
            "motion:half-up",
            "motion:horizontal",
            "motion:End",
            "motion:Home",
            "path:menu",
            "back:tree",
        ],
        &["vim:q,Esc", "less:q,Esc"],
        "vcs_host_nonrepository_diff_navigation_options_and_origin",
    ),
    (
        "e2e_key_context_of_the_version_control_screens_follows_the_backend",
        &["context:file-diff"],
        &["git", "jj"],
        "vcs_port_backend_contexts_and_bindings_real_repositories",
    ),
    (
        "e2e_key_context_of_the_version_control_screens_follows_the_backend",
        &[
            "context:root-canvas:changes",
            "context:root-canvas:log",
            "context:root-canvas:graph",
            "context:root-canvas:branches",
            "context:root-canvas:worktrees",
            "context:plugin-backend",
            "context:detail",
        ],
        &["fixture"],
        "vcs_port_backend_contexts_and_bindings_real_repositories",
    ),
    (
        "e2e_a_backend_binding_fires_only_for_that_backend",
        &[
            "binding:jj.changes/jj",
            "binding:git.changes/git",
            "binding:git_changes/git",
            "binding:git_changes/jj",
        ],
        &["fixture"],
        "vcs_port_backend_contexts_and_bindings_real_repositories",
    ),
    (
        "e2e_a_backend_binding_fires_only_for_that_backend",
        &["binding:wrong-backend-stays-canvas"],
        &["jj.changes/git", "git.changes/jj"],
        "vcs_port_backend_contexts_and_bindings_real_repositories",
    ),
    (
        "jj_changes_hub_footer_uses_jj_words_never_git_write_words",
        &[
            "footer:no-stage",
            "footer:no-unstage",
            "footer:no-commit",
            "footer:no-worktree",
            "footer:git-stage-unstage-commit",
            "footer:git-no-sync-bookmarks",
        ],
        &["en", "ja"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "jj_graph_footer_shows_jj_hint_not_git_base_pin_hint",
        &["graph:exact-backend-rows-and-priority"],
        &["en", "ja"],
        "vcs_port_graph_picker_and_backend_help_exact_rows",
    ),
    (
        "jj_bookmarks_footer_shows_search_only_hint_not_git_branch_ops",
        &[
            "bookmarks:main-present",
            "bookmarks:no-current-marker",
            "footer:no-checkout",
            "footer:no-new",
            "footer:no-delete",
        ],
        &["en", "ja"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "jj_changes_hub_footer_uses_jj_words_never_git_write_words",
        &["footer:literal"],
        &["jj:en,ja", "git:en"],
        "vcs_port_every_screen_footer_equals_old_hub",
    ),
    (
        "jj_changes_hub_footer_uses_jj_words_never_git_write_words",
        &["footer:worktree-word"],
        &["git"],
        "vcs_port_every_screen_footer_equals_old_hub",
    ),
    (
        "jj_graph_footer_shows_jj_hint_not_git_base_pin_hint",
        &["footer:literal"],
        &["jj", "git"],
        "vcs_port_graph_footer_equals_old_hub",
    ),
    (
        "jj_bookmarks_footer_shows_search_only_hint_not_git_branch_ops",
        &["footer:literal"],
        &["jj", "git"],
        "vcs_port_every_screen_footer_equals_old_hub",
    ),
    (
        "jj_chip_shows_in_git_view_for_jj_not_for_git",
        &["project:selected-backend-id"],
        &["git:en", "jj:en,ja"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "bookmark_chip_shows_for_jj_list_branch_chip_for_git_list",
        &["bookmarks:main-present", "bookmarks:no-current-marker"],
        &["en", "ja"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "jj_chip_shows_in_git_view_for_jj_not_for_git",
        &["chip:display"],
        &["git:en", "jj:en,ja"],
        "vcs_port_mode_chips_follow_the_backend_and_the_screen",
    ),
    (
        "bookmark_chip_shows_for_jj_list_branch_chip_for_git_list",
        &["chip:internal"],
        &["git:en", "jj:en,ja"],
        "vcs_port_mode_chips_follow_the_backend_and_the_screen",
    ),
    (
        "e2e_slash_filters_the_changes_list_and_the_operations_follow_the_row",
        &["filter:input-surface"],
        &["git"],
        "vcs_port_filter_input_line_opens_and_closes_and_dirty_bottom_row_base_pin",
    ),
    (
        "e2e_slash_filters_the_log_by_the_text_shown",
        &["filter:input-surface"],
        &["git"],
        "vcs_port_filter_input_line_opens_and_closes_and_dirty_bottom_row_base_pin",
    ),
    (
        "copy_menu_relabels_short_hash_to_change_id_for_jj_not_git",
        &[
            "copy:first-label-exact",
            "copy:second-label-exact",
            "copy:footer-exact-first-label",
            "copy:footer:no-other-backend-label",
        ],
        &["git:en,ja", "jj:en,ja"],
        "vcs_port_backend_display_words_copy_labels_and_project_origin",
    ),
    (
        "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
        &[
            "range:all-literal-notice",
            "range:default-literal-notice",
            "range:more-rows",
            "range:restores-rows",
            "base:s-noop",
            "base:x-noop",
        ],
        &["en", "ja"],
        "vcs_port_git_a_is_inert_and_jj_a_keeps_literal_notices",
    ),
    (
        "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
        &[
            "cells:initial",
            "cells:after-all",
            "cells:after-default",
            "hints:footer-help-match",
        ],
        &["en", "ja"],
        "jj_graph_and_bookmarks_offer_only_real_actions",
    ),
    (
        "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
        &[
            "range:per-tab-new-tab-default",
            "range:per-tab-tab0-keeps-expanded",
        ],
        &["en", "ja"],
        "vcs_host_jj_all_range_is_per_tab",
    ),
    (
        "e2e_vcs_graph_picker_jj_git_branch_source_empty_and_colocated_and_hints",
        &[
            "picker:full-cells-state",
            "picker:git-ref-source",
            "picker:empty",
            "picker:Enter-pop",
            "picker:no-filter",
        ],
        &["bare", "colocated"],
        "vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source",
    ),
    (
        "e2e_git_hub_to_log_shows_commit_subject",
        &["log:commit-subject-init"],
        &["git"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_graph_shows_commit_nodes",
        &["graph:commit-nodes-and-subjects"],
        &["git"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "git_view_renders_marker_and_filename",
        &["changes:untracked-marker-U", "changes:filename-foo"],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "git_log_render_shows_commit_summary",
        &["log:hello-world-summary"],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "git_branches_render_shows_current_marker",
        &["branches:feature-x", "branches:current-marker"],
        &["icons-on", "icons-off"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "jj_bookmarks_render_shows_bookmarks_title_and_never_a_current_marker",
        &["bookmarks:main", "bookmarks:no-star-marker"],
        &["jj"],
        "vcs_port_renderer_normal_rows_markers_and_base_selection",
    ),
    (
        "e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order",
        &["unborn:full-cells-state"],
        &["en", "ja"],
        "vcs_port_unborn_log_cells",
    ),
    (
        "e2e_git_commit_detail_shows_full_message",
        &[
            "detail:screen",
            "message:body-one",
            "message:body-two",
            "q:log",
        ],
        &["init"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_commit_copy_values",
        &[
            "screen:log",
            "subject:init",
            "metadata:subject",
            "metadata:author-t",
            "metadata:short-seven",
            "metadata:id-forty",
            "metadata:short-prefix",
            "metadata:date-nonempty",
            "leader:generic-copy",
            "leader:consumed",
        ],
        &["init"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_copy_leader_all_subkeys_consume_from_log",
        &[
            "screen:log",
            "subject:init",
            "metadata:subject",
            "metadata:author-t",
            "metadata:short-seven",
            "metadata:id-forty",
            "metadata:short-prefix",
            "metadata:date-nonempty",
        ],
        &["init"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_copy_leader_all_subkeys_consume_from_log",
        &["leader:generic-copy", "leader:consumed"],
        &["s,h,t,m,a,d"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_copy_leader_opens_in_graph_and_detail",
        &["screen:active", "leader:generic-copy", "escape:no-leader"],
        &["graph,detail"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_log_navigate_open_detail_and_back",
        &[
            "screen:log",
            "subject:gamma",
            "subject:beta",
            "subject:alpha",
            "j:distinct-id",
        ],
        &["history"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_log_navigate_open_detail_and_back",
        &[
            "detail:screen",
            "detail:selected-subject",
            "q:log",
            "q:preserved-id",
        ],
        &["enter,l"],
        "vcs_port_original_log_detail_history_and_six_copy_keys",
    ),
    (
        "e2e_git_graph_branch_picker_toggles",
        &["picker:pushed", "row:feature-x", "q:not-pushed", "q:graph"],
        &["history"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_git_graph_navigates_commit_rows",
        &[
            "screen:graph",
            "node:glyph",
            "ends:distinct-ids",
            "j:nonempty-and-not-top",
        ],
        &["history"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_git_graph_picker_current_only_then_all",
        &[
            "picker:pushed",
            "feature:on-initial",
            "feature:off-current",
            "feature:on-all",
            "q:not-pushed",
            "q:graph",
        ],
        &["history"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_graph_x_clears_the_base_and_zero_does_not",
        &["base:present", "zero:base-present", "x:base-absent"],
        &["history"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_graph_panel_c_is_current_only_and_l_applies",
        &[
            "picker:pushed",
            "feature:on-initial",
            "n:inert",
            "c:feature-off",
            "l:not-pushed",
            "l:graph",
        ],
        &["history"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_ui_graph_max_branches_caps_legend_entries",
        &["legend:hidden"],
        &["cap-2"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_ui_graph_max_branches_caps_legend_entries",
        &["legend:not-hidden"],
        &["cap-0"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_ui_graph_base_branches_config_auto_selects_base",
        &["base:absent-default", "base:present-configured"],
        &["history"],
        "vcs_port_original_graph_selection_base_picker_and_config",
    ),
    (
        "e2e_slash_filters_the_changes_list_and_the_operations_follow_the_row",
        &[
            "row:a-visible",
            "row:new-visible",
            "filter:editing",
            "query:drawn",
            "filter:a-hidden",
            "target:new",
            "enter:applied",
            "enter:query-preserved",
            "stage:new",
            "stage:a-unchanged",
            "escape:a-visible",
            "escape:changes",
            "escape:origin",
        ],
        &["new"],
        "vcs_port_original_changes_filters_row_actions_and_refresh",
    ),
    (
        "e2e_slash_filter_moves_over_the_shown_rows_only",
        &[
            "target:a-initial",
            "j:target-c",
            "j:clamped-c",
            "G:target-c",
            "g:target-a",
            "escape:new-visible",
        ],
        &["rs"],
        "vcs_port_original_changes_filters_row_actions_and_refresh",
    ),
    (
        "e2e_the_hub_g_and_l_are_the_list_keys_not_the_log_and_the_graph",
        &["ends:distinct-targets", "g:changes", "g:not-log-or-graph"],
        &["changed"],
        "vcs_port_original_changes_filters_row_actions_and_refresh",
    ),
    (
        "e2e_slash_filters_the_log_by_the_text_shown",
        &[
            "screen:log",
            "subject:alpha",
            "subject:beta",
            "filter:editing",
            "query:drawn",
            "filter:alpha-hidden",
            "filter:gamma-hidden",
            "selected:beta-id",
            "detail:screen",
            "detail:beta",
            "q:beta-id",
            "escape:alpha",
            "escape:log",
        ],
        &["beta"],
        "vcs_port_original_log_and_graph_filter_identity",
    ),
    (
        "e2e_slash_filters_the_graph_and_hides_the_connectors",
        &[
            "screen:graph",
            "filter:alpha-hidden",
            "filter:beta-hidden",
            "filter:gamma-visible",
            "selected:gamma-subject",
            "escape:alpha",
            "escape:graph",
        ],
        &["gamma"],
        "vcs_port_original_log_and_graph_filter_identity",
    ),
    (
        "e2e_tab_walks_the_git_screens_both_ways",
        &["screen:cycle", "backend:git"],
        &[
            "forward:1-log,2-graph,3-branches,4-worktrees,5-changes,6-log",
            "reverse:1-changes,2-worktrees,3-branches,4-graph,5-log,6-changes",
        ],
        "vcs_port_original_git_and_jj_cycles_and_contexts",
    ),
    (
        "e2e_tab_walks_the_git_screens_both_ways",
        &["help:tab-key-label", "help:tab-description"],
        &["backend:git"],
        "vcs_port_original_git_and_jj_cycles_and_contexts",
    ),
    (
        "e2e_tab_skips_the_worktrees_in_a_jj_repository",
        &["help:tab-description"],
        &["backend:jj"],
        "vcs_port_original_git_and_jj_cycles_and_contexts",
    ),
    (
        "e2e_vcs_graph_help_and_footer_follow_legacy_labels_and_priorities_in_both_languages",
        &["help:graph:key-order"],
        &["gamma"],
        "vcs_port_graph_help_keys_and_order_equal_legacy",
    ),
    (
        "e2e_tab_skips_the_worktrees_in_a_jj_repository",
        &["screen:cycle", "backend:jj"],
        &["forward:log,graph,branches,changes"],
        "vcs_port_original_git_and_jj_cycles_and_contexts",
    ),
    (
        "e2e_tab_walks_the_git_screens_both_ways",
        &["screen:initial", "jump:b-branches", "jump:w-worktrees"],
        &["backend:git"],
        "vcs_port_original_git_and_jj_cycles_and_contexts",
    ),
    (
        "e2e_tab_walks_the_git_screens_both_ways",
        &["footer:tab-screens-token"],
        &["backend:git"],
        "vcs_port_every_screen_footer_equals_old_hub",
    ),
    (
        "e2e_tab_skips_the_worktrees_in_a_jj_repository",
        &["screen:initial", "cycle:no-worktrees"],
        &["backend:jj"],
        "vcs_port_original_git_and_jj_cycles_and_contexts",
    ),
    (
        "vcs_file_status_colors_preserve_legacy_colors",
        &[
            "plugin:status-color",
            "drawn:foreground",
            "drawn:background",
            "drawn:modifier",
        ],
        &["modified,added,untracked,deleted,renamed,typechange,conflicted"],
        "vcs_port_real_status_conversion_and_drawn_colors",
    ),
    (
        "graph_legend_preserves_each_branch_node_color",
        &["legend:ordered-names-head-and-base"],
        &["git:worktree"],
        "vcs_port_real_graph_conversion_legend_and_legacy_cells",
    ),
    (
        "graph_legend_preserves_each_branch_node_color",
        &["legend:node-color"],
        &["feature-cyan,base-white,topic-green"],
        "vcs_port_real_graph_conversion_legend_and_legacy_cells",
    ),
    (
        "graph_cells_preserve_legacy_text_colors_and_modifiers",
        &["graph:each-glyph-fg-bg-modifier"],
        &["git:clean,worktree", "jj:clean,worktree"],
        "vcs_port_graph_legacy_glyphs_and_padding_styles_reach_the_bridge",
    ),
    (
        "graph_cells_preserve_legacy_text_colors_and_modifiers",
        &["plugin:independent-conversion", "drawn:independent-cells"],
        &["git:clean,worktree"],
        "vcs_port_real_graph_conversion_legend_and_legacy_cells",
    ),
    (
        "e2e_vcs_graph_jj_repository_tests_guard_optional_binary",
        &[
            "jj:all-new-cases-guarded-before-setup",
            "jj:changes-matrix-guarded-before-setup",
            "parser:whitespace-comments-literals",
            "parser:late-guard-rejected",
            "parser:positive-guard-rejected",
            "parser:guarded-helper",
            "parser:late-helper-rejected",
        ],
        &["structure"],
        "vcs_port_all_real_jj_cases_are_guarded_before_setup",
    ),
    (
        "e2e_vcs_graph_changelog_records_tab_cache_and_shared_list_differences",
        &["notes:decided-difference"],
        &[
            "second-tab",
            "non-selectable-rows",
            "uses-the-keymap",
            "active-or-inactive-close",
        ],
        "vcs_port_changelog_keeps_each_decided_graph_difference",
    ),
    (
        "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
        &["base:worktree-refusal-flash", "copy:worktree-has-no-menu"],
        &["en:worktree", "ja:worktree"],
        "vcs_port_graph_every_copy_base_reload_and_both_detail_open_keys",
    ),
    (
        "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
        &[
            "cells:base-actions",
            "copy:six-values-and-flashes",
            "fixture:multiline-message",
            "side:selected-before-and-after-base",
            "base:clear-flash",
            "base:unset-clear-silent",
            "r:preserves-identity",
        ],
        &["en:commit", "ja:commit"],
        "vcs_port_graph_every_copy_base_reload_and_both_detail_open_keys",
    ),
    (
        "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
        &[
            "detail:pushed-and-cells",
            "q:restores-graph-cursor-and-cells",
        ],
        &[
            "en:worktree:enter,l",
            "ja:worktree:enter,l",
            "en:commit:enter,l",
            "ja:commit:enter,l",
        ],
        "vcs_port_graph_every_copy_base_reload_and_both_detail_open_keys",
    ),
    (
        "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
        &["fixture:jj-subject", "fixture:pinned-timestamp"],
        &["jj", "colocated-jj"],
        "vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source",
    ),
    (
        "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
        &["detail:change-header", "cells:detail"],
        &["jj:right:en", "colocated-jj:right:en"],
        "vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source",
    ),
    (
        "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
        &["log:exact-backend-ids", "cells:each-row"],
        &[
            "jj:right:en,ja",
            "jj:inline:en,ja",
            "jj:unknown:en,ja",
            "colocated-git:right:en,ja",
            "colocated-git:inline:en,ja",
            "colocated-git:unknown:en,ja",
            "colocated-jj:right:en,ja",
            "colocated-jj:inline:en,ja",
            "colocated-jj:unknown:en,ja",
        ],
        "vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source",
    ),
    (
        "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
        &["watch:changed-rows", "watch:latest-commit-visible"],
        &["jj", "colocated-jj"],
        "vcs_port_jj_watch_refreshes_log_for_bare_and_colocated_repositories",
    ),
    (
        "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
        &[
            "cells:bookmarks",
            "rows:all-branch-kind",
            "footer:no-writes",
            "help:no-write-section",
            "menu:one-copy-item",
            "clipboard:alpha",
            "writes:enter-c-d-no-dialog",
            "refs:two-bookmarks",
            "cycle:all-forward-and-reverse-screens",
        ],
        &["en", "ja"],
        "vcs_port_jj_bookmark_cells_copy_and_full_tab_cycle",
    ),
    (
        "e2e_vcs_log_exit_returns_to_origin_and_h_goes_back_like_q",
        &["h:goes-back-like-q", "h:no-leader", "q-or-escape:origin"],
        &["q", "escape", "h"],
        "vcs_host_log_q_escape_and_h_return_to_the_origin",
    ),
    (
        "e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub",
        &[
            "fixture:seven-files",
            "discovery:repo-root",
            "cells:entire-body",
        ],
        &[
            "repo:relative",
            "repo:home",
            "repo:full",
            "nested:relative",
            "nested:home",
            "nested:full",
        ],
        "vcs_port_nested_changes_paths_compare_every_body_cell",
    ),
    (
        "e2e_vcs_changes_linked_worktree_external_index_event_refreshes_the_session",
        &[
            "watch:injected",
            "index:staged-marker",
            "enter:selected-path",
            "enter:diff",
        ],
        &["linked-gitdir"],
        "vcs_port_watch_head_refs_linked_index_and_refresh_filtered_identity",
    ),
    (
        "e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty",
        &["cells:all-selections-down-and-up"],
        &[
            "changed:en:140x20",
            "changed:ja:140x20",
            "changed:en:34x8",
            "changed:ja:34x8",
            "empty:en:140x20",
            "empty:ja:140x20",
            "empty:en:34x8",
            "empty:ja:34x8",
        ],
        "vcs_port_changes_every_selection_empty_and_nested_paths",
    ),
    (
        "e2e_vcs_copy_log_and_detail_clipboard_and_flash_match_old_hub_for_all_six_items",
        &[
            "copy:multiline-cjk-message",
            "menu:pending-copy",
            "clipboard:literal-value",
            "flash:copied",
        ],
        &[
            "en:log:s,h,t,m,a,d",
            "ja:log:s,h,t,m,a,d",
            "en:detail:s,h,t,m,a,d",
            "ja:detail:s,h,t,m,a,d",
        ],
        "copy_values_failures_and_cached_detail_match_host",
    ),
    (
        "e2e_vcs_copy_menu_cells_help_footer_and_space_membership_match_the_declaration",
        &[
            "menu:keys-labels-order",
            "menu:title",
            "help:section-rows",
            "footer:y-copy",
            "space:no-copy-acts",
            "footer:no-copy-acts",
            "menu:every-footer-cell",
        ],
        &["en:log", "ja:log", "en:detail", "ja:detail"],
        "vcs_port_copy_menu_literal_labels_help_footer_and_space_membership",
    ),
    (
        "e2e_vcs_copy_empty_log_and_screen_without_copy_acts_do_nothing_without_hints",
        &[
            "screen:fixed-log",
            "menu:not-offered",
            "footer:no-y",
            "help:no-copy",
            "y:no-leader",
            "y:no-flash",
            "y:no-write",
        ],
        &["log", "changes", "worktrees"],
        "vcs_port_copy_empty_and_unbound_prefix_have_no_hints_or_effects",
    ),
    (
        "e2e_vcs_copy_hints_disappear_when_the_menu_prefix_is_unbound",
        &[
            "menu:declaration-still-exists",
            "y:noop",
            "footer:no-y",
            "help:no-copy-section",
        ],
        &["canvas-y-noop"],
        "vcs_port_copy_empty_and_unbound_prefix_have_no_hints_or_effects",
    ),
    (
        "e2e_vcs_copy_clipboard_failure_matches_old_hub_without_writing",
        &[
            "clipboard:no-write",
            "flash:exact-copy-failure",
            "flash:error-text",
        ],
        &["log:en"],
        "copy_values_failures_and_cached_detail_match_host",
    ),
    (
        "e2e_vcs_copy_empty_subject_is_written_on_log_and_detail",
        &["clipboard:empty-string-is-written", "flash:copied"],
        &["log", "detail"],
        "copy_values_failures_and_cached_detail_match_host",
    ),
    (
        "e2e_vcs_copy_detail_uses_the_metadata_it_shows_and_log_missing_commit_flashes",
        &[
            "clipboard:displayed-metadata-or-none",
            "flash:copied-or-failed",
        ],
        &["log:object-removed", "detail:object-removed"],
        "copy_values_failures_and_cached_detail_match_host",
    ),
    (
        "e2e_vcs_copy_reopened_cached_details_keep_each_displayed_commit",
        &[
            "view:A-reuses-cached-arc",
            "clipboard:shown-id",
            "flash:copied",
        ],
        &["A:first", "B", "A:reopened"],
        "copy_values_failures_and_cached_detail_match_host",
    ),
    (
        "e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy",
        &[
            "screen:detail",
            "diff:empty-or-142-lines",
            "cells:open-and-every-motion",
            "layout:flash",
            "layout:resets-hscroll",
            "pushed:tab-and-backtab-noop",
            "q:log-cursor",
            "l:reopen-detail",
            "escape:log-cursor",
        ],
        &[
            "80:unified:empty:vim",
            "80:unified:empty:less",
            "80:unified:142:vim",
            "80:unified:142:less",
            "80:auto:empty:vim",
            "80:auto:empty:less",
            "80:auto:142:vim",
            "80:auto:142:less",
            "180:unified:empty:vim",
            "180:unified:empty:less",
            "180:unified:142:vim",
            "180:unified:142:less",
            "180:auto:empty:vim",
            "180:auto:empty:less",
            "180:auto:142:vim",
            "180:auto:142:less",
        ],
        "vcs_port_detail_full_matrix_and_cursor_return",
    ),
    (
        "e2e_vcs_detail_pushed_screen_is_absent_from_tabs_cycle_and_empty_row_open",
        &[
            "tabs:fixed-cycle-both-directions",
            "tabs:no-detail",
            "empty:no-selected-row",
            "empty:no-open-hint",
            "empty:enter-l-right-noop",
        ],
        &["unborn"],
        "vcs_host_unborn_log_fixed_tabs_and_q_origin",
    ),
    (
        "e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads",
        &[
            "r:replaces-cached-arc",
            "diff:empty-header-and-lines",
            "cells:reload-and-every-motion",
        ],
        &["80:unified", "80:auto", "180:unified", "180:auto"],
        "vcs_port_missing_detail_refresh_has_no_header_or_lines",
    ),
    (
        "e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling",
        &[
            "fixture:13-commits",
            "cursor:each-motion",
            "cells:open-and-each-motion",
        ],
        &[
            "en:right:mw+3,mw+4,12,58,220",
            "ja:right:mw+3,mw+4,12,58,220",
            "en:inline:mw+3,mw+4,12,58,220",
            "ja:inline:mw+3,mw+4,12,58,220",
            "en:unknown:mw+3,mw+4,12,58,220",
            "ja:unknown:mw+3,mw+4,12,58,220",
        ],
        "vcs_port_log_boundary_widths_motions_and_every_query",
    ),
    (
        "e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query",
        &[
            "filter:exact-visible-ids",
            "filter:cursor",
            "cells:filter-and-j",
        ],
        &[
            "right:long-cjk,author-cjk,author-ascii,date,short,subject-author,dot,upper,lower",
            "inline:long-cjk,author-cjk,author-ascii,date,short,subject-author,dot,upper,lower",
        ],
        "vcs_port_log_boundary_widths_motions_and_every_query",
    ),
    (
        "e2e_vcs_log_r_refreshes_current_screen_and_remaps_filtered_identity",
        &[
            "watch:no-spontaneous-refresh",
            "r:four-rows",
            "r:preserves-selected-id",
            "r:cursor-one",
            "changes:r-reloads",
        ],
        &["Summary-author-filter"],
        "vcs_port_watch_head_refs_linked_index_and_refresh_filtered_identity",
    ),
    (
        "e2e_vcs_log_head_and_refs_injected_watch_events_refresh_active_log",
        &["watch:no-spontaneous-refresh", "watch:adds-one-row"],
        &["HEAD", "refs-heads-main"],
        "vcs_port_watch_head_refs_linked_index_and_refresh_filtered_identity",
    ),
    (
        "e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st",
        &[
            "log:200-row-limit",
            "log:exact-backend-ids",
            "cells:open-and-bottom",
        ],
        &["205-commits"],
        "vcs_port_log_205_commits_keep_exactly_the_first_200",
    ),
    (
        "e2e_vcs_graph_picker_opens_over_shared_branch_key_and_matches_cells",
        &[
            "picker:pushed",
            "picker:screen-id",
            "cells:initial-and-added-branch",
            "b:branches-outside-graph",
            "branches:not-pushed",
        ],
        &["icons-off", "icons-on"],
        "vcs_port_picker_all_keys_icons_languages_and_branch_refresh",
    ),
    (
        "e2e_vcs_graph_picker_toggles_head_all_current_reorders_and_moves_like_legacy",
        &[
            "cells:every-character-key",
            "flash:every-character-key",
            "cells:control-and-motion-keys",
        ],
        &["en:icons-off", "en:icons-on", "ja:icons-off", "ja:icons-on"],
        "vcs_port_picker_all_keys_icons_languages_and_branch_refresh",
    ),
    (
        "e2e_vcs_graph_picker_has_no_filter",
        &[
            "slash:not-filtering",
            "slash:same-list",
            "cells:slash",
            "footer:no-filter",
            "b:modal-remains",
            "cells:b",
        ],
        &["git"],
        "vcs_port_picker_all_keys_icons_languages_and_branch_refresh",
    ),
    (
        "e2e_vcs_graph_picker_apply_rebuilds_and_reorder_derives_base",
        &[
            "apply:not-pushed",
            "cells:applied-and-reopened",
            "title:base-label-or-absent",
        ],
        &[
            "l:unchanged-order",
            "l:reordered",
            "enter:unchanged-order",
            "enter:reordered",
        ],
        "vcs_port_picker_apply_and_every_cancel_key",
    ),
    (
        "e2e_vcs_graph_picker_cancel_discards_tentative_and_retains_order",
        &[
            "cancel:not-pushed",
            "cells:cancelled-graph",
            "cells:reopened-order-and-visible",
        ],
        &["h", "q", "escape"],
        "vcs_port_picker_apply_and_every_cancel_key",
    ),
    (
        "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
        &[
            "fixture:connector",
            "cells:initial-every-motion-and-query",
            "motion:crosses-connector",
        ],
        &[
            "en:80:right-12,inline-12,right-1,right-0-priority",
            "ja:80:right-12,inline-12,right-1,right-0-priority",
            "en:160:right-12,inline-12,right-1,right-0-priority",
            "ja:160:right-12,inline-12,right-1,right-0-priority",
            "80:height-5,6,7,8:G,g",
            "160:height-5,6,7,8:G,g",
        ],
        "vcs_port_graph_widths_queries_connector_motion_and_short_heights",
    ),
    (
        "e2e_vcs_graph_picker_applied_visible_set_is_per_tab",
        &[
            "tab0:filtered-differs",
            "tab1:original-list",
            "tab0:restores-filtered",
        ],
        &["two-tabs"],
        "vcs_host_picker_visible_set_is_per_tab",
    ),
    (
        "e2e_vcs_graph_picker_tab_change_closes_panel_and_discards_tentative_like_legacy",
        &[
            "new-tab:no-picker",
            "old-tab:no-picker",
            "cells:restored-graph",
            "cells:reopened-picker-keeps-order-discards-tentative",
        ],
        &["two-tabs"],
        "vcs_host_tab_change_discards_tentative_picker_but_keeps_order",
    ),
    (
        "e2e_vcs_graph_base_visible_order_and_view_cache_are_per_tab",
        &[
            "tab0:base-title",
            "tab0:hidden-chip",
            "tab1:different-id",
            "tab1:no-base",
            "tab0:entire-cached-list-restored",
        ],
        &["two-tabs:cap-one"],
        "vcs_host_base_order_and_cached_view_are_per_tab",
    ),
    (
        "e2e_vcs_graph_git_h_and_left_return_like_the_old_hub",
        &["graph:closed", "origin:restored"],
        &["h", "left"],
        "vcs_host_git_graph_h_and_left_return_to_tree_and_preview",
    ),
    (
        "e2e_vcs_graph_jj_h_and_left_return_like_the_old_hub",
        &["graph:closed", "origin:restored"],
        &["h", "left"],
        "vcs_host_jj_graph_h_and_left_return_to_tree_and_preview",
    ),
    (
        "e2e_vcs_graph_tab_close_discards_only_closed_tabs_cached_views",
        &[
            "cache:both-tabs-present",
            "close:one-notification",
            "cache:surviving-tab",
            "cache:closed-tab-absent",
            "rows:surviving-tab-unchanged",
            "r:closed-tab-still-absent",
        ],
        &["active-tab", "inactive-tab"],
        "vcs_host_close_notifies_and_discards_only_the_closed_tab",
    ),
    (
        "e2e_vcs_branches_help_marks_and_filter_follow_each_project_list_declaration",
        &[
            "screen:declaration",
            "marks:enabled",
            "content:list",
            "help:V-iff-declared",
            "help:mark-wording",
            "help:filter-wording",
        ],
        &[
            "en:changes-marks",
            "ja:changes-marks",
            "en:log-no-marks",
            "ja:log-no-marks",
            "en:branches-no-marks",
            "ja:branches-no-marks",
        ],
        "vcs_port_declared_marks_and_branch_copy_help_follow_the_real_list",
    ),
    (
        "e2e_vcs_branches_copy_menu_clipboard_footer_and_help_match_legacy_acts",
        &[
            "help:git-write-descriptions",
            "help:filter",
            "help:no-marks",
            "footer:write-labels",
            "menu:key-label",
            "menu:cells",
            "clipboard:branch-name",
            "flash:copied",
        ],
        &["en:current", "ja:current", "en:other", "ja:other"],
        "vcs_port_declared_marks_and_branch_copy_help_follow_the_real_list",
    ),
    (
        "e2e_vcs_project_footer_log_changes_and_worktrees_have_one_entry_per_key",
        &[
            "screen:cycle",
            "footer:one-enter",
            "footer:no-duplicate-chords",
            "changes:shared-control-priority",
            "changes:write-order",
        ],
        &["changes", "log", "graph", "branches"],
        "vcs_port_log_and_changes_help_filter_marks_and_footer_priority",
    ),
    (
        "e2e_vcs_project_log_and_changes_help_filter_and_marks_match_key_behavior",
        &[
            "screen:identity",
            "footer:exact-shared-token-prefix",
            "help:one-filter-row",
            "help:no-V",
            "marks:disabled",
            "V:no-marks",
            "filter:typing",
            "filter:one-hit",
            "filter:no-hit",
            "escape:clears-filter",
            "escape:keeps-screen",
            "detail:screen",
            "detail:exact-footer",
        ],
        &["en", "ja"],
        "vcs_port_log_and_changes_help_filter_marks_and_footer_priority",
    ),
    // Frozen old-hub frames: each row says that the old test's compared body cells are held as a
    // fixture (`src/e2e_tests/vcs_frozen`) and replayed through the project path by the named test.
    (
        "e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub",
        &["frozen:cells"],
        &["relative"],
        "e2e_vcs_frozen_replay_changes_paths",
    ),
    (
        "e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_changes_git",
    ),
    (
        "e2e_vcs_changes_jj_and_colocated_body_cell_parity_and_enter_parent_baseline",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_changes_jj",
    ),
    (
        "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_branches_jj",
    ),
    (
        "e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_detail_cells",
    ),
    (
        "e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_detail_missing",
    ),
    (
        "e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_git_cells",
    ),
    (
        "e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_filter",
    ),
    (
        "e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_unborn",
    ),
    (
        "e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_limit",
    ),
    (
        "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_log_jj",
    ),
    (
        "e2e_vcs_graph_picker_opens_over_shared_branch_key_and_matches_cells",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_opens",
    ),
    (
        "e2e_vcs_graph_picker_toggles_head_all_current_reorders_and_moves_like_legacy",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_toggles",
    ),
    (
        "e2e_vcs_graph_picker_has_no_filter",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_no_filter",
    ),
    (
        "e2e_vcs_graph_picker_apply_rebuilds_and_reorder_derives_base",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_apply",
    ),
    (
        "e2e_vcs_graph_picker_cancel_discards_tentative_and_retains_order",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_cancel",
    ),
    (
        "e2e_vcs_graph_picker_tab_change_closes_panel_and_discards_tentative_like_legacy",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_tab_change",
    ),
    (
        "e2e_vcs_graph_picker_jj_git_branch_source_empty_and_colocated_and_hints",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_picker_jj_source",
    ),
    (
        "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_cells",
    ),
    (
        "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_base_reload_detail",
    ),
    (
        "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_graph_jj_cells",
    ),
    (
        "e2e_vcs_writes_stage_unstage_modified_new_deleted_renamed_cjk_and_path_styles",
        &[
            "stage:flash",
            "stage:status",
            "stage:rows",
            "unstage:flash",
            "unstage:status",
            "unstage:rows",
        ],
        &["en,ja:relative,home,full:a-modified,b-new,c-deleted,d-renamed,sub-cjk"],
        "vcs_port_write_stage_unstage_each_file_kind_style_and_language",
    ),
    (
        "e2e_vcs_writes_all_and_empty_guards_match_without_running_git",
        &[
            "empty:stage-all:flash",
            "empty:unstage-all:flash",
            "empty:index-bytes-status-and-lock-unchanged",
            "empty:rows",
            "changed:stage-all:flash-status-rows",
            "changed:unstage-all:flash-status-rows",
            "changed:second-unstage:nothing-staged",
        ],
        &["en,ja"],
        "vcs_port_write_all_and_empty_guards_run_no_git",
    ),
    (
        "e2e_vcs_writes_empty_file_keys_never_invoke_git_or_flash",
        &[
            "s:no-flash-no-act",
            "u:no-flash-no-act",
            "x:no-flash-no-dialog",
        ],
        &["git"],
        "vcs_port_write_empty_list_keys_never_invoke_git_or_flash",
    ),
    (
        "e2e_vcs_writes_discard_confirmation_and_failures",
        &[
            "discard:confirm-text",
            "discard:success-file-flash-rows-act",
            "untracked:failure-flash",
            "index-lock:s:failure-flash",
            "index-lock:u:failure-flash",
            "index-lock:S:failure-flash",
            "index-lock:U:failure-flash",
        ],
        &["en,ja"],
        "vcs_port_write_discard_confirmation_success_and_failures",
    ),
    (
        "e2e_vcs_writes_discard_cancel_n_matches_legacy_flash_and_closes",
        &[
            "n:flash-closes-no-act-bytes-unchanged",
            "N:flash-closes-no-act-bytes-unchanged",
        ],
        &["en,ja"],
        "vcs_port_write_discard_cancel_n_and_uppercase_n_flash_and_close",
    ),
    (
        "e2e_vcs_writes_discard_cancel_escape_matches_legacy_flash_and_closes",
        &["esc:flash-closes-no-act-bytes-unchanged"],
        &["en,ja"],
        "vcs_port_write_discard_cancel_escape_flashes_and_closes",
    ),
    (
        "e2e_vcs_writes_discard_enter_does_nothing_and_uppercase_y_confirms",
        &["enter:inert-no-act", "Y:confirms-act-flash-rows"],
        &["en,ja"],
        "vcs_port_write_discard_enter_does_nothing_and_uppercase_y_confirms",
    ),
    (
        "e2e_vcs_writes_commit_success_empty_and_failed_hook_preserves_prompt",
        &[
            "blank:flash-closes-no-commit",
            "success:flash-log-rows",
            "failed-hook:flash-keeps-typed-input",
        ],
        &["en,ja"],
        "vcs_port_write_commit_blank_success_and_failed_hook_keep_the_prompt",
    ),
    (
        "e2e_vcs_writes_read_only_refuses_every_git_write_before_prompt_or_dispatch",
        &[
            "s:guard-flash-no-prompt-no-act",
            "u:guard-flash-no-prompt-no-act",
            "S:guard-flash-no-prompt-no-act",
            "U:guard-flash-no-prompt-no-act",
            "x:guard-flash-no-prompt-no-act",
            "c:guard-flash-no-prompt-no-act",
        ],
        &["en,ja"],
        "vcs_port_write_read_only_refuses_every_git_write_before_prompt_or_dispatch",
    ),
    (
        "e2e_vcs_writes_refresh_current_screen_tree_status_and_commit_chip",
        &[
            "s:tree-dirty-and-list-refreshed",
            "u:tree-dirty-and-list-refreshed",
            "S:tree-dirty-and-list-refreshed",
            "U:tree-dirty-and-list-refreshed",
            "x:tree-dirty-and-list-refreshed",
            "commit:tree-dirty-view-and-chip-counts",
            "commit:rows-and-log",
        ],
        &["git"],
        "vcs_port_write_refresh_current_screen_tree_status_and_commit_chip",
    ),
    (
        "e2e_vcs_writes_footer_help_exact_except_empty_inert_file_keys",
        &[
            "footer:changed-tokens",
            "footer:empty-tokens",
            "help:changed-rows",
            "help:empty-rows",
        ],
        &["en,ja"],
        "vcs_port_write_footer_and_help_write_hints_and_empty_list_exception",
    ),
    (
        "e2e_vcs_writes_jj_sync_confirmation_cancel_enter_and_uppercase_y",
        &[
            "confirm:text",
            "cancel:n-N-esc-no-act-no-snapshot-flash-rows",
            "enter:inert",
            "Y:snapshot-act-flash-oplog-rows",
            "Y:tree-dirty-view-and-chip-counts",
            "hints:sync",
        ],
        &["en,ja:plain,colocated"],
        "vcs_port_write_jj_sync_confirmation_cancel_enter_and_uppercase_y",
    ),
    (
        "e2e_vcs_writes_jj_sync_setting_false_snapshots_without_prompt",
        &["no-prompt:act-flash-snapshot-rows", "hints:sync"],
        &["en,ja:plain,colocated"],
        "vcs_port_write_jj_sync_setting_false_snapshots_without_prompt",
    ),
    (
        "e2e_vcs_writes_jj_sync_read_only_refuses_before_prompt_or_snapshot",
        &[
            "guard-flash-no-prompt-no-act",
            "state-unchanged-rows",
            "hints:none-help-d-only",
        ],
        &["en,ja:confirm-false,confirm-true"],
        "vcs_port_write_jj_sync_read_only_refuses_before_prompt_or_snapshot",
    ),
    (
        "e2e_vcs_branches_checkout_current_and_other_branch_reload_and_tree_status",
        &[
            "checkout:flash",
            "checkout:current-branch",
            "checkout:screen-stays-on-branches",
            "checkout:row-marked-current",
            "checkout:tree-dirty-then-clean",
            "checkout:host-git-branch",
            "checkout:l-switches-main",
            "conflict:flash-and-branch-unchanged",
        ],
        &["en,ja"],
        "vcs_port_branch_checkout_current_and_other_branch_reload_and_tree_status",
    ),
    (
        "e2e_vcs_branches_create_success_blank_and_failure_keeps_typed_input",
        &[
            "blank:flash-closes",
            "duplicate:flash-keeps-typed-input",
            "success:flash-current-row",
        ],
        &["en,ja"],
        "vcs_port_branch_create_success_blank_and_failure_keeps_typed_input",
    ),
    (
        "e2e_vcs_branches_delete_dialog_safe_force_cancel_and_current_guard",
        &[
            "current:guard-flash-no-dialog",
            "confirm:text",
            "n:cancel-flash-ref-kept",
            "esc:cancel-flash-ref-kept",
            "y:deletes-flash",
            "y:unmerged-refused-ref-kept",
            "bang:force-deletes-flash",
        ],
        &["en,ja"],
        "vcs_port_branch_delete_dialog_safe_force_cancel_and_current_guard",
    ),
    (
        "e2e_vcs_branches_read_only_guard_precedes_all_prompts_and_writes",
        &[
            "enter:guard-flash-no-prompt-no-write",
            "c:guard-flash-no-prompt-no-write",
            "d:guard-flash-no-prompt-no-write",
        ],
        &["en,ja"],
        "vcs_port_branch_read_only_guard_precedes_all_prompts_and_writes",
    ),
    (
        "e2e_vcs_branches_empty_allows_create_and_has_no_row_actions",
        &[
            "rows:empty-placeholder",
            "enter:no-dialog",
            "d:no-dialog",
            "c:blank-flash",
        ],
        &["git"],
        "vcs_port_branch_empty_repository_allows_create_and_has_no_row_actions",
    ),
    (
        "e2e_vcs_worktrees_create_works_on_empty_filter_and_blank_does_not_reopen",
        &["filter:empty-create-works", "blank:flash-no-reopen"],
        &["en"],
        "vcs_port_worktree_create_works_on_empty_filter_and_blank_does_not_reopen",
    ),
    (
        "e2e_vcs_worktrees_diff_empty_and_refused_rows_flash_without_push",
        &["diff:refused-flash", "diff:stays-on-screen-cursor-and-root"],
        &["en,ja:main-clean,prunable,bare"],
        "vcs_port_worktree_diff_empty_and_refused_rows_flash_without_push",
    ),
    (
        "e2e_vcs_worktrees_read_only_blocks_create_before_prompt_but_all_navigation_and_diff_work",
        &[
            "create:guard-flash-no-prompt-no-write",
            "enter:opens-linked-root",
            "diff:opens-worktree-diff",
        ],
        &["en,ja"],
        "vcs_port_worktree_read_only_blocks_create_before_prompt_but_navigation_and_diff_work",
    ),
    // E/H ports (U9b3): the worktrees screens in `vcs_worktrees_port.rs`, written out as values.
    (
        "e2e_vcs_worktrees_create_dialog_blank_and_git_failure_flash_without_reopen",
        &[
            "create:dialog-prompt-and-empty-buffer",
            "create:flash",
            "create:no-dialog-after",
            "create:root-and-screen-kept",
        ],
        &["en,ja:blank,main"],
        "vcs_port_worktree_create_blank_and_git_failure_flash_without_reopen",
    ),
    (
        "e2e_vcs_worktrees_create_switches_to_legacy_target_with_slash_existing_branch_and_custom_directory",
        &[
            "create:target-dir-and-name",
            "create:root",
            "create:open-dir",
            "create:flash",
            "create:dialog-closed",
            "create:project-closed-tree-mode",
            "create:git-file",
        ],
        &["en,ja:slash-new-branch,existing-custom-dir"],
        "vcs_port_worktree_create_switches_with_slash_existing_branch_and_custom_directory",
    ),
    (
        "e2e_vcs_worktrees_enter_each_kind_matches_legacy_root_open_dir_and_flash_including_l",
        &["enter:root", "enter:open-dir", "enter:flash"],
        &["en,ja:linked,detached,current,prunable,bare:enter,l"],
        "vcs_port_worktree_enter_each_kind_root_open_dir_and_flash_including_l",
    ),
    (
        "e2e_vcs_worktrees_enter_each_kind_matches_legacy_root_open_dir_and_flash_including_l",
        &["enter:refused-keeps-project-screen"],
        &["en,ja:current,prunable,bare:enter,l"],
        "vcs_port_worktree_enter_each_kind_root_open_dir_and_flash_including_l",
    ),
    (
        "e2e_vcs_worktrees_ctrl_t_directory_root_open_dir_preserves_original_and_file_target_still_previews",
        &[
            "ctrl-t:tab-count",
            "ctrl-t:new-tab-root-open-dir",
            "ctrl-t:tree-mode",
            "ctrl-t:original-tab-root-screen-cursor",
            "ctrl-t:file-target-previews",
        ],
        &["en"],
        "vcs_port_host_only_worktree_ctrl_t_opens_new_tab_keeps_original_and_file_target_previews",
    ),
    (
        "e2e_vcs_worktree_filter_ignores_control_chords_like_legacy_and_keeps_shift_text",
        &[
            "filter:ctrl-chord-not-text",
            "filter:shift-and-cjk-text",
            "filter:alt-newline-nul-rejected",
            "filter:backspace-enter-esc",
        ],
        &["en,ja"],
        "vcs_port_worktree_filter_ignores_control_chords_and_keeps_shift_text",
    ),
    (
        "e2e_vcs_worktree_switch_vanished_after_plugin_check_keeps_screen_and_root",
        &[
            "vanished:act-observed",
            "vanished:root-open-dir-kept",
            "vanished:screen-frame-kept",
            "vanished:not-found-flash",
        ],
        &["en,ja"],
        "vcs_port_host_only_worktree_switch_vanished_after_plugin_check_keeps_screen_and_root",
    ),
    (
        "e2e_vcs_worktrees_ctrl_t_without_target_does_nothing_and_has_no_help_or_footer_hint",
        &[
            "no-target:no-controls-hint",
            "no-target:no-help-row",
            "no-target:inert-tabs-root-cursor-flash",
        ],
        &["current,prunable,bare"],
        "vcs_port_host_only_worktree_ctrl_t_without_target_is_inert_and_unlisted",
    ),
    (
        "e2e_vcs_worktrees_cells_open_move_filter_hit_and_empty_main_and_linked_both_widths_and_languages",
        &[
            "cells:edge-title-count-tabs",
            "cells:rows-kinds-order",
            "cells:open-cursor",
            "cells:move",
            "cells:filter-hits",
            "cells:filter-no-match-empty-row",
        ],
        &["main,linked:en,ja:80,160"],
        "vcs_port_worktree_top_edge_open_cursor_move_and_filter_rows",
    ),
    (
        "e2e_vcs_worktrees_cells_open_move_filter_hit_and_empty_main_and_linked_both_widths_and_languages",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_worktrees_cells",
    ),
    (
        "e2e_vcs_worktrees_diff_header_body_cells_layout_motion_and_back_match_legacy",
        &[
            "diff:screen",
            "diff:header-empty-title",
            "diff:lines",
            "diff:edge-title",
            "diff:motion-keys",
            "diff:layout-mark",
            "diff:back-restores-list-and-cursor",
        ],
        &["en,ja:80,160:linked,main"],
        "vcs_port_worktree_diff_title_body_layout_motion_and_back",
    ),
    (
        "e2e_vcs_worktrees_diff_header_body_cells_layout_motion_and_back_match_legacy",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_worktrees_diff",
    ),
    (
        "e2e_vcs_worktrees_help_matches_legacy_action_texts_in_both_languages_with_only_target_exception",
        &[
            "help:rows-texts",
            "help:ctrl-t-only-on-target",
            "help:section-title",
            "help:no-v-row",
        ],
        &["en,ja:main,linked"],
        "vcs_port_worktree_help_rows_in_both_languages_with_only_target_exception",
    ),
    (
        "e2e_vcs_branches_cells_current_selection_move_and_filter_at_two_widths",
        &["edge:title-count-tabs"],
        &["en,ja:80,160"],
        "e2e_vcs_branches_top_edge_literal_title_count_and_tabs_at_two_widths",
    ),
    (
        "e2e_vcs_branches_cells_current_selection_move_and_filter_at_two_widths",
        &["frozen:cells"],
        &["fixture"],
        "e2e_vcs_frozen_replay_branches_cells",
    ),
    (
        "e2e_vcs_graph_jj_title_matches_hub_in_both_languages",
        &["edge:title-count", "edge:no-base-text", "edge:tabs"],
        &["en,ja"],
        "e2e_vcs_graph_jj_top_edge_literal_in_both_languages",
    ),
    // ---- U9b5a: the old-hub e2e tests of vcs_hub_e2e_port.rs (90 rows) ----
    (
        "e2e_git_branches_view_lists_branches",
        &[
            "b:opens-branches",
            "title:literal",
            "non-current-branch:shown",
            "current-marker:shown",
            "decided:q-leaves-to-origin",
        ],
        &["port"],
        "vcs_hub_e2e_branches_screen_lists_branches_marks_the_current_one_and_q_leaves",
    ),
    (
        "e2e_jj_write_actions_are_gated_by_the_read_only_backend",
        &[
            "sanity:read-only",
            "decided:changes-keys-do-nothing",
            "changes:no-dialog",
            "changes:stays-on-screen",
            "q:leaves-to-origin",
        ],
        &["port"],
        "vcs_hub_e2e_jj_changes_stage_family_keys_do_nothing_and_leave_the_repository_alone",
    ),
    (
        "e2e_jj_write_actions_are_gated_by_the_read_only_backend",
        &["decided:bookmark-keys-do-nothing", "bookmarks:no-dialog"],
        &["port"],
        "vcs_hub_e2e_jj_bookmarks_enter_and_write_keys_dispatch_nothing",
    ),
    (
        "e2e_jj_write_actions_are_gated_by_the_read_only_backend",
        &["tree-diff:discard-refused"],
        &["port"],
        "vcs_hub_e2e_jj_tree_diff_discard_is_refused_and_stays_on_the_diff",
    ),
    (
        "e2e_jj_write_gate_holds_in_a_colocated_repository_pinned_to_jj",
        &[
            "sanity:git-state-not-empty",
            "decided:both-offered-by-chooser",
            "changes:keys-do-nothing",
            "bookmarks:keys-do-nothing",
            "git-status:unchanged",
            "bookmarks:list-opens",
        ],
        &["port"],
        "vcs_hub_e2e_colocated_repository_pinned_to_jj_offers_both_and_the_jj_choice_never_writes",
    ),
    (
        "e2e_ui_keeps_responding_while_a_git_commit_is_blocked",
        &[
            "commit:running",
            "log:not-committed",
            "q:leaves",
            "cursor:moves",
            "frame:draws",
            "still:running",
            "log:unchanged",
            "flag:cleared-after-release",
            "commit:completes",
        ],
        &["port"],
        "vcs_hub_e2e_ui_keeps_responding_while_a_real_git_commit_is_blocked",
    ),
    (
        "e2e_quit_during_a_git_write_asks_for_confirmation",
        &[
            "write:running",
            "Q:does-not-quit",
            "dialog:shown",
            "dialog:warning-text",
            "dialog:warning-drawn",
            "y:quits",
        ],
        &["port"],
        "vcs_hub_e2e_quit_during_a_real_git_commit_asks_even_with_confirm_quit_off",
    ),
    (
        "e2e_quit_during_a_git_write_asks_for_confirmation",
        &["guard:Q-does-not-quit", "guard:dialog-shown", "guard:warning-text"],
        &["port"],
        "e2e_quit_project_write_act_dialog_shows_even_with_confirm_quit_off",
    ),
    (
        "e2e_quit_during_a_git_write_asks_for_confirmation",
        &["guard:y-quits"],
        &["port"],
        "e2e_quit_project_write_act_y_quits_and_n_keeps_the_act_running_to_completion",
    ),
    (
        "e2e_git_branches_filter_narrows_and_clears",
        &[
            "list:opens",
            "list:initial-count",
            "filter:starts",
            "query:narrows-to-one",
            "query:drawn",
            "esc:clears",
        ],
        &["port"],
        "vcs_hub_e2e_branches_filter_narrows_to_the_query_and_clears_on_escape",
    ),
    (
        "e2e_jj_worktrees_key_flashes_instead_of_opening_the_git_list",
        &[
            "decided:6-worktrees-key-does-nothing",
            "no-dialog",
            "stays-on-changes",
        ],
        &["port"],
        "vcs_hub_e2e_jj_worktrees_key_does_nothing_and_opens_no_worktree_screen",
    ),
    (
        "e2e_jj_sync_key_on_a_git_repo_flashes_and_opens_nothing",
        &["decided:6-sync-key-does-nothing", "no-dialog", "no-act"],
        &["port"],
        "vcs_hub_e2e_git_repository_has_no_sync_key_that_flashes_or_opens_a_dialog",
    ),
    (
        "git_views_and_large_diff_render_is_bounded",
        &["views:alloc-bounded"],
        &["port"],
        "vcs_hub_e2e_project_views_and_large_diff_render_is_bounded",
    ),
    (
        "git_views_and_large_diff_render_is_bounded",
        &["diff:alloc-scales-linearly"],
        &["port"],
        "git_file_diff_and_colorization_scale_linearly_with_the_diff_size",
    ),
    (
        "e2e_enter_on_a_jj_bookmark_list_is_refused_like_every_other_write",
        &[
            "list:opens",
            "footer:no-checkout",
            "decided:enter-does-nothing",
            "enter:starts-no-job",
            "enter:stays-on-list",
        ],
        &["port"],
        "vcs_hub_e2e_jj_bookmarks_enter_and_write_keys_dispatch_nothing",
    ),
    (
        "e2e_enter_on_a_git_branch_list_still_checks_out",
        &["list:opens", "footer:checkout-token", "enter:checks-out"],
        &["port"],
        "vcs_hub_e2e_git_branches_footer_offers_checkout_and_enter_checks_out",
    ),
    (
        "e2e_s_stages_in_a_git_hub_and_syncs_in_a_jj_hub_and_h_l_open_and_close",
        &["s:stages-in-git", "s:opens-no-dialog"],
        &["port"],
        "vcs_port_write_stage_unstage_each_file_kind_style_and_language",
    ),
    (
        "e2e_s_stages_in_a_git_hub_and_syncs_in_a_jj_hub_and_h_l_open_and_close",
        &[
            "R:not-bound",
            "decided:6-jj-sync-key-on-git-repo-does-nothing",
        ],
        &["port"],
        "vcs_hub_e2e_git_repository_has_no_sync_key_that_flashes_or_opens_a_dialog",
    ),
    (
        "e2e_s_stages_in_a_git_hub_and_syncs_in_a_jj_hub_and_h_l_open_and_close",
        &["jj:s-is-the-sync-and-asks"],
        &["port"],
        "vcs_port_write_jj_sync_confirmation_cancel_enter_and_uppercase_y",
    ),
    (
        "e2e_s_stages_in_a_git_hub_and_syncs_in_a_jj_hub_and_h_l_open_and_close",
        &["l:opens-diff", "q:returns-to-list", "h:leaves-to-tree"],
        &["port"],
        "vcs_hub_e2e_l_opens_the_diff_q_returns_to_the_list_and_h_leaves",
    ),
    (
        "e2e_branch_list_y_menu_copies_the_name_and_c_creates",
        &[
            "list:opens",
            "y:opens-copy-leader",
            "clipboard:untouched-until-n",
            "n:copies-branch-name",
            "n:no-dialog",
            "c:asks-for-name",
            "decided:h-leaves-to-origin",
        ],
        &["port"],
        "vcs_hub_e2e_branch_list_y_menu_copies_the_name_and_c_creates",
    ),
    (
        "e2e_worktree_list_c_creates_and_n_does_not",
        &[
            "list:opens",
            "n:no-dialog",
            "c:asks-for-branch",
            "decided:h-leaves-to-origin",
        ],
        &["port"],
        "vcs_hub_e2e_worktree_list_c_creates_and_n_does_not",
    ),
    (
        "help_opens_in_git_view_and_shows_git_keys",
        &[
            "help:opens",
            "help:git-section",
            "help:stage-keys",
            "help:closes",
        ],
        &["port"],
        "vcs_hub_e2e_question_mark_opens_help_on_a_git_project_screen_and_closes_again",
    ),
    (
        "tab_keys_per_tab_git_mode_and_literal_in_filter",
        &[
            "tab:t-opens-new-tab",
            "tab:new-tab-plain",
            "tab:return-restores-screen",
        ],
        &["port"],
        "vcs_hub_e2e_t_on_a_git_project_screen_opens_a_plain_tab_and_the_first_tab_keeps_its_screen",
    ),
    (
        "tab_keys_per_tab_git_mode_and_literal_in_filter",
        &[
            "filter:t-no-tab",
            "filter:t-query",
            "filter:I-no-tab",
            "filter:I-query",
        ],
        &["port"],
        "vcs_hub_e2e_filter_input_types_t_and_capital_i_literally_instead_of_opening_a_tab",
    ),
    // ---- U9b5a: the newkeys.rs old-hub keys (72 rows; 6 more tab-cycle rows sit in the grouped
    // section with the other e2e_tab_* rows) ----
    (
        "e2e_p_and_bang_work_from_the_tree_and_every_preview",
        &[
            "origin:context",
            "hint:open-git-view",
            "hint:git-launch-tool",
            "p:opens-project",
            "q:closes",
            "q:origin-mode",
            "q:origin-context",
            "bang:queues-tool",
        ],
        &["tree", "text", "document", "table", "image", "diff"],
        "vcs_hub_e2e_p_and_bang_work_from_the_tree_and_every_preview",
    ),
    (
        "e2e_branch_and_worktree_filters_are_the_same_component",
        &[
            "b:opens-branches",
            "branches:filter-query",
            "branches:narrowed-one",
            "branches:esc-clears",
            "w:opens-worktrees",
            "worktrees:filter-query",
            "worktrees:esc-stays",
        ],
        &["git"],
        "vcs_hub_e2e_branch_and_worktree_filters_are_the_same_component",
    ),
    (
        "e2e_e_o_and_ctrl_t_act_on_the_row_of_the_changes_list",
        &[
            "hint:request-edit",
            "e:pending-edit",
            "o:launch-count",
            "o:launch-args",
            "hint:open-in-new-tab",
            "ctrl-t:hint-shown",
            "ctrl-t:tab-count",
            "ctrl-t:preview-mode",
            "ctrl-t:preview-path",
            "bracket:list-restored",
        ],
        &["port"],
        "vcs_hub_e2e_e_o_and_ctrl_t_act_on_the_row_of_the_changes_list",
    ),
    (
        "e2e_a_deleted_row_offers_neither_e_nor_o",
        &[
            "hint:request-edit",
            "hint:open-external",
            "e:no-pending-edit",
            "o:no-launch",
            "ctrl-t:gate-off",
            "ctrl-t:no-hint",
            "ctrl-t:no-tab",
        ],
        &["port"],
        "vcs_hub_e2e_a_deleted_row_offers_neither_e_nor_o",
    ),
];

// Kept independently from GROUPS so deleting a table entry cannot silently shrink the scope.
const OLD_TESTS: &[(&str, &[&str])] = IN_SCOPE_OLD_TESTS;

/// Rows added by the audit-gap pass (see the row-count assertion for the breakdown).
const GAP_ROWS: usize = 13 + 5 + 2 + 2 + 1;

/// Rows subtracted from the 1593 grouped rows. 1593 counts the old key-context group's 6 rows
/// (the assertions `context:root-canvas`, `context:plugin-backend` and `context:detail`, each for
/// the cases git and jj: 3 x 2 = 6), but those 6 are no longer produced by GROUPS. The 11 rows
/// that replace them (7 `context:*`, 4 `binding:*`) are added back separately in the row-count
/// assertion, so these 6 are subtracted to avoid counting the old group twice.
const CONTEXT_ROWS_MOVED: usize = 3 * 2;

fn port_map() -> Vec<(&'static str, String, &'static str)> {
    let mut table: Vec<_> = GROUPS
        .iter()
        .flat_map(|(old, assertions, cases, new)| {
            assertions.iter().flat_map(move |assertion| {
                cases.iter().flat_map(move |case| {
                    let mut expanded = vec![String::new()];
                    for dimension in case.split(':') {
                        expanded = expanded
                            .iter()
                            .flat_map(|prefix| {
                                dimension.split(',').map(move |value| {
                                    if prefix.is_empty() {
                                        value.to_string()
                                    } else {
                                        format!("{prefix}:{value}")
                                    }
                                })
                            })
                            .collect();
                    }
                    if old.ends_with(
                        "git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty",
                    ) && case.starts_with("changed:")
                    {
                        expanded = expanded
                            .iter()
                            .flat_map(|prefix| {
                                (0..7)
                                    .map(move |i| format!("{prefix}:down-{i}"))
                                    .chain((0..6).rev().map(move |i| format!("{prefix}:up-{i}")))
                            })
                            .collect();
                    }
                    expanded
                        .into_iter()
                        .map(move |case| (*old, format!("{assertion}@{case}"), *new))
                })
            })
        })
        .collect();
    table.extend(title_rows());
    table
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|s| s == "rs") {
            out.push(path);
        }
    }
}

fn names(source: &str) -> BTreeSet<&str> {
    source
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix("fn ")?
                .split_once('(')
                .map(|(name, _)| name.trim())
        })
        .collect()
}

/// Names of the `#[test]` functions of `source`: the attribute must sit among the lines directly
/// above the `fn` (other attributes, doc comments and blank lines may come between).
fn test_names(source: &str) -> BTreeSet<&str> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(name) = line
            .trim_start()
            .strip_prefix("fn ")
            .and_then(|rest| rest.split_once('('))
            .map(|(name, _)| name.trim())
        else {
            continue;
        };
        let is_test = lines[..i]
            .iter()
            .rev()
            .map(|l| l.trim())
            .take_while(|l| l.is_empty() || l.starts_with("#[") || l.starts_with("//"))
            .any(|l| l == "#[test]");
        if is_test {
            out.insert(name);
        }
    }
    out
}

#[test]
fn test_names_requires_the_test_attribute_directly_above_the_function() {
    let source = "#[test]\nfn a() {}\n\n#[cfg(x)]\n/// doc\n#[test]\nfn b() {}\nfn helper() {}\n#[test]\n\nfn c() {}\n#[cfg(x)]\nfn d() {}\n";
    assert_eq!(
        test_names(source),
        BTreeSet::from(["a", "b", "c"]),
        "helper and d carry no #[test] attribute"
    );
}

#[test]
fn every_ported_assertion_names_an_existing_new_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("crates/konoma-vcs/src"), &mut files);
    let contents: Vec<_> = files
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    // A mapped new test must be a real `#[test]` (a helper of the same name would never run).
    let functions: BTreeSet<_> = contents.iter().flat_map(|s| test_names(s)).collect();
    let table = port_map();
    let mut seen = BTreeSet::new();
    let mut old_names = BTreeSet::new();
    for (old, assertion, new) in &table {
        assert!(
            seen.insert((*old, assertion)),
            "duplicate assertion: {old}/{assertion}"
        );
        assert!(!assertion.is_empty());
        assert_ne!(old, new, "a port must not refer back to its old test");
        assert!(
            functions.contains(new),
            "{old}/{assertion}: missing new test fn {new}"
        );
        old_names.insert(*old);
    }
    for (file, tests) in OLD_TESTS {
        let source = std::fs::read_to_string(root.join(file)).unwrap();
        let functions = names(&source);
        for test in *tests {
            assert!(
                functions.contains(test),
                "old test moved or renamed: {file}::{test}"
            );
            assert!(
                old_names.contains(test),
                "in-scope old test is no longer named: {file}::{test}"
            );
        }
    }
    assert_eq!(
        old_names,
        OLD_TESTS
            .iter()
            .flat_map(|(_, tests)| tests.iter().copied())
            .chain(title_rows().into_iter().map(|(old, _, _)| old))
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        table.len(),
        // 1593 grouped rows (+ the 11 key-context and backend-binding rows that are now ordinary
        // assertions), 21 frozen-frame rows, the GAP_ROWS audit-gap rows (13 for the
        // flash texts, old-name config, q/origin, filter and base pins; 5 + 2 for the worktrees
        // Space and one-Enter loops; 2 for unborn open-then-Tab; 1 for the branch footer Enter
        // token), the 4 help rows (screen-cycle label and descriptions, graph key order) that
        // are now ordinary assertions, the 8 rows (6 mode-chip, 2 filter input-line) that are
        // now ordinary assertions, and the pending rows. (The 2 `help:context:changes` rows are
        // dropped: their facts are asserted in place, with no old test name to map.) The log
        // exit test lists `h` as a third key beside `q` and `Esc`: 3 more rows (one per id).
        // The 8 project-footer rows (3 + 1 changes, 2 graph, 2 bookmarks/branches) are now
        // ordinary assertions too. The 6 project-help rows (jj changes-label, close-view,
        // diff-all, bookmarks, external-tool; git worktree) are ordinary assertions in both
        // languages: 6 assertions x 2 = 12 rows replace the 6 pending rows.
        1593 + 11 + 4 + 8 + 8 + 12 + 3 - CONTEXT_ROWS_MOVED
            + 21
            + GAP_ROWS
            + 71
            // The Log Space minimum, now an ordinary assertion: git and jj.
            + 2
            // The D (VCS write) ports: vcs_writes.rs 298 (180 stage/unstage + 14 all/empty + 3
            // empty keys + 14 discard + 4 n/N + 2 esc + 4 enter/Y + 6 commit + 12 read-only + 7
            // refresh + 8 footer/help + 24 jj confirm + 8 jj no-confirm + 12 jj read-only),
            // vcs_branches.rs 46 (16 checkout + 6 create + 14 delete + 6 read-only + 4 empty),
            // vcs_worktrees.rs 20 (2 create + 12 diff + 6 read-only).
            + 298
            + 46
            + 20
            // The E/H ports (U9b3), 279 rows: worktrees create blank/failure 16 + create success
            // 28 + enter each kind 72 (60 + 12 refused) + Ctrl-t tab 5 + filter chords 8 +
            // vanished 8 + no-target Ctrl-t 9 + cells 49 (48 + 1 frozen) + diff 57 (56 + 1
            // frozen) + help 16; branches cells 5 (4 edge + 1 frozen); jj graph title 6.
            + 279
            // The U9b5a old-hub e2e ports, 168 rows: branches view 5 + jj write gate 8 (5 changes + 2
            // bookmarks + 1 tree diff) + colocated pin 6 + blocked commit 9 + quit 10 (6 + 4 guard)
            // + branches filter 6 + jj `w` 3 + git `R` 3 + speed 2 + jj Enter 5 + git Enter 3 +
            // default_keys hub keys 8 + branch y menu 7 + worktree c/n 4 + help 4 + tab/filter 7;
            // the newkeys.rs old-hub keys 78: p/! origins 48 (8 assertions x 6 origins) +
            // branch/worktree filter component 7 + e/o/ctrl-t 10 + deleted-row e/o/ctrl-t 7 +
            // tab-cycle extras 6 (initial screen, footer token, b/w jumps, jj no-worktrees — in
            // the grouped section).
            + 168,
        "a ported assertion/case row was removed"
    );
    assert_eq!(title_rows().len(), 71, "a title row was dropped");
    eprintln!(
        "U9b1: {} old tests; {} assertion/case rows; {} named new tests",
        old_names.len(),
        table.len(),
        table
            .iter()
            .map(|(_, _, new)| new)
            .collect::<BTreeSet<_>>()
            .len()
    );
    for (kind, count) in [
        (
            "existing parity",
            table
                .iter()
                .filter(|(_, _, n)| !n.starts_with("vcs_"))
                .count(),
        ),
        (
            "new B/C",
            table
                .iter()
                .filter(|(_, _, n)| {
                    n.starts_with("vcs_port_")
                        && !n.contains("real_jj_cases_are_guarded")
                        && !n.contains("changelog")
                })
                .count(),
        ),
        (
            "H",
            table
                .iter()
                .filter(|(_, _, n)| n.starts_with("vcs_host_"))
                .count(),
        ),
        (
            "S",
            table
                .iter()
                .filter(|(_, _, n)| {
                    n.contains("real_jj_cases_are_guarded") || n.contains("changelog")
                })
                .count(),
        ),
    ] {
        eprintln!("{kind}: {count} assertion/case rows");
    }
}

fn rust_tokens(source: &str) -> Vec<String> {
    let mut chars = source.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut depth = 1;
                while let Some(c) = chars.next() {
                    if c == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        depth += 1;
                    }
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            '"' => {
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        chars.next();
                    } else if c == '"' {
                        break;
                    }
                }
                tokens.push("literal".into());
            }
            c if c.is_alphanumeric() || c == '_' => {
                let mut word = c.to_string();
                while chars
                    .peek()
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_')
                {
                    word.push(chars.next().unwrap());
                }
                tokens.push(word);
            }
            c => tokens.push(c.to_string()),
        }
    }
    tokens
}

fn body_tokens(source: &str, name: &str) -> Option<Vec<String>> {
    let tokens = rust_tokens(source);
    let start = tokens.windows(2).position(|t| t == ["fn", name])?;
    let open = start + tokens[start..].iter().position(|t| t == "{")?;
    let mut depth = 1;
    let end = tokens[open + 1..].iter().position(|t| {
        if t == "{" {
            depth += 1;
        }
        if t == "}" {
            depth -= 1;
        }
        depth == 0
    })? + open
        + 1;
    Some(tokens[open + 1..end].to_vec())
}

fn jj_guarded(source: &str, name: &str, setup_names: &[&str]) -> bool {
    let Some(body) = body_tokens(source, name) else {
        return false;
    };
    let guard = rust_tokens("if !crate::vcs::jj::available() {");
    let guarded = body.windows(guard.len()).enumerate().find_map(|(i, t)| {
        if t != guard {
            return None;
        }
        let mut depth = 1;
        let start = i + guard.len();
        let end = body[start..].iter().position(|t| {
            if t == "{" {
                depth += 1;
            }
            if t == "}" {
                depth -= 1;
            }
            depth == 0
        })? + start;
        body[start..end].iter().any(|t| t == "return").then_some(i)
    });
    let setup = body
        .windows(2)
        .position(|t| setup_names.contains(&t[0].as_str()) && t[1] == "(");
    matches!((guarded, setup), (Some(g), Some(s)) if g < s)
}

const JJ_SETUPS: &[&str] = &[
    "jj",
    "run_jj",
    "jj_scratch_bare",
    "jj_graph_fixture",
    "graph_jj_fixture",
    "jj_repo",
    "jj_write_cases",
    "jj_launch_cases",
    "jj_diff_cases",
    "jj_wrapped_diff_host_cases",
    "jj_media_project_cases",
    "jj_diff_chrome_cases",
    "jj_colocated_representation_cases",
    "preview_shape_repo_backend",
    "jj_colocated_tool_cases",
    "jj_colocated_changed_list_cases",
    "selected_backend_repo_with_changed_sets",
];

/// Every test that creates a real jj repository, by file: each must return when jj is missing
/// before its first jj setup call.
fn jj_guarded_tests() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        (
            "vcs_project_finish_cases.rs",
            vec![
                "vcs_port_git_a_is_inert_and_jj_a_keeps_literal_notices",
                "vcs_port_graph_picker_and_backend_help_exact_rows",
                "vcs_port_renderer_normal_rows_markers_and_base_selection",
                "vcs_port_backend_contexts_and_bindings_real_repositories",
                "vcs_port_backend_display_words_copy_labels_and_project_origin",
                "vcs_port_mode_chips_follow_the_backend_and_the_screen",
                "vcs_port_mode_chips_do_not_leak_across_tabs",
                "vcs_port_moved_jj_worktree_close_reloads_the_changes_from_the_top",
            ],
        ),
        (
            "vcs_app_parity.rs",
            vec!["jj_graph_and_bookmarks_offer_only_real_actions", "jj_cases"],
        ),
        (
            "vcs_app_port_cases.rs",
            vec![
                "vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source",
                "vcs_port_jj_bookmark_cells_copy_and_full_tab_cycle",
                "vcs_port_jj_changes_fixture_matrix_cells_and_parent_baseline",
            ],
        ),
        (
            "vcs_host_only.rs",
            vec![
                "vcs_host_jj_all_range_is_per_tab",
                "vcs_host_jj_graph_h_and_left_return_to_tree_and_preview",
            ],
        ),
        (
            "vcs_project_port.rs",
            vec!["vcs_port_jj_watch_refreshes_log_for_bare_and_colocated_repositories"],
        ),
        (
            "vcs_project_remaining_cases.rs",
            vec!["vcs_port_original_git_and_jj_cycles_and_contexts"],
        ),
        (
            "vcs_app_media_cases.rs",
            vec![
                "wrapped_diff_hides_horizontal_moves_in_footer_help_and_menu",
                "media_both_sides_pages_keys_and_previews_match_host",
                "colocated_backend_selection_is_consistent_across_all_representations",
                "diff_screen_help_and_project_keys_match_host_in_every_representation",
                "jj_deleted_media_keeps_the_same_keys_after_project_and_refresh",
                "colocated_diff_tool_uses_the_selected_backend",
                "colocated_changed_file_navigation_uses_project_backend_in_every_representation",
                "colocated_tree_changed_file_navigation_keeps_its_global_backend",
            ],
        ),
        (
            "vcs_app_write_cases.rs",
            vec!["confirmation_retry_and_read_only_match_host"],
        ),
        (
            "vcs_app_worktree_cases.rs",
            vec!["project_launch_records_current_target_line_backend_and_cwd"],
        ),
        (
            "vcs_app_diff_cases.rs",
            vec!["jj_project_diff_baselines_match_host"],
        ),
    ]
}

#[test]
fn vcs_port_all_real_jj_cases_are_guarded_before_setup() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/e2e_tests");
    let setups = JJ_SETUPS.to_vec();
    for (file, tests) in jj_guarded_tests() {
        let source = std::fs::read_to_string(root.join(file)).unwrap();
        for name in tests {
            assert!(
                jj_guarded(&source, name, &setups),
                "jj availability guard missing or too late: {file}::{name}"
            );
        }
    }
    let parity = std::fs::read_to_string(root.join("vcs_app_parity.rs")).unwrap();
    for name in [
        "changes_log_detail_cells_and_state_match_host",
        "copy_values_failures_and_cached_detail_match_host",
    ] {
        let body = body_tokens(&parity, name).unwrap();
        assert!(
            body.windows(2).any(|t| t == ["jj_cases", "("]),
            "mixed git/jj recipe must use the guarded setup: {name}"
        );
    }
    assert!(jj_guarded("fn example ( ) { let unrelated = \"let dir { }\"; /* { */ if ! crate :: vcs :: jj :: available ( ) { return ; } jj (root); }", "example", &setups));
    assert!(!jj_guarded(
        "fn example() { jj(root); if !crate::vcs::jj::available() { return; } }",
        "example",
        &setups
    ));
    assert!(!jj_guarded(
        "fn example() { if crate::vcs::jj::available() { return; } jj(root); }",
        "example",
        &setups
    ));
    assert!(jj_guarded(
        "fn example() { if !crate::vcs::jj::available() { return; } jj_graph_fixture(); }",
        "example",
        &setups
    ));
    assert!(!jj_guarded(
        "fn example() { jj_graph_fixture(); if !crate::vcs::jj::available() { return; } }",
        "example",
        &setups
    ));
}

/// Whether a map row asserts a Git case (case dimension `git`, or a `git:` assertion id).
fn is_git_row(assertion: &str) -> bool {
    let (id, case) = assertion.split_once('@').unwrap_or((assertion, ""));
    id.starts_with("git:") || case.split([':', ',']).any(|d| d == "git")
}

/// The index of the first jj-availability guard that returns, if the body has one.
fn jj_guard_position(body: &[String]) -> Option<usize> {
    let guard = rust_tokens("if !crate::vcs::jj::available() {");
    body.windows(guard.len()).position(|t| t == guard)
}

/// A test that carries Git rows must run its Git cases before the jj guard returns: the guard
/// may only skip the jj cases. The Git half is recognised by the project open that precedes the
/// guard (`open(`), after the child-process isolation call.
fn git_cases_precede_the_guard(source: &str, name: &str) -> bool {
    let Some(body) = body_tokens(source, name) else {
        return false;
    };
    let Some(guard) = jj_guard_position(&body) else {
        // No guard at all: the test never skips, so nothing is lost.
        return true;
    };
    let isolated = body.windows(2).position(|t| t == ["isolated", "("]);
    let open = body.windows(2).position(|t| t == ["open", "("]);
    matches!((isolated, open), (Some(i), Some(o)) if i < o && o < guard)
}

#[test]
fn vcs_port_git_cases_run_before_the_jj_guard_returns() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/e2e_tests");
    let table = port_map();
    let mut checked = 0;
    for (file, tests) in jj_guarded_tests() {
        let source = std::fs::read_to_string(root.join(file)).unwrap();
        for name in tests {
            if !table
                .iter()
                .any(|(_, assertion, new)| *new == name && is_git_row(assertion))
            {
                continue;
            }
            checked += 1;
            assert!(
                git_cases_precede_the_guard(&source, name),
                "{file}::{name} carries Git rows but returns on a missing jj before its Git cases"
            );
        }
    }
    // The tests whose Git halves used to be skipped without jj must be in scope (4 carry Git rows).
    assert!(
        checked >= 4,
        "only {checked} Git-carrying guarded tests were checked"
    );
    // Detector self-checks: guard first (the old shape) is rejected; Git half first is accepted.
    assert!(!git_cases_precede_the_guard(
        "fn t() { if !crate::vcs::jj::available() { return; } if isolated(\"t\") { return; } open(a); }",
        "t"
    ));
    assert!(!git_cases_precede_the_guard(
        "fn t() { if isolated(\"t\") { return; } if !crate::vcs::jj::available() { return; } open(a); }",
        "t"
    ));
    assert!(git_cases_precede_the_guard(
        "fn t() { if isolated(\"t\") { return; } open(a); if !crate::vcs::jj::available() { return; } jj(b); }",
        "t"
    ));
    assert!(is_git_row("git:a:no-flash@en"));
    assert!(is_git_row("graph:open@git"));
    assert!(!is_git_row("jj:a:all-notice@en"));
    assert!(!is_git_row("palette:Red@real-status"));
}

#[test]
fn vcs_port_changelog_keeps_each_decided_graph_difference() {
    let notes = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("CHANGELOG.md"))
        .unwrap();
    let unreleased = notes
        .split_once("## [Unreleased]")
        .unwrap()
        .1
        .split("\n## [")
        .next()
        .unwrap();
    let unreleased = unreleased.split_whitespace().collect::<Vec<_>>().join(" ");
    for difference in [
        "second tab",
        "non-selectable rows",
        "uses the keymap",
        "Closing an active or inactive tab",
    ] {
        assert!(
            unreleased.contains(difference),
            "missing graph difference: {difference}"
        );
    }
}
