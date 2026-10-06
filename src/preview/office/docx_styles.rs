//! `word/styles.xml` and `word/numbering.xml` for the Word reader: what a paragraph's or run's style
//! implies (heading level, list membership, bold / italic / strike) and how a list label looks
//! (`1.`, `(a)`, `iv)`, `ア`, `①`, `第1条` ...).
//!
//! Everything here is read through `XmlReader` and is bounded: a style chain is followed at most
//! [`MAX_CHAIN`] steps (a cyclic `basedOn` stops at the first repeat), and counts of styles, lists
//! and levels are capped.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::BufRead;

use super::docx_xml::{read_element, Budget, Node, Tree};
use super::fmt_xlsx::{xml_err, XmlReader};
use super::OfficeError;
use quick_xml::events::Event;

/// Longest `basedOn` chain followed.
const MAX_CHAIN: usize = 32;
/// Most styles / abstract lists / list instances kept.
const MAX_ENTRIES: usize = 20_000;

/// Character formatting that Markdown can express. `None` = not set (inherit).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Fmt {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub strike: Option<bool>,
}

impl Fmt {
    /// `self` with every property `over` sets replaced.
    pub fn over(self, over: Fmt) -> Fmt {
        Fmt {
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            strike: over.strike.or(self.strike),
        }
    }

    /// Reads `w:b` / `w:i` / `w:strike` / `w:dstrike` from an `rPr`.
    pub fn from_rpr(rpr: &Node) -> Fmt {
        let strike = match (rpr.toggle("strike"), rpr.toggle("dstrike")) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), _) | (_, Some(false)) => Some(false),
            _ => None,
        };
        Fmt {
            bold: rpr.toggle("b"),
            italic: rpr.toggle("i"),
            strike,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct StyleDef {
    name: String,
    kind: String,
    based_on: Option<String>,
    outline: Option<u8>,
    /// `(numId, ilvl)`; `ilvl` is `None` when the style names only the list.
    num: Option<(u32, Option<u8>)>,
    fmt: Fmt,
}

/// What a paragraph style (with everything it is based on) amounts to.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParaStyle {
    /// `outlineLvl` + 1 (1..=9), from the style chain or its name.
    pub heading: Option<u8>,
    pub num: Option<(u32, Option<u8>)>,
    /// The ids of the styles of the chain (to match a list level's `pStyle`).
    pub ids: Vec<String>,
    pub fmt: Fmt,
    pub code: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Styles {
    defs: HashMap<String, StyleDef>,
}

impl Styles {
    pub fn parse(src: impl BufRead) -> Result<Styles, OfficeError> {
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        let mut defs: HashMap<String, StyleDef> = HashMap::new();
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::Eof => break,
                _ => continue,
            };
            if e.local_name().as_ref() != b"style" {
                continue;
            }
            let mut budget = Budget::new(20_000, 64 * 1024);
            let Tree::Ok(node) = read_element(&mut rd, &e, empty, &mut budget)? else {
                continue;
            };
            if defs.len() >= MAX_ENTRIES {
                break;
            }
            let Some(id) = node.attr("styleId").map(str::to_string) else {
                continue;
            };
            let mut d = StyleDef {
                kind: node.attr("type").unwrap_or("paragraph").to_string(),
                ..StyleDef::default()
            };
            d.name = node
                .child("name")
                .and_then(|n| n.attr("val"))
                .unwrap_or("")
                .to_string();
            d.based_on = node
                .child("basedOn")
                .and_then(|n| n.attr("val"))
                .map(str::to_string);
            if let Some(ppr) = node.child("pPr") {
                d.outline = ppr
                    .child("outlineLvl")
                    .and_then(|n| n.attr("val"))
                    .and_then(|v| v.trim().parse::<u8>().ok());
                d.num = ppr.child("numPr").and_then(num_pr);
            }
            if let Some(rpr) = node.child("rPr") {
                d.fmt = Fmt::from_rpr(rpr);
            }
            defs.insert(id, d);
        }
        Ok(Styles { defs })
    }

    /// The styles of the chain starting at `id`, most derived first (a cycle ends the chain).
    fn chain<'a>(&'a self, id: &str) -> Vec<(&'a str, &'a StyleDef)> {
        let mut out = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut cur = self.defs.get_key_value(id);
        while let Some((k, d)) = cur {
            if out.len() >= MAX_CHAIN || !seen.insert(k.as_str()) {
                break;
            }
            out.push((k.as_str(), d));
            cur = d
                .based_on
                .as_deref()
                .and_then(|b| self.defs.get_key_value(b));
        }
        out
    }

    /// The paragraph style `id` resolved. An unknown or absent id gives the empty style (but the
    /// id's own spelling is still tried as a heading name, e.g. `Heading1`).
    pub fn para(&self, id: Option<&str>) -> ParaStyle {
        let Some(id) = id else {
            return ParaStyle::default();
        };
        let chain = self.chain(id);
        let mut ps = ParaStyle::default();
        let mut fmt = Fmt::default();
        for (k, d) in chain.iter().rev() {
            fmt = fmt.over(d.fmt);
            let _ = k;
        }
        ps.fmt = fmt;
        ps.ids = chain.iter().map(|(k, _)| (*k).to_string()).collect();
        if chain.is_empty() {
            ps.ids.push(id.to_string());
        }
        let mut outline_found = false;
        for (_, d) in &chain {
            if let Some(o) = d.outline {
                // 9 = "body text" (an explicit "not a heading").
                ps.heading = (o < 9).then_some(o + 1);
                outline_found = true;
                break;
            }
        }
        if !outline_found {
            ps.heading = chain
                .iter()
                .find_map(|(_, d)| heading_from_name(&d.name))
                .or_else(|| heading_from_name(id));
        }
        ps.num = chain.iter().find_map(|(_, d)| d.num);
        ps.code = chain
            .iter()
            .any(|(k, d)| is_code_name(&d.name) || is_code_name(k));
        ps
    }

    /// Character style `id` resolved to its formatting.
    pub fn char_fmt(&self, id: &str) -> Fmt {
        let mut fmt = Fmt::default();
        for (_, d) in self.chain(id).iter().rev() {
            if d.kind == "character" || d.kind == "paragraph" {
                fmt = fmt.over(d.fmt);
            }
        }
        fmt
    }

    /// The numbering instance a style (named by `numStyleLink` / `styleLink`) points at.
    fn num_of_style(&self, id: &str) -> Option<u32> {
        self.chain(id).iter().find_map(|(_, d)| d.num.map(|n| n.0))
    }
}

