//! Value-based media diff drawing. Hosts own workers, caches and translations.
use super::{layout, MediaDiffGeometry, MediaDiffLayout, MediaDiffOrientation, MediaDiffPane};
use crate::diff_frame::DiffTitleValues;
use crate::picture_overlay::{PicturePaintKind, PicturePaintRequest};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Stylize,
    widgets::{Block, Paragraph, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Adopted picture metadata, without decoded pixels or an SDK dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaSideView {
    Picture {
        key: String,
        natural_px: (u32, u32),
        bytes: u64,
        page_count: Option<u32>,
    },
    Absent,
    Failed {
        reason: String,
    },
    PageMissing,
}

impl MediaSideView {
    /// Intrinsic size, never the dimensions of a decoded or encoded raster.
    pub fn natural_px(&self) -> Option<(u32, u32)> {
        match self {
            Self::Picture { natural_px, .. } => Some(*natural_px),
            _ => None,
        }
    }
}

/// Display form of a pending or adopted outcome. The SDK adapter resolves summary text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaDiffRenderState {
    Centered(String),
    Ready {
        same_bytes: bool,
        old: MediaSideView,
        new: MediaSideView,
    },
}

/// All user-facing words are supplied by the host.
#[derive(Clone, Copy, Debug)]
pub struct MediaDiffWords<'a> {
    pub before: &'a str,
    pub after: &'a str,
    pub identical: &'a str,
    pub absent_old: &'a str,
    pub absent_new: &'a str,
    pub cannot_display: &'a str,
    pub page_missing: &'a str,
}

pub struct MediaDiffDrawValues<'a> {
    pub state: &'a MediaDiffRenderState,
    pub title: DiffTitleValues<'a>,
    pub cell_px: (u32, u32),
    /// Independent of the line diff's unified/split setting and Markdown gutter.
    pub layout: MediaDiffLayout,
    pub page: u32,
    pub old_label: &'a str,
    pub new_label: &'a str,
    pub words: MediaDiffWords<'a>,
    pub format_size: fn(u64) -> String,
}

/// Draw the shared frame, captions, separator and sides. Picture preparation and protocol
/// rendering use the same resolved paint boundary as inline document pictures.
pub fn draw_media_diff(
    buf: &mut Buffer,
    area: Rect,
    values: MediaDiffDrawValues<'_>,
    mut painter: impl FnMut(&str, PicturePaintRequest, &mut Buffer) -> bool,
) {
    let inner = Block::bordered().inner(area);
    Block::bordered()
        .title(values.title.title(false))
        .render(area, buf);
    match values.state {
        MediaDiffRenderState::Centered(text) => draw_media_centered(buf, inner, text),
        MediaDiffRenderState::Ready {
            same_bytes,
            old,
            new,
        } => {
            let geom = layout(
                old.natural_px(),
                new.natural_px(),
                values.cell_px,
                inner,
                values.layout,
            );
            draw_media_caption(
                buf,
                geom.old.caption,
                old,
                MediaCaptionCtx {
                    label: values.old_label,
                    heading: values.words.before,
                    identical: false,
                },
                &values,
            );
            draw_media_caption(
                buf,
                geom.new.caption,
                new,
                MediaCaptionCtx {
                    label: values.new_label,
                    heading: values.words.after,
                    identical: *same_bytes,
                },
                &values,
            );
            draw_media_separator(buf, &geom);
            draw_media_side(
                buf,
                &geom.old,
                old,
                values.words.absent_old,
                values.words,
                &mut painter,
            );
            draw_media_side(
                buf,
                &geom.new,
                new,
                values.words.absent_new,
                values.words,
                &mut painter,
            );
        }
    }
}

fn draw_media_centered(buf: &mut Buffer, inner: Rect, text: &str) {
    if inner.height == 0 {
        return;
    }
    Paragraph::new(text).alignment(Alignment::Center).render(
        Rect {
            x: inner.x,
            y: inner.y + inner.height / 2,
            width: inner.width,
            height: 1,
        },
        buf,
    );
}

struct MediaCaptionCtx<'a> {
    heading: &'a str,
    label: &'a str,
    identical: bool,
}

fn draw_media_caption(
    buf: &mut Buffer,
    rect: Rect,
    side: &MediaSideView,
    ctx: MediaCaptionCtx<'_>,
    values: &MediaDiffDrawValues<'_>,
) {
    if rect.height == 0 || rect.width == 0 {
        return;
    }
    let mut parts = vec![ctx.heading.to_string(), ctx.label.to_string()];
    if let MediaSideView::Picture {
        natural_px,
        bytes,
        page_count,
        ..
    } = side
    {
        parts.push(format!("{}×{}", natural_px.0, natural_px.1));
        parts.push((values.format_size)(*bytes));
        if let Some(pc) = page_count {
            parts.push(format!("p. {}/{pc}", values.page));
        }
    }
    if ctx.identical {
        parts.push(values.words.identical.to_string());
    }
    let text = truncate_display(&parts.join(" · "), rect.width as usize);
    Paragraph::new(text).dim().render(rect, buf);
}

// Preserve the host's display-width truncation and ellipsis, including narrow CJK captions.
fn truncate_display(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > max - 1 {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

fn draw_media_separator(buf: &mut Buffer, geom: &MediaDiffGeometry) {
    let rect = geom.separator;
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let ch = match geom.orientation {
        MediaDiffOrientation::Side => "│",
        MediaDiffOrientation::Stack => "─",
    };
    let line = ch.repeat(rect.width as usize);
    for y in rect.y..rect.y + rect.height {
        Paragraph::new(line.clone()).dim().render(
            Rect {
                x: rect.x,
                y,
                width: rect.width,
                height: 1,
            },
            buf,
        );
    }
}

fn draw_media_side(
    buf: &mut Buffer,
    pane: &MediaDiffPane,
    side: &MediaSideView,
    absent: &str,
    words: MediaDiffWords<'_>,
    painter: &mut impl FnMut(&str, PicturePaintRequest, &mut Buffer) -> bool,
) {
    match side {
        MediaSideView::Picture { key, .. } => {
            let rect = pane.image;
            if rect.width == 0 || rect.height == 0 {
                return;
            }
            let request = PicturePaintRequest {
                target: rect,
                cols: rect.width,
                rows: rect.height,
                row_offset: 0,
                visible_rows: rect.height,
                kind: PicturePaintKind::Normal,
            };
            crate::picture_overlay::note_frame_paint(key, request);
            painter(key, request, buf);
        }
        MediaSideView::Absent => draw_media_centered(buf, pane.area, absent),
        MediaSideView::Failed { reason } => draw_media_centered(
            buf,
            pane.area,
            &format!("{}: {reason}", words.cannot_display),
        ),
        MediaSideView::PageMissing => draw_media_centered(buf, pane.area, words.page_missing),
    }
}

fn quantize_px(px: u32, step: u32) -> u32 {
    px.div_ceil(step).max(1) * step
}

/// Raster upper bound before intrinsic sizes are known. Quantization avoids re-rasterizing
/// vectors on every small viewport change; conversions and rounding match the original host.
pub fn media_raster_box(inner: Rect, cell_px: (u32, u32)) -> (u32, u32) {
    let (cw, ch) = cell_px;
    let w = quantize_px((inner.width as u32).max(1) * cw.max(1), 64);
    let h = quantize_px((inner.height as u32).max(1) * ch.max(1), 64);
    (w, h)
}
