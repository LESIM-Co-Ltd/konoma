//! Turning the raw characters between two delimiters into the string that gets drawn.
//!
//! mermaid spreads this over three places — the lexer strips the quotes, `FlowDB` strips them
//! *again* on the assembled text, and `common.getRows` splits the line breaks at render time —
//! so it is easy to implement one of the three and believe the label is done. All three are
//! here, plus the entity pass, because the parser's output is what later stages measure: a
//! label that still says `<br/>` is one box too wide and one line too short, and nothing
//! downstream will notice.

/// Everything mermaid does to a label between reading it and drawing it.
///
/// 1. trim, 2. drop one pair of enclosing quotes (and the backticks of a `"`…`"` markdown
///    string), 3. every `<br>` spelling and every literal `\n` becomes a real newline,
/// 4. `#nn;` / `#name;` entities become their characters.
pub fn decode_label(raw: &str) -> String {
    let t = raw.trim();
    let t = strip_quotes(t);
    let t = replace_breaks(t);
    let t = t.replace("\\n", "\n");
    decode_entities(&t)
}

/// Removes one enclosing pair of `"`, then one enclosing pair of backticks.
///
/// The second pass is the `"`…`"` markdown-string form. konoma has no markdown renderer inside
/// a diagram label, so the fences come off and the text inside is drawn literally — which is
/// also what mermaid does when HTML labels are off, and konoma's labels always are (§4-4).
fn strip_quotes(t: &str) -> &str {
    let t = match (t.strip_prefix('"'), t.strip_suffix('"')) {
        (Some(_), Some(_)) if t.len() >= 2 => &t[1..t.len() - 1],
        _ => t,
    };
    match (t.strip_prefix('`'), t.strip_suffix('`')) {
        (Some(_), Some(_)) if t.len() >= 2 => &t[1..t.len() - 1],
        _ => t,
    }
}

/// `/<\/?br\s*\/?>/gi` — the four spellings `<br>`, `<br/>`, `<br />` and the malformed but
/// common `</br>`, in any case. Written out because getting this wrong leaves the tag as
/// visible text in the box.
pub fn replace_breaks(t: &str) -> String {
    let chars: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(end) = match_break_tag(&chars, i) {
                out.push('\n');
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Matches `<` `/`? `br` (any case) whitespace* `/`? `>` starting at `i`, returning the index
/// just past the `>`.
fn match_break_tag(chars: &[char], i: usize) -> Option<usize> {
    let mut p = i + 1;
    if chars.get(p) == Some(&'/') {
        p += 1;
    }
    let b = chars.get(p)?;
    let r = chars.get(p + 1)?;
    if !(b.eq_ignore_ascii_case(&'b') && r.eq_ignore_ascii_case(&'r')) {
        return None;
    }
    p += 2;
    while chars.get(p).is_some_and(|c| c.is_whitespace()) {
        p += 1;
    }
    if chars.get(p) == Some(&'/') {
        p += 1;
    }
    if chars.get(p) == Some(&'>') {
        Some(p + 1)
    } else {
        None
    }
}

/// mermaid's escape hatch for characters its own lexer would eat: `#35;` is `#`, `#quot;` is
/// `"`. It works by rewriting `#\w+;` into an HTML entity and letting the browser decode it, so
/// a numeric form is a code point and a named form is an HTML entity name.
///
/// An unrecognised name is left exactly as written rather than turned into a stray `&name;`.
/// The browser would show the literal `&name;`; showing the literal `#name;` loses nothing and
/// keeps the parser from inventing characters.
fn decode_entities(t: &str) -> String {
    let chars: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' {
            let mut p = i + 1;
            while chars
                .get(p)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                p += 1;
            }
            if p > i + 1 && chars.get(p) == Some(&';') {
                let name: String = chars[i + 1..p].iter().collect();
                if let Some(c) = entity_char(&name) {
                    out.push(c);
                    i = p + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Resolves one entity body (the text between `#` and `;`).
fn entity_char(name: &str) -> Option<char> {
    if let Some(digits) = name.strip_prefix(|c: char| c == 'x' || c == 'X') {
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return u32::from_str_radix(digits, 16)
                .ok()
                .and_then(char::from_u32);
        }
    }
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        return name.parse::<u32>().ok().and_then(char::from_u32);
    }
    NAMED_ENTITIES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

/// The HTML entity names worth carrying. Everything here is either something mermaid's own
/// lexer would otherwise eat (`quot`, `lt`, `gt`, `amp`, `semi`) or a character a diagram author
/// cannot type on a normal keyboard. Anything else stays literal.
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("quot", '"'),
    ("apos", '\''),
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("nbsp", '\u{a0}'),
    ("semi", ';'),
    ("hellip", '…'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("bull", '•'),
    ("middot", '·'),
    ("deg", '°'),
    ("plusmn", '±'),
    ("times", '×'),
    ("divide", '÷'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("larr", '←'),
    ("rarr", '→'),
    ("uarr", '↑'),
    ("darr", '↓'),
    ("harr", '↔'),
    ("infin", '∞'),
    ("ne", '≠'),
    ("le", '≤'),
    ("ge", '≥'),
    ("sum", '∑'),
    ("radic", '√'),
    ("check", '✓'),
    ("cross", '✗'),
    ("star", '★'),
    ("euro", '€'),
    ("pound", '£'),
    ("yen", '¥'),
    ("cent", '¢'),
    ("sect", '§'),
    ("para", '¶'),
    ("dagger", '†'),
    ("alpha", 'α'),
    ("beta", 'β'),
    ("gamma", 'γ'),
    ("delta", 'δ'),
    ("epsilon", 'ε'),
    ("theta", 'θ'),
    ("lambda", 'λ'),
    ("mu", 'μ'),
    ("pi", 'π'),
    ("sigma", 'σ'),
    ("phi", 'φ'),
    ("omega", 'ω'),
];
