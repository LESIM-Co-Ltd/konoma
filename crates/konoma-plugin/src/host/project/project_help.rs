use super::*;

/// Translated section headings and descriptions; the SDK owns no message catalog.
pub struct ProjectHelpWords<'a> {
    pub plugin: &'a str,
    pub project: &'a str,
    pub picker: &'a str,
    pub graph: &'a str,
    pub new_tab: &'a str,
    pub move_list: &'a str,
    pub move_commit: &'a str,
    pub ends: &'a str,
    pub apply: &'a str,
    pub cancel: &'a str,
    pub filter: &'a str,
    pub mark: &'a str,
    pub search: &'a str,
    pub refresh: &'a str,
    pub screens: &'a str,
    /// Title of the graph's separate movement section.
    pub movement: &'a str,
    pub back_to_changes: &'a str,
    pub cycle: &'a str,
    pub back: &'a str,
    pub scroll: &'a str,
    pub copy: &'a str,
    pub actions: &'a str,
}

/// Subtract explained aliases individually, retaining every uncovered key.
pub fn remaining_project_open_control(
    mut control: ProjectControl,
    covered: &[KeyPress],
) -> Option<ProjectControl> {
    control.keys.retain(|key| !covered.contains(key));
    (!control.keys.is_empty()).then_some(control)
}

impl ProjectPresentation {
    /// Project help, including its menus, from this same redraw's resolved values.
    pub fn help_sections(
        &self,
        words: &ProjectHelpWords<'_>,
        screens: &[(String, String)],
    ) -> Vec<CoreSection> {
        let mut sections = Vec::new();
        let mut plugin = CoreSection {
            title: words.plugin.into(),
            rows: self.plugin_rows.clone(),
        };
        if self.facts.picker {
            plugin.title = words.picker.into();
            for (keys, description) in &mut plugin.rows {
                if description == words.apply {
                    *keys = "l/Enter".into();
                }
            }
            plugin.rows.splice(
                0..0,
                [
                    ("j/k".into(), words.move_list.into()),
                    ("g/G".into(), words.ends.into()),
                ],
            );
            plugin
                .rows
                .push(("h/Esc  ·  q".into(), words.cancel.into()));
            return vec![plugin];
        }
        if self.facts.graph_jj.is_some() {
            plugin.title = words.graph.into();
            let motions = &self.facts.motions;
            if !motions.ends.is_empty() {
                plugin
                    .rows
                    .insert(0, (joined(&motions.ends, " / "), words.ends.into()));
            }
            if self.facts.filter && !motions.filter.is_empty() {
                plugin
                    .rows
                    .push((joined(&motions.filter, " / "), words.search.into()));
            }
            plugin.rows.extend([
                ("r".into(), words.refresh.into()),
                (cycle_keys(), words.screens.into()),
                (self.leave_keys(), words.back_to_changes.into()),
            ]);
            sections.push(plugin);
            let scroll = motions.plain_scroll();
            if !scroll.is_empty() {
                sections.push(CoreSection {
                    title: words.movement.into(),
                    rows: vec![(joined(&scroll, " / "), words.move_commit.into())],
                });
            }
        } else {
            if !plugin.rows.is_empty() {
                sections.push(plugin);
            }
            let mut navigation = CoreSection {
                title: words.project.into(),
                rows: Vec::new(),
            };
            if let Some(control) = self.new_tab_control() {
                navigation
                    .rows
                    .push((control.rendered_keys(), words.new_tab.into()));
            }
            if let Some(hints) = self.text_help() {
                navigation.rows.extend_from_slice(hints);
            } else {
                if let Some(control) = self
                    .open_control()
                    .cloned()
                    .and_then(|c| remaining_project_open_control(c, &self.covered))
                {
                    navigation
                        .rows
                        .push((control.rendered_keys(), control.label));
                }
                if !self.facts.pushed {
                    // A version-control project lists the screens it cycles; any other
                    // provider keeps the generic wording.
                    let description = if self.facts.vcs_jj.is_some() {
                        words.screens
                    } else {
                        words.cycle
                    };
                    navigation.rows.push((cycle_keys(), description.into()));
                }
                let joined = |keys: &[KeyPress]| joined(keys, "/");
                let motions = &self.facts.motions;
                let plain = motions.plain_scroll();
                navigation.rows.push((
                    if motions.back.is_empty() {
                        "Esc".into()
                    } else {
                        format!("{} / Esc", joined(&motions.back))
                    },
                    words.back.into(),
                ));
                navigation.rows.push((
                    if plain.is_empty() {
                        "↑/↓".into()
                    } else {
                        format!("{} / ↑/↓", joined(&plain))
                    },
                    words.scroll.into(),
                ));
            }
            if self.facts.content == Some(ProjectPresentationContent::List) {
                if self.facts.filter && !self.facts.motions.filter.is_empty() {
                    navigation.rows.push((
                        self.facts
                            .motions
                            .filter
                            .iter()
                            .map(|k| help_key(*k))
                            .collect::<Vec<_>>()
                            .join("/"),
                        words.filter.into(),
                    ));
                }
                if self.facts.marks {
                    navigation.rows.push(("V".into(), words.mark.into()));
                }
            }
            if !self.facts.pushed {
                navigation.rows.extend_from_slice(screens);
            }
            sections.push(navigation);
        }
        let pairs: Vec<_> = self
            .copy_menu()
            .iter()
            .map(|item| (item.key, item.label.clone()))
            .collect();
        for key in &self.facts.copy_keys {
            sections.extend(konoma_host_core::help::menu_section(
                words.copy,
                &help_key(*key),
                &pairs,
            ));
        }
        let pairs: Vec<_> = self
            .actions_menu()
            .iter()
            .map(|item| (item.key, item.label.clone()))
            .collect();
        sections.extend(konoma_host_core::help::menu_section(
            words.actions,
            "Space",
            &pairs,
        ));
        sections
    }
}

