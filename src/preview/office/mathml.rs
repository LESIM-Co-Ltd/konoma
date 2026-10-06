//! Presentation MathML (W3C MathML 3, section 3) to LaTeX, for the formulas an OpenDocument text
//! carries (LibreOffice Math stores each one as an embedded object whose `content.xml` is MathML).
//!
//! A pure function in the image of [`super::omml::to_latex`]: [`to_latex`] takes the XML of one
//! `<math>` element and returns LaTeX that konoma's math engine (RaTeX, `preview/math.rs`) can
//! draw, or `None` (the caller then shows [`fallback_text`]). Elements are matched by *local name*
//! (LibreOffice writes the default namespace, older files a `math:` prefix). The reader is bounded:
//! input length, element depth, element count and output length are capped, nothing is expanded
//! (quick-xml never expands DTD entities) and no input makes it panic. The character tables are
//! the ones of the OMML converter, so both produce the same LaTeX for the same symbol.
//!
//! | MathML | LaTeX |
//! |---|---|
//! | `mi` `mn` `mo` | the characters (Greek, operators and relations by name); a multi-letter `mi` is a function (`\sin`) or `\operatorname{..}` |
//! | `mtext` `ms` | `\text{..}` |
//! | `mspace` | `\,` `\quad` `\qquad` by width |
//! | `mfrac` | `\frac{..}{..}`; `linethickness="0"` is `\atop`; `bevelled` is `a/b` |
//! | `msqrt` `mroot` | `\sqrt{..}` `\sqrt[n]{..}` |
//! | `msub` `msup` `msubsup` `mmultiscripts` | `{b}_{..}^{..}` (a big operator or a function name keeps its limits: `\sum_{..}^{..}`) |
//! | `munder` `mover` `munderover` | accents (`\hat` `\bar` `\vec` `\dot` ..), `\overline` `\underline` `\overbrace` `\underbrace`, limits of `\sum` / `\lim`, else `\overset` / `\underset` |
//! | `mfenced`, an `mrow` with stretchy fences | `\left( .. \right)` |
//! | `mtable` `mtr` `mtd` | `\begin{matrix} .. \end{matrix}` |
//! | `menclose` | `\boxed` `\overline` `\underline` `\sqrt` `\cancel` (what RaTeX draws); the content alone otherwise |
//! | `mphantom` | `\phantom{..}` |
//! | `mstyle` `mrow` `mpadded` `merror` `semantics` `maction` | transparent (`mathvariant` is inherited) |
//! | `annotation` (StarMath) | not converted; [`fallback_text`] offers it when the conversion fails |

use quick_xml::events::{BytesStart, Event};

use super::fmt_xlsx::{XmlReader, MAX_XML_DEPTH};
use super::omml::{
    accent_cmd, delim_cmd, drawable, escape_text, map_chars, nary_cmd, FUNCS, MAX_OUTPUT,
};

/// Longest MathML accepted (bytes).
const MAX_INPUT: usize = 1 << 20;
/// Most elements a formula may hold.
const MAX_NODES: usize = 100_000;
/// Longest text of one token kept (bytes).
const MAX_TOKEN_TEXT: usize = 8 * 1024;
/// Longest attribute value kept (a keyword or a short length, never more).
const MAX_ATTR: usize = 96;
/// Most rows / cells / scripts one element lays out.
const MAX_KIDS: usize = 4_000;

/// One parsed element: the local name, the few attributes the conversion reads, the text of a
/// token element and the children.
#[derive(Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    kids: Vec<Node>,
}

impl Node {
    fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == local)
            .map(|(_, v)| v.as_str())
    }

    fn is(&self, local: &str) -> bool {
        self.name == local
    }

    /// The children that lay something out (annotations and `mprescripts` markers are not content).
    fn content(&self) -> impl Iterator<Item = &Node> {
        self.kids
            .iter()
            .filter(|k| !matches!(k.name.as_str(), "annotation" | "annotation-xml"))
    }
}

/// Attributes worth keeping (matched on the local name).
const KEPT_ATTRS: &[&str] = &[
    "mathvariant",
    "open",
    "close",
    "separators",
    "stretchy",
    "fence",
    "accent",
    "accentunder",
    "linethickness",
    "width",
    "notation",
    "bevelled",
    "form",
    "encoding",
    "lquote",
    "rquote",
];

