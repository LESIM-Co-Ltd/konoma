use super::*;
use konoma_plugin::input::Resource;
use konoma_plugin::testing::{check_manifest, open_file};
use konoma_plugin::FailureKind;

fn tmp_file(name: &str, bytes: &[u8]) -> (konoma_test_support::TmpDir, PathBuf) {
    let dir = konoma_test_support::unique_tmp("konoma_markdown_plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn view_of(session: &mut dyn Session, repr: Option<&str>) -> View {
    let req = ViewReq {
        repr: repr.map(str::to_string),
        ..ViewReq::default()
    };
    session
        .view(&req, &ViewCx::new(Settings::empty(), "en"))
        .expect("a view")
}

fn document(view: View) -> DocumentView {
    match view {
        View::Document(d) => d,
        other => panic!("not a document view: {other:?}"),
    }
}

fn settings(pairs: &[(&str, Value)]) -> Settings {
    let mut s = Settings::new();
    for (k, v) in pairs {
        s.set(*k, v.clone());
    }
    s
}

fn open_with(settings: &Settings, path: &Path) -> Box<dyn Session> {
    MarkdownPlugin
        .open(
            Input::File {
                path: path.to_path_buf(),
                hint: None,
            },
            &OpenCx::new(settings, "en"),
        )
        .expect("a file opens")
}

fn converted(from: &str) -> Origin {
    Origin::Converted {
        from: PathBuf::from(from),
    }
}

fn open_content(text: &str, resources: Resources, origin: Origin) -> Box<dyn Session> {
    MarkdownPlugin
        .open(
            Input::Content {
                content_type: CONTENT_TYPE.to_string(),
                text: text.to_string(),
                resources,
                origin,
            },
            &OpenCx::bare(),
        )
        .expect("content opens")
}

#[test]
fn the_manifest_is_clean_and_names_the_files_and_the_content_type() {
    assert!(check_manifest(&MarkdownPlugin).is_empty());
    let m = markdown_manifest();
    assert_eq!(m.id, MARKDOWN_ID);
    let names: Vec<&str> = m.settings.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "task_states",
            "autolink",
            "alerts",
            "emoji",
            "frontmatter",
            "footnotes",
            "inline_html",
            "details",
            "table_align",
            "image_align"
        ]
    );
    use konoma_plugin::Target;
    let targets: Vec<&Target> = m.binds.iter().flat_map(|b| b.targets.iter()).collect();
    assert!(targets.contains(&&Target::Glob("*.md".into())));
    assert!(targets.contains(&&Target::Content("markdown".into())));
    // `e` edits in the terminal at the caret line, `o` hands the file to the OS.
    assert!(m.verbs.iter().any(|v| v.verb == Verb::Edit && v.line_token));
    assert!(m.verbs.iter().any(|v| v.verb == Verb::Open));
}

#[test]
fn a_file_is_a_document_of_its_text_with_the_files_origin() {
    let (_dir, path) = tmp_file("a.md", b"# Title\n\nbody\n");
    let mut s = open_file(&MarkdownPlugin, &path).unwrap();
    let d = document(view_of(s.as_mut(), None));
    // The lines are joined with `\n`; the file's final newline is not kept.
    assert_eq!(d.markdown, "# Title\n\nbody");
    assert_eq!(d.origin, Origin::File(path));
    assert!(!d.truncated);
    assert!(d.resources.is_empty());
    assert_eq!(d.options, DocOptions::default());
}

#[test]
fn the_other_representation_is_the_source_and_the_rendered_one_comes_back() {
    let (_dir, path) = tmp_file("a.md", b"x\ny\n");
    let mut s = open_file(&MarkdownPlugin, &path).unwrap();
    let l = s.layout();
    let ids: Vec<&str> = l.reprs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, [REPR_RENDERED, REPR_SOURCE]);
    assert_eq!(l.default_repr.as_deref(), Some(REPR_RENDERED));
    assert!(l.pages.is_none());
    // The source of a file is the file, with the file's own line numbers.
    match view_of(s.as_mut(), Some(REPR_SOURCE)) {
        View::Text(t) => {
            assert!(matches!(&t.source, TextSource::File(p) if *p == path));
            assert!(t.file_lines);
            assert!(t.styler.is_none());
        }
        other => panic!("not a text view: {other:?}"),
    }
    // And back: the same document as the first time (nothing is read again).
    std::fs::write(&path, "changed\n").unwrap();
    assert_eq!(
        document(view_of(s.as_mut(), Some(REPR_RENDERED))).markdown,
        "x\ny"
    );
    assert_eq!(document(view_of(s.as_mut(), None)).markdown, "x\ny");
}

