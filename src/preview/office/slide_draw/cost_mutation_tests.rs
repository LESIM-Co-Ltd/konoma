//! Tests from mutation testing of the time model (`cost.rs`, `cost_vector.rs`): every feature adds
//! to the predicted time in its own bucket and in proportion, the markup scan counts what the
//! writer wrote, and the SVG picture scan sizes what it paints.

use super::super::cost::{scan, scan_vector, Features, Predicted};

/// One feature set per field, each with `k` units of that field only.
fn each_field(k: f64) -> Vec<(&'static str, Features)> {
    let d = Features::default();
    vec![
        ("els", Features { els: k, ..d }),
        ("segs", Features { segs: k, ..d }),
        ("glyphs", Features { glyphs: k, ..d }),
        ("glyphs_sq", Features { glyphs_sq: k, ..d }),
        ("spans", Features { spans: k, ..d }),
        ("pic_dec_px", Features { pic_dec_px: k, ..d }),
        ("pic_bytes", Features { pic_bytes: k, ..d }),
        ("svg_bytes", Features { svg_bytes: k, ..d }),
        ("tiles", Features { tiles: k, ..d }),
        ("edge_px", Features { edge_px: k, ..d }),
        ("fill_px2", Features { fill_px2: k, ..d }),
        ("grad_px2", Features { grad_px2: k, ..d }),
        ("vec_px2", Features { vec_px2: k, ..d }),
        ("pic_px2", Features { pic_px2: k, ..d }),
        (
            "filter_units",
            Features {
                filter_units: k,
                ..d
            },
        ),
    ]
}

#[test]
fn every_feature_adds_time_in_proportion_in_its_own_bucket() {
    let base = Features::default().predict();
    // nothing drawn still costs the process's own start-up
    assert!(base.fixed > 0.0 && base.edge == 0.0 && base.area == 0.0);
    let fixed_ones = [
        "els",
        "segs",
        "glyphs",
        "glyphs_sq",
        "spans",
        "pic_dec_px",
        "pic_bytes",
        "svg_bytes",
        "tiles",
    ];
    let area_ones = ["fill_px2", "grad_px2", "vec_px2", "pic_px2", "filter_units"];
    let one = each_field(1000.0);
    let two = each_field(2000.0);
    for ((name, a), (_, b)) in one.iter().zip(two.iter()) {
        let (pa, pb) = (a.predict(), b.predict());
        let (da, db) = (
            (
                pa.fixed - base.fixed,
                pa.edge - base.edge,
                pa.area - base.area,
            ),
            (
                pb.fixed - base.fixed,
                pb.edge - base.edge,
                pb.area - base.area,
            ),
        );
        let bucket = if fixed_ones.contains(name) {
            0
        } else if *name == "edge_px" {
            1
        } else {
            assert!(area_ones.contains(name), "{name}");
            2
        };
        let parts = [da.0, da.1, da.2];
        let parts2 = [db.0, db.1, db.2];
        for i in 0..3 {
            if i == bucket {
                assert!(parts[i] > 0.0, "{name}: bucket {i} gets {}", parts[i]);
                // linear: twice the units, twice the time
                assert!(
                    (parts2[i] - 2.0 * parts[i]).abs() < 1e-9 * parts[i].max(1.0),
                    "{name}: {} then {}",
                    parts[i],
                    parts2[i]
                );
            } else {
                assert_eq!(parts[i], 0.0, "{name} touches bucket {i}");
            }
        }
    }
    // a text run (five elements and a span) takes 50 microseconds, as documented
    let run = Features {
        els: 5.0,
        spans: 1.0,
        ..Features::default()
    };
    assert!((run.predict().fixed - base.fixed - 0.05).abs() < 1e-9);
    // all the features at once add up
    let mut all = Features::default();
    for (_, f) in one.iter() {
        all.add_scaled(f, 1.0);
    }
    let sum: f64 = one
        .iter()
        .map(|(_, f)| f.predict().at(1.0) - base.at(1.0))
        .sum();
    assert!((all.predict().at(1.0) - base.at(1.0) - sum).abs() < 1e-9 * sum);
}

#[test]
fn add_scaled_adds_every_field_times_the_factor() {
    let mut a = Features::default();
    for (_, f) in each_field(3.0) {
        a.add_scaled(&f, 2.0);
    }
    for (name, f) in each_field(6.0) {
        // field by field: `a` has 6 in each
        let (pa, pf) = (a.predict(), f.predict());
        let base = Features::default().predict();
        let _ = (pa, pf, base);
        let one = Features::default();
        let mut only = one;
        only.add_scaled(&f, 1.0);
        assert_eq!(only.predict(), f.predict(), "{name}");
    }
    let base = Features::default().predict().at(1.0);
    let total: f64 = each_field(6.0)
        .iter()
        .map(|(_, f)| f.predict().at(1.0) - base)
        .sum();
    assert!((a.predict().at(1.0) - base - total).abs() < 1e-9 * total);
}

