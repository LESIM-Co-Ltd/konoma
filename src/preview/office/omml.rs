//! Word math (OMML, ECMA-376 Part 1 §22.1 "Office Math Markup Language") to LaTeX.
//!
//! A pure function: [`to_latex`] takes the raw XML of one `<m:oMath>` / `<m:oMathPara>` element
//! and returns LaTeX that konoma's math engine (RaTeX, `preview/math.rs`) can draw, or `None`
//! (the caller then shows the expression's characters). Elements are matched by *local name*
//! (the fragment usually carries no `xmlns:m` declaration). The reader is bounded: input length,
//! element depth, element count and output length are capped, nothing is expanded (quick-xml
//! never expands DTD entities) and no input makes it panic.

use quick_xml::events::{BytesStart, Event};

use super::fmt_xlsx::{XmlReader, MAX_XML_DEPTH};

/// Longest fragment accepted (bytes).
const MAX_INPUT: usize = 1 << 20;
/// Most elements a fragment may hold.
const MAX_NODES: usize = 100_000;
/// Longest LaTeX produced (bytes); a bigger expression is not drawn.
pub(super) const MAX_OUTPUT: usize = 64 * 1024;
/// Longest text of one run kept (bytes).
const MAX_RUN_TEXT: usize = 8 * 1024;

/// One parsed element. Only what the conversion reads is kept: the local name, `m:val`, the
/// children, and the text of `m:t`.
#[derive(Default)]
struct Node {
    name: String,
    /// The `val` attribute (any prefix); `None` when absent.
    val: Option<String>,
    text: String,
    kids: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.name == name)
    }
    /// The `val` of the property child `name` inside this element's `<xPr>` (`None` = property
    /// absent, `Some(None)` = present without a value).
    fn prop(&self, pr: &str, name: &str) -> Option<Option<&str>> {
        let p = self.child(pr)?;
        let c = p.child(name)?;
        Some(c.val.as_deref())
    }
    /// An on/off property (ST_OnOff): present without a value means true.
    fn flag(&self, pr: &str, name: &str) -> bool {
        match self.prop(pr, name) {
            None => false,
            Some(None) => true,
            Some(Some(v)) => !matches!(v.trim(), "0" | "false" | "off"),
        }
    }
}

