// The outer frame of every project list: drawn like the old hub's, the title at the left of the
// top edge, the screen tabs at its right end (dropped first when the width is short), and no list
// row fewer than the old hub showed at the same terminal size.

use crate::test_support::top_edge;

#[derive(Clone, Copy, Debug)]
enum EdgeScreen {
    Changes,
    Log,
    Graph,
    Branches,
    Worktrees,
    Picker,
}

impl EdgeScreen {
    /// The project screen id the canvas shows for it.
    fn id(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::Log => "log",
            Self::Graph => "graph",
            Self::Branches => "branches",
            Self::Worktrees => "worktrees",
            Self::Picker => "graph_picker",
        }
    }
}

const GIT_EDGE_SCREENS: [EdgeScreen; 6] = [
    EdgeScreen::Changes,
    EdgeScreen::Log,
    EdgeScreen::Graph,
    EdgeScreen::Branches,
    EdgeScreen::Worktrees,
    EdgeScreen::Picker,
];
const JJ_EDGE_SCREENS: [EdgeScreen; 4] = [
    EdgeScreen::Changes,
    EdgeScreen::Log,
    EdgeScreen::Graph,
    EdgeScreen::Branches,
];

/// A repository whose every list has more rows than a short terminal shows (so a row lost to the
/// frame is visible as a count difference). `None`: jj is not installed.
fn edge_repo(id: &str) -> Option<std::path::PathBuf> {
    let root = old_repo(id, "edge")?;
    if id == "git" {
        for i in 0..8 {
            crate::test_support::run_git(
                &root,
                &["commit", "--allow-empty", "-qm", &format!("edge commit {i}")],
            );
            crate::test_support::run_git(&root, &["branch", &format!("edge-b{i}")]);
        }
        for i in 0..4 {
            let linked = format!("{}-edge-wt{i}", root.display());
            crate::test_support::run_git(&root, &["worktree", "add", "-q", "--detach", &linked]);
        }
    } else {
        for i in 0..8 {
            std::fs::write(root.join(format!("edge{i}.txt")), "x\n").unwrap();
            assert!(crate::test_support::run_jj(
                &root,
                &["new", "-m", &format!("edge change {i}")]
            ));
            assert!(crate::test_support::run_jj(
                &root,
                &["bookmark", "create", &format!("edge-b{i}"), "-r", "@"]
            ));
        }
        for i in 0..8 {
            std::fs::write(root.join(format!("wc{i}.txt")), "x\n").unwrap();
        }
    }
    Some(root)
}

/// The project canvas at the screen.
fn edge_new(
    root: &std::path::Path,
    id: &str,
    lang: &str,
    size: (u16, u16),
    screen: EdgeScreen,
) -> Sim {
    let mut new = open(root, config(id, lang), size);
    match screen {
        EdgeScreen::Changes => {}
        EdgeScreen::Log | EdgeScreen::Graph => self::screen(&mut new, screen.id()),
        EdgeScreen::Branches => press(&mut new, KeyCode::Char('b')),
        EdgeScreen::Worktrees => press(&mut new, KeyCode::Char('w')),
        EdgeScreen::Picker => {
            self::screen(&mut new, "graph");
            press(&mut new, KeyCode::Char('b'));
        }
    }
    assert_eq!(frame_screen(&new), screen.id(), "reaching {screen:?}");
    new
}

fn edge_buffer(sim: &mut Sim) -> ratatui::buffer::Buffer {
    drain(sim);
    sim.term
        .draw(|frame| ui::render(frame, &mut sim.app))
        .unwrap()
        .buffer
        .clone()
}

/// The drawn title of the shown list, as the host fills it (no filter query is set here).
fn edge_title(sim: &Sim) -> String {
    let content = sim.app.project_content().unwrap();
    let ProjectContent::List(list) = content.as_ref() else {
        panic!("a list screen is required")
    };
    if let Some(heading) = &list.heading {
        let counted = sim
            .app
            .project_visible_rows()
            .iter()
            .filter(|i| list.rows[**i].counts_toward_heading())
            .count();
        return format!(
            "{}  ({counted}){}",
            heading.before.plain(),
            heading.after.plain()
        );
    }
    list.title.as_ref().map(|t| t.plain()).unwrap_or_default()
}

