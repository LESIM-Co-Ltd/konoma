use konoma_plugin::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const CACHE_KEYS: [char; 2] = ['z', 'i'];

#[derive(Default)]
pub struct Log {
    pub changed: bool,
    pub views: Vec<String>,
    pub paths: Vec<PathBuf>,
    pub watches: usize,
}

pub struct WatchProject(pub Arc<Mutex<Log>>);
impl Plugin for WatchProject {
    fn manifest(&self) -> Manifest {
        Manifest::new("project-wake", "1").project(
            ProjectDecl::new()
                .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                .watch("declared/index")
                .screen(ProjectScreen::new("items", "Items").key(CACHE_KEYS[1]))
                .screen(ProjectScreen::new("text", "Text").key(CACHE_KEYS[0])),
        )
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Self(self.0.clone())))
    }
}
impl Session for WatchProject {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        let mut log = self.0.lock().unwrap();
        log.views.push(req.project.as_ref().unwrap().screen.clone());
        Ok(View::text([Line::from(vec![Span::styled(
            if log.changed {
                "after watch"
            } else {
                "before watch"
            },
            Style::new().fg(Color::Green).bold(),
        )])]))
    }
    fn watch_paths(&mut self, _: &ViewCx) -> Result<Option<Vec<PathBuf>>, Failure> {
        let mut log = self.0.lock().unwrap();
        log.watches += 1;
        Ok(Some(log.paths.clone()))
    }
}
