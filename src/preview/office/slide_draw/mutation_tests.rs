//! Tests added from mutation testing of the slide-drawing code: each pins a behaviour a mutant
//! (flipped comparison, swapped operand, removed branch) used to change without any test noticing.

use super::color::*;
use super::model::Rgba;

fn px(c: Rgba) -> (u8, u8, u8) {
    (c.r, c.g, c.b)
}

#[test]
fn grey_has_hue_zero_not_nan() {
    // A grey has max == min: the early return must give hue 0 (dividing by d == 0 would give NaN,
    // which `rgb_of` hides by treating a non-finite hue as 0, so only the hue itself shows it).
    for v in [0u8, 1, 128, 255] {
        let (h, s, l) = rgb_to_hsl(v, v, v);
        assert_eq!(h, 0.0, "grey {v}");
        assert_eq!(s, 0.0, "grey {v}");
        assert!((l - v as f64 / 255.0).abs() < 1e-12);
    }
}

#[test]
fn every_system_colour_name_has_its_windows_value() {
    let table: &[(&str, u32)] = &[
        ("scrollBar", 0xC8C8C8),
        ("background", 0x000000),
        ("activeCaption", 0x99B4D1),
        ("inactiveCaption", 0xBFCDDB),
        ("menu", 0xF0F0F0),
        ("btnFace", 0xF0F0F0),
        ("menuBar", 0xF0F0F0),
        ("window", 0xFFFFFF),
        ("highlightText", 0xFFFFFF),
        ("btnHighlight", 0xFFFFFF),
        ("windowFrame", 0x646464),
        ("menuText", 0),
        ("windowText", 0),
        ("captionText", 0),
        ("btnText", 0),
        ("infoText", 0),
        ("activeBorder", 0xB4B4B4),
        ("inactiveBorder", 0xF4F7FC),
        ("appWorkspace", 0xABABAB),
        ("highlight", 0x0078D7),
        ("btnShadow", 0xA0A0A0),
        ("grayText", 0x6D6D6D),
        ("inactiveCaptionText", 0x434E54),
        ("3dDkShadow", 0x696969),
        ("3dLight", 0xE3E3E3),
        ("infoBk", 0xFFFFE1),
        ("hotLight", 0x0066CC),
        ("gradientActiveCaption", 0xB9D1EA),
        ("gradientInactiveCaption", 0xD7E4F2),
        ("menuHighlight", 0x3399FF),
    ];
    for (name, v) in table {
        let c = system_color(name).unwrap_or_else(|| panic!("{name} unknown"));
        assert_eq!(px(c), ((v >> 16) as u8, (v >> 8) as u8, *v as u8), "{name}");
    }
    assert!(system_color("noSuchColour").is_none());
}

#[test]
fn mix_blends_colour_and_alpha_linearly() {
    let a = Rgba {
        r: 0,
        g: 100,
        b: 200,
        a: 0.2,
    };
    let b = Rgba {
        r: 100,
        g: 200,
        b: 0,
        a: 0.8,
    };
    let m = mix(a, b, 0.25);
    assert_eq!(px(m), (25, 125, 150));
    assert!((m.a - 0.35).abs() < 1e-12, "alpha {}", m.a);
    assert_eq!(mix(a, b, 0.0), a);
    assert_eq!(mix(a, b, 1.0), b);
    // A non-finite factor blends half way; an out-of-range one is clamped.
    assert_eq!(px(mix(a, b, f64::NAN)), (50, 150, 100));
    assert_eq!(px(mix(a, b, 7.0)), px(b));
    assert_eq!(px(mix(a, b, -7.0)), px(a));
}

#[test]
fn guide_operators_tan_and_mod_and_unknown_count() {
    use super::geom::{eval_formula, preset_adjust_names, Scope};
    // x * tan(angle): angles are 60000ths of a degree (60 degrees: tan = sqrt 3, not its inverse)
    assert!((eval_formula("tan 100 3600000", 100.0, 100.0, &[]) - 173.205_080_756_9).abs() < 1e-6);
    // mod is the Euclidean length of three values: sqrt(2^2 + 3^2 + 6^2) = 7
    assert!((eval_formula("mod 2 3 6", 100.0, 100.0, &[]) - 7.0).abs() < 1e-9);
    assert!((eval_formula("mod 3 4 0", 100.0, 100.0, &[]) - 5.0).abs() < 1e-9);
    // an unknown operator is 0 and is counted, once per use
    let s = Scope::new(100.0, 100.0);
    assert_eq!(s.eval("bogus 1 2"), 0.0);
    assert_eq!(s.unknown(), 1);
    assert_eq!(s.eval("bogus"), 0.0);
    assert_eq!(s.unknown(), 2);
    // names of the adjust values, in definition order
    let names = |n: &str| preset_adjust_names(n);
    assert_eq!(names("roundRect"), Some(vec!["adj".to_string()]));
    assert_eq!(
        names("upArrow"),
        Some(vec!["adj1".to_string(), "adj2".to_string()])
    );
    assert_eq!(names("rect"), Some(vec![]));
    assert_eq!(names("noSuchPreset"), None);
}