/// Elements whose text is data.
fn keeps_text(name: &str) -> bool {
    matches!(name, "mi" | "mn" | "mo" | "mtext" | "ms" | "annotation")
}

fn node_of(e: &BytesStart<'_>) -> Node {
    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false).flatten() {
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        if !KEPT_ATTRS.contains(&key.as_str()) || a.value.len() > MAX_ATTR {
            continue;
        }
        let v = match a.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
            Ok(v) => v.into_owned(),
            Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
        };
        attrs.push((key, v));
    }
    Node {
        name,
        attrs,
        ..Node::default()
    }
}

fn push_token_text(top: &mut Node, s: &str) {
    if keeps_text(&top.name) && top.text.len() < MAX_TOKEN_TEXT {
        top.text.push_str(s);
    }
}

fn parse(src: &str) -> Option<Node> {
    let mut rd = XmlReader::new(src.as_bytes());
    let mut buf = Vec::new();
    // Index 0 is a pseudo root holding the top-level elements.
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut count = 0usize;
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).ok()? {
            Event::Start(e) => {
                count += 1;
                if count > MAX_NODES || stack.len() > MAX_XML_DEPTH {
                    return None;
                }
                stack.push(node_of(&e));
            }
            Event::Empty(e) => {
                count += 1;
                if count > MAX_NODES {
                    return None;
                }
                let n = node_of(&e);
                stack.last_mut()?.kids.push(n);
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return None;
                }
                let n = stack.pop()?;
                stack.last_mut()?.kids.push(n);
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    push_token_text(top, &t.decode().ok()?);
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    push_token_text(top, &c.decode().ok()?);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    let name = r.decode().ok()?;
                    let s = match &*name {
                        "lt" => "<".to_string(),
                        "gt" => ">".to_string(),
                        "amp" => "&".to_string(),
                        "apos" => "'".to_string(),
                        "quot" => "\"".to_string(),
                        _ => match r.resolve_char_ref() {
                            Ok(Some(c)) => c.to_string(),
                            _ => String::new(),
                        },
                    };
                    push_token_text(top, &s);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return None;
    }
    stack.pop()
}

/// The `math` element of a parsed fragment (or the pseudo root when the fragment has none).
fn math_root(root: &Node) -> &Node {
    root.kids.iter().find(|k| k.is("math")).unwrap_or(root)
}

/// Converts the MathML of one formula to LaTeX. `None` when it is not well-formed, too big,
/// empty, or the result is not something RaTeX can draw (the caller shows [`fallback_text`]).
/// `_display` is accepted for the shape of the OMML converter: the caller decides inline or
/// display by what surrounds the formula (LibreOffice writes `display="block"` for every formula).
pub fn to_latex(xml: &str, _display: bool) -> Option<String> {
    if xml.len() > MAX_INPUT {
        return None;
    }
    let root = parse(xml)?;
    let math = math_root(&root);
    let out = seq(&math.kids, Cx::default()).trim().to_string();
    if out.is_empty() || out.len() > MAX_OUTPUT || !drawable(&out) {
        return None;
    }
    Some(out)
}

/// Whether the XML is a MathML formula (its root element is `math`, whatever the prefix): a formula
/// object of an OpenDocument can also be a chart or a drawing.
pub fn is_math(xml: &str) -> bool {
    xml.len() <= MAX_INPUT && parse(xml).is_some_and(|r| r.kids.iter().any(|k| k.is("math")))
}