#[test]
fn a_converted_document_has_no_front_matter_and_cannot_be_written_back() {
    let mut s = open_content(
        "---\na: 1\n---\n\n- [ ] t\n",
        Resources::new(),
        converted("w.docx"),
    );
    let d = document(view_of(s.as_mut(), None));
    assert!(!d.options.front_matter);
    assert!(!d.options.tasks_writable);
    assert_eq!(d.origin, converted("w.docx"));
    // Everything else follows the settings' defaults.
    assert!(d.options.alerts && d.options.footnotes && d.options.autolink);
    // Its source is the converted text as a string (there is no file with those lines).
    match view_of(s.as_mut(), Some(REPR_SOURCE)) {
        View::Text(t) => {
            assert!(matches!(&t.source, TextSource::Owned(text) if text.starts_with("---\na: 1")));
            assert!(!t.file_lines);
        }
        other => panic!("not a text view: {other:?}"),
    }
}

#[test]
fn a_file_content_is_written_back_and_shows_front_matter() {
    let mut s = open_content("x", Resources::new(), Origin::File(PathBuf::from("a.md")));
    let d = document(view_of(s.as_mut(), None));
    assert!(d.options.front_matter);
    assert!(d.options.tasks_writable);
}

#[test]
fn the_resources_of_a_converted_document_come_back_with_every_rendered_view() {
    let mut r = Resources::new();
    r.insert(
        "office-img://a",
        Resource::Bytes(Arc::from(vec![1u8, 2, 3].into_boxed_slice())),
    );
    let mut s = open_content("![a](office-img://a)", r, converted("w.docx"));
    assert_eq!(document(view_of(s.as_mut(), None)).resources.len(), 1);
    let _ = view_of(s.as_mut(), Some(REPR_SOURCE));
    assert_eq!(document(view_of(s.as_mut(), None)).resources.len(), 1);
}

#[test]
fn the_settings_become_the_options_and_a_missing_one_keeps_its_default() {
    let d = DocOptions::default();
    let from =
        |pairs: &[(&str, Value)]| doc_options(&settings(pairs), &Origin::File("a.md".into()));
    assert_eq!(from(&[]), d);
    let o = from(&[
        ("alerts", Value::Bool(false)),
        ("autolink", Value::Bool(false)),
        ("emoji", Value::Bool(false)),
        ("frontmatter", Value::Bool(false)),
        ("footnotes", Value::Bool(false)),
        ("inline_html", Value::Bool(false)),
        ("details", Value::Str("closed".into())),
        ("table_align", Value::Str("right".into())),
        ("image_align", Value::Str("left".into())),
        (
            "task_states",
            Value::List(vec![
                Value::Str(" ".into()),
                Value::Str("/".into()),
                Value::Str("x".into()),
            ]),
        ),
    ]);
    assert!(!o.alerts && !o.autolink && !o.emoji && !o.front_matter);
    assert!(!o.footnotes && !o.inline_html);
    assert_eq!(o.details, "closed");
    assert_eq!(o.table_align, "right");
    assert_eq!(o.image_align, "left");
    assert_eq!(o.task_states, [' ', '/', 'x']);
    // A value of another type is not a value.
    let o = from(&[
        ("alerts", Value::Str("no".into())),
        ("details", Value::Bool(true)),
    ]);
    assert!(o.alerts);
    assert_eq!(o.details, "auto");
}

#[test]
fn task_states_need_two_single_character_entries_or_the_default_pair_is_used() {
    let states = |xs: &[&str]| {
        let list = Value::List(xs.iter().map(|x| Value::Str((*x).into())).collect());
        doc_options(
            &settings(&[("task_states", list)]),
            &Origin::File("a.md".into()),
        )
        .task_states
    };
    assert_eq!(states(&[" ", "x"]), [' ', 'x']);
    assert_eq!(states(&["a", "b"]), ['a', 'b']);
    assert_eq!(states(&["○", "●"]), ['○', '●']);
    assert_eq!(states(&["x"]), [' ', 'x'], "one state has nowhere to go");
    assert_eq!(states(&[]), [' ', 'x']);
    assert_eq!(
        states(&[" ", "xx"]),
        [' ', 'x'],
        "an entry of two characters"
    );
    assert_eq!(states(&[" ", ""]), [' ', 'x'], "an empty entry");
}