/// The canvas rect of a frame: below the header row, the full width.
fn edge_area(buf: &ratatui::buffer::Buffer) -> ratatui::layout::Rect {
    ratatui::layout::Rect::new(0, 1, buf.area.width, buf.area.height - 1)
}

/// The tabs' text as the wide frame draws it: what lies between the title and the right corner
/// besides filler.
fn edge_tabs(buf: &ratatui::buffer::Buffer, title: &str) -> String {
    let area = edge_area(buf);
    let row = top_edge::row_text(buf, area.y, area.x..area.right());
    let inner = row
        .strip_prefix('┌')
        .and_then(|r| r.strip_suffix('┐'))
        .unwrap_or_else(|| panic!("a frame's top edge is required: {row:?}"));
    let after_title = inner
        .strip_prefix(title)
        .unwrap_or_else(|| panic!("the title is at the left of the top edge: {row:?} / {title:?}"));
    after_title.trim_matches('─').to_string()
}

/// The frame under the top edge: `│` at both ends of every row down to a bottom edge `└─…┘`.
fn edge_assert_frame(buf: &ratatui::buffer::Buffer, label: &str) {
    let area = edge_area(buf);
    let last = area.width - 1;
    let bottom = (area.y + 1..buf.area.bottom())
        .find(|y| buf[(0, *y)].symbol() == "└")
        .unwrap_or_else(|| panic!("{label}: no bottom edge"));
    assert!(bottom > area.y + 1, "{label}: the frame has a body");
    for y in area.y + 1..bottom {
        assert_eq!(buf[(0, y)].symbol(), "│", "{label}: left edge row {y}");
        assert_eq!(buf[(last, y)].symbol(), "│", "{label}: right edge row {y}");
    }
    let expected = format!("└{}┘", "─".repeat(usize::from(area.width) - 2));
    assert_eq!(
        top_edge::row_text(buf, bottom, 0..area.width),
        expected,
        "{label}: bottom edge"
    );
}

/// How many body rows of the frame show something. Same inner rows and columns for both sides:
/// rows 2.. below the top edge, inside the two side edges, above the bottom edge and the footer.
fn edge_drawn_rows(buf: &ratatui::buffer::Buffer) -> usize {
    let (width, height) = (buf.area.width, buf.area.height);
    (2..height - 2)
        .filter(|y| (1..width - 1).any(|x| buf[(x, *y)].symbol() != " "))
        .count()
}

/// How many screens a backend test must have checked: all of them, or none when jj is absent
/// (`old_repo` already printed the SKIPPED line). A silent pass over zero screens fails here.
#[track_caller]
fn edge_assert_checked(id: &str, checked: usize, per_screen: usize, screens: usize) {
    if id == "jj" && !crate::vcs::jj::available() {
        assert_eq!(checked, 0);
    } else {
        assert_eq!(checked, per_screen * screens, "{id}: screens actually checked");
    }
}

fn edge_backend(id: &str, screens: &[EdgeScreen]) -> usize {
    let Some(root) = edge_repo(id) else {
        return 0;
    };
    let mut checked = 0;
    for screen in screens {
        for lang in ["en", "ja"] {
            checked += 1;
            let label = format!("{id} {screen:?} {lang}");
            let mut new = edge_new(&root, id, lang, (200, 14), *screen);
            let buf = edge_buffer(&mut new);
            let title = edge_title(&new);
            assert!(!title.trim().is_empty(), "{label}: the list declares a title");
            // The wide frame: title at the top left, tabs at the right end, frame all around.
            let tabs = edge_tabs(&buf, &title);
            assert!(!tabs.trim().is_empty(), "{label}: tabs on the top edge");
            top_edge::check(&buf, edge_area(&buf), &title, &tabs)
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            let area = edge_area(&buf);
            let columns = top_edge::tabs_columns(area, &title, &tabs)
                .unwrap_or_else(|| panic!("{label}: the tabs fit at 200 columns"));
            assert_eq!(
                columns.end,
                area.right() - 1,
                "{label}: the tabs end at the right corner"
            );
            edge_assert_frame(&buf, &label);
            // Narrow: the exact fit keeps the tabs, one column less drops them and keeps the title.
            let need = top_edge::width(&title) + top_edge::width(&tabs) + 2;
            for width in [need + 3, need, need - 1, need - 7, top_edge::width(&title) + 2] {
                let width = width.max(top_edge::width(&title) + 2);
                new.resize(width, 14);
                drain(&mut new);
                let buf = edge_buffer(&mut new);
                let at = format!("{label} at {width} columns");
                top_edge::check(&buf, edge_area(&buf), &title, &tabs)
                    .unwrap_or_else(|e| panic!("{at}: {e}"));
                let row = top_edge::row_text(&buf, edge_area(&buf).y, 0..width);
                assert!(row.contains(&title), "{at}: the title is intact: {row:?}");
                assert_eq!(
                    row.contains(tabs.trim()),
                    width >= need,
                    "{at}: the tabs are drawn exactly when title and tabs fit: {row:?}"
                );
                edge_assert_frame(&buf, &at);
            }
        }
    }
    checked
}

