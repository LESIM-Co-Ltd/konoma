//! Compare the standalone runner's landed Preview with the host's real diff-to-Preview path.
use super::*;
use konoma_plugin::host::project::md_diff::{
    DiffBaseline, MdDiffKind, MdDiffRequest, MdDiffWorker,
};
use konoma_plugin::registry::{PluginOrigin, Registry};
use ratatui::{buffer::Buffer, layout::Rect};
use std::sync::Arc;

#[test]
fn standalone_preview_worker_gutter_cells_match_real_host() {
    for (name, wrap, motion) in [
        ("doc.md", true, 0),
        ("doc.md", false, 0),
        ("file.txt", true, 0),
        ("file.txt", false, 35),
    ] {
        let dir = sandbox("standalone-preview-gutter");
        init_git_repo(&dir);
        let path = dir.join(name);
        let old = (0..40).map(|i| format!("row {i}\n\n")).collect::<String>();
        let new = old.replacen("row 25\n", &format!("{}\n", "CHANGED 日本語 ".repeat(6)), 1);
        std::fs::write(&path, &old).unwrap();
        run_git(&dir, &["add", name]);
        run_git(&dir, &["commit", "-qm", "baseline"]);
        std::fs::write(&path, &new).unwrap();
        let mut cfg = cfg_en();
        cfg.ui.tabbar = "hidden".into();
        cfg.ui.statusbar = "bottom".into();
        cfg.ui.diff_view = "preview".into();
        cfg.ui.wrap = wrap;
        let mut sim = Sim::with_config_sized(&canon(&dir), cfg, 32, 11);
        sim.select(name);
        sim.key('d');
        assert!(sim.app.preview_from_diff_for_test());
        for _ in 0..motion {
            sim.key('l');
        }
        let completed = sim
            .term
            .draw(|frame| crate::ui::render(frame, &mut sim.app))
            .unwrap();
        let host_frame = completed.buffer.clone();
        let area = Rect::new(0, 0, 32, 10);
        let mut host = Buffer::empty(area);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                host[(x, y)] = host_frame[(x, y)].clone();
            }
        }
        let mut registry = Registry::new();
        for plugin in konoma_file_plugins::plugins() {
            registry.register(plugin, PluginOrigin::Standard).unwrap();
        }
        let source = konoma_plugin::host::file_preview::FilePreviewSource::new(
            Arc::new(registry),
            Vec::new(),
            false,
        );
        let outcome = name.ends_with(".md").then(|| {
            MdDiffWorker::compute_md_diff(&MdDiffRequest {
                gen: 0,
                root: dir.to_path_buf(),
                path: path.clone(),
                kind: MdDiffKind::Gutter,
                baseline: DiffBaseline::FollowSnapshot(old.as_bytes().to_vec()),
                md_frontmatter: true,
                md_footnotes: true,
                md_inline_html: true,
            })
        });
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(32, 11)).unwrap();
        let shared =
            konoma_host_core::shared::SharedConfig::parse(&format!("[ui]\nwrap={wrap}\n")).unwrap();
        let sdk_frame = konoma_plugin::app::run_preview_with_landed_gutter_for_test(
            &mut term,
            source,
            path,
            dir.to_path_buf(),
            &shared,
            outcome,
            crate::vcs::diff_contents(&old, &new),
            (0..motion)
                .map(|_| konoma_plugin::app::AppEvent::Key(konoma_plugin::Key::char('l')))
                .collect(),
        )
        .unwrap();
        assert_eq!(sdk_frame.area, Rect::new(0, 0, 32, 11));
        assert_eq!(host.area, area);
        let mut sdk = Buffer::empty(area);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                sdk[(x, y)] = sdk_frame[(x, y)].clone();
            }
        }
        assert_eq!(sdk.area, host.area);
        assert_eq!(
            sdk.content.len(),
            usize::from(area.width) * usize::from(area.height)
        );
        let mut compared = 0;
        let mut marked = false;
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let (a, b) = (&host[(x, y)], &sdk[(x, y)]);
                assert_eq!(
                    (a.symbol(), a.fg, a.bg, a.modifier),
                    (b.symbol(), b.fg, b.bg, b.modifier),
                    "{name}: cell ({x},{y})"
                );
                compared += 1;
                marked |= a.symbol() == "▌";
            }
        }
        assert_eq!(compared, usize::from(area.width) * usize::from(area.height));
        assert!(
            host.content.iter().any(|c| c.symbol() != " "),
            "{name}: nonempty content"
        );
        assert!(
            marked || motion > 0,
            "{name}: landed change gutter before horizontal motion"
        );
    }
}
