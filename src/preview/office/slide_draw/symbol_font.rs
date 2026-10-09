//! The characters the symbol fonts of Office show for the codes bullets and runs use.
//!
//! A bullet in `Symbol` / `Wingdings` is stored as the code of the glyph in a symbol-encoded font:
//! a byte `0x20..=0xFF`, usually written in the private-use area `U+F020..=U+F0FF` (`charset="2"`).
//! These fonts are not installed on the machines the preview runs on, so the code is mapped to the
//! Unicode character that has the same look.
//!
//! Sources: the Adobe Symbol encoding (the `SYMBOL.TXT` mapping Unicode publishes for the Microsoft
//! Symbol font: `0x2D` is U+2212 MINUS SIGN, `0xB7` U+2022 BULLET, ...) and the Wingdings
//! mapping of the Unicode "Wingdings and Webdings" proposal (L2/11-052, the basis of the
//! Unicode 7.0 symbols). Wingdings 2 and Webdings have no table and Wingdings 3 only the triangle bullet of the Office
//! themes: their other bullets are the plain bullet U+2022 (a documented approximation). Codes of Wingdings outside the shapes listed
//! are the plain bullet too.

/// The first and last code of the private-use window the symbol fonts use.
const PUA_FIRST: u32 = 0xF020;
const PUA_LAST: u32 = 0xF0FF;

/// Which symbol font a name is (case, spaces and a `Std`/`MT` suffix ignored), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolFont {
    Symbol,
    Wingdings,
    Wingdings2,
    Wingdings3,
    Webdings,
}

/// The symbol font named `name`, if it is one.
pub fn symbol_font(name: &str) -> Option<SymbolFont> {
    let f: String = name
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let f = f.trim_end_matches("mt");
    Some(match f {
        "symbol" => SymbolFont::Symbol,
        "wingdings" => SymbolFont::Wingdings,
        "wingdings2" => SymbolFont::Wingdings2,
        "wingdings3" => SymbolFont::Wingdings3,
        "webdings" => SymbolFont::Webdings,
        _ => return None,
    })
}

/// The one-byte code of `ch` in a symbol-encoded font: the private-use form loses `0xF000`; a
/// character `0x20..=0xFF` is itself.
fn code_of(ch: char) -> Option<u8> {
    let u = ch as u32;
    if (PUA_FIRST..=PUA_LAST).contains(&u) {
        Some((u - 0xF000) as u8)
    } else if (0x20..=0xFF).contains(&u) {
        Some(u as u8)
    } else {
        None
    }
}

/// The Unicode character the font `font` shows for `ch`; `ch` itself for a font that is not a
/// symbol font or a code outside `0x20..=0xFF`; U+2022 for a code the tables do not know.
pub fn map(font: &str, ch: char) -> char {
    let Some(f) = symbol_font(font) else {
        return ch;
    };
    let Some(c) = code_of(ch) else {
        return ch;
    };
    let u = match f {
        SymbolFont::Symbol => symbol(c),
        SymbolFont::Wingdings => wingdings(c),
        SymbolFont::Wingdings2 => wingdings2(c),
        SymbolFont::Wingdings3 => wingdings3(c),
        SymbolFont::Webdings => webdings(c),
    };
    u.and_then(char::from_u32).unwrap_or('\u{2022}')
}

/// How much larger than the text-size rule a bullet drawn with the stand-in character is made so
/// that it looks like the font's own glyph: Wingdings draws its circle and squares (`l`, `n`, `o`,
/// `q`, `r`) about 1.3 times as large as the Unicode characters of the fallback fonts. `1.0` for
/// everything else.
pub fn glyph_scale(font: &str, ch: char) -> f64 {
    match (symbol_font(font), code_of(ch)) {
        (Some(SymbolFont::Wingdings), Some(0x6C | 0x6E | 0x6F | 0x71 | 0x72)) => 1.3,
        _ => 1.0,
    }
}