fn num_pr(n: &Node) -> Option<(u32, Option<u8>)> {
    let id = n
        .child("numId")
        .and_then(|c| c.attr("val"))
        .and_then(|v| v.trim().parse::<u32>().ok())?;
    let lvl = n
        .child("ilvl")
        .and_then(|c| c.attr("val"))
        .and_then(|v| v.trim().parse::<u8>().ok())
        .map(|l| l.min(8));
    Some((id, lvl))
}

/// The `numPr` of a paragraph's direct properties.
pub(crate) fn direct_num(ppr: &Node) -> Option<(u32, Option<u8>)> {
    ppr.child("numPr").and_then(num_pr)
}

/// `heading 1`, `Heading1`, `見出し 1`, `標題 2` ... -> level (1..=9). `Title` is level 1.
pub(crate) fn heading_from_name(name: &str) -> Option<u8> {
    let n = name.trim().to_lowercase();
    if n == "title" {
        return Some(1);
    }
    for prefix in [
        "heading",
        "見出し",
        "標題",
        "标题",
        "제목",
        "überschrift",
        "titre",
        "título",
        "titolo",
    ] {
        if let Some(rest) = n.strip_prefix(prefix) {
            let digits: String = rest
                .trim()
                .chars()
                .filter_map(|c| match c {
                    '0'..='9' => Some(c),
                    // full-width digits
                    '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32),
                    _ => None,
                })
                .collect();
            if rest
                .trim()
                .chars()
                .all(|c| c.is_ascii_digit() || ('０'..='９').contains(&c))
                && !digits.is_empty()
            {
                if let Ok(v) = digits.parse::<u8>() {
                    if (1..=9).contains(&v) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn is_code_name(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "code"
            | "source code"
            | "sourcecode"
            | "preformatted text"
            | "html preformatted"
            | "htmlpreformatted"
            | "verbatim"
            | "plain text"
            | "code block"
            | "codeblock"
    )
}

// ---------------------------------------------------------------------------------------------
// numbering
// ---------------------------------------------------------------------------------------------

/// How a list level numbers its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LevelDef {
    pub start: u32,
    pub fmt: String,
    pub text: String,
    pub legal: bool,
    pub pstyle: Option<String>,
}

impl LevelDef {
    fn default_for(lvl: usize) -> LevelDef {
        LevelDef {
            start: 1,
            fmt: "decimal".into(),
            text: format!("%{}.", lvl + 1),
            legal: false,
            pstyle: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Abstract {
    levels: Vec<Option<LevelDef>>,
    style_link: Option<String>,
    num_style_link: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct NumInst {
    abstract_id: u32,
    start_override: HashMap<u8, u32>,
    level_override: HashMap<u8, LevelDef>,
}

#[derive(Debug, Default)]
pub(crate) struct Numbering {
    abstracts: HashMap<u32, Abstract>,
    nums: HashMap<u32, NumInst>,
}

/// Most characters of a level's text kept.
const MAX_LEVEL_TEXT_CHARS: usize = 64;
/// Most characters of a rendered label (`%1` repeated with long counters would otherwise multiply).
const MAX_LABEL_CHARS: usize = 128;

fn parse_level(l: &Node) -> LevelDef {
    let mut d = LevelDef {
        start: 1,
        fmt: "decimal".into(),
        text: String::new(),
        legal: false,
        pstyle: None,
    };
    if let Some(v) = l.child("start").and_then(|c| c.attr("val")) {
        d.start = v.trim().parse().unwrap_or(1);
    }
    if let Some(v) = l.child("numFmt").and_then(|c| c.attr("val")) {
        d.fmt = v.trim().chars().take(40).collect();
    }
    if let Some(v) = l.child("lvlText").and_then(|c| c.attr("val")) {
        // Level text is a few symbols and `%1`..`%9`; a real one is under 20 characters. The cap
        // keeps one tiny `lvlText` from becoming a label of tens of kilobytes on every paragraph.
        d.text = v.chars().take(MAX_LEVEL_TEXT_CHARS).collect();
    }
    d.legal = l.toggle("isLgl").unwrap_or(false);
    d.pstyle = l
        .child("pStyle")
        .and_then(|c| c.attr("val"))
        .map(str::to_string);
    d
}

impl Numbering {
    pub fn parse(src: impl BufRead) -> Result<Numbering, OfficeError> {
        let mut rd = XmlReader::new(src);
        let mut buf = Vec::new();
        let mut out = Numbering::default();
        loop {
            buf.clear();
            let ev = rd.read_event_into(&mut buf).map_err(xml_err)?;
            let (e, empty) = match ev {
                Event::Start(e) => (e.into_owned(), false),
                Event::Empty(e) => (e.into_owned(), true),
                Event::Eof => break,
                _ => continue,
            };
            let local = e.local_name().as_ref().to_vec();
            if local != b"abstractNum" && local != b"num" {
                continue;
            }
            let mut budget = Budget::new(20_000, 64 * 1024);
            let Tree::Ok(node) = read_element(&mut rd, &e, empty, &mut budget)? else {
                continue;
            };
            if out.abstracts.len() + out.nums.len() >= MAX_ENTRIES {
                break;
            }
            let Some(id) = node.attr("abstractNumId").or_else(|| node.attr("numId")) else {
                continue;
            };
            let Ok(id) = id.trim().parse::<u32>() else {
                continue;
            };
            if node.name == "abstractNum" {
                let mut a = Abstract {
                    levels: vec![None; 9],
                    style_link: node
                        .child("styleLink")
                        .and_then(|c| c.attr("val"))
                        .map(str::to_string),
                    num_style_link: node
                        .child("numStyleLink")
                        .and_then(|c| c.attr("val"))
                        .map(str::to_string),
                };
                for l in node.nodes().filter(|n| n.name == "lvl") {
                    let ilvl = l
                        .attr("ilvl")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if ilvl < 9 {
                        a.levels[ilvl] = Some(parse_level(l));
                    }
                }
                out.abstracts.insert(id, a);
            } else {
                let Some(aid) = node
                    .child("abstractNumId")
                    .and_then(|c| c.attr("val"))
                    .and_then(|v| v.trim().parse::<u32>().ok())
                else {
                    continue;
                };
                let mut inst = NumInst {
                    abstract_id: aid,
                    ..NumInst::default()
                };
                for o in node.nodes().filter(|n| n.name == "lvlOverride") {
                    let Some(ilvl) = o
                        .attr("ilvl")
                        .and_then(|v| v.trim().parse::<u8>().ok())
                        .filter(|&l| l < 9)
                    else {
                        continue;
                    };
                    if let Some(s) = o
                        .child("startOverride")
                        .and_then(|c| c.attr("val"))
                        .and_then(|v| v.trim().parse::<u32>().ok())
                    {
                        inst.start_override.insert(ilvl, s);
                    }
                    if let Some(l) = o.child("lvl") {
                        inst.level_override.insert(ilvl, parse_level(l));
                    }
                }
                out.nums.insert(id, inst);
            }
        }
        Ok(out)
    }

    /// The abstract list `num_id` uses (following `numStyleLink` a few steps).
    pub fn abstract_of(&self, num_id: u32, styles: &Styles) -> Option<u32> {
        let mut aid = self.nums.get(&num_id)?.abstract_id;
        for _ in 0..8 {
            let a = self.abstracts.get(&aid)?;
            if a.levels.iter().any(Option::is_some) {
                return Some(aid);
            }
            let link = a.num_style_link.as_deref().or(a.style_link.as_deref())?;
            let next = styles.num_of_style(link)?;
            let next_aid = self.nums.get(&next)?.abstract_id;
            if next_aid == aid {
                return Some(aid);
            }
            aid = next_aid;
        }
        Some(aid)
    }

    /// The definition of `lvl` for `num_id` (the instance's own override wins).
    pub fn level(&self, num_id: u32, abstract_id: u32, lvl: u8) -> Cow<'_, LevelDef> {
        if let Some(l) = self
            .nums
            .get(&num_id)
            .and_then(|n| n.level_override.get(&lvl))
        {
            return Cow::Borrowed(l);
        }
        match self
            .abstracts
            .get(&abstract_id)
            .and_then(|a| a.levels.get(usize::from(lvl)))
            .and_then(Option::as_ref)
        {
            Some(l) => Cow::Borrowed(l),
            None => Cow::Owned(LevelDef::default_for(usize::from(lvl))),
        }
    }

    /// The level of `abstract_id` whose `pStyle` names one of `ids` (a heading style that carries
    /// only `numId`).
    pub fn level_for_style(&self, abstract_id: u32, ids: &[String]) -> Option<u8> {
        let a = self.abstracts.get(&abstract_id)?;
        a.levels.iter().enumerate().find_map(|(i, l)| {
            let p = l.as_ref()?.pstyle.as_ref()?;
            ids.iter().any(|x| x == p).then_some(i as u8)
        })
    }

    /// `startOverride` of an instance at a level (it restarts the numbering the first time the
    /// instance is used).
    pub fn start_override(&self, num_id: u32, lvl: u8) -> Option<u32> {
        self.nums.get(&num_id)?.start_override.get(&lvl).copied()
    }

    pub fn has_num(&self, num_id: u32) -> bool {
        self.nums.contains_key(&num_id)
    }
}

// ---------------------------------------------------------------------------------------------
// label formatting
// ---------------------------------------------------------------------------------------------

/// A counter value in a Word `numFmt`. Unknown formats are decimal.
pub(crate) fn format_number(fmt: &str, n: u32) -> String {
    let n = n.min(999_999_999);
    match fmt {
        "decimal" | "cardinalText" | "ordinalText" | "hex" | "numberInDash" => {
            if fmt == "numberInDash" {
                format!("- {n} -")
            } else {
                n.to_string()
            }
        }
        "decimalZero" => format!("{n:02}"),
        "ordinal" => {
            let suffix = match (n % 10, n % 100) {
                (_, 11..=13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            };
            format!("{n}{suffix}")
        }
        "lowerLetter" => letters(n, b'a'),
        "upperLetter" => letters(n, b'A'),
        "lowerRoman" => roman(n).to_lowercase(),
        "upperRoman" => roman(n),
        "decimalFullWidth" | "decimalFullWidth2" => n
            .to_string()
            .chars()
            .map(|c| char::from_u32(c as u32 - '0' as u32 + '０' as u32).unwrap_or(c))
            .collect(),
        "decimalHalfWidth" => n.to_string(),
        "decimalEnclosedCircle" | "decimalEnclosedCircleChinese" => match n {
            1..=20 => char::from_u32(0x2460 + n - 1).map_or_else(|| n.to_string(), String::from),
            21..=35 => char::from_u32(0x3251 + n - 21).map_or_else(|| n.to_string(), String::from),
            36..=50 => char::from_u32(0x32B1 + n - 36).map_or_else(|| n.to_string(), String::from),
            _ => n.to_string(),
        },
        "decimalEnclosedParen" => match n {
            1..=20 => char::from_u32(0x2474 + n - 1).map_or_else(|| n.to_string(), String::from),
            _ => format!("({n})"),
        },
        "decimalEnclosedFullstop" => match n {
            1..=20 => char::from_u32(0x2488 + n - 1).map_or_else(|| n.to_string(), String::from),
            _ => format!("{n}."),
        },
        "ideographDigital" | "chineseCounting" | "japaneseDigitalTenThousand" => n
            .to_string()
            .chars()
            .map(|c| IDEOGRAPH_DIGITS[(c as u8 - b'0') as usize])
            .collect(),
        "japaneseCounting"
        | "chineseCountingThousand"
        | "taiwaneseCounting"
        | "taiwaneseCountingThousand"
        | "ideographTraditional"
        | "ideographLegalTraditional"
        | "chineseLegalSimplified"
        | "koreanCounting"
        | "koreanDigital"
        | "koreanLegal" => cjk_counting(n, false),
        "japaneseLegal" | "ideographLegal" => cjk_counting(n, true),
        "aiueo" | "aiueoFullWidth" => kana(n, AIUEO),
        "iroha" | "irohaFullWidth" => kana(n, IROHA),
        "ideographZodiac" | "ideographEnclosedCircle" => match n {
            1..=10 => char::from_u32(0x3220 + n - 1).map_or_else(|| n.to_string(), String::from),
            _ => n.to_string(),
        },
        _ => n.to_string(),
    }
}

const IDEOGRAPH_DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

const AIUEO: &str =
    "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン";
const IROHA: &str = "イロハニホヘトチリヌルヲワカヨタレソツネナラムウヰノオクヤマケフコエテアサキユメミシヱヒモセス";

/// Sequence labels over a fixed alphabet: after the last character they continue as two (`アア`).
fn kana(n: u32, alphabet: &str) -> String {
    let chars: Vec<char> = alphabet.chars().collect();
    if n == 0 || chars.is_empty() {
        return n.to_string();
    }
    let len = chars.len() as u32;
    let rep = (n - 1) / len + 1;
    let c = chars[((n - 1) % len) as usize];
    std::iter::repeat_n(c, rep.min(8) as usize).collect()
}

/// `a`, `b`, ..., `z`, `aa`, `bb`, ... -- Word repeats the letter (`aa`, `bbb`), it does not carry.
fn letters(n: u32, base: u8) -> String {
    if n == 0 {
        return "0".into();
    }
    let rep = (n - 1) / 26 + 1;
    let c = (base + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(c, rep.min(40) as usize).collect()
}

fn roman(n: u32) -> String {
    if n == 0 || n >= 4000 {
        return n.to_string();
    }
    const T: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut n = n;
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// Japanese / Chinese counting numerals: `十`, `十一`, `二十`, `百五`... (`legal`: 壱弐参拾).
fn cjk_counting(n: u32, legal: bool) -> String {
    let digits: [&str; 10] = if legal {
        ["零", "壱", "弐", "参", "四", "五", "六", "七", "八", "九"]
    } else {
        ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"]
    };
    let ten = if legal { "拾" } else { "十" };
    if n == 0 {
        return digits[0].to_string();
    }
    if n >= 10_000 {
        return n.to_string();
    }
    let mut s = String::new();
    let th = n / 1000;
    let h = (n / 100) % 10;
    let t = (n / 10) % 10;
    let o = n % 10;
    if th > 0 {
        if th > 1 {
            s.push_str(digits[th as usize]);
        }
        s.push('千');
    }
    if h > 0 {
        if h > 1 {
            s.push_str(digits[h as usize]);
        }
        s.push('百');
    }
    if t > 0 {
        if t > 1 {
            s.push_str(digits[t as usize]);
        }
        s.push_str(ten);
    }
    if o > 0 {
        s.push_str(digits[o as usize]);
    }
    s
}

/// Substitutes `%1`..`%9` in a level's text with the formatted counters of those levels. A bullet
/// level's text (a symbol, often a private-use code point of a symbol font) is not a label: the
/// caller shows a plain bullet.
pub(crate) fn render_label(text: &str, value_of: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if out.chars().count() >= MAX_LABEL_CHARS {
            break;
        }
        if c == '%' {
            if let Some(&d) = it.peek() {
                if let Some(k) = d.to_digit(10).filter(|k| (1..=9).contains(k)) {
                    it.next();
                    out.push_str(&value_of(k as usize - 1));
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}
