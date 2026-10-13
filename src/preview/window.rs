// Windowed file reading lives in the `konoma-widgets` crate; this path stays for the host.
// What stays here is the one test that measures allocations with the host's counting allocator
// (`mem_tests`), which exists only in the host's test binary.

pub use konoma_widgets::window::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> crate::test_support::TmpDir {
        let p = unique_tmp(&format!("konoma_window_{name}"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn read_lines_allocation_does_not_scale_with_line_length() {
        // Tight, module-local companion to `mem_tests::windowed_read_is_bounded_per_line`
        // (which owns the scaling check): a single call must stay within a small multiple of the
        // cap no matter how long the line is. Uncapped this allocated ~5.2x the line length.
        let mut body = vec![b'x'; MAX_LINE_BYTES * 64];
        body.push(b'\n');
        body.extend_from_slice(b"tail\n");
        let p = tmp("cap_alloc", &body);
        let mut w = FileWindow::open(&p).unwrap();
        let used = crate::mem_tests::allocated_by(|| {
            let lines = w.read_lines(0, 50).unwrap();
            std::hint::black_box(&lines);
        });
        assert!(
            used < (MAX_LINE_BYTES * 8) as u64,
            "read_lines の確保が行長に比例している: {used} バイト(上限 {MAX_LINE_BYTES} の8倍未満のはず)"
        );
        std::fs::remove_file(&p).ok();
    }
}
