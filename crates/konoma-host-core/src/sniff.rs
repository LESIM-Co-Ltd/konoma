//! The MIME type of a file read from its content (not its name): the one judgment that konoma and
//! the standalone plugin programs share, so a plugin bound by `Bind::mime("image/*")` is picked
//! the same way in both.

use std::path::Path;

/// The MIME type of the file at `path`, read from its leading bytes (`None` when the content
/// matches no known signature, or the file cannot be read).
pub fn mime_of_path(path: &Path) -> Option<String> {
    infer::get_from_path(path)
        .ok()
        .flatten()
        .map(|k| k.mime_type().to_string())
}

/// The MIME type of `bytes` already in memory (the same judgment as [`mime_of_path`]).
pub fn mime_of_bytes(bytes: &[u8]) -> Option<String> {
    infer::get(bytes).map(|k| k.mime_type().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_test_support::unique_tmp;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];

    #[test]
    fn bytes_of_a_png_are_image_png() {
        assert_eq!(mime_of_bytes(PNG).as_deref(), Some("image/png"));
    }

    #[test]
    fn text_and_empty_bytes_have_no_mime() {
        assert_eq!(mime_of_bytes(b"hello\n"), None);
        assert_eq!(mime_of_bytes(b""), None);
    }

    #[test]
    fn the_path_version_ignores_the_name_and_matches_the_bytes_version() {
        let d = unique_tmp("sniff_path");
        std::fs::create_dir_all(d.as_path()).unwrap();
        for name in ["noext", "wrong.txt", "right.png"] {
            let p = d.as_path().join(name);
            std::fs::write(&p, PNG).unwrap();
            assert_eq!(mime_of_path(&p).as_deref(), Some("image/png"), "{name}");
            assert_eq!(mime_of_path(&p), mime_of_bytes(PNG), "{name}");
        }
    }

    #[test]
    fn an_empty_text_or_missing_file_has_no_mime() {
        let d = unique_tmp("sniff_none");
        std::fs::create_dir_all(d.as_path()).unwrap();
        let e = d.as_path().join("empty");
        std::fs::write(&e, b"").unwrap();
        let t = d.as_path().join("t.txt");
        std::fs::write(&t, b"words\n").unwrap();
        assert_eq!(mime_of_path(&e), None);
        assert_eq!(mime_of_path(&t), None);
        assert_eq!(mime_of_path(&d.as_path().join("missing")), None);
    }
}
