// 内蔵テキスト読み込み (プレビュー共通基盤)。
//
// 役割:
//   - is_probably_text: 先頭バイトを見てテキスト/バイナリを判定 (NUL バイト有無)。
//   - load: サイズ・行数上限つきでファイルを読み、行に分割して返す。
// 外部依存ゼロ。素の環境でも必ず動く (外部依存が無くても動く、を満たす)。
// 巨大ファイルで固まらないよう上限を設ける。重くなったら後で別スレッド化を検討。

use std::io::Read;
use std::path::Path;

/// Number of bytes read at once for detection and incremental reading.
const SNIFF_BYTES: usize = 8 * 1024;
/// Maximum bytes read for preview (to guard against huge files).
const MAX_BYTES: usize = 1024 * 1024; // 1 MiB
/// Maximum number of lines shown in preview.
const MAX_LINES: usize = 5000;

/// Peek at the start to judge whether it looks like text.
/// If it contains a NUL byte, treat it as binary (the same naive check as less/file etc.).
/// If it cannot be opened, err on the safe side and return false (= treated as binary → can not preview).
pub fn is_probably_text(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; SNIFF_BYTES];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    if n == 0 {
        return true; // 空ファイルはテキスト扱い
    }
    !buf[..n].contains(&0)
}

/// The loaded text content.
pub struct TextContent {
    pub lines: Vec<String>,
    /// Whether it was truncated at a limit (byte count or line count).
    pub truncated: bool,
}

/// Read the file with limits and split it into lines. Non-UTF-8 bytes are converted lossily.
pub fn load(path: &Path) -> std::io::Result<TextContent> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; SNIFF_BYTES];
    let mut truncated = false;
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len() + n > MAX_BYTES {
            let take = MAX_BYTES - buf.len();
            buf.extend_from_slice(&chunk[..take]);
            truncated = true;
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        truncated = true;
    }
    Ok(TextContent { lines, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("konoma_text_test_{name}"));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn detects_text_and_loads_content() {
        let p = tmp("plain.txt", b"hello\nworld\n");
        assert!(is_probably_text(&p));
        let c = load(&p).unwrap();
        assert_eq!(c.lines, vec!["hello".to_string(), "world".to_string()]);
        assert!(!c.truncated);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn detects_binary_via_nul_byte() {
        let p = tmp("blob.bin", &[0x89, 0x50, 0x00, 0x01, 0x02]);
        assert!(!is_probably_text(&p));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn empty_file_is_text() {
        let p = tmp("empty", b"");
        assert!(is_probably_text(&p));
        std::fs::remove_file(&p).ok();
    }
}
