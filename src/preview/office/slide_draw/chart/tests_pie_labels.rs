//! Labels outside a pie: the two columns, the spreading of crowded labels, and leader lines that
//! never cross a label's text.

use super::pie::spread;
use super::tests::*;
use super::*;

fn pie_with_labels(vals: &[f64]) -> ChartModel {
    let mut m = pie_model(vals, GroupKind::Pie);
    m.groups[0].series[0].labels = Some(DataLabels {
        show_value: true,
        show_category: true,
        pos: Some(LabelPos::OutsideEnd),
        ..DataLabels::default()
    });
    m
}

/// The circle `(cx, cy, r)` of the first slice's bounding box.
fn circle(items: &[Item]) -> (f64, f64, f64) {
    let s = shapes(items)
        .into_iter()
        .find(|s| s.text.is_none() && s.fill.is_visible())
        .expect("a slice");
    let (x, y, w, h) = (
        s.xfrm.x / EMU_PER_PX,
        s.xfrm.y / EMU_PER_PX,
        s.xfrm.w / EMU_PER_PX,
        s.xfrm.h / EMU_PER_PX,
    );
    (x + w / 2.0, y + h / 2.0, w.min(h) / 2.0)
}

/// Does the segment cross the rectangle `(x0, y0, x1, y1)` (Liang-Barsky)?
fn seg_hits(s: (f64, f64, f64, f64), r: (f64, f64, f64, f64)) -> bool {
    let (dx, dy) = (s.2 - s.0, s.3 - s.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, s.0 - r.0),
        (dx, r.2 - s.0),
        (-dy, s.1 - r.1),
        (dy, r.3 - s.1),
    ] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// The text boxes of the labels `(x0, y0, x1, y1)`, without the 3 px of padding each side.
fn label_boxes(items: &[Item]) -> Vec<(f64, f64, f64, f64)> {
    texts(items)
        .iter()
        .filter(|t| t.0.starts_with('c') || t.0.contains(", "))
        .map(|t| (t.1 .0 + 3.0, t.1 .1, t.1 .0 + t.1 .2 - 3.0, t.1 .1 + t.1 .3))
        .collect()
}

fn data_sets() -> Vec<Vec<f64>> {
    vec![
        vec![40.0, 24.0, 14.0, 9.0, 5.0, 4.0, 3.0, 1.0],
        vec![30.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 62.0],
        vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        vec![90.0, 1.0, 1.0, 1.0, 1.0, 6.0],
        vec![5.0, 3.0, 2.0, 40.0, 50.0],
        vec![2.0, 98.0],
    ]
}

#[test]
fn leader_lines_never_cross_the_text_of_a_label() {
    let gray = Rgba::rgb(0x86, 0x86, 0x86);
    let mut leaders = 0;
    for vals in data_sets() {
        let items = draw(&pie_with_labels(&vals));
        let boxes = label_boxes(&items);
        assert_eq!(boxes.len(), vals.len(), "{vals:?}");
        for s in segs(&items).into_iter().filter(|s| s.4 == gray) {
            leaders += 1;
            for b in &boxes {
                // the label the line leads to touches it at its end: shrink the box a little
                let b = (b.0 + 0.5, b.1 + 0.5, b.2 - 0.5, b.3 - 0.5);
                assert!(
                    !seg_hits((s.0, s.1, s.2, s.3), b),
                    "{vals:?}: {s:?} hits {b:?}"
                );
            }
        }
    }
    assert!(
        leaders >= 10,
        "the crowded sets have leader lines ({leaders})"
    );
}

#[test]
fn leader_lines_of_a_side_do_not_cross_each_other() {
    let gray = Rgba::rgb(0x86, 0x86, 0x86);
    for vals in data_sets() {
        let items = draw(&pie_with_labels(&vals));
        let ls: Vec<_> = segs(&items).into_iter().filter(|s| s.4 == gray).collect();
        for (i, a) in ls.iter().enumerate() {
            for b in &ls[i + 1..] {
                // proper crossing of two segments
                let d = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
                    (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
                };
                let (a0, a1, b0, b1) = ((a.0, a.1), (a.2, a.3), (b.0, b.1), (b.2, b.3));
                let cross =
                    d(a0, a1, b0) * d(a0, a1, b1) < -1e-6 && d(b0, b1, a0) * d(b0, b1, a1) < -1e-6;
                assert!(!cross, "{vals:?}: {a:?} crosses {b:?}");
            }
        }
    }
}

#[test]
fn labels_are_in_two_columns_on_the_side_of_their_slice_and_clear_of_the_pie() {
    for vals in data_sets() {
        let items = draw(&pie_with_labels(&vals));
        let (cx, cy, r) = circle(&items);
        for b in label_boxes(&items) {
            // never over the disc: the nearest point of the box is outside the rim
            let nx = cx.clamp(b.0, b.2);
            let ny = cy.clamp(b.1, b.3);
            let d = ((nx - cx).powi(2) + (ny - cy).powi(2)).sqrt();
            assert!(
                d >= r - 0.5,
                "{vals:?}: {b:?} is {d} from the centre, r {r}"
            );
            // a label belongs to one side of the vertical centre line
            assert!(
                b.0 >= cx - 0.5 || b.2 <= cx + 0.5,
                "{vals:?}: {b:?} straddles {cx}"
            );
            // and stays inside the chart
            assert!(b.1 >= 0.0 && b.3 <= H + 0.5, "{vals:?}: {b:?}");
        }
    }
}

