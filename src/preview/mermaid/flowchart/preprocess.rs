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
    /// `flowchart.curve` taken from an `%%{init: { ... }}%%` directive, if the source had one and
    /// it named that one key. Every other key an init directive might carry (`theme`, `look`,
    /// …) is left completely alone — see [`init_flowchart_curve`]'s own doc for why this reads
    /// only the one thing.
    pub flowchart_curve: Option<String>,
}

/// Runs mermaid's preprocessing pipeline over a diagram source.
pub fn preprocess(src: &str) -> Preprocessed {
    let text = normalize_newlines(src);
    let text = single_quote_html_attributes(&text);
    let (text, title) = strip_front_matter(&text);
    let (text, flowchart_curve) = strip_directives(&text);
    let text = strip_comments(&text);
    Preprocessed {
        text,
        title,
        flowchart_curve,
    }
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

/// Removes `%%{ ... }%%` directives, keeping the newlines they spanned, and reads
/// `flowchart.curve` out of the last `init`/`initialize`/`config` directive that named one (real
/// mermaid re-applies each directive over its config in source order, so a later one overrides an
/// earlier one on the same key — matching that here costs nothing since more than one init
/// directive in one diagram is already a corner case).
///
/// mermaid's own pattern tolerates a directive that never closes; scanning for `}%%` covers
/// every directive that was ever valid, and an unterminated one falls back to "to end of line"
/// so a stray `%%{` cannot swallow the diagram.
fn strip_directives(src: &str) -> (String, Option<String>) {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut curve = None;
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
            // `stop` is either `e + 3` (right after the closing `}%%`) or end-of-line for a
            // directive that never closed; either way the directive's own text — everything
            // `init_flowchart_curve` needs — is `chars[i+3..body_end]`, with the trailing `%%`
            // (or nothing, if unterminated) trimmed back off but the `}` that closes it kept.
            let body_end = end.map_or(stop, |e| e + 1);
            let body: String = chars[i + 3..body_end].iter().collect();
            if let Some(found) = init_flowchart_curve(&body) {
                curve = Some(found);
            }
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
    (out, curve)
}

fn find_seq(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || chars.len() < needle.len() {
        return None;
    }
    (from..=chars.len() - needle.len()).find(|&p| chars[p..p + needle.len()] == *needle)
}

/// Is `c` a character that can be part of a bare JS-object-literal key or a bare-word value
/// (mermaid's directive body is JSON5, which allows both unquoted keys and unquoted string-ish
/// values — nothing forbids `%%{init: {flowchart: {curve: linear}}}%%`, even though every real
/// example anyone writes quotes both).
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Finds `key` used as an object key in `text` — quoted with `'`/`"`, or bare — and returns
/// everything in `text` after its `:` (and any whitespace right after the colon). `None` if `key`
/// never appears in exactly that shape.
///
/// This is not a JSON parser: it does not track object nesting, so a `key` belonging to some
/// *other* object earlier in the same text could be found first. That is fine for the one thing
/// this crate ever calls it for — reading `flowchart.curve` out of an `init` directive
/// ([`init_flowchart_curve`]) — because each call is already given a slice narrowed to the one
/// object the key has to come from (the whole directive body, then the `flowchart: { ... }`
/// object inside it, then `curve` inside *that*), the same narrowing
/// [`crate::preview::mermaid::render::style::cascade`] uses for `classDef`/`style` layers.
fn key_value_start<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let chars: Vec<char> = text.chars().collect();
    let key_chars: Vec<char> = key.chars().collect();
    if key_chars.is_empty() || chars.len() < key_chars.len() {
        return None;
    }
    let mut i = 0;
    while i + key_chars.len() <= chars.len() {
        if chars[i..i + key_chars.len()] == key_chars[..] {
            let before_ok = i == 0 || !is_word_char(chars[i - 1]);
            let after_idx = i + key_chars.len();
            let after_ok = after_idx >= chars.len() || !is_word_char(chars[after_idx]);
            if before_ok && after_ok {
                let quote_before = i > 0 && matches!(chars[i - 1], '\'' | '"');
                let quote_after = after_idx < chars.len() && matches!(chars[after_idx], '\'' | '"');
                // Either both bare, or both quoted (not necessarily with the *same* quote
                // character — a mismatched pair can only come from malformed input, which must
                // fall back to the default rather than treat this as a hard error, PRD design
                // principle #3, not be rejected as "not really a key").
                if quote_before == quote_after {
                    let mut j = after_idx + usize::from(quote_after);
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j < chars.len() && chars[j] == ':' {
                        j += 1;
                        while j < chars.len() && chars[j].is_whitespace() {
                            j += 1;
                        }
                        let byte_start: usize = chars[..j].iter().map(|c| c.len_utf8()).sum();
                        return Some(&text[byte_start..]);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// If `text` starts with `{`, returns the brace-balanced contents up to (not including) its
/// matching `}`. `None` if `text` does not start with `{`, or the brace never closes.
///
/// Does not special-case a `{`/`}` written inside a quoted string value — a real limitation, kept
/// because every key this crate ever looks for (`flowchart`, `curve`) is a short bare identifier
/// no author has a reason to put a brace next to, and because the fallback for getting the
/// nesting wrong is silently reading no curve at all, never a crash (PRD design principle #3).
fn balanced_object(text: &str) -> Option<&str> {
    let mut iter = text.char_indices();
    match iter.next() {
        Some((_, '{')) => {}
        _ => return None,
    }
    let mut depth = 1i32;
    for (idx, c) in iter {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[1..idx]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Reads a JSON5-ish string value at the start of `text`: a `'...'`/`"..."` literal (no escape
/// handling — none of mermaid's own curve names ever need one), or a bare word up to the next
/// non-word character. `None` if `text` starts with anything else — an object, a number, `true`,
/// `null`, all valid JSON5 values but none of them a curve name, so [`Curve::parse`] would only
/// ever fold them back into `Basis` anyway; failing here instead just skips a step, not a
/// behaviour (design principle #3 again — nothing here treats a wrong *type* as an error).
///
/// [`Curve::parse`]: crate::preview::mermaid::render::edges::Curve::parse
fn leading_string_value(text: &str) -> Option<String> {
    let mut chars = text.chars();
    match chars.next() {
        Some(q @ ('\'' | '"')) => {
            let rest = &text[q.len_utf8()..];
            let end = rest.find(q)?;
            Some(rest[..end].to_string())
        }
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            let end = text
                .char_indices()
                .find(|&(_, c)| !is_word_char(c))
                .map_or(text.len(), |(i, _)| i);
            Some(text[..end].to_string())
        }
        _ => None,
    }
}

/// Reads `flowchart.curve` out of one `%%{ ... }%%` directive's body (the raw text between the
/// opening `%%{` and the closing `}%%`, so still including the directive's own outer braces —
/// `init: {"flowchart": {"curve": "linear"}}`, say).
///
/// Not a JSON parser, on purpose: the directive body is JSON5 (bare keys, either quote
/// character), and the only thing any caller of this ever needs is one string at one key path —
/// PRD design principle #3, "an unknown or malformed value never blocks a render", applied to
/// config the way it is already applied to a bad shape name or an unclosed fence elsewhere in
/// this crate. Every other key an init directive can carry — `theme`, `look`, `fontFamily`, … —
/// is left completely untouched: the existing corpus has `%%{init: {"theme": "dark"}}%%` cases
/// whose output must not move, and reading a key this crate does not act on would be strictly
/// more code for the same zero effect.
fn init_flowchart_curve(directive_body: &str) -> Option<String> {
    let directive = key_value_start(directive_body, "init")
        .or_else(|| key_value_start(directive_body, "initialize"))
        .or_else(|| key_value_start(directive_body, "config"))?;
    let config = balanced_object(directive)?;
    let flowchart = balanced_object(key_value_start(config, "flowchart")?)?;
    leading_string_value(key_value_start(flowchart, "curve")?)
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
