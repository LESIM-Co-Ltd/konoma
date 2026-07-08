//! Paste-to-jump: read a path or a GitHub link from the clipboard and navigate to it.
//!
//! Motivation: when someone hands you a GitHub link (or an editor `path:line`), navigating there by
//! hand in the tree is tedious. `P` reads the clipboard, understands a local path or a GitHub
//! blob/raw URL (with a `#L123` / `:123` line anchor), resolves it against the repository working
//! tree, then reveals + previews it — switching root to the repo when the target is outside the
//! current root (the user-chosen behavior). Anything it cannot parse or find degrades to a flash
//! (design principle #3: never crash).

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::i18n::{tr, Msg};

/// A jump target parsed from clipboard text, before filesystem resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteTarget {
    /// A local path string (absolute, `~`-prefixed, or relative), with an optional 1-based line.
    Local { path: String, line: Option<usize> },
    /// A GitHub-ish URL broken into its URL-path components (owner/repo/blob/ref/…/file). Resolved
    /// by finding the longest trailing suffix that exists under the repo workdir / current root —
    /// which is robust to slashy branch names and to `blob`/`raw`/`tree`/`blame` forms without
    /// having to know where the ref ends.
    Url {
        components: Vec<String>,
        line: Option<usize>,
    },
}

impl PasteTarget {
    /// The 1-based line anchor, if the input carried one.
    pub fn line(&self) -> Option<usize> {
        match self {
            PasteTarget::Local { line, .. } | PasteTarget::Url { line, .. } => *line,
        }
    }
}

/// Parse clipboard text into a jump target. Returns None when nothing path-like can be recovered
/// (empty, or a non-GitHub URL we will not go fetch).
pub fn parse_paste_target(raw: &str) -> Option<PasteTarget> {
    let s = clean_input(raw);
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        return parse_url(rest);
    }
    // Not a URL → treat as a local path; peel off a trailing `#L123` / `:123` line anchor.
    let (path, line) = split_line_anchor(&s);
    if path.is_empty() {
        return None;
    }
    Some(PasteTarget::Local {
        path: path.to_string(),
        line,
    })
}

/// Trim whitespace, unwrap one level of surrounding `<> " ' \``, and drop a leading `@`
/// (Claude Code / konoma's own `@path#L12` reference copy round-trips straight back in).
fn clean_input(raw: &str) -> String {
    let mut s = raw.trim();
    for (open, close) in [('<', '>'), ('"', '"'), ('\'', '\''), ('`', '`')] {
        if s.len() >= 2 && s.starts_with(open) && s.ends_with(close) {
            s = s[1..s.len() - 1].trim();
            break;
        }
    }
    if let Some(r) = s.strip_prefix('@') {
        s = r;
    }
    s.trim().to_string()
}

/// Split off a trailing line anchor: `path#L123` / `path#123` / `path:123`. Returns (path, line).
fn split_line_anchor(s: &str) -> (&str, Option<usize>) {
    if let Some(hash) = s.rfind('#') {
        if let Some(n) = parse_line_fragment(&s[hash + 1..]) {
            return (&s[..hash], Some(n));
        }
    }
    // Editor / ripgrep style `path:123` — only when the tail is purely digits.
    if let Some(colon) = s.rfind(':') {
        let head = &s[..colon];
        let tail = &s[colon + 1..];
        if !head.is_empty() && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(n) = tail.parse::<usize>() {
                if n > 0 {
                    return (head, Some(n));
                }
            }
        }
    }
    (s, None)
}

/// Parse a URL fragment such as `L123`, `L123-L130`, `L123-130`, or `123` into the start line.
fn parse_line_fragment(frag: &str) -> Option<usize> {
    let f = frag
        .strip_prefix('L')
        .or_else(|| frag.strip_prefix('l'))
        .unwrap_or(frag);
    let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<usize>().ok().filter(|n| *n > 0)
}

/// Parse the part of a URL after the scheme. Only GitHub-ish hosts are accepted; the path becomes
/// components and `#L..` becomes the line.
fn parse_url(rest: &str) -> Option<PasteTarget> {
    let (host, pathpart) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    if !host.to_ascii_lowercase().contains("github") {
        return None;
    }
    let (path_and_query, frag) = match pathpart.split_once('#') {
        Some((pq, f)) => (pq, Some(f)),
        None => (pathpart, None),
    };
    let path_only = path_and_query.split('?').next().unwrap_or(path_and_query);
    let components: Vec<String> = path_only
        .split('/')
        .filter(|c| !c.is_empty())
        .map(percent_decode)
        .collect();
    if components.is_empty() {
        return None;
    }
    let line = frag.and_then(parse_line_fragment);
    Some(PasteTarget::Url { components, line })
}

/// Minimal percent-decoder for URL path components (turns `%20` etc. back into bytes).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Expand a leading `~` / `~/` against `$HOME`.
fn expand_tilde(s: &str) -> String {
    if s == "~" {
        if let Some(h) = std::env::var_os("HOME") {
            return h.to_string_lossy().into_owned();
        }
    } else if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = std::env::var_os("HOME") {
            return format!("{}/{}", PathBuf::from(h).display(), rest);
        }
    }
    s.to_string()
}

