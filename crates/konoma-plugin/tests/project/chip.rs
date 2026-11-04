use konoma_plugin::view::{ListView, View};
use konoma_plugin::*;

struct Static;
impl Session for Static {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(ListView::default()))
    }
}

#[test]
fn chip_default_uses_declaration() {
    let mut session: Box<dyn Session> = Box::new(Static);
    assert_eq!(session.chip(&ViewCx::bare()).unwrap(), None);
}

#[derive(Clone, Default)]
struct Changing(bool);
impl Plugin for Changing {
    fn manifest(&self) -> Manifest {
        Manifest::new("chip", "1").project(ProjectDecl::new().chip("declared"))
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(self.clone()))
    }
}
impl Session for Changing {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::List(ListView::default()))
    }
    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.0 = true;
        Ok(Outcome::Redraw)
    }
    fn chip(&mut self, cx: &ViewCx) -> Result<Option<Chip>, Failure> {
        assert_eq!(cx.lang, "ja");
        Ok(Some(Chip::new(vec![Span::styled(
            if self.0 { "⎇ dev" } else { "⎇ main" },
            Style::new().fg(Color::Green).bold(),
        )])))
    }
}

#[test]
fn chip_static_plugin_changes_between_calls() {
    let mut session = Changing::default()
        .open(
            Input::Project {
                root: "/p".into(),
                marker: ".git".into(),
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let cx = ViewCx::new(Settings::empty(), "ja");
    let first = konoma_plugin::testing::chip_with(&mut *session, &cx)
        .unwrap()
        .unwrap();
    session.act("switch", &ActCx::bare()).unwrap();
    let second = konoma_plugin::testing::chip_with(&mut *session, &cx)
        .unwrap()
        .unwrap();
    assert_eq!(first.text.plain(), "⎇ main");
    assert_eq!(second.text.plain(), "⎇ dev");
    assert_eq!(first.text.spans[0].style, second.text.spans[0].style);
}

struct ChipAnswer(Chip);
impl Session for ChipAnswer {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        unreachable!()
    }
    fn chip(&mut self, _: &ViewCx) -> Result<Option<Chip>, Failure> {
        Ok(Some(self.0.clone()))
    }
}

#[test]
fn chip_clips_total_text_and_preserves_styles() {
    let style = Style::new().fg(Color::Cyan).bold();
    let mut session = ChipAnswer(Chip::new(vec![
        Span::raw("あ".repeat(300)),
        Span::styled("い".repeat(300), style),
    ]));
    let chip = konoma_plugin::testing::chip_with(&mut session, &ViewCx::bare())
        .unwrap()
        .unwrap();
    assert_eq!(
        chip.text.plain(),
        format!("{}{}…", "あ".repeat(300), "い".repeat(211))
    );
    assert_eq!(chip.text.spans[1].style, style);
    assert_eq!(
        Chip::new("x".repeat(512)).clipped().text.plain(),
        "x".repeat(512)
    );
}

#[test]
fn chip_strips_all_controls_and_reserved_placeholder() {
    let mut session = ChipAnswer(Chip::new("a\0\t\nb\r\x1b\x7f\u{85}\u{10eeee}c"));
    let chip = konoma_plugin::testing::chip_with(&mut session, &ViewCx::bare())
        .unwrap()
        .unwrap();
    assert_eq!(chip.text.plain(), "abc");
}

#[test]
fn chip_bounds_spans_even_when_empty() {
    use konoma_plugin::project::MAX_CHIP_SPANS;
    let mut session = ChipAnswer(Chip::new(vec![Span::default(); MAX_CHIP_SPANS + 1]));
    let chip = konoma_plugin::testing::chip_with(&mut session, &ViewCx::bare())
        .unwrap()
        .unwrap();
    assert_eq!(chip.text.spans.len(), MAX_CHIP_SPANS);
}

#[cfg(feature = "proc")]
mod transport {
    use super::*;
    use crate::host_api::transport::{connect, finish};
    use konoma_plugin::proc::{frame, proto::*, wire::*};

    #[test]
    fn chip_non_project_manifest_has_no_capability() {
        let manifest = Manifest::new("file-only", "1");
        assert!(manifest.project.is_none());
        let wire = manifest_to_wire(&manifest).unwrap();
        assert!(!wire.session_chip);
        assert!(serde_json::to_value(&wire)
            .unwrap()
            .get("session_chip")
            .is_none());
        let decoded: WManifest = frame::decode(&frame::encode(&wire).unwrap()).unwrap();
        assert!(!decoded.session_chip);
        assert!(manifest_from_wire(decoded).unwrap().project.is_none());
    }

    #[test]
    fn chip_non_project_child_does_not_advertise_capability() {
        struct FileOnly;
        impl Plugin for FileOnly {
            fn manifest(&self) -> Manifest {
                Manifest::new("file-only", "1")
            }
            fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
                Ok(Box::new(Static))
            }
        }
        let (mut host, child) = std::os::unix::net::UnixStream::pair().unwrap();
        let thread = std::thread::spawn(move || {
            konoma_plugin::proc::serve_io(
                FileOnly,
                child.try_clone().unwrap(),
                child,
                &Default::default(),
            )
        });
        let dir = konoma_test_support::unique_tmp("chip_file_only");
        frame::send(
            &mut host,
            &Hello {
                versions: vec![2],
                host_version: "test".into(),
                side_dir: path_to_wire(dir.as_path()),
                lang: "en".into(),
            },
        )
        .unwrap();
        let HelloReply::Ack { version, manifest } = frame::recv(&mut host).unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(version, 2);
        assert!(manifest.project.is_none());
        assert!(!manifest.session_chip);
        assert!(serde_json::to_value(&manifest)
            .unwrap()
            .get("session_chip")
            .is_none());
        frame::send(&mut host, &ToChild::Shutdown).unwrap();
        thread.join().unwrap().unwrap();
    }

    fn input() -> Input {
        Input::Project {
            root: "/p".into(),
            marker: ".git".into(),
        }
    }

    #[test]
    fn chip_client_server_changes_and_passes_context() {
        let (plugin, threads, _dir) = connect(|child| {
            konoma_plugin::proc::serve_io(
                Changing::default(),
                child.try_clone()?,
                child,
                &Default::default(),
            )
        });
        let mut session = plugin.open(input(), &OpenCx::bare()).unwrap();
        let cx = ViewCx::new(Settings::empty(), "ja");
        assert_eq!(session.chip(&cx).unwrap().unwrap().text.plain(), "⎇ main");
        session.act("switch", &ActCx::bare()).unwrap();
        assert_eq!(session.chip(&cx).unwrap().unwrap().text.plain(), "⎇ dev");
        drop(session);
        finish(plugin, threads);
    }

    #[derive(Clone)]
    struct Answers;
    impl Plugin for Answers {
        fn manifest(&self) -> Manifest {
            Changing::default().manifest()
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(self.clone()))
        }
    }
    impl Session for Answers {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::List(ListView::default()))
        }
        fn chip(&mut self, cx: &ViewCx) -> Result<Option<Chip>, Failure> {
            match cx.lang {
                "panic" => panic!("chip panic"),
                "failure" => {
                    Err(Failure::io(t!(en: "chip failed", ja: "取得失敗"))
                        .with_detail("chip detail"))
                }
                "dirty" => Ok(Some(Chip::new(vec![
                    Span::raw(
                        "\n\x1b".to_string() + &"界".repeat(600)
                    );
                    65
                ]))),
                _ => Ok(None),
            }
        }
    }

    fn answer_plugin() -> (
        konoma_plugin::proc::client::ProcPlugin,
        crate::host_api::transport::Threads,
        konoma_test_support::TmpDir,
    ) {
        connect(|child| {
            konoma_plugin::proc::serve_io(Answers, child.try_clone()?, child, &Default::default())
        })
    }

    #[test]
    fn chip_server_reports_failure_and_keeps_session() {
        let (plugin, threads, _dir) = answer_plugin();
        let mut session = plugin.open(input(), &OpenCx::bare()).unwrap();
        let failure = session
            .chip(&ViewCx::new(Settings::empty(), "failure"))
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Io);
        assert_eq!(failure.message.get("ja"), "取得失敗");
        assert_eq!(failure.detail.as_deref(), Some("chip detail"));
        assert_eq!(session.chip(&ViewCx::bare()).unwrap(), None);
        session.view(&ViewReq::new(1, 1), &ViewCx::bare()).unwrap();
        drop(session);
        finish(plugin, threads);
    }

    #[test]
    fn chip_server_reports_panic_and_keeps_session() {
        let (plugin, threads, _dir) = answer_plugin();
        let mut session = plugin.open(input(), &OpenCx::bare()).unwrap();
        let failure = session
            .chip(&ViewCx::new(Settings::empty(), "panic"))
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::External);
        assert_eq!(failure.detail.as_deref(), Some("panic in chip: chip panic"));
        assert_eq!(session.chip(&ViewCx::bare()).unwrap(), None);
        session.view(&ViewReq::new(1, 1), &ViewCx::bare()).unwrap();
        drop(session);
        finish(plugin, threads);
    }

    #[test]
    fn chip_file_session_is_not_called() {
        let (plugin, threads, _dir) = answer_plugin();
        let mut session = plugin
            .open(
                Input::File {
                    path: "/unused".into(),
                    hint: None,
                },
                &OpenCx::bare(),
            )
            .unwrap();
        assert_eq!(
            session
                .chip(&ViewCx::new(Settings::empty(), "panic"))
                .unwrap(),
            None
        );
        drop(session);
        finish(plugin, threads);
    }

    #[test]
    fn chip_server_clips_answer() {
        let (mut host, child) = std::os::unix::net::UnixStream::pair().unwrap();
        let thread = std::thread::spawn(move || {
            konoma_plugin::proc::serve_io(
                Answers,
                child.try_clone().unwrap(),
                child,
                &Default::default(),
            )
        });
        let dir = konoma_test_support::unique_tmp("chip_raw");
        frame::send(
            &mut host,
            &Hello {
                versions: vec![2],
                host_version: "test".into(),
                side_dir: path_to_wire(dir.as_path()),
                lang: "en".into(),
            },
        )
        .unwrap();
        let HelloReply::Ack { manifest, .. } = frame::recv(&mut host).unwrap().unwrap() else {
            panic!()
        };
        assert!(manifest.session_chip);
        frame::send(
            &mut host,
            &ToChild::Open {
                sid: 1,
                rid: 1,
                input: WInput::Project {
                    root: path_to_wire(std::path::Path::new("/p")),
                    marker: ".git".into(),
                },
                cx: WCx::new("en"),
            },
        )
        .unwrap();
        let _: FromChild = frame::recv(&mut host).unwrap().unwrap();
        frame::send(
            &mut host,
            &ToChild::Chip {
                sid: 1,
                rid: 2,
                cx: WCx::new("dirty"),
            },
        )
        .unwrap();
        let answer: FromChild = frame::recv(&mut host).unwrap().unwrap();
        let Answer::Ok(Reply::Chip {
            chip: Some(chip), ..
        }) = answer.answer
        else {
            panic!()
        };
        let chip = Chip::new(line_from_wire(chip.text));
        assert_eq!(chip.text.plain(), format!("{}…", "界".repeat(511)));
        assert!(chip.text.spans.len() <= konoma_plugin::project::MAX_CHIP_SPANS);
        frame::send(&mut host, &ToChild::Shutdown).unwrap();
        thread.join().unwrap().unwrap();
    }

    #[test]
    fn chip_older_child_falls_back_without_sending_unknown_call() {
        // An old protocol-2 child knows projects, but has no Chip variant or capability.
        let (plugin, threads, _dir) = connect(|mut child| {
            let _: Hello = frame::recv(&mut child).unwrap().unwrap();
            let manifest = manifest_to_wire(&Changing::default().manifest()).unwrap();
            let json = serde_json::to_value(&manifest).unwrap();
            assert!(json.get("session_chip").is_none());
            frame::send(
                &mut child,
                &HelloReply::Ack {
                    version: 2,
                    manifest,
                },
            )
            .unwrap();
            let ToChild::Open { rid, .. } = frame::recv(&mut child).unwrap().unwrap() else {
                panic!()
            };
            frame::send(
                &mut child,
                &FromChild {
                    rid,
                    answer: Answer::Ok(Reply::Opened {
                        state: WState {
                            layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                            enabled: vec![],
                        },
                    }),
                },
            )
            .unwrap();
            loop {
                match frame::recv::<_, ToChild>(&mut child).unwrap() {
                    Some(ToChild::Close { .. }) => {}
                    Some(ToChild::Shutdown) | None => break,
                    other => panic!("old child received an unknown call: {other:?}"),
                }
            }
            Ok(())
        });
        let mut session = plugin.open(input(), &OpenCx::bare()).unwrap();
        assert_eq!(session.chip(&ViewCx::bare()).unwrap(), None);
        assert_eq!(session.chip(&ViewCx::bare()).unwrap(), None);
        drop(session);
        finish(plugin, threads);
    }

    #[test]
    fn chip_wire_round_trip_both_directions() {
        assert_eq!(PROTOCOL_VERSION, 2);
        let cx = WCx::new("ja");
        let call = ToChild::Chip {
            sid: 9,
            rid: 12,
            cx,
        };
        assert_eq!(
            frame::decode::<ToChild>(&frame::encode(&call).unwrap()).unwrap(),
            call
        );
        let chip = Chip::new(vec![Span::styled(
            "⎇ main",
            Style::new().fg(Color::Green).bold(),
        )]);
        for chip in [Some(chip), None, Some(Chip::default())] {
            let reply = FromChild {
                rid: 12,
                answer: Answer::Ok(Reply::Chip {
                    chip: chip.as_ref().map(chip_to_wire),
                    state: WState {
                        layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                        enabled: vec![],
                    },
                }),
            };
            let back: FromChild = frame::decode(&frame::encode(&reply).unwrap()).unwrap();
            assert_eq!(back, reply);
            let Answer::Ok(Reply::Chip { chip: back, .. }) = back.answer else {
                panic!()
            };
            assert_eq!(back.map(chip_from_wire), chip);
        }
        let dirty = Chip::new("\n\x1b".to_string() + &"界".repeat(600));
        assert_eq!(
            line_from_wire(chip_to_wire(&dirty).text),
            dirty.clipped().text
        );
        assert_eq!(chip_from_wire(chip_to_wire(&dirty)), dirty.clipped());
        // The receiver also clips a hostile child's unbounded answer.
        let hostile = WChip::new(line_to_wire(&dirty.text));
        assert_eq!(chip_from_wire(hostile), dirty.clipped());
    }

    #[test]
    fn chip_v2_vector_decodes_and_reencodes_exact_bytes() {
        let call = ToChild::Chip {
            sid: 9,
            rid: 12,
            cx: WCx::new("ja"),
        };
        let reply = FromChild {
            rid: 12,
            answer: Answer::Ok(Reply::Chip {
                chip: Some(chip_to_wire(&Chip::new(vec![Span::styled(
                    "⎇ main",
                    Style::new().fg(Color::Green).bold(),
                )]))),
                state: WState {
                    layout: layout_to_wire(&konoma_plugin::view::Layout::single()),
                    enabled: vec![],
                },
            }),
        };
        let records = [
            frame::encode(&call).unwrap(),
            frame::encode(&reply).unwrap(),
        ];
        let vectors = include_str!("vectors/project-chip-v2.msgpack.hex");
        for (i, name) in ["call", "reply"].iter().enumerate() {
            let hex = vectors
                .lines()
                .filter_map(|line| line.split_once(' '))
                .find(|(key, _)| key == name)
                .unwrap()
                .1;
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect();
            assert_eq!(bytes, records[i]);
            if i == 0 {
                let decoded: ToChild = frame::decode(&bytes).unwrap();
                assert_eq!(decoded, call);
                assert_eq!(frame::encode(&decoded).unwrap(), bytes);
            } else {
                let decoded: FromChild = frame::decode(&bytes).unwrap();
                assert_eq!(decoded, reply);
                assert_eq!(frame::encode(&decoded).unwrap(), bytes);
            }
        }
    }
}
