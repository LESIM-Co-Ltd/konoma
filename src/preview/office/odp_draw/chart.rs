//! Charts: a `draw:object` of a frame whose package part (`Object N/content.xml`) is an
//! OpenDocument chart is drawn as the shapes and text of the chart (see [`super::chart_read`]
//! for what is read of it and `slide_draw::chart` for how it is drawn), in a group at the frame.
//!
//! * The part is read through the document's chart read budget (shared with the text view's
//!   search for the chart's title): [`CHART_PART_BYTES`] at most per part, `styles.xml` of the
//!   object likewise.
//! * An object that is not a chart (a formula, a spreadsheet, a chart class that is not drawn:
//!   stock, gantt, surface) is drawn as the replacement picture LibreOffice stores for it
//!   (`ObjectReplacements/Object N`, which has no file extension: its kind is told by its first
//!   bytes; PNG, JPEG, GIF, SVG, EMF and WMF are drawn, LibreOffice's own `svm` is not), else as
//!   the placeholder.
//! * A chart that is drawn replaces the frame's picture: the stored replacement is not drawn
//!   under or over it.

use crate::preview::office::slide_draw as sd;

use super::chart_read;
use super::*;

/// Most bytes of one chart part (`content.xml` or `styles.xml` of the object) that are read.
pub(super) const CHART_PART_BYTES: u64 = 512 * 1024;
/// Longest object directory name followed.
const OBJECT_DIR_MAX: usize = 512;

impl<'a> Sb<'a> {
    /// The chart of a frame's `draw:object` as scene items; whether it was drawn (the frame's
    /// replacement picture is then not).
    pub(super) fn frame_chart(
        &mut self,
        _frame: &'a Node,
        object: &'a Node,
        xf: sd::Xfrm,
        out: &mut Vec<sd::Item>,
    ) -> bool {
        let Some(dir) = object.attr("href").and_then(part_of) else {
            return false;
        };
        if dir.len() > OBJECT_DIR_MAX {
            return false;
        }
        let dir = dir.trim_end_matches('/').to_string();
        let content = match self
            .media
            .read_part(&format!("{dir}/content.xml"), CHART_PART_BYTES)
        {
            PartRead::Bytes(b) => b,
            PartRead::Over => {
                self.truncated = true;
                return false;
            }
            PartRead::Missing => return false,
        };
        let Some(root) = chart_read::read_tree(&content) else {
            // (Not XML, or too large an element tree.)
            if content.len() as u64 > 1024 {
                self.truncated = true;
            }
            return false;
        };
        let styles = match self
            .media
            .read_part(&format!("{dir}/styles.xml"), CHART_PART_BYTES)
        {
            PartRead::Bytes(b) => chart_read::read_tree(&b),
            PartRead::Over => {
                self.truncated = true;
                None
            }
            PartRead::Missing => None,
        };
        let Some(parsed) = chart_read::parse(&root, styles.as_ref()) else {
            return false;
        };
        let (mut items, cut) = sd::chart::draw_chart(&parsed.model, xf.w, xf.h);
        self.truncated |= cut || parsed.truncated;
        let room = self.max_items.saturating_sub(self.items);
        if items.len() > room {
            items.truncate(room);
            self.truncated = true;
        }
        self.items += items.len();
        if items.is_empty() {
            return true;
        }
        out.push(sd::Item::Group(sd::GroupItem {
            xfrm: sd::Xfrm::rect(xf.x, xf.y, xf.w, xf.h),
            child_off: (0.0, 0.0),
            child_ext: (xf.w, xf.h),
            items,
        }));
        true
    }
}
