//! The same plugin declarations and owner behavior for the two hosts' key-sequence tests.
use konoma_plugin::prelude::*;
use konoma_plugin::view::{ListRow, ListView};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Log {
    pub acts: Vec<String>,
    pub outcome: Option<Outcome>,
    pub chip_failure: bool,
    pub graph: bool,
    /// Two more screens after `items`, reached by `s` and `u`, each with a footer hint.
    pub jumps: bool,
}

pub struct PresentationPlugin(pub Arc<Mutex<Log>>);

impl Plugin for PresentationPlugin {
    fn manifest(&self) -> Manifest {
        let screen = || ActContext::screen("items", ["item"]);
        let (graph, jumps) = {
            let log = self.0.lock().unwrap();
            (log.graph, log.jumps)
        };
        let mut project =
            ProjectDecl::new().marker(DetectionMarker::new("project.marker", MarkerKind::File));
        if graph {
            project = project.screen(ProjectScreen::new("graph", "Graph"));
        }
        let mut project = project
            .chip("status")
            .screen(
                ProjectScreen::new("items", "Items")
                    .key('i')
                    .marks(true)
                    .row_kind("item"),
            )
            .screen(ProjectScreen::new("text", "Text").pushed());
        if jumps {
            project = project
                .screen(
                    ProjectScreen::new("second", "Second")
                        .key('s')
                        .jump_hint(0, "Second"),
                )
                .screen(
                    ProjectScreen::new("third", "Third")
                        .key('u')
                        .jump_hint(0, "Third"),
                );
        }
        Manifest::new(if graph { "git" } else { "project-presentation" }, "1")
            .name("Presentation")
            .shows(ViewKind::List)
            .shows(ViewKind::Text)
            .project(project)
            .action(
                Act::new("open")
                    .key("Enter")
                    .label("inspect  row")
                    .help("inspect row")
                    .footer(0)
                    .project_context(screen()),
            )
            .action(
                Act::new("disabled")
                    .label("disabled")
                    .project_context(screen()),
            )
            .action(Act::new("later").label("later").project_context(screen()))
            .action(
                Act::new("shared")
                    .key("j")
                    .label("step")
                    .footer(1)
                    .project_context(screen()),
            )
            .action(
                Act::new("copy")
                    .key("c")
                    .copy()
                    .label("copy value")
                    .project_context(screen()),
            )
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Self(self.0.clone())))
    }
}

impl Session for PresentationPlugin {
    fn chip(&mut self, _: &ViewCx) -> Result<Option<Chip>, Failure> {
        if self.0.lock().unwrap().chip_failure {
            Err(Failure::external("chip failed"))
        } else {
            Ok(None)
        }
    }
    fn enabled(&self, id: &str) -> bool {
        id != "disabled"
    }
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let screen = req.project.as_ref().unwrap().screen.clone();
        if screen == "text" {
            return Ok(View::text(["pushed text"]));
        }
        // The two jump screens say which one is shown.
        let shown = match screen.as_str() {
            "second" | "third" => format!("shown-{screen}"),
            _ => "alpha".to_string(),
        };
        Ok(View::List(ListView {
            rows: vec![ListRow::new("item", vec![Line::raw(shown)])
                .value("alpha")
                .target("alpha.txt")],
            ..Default::default()
        }))
    }
    fn act(&mut self, id: &str, _: &ActCx) -> Result<Outcome, Failure> {
        let mut log = self.0.lock().unwrap();
        log.acts.push(id.into());
        Ok(log.outcome.clone().unwrap_or_else(|| match id {
            "copy" => Outcome::Copy {
                text: "alpha".into(),
            },
            "open" => Outcome::PushScreen {
                screen: "text".into(),
                selection: None,
            },
            _ => Outcome::Flash(format!("did {id}").into()),
        }))
    }
}