/// What to show for a formula that could not be converted: the StarMath source LibreOffice keeps
/// in an `annotation` (it reads like the formula: `{a} over {b}`), else the characters of its
/// tokens. May be empty.
pub fn fallback_text(xml: &str) -> String {
    if xml.len() > MAX_INPUT {
        return String::new();
    }
    let Some(root) = parse(xml) else {
        return String::new();
    };
    let math = math_root(&root);
    let mut ann = String::new();
    find_annotation(math, &mut ann, 0);
    let ann = ann.split_whitespace().collect::<Vec<_>>().join(" ");
    if !ann.is_empty() {
        return ann;
    }
    let mut s = String::new();
    token_text(math, &mut s, 0);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_annotation(n: &Node, out: &mut String, depth: usize) {
    if depth > MAX_XML_DEPTH || !out.is_empty() {
        return;
    }
    if n.is("annotation") && n.attr("encoding").is_some_and(|e| e.contains("StarMath")) {
        out.push_str(n.text.trim());
        return;
    }
    for k in &n.kids {
        find_annotation(k, out, depth + 1);
    }
}

fn token_text(n: &Node, out: &mut String, depth: usize) {
    if depth > MAX_XML_DEPTH || out.len() > MAX_TOKEN_TEXT {
        return;
    }
    if matches!(n.name.as_str(), "annotation" | "annotation-xml") {
        return;
    }
    if keeps_text(&n.name) {
        out.push_str(n.text.trim());
        out.push(' ');
    }
    for k in &n.kids {
        token_text(k, out, depth + 1);
    }
}

// ---------------------------------------------------------------------------------------------
// conversion
// ---------------------------------------------------------------------------------------------

/// Context passed down: the inherited `mathvariant`.
#[derive(Clone, Copy, Default)]
struct Cx {
    variant: Option<&'static str>,
}

fn seq(kids: &[Node], cx: Cx) -> String {
    let mut s = String::new();
    for k in kids {
        s.push_str(&conv(k, cx));
        if s.len() > MAX_OUTPUT {
            break;
        }
    }
    s
}

/// The children of `n` as one braced group (an implicit `mrow`).
fn group(n: &Node, cx: Cx) -> String {
    format!("{{{}}}", seq(&n.kids, cx).trim())
}

/// One child as a braced group.
fn arg(n: Option<&Node>, cx: Cx) -> String {
    match n {
        Some(n) => format!("{{{}}}", conv(n, cx).trim()),
        None => "{}".to_string(),
    }
}

fn conv(n: &Node, cx: Cx) -> String {
    match n.name.as_str() {
        "mrow" => conv_row(n, cx),
        "math" | "mpadded" | "merror" | "mtd" | "mtr" => seq(&n.kids, cx),
        "mstyle" => {
            let cx = Cx {
                variant: n.attr("mathvariant").and_then(variant_of).or(cx.variant),
            };
            conv_row(n, cx)
        }
        "semantics" => n.content().next().map(|k| conv(k, cx)).unwrap_or_default(),
        "maction" => n.kids.first().map(|k| conv(k, cx)).unwrap_or_default(),
        "annotation" | "annotation-xml" | "mprescripts" | "none" | "mglyph" => String::new(),
        "mi" => conv_ident(n, cx),
        "mn" => styled(&map_chars(n.text.trim(), false), n.text.trim(), n, cx),
        "mo" => conv_op(n, cx),
        "mtext" => text_cmd(&n.text),
        "ms" => {
            let l = n.attr("lquote").unwrap_or("\"");
            let r = n.attr("rquote").unwrap_or("\"");
            text_cmd(&format!("{l}{}{r}", n.text.trim()))
        }
        "mspace" => conv_space(n),
        "mfrac" => conv_frac(n, cx),
        "msqrt" => format!(r"\sqrt{}", group(n, cx)),
        "mroot" => {
            let mut it = n.content();
            let base = it.next();
            let idx = it.next();
            match idx {
                Some(i) => format!(r"\sqrt[{}]{}", conv(i, cx).trim(), arg(base, cx)),
                None => format!(r"\sqrt{}", arg(base, cx)),
            }
        }
        "msub" | "msup" | "msubsup" => conv_scripts(n, cx),
        "mmultiscripts" => conv_multiscripts(n, cx),
        "munder" | "mover" | "munderover" => conv_under_over(n, cx),
        "mfenced" => conv_fenced(n, cx),
        "mtable" => conv_table(n, cx),
        "menclose" => conv_enclose(n, cx),
        "mphantom" => format!(r"\phantom{}", group(n, cx)),
        // Unknown wrappers are transparent.
        _ => seq(&n.kids, cx),
    }
}

/// `\text{..}` of a token's text. Runs of white space are one space and no-break spaces are
/// spaces (RaTeX's text mode has no `~`); the spaces at the ends stay: LibreOffice writes
/// `<mtext>if </mtext>` meaning "if", a space, then the next token.
fn text_cmd(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    let mut prev_ws = false;
    for c in t.chars() {
        if c.is_ascii_whitespace() || c == '\u{00A0}' {
            if !prev_ws {
                s.push(' ');
            }
            prev_ws = true;
        } else {
            s.push(c);
            prev_ws = false;
        }
    }
    if s.is_empty() {
        return String::new();
    }
    if s.trim().is_empty() {
        return r"\ ".to_string();
    }
    format!(r"\text{{{}}}", escape_text(&s))
}

/// The alphabets of `mathvariant` that have a LaTeX form (`italic` is the default one).
fn variant_of(v: &str) -> Option<&'static str> {
    Some(match v.trim() {
        "normal" => "normal",
        "bold" => "bold",
        "bold-italic" => "bold-italic",
        "double-struck" => "double-struck",
        "script" | "bold-script" => "script",
        "fraktur" | "bold-fraktur" => "fraktur",
        "sans-serif" | "bold-sans-serif" | "sans-serif-italic" | "sans-serif-bold-italic" => {
            "sans-serif"
        }
        "monospace" => "monospace",
        _ => return None,
    })
}

