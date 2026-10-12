//! Tab stops of a paragraph (part of [`super`], the text layout).
//!
//! A tab character advances to the next tab stop. The stops are, in order of precedence: the
//! paragraph's explicit stops (`a:tabLst`), the hanging-indent position (`marL` when `indent` is
//! negative: PowerPoint and LibreOffice both treat it as a stop, which is how a paragraph that
//! starts with a tab and a negative indent lines up with its second line), and the default stops
//! at multiples of `defTabSz` after the last explicit one. Positions are measured from the left
//! edge of the text rectangle.
//!
//! Left (and decimal) stops need only the cursor position; centre and right stops also need the
//! width of the text up to the next tab, so a finished line is fixed up by [`fix_line`] (the line
//! breaker, which has not placed the text after the tab yet, assumes left stops).

use super::{emu_px, Atom, Kind};
use crate::preview::office::slide_draw::model::{Paragraph, TabAlign, DEFAULT_TAB_EMU};

/// A tab stop closer than this (px) to the cursor is passed over.
const EPS: f64 = 0.5;
/// Most explicit stops used (a forged `tabLst`).
pub(super) const MAX_STOPS: usize = 64;

/// The tab stops of one paragraph in pixels.
#[derive(Debug, Clone)]
pub(super) struct TabGeom {
    /// Explicit and hanging stops, sorted by position.
    stops: Vec<(f64, TabAlign)>,
    /// Distance of the default stops, px.
    def: f64,
}

impl TabGeom {
    pub(super) fn of(p: &Paragraph, mar_l_px: f64) -> TabGeom {
        let mut stops: Vec<(f64, TabAlign)> = p
            .tabs
            .iter()
            .take(MAX_STOPS)
            .map(|t| (emu_px(t.pos), t.align))
            .filter(|(x, _)| *x >= 0.0)
            .collect();
        if p.indent < 0.0 && mar_l_px > 0.0 {
            stops.push((mar_l_px, TabAlign::Left));
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let def_emu = if p.def_tab.is_finite() && p.def_tab > 0.0 {
            p.def_tab
        } else {
            DEFAULT_TAB_EMU
        };
        TabGeom {
            stops,
            // at least 4 px so a forged tiny `defTabSz` cannot produce a flood of stops
            def: emu_px(def_emu).max(4.0),
        }
    }

    /// The stop a tab at `pos` goes to.
    fn next(&self, pos: f64) -> (f64, TabAlign) {
        if let Some(s) = self.stops.iter().find(|s| s.0 > pos + EPS) {
            return *s;
        }
        let k = ((pos + EPS) / self.def).floor() + 1.0;
        (k * self.def, TabAlign::Left)
    }
}

/// Sets the width of the tab atoms of `atoms`, which start at `x` (px), as if every stop were a
/// left stop. The widths of the other atoms are as they are.
pub(super) fn resolve_left(atoms: &mut [Atom], x: f64, g: &TabGeom) {
    let mut pos = x;
    for a in atoms.iter_mut() {
        if a.tab {
            a.width = (g.next(pos).0 - pos).max(0.0);
        }
        pos += a.width;
    }
}

/// Sets the width of the tab atoms of one finished line that starts at `x0` (px), honouring
/// centre and right stops.
pub(super) fn fix_line(atoms: &mut [Atom], x0: f64, g: &TabGeom) {
    let mut pos = x0;
    for i in 0..atoms.len() {
        if atoms[i].tab {
            let (stop, align) = g.next(pos);
            let w = match align {
                TabAlign::Left | TabAlign::Decimal => stop - pos,
                TabAlign::Center | TabAlign::Right => {
                    let end = atoms[i + 1..]
                        .iter()
                        .position(|a| a.tab || a.kind == Kind::Break)
                        .map_or(atoms.len(), |n| i + 1 + n);
                    let seg = &atoms[i + 1..end];
                    let keep = seg
                        .iter()
                        .rposition(|a| a.kind != Kind::Space)
                        .map_or(0, |n| n + 1);
                    let text: f64 = seg[..keep].iter().map(|a| a.width).sum();
                    if align == TabAlign::Center {
                        stop - text / 2.0 - pos
                    } else {
                        stop - text - pos
                    }
                }
            };
            atoms[i].width = w.max(0.0);
        }
        pos += atoms[i].width;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{layout, Frag, Layout};
    use super::{TabGeom, MAX_STOPS};
    use crate::preview::office::slide_draw::model::*;

    fn para(text: &str, tabs: Vec<TabStop>) -> Paragraph {
        Paragraph {
            runs: vec![Run::text(text, 20.0)],
            tabs,
            ..Default::default()
        }
    }

    fn lay(p: Paragraph, w: f64) -> Layout {
        layout(
            &TextBody {
                paragraphs: vec![p],
                ..Default::default()
            },
            w,
            400.0,
        )
    }

    fn frag<'a>(l: &'a Layout, text: &str) -> &'a Frag {
        l.lines
            .iter()
            .flat_map(|l| &l.frags)
            .find(|f| f.text == text)
            .unwrap_or_else(|| panic!("no frag {text:?} in {:?}", l.lines))
    }

    const IN: f64 = 96.0; // px per inch