#[test]
fn a_two_slice_pie_has_a_label_on_each_side() {
    let items = draw(&pie_with_labels(&[50.0, 50.0]));
    let (cx, _, _) = circle(&items);
    let boxes = label_boxes(&items);
    assert_eq!(boxes.len(), 2);
    assert!(boxes.iter().any(|b| b.0 >= cx), "{boxes:?}");
    assert!(boxes.iter().any(|b| b.2 <= cx), "{boxes:?}");
}

#[test]
fn crowded_labels_fan_out_around_where_they_wanted_to_be() {
    // Twelve equal slices of 1: the labels on the right side are spread over the height of the
    // arc they belong to, not piled against one end.
    let items = draw(&pie_with_labels(&[1.0; 12]));
    let (cx, cy, r) = circle(&items);
    let right: Vec<_> = label_boxes(&items)
        .into_iter()
        .filter(|b| b.0 >= cx)
        .collect();
    assert_eq!(right.len(), 6);
    let (lo, hi) = right
        .iter()
        .fold((f64::MAX, f64::MIN), |a, b| (a.0.min(b.1), a.1.max(b.3)));
    // their block is centred about the centre line of the pie
    assert!(
        ((lo + hi) / 2.0 - cy).abs() < r * 0.35,
        "{lo} {hi} {cy} {r}"
    );
}

#[test]
fn spread_centres_a_colliding_block_on_its_wishes() {
    let ys = spread(&[50.0, 50.0, 50.0], &[20.0, 20.0, 20.0], 0.0, 100.0);
    assert_eq!(ys, vec![30.0, 50.0, 70.0]);
}

#[test]
fn spread_leaves_apart_boxes_where_they_are() {
    let ys = spread(&[10.0, 50.0, 90.0], &[20.0, 20.0, 20.0], 0.0, 100.0);
    assert_eq!(ys, vec![10.0, 50.0, 90.0]);
}

#[test]
fn spread_keeps_blocks_inside_the_limits() {
    let ys = spread(&[2.0, 2.0, 2.0], &[20.0, 20.0, 20.0], 0.0, 100.0);
    assert_eq!(ys, vec![10.0, 30.0, 50.0]);
    let ys = spread(&[99.0, 99.0, 99.0], &[20.0, 20.0, 20.0], 0.0, 100.0);
    assert_eq!(ys, vec![50.0, 70.0, 90.0]);
    // more than fits: ordered, no overlap, starting at the top limit
    let ys = spread(&[50.0; 6], &[30.0; 6], 0.0, 100.0);
    assert!(
        ys.windows(2).all(|w| (w[1] - w[0] - 30.0).abs() < 1e-9),
        "{ys:?}"
    );
    assert!(spread(&[], &[], 0.0, 100.0).is_empty());
}

#[test]
fn spread_never_overlaps_and_keeps_the_order_for_any_wishes() {
    let mut seed = 12345u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (1u64 << 31) as f64
    };
    for _ in 0..300 {
        let n = 1 + (next() * 9.0) as usize;
        let mut want: Vec<f64> = (0..n).map(|_| next() * 300.0).collect();
        want.sort_by(f64::total_cmp);
        let hs: Vec<f64> = (0..n).map(|_| 8.0 + next() * 16.0).collect();
        let ys = spread(&want, &hs, 0.0, 300.0);
        for i in 1..n {
            assert!(
                ys[i] - hs[i] / 2.0 >= ys[i - 1] + hs[i - 1] / 2.0 - 1e-6,
                "{want:?} {hs:?} {ys:?}"
            );
        }
        let total: f64 = hs.iter().sum();
        if total <= 300.0 {
            assert!(ys[0] - hs[0] / 2.0 >= -1e-6, "{ys:?}");
            assert!(ys[n - 1] + hs[n - 1] / 2.0 <= 300.0 + 1e-6, "{ys:?}");
        }
    }
}

#[test]
fn random_pies_keep_leader_lines_off_the_text_and_labels_off_each_other() {
    let gray = Rgba::rgb(0x86, 0x86, 0x86);
    let mut seed = 99u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (1u64 << 31) as f64
    };
    for case in 0..200 {
        let n = 2 + (next() * 12.0) as usize;
        let vals: Vec<f64> = (0..n)
            .map(|_| {
                if next() < 0.5 {
                    1.0 + next() * 3.0
                } else {
                    1.0 + next() * 60.0
                }
            })
            .collect();
        let items = draw(&pie_with_labels(&vals));
        let boxes = label_boxes(&items);
        assert_eq!(boxes.len(), n, "case {case}: {vals:?}");
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let overlap =
                    a.0 < b.2 - 0.5 && b.0 < a.2 - 0.5 && a.1 < b.3 - 0.5 && b.1 < a.3 - 0.5;
                assert!(!overlap, "case {case}: {vals:?}: {a:?} overlaps {b:?}");
            }
        }
        for s in segs(&items).into_iter().filter(|s| s.4 == gray) {
            for b in &boxes {
                let b = (b.0 + 0.5, b.1 + 0.5, b.2 - 0.5, b.3 - 0.5);
                assert!(
                    !seg_hits((s.0, s.1, s.2, s.3), b),
                    "case {case}: {vals:?}: {s:?} hits {b:?}"
                );
            }
        }
    }
}
