use std::io::{self, Write};
use std::process::ExitCode;

pub mod clock_build;

use konoma_plugin::prelude::*;
use konoma_plugin::proc::{frame, proto::*, wire::*};

struct Second;
impl Plugin for Second {
    fn manifest(&self) -> Manifest {
        let mut m = manifest("good");
        m.id = "second".into();
        m
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Second))
    }
}
impl Session for Second {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::Canvas(CanvasView::new(1, 1)))
    }
}

fn manifest(mode: &str) -> Manifest {
    let mut m = Manifest::new("fixture", "0.1.0")
        .bind(Bind::files(["*.konoma-demo"]))
        .shows(ViewKind::Canvas)
        .action(
            Act::new("change")
                .key("c")
                .label("Change")
                .help("Change the fixture"),
        );
    match mode {
        "manifest-id" => m.id = "Bad ID".into(),
        "manifest-declaration" => m.actions[0].id = "Bad Action".into(),
        "manifest-glob" => m.binds = vec![Bind::files(["["])],
        "manifest" => m.shows.clear(),
        "manifest-keys" => {
            m.actions[0].key = Some("Q".into());
            m.actions
                .push(Act::new("collision").key("Q").help("collision"));
        }
        "hints" => {
            m.actions[0].help = "".into();
            m.actions[0].label = "".into();
        }
        "manifest-destructive" => m.actions[0].destructive = true,
        "project-markers" => m.project = Some(ProjectDecl::new()),
        "project-v1" => {
            m.project =
                Some(ProjectDecl::new().marker(DetectionMarker::new(".git", MarkerKind::Any)))
        }
        _ => {}
    }
    m
}

fn state(mode: &str) -> WState {
    WState {
        layout: layout_to_wire(&Layout::single()),
        enabled: vec![("change".into(), mode != "disabled")],
    }
}

fn err() -> Answer {
    Answer::Err(failure_to_wire(&Failure::unsupported("fixture error")))
}

fn park() -> ! {
    loop {
        std::thread::park();
    }
}

