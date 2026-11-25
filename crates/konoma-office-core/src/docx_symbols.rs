//! Symbol fonts of Word (`Symbol`, `Wingdings`, `Wingdings 2`, `Wingdings 3`, `Webdings`).
//!
//! These fonts are not Unicode fonts: the letter `l` in a run set in Wingdings *draws* a black
//! circle, and Word stores such a character either as `w:sym` (`w:font` + `w:char="F06C"`), as a
//! private-use code point `U+F06C` (the code plus `0xF000`), or as the bare code `l`. The glyph is
//! the document's text, so it is mapped here to the Unicode character that looks like it. A code
//! the tables do not know is shown as [`FALLBACK`] rather than dropped: the reader sees that a
//! symbol stood there.

/// What an unknown code of a symbol font is shown as (a neutral bullet).
pub(crate) const FALLBACK: char = '\u{2022}';

/// The font of a run, as far as the converter cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Font {
    /// Any font that is not a symbol font (its characters are Unicode).
    Plain,
    Symbol,
    Wingdings,
    Wingdings2,
    Wingdings3,
    Webdings,
}

/// The symbol font a font name stands for (`None`: not a symbol font).
pub(crate) fn symbol_font(name: &str) -> Option<Font> {
    let n: String = name
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    match n.as_str() {
        "symbol" => Some(Font::Symbol),
        "wingdings" => Some(Font::Wingdings),
        "wingdings2" => Some(Font::Wingdings2),
        "wingdings3" => Some(Font::Wingdings3),
        "webdings" => Some(Font::Webdings),
        _ => None,
    }
}

/// `w:hint` of `w:rFonts`: which slot Word uses for a character that several scripts share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hint {
    Default,
    EastAsia,
    Cs,
}

/// The four font slots of a `w:rFonts`. A slot is `None` when it is not set (inherited from the
/// style below); `Some(Font::Plain)` is a font that is not a symbol font.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Fonts {
    pub ascii: Option<Font>,
    pub hansi: Option<Font>,
    pub cs: Option<Font>,
    pub east: Option<Font>,
    pub hint: Option<Hint>,
}

impl Fonts {
    /// `self` with every slot `over` sets replaced.
    pub fn over(self, over: Fonts) -> Fonts {
        Fonts {
            ascii: over.ascii.or(self.ascii),
            hansi: over.hansi.or(self.hansi),
            cs: over.cs.or(self.cs),
            east: over.east.or(self.east),
            hint: over.hint.or(self.hint),
        }
    }

    /// The font Word draws `c` in (ECMA-376 17.3.2.26): the `ascii` slot for U+0000..=U+007F, the
    /// `eastAsia` slot for East Asian characters, `cs` for complex scripts, `hAnsi` for the rest (the
    /// symbol-font private-use codes follow `ascii`);
    /// a character both Latin and East Asian by use (`°`, `§`, `×` ...) follows `w:hint`.
    fn of_char(&self, c: char) -> Option<Font> {
        let v = c as u32;
        let ambiguous = matches!(v, 0xA1 | 0xA4 | 0xA7 | 0xA8 | 0xAA | 0xAD | 0xAF | 0xB0..=0xB4
            | 0xB6..=0xBA | 0xBC..=0xBF | 0xD7 | 0xF7 | 0x2010..=0x2027 | 0x2030..=0x203B);
        if ambiguous && self.hint == Some(Hint::EastAsia) {
            return self.east;
        }
        if ambiguous && self.hint == Some(Hint::Cs) {
            return self.cs;
        }
        match v {
            0..=0x7F => self.ascii,
            // Word draws the private-use codes of a symbol font (`U+F0xx`) with the font of the
            // `ascii` slot (a style that names only that slot still works), else `hAnsi`.
            0xF000..=0xF0FF => self.ascii.or(self.hansi),
            0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF => self.cs,
            0x2E80..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFFEF
            | 0x20000..=0x3FFFF => self.east,
            _ => self.hansi,
        }
    }
}

/// The 8-bit code a character of a symbol-font run stands for: `U+F0xx` and the bare code `xx`.
fn code_of(c: char) -> Option<u8> {
    match c as u32 {
        v @ 0xF000..=0xF0FF => Some((v - 0xF000) as u8),
        v @ 0x20..=0xFF => Some(v as u8),
        _ => None,
    }
}

/// The text of a run set in `font`: every character that is a code of the font is replaced by
/// what it draws. Other characters (real Unicode text) are kept.
pub(crate) fn map_run_text(s: &str, fonts: Fonts) -> String {
    s.chars()
        .map(|c| match fonts.of_char(c) {
            Some(f) if f != Font::Plain => map_char(f, c),
            // No symbol font known for the character: a private-use code of the `F0xx` block is a
            // symbol of some symbol font; show that one stood there.
            _ if (0xF020..=0xF0FF).contains(&(c as u32)) => FALLBACK,
            _ => c,
        })
        .collect()
}

