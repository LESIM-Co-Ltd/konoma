// タブバー描画 (M4 / FR-5)。
//
// 複数のツリーコンテキストをタブとして上端に並べる。表示可否は ui::render が
// config `ui.tabbar` ("always" | "auto" | "hidden") を見て決め、ここは描画だけを担う。
// アクティブタブは反転＋太字。各タブは「番号:ディレクトリ名」。
//
// 幅に収まらないときは**アクティブタブを中心に**左右交互へ広げた可視窓だけを描き、
// 端に隠れているタブ数を「‹n / n›」で示す(アクティブが見えなくなる回帰の防止・
// タブ一覧は `T`)。

use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::App;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let active = app.active_tab_index();
    let n = app.tab_count();
    let chips: Vec<String> = (0..n)
        .map(|i| format!(" {}:{} ", i + 1, app.tab_label(i)))
        .collect();
    let widths: Vec<usize> = chips.iter().map(|c| c.width()).collect();
    let (s, e) = visible_range(&widths, active, area.width as usize);

    let mut spans: Vec<Span<'static>> = Vec::new();
    if s > 0 {
        spans.push(Span::from(format!("‹{} ", s)).dim());
    }
    for (k, chip) in chips.into_iter().enumerate().take(e).skip(s) {
        if k > s {
            spans.push(Span::from(" "));
        }
        if k == active {
            spans.push(Span::from(chip).reversed().bold());
        } else {
            spans.push(Span::from(chip).dim());
        }
    }
    if e < n {
        spans.push(Span::from(format!(" {}›", n - e)).dim());
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Width taken by an overflow marker (`‹12 ` / ` 12›`) for `hidden` tabs; 0 when nothing is hidden.
fn marker_width(hidden: usize) -> usize {
    if hidden == 0 {
        0
    } else {
        hidden.to_string().len() + 2
    }
}

/// Pick the visible tab range `[s, e)` for `avail` columns. Everything is shown when it fits;
/// otherwise the window grows from the active tab, alternating sides (≒ centered), reserving
/// room for the `‹n` / `n›` overflow markers of whichever sides stay hidden.
fn visible_range(widths: &[usize], active: usize, avail: usize) -> (usize, usize) {
    let n = widths.len();
    if n == 0 {
        return (0, 0);
    }
    let total: usize = widths.iter().sum::<usize>() + n.saturating_sub(1);
    if total <= avail {
        return (0, n);
    }
    let active = active.min(n - 1);
    let fits = |s: usize, e: usize| -> bool {
        let w: usize = widths[s..e].iter().sum::<usize>()
            + (e - s - 1)
            + marker_width(s)
            + marker_width(n - e);
        w <= avail
    };
    let (mut s, mut e) = (active, active + 1);
    loop {
        // 中央寄せ: 埋まりの少ない側から先に試し、入らなければもう片側、両方駄目なら確定。
        let prefer_right = (e - active) <= (active - s);
        let first = if prefer_right && e < n {
            Some((s, e + 1))
        } else if s > 0 {
            Some((s - 1, e))
        } else if e < n {
            Some((s, e + 1))
        } else {
            None
        };
        let second = if first == Some((s, e + 1)) {
            (s > 0).then(|| (s - 1, e))
        } else {
            (e < n).then(|| (s, e + 1))
        };
        match (first, second) {
            (Some((fs, fe)), _) if fits(fs, fe) => {
                s = fs;
                e = fe;
            }
            (_, Some((ss, se))) if fits(ss, se) => {
                s = ss;
                e = se;
            }
            _ => break,
        }
    }
    (s, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_keeps_active_visible_and_centers() {
        // 全部収まる: そのまま全表示。
        let w = vec![8, 8, 8];
        assert_eq!(visible_range(&w, 1, 80), (0, 3));
        // あふれ: アクティブ(4)を含む窓が中央寄せで選ばれ、両端にマーカー幅が確保される。
        let w = vec![10; 9]; // 各10桁+区切り → 全体98桁
        let (s, e) = visible_range(&w, 4, 40);
        assert!((s..e).contains(&4), "アクティブは必ず可視: {s}..{e}");
        assert!(s > 0 && e < 9, "両側に隠れタブ: {s}..{e}");
        let used: usize =
            w[s..e].iter().sum::<usize>() + (e - s - 1) + marker_width(s) + marker_width(9 - e);
        assert!(used <= 40, "マーカー込みで幅内: {used}");
        // 端: アクティブが先頭なら左マーカー無しで右へ伸びる。
        let (s, e) = visible_range(&w, 0, 40);
        assert_eq!(s, 0);
        assert!(e < 9);
        // 端: アクティブが末尾なら右マーカー無しで左へ伸びる。
        let (s, e) = visible_range(&w, 8, 40);
        assert_eq!(e, 9);
        assert!(s > 0);
        // 極端に狭い: アクティブ1枚だけでも返す(描画側で切れるのは許容)。
        let (s, e) = visible_range(&w, 4, 8);
        assert_eq!((s, e), (4, 5));
    }
}
