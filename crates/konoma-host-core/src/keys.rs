//! `[keys]`: the user's key overrides, as the raw tables the keymap layer interprets.

use std::collections::HashMap;

use serde::Deserialize;

/// Raw representation of the config file's `[keys.<context>]` (context name -> (chord string -> action string)).
/// The keymap layer does not depend on serde: config loading populates this type and hands it over.
#[derive(Debug, Clone, Default)]
pub struct KeysFileConfig {
    /// Context name (`global`, `tree`, `text`, `git.changes`, ...; the old surface names such as
    /// `preview_text` are read too) -> (chord string -> action string).
    pub surfaces: HashMap<String, HashMap<String, String>>,
}

/// Keybinding settings (`[keys]`).
///
/// Two systems coexist in the same `[keys]` table:
/// - **New form (Run2)**: subtables under `[keys.<context>]` (context name -> (chord string -> action string)).
///   Interpreted by the the keymap layer layer. serde flatten collects only the subtables directly under `[keys]` into `surfaces`.
/// - **Old form (backward-compat alias)**: the `copy_prefix`/`copy_*` scalars for path copy (FR-6). Kept for now.
///   The named fields consume them first, so they do not end up on the flatten side (`surfaces`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    /// Copy prefix (default "c"; "y" for vim-style yank). [backward-compat alias]
    pub copy_prefix: String,
    /// Suffix key for each copy target. [backward-compat alias]
    pub copy_name: String, // file name
    pub copy_full: String,     // full path
    pub copy_relative: String, // relative path
    pub copy_parent: String,   // parent directory
    /// The raw new-form `[keys.<context>]` table (context name -> (chord string -> action string)).
    /// Example: `surfaces["tree"]["space d"] = "file_delete"`. Interpreted by `KeyMap::from_config`.
    #[serde(flatten)]
    pub surfaces: HashMap<String, HashMap<String, String>>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            copy_prefix: "c".into(),
            copy_name: "n".into(),
            copy_full: "p".into(),
            copy_relative: "r".into(),
            copy_parent: "d".into(),
            surfaces: HashMap::new(),
        }
    }
}

impl KeysConfig {
    /// Builds the settings interpreted by the keymap layer (the keymap layer).
    /// Passes the new-form `[keys.<context>]` (`surfaces`) through as-is, while mapping the old `copy_*` aliases onto the `y` leader.
    ///
    /// The old aliases add `"y <suffix>"` to the Copy leader **only for suffixes the user changed from the default**
    /// (when left at the default it respects the new defaults n/r/f/p and does not clobber them). Since leaders are surface-independent,
    /// they are placed in the `global` table for convenience (the surface name does not matter because the keymap routes prefix `y` -> Copy). Existing chords are not broken.
    /// The old `copy_prefix` is not mapped because the new scheme fixes it to `y` (open_risk).
    pub fn to_keymap_config(&self) -> KeysFileConfig {
        let mut surfaces = self.surfaces.clone();
        let defaults = KeysConfig::default();
        let aliases: [(&str, &str, &str); 4] = [
            (&self.copy_name, &defaults.copy_name, "copy_name"),
            (
                &self.copy_relative,
                &defaults.copy_relative,
                "copy_relative",
            ),
            (&self.copy_full, &defaults.copy_full, "copy_full"),
            (&self.copy_parent, &defaults.copy_parent, "copy_parent"),
        ];
        for (cur, def, action) in aliases {
            let cur = cur.trim();
            if cur.is_empty() || cur == def {
                continue;
            }
            if let Some(c) = cur.chars().next() {
                surfaces
                    .entry("global".to_string())
                    .or_default()
                    .entry(format!("y {c}"))
                    .or_insert_with(|| action.to_string());
            }
        }
        KeysFileConfig { surfaces }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_aliases_map_to_nothing() {
        assert!(KeysConfig::default().to_keymap_config().surfaces.is_empty());
    }

    #[test]
    fn changed_alias_goes_to_the_global_y_leader_without_clobbering() {
        let mut k = KeysConfig {
            copy_full: "P".into(),
            ..Default::default()
        };
        k.surfaces
            .entry("global".into())
            .or_default()
            .insert("y P".into(), "copy_name".into());
        let c = k.to_keymap_config();
        assert_eq!(
            c.surfaces["global"]["y P"], "copy_name",
            "explicit chord wins"
        );
        k.surfaces.clear();
        let c = k.to_keymap_config();
        assert_eq!(c.surfaces["global"]["y P"], "copy_full");
    }

    #[test]
    fn blank_alias_is_ignored() {
        let k = KeysConfig {
            copy_name: "  ".into(),
            ..Default::default()
        };
        assert!(k.to_keymap_config().surfaces.is_empty());
    }
}
