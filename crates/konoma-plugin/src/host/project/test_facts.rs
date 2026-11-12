//! Facts as a host passes them with no config. `ProjectPresentationFacts::default()` has empty
//! motion keys, and an empty list hides its control (a hint is shown iff its key acts), so a test
//! that expects the canvas's own `j/k`, `q`, `/` and `?` must start from these instead.

use super::presentation::{ProjectMotionKeys, ProjectPresentationFacts};
use konoma_host_core::keymap::KeyPress;

/// The scroll, back, filter and help keys of the default keymap. The other groups (horizontal
/// scroll, ends, half page, page, line ends, diff layout) stay empty: each test sets the ones it
/// shows.
pub(super) fn default_keymap_motions() -> ProjectMotionKeys {
    ProjectMotionKeys {
        scroll: vec![KeyPress::ch('j'), KeyPress::ch('k')],
        back: vec![KeyPress::ch('q')],
        filter: vec![KeyPress::ch('/')],
        help: vec![KeyPress::ch('?')],
        ..Default::default()
    }
}

/// Default facts carrying [`default_keymap_motions`].
pub(super) fn default_keymap_facts() -> ProjectPresentationFacts {
    ProjectPresentationFacts {
        motions: default_keymap_motions(),
        ..Default::default()
    }
}
