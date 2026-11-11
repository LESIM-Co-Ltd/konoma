//! Partial U9b1 ledger. Name existence is not a proof of assertion strength or mutation coverage.
//! Unlisted tests and adapter-sensitive style guards must not be removed using this ledger.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

include!("vcs_port_pending_titles.rs");

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
        "e2e_tab_skips_the_worktrees_in_a_jj_repository",
        &["screen:cycle", "backend:jj"],
        &["forward:log,graph,branches,changes"],
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
        "e2e_vcs_log_exit_returns_to_origin_and_h_does_nothing",
        &["h:log-remains", "h:no-leader", "q-or-escape:origin"],
        &["q", "escape"],
        "vcs_host_log_q_and_escape_return_to_the_origin",
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
];

// Kept independently from GROUPS so deleting a table entry cannot silently shrink the scope.
const OLD_TESTS: &[(&str, &[&str])] = &[
    (
        "src/e2e_tests.rs",
        &[
            "e2e_git_commit_detail_shows_full_message",
            "e2e_git_commit_copy_values",
            "e2e_git_copy_leader_all_subkeys_consume_from_log",
            "e2e_git_copy_leader_opens_in_graph_and_detail",
            "e2e_git_log_navigate_open_detail_and_back",
            "e2e_git_graph_branch_picker_toggles",
            "e2e_git_graph_navigates_commit_rows",
            "e2e_git_graph_picker_current_only_then_all",
            "e2e_ui_graph_max_branches_caps_legend_entries",
            "e2e_ui_graph_base_branches_config_auto_selects_base",
            "e2e_git_hub_to_log_shows_commit_subject",
            "e2e_git_graph_shows_commit_nodes",
        ],
    ),
    (
        "src/e2e_tests/default_keys.rs",
        &[
            "e2e_graph_x_clears_the_base_and_zero_does_not",
            "e2e_graph_panel_c_is_current_only_and_l_applies",
            "e2e_the_hub_g_and_l_are_the_list_keys_not_the_log_and_the_graph",
        ],
    ),
    (
        "src/e2e_tests/newkeys.rs",
        &[
            "e2e_slash_filters_the_changes_list_and_the_operations_follow_the_row",
            "e2e_slash_filter_moves_over_the_shown_rows_only",
            "e2e_slash_filters_the_log_by_the_text_shown",
            "e2e_slash_filters_the_graph_and_hides_the_connectors",
            "e2e_tab_walks_the_git_screens_both_ways",
            "e2e_tab_skips_the_worktrees_in_a_jj_repository",
        ],
    ),
    (
        "src/ui/git.rs",
        &[
            "vcs_file_status_colors_preserve_legacy_colors",
            "graph_legend_preserves_each_branch_node_color",
            "graph_cells_preserve_legacy_text_colors_and_modifiers",
            "changes_and_log_titles_gate_branch_marker_by_icons",
            "git_view_renders_marker_and_filename",
            "git_log_render_shows_commit_summary",
            "git_branches_render_shows_current_marker",
            "jj_bookmarks_render_shows_bookmarks_title_and_never_a_current_marker",
        ],
    ),
    (
        "src/e2e_tests/vcs_graph.rs",
        &[
            "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
            "e2e_vcs_graph_picker_jj_git_branch_source_empty_and_colocated_and_hints",
        ],
    ),
    (
        "src/e2e_tests/vcs_changes.rs",
        &[
            "e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub",
            "e2e_vcs_changes_linked_worktree_external_index_event_refreshes_the_session",
            "e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty",
        ],
    ),
    (
        "src/e2e_tests/vcs_log.rs",
        &[
            "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
            "e2e_vcs_log_exit_returns_to_origin_and_h_does_nothing",
            "e2e_vcs_copy_log_and_detail_clipboard_and_flash_match_old_hub_for_all_six_items",
            "e2e_vcs_copy_menu_cells_help_footer_and_space_membership_match_the_declaration",
            "e2e_vcs_copy_empty_log_and_screen_without_copy_acts_do_nothing_without_hints",
            "e2e_vcs_copy_hints_disappear_when_the_menu_prefix_is_unbound",
            "e2e_vcs_copy_clipboard_failure_matches_old_hub_without_writing",
            "e2e_vcs_copy_empty_subject_is_written_on_log_and_detail",
            "e2e_vcs_copy_detail_uses_the_metadata_it_shows_and_log_missing_commit_flashes",
            "e2e_vcs_copy_reopened_cached_details_keep_each_displayed_commit",
            "e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy",
            "e2e_vcs_detail_pushed_screen_is_absent_from_tabs_cycle_and_empty_row_open",
            "e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads",
            "e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling",
            "e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query",
            "e2e_vcs_log_r_refreshes_current_screen_and_remaps_filtered_identity",
            "e2e_vcs_log_head_and_refs_injected_watch_events_refresh_active_log",
            "e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st",
        ],
    ),
    (
        "src/e2e_tests/vcs_graph.rs",
        &[
            "e2e_vcs_graph_jj_repository_tests_guard_optional_binary",
            "e2e_vcs_graph_changelog_records_tab_cache_and_shared_list_differences",
            "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
            "e2e_vcs_graph_picker_opens_over_shared_branch_key_and_matches_cells",
            "e2e_vcs_graph_picker_toggles_head_all_current_reorders_and_moves_like_legacy",
            "e2e_vcs_graph_picker_has_no_filter",
            "e2e_vcs_graph_picker_apply_rebuilds_and_reorder_derives_base",
            "e2e_vcs_graph_picker_cancel_discards_tentative_and_retains_order",
            "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
            "e2e_vcs_graph_picker_applied_visible_set_is_per_tab",
            "e2e_vcs_graph_picker_tab_change_closes_panel_and_discards_tentative_like_legacy",
            "e2e_vcs_graph_base_visible_order_and_view_cache_are_per_tab",
            "e2e_vcs_graph_git_h_and_left_return_like_the_old_hub",
            "e2e_vcs_graph_jj_h_and_left_return_like_the_old_hub",
            "e2e_vcs_graph_tab_close_discards_only_closed_tabs_cached_views",
        ],
    ),
    (
        "src/e2e_tests/vcs_branches.rs",
        &[
            "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
            "e2e_vcs_branches_help_marks_and_filter_follow_each_project_list_declaration",
            "e2e_vcs_branches_copy_menu_clipboard_footer_and_help_match_legacy_acts",
        ],
    ),
    (
        "src/e2e_tests/vcs_worktrees.rs",
        &[
            "e2e_vcs_project_footer_log_changes_and_worktrees_have_one_entry_per_key",
            "e2e_vcs_project_log_and_changes_help_filter_and_marks_match_key_behavior",
        ],
    ),
];

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
    table.extend(
        PENDING_TITLES
            .iter()
            .map(|(old, id, marker)| (*old, (*id).into(), *marker)),
    );
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
    let functions: BTreeSet<_> = contents.iter().flat_map(|s| names(s)).collect();
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
            functions.contains(new)
                || matches!(
                    *new,
                    "PENDING_DECISION:title-count" | "PENDING_DECISION:title-label"
                ),
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
            .chain(PENDING_TITLES.iter().map(|(old, _, _)| *old))
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        table.len(),
        1165 + PENDING_TITLES.len(),
        "a ported assertion/case row was removed"
    );
    for (old, id, marker) in PENDING_TITLES {
        eprintln!("{marker}: {old}/{id}");
    }
    eprintln!(
        "U9b1: {} old tests; {} assertion/case rows; {} named new tests",
        old_names.len(),
        table.len(),
        table
            .iter()
            .filter(|(_, _, new)| !new.starts_with("PENDING_DECISION:"))
            .map(|(_, _, new)| new)
            .collect::<BTreeSet<_>>()
            .len()
    );
    for (kind, count) in [
        (
            "existing parity",
            table
                .iter()
                .filter(|(_, _, n)| !n.starts_with("vcs_") && !n.starts_with("PENDING_DECISION:"))
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

#[test]
fn vcs_port_all_real_jj_cases_are_guarded_before_setup() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/e2e_tests");
    let setups = [
        "jj",
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
    for (file, tests) in [
        (
            "vcs_app_parity.rs",
            vec!["jj_graph_and_bookmarks_offer_only_real_actions", "jj_cases"],
        ),
        (
            "vcs_app_port_cases.rs",
            vec![
                "vcs_port_jj_and_colocated_log_cells_metadata_and_picker_source",
                "vcs_port_jj_bookmark_cells_copy_and_full_tab_cycle",
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
    ] {
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
