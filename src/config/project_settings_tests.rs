use super::Config;
use crate::plugins::effective_settings;
use konoma_plugin::Value;

fn load(text: &str) -> Config {
    let dir = crate::test_support::unique_tmp("project_settings_real_config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    let (cfg, error) = Config::load_path_reporting(&path);
    assert!(error.is_none(), "{error:?}");
    cfg
}

#[test]
fn project_settings_real_config_reads_old_new_precedence_and_wrong_types() {
    for moved in konoma_plugin::config::MOVED_PROJECT_SETTINGS {
        let (old, new, default) = match moved.old_key {
            "confirm_jj_sync" => ("false", "true", Value::Bool(true)),
            "commit_meta_align" => ("'inline'", "'right'", Value::Str("right".into())),
            "graph_base_branches" => ("['old']", "['new']", Value::List(Vec::new())),
            "graph_max_branches" => ("3", "5", Value::Int(12)),
            "worktree_dir" => ("'../old'", "'../new'", Value::Str("../".into())),
            other => panic!("uncovered project setting: {other}"),
        };
        let expected = |value: &str| {
            let table: toml::Table = format!("value = {value}").parse().unwrap();
            konoma_plugin::config::value_of(&table["value"])
        };
        let old_text = format!("[{}]\n{} = {old}\n", moved.old_section, moved.old_key);
        let new_text = format!("[plugins.{}]\n{} = {new}\n", moved.plugin, moved.new_key);
        let rename = (
            format!("[{}] {}", moved.old_section, moved.old_key),
            format!("[plugins.{}] {}", moved.plugin, moved.new_key),
        );
        let cfg = load(&old_text);
        assert_eq!(
            effective_settings(&cfg, moved.plugin).get(moved.new_key),
            Some(&expected(old)),
            "old name must reach {}.{}",
            moved.plugin,
            moved.new_key
        );
        assert!(cfg.renamed_settings.contains(&rename));
        for text in [new_text.clone(), format!("{old_text}{new_text}")] {
            let cfg = load(&text);
            assert_eq!(
                effective_settings(&cfg, moved.plugin).get(moved.new_key),
                Some(&expected(new)),
                "new name must reach {}.{}",
                moved.plugin,
                moved.new_key
            );
            assert!(!cfg.renamed_settings.contains(&rename));
        }
        for prefix in ["", &old_text] {
            let cfg = load(&format!(
                "{prefix}[plugins.{}]\n{} = {{ wrong = true }}\n",
                moved.plugin, moved.new_key
            ));
            assert_eq!(
                effective_settings(&cfg, moved.plugin).get(moved.new_key),
                Some(&default),
                "wrong new type must use the default, not the old name: {}.{}",
                moved.plugin,
                moved.new_key
            );
            assert!(!cfg.renamed_settings.contains(&rename));
        }
    }
}

#[test]
fn project_settings_real_config_rejects_wrong_old_types_as_before() {
    for moved in konoma_plugin::config::MOVED_PROJECT_SETTINGS {
        let dir = crate::test_support::unique_tmp("project_settings_bad_old_config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            format!(
                "[{}]\n{} = {{ wrong = true }}\n",
                moved.old_section, moved.old_key
            ),
        )
        .unwrap();
        let (cfg, error) = Config::load_path_reporting(&path);
        assert!(
            error.is_some(),
            "wrong old type must reject the file: {}",
            moved.old_key
        );
        assert_eq!(
            effective_settings(&cfg, moved.plugin),
            effective_settings(&Config::default(), moved.plugin)
        );
    }
}
