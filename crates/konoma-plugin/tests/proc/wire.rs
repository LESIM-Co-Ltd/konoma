//! Round trips of every wire type, and the checks on what an untrusted child sends.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use konoma_plugin::proc::frame;
use konoma_plugin::proc::wire::*;
use konoma_plugin::view::*;
use konoma_plugin::*;
use konoma_test_support::unique_tmp;

fn side_in(dir: &Path) -> Side {
    Side::new(dir, 'x')
}

/// Send `v` through bytes and back.
fn rt_view(mut v: View, side: &Side) -> Result<View, WireError> {
    let w = view_to_wire(&mut v, &mut Enc::new(side))?;
    let bytes = frame::encode(&w).unwrap();
    let w2: WView = frame::decode(&bytes).unwrap();
    view_from_wire(w2, &mut Dec::new(side))
}

fn rt_ok(v: View) -> View {
    let dir = unique_tmp("kp_wire");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    rt_view(v, &side_in(dir.as_path())).unwrap()
}

fn dbg(v: &View) -> String {
    format!("{v:?}")
}

fn style_bold_red() -> Style {
    Style::new()
        .fg(Color::Rgb(1, 2, 3))
        .bg(Color::Indexed(9))
        .bold()
}

fn lines() -> Vec<Line> {
    vec![
        Line::raw("plain"),
        Line::from(vec![
            Span::raw("a"),
            Span::styled("b", style_bold_red()),
            Span::styled("日本", Style::new().italic().underlined()),
        ]),
        Line::raw(""),
    ]
}

#[test]
fn text_lines_round_trip() {
    let v = View::Text(TextView {
        source: TextSource::Lines(lines()),
        styler: None,
        file_lines: true,
        syntax_name: Some("rust".into()),
    });
    let back = rt_ok(v);
    let View::Text(t) = &back else {
        panic!("{back:?}")
    };
    assert_eq!(t.syntax_name.as_deref(), Some("rust"));
    assert!(t.file_lines);
    let TextSource::Lines(l) = &t.source else {
        panic!()
    };
    assert_eq!(l, &lines());
}

#[test]
fn text_owned_and_file_round_trip() {
    let v = View::Text(TextView {
        source: TextSource::Owned("one\ntwo".into()),
        styler: None,
        file_lines: false,
        syntax_name: None,
    });
    let back = rt_ok(v);
    let View::Text(t) = &back else { panic!() };
    assert!(matches!(&t.source, TextSource::Owned(s) if s == "one\ntwo"));

    let v = View::Text(TextView {
        source: TextSource::File(PathBuf::from("/etc/hosts")),
        styler: None,
        file_lines: true,
        syntax_name: None,
    });
    let back = rt_ok(v);
    let View::Text(t) = &back else { panic!() };
    assert!(matches!(&t.source, TextSource::File(p) if p == Path::new("/etc/hosts")));
}

/// A styler is run in the child: the host receives the styled lines and no styler.
struct Upper;
impl LineStyler for Upper {
    fn style_block(&mut self, first_line: usize, text: &str) -> Vec<Line> {
        assert_eq!(first_line, 0);
        text.lines()
            .map(|l| Line::from(Span::styled(l.to_uppercase(), Style::new().bold())))
            .collect()
    }
}

struct NotReady;
impl LineStyler for NotReady {
    fn style_block(&mut self, _: usize, _: &str) -> Vec<Line> {
        panic!("a styler that is not ready must not be run");
    }
    fn ready(&self) -> bool {
        false
    }
}

#[test]
fn text_styler_runs_in_the_sender() {
    let v = View::Text(TextView {
        source: TextSource::Owned("ab\ncd".into()),
        styler: Some(Box::new(Upper)),
        file_lines: false,
        syntax_name: None,
    });
    let back = rt_ok(v);
    let View::Text(t) = &back else { panic!() };
    assert!(t.styler.is_none());
    let TextSource::Lines(l) = &t.source else {
        panic!("{:?}", t.source)
    };
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].plain(), "AB");
    assert_eq!(l[1].plain(), "CD");
}

#[test]
fn text_file_styler_runs_over_the_file_text() {
    let dir = unique_tmp("kp_styler");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let f = dir.as_path().join("a.txt");
    std::fs::write(&f, "xy\nzw").unwrap();
    let v = View::Text(TextView {
        source: TextSource::File(f),
        styler: Some(Box::new(Upper)),
        file_lines: true,
        syntax_name: None,
    });
    let back = rt_view(v, &side_in(dir.as_path())).unwrap();
    let View::Text(t) = &back else { panic!() };
    let TextSource::Lines(l) = &t.source else {
        panic!()
    };
    assert_eq!(l.iter().map(Line::plain).collect::<Vec<_>>(), ["XY", "ZW"]);
}

#[test]
fn text_styler_that_is_not_ready_is_not_run() {
    let v = View::Text(TextView {
        source: TextSource::Owned("ab".into()),
        styler: Some(Box::new(NotReady)),
        file_lines: false,
        syntax_name: None,
    });
    let back = rt_ok(v);
    let View::Text(t) = &back else { panic!() };
    assert!(matches!(&t.source, TextSource::Owned(s) if s == "ab"));
}

#[test]
fn a_child_view_with_a_relative_path_is_refused_when_sent_and_when_received() {
    let dir = unique_tmp("kp_rel");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    // Sending: the child's working directory means nothing to the host.
    let mut v = View::Text(TextView {
        source: TextSource::File(PathBuf::from("rel/x.txt")),
        styler: None,
        file_lines: false,
        syntax_name: None,
    });
    let e = view_to_wire(&mut v, &mut Enc::new(&side)).unwrap_err();
    assert!(
        matches!(&e, WireError::Bad(m) if m.contains("absolute")),
        "{e:?}"
    );
    let mut abs = View::Text(TextView {
        source: TextSource::File(PathBuf::from("/rel/x.txt")),
        styler: None,
        file_lines: false,
        syntax_name: None,
    });
    assert!(view_to_wire(&mut abs, &mut Enc::new(&side)).is_ok());

    // Receiving: the same rule, for a sender that did not go through ours.
    let w = WView::Text(WText {
        source: WTextSource::File(path_to_wire(Path::new("rel/x.txt"))),
        file_lines: false,
        syntax_name: None,
        note: None,
    });
    assert!(matches!(
        view_from_wire(w, &mut Dec::new(&side)),
        Err(WireError::Bad(_))
    ));
}

#[test]
fn every_other_path_a_child_sends_must_be_absolute_too() {
    let dir = unique_tmp("kp_rel2");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let rel = || PathBuf::from("rel/x.png");
    let bad: Vec<(&str, View)> = vec![
        (
            "image file",
            View::Image(ImageView::new(ImageData::File(rel()))),
        ),
        (
            "svg base dir",
            View::Image(ImageView::new(ImageData::Svg {
                data: Arc::from(b"<svg/>".to_vec()),
                base_dir: Some(PathBuf::from("rel")),
                trusted: false,
            })),
        ),
        (
            "frame file",
            View::Image(ImageView::new(ImageData::Frames(vec![(
                ImageData::File(rel()),
                Duration::from_millis(10),
            )]))),
        ),
    ];
    for (name, mut v) in bad {
        let r = view_to_wire(&mut v, &mut Enc::new(&side));
        assert!(matches!(r, Err(WireError::Bad(_))), "{name}: {r:?}");
    }
    // A list row's target, and the origin of a document.
    let mut list = View::List(ListView {
        rows: vec![ListRow {
            cells: vec![],
            kind: String::new(),
            target: Some(PathBuf::from("rel/t")),
            checked: None,
        }],
    });
    let r = view_to_wire(&mut list, &mut Enc::new(&side));
    assert!(matches!(r, Err(WireError::Bad(_))), "list target: {r:?}");
    let mut d = doc(Resources::default());
    let View::Document(dv) = &mut d else { panic!() };
    dv.origin = Origin::Converted {
        from: PathBuf::from("rel/b.docx"),
    };
    let r = view_to_wire(&mut d, &mut Enc::new(&side));
    assert!(
        matches!(r, Err(WireError::Bad(_))),
        "document origin: {r:?}"
    );
}

