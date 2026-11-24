//! Bounds and path/key checks for project declarations and action prompts.

use super::*;
use crate::keys::{reserved, KeySpec};
use crate::project::Choice;
use std::path::Component;

pub(super) fn problem(m: &Manifest) -> Option<ManifestReason> {
    if let Some(p) = &m.project {
        if p.watch_paths.len() > MAX_WATCH_PATHS {
            return Some(ManifestReason::TooMany {
                kind: ManifestCount::WatchPaths,
                count: p.watch_paths.len(),
                limit: MAX_WATCH_PATHS,
            });
        }
        for (index, path) in p.watch_paths.iter().enumerate() {
            if path.as_os_str().is_empty()
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
            {
                return Some(ManifestReason::InvalidWatchPath { index });
            }
        }
        for (index, marker) in p.markers.iter().enumerate() {
            if marker.name.is_empty()
                || marker.name.len() > MAX_MARKER_BYTES
                || matches!(marker.name.as_str(), "." | "..")
                || marker.name.contains('/')
                || marker.name.chars().any(is_unclean)
            {
                return Some(ManifestReason::InvalidMarker {
                    index,
                    limit: MAX_MARKER_BYTES,
                });
            }
        }
        for screen in &p.screens {
            if let Some(key) = screen.key {
                // `p` is the project entry key. Within a project the host handles screen
                // jumps before verbs, so it may also return to the changes screen.
                if key.is_control()
                    || (key != 'p' && reserved("list").iter().any(|(k, _)| *k == KeySpec::ch(key)))
                {
                    return Some(ManifestReason::InvalidScreenKey {
                        key: key.escape_default().to_string(),
                    });
                }
            }
        }
    }
    for action in &m.actions {
        let id = || display_name(&action.id);
        if action.refresh.len() > MAX_REFRESH_TARGETS {
            return Some(ManifestReason::TooMany {
                kind: ManifestCount::RefreshTargets,
                count: action.refresh.len(),
                limit: MAX_REFRESH_TARGETS,
            });
        }
        for target in &action.refresh {
            if let crate::project::RefreshTarget::Screen { screen } = target {
                if !m
                    .project
                    .as_ref()
                    .is_some_and(|p| p.screens.iter().any(|s| &s.id == screen))
                {
                    return Some(ManifestReason::UnknownRefreshScreen {
                        action: id(),
                        screen: display_name(screen),
                    });
                }
            }
        }
        if let Some(c) = &action.confirmation {
            if c.choices.is_empty() {
                return Some(ManifestReason::EmptyConfirmation { action: id() });
            }
            if c.choices.len() > MAX_CHOICES {
                return Some(ManifestReason::TooMany {
                    kind: ManifestCount::Choices,
                    count: c.choices.len(),
                    limit: MAX_CHOICES,
                });
            }
            let mut seen = std::collections::HashSet::new();
            for c in &c.choices {
                if c.key.is_control()
                    || matches!(c.key, 'n' | 'N')
                    || crate::keys::confirmation_reserved()
                        .iter()
                        .any(|(key, _)| *key == KeySpec::ch(c.key))
                {
                    return Some(ManifestReason::InvalidConfirmationKey {
                        action: id(),
                        key: c.key.escape_default().to_string(),
                    });
                }
                if !seen.insert(c.key) {
                    return Some(ManifestReason::DuplicateConfirmationKey {
                        action: id(),
                        key: c.key.escape_default().to_string(),
                    });
                }
                if let Some(reason) = choice_problem(&c.choice, &action.id) {
                    return Some(reason);
                }
                if let Some(status) = &c.status {
                    if let Some(reason) = choice_label_problem(status, &action.id) {
                        return Some(reason);
                    }
                }
            }
        }
        if let Some(c) = &action.choice_input {
            if c.choices.len() > MAX_CHOICES {
                return Some(ManifestReason::TooMany {
                    kind: ManifestCount::Choices,
                    count: c.choices.len(),
                    limit: MAX_CHOICES,
                });
            }
            for c in &c.choices {
                if let Some(reason) = choice_problem(c, &action.id) {
                    return Some(reason);
                }
            }
        }
    }
    None
}

fn choice_problem(c: &Choice, action: &str) -> Option<ManifestReason> {
    if c.value.len() > MAX_CHOICE_VALUE_BYTES {
        return Some(ManifestReason::ChoiceValueTooLong {
            action: display_name(action),
            limit: MAX_CHOICE_VALUE_BYTES,
        });
    }
    choice_label_problem(&c.label, action)
}

fn choice_label_problem(label: &crate::Tr, action: &str) -> Option<ManifestReason> {
    if std::iter::once(label.en())
        .chain(label.translations().map(|(_, t)| t))
        .any(|t| t.chars().count() > crate::text::MAX_TEXT_CHARS)
    {
        return Some(ManifestReason::ChoiceLabelTooLong {
            action: display_name(action),
            limit: crate::text::MAX_TEXT_CHARS,
        });
    }
    None
}
