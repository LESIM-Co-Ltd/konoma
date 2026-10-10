//! Picture colour effects (`a:blip` children) through the reader: parsed in document order into
//! [`sd::PicFx`], applied when the picture is drawn.

use super::*;

/// A deck whose one shape has a picture fill with `blip_children` inside the `a:blip`.
fn deck_with_blip(blip_children: &str) -> D {
    let fill = format!(
        r#"<a:blipFill><a:blip r:embed="rIdP">{blip_children}</a:blip><a:stretch><a:fillRect/></a:stretch></a:blipFill>"#
    );
    with_media(D::new(&shape(
        "",
        &format!("{}{RECT}{fill}", xf(0, 0, 100, 100)),
        "",
        "",
        "",
        "",
    )))
}

fn fx_of(blip_children: &str) -> Vec<sd::PicFx> {
    let d = deck_with_blip(blip_children);
    let Fill::Image(img) = first_shape(&d.scene()).fill.clone() else {
        panic!("not a picture fill")
    };
    img.fx
}

#[test]
fn duotone_takes_its_two_colours_through_the_theme_and_transforms() {
    let fx = fx_of(
        r#"<a:duotone><a:srgbClr val="102030"/><a:schemeClr val="accent1"><a:lumMod val="50000"/></a:schemeClr></a:duotone>"#,
    );
    let [sd::PicFx::Duotone { dark, light }] = fx.as_slice() else {
        panic!("{fx:?}")
    };
    assert_eq!(*dark, Rgba::rgb(0x10, 0x20, 0x30));
    // accent1 4472C4 at half luminance: darker than the accent, still blue-ish
    assert!(light.b > light.r && light.b < 0xC4, "{light:?}");
}

