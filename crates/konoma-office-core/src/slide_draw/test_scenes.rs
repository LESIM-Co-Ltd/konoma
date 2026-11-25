//! Scene builders the host's tests share with the core's own (feature `test-hooks`).

use super::model::*;

pub const RED: Rgba = Rgba::rgb(255, 0, 0);

/// A 960 x 540 px slide.
pub const W_PX: f64 = 960.0;
pub const H_PX: f64 = 540.0;

pub fn e(px: f64) -> f64 {
    px * EMU_PER_PX
}

pub fn scene(items: Vec<Item>) -> SlideScene {
    SlideScene {
        width: e(W_PX),
        height: e(H_PX),
        background: Fill::Solid(Rgba::WHITE),
        underlay: Vec::new(),
        items,
        truncated: false,
    }
}

pub fn fx_shape(w: f64, h: f64, effects: Effects) -> Item {
    let mut s = ShapeItem::new(Xfrm::rect(e(20.0), e(20.0), e(w), e(h)), Geometry::Rect);
    s.fill = Fill::Solid(RED);
    s.effects = effects;
    Item::Shape(s)
}

pub fn shadow(blur: f64) -> Effects {
    Effects {
        outer_shadow: Some(Shadow {
            blur_rad: e(blur),
            dist: e(4.0),
            dir_deg: 45.0,
            color: Rgba::new(0, 0, 0, 0.5),
            sx: 1.0,
            sy: 1.0,
            rot_with_shape: true,
        }),
        ..Default::default()
    }
}

/// A 960 x 540 slide with `n` full-slide shadows (about 1.1e8 units of filter work each).
pub fn shadowed_slide(n: usize) -> SlideScene {
    scene(
        (0..n)
            .map(|_| fx_shape(900.0, 500.0, shadow(30.0)))
            .collect(),
    )
}
