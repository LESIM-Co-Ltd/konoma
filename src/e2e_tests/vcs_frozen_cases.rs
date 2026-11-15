//! Frozen frames of the old hub, replayed through the real `p` project path.
//!
//! The old tests compare the legacy hub's body cells with the project screen's body cells; that is the
//! only pin to the released appearance. Each recipe below is the shared sequence of one old test. It is
//! run on the old hub to generate / check the fixtures (`vcs_frozen/<old test>.txt`) and on the new path
//! to replay them. A frame stores every compared cell: symbol, fg, bg and modifier.
//!
//! Generate with `KONOMA_FREEZE_VCS=1 cargo nextest run -p konoma -E 'test(vcs_frozen_generate)'`.
//! The generator is the only code that writes the fixture directory.

use super::vcs_changes::drain;
use super::*;
use ratatui::style::{Color, Modifier};
use std::ops::Range;
use std::path::Path;

#[path = "vcs_frozen_recipes.rs"]
mod recipes;

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/e2e_tests/vcs_frozen/");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Side {
    Old,
    New,
}

/// One side of a recipe run: the old hub or the project path, plus the frames captured so far.
pub(super) struct Ctx {
    pub(super) side: Side,
    sim: Option<Sim>,
    frames: Vec<(String, String)>,
}

impl Ctx {
    fn new(side: Side) -> Self {
        Self {
            side,
            sim: None,
            frames: Vec::new(),
        }
    }

    pub(super) fn is_old(&self) -> bool {
        self.side == Side::Old
    }

    pub(super) fn sim(&mut self) -> &mut Sim {
        self.sim.as_mut().expect("recipe must open a screen first")
    }

    /// Open the first screen: the legacy hub (old) or the project host through `p` (new).
    pub(super) fn open(&mut self, root: &Path, id: &str, cfg: Config, width: u16, height: u16) {
        match self.side {
            Side::Old => {
                let mut old_cfg = cfg;
                old_cfg.plugins.clear();
                let mut old = Sim::with_config_sized(root, old_cfg, width, height);
                old.app.open_git_view();
                assert!(old.app.is_git_view());
                old.draw();
                self.sim = Some(old);
            }
            Side::New => {
                let mut new = Sim::with_config_sized(root, cfg, width, height);
                assert!(
                    crate::plugins::build_registry(&new.app.cfg.plugin_disabled())
                        .manifest(id)
                        .is_some(),
                    "VCS plugin refused by registry"
                );
                new.app.tab.projects.wait_for_result();
                new.app.poll_projects();
                new.key('p');
                drain(&mut new);
                assert!(new.app.project_active(), "{:?}", new.app.flash);
                self.sim = Some(new);
            }
        }
    }

    /// Use a screen the recipe opened itself (a recipe whose opening differs from `open`).
    pub(super) fn set_sim(&mut self, sim: Sim) {
        self.sim = Some(sim);
    }

    /// Wait for the project host's workers (new path only; the old hub has none).
    pub(super) fn settle(&mut self) {
        let new = self.side == Side::New;
        if new {
            drain(self.sim());
        }
    }

    pub(super) fn key(&mut self, c: char) {
        self.sim().key(c);
        self.settle();
    }

    /// Type without waiting, so a recipe can type a whole query and settle once.
    pub(super) fn raw_key(&mut self, c: char) {
        self.sim().key(c);
    }

    pub(super) fn press(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.sim().press(code, mods);
        self.settle();
    }

    pub(super) fn enter(&mut self) {
        self.sim().enter();
        self.settle();
    }

    pub(super) fn esc(&mut self) {
        self.sim().esc();
        self.settle();
    }

    pub(super) fn tab(&mut self) {
        self.sim().tab();
        self.settle();
    }

    /// Run a step that exists only on the old hub (a hub API instead of keys).
    pub(super) fn on_old(&mut self, step: impl FnOnce(&mut Sim)) {
        if self.side == Side::Old {
            step(self.sim());
        }
    }

    /// Run a step that exists only on the project path; settles afterwards.
    pub(super) fn on_new(&mut self, step: impl FnOnce(&mut Sim)) {
        if self.side == Side::New {
            step(self.sim());
            self.settle();
        }
    }