    #[test]
    fn a_tab_goes_to_the_next_default_stop() {
        let l = lay(para("a\tb", vec![]), 600.0);
        assert!((frag(&l, "b").x - IN).abs() < 0.01, "{:?}", frag(&l, "b"));
        // the tab is a fragment of its own whose width reaches the stop
        let t = frag(&l, " ");
        assert!((t.x + t.width - IN).abs() < 0.01);
    }

    #[test]
    fn two_tabs_go_two_stops_and_text_past_a_stop_goes_to_the_next() {
        let l = lay(para("a\t\tb", vec![]), 600.0);
        assert!((frag(&l, "b").x - 2.0 * IN).abs() < 0.01);
        let long = "x".repeat(12); // wider than one inch at 20 pt
        let l = lay(para(&format!("{long}\tb"), vec![]), 600.0);
        assert!(
            (frag(&l, "b").x - 2.0 * IN).abs() < 0.01,
            "{:?}",
            frag(&l, "b")
        );
    }

    #[test]
    fn a_leading_tab_moves_to_the_first_stop() {
        let l = lay(para("\tb", vec![]), 600.0);
        assert!((frag(&l, "b").x - IN).abs() < 0.01);
    }

    #[test]
    fn explicit_stops_come_first_and_default_stops_follow_the_last_one() {
        let stops = vec![TabStop {
            pos: 200_000.0,
            align: TabAlign::Left,
        }];
        let l = lay(para("a\tb", stops.clone()), 600.0);
        assert!((frag(&l, "b").x - 200_000.0 / 9525.0).abs() < 0.01);
        // the second tab is past the last explicit stop: the next default stop
        let l = lay(para("a\t\tb", stops), 600.0);
        assert!((frag(&l, "b").x - IN).abs() < 0.01, "{:?}", frag(&l, "b"));
    }

    #[test]
    fn a_custom_default_distance_applies() {
        let p = Paragraph {
            def_tab: 457_200.0,
            ..para("a\tb", vec![])
        };
        assert!((frag(&lay(p, 600.0), "b").x - 48.0).abs() < 0.01);
    }

    #[test]
    fn a_forged_tiny_default_distance_neither_hangs_nor_floods() {
        let p = Paragraph {
            def_tab: 1.0,
            ..para("a\t\t\tb", vec![])
        };
        let l = lay(p, 600.0);
        let b = frag(&l, "b");
        assert!(b.x.is_finite() && b.x < 100.0, "{b:?}");
    }

    #[test]
    fn the_hanging_indent_position_is_a_stop() {
        // marL 36 px, indent -36 px: the first line starts at 0, a tab goes to 36 (the second
        // line's start), not to the one-inch stop.
        let p = Paragraph {
            mar_l: 342_900.0,
            indent: -342_900.0,
            ..para("\tb", vec![])
        };
        let l = lay(p, 600.0);
        assert!((frag(&l, "b").x - 36.0).abs() < 0.01, "{:?}", frag(&l, "b"));
        // without a hanging indent (indent 0) marL is no stop
        let p = Paragraph {
            mar_l: 342_900.0,
            indent: 0.0,
            ..para("\tb", vec![])
        };
        assert!((frag(&lay(p, 600.0), "b").x - IN).abs() < 0.01);
    }

    #[test]
    fn centre_and_right_stops_align_the_text_after_the_tab() {
        let stop = |align| {
            vec![TabStop {
                pos: 1_828_800.0, // 192 px
                align,
            }]
        };
        let l = lay(para("a\tbb", stop(TabAlign::Right)), 600.0);
        let b = frag(&l, "bb");
        assert!((b.x + b.width - 2.0 * IN).abs() < 0.5, "{b:?}");
        let l = lay(para("a\tbb", stop(TabAlign::Center)), 600.0);
        let b = frag(&l, "bb");
        assert!((b.x + b.width / 2.0 - 2.0 * IN).abs() < 0.5, "{b:?}");
        // decimal acts as left
        let l = lay(para("a\tbb", stop(TabAlign::Decimal)), 600.0);
        assert!((frag(&l, "bb").x - 2.0 * IN).abs() < 0.5);
        // text wider than the room before a right stop starts right after the previous text
        let long = "m".repeat(30);
        let l = lay(para(&format!("a\t{long}"), stop(TabAlign::Right)), 2000.0);
        let t = frag(&l, " ");
        assert!(t.width >= 0.0 && t.width < 1.0, "{t:?}");
    }

    #[test]
    fn wrapping_still_works_with_a_tab_in_the_text() {
        let words = "word ".repeat(30);
        let l = lay(para(&format!("{words}\tend"), vec![]), 200.0);
        assert!(l.lines.len() > 2);
        for line in &l.lines {
            let right = line.frags.iter().map(|f| f.x + f.width).fold(0.0, f64::max);
            assert!(right <= 200.5, "{right} {:?}", line.frags);
        }
    }

    #[test]
    fn many_explicit_stops_are_bounded() {
        let stops: Vec<TabStop> = (0..500)
            .map(|i| TabStop {
                pos: 10_000.0 * f64::from(i),
                align: TabAlign::Left,
            })
            .collect();
        let g = TabGeom::of(&para("", stops), 0.0);
        assert!(g.stops.len() <= MAX_STOPS);
    }

    #[test]
    fn a_nan_default_distance_falls_back_to_one_inch() {
        let p = Paragraph {
            def_tab: f64::NAN,
            ..para("a\tb", vec![])
        };
        assert!((frag(&lay(p, 600.0), "b").x - IN).abs() < 0.01);
    }
}