/// Applies a `mathvariant` (own or inherited) to a token's LaTeX. Only text with a letter changes
/// (digits and symbols look the same in every alphabet).
fn styled(body: &str, raw: &str, n: &Node, cx: Cx) -> String {
    if body.is_empty() || !raw.chars().any(char::is_alphabetic) {
        return body.to_string();
    }
    let v = n.attr("mathvariant").and_then(variant_of).or(cx.variant);
    match v {
        Some("normal") => format!(r"\mathrm{{{body}}}"),
        Some("bold") => format!(r"\mathbf{{{body}}}"),
        Some("bold-italic") => format!(r"\boldsymbol{{{body}}}"),
        Some("double-struck") => format!(r"\mathbb{{{body}}}"),
        Some("script") => format!(r"\mathcal{{{body}}}"),
        Some("fraktur") => format!(r"\mathfrak{{{body}}}"),
        Some("sans-serif") => format!(r"\mathsf{{{body}}}"),
        Some("monospace") => format!(r"\mathtt{{{body}}}"),
        _ => body.to_string(),
    }
}

fn conv_ident(n: &Node, cx: Cx) -> String {
    let t = n.text.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.chars().count() > 1 && t.chars().all(|c| c.is_ascii_alphabetic()) {
        // A name of several letters: a function (upright, with its own spacing) or a word.
        if FUNCS.contains(&t) {
            return format!(r"\{t} ");
        }
        return format!(r"\operatorname{{{t}}}");
    }
    styled(&map_chars(t, false), t, n, cx)
}

/// Whether `n` is a `mo` holding one big operator (a sum, an integral, a union ...).
fn nary_of(n: &Node) -> Option<&'static str> {
    if !n.is("mo") {
        return None;
    }
    let t = n.text.trim();
    let mut cs = t.chars();
    let c = cs.next()?;
    if cs.next().is_some() {
        return None;
    }
    nary_cmd(c)
}

/// Whether `n` is a token that spells a function name taking limits (`lim`, `max` ...).
fn func_of(n: &Node) -> Option<&'static str> {
    if !(n.is("mi") || n.is("mo") || n.is("mtext")) {
        return None;
    }
    let t = n.text.trim();
    FUNCS.iter().copied().find(|f| *f == t)
}

fn conv_op(n: &Node, _cx: Cx) -> String {
    let t = n.text.trim();
    if t.is_empty() {
        return String::new();
    }
    // Function application, invisible times / separator / plus: spacing marks, nothing to draw.
    if t.chars().all(|c| matches!(c, '\u{2061}'..='\u{2064}')) {
        return String::new();
    }
    if let Some(cmd) = nary_of(n) {
        return format!("{cmd} ");
    }
    map_chars(t, false)
}

fn conv_space(n: &Node) -> String {
    let w = n.attr("width").unwrap_or("");
    let (num, unit): (String, String) = {
        let num: String = w
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        let unit = w[num.len()..].trim().to_string();
        (num, unit)
    };
    let Ok(v) = num.parse::<f64>() else {
        // Named spaces (`thinmathspace` ...) and a missing width: a thin space.
        return if w.contains("negative") || w.starts_with('-') {
            String::new()
        } else {
            r"\,".to_string()
        };
    };
    let em = match unit.as_str() {
        "em" | "" => v,
        "ex" => v * 0.5,
        "px" => v / 16.0,
        "pt" => v / 10.0,
        "mm" => v / 3.5,
        "cm" => v / 0.35,
        "in" => v / 0.139,
        "%" => v / 100.0,
        _ => v,
    };
    if em <= 0.0 {
        String::new()
    } else if em >= 1.5 {
        r"\qquad ".to_string()
    } else if em >= 0.75 {
        r"\quad ".to_string()
    } else if em >= 0.3 {
        r"\; ".to_string()
    } else {
        r"\, ".to_string()
    }
}