    /// Capture a region the way `compare_body` compares it: (x, y) of the frame, with a per-side
    /// offset (the old hub draws inside a border and one title row more).
    pub(super) fn region(
        &mut self,
        label: &str,
        xs: Range<u16>,
        ys: Range<u16>,
        old_offset: (u16, u16),
        new_offset: (u16, u16),
    ) {
        assert!(
            !self.frames.iter().any(|(l, _)| l == label),
            "duplicate frozen label: {label}"
        );
        let offset = match self.side {
            Side::Old => old_offset,
            Side::New => new_offset,
        };
        let sim = self.sim();
        // A fresh draw, as compare_body does: TestBackend keeps stale symbols under wide glyphs.
        let buffer = sim
            .term
            .draw(|frame| ui::render(frame, &mut sim.app))
            .unwrap()
            .buffer
            .clone();
        let mut rows = Vec::new();
        for y in ys.clone() {
            let mut row = Vec::new();
            for x in xs.clone() {
                let cell = &buffer[(x + offset.0, y + offset.1)];
                row.push((cell.symbol().to_string(), cell.fg, cell.bg, cell.modifier));
            }
            rows.push((y, row));
        }
        self.frames
            .push((label.to_string(), encode(&xs, &ys, &rows)));
    }

    /// The body region of `compare_body`: below the two header rows, inside the old hub's border.
    pub(super) fn body(&mut self, label: &str, width: u16, height: u16) {
        self.region(label, 0..width - 2, 2..height - 2, (1, 0), (1, 0));
    }
}

fn isolated(test: &str) -> bool {
    super::vcs_host_only::isolate_test("vcs_frozen_cases", test)
}

/// Points `HOME` at a recipe's fixture directory. Home-relative paths (`~/...`) are drawn from
/// `HOME` when a frame is drawn, so without this the frames would embed the machine's temp path.
/// `HOME` is process-wide, so only a test that runs in its own child process (`isolated`) may call
/// this. The returned guard puts the previous `HOME` back when the recipe ends, so a process that
/// runs several recipes (the generator) draws every recipe's frames from the same starting `HOME`
/// whatever the recipe order.
#[must_use = "HOME is restored when the guard drops"]
pub(super) fn set_home(dir: &Path) -> HomeGuard {
    assert!(
        std::env::var_os("KONOMA_VCS_HOST_CHILD").is_some(),
        "a recipe that sets HOME must run in an isolated child process"
    );
    let previous = std::env::var_os("HOME");
    std::env::set_var("HOME", dir);
    HomeGuard(previous)
}

pub(super) struct HomeGuard(Option<std::ffi::OsString>);

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// A scratch repository directory whose own name is stable. Frames that show the repository path
/// (relative to its parent) would otherwise embed the pid-based name of `unique_tmp`. The guard
/// removes the whole scratch tree.
pub(super) fn stable_root(name: &str) -> (crate::test_support::TmpDir, std::path::PathBuf) {
    let outer = sandbox(&format!("frozen_{name}")).into_canonical().unwrap();
    let root = outer.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    (outer, root)
}

type Row = (u16, Vec<(String, Color, Color, Modifier)>);

fn style_letter(index: usize) -> char {
    let letters = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    *letters
        .get(index)
        .unwrap_or_else(|| panic!("more than {} styles in one frame", letters.len())) as char
}

/// Text form of one frame: a style palette, then per row the symbols, the symbol widths (only when
/// some symbol is not exactly one char) and the run-length encoded style letters.
fn encode(xs: &Range<u16>, ys: &Range<u16>, rows: &[Row]) -> String {
    let mut palette: Vec<String> = Vec::new();
    let mut body = String::new();
    for (y, row) in rows {
        let symbols: String = row.iter().map(|c| c.0.as_str()).collect();
        body.push_str(&format!("row {y} T|{symbols}|\n"));
        if row.iter().any(|c| c.0.chars().count() != 1) {
            let widths: String = row
                .iter()
                .map(|c| char::from_digit(c.0.chars().count().min(9) as u32, 10).unwrap())
                .collect();
            body.push_str(&format!("row {y} L|{widths}|\n"));
        }
        let mut runs = String::new();
        let mut last: Option<(char, usize)> = None;
        for c in row {
            let style = format!("{:?}\t{:?}\t{:?}", c.1, c.2, c.3);
            let index = match palette.iter().position(|p| *p == style) {
                Some(i) => i,
                None => {
                    palette.push(style);
                    palette.len() - 1
                }
            };
            let letter = style_letter(index);
            last = match last {
                Some((l, n)) if l == letter => Some((l, n + 1)),
                Some((l, n)) => {
                    runs.push_str(&format!("{l}{n}"));
                    Some((letter, 1))
                }
                None => Some((letter, 1)),
            };
        }
        if let Some((l, n)) = last {
            runs.push_str(&format!("{l}{n}"));
        }
        body.push_str(&format!("row {y} S|{runs}|\n"));
    }
    let mut out = format!(
        "region x={}..{} y={}..{}\n",
        xs.start, xs.end, ys.start, ys.end
    );
    for (i, style) in palette.iter().enumerate() {
        out.push_str(&format!("style {}\t{style}\n", style_letter(i)));
    }
    out.push_str(&body);
    out
}