fn joined(keys: &[KeyPress], separator: &str) -> String {
    keys.iter()
        .map(|k| help_key(*k))
        .collect::<Vec<_>>()
        .join(separator)
}

/// The screen-cycle keys. Tab and Shift-Tab are fixed keys of the canvas (not in the host's
/// keymap), so they are written in the help notation every other help screen uses.
fn cycle_keys() -> String {
    joined(
        &[KeyPress::key(KeyCode::Tab), KeyPress::key(KeyCode::BackTab)],
        " / ",
    )
}

impl ProjectPresentation {
    /// The graph's leave row: the Left keys and Esc zipped against the back keys, as the old
    /// hub wrote it (`h / Esc  ·  q`).
    fn leave_keys(&self) -> String {
        let motions = &self.facts.motions;
        let mut first = motions.left.clone();
        for key in &motions.back {
            if !first.contains(key) {
                first.push(*key);
            }
        }
        let esc = KeyPress::key(KeyCode::Esc);
        let columns: Vec<Vec<KeyPress>> = (0..first.len().max(1))
            .map(|i| {
                first
                    .get(i)
                    .copied()
                    .into_iter()
                    .chain((i == 0).then_some(esc))
                    .collect()
            })
            .collect();
        write_columns(&columns, true)
    }
}

pub(super) fn write_columns(cols: &[Vec<KeyPress>], zipped: bool) -> String {
    let mut seen = Vec::new();
    cols.iter()
        .map(|column| {
            column
                .iter()
                .filter_map(|key| {
                    let text = help_key(*key);
                    if seen.contains(&text) {
                        None
                    } else {
                        seen.push(text.clone());
                        Some(text)
                    }
                })
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .filter(|column| !column.is_empty())
        .collect::<Vec<_>>()
        .join(if zipped { "  ·  " } else { " / " })
}