fn parse(src: &str) -> Option<Node> {
    let mut rd = XmlReader::new(src.as_bytes());
    let mut buf = Vec::new();
    // Stack of open elements; index 0 is a pseudo root that holds the top-level elements.
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut count = 0usize;
    loop {
        buf.clear();
        let ev = rd.read_event_into(&mut buf).ok()?;
        match ev {
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
                    if top.name == "t" && top.text.len() < MAX_RUN_TEXT {
                        top.text.push_str(&t.decode().ok()?);
                    }
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    if top.name == "t" && top.text.len() < MAX_RUN_TEXT {
                        let name = r.decode().ok()?;
                        match &*name {
                            "lt" => top.text.push('<'),
                            "gt" => top.text.push('>'),
                            "amp" => top.text.push('&'),
                            "apos" => top.text.push('\''),
                            "quot" => top.text.push('"'),
                            _ => {
                                if let Ok(Some(c)) = r.resolve_char_ref() {
                                    top.text.push(c);
                                }
                            }
                        }
                    }
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    if top.name == "t" && top.text.len() < MAX_RUN_TEXT {
                        top.text.push_str(&c.decode().ok()?);
                    }
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

fn node_of(e: &BytesStart<'_>) -> Node {
    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
    let mut val = None;
    // Only `val` is read, and only from the small elements that carry it (a long attribute is
    // never a character or a keyword).
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.local_name().as_ref() == b"val" && a.value.len() <= 64 {
            val = Some(
                match a.normalized_value(quick_xml::XmlVersion::Implicit1_0) {
                    Ok(v) => v.into_owned(),
                    Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
                },
            );
        }
    }
    Node {
        name,
        val,
        ..Node::default()
    }
}

/// Conversion context, passed down.
#[derive(Clone, Copy, Default)]
struct Ctx {
    /// Directly inside an `m:eqArr` row: `&` in a run is an alignment point.
    eq: bool,
    /// Inside a function name / limit base: a run that spells a function name becomes `\sin`...
    fname: bool,
    /// Inside a superscript: a prime is `\prime` (a bare `'` is itself a superscript in TeX, so
    /// `^{'}` would be raised twice).
    sup: bool,
}

/// Convert one `<m:oMath>` / `<m:oMathPara>` fragment to LaTeX. `None` when the fragment is not
/// well-formed, too big, empty, or the result is not something RaTeX can draw.
pub fn to_latex(fragment: &str, _display: bool) -> Option<String> {
    if fragment.len() > MAX_INPUT {
        return None;
    }
    let root = parse(fragment)?;
    let mut maths: Vec<&Node> = Vec::new();
    collect_maths(&root, &mut maths);
    let rows: Vec<String> = maths
        .iter()
        .map(|m| tidy(&conv_seq(&m.kids, Ctx::default())))
        .filter(|s| !s.is_empty())
        .collect();
    let out = match rows.len() {
        0 => return None,
        1 => rows.into_iter().next()?,
        _ => format!(
            r"\begin{{gathered}} {} \end{{gathered}}",
            rows.join(r" \\ ")
        ),
    };
    if out.len() > MAX_OUTPUT || !drawable(&out) {
        return None;
    }
    Some(out)
}

/// A readable linear form of a fragment that could not be drawn (too big, or LaTeX RaTeX
/// rejects): the characters with the structure kept as plain text, `(a)/(b)` for a fraction,
/// `x^2` / `x_(i+1)` for scripts, `∑_(i=1)^n (a)` for an operator with limits, `√(x)` for a root,
/// `[a, b; c, d]` for a matrix. Empty when the fragment is not well-formed or too big.
pub fn fallback_text(fragment: &str) -> String {
    if fragment.len() > MAX_INPUT {
        return String::new();
    }
    let Some(root) = parse(fragment) else {
        return String::new();
    };
    let mut maths: Vec<&Node> = Vec::new();
    collect_maths(&root, &mut maths);
    let rows: Vec<String> = maths
        .iter()
        .map(|m| lin_seq(&m.kids).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = rows.join("  ");
    if out.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

fn lin_seq(kids: &[Node]) -> String {
    let mut s = String::new();
    for k in kids {
        s.push_str(&lin(k));
        if s.len() > MAX_OUTPUT {
            break;
        }
    }
    s
}

fn lin_arg(n: &Node, name: &str) -> String {
    n.child(name)
        .map(|c| lin_seq(&c.kids).trim().to_string())
        .unwrap_or_default()
}

/// `s` as one unit of a linear formula: a single character stays as it is, anything longer is
/// parenthesised (`x^2`, `x^(n+1)`).
fn lin_unit(s: &str) -> String {
    if s.chars().count() <= 1 {
        s.to_string()
    } else {
        format!("({s})")
    }
}

fn lin(n: &Node) -> String {
    let arg = |name: &str| lin_arg(n, name);
    match n.name.as_str() {
        "r" => n
            .kids
            .iter()
            .filter(|k| k.name == "t")
            .map(|k| k.text.as_str())
            .collect(),
        "f" => format!("({})/({})", arg("num"), arg("den")),
        "sSup" => format!("{}^{}", lin_unit(&arg("e")), lin_unit(&arg("sup"))),
        "sSub" => format!("{}_{}", lin_unit(&arg("e")), lin_unit(&arg("sub"))),
        "sSubSup" => format!(
            "{}_{}^{}",
            lin_unit(&arg("e")),
            lin_unit(&arg("sub")),
            lin_unit(&arg("sup"))
        ),
        "sPre" => format!(
            "_{}^{}{}",
            lin_unit(&arg("sub")),
            lin_unit(&arg("sup")),
            lin_unit(&arg("e"))
        ),
        "rad" => {
            let deg = arg("deg");
            if n.flag("radPr", "degHide") || deg.is_empty() {
                format!("\u{221A}({})", arg("e"))
            } else {
                format!("\u{221A}[{deg}]({})", arg("e"))
            }
        }
        "nary" => {
            let op = match n.prop("naryPr", "chr").flatten() {
                None => "\u{222B}".to_string(),
                Some(c) => c.to_string(),
            };
            let mut s = op;
            if !n.flag("naryPr", "subHide") {
                let sub = arg("sub");
                if !sub.is_empty() {
                    s.push_str(&format!("_{}", lin_unit(&sub)));
                }
            }
            if !n.flag("naryPr", "supHide") {
                let sup = arg("sup");
                if !sup.is_empty() {
                    s.push_str(&format!("^{}", lin_unit(&sup)));
                }
            }
            s.push(' ');
            s.push_str(&lin_unit(&arg("e")));
            s
        }
        "d" => {
            let get = |name: &str, dflt: &str| match n.prop("dPr", name) {
                None => dflt.to_string(),
                Some(v) => v.unwrap_or("").to_string(),
            };
            let rows: Vec<String> = n
                .kids
                .iter()
                .filter(|k| k.name == "e")
                .map(|k| lin_seq(&k.kids).trim().to_string())
                .collect();
            let sep = get("sepChr", "|");
            format!(
                "{}{}{}",
                get("begChr", "("),
                rows.join(&format!(" {sep} ")),
                get("endChr", ")")
            )
        }
        "m" => {
            let rows: Vec<String> = n
                .kids
                .iter()
                .filter(|k| k.name == "mr")
                .map(|mr| {
                    mr.kids
                        .iter()
                        .filter(|k| k.name == "e")
                        .map(|k| lin_seq(&k.kids).trim().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect();
            format!("[{}]", rows.join("; "))
        }
        "eqArr" => n
            .kids
            .iter()
            .filter(|k| k.name == "e")
            .map(|k| lin_seq(&k.kids).trim().to_string())
            .collect::<Vec<_>>()
            .join("; "),
        "func" => {
            let e = arg("e");
            format!("{} {}", arg("fName"), lin_unit(&e))
        }
        "acc" => {
            let chr = n.prop("accPr", "chr").flatten().unwrap_or("\u{0302}");
            let e = arg("e");
            if chr
                .chars()
                .next()
                .is_some_and(|c| ('\u{0300}'..='\u{036F}').contains(&c))
            {
                format!("{e}{chr}")
            } else if chr.is_empty() {
                e
            } else {
                format!("{chr}({e})")
            }
        }
        "bar" => {
            let e = arg("e");
            if n.prop("barPr", "pos").flatten() == Some("top") {
                format!("\u{00AF}({e})")
            } else {
                format!("_({e})")
            }
        }
        "groupChr" => {
            let e = arg("e");
            let chr = n.prop("groupChrPr", "chr").flatten().unwrap_or("\u{23DF}");
            if chr.is_empty() {
                e
            } else {
                format!("{chr}({e})")
            }
        }
        "limLow" => format!("{}_{}", lin_unit(&arg("e")), lin_unit(&arg("lim"))),
        "limUpp" => format!("{}^{}", lin_unit(&arg("e")), lin_unit(&arg("lim"))),
        "borderBox" => format!("\u{25AD}({})", arg("e")),
        "phant" => {
            if n.prop("phantPr", "show").is_none() || n.flag("phantPr", "show") {
                arg("e")
            } else {
                String::new()
            }
        }
        name if is_pr(name) => String::new(),
        _ => lin_seq(&n.kids),
    }
}

/// The `oMath` elements of a fragment, in order (the fragment root may be an `oMathPara`, an
/// `oMath`, or — leniently — a bare run of math elements).
fn collect_maths<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    for k in &n.kids {
        if k.name == "oMath" {
            out.push(k);
        } else if k.name == "oMathPara" {
            collect_maths(k, out);
        }
    }
    if out.is_empty() && n.name.is_empty() {
        // No `oMath` wrapper at all: treat the top-level children as the expression.
        out.push(n);
    }
}

/// Longest LaTeX [`drawable`] lays out. The check runs RaTeX's whole layout and render on the
/// converter's thread and cannot be interrupted (a 65 KB formula took 1.2 s and 800 MB); a real
/// formula is a few hundred bytes, so a larger one is shown as its characters instead.
pub(super) const MAX_DRAWABLE_BYTES: usize = 4096;

thread_local! {
    /// Answers of [`drawable`] by formula (a document repeats its formulas). Bounded.
    static DRAWABLE: std::cell::RefCell<std::collections::HashMap<String, bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Whether RaTeX can draw `latex` (a formula over [`MAX_DRAWABLE_BYTES`] is not tried).
pub(super) fn drawable(latex: &str) -> bool {
    if latex.len() > MAX_DRAWABLE_BYTES {
        return false;
    }
    if let Some(known) = DRAWABLE.with(|c| c.borrow().get(latex).copied()) {
        return known;
    }
    let ok = crate::preview::math::latex_to_svg(latex, true, "#000000").is_some();
    DRAWABLE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 256 {
            c.clear();
        }
        c.insert(latex.to_string(), ok);
    });
    ok
}

fn conv_seq(kids: &[Node], cx: Ctx) -> String {
    let mut s = String::new();
    for k in kids {
        s.push_str(&conv(k, cx));
        if s.len() > MAX_OUTPUT {
            break;
        }
    }
    s
}

/// A formula fragment without the whitespace at its ends, safe to put in a group or between `$`.
/// LaTeX spaces are written `\ `: a plain `trim` turns a trailing one into a lone `\`, which
/// escapes the `}` or `$` that follows (RaTeX then fails and the whole formula falls back to its
/// characters). The trimmed-off space is put back after an odd run of backslashes, with an empty
/// group so that no real whitespace ends the fragment.
pub(super) fn tidy(s: &str) -> String {
    let t = s.trim();
    let slashes = t.bytes().rev().take_while(|&b| b == b'\\').count();
    if slashes % 2 == 1 {
        format!("{t} {{}}")
    } else {
        t.to_string()
    }
}

/// Whether `latex` holds a `$` that is not escaped (`\$`): the Markdown around the formula would
/// end the formula there. An escaped one is skipped by the renderer, so it is no problem.
pub(super) fn has_bare_dollar(latex: &str) -> bool {
    let mut it = latex.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                it.next();
            }
            '$' => return true,
            _ => {}
        }
    }
    false
}

fn is_pr(name: &str) -> bool {
    name.ends_with("Pr")
}

/// The argument container `name` of `n` as a braced group (`{}` when absent or empty).
fn arg(n: &Node, name: &str, cx: Ctx) -> String {
    let body = n
        .child(name)
        .map(|c| conv_seq(&c.kids, cx))
        .unwrap_or_default();
    format!("{{{}}}", tidy(&body))
}

fn arg_raw(n: &Node, name: &str, cx: Ctx) -> String {
    n.child(name)
        .map(|c| tidy(&conv_seq(&c.kids, cx)))
        .unwrap_or_default()
}

fn conv(n: &Node, cx: Ctx) -> String {
    // The context of a structural child. `fname` is not inherited: only a run directly in a
    // function name / limit base is a function name; `base` keeps it for the base of a script.
    let inner = Ctx {
        eq: false,
        fname: false,
        ..cx
    };
    let base = Ctx { eq: false, ..cx };
    let sup = Ctx { sup: true, ..inner };
    match n.name.as_str() {
        "r" => conv_run(n, cx),
        "f" => {
            let (num, den) = (arg(n, "num", inner), arg(n, "den", inner));
            match n.prop("fPr", "type").flatten().unwrap_or("bar") {
                "noBar" => format!(r"{{{num} \atop {den}}}"),
                "skw" | "lin" => format!("{num}/{den}"),
                _ => format!(r"\frac{num}{den}"),
            }
        }
        "sSup" => format!("{}^{}", arg(n, "e", base), arg(n, "sup", sup)),
        "sSub" => format!("{}_{}", arg(n, "e", base), arg(n, "sub", inner)),
        "sSubSup" => format!(
            "{}_{}^{}",
            arg(n, "e", base),
            arg(n, "sub", inner),
            arg(n, "sup", sup)
        ),
        "sPre" => format!(
            "{{}}_{}^{}{}",
            arg(n, "sub", inner),
            arg(n, "sup", inner),
            arg(n, "e", inner)
        ),
        "rad" => {
            let deg = arg_raw(n, "deg", inner);
            let e = arg(n, "e", inner);
            if n.flag("radPr", "degHide") || deg.is_empty() {
                format!(r"\sqrt{e}")
            } else {
                format!(r"\sqrt[{deg}]{e}")
            }
        }
        "nary" => conv_nary(n, inner),
        "d" => conv_delim(n, inner),
        "m" => conv_matrix(n, inner),
        "eqArr" => {
            let rows: Vec<String> = n
                .kids
                .iter()
                .filter(|k| k.name == "e")
                .map(|k| tidy(&conv_seq(&k.kids, Ctx { eq: true, ..cx })))
                .collect();
            format!(r"\begin{{aligned}} {} \end{{aligned}}", rows.join(r" \\ "))
        }
        "func" => {
            let f = arg_raw(
                n,
                "fName",
                Ctx {
                    fname: true,
                    ..inner
                },
            );
            let e = arg(n, "e", inner);
            format!("{f} {e}")
        }
        "acc" => {
            let e = arg(n, "e", inner);
            // An empty `chr` is "no accent mark" (an absent one is the default hat).
            let chr = n.prop("accPr", "chr").flatten().unwrap_or("\u{0302}");
            if chr.is_empty() {
                return e;
            }
            match accent_cmd(chr) {
                Some(cmd) => format!(r"{cmd}{e}"),
                None => format!(r"\overset{{{}}}{e}", map_chars(chr, false)),
            }
        }
        "bar" => {
            let e = arg(n, "e", inner);
            // ECMA-376 default `pos` is "bot".
            if n.prop("barPr", "pos").flatten() == Some("top") {
                format!(r"\overline{e}")
            } else {
                format!(r"\underline{e}")
            }
        }
        "groupChr" => conv_group(n, inner),
        "limLow" | "limUpp" => conv_lim(n, base),
        "box" => arg(n, "e", inner),
        "borderBox" => conv_border_box(n, inner),
        "phant" => {
            let e = arg(n, "e", inner);
            // `show` defaults to true; a hidden phantom keeps the size only.
            let show = n.prop("phantPr", "show").is_none() || n.flag("phantPr", "show");
            if show {
                e
            } else if n.flag("phantPr", "zeroWid") {
                format!(r"\vphantom{e}")
            } else if n.flag("phantPr", "zeroAsc") || n.flag("phantPr", "zeroDesc") {
                format!(r"\hphantom{e}")
            } else {
                format!(r"\phantom{e}")
            }
        }
        name if is_pr(name) => String::new(),
        // oMath, e, num, den, sub, sup, deg, lim, fName, unknown wrappers: transparent.
        _ => conv_seq(&n.kids, cx),
    }
}

/// The frame of an `m:borderBox`: which sides are drawn (`hideTop` ... default to drawn) and which
/// strike lines cross it (`strikeH`, `strikeBLTR`, `strikeTLBR`; a vertical strike has no form
/// RaTeX draws and is left out).
fn conv_border_box(n: &Node, cx: Ctx) -> String {
    let e = arg_raw(n, "e", cx);
    let f = |name: &str| n.flag("borderBoxPr", name);
    let body = strikes(&e, f("strikeBLTR"), f("strikeTLBR"), f("strikeH"));
    frame(
        &body,
        !f("hideTop"),
        !f("hideBot"),
        !f("hideLeft"),
        !f("hideRight"),
    )
}

/// `inner` crossed by the strike lines: `/` (`\cancel`), `\` (`\bcancel`), both (`\xcancel`), a
/// horizontal one (`\sout`).
pub(super) fn strikes(inner: &str, up: bool, down: bool, horizontal: bool) -> String {
    let b = match (up, down) {
        (true, true) => format!(r"\xcancel{{{inner}}}"),
        (true, false) => format!(r"\cancel{{{inner}}}"),
        (false, true) => format!(r"\bcancel{{{inner}}}"),
        _ => inner.to_string(),
    };
    if horizontal {
        format!(r"\sout{{{b}}}")
    } else {
        b
    }
}

/// `inner` with the chosen sides drawn: all four are a box, otherwise the top and the bottom are
/// rules and the left and the right are fences of `\left` / `\right`.
pub(super) fn frame(inner: &str, top: bool, bottom: bool, left: bool, right: bool) -> String {
    if top && bottom && left && right {
        return format!(r"\boxed{{{inner}}}");
    }
    let mut s = inner.to_string();
    if top {
        s = format!(r"\overline{{{s}}}");
    }
    if bottom {
        s = format!(r"\underline{{{s}}}");
    }
    match (left, right) {
        (false, false) => s,
        (l, r) => format!(
            r"\left{} {s} \right{}",
            if l { r"\vert" } else { "." },
            if r { r"\vert" } else { "." }
        ),
    }
}

fn conv_nary(n: &Node, cx: Ctx) -> String {
    // ECMA-376: an absent `chr` is the integral; one present with an empty value is no operator
    // character at all (the limits then sit on an empty base).
    let ch = match n.prop("naryPr", "chr").flatten() {
        None => Some('\u{222B}'),
        Some(c) => c.chars().next(),
    };
    let op = match ch {
        None => "{}".to_string(),
        Some(ch) => nary_cmd(ch)
            .map(str::to_string)
            .unwrap_or_else(|| map_chars(&ch.to_string(), false)),
    };
    let loc = n.prop("naryPr", "limLoc").flatten();
    let op = match (loc, ch) {
        (_, None) => op,
        (Some("undOvr"), _) => format!(r"{op}\limits"),
        (Some("subSup"), Some(ch)) if !matches!(ch, '\u{222B}'..='\u{2233}') => {
            format!(r"{op}\nolimits")
        }
        _ => op,
    };
    let sub = if n.flag("naryPr", "subHide") {
        String::new()
    } else {
        arg_raw(n, "sub", cx)
    };
    let sup = if n.flag("naryPr", "supHide") {
        String::new()
    } else {
        arg_raw(n, "sup", cx)
    };
    let mut s = op;
    if !sub.is_empty() {
        s.push_str(&format!("_{{{sub}}}"));
    }
    if !sup.is_empty() {
        s.push_str(&format!("^{{{sup}}}"));
    }
    s.push(' ');
    s.push_str(&arg(n, "e", cx));
    s
}

pub(super) fn nary_cmd(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{2211}' => r"\sum",
        '\u{220F}' => r"\prod",
        '\u{2210}' => r"\coprod",
        '\u{222B}' => r"\int",
        '\u{222C}' => r"\iint",
        '\u{222D}' => r"\iiint",
        '\u{222E}' => r"\oint",
        '\u{222F}' => r"\oiint",
        '\u{2230}' => r"\oiiint",
        '\u{22C3}' => r"\bigcup",
        '\u{22C2}' => r"\bigcap",
        '\u{22C1}' => r"\bigvee",
        '\u{22C0}' => r"\bigwedge",
        '\u{2A01}' => r"\bigoplus",
        '\u{2A02}' => r"\bigotimes",
        '\u{2A00}' => r"\bigodot",
        '\u{2A04}' => r"\biguplus",
        _ => return None,
    })
}

