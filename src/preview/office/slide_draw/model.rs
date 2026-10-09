//! The format-neutral drawing model of one slide.
//!
//! The PowerPoint and OpenDocument readers lower their slides into a [`SlideScene`]; [`super::render_svg`]
//! turns a scene into an SVG. The model knows nothing about either file format.
//!
//! # Units
//!
//! * All geometry is in **EMU** as `f64` (1 in = 914 400 EMU, 1 pt = 12 700 EMU, 1 px at 96 dpi
//!   = 9 525 EMU), with the origin at the top-left corner of the slide and y growing downwards.
//! * Angles are in degrees, clockwise, as in DrawingML.
//! * Font sizes are in points; paragraph margins / indents / line widths are in EMU.
//! * Fractions (`crop`, gradient stops, alpha, ...) are `0.0..=1.0` unless stated otherwise.
//!
//! # What is deliberately not in the model
//!
//! There is **no table item and no chart item**. A format reader lowers a table, a chart or a
//! SmartArt drawing into the shapes, lines and text boxes it is made of (cell fills become
//! rectangles, cell borders become lines, cell text becomes a text-carrying rectangle, a bar of a
//! chart becomes a rectangle, ...). Keeping the model this small is what lets both formats share
//! one renderer and lets the renderer be budgeted and tested on its own.
//!
//! Every type derives `Debug`/`Clone`/`PartialEq`; nothing here holds a reference, so a scene can
//! be built on one thread and drawn on another (or in a child process).

/// EMU per pixel at 96 dpi: the scale between the model and the SVG the renderer writes.
pub const EMU_PER_PX: f64 = 9525.0;
/// EMU per point.
pub const EMU_PER_PT: f64 = 12700.0;
/// EMU per inch.
pub const EMU_PER_IN: f64 = 914_400.0;

/// An sRGB colour with straight alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// Opacity, `0.0` (transparent) to `1.0` (opaque).
    pub a: f64,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0.0,
    };

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 1.0 }
    }

    /// A colour with an alpha.
    pub const fn new(r: u8, g: u8, b: u8, a: f64) -> Rgba {
        Rgba { r, g, b, a }
    }

    /// Parses `RRGGBB` (with or without a leading `#`); `None` for anything else.
    pub fn from_hex(s: &str) -> Option<Rgba> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.is_ascii() {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    /// Returns the colour with its alpha replaced.
    pub fn with_alpha(self, a: f64) -> Rgba {
        Rgba { a, ..self }
    }
}

impl Default for Rgba {
    fn default() -> Self {
        Rgba::BLACK
    }
}

/// A point, in the coordinate space of whatever contains it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Pt {
        Pt { x, y }
    }
}

/// A rectangle given as (left, top, right, bottom). Used for crops (fractions), insets and
/// text rectangles (EMU); the field the rectangle sits in says which.
pub type Rect4 = (f64, f64, f64, f64);

/// A box with a rotation and flips.
///
/// DrawingML semantics: `(x, y, w, h)` is the box *before* rotation; the flips mirror the content
/// inside the box, and then the box is rotated clockwise by `rot_deg` about its **centre**.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Xfrm {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub rot_deg: f64,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Xfrm {
    /// An unrotated, unflipped box.
    pub const fn rect(x: f64, y: f64, w: f64, h: f64) -> Xfrm {
        Xfrm {
            x,
            y,
            w,
            h,
            rot_deg: 0.0,
            flip_h: false,
            flip_v: false,
        }
    }
}

/// One slide, ready to draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SlideScene {
    /// Slide size (EMU).
    pub width: f64,
    pub height: f64,
    /// Painted first, over the whole slide.
    pub background: Fill,
    /// In paint order, back to front.
    pub items: Vec<Item>,
    /// The reader stopped at a budget of its own: something is missing from `items`.
    pub truncated: bool,
}

/// One thing on a slide.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Shape(ShapeItem),
    Picture(PictureItem),
    Group(GroupItem),
}

/// A geometric shape with a fill, an outline and optionally text.
///
/// Tables, charts and SmartArt are lowered by the readers into several of these (see the module
/// documentation).
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeItem {
    pub xfrm: Xfrm,
    pub geom: Geometry,
    pub fill: Fill,
    pub line: Option<Line>,
    pub text: Option<TextBody>,
    /// Where the text goes, in the shape's own box coordinates (0..`xfrm.w`, 0..`xfrm.h`), or
    /// `None` for the whole box. Presets define it (an ellipse's inscribed rectangle, ...).
    pub text_rect: Option<Rect4>,
    pub effects: Effects,
}

