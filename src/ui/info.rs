//! Centered popup for file info (`i`). Shows kind/size/modified/permissions/symlink target/path.
//! Follows the help/bookmarks popups. Timestamps are shown as UTC (absolute) + relative (no added dependency).

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::fileops;
use crate::i18n::tr;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(path) = app.info_target() else {
        return;
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string();

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(name).bold());
    lines.push(Line::from(""));

    // ラベル: 値 の1行を作る。
    let row = |label: &str, value: String| -> Line<'static> {
        Line::from(vec![
            Span::from(format!("{label:<10}")).fg(Color::Cyan),
            Span::from(value),
        ])
    };

    match fileops::file_info(&path) {
        Ok(fi) => {
            let kind = if fi.is_symlink {
                tr(app.lang, crate::i18n::Msg::Symlink)
            } else if fi.is_dir {
                tr(app.lang, crate::i18n::Msg::InfoDirectory)
            } else {
                tr(app.lang, crate::i18n::Msg::InfoFile)
            };
            lines.push(row(
                tr(app.lang, crate::i18n::Msg::InfoType),
                kind.to_string(),
            ));

            // サイズ(ディレクトリは項目数も)。
            let size = match fi.child_count {
                Some(n) => format!(
                    "{}  ({} {})",
                    fileops::human_size(fi.size),
                    n,
                    tr(app.lang, crate::i18n::Msg::InfoItems)
                ),
                None => fileops::human_size(fi.size),
            };
            lines.push(row(tr(app.lang, crate::i18n::Msg::InfoSize), size));

            // 更新日時(UTC 絶対 ＋ 相対)。
            if let Some(epoch) = fi.modified_epoch {
                let abs = fileops::format_epoch_utc(epoch);
                let rel = now_epoch().map(|now| format_ago(app.lang, now.saturating_sub(epoch)));
                let val = match rel {
                    Some(r) => format!("{abs}  ({r})"),
                    None => abs,
                };
                lines.push(row(tr(app.lang, crate::i18n::Msg::InfoModified), val));
            }

            lines.push(row(
                tr(app.lang, crate::i18n::Msg::InfoPerm),
                fileops::permission_string(fi.mode),
            ));

            if let Some(target) = &fi.symlink_target {
                lines.push(row(
                    tr(app.lang, crate::i18n::Msg::InfoTarget),
                    target.display().to_string(),
                ));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(format!("  {}", app.format_path(&path))).dim());
        }
        Err(e) => {
            lines.push(
                Line::from(format!("{}: {e}", tr(app.lang, crate::i18n::Msg::Failed)))
                    .fg(Color::Red),
            );
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(tr(app.lang, crate::i18n::Msg::InfoClose)).dim());

    // ポップアップ(bookmarks を踏襲)。
    let w = 72.min(area.width.saturating_sub(2)).max(28);
    let h = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(5);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let block = Block::bordered()
        .title(tr(app.lang, crate::i18n::Msg::InfoTitle))
        .border_style(Style::new().fg(Color::DarkGray));
    let para = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(Clear, popup);
    frame.render_widget(para, popup);
}

/// The current UNIX epoch seconds (None on failure to obtain).
fn now_epoch() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Turn elapsed seconds into a relative expression ("just now"/"N min ago"/…). Pure function (testable).
fn format_ago(lang: crate::i18n::Lang, secs: u64) -> String {
    let (n, msg) = if secs < 60 {
        return tr(lang, crate::i18n::Msg::JustNow).to_string();
    } else if secs < 3600 {
        (secs / 60, crate::i18n::Msg::AgoMin)
    } else if secs < 86400 {
        (secs / 3600, crate::i18n::Msg::AgoHr)
    } else if secs < 86400 * 30 {
        (secs / 86400, crate::i18n::Msg::AgoDays)
    } else if secs < 86400 * 365 {
        (secs / (86400 * 30), crate::i18n::Msg::AgoMonths)
    } else {
        (secs / (86400 * 365), crate::i18n::Msg::AgoYears)
    };
    format!("{n} {}", tr(lang, msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn format_ago_buckets() {
        assert_eq!(format_ago(Lang::En, 10), "just now");
        assert_eq!(format_ago(Lang::En, 120), "2 min ago");
        assert_eq!(format_ago(Lang::En, 7200), "2 hr ago");
        assert_eq!(format_ago(Lang::En, 86400 * 3), "3 days ago");
        // months バケット(30日〜1年): 60日 → 2 months。
        assert_eq!(format_ago(Lang::En, 86400 * 60), "2 months ago");
        assert_eq!(format_ago(Lang::En, 86400 * 400), "1 years ago");
    }
}