/// `w:sym`: the character `c` (`w:char`) of the font named `font` (`w:font`).
pub(crate) fn map_sym(font_name: &str, c: char) -> char {
    match symbol_font(font_name) {
        Some(f) => map_char(f, c),
        // A font that is not a symbol font names a real Unicode character; a private-use one that
        // cannot be placed is shown as a symbol.
        None => {
            if (0xE000..=0xF8FF).contains(&(c as u32)) {
                FALLBACK
            } else {
                c
            }
        }
    }
}

/// One character of a run in a symbol font.
pub(crate) fn map_char(font: Font, c: char) -> char {
    let Some(code) = code_of(c) else {
        return c;
    };
    // The space and the no-break space draw nothing; control codes stay as they are.
    if code == 0x20 {
        return ' ';
    }
    if code == 0xA0 {
        return '\u{00A0}';
    }
    if code < 0x20 {
        return c;
    }
    let m = match font {
        Font::Symbol => symbol(code),
        Font::Wingdings => wingdings(code),
        Font::Wingdings2 => wingdings2(code),
        Font::Wingdings3 => None,
        Font::Webdings => webdings(code),
        Font::Plain => return c,
    };
    m.unwrap_or(FALLBACK)
}

/// Adobe Symbol encoding.
fn symbol(code: u8) -> Option<char> {
    Some(match code {
        // Digits, punctuation and the other ASCII characters are what they look like.
        0x21
        | 0x23
        | 0x25..=0x26
        | 0x28..=0x29
        | 0x2B..=0x2C
        | 0x2E..=0x3F
        | 0x5B
        | 0x5D
        | 0x5F
        | 0x7B..=0x7D => code as char,
        0x22 => '\u{2200}',
        0x2A => '\u{2217}',
        0x24 => '\u{2203}',
        0x27 => '\u{220B}',
        0x2D => '\u{2212}',
        0x40 => '\u{2245}',
        0x41 => '\u{0391}',
        0x42 => '\u{0392}',
        0x43 => '\u{03A7}',
        0x44 => '\u{0394}',
        0x45 => '\u{0395}',
        0x46 => '\u{03A6}',
        0x47 => '\u{0393}',
        0x48 => '\u{0397}',
        0x49 => '\u{0399}',
        0x4A => '\u{03D1}',
        0x4B => '\u{039A}',
        0x4C => '\u{039B}',
        0x4D => '\u{039C}',
        0x4E => '\u{039D}',
        0x4F => '\u{039F}',
        0x50 => '\u{03A0}',
        0x51 => '\u{0398}',
        0x52 => '\u{03A1}',
        0x53 => '\u{03A3}',
        0x54 => '\u{03A4}',
        0x55 => '\u{03A5}',
        0x56 => '\u{03C2}',
        0x57 => '\u{03A9}',
        0x58 => '\u{039E}',
        0x59 => '\u{03A8}',
        0x5A => '\u{0396}',
        0x5C => '\u{2234}',
        0x5E => '\u{22A5}',
        0x61 => '\u{03B1}',
        0x62 => '\u{03B2}',
        0x63 => '\u{03C7}',
        0x64 => '\u{03B4}',
        0x65 => '\u{03B5}',
        0x66 => '\u{03C6}',
        0x67 => '\u{03B3}',
        0x68 => '\u{03B7}',
        0x69 => '\u{03B9}',
        0x6A => '\u{03D5}',
        0x6B => '\u{03BA}',
        0x6C => '\u{03BB}',
        0x6D => '\u{03BC}',
        0x6E => '\u{03BD}',
        0x6F => '\u{03BF}',
        0x70 => '\u{03C0}',
        0x71 => '\u{03B8}',
        0x72 => '\u{03C1}',
        0x73 => '\u{03C3}',
        0x74 => '\u{03C4}',
        0x75 => '\u{03C5}',
        0x76 => '\u{03D6}',
        0x77 => '\u{03C9}',
        0x78 => '\u{03BE}',
        0x79 => '\u{03C8}',
        0x7A => '\u{03B6}',
        0x7E => '\u{223C}',
        0xA3 => '\u{2264}',
        0xA5 => '\u{221E}',
        0xA7 => '\u{2663}',
        0xA8 => '\u{2666}',
        0xA9 => '\u{2665}',
        0xAA => '\u{2660}',
        0xAB => '\u{2194}',
        0xAC => '\u{2190}',
        0xAD => '\u{2191}',
        0xAE => '\u{2192}',
        0xAF => '\u{2193}',
        0xB0 => '\u{00B0}',
        0xB1 => '\u{00B1}',
        0xB3 => '\u{2265}',
        0xB4 => '\u{00D7}',
        0xB5 => '\u{221D}',
        0xB6 => '\u{2202}',
        0xB7 => '\u{2022}',
        0xB8 => '\u{00F7}',
        0xB9 => '\u{2260}',
        0xBA => '\u{2261}',
        0xBB => '\u{2248}',
        0xBC => '\u{2026}',
        0xC0 => '\u{2135}',
        0xC1 => '\u{2111}',
        0xC2 => '\u{211C}',
        0xC3 => '\u{2118}',
        0xC4 => '\u{2297}',
        0xC5 => '\u{2295}',
        0xC6 => '\u{2205}',
        0xC7 => '\u{2229}',
        0xC8 => '\u{222A}',
        0xC9 => '\u{2283}',
        0xCA => '\u{2287}',
        0xCB => '\u{2284}',
        0xCC => '\u{2282}',
        0xCD => '\u{2286}',
        0xCE => '\u{2208}',
        0xCF => '\u{2209}',
        0xD0 => '\u{2220}',
        0xD1 => '\u{2207}',
        0xD5 => '\u{220F}',
        0xD6 => '\u{221A}',
        0xD7 => '\u{22C5}',
        0xD8 => '\u{00AC}',
        0xD9 => '\u{2227}',
        0xDA => '\u{2228}',
        0xDB => '\u{21D4}',
        0xDC => '\u{21D0}',
        0xDD => '\u{21D1}',
        0xDE => '\u{21D2}',
        0xDF => '\u{21D3}',
        0xE0 => '\u{25CA}',
        0xE1 => '\u{2329}',
        0xE5 => '\u{2211}',
        0xF1 => '\u{232A}',
        0xF2 => '\u{222B}',
        _ => return None,
    })
}

