use super::fixture::{KEY_CASES, LEGACY_MENU_CASE};
use konoma_host_core::keymap::{scheme_from_str, CoreContext, CoreKeys, HostOp, KeyPress};
use konoma_host_core::shared::SharedConfig;
use konoma_plugin::host::project::{diff_keys::file_diff_keys, diff_presentation::DiffUiOp};

#[test]
fn file_diff_key_sequences_resolve_like_the_host() {
    for scheme in ["vim", "less"] {
        for case in KEY_CASES {
            let config = SharedConfig::parse(case.config).unwrap();
            let keys = file_diff_keys(scheme_from_str(scheme), &config.keys);
            for &(key, expected) in case.probes {
                let key = KeyPress::parse(key).unwrap();
                let actual = match DiffUiOp::resolve_file_diff(&keys, key) {
                    Some(DiffUiOp::View) => Some("view"),
                    Some(DiffUiOp::Layout) => Some("layout"),
                    Some(DiffUiOp::PageNext) => Some("page-next"),
                    Some(DiffUiOp::PagePrev) => Some("page-prev"),
                    Some(DiffUiOp::NextBlock) => Some("block-next"),
                    Some(DiffUiOp::PrevBlock) => Some("block-prev"),
                    Some(DiffUiOp::NextFile) => Some("file-next"),
                    Some(DiffUiOp::PrevFile) => Some("file-prev"),
                    Some(DiffUiOp::Tool) => Some("tool"),
                    Some(DiffUiOp::Global(2)) => Some("quit"),
                    None if keys.host_op(CoreContext::Diff, key)
                        == Some(HostOp::CycleRepresentation) =>
                    {
                        Some("representation")
                    }
                    None => None,
                    other => panic!("unexpected {other:?}"),
                };
                assert_eq!(actual, expected, "{scheme} {key:?}: {}", case.config);
            }
        }
        let config = SharedConfig::parse(LEGACY_MENU_CASE.0).unwrap();
        let keys = file_diff_keys(scheme_from_str(scheme), &config.keys);
        let menu = keys
            .menu(CoreContext::Diff, konoma_host_core::keymap::CoreMenu::Copy)
            .unwrap();
        assert_eq!(
            menu.items
                .iter()
                .find(|item| item.key == KeyPress::ch('x'))
                .map(|item| item.action.name())
                .as_deref(),
            Some(LEGACY_MENU_CASE.1),
            "legacy menu overrides precede global overrides"
        );
        // Opting in must not change the keys or config interpretation of ordinary file apps.
        let config = SharedConfig::parse("[keys.diff]\nj = 'diff_next_change'\n").unwrap();
        let ordinary = CoreKeys::from_keys_config(scheme_from_str(scheme), &config.keys);
        assert_eq!(ordinary.host_op(CoreContext::Diff, KeyPress::ch('R')), None);
        assert_eq!(ordinary.host_op(CoreContext::Diff, KeyPress::ch('n')), None);
        assert_eq!(ordinary.host_op(CoreContext::Diff, KeyPress::ch('!')), None);
        assert_eq!(
            ordinary.host_op(CoreContext::Diff, KeyPress::ctrl_ch('n')),
            None
        );
        assert_eq!(
            ordinary.resolve(CoreContext::Diff, KeyPress::ch('j')),
            konoma_host_core::keymap::CoreResolution::Action(
                konoma_host_core::keymap::CoreAction::Navigate(
                    konoma_host_core::keymap::Motion::Down
                )
            )
        );
    }
}

#[test]
fn late_blocks_keep_distinct_identity_and_editor_lines() {
    use super::blocks_fixture::{late_diff, LAYOUTS, STEPS};
    use konoma_plugin::host::project::diff_blocks::{
        source_diff_blocks, source_diff_edit_line, step_change_block, DiffBlockCursor,
    };
    let config = SharedConfig::parse("").unwrap();
    let keys = file_diff_keys(scheme_from_str("vim"), &config.keys);
    let lines = late_diff();
    for &(split, viewport, rows) in LAYOUTS {
        let blocks = source_diff_blocks(&lines, "txt", split, viewport);
        assert_eq!(blocks.rows, rows);
        assert_eq!(blocks.max_v, 4);
        let mut cursor = DiffBlockCursor {
            scroll: rows[1].min(4),
            block: Some(1),
        };
        for &(key, block, editor_line) in STEPS {
            let op = DiffUiOp::resolve_file_diff(&keys, KeyPress::parse(key).unwrap());
            match op {
                Some(DiffUiOp::NextBlock) => {
                    cursor = step_change_block(&blocks, cursor, 1).unwrap()
                }
                Some(DiffUiOp::PrevBlock) => {
                    cursor = step_change_block(&blocks, cursor, -1).unwrap()
                }
                Some(DiffUiOp::Edit) => {
                    assert_eq!(source_diff_edit_line(&lines, &blocks, cursor), editor_line)
                }
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(cursor.block, Some(block), "{split} {key}: block identity");
            assert_eq!(cursor.scroll, 4, "{split} {key}: clamped scroll");
        }
    }
}
