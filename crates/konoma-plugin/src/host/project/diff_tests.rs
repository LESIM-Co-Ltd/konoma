use crate::host::project::ProjectDiff;
use std::sync::Arc;

#[test]
fn project_diff_cache_keys_each_viewport_component() {
    let diff = ProjectDiff::new(
        Vec::new(),
        vec!["header".into()],
        String::new(),
        "empty".into(),
    );
    let base = (80, true, 0, 0, 5);
    for changed in [
        (81, true, 0, 0, 5),
        (80, false, 0, 0, 5),
        (80, true, 1, 0, 5),
        (80, true, 0, 1, 5),
        (80, true, 0, 0, 6),
    ] {
        let first = diff.layout(base.0, base.1, base.2, base.3, base.4);
        let hit = diff.layout(base.0, base.1, base.2, base.3, base.4);
        assert!(Arc::ptr_eq(&first, &hit));
        let miss = diff.layout(changed.0, changed.1, changed.2, changed.3, changed.4);
        assert!(
            !Arc::ptr_eq(&first, &miss),
            "key {changed:?} must recompute"
        );
    }
    let stacked = diff.layout(80, false, 0, 0, 5);
    assert!(Arc::ptr_eq(&stacked, &diff.layout(80, false, 8, 0, 5)));
}