impl ShapeItem {
    /// A shape with no fill, no line, no text and no effects.
    pub fn new(xfrm: Xfrm, geom: Geometry) -> ShapeItem {
        ShapeItem {
            xfrm,
            geom,
            fill: Fill::None,
            line: None,
            text: None,
            text_rect: None,
            effects: Effects::default(),
        }
    }
}

/// A picture, clipped to a geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureItem {
    pub xfrm: Xfrm,
    /// The picture (its `key` names the bytes, `crop` the visible part of them). Only
    /// [`ImageMode::Stretch`] makes sense here; `Tile` is drawn as `Stretch` over the box.
    pub image: ImageFill,
    /// The clip shape; [`Geometry::Rect`] by default.
    pub geom: Geometry,
    pub line: Option<Line>,
    pub effects: Effects,
}

impl PictureItem {
    /// A rectangular picture without an outline.
    pub fn new(xfrm: Xfrm, key: impl Into<String>) -> PictureItem {
        PictureItem {
            xfrm,
            image: ImageFill::stretch(key),
            geom: Geometry::Rect,
            line: None,
            effects: Effects::default(),
        }
    }
}

/// A group (DrawingML `grpSp`).
///
/// The children are placed in the group's *child coordinate space*, the rectangle
/// `child_off .. child_off + child_ext`, which is mapped onto the group's own box `xfrm`; then the
/// group's flips and rotation apply to the whole. Groups nest to any depth the reader allows.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupItem {
    /// Where the group sits on its parent (box, rotation, flips).
    pub xfrm: Xfrm,
    pub child_off: (f64, f64),
    pub child_ext: (f64, f64),
    pub items: Vec<Item>,
}

// ---------------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------------

/// The outline of a shape or the clip of a picture.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Geometry {
    /// The box itself.
    #[default]
    Rect,
    /// The ellipse inscribed in the box.
    Ellipse,
    /// A straight line from the top-left corner of the box to the bottom-right one (use the flips
    /// to get the other diagonal; a zero `w` or `h` gives a vertical / horizontal line).
    Line,
    /// Arbitrary paths (custom geometry, or a preset already evaluated by the reader).
    Paths(Vec<GeomPath>),
}

/// How a path is filled relative to the shape fill (DrawingML `path@fill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathFill {
    /// The shape fill as it is.
    #[default]
    Norm,
    /// Not filled.
    None,
    Lighten,
    LightenLess,
    Darken,
    DarkenLess,
}

/// One path of a custom geometry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeomPath {
    /// The path's own coordinate space; the commands are scaled by `box / (w, h)`. `0` on an axis
    /// means "use the box" (no scaling on that axis).
    pub w: f64,
    pub h: f64,
    pub fill_mode: PathFill,
    /// Whether the outline is stroked (`path@stroke`).
    pub stroke: bool,
    pub cmds: Vec<PathCmd>,
}

/// A path command, in the path's coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo(Pt),
    LineTo(Pt),
    QuadTo(Pt, Pt),
    CubicTo(Pt, Pt, Pt),
    /// DrawingML `arcTo`: an elliptical arc with radii `wr`/`hr` that **continues from the current
    /// point**, which lies on the ellipse at angle `st_deg`; it sweeps `sw_deg` degrees (positive =
    /// clockwise on screen). Angles are measured on the (stretched) ellipse as DrawingML does: the
    /// angle is the visual angle, so the start point is derived from the ellipse parametrisation
    /// `atan2(sin * wr, cos * hr)`.
    ArcTo {
        wr: f64,
        hr: f64,
        st_deg: f64,
        sw_deg: f64,
    },
    Close,
}

// ---------------------------------------------------------------------------------------------
// Fill
// ---------------------------------------------------------------------------------------------

/// How an area is painted.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Fill {
    /// Not painted.
    #[default]
    None,
    Solid(Rgba),
    Gradient(Gradient),
    /// A two-colour preset pattern (`pct5`, `horz`, `dnDiag`, ...).
    Pattern {
        preset: String,
        fg: Rgba,
        bg: Rgba,
    },
    Image(ImageFill),
}