fn zero_thickness(v: &str) -> bool {
    let num: String = v
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse::<f64>().is_ok_and(|x| x == 0.0)
}

fn conv_frac(n: &Node, cx: Cx) -> String {
    let mut it = n.content();
    let (num, den) = (arg(it.next(), cx), arg(it.next(), cx));
    if n.attr("linethickness").is_some_and(zero_thickness) {
        return format!(r"{{{num} \atop {den}}}");
    }
    if n.attr("bevelled") == Some("true") {
        return format!("{num}/{den}");
    }
    format!(r"\frac{num}{den}")
}

/// A script base: a big operator or a function name keeps its limit behaviour (no braces), anything
/// else is one braced group.
fn script_base(b: Option<&Node>, cx: Cx) -> String {
    match b {
        Some(b) => {
            if let Some(cmd) = nary_of(b) {
                return format!("{cmd} ");
            }
            if let Some(f) = func_of(b) {
                return format!(r"\{f} ");
            }
            arg(Some(b), cx)
        }
        None => "{}".to_string(),
    }
}

fn conv_scripts(n: &Node, cx: Cx) -> String {
    let mut it = n.content();
    let base = it.next();
    let b = script_base(base, cx);
    match n.name.as_str() {
        "msub" => format!("{b}_{}", arg(it.next(), cx)),
        "msup" => format!("{b}^{}", arg(it.next(), cx)),
        _ => {
            let sub = arg(it.next(), cx);
            let sup = arg(it.next(), cx);
            format!("{b}_{sub}^{sup}")
        }
    }
}

fn conv_multiscripts(n: &Node, cx: Cx) -> String {
    let mut it = n.kids.iter();
    let Some(base) = it.next() else {
        return String::new();
    };
    let mut post = String::new();
    let mut pre = String::new();
    let mut in_pre = false;
    let mut pending: Vec<&Node> = Vec::new();
    let mut count = 0usize;
    let flush = |pending: &mut Vec<&Node>, target: &mut String| {
        for pair in pending.chunks(2) {
            let sub = pair.first().filter(|k| !k.is("none"));
            let sup = pair.get(1).filter(|k| !k.is("none"));
            if let Some(s) = sub {
                target.push_str(&format!("_{}", arg(Some(s), cx)));
            }
            if let Some(s) = sup {
                target.push_str(&format!("^{}", arg(Some(s), cx)));
            }
        }
        pending.clear();
    };
    for k in it {
        count += 1;
        if count > MAX_KIDS {
            break;
        }
        if k.is("mprescripts") {
            flush(&mut pending, &mut post);
            in_pre = true;
            continue;
        }
        pending.push(k);
    }
    if in_pre {
        flush(&mut pending, &mut pre);
    } else {
        flush(&mut pending, &mut post);
    }
    let pre = if pre.is_empty() {
        String::new()
    } else {
        format!("{{}}{pre}")
    };
    format!("{pre}{}{post}", arg(Some(base), cx))
}

/// The first character of a `mo`'s text, when the node is a `mo`.
fn mo_char(n: Option<&Node>) -> Option<char> {
    let n = n?;
    if !n.is("mo") {
        return None;
    }
    n.text.trim().chars().next()
}

fn is_one_token(n: &Node) -> bool {
    matches!(n.name.as_str(), "mi" | "mn") && n.text.trim().chars().count() <= 1
}

fn conv_under_over(n: &Node, cx: Cx) -> String {
    let mut it = n.content();
    let Some(base) = it.next() else {
        return String::new();
    };
    let (under, over) = match n.name.as_str() {
        "munder" => (it.next(), None),
        "mover" => (None, it.next()),
        _ => {
            let u = it.next();
            (u, it.next())
        }
    };
    // A big operator or a function name takes its limits as scripts.
    if nary_of(base).is_some() || func_of(base).is_some() {
        let b = script_base(Some(base), cx);
        let mut s = b;
        if let Some(u) = under {
            s.push_str(&format!("_{}", arg(Some(u), cx)));
        }
        if let Some(o) = over {
            s.push_str(&format!("^{}", arg(Some(o), cx)));
        }
        return s;
    }
    match (under, over) {
        (None, Some(o)) => over_only(base, o, cx),
        (Some(u), None) => under_only(base, u, cx),
        (Some(u), Some(o)) => format!(
            r"\overset{}{{\underset{}{}}}",
            arg(Some(o), cx),
            arg(Some(u), cx),
            arg(Some(base), cx)
        ),
        (None, None) => conv(base, cx),
    }
}