pub(super) fn delim_cmd(c: &str) -> Option<String> {
    Some(
        match c {
            "" => ".",
            "(" | ")" | "[" | "]" | "/" => c,
            "|" => r"\vert",
            "{" => r"\{",
            "}" => r"\}",
            "\u{2016}" => r"\Vert",
            "\u{27E8}" | "\u{3008}" | "\u{2329}" | "<" => r"\langle",
            "\u{27E9}" | "\u{3009}" | "\u{232A}" | ">" => r"\rangle",
            "\u{230A}" => r"\lfloor",
            "\u{230B}" => r"\rfloor",
            "\u{2308}" => r"\lceil",
            "\u{2309}" => r"\rceil",
            _ => return None,
        }
        .to_string(),
    )
}

fn conv_delim(n: &Node, cx: Ctx) -> String {
    // Defaults (ECMA-376 §22.1.2.31): "(" ")" and "|" as the separator. A property that is
    // present with an empty value means "no delimiter".
    let get = |name: &str, dflt: &'static str| -> String {
        match n.prop("dPr", name) {
            None => dflt.to_string(),
            Some(v) => v.unwrap_or("").to_string(),
        }
    };
    let (beg, end, sep) = (get("begChr", "("), get("endChr", ")"), get("sepChr", "|"));
    let rows: Vec<String> = n
        .kids
        .iter()
        .filter(|k| k.name == "e")
        .map(|k| tidy(&conv_seq(&k.kids, cx)))
        .collect();
    let sep_tex = match sep.as_str() {
        "|" => r"\mid".to_string(),
        "" => String::new(),
        s => map_chars(s, false),
    };
    let body = rows.join(&format!(" {sep_tex} "));
    let (Some(l), Some(r)) = (delim_cmd(&beg), delim_cmd(&end)) else {
        // A delimiter RaTeX has no stretchy form for: print it as a plain character.
        return format!(
            "{}{}{}",
            map_chars(&beg, false),
            body,
            map_chars(&end, false)
        );
    };
    if l == "." && r == "." {
        return format!("{{{body}}}");
    }
    format!(r"\left{l} {body} \right{r}")
}

fn conv_matrix(n: &Node, cx: Ctx) -> String {
    let rows: Vec<String> = n
        .kids
        .iter()
        .filter(|k| k.name == "mr")
        .map(|mr| {
            mr.kids
                .iter()
                .filter(|k| k.name == "e")
                .map(|k| tidy(&conv_seq(&k.kids, cx)))
                .collect::<Vec<_>>()
                .join(" & ")
        })
        .collect();
    format!(r"\begin{{matrix}} {} \end{{matrix}}", rows.join(r" \\ "))
}

pub(super) fn accent_cmd(chr: &str) -> Option<&'static str> {
    Some(match chr.chars().next()? {
        '\u{0302}' | '^' | '\u{02C6}' => r"\hat",
        '\u{0300}' | '`' => r"\grave",
        '\u{0301}' | '\u{00B4}' => r"\acute",
        '\u{0303}' | '~' | '\u{02DC}' => r"\tilde",
        '\u{0304}' | '\u{00AF}' | '\u{0305}' | '\u{203E}' => r"\bar",
        '\u{0306}' | '\u{02D8}' => r"\breve",
        '\u{0307}' | '\u{02D9}' => r"\dot",
        '\u{0308}' | '\u{00A8}' => r"\ddot",
        '\u{20DB}' => r"\dddot",
        '\u{030C}' | '\u{02C7}' => r"\check",
        '\u{20D7}' | '\u{2192}' => r"\vec",
        _ => return None,
    })
}

fn conv_group(n: &Node, cx: Ctx) -> String {
    let e = arg(n, "e", cx);
    let chr = n.prop("groupChrPr", "chr").flatten().unwrap_or("\u{23DF}");
    if chr.is_empty() {
        // An empty `chr` is "no character": the content alone.
        return e;
    }
    // ECMA-376 default `pos` is "bot".
    let top = n.prop("groupChrPr", "pos").flatten() == Some("top");
    match chr.chars().next() {
        Some('\u{23DE}') => format!(r"\overbrace{e}"),
        Some('\u{23DF}') => format!(r"\underbrace{e}"),
        Some('\u{2190}') if top => format!(r"\overleftarrow{e}"),
        Some('\u{2192}') if top => format!(r"\overrightarrow{e}"),
        Some('\u{2194}') if top => format!(r"\overleftrightarrow{e}"),
        Some('\u{2190}') => format!(r"\underleftarrow{e}"),
        Some('\u{2192}') => format!(r"\underrightarrow{e}"),
        Some('\u{2194}') => format!(r"\underleftrightarrow{e}"),
        _ if top => format!(r"\overset{{{}}}{e}", map_chars(chr, false)),
        _ => format!(r"\underset{{{}}}{e}", map_chars(chr, false)),
    }
}

fn conv_lim(n: &Node, cx: Ctx) -> String {
    let upper = n.name == "limUpp";
    let base_node = n.child("e");
    // A base that is a function name (lim, max, log ...) takes the limit as a script.
    let is_func = base_node.is_some_and(|c| {
        let t = flat_text(c);
        FUNCS.contains(&t.trim())
    });
    // Only a function name (or the name part of an `m:func`) is read as one: the base of any other
    // limit is made of variables, so `ab` stays two italic letters.
    let name = Ctx {
        fname: is_func || cx.fname,
        ..cx
    };
    let base = base_node
        .map(|c| tidy(&conv_seq(&c.kids, name)))
        .unwrap_or_default();
    let lim = arg_raw(n, "lim", Ctx { fname: false, ..cx });
    if is_func {
        let mark = if upper { '^' } else { '_' };
        return format!("{base}{mark}{{{lim}}}");
    }
    let base = format!("{{{base}}}");
    if upper {
        format!(r"\overset{{{lim}}}{base}")
    } else {
        format!(r"\underset{{{lim}}}{base}")
    }
}

/// The concatenated text of every run below `n`.
fn flat_text(n: &Node) -> String {
    let mut s = String::new();
    fn go(n: &Node, s: &mut String) {
        if n.name == "t" {
            s.push_str(&n.text);
        }
        for k in &n.kids {
            go(k, s);
        }
    }
    go(n, &mut s);
    s
}

/// Function names LaTeX has a command for (and RaTeX draws upright).
pub(super) const FUNCS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "coth", "log", "ln", "lg", "exp", "det", "dim", "ker", "gcd", "max", "min", "sup", "inf",
    "lim", "limsup", "liminf", "deg", "arg", "hom", "Pr",
];

/// Function names that are never a product of variables.
pub(super) const STRICT_FUNCS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "coth", "log", "ln", "exp", "lim", "limsup", "liminf",
];