pub fn fixture(mode: &str, args: &[String]) -> ExitCode {
    if mode == "signal" {
        use std::os::fd::FromRawFd;
        // SAFETY: the test launcher reserves fd 3 for this readiness handshake.
        let mut ready = unsafe { std::fs::File::from_raw_fd(3) };
        ready.write_all(&std::process::id().to_ne_bytes()).unwrap();
        drop(ready);
        park();
    }
    if mode == "multi" {
        // Same SDK entry used by export!(A, B), retaining the real --plugin selector.
        return konoma_plugin::proc::cli_main_many(vec![
            Box::new(konoma_demo_clock::Clock::default()),
            Box::new(Second),
        ]);
    }
    if mode == "relative-args" {
        assert!(
            std::path::Path::new(&args[0]).is_file(),
            "relative argument must resolve in caller's directory"
        );
    }
    assert_eq!(args.last().map(String::as_str), Some("--serve"));
    let mut input = io::stdin();
    let mut output = io::stdout();
    eprintln!("fixture diagnostic\nsecond line");
    let hello: Hello = frame::recv(&mut input).unwrap().unwrap();
    if mode == "handshake" {
        park();
    }
    if !hello.versions.contains(&PROTOCOL_VERSION) && mode != "refusal" {
        frame::send(
            &mut output,
            &HelloReply::Refused {
                supported: if mode == "refusal-empty" {
                    vec![]
                } else {
                    vec![PROTOCOL_VERSION]
                },
                reason: "unsupported version".into(),
            },
        )
        .unwrap();
        if mode == "refusal-crash" {
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }
    let mut wm = manifest_to_wire(&manifest(mode)).unwrap();
    if mode.starts_with("chip") {
        wm.project = Some(project_to_wire(
            &ProjectDecl::new().marker(DetectionMarker::new(".git", MarkerKind::Any)),
        ));
        wm.session_chip = true;
        if mode == "chip-legacy" {
            wm.session_chip = false;
        }
        if mode == "chip-no-project" || mode == "chip-v1" {
            wm.project = None;
        }
    }
    if mode == "handshake-manifest" {
        let mut value = WValue::Bool(true);
        for _ in 0..34 {
            value = WValue::List(vec![value]);
        }
        wm.settings.push(WSetting {
            name: "nested".into(),
            default: value,
            help: tr_to_wire(&"nested".into()),
        });
    }
    frame::send(
        &mut output,
        &HelloReply::Ack {
            version: if mode == "handshake-version" {
                u32::MAX
            } else if mode == "project-v1" || mode == "outcome-v1" || mode == "chip-v1" {
                1
            } else {
                PROTOCOL_VERSION
            },
            manifest: wm,
        },
    )
    .unwrap();
    let side = Side::new(path_from_wire(hello.side_dir).unwrap(), 'c');
    let mut opened = false;
    loop {
        let msg = match frame::recv::<_, ToChild>(&mut input) {
            Ok(Some(msg)) => msg,
            Ok(None) => return ExitCode::SUCCESS,
            Err(e) => {
                let ignore = match &e {
                    frame::FrameError::TooLarge(_) => mode == "oversized",
                    _ => mode == "malformed",
                };
                if ignore {
                    continue;
                }
                return ExitCode::FAILURE;
            }
        };
        let (rid, answer) = match msg {
            ToChild::WatchPaths { .. } => {
                panic!("fixture does not advertise session_watch")
            }
            ToChild::TabClosed { .. } => {
                panic!("fixture does not advertise session_tab_closed")
            }
            ToChild::Open { rid, input, .. } => {
                if mode.starts_with("chip") {
                    assert!(
                        !matches!(mode, "chip-legacy" | "chip-no-project" | "chip-v1"),
                        "unsupported chip must not be opened"
                    );
                    assert!(
                        matches!(input, WInput::Project { root, marker } if root == path_to_wire(std::path::Path::new("/project")) && marker == ".git")
                    );
                }
                if mode == "open" {
                    park();
                }
                opened = true;
                (rid, Answer::Ok(Reply::Opened { state: state(mode) }))
            }
            ToChild::Chip { sid, rid, cx } => {
                assert!(opened && sid == 1);
                assert_eq!(
                    cx.lang,
                    if matches!(mode, "chip" | "chip-none" | "chip-empty") {
                        "ja"
                    } else {
                        "en"
                    }
                );
                match mode {
                    "chip-timeout" => park(),
                    "chip-eof" => return ExitCode::FAILURE,
                    "chip-frame" => {
                        frame::write_frame(&mut output, &[0xc1]).unwrap();
                        continue;
                    }
                    "chip-rid" => {
                        frame::send(
                            &mut output,
                            &FromChild {
                                rid: rid + 1,
                                answer: err(),
                            },
                        )
                        .unwrap();
                        continue;
                    }
                    "chip-error" => {
                        frame::send(&mut output, &FromChild { rid, answer: err() }).unwrap();
                        continue;
                    }
                    "chip-wrong" => {
                        frame::send(
                            &mut output,
                            &FromChild {
                                rid,
                                answer: Answer::Ok(Reply::Opened { state: state(mode) }),
                            },
                        )
                        .unwrap();
                        continue;
                    }
                    "chip-dirty" => {
                        let chip = WChip::new(line_to_wire(&Line::from(vec![
                            Span::raw(
                                "\n\x1b".to_string() + &"界".repeat(600)
                            );
                            65
                        ])));
                        frame::send(
                            &mut output,
                            &FromChild {
                                rid,
                                answer: Answer::Ok(Reply::Chip {
                                    chip: Some(chip),
                                    state: state(mode),
                                }),
                            },
                        )
                        .unwrap();
                        continue;
                    }
                    _ => {}
                }
                let chip = match mode {
                    "chip-none" => None,
                    "chip-empty" => Some(Chip::default()),
                    _ => Some(Chip::new(vec![Span::styled(
                        "⎇ main",
                        Style::new().fg(Color::Green).bold(),
                    )])),
                };
                (
                    rid,
                    Answer::Ok(Reply::Chip {
                        chip: chip.as_ref().map(chip_to_wire),
                        state: state(mode),
                    }),
                )
            }
            ToChild::View { sid, rid, .. } => {
                if !opened || sid != 1 {
                    if mode == "unknown" {
                        return ExitCode::FAILURE;
                    }
                    if mode == "unknown-ok" && rid == 2 {
                        (rid, Answer::Ok(Reply::Opened { state: state(mode) }))
                    } else {
                        (rid, err())
                    }
                } else if mode == "cancel" && rid == 3 {
                    // A call that does not answer even when the subsequent Cancel arrives.
                    continue;
                } else {
                    if rid == 2 {
                        match mode {
                            "hints-eof" => return ExitCode::FAILURE,
                            "hints-timeout" => park(),
                            "hints-frame" => {
                                frame::write_frame(&mut output, &[0xc1]).unwrap();
                                continue;
                            }
                            "hints-wrong-reply" => {
                                frame::send(
                                    &mut output,
                                    &FromChild {
                                        rid,
                                        answer: Answer::Ok(Reply::Opened { state: state(mode) }),
                                    },
                                )
                                .unwrap();
                                continue;
                            }
                            "hints-wrong-rid" => {
                                frame::send(
                                    &mut output,
                                    &FromChild {
                                        rid: rid + 1,
                                        answer: err(),
                                    },
                                )
                                .unwrap();
                                continue;
                            }
                            "hints-view-error" => {
                                frame::send(&mut output, &FromChild { rid, answer: err() })
                                    .unwrap();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if mode == "view-timeout" {
                        park();
                    }
                    let mut view = if mode == "view" {
                        View::Text(konoma_plugin::view::TextView {
                            source: konoma_plugin::view::TextSource::Owned("undeclared".into()),
                            styler: None,
                            file_lines: false,
                            syntax_name: None,
                        })
                    } else {
                        View::Canvas(CanvasView::new(1, 1))
                    };
                    let view = view_to_wire(&mut view, &mut Enc::new(&side)).unwrap();
                    (
                        rid,
                        Answer::Ok(Reply::Viewed {
                            view,
                            state: state(mode),
                        }),
                    )
                }
            }
            ToChild::Act { rid, .. } | ToChild::Key { rid, .. } => {
                if mode == "act" && rid == 4 || mode == "key" && rid == 5 {
                    return ExitCode::FAILURE;
                }
                if mode == "act-timeout" && rid == 4 {
                    park();
                }
                if mode == "act-frame" && rid == 4 {
                    frame::write_frame(&mut output, &[0xc1]).unwrap();
                    continue;
                }
                (
                    rid,
                    if mode == "action-error" {
                        err()
                    } else {
                        Answer::Ok(Reply::Acted {
                            outcome: match mode {
                                "outcome-push" => outcome_to_wire(&Outcome::PushScreen {
                                    screen: "detail".into(),
                                    selection: Some("row".into()),
                                }),
                                "outcome-file" => outcome_to_wire(&Outcome::OpenFile {
                                    path: "/project/file".into(),
                                    line: Some(3),
                                }),
                                "outcome-diff" => outcome_to_wire(&Outcome::OpenDiff {
                                    path: "/project/file".into(),
                                }),
                                "outcome-root" => outcome_to_wire(&Outcome::SwitchRoot {
                                    root: "/project".into(),
                                    message: None,
                                }),
                                "outcome-tab" | "outcome-v1" => {
                                    outcome_to_wire(&Outcome::OpenTab {
                                        path: "/project".into(),
                                    })
                                }
                                "outcome-relative" => outcome_to_wire(&Outcome::OpenTab {
                                    path: "relative".into(),
                                }),
                                _ => WOutcome::Unhandled,
                            },
                            state: state(mode),
                        })
                    },
                )
            }
            ToChild::Cancel { .. } | ToChild::Close { .. } => continue,
            ToChild::Shutdown => {
                if mode == "shutdown" {
                    park();
                }
                if mode == "shutdown-error" {
                    return ExitCode::FAILURE;
                }
                return ExitCode::SUCCESS;
            }
        };
        frame::send(&mut output, &FromChild { rid, answer }).unwrap();
        output.flush().unwrap();
        if mode == "act-exit-after-reply" && rid == 4
            || mode == "key-exit-after-reply" && rid == 5
            || mode == "chip-exit-after-reply" && rid == 2
        {
            // A protocol barrier makes death visible on the subsequent read, without a race
            // between EOF and a broken write or any sleep to schedule the parent/child.
            let _: Option<ToChild> = frame::recv(&mut input).unwrap();
            return ExitCode::SUCCESS;
        }
    }
}