fn doc(resources: Resources) -> View {
    View::Document(DocumentView {
        markdown: "# T\n\n![x](a.png)".into(),
        origin: Origin::Converted {
            from: PathBuf::from("/a/b.docx"),
        },
        resources,
        options: DocOptions {
            details: "open".into(),
            task_states: vec![' ', 'x'],
            tasks_writable: true,
            table_align: "right".into(),
            ..DocOptions::default()
        },
        title: Some("Title".into()),
        truncated: true,
    })
}

#[test]
fn document_round_trip_with_resources() {
    let mut res = Resources::new();
    res.insert("a.png", Resource::Bytes(Arc::from(vec![1u8, 2, 3])));
    res.insert("b.bin", Resource::Bytes(Arc::from(vec![])));
    let back = rt_ok(doc(res));
    let View::Document(d) = &back else { panic!() };
    assert_eq!(d.markdown, "# T\n\n![x](a.png)");
    assert_eq!(
        d.origin,
        Origin::Converted {
            from: PathBuf::from("/a/b.docx")
        }
    );
    assert_eq!(d.title.as_deref(), Some("Title"));
    assert!(d.truncated);
    assert_eq!(d.options.details, "open");
    assert_eq!(d.options.task_states, vec![' ', 'x']);
    assert!(d.options.tasks_writable);
    assert_eq!(d.options.table_align, "right");
    assert_eq!(d.resources.len(), 2);
    assert!(matches!(d.resources.get("a.png"), Some(Resource::Bytes(b)) if **b == [1, 2, 3]));
}

#[test]
fn document_origin_file_round_trip_and_relative_origin_refused() {
    let v = View::Document(DocumentView {
        markdown: String::new(),
        origin: Origin::File(PathBuf::from("/x/y.md")),
        resources: Resources::new(),
        options: DocOptions::default(),
        title: None,
        truncated: false,
    });
    let back = rt_ok(v);
    let View::Document(d) = &back else { panic!() };
    assert_eq!(d.origin, Origin::File(PathBuf::from("/x/y.md")));

    let dir = unique_tmp("kp_origin");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    for origin in [
        WOrigin::File(path_to_wire(Path::new("y.md"))),
        WOrigin::Converted {
            from: path_to_wire(Path::new("../y.docx")),
        },
    ] {
        let mut w = match view_to_wire(&mut doc(Resources::new()), &mut Enc::new(&side)).unwrap() {
            WView::Document(d) => d,
            _ => unreachable!(),
        };
        w.origin = origin;
        assert!(matches!(
            view_from_wire(WView::Document(w), &mut Dec::new(&side)),
            Err(WireError::Bad(_))
        ));
    }
}

#[test]
fn lazy_resources_are_dropped_not_sent() {
    #[derive(Debug)]
    struct L;
    impl LazyResource for L {
        fn natural_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn render(&self, _: &ResourceReq, _: &Cancel) -> Result<ImageData, Failure> {
            unreachable!()
        }
    }
    let mut res = Resources::new();
    res.insert("lazy", Resource::Lazy(Arc::new(L)));
    res.insert("bytes", Resource::Bytes(Arc::from(vec![9u8])));
    let back = rt_ok(doc(res));
    let View::Document(d) = &back else { panic!() };
    assert!(d.resources.get("lazy").is_none());
    assert!(d.resources.get("bytes").is_some());
}

#[test]
fn big_resources_go_through_side_files_and_the_files_are_deleted() {
    let dir = unique_tmp("kp_side_big");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = Side::new(dir.as_path(), 'x').with_threshold(16);
    let big = vec![7u8; 100];
    let mut res = Resources::new();
    res.insert("big", Resource::Bytes(Arc::from(big.clone())));
    res.insert("small", Resource::Bytes(Arc::from(vec![1u8; 4])));
    let mut v = doc(res);
    let w = view_to_wire(&mut v, &mut Enc::new(&side)).unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.as_path()).unwrap().collect();
    assert_eq!(files.len(), 1, "only the big one goes to a file");
    let back = view_from_wire(w, &mut Dec::new(&side)).unwrap();
    let View::Document(d) = &back else { panic!() };
    assert!(matches!(d.resources.get("big"), Some(Resource::Bytes(b)) if **b == *big));
    assert_eq!(
        std::fs::read_dir(dir.as_path()).unwrap().count(),
        0,
        "read files are deleted"
    );
}

#[test]
fn table_round_trip_keeps_every_cell() {
    struct G;
    impl Grid for G {
        fn nrows(&self) -> usize {
            3
        }
        fn ncols(&self) -> usize {
            4
        }
        fn header(&self, col: usize) -> Option<String> {
            (col != 2).then(|| format!("H{col}"))
        }
        fn cell(&self, row: usize, col: usize) -> String {
            format!("{row}:{col}")
        }
        fn row_len(&self, row: usize) -> usize {
            if row == 1 {
                2
            } else {
                4
            }
        }
        fn truncated(&self) -> bool {
            true
        }
    }
    let v = View::Table(TableView {
        grid: Arc::new(G),
        title: Some("Sheet1".into()),
        rainbow: true,
        addressed: true,
    });
    let back = rt_ok(v);
    let View::Table(t) = &back else { panic!() };
    assert_eq!(t.title.as_deref(), Some("Sheet1"));
    assert!(t.rainbow && t.addressed);
    let g = &t.grid;
    assert_eq!((g.nrows(), g.ncols()), (3, 4));
    assert!(g.truncated());
    assert_eq!(g.header(0).as_deref(), Some("H0"));
    assert_eq!(g.header(2), None);
    assert_eq!(g.header(3).as_deref(), Some("H3"));
    assert_eq!(g.row_len(1), 2);
    assert_eq!(g.row_len(0), 4);
    for r in 0..3 {
        for c in 0..g.row_len(r) {
            assert_eq!(g.cell(r, c), format!("{r}:{c}"));
            assert_eq!(g.cell_str(r, c), Some(format!("{r}:{c}").as_str()));
        }
    }
    let mut seen = Vec::new();
    g.visit_row(1, &mut |c, t| seen.push((c, t.to_string())));
    assert_eq!(seen, [(0, "1:0".to_string()), (1, "1:1".to_string())]);
}

/// A sparse grid (visit_row visits few cells) is sent with the cells it has and empty strings between.
#[test]
fn sparse_table_keeps_what_visit_row_reports() {
    struct Sparse;
    impl Grid for Sparse {
        fn nrows(&self) -> usize {
            1
        }
        fn ncols(&self) -> usize {
            5
        }
        fn header(&self, _: usize) -> Option<String> {
            None
        }
        fn cell(&self, _: usize, col: usize) -> String {
            if col == 3 {
                "x".into()
            } else {
                String::new()
            }
        }
        fn visit_row(&self, _: usize, f: &mut dyn FnMut(usize, &str)) {
            f(3, "x");
        }
    }
    let back = rt_ok(View::Table(TableView {
        grid: Arc::new(Sparse),
        title: None,
        rainbow: false,
        addressed: false,
    }));
    let View::Table(t) = &back else { panic!() };
    assert_eq!(t.grid.cell(0, 3), "x");
    assert_eq!(t.grid.cell(0, 0), "");
}

fn all_images() -> Vec<ImageData> {
    vec![
        ImageData::File(PathBuf::from("/tmp/a.png")),
        ImageData::Encoded(Arc::from(vec![1u8, 2, 3, 4])),
        ImageData::Svg {
            data: Arc::from(b"<svg/>".to_vec()),
            base_dir: Some(PathBuf::from("/d")),
            trusted: true,
        },
        ImageData::Svg {
            data: Arc::from(b"<svg/>".to_vec()),
            base_dir: None,
            trusted: false,
        },
        ImageData::Rgba {
            width: 2,
            height: 1,
            pixels: Arc::new(vec![1, 2, 3, 4, 5, 6, 7, 8]),
        },
        ImageData::Frames(vec![
            (
                ImageData::File(PathBuf::from("/tmp/f1.png")),
                Duration::from_millis(40),
            ),
            (
                ImageData::Rgba {
                    width: 1,
                    height: 1,
                    pixels: Arc::new(vec![0, 0, 0, 255]),
                },
                Duration::from_micros(1234),
            ),
        ]),
    ]
}

