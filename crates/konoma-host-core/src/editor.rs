//! `[editor]`: which editor `e` opens, and the argv it is launched with.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

/// External editor settings (FR: delegate editing with `e`). **Configurable per extension.**
/// Priority: per-extension `[editor.ext]` -> `editor.command` (global default) -> `$VISUAL` -> `$EDITOR` -> `vim`.
/// Values are command + args (whitespace-separated). `{path}` is substituted with the file path (otherwise
/// appended at the end); `{line}` with the current preview line. Without a `{line}` token, common editors
/// still open at the line automatically (vim `+N`, VS Code `-g path:N`, Sublime/Helix `path:N`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Default editor when there is no per-extension match. If empty, $VISUAL -> $EDITOR -> vim.
    pub command: String,
    /// Map of extension (no dot) -> editor command. Example: `rs = "nvim"`, `md = "code -w"`.
    pub ext: HashMap<String, String>,
}

impl EditorConfig {
    /// The configured editor command (the pure part, excluding env / the default vi). Priority: per-extension -> command.
    /// None if both are unset / whitespace-only. Extensions are matched in lowercase.
    pub fn configured(&self, ext: &str) -> Option<String> {
        let by_ext = self
            .ext
            .get(ext)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        by_ext.or_else(|| {
            let d = self.command.trim();
            (!d.is_empty()).then(|| d.to_string())
        })
    }

    /// Resolves the argv of the editor for editing `path`, optionally jumping to `line` (1-based).
    /// Priority: per-extension -> command -> `$VISUAL` -> `$EDITOR` -> `vim`. `{path}` substitution or appended at the end.
    /// `line` matches the on-screen preview position; see `build_argv`/`apply_editor_line` for how it reaches the editor.
    pub fn resolve(&self, path: &Path, line: Option<usize>) -> Vec<String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let tmpl = self
            .configured(&ext)
            .or_else(|| env_nonempty("VISUAL"))
            .or_else(|| env_nonempty("EDITOR"))
            .unwrap_or_else(|| "vim".to_string());
        build_argv(&tmpl, path, line)
    }
}

/// Reads an environment variable; None if whitespace-only / unset.
pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Splits a command template (whitespace-separated) into argv and inserts the file path (and,
/// optionally, a line number). A token containing `{path}` is substituted, otherwise the path is
/// appended at the end. `{line}` is substituted with `line` (or 1 when unknown). When `line` is
/// present but the template has no `{line}` token, `apply_editor_line` injects the right line flag
/// for common editors. An empty template is `vim <path>`.
pub fn build_argv(tmpl: &str, path: &Path, line: Option<usize>) -> Vec<String> {
    let p = path.to_string_lossy().to_string();
    let mut argv: Vec<String> = tmpl.split_whitespace().map(|s| s.to_string()).collect();
    if argv.is_empty() {
        argv.push("vim".to_string());
    }
    let mut path_sub = false;
    let mut line_sub = false;
    for a in argv.iter_mut() {
        if a.contains("{path}") {
            *a = a.replace("{path}", &p);
            path_sub = true;
        }
        if a.contains("{line}") {
            *a = a.replace("{line}", &line.unwrap_or(1).to_string());
            line_sub = true;
        }
    }
    if !path_sub {
        argv.push(p);
    }
    if let Some(l) = line {
        if !line_sub {
            apply_editor_line(&mut argv, l);
        }
    }
    argv
}

/// When the editor template has no explicit `{line}` token, inject a line-jump argument for common
/// editors so the file opens at `line` (1-based). The vi/vim family also gets `+normal! zt` to scroll
/// that line to the top of the window (matching konoma's top-of-view). Also recognizes terminal editors
/// that accept `+N`, VS Code (`-g <path>:<line>`), and Sublime/Helix/Zed (`<path>:<line>`). Unknown
/// editors are left unchanged (they simply open at the top — configure `{line}` for them explicitly).
fn apply_editor_line(argv: &mut Vec<String>, line: usize) {
    let Some(prog) = argv.first() else {
        return;
    };
    let name = Path::new(prog)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        // vi/vim family: `+N` positions the cursor, and `+normal! zt` scrolls that line to the top of
        // the window so it matches konoma's top-of-view. Without `zt`, vim leaves the window at the file
        // top when N fits on the first screen (cursor buried mid-screen — 2026-07-09 user report).
        "vim" | "nvim" | "vi" | "view" | "gvim" | "mvim" | "vimx" | "neovim" => {
            argv.insert(1, format!("+{line}"));
            argv.insert(2, "+normal! zt".to_string());
        }
        // Other terminal editors that accept `+N` (no reliable scroll-to-top flag; open at the line).
        "nano" | "pico" | "emacs" | "emacsclient" | "gedit" | "kak" | "kakoune" | "joe" | "jed"
        | "ne" | "mcedit" | "micro" => {
            argv.insert(1, format!("+{line}"));
        }
        // VS Code family: `-g <path>:<line>`.
        "code" | "code-insiders" | "codium" | "vscodium" | "cursor" | "windsurf" => {
            if let Some(last) = argv.last_mut() {
                *last = format!("{last}:{line}");
            }
            argv.insert(1, "-g".to_string());
        }
        // Sublime / Helix / Zed: `<path>:<line>`.
        "subl" | "sublime_text" | "hx" | "helix" | "zed" => {
            if let Some(last) = argv.last_mut() {
                *last = format!("{last}:{line}");
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_prefers_ext_then_command_and_skips_blanks() {
        let mut e = EditorConfig {
            command: " nvim ".into(),
            ..Default::default()
        };
        e.ext.insert("md".into(), "code -w".into());
        e.ext.insert("blank".into(), "  ".into());
        assert_eq!(e.configured("md").as_deref(), Some("code -w"));
        assert_eq!(e.configured("rs").as_deref(), Some("nvim"));
        assert_eq!(e.configured("blank").as_deref(), Some("nvim"));
        assert_eq!(EditorConfig::default().configured("rs"), None);
    }

    #[test]
    fn build_argv_substitutes_and_injects_the_line() {
        let p = Path::new("/t/a.rs");
        assert_eq!(build_argv("   ", p, None), vec!["vim", "/t/a.rs"]);
        assert_eq!(
            build_argv("code -g {path}:{line}", p, Some(4)),
            vec!["code", "-g", "/t/a.rs:4"]
        );
        assert_eq!(
            build_argv("nvim", p, Some(7)),
            vec!["nvim", "+7", "+normal! zt", "/t/a.rs"]
        );
        assert_eq!(build_argv("hx", p, Some(3)), vec!["hx", "/t/a.rs:3"]);
        assert_eq!(build_argv("weird", p, Some(3)), vec!["weird", "/t/a.rs"]);
    }

    #[test]
    fn configured_command_is_used_by_resolve() {
        let e = EditorConfig {
            command: "nano".into(),
            ..Default::default()
        };
        assert_eq!(
            e.resolve(Path::new("/t/a.txt"), Some(2)),
            vec!["nano", "+2", "/t/a.txt"]
        );
    }
}