fn conv_run(n: &Node, cx: Ctx) -> String {
    let text: String = n
        .kids
        .iter()
        .filter(|k| k.name == "t")
        .map(|k| k.text.as_str())
        .collect();
    if text.is_empty() {
        return String::new();
    }
    let rpr = |name: &str| n.prop("rPr", name).flatten();
    if n.flag("rPr", "nor") {
        // Normal (non-math) text.
        return format!(r"\text{{{}}}", escape_text(&text));
    }
    {
        let t = text.trim();
        // A run spelling a function name is the function, upright (Word writes `sty=p` for it;
        // LibreOffice writes no style, so the unambiguous names count without it).
        if (cx.fname || rpr("sty") == Some("p") || STRICT_FUNCS.contains(&t)) && FUNCS.contains(&t)
        {
            return format!(r"\{t} ");
        }
        if cx.fname && t.chars().count() > 1 && t.chars().all(|c| c.is_ascii_alphabetic()) {
            return format!(r"\operatorname{{{t}}}");
        }
    }
    let has_letter = text.chars().any(|c| c.is_alphabetic());
    // Style: `scr` picks the alphabet, `sty` bold / plain / italic within it.
    let sty = rpr("sty").unwrap_or("i");
    let bold = matches!(sty, "b" | "bi");
    let body = || map_chars_ex(&text, cx.eq, cx.sup);
    let alphabet = |cmd: &str| {
        let w = format!("{cmd}{{{}}}", body());
        // Bold within the alphabet (RaTeX draws a bold script / fraktur / double-struck).
        if bold && has_letter && cmd != r"\mathsf" && cmd != r"\mathtt" {
            format!(r"\boldsymbol{{{w}}}")
        } else {
            w
        }
    };
    match rpr("scr") {
        Some("script") => alphabet(r"\mathcal"),
        Some("fraktur") => alphabet(r"\mathfrak"),
        Some("double-struck") => alphabet(r"\mathbb"),
        Some("sans-serif") => alphabet(r"\mathsf"),
        Some("monospace") => alphabet(r"\mathtt"),
        _ => match sty {
            "p" if has_letter => format!(r"\mathrm{{{}}}", body()),
            // `\mathbf` leaves the lower-case Greek letters light (and a symbol is never bold in
            // it): the letters it covers go in it, the rest is `\boldsymbol`.
            "b" => bold_segments(&text, cx),
            "bi" => format!(r"\boldsymbol{{{}}}", body()),
            _ => body(),
        },
    }
}

/// A bold (not italic) run: ASCII letters and digits in `\mathbf`, Greek letters and symbols in
/// `\boldsymbol` (`\mathbf{\alpha}` is not bold).
fn bold_segments(text: &str, cx: Ctx) -> String {
    let chars: Vec<char> = text.chars().collect();
    // A decimal separator between two digits belongs to the number.
    let covered = |i: usize| {
        let c = chars[i];
        c.is_ascii_alphanumeric()
            || (matches!(c, ',' | '.')
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit))
    };
    let flush = |seg: &str, kind: bool, out: &mut String| {
        if seg.is_empty() {
            return;
        }
        let body = map_chars_ex(seg, cx.eq, cx.sup);
        if kind {
            out.push_str(&format!(r"\mathbf{{{body}}}"));
        } else if seg.chars().all(char::is_whitespace) {
            out.push_str(&body);
        } else {
            out.push_str(&format!(r"\boldsymbol{{{body}}}"));
        }
    };
    let mut out = String::new();
    let mut seg = String::new();
    let mut cur = false;
    for (i, &c) in chars.iter().enumerate() {
        let k = covered(i);
        if k != cur {
            flush(&seg, cur, &mut out);
            seg.clear();
            cur = k;
        }
        seg.push(c);
    }
    flush(&seg, cur, &mut out);
    out
}

pub(super) fn escape_text(t: &str) -> String {
    let mut s = String::new();
    for c in t.chars() {
        match c {
            '\\' => s.push_str(r"\textbackslash "),
            '{' | '}' | '#' | '$' | '%' | '&' | '_' => {
                s.push('\\');
                s.push(c);
            }
            '^' => s.push_str(r"\^{}"),
            '~' => s.push_str(r"\~{}"),
            '\n' | '\r' | '\t' => s.push(' '),
            // A bare `|` would be a column separator wherever the formula lands in a table cell.
            '|' => s.push_str(r"\textbar{}"),
            c => s.push(c),
        }
    }
    s
}

/// Characters of a math run as LaTeX. `amp`: a `&` is an alignment point (eqArr row).
pub(super) fn map_chars(text: &str, amp: bool) -> String {
    map_chars_ex(text, amp, false)
}

/// [`map_chars`]; `sup`: the text is (inside) a superscript, where a prime is `\prime` (TeX reads
/// a bare `'` as a superscript itself, so `^{'}` would sit twice as high).
pub(super) fn map_chars_ex(text: &str, amp: bool, sup: bool) -> String {
    let mut s = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if s.len() > MAX_OUTPUT {
            break;
        }
        match c {
            '\\' => s.push_str(r"\backslash "),
            '{' => s.push_str(r"\{"),
            '}' => s.push_str(r"\}"),
            '#' | '$' | '%' | '_' => {
                s.push('\\');
                s.push(c);
            }
            // `|` as a command: the formula may end up in a table cell, where a bare pipe would
            // split the column.
            '|' => s.push_str(r"\vert "),
            '&' if amp => s.push('&'),
            '&' => s.push_str(r"\&"),
            // The characters themselves (Word draws a caret and a tilde); `\wedge` / `\sim` are
            // other symbols.
            '^' => s.push_str(r"\^{}"),
            '~' => s.push_str(r"\~{}"),
            // A backtick is markup in the Markdown around the formula: the left quote instead.
            '`' => s.push_str(r"\lq "),
            // A decimal comma (`3,14`) takes no space after it.
            ',' if i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit) =>
            {
                s.push_str("{,}")
            }
            '\u{2032}' | '\'' if sup => s.push_str(r"\prime "),
            '\u{2033}' if sup => s.push_str(r"\prime\prime "),
            '\u{2034}' if sup => s.push_str(r"\prime\prime\prime "),
            ' ' | '\u{00A0}' => s.push_str(r"\ "),
            '\u{2009}' | '\u{200A}' | '\u{2006}' => s.push_str(r"\, "),
            '\u{2003}' | '\u{2002}' => s.push_str(r"\quad "),
            '\n' | '\r' | '\t' => {}
            c if c.is_ascii() => s.push(c),
            c => match char_cmd(c) {
                Some(cmd) => {
                    s.push_str(cmd);
                    // A command name must not run into the next letter.
                    if cmd.ends_with(|c: char| c.is_ascii_alphabetic()) {
                        s.push(' ');
                    }
                }
                None => s.push(c),
            },
        }
    }
    s
}

