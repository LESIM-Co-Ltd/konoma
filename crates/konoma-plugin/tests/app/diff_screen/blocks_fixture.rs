use konoma_vcs_core::git::{DiffLine, DiffLineKind};

pub fn late_diff() -> Vec<DiffLine> {
    (0..12)
        .flat_map(|i| {
            let changed = [0, 4, 8, 10].contains(&i);
            let new = DiffLine {
                kind: if changed {
                    DiffLineKind::Added
                } else {
                    DiffLineKind::Context
                },
                old_no: (!changed).then_some(i + 1),
                new_no: Some(i + 1),
                text: format!("line {i}"),
            };
            let old = changed.then(|| DiffLine {
                kind: DiffLineKind::Removed,
                old_no: Some(i + 1),
                new_no: None,
                text: format!("old {i}"),
            });
            old.into_iter().chain([new])
        })
        .collect()
}

// n n e N N e, starting on block 1. All late blocks clamp to scroll row 4.
pub const STEPS: &[(&str, usize, Option<usize>)] = &[
    ("n", 2, None),
    ("n", 3, None),
    ("e", 3, Some(11)),
    ("N", 2, None),
    ("N", 1, None),
    ("e", 1, Some(5)),
];

// split, viewport, independent block rows
pub const LAYOUTS: &[(bool, usize, &[usize])] =
    &[(false, 12, &[0, 5, 10, 13]), (true, 8, &[0, 4, 8, 10])];