#[test]
fn every_image_variant_round_trips() {
    for img in all_images() {
        let mut view = ImageView::new(img.clone());
        view.sharp_zoom = true;
        view.raster_max_px = Some(2048);
        let back = rt_ok(View::Image(view));
        let View::Image(i) = &back else { panic!() };
        assert!(i.sharp_zoom);
        assert_eq!(i.raster_max_px, Some(2048));
        // A picture from a child is never trusted, whatever it claimed (see the next test).
        let expected = match img {
            ImageData::Svg { data, base_dir, .. } => ImageData::Svg {
                data,
                base_dir,
                trusted: false,
            },
            other => other,
        };
        assert_eq!(format!("{:?}", i.data), format!("{expected:?}"));
    }
}

#[test]
fn an_svg_that_claims_to_be_trusted_arrives_untrusted() {
    let img = ImageData::Svg {
        data: Arc::from(b"<svg/>".to_vec()),
        base_dir: None,
        trusted: true,
    };
    let back = rt_ok(View::Image(ImageView::new(img)));
    let View::Image(i) = &back else { panic!() };
    assert!(
        matches!(&i.data, ImageData::Svg { trusted: false, .. }),
        "{:?}",
        i.data
    );

    // Also when the wire itself says trusted (a hostile child does not go through our sender).
    let dir = unique_tmp("kp_svgtrust");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let mut v = View::Image(ImageView::new(ImageData::Svg {
        data: Arc::from(b"<svg/>".to_vec()),
        base_dir: None,
        trusted: false,
    }));
    let w = view_to_wire(&mut v, &mut Enc::new(&side)).unwrap();
    let mut text = format!("{w:?}");
    assert!(text.contains("trusted: false"), "{text}");
    text.clear();
    let WView::Image(wi) = w else { panic!() };
    let WImage::One(WLeaf::Svg { data, base_dir, .. }) = wi.data else {
        panic!()
    };
    let hostile = WView::Image(WImageView {
        data: WImage::One(WLeaf::Svg {
            data,
            base_dir,
            trusted: true,
        }),
        sharp_zoom: false,
        raster_max_px: None,
    });
    let View::Image(i) = view_from_wire(hostile, &mut Dec::new(&side)).unwrap() else {
        panic!()
    };
    assert!(matches!(&i.data, ImageData::Svg { trusted: false, .. }));
}

#[test]
fn a_lazy_image_is_refused_on_send() {
    #[derive(Debug)]
    struct L;
    impl LazyResource for L {
        fn natural_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn render(&self, _: &ResourceReq, _: &Cancel) -> Result<ImageData, Failure> {
            unreachable!()
        }
    }
    let dir = unique_tmp("kp_lazy");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut v = View::Image(ImageView::new(ImageData::Lazy(Arc::new(L))));
    assert!(view_to_wire(&mut v, &mut Enc::new(&side_in(dir.as_path()))).is_err());
    let mut v = View::Image(ImageView::new(ImageData::Frames(vec![(
        ImageData::Lazy(Arc::new(L)),
        Duration::ZERO,
    )])));
    assert!(view_to_wire(&mut v, &mut Enc::new(&side_in(dir.as_path()))).is_err());
}

#[test]
fn an_animation_inside_an_animation_is_refused() {
    let dir = unique_tmp("kp_nest");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let inner = ImageData::Frames(vec![]);
    let mut v = View::Image(ImageView::new(ImageData::Frames(vec![(
        inner,
        Duration::ZERO,
    )])));
    assert!(view_to_wire(&mut v, &mut Enc::new(&side_in(dir.as_path()))).is_err());
}

#[test]
fn relative_image_paths_from_the_wire_are_refused() {
    let dir = unique_tmp("kp_relimg");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    for leaf in [
        WLeaf::File(path_to_wire(Path::new("a.png"))),
        WLeaf::File(path_to_wire(Path::new("./a.png"))),
        WLeaf::File(path_to_wire(Path::new(""))),
        WLeaf::Svg {
            data: WBin::Inline(b"<svg/>".to_vec()),
            base_dir: Some(path_to_wire(Path::new("d"))),
            trusted: false,
        },
    ] {
        let w = WView::Image(WImageView {
            data: WImage::One(leaf.clone()),
            sharp_zoom: false,
            raster_max_px: None,
        });
        assert!(
            matches!(
                view_from_wire(w, &mut Dec::new(&side)),
                Err(WireError::Bad(_))
            ),
            "{leaf:?}"
        );
        // Inside an animation too.
        let w = WView::Image(WImageView {
            data: WImage::Frames(vec![(leaf.clone(), 5)]),
            sharp_zoom: false,
            raster_max_px: None,
        });
        assert!(view_from_wire(w, &mut Dec::new(&side)).is_err(), "{leaf:?}");
    }
}

#[test]
fn rgba_with_a_wrong_pixel_count_is_refused() {
    let dir = unique_tmp("kp_rgba");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    for (w, h, n) in [
        (2, 2, 15),
        (2, 2, 17),
        (0, 0, 0),
        (0, 5, 0),
        (u32::MAX, u32::MAX, 4),
    ] {
        let v = WView::Image(WImageView {
            data: WImage::One(WLeaf::Rgba {
                width: w,
                height: h,
                pixels: WBin::Inline(vec![0; n]),
            }),
            sharp_zoom: false,
            raster_max_px: None,
        });
        assert!(
            matches!(
                view_from_wire(v, &mut Dec::new(&side)),
                Err(WireError::Bad(_))
            ),
            "{w}x{h} with {n} bytes"
        );
    }
}

#[test]
fn list_diff_message_reopen_round_trip() {
    let list = View::List(ListView {
        rows: vec![
            ListRow {
                cells: lines(),
                kind: "file".into(),
                target: Some(PathBuf::from("/a/b")),
                checked: Some(true),
            },
            ListRow {
                cells: vec![],
                kind: String::new(),
                target: None,
                checked: None,
            },
        ],
    });
    let s = dbg(&list);
    assert_eq!(dbg(&rt_ok(list)), s);

    let diff = View::Diff(DiffView {
        hunks: vec![
            DiffHunk {
                lines: vec![
                    DiffLine {
                        kind: DiffKind::Context,
                        old_no: Some(1),
                        new_no: Some(1),
                        text: "a".into(),
                    },
                    DiffLine {
                        kind: DiffKind::Added,
                        old_no: None,
                        new_no: Some(2),
                        text: "b".into(),
                    },
                    DiffLine {
                        kind: DiffKind::Removed,
                        old_no: Some(2),
                        new_no: None,
                        text: "c".into(),
                    },
                ],
            },
            DiffHunk { lines: vec![] },
        ],
    });
    let s = dbg(&diff);
    assert_eq!(dbg(&rt_ok(diff)), s);

    let msg = View::Message(MessageView { lines: lines() });
    let s = dbg(&msg);
    assert_eq!(dbg(&rt_ok(msg)), s);

    let mut res = Resources::new();
    res.insert("r", Resource::Bytes(Arc::from(vec![5u8])));
    let re = View::Reopen(Reopen {
        content_type: "text/markdown".into(),
        text: "# x".into(),
        resources: res,
        origin: Origin::File(PathBuf::from("/o")),
        truncated: true,
    });
    let s = dbg(&re);
    assert_eq!(dbg(&rt_ok(re)), s);
}

fn rich_canvas() -> CanvasView {
    let mut c = CanvasView::new(12, 4);
    c.set_string(0, 0, "hello", style_bold_red());
    c.set_string(0, 1, "日本語", Style::new().fg(Color::Green));
    c.set_string(0, 2, "e\u{301}x", Style::new());
    // Two regional indicators in neighbouring cells with the same style must stay two cells.
    if let Some(cell) = c.cell_mut(8, 3) {
        *cell = CanvasCell::new("\u{1F1EF}", Style::new());
    }
    if let Some(cell) = c.cell_mut(9, 3) {
        *cell = CanvasCell::new("\u{1F1F5}", Style::new());
    }
    let c = c.redraw_after(Duration::from_micros(1500)).text_input(true);
    c.check().expect("the fixture is a valid canvas");
    c
}

