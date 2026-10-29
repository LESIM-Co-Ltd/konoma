use std::cell::RefCell;
use std::collections::HashMap;

use konoma_widgets::markdown::{self, CodeStyle, ImageSlot, MathSlot, MermaidSlot};
use ratatui::text::Line;

use super::*;

fn text_of(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(|l| l.to_string()).collect()
}

fn settings(width: u16, overrides: &HashMap<usize, bool>) -> RenderSettings<'_> {
    RenderSettings {
        width,
        code: CodeStyle::default(),
        code_theme: "base16-ocean.dark",
        icons: false,
        mermaid_caption: "diagram",
        details_open: overrides,
    }
}

/// Run `render_document` with slots that answer `Unavailable`/`Text`/`Raw` and note every ask.
struct Asks {
    images: RefCell<Vec<String>>,
    mermaid: RefCell<Vec<String>>,
    math: RefCell<Vec<(String, bool)>>,
}

fn render(text: &str, opts: &DocOptions, width: u16, picture_on: bool) -> (RenderedDocument, Asks) {
    let asks = Asks {
        images: RefCell::new(Vec::new()),
        mermaid: RefCell::new(Vec::new()),
        math: RefCell::new(Vec::new()),
    };
    let none = HashMap::new();
    let image = |u: &str, _: Option<u16>| {
        asks.images.borrow_mut().push(u.to_string());
        ImageSlot::Unavailable
    };
    let mermaid = |c: &str| {
        asks.mermaid.borrow_mut().push(c.to_string());
        MermaidSlot::Text
    };
    let math = |l: &str, d: bool| {
        asks.math.borrow_mut().push((l.to_string(), d));
        MathSlot::Raw
    };
    let r = render_document(
        text,
        opts,
        &settings(width, &none),
        &PictureSlots {
            image: &image,
            mermaid: &mermaid,
            math: &math,
            mermaid_on: picture_on,
            math_on: picture_on,
            remote_on: picture_on,
        },
    );
    (r, asks)
}

const DOC: &str = "# Title\n\nSome *emphasis* and a long paragraph that will need to be wrapped when the width is narrow enough to force it.\n\n- [ ] todo\n- [x] done\n\n```rust\nfn main() {}\n```\n";

fn plain() -> DocOptions {
    DocOptions {
        front_matter: false,
        footnotes: false,
        inline_html: false,
        ..DocOptions::default()
    }
}

#[test]
fn lines_match_the_renderer_primitive_the_host_used_before() {
    // The host drew a document through exactly this call; the shared function must give the
    // same lines, placements and record for the same input.
    let opts = plain();
    let (r, _) = render(DOC, &opts, 60, false);
    let none = HashMap::new();
    let s = settings(60, &none);
    let (lines, images, extras) = markdown::render_markdown_with_images_aligned(
        DOC,
        60,
        s.code,
        s.code_theme,
        false,
        &opts.task_states,
        &|_, _| ImageSlot::Unavailable,
        &|_| MermaidSlot::Text,
        "diagram",
        opts.alerts,
        &|_, _| MathSlot::Raw,
        false,
        block_aligns(&opts),
    );
    assert_eq!(text_of(&r.lines), text_of(&lines));
    assert_eq!(r.images.len(), images.len());
    assert_eq!(r.extras.tasks.len(), extras.tasks.len());
    assert_eq!(r.pre_src, DOC);
    assert_eq!(r.src_lines, DOC.lines().count());
}

#[test]
fn front_matter_is_split_off_and_placements_shift_below_it() {
    let text = "---\ntitle: T\n---\n\n![p](pic.png)\n";
    let mut opts = plain();
    opts.front_matter = true;
    let (with, _) = render(text, &opts, 40, false);
    let (without, _) = render(text, &plain(), 40, false);
    assert!(!with.pre_src.contains("title: T"));
    assert!(without.pre_src.contains("title: T"));
    assert!(!with.lines.is_empty());
    // The metadata block is prepended to the body lines.
    assert!(text_of(&with.lines).iter().any(|l| l.contains("title")));
}

#[test]
fn picture_lookups_are_asked_once_per_place_and_only_for_what_the_document_holds() {
    let text = "![a](a.png)\n\n![b](b.png)\n\n```mermaid\ngraph TD\nA-->B\n```\n\n$$x^2$$\n";
    let (r, asks) = render(text, &plain(), 60, true);
    assert_eq!(*asks.images.borrow(), vec!["a.png", "b.png"]);
    assert!(!asks.mermaid.borrow().is_empty());
    assert!(asks.math.borrow().iter().any(|(l, d)| l == "x^2" && *d));
    assert_eq!(r.mermaid_fences.len(), 1);
    assert_eq!(r.math_exprs, vec![("x^2".to_string(), true)]);
    // Nothing to ask about in a document without pictures.
    let (_, asks) = render("just text\n", &plain(), 60, true);
    assert!(asks.images.borrow().is_empty());
    assert!(asks.math.borrow().is_empty());
}

