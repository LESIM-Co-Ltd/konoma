use super::*;

#[path = "../../crates/konoma-plugin/tests/app/diff_screen/blocks_fixture.rs"]
mod fixture;

#[test]
fn late_blocks_match_identity_and_editor_line_literals() {
    use crate::keymap::{Action, Context, KeyMap, KeyPress, KeyScheme, Resolution};
    let dir = crate::test_support::unique_tmp("file_diff_blocks_bridge");
    std::fs::create_dir_all(&dir).unwrap();
    let keys = KeyMap::defaults(KeyScheme::Vim);
    for &(split, viewport, rows) in fixture::LAYOUTS {
        let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
        let path = dir.join("diff.txt");
        app.tab.preview_path = Some(path.clone());
        app.tab.preview_kind = Some(PreviewKind::GitDiff(path.clone()));
        app.tab.diff_view = DiffView::Source;
        app.diff_cache = Some(DiffCache {
            path: path.clone(),
            lines: fixture::late_diff(),
        });
        app.diff_split_drawn = split;
        app.tab.preview_viewport = viewport as u16;
        app.tab.preview_scroll = rows[1].min(4) as u16;
        app.change_at = Some((path, 1));
        assert_eq!(app.diff_blocks().rows, rows);
        assert_eq!(app.diff_blocks().max_v, 4);
        for &(key, block, editor_line) in fixture::STEPS {
            match keys.resolve(Context::Diff, None, KeyPress::parse(key).unwrap()) {
                Resolution::Action(Action::DiffNextChange) => app.diff_change_jump(1),
                Resolution::Action(Action::DiffPrevChange) => app.diff_change_jump(-1),
                Resolution::Action(Action::RequestEdit) => {
                    assert_eq!(app.diff_edit_line(), editor_line)
                }
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(
                app.change_at.as_ref().map(|c| c.1),
                Some(block),
                "{split} {key}: block identity"
            );
            assert_eq!(app.tab.preview_scroll, 4, "{split} {key}: clamped scroll");
        }
    }
}
