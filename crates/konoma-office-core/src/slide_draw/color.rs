//! Colour maths for the slide readers: HSL conversion, the DrawingML colour transforms
//! (ECMA-376 Part 1, 20.1.2.3) and the named colours (`prstClr`, `sysClr`).
//!
//! # Which colour space each transform works in
//!
//! The standard describes the transforms loosely. This module follows what PowerPoint does as
//! documented by [MS-ODRAWXML] and implemented by LibreOffice (`oox/source/drawingml/color.cxx`,
//! read 2026-10-09), which is what decks in the wild are authored against:
//!
//! * `lumMod`, `lumOff`, `lum`, `satMod`, `satOff`, `sat`, `hueMod`, `hueOff`, `hue`, `comp`:
//!   in **HSL** (computed from sRGB values, no gamma).
//! * `tint`, `shade`, `inv`, `red/green/blue` (+`Mod`/`Off`): in **linear RGB** ("crgb" in
//!   LibreOffice): the colour is converted from sRGB with the IEC 61966-2-1 transfer function,
//!   changed, and converted back. `shade` multiplies the linear value by the factor (so
//!   shade 50 % of white is `#BCBCBC`, not `#808080`); `tint` moves it towards white:
//!   `c' = 1 - (1 - c) * tint` (0 % = white, 100 % = unchanged).
//! * `gray`: LibreOffice's integer weights `(22 R + 72 G + 6 B) / 100` on the sRGB values.
//! * `gamma` / `invGamma`: LibreOffice's pure power law with exponent 2.3 on the linear value
//!   (`gamma`: `c^(1/2.3)`, `invGamma`: `c^2.3`).
//! * `alpha` sets the opacity, `alphaMod` multiplies it, `alphaOff` adds to it (clamped to 0..1).
//!
//! All factors in [`ColorMod`] are fractions (`1.0` = 100 %); hue offsets are in degrees. Readers
//! divide the file's 1000ths of a percent / 60000ths of a degree themselves. Transforms are applied
//! in order. The working colour is kept as `f64` between transforms and rounded to bytes once, at
//! the end.

use super::model::Rgba;

/// A DrawingML colour transform (a child of `srgbClr`, `schemeClr`, ...). Fractions: `1.0` = 100 %.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMod {
    Tint(f64),
    Shade(f64),
    /// Absolute luminance (HSL lightness), `0..=1`.
    Lum(f64),
    LumMod(f64),
    LumOff(f64),
    /// Absolute saturation.
    Sat(f64),
    SatMod(f64),
    SatOff(f64),
    /// Absolute hue in degrees.
    Hue(f64),
    HueMod(f64),
    /// Hue offset in degrees.
    HueOff(f64),
    Alpha(f64),
    AlphaMod(f64),
    AlphaOff(f64),
    /// Complement: hue + 180 degrees.
    Comp,
    /// Inverse (in linear RGB).
    Inv,
    Gray,
    Gamma,
    InvGamma,
    /// Absolute channel values `0..=1` (in linear RGB).
    Red(f64),
    RedMod(f64),
    RedOff(f64),
    Green(f64),
    GreenMod(f64),
    GreenOff(f64),
    Blue(f64),
    BlueMod(f64),
    BlueOff(f64),
}

/// sRGB (0..1) to linear light, IEC 61966-2-1.
pub fn srgb_to_linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light (0..1) to sRGB, IEC 61966-2-1.
pub fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c >= 1.0 {
        1.0
    } else if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// RGB bytes to HSL: hue in degrees `0..360`, saturation and lightness `0..=1`.
pub fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    hsl_of(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

/// HSL (hue in degrees, saturation and lightness `0..=1`) to RGB bytes.
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let (r, g, b) = rgb_of(h, s, l);
    (to_byte(r), to_byte(g), to_byte(b))
}

fn hsl_of(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-12 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-12);
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s.clamp(0.0, 1.0), l)
}

