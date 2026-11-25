//! Strings in several languages.

use std::borrow::Cow;

/// A user-visible string. English is required; other languages are optional and fall back to
/// English. Language names are ISO 639-1 codes: `"en"`, `"ja"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tr {
    en: Cow<'static, str>,
    others: Vec<(Cow<'static, str>, Cow<'static, str>)>,
}

/// The longest user-visible text the host keeps from a plugin, in characters: a message, a label,
/// a help line, a page name. Longer ones are cut at this length with a trailing `…` when the host
/// takes them in, so a plugin cannot hand the interface a text it has to copy and measure on
/// every frame.
pub const MAX_TEXT_CHARS: usize = 512;

/// The most languages besides English a [`Tr`] keeps when the host takes it in.
pub const MAX_TR_LANGUAGES: usize = 16;

/// Whether `c` is dropped from a text when the host takes it in: a control character other than
/// a newline or a tab (C0, DEL, C1: they would reach the terminal), and U+10EEEE, the character
/// the kitty graphics protocol uses for its Unicode placeholders (it would show an image the host
/// has placed).
fn is_stripped(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t') || c == '\u{10EEEE}'
}

/// `s` as the host keeps it: control characters (other than a newline and a tab) and U+10EEEE
/// removed, at most [`MAX_TEXT_CHARS`] characters, the last of a cut one an ellipsis. Whole
/// characters are kept (a cut never lands inside one).
pub fn clip_text(s: &str) -> std::borrow::Cow<'_, str> {
    let clean: std::borrow::Cow<'_, str> = if s.chars().any(is_stripped) {
        std::borrow::Cow::Owned(s.chars().filter(|c| !is_stripped(*c)).collect())
    } else {
        std::borrow::Cow::Borrowed(s)
    };
    match clean.char_indices().nth(MAX_TEXT_CHARS) {
        None => clean,
        Some((end, _)) => {
            let keep = clean[..end]
                .char_indices()
                .nth(MAX_TEXT_CHARS - 1)
                .map_or(end, |(i, _)| i);
            std::borrow::Cow::Owned(format!("{}…", &clean[..keep]))
        }
    }
}

impl Tr {
    /// The same text with every language cut to [`MAX_TEXT_CHARS`] (see [`clip_text`]).
    pub fn clipped(&self) -> Tr {
        Tr {
            en: clip_text(&self.en).into_owned().into(),
            // At most MAX_TR_LANGUAGES besides English, each language name short: `get` walks them.
            others: self
                .others
                .iter()
                .take(MAX_TR_LANGUAGES)
                .map(|(l, t)| {
                    (
                        l.chars().take(35).collect::<String>().into(),
                        clip_text(t).into_owned().into(),
                    )
                })
                .collect(),
        }
    }

    /// A string with only the English text.
    pub fn new(en: impl Into<Cow<'static, str>>) -> Tr {
        Tr {
            en: en.into(),
            others: Vec::new(),
        }
    }

    /// Add (or replace) the text for `lang`. `"en"` replaces the English text.
    pub fn with(
        mut self,
        lang: impl Into<Cow<'static, str>>,
        text: impl Into<Cow<'static, str>>,
    ) -> Tr {
        let lang = lang.into();
        let text = text.into();
        if lang == "en" {
            self.en = text;
        } else if let Some(slot) = self.others.iter_mut().find(|(l, _)| *l == lang) {
            slot.1 = text;
        } else {
            self.others.push((lang, text));
        }
        self
    }

    /// Add the Japanese text.
    pub fn ja(self, text: impl Into<Cow<'static, str>>) -> Tr {
        self.with("ja", text)
    }

    /// The text for `lang`, or the English text when there is none.
    pub fn get(&self, lang: &str) -> &str {
        if lang != "en" {
            if let Some((_, t)) = self.others.iter().find(|(l, _)| l == lang) {
                return t;
            }
        }
        &self.en
    }

    /// The English text.
    /// The `(language, text)` pairs besides English, in declaration order.
    pub fn translations(&self) -> impl Iterator<Item = (&str, &str)> {
        self.others.iter().map(|(l, t)| (&**l, &**t))
    }

    pub fn en(&self) -> &str {
        &self.en
    }
}

impl From<&'static str> for Tr {
    fn from(s: &'static str) -> Tr {
        Tr::new(s)
    }
}

impl From<String> for Tr {
    fn from(s: String) -> Tr {
        Tr::new(s)
    }
}

/// Build a [`Tr`]: `t!(en: "width", ja: "幅")`. The Japanese text is optional.
#[macro_export]
macro_rules! t {
    (en: $en:expr, ja: $ja:expr $(,)?) => {
        $crate::text::Tr::new($en).ja($ja)
    };
    (en: $en:expr $(,)?) => {
        $crate::text::Tr::new($en)
    };
}
