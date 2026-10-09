use super::*;

#[test]
fn the_symbol_fonts_are_recognised_by_name() {
    assert_eq!(symbol_font("Symbol"), Some(SymbolFont::Symbol));
    assert_eq!(symbol_font("symbol"), Some(SymbolFont::Symbol));
    assert_eq!(symbol_font("Wingdings"), Some(SymbolFont::Wingdings));
    assert_eq!(symbol_font("Wingdings 2"), Some(SymbolFont::Wingdings2));
    assert_eq!(symbol_font("Wingdings 3"), Some(SymbolFont::Wingdings3));
    assert_eq!(symbol_font("Webdings"), Some(SymbolFont::Webdings));
    assert_eq!(symbol_font("Symbol MT"), Some(SymbolFont::Symbol));
    assert_eq!(symbol_font("Arial"), None);
    assert_eq!(symbol_font(""), None);
}

#[test]
fn symbol_codes_map_to_the_characters_of_the_font() {
    // the examples PowerPoint bullets use
    assert_eq!(map("Symbol", '\u{F02D}'), '\u{2212}'); // minus
    assert_eq!(map("Symbol", '\u{F0B7}'), '\u{2022}'); // bullet
    assert_eq!(map("Symbol", '\u{F0D8}'), '\u{00AC}'); // not
    assert_eq!(map("Symbol", '\u{F061}'), '\u{03B1}'); // alpha
    assert_eq!(map("Symbol", '\u{F0AE}'), '\u{2192}'); // right arrow
    assert_eq!(map("Symbol", '\u{F0A5}'), '\u{221E}'); // infinity
    assert_eq!(map("Symbol", '\u{F0E5}'), '\u{2211}'); // sum
                                                       // the plain code (charset 2 without the private-use offset) is the same
    assert_eq!(map("Symbol", '\u{2D}'), '\u{2212}');
    assert_eq!(map("Symbol", 'a'), '\u{03B1}');
}

#[test]
fn symbol_tables_are_complete_for_their_windows() {
    for c in (0x20u8..=0x7E).chain(0xA0..=0xFE) {
        assert!(symbol(c).is_some(), "{c:#x}");
        assert!(char::from_u32(symbol(c).unwrap()).is_some());
    }
    // holes of the encoding
    assert_eq!(symbol(0x7F), None);
    assert_eq!(symbol(0x9F), None);
    assert_eq!(symbol(0xFF), None);
    // an unknown code is the plain bullet
    assert_eq!(map("Symbol", '\u{F0FF}'), '\u{2022}');
}

#[test]
fn wingdings_bullets() {
    for (code, want) in [
        (0xF06C, '\u{25CF}'),
        (0xF06E, '\u{25A0}'),
        (0xF06F, '\u{25A1}'),
        (0xF071, '\u{2751}'),
        (0xF072, '\u{2752}'),
        (0xF075, '\u{25C6}'),
        (0xF076, '\u{2756}'),
        (0xF0A7, '\u{25AA}'),
        (0xF0A8, '\u{25FB}'),
        (0xF0D8, '\u{27A2}'),
        (0xF0E8, '\u{2794}'),
        (0xF0FC, '\u{2714}'),
        (0xF0FB, '\u{2717}'),
        (0xF0FD, '\u{2612}'),
        (0xF0FE, '\u{2611}'),
        (0xF0AB, '\u{2726}'),
        (0xF0AC, '\u{2605}'),
    ] {
        assert_eq!(
            map("Wingdings", char::from_u32(code).unwrap()),
            want,
            "{code:#x}"
        );
    }
    // the plain code too
    assert_eq!(map("Wingdings", 'l'), '\u{25CF}');
    // unknown codes and the fonts without a table are the plain bullet
    assert_eq!(map("Wingdings", '\u{F0FF}'), '\u{2022}');
    assert_eq!(map("Wingdings 2", '\u{F06C}'), '\u{2022}');
    assert_eq!(map("Wingdings 3", '\u{F06C}'), '\u{2022}');
    assert_eq!(map("Wingdings 3", '\u{F07D}'), '\u{25B6}');
    assert_eq!(map("Webdings", '\u{F06C}'), '\u{2022}');
}

#[test]
fn other_fonts_and_other_characters_are_left_alone() {
    assert_eq!(map("Arial", '\u{F06C}'), '\u{F06C}');
    assert_eq!(map("Arial", '\u{2022}'), '\u{2022}');
    assert_eq!(map("Wingdings", '\u{2022}'), '\u{2022}');
    assert_eq!(map("Symbol", '\u{3042}'), '\u{3042}');
    assert_eq!(map("Symbol", ' '), ' ');
}

#[test]
fn the_glyph_scale_enlarges_only_the_wingdings_shapes() {
    assert_eq!(glyph_scale("Wingdings", '\u{F06C}'), 1.3);
    assert_eq!(glyph_scale("Wingdings", '\u{F06E}'), 1.3);
    assert_eq!(glyph_scale("Wingdings", '\u{F0A7}'), 1.0);
    assert_eq!(glyph_scale("Symbol", '\u{F0B7}'), 1.0);
    assert_eq!(glyph_scale("Arial", '\u{F06C}'), 1.0);
    assert_eq!(glyph_scale("Wingdings", '\u{3042}'), 1.0);
}
