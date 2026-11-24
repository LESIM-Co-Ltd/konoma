//! A static project provider recording the real requests received on its owner worker.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView, ListRow, ListView};
use konoma_plugin::{Origin, Resources};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct ProjectLog {
    pub watch_paths: Option<Vec<std::path::PathBuf>>,
    pub watch_threads: Vec<std::thread::ThreadId>,
    pub chip: Option<Chip>,
    pub chips: usize,
    pub chip_failure: bool,
    pub chip_panic: bool,
    pub no_declared_chip: bool,
    pub file_only: bool,
    pub chip_from_marker: bool,
    pub chip_from_watch: bool,
    pub chip_hold: Option<Arc<ViewHold>>,
    pub views: Vec<ProjectView>,
    pub acts: Vec<(String, ProjectAct)>,
    pub threads: Vec<std::thread::ThreadId>,
    pub manifests: usize,
    pub opens: usize,
    pub view_hold: Option<Arc<ViewHold>>,
    pub outcome: Option<Outcome>,
    pub reverse_rows: bool,
    pub omit_row: Option<String>,
    pub disabled: Vec<String>,
}

pub(crate) struct ViewHold {
    pub screen: String,
    pub entered: super::test_plugins::Latch,
    pub release: super::test_plugins::Latch,
}

impl ViewHold {
    pub(crate) fn new(screen: &str) -> Arc<Self> {
        Arc::new(Self {
            screen: screen.into(),
            entered: Default::default(),
            release: Default::default(),
        })
    }
}

pub(crate) struct Screens {
    pub log: Arc<Mutex<ProjectLog>>,
    pub read_only: bool,
    pub hold: Option<Arc<super::test_plugins::Latch>>,
    pub started: Option<Arc<super::test_plugins::Latch>>,
    pub outcome: Option<Outcome>,
}

fn action(id: &'static str, key: &str, context: ActContext) -> Act {
    Act::new(id)
        .key(key)
        .label(id)
        .help(id)
        .footer(1)
        .project_context(context)
}

