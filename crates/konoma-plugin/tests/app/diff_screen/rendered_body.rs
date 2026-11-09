use konoma_plugin::document_render::{PictureSlots, RenderSettings};
use konoma_plugin::host::project::rendered_diff::{render_diff_document, RenderedDiffInput};
use konoma_widgets::markdown::{ImageSlot, MathSlot, MermaidSlot};

#[path = "rendered_body_fixture.rs"]
mod fixture;

#[test]
fn rendered_body_matches_host_cells_marks_and_both_versions_resources() {
    check("resources");
}

#[test]
fn rendered_body_preserves_details_postprocess_and_display_cap() {
    check("details-cap");
}

fn check(case: &str) {
    let (_dir, outcome) = fixture::outcome(case);
    let (old_pre, new_pre, ops) = fixture::ready(&outcome);
    let input = RenderedDiffInput::new(old_pre, new_pre, ops);
    let display = konoma_plugin::app::Display::default();
    let overrides = [(0, false), (1, true)].into();
    let settings = RenderSettings {
        width: fixture::WIDTH,
        code: display.code_style(),
        code_theme: &display.theme.code_theme,
        icons: display.icons,
        mermaid_caption: "Enter: full screen",
        details_open: &overrides,
    };
    let body = render_diff_document(
        &input,
        &fixture::options(),
        &settings,
        &PictureSlots {
            image: &|_, _| ImageSlot::Loading,
            mermaid: &|_| MermaidSlot::Image { cols: 9, rows: 2 },
            math: &|_, _| MathSlot::Image { cols: 4, rows: 1 },
            mermaid_on: true,
            math_on: true,
            remote_on: true,
        },
    );
    let prefix = body.row_prefix(true);
    for scroll in [0, prefix.last().copied().unwrap().saturating_sub(12)] {
        let (visible, local) = body.visible_slice(&prefix, scroll as u16, 12);
        assert!(!visible.is_empty());
        use ratatui::{
            buffer::Buffer,
            layout::Rect,
            widgets::{Paragraph, Widget, Wrap},
        };
        let area = Rect::new(0, 0, fixture::WIDTH, 12);
        let mut full = Buffer::empty(area);
        let mut sliced = Buffer::empty(area);
        Paragraph::new(body.lines.clone())
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0))
            .render(area, &mut full);
        Paragraph::new(visible)
            .wrap(Wrap { trim: false })
            .scroll((local, 0))
            .render(area, &mut sliced);
        assert_eq!(sliced, full, "visible slice preserves every cell");
    }
    let actual = fixture::record(
        &body.lines,
        &body.images,
        &body.diff_marks,
        &body.remote_urls,
        &body.mermaid_fences,
        &body.math_exprs,
        body.src_lines,
        prefix,
    );
    fixture::assert_reference(case, actual);
}