fn near(got: (f64, f64, f64), want: (f64, f64, f64)) {
    let ok = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(
        ok(got.0, want.0) && ok(got.1, want.1) && ok(got.2, want.2),
        "{got:?} != {want:?}"
    );
}

#[test]
fn auto_range_edge_cases() {
    use super::chart::scale::auto_range;
    // no data span at all: 0..1 with a unit from the tick count
    near(auto_range(0.0, 0.0, 5.0), (0.0, 1.0, 0.2));
    // equal values: the padding is 5 % of the value's magnitude (1000 -> 50, so 1050 -> unit 200)
    near(auto_range(1000.0, 1000.0, 10.0), (0.0, 1200.0, 200.0));
    // all-negative data far from zero reach up to 0, padded below
    near(auto_range(-10.0, -5.0, 5.0), (-15.0, 0.0, 5.0));
    // all-negative data in the top sixth of their range do not reach 0
    near(auto_range(-10.0, -9.0, 5.0), (-10.5, -8.5, 0.5));
    // the same for positive data
    near(auto_range(10.0, 11.0, 5.0), (9.5, 11.5, 0.5));
    // data ending exactly at 0: the top is 0, and takes no padding into the unit
    near(auto_range(-9.3, 0.0, 5.0), (-10.0, 0.0, 2.0));
}

#[test]
fn gray_weights_each_channel() {
    // LibreOffice weights 22 / 72 / 6 per cent, integer division: each channel on its own and mixed.
    assert_eq!(
        px(apply_mods(Rgba::rgb(255, 0, 0), &[ColorMod::Gray])),
        (56, 56, 56)
    );
    assert_eq!(
        px(apply_mods(Rgba::rgb(0, 0, 255), &[ColorMod::Gray])),
        (15, 15, 15)
    );
    assert_eq!(
        px(apply_mods(Rgba::rgb(100, 200, 50), &[ColorMod::Gray])),
        (169, 169, 169)
    );
}

#[test]
fn saturation_and_hue_transforms_are_exact() {
    // A colour with a mid saturation, lightness and hue so that every operator gives a different result.
    let base = Rgba::rgb(191, 96, 64);
    let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
    assert!(s > 0.3 && s < 0.7 && h > 10.0 && h < 30.0, "{h} {s} {l}");
    let want = hsl_to_rgb;
    assert_eq!(
        px(apply_mods(base, &[ColorMod::Sat(0.25)])),
        want(h, 0.25, l)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::SatMod(0.5)])),
        want(h, s * 0.5, l)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::SatOff(0.125)])),
        want(h, s + 0.125, l)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::Hue(100.0)])),
        want(100.0, s, l)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::HueMod(3.0)])),
        want(h * 3.0, s, l)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::HueOff(50.0)])),
        want(h + 50.0, s, l)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::Comp])),
        want(h + 180.0, s, l)
    );
    assert_eq!(px(apply_mods(base, &[ColorMod::Lum(0.7)])), want(h, s, 0.7));
    assert_eq!(
        px(apply_mods(base, &[ColorMod::LumMod(0.5)])),
        want(h, s, l * 0.5)
    );
    assert_eq!(
        px(apply_mods(base, &[ColorMod::LumOff(0.125)])),
        want(h, s, l + 0.125)
    );
}

#[test]
fn channel_transforms_work_in_linear_light() {
    // Absolute / modulate / offset of each channel, starting from a mid grey.
    let grey = Rgba::rgb(128, 128, 128);
    let lin = srgb_to_linear(128.0 / 255.0);
    let byte = |v: f64| (linear_to_srgb(v) * 255.0).round() as u8;
    let others = byte(lin);
    let cases: [(ColorMod, usize, f64); 9] = [
        (ColorMod::Red(0.5), 0, 0.5),
        (ColorMod::RedMod(0.5), 0, lin * 0.5),
        (ColorMod::RedOff(0.25), 0, lin + 0.25),
        (ColorMod::Green(0.5), 1, 0.5),
        (ColorMod::GreenMod(0.5), 1, lin * 0.5),
        (ColorMod::GreenOff(0.25), 1, lin + 0.25),
        (ColorMod::Blue(0.5), 2, 0.5),
        (ColorMod::BlueMod(0.5), 2, lin * 0.5),
        (ColorMod::BlueOff(0.25), 2, lin + 0.25),
    ];
    for (m, ch, v) in cases {
        let got = apply_mods(grey, &[m]);
        let got = [got.r, got.g, got.b];
        for (i, g) in got.iter().enumerate() {
            let want = if i == ch { byte(v) } else { others };
            assert_eq!(*g, want, "{m:?} channel {i}");
        }
    }
}
