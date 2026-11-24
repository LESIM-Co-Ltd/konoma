//! Display-independent colors and graph-cell emphasis used by the VCS readers.

/// The colors used by file statuses and commit-graph lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsColor {
    Reset,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    LightRed,
    LightGreen,
    LightYellow,
}

/// A graph cell's foreground and emphasis. An unset foreground inherits its surroundings;
/// an explicit `Reset` foreground requests the terminal's default color.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GraphStyle {
    pub fg: Option<VcsColor>,
    pub bold: bool,
}

impl GraphStyle {
    pub const fn foreground(color: VcsColor) -> Self {
        Self {
            fg: Some(color),
            bold: false,
        }
    }
}
