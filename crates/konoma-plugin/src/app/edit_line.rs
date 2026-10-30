//! The line `e` opens the editor at, for the text or the document on screen.
//!
//! The loop publishes what the line is computed from after each frame (cheap data), and
//! [`edit_line`] computes it when asked, by the one function the konoma host uses
//! (`konoma_widgets::caret`): the caret line of a text, a line estimated from the top of a
//! rendered Markdown document and corrected by its content. A converted document, a message, a
//! table and a picture have none. The launch of the editor is not here: this is the line it
//! would take.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use super::document::Laid;

/// What the line is computed from.
#[derive(Clone, Default)]
enum Target {
    /// The screen has no line to open at.
    #[default]
    None,
    /// The file's text, the caret on this 0-based line.
    Text { caret_line: usize },
    /// A Markdown file shown as a document, scrolled `scroll` display rows.
    Document {
        laid: Arc<Laid>,
        scroll: usize,
        wrap: bool,
        /// The lines of the source the document was made from.
        src_lines: usize,
        /// The file the document was read from (the content refinement reads it again).
        path: PathBuf,
    },
}

thread_local! {
    static TARGET: RefCell<Target> = const { RefCell::new(Target::None) };
}

/// Forget the line of a screen that is gone.
pub(super) fn clear() {
    TARGET.with(|t| *t.borrow_mut() = Target::None);
}

/// The file's text with its caret on `caret_line` (0-based) is on screen.
pub(super) fn publish_text(caret_line: usize) {
    TARGET.with(|t| *t.borrow_mut() = Target::Text { caret_line });
}

/// A Markdown document read from `path` is on screen (`None`: a converted one has no line).
pub(super) fn publish_document(
    laid: &Arc<Laid>,
    scroll: usize,
    wrap: bool,
    source: Option<(usize, PathBuf)>,
) {
    TARGET.with(|t| {
        *t.borrow_mut() = match source {
            Some((src_lines, path)) => Target::Document {
                laid: Arc::clone(laid),
                scroll,
                wrap,
                src_lines,
                path,
            },
            None => Target::None,
        }
    });
}

/// The 1-based line `e` opens the file at, for the screen the loop last drew on this thread; None
/// when the screen has none (a converted document, generated text, a picture, a message).
pub fn edit_line() -> Option<usize> {
    let target = TARGET.with(|t| t.borrow().clone());
    match target {
        Target::None => None,
        Target::Text { caret_line } => Some(konoma_widgets::caret::text_edit_line(caret_line)),
        Target::Document {
            laid,
            scroll,
            wrap,
            src_lines,
            path,
        } => {
            let view = konoma_widgets::caret::MdView {
                scroll,
                view_rows: laid.rows(),
                src_lines,
                lines: &laid.lines,
                row_prefix: &laid.row_prefix,
                wrap,
                width_known: true,
                focus: None,
            };
            konoma_widgets::caret::md_edit_line(&view, &|| {
                let text = std::fs::read_to_string(&path).ok()?;
                Some(text.lines().map(str::to_string).collect())
            })
        }
    }
}
