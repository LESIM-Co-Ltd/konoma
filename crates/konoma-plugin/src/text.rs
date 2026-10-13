//! Strings in several languages.

use std::borrow::Cow;

/// A user-visible string. English is required; other languages are optional and fall back to
/// English. Language names are ISO 639-1 codes: `"en"`, `"ja"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tr {
    en: Cow<'static, str>,
    others: Vec<(Cow<'static, str>, Cow<'static, str>)>,
}

impl Tr {
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
