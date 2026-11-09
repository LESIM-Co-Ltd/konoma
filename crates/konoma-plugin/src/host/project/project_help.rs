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
            plugin.rows.splice(
                0..0,
                [
                    ("j/k / ↑/↓".into(), words.move_commit.into()),
                    ("g/G".into(), words.ends.into()),
                ],
            );
            plugin.rows.extend([
                ("/".into(), words.search.into()),
                ("r".into(), words.refresh.into()),
                ("Tab / Shift-Tab".into(), words.screens.into()),
                ("h/q/Esc".into(), words.back_to_changes.into()),
            ]);
            sections.push(plugin);
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
                    navigation
                        .rows
                        .push(("Tab / Shift-Tab".into(), words.cycle.into()));
                }
                navigation.rows.extend([
                    ("q / Esc".into(), words.back.into()),
                    ("j/k / ↑/↓".into(), words.scroll.into()),
                ]);
            }
            if self.facts.content == Some(ProjectPresentationContent::List) {
                if self.facts.filter {
                    navigation.rows.push(("/".into(), words.filter.into()));
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