#[test]
fn a_duotone_with_fewer_than_two_colours_is_ignored() {
    assert!(fx_of(r#"<a:duotone><a:srgbClr val="102030"/></a:duotone>"#).is_empty());
    assert!(fx_of(r#"<a:duotone/>"#).is_empty());
}

#[test]
fn simple_effects_are_parsed_in_document_order() {
    let fx = fx_of(
        r#"<a:grayscl/><a:biLevel thresh="25000"/><a:lum bright="20000" contrast="-40000"/><a:hsl hue="5400000" sat="10000" lum="-10000"/><a:tint hue="10800000" amt="50000"/><a:clrRepl><a:srgbClr val="FF0000"><a:alpha val="50000"/></a:srgbClr></a:clrRepl>"#,
    );
    assert_eq!(
        fx,
        vec![
            sd::PicFx::Grayscale,
            sd::PicFx::BiLevel { thresh: 0.25 },
            sd::PicFx::Lum {
                bright: 0.2,
                contrast: -0.4
            },
            sd::PicFx::Hsl {
                hue: 90.0,
                sat: 0.1,
                lum: -0.1
            },
            sd::PicFx::Tint {
                hue: 180.0,
                amt: 0.5
            },
            sd::PicFx::ClrRepl(Rgba::new(255, 0, 0, 0.5)),
        ]
    );
}

#[test]
fn bilevel_without_a_threshold_is_the_middle() {
    assert_eq!(
        fx_of(r#"<a:biLevel/>"#),
        vec![sd::PicFx::BiLevel { thresh: 0.5 }]
    );
}

#[test]
fn clr_change_reads_from_to_and_use_a() {
    let fx = fx_of(
        r#"<a:clrChange><a:clrFrom><a:srgbClr val="FFFFFF"/></a:clrFrom><a:clrTo><a:srgbClr val="FFFFFF"><a:alpha val="0"/></a:srgbClr></a:clrTo></a:clrChange><a:clrChange useA="0"><a:clrFrom><a:srgbClr val="000000"/></a:clrFrom><a:clrTo><a:srgbClr val="FF0000"/></a:clrTo></a:clrChange>"#,
    );
    assert_eq!(
        fx,
        vec![
            sd::PicFx::ClrChange {
                from: Rgba::rgb(255, 255, 255),
                to: Rgba::new(255, 255, 255, 0.0),
                use_alpha: true
            },
            sd::PicFx::ClrChange {
                from: Rgba::rgb(0, 0, 0),
                to: Rgba::rgb(255, 0, 0),
                use_alpha: false
            },
        ]
    );
    // a clrChange missing one side is ignored
    assert!(fx_of(
        r#"<a:clrChange><a:clrFrom><a:srgbClr val="FFFFFF"/></a:clrFrom></a:clrChange>"#
    )
    .is_empty());
}

#[test]
fn alpha_mod_fix_stays_the_alpha_and_is_no_effect() {
    assert!(fx_of(r#"<a:alphaModFix amt="40000"/>"#).is_empty());
}

#[test]
fn a_picture_item_carries_its_effects_and_the_drawing_recolours_the_pixels() {
    let sp_pr = format!("{}{RECT}", xf(0, 0, 914_400, 914_400));
    let mut d = with_media(D::new(&pic_xml(
        "rIdP",
        &sp_pr,
        r#"<a:stretch><a:fillRect/></a:stretch>"#,
        r#"<a:duotone><a:srgbClr val="FF0000"/><a:srgbClr val="FF0000"/></a:duotone>"#,
    )));
    // (the shared `tiny_png` is not a decodable picture)
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    d.media[0].1 = png;
    let doc = d.load();
    let Item::Picture(p) = &doc.slide_scenes[0].items[0] else {
        panic!()
    };
    assert_eq!(p.image.fx.len(), 1);
    let media: std::collections::HashMap<String, std::sync::Arc<Vec<u8>>> = doc
        .images
        .iter()
        .map(|i| (i.key.clone(), std::sync::Arc::new(i.bytes.clone())))
        .collect();
    let r = sd::render_svg(&doc.slide_scenes[0], &|k| media.get(k).cloned());
    let img = crate::preview::svg::rasterize_trusted(
        r.svg.as_bytes(),
        std::path::Path::new("/slide.svg"),
        200,
    )
    .unwrap()
    .to_rgba8();
    // the whole picture is the duotone's single colour, whatever the picture held
    let c = img.get_pixel(14, 4).0;
    assert!(c[0] > 240 && c[1] < 15 && c[2] < 15, "{c:?}");
}

#[test]
fn the_number_of_effects_is_bounded_and_flags_truncation() {
    let many = "<a:grayscl/>".repeat(40);
    let d = deck_with_blip(&many);
    let sc = d.scene();
    let Fill::Image(img) = first_shape(&sc).fill.clone() else {
        panic!()
    };
    assert_eq!(img.fx.len(), super::super::style::MAX_PIC_FX);
    assert!(sc.truncated);
}

#[test]
fn a_picture_the_effects_cannot_be_applied_to_is_still_embedded_as_it_is() {
    // `tiny_png` has a PNG signature but no decodable data: the recolouring fails, the picture is
    // embedded unchanged (not replaced by the grey placeholder cross).
    let sp_pr = format!("{}{RECT}", xf(0, 0, 914_400, 914_400));
    let d = with_media(D::new(&pic_xml(
        "rIdP",
        &sp_pr,
        r#"<a:stretch><a:fillRect/></a:stretch>"#,
        r#"<a:grayscl/>"#,
    )));
    let doc = d.load();
    let media: std::collections::HashMap<String, std::sync::Arc<Vec<u8>>> = doc
        .images
        .iter()
        .map(|i| (i.key.clone(), std::sync::Arc::new(i.bytes.clone())))
        .collect();
    let r = sd::render_svg(&doc.slide_scenes[0], &|k| media.get(k).cloned());
    assert!(r.svg.contains("<image "), "{}", r.svg);
    assert!(!r.svg.contains("#bfbfbf"), "placeholder drawn");
}
