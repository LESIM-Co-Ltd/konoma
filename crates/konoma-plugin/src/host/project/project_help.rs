use super::*;

/// Translated section headings and descriptions; the SDK owns no message catalog.
#[derive(Default)]
pub struct ProjectHelpWords<'a> {
    /// Title of a version-control screen's main section (empty: the graph's `graph`).
    pub screen_title: &'a str,
    /// This screen's filter wording (`/`).
    pub screen_filter: &'a str,
    /// What the home screen's leave row says (`q` closes the project view).
    pub close_view: &'a str,
    /// What the project's tool key (`!`) launches.
    pub tool: &'a str,
    /// The help word of each jump key, by the screen it jumps to.
    pub jumps: &'a [(&'a str, &'a str)],
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
    /// A pushed version-control detail's own rows: the horizontal scroll and line ends, the
    /// pages, the half pages, the diff layout and the back key.
    pub detail_hscroll: &'a str,
    pub detail_page: &'a str,
    pub detail_half_page: &'a str,
    pub detail_layout: &'a str,
    pub detail_back: &'a str,
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
        if self.facts.pushed
            && self.facts.vcs_jj.is_some()
            && self.facts.content == Some(ProjectPresentationContent::Diff)
        {
            // A pushed version-control detail is a diff, not a list: it has no filter, refresh
            // or screen cycle (none of them act on it), only the old hub's detail rows. `h` and
            // `l` pan here (the Left key leaves lists only), so the leave row is `q / Esc`.
            let motions = &self.facts.motions;
            plugin.title = words.screen_title.into();
            // Rows the plugin itself declares for this screen follow the host's rows.
            let declared = std::mem::take(&mut plugin.rows);
            let hscroll: Vec<String> = [&motions.pairs.hscroll, &motions.pairs.line_ends]
                .into_iter()
                .map(MotionPairs::text)
                .filter(|text| !text.is_empty())
                .collect();
            let pages = [
                (&motions.pairs.page, words.detail_page),
                (&motions.pairs.half_page, words.detail_half_page),
            ];
            let mut rows: Vec<(String, &str)> = vec![(hscroll.join("  "), words.detail_hscroll)];
            rows.extend(
                pages
                    .into_iter()
                    .map(|(pair, word)| (MotionPairs::text(pair), word)),
            );
            rows.push((joined(&motions.diff_layout, " / "), words.detail_layout));
            rows.push((joined(&motions.ends, " / "), words.ends));
            rows.push((
                joined(
                    &motions.leave_keys(false, &[KeyPress::key(KeyCode::Esc)]),
                    " / ",
                ),
                words.detail_back,
            ));
            plugin.rows = rows
                .into_iter()
                .filter(|(keys, _)| !keys.is_empty())
                .map(|(keys, word)| (keys, word.into()))
                .chain(declared)
                .collect();
            sections.push(plugin);
            let scroll = MotionPairs::text(&motions.pairs.scroll);
            if !scroll.is_empty() {
                sections.push(CoreSection {
                    title: words.movement.into(),
                    rows: vec![(format!("{scroll} / ↑ ↓"), words.scroll.into())],
                });
            }
        } else if self.facts.graph_jj.is_some() || self.facts.vcs_jj.is_some() {
            // A version-control screen, in the old hub's order: its own rows (the plugin's
            // declarations, with the jump keys and the new-tab key merged by footer priority
            // where the old hub listed them), then filter, refresh, the screen cycle, the
            // tool and the leave row; the movement keys form a section of their own.
            let graph = self.facts.graph_jj.is_some();
            plugin.title = if words.screen_title.is_empty() {
                words.graph.into()
            } else {
                words.screen_title.into()
            };
            let motions = &self.facts.motions;
            let mut rows: Vec<(u8, (String, String))> = self
                .plugin_row_footers
                .iter()
                .copied()
                .zip(plugin.rows.drain(..))
                .collect();
            if !graph {
                for jump in &self.jump_hints {
                    let Some((_, word)) = words.jumps.iter().find(|(id, _)| *id == jump.screen)
                    else {
                        continue;
                    };
                    rows.push((jump.footer, (help_key(jump.key), (*word).into())));
                }
                if let Some(control) = self.new_tab_control() {
                    rows.push((
                        self.facts.new_tab_footer.unwrap_or(u8::MAX),
                        (control.rendered_keys(), words.new_tab.into()),
                    ));
                }
                rows.sort_by_key(|(priority, _)| *priority);
            }
            plugin.rows = rows.into_iter().map(|(_, row)| row).collect();
            if !motions.ends.is_empty() {
                plugin
                    .rows
                    .insert(0, (joined(&motions.ends, " / "), words.ends.into()));
            }
            if self.facts.filter && !motions.filter.is_empty() {
                let word = if words.screen_filter.is_empty() {
                    words.search
                } else {
                    words.screen_filter
                };
                plugin
                    .rows
                    .push((joined(&motions.filter, " / "), word.into()));
            }
            // A screen that declares marks names the mark key too (the old hub had none).
            plugin.rows.extend(self.mark_row(words));
            plugin.rows.extend([
                ("r".into(), words.refresh.into()),
                (cycle_keys(), words.screens.into()),
            ]);
            if !self.facts.tool_keys.is_empty() {
                plugin
                    .rows
                    .push((joined(&self.facts.tool_keys, " / "), words.tool.into()));
            }
            let leave = if self.facts.home && !words.close_view.is_empty() {
                words.close_view
            } else {
                words.back_to_changes
            };
            plugin.rows.push((self.leave_keys(), leave.into()));
            sections.push(plugin);
            let scroll = motions.plain_scroll();
            if !scroll.is_empty() {
                sections.push(CoreSection {
                    title: words.movement.into(),
                    rows: vec![(
                        joined(&scroll, " / "),
                        if graph {
                            words.move_commit
                        } else {
                            words.move_list
                        }
                        .into(),
                    )],
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
                navigation.rows.extend(self.mark_row(words));
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
    /// The `V` row of a list screen that declares marks, from the mark control the footer and
    /// the key handling share (so the help names the key that acts).
    fn mark_row(&self, words: &ProjectHelpWords<'_>) -> Option<(String, String)> {
        if self.facts.content != Some(ProjectPresentationContent::List) || !self.facts.marks {
            return None;
        }
        let control = self
            .controls
            .iter()
            .find(|c| c.id == ProjectControlId::Mark)?;
        Some((control.rendered_keys(), words.mark.into()))
    }

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