#[test]
fn canvas_round_trip_cell_for_cell() {
    let c = rich_canvas();
    let back = rt_ok(View::Canvas(c.clone()));
    let View::Canvas(b) = &back else { panic!() };
    assert_eq!((b.cols, b.rows), (12, 4));
    assert_eq!(b.cells, c.cells);
    assert!(b.text_input);
    // 1500 us is sent as 2 ms (rounded up) so a deadline is never made earlier.
    assert_eq!(b.redraw_after, Some(Duration::from_millis(2)));
}

#[test]
fn canvas_wide_character_followed_by_a_different_style_continuation() {
    let mut c = CanvasView::new(4, 1);
    c.set_string(0, 0, "日", Style::new().bold());
    // The continuation cell may carry its own style.
    if let Some(cell) = c.cell_mut(1, 0) {
        cell.style = Style::new().fg(Color::Red);
    }
    c.check().unwrap();
    let back = rt_ok(View::Canvas(c.clone()));
    let View::Canvas(b) = &back else { panic!() };
    assert_eq!(b.cells, c.cells);
}

#[test]
fn canvas_with_images_round_trips_and_keeps_unchanged_pictures_unchanged() {
    let c = CanvasView::new(10, 5)
        .image(CanvasImage::new(
            1,
            CellRect::new(0, 0, 4, 2),
            ImageData::Rgba {
                width: 1,
                height: 1,
                pixels: Arc::new(vec![1, 2, 3, 4]),
            },
        ))
        .image(CanvasImage::unchanged(2, CellRect::new(4, 2, 3, 3)))
        .image(CanvasImage::new(
            3,
            CellRect::new(0, 3, 2, 2),
            ImageData::Frames(vec![(
                ImageData::File(PathBuf::from("/f.png")),
                Duration::from_millis(30),
            )]),
        ));
    c.check().unwrap();
    let back = rt_ok(View::Canvas(c.clone()));
    let View::Canvas(b) = &back else { panic!() };
    assert_eq!(b.images.len(), 3);
    assert!(b.images[1].image.is_none());
    assert_eq!(format!("{:?}", b.images), format!("{:?}", c.images));
}

#[test]
fn canvas_redraw_after_is_milliseconds_and_saturates() {
    for (d, want) in [
        (Duration::ZERO, 0u64),
        (Duration::from_millis(250), 250),
        (Duration::from_nanos(1), 1),
        (Duration::from_secs(10_000_000), u32::MAX as u64),
    ] {
        let c = CanvasView::new(1, 1).redraw_after(d);
        let back = rt_ok(View::Canvas(c));
        let View::Canvas(b) = &back else { panic!() };
        assert_eq!(b.redraw_after, Some(Duration::from_millis(want)), "{d:?}");
    }
    let back = rt_ok(View::Canvas(CanvasView::new(1, 1)));
    let View::Canvas(b) = &back else { panic!() };
    assert_eq!(b.redraw_after, None);
}

#[test]
fn an_empty_canvas_round_trips() {
    for (cols, rows) in [(0, 0), (0, 3), (3, 0), (1, 1)] {
        let c = CanvasView::new(cols, rows);
        let back = rt_ok(View::Canvas(c.clone()));
        let View::Canvas(b) = &back else { panic!() };
        assert_eq!(b.cells, c.cells);
    }
}

/// A canvas that fails its own check is not sent by an honest sender.
#[test]
fn a_canvas_that_fails_check_is_refused_on_send() {
    let dir = unique_tmp("kp_badsend");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut c = CanvasView::new(3, 1);
    c.cells[0].symbol = "\u{7}".into();
    assert!(view_to_wire(&mut View::Canvas(c), &mut Enc::new(&side_in(dir.as_path()))).is_err());
}

fn canvas_wire(c: WCanvas) -> WView {
    WView::Canvas(c)
}

fn run(style: &WStyle, text: &str) -> (WStyle, String) {
    (*style, text.to_string())
}

fn plain_style() -> WStyle {
    style_to_wire(&Style::new())
}

fn try_canvas(c: WCanvas) -> Result<View, WireError> {
    let dir = unique_tmp("kp_wcanvas");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    view_from_wire(canvas_wire(c), &mut Dec::new(&side))
}

fn canvas_of(cols: u16, rows: u16, runs: Vec<Vec<(WStyle, String)>>) -> WCanvas {
    // The styles go into a table, the runs refer to them by number, as `canvas_to_wire` does.
    let mut table: Vec<WStyle> = Vec::new();
    let runs = runs
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(st, text)| {
                    let i = table.iter().position(|t| *t == st).unwrap_or_else(|| {
                        table.push(st);
                        table.len() - 1
                    });
                    (i as u32, text)
                })
                .collect()
        })
        .collect();
    WCanvas {
        cols,
        rows,
        styles: WBin::Inline(pack_styles(&table)),
        runs,
        images: vec![],
        redraw_after_ms: None,
        text_input: false,
    }
}

#[test]
fn hostile_canvases_are_refused() {
    let p = plain_style();
    // Sanity: the well-formed one passes.
    assert!(try_canvas(canvas_of(3, 1, vec![vec![run(&p, "abc")]])).is_ok());
    let bad: Vec<(&str, WCanvas)> = vec![
        ("row too short", canvas_of(3, 1, vec![vec![run(&p, "ab")]])),
        ("row too long", canvas_of(3, 1, vec![vec![run(&p, "abcd")]])),
        ("too few rows", canvas_of(3, 2, vec![vec![run(&p, "abc")]])),
        (
            "too many rows",
            canvas_of(3, 1, vec![vec![run(&p, "abc")], vec![run(&p, "abc")]]),
        ),
        ("huge side", canvas_of(u16::MAX, 1, vec![vec![]])),
        ("huge area", canvas_of(4096, 4096, vec![])),
        (
            "control char",
            canvas_of(3, 1, vec![vec![run(&p, "a\u{7}c")]]),
        ),
        (
            "stray continuation",
            canvas_of(3, 1, vec![vec![run(&p, "ab\u{0}")]]),
        ),
        (
            "leading continuation",
            canvas_of(3, 1, vec![vec![run(&p, "\u{0}bc")]]),
        ),
        (
            "wide char at the edge",
            canvas_of(3, 1, vec![vec![run(&p, "ab日")]]),
        ),
        (
            "wide char without continuation",
            canvas_of(3, 1, vec![vec![run(&p, "日bc")]]),
        ),
        (
            "zero-width symbol",
            canvas_of(1, 1, vec![vec![run(&p, "\u{200B}")]]),
        ),
        (
            "image outside",
            WCanvas {
                images: vec![WCanvasImage {
                    id: 1,
                    x: 2,
                    y: 0,
                    cols: 5,
                    rows: 1,
                    image: None,
                }],
                ..canvas_of(3, 1, vec![vec![run(&p, "abc")]])
            },
        ),
        (
            "relative picture",
            WCanvas {
                images: vec![WCanvasImage {
                    id: 1,
                    x: 0,
                    y: 0,
                    cols: 1,
                    rows: 1,
                    image: Some(WImage::One(WLeaf::File(path_to_wire(Path::new("a.png"))))),
                }],
                ..canvas_of(3, 1, vec![vec![run(&p, "abc")]])
            },
        ),
    ];
    for (name, c) in bad {
        assert!(try_canvas(c).is_err(), "{name} must be refused");
    }
}

#[test]
fn a_canvas_with_a_huge_declared_size_allocates_nothing() {
    // 4096 x 1024 is within the limits but no row data is sent: refused by the row count
    // before any cell is built.
    let c = canvas_of(4096, 1024, vec![]);
    assert!(matches!(try_canvas(c), Err(WireError::Bad(_))));
}

