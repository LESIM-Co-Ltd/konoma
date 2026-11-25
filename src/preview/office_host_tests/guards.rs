//! Watchman for the place where the Office core repeats something the host owns: it does not know
//! the host's table row cap, so it carries its own copy of the number. If the host changes it,
//! this fails. (The slide words the core's tests spell out are guarded by the `konoma-office`
//! plugin, which now hands them in.)

use konoma_office_core::container::{Limits, DEFAULT_MAX_ROWS};

#[test]
fn the_cores_default_row_cap_is_the_table_previews() {
    assert_eq!(DEFAULT_MAX_ROWS, crate::preview::table::MAX_ROWS);
    assert_eq!(Limits::default().max_rows, crate::preview::table::MAX_ROWS);
}