/// Adobe Symbol encoding.
fn symbol(c: u8) -> Option<u32> {
    // 0x20..=0x7E
    const LOW: [u32; 95] = [
        0x20, 0x21, 0x2200, 0x23, 0x2203, 0x25, 0x26, 0x220B, 0x28, 0x29, 0x2217, 0x2B, 0x2C,
        0x2212, 0x2E, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B,
        0x3C, 0x3D, 0x3E, 0x3F, 0x2245, // 0x40
        0x391, 0x392, 0x3A7, 0x394, 0x395, 0x3A6, 0x393, 0x397, 0x399, 0x3D1, 0x39A, 0x39B, 0x39C,
        0x39D, 0x39F, 0x3A0, 0x398, 0x3A1, 0x3A3, 0x3A4, 0x3A5, 0x3C2, 0x3A9, 0x39E, 0x3A8,
        0x396, // 0x5A
        0x5B, 0x2234, 0x5D, 0x22A5, 0x5F, 0x203E, // 0x60
        0x3B1, 0x3B2, 0x3C7, 0x3B4, 0x3B5, 0x3C6, 0x3B3, 0x3B7, 0x3B9, 0x3D5, 0x3BA, 0x3BB, 0x3BC,
        0x3BD, 0x3BF, 0x3C0, 0x3B8, 0x3C1, 0x3C3, 0x3C4, 0x3C5, 0x3D6, 0x3C9, 0x3BE, 0x3C8,
        0x3B6, // 0x7A
        0x7B, 0x7C, 0x7D, 0x223C, // 0x7E
    ];
    // 0xA0..=0xFE
    const HIGH: [u32; 95] = [
        0x20AC, 0x3D2, 0x2032, 0x2264, 0x2044, 0x221E, 0x192, 0x2663, 0x2666, 0x2665, 0x2660,
        0x2194, 0x2190, 0x2191, 0x2192, 0x2193, // 0xAF
        0xB0, 0xB1, 0x2033, 0x2265, 0xD7, 0x221D, 0x2202, 0x2022, 0xF7, 0x2260, 0x2261, 0x2248,
        0x2026, 0x2502, 0x2500, 0x21B5, // 0xBF
        0x2135, 0x2111, 0x211C, 0x2118, 0x2297, 0x2295, 0x2205, 0x2229, 0x222A, 0x2283, 0x2287,
        0x2284, 0x2282, 0x2286, 0x2208, 0x2209, // 0xCF
        0x2220, 0x2207, 0xAE, 0xA9, 0x2122, 0x220F, 0x221A, 0x22C5, 0xAC, 0x2227, 0x2228, 0x21D4,
        0x21D0, 0x21D1, 0x21D2, 0x21D3, // 0xDF
        0x25CA, 0x2329, 0xAE, 0xA9, 0x2122, 0x2211, 0x239B, 0x239C, 0x239D, 0x23A1, 0x23A2, 0x23A3,
        0x23A7, 0x23A8, 0x23A9, 0x23AA, // 0xEF
        0x20, 0x232A, 0x222B, 0x2320, 0x23AE, 0x2321, 0x239E, 0x239F, 0x23A0, 0x23A4, 0x23A5,
        0x23A6, 0x23AB, 0x23AC, 0x23AD, // 0xFE
    ];
    match c {
        0x20..=0x7E => Some(LOW[(c - 0x20) as usize]),
        0xA0..=0xFE => Some(HIGH[(c - 0xA0) as usize]),
        _ => None,
    }
}