#[test]
fn too_many_canvas_pictures_or_frames_are_refused() {
    let p = plain_style();
    let images = (0..300u64)
        .map(|id| WCanvasImage {
            id,
            x: 0,
            y: 0,
            cols: 1,
            rows: 1,
            image: None,
        })
        .collect();
    let c = WCanvas {
        images,
        ..canvas_of(1, 1, vec![vec![run(&p, "a")]])
    };
    assert!(try_canvas(c).is_err());

    let frames = (0..5001)
        .map(|_| {
            (
                WLeaf::Rgba {
                    width: 1,
                    height: 1,
                    pixels: WBin::Inline(vec![0; 4]),
                },
                1,
            )
        })
        .collect();
    let v = WView::Image(WImageView {
        data: WImage::Frames(frames),
        sharp_zoom: false,
        raster_max_px: None,
    });
    let dir = unique_tmp("kp_frames");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    assert!(matches!(
        view_from_wire(v, &mut Dec::new(&side_in(dir.as_path()))),
        Err(WireError::TooLarge(_))
    ));
}

#[test]
fn tables_from_the_wire_are_bounded() {
    let mk = |ncols: u64, header: usize, rows: Vec<Vec<String>>| {
        WView::Table(WTable {
            title: None,
            rainbow: false,
            addressed: false,
            ncols,
            header: vec![None; header],
            truncated: false,
            rows,
        })
    };
    let dir = unique_tmp("kp_tbl");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let go = |v| view_from_wire(v, &mut Dec::new(&side));
    assert!(go(mk(2, 2, vec![vec!["a".into(), "b".into()]])).is_ok());
    assert!(matches!(
        go(mk(1, 0, vec![vec!["a".into(), "b".into()]])),
        Err(WireError::Bad(_))
    ));
    assert!(matches!(go(mk(1, 2, vec![])), Err(WireError::Bad(_))));
    assert!(go(mk(u64::MAX, 0, vec![])).is_err());
    assert!(matches!(
        go(mk(70_000, 0, vec![])),
        Err(WireError::TooLarge(_))
    ));
    // Over the cell cap: counted with a minimum of one cell per row.
    let rows = vec![Vec::new(); TABLE_MAX_CELLS + 1];
    assert!(matches!(go(mk(1, 0, rows)), Err(WireError::TooLarge(_))));
}

