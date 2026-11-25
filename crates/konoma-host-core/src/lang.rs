//! Display language and how the `ui.lang` setting is resolved.

/// Display language. Selected by the `ui.lang` setting (default en).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Jp,
}

impl Lang {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "jp" | "ja" | "japanese" | "日本語" => Self::Jp,
            _ => Self::En, // default to English
        }
    }

    /// Resolve the `ui.lang` setting. Unspecified-equivalent values (empty / "auto" / "system")
    /// follow the OS default language, which `os` supplies (called only on that branch); any other
    /// value is an explicit choice passed to `parse`. The OS lookup is injected so a host can pin
    /// it in tests (konoma's own tests always get English).
    pub fn resolve_with(s: &str, os: impl FnOnce() -> Lang) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "system" => os(),
            _ => Self::parse(s),
        }
    }

    /// The OS default language via `sys-locale` (no external process: on macOS this links
    /// CoreFoundation directly, on Linux/BSD it reads `LANGUAGE` -> `LC_ALL` -> `LC_MESSAGES` ->
    /// `LANG`). English if undeterminable. The tag may use either separator (`ja-JP` or `ja_JP`);
    /// `from_lang_tag` only checks the leading "ja".
    pub fn from_os_locale() -> Self {
        match sys_locale::get_locale() {
            Some(tag) => Self::from_lang_tag(&tag),
            None => Self::En,
        }
    }

    /// Jp if the language part of a language tag/locale string is Japanese (starts with "ja"), otherwise En.
    /// Examples: "ja", "ja-JP", "ja_JP.UTF-8" -> Jp / "en-US", "en_US.UTF-8", "C", "" -> En.
    pub fn from_lang_tag(s: &str) -> Self {
        if s.trim().to_ascii_lowercase().starts_with("ja") {
            Self::Jp
        } else {
            Self::En
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_values_never_ask_the_os() {
        let boom = || -> Lang { panic!("os lookup must not run for an explicit value") };
        assert_eq!(Lang::resolve_with("jp", boom), Lang::Jp);
        assert_eq!(Lang::resolve_with("ja", boom), Lang::Jp);
        assert_eq!(Lang::resolve_with("  Japanese ", boom), Lang::Jp);
        assert_eq!(Lang::resolve_with("en", boom), Lang::En);
        assert_eq!(
            Lang::resolve_with("fr", boom),
            Lang::En,
            "unknown is English"
        );
    }

    #[test]
    fn auto_like_values_follow_the_os_closure() {
        for s in ["", "   ", "auto", "AUTO", " System "] {
            assert_eq!(Lang::resolve_with(s, || Lang::Jp), Lang::Jp, "{s:?}");
            assert_eq!(Lang::resolve_with(s, || Lang::En), Lang::En, "{s:?}");
        }
    }

    #[test]
    fn os_closure_runs_exactly_once_on_the_auto_branch() {
        let mut n = 0;
        let _ = Lang::resolve_with("auto", || {
            n += 1;
            Lang::En
        });
        assert_eq!(n, 1);
    }

    #[test]
    fn lang_tags_in_either_separator_style() {
        assert_eq!(Lang::from_lang_tag("ja"), Lang::Jp);
        assert_eq!(Lang::from_lang_tag("ja-JP"), Lang::Jp);
        assert_eq!(Lang::from_lang_tag("ja_JP.UTF-8"), Lang::Jp);
        assert_eq!(Lang::from_lang_tag(" JA-jp "), Lang::Jp);
        assert_eq!(Lang::from_lang_tag("en-US"), Lang::En);
        assert_eq!(Lang::from_lang_tag("C"), Lang::En);
        assert_eq!(Lang::from_lang_tag(""), Lang::En);
    }

    #[test]
    fn parse_accepts_the_native_name() {
        assert_eq!(Lang::parse("日本語"), Lang::Jp);
        assert_eq!(Lang::parse(""), Lang::En);
    }
}
