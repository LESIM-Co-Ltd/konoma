//! Text layout for slide shapes.
//!
//! [`layout`] turns a [`TextBody`] into positioned line boxes in pixels. The SVG writer draws
//! them; it never re-flows anything, so what this module decides is what the picture shows.
//!
//! # Measuring
//!
//! Widths come from [`super::fonts::measure`], i.e. from the face resvg will resolve for the
//! same font stack (see that module for the substitution table and its one approximation).
//! Every run is split by script (Latin / East Asian / complex) and each piece is measured with the
//! stack of its own typeface, like DrawingML's `latin` / `ea` / `cs` split.
//!
//! # Vertical metrics (approximation)
//!
//! A line's single height is `1.2 x` the largest font size on it; the baseline sits `0.93 x` size
//! below the top of a single-spaced line. The real ascent and descent of the face are not read
//! (PowerPoint's own values depend on the font's hhea/OS-2 tables; 1.2 is what Calibri and Arial
//! come close to). Percentage line spacing scales the height and puts the extra above the
//! baseline (as PowerPoint does); exact-point spacing puts the baseline at 80 % of the line.
//!
//! # Line breaking
//!
//! Greedy. Break opportunities: after a space, and between any two East-Asian characters or an
//! East-Asian character and a word. Basic kinsoku: a line never starts with a closing character
//! (`、。，．）」』】〕〉》！？ー` small kana ...) and never ends with an opening one
//! (`（「『【〔〈《`): the offending character moves to the other line together with its
//! neighbour. A word wider than the line is broken between characters; with `wrap = false`
//! nothing is broken except at forced breaks. Not done: hanging punctuation, hyphenation, bidi
//! reordering inside a line beyond reversing the atoms of right-to-left paragraphs.
//!
//! # Tabs
//!
//! A tab character goes to the next tab stop: the paragraph's own stops (left, centre, right;
//! decimal as left), the hanging-indent position, then the default stops (see `text_tabs`). A
//! tab at the start of a wrapped line is dropped like any space there; tabs in a distributed
//! paragraph keep their natural width.
//!
//! # Other approximations
//!
//! * Small caps: lower-case letters are drawn as capitals at 80 % size.
//! * A space-before on the first paragraph of a text body is ignored (as PowerPoint does).
//! * `lnSpcReduction` lowers percentage line spacing and percentage paragraph spacing; point
//!   spacings are left alone.
//! * Multiple columns are filled top to bottom with no gutter and always top-anchored.
//! * `normAutofit` uses the stored `fontScale` / `lnSpcReduction` and nothing else. A file without
//!   them (or with 100 % / 0 %) is drawn at full size even when the text overflows: PowerPoint
//!   recomputes the scales only while the text is being edited, not when a file is opened.
//! * `spAutoFit` (shape grows) does nothing: the text is laid out in the box it has.
//! * East-Asian vertical text keeps punctuation where it is (no vertical glyph forms) and
//!   rotates Latin words a quarter turn clockwise.
//! * Wingdings / Symbol bullet characters are mapped to Unicode look-alikes (the fonts do not
//!   exist here).

use std::collections::HashMap;
use std::sync::Arc;

use super::fonts::{self, Script};
#[path = "text_tabs.rs"]
mod tabs;
use super::model::{
    Align, Anchor, AutoFit, Bullet, BulletKind, BulletSize, Caps, Fill, FontSpec, ImageFill,
    Paragraph, Rgba, Run, RunKind, Spacing, Strike, TextBody, Underline, Vert, EMU_PER_PX,
};

/// Pixels per point at 96 dpi.
pub const PX_PER_PT: f64 = 96.0 / 72.0;
/// Single line height as a multiple of the font size.
pub const LINE_HEIGHT: f64 = 1.2;
/// Distance of the baseline below the top of a single-spaced line, as a multiple of the size.
pub const ASCENT: f64 = 0.93;
/// Most characters laid out in one text body; the rest is dropped and the layout says so.
pub const MAX_TEXT_CHARS: usize = 200_000;
/// Most lines produced for one text body.
pub const MAX_LINES: usize = 20_000;

/// The style of a piece of drawn text.
#[derive(Debug, Clone, PartialEq)]
pub struct FragStyle {
    /// CSS `font-family` value (not XML-escaped).
    pub family: Arc<str>,
    pub size_px: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub strike: Strike,
    pub color: Rgba,
    pub highlight: Option<Rgba>,
    /// Extra advance after each character, px.
    pub spacing_px: f64,
}

/// A piece of text drawn at one position with one style.
#[derive(Debug, Clone, PartialEq)]
pub struct Frag {
    pub text: String,
    /// Start of the text on its baseline, px, in the layout frame.
    pub x: f64,
    pub y: f64,
    /// Advance width, px.
    pub width: f64,
    pub style: FragStyle,
    /// Drawn rotated a quarter turn clockwise about `(x, y)` (Latin words in East-Asian vertical
    /// text).
    pub rot90: bool,
}

/// One laid-out line.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    /// Top of the line box and its height, px.
    pub top: f64,
    pub height: f64,
    /// Baseline of the unshifted text, px.
    pub baseline: f64,
    pub frags: Vec<Frag>,
}

/// A bullet to draw.
#[derive(Debug, Clone, PartialEq)]
pub enum BulletDraw {
    Text(Frag),
    /// A picture bullet: `size` px square with its top-left corner at `(x, y)`.
    Picture {
        image: ImageFill,
        x: f64,
        y: f64,
        size: f64,
    },
}

/// How the layout frame relates to the shape's text rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// The frame is the text rectangle.
    Normal,
    /// The frame is rotated 90 degrees clockwise: its width is the rectangle's height.
    Rot90,
    /// Rotated 90 degrees counter-clockwise.
    Rot270,
}

/// The result of laying out a text body.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub frame: Frame,
    /// Size of the layout frame, px.
    pub frame_w: f64,
    pub frame_h: f64,
    pub lines: Vec<LineBox>,
    pub bullets: Vec<BulletDraw>,
    /// Extent of the text, px.
    pub content_w: f64,
    pub content_h: f64,
    /// Text was dropped because of [`MAX_TEXT_CHARS`] / [`MAX_LINES`].
    pub truncated: bool,
}