#[test]
fn a_table_over_the_cap_is_refused_on_send() {
    struct Big;
    impl Grid for Big {
        fn nrows(&self) -> usize {
            TABLE_MAX_CELLS + 1
        }
        fn ncols(&self) -> usize {
            1
        }
        fn header(&self, _: usize) -> Option<String> {
            None
        }
        fn cell(&self, _: usize, _: usize) -> String {
            String::new()
        }
        fn visit_row(&self, _: usize, _: &mut dyn FnMut(usize, &str)) {}
    }
    let dir = unique_tmp("kp_bigtbl");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut v = View::Table(TableView {
        grid: Arc::new(Big),
        title: None,
        rainbow: false,
        addressed: false,
    });
    assert!(matches!(
        view_to_wire(&mut v, &mut Enc::new(&side_in(dir.as_path()))),
        Err(WireError::TooLarge(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// Side files
// ---------------------------------------------------------------------------------------------

fn bin_view(name: &str, len: u64) -> WView {
    WView::Image(WImageView {
        data: WImage::One(WLeaf::Encoded(WBin::File {
            name: name.into(),
            len,
        })),
        sharp_zoom: false,
        raster_max_px: None,
    })
}

#[test]
fn side_file_checks() {
    use std::os::unix::fs::symlink;
    let dir = unique_tmp("kp_sidechk");
    let d = dir.as_path();
    std::fs::create_dir_all(d).unwrap();
    let side = Side::new(d, 'x');
    let go = |name: &str, len: u64| view_from_wire(bin_view(name, len), &mut Dec::new(&side));

    std::fs::write(d.join("ok"), b"12345").unwrap();
    assert!(go("ok", 5).is_ok());
    assert!(!d.join("ok").exists(), "deleted after reading");

    std::fs::write(d.join("short"), b"123").unwrap();
    assert!(
        matches!(go("short", 5), Err(WireError::Bad(_))),
        "announced length differs"
    );
    std::fs::write(d.join("long"), b"123456").unwrap();
    assert!(matches!(go("long", 5), Err(WireError::Bad(_))));

    assert!(matches!(go("missing", 1), Err(WireError::Bad(_))));
    for name in [
        "",
        ".",
        "..",
        "a/b",
        "../x",
        "/etc/hosts",
        "a\0b",
        &"n".repeat(65),
    ] {
        assert!(matches!(go(name, 1), Err(WireError::Bad(_))), "{name:?}");
    }

    std::fs::create_dir(d.join("dir")).unwrap();
    assert!(
        matches!(go("dir", 0), Err(WireError::Bad(_))),
        "a directory"
    );

    let outside = unique_tmp("kp_sideout");
    std::fs::create_dir_all(outside.as_path()).unwrap();
    std::fs::write(outside.as_path().join("secret"), b"12345").unwrap();
    symlink(outside.as_path().join("secret"), d.join("link")).unwrap();
    assert!(
        matches!(go("link", 5), Err(WireError::Bad(_))),
        "a symlink is not followed"
    );
    assert!(outside.as_path().join("secret").exists());

    assert!(matches!(
        go("ok2", SIDE_FILE_MAX as u64 + 1),
        Err(WireError::TooLarge(_))
    ));
}

#[test]
fn side_files_total_is_bounded_per_message() {
    let dir = unique_tmp("kp_sidetotal");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = Side::new(dir.as_path(), 'x');
    let leaves = (0..3)
        .map(|i| {
            WLeaf::Encoded(WBin::File {
                name: format!("f{i}"),
                len: SIDE_FILE_MAX as u64,
            })
        })
        .map(|l| (l, 1))
        .collect();
    let v = WView::Image(WImageView {
        data: WImage::Frames(leaves),
        sharp_zoom: false,
        raster_max_px: None,
    });
    // 3 x 256 MiB announced is over the 512 MiB total. (The files do not exist, so this also
    // fails on the first one; the point is that nothing is read and nothing hangs.)
    assert!(view_from_wire(v, &mut Dec::new(&side)).is_err());
}

// ---------------------------------------------------------------------------------------------
// Input, key, request, outcome, failure, layout, cx
// ---------------------------------------------------------------------------------------------

fn rt<T, W>(x: &T, to: impl Fn(&T) -> W, from: impl Fn(W) -> Result<T, WireError>) -> T
where
    W: serde::Serialize + serde::de::DeserializeOwned,
{
    let w = to(x);
    let bytes = frame::encode(&w).unwrap();
    let w2: W = frame::decode(&bytes).unwrap();
    from(w2).unwrap()
}

#[test]
fn every_key_kind_round_trips() {
    let codes = vec![
        KeyCode::Char('a'),
        KeyCode::Char('日'),
        KeyCode::Char('\u{1F600}'),
        KeyCode::Enter,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Esc,
        KeyCode::F(0),
        KeyCode::F(12),
        KeyCode::F(255),
        KeyCode::Paste("multi\nline 日本".into()),
        KeyCode::Paste(String::new()),
    ];
    let mods = [
        KeyMods::NONE,
        KeyMods::CTRL,
        KeyMods::ALT,
        KeyMods::SHIFT,
        KeyMods::CTRL | KeyMods::ALT | KeyMods::SHIFT,
    ];
    for code in &codes {
        for m in mods {
            let k = Key::new(code.clone()).with_mods(m);
            let back = rt(&k, key_to_wire, |w| Ok(key_from_wire(w)));
            assert_eq!(back, k);
        }
    }
}

#[test]
fn key_mods_from_the_wire_drop_unknown_bits() {
    let w = WKey {
        code: WKeyCode::Char('x'),
        mods: 0b1111_1111,
    };
    let k = key_from_wire(w);
    assert_eq!(k.mods, KeyMods::CTRL | KeyMods::ALT | KeyMods::SHIFT);
    assert_eq!(KeyMods::from_bits_truncate(0b1111_1000), KeyMods::NONE);
    assert_eq!(KeyMods::from_bits_truncate(2), KeyMods::ALT);
}

#[test]
fn style_bits_from_the_wire_are_truncated() {
    assert_eq!(Modifier::from_bits_truncate(0xFFFF).bits(), (1 << 9) - 1);
    let w = WStyle {
        fg: None,
        bg: None,
        add: 0xFFFF,
        sub: 0x8000,
    };
    let s = style_from_wire(w);
    assert_eq!(s.add, Modifier::from_bits_truncate(0xFFFF));
    assert_eq!(s.sub.bits(), 0);
}

#[test]
fn every_color_and_modifier_round_trips() {
    let colors = [
        Color::Reset,
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
        Color::Indexed(0),
        Color::Indexed(255),
        Color::Rgb(0, 0, 0),
        Color::Rgb(255, 128, 1),
    ];
    for c in colors {
        let s = Style::from_parts(
            Some(c),
            Some(c),
            Modifier::from_bits_truncate(0x1FF),
            Modifier::BOLD,
        );
        assert_eq!(rt(&s, style_to_wire, |w| Ok(style_from_wire(w))), s);
    }
    let none = Style::new();
    assert_eq!(rt(&none, style_to_wire, |w| Ok(style_from_wire(w))), none);
}

#[test]
fn input_round_trips() {
    let dir = unique_tmp("kp_input");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = Side::new(dir.as_path(), 'x').with_threshold(8);
    let mut res = Resources::new();
    res.insert("small", Resource::Bytes(Arc::from(vec![1u8, 2])));
    res.insert("large", Resource::Bytes(Arc::from(vec![3u8; 64])));
    let inputs = vec![
        Input::File {
            path: PathBuf::from("/a/b.txt"),
            hint: Some("tsv".into()),
        },
        Input::File {
            path: PathBuf::from("/a/c.txt"),
            hint: None,
        },
        Input::Content {
            content_type: "text/markdown".into(),
            text: "# x".into(),
            resources: res,
            origin: Origin::Converted {
                from: PathBuf::from("/o.docx"),
            },
        },
        Input::Embed {
            kind: "math".into(),
            source: "x^2".into(),
            display: true,
        },
        Input::Embed {
            kind: "mermaid".into(),
            source: String::new(),
            display: false,
        },
    ];
    for i in inputs {
        let w = input_to_wire(&i, &mut Enc::new(&side)).unwrap();
        let bytes = frame::encode(&w).unwrap();
        let back = input_from_wire(frame::decode(&bytes).unwrap(), &mut Dec::new(&side)).unwrap();
        assert_eq!(format!("{back:?}"), format!("{i:?}"));
    }
}

#[test]
fn a_file_input_is_made_absolute_and_a_relative_one_from_the_wire_is_refused() {
    let dir = unique_tmp("kp_inabs");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let w = input_to_wire(
        &Input::File {
            path: PathBuf::from("rel/a.txt"),
            hint: None,
        },
        &mut Enc::new(&side),
    )
    .unwrap();
    let back = input_from_wire(w, &mut Dec::new(&side)).unwrap();
    let Input::File { path, .. } = back else {
        panic!()
    };
    assert!(path.is_absolute() && path.ends_with("rel/a.txt"));

    let w = WInput::File {
        path: path_to_wire(Path::new("rel/a.txt")),
        hint: None,
    };
    assert!(matches!(
        input_from_wire(w, &mut Dec::new(&side)),
        Err(WireError::Bad(_))
    ));
    let w = WInput::Content {
        content_type: "t".into(),
        text: String::new(),
        resources: vec![],
        origin: WOrigin::File(path_to_wire(Path::new("o"))),
    };
    assert!(input_from_wire(w, &mut Dec::new(&side)).is_err());
}

#[test]
fn lazy_resources_in_an_input_are_not_sent() {
    #[derive(Debug)]
    struct L;
    impl LazyResource for L {
        fn natural_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn render(&self, _: &ResourceReq, _: &Cancel) -> Result<ImageData, Failure> {
            unreachable!()
        }
    }
    let dir = unique_tmp("kp_inlazy");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let mut res = Resources::new();
    res.insert("lazy", Resource::Lazy(Arc::new(L)));
    let i = Input::Content {
        content_type: "t".into(),
        text: String::new(),
        resources: res,
        origin: Origin::File(PathBuf::from("/o")),
    };
    let w = input_to_wire(&i, &mut Enc::new(&side)).unwrap();
    let back = input_from_wire(w, &mut Dec::new(&side)).unwrap();
    let Input::Content { resources, .. } = back else {
        panic!()
    };
    assert!(resources.is_empty());
}

#[test]
fn view_requests_round_trip() {
    let reqs = vec![
        ViewReq::new(80, 24),
        ViewReq::new(0, 0).repr("source").page(3).cell_px(9, 18),
        ViewReq::new(u16::MAX, u16::MAX).page(usize::MAX >> 1),
    ];
    for r in reqs {
        let back = rt(&r, req_to_wire, req_from_wire);
        assert_eq!(back, r);
    }
}

#[test]
fn outcomes_round_trip() {
    for o in [
        Outcome::Unhandled,
        Outcome::Redraw,
        Outcome::Reload,
        Outcome::Flash(Tr::new("saved").ja("保存しました")),
        Outcome::Flash(Tr::new("")),
    ] {
        assert_eq!(rt(&o, outcome_to_wire, |w| Ok(outcome_from_wire(w))), o);
    }
}

fn all_failures() -> Vec<Failure> {
    use FailureKind::*;
    [
        Unsupported,
        Encrypted,
        Corrupt,
        TooLarge,
        TooHeavy,
        Timeout,
        Cancelled,
        Io,
        External,
    ]
    .into_iter()
    .flat_map(|k| {
        [
            Failure::new(k, Tr::new("m").ja("め")),
            Failure::new(k, Tr::new("m")).with_detail("d\nd2"),
        ]
    })
    .collect()
}

#[test]
fn every_failure_kind_round_trips() {
    for f in all_failures() {
        assert_eq!(rt(&f, failure_to_wire, |w| Ok(failure_from_wire(w))), f);
    }
}

#[test]
fn a_failure_from_the_wire_is_clipped() {
    let long = "x".repeat(10_000);
    let w = WFailure {
        kind: WFailureKind::External,
        message: WTr {
            en: long.clone(),
            others: vec![],
        },
        detail: Some(long.clone()),
    };
    let f = failure_from_wire(w);
    assert!(f.message.en().chars().count() <= konoma_plugin::text::MAX_TEXT_CHARS);
    assert!(f.detail.as_deref().unwrap().chars().count() <= konoma_plugin::text::MAX_TEXT_CHARS);
}

#[test]
fn layouts_round_trip() {
    let layouts = vec![
        Layout::single(),
        Layout {
            reprs: vec![
                ReprDecl::new("rendered", Tr::new("Rendered").ja("表示")),
                ReprDecl::new("source", Tr::new("Source")),
            ],
            default_repr: Some("rendered".into()),
            default_repr_without_pixels: Some("source".into()),
            pages: Some(PageLayout::Separate {
                count: Some(3),
                titles: vec!["a".into(), "b".into()],
            }),
        },
        Layout {
            pages: Some(PageLayout::Separate {
                count: None,
                titles: vec![],
            }),
            ..Layout::default()
        },
        Layout {
            pages: Some(PageLayout::Anchored {
                heading_level: 2,
                count: 10,
            }),
            ..Layout::default()
        },
    ];
    for l in layouts {
        assert_eq!(rt(&l, layout_to_wire, |w| Ok(layout_from_wire(w))), l);
    }
}

#[test]
fn a_layout_from_the_wire_is_clipped_to_the_host_limits() {
    let l = Layout {
        reprs: (0..200)
            .map(|i| ReprDecl::new(format!("r{i}"), Tr::new("x")))
            .collect(),
        ..Layout::default()
    };
    let back = layout_from_wire(layout_to_wire(&l));
    assert!(back.reprs.len() <= MAX_LAYOUT_REPRS);
}

fn cx_source<'a>(s: &'a Settings, scope: &'a ReadScope) -> CxSource<'a> {
    CxSource {
        settings: s,
        lang: "ja",
        term: TermCaps::new().pixels(true).colors(ColorDepth::TrueColor),
        external: true,
        read_scope: scope,
    }
}

#[test]
fn the_context_round_trips() {
    let dir = unique_tmp("kp_cx");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let mut table = std::collections::BTreeMap::new();
    table.insert(
        "k".to_string(),
        Value::List(vec![Value::Int(-3), Value::Float(1.5), Value::Bool(false)]),
    );
    let settings = Settings::new()
        .with("a", Value::Str("日本".into()))
        .with("b", Value::Int(i64::MIN))
        .with("t", Value::Table(table));
    for scope in [
        ReadScope::unrestricted(),
        ReadScope::within([dir.as_path()]),
    ] {
        for (pixels, colors) in [
            (false, ColorDepth::Ansi16),
            (true, ColorDepth::Ansi256),
            (true, ColorDepth::TrueColor),
        ] {
            let src = CxSource {
                term: TermCaps::new().pixels(pixels).colors(colors),
                ..cx_source(&settings, &scope)
            };
            let w = cx_to_wire(&src).unwrap();
            let bytes = frame::encode(&w).unwrap();
            let back = cx_from_wire(frame::decode(&bytes).unwrap()).unwrap();
            assert_eq!(back.settings, settings);
            assert_eq!(back.lang, "ja");
            assert_eq!(back.term, src.term);
            assert!(back.external);
            assert_eq!(back.read_scope.dirs(), scope.dirs());
            assert_eq!(back.read_scope.is_restricted(), scope.is_restricted());
        }
    }
}

#[test]
fn a_deeply_nested_setting_is_refused() {
    let mut v = Value::Int(1);
    for _ in 0..100 {
        v = Value::List(vec![v]);
    }
    let settings = Settings::new().with("deep", v);
    let scope = ReadScope::unrestricted();
    assert!(cx_to_wire(&cx_source(&settings, &scope)).is_err());

    let mut w = WValue::Int(1);
    for _ in 0..100 {
        w = WValue::List(vec![w]);
    }
    let wcx = WCx {
        settings: vec![("deep".into(), w)],
        lang: "en".into(),
        pixels: false,
        colors: 0,
        external: false,
        read_dirs: None,
    };
    assert!(matches!(cx_from_wire(wcx), Err(WireError::TooLarge(_))));
}

#[test]
fn unknown_color_depth_reads_as_the_lowest() {
    let wcx = WCx {
        settings: vec![],
        lang: "en".into(),
        pixels: true,
        colors: 77,
        external: false,
        read_dirs: Some(vec![path_to_wire(Path::new("/definitely/not/there"))]),
    };
    let c = cx_from_wire(wcx).unwrap();
    assert_eq!(c.term.colors, ColorDepth::Ansi16);
    // A scope whose directories do not exist allows nothing, but is still restricted.
    assert!(c.read_scope.is_restricted());
}

#[test]
fn paths_with_a_nul_byte_are_refused() {
    // msgpack bin8: 0xc4, length, bytes.
    let raw = [&[0xc4u8, 4][..], b"/a\0b"].concat();
    let w: WPath = frame::decode(&raw).unwrap();
    assert!(path_from_wire(w).is_err());
}

#[test]
fn paths_that_are_not_utf8_survive() {
    use std::os::unix::ffi::OsStrExt;
    let p = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe.png"));
    let w = path_to_wire(&p);
    let bytes = frame::encode(&w).unwrap();
    let back = path_from_wire(frame::decode(&bytes).unwrap()).unwrap();
    assert_eq!(back, p);
}

// ---------------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------------

#[test]
fn the_manifest_round_trips_through_bytes_and_json() {
    let m = full_manifest();
    let w = manifest_to_wire(&m).unwrap();
    let bytes = frame::encode(&w).unwrap();
    let back = manifest_from_wire(frame::decode(&bytes).unwrap()).unwrap();
    let a = konoma_plugin::proc::manifest_to_json(&m).unwrap();
    let b = konoma_plugin::proc::manifest_to_json(&back).unwrap();
    assert_eq!(a, b);
    let from_json = konoma_plugin::proc::manifest_from_json(&a).unwrap();
    assert_eq!(
        konoma_plugin::proc::manifest_to_json(&from_json).unwrap(),
        a
    );
    assert_eq!(back.id, m.id);
    assert_eq!(back.actions.len(), m.actions.len());
    assert_eq!(back.binds.len(), m.binds.len());
}

#[test]
fn a_bad_manifest_json_is_an_error_not_a_panic() {
    for s in [
        "",
        "{",
        "[]",
        "null",
        "{\"id\": 3}",
        "\u{0}",
        &"[".repeat(10_000),
    ] {
        assert!(konoma_plugin::proc::manifest_from_json(s).is_err(), "{s:?}");
    }
}

fn full_manifest() -> Manifest {
    Manifest::new("full", "1.2.3")
        .name(Tr::new("Full").ja("全部"))
        .shows(ViewKind::Text)
        .file_text()
        .sized()
        .bind(Bind::files(["*.full", "*.f2"]))
        .bind(Bind::mime("image/*").option())
        .bind(Bind::embed("math"))
        .action(
            Act::new("a1")
                .key("x")
                .label(Tr::new("One").ja("一"))
                .help(Tr::new("h"))
                .footer(3),
        )
        .action(Act::new("a2").key("ctrl-y").writes().destructive())
        .setting(SettingDecl::new("size", Value::Int(3), Tr::new("The size")))
        .screen("main")
}

// ---------------------------------------------------------------------------------------------
// The frame budget: styles once per answer, coloured text that is too much goes out plain
// ---------------------------------------------------------------------------------------------

#[test]
fn the_largest_canvas_with_a_distinct_style_in_every_cell_fits_in_a_frame() {
    let dir = unique_tmp("kp_maxcanvas");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let side_len = 2048u16;
    let mut c = CanvasView::new(side_len, side_len);
    for (i, cell) in c.cells.iter_mut().enumerate() {
        let i = i as u32;
        cell.symbol = "a".into();
        cell.style = Style::new().fg(Color::Rgb((i >> 16) as u8, (i >> 8) as u8, i as u8));
    }
    c.check().expect("the largest canvas is a valid one");
    let mut enc = Enc::new(&side);
    let w = view_to_wire(&mut View::Canvas(c), &mut enc).expect("it converts");
    let bytes = frame::encode(&w).expect("the frame is within the limit");
    assert!(
        bytes.len() < frame::MAX_FRAME,
        "{} bytes of {}",
        bytes.len(),
        frame::MAX_FRAME
    );
    let WView::Canvas(wc) = &w else { panic!() };
    assert!(
        matches!(wc.styles, WBin::File { .. }),
        "the style table is large and goes through a side file"
    );
    // And it arrives whole.
    let w2: WView = frame::decode(&bytes).unwrap();
    let View::Canvas(back) = view_from_wire(w2, &mut Dec::new(&side)).expect("it converts back")
    else {
        panic!()
    };
    assert_eq!(back.cells.len(), side_len as usize * side_len as usize);
    for i in [0usize, 1, 255, 256, 65_535, 65_536, 4_194_303] {
        assert_eq!(
            back.cells[i].style,
            Style::new().fg(Color::Rgb((i >> 16) as u8, (i >> 8) as u8, i as u8)),
            "cell {i}"
        );
    }
}

#[test]
fn a_style_table_that_is_not_whole_records_or_a_run_past_it_is_refused() {
    let p = plain_style();
    let mut c = canvas_of(3, 1, vec![vec![run(&p, "abc")]]);
    assert!(try_canvas(c.clone()).is_ok());
    // A run that names a style the table does not have.
    c.runs = vec![vec![(7, "abc".to_string())]];
    assert!(matches!(try_canvas(c.clone()), Err(WireError::Bad(_))));
    // A table cut in the middle of a record.
    let mut c = canvas_of(3, 1, vec![vec![run(&p, "abc")]]);
    c.styles = WBin::Inline(vec![0; STYLE_RECORD + 5]);
    assert!(matches!(try_canvas(c), Err(WireError::Bad(_))));
    // A colour tag that does not exist.
    let mut c = canvas_of(3, 1, vec![vec![run(&p, "abc")]]);
    c.styles = WBin::Inline(vec![250; STYLE_RECORD]);
    assert!(matches!(try_canvas(c), Err(WireError::Bad(_))));
}

fn coloured_text(chars: usize) -> TextView {
    struct PerChar;
    impl LineStyler for PerChar {
        fn style_block(&mut self, _: usize, text: &str) -> Vec<Line> {
            text.lines()
                .map(|l| {
                    Line::from(
                        l.chars()
                            .enumerate()
                            .map(|(i, ch)| {
                                Span::styled(
                                    ch.to_string(),
                                    Style::new().fg(Color::Rgb(i as u8, (i >> 8) as u8, 7)),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect()
        }
    }
    TextView {
        source: TextSource::Owned("x".repeat(chars)),
        styler: Some(Box::new(PerChar)),
        file_lines: false,
        syntax_name: None,
    }
}

#[test]
fn coloured_text_that_would_not_fit_goes_out_plain_with_the_reason() {
    let dir = unique_tmp("kp_styledfallback");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    // One span per character: far more than STYLED_MAX_ENCODED once the style is spelled out.
    let mut v = View::Text(coloured_text(1_500_000));
    let w = view_to_wire(&mut v, &mut Enc::new(&side)).expect("it does not fail");
    let WView::Text(t) = &w else { panic!() };
    assert!(
        matches!(&t.source, WTextSource::Owned(s) if s.len() == 1_500_000),
        "sent as the plain text"
    );
    let note = t.note.as_deref().expect("the reason is recorded");
    assert!(note.contains("sent plain"), "{note}");
    assert!(frame::encode(&w).unwrap().len() < frame::MAX_FRAME);

    // A modest amount of colour is sent as it is, with no note.
    let mut v = View::Text(coloured_text(2_000));
    let w = view_to_wire(&mut v, &mut Enc::new(&side)).unwrap();
    let WView::Text(t) = &w else { panic!() };
    assert!(matches!(&t.source, WTextSource::Lines(_)));
    assert!(t.note.is_none());
}

#[test]
fn lines_a_plugin_styled_itself_are_held_to_the_same_limit() {
    let dir = unique_tmp("kp_styledlines");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let n = 1_500_000usize;
    let line = Line::from(
        (0..n)
            .map(|i| Span::styled("y", Style::new().fg(Color::Rgb(i as u8, (i >> 8) as u8, 1))))
            .collect::<Vec<_>>(),
    );
    let mut v = View::Text(TextView {
        source: TextSource::Lines(vec![line]),
        styler: None,
        file_lines: false,
        syntax_name: None,
    });
    let w = view_to_wire(&mut v, &mut Enc::new(&side)).expect("not a failure");
    let WView::Text(t) = &w else { panic!() };
    assert!(
        matches!(&t.source, WTextSource::Owned(s) if s.len() == n),
        "the words are kept"
    );
    assert!(t.note.is_some());
}

// ---------------------------------------------------------------------------------------------
// Side files: totals, and the ones nobody will read
// ---------------------------------------------------------------------------------------------

fn put_file(dir: &Path, name: &str, len: u64) {
    let f = std::fs::File::create(dir.join(name)).unwrap();
    f.set_len(len).unwrap();
}

#[test]
fn the_total_of_side_files_in_one_message_is_bounded_and_the_error_says_too_large() {
    let dir = unique_tmp("kp_sidetotal2");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let len = 600 << 10;
    for n in ["c-1", "c-2"] {
        put_file(dir.as_path(), n, len);
    }
    let leaf = |n: &str| {
        (
            WLeaf::Encoded(WBin::File {
                name: n.into(),
                len,
            }),
            1u64,
        )
    };
    let v = WView::Image(WImageView {
        data: WImage::Frames(vec![leaf("c-1"), leaf("c-2")]),
        sharp_zoom: false,
        raster_max_px: None,
    });
    // 1 MiB in all: the first file fits, the second takes it over.
    let r = view_from_wire(v, &mut Dec::with_total_max(&side, 1 << 20));
    assert!(matches!(r, Err(WireError::TooLarge(_))), "{r:?}");
    const _: () = assert!(SIDE_TOTAL_MAX_FROM_CHILD < SIDE_TOTAL_MAX);
    assert_eq!(
        SIDE_TOTAL_MAX_FROM_CHILD,
        256 << 20,
        "the canvas hold limit of the host"
    );
}

#[test]
fn side_files_of_a_view_that_fails_to_convert_can_be_deleted_by_name() {
    let dir = unique_tmp("kp_sidediscard");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    put_file(dir.as_path(), "c-1", 10);
    put_file(dir.as_path(), "c-2", 10);
    put_file(dir.as_path(), "keep", 10);
    let rel = WLeaf::File(path_to_wire(Path::new("rel/x.png")));
    let file = |n: &str| {
        WLeaf::Encoded(WBin::File {
            name: n.into(),
            len: 10,
        })
    };
    let v = WView::Image(WImageView {
        // The first frame breaks the rules; the two after it are never read.
        data: WImage::Frames(vec![(rel, 1), (file("c-1"), 1), (file("c-2"), 1)]),
        sharp_zoom: false,
        raster_max_px: None,
    });
    let names = view_side_names(&v);
    assert_eq!(names, vec!["c-1".to_string(), "c-2".to_string()]);
    let mut dec = Dec::new(&side);
    assert!(view_from_wire(v, &mut dec).is_err());
    assert!(
        dir.as_path().join("c-1").exists(),
        "unread, as it stopped at the first"
    );
    dec.discard(&names);
    assert!(!dir.as_path().join("c-1").exists());
    assert!(!dir.as_path().join("c-2").exists());
    assert!(dir.as_path().join("keep").exists(), "only the named ones");

    // A name that is not a plain file name is not followed out of the directory.
    let outside = unique_tmp("kp_sidediscard_out");
    std::fs::create_dir_all(outside.as_path()).unwrap();
    put_file(outside.as_path(), "victim", 1);
    let evil = format!(
        "../{}/victim",
        outside.as_path().file_name().unwrap().to_str().unwrap()
    );
    dec.discard(&[evil]);
    assert!(outside.as_path().join("victim").exists());
}

#[test]
fn the_side_files_a_sender_wrote_are_listed_and_a_failed_conversion_deletes_them() {
    let dir = unique_tmp("kp_senddiscard");
    std::fs::create_dir_all(dir.as_path()).unwrap();
    let side = side_in(dir.as_path());
    let mut enc = Enc::new(&side);
    let mut v = View::Image(ImageView::new(ImageData::Frames(vec![
        (
            ImageData::Encoded(Arc::from(vec![1u8; (1 << 20) + 1])),
            Duration::from_millis(10),
        ),
        (
            ImageData::Encoded(Arc::from(vec![2u8; (1 << 20) + 1])),
            Duration::from_millis(10),
        ),
        // The last frame breaks the rules: the two files written before it are not wanted.
        (
            ImageData::File(PathBuf::from("rel.png")),
            Duration::from_millis(10),
        ),
    ])));
    assert!(view_to_wire(&mut v, &mut enc).is_err());
    let left = |d: &Path| std::fs::read_dir(d).unwrap().count();
    assert_eq!(left(dir.as_path()), 2);
    enc.discard();
    assert_eq!(left(dir.as_path()), 0);

    // On success the names `view_side_names` finds are exactly the ones written.
    let mut enc = Enc::new(&side);
    let mut v = View::Image(ImageView::new(ImageData::Frames(vec![
        (
            ImageData::Encoded(Arc::from(vec![1u8; (1 << 20) + 1])),
            Duration::from_millis(10),
        ),
        (
            ImageData::Rgba {
                width: 1024,
                height: 513,
                pixels: Arc::new(vec![0; 1024 * 513 * 4]),
            },
            Duration::from_millis(10),
        ),
    ])));
    let w = view_to_wire(&mut v, &mut enc).unwrap();
    let mut named = view_side_names(&w);
    let mut written = enc.into_written();
    named.sort();
    written.sort();
    assert_eq!(named, written);
    assert_eq!(named.len(), 2);
}
