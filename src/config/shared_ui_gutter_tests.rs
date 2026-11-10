//! The shared reader and the host read the same display switch from the same text.
use super::Config;
use konoma_host_core::shared::SharedConfig;

#[test]
fn shared_ui_git_gutter_matches_host_for_absent_true_and_false() {
    for (text, expected) in [
        ("[ui]\n", true),
        ("[ui]\ngit_gutter = true\n", true),
        ("[ui]\ngit_gutter = false\n", false),
    ] {
        let host: Config = toml::from_str(text).unwrap();
        let shared = SharedConfig::parse(text).unwrap();
        assert_eq!(host.ui.git_gutter, expected, "host git_gutter: {text}");
        assert_eq!(
            shared.ui.git_gutter, host.ui.git_gutter,
            "shared git_gutter matches host: {text}"
        );
    }
}
