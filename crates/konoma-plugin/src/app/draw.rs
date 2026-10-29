//! Drawing a frame with ratatui.

use ratatui::layout::Rect;
use ratatui::style::{Modifier as RModifier, Style as RStyle};
use ratatui::text::Line as RLine;
use ratatui::widgets::{Block, Paragraph, Widget};

use std::sync::Arc;

use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::lang::Lang;
use konoma_widgets::table::{
    render_cell_popup, render_table, CellPopup, TableOptions, TableViewState,
};

use crate::ratatui_draw;
use crate::style::Line;
use crate::table_ui::{cell_view, GridSource};
use crate::view::{CanvasView, Grid};

/// The text of a file: lines that wrap, or scroll sideways.
pub struct TextFrame {
    pub lines: Vec<Line>,
    /// Wrap at the edge of the body (`[ui] wrap`). Off, the lines are cut at it.
    pub wrap: bool,
    /// Cells scrolled off the left edge (0 while wrapping).
    pub hscroll: u16,
}

/// What is drawn in the body.
pub enum Frame {
    /// Lines from the top of the body.
    Lines(Vec<Line>),
    /// The lines of a file's text: wrapped at the edge (`ui.wrap`), or cut `hscroll` cells from
    /// the left.
    Text(TextFrame),
    /// Cells copied one to one.
    Canvas(CanvasView),
    /// A table with its cursor (and the full-cell popup while it is open).
    Table(Box<TableFrame>),
    /// A still picture in a bordered frame. The border is drawn here; what is inside (the
    /// spinner, the failure line or the picture) is drawn by the pictures
    /// ([`crate::app::Pictures::draw_still`]), or, with no pictures, the line that says the
    /// terminal cannot show it.
    Image(ImageFrame),
    /// The same frame for a picture that could not be asked for (the session's `view` failed):
    /// the reason is drawn inside it ([`crate::app::failure_line`]).
    ImageFailed(FailedImage),
    /// A Markdown document in a bordered frame.
    Document(super::document::DocFrame),
}

/// The picture of a `View::Image` on screen.
#[derive(Clone, Debug)]
pub struct ImageFrame {
    pub view: crate::view::ImageView,
    /// The file name, in the frame title.
    pub name: String,
    pub lang: Lang,
}

/// The frame of a picture whose `view` failed, with the reason inside.
#[derive(Clone, Debug)]
pub struct FailedImage {
    /// The file name, in the frame title.
    pub name: String,
    pub lang: Lang,
    /// Why, when the plugin's failure maps to a reason of a picture.
    pub why: Option<konoma_svg::image::ImageFailure>,
    /// The plugin whose settings the reason may name.
    pub plugin_id: String,
}

/// The frame title: the file name between spaces (konoma's shape), or ` image ` with none.
fn frame_title(name: &str) -> String {
    if name.is_empty() {
        " image ".to_string()
    } else {
        format!(" {name} ")
    }
}

impl ImageFrame {
    /// The frame title: the file name between spaces (konoma's shape), or ` image ` with none.
    pub fn title(&self) -> String {
        frame_title(&self.name)
    }
}

/// The inside of the border of an image frame drawn in `body`.
pub fn image_inner(body: Rect) -> Rect {
    Block::bordered().inner(body)
}

/// The full-cell popup while it is open.
#[derive(Clone, Debug, Default)]
pub struct PopupState {
    /// Wrapped rows scrolled off the top.
    pub scroll: u16,
    /// Rows the popup showed at the last draw (the step of `PageUp` / `PageDown`).
    pub viewport: u16,
    /// The keys that act in it, as the footer line on its border.
    pub footer: String,
}

/// A table on screen: the grid and everything the widget needs to draw it, plus the state a draw
/// settles (the scroll offsets and the page size, the popup's clamped scroll).
pub struct TableFrame {
    pub grid: Arc<dyn Grid>,
    /// The file name, first in the frame title.
    pub name: String,
    /// What the plugin put after the name (a sheet's name and position).
    pub note: Option<String>,
    pub rainbow: bool,
    pub addressed: bool,
    pub lang: Lang,
    /// The language code the plugin's own texts are read in.
    pub lang_code: String,
    pub state: TableViewState,
    pub popup: Option<PopupState>,
}

impl TableFrame {
    /// Keep the cursor inside the grid (a grid read again may be smaller).
    pub fn clamp_cursor(&mut self) {
        let (nr, nc) = (self.grid.nrows(), self.grid.ncols());
        self.state.cursor_row = self.state.cursor_row.min(nr.saturating_sub(1));
        self.state.cursor_col = self.state.cursor_col.min(nc.saturating_sub(1));
    }

    /// The popup's content for the cursor cell.
    pub fn cell(&self) -> Option<crate::table_ui::TableCellView> {
        cell_view(
            &*self.grid,
            self.addressed,
            (self.state.cursor_row, self.state.cursor_col),
            &self.lang_code,
        )
    }

    /// The frame title: the same words as the konoma host's.
    fn title(&self) -> String {
        let capped = self
            .grid
            .truncated()
            .then(|| CoreMsg::SheetCappedWord.tr(self.lang));
        let sheet = self
            .addressed
            .then(|| self.note.clone().unwrap_or_default());
        konoma_widgets::table::table_title(
            &self.name,
            (self.grid.nrows(), self.grid.ncols()),
            (self.state.cursor_row, self.state.cursor_col),
            sheet.as_deref(),
            capped,
        )
    }

