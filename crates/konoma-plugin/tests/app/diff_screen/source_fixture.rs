use konoma_plugin::prelude::*;

pub const SOURCE_KEYS: &[&str] = &["Enter", "s", "n", "n", "N", "h", "l", "q"];
pub const FILE_KEYS: &[&str] = &["Enter", "s", "ctrl-n", "ctrl-n", "ctrl-p", "q"];
pub const REFRESH_KEYS: &[&str] = &["Enter", "r"];
pub const MOTION_KEYS: &[&str] = &[
    "Enter", "Down", "PageDown", "PageUp", "ctrl-d", "ctrl-u", "End", "Home", "Right", "Left", "$",
    "0", "q",
];

pub fn raw_diff() -> Vec<konoma_vcs_core::git::DiffLine> {
    use konoma_vcs_core::git::{DiffLine, DiffLineKind};
    (0..36)
        .flat_map(|i| {
            let changed = [0, 7, 18, 31].contains(&i);
            let line = |kind, old_no, new_no, label| DiffLine {
                kind,
                old_no,
                new_no,
                text: format!(
                    "{label} {i:02}: alpha beta gamma delta epsilon zeta eta theta iota kappa"
                ),
            };
            changed
                .then(|| line(DiffLineKind::Removed, Some(i + 1), None, "old"))
                .into_iter()
                .chain([line(
                    if changed {
                        DiffLineKind::Added
                    } else {
                        DiffLineKind::Context
                    },
                    (!changed).then_some(i + 1),
                    Some(i + 1),
                    "line",
                )])
        })
        .collect()
}

pub struct SourceProject;
impl Plugin for SourceProject {
    fn manifest(&self) -> Manifest {
        Manifest::new("source-project", "1")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(ProjectScreen::new("items", "Items").row_kind("file")),
            )
            .action(
                Act::new("diff")
                    .key("Enter")
                    .project_context(ActContext::screen("items", ["file"])),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::Project { root, .. } = input else {
            return Err(Failure::unsupported("project only"));
        };
        Ok(Box::new(SourceSession(root)))
    }
}
struct SourceSession(std::path::PathBuf);
impl Session for SourceSession {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(konoma_plugin::view::ListView {
            rows: (0..20)
                .map(|i| {
                    konoma_plugin::view::ListRow::new("file", vec![Line::raw(format!("file {i}"))])
                        .value(format!("file-{i}"))
                })
                .collect(),
            ..Default::default()
        }))
    }
    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        Ok(Outcome::OpenDiff {
            path: self.0.join("b.txt"),
        })
    }
}