// ---------------------------------------------------------------------------------------------
// Atoms
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Space,
    Word,
    Cjk,
    Break,
}

#[derive(Debug, Clone)]
struct Atom {
    text: String,
    style: usize,
    kind: Kind,
    width: f64,
    no_start: bool,
    no_end: bool,
    /// A break is allowed before this atom whatever precedes it (pieces of an exploded word).
    force_break: bool,
    /// A tab character: a space whose width is set by the tab stops (see [`tabs`]).
    tab: bool,
}

#[derive(Debug, Clone)]
struct StyleEntry {
    style: FragStyle,
    stack: Vec<String>,
    /// Baseline shift (px, up positive) for super- and subscripts.
    shift: f64,
}

struct Ctx {
    styles: Vec<StyleEntry>,
    index: HashMap<(usize, Script, bool), usize>,
    font_scale: f64,
    chars: usize,
    truncated: bool,
}

const NO_START: &str = "、。，．）」』】〕〉》！？ーゝゞ々・：；）］｝｣､･ｰﾞﾟぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶ,.)]}!?:;%\u{2019}\u{201D}\u{3009}\u{300B}";
const NO_END: &str = "（「『【〔〈《([{｢\u{2018}\u{201C}［｛";

fn run_color(fill: &Fill) -> Rgba {
    match fill {
        Fill::Solid(c) => *c,
        Fill::Gradient(g) => g.stops.first().map(|s| s.1).unwrap_or(Rgba::BLACK),
        Fill::Pattern { fg, .. } => *fg,
        _ => Rgba::BLACK,
    }
}

fn font_name(f: &FontSpec, sc: Script) -> Option<&str> {
    match sc {
        Script::Latin => f.latin.as_deref(),
        Script::EastAsian => f.east_asian.as_deref().or(f.latin.as_deref()),
        Script::Complex => f.complex.as_deref().or(f.latin.as_deref()),
    }
}

/// Set in the style key of characters drawn in the symbol stand-in font (run keys never reach it).
const SYMBOL_KEY_BIT: usize = 1 << (usize::BITS - 2);

/// The font that draws characters taken from a symbol font.
const SYMBOL_STAND_IN: &str = "Arial Unicode MS";

/// The Unicode character a run shows for `c` when the font that applies to `c` is a symbol font:
/// a private-use code (`U+F020..=U+F0FF`) takes the `a:sym` font (else the Latin one), any other
/// character the Latin font (so 'p' in a `Symbol` run is a pi). `None` for other fonts.
fn symbol_run_char(f: &FontSpec, c: char) -> Option<char> {
    let pua = ('\u{F020}'..='\u{F0FF}').contains(&c);
    let font = if pua {
        f.symbol.as_deref().or(f.latin.as_deref())?
    } else {
        f.latin.as_deref()?
    };
    super::symbol_font::symbol_font(font)?;
    if !pua && !c.is_ascii_graphic() {
        return None;
    }
    Some(super::symbol_font::map(font, c))
}

fn clean_size(pt: f64, default_pt: f64) -> f64 {
    if pt.is_finite() && pt > 0.0 {
        pt.clamp(0.5, 3000.0)
    } else {
        default_pt
    }
}

impl Ctx {
    fn style_id(&mut self, run_key: usize, run: &Run, sc: Script, small: bool) -> usize {
        if let Some(&i) = self.index.get(&(run_key, sc, small)) {
            return i;
        }
        let base_px = clean_size(run.size_pt, 18.0) * PX_PER_PT * self.font_scale;
        let scripted = run.baseline_pct != 0.0 && run.baseline_pct.is_finite();
        let mut size_px = if scripted {
            base_px * 2.0 / 3.0
        } else {
            base_px
        };
        if small {
            size_px *= 0.8;
        }
        let size_px = size_px.clamp(0.5, 5000.0);
        let stack = fonts::stack_for(font_name(&run.font, sc), sc);
        let style = FragStyle {
            family: fonts::css_family(&stack).into(),
            size_px,
            bold: run.bold,
            italic: run.italic,
            underline: run.underline,
            strike: run.strike,
            color: run_color(&run.fill),
            highlight: run.highlight,
            spacing_px: if run.spacing_pt.is_finite() {
                (run.spacing_pt * PX_PER_PT).clamp(-50.0, 500.0)
            } else {
                0.0
            },
        };
        let shift = if scripted {
            run.baseline_pct.clamp(-200.0, 200.0) / 100.0 * base_px
        } else {
            0.0
        };
        self.styles.push(StyleEntry {
            style,
            stack,
            shift,
        });
        let id = self.styles.len() - 1;
        self.index.insert((run_key, sc, small), id);
        id
    }

    /// The style of a run's characters that were mapped out of a symbol font: the run's style in
    /// a font that has the look-alike Unicode characters.
    fn symbol_style_id(&mut self, run_key: usize, run: &Run, small: bool) -> usize {
        let mut r = run.clone();
        r.font.latin = Some(SYMBOL_STAND_IN.to_string());
        self.style_id(run_key | SYMBOL_KEY_BIT, &r, Script::Latin, small)
    }

    fn atom(&mut self, text: String, style: usize, kind: Kind) -> Atom {
        let e = &self.styles[style];
        let n = text.chars().count() as f64;
        let w = fonts::measure(
            &e.stack,
            e.style.bold,
            e.style.italic,
            &text,
            e.style.size_px,
        ) + n * e.style.spacing_px;
        let first = text.chars().next();
        let last = text.chars().last();
        Atom {
            no_start: first.is_some_and(|c| NO_START.contains(c)) && kind != Kind::Space,
            no_end: last.is_some_and(|c| NO_END.contains(c)),
            text,
            style,
            kind,
            width: w.max(0.0),
            force_break: false,
            tab: false,
        }
    }
}

