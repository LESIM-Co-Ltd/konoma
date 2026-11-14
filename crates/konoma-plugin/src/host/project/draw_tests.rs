//! The screen tabs on the first row show the key that jumps now.

use std::collections::BTreeSet;

use konoma_host_core::project::DiffLayout;
use ratatui::{buffer::Buffer, layout::Rect};

use super::draw::{draw_project, ProjectDrawValues};
use crate::host::bind::{bind_jumps, JumpBinding};
use crate::project::{ProjectDecl, ProjectScreen};
use crate::Manifest;

fn declaration() -> ProjectDecl {
    ProjectDecl::new()
        .screen(ProjectScreen::new("changes", "Changes"))
        .screen(ProjectScreen::new("branches", "Branches").key('b'))
        .screen(ProjectScreen::new("worktrees", "Worktrees").key('w'))
        // A pushed screen is not a tab.
        .screen(ProjectScreen::new("detail", "Detail").key('z').pushed())
}

fn jumps(user: &[(&str, &str)]) -> Vec<JumpBinding> {
    let manifest = Manifest::new("vcs", "1").project(declaration());
    let table = user
        .iter()
        .map(|(k, a)| (k.to_string(), a.to_string()))
        .collect();
    bind_jumps("vcs", &manifest, (!user.is_empty()).then_some(&table)).0
}