fn file_text(recipe: &str, frames: &[(String, String)]) -> String {
    let mut out = format!("# frozen vcs frames: {recipe}\n");
    for (label, text) in frames {
        assert!(!label.contains('\n'));
        out.push_str(&format!("case {label}\n{text}"));
    }
    out
}

fn parse_file(text: &str) -> Vec<(String, String)> {
    let mut frames: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(label) = line.strip_prefix("case ") {
            frames.push((label.to_string(), String::new()));
        } else if let Some(last) = frames.last_mut() {
            last.1.push_str(line);
            last.1.push('\n');
        }
    }
    frames
}

fn fixture_path(recipe: &str) -> std::path::PathBuf {
    Path::new(FIXTURE_DIR).join(format!("{recipe}.txt"))
}

fn load(recipe: &str) -> Vec<(String, String)> {
    let path = fixture_path(recipe);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("frozen fixture {} unreadable: {e}", path.display()));
    parse_file(&text)
}

/// Cell-level report between a fixture and a fresh run: label set first, then every differing line
/// (a differing `T`/`L`/`S` row or palette line is a differing symbol, width, fg, bg or modifier).
fn differences(expected: &[(String, String)], actual: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    let expected_labels: Vec<_> = expected.iter().map(|f| f.0.as_str()).collect();
    let actual_labels: Vec<_> = actual.iter().map(|f| f.0.as_str()).collect();
    if expected_labels != actual_labels {
        for l in &expected_labels {
            if !actual_labels.contains(l) {
                out.push(format!("case missing from the run: {l}"));
            }
        }
        for l in &actual_labels {
            if !expected_labels.contains(l) {
                out.push(format!("case missing from the fixture: {l}"));
            }
        }
        if out.is_empty() {
            out.push("case order differs".into());
        }
        return out;
    }
    for (e, a) in expected.iter().zip(actual) {
        if e.1 == a.1 {
            continue;
        }
        let mut shown = 0;
        let (el, al): (Vec<_>, Vec<_>) = (e.1.lines().collect(), a.1.lines().collect());
        for i in 0..el.len().max(al.len()) {
            let (x, y) = (el.get(i).copied(), al.get(i).copied());
            if x != y && shown < 3 {
                out.push(format!("[{}] line {i}: fixture {x:?} != run {y:?}", e.0));
                shown += 1;
            }
        }
    }
    out
}

pub(super) struct Recipe {
    /// Name of the old test; also the fixture file stem.
    pub(super) name: &'static str,
    /// Every case of the recipe needs jj (the whole recipe is skipped without it).
    pub(super) jj: bool,
    /// The enumeration of the old test's comparison sites x loop cases, written as the product.
    pub(super) min_cases: usize,
    pub(super) run: fn(&mut Ctx),
}

fn run_recipe(recipe: &Recipe, side: Side) -> Vec<(String, String)> {
    let mut ctx = Ctx::new(side);
    (recipe.run)(&mut ctx);
    ctx.frames
}

fn find(name: &str) -> &'static Recipe {
    recipes::RECIPES
        .iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("unknown recipe {name}"))
}

fn skipped(recipe: &Recipe) -> bool {
    recipe.jj && !crate::vcs::jj::available()
}

