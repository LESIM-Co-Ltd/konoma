//! Real input on both hosts; worker barriers settle replies before each observation.
use super::*;
use konoma_host_core::project::DiffView;
use konoma_plugin::app::project::{
    drive_diff_representation_for_test, prepare_projects, ProjectBackend, ProjectServiceInfo,
    ProjectServices, ProjectSettings,
};
use konoma_plugin::host::project::MatchedProject;
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_vcs_core::{
    git::{ChangeEntry, DiffLine},
    vcs::{Git, Vcs},
};
use std::{path::Path, sync::Arc};

struct Project;
impl Plugin for Project {
    fn manifest(&self) -> Manifest {
        Manifest::new("cycle-project", "0").project(
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
        Ok(View::List(Default::default()))
    }
}
struct Services;
impl ProjectServices for Services {
    fn inspect(
        &self,
        p: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        Ok(ProjectServiceInfo {
            workdir: p.root.clone(),
            backend: ProjectBackend::Git,
        })
    }
    fn file_diff(&self, i: &ProjectServiceInfo, p: &Path) -> Vec<DiffLine> {
        Git.file_diff(&i.workdir, p)
    }
    fn changed_files(&self, i: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        Git.changed_files(&i.workdir)
    }
    fn base_contents(&self, i: &ProjectServiceInfo, p: &Path) -> Option<Vec<u8>> {
        Git.base_contents(&i.workdir, p)
    }
    fn spawn_file_diff(
        self: Arc<Self>,
        request: konoma_plugin::app::diff_screen::FileDiffRequest,
    ) -> Result<konoma_plugin::app::diff_screen::FileDiffReply, Failure> {
        request.spawn(self)
    }
}

fn fixture() -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/representation_cycles.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn pdf() -> Vec<u8> {
    let fixture = fixture();
    let objects = fixture["pdf_objects"].as_array().unwrap();
    let mut data = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(data.len());
        data.extend_from_slice(
            format!("{} 0 obj\n{}\nendobj\n", i + 1, obj.as_str().unwrap()).as_bytes(),
        );
    }
    let start = data.len();
    data.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in &offsets[1..] {
        data.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    data.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    data
}

#[test]
fn all_representation_cycles_and_detours_match_host() {
    let dir = sandbox("representation_cycles").into_canonical().unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("project.marker"), "").unwrap();
    let names = [
        "a.md", "b.svg", "c.txt", "d.png", "e.pdf", "f.rs", "g.bin", "h_big.md", "i.md",
    ];
    let f = fixture();
    for (old, rgba) in [(true, &f["old_rgba"]), (false, &f["new_rgba"])] {
        let color = [
            rgba[0].as_u64().unwrap() as u8,
            rgba[1].as_u64().unwrap() as u8,
            rgba[2].as_u64().unwrap() as u8,
        ];
        for name in names {
            let path = dir.join(name);
            match name {
                "b.svg" => std::fs::write(
                    path,
                    media_diff_solid_svg_bytes(8, 8, (color[0], color[1], color[2])),
                )
                .unwrap(),
                "d.png" => write_solid_png(&path, 8, 8, color),
                "e.pdf" => {
                    let mut bytes = pdf();
                    if !old {
                        bytes.extend_from_slice(b"\n% changed\n");
                    }
                    std::fs::write(path, bytes).unwrap();
                }
                "g.bin" => std::fs::write(path, [0, color[0], 0, color[1]]).unwrap(),
                "h_big.md" if !old => {
                    std::fs::write(path, "x".repeat(5 * 1024 * 1024 + 1)).unwrap()
                }
                _ => std::fs::write(
                    path,
                    if old {
                        "# Old\n\nbody\n"
                    } else {
                        "# New\n\nchanged\n"
                    },
                )
                .unwrap(),
            }
        }
        if old {
            run_git(&dir, &["add", "-A"]);
            run_git(&dir, &["commit", "-qm", "baseline"]);
        }
    }
    for initial in ["source", "rendered", "preview"] {
        for start in names[..5].iter().chain(&names[6..7]) {
            let text = format!("[ui]\nlang='en'\ndiff_view='{initial}'\n");
            let cfg: Config = toml::from_str(&text).unwrap();
            let shared = konoma_host_core::shared::SharedConfig::parse(&text).unwrap();
            let mut host = Sim::with_config_sized(&canon(&dir), cfg, 42, 12);
            host.select(start);
            host.key('d');
            let mut plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(Project)];
            plugins.extend(konoma_file_plugins::plugins());
            let mut sdk = prepare_projects(
                plugins,
                &dir,
                TermCaps::new(),
                (shared, None),
                Arc::new(Services),
            )
            .unwrap_or_else(|e| panic!("{}", e.error));
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(42, 12)).unwrap();
            let compare =
                |host: &Sim, state: (DiffView, DiffView, std::path::PathBuf), key: &str| {
                    let shown = if host.app.preview_from_diff_for_test() {
                        DiffView::Preview
                    } else {
                        host.app.diff_view_for_test()
                    };
                    assert_eq!(
                        (state.0, state.1),
                        (shown, host.app.diff_view_choice_for_test()),
                        "{start} initial={initial} after {key}"
                    );
                    assert_eq!(
                        Some(state.2.as_path()),
                        host.app.tab.preview_path.as_deref()
                    );
                };
            compare(
                &host,
                drive_diff_representation_for_test(
                    &mut terminal,
                    &mut sdk,
                    Some(dir.join(start)),
                    None,
                )
                .unwrap(),
                "open",
            );
            for _ in 0..6 {
                host.key('R');
                compare(
                    &host,
                    drive_diff_representation_for_test(
                        &mut terminal,
                        &mut sdk,
                        None,
                        Some(Key::char('R')),
                    )
                    .unwrap(),
                    "R",
                );
            }
            if host.app.preview_from_diff_for_test() {
                host.key('R');
                compare(
                    &host,
                    drive_diff_representation_for_test(
                        &mut terminal,
                        &mut sdk,
                        None,
                        Some(Key::char('R')),
                    )
                    .unwrap(),
                    "R from Preview",
                );
            }
            for _ in 0..names.len() {
                host.ctrl('n');
                compare(
                    &host,
                    drive_diff_representation_for_test(
                        &mut terminal,
                        &mut sdk,
                        None,
                        Some(Key::ctrl('n')),
                    )
                    .unwrap(),
                    "Ctrl-n detour",
                );
                if host.app.preview_from_diff_for_test() {
                    host.key('R');
                    compare(
                        &host,
                        drive_diff_representation_for_test(
                            &mut terminal,
                            &mut sdk,
                            None,
                            Some(Key::char('R')),
                        )
                        .unwrap(),
                        "R after Preview choice detour",
                    );
                }
            }
            for _ in 0..names.len() {
                host.ctrl('p');
                compare(
                    &host,
                    drive_diff_representation_for_test(
                        &mut terminal,
                        &mut sdk,
                        None,
                        Some(Key::ctrl('p')),
                    )
                    .unwrap(),
                    "Ctrl-p return",
                );
            }
        }
    }
}