/// The over-brace / under-brace characters.
const OVERBRACE: char = '\u{23DE}';
const UNDERBRACE: char = '\u{23DF}';

fn over_only(base: &Node, o: &Node, cx: Cx) -> String {
    // `\overbrace{x}` with a label: the label arrives as the outer `mover`.
    if base.is("mover") {
        let inner: Vec<&Node> = base.content().collect();
        if inner
            .get(1)
            .is_some_and(|b| mo_char(Some(b)) == Some(OVERBRACE))
        {
            return format!("{}^{}", conv(base, cx), arg(Some(o), cx));
        }
    }
    let b = arg(Some(base), cx);
    let Some(c) = mo_char(Some(o)) else {
        return format!(r"\overset{}{b}", arg(Some(o), cx));
    };
    let chr = c.to_string();
    match c {
        OVERBRACE => format!(r"\overbrace{b}"),
        '\u{00AF}' | '\u{203E}' | '\u{0305}' | '\u{2015}' | '_' => format!(r"\overline{b}"),
        '\u{2192}' | '\u{20D7}' => {
            if is_one_token(base) {
                format!(r"\vec{b}")
            } else {
                format!(r"\overrightarrow{b}")
            }
        }
        '\u{2190}' => format!(r"\overleftarrow{b}"),
        '\u{2194}' => format!(r"\overleftrightarrow{b}"),
        _ => match accent_cmd(&chr) {
            Some(cmd) => format!("{cmd}{b}"),
            None => format!(r"\overset{}{b}", arg(Some(o), cx)),
        },
    }
}

fn under_only(base: &Node, u: &Node, cx: Cx) -> String {
    if base.is("munder") {
        let inner: Vec<&Node> = base.content().collect();
        if inner
            .get(1)
            .is_some_and(|b| mo_char(Some(b)) == Some(UNDERBRACE))
        {
            return format!("{}_{}", conv(base, cx), arg(Some(u), cx));
        }
    }
    let b = arg(Some(base), cx);
    match mo_char(Some(u)) {
        Some(UNDERBRACE) => format!(r"\underbrace{b}"),
        Some('_' | '\u{00AF}' | '\u{0332}' | '\u{203E}' | '\u{2015}') => format!(r"\underline{b}"),
        Some('\u{2192}') => format!(r"\underrightarrow{b}"),
        Some('\u{2190}') => format!(r"\underleftarrow{b}"),
        Some('\u{2194}') => format!(r"\underleftrightarrow{b}"),
        _ => format!(r"\underset{}{b}", arg(Some(u), cx)),
    }
}

/// Splits a `separators` attribute into its characters (whitespace ignored).
fn separators(s: &str) -> Vec<char> {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn conv_fenced(n: &Node, cx: Cx) -> String {
    let open = n.attr("open").unwrap_or("(").trim().to_string();
    let close = n.attr("close").unwrap_or(")").trim().to_string();
    let seps = separators(n.attr("separators").unwrap_or(","));
    let items: Vec<String> = n
        .content()
        .take(MAX_KIDS)
        .map(|k| conv(k, cx).trim().to_string())
        .collect();
    let mut body = String::new();
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            if let Some(&s) = seps.get(i - 1).or(seps.last()) {
                let c = s.to_string();
                body.push(' ');
                body.push_str(&match c.as_str() {
                    "|" => r"\mid".to_string(),
                    _ => map_chars(&c, false),
                });
                body.push(' ');
            } else {
                body.push(' ');
            }
        }
        body.push_str(it);
    }
    wrap_fence(&open, &close, &body)
}

