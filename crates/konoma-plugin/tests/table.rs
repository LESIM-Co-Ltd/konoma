//! The table view's defaults and the work-group declaration.

use konoma_plugin::view::Grid;
use konoma_plugin::Manifest;

/// A grid that makes its cell text on demand (no borrowed strings).
struct Made;

impl Grid for Made {
    fn nrows(&self) -> usize {
        2
    }
    fn ncols(&self) -> usize {
        3
    }
    fn header(&self, _col: usize) -> Option<String> {
        None
    }
    fn cell(&self, row: usize, col: usize) -> String {
        format!("r{row}c{col}")
    }
}

/// A grid that keeps its cells as text.
struct Kept(Vec<Vec<String>>);

impl Grid for Kept {
    fn nrows(&self) -> usize {
        self.0.len()
    }
    fn ncols(&self) -> usize {
        self.0.iter().map(Vec::len).max().unwrap_or(0)
    }
    fn header(&self, _col: usize) -> Option<String> {
        None
    }
    fn cell(&self, row: usize, col: usize) -> String {
        self.cell_str(row, col).unwrap_or_default().to_string()
    }
    fn cell_str(&self, row: usize, col: usize) -> Option<&str> {
        Some(
            self.0
                .get(row)
                .and_then(|r| r.get(col))
                .map_or("", String::as_str),
        )
    }
}

#[test]
fn the_default_visit_row_gives_every_column_in_order() {
    let mut seen = Vec::new();
    Made.visit_row(1, &mut |c, t| seen.push((c, t.to_string())));
    assert_eq!(
        seen,
        [
            (0, "r1c0".to_string()),
            (1, "r1c1".to_string()),
            (2, "r1c2".to_string())
        ]
    );
}

#[test]
fn the_default_visit_row_borrows_the_cells_of_a_grid_that_keeps_text() {
    let g = Kept(vec![vec!["a".into(), "b".into()], vec!["c".into()]]);
    let mut seen = Vec::new();
    g.visit_row(1, &mut |c, t| seen.push((c, t.to_string())));
    // A ragged row is padded to the table's width, like the cells the renderer draws.
    assert_eq!(seen, [(0, "c".to_string()), (1, String::new())]);
}

#[test]
fn a_grid_has_no_detail_and_every_column_in_a_row_by_default() {
    assert!(Made.detail(0, 0).is_none());
    assert_eq!(Made.row_len(0), 3);
    assert!(!Made.truncated());
}

#[test]
fn a_manifest_has_no_work_group_unless_it_names_one() {
    let plain = Manifest::new("p", "0");
    assert_eq!(plain.work_group, None);
    let heavy = Manifest::new("h", "0").work_group("office");
    assert_eq!(heavy.work_group.as_deref(), Some("office"));
}