#[test]
fn collection_for_fetching_follows_the_flags() {
    let text = "![r](https://example.com/x.png)\n\n```mermaid\ngraph TD\nA-->B\n```\n\n$$y$$\n";
    let (on, _) = render(text, &plain(), 60, true);
    assert_eq!(on.remote_urls, vec!["https://example.com/x.png"]);
    assert_eq!(on.mermaid_fences.len(), 1);
    assert_eq!(on.math_exprs.len(), 1);
    let (off, _) = render(text, &plain(), 60, false);
    assert!(off.remote_urls.is_empty());
    assert!(off.mermaid_fences.is_empty());
    assert!(off.math_exprs.is_empty());
}

#[test]
fn width_changes_where_lines_wrap() {
    let (wide, _) = render(DOC, &plain(), 100, false);
    let (narrow, _) = render(DOC, &plain(), 20, false);
    let rows = |r: &RenderedDocument, w: u16| *wrap_row_prefix(&r.lines, w).last().unwrap();
    assert!(rows(&narrow, 20) > rows(&wide, 100));
    let pre = wrap_row_prefix(&narrow.lines, 20);
    assert_eq!(pre.len(), narrow.lines.len() + 1);
    assert_eq!(pre[0], 0);
    assert!(pre.windows(2).all(|w| w[1] > w[0]));
}

#[test]
fn details_state_follows_override_then_option_then_attribute() {
    let text = "<details>\n<summary>s</summary>\n\nbody\n\n</details>\n\n<details open>\n<summary>t</summary>\n\nb2\n\n</details>\n";
    let mut opts = plain();
    opts.details = "auto".into();
    let none = HashMap::new();
    let image = |_: &str, _: Option<u16>| ImageSlot::Unavailable;
    let mermaid = |_: &str| MermaidSlot::Text;
    let math = |_: &str, _: bool| MathSlot::Raw;
    let slots = PictureSlots {
        image: &image,
        mermaid: &mermaid,
        math: &math,
        mermaid_on: false,
        math_on: false,
        remote_on: false,
    };
    let r = render_document(text, &opts, &settings(60, &none), &slots);
    assert_eq!(r.details_states, vec![false, true]);
    let over: HashMap<usize, bool> = HashMap::from([(0, true)]);
    let r = render_document(text, &opts, &settings(60, &over), &slots);
    assert_eq!(r.details_states, vec![true, true]);
    opts.details = "closed".into();
    let r = render_document(text, &opts, &settings(60, &none), &slots);
    assert_eq!(r.details_states, vec![false, false]);
    assert_eq!(markdown::current_details_states(), vec![false, false]);
}

#[test]
fn details_default_open_and_aligns() {
    assert!(details_default_open("open", false));
    assert!(!details_default_open("closed", true));
    assert!(details_default_open("auto", true));
    assert!(!details_default_open("anything", false));
    let mut o = plain();
    o.table_align = "right".into();
    assert_eq!(block_aligns(&o).table, markdown::BlockAlign::Right);
}

#[test]
fn an_inline_picture_is_placed_below_the_front_matter_block() {
    let text = "---\ntitle: T\n---\n\n![p](pic.png)\n";
    let none = HashMap::new();
    let image = |_: &str, _: Option<u16>| ImageSlot::Inline { cols: 4, rows: 2 };
    let mermaid = |_: &str| MermaidSlot::Text;
    let math = |_: &str, _: bool| MathSlot::Raw;
    let slots = PictureSlots {
        image: &image,
        mermaid: &mermaid,
        math: &math,
        mermaid_on: false,
        math_on: false,
        remote_on: false,
    };
    let mut opts = plain();
    let base = render_document(text, &opts, &settings(40, &none), &slots);
    opts.front_matter = true;
    let fm = render_document(text, &opts, &settings(40, &none), &slots);
    assert_eq!(base.images.len(), 1);
    assert_eq!(fm.images.len(), 1);
    // The placement points at the row where the picture's lines start in the lines returned.
    let block = fm.lines.len() - base.lines.len();
    assert!(fm.images[0].line < fm.lines.len());
    assert!(fm.images[0].line >= block);
    assert!(fm.images[0].line >= base.images[0].line);
}
