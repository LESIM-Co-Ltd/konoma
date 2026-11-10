//! Optional gutter inputs leave ordinary file apps and converted documents unchanged.
use super::document::{lay_out, lay_out_with_gutter, DocCache};
use super::Display;
use crate::host::project::md_diff::MdDiffOutcome;
use crate::host::project::preview_gutter::FilePreviewGutter;
use crate::input::Origin;
use crate::view::DocumentView;
use konoma_host_core::lang::Lang;
use konoma_widgets::markdown::{BlockOp, PreviewMark};
use std::sync::Arc;

fn document(origin: Origin) -> DocumentView {
    DocumentView {
        markdown: "# Title\n\nparagraph\n".into(),
        origin,
        resources: Default::default(),
        options: Default::default(),
        title: None,
        truncated: false,
    }
}

#[test]
fn landed_ops_project_rendered_rows_without_aligning_again() {
    let outcome = MdDiffOutcome::Ready {
        old_pre: String::new(),
        new_pre: String::new(),
        ops: vec![
            BlockOp::Insert { new: 0 },
            BlockOp::Replace { old: 0, new: 1 },
            BlockOp::Delete { old: 1 },
            BlockOp::Equal { new: 2 },
        ],
        any_change: true,
        front_matter_only: false,
    };
    let gutter = FilePreviewGutter::from_outcome(Some(&outcome)).unwrap();
    assert!(gutter.has_changes());
    assert_eq!(
        gutter.project(&[0..2, 2..6, 6..9]),
        vec![
            (0..2, PreviewMark::Added),
            (2..6, PreviewMark::Modified),
            (6..7, PreviewMark::Deleted),
        ]
    );
    for outcome in [
        None,
        Some(&MdDiffOutcome::NoBaseline),
        Some(&MdDiffOutcome::Unavailable),
    ] {
        assert!(FilePreviewGutter::from_outcome(outcome).is_none());
    }
}

#[test]
fn document_cache_tracks_gutter_arrival_changes_and_removal() {
    let dv = document(Origin::File("doc.md".into()));
    let display = Display::default();
    let mut slot = None;
    let plain = DocCache::get(&mut slot, &dv, 30, 8, Lang::En, None, &display);
    let ops = [BlockOp::Equal { new: 0 }, BlockOp::Insert { new: 1 }];
    let get = |slot: &mut _, ops: Option<&[BlockOp]>| {
        DocCache::get_with_gutter(
            slot,
            &dv,
            30,
            8,
            Lang::En,
            None,
            &display,
            ops.map(FilePreviewGutter::new),
        )
    };
    let added = get(&mut slot, Some(&ops[..]));
    assert!(!Arc::ptr_eq(&plain, &added));
    assert!(added.lines.iter().any(|l| l.to_string().starts_with('▌')));
    assert!(Arc::ptr_eq(&added, &get(&mut slot, Some(&ops[..]))));
    let changed = [
        BlockOp::Equal { new: 0 },
        BlockOp::Replace { old: 1, new: 1 },
    ];
    let modified = get(&mut slot, Some(&changed[..]));
    assert!(!Arc::ptr_eq(&added, &modified));
    assert_ne!(added.lines, modified.lines);
    let gone = get(&mut slot, None);
    assert_eq!(plain.lines, gone.lines);
    assert_eq!(plain.row_prefix, gone.row_prefix);
    assert_eq!(plain.max_line_cols, gone.max_line_cols);
}

#[test]
fn converted_documents_and_standalone_mermaid_ignore_file_gutters() {
    let ops = [BlockOp::Insert { new: 0 }, BlockOp::Insert { new: 1 }];
    for origin in [
        Origin::Converted {
            from: "doc.docx".into(),
        },
        Origin::File("diagram.mmd".into()),
        Origin::File("diagram.mermaid".into()),
        Origin::File("diagram.MMD".into()),
    ] {
        let dv = document(origin);
        let display = Display::default();
        let plain = lay_out(&dv, 30, 8, Lang::En, None, &display);
        let gutter = lay_out_with_gutter(
            &dv,
            30,
            8,
            Lang::En,
            None,
            &display,
            Some(FilePreviewGutter::new(&ops)),
        );
        assert_eq!(plain.lines, gutter.lines);
        assert_eq!(plain.row_prefix, gutter.row_prefix);
        assert_eq!(plain.max_line_cols, gutter.max_line_cols);
    }
}
