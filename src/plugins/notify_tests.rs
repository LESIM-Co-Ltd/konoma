//! OS notification delivery is separate from deterministic discovery and screen tests.

#[test]
fn native_watch_delivers_a_file_created_after_registration() {
    let dir = crate::test_support::unique_tmp("konoma_native_watch");
    std::fs::create_dir_all(&dir).unwrap();
    super::projects::assert_native_watch_delivery(&std::fs::canonicalize(&dir).unwrap());
}