#[test]
fn the_biggest_raster_is_the_positive_root_and_the_boundaries_are_exact() {
    let p = Predicted {
        fixed: 10.0,
        edge: 3.0,
        area: 2.0,
    };
    let r = p.max_ratio(100.0);
    assert!((p.at(r) - 100.0).abs() < 1e-9, "{}", p.at(r));
    // only an edge: linear
    let p = Predicted {
        fixed: 10.0,
        edge: 3.0,
        area: 0.0,
    };
    assert!((p.max_ratio(100.0) - 30.0).abs() < 1e-9);
    // nothing depends on the raster
    let p = Predicted {
        fixed: 10.0,
        edge: 0.0,
        area: 0.0,
    };
    assert_eq!(p.max_ratio(100.0), f64::INFINITY);
    // the thresholds are exclusive: a coefficient of 1e-12 is nothing
    let p = Predicted {
        fixed: 0.0,
        edge: 0.0,
        area: 1e-12,
    };
    assert_eq!(p.max_ratio(100.0), f64::INFINITY);
    let p = Predicted {
        fixed: 0.0,
        edge: 1e-12,
        area: 0.0,
    };
    assert_eq!(p.max_ratio(100.0), f64::INFINITY);
    // over budget already, or no budget: zero; not a number: zero
    let p = Predicted {
        fixed: 100.0,
        edge: 1.0,
        area: 1.0,
    };
    assert_eq!(p.max_ratio(100.0), 0.0);
    assert_eq!(p.max_ratio(50.0), 0.0);
    assert_eq!(p.max_ratio(f64::NAN), 0.0);
}

#[test]
fn the_scan_counts_elements_runs_characters_and_path_segments() {
    let s = scan(
        r#"<g><path d="M0 0 L10 10 C1 2 3 4 5 6 Q1 1 2 2 Z"/><text><tspan x="1">héllo</tspan><tspan>ab</tspan></text></g>"#,
    );
    assert_eq!(s.els, 5.0, "g, path, text, two tspans");
    assert_eq!(s.segs, 5.0);
    assert_eq!(s.spans, 2.0);
    assert_eq!(s.glyphs, 7.0);
    assert_eq!(s.glyphs_sq, 25.0 + 4.0);
    // tags that close, comments and declarations are not elements
    let s = scan("<?xml version='1.0'?><!-- c --><a></a>");
    assert_eq!(s.els, 1.0);
    // a run that is never closed counts to the end; a path that is never closed does not panic
    let s = scan("<tspan>abc");
    assert_eq!((s.spans, s.glyphs, s.glyphs_sq), (1.0, 3.0, 9.0));
    let s = scan(r#"<path d="M0 0 L1"#);
    assert_eq!(s.segs, 2.0);
    // a run whose start tag has no end
    let s = scan("<tspan");
    assert_eq!((s.spans, s.glyphs), (1.0, 0.0));
    // text with child tags counts all the characters between the tags of the run
    let s = scan("<tspan>a<b/>c</tspan>");
    assert_eq!(s.spans, 1.0);
    assert_eq!(s.glyphs, 6.0);
}

fn svg(inner: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="1000">{inner}</svg>"#)
}

#[test]
fn the_picture_scan_sizes_what_the_shapes_paint() {
    let pi = std::f64::consts::PI;
    let circle = scan_vector(&svg(r#"<circle cx="500" cy="500" r="10"/>"#));
    assert!((circle.area - pi * 100.0).abs() < 1e-9, "{}", circle.area);
    let ellipse = scan_vector(&svg(r#"<ellipse rx="10" ry="20"/>"#));
    assert!((ellipse.area - pi * 200.0).abs() < 1e-9, "{}", ellipse.area);
    let rect = scan_vector(&svg(r#"<rect width="30" height="40"/>"#));
    assert_eq!(rect.area, 1200.0);
    // the picture's own size is the most one element can paint
    let big = scan_vector(&svg(r#"<rect width="5000" height="5000"/>"#));
    assert_eq!(big.area, 1_000_000.0);
    // elements: the svg itself and the shape
    assert_eq!(rect.els, 2.0);
    assert_eq!((rect.width, rect.height), (1000.0, 1000.0));
}

#[test]
fn only_the_outermost_svg_sizes_the_picture() {
    let v = scan_vector(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><svg width="5000" height="6000" viewBox="0 0 7 8"><rect width="10" height="10"/></svg></svg>"#,
    );
    assert_eq!((v.width, v.height), (200.0, 100.0));
    assert_eq!(v.els, 3.0);
    // viewBox wins over width / height
    let v = scan_vector(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 30 40"/>"#,
    );
    assert_eq!((v.width, v.height), (30.0, 40.0));
    // a picture of no size is 100 x 100
    let v = scan_vector("<svg/>");
    assert_eq!((v.width, v.height), (100.0, 100.0));
}

#[test]
fn a_path_paints_its_bounding_box_not_the_corner_of_the_picture() {
    // a square away from the origin: 100 x 100 units, not 600 x 600
    let v = scan_vector(&svg(r#"<path d="M500 500 L600 500 L600 600 L500 600 Z"/>"#));
    assert_eq!(v.area, 10_000.0);
    assert_eq!(v.segs, 5.0);
    // a stroked line grows its box by half the stroke width on each side
    let v = scan_vector(&svg(
        r#"<path d="M500 500 L600 500" stroke="red" stroke-width="10" fill="none"/>"#,
    ));
    assert_eq!(v.area, 110.0 * 10.0);
    // negative coordinates are cut to the picture
    let v = scan_vector(&svg(r#"<path d="M-50 -50 L50 50"/>"#));
    assert_eq!(v.area, 50.0 * 50.0);
}
