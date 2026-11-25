//! Independent host-resolution literals, consumed by both hosts' tests.
pub struct KeyCase {
    pub config: &'static str,
    pub probes: &'static [(&'static str, Option<&'static str>)],
}

pub const LEGACY_MENU_CASE: (&str, &str) = (
    "[keys.preview_git_diff]\n'y x' = 'page_next'\n[keys.global]\n'y x' = 'page_prev'\n",
    "page_prev",
);

pub const KEY_CASES: &[KeyCase] = &[
    KeyCase {
        config: "",
        probes: &[
            ("R", Some("view")),
            ("s", Some("layout")),
            ("J", Some("page-next")),
            ("K", Some("page-prev")),
            ("n", Some("block-next")),
            ("N", Some("block-prev")),
            ("ctrl-n", Some("file-next")),
            ("ctrl-p", Some("file-prev")),
            ("!", Some("tool")),
        ],
    },
    KeyCase {
        config: "[keys.diff]\nn = 'page_next'\nz = 'diff_next_change'\nR = 'cycle_representation'\n",
        probes: &[
            ("n", Some("page-next")),
            ("z", Some("block-next")),
            ("R", Some("representation")),
        ],
    },
    KeyCase {
        config: "[keys.diff]\nn = 'noop'\nR = 'noop'\n'!' = 'disabled'\nctrl-n = 'noop'\n",
        probes: &[("n", None), ("R", None), ("!", None), ("ctrl-n", None)],
    },
    KeyCase {
        config: "[keys.global]\nn = 'quit'\nX = 'quit'\n[keys.diff]\nn = 'page_next'\nX = 'diff_next_change'\nQ = 'diff_prev_change'\n",
        probes: &[
            ("n", Some("block-next")),
            ("X", Some("quit")),
            ("Q", Some("quit")),
        ],
    },
    KeyCase {
        config: "[keys.preview_git_diff]\nn = 'noop'\nR = 'noop'\n[keys.diff]\nn = 'diff_prev_change'\n",
        probes: &[("n", Some("block-prev")), ("R", None)],
    },
];
