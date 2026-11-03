//! The `,` menu's two display options, independent of any host: how a path is written in a title
//! (`p`) and whether text and documents wrap (`w`). konoma and the standalone app both apply a
//! menu item through [`ViewOptions::apply`], so the result of an item is the same on both.

use crate::keymap::CoreAction;
use crate::paths::{home_relative, rel_to_open};
use std::path::Path;

/// Path display style of a title. Cycled with `p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    /// Relative to the launch directory name (e.g. konoma/src/main.rs). Default.
    Relative,
    /// Relative to HOME (e.g. ~/work/konoma).
    Home,
    /// Full path (e.g. /Users/me/work/konoma).
    Full,
}

impl PathStyle {
    pub fn parse(s: &str) -> Self {
        match s {
            "home" => Self::Home,
            "full" => Self::Full,
            _ => Self::Relative,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Relative => Self::Home,
            Self::Home => Self::Full,
            Self::Full => Self::Relative,
        }
    }

    /// `path` written in this style; `open_dir` is the base of [`PathStyle::Relative`].
    pub fn format(self, open_dir: &Path, path: &Path) -> String {
        match self {
            Self::Full => path.display().to_string(),
            Self::Home => home_relative(path),
            Self::Relative => rel_to_open(open_dir, path),
        }
    }
}

/// What the caller must redo after a `,` item changed an option.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effect {
    /// The line breaks of a decorated document are stale (wrap changed).
    pub relayout: bool,
    /// The horizontal offset means nothing any more (wrap turned on).
    pub hscroll_home: bool,
    /// The title must be written again (path style changed).
    pub retitle: bool,
}

/// The options the `,` menu changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewOptions {
    pub wrap: bool,
    pub path_style: PathStyle,
}

impl ViewOptions {
    /// Applies a `,` menu item. An action that is not an option changes nothing.
    pub fn apply(&mut self, action: CoreAction) -> Effect {
        match action {
            CoreAction::CyclePathStyle => {
                self.path_style = self.path_style.next();
                Effect {
                    retitle: true,
                    ..Effect::default()
                }
            }
            CoreAction::ToggleWrap => {
                self.wrap = !self.wrap;
                Effect {
                    relayout: true,
                    hscroll_home: self.wrap,
                    ..Effect::default()
                }
            }
            _ => Effect::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_style_cycles_back_to_the_start() {
        let s = PathStyle::Relative;
        assert_eq!(s.next(), PathStyle::Home);
        assert_eq!(s.next().next(), PathStyle::Full);
        assert_eq!(s.next().next().next(), s);
        assert_eq!(PathStyle::parse("home"), PathStyle::Home);
        assert_eq!(PathStyle::parse("full"), PathStyle::Full);
        assert_eq!(PathStyle::parse("x"), PathStyle::Relative);
    }

    #[test]
    fn an_item_changes_its_option_and_reports_what_to_redo() {
        let mut o = ViewOptions {
            wrap: true,
            path_style: PathStyle::Relative,
        };
        let e = o.apply(CoreAction::ToggleWrap);
        assert!(!o.wrap && e.relayout && !e.hscroll_home && !e.retitle);
        let e = o.apply(CoreAction::ToggleWrap);
        assert!(o.wrap && e.relayout && e.hscroll_home);
        let e = o.apply(CoreAction::CyclePathStyle);
        assert_eq!(o.path_style, PathStyle::Home);
        assert!(e.retitle && !e.relayout);
        assert_eq!(o.apply(CoreAction::Quit), Effect::default());
        assert_eq!(o.path_style, PathStyle::Home);
    }

    #[test]
    fn a_path_is_written_in_each_style() {
        let dir = Path::new("/a/proj");
        let f = Path::new("/a/proj/src/x.rs");
        assert_eq!(PathStyle::Relative.format(dir, f), "proj/src/x.rs");
        assert_eq!(PathStyle::Full.format(dir, f), "/a/proj/src/x.rs");
        assert_eq!(PathStyle::Home.format(dir, f), home_relative(f));
    }
}
