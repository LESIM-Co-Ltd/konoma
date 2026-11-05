//! A static project provider recording the real requests received on its owner worker.

use konoma_plugin::prelude::*;
use konoma_plugin::view::{DiffHunk, DiffKind, DiffLine, DiffView, ListRow, ListView};
use konoma_plugin::{Origin, Resources};
use std::sync::{Arc, Mutex};

/// Observes the real VCS session without substituting its data or action implementation.
#[cfg(feature = "git")]
#[derive(Default)]
pub(crate) struct VcsCalls {
    pub acts: Vec<String>,
    pub views: usize,
    pub chips: usize,
    pub remove_switch_target: bool,
}

#[cfg(feature = "git")]
pub(crate) struct ObservedVcs {
    pub plugin: Arc<dyn Plugin>,
    pub calls: Arc<Mutex<VcsCalls>>,
}

#[cfg(feature = "git")]
impl Plugin for ObservedVcs {
    fn manifest(&self) -> Manifest {
        self.plugin.manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ObservedVcsSession {
            session: self.plugin.open(input, cx)?,
            calls: self.calls.clone(),
        }))
    }
}

#[cfg(feature = "git")]
struct ObservedVcsSession {
    session: Box<dyn Session>,
    calls: Arc<Mutex<VcsCalls>>,
}

#[cfg(feature = "git")]
impl Session for ObservedVcsSession {
    fn enabled(&self, action: &str) -> bool {
        self.session.enabled(action)
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.calls.lock().unwrap().views += 1;
        self.session.view(req, cx)
    }

    fn act(&mut self, action: &str, cx: &ActCx) -> Result<Outcome, Failure> {
        self.calls.lock().unwrap().acts.push(action.into());
        let outcome = self.session.act(action, cx)?;
        if let (true, Outcome::SwitchRoot { root, .. }) =
            (self.calls.lock().unwrap().remove_switch_target, &outcome)
        {
            // Remove it after the real plugin checked it and before the host sees the reply.
            std::fs::remove_dir_all(root).unwrap();
        }
        Ok(outcome)
    }

    fn chip(&mut self, cx: &ViewCx) -> Result<Option<Chip>, Failure> {
        self.calls.lock().unwrap().chips += 1;
        self.session.chip(cx)
    }

    fn watch_paths(&mut self, cx: &ViewCx) -> Result<Option<Vec<std::path::PathBuf>>, Failure> {
        self.session.watch_paths(cx)
    }
}