impl Plugin for Screens {
    fn manifest(&self) -> Manifest {
        self.log.lock().unwrap().manifests += 1;
        let on = || ActContext::screen("items", ["item"]);
        let any = || ActContext::screen("items", Vec::<String>::new());
        let mut m = Manifest::new("project-screens", "1")
            .name("Project screens")
            .project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("*.screen-project", MarkerKind::File))
                    .screen(
                        ProjectScreen::new("items", "Items")
                            .marks(true)
                            .checks(true)
                            .key('i')
                            .row_kind("item")
                            .row_kind("other"),
                    )
                    .screen(ProjectScreen::new("text", "Text").key('z'))
                    .screen(ProjectScreen::new("diff", "Diff").key('d'))
                    .screen(ProjectScreen::new("doc", "Document").key('b'))
                    .watch("metadata/index")
                    .chip("project chip"),
            )
            .shows(ViewKind::List)
            .shows(ViewKind::Text)
            .shows(ViewKind::Diff)
            .shows(ViewKind::Document)
            .action(action("push", "Enter", on()))
            .action(action("capture", "x", on()))
            .action(action("screen", "a", any()))
            .action(action(
                "other",
                "Enter",
                ActContext::screen("items", ["other"]),
            ))
            .action(action("diff-act", "x", ActContext::Diff))
            .action(action("tree-shortcut", "z", ActContext::TreeSelection))
            .action(action("screen-shortcut", "z", any()))
            .action(
                Act::new("tree-act")
                    .label("tree-act")
                    .help("tree-act")
                    .footer(1)
                    .project_context(ActContext::TreeSelection),
            )
            .action(action("write", "w", on()).writes())
            .action(
                action("confirm", "F2", on()).confirmation(Confirmation::new(
                    "Confirm project?",
                    vec![
                        ConfirmChoice::new('y', "yes-value", "Accept"),
                        ConfirmChoice::new('!', "force-value", "Force"),
                    ],
                )),
            )
            .action(
                action("text-input", "F3", on())
                    .writes()
                    .text_input(TextInput::new("Project name", "初期")),
            )
            .action(action("choice", "F4", on()).choice_input(ChoiceInput::new(
                "Choose target",
                vec![
                    Choice::new("one-value", "First"),
                    Choice::new("two-value", "Second"),
                ],
            )))
            .action(
                action("answers", "F5", on())
                    .confirmation(Confirmation::new(
                        "Proceed?",
                        vec![ConfirmChoice::new('y', "accepted", "Yes")],
                    ))
                    .text_input(TextInput::new("Name", "seed"))
                    .choice_input(ChoiceInput::new(
                        "Target",
                        vec![
                            Choice::new("first", "First"),
                            Choice::new("second", "Second"),
                        ],
                    )),
            )
            .action(action("current", "F6", on()).refresh(RefreshTarget::CurrentScreen))
            .action(action("target", "F7", on()).refresh(RefreshTarget::screen("text")))
            .action(action("all", "F8", on()).refresh(RefreshTarget::AllScreens))
            .action(action("tree-status", "F9", on()).refresh(RefreshTarget::TreeStatus))
            .action(action("chip", "F10", on()).refresh(RefreshTarget::Chip))
            .action(
                action("current-chip", "F12", on())
                    .refresh(RefreshTarget::CurrentScreen)
                    .refresh(RefreshTarget::Chip),
            )
            .action(
                action("tree-chip", "F1", on())
                    .refresh(RefreshTarget::TreeStatus)
                    .refresh(RefreshTarget::Chip),
            )
            .action(action("navigate", "F11", on()));
        if self.read_only {
            m = m.read_only();
        }
        if let Some(project) = &mut m.project {
            project.chip = (!self.log.lock().unwrap().no_declared_chip)
                .then(|| Tr::new("project chip").ja("宣言チップ"));
        }
        if self.log.lock().unwrap().file_only {
            m.project = None;
            m.actions.clear();
            m = m.bind(Bind::files(["*.screen-file"]));
        }
        m
    }

    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (root, marker) = match input {
            Input::Project { root, marker } => (root, marker),
            Input::File { path, .. } => (path.parent().unwrap().to_path_buf(), String::new()),
            _ => return Err(Failure::unsupported("project or file required")),
        };
        let mut log = self.log.lock().unwrap();
        log.opens += 1;
        log.threads.push(std::thread::current().id());
        drop(log);
        Ok(Box::new(ScreenSession {
            root,
            marker,
            log: self.log.clone(),
            hold: self.hold.clone(),
            started: self.started.clone(),
            outcome: self.outcome.clone(),
        }))
    }
}

struct ScreenSession {
    root: std::path::PathBuf,
    marker: String,
    log: Arc<Mutex<ProjectLog>>,
    hold: Option<Arc<super::test_plugins::Latch>>,
    started: Option<Arc<super::test_plugins::Latch>>,
    outcome: Option<Outcome>,
}

impl Session for ScreenSession {
    fn watch_paths(&mut self, _: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        let mut log = self.log.lock().unwrap();
        log.watch_threads.push(std::thread::current().id());
        Ok(log.watch_paths.clone())
    }