/// The tab row (row 0) as text.
fn tab_row(jumps: &[JumpBinding], lang: &str) -> String {
    let decl = declaration();
    let area = Rect::new(0, 0, 60, 6);
    let mut buf = Buffer::empty(area);
    let marked = BTreeSet::new();
    draw_project(
        &mut buf,
        area,
        ProjectDrawValues {
            declaration: &decl,
            active_screen: "changes",
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            marked: &marked,
            content: None,
            visible_rows: &[],
            layout: DiffLayout::Unified,
            lang,
            jumps,
            unavailable: "",
            query: "",
        },
    );
    (0..area.width)
        .map(|x| buf[(x, 0)].symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn the_default_tab_bar_shows_the_declared_keys() {
    assert_eq!(
        tab_row(&jumps(&[]), "en"),
        " Changes  b:Branches  w:Worktrees"
    );
}

#[test]
fn a_rebound_key_is_the_one_the_tab_shows() {
    let row = tab_row(&jumps(&[("B", "goto_branches")]), "en");
    assert_eq!(row, " Changes  B:Branches  w:Worktrees");
    assert!(!row.contains("b:Branches"), "{row}");
}

#[test]
fn a_removed_jump_shows_its_tab_without_a_key() {
    let row = tab_row(&jumps(&[("b", "noop")]), "en");
    assert_eq!(row, " Changes  Branches  w:Worktrees");
    let row = tab_row(&jumps(&[("b", "noop"), ("w", "noop")]), "en");
    assert_eq!(row, " Changes  Branches  Worktrees");
}

#[test]
fn a_swap_shows_each_screen_with_the_other_key() {
    let row = tab_row(
        &jumps(&[("w", "goto_branches"), ("b", "goto_worktrees")]),
        "en",
    );
    assert_eq!(row, " Changes  w:Branches  b:Worktrees");
}

#[test]
fn no_jump_bindings_at_all_show_no_keys() {
    // The tab bar reads the effective keys only: the declared ones are not looked at.
    assert_eq!(tab_row(&[], "en"), " Changes  Branches  Worktrees");
}

// The outer frame of a list: title at the left of the top edge, tabs at the right end.
mod framed {
    use super::*;
    use crate::host::project::draw::{project_body_area, unframed_body_area};
    use crate::host::project::ProjectContent;
    use crate::style::Line as SdkLine;
    use crate::view::{ListHeading, ListRow, ListView};

    const TABS_WIDTH: u16 = 34; // " Changes " + " b:Branches " + " w:Worktrees "

    fn rows(list: ListView, query: &str, width: u16) -> Vec<String> {
        let decl = declaration();
        let area = Rect::new(0, 0, width, 8);
        let mut buf = Buffer::empty(area);
        let marked = BTreeSet::new();
        let visible: Vec<usize> = (0..list.rows.len()).collect();
        let jumps = jumps(&[]);
        let content = ProjectContent::List(list);
        draw_project(
            &mut buf,
            area,
            ProjectDrawValues {
                declaration: &decl,
                active_screen: "changes",
                cursor: 0,
                scroll: 0,
                hscroll: 0,
                marked: &marked,
                content: Some(&content),
                visible_rows: &visible,
                layout: DiffLayout::Unified,
                lang: "en",
                jumps: &jumps,
                unavailable: "",
                query,
            },
        );
        (0..area.height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn items(n: usize) -> Vec<ListRow> {
        (0..n)
            .map(|i| ListRow::new("item", vec![SdkLine::raw(format!("row {i}"))]))
            .collect()
    }

    fn titled(title: &str, n: usize) -> ListView {
        ListView {
            rows: items(n),
            title: Some(SdkLine::raw(title)),
            ..Default::default()
        }
    }

    #[test]
    fn the_title_is_at_the_left_and_the_tabs_at_the_right_of_the_top_edge() {
        let r = rows(titled(" Plain ", 2), "", 60);
        assert!(r[0].starts_with("┌ Plain ─"), "{}", r[0]);
        assert!(r[0].ends_with(" w:Worktrees ┐"), "{}", r[0]);
        assert!(r[0].contains(" Changes "), "{}", r[0]);
        assert!(r[1].starts_with("│row 0"), "{}", r[1]);
        assert!(r[1].ends_with('│'), "{}", r[1]);
        assert!(r[7].starts_with('└') && r[7].ends_with('┘'), "{}", r[7]);
        assert!(r[7].chars().skip(1).take(58).all(|c| c == '─'), "{}", r[7]);
    }

    #[test]
    fn a_plain_title_line_is_drawn_exactly_as_given_with_no_count() {
        // A plugin written before the heading existed sets only `title`.
        let r = rows(titled(" Mine (9) ", 3), "ab", 60);
        assert!(r[0].starts_with("┌ Mine (9) ─"), "{}", r[0]);
        assert!(!r[0].contains("/ab"), "{}", r[0]);
    }

    #[test]
    fn the_tabs_fit_at_exactly_the_width_they_need_and_drop_one_column_before() {
        let title = " T "; // 3 columns
        let fits = title.len() as u16 + TABS_WIDTH + 2;
        let r = rows(titled(title, 1), "", fits);
        assert!(r[0].starts_with("┌ T "), "{}", r[0]);
        assert!(r[0].ends_with(" w:Worktrees ┐"), "{}", r[0]);
        assert_eq!(r[0].chars().count(), usize::from(fits));
        let r = rows(titled(title, 1), "", fits - 1);
        assert!(r[0].starts_with("┌ T ─"), "the title stays: {}", r[0]);
        assert!(
            !r[0].contains("Changes") && !r[0].contains("Worktrees"),
            "the tabs are what drops, not the title: {}",
            r[0]
        );
        assert!(r[0].ends_with('┐'), "{}", r[0]);
    }

    #[test]
    fn a_frame_narrower_than_the_title_does_not_panic() {
        for width in [2, 3, 5, 8] {
            let r = rows(titled(" A long title ", 2), "", width);
            assert_eq!(r.len(), 8);
            assert!(r
                .iter()
                .all(|row| row.chars().count() == usize::from(width)));
        }
    }

    #[test]
    fn the_heading_fills_in_the_query_and_the_count_between_the_plugins_words() {
        let heading = ListHeading::new(SdkLine::raw(" Changes")).after(SdkLine::raw(" "));
        let mut rows_ = items(3);
        rows_[2] = rows_[2].clone().filter_fallback(true); // a message row: not selectable
        let list = ListView {
            rows: rows_,
            heading: Some(heading),
            ..Default::default()
        };
        let none = rows(list.clone(), "", 60);
        assert!(none[0].starts_with("┌ Changes  (2) ─"), "{}", none[0]);
        let with = rows(list, "ab", 60);
        assert!(with[0].starts_with("┌ Changes /ab  (2) ─"), "{}", with[0]);
    }

    #[test]
    fn the_counted_flag_decides_what_the_heading_counts() {
        let heading = || ListHeading::new(SdkLine::raw(" H"));
        let mut a = items(3);
        a[2] = a[2].clone().filter_fallback(true).counted(true); // unselectable but counts
        let list = ListView {
            rows: a,
            heading: Some(heading()),
            ..Default::default()
        };
        assert!(
            rows(list, "", 60)[0].starts_with("┌ H  (3)─"),
            "unselectable counted"
        );
        let mut b = items(3);
        b[0] = b[0].clone().counted(false); // selectable but not an item
        let list = ListView {
            rows: b,
            heading: Some(heading()),
            ..Default::default()
        };
        assert!(
            rows(list, "", 60)[0].starts_with("┌ H  (2)─"),
            "selectable uncounted"
        );
        let list = ListView {
            rows: items(3),
            heading: Some(heading()),
            ..Default::default()
        };
        assert!(
            rows(list, "", 60)[0].starts_with("┌ H  (3)─"),
            "default follows selectable"
        );
    }

    #[test]
    fn the_frame_leaves_the_old_hubs_body_size() {
        for (w, h) in [(120, 30), (54, 12), (9, 4), (3, 3), (80, 2)] {
            let area = Rect::new(0, 1, w, h);
            let (framed, old) = (project_body_area(area), unframed_body_area(area));
            assert_eq!(
                (framed.width, framed.height),
                (old.width, old.height),
                "{w}x{h}"
            );
        }
    }
}