/// Splits the runs of a paragraph into atoms.
fn build_atoms(para: &Paragraph, run_base: usize, ctx: &mut Ctx) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    for (ri, run) in para.runs.iter().enumerate() {
        let key = run_base + ri;
        if ctx.chars >= MAX_TEXT_CHARS {
            ctx.truncated = true;
            break;
        }
        if run.kind == RunKind::LineBreak {
            let st = ctx.style_id(key, run, Script::Latin, false);
            let mut a = ctx.atom(String::new(), st, Kind::Break);
            a.width = 0.0;
            out.push(a);
            continue;
        }
        // Group consecutive characters into (kind, script, small-caps) pieces.
        let mut buf = String::new();
        let mut cur: Option<(Kind, Script, bool, bool)> = None;
        let flush = |buf: &mut String,
                     cur: &mut Option<(Kind, Script, bool, bool)>,
                     out: &mut Vec<Atom>,
                     ctx: &mut Ctx| {
            if let Some((kind, sc, small, sym)) = cur.take() {
                if !buf.is_empty() {
                    let st = if sym {
                        ctx.symbol_style_id(key, run, small)
                    } else {
                        ctx.style_id(key, run, sc, small)
                    };
                    let a = ctx.atom(std::mem::take(buf), st, kind);
                    out.push(a);
                }
            }
            buf.clear();
        };
        for c0 in run.text.chars() {
            if ctx.chars >= MAX_TEXT_CHARS {
                ctx.truncated = true;
                break;
            }
            ctx.chars += 1;
            if c0 == '\n' || c0 == '\u{b}' {
                flush(&mut buf, &mut cur, &mut out, ctx);
                let st = ctx.style_id(key, run, Script::Latin, false);
                let mut a = ctx.atom(String::new(), st, Kind::Break);
                a.width = 0.0;
                out.push(a);
                continue;
            }
            if matches!(c0, '\r' | '\u{200b}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}')
                || (c0.is_control() && c0 != '\t')
            {
                continue;
            }
            if c0 == '\t' {
                flush(&mut buf, &mut cur, &mut out, ctx);
                let st = ctx.style_id(key, run, Script::Latin, false);
                let mut a = ctx.atom(" ".to_string(), st, Kind::Space);
                a.tab = true;
                out.push(a);
                continue;
            }
            let lower = c0.is_lowercase();
            let (c, small) = match run.caps {
                Caps::None => (c0, false),
                Caps::All => (c0.to_uppercase().next().unwrap_or(c0), false),
                Caps::Small => (c0.to_uppercase().next().unwrap_or(c0), lower),
            };
            // A character the run shows in a symbol font (Symbol, Wingdings, ...) is replaced by
            // the Unicode character of the same look and drawn in a font that has it.
            let mapped = symbol_run_char(&run.font, c);
            let sym = mapped.is_some();
            let c = mapped.unwrap_or(c);
            let (kind, sc) = if c == ' ' {
                (Kind::Space, Script::Latin)
            } else if sym {
                (Kind::Word, Script::Latin)
            } else {
                let sc = fonts::script_of(c);
                (
                    if sc == Script::EastAsian {
                        Kind::Cjk
                    } else {
                        Kind::Word
                    },
                    sc,
                )
            };
            let this = (kind, sc, small, sym);
            let same = cur == Some(this) && kind != Kind::Cjk;
            if !same {
                flush(&mut buf, &mut cur, &mut out, ctx);
                cur = Some(this);
            }
            buf.push(c);
        }
        flush(&mut buf, &mut cur, &mut out, ctx);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Line breaking
// ---------------------------------------------------------------------------------------------

struct Line {
    atoms: Vec<Atom>,
    /// Ended by a forced break (or is the last line of the paragraph).
    forced: bool,
}

fn allowed(prev: &Atom, cur: &Atom) -> bool {
    if prev.kind == Kind::Break || cur.kind == Kind::Break {
        return true;
    }
    if prev.kind == Kind::Space {
        return true;
    }
    if cur.kind == Kind::Space {
        return false;
    }
    if cur.force_break && !(cur.no_start || prev.no_end) {
        return true;
    }
    if prev.kind == Kind::Word && cur.kind == Kind::Word {
        return false;
    }
    !(cur.no_start || prev.no_end)
}

fn clusters(atoms: Vec<Atom>) -> std::collections::VecDeque<Vec<Atom>> {
    let mut out = std::collections::VecDeque::new();
    let mut cur: Vec<Atom> = Vec::new();
    for a in atoms {
        if let Some(last) = cur.last() {
            if allowed(last, &a) {
                out.push_back(std::mem::take(&mut cur));
            }
        }
        cur.push(a);
    }
    if !cur.is_empty() {
        out.push_back(cur);
    }
    out
}

fn trim_trailing_spaces(atoms: &mut Vec<Atom>) {
    let brk = atoms.last().is_some_and(|a| a.kind == Kind::Break);
    let tail = if brk { atoms.pop() } else { None };
    while atoms.last().is_some_and(|a| a.kind == Kind::Space) {
        atoms.pop();
    }
    if let Some(t) = tail {
        atoms.push(t);
    }
}

/// Breaks the atoms of one paragraph into lines. `first_w` / `rest_w`: widths available on the
/// first and later lines (`f64::INFINITY` = never wrap).
fn break_lines(
    atoms: Vec<Atom>,
    first_w: f64,
    rest_w: f64,
    ctx: &mut Ctx,
    tg: &tabs::TabGeom,
    x_first: f64,
    x_rest: f64,
) -> (Vec<Line>, bool) {
    let mut lines: Vec<Line> = Vec::new();
    let mut cur: Vec<Atom> = Vec::new();
    let mut used = 0.0f64;
    let mut q = clusters(atoms);
    let mut truncated = false;
    let mut ended_with_break = false;
    while let Some(mut cl) = q.pop_front() {
        if lines.len() >= MAX_LINES {
            truncated = true;
            break;
        }
        let avail = if lines.is_empty() { first_w } else { rest_w };
        if cl.iter().any(|a| a.tab) {
            let x0 = if lines.is_empty() { x_first } else { x_rest };
            tabs::resolve_left(&mut cl, x0 + used, tg);
        }
        ended_with_break = cl.len() == 1 && cl[0].kind == Kind::Break;
        if ended_with_break {
            cur.extend(cl);
            trim_trailing_spaces(&mut cur);
            lines.push(Line {
                atoms: std::mem::take(&mut cur),
                forced: true,
            });
            used = 0.0;
            continue;
        }
        let all_space = cl.iter().all(|a| a.kind == Kind::Space);
        if all_space && cur.is_empty() && !lines.is_empty() {
            continue; // leading space of a wrapped line
        }
        let total: f64 = cl.iter().map(|a| a.width).sum();
        let trail: f64 = cl
            .iter()
            .rev()
            .take_while(|a| a.kind == Kind::Space)
            .map(|a| a.width)
            .sum();
        let fits = used + total - trail <= avail + 0.01;
        if fits {
            used += total;
            cur.extend(cl);
            continue;
        }
        let has_content = cur.iter().any(|a| a.kind != Kind::Space);
        if has_content {
            trim_trailing_spaces(&mut cur);
            lines.push(Line {
                atoms: std::mem::take(&mut cur),
                forced: false,
            });
            used = 0.0;
            q.push_front(cl);
            continue;
        }
        // The line is empty and the cluster still does not fit.
        let single_chars = cl.iter().all(|a| a.text.chars().count() <= 1);
        if single_chars || all_space {
            used += total;
            cur.extend(cl);
            continue;
        }
        // Break the cluster between characters.
        let mut pieces: Vec<Atom> = Vec::new();
        for a in cl {
            if a.text.chars().count() <= 1 {
                pieces.push(a);
                continue;
            }
            let n = a.text.chars().count();
            let share = a.width / n as f64;
            for (i, ch) in a.text.chars().enumerate() {
                let t = ch.to_string();
                let e = &ctx.styles[a.style];
                let w = fonts::measure(&e.stack, e.style.bold, e.style.italic, &t, e.style.size_px)
                    + e.style.spacing_px;
                pieces.push(Atom {
                    no_start: NO_START.contains(ch) && a.kind != Kind::Space,
                    no_end: NO_END.contains(ch),
                    text: t,
                    style: a.style,
                    kind: if a.kind == Kind::Space {
                        Kind::Space
                    } else {
                        Kind::Cjk
                    },
                    width: if w > 0.0 { w } else { share },
                    force_break: i > 0 || a.force_break,
                    tab: false,
                });
            }
        }
        let mut sub = clusters(pieces);
        while let Some(c) = sub.pop_back() {
            q.push_front(c);
        }
    }
    // a forced break at the very end leaves an empty last line (as PowerPoint draws it)
    if !cur.is_empty() || lines.is_empty() || ended_with_break {
        trim_trailing_spaces(&mut cur);
        lines.push(Line {
            atoms: cur,
            forced: true,
        });
    }
    (lines, truncated)
}

// ---------------------------------------------------------------------------------------------
// Numbering and bullets
// ---------------------------------------------------------------------------------------------

fn roman(mut n: u32, upper: bool) -> String {
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    if upper {
        s.to_uppercase()
    } else {
        s
    }
}

fn alpha(n: u32, upper: bool) -> String {
    if n == 0 {
        return "0".into();
    }
    let n = n - 1;
    let c = (b'a' + (n % 26) as u8) as char;
    let s: String = std::iter::repeat_n(c, (n / 26 + 1).min(8) as usize).collect();
    if upper {
        s.to_uppercase()
    } else {
        s
    }
}

/// The text of an automatic number: `scheme` is DrawingML's `buAutoNum@type`; unknown schemes
/// fall back to `arabicPeriod`.
pub fn autonum_text(scheme: &str, n: u32) -> String {
    let (body, tail): (String, &str) = if let Some(s) = scheme.strip_prefix("arabic") {
        (n.to_string(), suffix(s))
    } else if let Some(s) = scheme.strip_prefix("alphaLc") {
        (alpha(n, false), suffix(s))
    } else if let Some(s) = scheme.strip_prefix("alphaUc") {
        (alpha(n, true), suffix(s))
    } else if let Some(s) = scheme.strip_prefix("romanLc") {
        (roman(n, false), suffix(s))
    } else if let Some(s) = scheme.strip_prefix("romanUc") {
        (roman(n, true), suffix(s))
    } else if scheme.starts_with("circleNumDbPlain") {
        return (1..=20)
            .contains(&n)
            .then(|| char::from_u32(0x2460 + n - 1))
            .flatten()
            .map(String::from)
            .unwrap_or_else(|| format!("{n}."));
    } else if scheme.starts_with("circleNumWdBlackPlain")
        || scheme.starts_with("circleNumWdWhitePlain")
    {
        return (1..=10)
            .contains(&n)
            .then(|| char::from_u32(0x2776 + n - 1))
            .flatten()
            .map(String::from)
            .unwrap_or_else(|| format!("{n}."));
    } else {
        (n.to_string(), ".")
    };
    match tail {
        "ParenBoth" => format!("({body})"),
        "ParenR" => format!("{body})"),
        "Plain" => body,
        "Period" => format!("{body}."),
        _ => format!("{body}."),
    }
}

fn suffix(s: &str) -> &str {
    match s {
        "Period" | "ParenR" | "ParenBoth" | "Plain" => s,
        _ => "Period",
    }
}

/// The character a symbol-font bullet shows (see [`super::symbol_font`]); other fonts keep `ch`.
fn symbol_bullet(font: &str, ch: char) -> char {
    super::symbol_font::map(font, ch)
}

// ---------------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------------

struct ParaOut {
    /// Lines with the baseline offset from the top of the paragraph's first line top.
    lines: Vec<PLine>,
    before: f64,
    after: f64,
    bullet: Option<PBullet>,
}

struct PLine {
    frags: Vec<Frag>, // x relative to the frame, y relative to the line top (baseline)
    height: f64,
    baseline: f64, // from line top
    width: f64,
    x_left: f64,
}

struct PBullet {
    draw: BulletDraw, // x relative to frame; y relative to the first line top (baseline for text)
}

fn spacing_px(sp: Spacing, size_px: f64, red: f64) -> f64 {
    match sp {
        Spacing::Pts(p) => {
            if p.is_finite() {
                (p * PX_PER_PT).clamp(-1e5, 1e5)
            } else {
                0.0
            }
        }
        Spacing::Pct(p) => {
            let p = if p.is_finite() {
                (p - red).max(0.0)
            } else {
                0.0
            };
            (p * LINE_HEIGHT * size_px).clamp(-1e5, 1e5)
        }
    }
}

fn emu_px(v: f64) -> f64 {
    if v.is_finite() {
        (v / EMU_PER_PX).clamp(-1e6, 1e6)
    } else {
        0.0
    }
}

/// Lays out a text body in a rectangle of `width_px` x `height_px` (the text rectangle after the
/// insets, in the shape's unrotated frame).
pub fn layout(body: &TextBody, width_px: f64, height_px: f64) -> Layout {
    let (w, h) = (
        if width_px.is_finite() {
            width_px.clamp(0.0, 1e6)
        } else {
            0.0
        },
        if height_px.is_finite() {
            height_px.clamp(0.0, 1e6)
        } else {
            0.0
        },
    );
    let (frame, fw, fh) = match body.vert {
        Vert::Horz | Vert::EaVert => (Frame::Normal, w, h),
        Vert::Vert => (Frame::Rot90, h, w),
        Vert::Vert270 => (Frame::Rot270, h, w),
    };
    let (font_scale, red) = match body.autofit {
        AutoFit::Normal {
            font_scale,
            ln_spc_reduction,
        } => (
            if font_scale.is_finite() && font_scale > 0.0 {
                font_scale.min(1.0)
            } else {
                1.0
            },
            if ln_spc_reduction.is_finite() {
                ln_spc_reduction.clamp(0.0, 0.8)
            } else {
                0.0
            },
        ),
        _ => (1.0, 0.0),
    };
    // `normAutofit` is drawn with the scales the file stores and no others: PowerPoint does not
    // recompute them when it opens a file (it does so while the text is edited), so a file
    // without stored scales is shown at full size, overflow or not.
    layout_at(body, frame, fw, fh, font_scale, red)
}

fn layout_at(body: &TextBody, frame: Frame, fw: f64, fh: f64, font_scale: f64, red: f64) -> Layout {
    let mut ctx = Ctx {
        styles: Vec::new(),
        index: HashMap::new(),
        font_scale,
        chars: 0,
        truncated: false,
    };
    let mut out = if body.vert == Vert::EaVert {
        layout_ea_vert(body, fw, fh, &mut ctx, red)
    } else {
        layout_horizontal(body, fw, fh, &mut ctx, red)
    };
    out.frame = frame;
    out.frame_w = fw;
    out.frame_h = fh;
    out.truncated |= ctx.truncated;
    out
}

fn paragraphs_out(
    body: &TextBody,
    col_w: f64,
    ctx: &mut Ctx,
    red: f64,
    vertical_measure: Option<f64>,
) -> (Vec<ParaOut>, bool) {
    let mut outs: Vec<ParaOut> = Vec::new();
    let mut truncated = false;
    let mut counters: Vec<Option<(String, u32)>> = vec![None; 10];
    let mut run_base = 0usize;
    for (pi, para) in body.paragraphs.iter().enumerate() {
        let atoms = build_atoms(para, run_base, ctx);
        run_base += para.runs.len();
        if ctx.truncated {
            truncated = true;
        }
        let lvl = (para.level as usize).min(9);
        // --- bullet ---
        let first_run = para
            .runs
            .iter()
            .find(|r| r.kind != RunKind::LineBreak)
            .or(para.runs.first());
        let mut bullet_text: Option<(String, FragStyle, f64)> = None; // text, style, width
        let mut bullet_stack: Vec<String> = Vec::new();
        let mut bullet_pic: Option<(ImageFill, f64)> = None;
        let has_text = atoms.iter().any(|a| a.kind != Kind::Break);
        if let (Some(b), true) = (&para.bullet, has_text || !atoms.is_empty()) {
            let fr = first_run
                .cloned()
                .unwrap_or_else(|| Run::text("", para.end_size_pt));
            match &b.kind {
                BulletKind::Char(s) => {
                    for c in counters.iter_mut().skip(lvl) {
                        *c = None;
                    }
                    let font = b
                        .font
                        .as_ref()
                        .and_then(|f| f.symbol.as_deref().or(f.latin.as_deref()));
                    // The size factor of the font's own glyph is looked up by the stored code,
                    // before the code is replaced by the stand-in character.
                    let glyph = s
                        .chars()
                        .next()
                        .zip(font)
                        .map_or(1.0, |(c, f)| super::symbol_font::glyph_scale(f, c));
                    let s: String = s
                        .chars()
                        .take(4)
                        .map(|c| font.map_or(c, |f| symbol_bullet(f, c)))
                        .collect();
                    let s = if s.is_empty() { "•".to_string() } else { s };
                    let (st, stack) = bullet_style(b, &fr, ctx, glyph);
                    bullet_stack = stack;
                    bullet_text = Some((s, st, 0.0));
                }
                BulletKind::AutoNum { scheme, start } => {
                    let n = match &counters[lvl] {
                        Some((s, next)) if s == scheme => *next,
                        _ => (*start).max(1),
                    };
                    counters[lvl] = Some((scheme.clone(), n.saturating_add(1)));
                    for c in counters.iter_mut().skip(lvl + 1) {
                        *c = None;
                    }
                    let (st, stack) = bullet_style(b, &fr, ctx, 1.0);
                    bullet_stack = stack;
                    bullet_text = Some((autonum_text(scheme, n), st, 0.0));
                }
                BulletKind::Picture(img) => {
                    let sz = clean_size(fr.size_pt, 18.0) * PX_PER_PT * ctx.font_scale;
                    bullet_pic = Some((img.clone(), sz));
                }
            }
        } else if para.bullet.is_none() && has_text {
            for c in counters.iter_mut().skip(lvl) {
                *c = None;
            }
        }
        if let Some((t, st, wd)) = bullet_text.as_mut() {
            *wd = fonts::measure(&bullet_stack, st.bold, st.italic, t, st.size_px);
        }
        let bullet_w = bullet_text
            .as_ref()
            .map(|b| b.2)
            .or(bullet_pic.as_ref().map(|b| b.1))
            .unwrap_or(0.0);
        let bsize = bullet_text
            .as_ref()
            .map(|b| b.1.size_px)
            .or(bullet_pic.as_ref().map(|b| b.1))
            .unwrap_or(0.0);

        // --- geometry ---
        let mar_l = emu_px(para.mar_l).max(0.0);
        let indent = emu_px(para.indent);
        let has_bullet = bullet_text.is_some() || bullet_pic.is_some();
        let bullet_x = (mar_l + indent).max(0.0);
        // Without a bullet the first line starts at `marL + indent` (relative to the margin, as
        // DrawingML says), never left of the box; a hanging indent (`indent < 0`) leaves room for
        // a tab to reach `marL` (see `text_tabs`: that position is a tab stop).
        let first_x = if has_bullet {
            mar_l.max(bullet_x + bullet_w + 0.3 * bsize)
        } else {
            (mar_l + indent).max(0.0)
        };
        let tab_geom = tabs::TabGeom::of(para, mar_l);
        let (first_w, rest_w) = match vertical_measure {
            Some(len) => (len, len),
            None if body.wrap => ((col_w - first_x).max(1.0), (col_w - mar_l).max(1.0)),
            None => (f64::INFINITY, f64::INFINITY),
        };
        let (lines, t) = if atoms.is_empty() {
            (
                vec![Line {
                    atoms: Vec::new(),
                    forced: true,
                }],
                false,
            )
        } else {
            break_lines(atoms, first_w, rest_w, ctx, &tab_geom, first_x, mar_l)
        };
        truncated |= t;

        // --- lines ---
        let end_px = clean_size(para.end_size_pt, 18.0) * PX_PER_PT * ctx.font_scale;
        let n_lines = lines.len();
        let mut plines: Vec<PLine> = Vec::new();
        for (li, line) in lines.iter().enumerate() {
            let size = line
                .atoms
                .iter()
                .map(|a| ctx.styles[a.style].style.size_px)
                .fold(0.0f64, f64::max);
            let size = if size > 0.0 { size } else { end_px };
            let single = LINE_HEIGHT * size;
            let (lh, baseline) = match para.line_spacing {
                Spacing::Pct(p) => {
                    let p = if p.is_finite() {
                        (p - red).max(0.1)
                    } else {
                        1.0
                    };
                    let lh = single * p;
                    (lh, lh - (LINE_HEIGHT - ASCENT) * size)
                }
                Spacing::Pts(p) => {
                    let lh = if p.is_finite() && p > 0.0 {
                        p * PX_PER_PT
                    } else {
                        single
                    };
                    (lh, lh * 0.8)
                }
            };
            let baseline = baseline.max(0.0);
            let x0 = if li == 0 { first_x } else { mar_l };
            let mut atoms_v: Vec<Atom> = line.atoms.clone();
            tabs::fix_line(&mut atoms_v, x0, &tab_geom);
            let natural: f64 = atoms_v.iter().map(|a| a.width).sum();
            let align = effective_align(para);
            let justify = matches!(para.align, Align::Justify | Align::Distributed)
                && ((li + 1 < n_lines && !line.forced) || para.align == Align::Distributed)
                && vertical_measure.is_none()
                && body.wrap;
            let avail = if vertical_measure.is_some() {
                first_w
            } else {
                (col_w - x0).max(0.0)
            };
            let mut extra_gap = 0.0;
            if justify && para.align == Align::Distributed {
                atoms_v = explode_chars(&atoms_v, ctx);
            }
            let natural = if justify && para.align == Align::Distributed {
                atoms_v.iter().map(|a| a.width).sum()
            } else {
                natural
            };
            let gap_after: Vec<bool> = if justify {
                let space_gaps: Vec<bool> = atoms_v
                    .iter()
                    .enumerate()
                    .map(|(i, a)| a.kind == Kind::Space && i + 1 < atoms_v.len())
                    .collect();
                if space_gaps.iter().any(|g| *g) && para.align == Align::Justify {
                    space_gaps
                } else {
                    (0..atoms_v.len()).map(|i| i + 1 < atoms_v.len()).collect()
                }
            } else {
                vec![false; atoms_v.len()]
            };
            let n_gaps = gap_after.iter().filter(|g| **g).count();
            if justify && n_gaps > 0 && avail > natural {
                extra_gap = (avail - natural) / n_gaps as f64;
            }
            let width = natural + extra_gap * n_gaps as f64;
            let x_start = match align {
                Align::Center => x0 + (avail - width) / 2.0,
                Align::Right => x0 + avail - width,
                _ => x0,
            };
            // frags
            let mut frags: Vec<Frag> = Vec::new();
            // Right-to-left lines keep their logical order here: positions are mirrored below, and
            // the shaper reorders the characters inside a fragment (bidi).
            let order: Vec<usize> = (0..atoms_v.len()).collect();
            let mut x = x_start;
            let mut after_tab = false;
            for &ai in &order {
                let a = &atoms_v[ai];
                let e = &ctx.styles[a.style];
                if a.kind == Kind::Break {
                    continue;
                }
                let y = baseline - e.shift;
                // A tab is a fragment of its own: the text after it starts where the stop is,
                // not where the shaper would put it after a space.
                let merge = extra_gap == 0.0
                    && !a.tab
                    && !after_tab
                    && frags.last().is_some_and(|f: &Frag| {
                        f.style == e.style && (f.x + f.width - x).abs() < 1e-6 && f.y == y
                    });
                if merge {
                    let f = frags.last_mut().unwrap();
                    f.text.push_str(&a.text);
                    f.width += a.width;
                } else {
                    frags.push(Frag {
                        text: a.text.clone(),
                        x,
                        y,
                        width: a.width,
                        style: e.style.clone(),
                        rot90: false,
                    });
                }
                x += a.width;
                after_tab = a.tab;
                if gap_after[ai] {
                    x += extra_gap;
                }
            }
            plines.push(PLine {
                frags,
                height: lh,
                baseline,
                width,
                x_left: x_start,
            });
        }
        // mirror right-to-left paragraphs
        if para.rtl {
            for l in plines.iter_mut() {
                for f in l.frags.iter_mut() {
                    f.x = col_w - (f.x + f.width);
                }
                l.x_left = col_w - (l.x_left + l.width);
            }
        }
        // --- bullet placement ---
        let bullet = if let Some((t, st, wd)) = bullet_text {
            let l0 = &plines[0];
            let mut bx = if has_bullet {
                (l0.x_left - (first_x - bullet_x)).max(if para.rtl { f64::MIN } else { 0.0 })
            } else {
                bullet_x
            };
            if matches!(
                para.align,
                Align::Left | Align::Justify | Align::Distributed
            ) && !para.rtl
            {
                bx = bullet_x;
            }
            if para.rtl {
                bx = col_w - (l0.x_left + l0.width) + (first_x - bullet_x) - wd;
                if matches!(
                    para.align,
                    Align::Left | Align::Justify | Align::Distributed
                ) {
                    bx = col_w - bullet_x - wd;
                }
            }
            Some(PBullet {
                draw: BulletDraw::Text(Frag {
                    text: t,
                    x: bx,
                    y: l0.baseline,
                    width: wd,
                    style: st,
                    rot90: false,
                }),
            })
        } else if let Some((img, sz)) = bullet_pic {
            let l0 = &plines[0];
            Some(PBullet {
                draw: BulletDraw::Picture {
                    image: img,
                    x: if para.rtl {
                        col_w - bullet_x - sz
                    } else {
                        bullet_x
                    },
                    y: l0.baseline - ASCENT * sz,
                    size: sz,
                },
            })
        } else {
            None
        };
        let first_size = plines.first().map_or(end_px, |l| l.height / LINE_HEIGHT);
        let before = if pi == 0 {
            0.0
        } else {
            spacing_px(para.spc_before, first_size, red)
        };
        let after = spacing_px(para.spc_after, first_size, red);
        outs.push(ParaOut {
            lines: plines,
            before,
            after,
            bullet,
        });
    }
    (outs, truncated)
}

fn effective_align(p: &Paragraph) -> Align {
    // Justified text uses the start edge; mirroring for right-to-left is done on the finished
    // line, so the alignment here is the one in the left-to-right frame.
    match (p.align, p.rtl) {
        (Align::Left, true) => Align::Right,
        (Align::Right, true) => Align::Left,
        (a, _) => a,
    }
}

fn explode_chars(atoms: &[Atom], ctx: &Ctx) -> Vec<Atom> {
    let mut v = Vec::new();
    for a in atoms {
        if a.kind == Kind::Break || a.text.chars().count() <= 1 {
            v.push(a.clone());
            continue;
        }
        let e = &ctx.styles[a.style];
        for ch in a.text.chars() {
            let t = ch.to_string();
            let w = fonts::measure(&e.stack, e.style.bold, e.style.italic, &t, e.style.size_px)
                + e.style.spacing_px;
            v.push(Atom {
                text: t,
                style: a.style,
                kind: a.kind,
                width: w,
                no_start: false,
                no_end: false,
                force_break: true,
                tab: false,
            });
        }
    }
    v
}

fn bullet_style(b: &Bullet, first: &Run, ctx: &mut Ctx, glyph: f64) -> (FragStyle, Vec<String>) {
    let mut run = first.clone();
    if let Some(f) = &b.font {
        if f.latin.is_some() || f.symbol.is_some() {
            run.font.latin = f.symbol.clone().or(f.latin.clone());
        }
    }
    let base = clean_size(first.size_pt, 18.0);
    run.size_pt = match b.size {
        BulletSize::FollowText => base,
        BulletSize::Pct(p) if p.is_finite() && p > 0.0 => base * p,
        BulletSize::Pts(p) if p.is_finite() && p > 0.0 => p,
        _ => base,
    };
    run.size_pt *= glyph;
    run.baseline_pct = 0.0;
    run.underline = Underline::None;
    run.strike = Strike::None;
    run.highlight = None;
    run.caps = Caps::None;
    if let Some(c) = b.color {
        run.fill = Fill::Solid(c);
    }
    let key = usize::MAX - ctx.styles.len();
    let id = ctx.style_id(key, &run, Script::Latin, false);
    let mut st = ctx.styles[id].style.clone();
    let mut stack = ctx.styles[id].stack.clone();
    // Symbol-font bullets are drawn in a font that has the look-alike glyphs.
    let symbolish = run
        .font
        .latin
        .as_deref()
        .is_some_and(|n| super::symbol_font::symbol_font(n).is_some());
    if symbolish {
        stack = fonts::stack_for(Some("Arial Unicode MS"), Script::Latin);
        st.family = fonts::css_family(&stack).into();
    }
    (st, stack)
}

fn layout_horizontal(body: &TextBody, w: f64, h: f64, ctx: &mut Ctx, red: f64) -> Layout {
    let cols = body.columns.clamp(1, 16) as usize;
    let col_w = (w / cols as f64).max(0.0);
    let (paras, trunc) = paragraphs_out(body, col_w, ctx, red, None);

    // Flatten to absolute line records.
    struct Rec<'a> {
        pl: &'a PLine,
        para: usize,
        first: bool,
        gap_before: f64,
    }
    let mut recs: Vec<Rec> = Vec::new();
    for (pi, p) in paras.iter().enumerate() {
        for (li, pl) in p.lines.iter().enumerate() {
            recs.push(Rec {
                pl,
                para: pi,
                first: li == 0,
                gap_before: if li == 0 { p.before } else { 0.0 },
            });
        }
    }
    let after_of = |pi: usize| paras[pi].after;
    // vertical stacking
    let mut lines_out: Vec<LineBox> = Vec::new();
    let mut bullets_out: Vec<BulletDraw> = Vec::new();
    let mut col = 0usize;
    let mut y = 0.0f64;
    let mut content_h = 0.0f64;
    let mut content_w = 0.0f64;
    let mut placed: Vec<(usize, f64, usize)> = Vec::new(); // rec index, top, column
    for (i, r) in recs.iter().enumerate() {
        y += r.gap_before;
        if cols > 1 && y + r.pl.height > h && y > 0.0 && col + 1 < cols && h > 0.0 {
            col += 1;
            y = 0.0;
        }
        placed.push((i, y, col));
        y += r.pl.height;
        content_h = content_h.max(y);
        let last_of_para = recs.get(i + 1).is_none_or(|n| n.para != r.para);
        if last_of_para {
            y += after_of(r.para);
        }
    }
    // The trailing space after the last paragraph does not count as text.
    let anchor_off = if cols > 1 {
        0.0
    } else {
        match body.anchor {
            Anchor::Top => 0.0,
            Anchor::Middle => (h - content_h) / 2.0,
            Anchor::Bottom => h - content_h,
        }
    };
    // anchorCtr: centre the block (its width being the widest line) horizontally.
    let x_shift_block = if body.anchor_ctr && cols == 1 {
        let block_x0 = recs
            .iter()
            .map(|r| r.pl.x_left)
            .fold(f64::INFINITY, f64::min);
        let block_x1 = recs
            .iter()
            .map(|r| r.pl.x_left + r.pl.width)
            .fold(f64::NEG_INFINITY, f64::max);
        if block_x0.is_finite() && block_x1.is_finite() {
            (w - (block_x1 - block_x0)) / 2.0 - block_x0
        } else {
            0.0
        }
    } else {
        0.0
    };
    for &(ri, top, c) in &placed {
        let r = &recs[ri];
        let xoff = c as f64 * col_w + x_shift_block;
        let top = top + anchor_off;
        let baseline_abs = top + r.pl.baseline;
        let frags: Vec<Frag> =
            r.pl.frags
                .iter()
                .map(|f| {
                    let mut f = f.clone();
                    f.x += xoff;
                    f.y += top;
                    f
                })
                .collect();
        content_w = content_w.max(r.pl.width);
        lines_out.push(LineBox {
            top,
            height: r.pl.height,
            baseline: baseline_abs,
            frags,
        });
        if r.first {
            if let Some(b) = &paras[r.para].bullet {
                bullets_out.push(match &b.draw {
                    BulletDraw::Text(f) => BulletDraw::Text(Frag {
                        x: f.x + xoff,
                        y: top + f.y,
                        ..f.clone()
                    }),
                    BulletDraw::Picture {
                        image,
                        x,
                        y: py,
                        size,
                    } => BulletDraw::Picture {
                        image: image.clone(),
                        x: x + xoff,
                        y: top + py,
                        size: *size,
                    },
                });
            }
        }
    }
    Layout {
        frame: Frame::Normal,
        frame_w: w,
        frame_h: h,
        lines: lines_out,
        bullets: bullets_out,
        content_w,
        content_h,
        truncated: trunc,
    }
}