#[test]
fn the_settings_reach_the_view_through_open() {
    let (_dir, path) = tmp_file("a.md", b"x\n");
    let s = settings(&[("alerts", Value::Bool(false))]);
    let mut session = open_with(&s, &path);
    assert!(!document(view_of(session.as_mut(), None)).options.alerts);
}

#[test]
fn a_long_file_is_cut_at_the_line_limit_and_says_so() {
    let mut text = String::new();
    for i in 0..(MAX_LINES + 20) {
        text.push_str(&format!("line {i}\n"));
    }
    let (_dir, path) = tmp_file("long.md", text.as_bytes());
    let (md, cut) = read_limited(&path).unwrap();
    assert!(cut);
    assert_eq!(md.lines().count(), MAX_LINES);
    assert!(md.ends_with(&format!("line {}", MAX_LINES - 1)));
    let mut s = open_file(&MarkdownPlugin, &path).unwrap();
    assert!(document(view_of(s.as_mut(), None)).truncated);
}

#[test]
fn a_big_file_is_cut_at_the_byte_limit_and_a_file_of_exactly_that_size_is_not() {
    let line = "x".repeat(99);
    let mut text = String::new();
    while text.len() < MAX_BYTES + 5000 {
        text.push_str(&line);
        text.push('\n');
    }
    let (_d1, big) = tmp_file("big.md", text.as_bytes());
    let (md, cut) = read_limited(&big).unwrap();
    assert!(cut);
    assert!(md.len() <= MAX_BYTES);
    // Exactly the limit: nothing is cut (one long line, so the line limit is not in play).
    let (_d2, exact) = tmp_file("exact.md", "y".repeat(MAX_BYTES).as_bytes());
    let (md, cut) = read_limited(&exact).unwrap();
    assert!(!cut);
    assert_eq!(md.len(), MAX_BYTES);
}

#[test]
fn bytes_that_are_not_utf8_are_replaced_and_crlf_lines_are_joined_with_lf() {
    let (_dir, path) = tmp_file("b.md", b"a\xffb\r\nc\r\n");
    let (md, cut) = read_limited(&path).unwrap();
    assert!(!cut);
    assert_eq!(md, "a\u{fffd}b\nc");
}

#[test]
fn an_empty_file_is_an_empty_document() {
    let (_dir, path) = tmp_file("e.md", b"");
    let mut s = open_file(&MarkdownPlugin, &path).unwrap();
    let d = document(view_of(s.as_mut(), None));
    assert_eq!(d.markdown, "");
    assert!(!d.truncated);
}

#[test]
fn a_file_that_cannot_be_read_is_an_io_failure() {
    let r = open_file(&MarkdownPlugin, Path::new("/nonexistent/konoma/a.md"));
    assert_eq!(r.err().map(|f| f.kind), Some(FailureKind::Io));
}

#[test]
fn only_files_and_markdown_content_open() {
    let embed = MarkdownPlugin.open(
        Input::Embed {
            kind: "math".into(),
            source: "x".into(),
            display: false,
        },
        &OpenCx::bare(),
    );
    assert_eq!(embed.err().map(|f| f.kind), Some(FailureKind::Unsupported));
    let other = MarkdownPlugin.open(
        Input::Content {
            content_type: "html".into(),
            text: String::new(),
            resources: Resources::new(),
            origin: converted("x"),
        },
        &OpenCx::bare(),
    );
    assert_eq!(other.err().map(|f| f.kind), Some(FailureKind::Unsupported));
}

#[test]
fn the_default_options_are_the_documented_defaults() {
    // The values the `[ui] md_*` settings have always defaulted to.
    let d = DocOptions::from_settings(Settings::empty());
    assert_eq!(d, DocOptions::default());
    assert_eq!(d.task_states, [' ', 'x']);
    assert_eq!(d.details, "auto");
    assert_eq!(d.table_align, "left");
    assert_eq!(d.image_align, "center");
    assert!(d.front_matter && d.footnotes && d.inline_html && d.autolink && d.emoji && d.alerts);
}