/// Wingdings: the shapes bullets use, from the Unicode mapping of the font.
fn wingdings(c: u8) -> Option<u32> {
    Some(match c {
        0x21 => 0x270F, // pencil
        0x22 => 0x2702, // scissors
        0x28 => 0x260E, // telephone
        0x29 => 0x2706,
        0x2A => 0x2709, // envelope
        0x46 => 0x2690, // flag
        0x4A => 0x263A, // smiley
        0x4C => 0x2639,
        0x51 => 0x2708, // aeroplane
        0x52 => 0x263C, // sun
        0x54 => 0x2744, // snowflake
        0x56 => 0x271E, // cross
        0x58 => 0x2720, // maltese cross
        0x59 => 0x2721, // star of David
        0x5A => 0x262A,
        0x5B => 0x262F,
        0x5D => 0x2638,
        0x5E => 0x2648,
        0x6C => 0x25CF, // black circle
        0x6D => 0x274D, // shadowed white circle
        0x6E => 0x25A0, // black square
        0x6F => 0x25A1, // white square
        0x70 => 0x25A1, // heavy white square
        0x71 => 0x2751, // lower right shadowed white square
        0x72 => 0x2752, // upper right shadowed white square
        0x73 => 0x2B27, // black medium lozenge
        0x74 => 0x29EB, // black lozenge
        0x75 => 0x25C6, // black diamond
        0x76 => 0x2756, // black diamond minus white X
        0x77 => 0x2B25, // black medium diamond
        0x78 => 0x2327, // X in a rectangle box
        0x79 => 0x2353, // APL functional symbol quad up caret
        0x7A => 0x2318, // place of interest sign
        0x7B => 0x2740, // white florette
        0x7C => 0x273F, // black florette
        0x7D => 0x275D, // heavy double turned comma quotation mark
        0x7E => 0x275E,
        0x9E => 0xB7,   // middle dot
        0x9F => 0x2022, // bullet
        0xA0 => 0x25AA, // black small square
        0xA1 => 0x25CB, // white circle
        0xA4 => 0x25C9, // fisheye
        0xA5 => 0x25CE, // bullseye
        0xA7 => 0x25AA, // black small square
        0xA8 => 0x25FB, // white medium square
        0xAB => 0x2726,
        0xAC => 0x2605, // black star
        0xAD => 0x2736, // six pointed black star
        0xAE => 0x2734, // eight pointed black star
        0xAF => 0x2739, // twelve pointed black star
        0xB0 => 0x2735, // eight pointed pinwheel star
        0xB2 => 0x2316, // position indicator
        0xB3 => 0x27E1,
        0xB4 => 0x2311,
        0xB6 => 0x272A, // circled white star
        0xB7 => 0x2730, // shadowed white star
        0xD8 => 0x27A2, // three-D top-lighted rightwards arrowhead
        0xD9 => 0x27A3,
        0xDC => 0x2794,
        0xE0 => 0x2794,
        0xE8 => 0x2794, // heavy wide-headed rightwards arrow
        0xEF => 0x21E6, // leftwards white arrow
        0xF0 => 0x21E8, // rightwards white arrow
        0xF1 => 0x21E7, // upwards white arrow
        0xF2 => 0x21E9, // downwards white arrow
        0xF3 => 0x2B04, // left right white arrow
        0xF4 => 0x21F3, // up down white arrow
        0xF8 => 0x27A2,
        0xFB => 0x2717, // ballot X
        0xFC => 0x2714, // heavy check mark
        0xFD => 0x2612, // ballot box with X
        0xFE => 0x2611, // ballot box with check
        _ => return None,
    })
}

/// Wingdings 2: no table (no mapping this module could cite); every code is the plain bullet.
fn wingdings2(_c: u8) -> Option<u32> {
    None
}

/// Wingdings 3: only the arrow bullet of the Office themes (`}`, a right-pointing triangle; checked
/// against LibreOffice's drawing of such a bullet); every other code is the plain bullet.
fn wingdings3(c: u8) -> Option<u32> {
    match c {
        0x7D => Some(0x25B6),
        _ => None,
    }
}

/// Webdings: no table, as for Wingdings 2.
fn webdings(_c: u8) -> Option<u32> {
    None
}

#[cfg(test)]
#[path = "symbol_font_tests.rs"]
mod tests;
