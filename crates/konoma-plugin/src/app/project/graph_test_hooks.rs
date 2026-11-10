//! Read private plugin state on its owner, with an independent sink for each real session.
use crate::prelude::*;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectGraphSnapshot {
    pub base: Option<String>,
    pub label: Option<String>,
    pub visible: Vec<String>,
    pub order: Vec<String>,
    pub tentative: Vec<String>,
    pub all: bool,
    pub picker_cursor: Option<usize>,
}

#[derive(Clone, Default)]
pub struct ProjectGraphProbe(Arc<Mutex<Option<ProjectGraphSnapshot>>>);
impl ProjectGraphProbe {
    pub fn snapshot(&self, picker_cursor: Option<usize>) -> Option<ProjectGraphSnapshot> {
        self.0.lock().unwrap().clone().map(|mut state| {
            state.picker_cursor = picker_cursor;
            state
        })
    }
    fn capture<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Restore(Option<ProjectGraphProbe>);
        impl Drop for Restore {
            fn drop(&mut self) {
                SINK.with(|s| *s.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(SINK.with(|s| s.replace(Some(self.clone()))));
        f()
    }
}
thread_local! {
    static SINK: RefCell<Option<ProjectGraphProbe>> = const { RefCell::new(None) };
}

pub fn record_graph_for_test(snapshot: ProjectGraphSnapshot) {
    SINK.with(|sink| {
        if let Some(sink) = sink.borrow().as_ref() {
            *sink.0.lock().unwrap() = Some(snapshot);
        }
    });
}

pub fn probe_graph_plugin_for_test(
    plugin: Arc<dyn Plugin>,
) -> (Arc<dyn Plugin>, ProjectGraphProbe) {
    let probe = ProjectGraphProbe::default();
    (Arc::new(ProbePlugin(plugin, probe.clone())), probe)
}
struct ProbePlugin(Arc<dyn Plugin>, ProjectGraphProbe);
impl Plugin for ProbePlugin {
    fn manifest(&self) -> Manifest {
        self.0.manifest()
    }
    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProbeSession(
            self.0.open(input, cx)?,
            self.1.clone(),
        )))
    }
}
struct ProbeSession(Box<dyn Session>, ProjectGraphProbe);
impl Session for ProbeSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.1.capture(|| self.0.view(req, cx))
    }
    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        self.1.capture(|| self.0.act(action, cx))
    }
    fn key(&mut self, key: &Key, cx: &ActCx) -> Result<Outcome, Failure> {
        self.0.key(key, cx)
    }
    fn enabled(&self, action: &str) -> bool {
        self.0.enabled(action)
    }
    fn layout(&self) -> Layout {
        self.0.layout()
    }
    fn chip(&mut self, cx: &ViewCx) -> Result<Option<Chip>, Failure> {
        self.0.chip(cx)
    }
    fn watch_paths(&mut self, cx: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        self.0.watch_paths(cx)
    }
    fn tab_closed(&mut self, tab: u64) {
        self.0.tab_closed(tab);
    }
}
