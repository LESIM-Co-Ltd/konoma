//! Inline picture placement and painting policy, without caches, workers or terminal I/O.

use std::hash::{Hash, Hasher};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Widget};

use crate::inline_images::reset_cells_under_picture;
use crate::markdown::{is_mermaid_fence_url, mermaid_focus_border_x, BlockAlign};

/// The geometry and identity of an overlay, independent of pixels and animation frames.
pub struct OverlaySig(std::collections::hash_map::DefaultHasher);

impl OverlaySig {
    pub fn new(inner: Rect) -> Self {
        let mut s = Self(std::collections::hash_map::DefaultHasher::new());
        s.hash((inner.x, inner.y, inner.width, inner.height));
        s
    }

    /// Add a canvas id or an inline URL, retaining the existing canvas signature format.
    pub fn add(&mut self, id: impl Hash, at: Rect) {
        self.hash((id, at.x, at.y, at.width, at.height));
    }

    fn hash(&mut self, value: impl Hash) {
        value.hash(&mut self.0);
    }

    pub fn finish(&self) -> u64 {
        self.0.finish()
    }
}

/// A reserved picture, with its logical row already converted by the host's text layout.
#[derive(Clone, Copy, Debug)]
pub struct InlinePicturePlacement<'a> {
    pub url: &'a str,
    pub visual_row: usize,
    pub col: u16,
    pub cols: u16,
    pub rows: u16,
    /// Source fence ordinal, including upstream loading and text fallback fences.
    pub fence_ord: Option<usize>,
}

/// Host values for one overlay pass. The host translates and formats the focus title.
#[derive(Clone, Copy, Debug)]
pub struct InlineOverlayValues<'a> {
    pub inner: Rect,
    pub scroll: usize,
    pub gutter: u16,
    pub focused_ordinal: Option<usize>,
    pub zoom: f64,
    pub image_align: BlockAlign,
    pub focus_title: &'a str,
}

/// Image preparation requested by the overlay. Density prepares pixels without painting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicturePaintKind {
    Density,
    Zoom,
    /// The freshest full image while a zoom crop is being encoded.
    UnzoomedFallback,
    Normal,
}

/// Fully resolved geometry: painters must use these coordinates and crop band unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicturePaintRequest {
    pub target: Rect,
    pub cols: u16,
    pub rows: u16,
    pub row_offset: u16,
    pub visible_rows: u16,
    pub kind: PicturePaintKind,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static PAINTS: std::cell::RefCell<Option<Vec<(String, PicturePaintRequest)>>> = const { std::cell::RefCell::new(None) };
}

/// Observe the requests of the real shared overlay, without replacing its painter.
#[cfg(any(test, feature = "test-hooks"))]
pub fn record_inline_paints_for_test<T>(
    draw: impl FnOnce() -> T,
) -> (T, Vec<(String, PicturePaintRequest)>) {
    PAINTS.with(|p| *p.borrow_mut() = Some(Vec::new()));
    let result = draw();
    let paints = PAINTS.with(|p| p.borrow_mut().take().unwrap_or_default());
    (result, paints)
}

/// Observe a reservation made by a shared frame outside the inline-document adapter.
pub(crate) fn note_frame_paint(_key: &str, _request: PicturePaintRequest) {
    #[cfg(any(test, feature = "test-hooks"))]
    PAINTS.with(|p| {
        if let Some(paints) = p.borrow_mut().as_mut() {
            paints.push((_key.to_owned(), _request));
        }
    });
}

/// Paint visible reservations and return their signature, even while their protocols are pending.
/// The painter prepares images and draws a protocol, returning whether one was available. A
/// missing zoom protocol causes a normal request here, so both hosts use the same fallback order.
pub fn draw_inline_pictures<'a>(
    buf: &mut Buffer,
    placements: impl IntoIterator<Item = InlinePicturePlacement<'a>>,
    values: InlineOverlayValues<'_>,
    mut painter: impl FnMut(&str, PicturePaintRequest, &mut Buffer) -> bool,
) -> Option<u64> {
    let mut paint = |url: &str, request: PicturePaintRequest, buf: &mut Buffer| {
        #[cfg(any(test, feature = "test-hooks"))]
        PAINTS.with(|p| {
            if let Some(paints) = p.borrow_mut().as_mut() {
                paints.push((url.to_string(), request));
            }
        });
        painter(url, request, buf)
    };
    let InlineOverlayValues {
        inner,
        scroll,
        gutter,
        focused_ordinal,
        zoom,
        image_align,
        focus_title,
    } = values;
    let mut sig = OverlaySig::new(inner);
    sig.hash(focused_ordinal);
    sig.hash((zoom * 1000.0) as u64);
    let mut drawn = 0;
    let top_bound = i32::from(inner.y);
    let bottom_bound = i32::from(inner.y + inner.height);
    for p in placements {
        let mermaid = is_mermaid_fence_url(p.url);
        let focused = mermaid && p.fence_ord.is_some() && focused_ordinal == p.fence_ord;
        let top = top_bound + p.visual_row as i32 - scroll as i32;
        let vis_top = top.max(top_bound);
        let vis_bottom = (top + i32::from(p.rows)).min(bottom_bound);
        if vis_bottom <= vis_top {
            continue;
        }
        let row_offset = (vis_top - top) as u16;
        let visible_rows = (vis_bottom - vis_top) as u16;
        let cols = p.cols.min(inner.width);
        if cols == 0 || visible_rows == 0 {
            continue;
        }
        let target = Rect::new(
            inner.x + gutter + p.col.min(inner.width.saturating_sub(cols)),
            vis_top as u16,
            cols,
            visible_rows,
        );
        // Count the reservation before asking for any protocol: pixels arriving do not move it.
        sig.add(p.url, target);
        drawn += 1;
        // Keep the theme background beneath transparent pixels; discard text SGR attributes.
        reset_cells_under_picture(buf, target);
        if focused {
            let btop = top - 1;
            let bbot = top + i32::from(p.rows) + 1;
            let tw = Span::from(focus_title).width() as u16 + 2;
            let bw = (cols + 2).max(tw).min(inner.width);
            let bx = inner.x + mermaid_focus_border_x(image_align, inner.width, bw);
            if btop >= top_bound && bbot <= bottom_bound {
                Block::bordered()
                    .border_style(Style::new().fg(Color::Cyan))
                    .title(focus_title)
                    .title_style(
                        Style::new()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::REVERSED),
                    )
                    .render(Rect::new(bx, btop as u16, bw, (bbot - btop) as u16), buf);
            }
        }
        let mut request = PicturePaintRequest {
            target,
            cols,
            rows: p.rows,
            row_offset,
            visible_rows,
            kind: PicturePaintKind::Normal,
        };
        if mermaid {
            request.kind = PicturePaintKind::Density;
            paint(p.url, request, buf);
        }
        if focused && zoom > 1.001 && row_offset == 0 && visible_rows >= p.rows {
            request.kind = PicturePaintKind::Zoom;
            if paint(p.url, request, buf) {
                continue;
            }
            request.kind = PicturePaintKind::UnzoomedFallback;
            if paint(p.url, request, buf) {
                continue;
            }
        }
        request.kind = PicturePaintKind::Normal;
        paint(p.url, request, buf);
    }
    (drawn > 0).then(|| sig.finish())
}
