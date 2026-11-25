//! The project Preview and the real host receive the same config text.
use super::*;
use konoma_plugin::app::diff_screen::{FileDiffData, FileDiffReply, FileDiffRequest};
use konoma_plugin::app::project::{
    ProjectBackend, ProjectServiceInfo, ProjectServices, ProjectSettings,
};
use konoma_plugin::host::project::md_diff::{
    DiffBaseline, MdDiffKind, MdDiffRequest, MdDiffWorker,
};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::view::ListView;
use konoma_vcs_core::git::{ChangeEntry, DiffLine};
use ratatui::{buffer::Buffer, layout::Rect};
use std::path::Path;
use std::sync::{mpsc, Arc};

struct Project;
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("gutter-config-project", "0").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .screen(ProjectScreen::new("changes", "Changes")),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Project))
    }
}
impl Session for Project {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(ListView::default()))
    }
}
struct Services(Vec<DiffLine>);
impl ProjectServices for Services {
    fn inspect(
        &self,
        project: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: project.root.clone(),
            backend: ProjectBackend::Git,
        })
    }
    fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        self.0.clone()
    }
    fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        Vec::new()
    }
    fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        None
    }
    fn spawn_file_diff(self: Arc<Self>, _: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: self.0.clone(),
            paths: Vec::new(),
        }))
        .unwrap();
        Ok(rx)
    }
}

#[test]
fn standalone_project_preview_git_gutter_config_cells_match_host() {
    for (setting, enabled) in [
        ("", true),
        ("git_gutter = true\n", true),
        ("git_gutter = false\n", false),
    ] {
        for name in ["doc.md", "file.txt"] {
            let dir = sandbox("project-preview-gutter-config")
                .into_canonical()
                .unwrap();
            init_git_repo(&dir);
            std::fs::write(dir.join("project.marker"), "").unwrap();
            let path = dir.join(name);
            let old = (0..40).map(|i| format!("row {i}\n\n")).collect::<String>();
            let new = old.replacen("row 25\n", "CHANGED 日本語\n", 1);
            std::fs::write(&path, &old).unwrap();
            run_git(&dir, &["add", name]);
            run_git(&dir, &["commit", "-qm", "baseline"]);
            std::fs::write(&path, &new).unwrap();
            let text = format!("[ui]\nlang='en'\ntabbar='hidden'\nstatusbar='bottom'\ndiff_view='preview'\n{setting}");
            let cfg: Config = toml::from_str(&text).unwrap();
            let shared = konoma_host_core::shared::SharedConfig::parse(&text).unwrap();
            let mut host = Sim::with_config_sized(&canon(&dir), cfg, 32, 11);
            host.select(name);
            host.key('d');
            assert!(host.app.preview_from_diff_for_test());
            let lands = enabled || name.ends_with(".txt");
            let scroll = if name.ends_with(".md") {
                usize::from(host.app.tab.preview_scroll)
            } else {
                host.app.preview_top_line()
            };
            if lands {
                assert!(scroll > 0, "host lands below the fold: {name} {setting}");
            } else {
                assert_eq!(scroll, 0, "host Markdown without gutter stays at top");
            }
            let host_frame = host
                .term
                .draw(|frame| crate::ui::render(frame, &mut host.app))
                .unwrap()
                .buffer
                .clone();
            let mut plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(Project)];
            plugins.extend(konoma_file_plugins::plugins());
            let mut prepared = konoma_plugin::app::project::prepare_projects(
                plugins,
                &dir,
                konoma_plugin::TermCaps::new(),
                (shared, None),
                Arc::new(Services(crate::vcs::diff_contents(&old, &new))),
            )
            .unwrap_or_else(|e| panic!("{}", e.error));
            let source = konoma_plugin::host::file_preview::FilePreviewSource::new(
                prepared.registry.clone(),
                Vec::new(),
                false,
            );
            let outcome = (enabled && name.ends_with(".md")).then(|| {
                MdDiffWorker::compute_md_diff(&MdDiffRequest {
                    gen: 0,
                    path: path.clone(),
                    root: dir.to_path_buf(),
                    kind: MdDiffKind::Gutter,
                    baseline: DiffBaseline::FollowSnapshot(old.as_bytes().to_vec()),
                    md_frontmatter: true,
                    md_footnotes: true,
                    md_inline_html: true,
                })
            });
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(32, 11)).unwrap();
            let sdk_frame = konoma_plugin::app::run_project_preview_with_landed_gutter_for_test(
                &mut terminal,
                &mut prepared,
                source,
                path,
                outcome,
            )
            .unwrap();
            let area = Rect::new(0, 0, 32, 10);
            let crop = |frame: &Buffer| {
                assert_eq!(frame.area, Rect::new(0, 0, 32, 11));
                let mut out = Buffer::empty(area);
                for y in area.y..area.bottom() {
                    for x in area.x..area.right() {
                        out[(x, y)] = frame[(x, y)].clone();
                    }
                }
                out
            };
            let (host_cells, sdk_cells) = (crop(&host_frame), crop(&sdk_frame));
            assert_eq!(host_cells.area, sdk_cells.area);
            let count = usize::from(area.width) * usize::from(area.height);
            assert_eq!(host_cells.content.len(), count);
            assert_eq!(sdk_cells.content.len(), count);
            assert!(
                sdk_cells.content.iter().any(|c| c.symbol() != " "),
                "nonempty project Preview"
            );
            let marked = sdk_cells
                .content
                .iter()
                .any(|c| matches!(c.symbol(), "▌" | "▔"));
            assert_eq!(
                marked, enabled,
                "project Preview honours git_gutter: {name} {setting}"
            );
            let changed = sdk_cells.content.iter().any(|c| c.symbol() == "C");
            assert_eq!(
                changed, lands,
                "project Preview first-change landing matches host: {name} {setting}"
            );
            for (index, (a, b)) in host_cells
                .content
                .iter()
                .zip(&sdk_cells.content)
                .enumerate()
            {
                assert_eq!(
                    (a.symbol(), a.fg, a.bg, a.modifier),
                    (b.symbol(), b.fg, b.bg, b.modifier),
                    "project Preview cell {index}: {name} {setting}"
                );
            }
        }
    }
}
