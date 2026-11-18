//! Adapt the shared project-setting readers to konoma's loaded configuration.
use std::collections::BTreeMap;

use konoma_plugin::config::{self as readers, OldNameReading, MOVED_PROJECT_SETTINGS};
use konoma_plugin::{Settings, Value};

type Renames = Vec<(String, String)>;

fn value<T>(reading: OldNameReading<T>, convert: impl FnOnce(T) -> Value) -> (Value, Renames) {
    (
        convert(reading.value),
        reading.renamed.into_iter().collect(),
    )
}

pub(super) fn read(root: &toml::Table) -> (BTreeMap<String, Settings>, Renames) {
    let mut settings = BTreeMap::<String, Settings>::new();
    let mut renamed = Vec::new();
    for moved in MOVED_PROJECT_SETTINGS {
        let old = root
            .get(moved.old_section)
            .and_then(|t| t.get(moved.old_key));
        let new = root
            .get("plugins")
            .and_then(|p| p.get(moved.plugin))
            .and_then(|t| t.get(moved.new_key));
        if old.is_none() && new.is_none() {
            continue;
        }
        // Locations and precedence belong to MOVED_PROJECT_SETTINGS and its readers only.
        let (setting, names) = match moved.old_key {
            "confirm_jj_sync" => value(readers::confirm_jj_sync(root), Value::Bool),
            "commit_meta_align" => {
                value(readers::commit_meta_align(root, moved.plugin), Value::Str)
            }
            "graph_base_branches" => {
                value(readers::graph_base_branches(root, moved.plugin), |items| {
                    Value::List(items.into_iter().map(Value::Str).collect())
                })
            }
            "graph_max_branches" => value(readers::graph_max_branches(root, moved.plugin), |n| {
                Value::Int(n as i64)
            }),
            "worktree_dir" => value(readers::worktree_dir(root), Value::Str),
            _ => continue,
        };
        settings
            .entry(moved.plugin.into())
            .or_default()
            .set(moved.new_key, setting);
        renamed.extend(names);
    }
    (settings, renamed)
}
