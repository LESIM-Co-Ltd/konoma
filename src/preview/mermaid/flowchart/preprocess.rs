//! The text mermaid hands its parser is not the text the author wrote.
//!
//! Five rewrites happen first, and **in this order** — mermaid's `preprocessDiagram()`:
//!
//! 1. CRLF (and lone CR) collapse to LF,
//! 2. double quotes inside HTML tag attributes become single quotes,
//! 3. a leading YAML front matter block is lifted out,
//! 4. `%%{ ... }%%` directives are removed,
//! 5. `%%` comment lines are removed.
//!
//! The order matters and is not guessable (`docs/FEATURE-MERMAID-RENDERER.md` §2-6): comments
//! go last so that a directive, whose first line also starts with `%%`, is not decapitated by
//! the comment pass; front matter goes before directives so a `%%` inside the YAML is never
//! seen as a directive.
//!
//! Everything removed here is replaced by the newlines it occupied instead of being deleted, so
//! that a line number in the preprocessed text is still a line number in the file the user is
//! looking at. Error messages are the whole reason konoma is writing this parser at all — the
//! current crate throws away messages as good as `unknown participant 'API' at line 3` — so the
//! numbers have to be true.

/// The result of running the five rewrites.
pub struct Preprocessed {
    /// The text the parser reads. Line numbers match the original source.
    pub text: String,
    /// `title:` taken from the front matter block, if there was one.
    pub title: Option<String>,
}

/// Runs mermaid's preprocessing pipeline over a diagram source.
pub fn preprocess(src: &str) -> Preprocessed {
    let text = normalize_newlines(src);
    let text = single_quote_html_attributes(&text);
    let (text, title) = strip_front_matter(&text);
    let text = strip_directives(&text);
    let text = strip_comments(&text);
    Preprocessed { text, title }
}

/// `\r\n` and a lone `\r` both become `\n`.
fn normalize_newlines(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// mermaid rewrites `<tag attr="v">` to `<tag attr='v'>` because its lexer would otherwise take
/// the `"` as the start of a quoted label and swallow the diagram. Only the attribute part of a
/// well-formed opening tag is touched; a bare `<` elsewhere is left alone.
fn single_quote_html_attributes(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // `<(\w+)([^>]*)>` — the tag name must be word characters and the tag must close.
        let mut j = i + 1;
        while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
            j += 1;
        }
        if j == i + 1 {
            out.push('<');
            i += 1;
            continue;
        }
        let mut k = j;
        while k < chars.len() && chars[k] != '>' {
            k += 1;
        }
        if k >= chars.len() {
            out.push('<');
            i += 1;
            continue;
        }
        out.push('<');
        out.extend(&chars[i + 1..j]);
        out.push_str(&requote_attributes(&chars[j..k]));
        out.push('>');
        i = k + 1;
    }
    out
}

/// `="([^"]*)"` -> `='$1'`, applied to the attribute run of one tag.
fn requote_attributes(attrs: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < attrs.len() {
        if attrs[i] == '=' && i + 1 < attrs.len() && attrs[i + 1] == '"' {
            if let Some(end) = (i + 2..attrs.len()).find(|&p| attrs[p] == '"') {
                out.push('=');
                out.push('\'');
                out.extend(&attrs[i + 2..end]);
                out.push('\'');
                i = end + 1;
                continue;
            }
        }
        out.push(attrs[i]);
        i += 1;
    }
    out
}

/// Lifts a leading `---` YAML block out of the source, keeping its newlines.
///
/// Only a *leading* block counts (mermaid anchors the pattern at the start with no `m` flag),
/// which is what keeps a `---` used as a link (`A --- B` never starts a line as `---` alone…)
/// or a horizontal rule further down from eating the diagram.
fn strip_front_matter(src: &str) -> (String, Option<String>) {
    let lines: Vec<&str> = src.split('\n').collect();
    if lines.is_empty() || lines[0].trim_end() != "---" {
        return (src.to_string(), None);
    }
    let close = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim_end() == "---")
        .map(|(i, _)| i);
    let Some(close) = close else {
        return (src.to_string(), None);
    };
    let title = front_matter_title(&lines[1..close]);
    let mut out = String::with_capacity(src.len());
    for _ in 0..=close {
        out.push('\n');
    }
    out.push_str(&lines[close + 1..].join("\n"));
    (out, title)
}

/// Reads a top-level `title:` out of a front matter block.
///
/// Deliberately not a YAML parser: only a key at column 0 counts, so a `title:` nested under
/// `config:` is not mistaken for the diagram's own. Everything else in the block (`config`,
/// `displayMode`, …) configures rendering options konoma does not implement (§2-5).
fn front_matter_title(body: &[&str]) -> Option<String> {
    for line in body {
        // `strip_prefix` matching means the key sat at column 0, so a `title:` nested under
        // `config:` (which is indented) is skipped rather than taken for the diagram's own.
        let Some(rest) = line.strip_prefix("title:") else {
            continue;
        };
        let v = rest.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(v);
        if v.is_empty() {
            return None;
        }
        return Some(v.to_string());
    }
    None
}

/// Removes `%%{ ... }%%` directives, keeping the newlines they spanned.
///
/// mermaid's own pattern tolerates a directive that never closes; scanning for `}%%` covers
/// every directive that was ever valid, and an unterminated one falls back to "to end of line"
/// so a stray `%%{` cannot swallow the diagram.
fn strip_directives(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && chars.get(i + 1) == Some(&'%') && chars.get(i + 2) == Some(&'{') {
            let end = find_seq(&chars, i + 3, &['}', '%', '%']);
            let stop = match end {
                Some(e) => e + 3,
                None => (i..chars.len())
                    .find(|&p| chars[p] == '\n')
                    .unwrap_or(chars.len()),
            };
            for c in &chars[i..stop] {
                if *c == '\n' {
                    out.push('\n');
                }
            }
            i = stop;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn find_seq(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || chars.len() < needle.len() {
        return None;
    }
    (from..=chars.len() - needle.len()).find(|&p| chars[p..p + needle.len()] == *needle)
}

/// Removes whole-line `%%` comments, keeping the line so numbering does not shift.
///
/// mermaid's pattern is `/^\s*%%(?!{)[^\n]+\n?/gm`: the comment owns the whole line, and the
/// negative lookahead is what leaves an already-processed directive alone. **There is no
/// end-of-line comment** — `A --> B %% note` keeps the `%% note` as part of the statement — so
/// this must not be softened into "strip from `%%` onwards".
///
/// Note that the `%%{` exclusion cannot fire in this pipeline: [`strip_directives`] runs first
/// and removes every `%%{ … }%%`, falling back to "to the end of the line" when one never
/// closes, so nothing beginning `%%{` reaches here. It is kept because it is mermaid's rule and
/// because it is what makes this pass safe on its own, but no test can reach it — changing it
/// changes nothing observable.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for (i, line) in src.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        let is_comment = trimmed.starts_with("%%") && !trimmed.starts_with("%%{");
        if !is_comment {
            out.push_str(line);
        }
    }
    out
}
