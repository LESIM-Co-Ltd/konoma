//! Nerd Font icon mapping for the leading column of tree rows.
//!
//! Design (important):
//! - Icons are **mostly monochrome = they inherit the terminal's default foreground**. No color is set here
//!   (`ui/tree.rs` sets no Style = it automatically follows the user's theme color).
//! - In the future, color is applied semantically only to entries with a **git status (FR-7)**. No decorative per-extension colors.
//! - On terminals without a Nerd Font, `ui.icons = false` falls back to plain symbols
//!   (branched on the caller side; if no icon is emitted, no tofu (□) appears either).
//!
//! Code points have been verified by rendering with the actual Symbols Nerd Font (standard devicons/seti positions).

use std::path::Path;

/// Icon for Markdown links (placed before the link label). Used only when `ui.icons=true`.
pub fn link_icon() -> char {
    '\u{f0c1}' // nf-fa-link (鎖リンク)
}

/// Icon for directories. Distinguished by open/closed state.
pub fn dir_icon(expanded: bool) -> char {
    if expanded {
        '\u{f07c}' // nf-fa-folder_open
    } else {
        '\u{f07b}' // nf-fa-folder
    }
}

/// Icon for files. Resolved in order: special file name → extension → default (generic file).
pub fn file_icon(path: &Path) -> char {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name.to_ascii_lowercase().as_str() {
        ".gitignore" | ".gitattributes" | ".gitmodules" => return '\u{e702}', // git
        "license" | "license.md" | "license.txt" | "copying" => return '\u{f0f6}', // doc
        _ => {}
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => '\u{e7a8}',                                             // rust
        "md" | "markdown" | "mmd" | "mermaid" => '\u{e73e}',            // markdown
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" => '\u{e615}', // config gear
        "json" => '\u{e60b}',                                           // json
        "js" | "mjs" | "cjs" => '\u{e74e}',                             // javascript
        "ts" | "tsx" => '\u{e628}',                                     // typescript
        "py" => '\u{e73c}',                                             // python
        "go" => '\u{e627}',                                             // go
        "c" | "h" => '\u{e61e}',                                        // c
        "cpp" | "cc" | "cxx" | "hpp" => '\u{e61d}',                     // c++
        "sh" | "bash" | "zsh" | "fish" => '\u{f489}',                   // shell
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => '\u{f1c5}', // image
        "mp4" | "mov" | "mkv" | "webm" | "avi" => '\u{f03d}',           // video
        "lock" => '\u{f023}',                                           // lock (Cargo.lock 等)
        "txt" | "log" => '\u{f15c}',                                    // text
        _ => '\u{f016}',                                                // 既定: 汎用ファイル
    }
}
