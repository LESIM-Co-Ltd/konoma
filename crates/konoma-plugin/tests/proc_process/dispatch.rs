use super::*;

#[test]
fn file_dispatch_without_arguments_does_not_call_the_app() {
    let code = cli_dispatch(Vec::new(), Vec::new(), false, |_, _, _| {
        panic!("a file export without arguments must not open cwd")
    });
    assert_eq!(code, ExitCode::from(2));
}

#[cfg(feature = "app")]
mod projects {
    use super::*;
    use crate::app::project::{ProjectBackend, ProjectServiceInfo, ProjectServices};
    use crate::app::{AppError, AppEvent};
    use crate::host::project::MatchedProject;
    use crate::prelude::*;
    use crate::TermCaps;
    use konoma_vcs_core::git::{ChangeEntry, DiffLine};
    use std::path::{Path, PathBuf};

    struct Fixture {
        id: &'static str,
        fail_open: bool,
        remove: Option<PathBuf>,
    }

    impl Plugin for Fixture {
        fn manifest(&self) -> Manifest {
            if let Some(dir) = &self.remove {
                std::fs::remove_dir(dir).unwrap();
            }
            Manifest::new(self.id, "1.0.0").project(
                ProjectDecl::new()
                    .marker(DetectionMarker::new("project.marker", MarkerKind::File))
                    .screen(ProjectScreen::new("items", "Items")),
            )
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            if self.fail_open {
                return Err(Failure::external("owner failed"));
            }
            Ok(Box::new(FixtureSession))
        }
    }

