//! Complete logical frames for standalone/host parity, including the bottom status row.
#[cfg(feature = "git")]
use super::Sim;
use ratatui::buffer::Buffer;

#[cfg(feature = "git")]
impl Sim {
    /// Poll and land project work before capturing the renderer's logical buffer. The backend
    /// retains continuation cells of wide glyphs, so its stored screen is not a frame capture.
    #[cfg(feature = "git")]
    pub(super) fn project_frame(&mut self) -> Buffer {
        super::vcs_changes::drain(self);
        self.term
            .draw(|frame| crate::ui::render(frame, &mut self.app))
            .expect("capture project frame")
            .buffer
            .clone()
    }
}

pub(super) fn compare_frames(expected: &Buffer, actual: &Buffer) -> Result<(), String> {
    if expected.area != actual.area {
        return Err(format!(
            "frame rect: {:?} != {:?}",
            expected.area, actual.area
        ));
    }
    let area = expected.area;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let before = &expected[(x, y)];
            let after = &actual[(x, y)];
            if (before.symbol(), before.fg, before.bg, before.modifier)
                != (after.symbol(), after.fg, after.bg, after.modifier)
            {
                let row = |buffer: &Buffer| -> String {
                    (area.x..area.right())
                        .map(|x| buffer[(x, y)].symbol())
                        .collect()
                };
                return Err(format!(
                    "frame cell ({x}, {y})\nexpected: {:?}\nactual:   {:?}\nexpected style: fg={:?} bg={:?} modifiers={:?}\nactual style:   fg={:?} bg={:?} modifiers={:?}",
                    row(expected), row(actual), before.fg, before.bg, before.modifier,
                    after.fg, after.bg, after.modifier,
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn frame_comparison_covers_the_last_row_and_every_style_component() {
    use ratatui::{
        layout::Rect,
        style::{Color, Modifier},
    };
    let expected = Buffer::empty(Rect::new(2, 3, 4, 2));
    for field in 0..4 {
        let mut actual = expected.clone();
        let cell = &mut actual[(5, 4)];
        match field {
            0 => {
                cell.set_symbol("x");
            }
            1 => cell.fg = Color::Red,
            2 => cell.bg = Color::Blue,
            _ => cell.modifier = Modifier::BOLD,
        }
        let mismatch =
            compare_frames(&expected, &actual).expect_err("last row/style must be compared");
        assert!(mismatch.contains("(5, 4)"), "{mismatch}");
        assert!(mismatch.contains("expected style:") && mismatch.contains("actual style:"));
    }
    compare_frames(&expected, &expected).unwrap();
}

#[cfg(feature = "git")]
#[test]
fn project_frame_capture_keeps_the_whole_rect() {
    let fixture = konoma_plugin::app::project::read_snapshot_for_test("vcs_app_idle");
    let width = fixture["rect"][2].as_u64().unwrap() as u16;
    let height = fixture["rect"][3].as_u64().unwrap() as u16;
    let root = crate::test_support::unique_tmp("project_frame_capture");
    std::fs::create_dir_all(&root).unwrap();
    let mut sim = Sim::with_config_sized(&root, crate::config::Config::default(), width, height);
    let frame = sim.project_frame();
    assert_eq!(frame.area, ratatui::layout::Rect::new(0, 0, width, height));
    assert_eq!(
        frame.content.len(),
        usize::from(width) * usize::from(height)
    );
    compare_frames(&frame, &sim.project_frame()).unwrap();
}
