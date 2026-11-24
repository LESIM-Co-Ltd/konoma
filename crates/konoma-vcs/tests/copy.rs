use konoma_plugin::prelude::*;

#[test]
fn vcs_copy_manifest_has_six_ordered_localized_items_on_log_and_detail_including_jj_ids() {
    for (plugin, jj) in [
        (&konoma_vcs::Git as &dyn Plugin, false),
        (&konoma_vcs::Jj, true),
    ] {
        let manifest = plugin.manifest();
        for screen in ["log", "graph", "detail"] {
            let acts: Vec<_> = manifest
                .actions
                .iter()
                .filter(|a| {
                    a.copy
                        && matches!(&a.project_context,
                Some(ActContext::Screen { screen: s, .. }) if s == screen)
                })
                .collect();
            assert_eq!(acts.len(), 6, "{screen}");
            for (act, (key, en, ja)) in acts.iter().zip([
                (
                    "s",
                    if jj { "change id" } else { "short hash" },
                    if jj { "change id" } else { "短ハッシュ" },
                ),
                (
                    "h",
                    if jj { "commit id" } else { "full hash" },
                    if jj {
                        "commit id"
                    } else {
                        "完全ハッシュ"
                    },
                ),
                ("t", "subject", "件名"),
                ("m", "message", "メッセージ"),
                ("a", "author", "著者"),
                ("d", "date", "日付"),
            ]) {
                assert_eq!(act.key.as_deref(), Some(key));
                assert_eq!(act.label.get("en"), en);
                assert_eq!(act.label.get("ja"), ja);
                assert_eq!(act.footer, None);
                let Some(ActContext::Screen { row_kinds, .. }) = &act.project_context else {
                    panic!()
                };
                assert_eq!(
                    row_kinds,
                    &if screen != "detail" {
                        vec!["commit".to_string()]
                    } else {
                        vec![]
                    }
                );
            }
        }
    }
}
