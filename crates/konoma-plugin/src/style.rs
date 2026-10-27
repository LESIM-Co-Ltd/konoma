//! Colors, modifiers, spans and lines. The host maps these onto its terminal library.

use std::ops::{BitOr, BitOrAssign};

/// A terminal color. The variants are in the same order as ratatui's `Color` so the host can map
/// them one to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Text modifiers. The bits are at the same positions as ratatui's `Modifier`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Modifier(u16);

impl Modifier {
    pub const EMPTY: Modifier = Modifier(0);
    pub const BOLD: Modifier = Modifier(1 << 0);
    pub const DIM: Modifier = Modifier(1 << 1);
    pub const ITALIC: Modifier = Modifier(1 << 2);
    pub const UNDERLINED: Modifier = Modifier(1 << 3);
    pub const SLOW_BLINK: Modifier = Modifier(1 << 4);
    pub const RAPID_BLINK: Modifier = Modifier(1 << 5);
    pub const REVERSED: Modifier = Modifier(1 << 6);
    pub const HIDDEN: Modifier = Modifier(1 << 7);
    pub const CROSSED_OUT: Modifier = Modifier(1 << 8);

    /// The raw bits.
    /// The modifiers named by `bits`; bits that are no modifier are dropped (a style that crossed
    /// a process boundary is untrusted data).
    pub const fn from_bits_truncate(bits: u16) -> Modifier {
        Modifier(bits & ((1 << 9) - 1))
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether every bit of `other` is set in `self`.
    pub const fn contains(self, other: Modifier) -> bool {
        self.0 & other.0 == other.0
    }

    /// Set the bits of `other`.
    pub fn insert(&mut self, other: Modifier) {
        self.0 |= other.0;
    }

    /// Whether no bit is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Modifier {
    type Output = Modifier;
    fn bitor(self, rhs: Modifier) -> Modifier {
        Modifier(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modifier {
    fn bitor_assign(&mut self, rhs: Modifier) {
        self.0 |= rhs.0;
    }
}

/// A style: colors plus added and removed modifiers (the same meaning as ratatui's
/// `add_modifier` / `sub_modifier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub add: Modifier,
    pub sub: Modifier,
}

impl Style {
    /// The empty style.
    pub fn new() -> Style {
        Style::default()
    }

    /// A style from all four parts (`Style` is `#[non_exhaustive]`, so it cannot be built with a
    /// struct expression outside this crate).
    pub fn from_parts(fg: Option<Color>, bg: Option<Color>, add: Modifier, sub: Modifier) -> Style {
        Style { fg, bg, add, sub }
    }

    pub fn fg(mut self, c: Color) -> Style {
        self.fg = Some(c);
        self
    }

    pub fn bg(mut self, c: Color) -> Style {
        self.bg = Some(c);
        self
    }

    pub fn add_modifier(mut self, m: Modifier) -> Style {
        self.add |= m;
        self
    }

    pub fn sub_modifier(mut self, m: Modifier) -> Style {
        self.sub |= m;
        self
    }

    pub fn bold(self) -> Style {
        self.add_modifier(Modifier::BOLD)
    }

    pub fn dim(self) -> Style {
        self.add_modifier(Modifier::DIM)
    }

    pub fn italic(self) -> Style {
        self.add_modifier(Modifier::ITALIC)
    }

    pub fn underlined(self) -> Style {
        self.add_modifier(Modifier::UNDERLINED)
    }

    pub fn reversed(self) -> Style {
        self.add_modifier(Modifier::REVERSED)
    }
}

/// A run of text in one style.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    /// Text with no style.
    pub fn raw(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            style: Style::default(),
        }
    }

    /// Text with `style`.
    pub fn styled(text: impl Into<String>, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
        }
    }
}

/// A line of spans. `style` applies to the whole line (under the spans' own styles).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Line {
    pub spans: Vec<Span>,
    pub style: Style,
}

impl Line {
    /// A line of one unstyled span.
    pub fn raw(text: impl Into<String>) -> Line {
        Line {
            spans: vec![Span::raw(text)],
            style: Style::default(),
        }
    }

    /// The line's text without styles.
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

impl From<Vec<Span>> for Line {
    fn from(spans: Vec<Span>) -> Line {
        Line {
            spans,
            style: Style::default(),
        }
    }
}

impl From<Span> for Line {
    fn from(span: Span) -> Line {
        Line::from(vec![span])
    }
}

impl From<String> for Line {
    fn from(s: String) -> Line {
        Line::raw(s)
    }
}

impl From<&str> for Line {
    fn from(s: &str) -> Line {
        Line::raw(s)
    }
}