#[derive(Default)]
pub(crate) struct ProjectLog {
    pub open_label: Option<Tr>,
    pub push_help_alias: bool,
    pub hint_groups: bool,
    pub rebound_hint_key: bool,
    pub act_failures: usize,
    pub act_error: Option<Failure>,
    pub act_panic: bool,
    pub watch_failure_after_act: bool,
    pub view_failure_after_act: bool,
    pub row_name: Option<String>,
    pub prompt_default: Option<bool>,
    pub prompt_value: Option<toml::Value>,
    pub prompt_settings: Vec<bool>,
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

pub(crate) fn bind_push_help_alias(keys: &mut crate::plugins::PluginKeys) {
    // Project declarations reserve l. Reuse the existing binding seam after validation so the
    // real help builder sees one grouped Enter/l row, without changing the product's key rules.
    keys.share_key_for_test(
        crate::plugins::PluginAct::new("project-screens", "push-alias-source"),
        crate::plugins::PluginAct::new("project-screens", "push-alias"),
    );
}

impl Plugin for Screens {
    fn manifest(&self) -> Manifest {
        self.log.lock().unwrap().manifests += 1;
        let open_label = self
            .log
            .lock()
            .unwrap()
            .open_label
            .clone()
            .unwrap_or_else(|| Tr::new("push"));
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
            .action(
                action("retry-refresh", "ctrl-F7", on())
                    .text_input(TextInput::new("Retry message", "seed").keep_on_failure())
                    .refresh(RefreshTarget::CurrentScreen),
            )
            .action(
                action("retry-input", "ctrl-F5", on())
                    .text_input(TextInput::new("Retry message", "seed").keep_on_failure()),
            )
            .action(
                action("retry-confirmed", "ctrl-F6", on())
                    .confirmation(Confirmation::new(
                        "Confirm retry?",
                        vec![ConfirmChoice::new('y', "yes", "Yes")],
                    ))
                    .text_input(TextInput::new("Retry message", "seed").keep_on_failure()),
            )
            .setting(SettingDecl::new(
                "confirm_sync",
                Value::Bool(self.log.lock().unwrap().prompt_default.unwrap_or(true)),
                "Ask before sync",
            ))
            .action(
                action("row-prompt", "ctrl-F2", any()).confirmation(Confirmation::new(
                    Tr::new("Discard {row}? Again {row}; {other}").with("ja", "JA {row} / {row}"),
                    vec![ConfirmChoice::new('y', "yes", "Yes")],
                )),
            )
            .action(
                action("conditional", "ctrl-F3", on()).confirmation(
                    Confirmation::new("Sync project?", vec![ConfirmChoice::new('y', "yes", "Yes")])
                        .setting("confirm_sync"),
                ),
            )
            .action(
                action("conditional-input", "ctrl-F4", on())
                    .confirmation(
                        Confirmation::new(
                            "Sync project?",
                            vec![ConfirmChoice::new('y', "yes", "Yes")],
                        )
                        .setting("confirm_sync"),
                    )
                    .text_input(TextInput::new("Sync message", "seed"))
                    .choice_input(ChoiceInput::new(
                        "Sync target",
                        vec![Choice::new("all", "All")],
                    )),
            )
            .action(action("push", "Enter", on()).label(open_label))
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
                        ConfirmChoice::new('a', "lower-value", "Lowercase"),
                        ConfirmChoice::new('A', "upper-value", "Uppercase"),
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
        if self.log.lock().unwrap().push_help_alias {
            m = m
                .hint_group(HintGroup::new("push-open", "push").help("push"))
                .action(action("push-alias", "ctrl-F8", on()).hint_group("push-open"))
                .action(Act::new("push-alias-source").key("l").context("canvas"));
            for act in &mut m.actions {
                if act.id == "push" {
                    act.hint_group = Some("push-open".into());
                }
            }
        }
        if self.log.lock().unwrap().hint_groups {
            m = m
                .hint_group(HintGroup::new("stage", "stage(all)").help("stage / stage all"))
                .hint_group(HintGroup::new("unstage", "unstage(all)").help("unstage / unstage all"))
                .action(action("hint-before", "ctrl-F8", any()).footer(0))
                .action(
                    action("hint-stage", "s", any())
                        .label("stage")
                        .footer(8)
                        .hint_group("stage"),
                )
                .action(action("hint-between", "ctrl-F9", any()).footer(4))
                .action(
                    action("hint-stage-all", "S", any())
                        .label("stage all")
                        .footer(2)
                        .hint_group("stage"),
                )
                .action(
                    action("hint-unstage", "u", any())
                        .label("unstage")
                        .footer(9)
                        .hint_group("unstage"),
                )
                .action(
                    action("hint-unstage-all", "U", any())
                        .label("unstage all")
                        .footer(9)
                        .hint_group("unstage"),
                )
                .action(action("hint-after", "ctrl-F10", any()).footer(10));
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
        if log.watch_failure_after_act && !log.acts.is_empty() {
            return Err(Failure::external("project watch failed"));
        }
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
        if log.view_failure_after_act && !log.acts.is_empty() {
            return Err(Failure::external("project refresh failed"));
        }
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
        let row_name = log.row_name.clone();
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
                header: vec![],
                hunks: vec![DiffHunk {
                    path: None,
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
        if let View::List(list) = &mut view {
            list.rows[0].name = row_name;
        }
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
        let default = Value::Bool(log.prompt_default.unwrap_or(true));
        log.prompt_settings.push(!matches!(
            cx.settings.get("confirm_sync").unwrap_or(&default),
            Value::Bool(false)
        ));
        log.acts.push((action.into(), context.clone()));
        log.threads.push(std::thread::current().id());
        drop(log);
        if let Some(started) = &self.started {
            started.open();
        }
        if let Some(hold) = &self.hold {
            hold.wait();
        }
        let mut log = self.log.lock().unwrap();
        if log.act_panic {
            drop(log);
            panic!("project action panic");
        }
        if log.act_failures > 0 {
            log.act_failures -= 1;
            return Err(log
                .act_error
                .clone()
                .unwrap_or_else(|| Failure::external("project action failed")));
        }
        let retry_outcome = log.outcome.clone();
        drop(log);
        Ok(match action {
            "retry-input" | "retry-confirmed" | "retry-refresh" => {
                retry_outcome.unwrap_or(Outcome::Unhandled)
            }
            "push" | "push-alias" => Outcome::PushScreen {
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
