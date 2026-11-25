use super::*;
use konoma_host_core::shared::SharedConfig;

#[path = "../../crates/konoma-plugin/tests/app/diff_screen/fixture.rs"]
mod fixture;

#[test]
fn file_diff_key_sequences_match_independent_literals() {
    for scheme in [KeyScheme::Vim, KeyScheme::Less] {
        for case in fixture::KEY_CASES {
            let config = SharedConfig::parse(case.config).unwrap();
            let keys = KeyMap::from_config(scheme, &config.keys.to_keymap_config());
            for &(key, expected) in case.probes {
                let key = KeyPress::parse(key).unwrap();
                let actual = match keys.resolve(Context::Diff, None, key) {
                    Resolution::Action(Action::CycleDiffLayout) => Some("layout"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::CycleDiffView) => Some("view"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::PageNext) => Some("page-next"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::PagePrev) => Some("page-prev"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::DiffNextChange) => Some("block-next"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::DiffPrevChange) => Some("block-prev"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::JumpNextChange) => Some("file-next"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::JumpPrevChange) => Some("file-prev"),
                    #[cfg(feature = "git")]
                    Resolution::Action(Action::GitLaunchTool) => Some("tool"),
                    Resolution::Action(Action::CycleRepresentation) => Some("representation"),
                    Resolution::Action(Action::Quit) => Some("quit"),
                    Resolution::Unbound => None,
                    other => panic!("unexpected {other:?}"),
                };
                assert_eq!(actual, expected, "{scheme:?} {key:?}: {}", case.config);
            }
        }
        let config = SharedConfig::parse(fixture::LEGACY_MENU_CASE.0).unwrap();
        let keys = KeyMap::from_config(scheme, &config.keys.to_keymap_config());
        let menu = keys.menu(Context::Diff, LeaderId::Copy).unwrap();
        assert_eq!(
            menu.items
                .iter()
                .find(|item| item.key == KeyPress::ch('x'))
                .map(|item| action_name(item.action))
                .as_deref(),
            Some(fixture::LEGACY_MENU_CASE.1),
            "legacy menu overrides precede global overrides"
        );
    }
}