fn rgb_of(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = if h.is_finite() {
        h.rem_euclid(360.0)
    } else {
        0.0
    };
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

fn to_byte(v: f64) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// Applies one transform to a colour (see the module documentation for the spaces used). Only
/// the tests apply a single transform; the readers apply the whole list ([`apply_mods`]).
#[cfg(test)]
pub fn apply_mod(c: Rgba, m: ColorMod) -> Rgba {
    apply_mods(c, &[m])
}

/// Applies the transforms in order.
pub fn apply_mods(c: Rgba, mods: &[ColorMod]) -> Rgba {
    // Working state: sRGB channels 0..1 plus alpha.
    let mut rgb = [c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0];
    let mut a = c.a;
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    for &m in mods {
        match m {
            ColorMod::Alpha(v) => a = fin(v).clamp(0.0, 1.0),
            ColorMod::AlphaMod(v) => a = (a * fin(v)).clamp(0.0, 1.0),
            ColorMod::AlphaOff(v) => a = (a + fin(v)).clamp(0.0, 1.0),
            ColorMod::Lum(_)
            | ColorMod::LumMod(_)
            | ColorMod::LumOff(_)
            | ColorMod::Sat(_)
            | ColorMod::SatMod(_)
            | ColorMod::SatOff(_)
            | ColorMod::Hue(_)
            | ColorMod::HueMod(_)
            | ColorMod::HueOff(_)
            | ColorMod::Comp => {
                let (mut h, mut s, mut l) = hsl_of(rgb[0], rgb[1], rgb[2]);
                match m {
                    ColorMod::Lum(v) => l = fin(v),
                    ColorMod::LumMod(v) => l *= fin(v),
                    ColorMod::LumOff(v) => l += fin(v),
                    ColorMod::Sat(v) => s = fin(v),
                    ColorMod::SatMod(v) => s *= fin(v),
                    ColorMod::SatOff(v) => s += fin(v),
                    ColorMod::Hue(v) => h = fin(v),
                    ColorMod::HueMod(v) => h *= fin(v),
                    ColorMod::HueOff(v) => h += fin(v),
                    _ => h += 180.0,
                }
                let (r, g, b) = rgb_of(h, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
                rgb = [r, g, b];
            }
            ColorMod::Gray => {
                let g = (to_byte(rgb[0]) as u32 * 22
                    + to_byte(rgb[1]) as u32 * 72
                    + to_byte(rgb[2]) as u32 * 6)
                    / 100;
                let g = g as f64 / 255.0;
                rgb = [g, g, g];
            }
            _ => {
                // Everything left works on linear light.
                let mut lin = rgb.map(srgb_to_linear);
                match m {
                    ColorMod::Tint(v) => {
                        let v = fin(v).clamp(0.0, 1.0);
                        lin = lin.map(|c| 1.0 - (1.0 - c) * v);
                    }
                    ColorMod::Shade(v) => {
                        let v = fin(v).clamp(0.0, 1.0);
                        lin = lin.map(|c| c * v);
                    }
                    ColorMod::Inv => lin = lin.map(|c| 1.0 - c),
                    ColorMod::Gamma => lin = lin.map(|c| c.clamp(0.0, 1.0).powf(1.0 / 2.3)),
                    ColorMod::InvGamma => lin = lin.map(|c| c.clamp(0.0, 1.0).powf(2.3)),
                    ColorMod::Red(v) => lin[0] = fin(v),
                    ColorMod::RedMod(v) => lin[0] *= fin(v),
                    ColorMod::RedOff(v) => lin[0] += fin(v),
                    ColorMod::Green(v) => lin[1] = fin(v),
                    ColorMod::GreenMod(v) => lin[1] *= fin(v),
                    ColorMod::GreenOff(v) => lin[1] += fin(v),
                    ColorMod::Blue(v) => lin[2] = fin(v),
                    ColorMod::BlueMod(v) => lin[2] *= fin(v),
                    ColorMod::BlueOff(v) => lin[2] += fin(v),
                    _ => {}
                }
                rgb = lin.map(linear_to_srgb);
            }
        }
    }
    Rgba {
        r: to_byte(rgb[0]),
        g: to_byte(rgb[1]),
        b: to_byte(rgb[2]),
        a: if a.is_finite() {
            a.clamp(0.0, 1.0)
        } else {
            1.0
        },
    }
}

/// Blends `a` towards `b` by `t` (`0` = `a`, `1` = `b`) in sRGB; alpha is blended too.
pub fn mix(a: Rgba, b: Rgba, t: f64) -> Rgba {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let f = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    Rgba {
        r: f(a.r, b.r),
        g: f(a.g, b.g),
        b: f(a.b, b.b),
        a: a.a + (b.a - a.a) * t,
    }
}

/// Name -> `#RRGGBB` for the CSS / DrawingML preset colours (`prstClr@val`), canonical spelling
/// (`darkBlue`). Case-insensitive lookup is done by [`preset_color`].
const PRESET: &[(&str, u32)] = &[
    ("aliceblue", 0xF0F8FF),
    ("antiquewhite", 0xFAEBD7),
    ("aqua", 0x00FFFF),
    ("aquamarine", 0x7FFFD4),
    ("azure", 0xF0FFFF),
    ("beige", 0xF5F5DC),
    ("bisque", 0xFFE4C4),
    ("black", 0x000000),
    ("blanchedalmond", 0xFFEBCD),
    ("blue", 0x0000FF),
    ("blueviolet", 0x8A2BE2),
    ("brown", 0xA52A2A),
    ("burlywood", 0xDEB887),
    ("cadetblue", 0x5F9EA0),
    ("chartreuse", 0x7FFF00),
    ("chocolate", 0xD2691E),
    ("coral", 0xFF7F50),
    ("cornflowerblue", 0x6495ED),
    ("cornsilk", 0xFFF8DC),
    ("crimson", 0xDC143C),
    ("cyan", 0x00FFFF),
    ("darkblue", 0x00008B),
    ("darkcyan", 0x008B8B),
    ("darkgoldenrod", 0xB8860B),
    ("darkgray", 0xA9A9A9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xA9A9A9),
    ("darkkhaki", 0xBDB76B),
    ("darkmagenta", 0x8B008B),
    ("darkolivegreen", 0x556B2F),
    ("darkorange", 0xFF8C00),
    ("darkorchid", 0x9932CC),
    ("darkred", 0x8B0000),
    ("darksalmon", 0xE9967A),
    ("darkseagreen", 0x8FBC8F),
    ("darkslateblue", 0x483D8B),
    ("darkslategray", 0x2F4F4F),
    ("darkslategrey", 0x2F4F4F),
    ("darkturquoise", 0x00CED1),
    ("darkviolet", 0x9400D3),
    ("deeppink", 0xFF1493),
    ("deepskyblue", 0x00BFFF),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1E90FF),
    ("firebrick", 0xB22222),
    ("floralwhite", 0xFFFAF0),
    ("forestgreen", 0x228B22),
    ("fuchsia", 0xFF00FF),
    ("gainsboro", 0xDCDCDC),
    ("ghostwhite", 0xF8F8FF),
    ("gold", 0xFFD700),
    ("goldenrod", 0xDAA520),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xADFF2F),
    ("honeydew", 0xF0FFF0),
    ("hotpink", 0xFF69B4),
    ("indianred", 0xCD5C5C),
    ("indigo", 0x4B0082),
    ("ivory", 0xFFFFF0),
    ("khaki", 0xF0E68C),
    ("lavender", 0xE6E6FA),
    ("lavenderblush", 0xFFF0F5),
    ("lawngreen", 0x7CFC00),
    ("lemonchiffon", 0xFFFACD),
    ("lightblue", 0xADD8E6),
    ("lightcoral", 0xF08080),
    ("lightcyan", 0xE0FFFF),
    ("lightgoldenrodyellow", 0xFAFAD2),
    ("lightgray", 0xD3D3D3),
    ("lightgreen", 0x90EE90),
    ("lightgrey", 0xD3D3D3),
    ("lightpink", 0xFFB6C1),
    ("lightsalmon", 0xFFA07A),
    ("lightseagreen", 0x20B2AA),
    ("lightskyblue", 0x87CEFA),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xB0C4DE),
    ("lightyellow", 0xFFFFE0),
    ("lime", 0x00FF00),
    ("limegreen", 0x32CD32),
    ("linen", 0xFAF0E6),
    ("magenta", 0xFF00FF),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66CDAA),
    ("mediumblue", 0x0000CD),
    ("mediumorchid", 0xBA55D3),
    ("mediumpurple", 0x9370DB),
    ("mediumseagreen", 0x3CB371),
    ("mediumslateblue", 0x7B68EE),
    ("mediumspringgreen", 0x00FA9A),
    ("mediumturquoise", 0x48D1CC),
    ("mediumvioletred", 0xC71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xF5FFFA),
    ("mistyrose", 0xFFE4E1),
    ("moccasin", 0xFFE4B5),
    ("navajowhite", 0xFFDEAD),
    ("navy", 0x000080),
    ("oldlace", 0xFDF5E6),
    ("olive", 0x808000),
    ("olivedrab", 0x6B8E23),
    ("orange", 0xFFA500),
    ("orangered", 0xFF4500),
    ("orchid", 0xDA70D6),
    ("palegoldenrod", 0xEEE8AA),
    ("palegreen", 0x98FB98),
    ("paleturquoise", 0xAFEEEE),
    ("palevioletred", 0xDB7093),
    ("papayawhip", 0xFFEFD5),
    ("peachpuff", 0xFFDAB9),
    ("peru", 0xCD853F),
    ("pink", 0xFFC0CB),
    ("plum", 0xDDA0DD),
    ("powderblue", 0xB0E0E6),
    ("purple", 0x800080),
    ("red", 0xFF0000),
    ("rosybrown", 0xBC8F8F),
    ("royalblue", 0x4169E1),
    ("saddlebrown", 0x8B4513),
    ("salmon", 0xFA8072),
    ("sandybrown", 0xF4A460),
    ("seagreen", 0x2E8B57),
    ("seashell", 0xFFF5EE),
    ("sienna", 0xA0522D),
    ("silver", 0xC0C0C0),
    ("skyblue", 0x87CEEB),
    ("slateblue", 0x6A5ACD),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xFFFAFA),
    ("springgreen", 0x00FF7F),
    ("steelblue", 0x4682B4),
    ("tan", 0xD2B48C),
    ("teal", 0x008080),
    ("thistle", 0xD8BFD8),
    ("tomato", 0xFF6347),
    ("turquoise", 0x40E0D0),
    ("violet", 0xEE82EE),
    ("wheat", 0xF5DEB3),
    ("white", 0xFFFFFF),
    ("whitesmoke", 0xF5F5F5),
    ("yellow", 0xFFFF00),
    ("yellowgreen", 0x9ACD32),
];