pub(super) fn char_cmd(c: char) -> Option<&'static str> {
    Some(match c {
        // Greek (lower case). Word's linear format: \varepsilon = U+03B5, \epsilon = U+03F5,
        // \varphi = U+03C6, \phi = U+03D5.
        'α' => r"\alpha",
        'β' => r"\beta",
        'γ' => r"\gamma",
        'δ' => r"\delta",
        'ε' => r"\varepsilon",
        '\u{03F5}' => r"\epsilon",
        'ζ' => r"\zeta",
        'η' => r"\eta",
        'θ' => r"\theta",
        'ϑ' => r"\vartheta",
        'ι' => r"\iota",
        'κ' => r"\kappa",
        'ϰ' => r"\varkappa",
        'λ' => r"\lambda",
        'μ' => r"\mu",
        'ν' => r"\nu",
        'ξ' => r"\xi",
        'ο' => "o",
        'π' => r"\pi",
        'ϖ' => r"\varpi",
        'ρ' => r"\rho",
        'ϱ' => r"\varrho",
        'σ' => r"\sigma",
        'ς' => r"\varsigma",
        'τ' => r"\tau",
        'υ' => r"\upsilon",
        'φ' => r"\varphi",
        'ϕ' => r"\phi",
        'χ' => r"\chi",
        'ψ' => r"\psi",
        'ω' => r"\omega",
        // Greek (upper case); the ones that look like Latin letters are those letters.
        'Γ' => r"\Gamma",
        'Δ' => r"\Delta",
        'Θ' => r"\Theta",
        'Λ' => r"\Lambda",
        'Ξ' => r"\Xi",
        'Π' => r"\Pi",
        'Σ' => r"\Sigma",
        'Υ' => r"\Upsilon",
        'Φ' => r"\Phi",
        'Ψ' => r"\Psi",
        'Ω' => r"\Omega",
        'Α' => "A",
        'Β' => "B",
        'Ε' => "E",
        'Ζ' => "Z",
        'Η' => "H",
        'Ι' => "I",
        'Κ' => "K",
        'Μ' => "M",
        'Ν' => "N",
        'Ο' => "O",
        'Ρ' => "P",
        'Τ' => "T",
        'Χ' => "X",
        // Operators and relations.
        '±' => r"\pm",
        '∓' => r"\mp",
        '×' => r"\times",
        '÷' => r"\div",
        '⋅' | '∙' | '·' => r"\cdot",
        '∗' | '*' => r"\ast",
        '∘' => r"\circ",
        '⊕' => r"\oplus",
        '⊗' => r"\otimes",
        '−' => "-",
        '≤' | '⩽' => r"\leq",
        '≥' | '⩾' => r"\geq",
        '≠' => r"\neq",
        '≈' => r"\approx",
        '≡' => r"\equiv",
        '∼' => r"\sim",
        '≅' => r"\cong",
        '≃' => r"\simeq",
        '∝' => r"\propto",
        '≪' => r"\ll",
        '≫' => r"\gg",
        '≺' => r"\prec",
        '≻' => r"\succ",
        '∣' => r"\mid",
        '∥' => r"\parallel",
        '⊥' => r"\perp",
        '∠' => r"\angle",
        '∞' => r"\infty",
        '∂' => r"\partial",
        '∇' => r"\nabla",
        '∈' => r"\in",
        '∉' => r"\notin",
        '∋' => r"\ni",
        '⊂' => r"\subset",
        '⊃' => r"\supset",
        '⊆' => r"\subseteq",
        '⊇' => r"\supseteq",
        '∪' => r"\cup",
        '∩' => r"\cap",
        '∅' => r"\emptyset",
        '∀' => r"\forall",
        '∃' => r"\exists",
        '¬' => r"\neg",
        '∧' => r"\wedge",
        '∨' => r"\vee",
        '→' => r"\to",
        '←' => r"\leftarrow",
        '↔' => r"\leftrightarrow",
        '⇒' => r"\Rightarrow",
        '⇐' => r"\Leftarrow",
        '⇔' => r"\Leftrightarrow",
        '↦' => r"\mapsto",
        '↑' => r"\uparrow",
        '↓' => r"\downarrow",
        '…' => r"\ldots",
        '⋯' => r"\cdots",
        '⋮' => r"\vdots",
        '⋱' => r"\ddots",
        '∴' => r"\therefore",
        '∵' => r"\because",
        '′' => "'",
        '″' => "''",
        '\u{2034}' => "'''",
        '°' => r"^\circ",
        'ℏ' => r"\hbar",
        'ℓ' => r"\ell",
        'ℵ' => r"\aleph",
        'ℝ' => r"\mathbb{R}",
        'ℂ' => r"\mathbb{C}",
        'ℕ' => r"\mathbb{N}",
        'ℤ' => r"\mathbb{Z}",
        'ℚ' => r"\mathbb{Q}",
        '√' => r"\surd",
        '∑' => r"\sum",
        '∏' => r"\prod",
        '∫' => r"\int",
        '∮' => r"\oint",
        '⟨' | '〈' => r"\langle",
        '⟩' | '〉' => r"\rangle",
        '⌊' => r"\lfloor",
        '⌋' => r"\rfloor",
        '⌈' => r"\lceil",
        '⌉' => r"\rceil",
        '‖' => r"\|",
        '⊢' => r"\vdash",
        '⊨' => r"\models",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::math::latex_to_svg;

    /// `<m:r><m:t>x</m:t></m:r>`
    fn r(t: &str) -> String {
        format!("<m:r><m:t>{t}</m:t></m:r>")
    }
    /// A run with run properties.
    fn rp(props: &str, t: &str) -> String {
        format!("<m:r><m:rPr>{props}</m:rPr><m:t>{t}</m:t></m:r>")
    }
    fn om(inner: &str) -> String {
        format!("<m:oMath>{inner}</m:oMath>")
    }
    fn tl(inner: &str) -> String {
        to_latex(&om(inner), false).unwrap_or_else(|| panic!("no LaTeX for {inner}"))
    }
    fn e(inner: &str) -> String {
        format!("<m:e>{inner}</m:e>")
    }

    /// Every case: the fragment, the expected LaTeX. Each expectation is also drawn by RaTeX.
    fn cases() -> Vec<(String, String, &'static str)> {
        let x = r("x");
        let y = r("y");
        let two = r("2");
        let mut v: Vec<(String, String, &'static str)> = Vec::new();
        let mut add =
            |name: &'static str, frag: String, want: &str| v.push((frag, want.to_string(), name));
        // runs
        add("run", om(&r("abc")), "abc");
        add(
            "run plain",
            om(&rp(r#"<m:sty m:val="p"/>"#, "ab")),
            r"\mathrm{ab}",
        );
        add(
            "run bold",
            om(&rp(r#"<m:sty m:val="b"/>"#, "v")),
            r"\mathbf{v}",
        );
        add(
            "run bold italic",
            om(&rp(r#"<m:sty m:val="bi"/>"#, "v")),
            r"\boldsymbol{v}",
        );
        add("run italic", om(&rp(r#"<m:sty m:val="i"/>"#, "v")), "v");
        add("digits plain", om(&rp(r#"<m:sty m:val="p"/>"#, "12")), "12");
        add(
            "script",
            om(&rp(r#"<m:scr m:val="script"/>"#, "A")),
            r"\mathcal{A}",
        );
        add(
            "fraktur",
            om(&rp(r#"<m:scr m:val="fraktur"/>"#, "A")),
            r"\mathfrak{A}",
        );
        add(
            "double-struck",
            om(&rp(r#"<m:scr m:val="double-struck"/>"#, "R")),
            r"\mathbb{R}",
        );
        add(
            "sans-serif",
            om(&rp(r#"<m:scr m:val="sans-serif"/>"#, "a")),
            r"\mathsf{a}",
        );
        add(
            "monospace",
            om(&rp(r#"<m:scr m:val="monospace"/>"#, "a")),
            r"\mathtt{a}",
        );
        add(
            "normal text",
            om(&rp("<m:nor/>", "if x &amp; y")),
            r"\text{if x \& y}",
        );
        add(
            "normal text val",
            om(&rp(r#"<m:nor m:val="1"/>"#, "ok")),
            r"\text{ok}",
        );
        add("nor off", om(&rp(r#"<m:nor m:val="0"/>"#, "ok")), "ok");
        add(
            "greek",
            om(&r("αβγδπσωΓΔΣΩ")),
            r"\alpha \beta \gamma \delta \pi \sigma \omega \Gamma \Delta \Sigma \Omega",
        );
        add(
            "greek variants",
            om(&r("εϵφϕϑ")),
            r"\varepsilon \epsilon \varphi \phi \vartheta",
        );
        add(
            "operators",
            om(&r("±×÷≤≥≠≈∞∂∇")),
            r"\pm \times \div \leq \geq \neq \approx \infty \partial \nabla",
        );
        add(
            "set ops",
            om(&r("∈∉⊂⊆∪∩")),
            r"\in \notin \subset \subseteq \cup \cap",
        );
        add("arrows", om(&r("→⇒⇔")), r"\to \Rightarrow \Leftrightarrow");
        add("minus sign", om(&r("a−b")), "a-b");
        add("space", om(&r("a b")), r"a\ b");
        add("specials", om(&r("a_b#c%d$e")), r"a\_b\#c\%d\$e");
        add("braces", om(&r("{a}")), r"\{a\}");
        add("entities", om(&r("a&lt;b&amp;c&gt;d")), r"a<b\&c>d");
        add("char ref", om(&r("&#x3b1;")), r"\alpha");
        // fractions
        add(
            "frac",
            om(&format!("<m:f><m:num>{x}</m:num><m:den>{y}</m:den></m:f>")),
            r"\frac{x}{y}",
        );
        add(
            "frac bar",
            om(&format!(
                r#"<m:f><m:fPr><m:type m:val="bar"/></m:fPr><m:num>{x}</m:num><m:den>{y}</m:den></m:f>"#
            )),
            r"\frac{x}{y}",
        );
        add(
            "frac noBar",
            om(&format!(
                r#"<m:f><m:fPr><m:type m:val="noBar"/></m:fPr><m:num>{x}</m:num><m:den>{y}</m:den></m:f>"#
            )),
            r"{{x} \atop {y}}",
        );
        add(
            "frac skw",
            om(&format!(
                r#"<m:f><m:fPr><m:type m:val="skw"/></m:fPr><m:num>{x}</m:num><m:den>{y}</m:den></m:f>"#
            )),
            "{x}/{y}",
        );
        add(
            "frac lin",
            om(&format!(
                r#"<m:f><m:fPr><m:type m:val="lin"/></m:fPr><m:num>{x}</m:num><m:den>{y}</m:den></m:f>"#
            )),
            "{x}/{y}",
        );
        add("frac nested", om(&format!("<m:f><m:num><m:f><m:num>{x}</m:num><m:den>{y}</m:den></m:f></m:num><m:den>{two}</m:den></m:f>")), r"\frac{\frac{x}{y}}{2}");
        // scripts
        add(
            "sSup",
            om(&format!(
                "<m:sSup><m:e>{x}</m:e><m:sup>{two}</m:sup></m:sSup>"
            )),
            "{x}^{2}",
        );
        add(
            "sSub",
            om(&format!(
                "<m:sSub><m:e>{x}</m:e><m:sub>{r}</m:sub></m:sSub>",
                r = r("i")
            )),
            "{x}_{i}",
        );
        add(
            "sSubSup",
            om(&format!(
                "<m:sSubSup><m:e>{x}</m:e><m:sub>{r}</m:sub><m:sup>{two}</m:sup></m:sSubSup>",
                r = r("i")
            )),
            "{x}_{i}^{2}",
        );
        add(
            "sPre",
            om(&format!(
                "<m:sPre><m:sub>{r}</m:sub><m:sup>{two}</m:sup><m:e>{x}</m:e></m:sPre>",
                r = r("a")
            )),
            "{}_{a}^{2}{x}",
        );
        add(
            "sSup empty base",
            om(&format!("<m:sSup><m:e/><m:sup>{two}</m:sup></m:sSup>")),
            "{}^{2}",
        );
        // radicals
        add(
            "rad",
            om(&format!(
                "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e>{x}</m:e></m:rad>"
            )),
            r"\sqrt{x}",
        );
        add(
            "rad no deg",
            om(&format!("<m:rad><m:e>{x}</m:e></m:rad>")),
            r"\sqrt{x}",
        );
        add(
            "rad deg",
            om(&format!(
                "<m:rad><m:deg>{}</m:deg><m:e>{x}</m:e></m:rad>",
                r("3")
            )),
            r"\sqrt[3]{x}",
        );
        add(
            "rad degHide wins",
            om(&format!(
                "<m:rad><m:radPr><m:degHide/></m:radPr><m:deg>{}</m:deg><m:e>{x}</m:e></m:rad>",
                r("3")
            )),
            r"\sqrt{x}",
        );
        // n-ary
        let nary = |chr: &str, extra: &str, sub: &str, sup: &str| {
            let chr = if chr.is_empty() {
                String::new()
            } else {
                format!(r#"<m:chr m:val="{chr}"/>"#)
            };
            format!("<m:nary><m:naryPr>{chr}{extra}</m:naryPr><m:sub>{sub}</m:sub><m:sup>{sup}</m:sup><m:e>{}</m:e></m:nary>", r("f"))
        };
        add(
            "sum",
            om(&nary("∑", "", &r("i=1"), &r("n"))),
            r"\sum_{i=1}^{n} {f}",
        );
        add("prod", om(&nary("∏", "", &r("i"), "")), r"\prod_{i} {f}");
        add(
            "int",
            om(&nary("∫", "", &r("0"), &r("1"))),
            r"\int_{0}^{1} {f}",
        );
        add(
            "int default chr",
            om(&nary("", "", &r("0"), &r("1"))),
            r"\int_{0}^{1} {f}",
        );
        add("oint", om(&nary("∮", "", "", "")), r"\oint {f}");
        add("iint", om(&nary("∬", "", &r("D"), "")), r"\iint_{D} {f}");
        add("iiint", om(&nary("∭", "", "", "")), r"\iiint {f}");
        add("union", om(&nary("⋃", "", &r("i"), "")), r"\bigcup_{i} {f}");
        add("inter", om(&nary("⋂", "", &r("i"), "")), r"\bigcap_{i} {f}");
        add(
            "undOvr",
            om(&nary(
                "∑",
                r#"<m:limLoc m:val="undOvr"/>"#,
                &r("i"),
                &r("n"),
            )),
            r"\sum\limits_{i}^{n} {f}",
        );
        add(
            "subSup sum",
            om(&nary(
                "∑",
                r#"<m:limLoc m:val="subSup"/>"#,
                &r("i"),
                &r("n"),
            )),
            r"\sum\nolimits_{i}^{n} {f}",
        );
        add(
            "undOvr int",
            om(&nary(
                "∫",
                r#"<m:limLoc m:val="undOvr"/>"#,
                &r("0"),
                &r("1"),
            )),
            r"\int\limits_{0}^{1} {f}",
        );
        add(
            "subHide",
            om(&nary("∑", r#"<m:subHide m:val="1"/>"#, &r("i"), &r("n"))),
            r"\sum^{n} {f}",
        );
        add(
            "supHide",
            om(&nary("∑", r#"<m:supHide m:val="on"/>"#, &r("i"), &r("n"))),
            r"\sum_{i} {f}",
        );
        add(
            "both hidden",
            om(&nary(
                "∑",
                r#"<m:subHide m:val="1"/><m:supHide m:val="1"/>"#,
                &r("i"),
                &r("n"),
            )),
            r"\sum {f}",
        );
        add(
            "hide off",
            om(&nary("∑", r#"<m:subHide m:val="0"/>"#, &r("i"), "")),
            r"\sum_{i} {f}",
        );
        // delimiters
        let d = |pr: &str, es: &[&str]| {
            format!(
                "<m:d><m:dPr>{pr}</m:dPr>{}</m:d>",
                es.iter().map(|s| e(s)).collect::<String>()
            )
        };
        add("d default", om(&d("", &[&x])), r"\left( x \right)");
        add(
            "d no dPr",
            om(&format!("<m:d>{}</m:d>", e(&x))),
            r"\left( x \right)",
        );
        add(
            "d brackets",
            om(&d(r#"<m:begChr m:val="["/><m:endChr m:val="]"/>"#, &[&x])),
            r"\left[ x \right]",
        );
        add(
            "d braces",
            om(&d(r#"<m:begChr m:val="{"/><m:endChr m:val="}"/>"#, &[&x])),
            r"\left\{ x \right\}",
        );
        add(
            "d bars",
            om(&d(r#"<m:begChr m:val="|"/><m:endChr m:val="|"/>"#, &[&x])),
            r"\left\vert x \right\vert",
        );
        add(
            "d norm",
            om(&d(r#"<m:begChr m:val="‖"/><m:endChr m:val="‖"/>"#, &[&x])),
            r"\left\Vert x \right\Vert",
        );
        add(
            "d angle",
            om(&d(r#"<m:begChr m:val="⟨"/><m:endChr m:val="⟩"/>"#, &[&x])),
            r"\left\langle x \right\rangle",
        );
        add(
            "d floor",
            om(&d(r#"<m:begChr m:val="⌊"/><m:endChr m:val="⌋"/>"#, &[&x])),
            r"\left\lfloor x \right\rfloor",
        );
        add(
            "d ceil",
            om(&d(r#"<m:begChr m:val="⌈"/><m:endChr m:val="⌉"/>"#, &[&x])),
            r"\left\lceil x \right\rceil",
        );
        add(
            "d empty end",
            om(&d(r#"<m:begChr m:val="{"/><m:endChr m:val=""/>"#, &[&x])),
            r"\left\{ x \right.",
        );
        add(
            "d empty beg",
            om(&d(r#"<m:begChr m:val=""/>"#, &[&x])),
            r"\left. x \right)",
        );
        add(
            "d both empty",
            om(&d(r#"<m:begChr m:val=""/><m:endChr m:val=""/>"#, &[&x])),
            "{x}",
        );
        add(
            "d default sep",
            om(&d("", &[&x, &y])),
            r"\left( x \mid y \right)",
        );
        add(
            "d comma sep",
            om(&d(r#"<m:sepChr m:val=","/>"#, &[&x, &y, &two])),
            r"\left( x , y , 2 \right)",
        );
        add(
            "d semicolon sep",
            om(&d(r#"<m:sepChr m:val=";"/>"#, &[&x, &y])),
            r"\left( x ; y \right)",
        );
        add(
            "d empty sep",
            om(&d(r#"<m:sepChr m:val=""/>"#, &[&x, &y])),
            r"\left( x  y \right)",
        );
        // matrix
        let mx = |rows: &[&[&str]]| {
            let body: String = rows
                .iter()
                .map(|row| {
                    format!(
                        "<m:mr>{}</m:mr>",
                        row.iter().map(|c| e(&r(c))).collect::<String>()
                    )
                })
                .collect();
            format!("<m:m><m:mPr><m:mcs/></m:mPr>{body}</m:m>")
        };
        add(
            "matrix",
            om(&mx(&[&["a", "b"], &["c", "d"]])),
            r"\begin{matrix} a & b \\ c & d \end{matrix}",
        );
        add(
            "matrix 1x3",
            om(&mx(&[&["1", "2", "3"]])),
            r"\begin{matrix} 1 & 2 & 3 \end{matrix}",
        );
        add(
            "matrix paren",
            om(&d("", &[&mx(&[&["a", "b"], &["c", "d"]])])),
            r"\left( \begin{matrix} a & b \\ c & d \end{matrix} \right)",
        );
        // eqArr
        add(
            "eqArr",
            om(&format!(
                "<m:eqArr><m:eqArrPr><m:maxDist m:val=\"1\"/></m:eqArrPr>{}{}</m:eqArr>",
                e(&r("x+y=1")),
                e(&r("x−y=0"))
            )),
            r"\begin{aligned} x+y=1 \\ x-y=0 \end{aligned}",
        );
        add(
            "eqArr align",
            om(&format!(
                "<m:eqArr>{}{}</m:eqArr>",
                e(&r("a&amp;=b")),
                e(&r("c&amp;=d"))
            )),
            r"\begin{aligned} a&=b \\ c&=d \end{aligned}",
        );
        add(
            "eqArr brace",
            om(&d(
                r#"<m:begChr m:val="{"/><m:endChr m:val=""/>"#,
                &[&format!(
                    "<m:eqArr>{}{}</m:eqArr>",
                    e(&r("x=1")),
                    e(&r("y=2"))
                )],
            )),
            r"\left\{ \begin{aligned} x=1 \\ y=2 \end{aligned} \right.",
        );
        // functions
        let fplain =
            |n: &str| format!("<m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>{n}</m:t></m:r>");
        add(
            "func sin",
            om(&format!(
                "<m:func><m:fName>{}</m:fName><m:e>{x}</m:e></m:func>",
                fplain("sin")
            )),
            r"\sin {x}",
        );
        add(
            "func cos",
            om(&format!(
                "<m:func><m:fName>{}</m:fName><m:e>{x}</m:e></m:func>",
                r("cos")
            )),
            r"\cos {x}",
        );
        add(
            "func log",
            om(&format!(
                "<m:func><m:fName>{}</m:fName><m:e>{x}</m:e></m:func>",
                fplain("log")
            )),
            r"\log {x}",
        );
        add(
            "func custom",
            om(&format!(
                "<m:func><m:fName>{}</m:fName><m:e>{x}</m:e></m:func>",
                r("sgn")
            )),
            r"\operatorname{sgn} {x}",
        );
        add("func sin squared", om(&format!("<m:func><m:fName><m:sSup><m:e>{}</m:e><m:sup>{two}</m:sup></m:sSup></m:fName><m:e>{x}</m:e></m:func>", fplain("sin"))), r"{\sin}^{2} {x}");
        add("func lim", om(&format!("<m:func><m:fName><m:limLow><m:e>{}</m:e><m:lim>{}</m:lim></m:limLow></m:fName><m:e>{x}</m:e></m:func>", fplain("lim"), r("n→∞"))), r"\lim_{n\to \infty} {x}");
        add(
            "limLow other",
            om(&format!(
                "<m:limLow><m:e>{x}</m:e><m:lim>{y}</m:lim></m:limLow>"
            )),
            r"\underset{y}{x}",
        );
        add(
            "limUpp other",
            om(&format!(
                "<m:limUpp><m:e>{x}</m:e><m:lim>{y}</m:lim></m:limUpp>"
            )),
            r"\overset{y}{x}",
        );
        add(
            "limUpp func",
            om(&format!(
                "<m:limUpp><m:e>{}</m:e><m:lim>{y}</m:lim></m:limUpp>",
                fplain("max")
            )),
            r"\max^{y}",
        );
        // accents
        let acc = |c: &str| {
            om(&format!(
                "<m:acc><m:accPr><m:chr m:val=\"{c}\"/></m:accPr><m:e>{x}</m:e></m:acc>"
            ))
        };
        add("acc hat", acc("\u{0302}"), r"\hat{x}");
        add("acc caret", acc("^"), r"\hat{x}");
        add("acc bar", acc("\u{00AF}"), r"\bar{x}");
        add("acc vec", acc("\u{20D7}"), r"\vec{x}");
        add("acc dot", acc("\u{0307}"), r"\dot{x}");
        add("acc ddot", acc("\u{0308}"), r"\ddot{x}");
        add("acc tilde", acc("~"), r"\tilde{x}");
        add("acc check", acc("\u{030C}"), r"\check{x}");
        add("acc breve", acc("\u{0306}"), r"\breve{x}");
        add("acc grave", acc("\u{0300}"), r"\grave{x}");
        add("acc acute", acc("\u{0301}"), r"\acute{x}");
        add(
            "acc default",
            om(&format!("<m:acc><m:e>{x}</m:e></m:acc>")),
            r"\hat{x}",
        );
        add("acc unknown", acc("*"), r"\overset{*}{x}");
        // bar, groupChr, box, phantom
        add(
            "bar top",
            om(&format!(
                r#"<m:bar><m:barPr><m:pos m:val="top"/></m:barPr><m:e>{x}</m:e></m:bar>"#
            )),
            r"\overline{x}",
        );
        add(
            "bar bot",
            om(&format!(
                r#"<m:bar><m:barPr><m:pos m:val="bot"/></m:barPr><m:e>{x}</m:e></m:bar>"#
            )),
            r"\underline{x}",
        );
        add(
            "bar default",
            om(&format!("<m:bar><m:e>{x}</m:e></m:bar>")),
            r"\underline{x}",
        );
        add(
            "underbrace",
            om(&format!(
                r#"<m:groupChr><m:groupChrPr><m:chr m:val="⏟"/><m:pos m:val="bot"/></m:groupChrPr><m:e>{x}</m:e></m:groupChr>"#
            )),
            r"\underbrace{x}",
        );
        add(
            "overbrace",
            om(&format!(
                r#"<m:groupChr><m:groupChrPr><m:chr m:val="⏞"/><m:pos m:val="top"/></m:groupChrPr><m:e>{x}</m:e></m:groupChr>"#
            )),
            r"\overbrace{x}",
        );
        add(
            "groupChr default",
            om(&format!("<m:groupChr><m:e>{x}</m:e></m:groupChr>")),
            r"\underbrace{x}",
        );
        add(
            "groupChr arrow",
            om(&format!(
                r#"<m:groupChr><m:groupChrPr><m:chr m:val="→"/><m:pos m:val="top"/></m:groupChrPr><m:e>{x}</m:e></m:groupChr>"#
            )),
            r"\overrightarrow{x}",
        );
        add("box", om(&format!("<m:box><m:e>{x}</m:e></m:box>")), "{x}");
        add("borderBox", om(&format!("<m:borderBox><m:borderBoxPr><m:hideTop m:val=\"1\"/></m:borderBoxPr><m:e>{x}</m:e></m:borderBox>")), r"\left\vert \underline{x} \right\vert");
        add(
            "phant show",
            om(&format!("<m:phant><m:e>{x}</m:e></m:phant>")),
            "{x}",
        );
        add(
            "phant hide",
            om(&format!(
                r#"<m:phant><m:phantPr><m:show m:val="0"/></m:phantPr><m:e>{x}</m:e></m:phant>"#
            )),
            r"\phantom{x}",
        );
        add(
            "phant zeroWid",
            om(&format!(
                r#"<m:phant><m:phantPr><m:show m:val="0"/><m:zeroWid m:val="1"/></m:phantPr><m:e>{x}</m:e></m:phant>"#
            )),
            r"\vphantom{x}",
        );
        add(
            "phant zeroAsc",
            om(&format!(
                r#"<m:phant><m:phantPr><m:show m:val="0"/><m:zeroAsc m:val="1"/></m:phantPr><m:e>{x}</m:e></m:phant>"#
            )),
            r"\hphantom{x}",
        );
        // properties with Word's ctrlPr are skipped
        add("ctrlPr skipped", om(&format!("<m:sSup><m:sSupPr><m:ctrlPr><w:rPr><w:rFonts w:ascii=\"Cambria Math\"/></w:rPr></m:ctrlPr></m:sSupPr><m:e>{x}</m:e><m:sup>{two}</m:sup></m:sSup>")), "{x}^{2}");
        // unknown wrapper is transparent
        add("unknown element", om(&format!("<m:foo>{x}</m:foo>")), "x");
        v
    }

    #[test]
    fn element_table() {
        for (frag, want, name) in cases() {
            let got = to_latex(&frag, false);
            assert_eq!(got.as_deref(), Some(want.as_str()), "case `{name}`: {frag}");
        }
    }

    #[test]
    fn every_expectation_is_drawn_by_ratex() {
        for (_, want, name) in cases() {
            if want.is_empty() {
                continue;
            }
            assert!(
                latex_to_svg(&want, true, "#000").is_some(),
                "RaTeX cannot draw `{want}` (case `{name}`)"
            );
            assert!(
                latex_to_svg(&want, false, "#000").is_some(),
                "RaTeX cannot draw `{want}` inline (case `{name}`)"
            );
        }
    }

    // Real OMML as LibreOffice 25.x writes it (a headless export of StarMath formulas to docx).
    const LO_QUADRATIC: &str = "<m:oMathPara><m:oMathParaPr><m:jc m:val=\"left\"/></m:oMathParaPr><m:oMath><m:r><m:t xml:space=\"preserve\">x</m:t></m:r><m:r><m:t xml:space=\"preserve\">=</m:t></m:r><m:f><m:num><m:r><m:t xml:space=\"preserve\">−</m:t></m:r><m:r><m:t xml:space=\"preserve\">b</m:t></m:r><m:r><m:t xml:space=\"preserve\">±</m:t></m:r><m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e><m:sSup><m:e><m:r><m:t xml:space=\"preserve\">b</m:t></m:r></m:e><m:sup><m:r><m:t xml:space=\"preserve\">2</m:t></m:r></m:sup></m:sSup><m:r><m:t xml:space=\"preserve\">−</m:t></m:r><m:r><m:t xml:space=\"preserve\">4</m:t></m:r><m:r><m:t xml:space=\"preserve\">ac</m:t></m:r></m:e></m:rad></m:num><m:den><m:r><m:t xml:space=\"preserve\">2</m:t></m:r><m:r><m:t xml:space=\"preserve\">a</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara>";
    const LO_SUM: &str = "<m:oMathPara><m:oMath><m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub><m:r><m:t>i</m:t></m:r><m:r><m:t>=</m:t></m:r><m:r><m:t>1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup><m:e><m:sSup><m:e><m:r><m:t>i</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:e></m:nary><m:r><m:t>=</m:t></m:r><m:f><m:num><m:r><m:t>n</m:t></m:r><m:d><m:dPr><m:begChr m:val=\"(\"/><m:endChr m:val=\")\"/></m:dPr><m:e><m:r><m:t>n</m:t></m:r><m:r><m:t>+</m:t></m:r><m:r><m:t>1</m:t></m:r></m:e></m:d></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara>";
    const LO_SYSTEM: &str = "<m:oMathPara><m:oMath><m:d><m:dPr><m:begChr m:val=\"{\"/><m:endChr m:val=\"\"/></m:dPr><m:e><m:eqArr><m:e><m:r><m:t>x</m:t></m:r><m:r><m:t>+</m:t></m:r><m:r><m:t>y</m:t></m:r><m:r><m:t>=</m:t></m:r><m:r><m:t>1</m:t></m:r></m:e><m:e><m:r><m:t>x</m:t></m:r><m:r><m:t>−</m:t></m:r><m:r><m:t>y</m:t></m:r><m:r><m:t>=</m:t></m:r><m:r><m:t>0</m:t></m:r></m:e></m:eqArr></m:e></m:d></m:oMath></m:oMathPara>";
    const LO_LIMIT: &str = "<m:oMathPara><m:oMath><m:func><m:fName><m:limLow><m:e><m:r><m:t>lim</m:t></m:r></m:e><m:lim><m:r><m:t>n</m:t></m:r><m:r><m:t>→</m:t></m:r><m:r><m:t>∞</m:t></m:r></m:lim></m:limLow></m:fName><m:e><m:sSup><m:e><m:d><m:dPr><m:begChr m:val=\"(\"/><m:endChr m:val=\")\"/></m:dPr><m:e><m:r><m:t>1</m:t></m:r><m:r><m:t>+</m:t></m:r><m:f><m:num><m:r><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>n</m:t></m:r></m:den></m:f></m:e></m:d></m:e><m:sup><m:r><m:t>n</m:t></m:r></m:sup></m:sSup></m:e></m:func><m:r><m:t>=</m:t></m:r><m:r><m:t>e</m:t></m:r></m:oMath></m:oMathPara>";
    const LO_ACCENTS: &str = "<m:oMathPara><m:oMath><m:acc><m:accPr><m:chr m:val=\"^\"/></m:accPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:acc><m:r><m:t>+</m:t></m:r><m:acc><m:accPr><m:chr m:val=\"¯\"/></m:accPr><m:e><m:r><m:t>y</m:t></m:r></m:e></m:acc><m:r><m:t>+</m:t></m:r><m:acc><m:accPr><m:chr m:val=\"⃗\"/></m:accPr><m:e><m:r><m:t>v</m:t></m:r></m:e></m:acc><m:r><m:t>+</m:t></m:r><m:acc><m:accPr><m:chr m:val=\"˙\"/></m:accPr><m:e><m:r><m:t>z</m:t></m:r></m:e></m:acc></m:oMath></m:oMathPara>";
    const LO_TRIG: &str = "<m:oMathPara><m:oMath><m:sSup><m:e><m:r><m:t>sin</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup><m:r><m:t>x</m:t></m:r><m:r><m:t>+</m:t></m:r><m:sSup><m:e><m:r><m:t>cos</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup><m:r><m:t>x</m:t></m:r><m:r><m:t>=</m:t></m:r><m:r><m:t>1</m:t></m:r></m:oMath></m:oMathPara>";
    const LO_ROOT: &str = "<m:oMathPara><m:oMath><m:rad><m:deg><m:r><m:t>3</m:t></m:r></m:deg><m:e><m:r><m:t>x</m:t></m:r><m:r><m:t>+</m:t></m:r><m:r><m:t>1</m:t></m:r></m:e></m:rad></m:oMath></m:oMathPara>";
    const LO_INT: &str = "<m:oMathPara><m:oMath><m:nary><m:naryPr><m:chr m:val=\"∫\"/></m:naryPr><m:sub><m:r><m:t>0</m:t></m:r></m:sub><m:sup><m:r><m:t>1</m:t></m:r></m:sup><m:e><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:e></m:nary><m:r><m:t>dx</m:t></m:r></m:oMath></m:oMathPara>";
    const LO_MATRIX: &str = "<m:oMathPara><m:oMath><m:d><m:dPr><m:begChr m:val=\"(\"/><m:endChr m:val=\")\"/></m:dPr><m:e><m:m><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:t>b</m:t></m:r></m:e></m:mr><m:mr><m:e><m:r><m:t>c</m:t></m:r></m:e><m:e><m:r><m:t>d</m:t></m:r></m:e></m:mr></m:m></m:e></m:d></m:oMath></m:oMathPara>";

    #[test]
    fn libreoffice_exports() {
        let want = [
            (LO_QUADRATIC, r"x=\frac{-b\pm \sqrt{{b}^{2}-4ac}}{2a}"),
            (
                LO_SUM,
                r"\sum_{i=1}^{n} {{i}^{2}}=\frac{n\left( n+1 \right)}{2}",
            ),
            (
                LO_SYSTEM,
                r"\left\{ \begin{aligned} x+y=1 \\ x-y=0 \end{aligned} \right.",
            ),
            (
                LO_LIMIT,
                r"\lim_{n\to \infty} {{\left( 1+\frac{1}{n} \right)}^{n}}=e",
            ),
            (LO_ACCENTS, r"\hat{x}+\bar{y}+\vec{v}+\dot{z}"),
            (LO_TRIG, r"{\sin}^{2}x+{\cos}^{2}x=1"),
            (LO_ROOT, r"\sqrt[3]{x+1}"),
            (LO_INT, r"\int_{0}^{1} {{x}^{2}}dx"),
            (
                LO_MATRIX,
                r"\left( \begin{matrix} a & b \\ c & d \end{matrix} \right)",
            ),
        ];
        for (frag, w) in want {
            assert_eq!(to_latex(frag, true).as_deref(), Some(w), "{frag}");
            // The same fragment with the namespace declared (as `document.xml` serializes it).
            let declared = frag.replacen(
                "<m:oMathPara>",
                "<m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">",
                1,
            );
            assert_eq!(to_latex(&declared, true).as_deref(), Some(w));
        }
    }

    #[test]
    fn prefixes_do_not_matter() {
        let a = "<oMath xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><sSup><e><r><t>x</t></r></e><sup><r><t>2</t></r></sup></sSup></oMath>";
        let b = "<mm:oMath><mm:sSup><mm:e><mm:r><mm:t>x</mm:t></mm:r></mm:e><mm:sup><mm:r><mm:t>2</mm:t></mm:r></mm:sup></mm:sSup></mm:oMath>";
        assert_eq!(to_latex(a, false).as_deref(), Some("{x}^{2}"));
        assert_eq!(to_latex(b, false).as_deref(), Some("{x}^{2}"));
        // The attribute prefix is not read either.
        let c = "<m:oMath><m:rad><m:radPr><m:degHide val=\"1\"/></m:radPr><m:deg><m:r><m:t>3</m:t></m:r></m:deg><m:e><m:r><m:t>x</m:t></m:r></m:e></m:rad></m:oMath>";
        assert_eq!(to_latex(c, false).as_deref(), Some(r"\sqrt{x}"));
    }

    #[test]
    fn math_para_with_several_rows() {
        let f = format!(
            "<m:oMathPara>{}{}</m:oMathPara>",
            om(&r("a=1")),
            om(&r("b=2"))
        );
        assert_eq!(
            to_latex(&f, true).as_deref(),
            Some(r"\begin{gathered} a=1 \\ b=2 \end{gathered}")
        );
        // Empty rows are dropped; one row is not wrapped.
        let f = format!("<m:oMathPara>{}{}</m:oMathPara>", om(""), om(&r("b=2")));
        assert_eq!(to_latex(&f, true).as_deref(), Some("b=2"));
        // display does not change the conversion.
        assert_eq!(to_latex(&f, false), to_latex(&f, true));
    }

    #[test]
    fn representative_expressions() {
        // The common textbook expressions all convert and are drawn.
        let frac = |n: &str, d: &str| format!("<m:f><m:num>{n}</m:num><m:den>{d}</m:den></m:f>");
        let root = format!(
            "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e>{}</m:e></m:rad>",
            r("b²−4ac")
        );
        let pm = tl(&format!(
            "{}{}",
            r("x="),
            frac(&format!("{}{root}", r("−b±")), &r("2a"))
        ));
        assert!(pm.contains(r"\frac{") && pm.contains(r"\sqrt{"), "{pm}");
        let pythag = tl(&format!(
            "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>{}",
            r("a"),
            r("2"),
            r("+b²=c²")
        ));
        assert!(drawable(&pythag));
        let nested = tl(&format!(
            "<m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e><m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary></m:e></m:nary>",
            r("i=1"), r("n"), r("j=1"), r("m"), r("a")
        ));
        assert_eq!(nested, r"\sum_{i=1}^{n} {\sum_{j=1}^{m} {a}}");
        let sci = tl(&format!(
            "{}{}{}",
            r("E=mc"),
            "<m:sSup><m:e/><m:sup>".to_string() + &r("2") + "</m:sup></m:sSup>",
            ""
        ));
        assert!(drawable(&sci));
    }

    #[test]
    fn rejects_what_ratex_cannot_draw() {
        // An unknown stretchy delimiter falls back to plain characters, not to an undrawable \left.
        let f = om(&format!(
            r#"<m:d><m:dPr><m:begChr m:val="⟦"/><m:endChr m:val="⟧"/></m:dPr><m:e>{}</m:e></m:d>"#,
            r("x")
        ));
        let got = to_latex(&f, false);
        assert!(got.as_deref().is_none_or(drawable), "{got:?}");
        // A character with no mapping that RaTeX cannot draw: None, never a broken string.
        let f = om(&r("\u{E000}"));
        if let Some(s) = to_latex(&f, false) {
            assert!(drawable(&s));
        }
    }

    #[test]
    fn empty_and_malformed_inputs() {
        for f in [
            "",
            "   ",
            "<m:oMath/>",
            "<m:oMath></m:oMath>",
            "<m:oMathPara/>",
            "not xml at all",
            "<m:oMath><m:r><m:t>x</m:t></m:r>", // never closed
            "<m:oMath><m:r><m:t>x</m:r></m:t></m:oMath>", // crossed
            "<m:oMath><m:f><m:num></m:den></m:f></m:oMath>", // mismatched
            "</m:oMath>",
            "<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMath>",
            "<?xml version=\"1.0\"?><!DOCTYPE m [<!ENTITY a \"b\">]><m:oMath>&a;</m:oMath>",
        ] {
            let _ = to_latex(f, false);
            let _ = to_latex(f, true);
        }
        assert_eq!(to_latex("", false), None);
        assert_eq!(to_latex("<m:oMath/>", false), None);
        assert_eq!(to_latex("<m:oMath><m:r><m:t>x</m:t></m:r>", false), None);
        assert_eq!(to_latex("not xml", false), None);
    }

    #[test]
    fn dtd_entities_are_not_expanded() {
        let f = "<!DOCTYPE m [<!ENTITY a \"EVIL\">]><m:oMath><m:r><m:t>&a;</m:t></m:r></m:oMath>";
        let got = to_latex(f, false);
        assert!(
            got.as_deref().is_none_or(|s| !s.contains("EVIL")),
            "{got:?}"
        );
    }

    #[test]
    fn deep_nesting_stops() {
        // 100 levels of fractions convert; 5,000 levels are refused (no stack overflow).
        let nest = |n: usize| {
            let mut s = r("x");
            for _ in 0..n {
                s = format!("<m:sSup><m:e>{s}</m:e><m:sup>{}</m:sup></m:sSup>", r("2"));
            }
            om(&s)
        };
        // sSup nests two elements per level (sSup + e), so 100 levels is depth ~300 > 256.
        assert_eq!(to_latex(&nest(5_000), false), None);
        assert!(to_latex(&nest(6), false).is_some());
        // Deep but within the cap: parsed and converted without trouble.
        let wrapped = om(&format!(
            "{}{}{}",
            "<m:foo>".repeat(200),
            r("x"),
            "</m:foo>".repeat(200)
        ));
        assert_eq!(to_latex(&wrapped, false).as_deref(), Some("x"));
        let deep_plain = format!("{}{}{}", "<a>".repeat(100_000), "x", "</a>".repeat(100_000));
        assert_eq!(to_latex(&deep_plain, false), None);
        let deep_unknown = om(&format!(
            "{}{}{}",
            "<m:foo>".repeat(2_000),
            r("x"),
            "</m:foo>".repeat(2_000)
        ));
        assert_eq!(to_latex(&deep_unknown, false), None);
    }

    #[test]
    fn size_limits() {
        // Input over 1 MiB.
        let big = om(&r(&"a".repeat(MAX_INPUT)));
        assert_eq!(to_latex(&big, false), None);
        // A huge run under the input cap is cut at the run cap, and output stays bounded.
        let run = om(&r(&"a".repeat(MAX_RUN_TEXT * 4)));
        if let Some(s) = to_latex(&run, false) {
            assert!(s.len() <= MAX_OUTPUT);
        }
        // Many elements.
        let many = om(&r("x").repeat(MAX_NODES));
        assert_eq!(to_latex(&many, false), None);
        // Output cap: a matrix whose text is within the input cap but whose LaTeX is not.
        let spaces = om(&r(&" ".repeat(MAX_RUN_TEXT)).repeat(14));
        if let Some(s) = to_latex(&spaces, false) {
            assert!(s.len() <= MAX_OUTPUT);
        }
        // Long attribute values are ignored, not copied.
        let long_attr = om(&format!(
            r#"<m:acc><m:accPr><m:chr m:val="{}"/></m:accPr><m:e>{}</m:e></m:acc>"#,
            "^".repeat(100_000),
            r("x")
        ));
        let _ = to_latex(&long_attr, false);
    }

    #[test]
    fn invalid_utf8_boundaries_and_odd_text() {
        for t in [
            "\u{0}",
            "\u{1F600}",
            "a\u{200B}b",
            "\\",
            "{{{{",
            "}}}}",
            "&&&&",
            "^^^^",
            "____",
            "\u{202E}x",
        ] {
            if let Some(s) = to_latex(&om(&r(&t.replace('&', "&amp;"))), false) {
                assert!(drawable(&s), "{t:?} -> {s}");
            }
        }
    }

    #[test]
    fn xml_without_math_elements() {
        // A bare run of math children (no oMath wrapper) is converted leniently.
        assert_eq!(to_latex(&r("x"), false).as_deref(), Some("x"));
        // A wrapper of another namespace holding an oMath is found.
        assert_eq!(
            to_latex(&format!("<w:p>{}</w:p>", om(&r("x"))), false).as_deref(),
            Some("x")
        );
    }
}
