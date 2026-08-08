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
    '\u{f0c1}' // nf-fa-link (chain link)
}

/// Icons for Markdown task-list checkboxes. Used only when `ui.icons=true`.
/// Unicode ☐/☑ (U+2610/U+2611) are East-Asian-Neutral = 1 cell to unicode-width, but CJK
/// fallback fonts draw them double-width, clipping the glyph and halving the focus highlight —
/// so like the tree icons we use Nerd Font glyphs (guaranteed 1 cell) instead.
pub fn task_icon(checked: bool) -> char {
    if checked {
        '\u{f046}' // nf-fa-check_square_o
    } else {
        '\u{f096}' // nf-fa-square_o
    }
}

/// Glyph for a repo's current branch (U+2387 ALTERNATIVE KEY SYMBOL, ⎇ — the conventional
/// git-branch icon). Used only when `ui.icons=true`; see `branch_marker`.
pub fn branch_icon() -> char {
    '\u{2387}'
}

/// Glyph for the git graph's pinned base branch (U+2316 POSITION INDICATOR, ⌖). Used only when
/// `ui.icons=true`; see `base_marker`.
pub fn base_icon() -> char {
    '\u{2316}'
}

/// Display marker for a repo's current branch, placed directly before the branch name — the tree
/// root chip, the git changes/log titles, and the graph legend's HEAD entry. `icons=true` →
/// `branch_icon()` (⎇); `icons=false` → the ASCII label `"br:"`.
///
/// Neither Menlo/SF Mono/Monaco/Courier/Andale Mono/Courier New nor HackGen Console NF (measured
/// via a `fontTools` cmap dump) contain U+2387 — macOS always falls back to Apple Symbols, which
/// does, so the glyph never renders as tofu (□) there. But that fallback glyph's advance width
/// (~1.033em) is ~1.7x a Menlo cell's (~0.602em); if the terminal doesn't shrink it to fit the
/// cell, it can spill into the next cell — the same failure mode the ☐/☑ task icons had before
/// v0.6.0. Callers must put a space after the marker (never glue it directly to the next glyph)
/// so an oversized fallback glyph overflows into blank space instead of overlapping content.
pub fn branch_marker(icons: bool) -> &'static str {
    if icons {
        "\u{2387}"
    } else {
        "br:"
    }
}

/// Display marker for the git graph's pinned base branch, placed directly before the branch name
/// in the graph legend's base entry. `icons=true` → `base_icon()` (⌖); `icons=false` → the ASCII
/// label `"base:"`. Same overflow caveat as `branch_marker` — always follow with a space.
pub fn base_marker(icons: bool) -> &'static str {
    if icons {
        "\u{2316}"
    } else {
        "base:"
    }
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
        "lock" => '\u{f023}',                                           // lock (Cargo.lock etc.)
        "txt" | "log" => '\u{f15c}',                                    // text
        _ => '\u{f016}',                                                // default: generic file
    }
}