/// Canonicalize `p` if it exists, else None.
fn canonical_existing(p: &Path) -> Option<PathBuf> {
    if p.exists() {
        Some(p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
    } else {
        None
    }
}

/// Read the system clipboard as text. Err on environments where arboard is unavailable.
fn read_clipboard() -> anyhow::Result<String> {
    let mut cb = arboard::Clipboard::new()?;
    Ok(cb.get_text()?)
}

impl App {
    /// `P`: read a path / GitHub link from the clipboard and jump there.
    pub fn paste_jump(&mut self) {
        match read_clipboard() {
            Ok(text) => self.paste_jump_from(&text),
            Err(_) => {
                self.flash = Some(tr(self.lang, Msg::PasteJumpNoClipboard).to_string());
            }
        }
    }

    /// Core of paste-jump, separated from the clipboard read so it is testable with a literal string.
    /// Parses `raw`, resolves it to an existing file/dir, and navigates (revealing + previewing).
    pub fn paste_jump_from(&mut self, raw: &str) {
        let Some(target) = parse_paste_target(raw) else {
            self.flash = Some(tr(self.lang, Msg::PasteJumpUnrecognized).to_string());
            return;
        };
        let line = target.line();
        let Some(path) = self.resolve_paste_target(&target) else {
            self.flash = Some(format!(
                "{}{}",
                tr(self.lang, Msg::PasteJumpNotFound),
                raw.trim()
            ));
            return;
        };
        self.paste_jump_to(&path, line);
    }

    /// Resolve a parsed target to an existing canonical path, or None.
    fn resolve_paste_target(&self, target: &PasteTarget) -> Option<PathBuf> {
        match target {
            PasteTarget::Local { path, .. } => self.resolve_local_path(path),
            PasteTarget::Url { components, .. } => self.resolve_url_components(components),
        }
    }

    /// A local path: absolute is used as-is; relative is tried against the current root, then the
    /// repo workdir. Returns the first that exists.
    fn resolve_local_path(&self, raw: &str) -> Option<PathBuf> {
        let pb = PathBuf::from(expand_tilde(raw));
        let candidates: Vec<PathBuf> = if pb.is_absolute() {
            vec![pb]
        } else {
            let mut v = vec![self.root.join(&pb)];
            if let Some(wd) = crate::git::workdir(&self.root) {
                if wd != self.root {
                    v.push(wd.join(&pb));
                }
            }
            v
        };
        candidates.iter().find_map(|c| canonical_existing(c))
    }

    /// URL path components: search under the repo workdir (preferred) then the current root, taking
    /// the longest trailing suffix that exists. Longest-first so `src/app.rs` wins over a bare
    /// `app.rs`, and the owner/repo/blob/ref prefix (which never exists on disk) is skipped.
    fn resolve_url_components(&self, components: &[String]) -> Option<PathBuf> {
        let mut bases: Vec<PathBuf> = Vec::new();
        if let Some(wd) = crate::git::workdir(&self.root) {
            bases.push(wd);
        }
        if !bases.iter().any(|b| b == &self.root) {
            bases.push(self.root.clone());
        }
        for base in &bases {
            for start in 0..components.len() {
                let mut cand = base.clone();
                for c in &components[start..] {
                    cand.push(c);
                }
                if let Some(p) = canonical_existing(&cand) {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Navigate to an existing `path`: switch root to its repo workdir (or parent) when it is
    /// outside the current root, deep-reveal it in the tree, and open a file preview (scrolling to
    /// `line` when given). Sets a `→ path[:line]` flash. Also reused by "open link in a new tab"
    /// (`md_open_focused_link_new_tab`) after it creates the fresh tab.
    pub(crate) fn paste_jump_to(&mut self, path: &Path, line: Option<usize>) {
        if !path.starts_with(&self.root) {
            let anchor = if path.is_dir() {
                path.to_path_buf()
            } else {
                path.parent().map(Path::to_path_buf).unwrap_or_default()
            };
            let new_root = crate::git::workdir(&anchor)
                .filter(|wd| path.starts_with(wd))
                .unwrap_or(anchor);
            let new_root = new_root.canonicalize().unwrap_or(new_root);
            self.jump_to_dir(new_root);
        }
        let revealed = self.reveal_path_deep(path).unwrap_or(false);
        let mut disp = super::home_relative(path);
        if path.is_file() {
            self.enter_preview(path);
            if let Some(l) = line {
                self.preview_goto_line(l);
                disp.push_str(&format!(":{l}"));
            }
        } else if !revealed && path != self.root {
            // A directory we could not surface in the tree (hidden by the dotfile filter), and it is
            // not the new root itself (jumping *into* a directory makes it the root, which is fine).
            self.flash = Some(format!(
                "{}: {}",
                tr(self.lang, Msg::JumpTargetHidden),
                disp
            ));
            return;
        }
        self.flash = Some(format!("→ {disp}"));
    }

    /// Scroll a windowed (Code/Text, or raw Markdown) preview so 1-based `line` is visible with a
    /// few lines of context above and the caret on it. A decorated Markdown/Mermaid preview is first
    /// switched to raw source (which is windowed) so the line is addressable. No-op for image / table
    /// / pdf previews (there is no line coordinate to land on).
    fn preview_goto_line(&mut self, line: usize) {
        if self.is_decorated_kind() && !self.is_md_raw() {
            self.toggle_md_raw();
        }
        if !self.is_windowed() {
            return;
        }
        let line0 = line.saturating_sub(1);
        let top = line0.saturating_sub(3);
        if let Some(win) = self.preview_win.as_mut() {
            if let Ok((off, _)) = win.advance(0, top) {
                self.preview_byte_top = off;
                self.preview_top_line = top;
                self.preview_cursor_line = line0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_absolute_with_colon_line() {
        assert_eq!(
            parse_paste_target("/Users/x/src/app.rs:120"),
            Some(PasteTarget::Local {
                path: "/Users/x/src/app.rs".into(),
                line: Some(120),
            })
        );
    }

    #[test]
    fn parses_local_with_hash_line() {
        assert_eq!(
            parse_paste_target("src/app.rs#L42"),
            Some(PasteTarget::Local {
                path: "src/app.rs".into(),
                line: Some(42),
            })
        );
    }

    #[test]
    fn parses_plain_relative_path() {
        assert_eq!(
            parse_paste_target("  docs/PRD.md  "),
            Some(PasteTarget::Local {
                path: "docs/PRD.md".into(),
                line: None,
            })
        );
    }

    #[test]
    fn parses_github_blob_url_with_line() {
        assert_eq!(
            parse_paste_target("https://github.com/LESIM-Co-Ltd/konoma/blob/main/src/app.rs#L120"),
            Some(PasteTarget::Url {
                components: vec![
                    "LESIM-Co-Ltd".into(),
                    "konoma".into(),
                    "blob".into(),
                    "main".into(),
                    "src".into(),
                    "app.rs".into(),
                ],
                line: Some(120),
            })
        );
    }

    #[test]
    fn parses_github_raw_url() {
        assert_eq!(
            parse_paste_target("https://raw.githubusercontent.com/o/r/main/src/lib.rs"),
            Some(PasteTarget::Url {
                components: vec![
                    "o".into(),
                    "r".into(),
                    "main".into(),
                    "src".into(),
                    "lib.rs".into(),
                ],
                line: None,
            })
        );
    }

    #[test]
    fn line_range_takes_start() {
        // `#L120-L130` and `?plain=1` both appear on real GitHub links.
        assert_eq!(
            parse_paste_target("https://github.com/o/r/blob/main/x.rs?plain=1#L120-L130")
                .and_then(|t| t.line()),
            Some(120)
        );
    }

    #[test]
    fn strips_at_ref_and_wrapping() {
        // konoma's own `Y` copy is `@path#L12`; wrapped `<...>` also unwraps.
        assert_eq!(
            parse_paste_target("@src/main.rs#L7"),
            Some(PasteTarget::Local {
                path: "src/main.rs".into(),
                line: Some(7),
            })
        );
        assert_eq!(
            parse_paste_target("<https://github.com/o/r/blob/dev/a.rs>").and_then(|t| match t {
                PasteTarget::Url { components, .. } => components.last().cloned(),
                _ => None,
            }),
            Some("a.rs".into())
        );
    }

    #[test]
    fn slashy_branch_is_left_to_suffix_search() {
        // A branch name with a slash makes the ref span two components; the file suffix is still the
        // tail, and resolve_url_components finds it by existence (parser keeps all components).
        let t = parse_paste_target("https://github.com/o/r/blob/feat/foo/src/app.rs#L3").unwrap();
        assert_eq!(t.line(), Some(3));
        match t {
            PasteTarget::Url { components, .. } => {
                assert_eq!(components.last().unwrap(), "app.rs");
                assert!(components.contains(&"feat".to_string()));
            }
            _ => panic!("expected Url"),
        }
    }

    #[test]
    fn non_github_url_is_rejected() {
        assert_eq!(parse_paste_target("https://example.com/a/b.rs"), None);
    }

    #[test]
    fn empty_input_is_none() {
        assert_eq!(parse_paste_target("   "), None);
        assert_eq!(parse_paste_target(""), None);
    }

    #[test]
    fn colon_without_digits_is_not_a_line() {
        // A path containing a colon but no trailing number stays intact.
        assert_eq!(
            parse_paste_target("weird:name"),
            Some(PasteTarget::Local {
                path: "weird:name".into(),
                line: None,
            })
        );
    }

    #[test]
    fn percent_decodes_spaces() {
        let t = parse_paste_target("https://github.com/o/r/blob/main/my%20dir/file.rs").unwrap();
        match t {
            PasteTarget::Url { components, .. } => {
                assert!(components.contains(&"my dir".to_string()));
            }
            _ => panic!("expected Url"),
        }
    }
}