impl Fill {
    /// Whether anything would be painted.
    pub fn is_visible(&self) -> bool {
        match self {
            Fill::None => false,
            Fill::Solid(c) => c.a > 0.0,
            _ => true,
        }
    }
}

/// The shape of a gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradKind {
    /// `angle_deg` is the direction of the gradient line, clockwise from "to the right".
    /// `scaled`: the angle is scaled with the box's aspect ratio (`lin@scaled`).
    Linear { angle_deg: f64, scaled: bool },
    /// Circular, centred on the focus rectangle (`fill_to_rect`).
    Radial,
    /// Rectangular, growing from the focus rectangle to the box edges.
    Rect,
    /// Follows the shape's outline from the focus rectangle outwards.
    Path,
    /// An OpenDocument `ellipsoid` gradient, turned by `angle_deg` (clockwise) about the centre
    /// of the box. LibreOffice does not draw a scaled circle: the colour at a point is its
    /// distance from a segment through the centre (along the longer side of the box, turned),
    /// over the radius `short side * sqrt(2) / 2`, and the segment is `(long side - short side) *
    /// sqrt(2) / 2` either side of the centre. A box that is square is a circle. (Measured on
    /// LibreOffice's renderings at 0, 30, 60 and 90 degrees.)
    Ellipsoid { angle_deg: f64 },
    /// [`GradKind::Rect`] turned by `angle_deg` (clockwise) about the centre of the box (an
    /// OpenDocument `square` or `rectangular` gradient with an angle); what the turned rectangle
    /// does not reach is the last colour.
    RectRotated { angle_deg: f64 },
}

/// A gradient fill.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub kind: GradKind,
    /// `(position 0..1, colour)`, in ascending position order.
    pub stops: Vec<(f64, Rgba)>,
    /// `fillToRect` as fractions of the box: where a radial / rect / path gradient starts.
    pub fill_to_rect: Rect4,
    /// Whether the gradient turns with the shape (`gradFill@rotWithShape`).
    pub rot_with_shape: bool,
}

impl Gradient {
    /// A linear gradient.
    pub fn linear(angle_deg: f64, stops: Vec<(f64, Rgba)>) -> Gradient {
        Gradient {
            kind: GradKind::Linear {
                angle_deg,
                scaled: false,
            },
            stops,
            fill_to_rect: (0.5, 0.5, 0.5, 0.5),
            rot_with_shape: true,
        }
    }
}

/// Where a tile starts inside its area (`a:tile@algn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RectAlign {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

/// Mirroring of alternate tiles (`a:tile@flip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TileFlip {
    #[default]
    None,
    X,
    Y,
    Xy,
}

/// How an image is laid into its area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageMode {
    /// Stretched over the area. `fill_rect` (`a:stretch/a:fillRect`) is the inset of the image
    /// from the area as fractions (l, t, r, b); negative values let it overflow.
    Stretch { fill_rect: Rect4 },
    /// Repeated. `sx`/`sy` are the scale (1.0 = native size at the image's own dpi, which the
    /// renderer takes as 96), `tx`/`ty` the offset of the first tile (EMU).
    Tile {
        sx: f64,
        sy: f64,
        tx: f64,
        ty: f64,
        align: RectAlign,
        flip: TileFlip,
    },
}

/// A picture used as a fill or as a picture item's content.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageFill {
    /// Names the image bytes; resolved through the media callback of [`super::render_svg`].
    pub key: String,
    /// The visible part of the image as fractions removed from each side (l, t, r, b);
    /// negative values add empty space.
    pub crop: Rect4,
    pub mode: ImageMode,
    /// Opacity, `0..=1`.
    pub alpha: f64,
    /// Colour effects of the picture (`a:blip` children), applied in order to the decoded
    /// pixels before the picture is embedded (see [`super::pic_fx`]).
    pub fx: Vec<PicFx>,
    /// Enlarged without smoothing (nearest-neighbour): LibreOffice draws an upscaled bitmap
    /// *fill* that way (a picture item is smoothed). PowerPoint smooths, so the pptx reader
    /// leaves this off. Has no effect on a picture that is shrunk.
    pub pixelated: bool,
}

