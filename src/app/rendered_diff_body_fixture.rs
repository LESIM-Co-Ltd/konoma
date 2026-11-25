//! Local test helpers; only input and expected JSON data are shared with the other host.
use konoma_plugin::host::project::md_diff::{
    DiffBaseline, MdDiffKind, MdDiffOutcome, MdDiffRequest, MdDiffWorker,
};
use konoma_plugin::view::DocOptions;
use konoma_widgets::markdown::{BlockOp, DiffMark, ImagePlacement};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Paragraph, Widget, Wrap},
};
use serde_json::{json, Value};

const SNAPSHOT_DIR: &str = "snapshots";

pub const WIDTH: u16 = 32;

pub fn options() -> DocOptions {
    DocOptions {
        details: "closed".into(),
        ..DocOptions::default()
    }
}

fn read_json(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(SNAPSHOT_DIR)
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).expect("rendered diff fixture")).unwrap()
}

pub fn sources(case: &str) -> (String, String) {
    fn source(value: &Value) -> String {
        if let Some(text) = value.as_str() {
            return text.to_string();
        }
        let mut text = value["prefix"].as_str().unwrap().to_string();
        let template = value["row_template"].as_str().unwrap();
        for i in 0..value["count"].as_u64().unwrap() {
            text.push_str(&template.replace("{i}", &i.to_string()));
        }
        text.push_str(value["tail"].as_str().unwrap());
        text
    }
    let inputs = read_json("rendered_diff_body_input.json");
    let input = &inputs[case];
    (source(&input["old"]), source(&input["new"]))
}

pub fn outcome(case: &str) -> (konoma_test_support::TmpDir, MdDiffOutcome) {
    let dir = konoma_test_support::unique_tmp("rendered_body");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    let (old, new) = sources(case);
    std::fs::write(&path, new).unwrap();
    let outcome = MdDiffWorker::compute_md_diff(&MdDiffRequest {
        gen: 0,
        path,
        root: dir.to_path_buf(),
        kind: MdDiffKind::Rendered,
        baseline: DiffBaseline::FollowSnapshot(old.into_bytes()),
        md_frontmatter: true,
        md_footnotes: true,
        md_inline_html: true,
    });
    (dir, outcome)
}

pub fn ready(outcome: &MdDiffOutcome) -> (&str, &str, &[BlockOp]) {
    let MdDiffOutcome::Ready {
        old_pre,
        new_pre,
        ops,
        ..
    } = outcome
    else {
        panic!("fixture must produce Ready");
    };
    (old_pre, new_pre, ops)
}

#[allow(clippy::too_many_arguments)]
pub fn record(
    lines: &[Line<'static>],
    images: &[ImagePlacement],
    marks: &[(std::ops::Range<usize>, DiffMark)],
    remote: &[String],
    fences: &[String],
    math: &[(String, bool)],
    src_lines: usize,
    prefix: Vec<usize>,
) -> Value {
    let total = prefix.last().copied().unwrap_or(lines.len());
    let frames: Vec<_> = [0, total.saturating_sub(12)]
        .into_iter()
        .map(|scroll| {
            let rect = Rect::new(0, 0, WIDTH, 12);
            let mut buf = Buffer::empty(rect);
            Paragraph::new(lines.to_vec())
                .wrap(Wrap { trim: false })
                .scroll((scroll as u16, 0))
                .render(rect, &mut buf);
            let cells: Vec<_> = buf
                .content
                .iter()
                .map(|c| {
                    json!([
                        c.symbol(),
                        format!("{:?}", c.fg),
                        format!("{:?}", c.bg),
                        c.modifier.bits()
                    ])
                })
                .collect();
            let mut palette = Vec::<Value>::new();
            let mut runs = Vec::<(usize, usize)>::new();
            for cell in cells {
                let index = palette.iter().position(|c| *c == cell).unwrap_or_else(|| {
                    palette.push(cell);
                    palette.len() - 1
                });
                if let Some(last) = runs.last_mut().filter(|last| last.1 == index) {
                    last.0 += 1;
                } else {
                    runs.push((1, index));
                }
            }
            json!({"rect": [0,0,WIDTH,12], "scroll": scroll, "palette": palette, "runs": runs})
        })
        .collect();
    json!({
        "frames": frames,
        "marks": marks.iter().map(|(r,m)| json!([r.start,r.end,format!("{m:?}")])).collect::<Vec<_>>(),
        "images": images.iter().map(|p| json!([p.url,p.alt,p.line,p.col,p.cols,p.rows,p.fence_ord])).collect::<Vec<_>>(),
        "remote": remote, "fences": fences, "math": math, "src_lines": src_lines,
        "prefix_len": prefix.len(), "total": total,
        "max_cols": lines.iter().map(Line::width).max().unwrap_or(0),
    })
}

pub fn assert_reference(case: &str, actual: Value) {
    let all = read_json("rendered_diff_body.json");
    let expected = &all[case];
    assert_eq!(actual["frames"].as_array().unwrap().len(), 2);
    assert_eq!(
        actual["frames"].as_array().unwrap().len(),
        expected["frames"].as_array().unwrap().len()
    );
    for (actual, expected) in actual["frames"]
        .as_array()
        .unwrap()
        .iter()
        .zip(expected["frames"].as_array().unwrap())
    {
        assert_eq!(actual["rect"], expected["rect"], "whole rect");
        assert_eq!(actual["scroll"], expected["scroll"], "scroll");
        let expand = |frame: &Value| {
            frame["runs"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|r| {
                    std::iter::repeat_n(
                        frame["palette"][r[1].as_u64().unwrap() as usize].clone(),
                        r[0].as_u64().unwrap() as usize,
                    )
                })
                .collect::<Vec<_>>()
        };
        let a = expand(actual);
        let e = expand(expected);
        assert_eq!(a.len(), WIDTH as usize * 12, "compared count");
        assert!(
            a.iter().any(|c| !c[0].as_str().unwrap().trim().is_empty()),
            "non-empty"
        );
        assert_eq!(a, e, "symbol / fg / bg / modifier");
    }
    for key in [
        "marks",
        "images",
        "remote",
        "fences",
        "math",
        "src_lines",
        "prefix_len",
        "total",
        "max_cols",
    ] {
        assert_eq!(actual[key], expected[key], "{case}: {key}");
    }
}