    fn chip(&mut self, _: &ViewCx) -> Result<Option<Chip>, Failure> {
        let mut log = self.log.lock().unwrap();
        log.chips += 1;
        log.threads.push(std::thread::current().id());
        let chip = log.chip.clone();
        let failure = log.chip_failure;
        let panic = log.chip_panic;
        let from_marker = log.chip_from_marker;
        let from_watch = log.chip_from_watch;
        let hold = log.chip_hold.take();
        drop(log);
        if let Some(hold) = hold {
            hold.entered.open();
            hold.release.wait();
        }
        assert!(!panic, "chip fixture panic");
        if failure {
            return Err(Failure::external("chip fixture failure"));
        }
        if from_marker {
            return Ok(Some(Chip::new(std::fs::read_to_string(
                self.root.join(&self.marker),
            )?)));
        }
        if from_watch {
            return std::fs::read_to_string(self.root.join("metadata/index"))
                .map(|text| Some(Chip::new(text)))
                .map_err(Failure::from);
        }
        Ok(chip)
    }
    fn enabled(&self, action: &str) -> bool {
        !self
            .log
            .lock()
            .unwrap()
            .disabled
            .iter()
            .any(|disabled| disabled == action)
    }
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        if req.project.is_none() {
            return Ok(View::text(["file session"]));
        }
        let project = req
            .project
            .as_ref()
            .ok_or_else(|| Failure::unsupported("missing project view"))?;
        let mut log = self.log.lock().unwrap();
        log.views.push(project.clone());
        log.threads.push(std::thread::current().id());
        let hold = if log
            .view_hold
            .as_ref()
            .is_some_and(|hold| hold.screen == project.screen)
        {
            log.view_hold.take()
        } else {
            None
        };
        let reverse = log.reverse_rows;
        let omit = log.omit_row.clone();
        drop(log);
        if let Some(hold) = &hold {
            hold.entered.open();
            hold.release.wait();
        }
        let mut view = match project.screen.as_str() {
            "items" => {
                use konoma_plugin::style::{Color, Line, Span, Style};
                View::List(ListView {
                    rows: vec![
                        ListRow::new(
                            "item",
                            vec![Line::from(vec![
                                Span::styled("alpha", Style::new().fg(Color::Red).bold()),
                                Span::styled(" styled", Style::new().fg(Color::Green)),
                            ])],
                        )
                        .value("opaque-alpha")
                        .target(self.root.join("file.txt"))
                        .checked(true)
                        .marked(true),
                        ListRow::new("other", vec![Line::raw("beta")])
                            .value("opaque-beta")
                            .checked(false),
                        ListRow::new("item", vec![Line::raw("gamma")]).value("opaque-gamma"),
                    ],
                })
            }
            "text" => View::text([format!(
                "text selection={}",
                project.selection.as_deref().unwrap_or("none")
            )]),
            "diff" => View::Diff(DiffView {
                hunks: vec![DiffHunk {
                    lines: vec![
                        DiffLine {
                            kind: DiffKind::Added,
                            old_no: None,
                            new_no: Some(1),
                            text: "project addition".into(),
                        },
                        DiffLine {
                            kind: DiffKind::Removed,
                            old_no: Some(1),
                            new_no: None,
                            text: "project removal".into(),
                        },
                    ],
                }],
            }),
            "doc" => View::Document(konoma_plugin::view::DocumentView {
                markdown: "# Project document\n\n**strong body**\n\n- list entry\n".into(),
                origin: Origin::Converted {
                    from: self.root.clone(),
                },
                resources: Resources::default(),
                options: konoma_plugin::view::DocOptions::default(),
                title: None,
                truncated: false,
            }),
            _ => return Err(Failure::unsupported("unknown project screen")),
        };
        if reverse {
            if let View::List(list) = &mut view {
                list.rows.reverse();
            }
        }
        if let View::List(list) = &mut view {
            list.rows.retain(|row| omit.as_deref() != Some(&row.value));
        }
        Ok(view)
    }

    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        let context = cx
            .project
            .as_ref()
            .ok_or_else(|| Failure::unsupported("missing project act"))?;
        let mut log = self.log.lock().unwrap();
        log.acts.push((action.into(), context.clone()));
        log.threads.push(std::thread::current().id());
        drop(log);
        if let Some(started) = &self.started {
            started.open();
        }
        if let Some(hold) = &self.hold {
            hold.wait();
        }
        Ok(match action {
            "push" => Outcome::PushScreen {
                screen: "text".into(),
                selection: context.selected.as_ref().map(|row| row.value.clone()),
            },
            "navigate" => self
                .outcome
                .clone()
                .or_else(|| self.log.lock().unwrap().outcome.clone())
                .unwrap_or(Outcome::Unhandled),
            "tree-act" if self.outcome.is_some() => {
                self.outcome.clone().unwrap_or(Outcome::Unhandled)
            }
            _ => Outcome::Flash(Tr::new(format!("did {action}"))),
        })
    }
}