    struct FixtureSession;
    impl Session for FixtureSession {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::Message(crate::view::MessageView { lines: vec![] }))
        }
    }

    struct Services(bool);
    impl ProjectServices for Services {
        fn inspect(
            &self,
            p: &MatchedProject,
            _: &crate::app::project::ProjectSettings,
        ) -> Result<ProjectServiceInfo, Failure> {
            if self.0 {
                return Err(Failure::external("backend failed"));
            }
            Ok(ProjectServiceInfo {
                workdir: p.root.clone(),
                backend: ProjectBackend::Git,
            })
        }
        fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
            vec![]
        }
        fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
            vec![]
        }
        fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
            None
        }
    }

    fn fixture() -> Box<dyn Plugin> {
        Box::new(Fixture {
            id: "fixture",
            fail_open: false,
            remove: None,
        })
    }

    struct FailedTerminal;
    impl ratatui::backend::Backend for FailedTerminal {
        type Error = std::io::Error;
        fn draw<'a, I>(&mut self, _: I) -> Result<(), Self::Error>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Err(std::io::Error::other("terminal draw failed"))
        }
        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn get_cursor_position(&mut self) -> Result<ratatui::layout::Position, Self::Error> {
            Ok((0, 0).into())
        }
        fn set_cursor_position<P: Into<ratatui::layout::Position>>(
            &mut self,
            _: P,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn clear(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn clear_region(&mut self, _: ratatui::backend::ClearType) -> Result<(), Self::Error> {
            Ok(())
        }
        fn size(&self) -> Result<ratatui::layout::Size, Self::Error> {
            Ok((60, 15).into())
        }
        fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
            Ok(ratatui::backend::WindowSize {
                columns_rows: (60, 15).into(),
                pixels: (600, 300).into(),
            })
        }
        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    // Only this test adapter injects configuration and terminal entry. Preparation, chooser
    // handling, the loop, and CLI error mapping are the production implementations.
    fn run(
        plugins: Vec<Box<dyn Plugin>>,
        args: Vec<std::ffi::OsString>,
        fail_backend: bool,
        terminal: impl FnOnce(&mut crate::app::project::PreparedProjects) -> Result<(), AppError>,
    ) -> ExitCode {
        project_dispatch(plugins, args, |plugins, dir| {
            let prepared = crate::app::project::prepare_projects(
                plugins.into_iter().map(Arc::from).collect(),
                dir,
                TermCaps::new(),
                (konoma_host_core::shared::SharedConfig::default(), None),
                Arc::new(Services(fail_backend)),
            );
            app_exit(match prepared {
                Ok(mut prepared) => terminal(&mut prepared),
                Err(failure) => Err(failure.error),
            })
        })
    }

    #[test]
    fn project_cli_refuses_missing_not_directory_and_no_project_with_two() {
        let dir = konoma_test_support::unique_tmp("cli_project_refusal");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file");
        std::fs::write(&file, "file").unwrap();
        for path in [
            dir.join("missing"),
            file.clone(),
            file.join("child"),
            dir.to_path_buf(),
        ] {
            assert_eq!(
                run(
                    vec![fixture()],
                    vec![path.into_os_string()],
                    false,
                    |_| panic!("refused before terminal")
                ),
                ExitCode::from(2)
            );
        }
    }

    #[test]
    fn project_cli_usage_errors_exit_two_before_terminal() {
        for args in [
            vec!["--unknown"],
            vec![".", "extra"],
            vec![".", "--unknown"],
            vec!["--plugin", "unknown", "."],
        ] {
            assert_eq!(
                run(
                    vec![fixture()],
                    args.into_iter().map(Into::into).collect(),
                    false,
                    |_| panic!("usage never enters the terminal")
                ),
                ExitCode::from(2)
            );
        }
    }

    #[test]
    fn project_cli_detection_failure_exits_one() {
        let dir = konoma_test_support::unique_tmp("cli_project_detection");
        std::fs::create_dir_all(&dir).unwrap();
        let plugin = Fixture {
            id: "fixture",
            fail_open: false,
            remove: Some(dir.to_path_buf()),
        };
        assert_eq!(
            run(
                vec![Box::new(plugin)],
                vec![dir.as_os_str().into()],
                false,
                |_| panic!("detection failed before terminal")
            ),
            ExitCode::from(1)
        );
    }

    #[test]
    fn project_cli_backend_and_owner_failure_exit_one() {
        let dir = konoma_test_support::unique_tmp("cli_project_backend");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.marker"), "").unwrap();
        for fail_backend in [false, true] {
            let plugin = Fixture {
                id: "fixture",
                fail_open: !fail_backend,
                remove: None,
            };
            assert_eq!(
                run(
                    vec![Box::new(plugin)],
                    vec![dir.as_os_str().into()],
                    fail_backend,
                    |_| panic!("backend/owner failed before terminal")
                ),
                ExitCode::from(1)
            );
        }
    }

    #[test]
    fn project_cli_terminal_failure_exits_one() {
        let dir = konoma_test_support::unique_tmp("cli_project_terminal");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.marker"), "").unwrap();
        assert_eq!(
            run(vec![fixture()], vec![dir.as_os_str().into()], false, |_| {
                Err(AppError::NoTerminal)
            }),
            ExitCode::from(1)
        );
        assert_eq!(
            run(
                vec![fixture()],
                vec![dir.as_os_str().into()],
                false,
                |prepared| {
                    let mut terminal = ratatui::Terminal::new(FailedTerminal).unwrap();
                    let mut events = [AppEvent::Key(Key::char('q'))].into_iter();
                    // Fail the backend used by the actual project loop, rather than manufacturing a
                    // terminal AppError in the CLI adapter.
                    crate::app::project::run_project_with(
                        &mut terminal,
                        prepared,
                        &mut events,
                        &Default::default(),
                        None,
                    )
                }
            ),
            ExitCode::from(1)
        );
    }

    #[test]
    fn project_cli_chooser_cancel_exits_zero() {
        let dir = konoma_test_support::unique_tmp("cli_project_cancel");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.marker"), "").unwrap();
        assert_eq!(
            run(
                vec![
                    fixture(),
                    Box::new(Fixture {
                        id: "second",
                        fail_open: false,
                        remove: None
                    })
                ],
                vec![dir.as_os_str().into()],
                false,
                |prepared| {
                    assert!(prepared.dialog.is_some());
                    let mut terminal =
                        ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 15)).unwrap();
                    let mut events = [AppEvent::Key(Key::new(KeyCode::Esc))].into_iter();
                    crate::app::project::run_project_with(
                        &mut terminal,
                        prepared,
                        &mut events,
                        &Default::default(),
                        None,
                    )?;
                    assert!(prepared.dialog.is_none(), "the chooser was cancelled");
                    assert!(prepared.screen.is_none(), "no project screen was chosen");
                    Ok(())
                }
            ),
            ExitCode::from(0)
        );
    }
}