/// New path: replay the recipe through `p` and compare every cell with the frozen old-hub frame.
fn replay(name: &str) {
    let recipe = find(name);
    if skipped(recipe) {
        return;
    }
    let expected = load(name);
    let actual = run_recipe(recipe, Side::New);
    let report = differences(&expected, &actual);
    assert!(
        report.is_empty(),
        "{} differences from the frozen frames ({} cases)\n{}",
        report.len(),
        expected.len(),
        report
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Old hub: the fixture must still be what the old hub draws.
// U9l deletes this check together with the old hub.
fn stale(name: &str) {
    let recipe = find(name);
    if skipped(recipe) {
        return;
    }
    let expected = load(name);
    let actual = run_recipe(recipe, Side::Old);
    let report = differences(&expected, &actual);
    assert!(
        report.is_empty(),
        "frozen fixture {name} is stale: {} differences\n{}",
        report.len(),
        report
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn e2e_vcs_frozen_generate_fixtures_only_when_asked() {
    if std::env::var("KONOMA_FREEZE_VCS").as_deref() != Ok("1") {
        return;
    }
    // The generator runs the recipes that set `HOME`; it gets its own process (it inherits
    // `KONOMA_FREEZE_VCS`).
    if isolated("e2e_vcs_frozen_generate_fixtures_only_when_asked") {
        return;
    }
    std::fs::create_dir_all(FIXTURE_DIR).unwrap();
    for recipe in recipes::RECIPES {
        assert!(!skipped(recipe), "jj is required to freeze {}", recipe.name);
        let frames = run_recipe(recipe, Side::Old);
        std::fs::write(fixture_path(recipe.name), file_text(recipe.name, &frames)).unwrap();
    }
}

#[test]
fn e2e_vcs_frozen_case_count_is_at_least_the_enumerated_sites_times_loop_cases() {
    let mut total = 0;
    for recipe in recipes::RECIPES {
        let count = load(recipe.name).len();
        assert!(
            count >= recipe.min_cases,
            "{}: {count} frozen cases, the enumeration has {}",
            recipe.name,
            recipe.min_cases
        );
        total += count;
    }
    assert!(
        total >= recipes::RECIPES.iter().map(|r| r.min_cases).sum::<usize>(),
        "frozen total {total}"
    );
    let files = std::fs::read_dir(FIXTURE_DIR).unwrap().count();
    assert_eq!(files, recipes::RECIPES.len(), "fixture without a recipe");
}

#[test]
fn e2e_vcs_frozen_encoding_distinguishes_symbol_width_and_every_style_part() {
    let cell = |s: &str, fg, bg, m| (s.to_string(), fg, bg, m);
    let base = vec![(
        2u16,
        vec![
            cell("a", Color::Red, Color::Reset, Modifier::empty()),
            cell("日", Color::Red, Color::Reset, Modifier::empty()),
            cell(" ", Color::Red, Color::Reset, Modifier::empty()),
        ],
    )];
    let enc = |rows: &[Row]| encode(&(0..3), &(2..3), rows);
    let reference = enc(&base);
    for (i, change) in [
        (0, cell("b", Color::Red, Color::Reset, Modifier::empty())),
        (0, cell("a", Color::Green, Color::Reset, Modifier::empty())),
        (0, cell("a", Color::Red, Color::Blue, Modifier::empty())),
        (0, cell("a", Color::Red, Color::Reset, Modifier::BOLD)),
        (1, cell("日", Color::Red, Color::Reset, Modifier::DIM)),
        (2, cell("", Color::Red, Color::Reset, Modifier::empty())),
    ] {
        let mut rows = base.clone();
        rows[0].1[i] = change;
        assert_ne!(enc(&rows), reference);
    }
}

#[test]
fn e2e_vcs_frozen_stale_fixture_is_reported_per_cell() {
    let a = vec![("case".to_string(), "row 2 T|ab|\nrow 2 S|a2|\n".to_string())];
    let b = vec![("case".to_string(), "row 2 T|ac|\nrow 2 S|a2|\n".to_string())];
    assert!(differences(&a, &a).is_empty());
    assert_eq!(differences(&a, &b).len(), 1);
    assert_eq!(differences(&a, &[]).len(), 1);
    assert_eq!(parse_file(&file_text("r", &a)), a);
}

macro_rules! stale_tests {
    ($($fn_name:ident => $recipe:expr,)*) => {
        $(
            #[test]
            fn $fn_name() {
                stale($recipe);
            }
        )*
    };
}

stale_tests! {
    e2e_vcs_frozen_stale_changes_git => "e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty",
    e2e_vcs_frozen_stale_changes_jj => "e2e_vcs_changes_jj_and_colocated_body_cell_parity_and_enter_parent_baseline",
    e2e_vcs_frozen_stale_branches_cells => "e2e_vcs_branches_cells_current_selection_move_and_filter_at_two_widths",
    e2e_vcs_frozen_stale_branches_jj => "e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints",
    e2e_vcs_frozen_stale_log_detail_cells => "e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy",
    e2e_vcs_frozen_stale_log_detail_missing => "e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads",
    e2e_vcs_frozen_stale_log_git_cells => "e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling",
    e2e_vcs_frozen_stale_log_filter => "e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query",
    e2e_vcs_frozen_stale_log_unborn => "e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order",
    e2e_vcs_frozen_stale_log_limit => "e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st",
    e2e_vcs_frozen_stale_log_jj => "e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend",
    e2e_vcs_frozen_stale_graph_picker_opens => "e2e_vcs_graph_picker_opens_over_shared_branch_key_and_matches_cells",
    e2e_vcs_frozen_stale_graph_picker_toggles => "e2e_vcs_graph_picker_toggles_head_all_current_reorders_and_moves_like_legacy",
    e2e_vcs_frozen_stale_graph_picker_no_filter => "e2e_vcs_graph_picker_has_no_filter",
    e2e_vcs_frozen_stale_graph_picker_apply => "e2e_vcs_graph_picker_apply_rebuilds_and_reorder_derives_base",
    e2e_vcs_frozen_stale_graph_picker_cancel => "e2e_vcs_graph_picker_cancel_discards_tentative_and_retains_order",
    e2e_vcs_frozen_stale_graph_picker_tab_change => "e2e_vcs_graph_picker_tab_change_closes_panel_and_discards_tentative_like_legacy",
    e2e_vcs_frozen_stale_graph_picker_jj_source => "e2e_vcs_graph_picker_jj_git_branch_source_empty_and_colocated_and_hints",
    e2e_vcs_frozen_stale_graph_cells => "e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority",
    e2e_vcs_frozen_stale_graph_base_reload_detail => "e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub",
    e2e_vcs_frozen_stale_graph_jj_cells => "e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys",
    e2e_vcs_frozen_stale_changes_title_matrix => "e2e_vcs_changes_title_matches_hub_for_icons_languages_filters_and_branch_states",
    e2e_vcs_frozen_stale_changes_title_unborn => "e2e_vcs_changes_title_without_a_branch_matches_hub",
    e2e_vcs_frozen_stale_changes_title_words => "e2e_git_titles_changes_log_branches_and_jj_labels_come_from_the_plugin",
}

// The recipes below point the process-wide `HOME` at their fixture (`set_home`), so each runs in
// its own process; the other tests of this binary read `HOME` too.
#[test]
fn e2e_vcs_frozen_stale_changes_paths() {
    if isolated("e2e_vcs_frozen_stale_changes_paths") {
        return;
    }
    stale("e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub");
}

#[test]
fn e2e_vcs_frozen_stale_worktrees_cells() {
    if isolated("e2e_vcs_frozen_stale_worktrees_cells") {
        return;
    }
    stale("e2e_vcs_worktrees_cells_open_move_filter_hit_and_empty_main_and_linked_both_widths_and_languages");
}

#[test]
fn e2e_vcs_frozen_stale_worktrees_diff() {
    if isolated("e2e_vcs_frozen_stale_worktrees_diff") {
        return;
    }
    stale("e2e_vcs_worktrees_diff_header_body_cells_layout_motion_and_back_match_legacy");
}

#[test]
fn e2e_vcs_frozen_replay_changes_paths() {
    if isolated("e2e_vcs_frozen_replay_changes_paths") {
        return;
    }
    replay("e2e_vcs_changes_nested_tree_and_launch_directory_path_styles_match_the_hub");
}

#[test]
fn e2e_vcs_frozen_replay_changes_git() {
    replay("e2e_vcs_changes_git_body_cell_parity_with_staged_unstaged_rename_delete_cjk_and_empty");
}

#[test]
fn e2e_vcs_frozen_replay_changes_title_matrix() {
    replay("e2e_vcs_changes_title_matches_hub_for_icons_languages_filters_and_branch_states");
}

#[test]
fn e2e_vcs_frozen_replay_changes_title_unborn() {
    replay("e2e_vcs_changes_title_without_a_branch_matches_hub");
}

#[test]
fn e2e_vcs_frozen_replay_changes_title_words() {
    replay("e2e_git_titles_changes_log_branches_and_jj_labels_come_from_the_plugin");
}

#[test]
fn e2e_vcs_frozen_replay_changes_jj() {
    replay("e2e_vcs_changes_jj_and_colocated_body_cell_parity_and_enter_parent_baseline");
}

#[test]
fn e2e_vcs_frozen_replay_branches_cells() {
    replay("e2e_vcs_branches_cells_current_selection_move_and_filter_at_two_widths");
}

#[test]
fn e2e_vcs_frozen_replay_branches_jj() {
    replay("e2e_vcs_branches_jj_bookmark_cells_copy_and_no_write_hints");
}

#[test]
fn e2e_vcs_frozen_replay_log_detail_cells() {
    replay("e2e_vcs_detail_cells_motions_layout_empty_and_less_match_legacy");
}

#[test]
fn e2e_vcs_frozen_replay_log_detail_missing() {
    replay("e2e_vcs_detail_missing_commit_empty_without_header_matches_legacy_and_r_reloads");
}

#[test]
fn e2e_vcs_frozen_replay_log_git_cells() {
    replay("e2e_vcs_log_git_cells_alignments_boundaries_cjk_ellipsis_and_scrolling");
}

#[test]
fn e2e_vcs_frozen_replay_log_filter() {
    replay("e2e_vcs_log_filter_uses_legacy_text_and_cursor_for_every_query");
}

#[test]
fn e2e_vcs_frozen_replay_log_unborn() {
    replay("e2e_vcs_log_unborn_empty_cells_and_fixed_tab_order");
}

#[test]
fn e2e_vcs_frozen_replay_log_limit() {
    replay("e2e_vcs_log_limit_matches_hub_at_200_and_omits_201st");
}

#[test]
fn e2e_vcs_frozen_replay_log_jj() {
    replay("e2e_vcs_log_jj_and_both_colocated_backends_match_the_selected_legacy_backend");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_opens() {
    replay("e2e_vcs_graph_picker_opens_over_shared_branch_key_and_matches_cells");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_toggles() {
    replay("e2e_vcs_graph_picker_toggles_head_all_current_reorders_and_moves_like_legacy");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_no_filter() {
    replay("e2e_vcs_graph_picker_has_no_filter");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_apply() {
    replay("e2e_vcs_graph_picker_apply_rebuilds_and_reorder_derives_base");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_cancel() {
    replay("e2e_vcs_graph_picker_cancel_discards_tentative_and_retains_order");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_tab_change() {
    replay("e2e_vcs_graph_picker_tab_change_closes_panel_and_discards_tentative_like_legacy");
}

#[test]
fn e2e_vcs_frozen_replay_graph_picker_jj_source() {
    replay("e2e_vcs_graph_picker_jj_git_branch_source_empty_and_colocated_and_hints");
}

#[test]
fn e2e_vcs_frozen_replay_graph_cells() {
    replay("e2e_vcs_graph_cells_open_move_filter_inline_width_legend_cap_and_priority");
}

#[test]
fn e2e_vcs_frozen_replay_graph_base_reload_detail() {
    replay("e2e_vcs_graph_base_reload_detail_copy_and_cursor_return_match_the_hub");
}

#[test]
fn e2e_vcs_frozen_replay_graph_jj_cells() {
    replay("e2e_vcs_graph_jj_cells_title_all_range_flash_and_no_base_keys");
}

#[test]
fn e2e_vcs_frozen_replay_worktrees_cells() {
    if isolated("e2e_vcs_frozen_replay_worktrees_cells") {
        return;
    }
    replay("e2e_vcs_worktrees_cells_open_move_filter_hit_and_empty_main_and_linked_both_widths_and_languages");
}

#[test]
fn e2e_vcs_frozen_replay_worktrees_diff() {
    if isolated("e2e_vcs_frozen_replay_worktrees_diff") {
        return;
    }
    replay("e2e_vcs_worktrees_diff_header_body_cells_layout_motion_and_back_match_legacy");
}