/// `\left<open> body \right<close>`; a delimiter RaTeX has no stretchy form for is printed as a
/// plain character, and no delimiter at all is a group.
fn wrap_fence(open: &str, close: &str, body: &str) -> String {
    let (Some(l), Some(r)) = (delim_cmd(open), delim_cmd(close)) else {
        return format!(
            "{}{}{}",
            map_chars(open, false),
            body,
            map_chars(close, false)
        );
    };
    if l == "." && r == "." {
        return format!("{{{body}}}");
    }
    format!(r"\left{l} {body} \right{r}")
}

fn stretchy_fence(n: &Node) -> bool {
    n.is("mo")
        && (n.attr("stretchy") == Some("true")
            || (n.attr("fence") == Some("true") && n.attr("stretchy") != Some("false")))
}

fn is_open_char(c: &str) -> bool {
    matches!(
        c,
        "(" | "["
            | "{"
            | "|"
            | "\u{2016}"
            | "\u{27E8}"
            | "\u{3008}"
            | "\u{2329}"
            | "\u{230A}"
            | "\u{2308}"
    )
}

fn is_close_char(c: &str) -> bool {
    matches!(
        c,
        ")" | "]"
            | "}"
            | "|"
            | "\u{2016}"
            | "\u{27E9}"
            | "\u{3009}"
            | "\u{232A}"
            | "\u{230B}"
            | "\u{2309}"
    )
}

/// An `mrow`. When its first / last child is a stretchy fence (LibreOffice's `left ( .. right )`)
/// the row is drawn between `\left` and `\right`; a lone stretchy fence pairs with an empty one
/// (`\left\{ .. \right.`, the `cases` shape).
fn conv_row(n: &Node, cx: Cx) -> String {
    let kids: Vec<&Node> = n.content().collect();
    // A table between two plain parentheses is a matrix: its fences stretch (LibreOffice's
    // `matrix` writes them stretchy, other producers often do not).
    let around_table = kids.len() == 3 && kids[1].is("mtable");
    let fence = |k: &Node| k.is("mo") && (around_table || stretchy_fence(k));
    let opener = kids
        .first()
        .filter(|k| fence(k) && is_open_char(k.text.trim()));
    let closer = kids
        .last()
        .filter(|k| kids.len() > 1 && fence(k) && is_close_char(k.text.trim()));
    if opener.is_none() && closer.is_none() {
        return seq(&n.kids, cx);
    }
    let from = usize::from(opener.is_some());
    let to = kids.len() - usize::from(closer.is_some());
    let mut body = String::new();
    for k in &kids[from..to.max(from)] {
        body.push_str(&conv(k, cx));
        if body.len() > MAX_OUTPUT {
            break;
        }
    }
    let l = opener.map(|k| k.text.trim()).unwrap_or("");
    let r = closer.map(|k| k.text.trim()).unwrap_or("");
    wrap_fence(l, r, body.trim())
}

fn conv_table(n: &Node, cx: Cx) -> String {
    let mut rows: Vec<String> = Vec::new();
    for tr in n.content().take(MAX_KIDS) {
        if !(tr.is("mtr") || tr.is("mlabeledtr")) {
            continue;
        }
        let skip = usize::from(tr.is("mlabeledtr"));
        let cells: Vec<String> = tr
            .content()
            .skip(skip)
            .take(MAX_KIDS)
            .map(|c| conv(c, cx).trim().to_string())
            .collect();
        rows.push(cells.join(" & "));
    }
    if rows.is_empty() {
        return String::new();
    }
    format!(r"\begin{{matrix}} {} \end{{matrix}}", rows.join(r" \\ "))
}

fn conv_enclose(n: &Node, cx: Cx) -> String {
    let body = group(n, cx);
    let notation = n.attr("notation").unwrap_or("longdiv");
    let has = |w: &str| notation.split_whitespace().any(|t| t == w);
    if has("box") || has("roundedbox") || has("circle") {
        format!(r"\boxed{body}")
    } else if has("top") {
        format!(r"\overline{body}")
    } else if has("bottom") {
        format!(r"\underline{body}")
    } else if has("radical") {
        format!(r"\sqrt{body}")
    } else if has("updiagonalstrike") && has("downdiagonalstrike") || has("cross") {
        format!(r"\xcancel{body}")
    } else if has("updiagonalstrike") {
        format!(r"\cancel{body}")
    } else if has("downdiagonalstrike") {
        format!(r"\bcancel{body}")
    } else {
        // Strikes and long division have no drawn form here: the content alone.
        body[1..body.len() - 1].to_string()
    }
}