/// A colour effect of a picture (a child of DrawingML `a:blip`).
#[derive(Debug, Clone, PartialEq)]
pub enum PicFx {
    /// `a:grayscl`.
    Grayscale,
    /// `a:biLevel`: luminance at or above `thresh` (`0..=1`) becomes white, below black.
    BiLevel { thresh: f64 },
    /// `a:duotone`: the picture's luminance mapped between two colours (`0` = `dark`, `1` = `light`).
    Duotone { dark: Rgba, light: Rgba },
    /// `a:clrChange`: pixels of colour `from` become `to` (its alpha only when `use_alpha`).
    ClrChange {
        from: Rgba,
        to: Rgba,
        use_alpha: bool,
    },
    /// `a:clrRepl`: every pixel becomes the colour, the picture's alpha is kept.
    ClrRepl(Rgba),
    /// `a:lum`: brightness and contrast, both `-1..=1`.
    Lum { bright: f64, contrast: f64 },
    /// `a:hsl`: hue shift in degrees, saturation and luminance offsets `-1..=1`.
    Hsl { hue: f64, sat: f64, lum: f64 },
    /// `a:tint`: shift toward (positive `amt`) or away from a hue (degrees), `amt` in `-1..=1`.
    Tint { hue: f64, amt: f64 },
}

