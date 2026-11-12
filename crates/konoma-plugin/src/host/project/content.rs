//! Prepare project display data on the session owner, using the shared widgets.

use super::{ProjectDiff, ProjectMessages};
use crate::view::{TextSource, View, ViewReq};
use crate::{Failure, ReadScope, Tr};
use ratatui::text::Line;

pub enum ProjectContent {
    List(crate::view::ListView),
    Lines(Vec<Line<'static>>),
    Diff(ProjectDiff),
}

/// The same text, Markdown and diff widgets the file previews use, prepared off the UI thread.
pub fn content(
    view: View,
    req: &ViewReq,
    scope: &ReadScope,
    theme: &str,
    messages: &ProjectMessages,
) -> Result<ProjectContent, Failure> {
    use crate::ratatui_draw::into_line;
    use konoma_widgets::markdown as md;
    Ok(match view {
        View::List(list) => ProjectContent::List(list),
        View::Text(mut text) => {
            let lines = match text.source {
                TextSource::Lines(lines) => lines.into_iter().map(into_line).collect(),
                source => {
                    let raw = match source {
                        TextSource::Owned(text) => text,
                        TextSource::File(path) => {
                            if !scope.allows(&path) {
                                return Err(Failure::unsupported(Tr::new(
                                    messages.outside_read_dirs.clone(),
                                )));
                            }
                            std::fs::read_to_string(path)?
                        }
                        TextSource::Lines(_) => unreachable!(),
                    };
                    match text.styler.as_mut() {
                        Some(styler) => styler
                            .style_block(0, &raw)
                            .into_iter()
                            .map(into_line)
                            .collect(),
                        None => raw
                            .lines()
                            .map(|l| ratatui::text::Line::raw(l.to_string()))
                            .collect(),
                    }
                }
            };
            ProjectContent::Lines(lines)
        }
        View::Diff(diff) => {
            let mut previous = None;
            let lines: Vec<_> = diff
                .hunks
                .into_iter()
                .flat_map(|h| {
                    let marker = if h.path != previous {
                        h.path.as_ref().map(|path| crate::view::DiffLine {
                            kind: crate::view::DiffKind::Context,
                            old_no: None,
                            new_no: None,
                            text: path.clone(),
                        })
                    } else {
                        None
                    };
                    previous = h.path;
                    marker.into_iter().chain(h.lines)
                })
                .map(|line| konoma_vcs_core::git::DiffLine {
                    kind: match line.kind {
                        crate::view::DiffKind::Added => konoma_vcs_core::git::DiffLineKind::Added,
                        crate::view::DiffKind::Removed => {
                            konoma_vcs_core::git::DiffLineKind::Removed
                        }
                        _ => konoma_vcs_core::git::DiffLineKind::Context,
                    },
                    old_no: line.old_no.and_then(|n| n.try_into().ok()),
                    new_no: line.new_no.and_then(|n| n.try_into().ok()),
                    text: line.text,
                })
                .collect();
            ProjectContent::Diff(
                ProjectDiff::new(
                    lines,
                    diff.header.into_iter().map(into_line).collect(),
                    theme.into(),
                    messages.no_changes.as_str().into(),
                )
                .with_title(diff.title.map(into_line)),
            )
        }
        View::Document(doc) => {
            let (lines, _, _) = md::render_markdown_with_images_aligned(
                &doc.markdown,
                req.cols,
                md::CodeStyle::default(),
                theme,
                false,
                &doc.options.task_states,
                &|_, _| md::ImageSlot::Unavailable,
                &|_| md::MermaidSlot::Text,
                messages.mermaid_caption.as_str(),
                doc.options.alerts,
                &|_, _| md::MathSlot::Raw,
                false,
                md::BlockAligns {
                    table: md::BlockAlign::from_config(
                        &doc.options.table_align,
                        md::BlockAlign::Left,
                    ),
                    image: md::BlockAlign::from_config(
                        &doc.options.image_align,
                        md::BlockAlign::Center,
                    ),
                },
            );
            ProjectContent::Lines(lines)
        }
        View::Message(message) => {
            ProjectContent::Lines(message.lines.into_iter().map(into_line).collect())
        }
        _ => {
            return Err(Failure::unsupported(Tr::new(
                messages.view_unsupported.clone(),
            )))
        }
    })
}
