//! What differs between the hosts that decode canvas pictures: the read scope, the limits, the
//! cancellation and who draws an SVG. Everything that decides how a picture looks (the decoders,
//! the size caps, the frame delay, the GIF byte budget, the header guards) is fixed in this
//! module's siblings and is not a parameter.

use std::path::{Path, PathBuf};

use image::DynamicImage;
use konoma_svg::svg::RasterError;

use super::CanvasLimits;
use crate::ReadScope;

/// Draws the SVG a plugin hands over when it is not trusted. The host owns this because the safe
/// way to draw an untrusted document is a supervised child process of the host's own executable
/// (konoma: `svg_proc`), which a library cannot start. A host with no such process may draw in
/// its own process (`konoma_svg::svg::rasterize_guarded_within`); the shared code does the
/// cheap refusals (scope, trust) before it calls here.
///
/// Both methods block (call from a worker) and poll `cancelled`; a drawing that is stopped returns
/// [`RasterError::Cancelled`].
pub trait SvgDraw {
    /// Draw `data` that the plugin named a directory for: relative references resolve from the
    /// folder of `base`, and with `roots` set every file the drawing reads must be inside one of
    /// those real directories.
    fn untrusted(
        &self,
        data: &[u8],
        base: &Path,
        roots: Option<&[PathBuf]>,
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError>;

    /// Draw `data` that has no directory to be relative to: the drawing reads no file at all.
    fn embedded(
        &self,
        data: &[u8],
        max_px: u32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<DynamicImage, RasterError>;
}

/// The host's choices for one [`decode_canvas`](super::decode_canvas) call.
pub struct CanvasPolicy<'a> {
    /// The folders the host may read for the plugin ([`ReadScope::unrestricted`] for a standard
    /// plugin).
    pub scope: &'a ReadScope,
    /// The work limits of one answer ([`CanvasLimits::PRODUCTION`]; a test uses smaller ones).
    pub limits: CanvasLimits,
    /// Whether the job was superseded: polled while hashing, between pictures and frames, and by
    /// the decoders that can stop.
    pub cancelled: &'a dyn Fn() -> bool,
    /// Draws the SVGs that are not trusted.
    pub svg: &'a dyn SvgDraw,
    /// Whether the host believes the `trusted` claim of an SVG the plugin returns (decided by
    /// where the plugin comes from, never by what it says).
    pub trust_own_svg: bool,
    /// The longest side, in pixels, an SVG is drawn at (the user's `ui.svg_max_px`).
    pub raster_px: u32,
}

fn never() -> bool {
    false
}

impl<'a> CanvasPolicy<'a> {
    /// The production limits, never cancelled, no SVG trusted.
    pub fn new(scope: &'a ReadScope, svg: &'a dyn SvgDraw, raster_px: u32) -> CanvasPolicy<'a> {
        CanvasPolicy {
            scope,
            limits: CanvasLimits::PRODUCTION,
            cancelled: &never,
            svg,
            trust_own_svg: false,
            raster_px,
        }
    }

    /// The same with `limits`.
    pub fn limits(mut self, limits: CanvasLimits) -> CanvasPolicy<'a> {
        self.limits = limits;
        self
    }

    /// The same with the cancellation poll `cancelled`.
    pub fn cancelled(mut self, cancelled: &'a dyn Fn() -> bool) -> CanvasPolicy<'a> {
        self.cancelled = cancelled;
        self
    }

    /// The same with the plugin's `trusted` SVG claim believed (or not).
    pub fn trust_own_svg(mut self, trust: bool) -> CanvasPolicy<'a> {
        self.trust_own_svg = trust;
        self
    }
}
