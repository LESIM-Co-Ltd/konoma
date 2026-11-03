use std::io::{self, Write};
use std::process::ExitCode;

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
            ToChild::Open { rid, .. } => {
                if mode == "open" {
                    park();
                }
                opened = true;
                (rid, Answer::Ok(Reply::Opened { state: state(mode) }))
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
                            outcome: WOutcome::Unhandled,
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
        if mode == "act-exit-after-reply" && rid == 4 || mode == "key-exit-after-reply" && rid == 5
        {
            // A protocol barrier makes death visible on the subsequent read, without a race
            // between EOF and a broken write or any sleep to schedule the parent/child.
            let _: Option<ToChild> = frame::recv(&mut input).unwrap();
            return ExitCode::SUCCESS;
        }
    }
}
