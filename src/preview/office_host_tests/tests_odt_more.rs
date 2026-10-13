//! Moved from `konoma-office-core/src/tests_odt_more.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::{tiny_png, visible};
use super::tests_odt::*;

#[test]
fn a_picture_with_bytes_that_are_not_an_image_still_converts() {
    let o = Ox::new(&p(
        r#"<draw:frame><draw:image xlink:href="Pictures/x.png"/></draw:frame>"#,
    ))
    .part("Pictures/x.png", b"not really a png");
    let d = conv(&o);
    assert_eq!(d.images.len(), 1);
    let _ = tiny_png(0);
    let _ = visible("x");
}
