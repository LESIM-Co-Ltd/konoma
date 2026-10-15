//! A key press as plain data, for [`crate::Session::key`].
//!
//! This is the key a plugin *receives*. It is not [`crate::keys::KeyCode`], which names a key in a
//! manifest declaration and cannot hold a paste.

use std::ops::{BitOr, BitOrAssign};

/// Which key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A character. Shift is already in it (`'A'`), and also in [`KeyMods::SHIFT`] when the
    /// terminal reports it.
    Char(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    /// Shift-Tab.
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Esc,
    /// A function key, `F(1)` to `F(12)`.
    F(u8),
    /// A paste, which arrives as one key holding all the text.
    Paste(String),
}

/// The modifier keys held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyMods(u8);

impl KeyMods {
    pub const NONE: KeyMods = KeyMods(0);
    pub const CTRL: KeyMods = KeyMods(1);
    pub const ALT: KeyMods = KeyMods(2);
    pub const SHIFT: KeyMods = KeyMods(4);

    /// The raw bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: KeyMods) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no modifier is held.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for KeyMods {
    type Output = KeyMods;
    fn bitor(self, rhs: KeyMods) -> KeyMods {
        KeyMods(self.0 | rhs.0)
    }
}

impl BitOrAssign for KeyMods {
    fn bitor_assign(&mut self, rhs: KeyMods) {
        self.0 |= rhs.0;
    }
}

/// A key and the modifiers held with it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyMods,
}

impl Key {
    /// `code` with no modifier.
    pub fn new(code: KeyCode) -> Key {
        Key {
            code,
            mods: KeyMods::NONE,
        }
    }

    /// A plain character.
    pub fn char(c: char) -> Key {
        Key::new(KeyCode::Char(c))
    }

    /// `Ctrl` plus a character.
    pub fn ctrl(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: KeyMods::CTRL,
        }
    }

    /// A paste of `text`.
    pub fn paste(text: impl Into<String>) -> Key {
        Key::new(KeyCode::Paste(text.into()))
    }

    /// The same key with `mods` added.
    pub fn with_mods(mut self, mods: KeyMods) -> Key {
        self.mods |= mods;
        self
    }
}