#[test]
fn vcs_project_frame_git_lists_draw_the_frame_title_left_and_tabs_right_until_the_width_is_short()
{
    if isolated("vcs_project_frame_git_lists_draw_the_frame_title_left_and_tabs_right_until_the_width_is_short") {
        return;
    }
    let checked = edge_backend("git", &GIT_EDGE_SCREENS);
    edge_assert_checked("git", checked, 2, GIT_EDGE_SCREENS.len());
}

#[test]
fn vcs_project_frame_jj_lists_draw_the_frame_title_left_and_tabs_right_until_the_width_is_short() {
    if isolated("vcs_project_frame_jj_lists_draw_the_frame_title_left_and_tabs_right_until_the_width_is_short") {
        return;
    }
    let checked = edge_backend("jj", &JJ_EDGE_SCREENS);
    edge_assert_checked("jj", checked, 2, JJ_EDGE_SCREENS.len());
}

/// The terminal sizes of the row-count check, in the order of the expected counts below.
const EDGE_ROW_SIZES: [(u16, u16); 3] = [(120, 10), (60, 8), (160, 14)];

/// How many list rows each project screen draws at each of [`EDGE_ROW_SIZES`] (the counts the
/// released hub drew at the same sizes: the project frame must never show fewer).
const GIT_EDGE_ROWS: [(EdgeScreen, [usize; 3]); 6] = [
    (EdgeScreen::Changes, [5, 4, 5]),
    (EdgeScreen::Log, [6, 4, 9]),
    (EdgeScreen::Graph, [5, 4, 10]),
    (EdgeScreen::Branches, [6, 4, 10]),
    (EdgeScreen::Worktrees, [5, 4, 5]),
    (EdgeScreen::Picker, [6, 4, 10]),
];

const JJ_EDGE_ROWS: [(EdgeScreen, [usize; 3]); 4] = [
    (EdgeScreen::Changes, [6, 4, 8]),
    (EdgeScreen::Log, [6, 4, 10]),
    (EdgeScreen::Graph, [6, 4, 10]),
    (EdgeScreen::Branches, [6, 4, 8]),
];

fn edge_same_rows<const N: usize>(id: &str, expected: &[(EdgeScreen, [usize; 3]); N]) -> usize {
    let Some(root) = edge_repo(id) else {
        return 0;
    };
    let mut checked = 0;
    for (screen, rows) in expected {
        for (size, want) in EDGE_ROW_SIZES.iter().zip(rows) {
            checked += 1;
            let label = format!("{id} {screen:?} at {size:?}");
            let mut new = edge_new(&root, id, "en", *size, *screen);
            let new_buf = edge_buffer(&mut new);
            assert_eq!(
                edge_drawn_rows(&new_buf),
                *want,
                "{label}: list rows drawn, project vs the hub's count"
            );
        }
    }
    checked
}

#[test]
fn vcs_project_frame_git_lists_show_as_many_rows_as_the_old_hub() {
    if isolated("vcs_project_frame_git_lists_show_as_many_rows_as_the_old_hub") {
        return;
    }
    let checked = edge_same_rows("git", &GIT_EDGE_ROWS);
    edge_assert_checked("git", checked, 3, GIT_EDGE_SCREENS.len());
}

#[test]
fn vcs_project_frame_jj_lists_show_as_many_rows_as_the_old_hub() {
    if isolated("vcs_project_frame_jj_lists_show_as_many_rows_as_the_old_hub") {
        return;
    }
    let checked = edge_same_rows("jj", &JJ_EDGE_ROWS);
    edge_assert_checked("jj", checked, 3, JJ_EDGE_SCREENS.len());
}