    /// Draw the table (and the popup over it) into `area`. The offsets and the page size the
    /// widget settled are kept only when it laid the grid out (a collapsed area must not clobber
    /// the last real page size).
    ///
    /// `screen` is the whole terminal: the popup is centred in it (as the host does), not in the
    /// table's area, so both size and place it the same.
    pub fn draw(&mut self, buf: &mut ratatui::buffer::Buffer, area: Rect, screen: Rect) {
        let src = GridSource(&*self.grid);
        let opts = TableOptions {
            title: self.title(),
            empty_label: CoreMsg::SheetEmptyWord.tr(self.lang).to_string(),
            rainbow: self.rainbow,
            gutter: self.addressed,
            is_hit: &|_, _| false,
        };
        let mut state = self.state;
        if render_table(buf, area, &src, &mut state, &opts) {
            self.state = state;
        }
        let Some(view) = self.popup.is_some().then(|| self.cell()).flatten() else {
            self.popup = None;
            return;
        };
        let Some(p) = self.popup.as_mut() else {
            return;
        };
        let title = konoma_widgets::table::cell_popup_title(
            CoreMsg::TableCellTitle.tr(self.lang),
            &view.header,
            (view.row, view.col),
            (view.nrows, view.ncols),
        );
        let (scroll, viewport) = render_cell_popup(
            buf,
            screen,
            CellPopup {
                title,
                footer: p.footer.clone(),
                text: view.text,
                scroll: p.scroll,
            },
        );
        p.scroll = scroll;
        p.viewport = viewport;
    }
}

/// The footer: the keys first (a narrow terminal cuts the end), where the text is (only for a
/// frame that scrolls: `scrolls`), the title.
pub fn footer(
    hints: &[String],
    title: &str,
    frame: &Frame,
    scroll: usize,
    scrolls: bool,
) -> String {
    let pos = match frame {
        Frame::Lines(l) if scrolls && !l.is_empty() => format!("  line {}", scroll + 1),
        Frame::Text(t) if scrolls && !t.lines.is_empty() => format!("  line {}", scroll + 1),
        // A document says where it is on its frame (the scroll label), as the host does.
        _ => String::new(),
    };
    format!(" {}{pos}  {title}", hints.join("  "))
}

/// The part of `area` the body is drawn in: all of it but the footer row.
pub fn body_of(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
}

/// `bg` is `[ui.theme] bg`: painted under everything first, as the host does (a span without a
/// background keeps it; `None` leaves the terminal's own).
pub fn draw(
    f: &mut ratatui::Frame,
    frame: &mut Frame,
    footer: &str,
    bg: Option<ratatui::style::Color>,
) {
    let area = f.area();
    if let Some(bg) = bg {
        f.buffer_mut().set_style(area, RStyle::default().bg(bg));
    }
    if area.height == 0 {
        return;
    }
    let body = body_of(area);
    match frame {
        Frame::Lines(lines) => {
            let text: Vec<RLine> = lines.iter().map(ratatui_draw::line).collect();
            Paragraph::new(text).render(body, f.buffer_mut());
        }
        Frame::Text(t) => {
            let text: Vec<RLine> = t.lines.iter().map(ratatui_draw::line).collect();
            let mut para = Paragraph::new(text);
            if t.wrap {
                para = para.wrap(ratatui::widgets::Wrap { trim: false });
            }
            para.scroll((0, t.hscroll)).render(body, f.buffer_mut());
        }
        Frame::Canvas(c) => ratatui_draw::paint_canvas(f.buffer_mut(), body, c),
        Frame::Document(d) => d.draw(f.buffer_mut(), body),
        Frame::Image(i) => Block::bordered()
            .title(i.title())
            .render(body, f.buffer_mut()),
        Frame::ImageFailed(i) => {
            let block = Block::bordered().title(frame_title(&i.name));
            let inner = block.inner(body);
            block.render(body, f.buffer_mut());
            Paragraph::new(super::still::failure_line(
                i.why.as_ref(),
                i.lang,
                &i.plugin_id,
            ))
            .render(inner, f.buffer_mut());
        }
        Frame::Table(t) => {
            let screen = f.area();
            t.draw(f.buffer_mut(), body, screen)
        }
    }
    let foot = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    Paragraph::new(footer.to_string())
        .style(RStyle::default().add_modifier(RModifier::REVERSED))
        .render(foot, f.buffer_mut());
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::style::Style;

    #[test]
    fn a_canvas_is_clipped_to_its_area_and_touches_nothing_outside_it() {
        let mut c = CanvasView::new(6, 5);
        for y in 0..5 {
            c.set_string(0, y, "xxxxxx", Style::new());
        }
        // The area is smaller than the canvas and sits inside a bigger buffer, so a cell that
        // is not clipped would land on the buffer (the rows and columns around the area).
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 8));
        ratatui_draw::paint_canvas(&mut buf, Rect::new(1, 1, 3, 2), &c);
        for y in 0..8u16 {
            for x in 0..8u16 {
                let inside = (1..4).contains(&x) && (1..3).contains(&y);
                let want = if inside { "x" } else { " " };
                assert_eq!(buf[(x, y)].symbol(), want, "cell {x},{y}");
            }
        }
    }
}