impl ImageFill {
    /// A full image stretched over its area.
    pub fn stretch(key: impl Into<String>) -> ImageFill {
        ImageFill {
            key: key.into(),
            crop: (0.0, 0.0, 0.0, 0.0),
            mode: ImageMode::Stretch {
                fill_rect: (0.0, 0.0, 0.0, 0.0),
            },
            alpha: 1.0,
            fx: Vec::new(),
            pixelated: false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Line
// ---------------------------------------------------------------------------------------------

/// Dash pattern. The presets are given in multiples of the line width as PowerPoint does
/// (`dot` = 1:3, `dash` = 4:3, `lgDash` = 8:3, `dashDot` = 4:3:1:3, ...); `Custom` holds
/// `(dash, space)` pairs in multiples of the line width too (DrawingML `custDash/ds`, where the
/// values are 1000ths of a percent -- the reader divides them).
#[derive(Debug, Clone, PartialEq, Default)]
#[allow(clippy::enum_variant_names)] // DrawingML's own preset names
pub enum Dash {
    #[default]
    Solid,
    Dot,
    Dash,
    LgDash,
    DashDot,
    LgDashDot,
    LgDashDotDot,
    SysDash,
    SysDot,
    SysDashDot,
    SysDashDotDot,
    Custom(Vec<(f64, f64)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cap {
    /// Butt.
    #[default]
    Flat,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Join {
    #[default]
    Round,
    Bevel,
    /// With the miter limit (a ratio; DrawingML `miter@lim` / 100000).
    Miter(f64),
}

/// Compound (multi-stroke) lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compound {
    #[default]
    Sng,
    Dbl,
    ThickThin,
    ThinThick,
    Tri,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowSize {
    Sm,
    Med,
    Lg,
}

/// The kind of an arrow head / tail.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrowKind {
    Triangle,
    Stealth,
    Diamond,
    Oval,
    /// An open arrow head (two strokes).
    Arrow,
    /// A reader-supplied head: filled paths in a coordinate space of their own (`w`/`h` of each
    /// path), with the **tip at the right-middle** (`w`, `h/2`) pointing along +x and the base at
    /// x = 0. Used for the heads of ODF (`draw:marker`).
    Custom(Vec<GeomPath>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arrow {
    pub kind: ArrowKind,
    /// Width of the head across the line.
    pub w: ArrowSize,
    /// Length of the head along the line.
    pub len: ArrowSize,
}

/// An outline.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// EMU. `0` draws a hairline (1 px).
    pub width: f64,
    pub fill: Fill,
    pub dash: Dash,
    pub cap: Cap,
    pub join: Join,
    pub compound: Compound,
    /// At the start of the path (first point).
    pub head: Option<Arrow>,
    /// At the end of the path (last point).
    pub tail: Option<Arrow>,
}

impl Line {
    /// A solid single line of the given width and colour.
    pub fn solid(width: f64, color: Rgba) -> Line {
        Line {
            width,
            fill: Fill::Solid(color),
            dash: Dash::Solid,
            cap: Cap::Flat,
            join: Join::Round,
            compound: Compound::Sng,
            head: None,
            tail: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------------------------

/// A drop shadow (outer or inner).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Blur radius (EMU).
    pub blur_rad: f64,
    /// Distance from the shape (EMU).
    pub dist: f64,
    /// Direction, degrees clockwise from "to the right".
    pub dir_deg: f64,
    pub color: Rgba,
    /// Horizontal / vertical scale of the shadow (1.0 = same size).
    pub sx: f64,
    pub sy: f64,
    pub rot_with_shape: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    /// Radius (EMU).
    pub rad: f64,
    pub color: Rgba,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reflection {
    pub blur_rad: f64,
    pub start_alpha: f64,
    pub end_alpha: f64,
    /// Where along the reflection's height (`0..=1`, from the shape's edge) the fade starts
    /// (`stPos`, default 0) and ends (`endPos`, default 1).
    pub start_pos: f64,
    pub end_pos: f64,
    pub dist: f64,
    pub dir_deg: f64,
    pub fade_dir_deg: f64,
    pub sx: f64,
    pub sy: f64,
}

/// Visual effects. The model carries all of them; see [`super::render_svg`] for which are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Effects {
    pub outer_shadow: Option<Shadow>,
    pub inner_shadow: Option<Shadow>,
    pub glow: Option<Glow>,
    /// Soft edge radius (EMU).
    pub soft_edge: Option<f64>,
    pub reflection: Option<Reflection>,
}

// ---------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Text direction (`bodyPr@vert`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::enum_variant_names)] // DrawingML's own names
pub enum Vert {
    #[default]
    Horz,
    /// Rotated 90 degrees clockwise (lines run top to bottom).
    Vert,
    /// Rotated 90 degrees counter-clockwise.
    Vert270,
    /// East-Asian vertical: upright CJK glyphs, columns running right to left, Latin rotated.
    EaVert,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AutoFit {
    #[default]
    None,
    /// `normAutofit`: the saved scales. `font_scale` multiplies every size (`0.9` = 90 %),
    /// `ln_spc_reduction` is subtracted from percentage line spacing (`0.1` = 10 points of percent).
    Normal {
        font_scale: f64,
        ln_spc_reduction: f64,
    },
    /// `spAutoFit`: the shape grows to fit its text. The renderer does not resize shapes; the text
    /// is laid out in the box as it is.
    Shape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
    Distributed,
}

/// Paragraph or line spacing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spacing {
    /// A fraction of the font size (for `spc_before`/`spc_after`) or of the single line height
    /// (for `line_spacing`); `1.0` = 100 %.
    Pct(f64),
    /// Exact points.
    Pts(f64),
}

impl Default for Spacing {
    fn default() -> Self {
        Spacing::Pts(0.0)
    }
}

/// The text of a shape.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBody {
    /// (left, top, right, bottom) insets in EMU.
    pub insets: Rect4,
    pub anchor: Anchor,
    /// `anchorCtr`: the text block is centred horizontally as a whole (its width being the widest
    /// line).
    pub anchor_ctr: bool,
    /// Whether lines wrap at the right edge of the text rectangle.
    pub wrap: bool,
    pub vert: Vert,
    pub autofit: AutoFit,
    /// `bodyPr@rot`: extra rotation of the text block (degrees).
    pub rot_deg: f64,
    /// `bodyPr@upright`: the text stays upright when the shape is rotated.
    pub upright: bool,
    /// Number of text columns (>= 1).
    pub columns: u32,
    pub paragraphs: Vec<Paragraph>,
}

impl Default for TextBody {
    fn default() -> Self {
        TextBody {
            insets: (91440.0, 45720.0, 91440.0, 45720.0),
            anchor: Anchor::Top,
            anchor_ctr: false,
            wrap: true,
            vert: Vert::Horz,
            autofit: AutoFit::None,
            rot_deg: 0.0,
            upright: false,
            columns: 1,
            paragraphs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub align: Align,
    /// Outline level (0-based); informational for the readers, the layout uses `mar_l`/`indent`.
    pub level: u8,
    /// Left margin of the paragraph (EMU).
    pub mar_l: f64,
    /// Indent of the first line relative to `mar_l` (EMU; negative = hanging).
    pub indent: f64,
    pub spc_before: Spacing,
    pub spc_after: Spacing,
    pub line_spacing: Spacing,
    pub bullet: Option<Bullet>,
    pub runs: Vec<Run>,
    /// Font size (pt) that gives an empty paragraph its height.
    pub end_size_pt: f64,
    /// Right-to-left paragraph: `Left`/`Right` are mirrored.
    pub rtl: bool,
    /// Explicit tab stops, positions in EMU from the left edge of the text rectangle.
    pub tabs: Vec<TabStop>,
    /// Distance of the default tab stops (EMU); `0` or less = [`DEFAULT_TAB_EMU`]. They apply
    /// after the last explicit stop.
    pub def_tab: f64,
}

/// The default distance of tab stops: one inch (DrawingML `defTabSz`).
pub const DEFAULT_TAB_EMU: f64 = 914_400.0;

/// How text at a tab stop is aligned to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAlign {
    Left,
    Center,
    Right,
    /// Aligned at the decimal point; drawn like [`TabAlign::Left`].
    Decimal,
}

/// An explicit tab stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabStop {
    /// Position in EMU from the left edge of the text rectangle.
    pub pos: f64,
    pub align: TabAlign,
}

impl Default for Paragraph {
    fn default() -> Self {
        Paragraph {
            align: Align::Left,
            level: 0,
            mar_l: 0.0,
            indent: 0.0,
            spc_before: Spacing::Pts(0.0),
            spc_after: Spacing::Pts(0.0),
            line_spacing: Spacing::Pct(1.0),
            bullet: None,
            runs: Vec::new(),
            end_size_pt: 18.0,
            rtl: false,
            tabs: Vec::new(),
            def_tab: DEFAULT_TAB_EMU,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BulletKind {
    /// A literal character (or short string).
    Char(String),
    /// An automatic number. `scheme` is the DrawingML `buAutoNum@type` (`arabicPeriod`, ...);
    /// `start` is the first number. Numbering runs over consecutive paragraphs that have the same
    /// scheme at the same level.
    AutoNum { scheme: String, start: u32 },
    /// A picture bullet.
    Picture(ImageFill),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BulletSize {
    /// Fraction of the first run's size (`1.0` = same).
    Pct(f64),
    Pts(f64),
    FollowText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bullet {
    pub kind: BulletKind,
    pub font: Option<FontSpec>,
    /// `None` follows the text colour.
    pub color: Option<Rgba>,
    pub size: BulletSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunKind {
    #[default]
    Text,
    /// A forced line break (`a:br`); `text` is ignored.
    LineBreak,
    /// A field (slide number, date, ...): `text` is the cached value, drawn like text.
    Field,
}

/// The typefaces of a run, per script. `None` inherits the default (the reader resolves theme
/// fonts to names before building the model).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FontSpec {
    pub latin: Option<String>,
    pub east_asian: Option<String>,
    pub complex: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strike {
    #[default]
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Caps {
    #[default]
    None,
    All,
    Small,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub kind: RunKind,
    pub font: FontSpec,
    pub size_pt: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub strike: Strike,
    /// The text colour (`Solid` in practice; other fills are drawn with their first colour).
    pub fill: Fill,
    pub highlight: Option<Rgba>,
    /// Super/subscript offset in percent of the font size (`30.0` = superscript 30 %, `-25.0` =
    /// subscript); `0` = baseline. Super/subscripts are drawn at 2/3 of the size.
    pub baseline_pct: f64,
    /// Extra spacing between characters (pt).
    pub spacing_pt: f64,
    pub caps: Caps,
    pub lang: Option<String>,
}

impl Default for Run {
    fn default() -> Self {
        Run {
            text: String::new(),
            kind: RunKind::Text,
            font: FontSpec::default(),
            size_pt: 18.0,
            bold: false,
            italic: false,
            underline: Underline::None,
            strike: Strike::None,
            fill: Fill::Solid(Rgba::BLACK),
            highlight: None,
            baseline_pct: 0.0,
            spacing_pt: 0.0,
            caps: Caps::None,
            lang: None,
        }
    }
}

impl Run {
    /// A plain text run of the given size.
    pub fn text(text: impl Into<String>, size_pt: f64) -> Run {
        Run {
            text: text.into(),
            size_pt,
            ..Run::default()
        }
    }
}