/// Wingdings: the symbols documents use as bullets, check boxes, ticks, arrows and stars.
fn wingdings(code: u8) -> Option<char> {
    Some(match code {
        0x21 => '\u{270F}',
        0x22 => '\u{2702}',
        0x23 => '\u{2701}',
        0x28 => '\u{260E}',
        0x29 => '\u{2706}',
        0x36 => '\u{231B}',
        0x41 => '\u{270C}',
        0x45 => '\u{261C}',
        0x46 => '\u{261E}',
        0x47 => '\u{261D}',
        0x48 => '\u{261F}',
        0x4A => '\u{263A}',
        0x4C => '\u{2639}',
        0x4E => '\u{2620}',
        0x51 => '\u{2708}',
        0x52 => '\u{263C}',
        0x54 => '\u{2744}',
        0x56 => '\u{271E}',
        0x58 => '\u{2720}',
        0x59 => '\u{2721}',
        0x5A => '\u{262A}',
        0x5B => '\u{262F}',
        0x5D => '\u{2638}',
        0x5E..=0x69 => char::from_u32(0x2648 + u32::from(code) - 0x5E)?,
        0x6C => '\u{25CF}',
        0x6D => '\u{274D}',
        0x6E => '\u{25A0}',
        // `o` and `¨` are the empty box of forms and bullets.
        0x6F | 0xA8 => '\u{2610}',
        0x71 => '\u{2751}',
        0x72 => '\u{2752}',
        0x75 => '\u{25C6}',
        0x76 => '\u{2756}',
        0x78 => '\u{2327}',
        0x7A => '\u{2318}',
        0x7B => '\u{273F}',
        0x7C => '\u{2740}',
        0x7D => '\u{275D}',
        0x7E => '\u{275E}',
        0x80 => '\u{24EA}',
        0x81..=0x8A => char::from_u32(0x2460 + u32::from(code) - 0x81)?,
        0x8B => '\u{24FF}',
        0x8C..=0x95 => char::from_u32(0x2776 + u32::from(code) - 0x8C)?,
        0xA1 => '\u{25CB}',
        0xA4 => '\u{25C9}',
        0xA5 => '\u{25CE}',
        0xA7 => '\u{25AA}',
        0xAA => '\u{2726}',
        0xAB => '\u{2605}',
        0xAC => '\u{2736}',
        0xAD => '\u{2734}',
        0xAE => '\u{2739}',
        0xAF => '\u{2735}',
        0xB5 => '\u{272A}',
        0xB6 => '\u{2730}',
        0xD8 => '\u{27A2}',
        0xE8 => '\u{2794}',
        0xF0 => '\u{21E8}',
        0xF1 => '\u{21E7}',
        0xF2 => '\u{21E9}',
        0xFB => '\u{2717}',
        0xFC => '\u{2713}',
        0xFD => '\u{2612}',
        0xFE => '\u{2611}',
        _ => return None,
    })
}

/// Wingdings 2: the check boxes of forms.
fn wingdings2(code: u8) -> Option<char> {
    Some(match code {
        0x50 => '\u{2611}',
        0x52 => '\u{2612}',
        0xA3 => '\u{2610}',
        _ => return None,
    })
}

/// Webdings: the tick and the cross.
fn webdings(code: u8) -> Option<char> {
    Some(match code {
        0x61 => '\u{2713}',
        0x72 => '\u{2717}',
        _ => return None,
    })
}