/// The colour of a `prstClr@val` name (case-insensitive). The short spellings `dk*`, `lt*` and
/// `med*` of DrawingML (`dkBlue`, `ltGray`, `medPurple`) are the `dark*`, `light*` and `medium*`
/// colours. `None` for an unknown name.
pub fn preset_color(name: &str) -> Option<Rgba> {
    let n = name.trim().to_ascii_lowercase();
    let find = |k: &str| PRESET.iter().find(|(p, _)| *p == k).map(|(_, v)| *v);
    let v = find(&n).or_else(|| {
        let long = if let Some(r) = n.strip_prefix("dk") {
            format!("dark{r}")
        } else if let Some(r) = n.strip_prefix("lt") {
            format!("light{r}")
        } else {
            format!("medium{}", n.strip_prefix("med")?)
        };
        find(&long)
    })?;
    Some(Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// The Windows default colour of a `sysClr@val` name, used when the file gives no `lastClr`
/// (values of the Windows 10 light theme). `None` for an unknown name.
pub fn system_color(name: &str) -> Option<Rgba> {
    let v: u32 = match name {
        "scrollBar" => 0xC8C8C8,
        "background" => 0x000000,
        "activeCaption" => 0x99B4D1,
        "inactiveCaption" => 0xBFCDDB,
        "menu" | "btnFace" | "menuBar" => 0xF0F0F0,
        "window" | "highlightText" | "btnHighlight" => 0xFFFFFF,
        "windowFrame" => 0x646464,
        "menuText" | "windowText" | "captionText" | "btnText" | "infoText" => 0x000000,
        "activeBorder" => 0xB4B4B4,
        "inactiveBorder" => 0xF4F7FC,
        "appWorkspace" => 0xABABAB,
        "highlight" => 0x0078D7,
        "btnShadow" => 0xA0A0A0,
        "grayText" => 0x6D6D6D,
        "inactiveCaptionText" => 0x434E54,
        "3dDkShadow" => 0x696969,
        "3dLight" => 0xE3E3E3,
        "infoBk" => 0xFFFFE1,
        "hotLight" => 0x0066CC,
        "gradientActiveCaption" => 0xB9D1EA,
        "gradientInactiveCaption" => 0xD7E4F2,
        "menuHighlight" => 0x3399FF,
        _ => return None,
    };
    Some(Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(c: Rgba, hex: &str, tol: i32) {
        let want = Rgba::from_hex(hex).unwrap();
        let d = |a: u8, b: u8| (a as i32 - b as i32).abs();
        assert!(
            d(c.r, want.r) <= tol && d(c.g, want.g) <= tol && d(c.b, want.b) <= tol,
            "got {:02X}{:02X}{:02X}, want {hex}",
            c.r,
            c.g,
            c.b
        );
    }

    const ACCENT1: &str = "4F81BD"; // Office 2007-2010 theme "Accent 1"

    #[test]
    fn hsl_round_trips_every_primary_and_a_grid() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(51) {
                    let (h, s, l) = rgb_to_hsl(r, g, b);
                    assert_eq!(hsl_to_rgb(h, s, l), (r, g, b), "{r} {g} {b}");
                }
            }
        }
        // red = hue 0, green = 120, blue = 240; grey has no saturation
        assert_eq!(rgb_to_hsl(255, 0, 0), (0.0, 1.0, 0.5));
        assert_eq!(rgb_to_hsl(0, 255, 0).0, 120.0);
        assert_eq!(rgb_to_hsl(0, 0, 255).0, 240.0);
        assert_eq!(rgb_to_hsl(128, 128, 128).1, 0.0);
    }

    #[test]
    fn lum_mod_off_matches_the_office_palette() {
        // "Blue, Accent 1, Lighter 40%" is lumMod 60000 + lumOff 40000 = #95B3D7 in Office 2007's
        // palette picker; "Darker 25%" is lumMod 75000 = #366092; "Lighter 80%" is #DCE6F2.
        let base = Rgba::from_hex(ACCENT1).unwrap();
        close(
            apply_mods(base, &[ColorMod::LumMod(0.6), ColorMod::LumOff(0.4)]),
            "95B3D7",
            1,
        );
        close(apply_mods(base, &[ColorMod::LumMod(0.75)]), "366092", 1);
        close(
            apply_mods(base, &[ColorMod::LumMod(0.2), ColorMod::LumOff(0.8)]),
            "DCE6F2",
            1,
        );
        close(apply_mods(base, &[ColorMod::LumMod(0.5)]), "244061", 1);
    }

    #[test]
    fn tint_and_shade_work_in_linear_light() {
        // linear 0.5 is sRGB 0.7354 = 187.5 -> 188 (IEC 61966-2-1)
        close(apply_mod(Rgba::BLACK, ColorMod::Tint(0.5)), "BCBCBC", 1);
        close(apply_mod(Rgba::WHITE, ColorMod::Shade(0.5)), "BCBCBC", 1);
        // 100 % tint and shade leave the colour alone
        let base = Rgba::from_hex(ACCENT1).unwrap();
        close(apply_mod(base, ColorMod::Tint(1.0)), ACCENT1, 1);
        close(apply_mod(base, ColorMod::Shade(1.0)), ACCENT1, 1);
        // 0 % tint is white, 0 % shade is black
        close(apply_mod(base, ColorMod::Tint(0.0)), "FFFFFF", 0);
        close(apply_mod(base, ColorMod::Shade(0.0)), "000000", 0);
    }

    #[test]
    fn sat_hue_and_comp() {
        let red = Rgba::rgb(255, 0, 0);
        close(apply_mod(red, ColorMod::HueOff(120.0)), "00FF00", 0);
        close(apply_mod(red, ColorMod::Comp), "00FFFF", 0);
        close(apply_mod(red, ColorMod::Hue(240.0)), "0000FF", 0);
        close(apply_mod(red, ColorMod::SatMod(0.0)), "808080", 0);
        close(apply_mod(red, ColorMod::SatOff(-1.0)), "808080", 0);
        // satMod above 100 % clamps at fully saturated
        let c = Rgba::from_hex("4F81BD").unwrap();
        let (_, s, _) = {
            let m = apply_mod(c, ColorMod::SatMod(5.0));
            rgb_to_hsl(m.r, m.g, m.b)
        };
        assert!((s - 1.0).abs() < 0.01);
        // hueMod halves the hue: blue 240 -> 120 green
        close(
            apply_mod(Rgba::rgb(0, 0, 255), ColorMod::HueMod(0.5)),
            "00FF00",
            0,
        );
        // lum is absolute
        close(apply_mod(red, ColorMod::Lum(0.0)), "000000", 0);
        close(apply_mod(red, ColorMod::Lum(1.0)), "FFFFFF", 0);
    }

    #[test]
    fn alpha_transforms_compose() {
        let c = Rgba::rgb(10, 20, 30);
        assert_eq!(apply_mod(c, ColorMod::Alpha(0.4)).a, 0.4);
        let m = apply_mods(c, &[ColorMod::Alpha(0.5), ColorMod::AlphaMod(0.5)]);
        assert!((m.a - 0.25).abs() < 1e-9);
        let m = apply_mods(c, &[ColorMod::Alpha(0.5), ColorMod::AlphaOff(0.75)]);
        assert_eq!(m.a, 1.0);
        let m = apply_mods(c, &[ColorMod::AlphaOff(-3.0)]);
        assert_eq!(m.a, 0.0);
        // colour channels untouched
        assert_eq!((m.r, m.g, m.b), (10, 20, 30));
    }

    #[test]
    fn inverse_gray_and_gamma() {
        close(apply_mod(Rgba::BLACK, ColorMod::Inv), "FFFFFF", 0);
        close(apply_mod(Rgba::WHITE, ColorMod::Inv), "000000", 0);
        // LibreOffice weights (22, 72, 6)/100: pure green 255 -> 183
        let g = apply_mod(Rgba::rgb(0, 255, 0), ColorMod::Gray);
        assert_eq!((g.r, g.g, g.b), (183, 183, 183));
        // gamma then invGamma is the identity (up to rounding)
        let base = Rgba::from_hex(ACCENT1).unwrap();
        let back = apply_mods(base, &[ColorMod::Gamma, ColorMod::InvGamma]);
        close(back, ACCENT1, 1);
        // gamma brightens mid tones
        let mid = apply_mod(Rgba::rgb(128, 128, 128), ColorMod::Gamma);
        assert!(mid.r > 128);
    }

    #[test]
    fn channel_transforms_use_linear_light() {
        let c = Rgba::rgb(0, 0, 0);
        // absolute red 1.0 -> full red, other channels 0
        close(apply_mod(c, ColorMod::Red(1.0)), "FF0000", 0);
        close(apply_mod(c, ColorMod::GreenOff(1.0)), "00FF00", 0);
        close(apply_mod(Rgba::WHITE, ColorMod::BlueMod(0.0)), "FFFF00", 0);
        // red 0.5 in linear light is sRGB 188 (see tint test)
        close(apply_mod(c, ColorMod::Red(0.5)), "BC0000", 1);
        close(apply_mod(Rgba::WHITE, ColorMod::RedMod(0.5)), "BCFFFF", 1);
        close(apply_mod(c, ColorMod::BlueOff(0.5)), "0000BC", 1);
        close(apply_mod(c, ColorMod::Green(0.0)), "000000", 0);
        close(apply_mod(c, ColorMod::RedOff(0.0)), "000000", 0);
        close(apply_mod(Rgba::WHITE, ColorMod::GreenMod(0.5)), "FFBCFF", 1);
        close(apply_mod(Rgba::WHITE, ColorMod::Blue(0.0)), "FFFF00", 0);
    }

    #[test]
    fn non_finite_inputs_never_poison_the_colour() {
        let c = Rgba::rgb(100, 100, 100);
        for m in [
            ColorMod::LumMod(f64::NAN),
            ColorMod::LumOff(f64::INFINITY),
            ColorMod::Tint(f64::NAN),
            ColorMod::Shade(f64::NEG_INFINITY),
            ColorMod::HueOff(f64::NAN),
            ColorMod::Alpha(f64::NAN),
            ColorMod::Red(f64::NAN),
        ] {
            let o = apply_mod(c, m);
            assert!(o.a.is_finite());
        }
    }

    #[test]
    fn transfer_functions_round_trip() {
        for i in 0..=255 {
            let c = i as f64 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(c)) - c).abs() < 1e-9);
        }
        assert_eq!(linear_to_srgb(2.0), 1.0);
        assert_eq!(srgb_to_linear(-1.0), 0.0);
    }

    #[test]
    fn preset_names() {
        let hex = |n: &str| {
            let c = preset_color(n).unwrap_or_else(|| panic!("{n}"));
            format!("{:02X}{:02X}{:02X}", c.r, c.g, c.b)
        };
        assert_eq!(hex("red"), "FF0000");
        assert_eq!(hex("Red"), "FF0000");
        assert_eq!(hex("dkBlue"), "00008B");
        assert_eq!(hex("darkBlue"), "00008B");
        assert_eq!(hex("ltGray"), "D3D3D3");
        assert_eq!(hex("medPurple"), "9370DB");
        assert_eq!(hex("mediumPurple"), "9370DB");
        assert_eq!(hex("ltGoldenrodYellow"), "FAFAD2");
        assert_eq!(hex("gray"), "808080");
        assert_eq!(hex("green"), "008000");
        assert_eq!(hex("lime"), "00FF00");
        assert_eq!(hex("dkSlateGrey"), "2F4F4F");
        assert_eq!(hex("whiteSmoke"), "F5F5F5");
        assert!(preset_color("notAColour").is_none());
        // every table entry is reachable and distinct names do not collide
        for (n, v) in PRESET {
            let c = preset_color(n).unwrap();
            assert_eq!(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32, *v);
        }
    }

    #[test]
    fn system_names() {
        assert_eq!(system_color("windowText"), Some(Rgba::BLACK));
        assert_eq!(system_color("window"), Some(Rgba::WHITE));
        assert!(system_color("nope").is_none());
    }

    #[test]
    fn mix_and_hex() {
        let m = mix(Rgba::BLACK, Rgba::WHITE, 0.5);
        assert_eq!((m.r, m.g, m.b), (128, 128, 128));
        assert_eq!(mix(Rgba::BLACK, Rgba::WHITE, f64::NAN).r, 128);
        assert_eq!(Rgba::from_hex("#0a0B0c"), Some(Rgba::rgb(10, 11, 12)));
        assert_eq!(Rgba::from_hex("12345"), None);
        assert_eq!(Rgba::from_hex("zzzzzz"), None);
        assert_eq!(Rgba::from_hex("ééé"), None);
    }
}