/// How a character of East-Asian vertical text is placed in its cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerticalForm {
    /// Drawn as it is, upright.
    Upright,
    /// Drawn turned a quarter turn clockwise.
    Rotate,
    /// Drawn upright, moved to the top right of the cell.
    TopRight,
}

/// The placement of `c` in vertical text (the vertical glyph forms the fonts have, made from the
/// horizontal glyph: SVG text has no vertical writing mode here).
fn vertical_form(c: char) -> VerticalForm {
    match c {
        '、' | '。' | '，' | '．' | '､' | '｡' => VerticalForm::TopRight,
        '「' | '」' | '『' | '』' | '（' | '）' | '【' | '】' | '〔' | '〕' | '《' | '》'
        | '〈' | '〉' | '［' | '］' | '｛' | '｝' | '｢' | '｣' | 'ー' | 'ｰ' | '〜' | '～' | '—'
        | '―' | '…' | '‥' | '－' | '−' | '(' | ')' | '[' | ']' | '{' | '}' | '〝' | '〟' => {
            VerticalForm::Rotate
        }
        _ => VerticalForm::Upright,
    }
}

fn layout_ea_vert(body: &TextBody, w: f64, h: f64, ctx: &mut Ctx, red: f64) -> Layout {
    // Lay out as horizontal text of width `h` with East-Asian characters one em wide, then turn
    // each line into a column.
    let (paras, trunc) = paragraphs_out(body, h, ctx, red, if body.wrap { Some(h) } else { None });
    let mut cols: Vec<(&PLine, f64)> = Vec::new(); // line, extra gap before
    for p in &paras {
        for (li, l) in p.lines.iter().enumerate() {
            cols.push((l, if li == 0 { p.before } else { 0.0 }));
        }
    }
    let total: f64 = cols.iter().map(|(l, g)| l.height + g).sum();
    let right = match body.anchor {
        Anchor::Top => w,
        Anchor::Middle => (w + total) / 2.0,
        Anchor::Bottom => total,
    };
    let mut x_right = right;
    let mut lines_out = Vec::new();
    let mut content_h = 0.0f64;
    // `anchorCtr` in vertical text centres the block along the lines (top to bottom): by the
    // longest column.
    let longest = cols
        .iter()
        .map(|(l, _)| l.width + l.x_left)
        .fold(0.0f64, f64::max);
    let along_shift = if body.anchor_ctr && longest.is_finite() {
        (h - longest) / 2.0
    } else {
        0.0
    };
    for (l, gap) in cols {
        x_right -= gap;
        let center = x_right - l.height / 2.0;
        let mut frags = Vec::new();
        for f in &l.frags {
            let along = f.x; // distance along the column (from its start)
            let size = f.style.size_px;
            // Characters of East-Asian text stand upright, one per cell (so do the punctuation
            // marks that have a vertical form of their own, whatever their script); Latin text is
            // rotated.
            let upright = f.text.chars().all(|c| {
                fonts::script_of(c) == Script::EastAsian
                    || vertical_form(c) != VerticalForm::Upright
            });
            if upright {
                let mut yy = along + along_shift;
                for ch in f.text.chars() {
                    let (x, y, rot90) = match vertical_form(ch) {
                        // Brackets, the long-vowel mark, dashes and the ellipsis are the
                        // horizontal glyph turned a quarter turn clockwise (the vertical form of
                        // each is exactly that), about the middle of its cell.
                        VerticalForm::Rotate => (center - 0.38 * size, yy, true),
                        // The comma and the full stop sit at the top right of their cell in
                        // vertical text (bottom left in horizontal text).
                        VerticalForm::TopRight => (
                            center - size / 2.0 + 0.55 * size,
                            yy + 0.88 * size - 0.5 * size,
                            false,
                        ),
                        VerticalForm::Upright => (center - size / 2.0, yy + 0.88 * size, false),
                    };
                    frags.push(Frag {
                        text: ch.to_string(),
                        x,
                        y,
                        width: size,
                        style: f.style.clone(),
                        rot90,
                    });
                    yy += size + f.style.spacing_px;
                }
            } else {
                frags.push(Frag {
                    text: f.text.clone(),
                    x: center - 0.35 * size,
                    y: along + along_shift,
                    width: f.width,
                    style: f.style.clone(),
                    rot90: true,
                });
            }
        }
        content_h = content_h.max(l.width + l.x_left);
        lines_out.push(LineBox {
            top: x_right - l.height,
            height: l.height,
            baseline: center,
            frags,
        });
        x_right -= l.height;
    }
    Layout {
        frame: Frame::Normal,
        frame_w: w,
        frame_h: h,
        lines: lines_out,
        bullets: Vec::new(),
        content_w: total,
        content_h,
        truncated: trunc,
    }
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
