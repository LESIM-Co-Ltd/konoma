//! Text measurement for konoma's own mermaid renderer (stage 0).
//!
//! Every mermaid box is sized from the width of its label, and everything downstream — rank
//! assignment, ordering inside a rank, final coordinates — is derived from those box sizes. So a
//! wrong width does not merely make one box ugly, it moves the whole diagram. konoma has already
//! been bitten by this once (2026-07-17: a theme-dependent measuring font made an edge take a
//! detour ring; see `preview::markdown::mermaid_to_svg`).
//!
//! The rule that keeps that from happening again is: **measure with the same font database, and the
//! same font-resolution rules, that resvg will use when it actually draws the SVG.** Concretely
//! that means [`crate::preview::svg::shared_fontdb`] plus a `fontdb::Query` built exactly the way
//! usvg's own `FontResolver::default_font_selector` builds it for `font-family="sans-serif"`, and
//! `skrifa` — the very crate, at the very version, that usvg 0.48 reads glyph metrics with.
//!
//! What this module does *not* do is shape text. It sums horizontal advances and adds pair
//! kerning, where usvg runs a full harfrust shaping pass. Three of harfrust's rules had to be
//! mirrored for the two to agree on real fonts, each because measurement showed they did not:
//! which table the kerning is read from ([`KernSource`]), that only the Latin script's features
//! count, and that a variable face's `opsz` axis follows `font-size`.
//!
//! The difference between the two is what the module's main instrument measures (see the tests): a
//! minimal `<text>` SVG is put through usvg with the same font DB and [`measure`] is compared
//! against `Text::bounding_box().width()`, which usvg computes from the very advances it used to
//! place the glyphs. That comparison is **konoma's own measurement against resvg's real shaper**,
//! never against another copy of itself, so it cannot quietly turn into a self-comparison the way
//! the markdown diff harness did.

// Stage 0 is groundwork: the mermaid parser/layout that will call all of this arrives in stage 1
// (docs/FEATURE-MERMAID-RENDERER.md §7). Until then only the tests exercise it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use resvg::usvg::fontdb;
use skrifa::instance::{Location, LocationRef, Size};
use skrifa::raw::tables::gpos::{ExtensionSubtable, Gpos, PairPos, PositionLookup};
use skrifa::raw::tables::kern::{Kern, SubtableKind};
use skrifa::raw::types::GlyphId16;
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider, Tag};

/// The `font-family` konoma writes into the mermaid SVGs it generates.
///
/// usvg understands exactly five generic families — `serif`, `sans-serif`, `cursive`, `fantasy`,
/// `monospace`. Anything else in a font stack (`system-ui`, `ui-sans-serif`, `-apple-system`, …) is
/// looked up as a *named* family, and when every name misses, usvg's selector appends `Serif` and
/// the diagram silently gets drawn in a serif face that nothing measured. Keeping the attribute to
/// the single generic that we also measure with removes that whole class of mismatch: there is only
/// one name, and [`TextMetrics::resolve`] resolves it the same way usvg will.
pub const FONT_FAMILY: &str = "sans-serif";

/// Layout font size (px), pinned for **every** theme.
///
/// Inherited intent from `mermaid_to_svg` (see `docs/FEATURE-MERMAID-RENDERER.md` §4-3): themes may
/// change colors, never metrics. When each theme measured with its own font/size, label boxes
/// changed size per theme and edge routing changed with them (user-reported detour ring,
/// 2026-07-17). Colors are theme data; the measuring font is not.
pub const FONT_SIZE: f32 = 14.0;

/// Why no font could be used for measuring (and therefore for drawing).
///
/// usvg does not fail when it cannot resolve a font: it drops the glyphs and the `<text>` simply is
/// not there — a diagram of empty boxes, with no error anywhere. So the renderer has to ask *before*
/// it builds an SVG, and degrade to the text diagram when the answer is `Err` (PRD design
/// principle #3: never show a broken preview).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontUnavailable {
    /// The font database has no face for [`FONT_FAMILY`] (nor for usvg's `serif` last resort).
    /// A minimal Linux container with no fonts installed lands here.
    NoMatch,
    /// A face matched but its data could not be parsed as an OpenType face (or reported a
    /// nonsensical units-per-em). Treated the same as "no font": do not draw.
    Unreadable,
}

impl std::fmt::Display for FontUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMatch => write!(f, "no font matches '{FONT_FAMILY}'"),
            Self::Unreadable => write!(f, "the matched font face could not be read"),
        }
    }
}

/// Where a face's kerning comes from — and, therefore, where konoma has to read it from to agree
/// with what resvg draws.
///
/// harfrust (usvg 0.48's shaper) picks exactly one source: if the font has GPOS with a `kern`
/// feature it kerns from GPOS and **ignores the legacy `kern` table entirely**; only when there is
/// no GPOS `kern` does it fall back to that table (`hb/ot_shape.rs`, `apply_gpos` / `apply_kern`).
/// Reading the legacy table for a GPOS font was measured to be wrong by up to 12% on real system
/// faces (`measurement_matches_resvg_across_the_whole_font_book`), so konoma follows the same rule
/// rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernSource {
    /// GPOS `kern` feature; the payload is the lookup indices that feature points at.
    Gpos(Vec<u16>),
    /// The legacy `kern` table (no GPOS `kern` feature in this face).
    LegacyTable,
    /// Nothing this code can read kerns this face. Either the face genuinely does not kern, or its
    /// kerning lives in AAT `kerx` — a state machine that needs a shaper. `kerx` is detected rather
    /// than ignored: harfrust prefers it over the legacy `kern` table, so reading `kern` for such a
    /// face would apply kerning resvg does not (measured at up to 7px on macOS's Indic faces).
    None,
}

/// One character's contribution, in font units.
#[derive(Clone, Copy)]
struct CharAdvance {
    /// Glyph in the measuring face, or `None` when the face has no glyph for this character.
    gid: Option<u32>,
    /// Horizontal advance in font units.
    units: f32,
}

#[derive(Default)]
struct Cache {
    /// Keyed by character **and** optical size: a variable face with an `opsz` axis has different
    /// advances at different sizes, and usvg sets that axis from `font-size` (its
    /// `FontOpticalSizing::Auto` default). Faces without the axis all share key 0.
    chars: HashMap<(char, u32), CharAdvance>,
    /// Pair kerning in font units, keyed by (left glyph, right glyph). Read from whichever source
    /// [`KernSource`] selected for this face.
    kern: HashMap<(u32, u32), i32>,
}

/// Advance-width measurement bound to one resolved face of one font database.
pub struct TextMetrics {
    db: Arc<fontdb::Database>,
    face: fontdb::ID,
    upem: f32,
    /// Where this face's kerning is read from (see [`KernSource`]).
    kern_source: KernSource,
    /// Whether the face has an `opsz` variation axis, which usvg drives from `font-size`.
    has_opsz: bool,
    cache: Mutex<Cache>,
    /// How many times the face data has been opened. Only the cold path touches it; the cache test
    /// uses it as a deterministic stand-in for "did this call do real work" (konoma prefers a
    /// structural guard over a wall-clock one — see `speed_tests.rs`).
    face_opens: AtomicUsize,
}

impl TextMetrics {
    /// Resolve [`FONT_FAMILY`] in `db` **exactly the way usvg will**.
    ///
    /// usvg's `FontResolver::default_font_selector` turns the parsed font stack into a
    /// `fontdb::Family` list and then appends `Family::Serif` as a last resort before calling
    /// `Database::query`. Reproducing that list — rather than querying `SansSerif` alone — is the
    /// point: whatever face comes back here is the face usvg will hand to harfrust, including in the
    /// awkward case where a machine has a serif font but no sans-serif one.
    pub fn resolve(db: Arc<fontdb::Database>) -> Result<Self, FontUnavailable> {
        Self::resolve_families(db, &[fontdb::Family::SansSerif, fontdb::Family::Serif])
    }

    /// [`resolve`](Self::resolve) against an explicit family list. Used by the cross-font test to
    /// bind measurement to a named family and check konoma still agrees with resvg for it, so the
    /// instrument says something about the fonts a Linux box has rather than only about this one.
    pub fn resolve_families(
        db: Arc<fontdb::Database>,
        families: &[fontdb::Family],
    ) -> Result<Self, FontUnavailable> {
        let face = db
            .query(&fontdb::Query {
                families,
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            })
            .ok_or(FontUnavailable::NoMatch)?;
        let probed = db
            .with_face_data(face, |data, index| {
                let font = FontRef::from_index(data, index).ok()?;
                let upem = font
                    .metrics(Size::unscaled(), LocationRef::default())
                    .units_per_em;
                // usvg rejects the same range (see its `ResolvedFont` construction), so a face it
                // would refuse must not be one we measure with either.
                if !(16..=16384).contains(&upem) {
                    return None;
                }
                let has_opsz = font.axes().get_by_tag(Tag::new(b"opsz")).is_some();
                Some((upem as f32, kern_source(&font), has_opsz))
            })
            .flatten();
        let (upem, kern_source, has_opsz) = probed.ok_or(FontUnavailable::Unreadable)?;
        Ok(Self {
            db,
            face,
            upem,
            kern_source,
            has_opsz,
            cache: Mutex::new(Cache::default()),
            face_opens: AtomicUsize::new(0),
        })
    }

    /// The face measurement is bound to. Used by the tests to pin that usvg resolves the same one.
    pub fn face_id(&self) -> fontdb::ID {
        self.face
    }

    /// Units per em of the measuring face.
    pub fn units_per_em(&self) -> f32 {
        self.upem
    }

    /// Where this face's kerning is read from.
    pub fn kern_source(&self) -> &KernSource {
        &self.kern_source
    }

    /// Whether this face kerns at all.
    pub fn kerns(&self) -> bool {
        !matches!(self.kern_source, KernSource::None)
    }

    /// Whether the measuring face lacks a glyph for at least one character of `text` (so usvg would
    /// fall back to another face and konoma's one-em rule takes over).
    pub fn has_missing_glyph(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let key = self.size_key(FONT_SIZE);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        self.fill(&mut cache, &chars, FONT_SIZE, key);
        chars
            .iter()
            .any(|c| cache.chars.get(&(*c, key)).is_none_or(|e| e.gid.is_none()))
    }

    /// Cache bucket for a font size. Only a face with an `opsz` axis needs more than one.
    fn size_key(&self, font_size: f32) -> u32 {
        if self.has_opsz {
            font_size.to_bits()
        } else {
            0
        }
    }

    /// How many times face data had to be opened so far (cache misses).
    pub fn face_opens(&self) -> usize {
        self.face_opens.load(Ordering::Relaxed)
    }

    /// Width, in px, of `text` drawn on a single line at `font_size`.
    ///
    /// Sum of horizontal advances plus pair kerning. A character the measuring face has no glyph
    /// for counts as **one em** — usvg will re-shape such a run with a fallback face, and for the
    /// CJK ranges that dominate that case the fallback's advance is exactly 1em (verified against
    /// usvg in this module's tests).
    ///
    /// Known approximation: on a variable face the advances follow the `opsz` axis (as usvg does)
    /// but the kern values are read from the default instance, so a face that also varies its
    /// kerning is off by a fraction of a pixel per pair (≤1.6px on a 12-character label, measured on
    /// macOS's system face).
    ///
    /// `text` is expected to be a single line: callers split on `<br>` and `\n` first. A literal
    /// newline here is measured as whatever the face does with it, which is not what an SVG renderer
    /// would do with it.
    pub fn measure(&self, text: &str, font_size: f32) -> f32 {
        self.measure_impl(text, font_size, true)
    }

    /// `measure` with kerning switchable — the harness reports both, so what kerning contributes is
    /// visible rather than assumed.
    pub fn measure_kerning(&self, text: &str, font_size: f32, kerning: bool) -> f32 {
        self.measure_impl(text, font_size, kerning)
    }

    fn measure_impl(&self, text: &str, font_size: f32, kerning: bool) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let chars: Vec<char> = text.chars().collect();
        let key = self.size_key(font_size);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        self.fill(&mut cache, &chars, font_size, key);

        let mut units = 0.0f32;
        let mut prev: Option<u32> = None;
        for &c in &chars {
            let Some(entry) = cache.chars.get(&(c, key)).copied() else {
                // Only reachable if the face became unreadable between resolve and now.
                units += self.upem;
                prev = None;
                continue;
            };
            match entry.gid {
                Some(gid) => {
                    if kerning {
                        if let Some(p) = prev {
                            units += cache.kern.get(&(p, gid)).copied().unwrap_or(0) as f32;
                        }
                    }
                    units += entry.units;
                    prev = Some(gid);
                }
                None => {
                    // Substituted from another face; no kern pair spans the seam.
                    units += entry.units;
                    prev = None;
                }
            }
        }
        units * (font_size / self.upem)
    }

    /// Populate the per-character and per-pair caches for `chars`, opening the face at most once.
    fn fill(&self, cache: &mut Cache, chars: &[char], font_size: f32, key: u32) {
        let need_chars = chars.iter().any(|c| !cache.chars.contains_key(&(*c, key)));
        let need_pairs = need_chars || {
            let mut miss = false;
            let mut prev: Option<u32> = None;
            for c in chars {
                let gid = cache.chars.get(&(*c, key)).and_then(|e| e.gid);
                if let (Some(p), Some(g)) = (prev, gid) {
                    miss |= !cache.kern.contains_key(&(p, g));
                }
                prev = gid;
            }
            miss
        };
        if !need_chars && !need_pairs {
            return;
        }
        let upem = self.upem;
        self.face_opens.fetch_add(1, Ordering::Relaxed);
        let _ = self.db.with_face_data(self.face, |data, index| {
            let Ok(font) = FontRef::from_index(data, index) else {
                return;
            };
            let charmap = font.charmap();
            // Match usvg's automatic optical sizing: when the face has an `opsz` axis and nothing
            // set it explicitly, it is driven from `font-size` (CSS `font-optical-sizing: auto`).
            // Skipping this measured 12% narrow on macOS's variable system face.
            let location: Location = if self.has_opsz {
                font.axes().location([(Tag::new(b"opsz"), font_size)])
            } else {
                font.axes().location::<[(Tag, f32); 0]>([])
            };
            let metrics = font.glyph_metrics(Size::unscaled(), &location);
            for &c in chars {
                if cache.chars.contains_key(&(c, key)) {
                    continue;
                }
                let entry = match charmap.map(c) {
                    Some(gid) if gid.to_u32() != 0 => CharAdvance {
                        gid: Some(gid.to_u32()),
                        units: metrics.advance_width(gid).unwrap_or(0.0),
                    },
                    // No glyph here: usvg re-shapes with a fallback face. One em is the
                    // approximation (see `measure`).
                    _ => CharAdvance {
                        gid: None,
                        units: upem,
                    },
                };
                cache.chars.insert((c, key), entry);
            }
            let legacy = matches!(self.kern_source, KernSource::LegacyTable)
                .then(|| font.kern().ok())
                .flatten();
            let gpos = match &self.kern_source {
                KernSource::Gpos(idx) => font.gpos().ok().map(|g| (g, idx.as_slice())),
                _ => None,
            };
            let mut prev: Option<u32> = None;
            for c in chars {
                let gid = cache.chars.get(&(*c, key)).and_then(|e| e.gid);
                if let (Some(p), Some(g)) = (prev, gid) {
                    cache.kern.entry((p, g)).or_insert_with(|| {
                        if let Some((gpos, idx)) = &gpos {
                            gpos_kern_pair(gpos, idx, p, g)
                        } else {
                            legacy.as_ref().map_or(0, |k| kern_pair(k, p, g))
                        }
                    });
                }
                prev = gid;
            }
        });
    }
}

/// Legacy `kern` adjustment for a glyph pair, in font units.
///
/// Only horizontal, non-cross-stream subtables are consulted; format 1 is an AAT state machine that
/// needs a shaper to run, so it is skipped rather than guessed at.
fn kern_pair(kern: &Kern, left: u32, right: u32) -> i32 {
    let (l, r) = (GlyphId::new(left), GlyphId::new(right));
    for sub in kern.subtables() {
        let Ok(sub) = sub else { continue };
        if !sub.is_horizontal() || sub.is_cross_stream() {
            continue;
        }
        let Ok(kind) = sub.kind() else { continue };
        let v = match kind {
            SubtableKind::Format0(t) => t.kerning(l, r),
            SubtableKind::Format2(t) => t.kerning(l, r),
            SubtableKind::Format3(t) => t.kerning(l, r),
            SubtableKind::Format1(_) => None,
        };
        if let Some(v) = v {
            return v;
        }
    }
    0
}

/// Which kerning source harfrust would use for this face, mirroring its decision in
/// `hb/ot_shape.rs`: GPOS wins whenever the face has a GPOS `kern` feature, and only otherwise does
/// the legacy `kern` table apply.
fn kern_source(font: &FontRef) -> KernSource {
    let gpos_kern = gpos_kern_lookups(font);
    // harfrust's precedence (`hb/ot_shape.rs`): AAT `kerx` wins unless the face has GSUB **and**
    // GPOS **and** a GPOS `kern` feature, in which case GPOS positions the run. Only when no `kerx`
    // is present does the legacy `kern` table get a turn.
    let has_kerx = font.kerx().is_ok();
    let full_ot = font.gsub().is_ok() && font.gpos().is_ok() && gpos_kern.is_some();
    if has_kerx && !full_ot {
        return KernSource::None;
    }
    if let Some(lookups) = gpos_kern {
        return KernSource::Gpos(lookups);
    }
    let has_legacy = font
        .kern()
        .ok()
        .is_some_and(|k| k.subtables().flatten().any(|s| s.is_horizontal()));
    if has_legacy {
        KernSource::LegacyTable
    } else {
        KernSource::None
    }
}

/// Lookup indices of the GPOS `kern` feature **as reachable from the Latin script**, or `None` when
/// there is none.
///
/// The script matters. harfrust builds its feature map for the script it chose for the run and asks
/// whether `kern` is in *that* map; a face can carry a `kern` feature that only some other script's
/// LangSys references, and for Latin text harfrust then falls back to the legacy `kern` table.
/// Accepting any `kern` feature in the table was measured to disagree with resvg by 6px on a real
/// face (`Tamil MN`). mermaid labels are shaped as Latin/common, so `latn` and
/// the default script are the ones to look in.
fn gpos_kern_lookups(font: &FontRef) -> Option<Vec<u16>> {
    let gpos = font.gpos().ok()?;
    let features = gpos.feature_list().ok()?;
    let scripts = gpos.script_list().ok()?;
    // harfbuzz picks one script for the run and uses only that script's features. For Latin text it
    // is `latn` when the face has it, and only otherwise the default script. Taking the union of the
    // two applies kerning resvg does not (measured 6px on `Tamil MN`, whose `kern` hangs off DFLT
    // while `latn` is present without it).
    let records = scripts.script_records();
    let chosen = records
        .iter()
        .find(|r| r.script_tag() == Tag::new(b"latn"))
        .or_else(|| records.iter().find(|r| r.script_tag() == Tag::new(b"DFLT")))?;
    let script = chosen.script(scripts.offset_data()).ok()?;
    // The default LangSys is what an unspecified language resolves to, which is how usvg shapes
    // (it never sets a language).
    let visible: Vec<u16> = match script.default_lang_sys() {
        Some(Ok(lang)) => lang.feature_indices().iter().map(|i| i.get()).collect(),
        _ => Vec::new(),
    };
    if visible.is_empty() {
        return None;
    }
    let feature_records = features.feature_records();
    let mut lookups: Vec<u16> = Vec::new();
    for idx in visible {
        let Some(rec) = feature_records.get(idx as usize) else {
            continue;
        };
        if rec.feature_tag() != Tag::new(b"kern") {
            continue;
        }
        if let Ok(feature) = rec.feature(features.offset_data()) {
            lookups.extend(feature.lookup_list_indices().iter().map(|i| i.get()));
        }
    }
    if lookups.is_empty() {
        return None;
    }
    lookups.sort_unstable();
    lookups.dedup();
    Some(lookups)
}

/// Total GPOS `kern` adjustment for a glyph pair, in font units.
///
/// Only pair positioning (lookup type 2, plus type 9 extensions wrapping it) is evaluated —
/// contextual kerning is rare in text faces and would need a shaper. Within one lookup the first
/// matching subtable wins, as the OpenType spec requires; adjustments from different lookups of the
/// feature accumulate.
fn gpos_kern_pair(gpos: &Gpos, lookup_indices: &[u16], left: u32, right: u32) -> i32 {
    let Ok(list) = gpos.lookup_list() else {
        return 0;
    };
    let lookups = list.lookups();
    let (l, r) = (GlyphId::new(left), GlyphId::new(right));
    let mut total = 0;
    for &idx in lookup_indices {
        let Ok(lookup) = lookups.get(idx as usize) else {
            continue;
        };
        let subtables: Vec<PairPos> = match &lookup {
            PositionLookup::Pair(inner) => inner.subtables().iter().flatten().collect(),
            PositionLookup::Extension(inner) => inner
                .subtables()
                .iter()
                .flatten()
                .filter_map(|ext| match ext {
                    ExtensionSubtable::Pair(p) => p.extension().ok(),
                    _ => None,
                })
                .collect(),
            _ => continue,
        };
        for sub in &subtables {
            if let Some(v) = pair_pos_advance(sub, l, r) {
                total += v;
                break;
            }
        }
    }
    total
}

/// Advance adjustment a single `PairPos` subtable applies to `left`/`right`, or `None` when the
/// subtable does not cover the pair.
fn pair_pos_advance(pair: &PairPos, left: GlyphId, right: GlyphId) -> Option<i32> {
    let x_adv = |v: &skrifa::raw::tables::gpos::ValueRecord| {
        v.x_advance.map(|a| a.get() as i32).unwrap_or(0)
    };
    match pair {
        PairPos::Format1(t) => {
            let index = t.coverage().ok()?.get(left)? as usize;
            let set = t.pair_sets().get(index).ok()?;
            let right16: GlyphId16 = right.try_into().ok()?;
            for rec in set.pair_value_records().iter() {
                let Ok(rec) = rec else { break };
                match rec.second_glyph().cmp(&right16) {
                    std::cmp::Ordering::Less => continue,
                    // Records are ordered by second glyph, so once we are past it there is no match.
                    std::cmp::Ordering::Greater => break,
                    std::cmp::Ordering::Equal => {
                        return Some(x_adv(&rec.value_record1) + x_adv(&rec.value_record2))
                    }
                }
            }
            None
        }
        PairPos::Format2(t) => {
            // Format 2 is class-based, but the first glyph must still be covered.
            t.coverage().ok()?.get(left)?;
            let c1 = t.class_def1().ok()?.get(left) as usize;
            let c2 = t.class_def2().ok()?.get(right) as usize;
            let rec1 = t.class1_records().get(c1).ok()?;
            let rec2 = rec1.class2_records().get(c2).ok()?;
            Some(x_adv(&rec2.value_record1) + x_adv(&rec2.value_record2))
        }
    }
}

/// Process-wide metrics against konoma's shared system font DB (the same `Arc` resvg draws with).
///
/// Resolution is attempted once. An `Err` is sticky for the process, which is what we want: the font
/// database is loaded once too, so the answer cannot change under us.
pub fn shared() -> Result<&'static TextMetrics, FontUnavailable> {
    static M: OnceLock<Result<TextMetrics, FontUnavailable>> = OnceLock::new();
    M.get_or_init(|| TextMetrics::resolve(crate::preview::svg::shared_fontdb()))
        .as_ref()
        .map_err(|e| *e)
}

/// Whether a diagram may be rendered as an image at all.
///
/// **The gate.** `false` means usvg would draw boxes with no text in them, so the caller must not
/// build an SVG and must fall back to the text diagram instead (PRD design principle #3).
pub fn fonts_available() -> bool {
    shared().is_ok()
}

/// The text usvg will really shape, for a run of characters written into a `<text>` element.
///
/// **This is the shared measuring path's one piece of policy, and it exists because measuring the
/// author's bytes measures something that never gets drawn.** usvg folds whitespace *before* it
/// shapes (`usvg/src/parser/svgtree/text.rs`, `trim_text` plus `trim_text_nodes`), so
/// `"loop  Every minute"` is drawn exactly as wide as `"loop Every minute"` — while a renderer
/// that measured the two spaces sizes a box that is one space too wide, and every coordinate
/// downstream of that box moves with it. Stage 4 hit this for real; `docs/STATUS.md` recorded it
/// as a hole in *every* diagram kind's measurement, which is why the fix is here rather than in
/// the one language that noticed.
///
/// usvg's rule, transcribed rather than recalled:
///
/// 1. `\r`, `\n` and `\t` each become a space (note: a *space*, not nothing — usvg deviates from
///    SVG 1.1's "remove all newline characters" here, and konoma follows usvg, which is what
///    actually draws);
/// 2. runs of spaces collapse to one (this is `xml:space="default"`, which is what
///    [`super::render::svg`] emits — it writes no `xml:space` attribute at all);
/// 3. one leading and one trailing space are dropped, which after step 2 is a plain trim.
///
/// **Only U+0020 collapses.** No-break space (U+00A0) and ideographic space (U+3000) are ordinary
/// glyphs to a shaper, so `str::split_whitespace` and `str::trim` would both be wrong here: they
/// would fold characters resvg is going to draw.
pub fn collapse_spaces(text: &str) -> std::borrow::Cow<'_, str> {
    let needs = |t: &str| {
        let b = t.as_bytes();
        b.first() == Some(&b' ')
            || b.last() == Some(&b' ')
            || t.contains("  ")
            || t.contains(['\r', '\n', '\t'])
    };
    if !needs(text) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        let c = match c {
            '\r' | '\n' | '\t' => ' ',
            _ => c,
        };
        if c == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        out.push(c);
    }
    // Steps 3: after collapsing there is at most one space at each end.
    if out.starts_with(' ') {
        out.remove(0);
    }
    if out.ends_with(' ') {
        out.pop();
    }
    std::borrow::Cow::Owned(out)
}

/// Width in px of `text` at `font_size`, measured against the shared font DB.
///
/// [`collapse_spaces`] is applied first, so this answers "how wide will resvg draw this?" rather
/// than "how wide are these bytes?" — see that function for why the two differ.
///
/// Call only when [`fonts_available`] is true. With no font resolvable this returns a crude
/// em-based estimate rather than zero, purely so that a caller that skipped the gate produces a
/// misshapen diagram instead of a degenerate one; it is not a supported mode.
pub fn measure(text: &str, font_size: f32) -> f32 {
    let text = collapse_spaces(text);
    match shared() {
        Ok(m) => m.measure(&text, font_size),
        Err(_) => estimate_without_font(&text, font_size),
    }
}

/// Same as [`measure`] but says so when there is no font, for callers that want to branch.
pub fn try_measure(text: &str, font_size: f32) -> Result<f32, FontUnavailable> {
    let text = collapse_spaces(text);
    shared().map(|m| m.measure(&text, font_size))
}

/// Last-resort width estimate with no font at all: full-width characters count as one em, the rest
/// as half an em. Only reachable when the gate was skipped.
fn estimate_without_font(text: &str, font_size: f32) -> f32 {
    use unicode_width::UnicodeWidthChar;
    text.chars()
        .map(|c| {
            if c.width().unwrap_or(0) >= 2 {
                1.0
            } else {
                0.5
            }
        })
        .sum::<f32>()
        * font_size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::svg::shared_fontdb;
    use resvg::usvg;
    use std::sync::Mutex as StdMutex;

    /// The measurement corpus. Each entry is (name, text) and every category the mermaid renderer
    /// will actually meet is represented: CJK, emoji, western text with kern pairs, thin glyphs,
    /// runs of spaces, a long line, a mixed script line, the empty string and a single character.
    const CORPUS: &[(&str, &str)] = &[
        ("empty", ""),
        ("single-char", "A"),
        ("single-cjk", "処"),
        ("cjk", "処理を実行する"),
        ("cjk-punct", "開始、確認。「完了」"),
        ("emoji", "🚀"),
        ("emoji-in-text", "Deploy 🚀 done"),
        ("kern-avatar", "AVATAR To Wa"),
        ("kern-wavy", "Yo, Wavy!"),
        ("kern-quotes", "AWAY LTV P,"),
        ("thin-illicit", "illicit"),
        ("thin-iiiii", "iiiii"),
        ("thin-lll", "lllllllllllllllllll"),
        ("spaces-inner", "a  b   c    d"),
        ("spaces-around", "  spaced  out  "),
        (
            "long",
            "The quick brown fox jumps over the lazy dog, then does it again",
        ),
        ("mixed", "混在 mixed ラベル 123"),
        (
            "mixed-long",
            "konoma のプレビュー preview を全画面 fullscreen で描画する",
        ),
        ("digits", "0123456789"),
        ("punct", "(){}[]<>|/\\-_=+*&^%$#@!?.,;:"),
        ("upper", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ("lower", "abcdefghijklmnopqrstuvwxyz"),
    ];

    /// What usvg actually did with one string.
    struct Probe {
        /// `Text::bounding_box().width()` — usvg builds this from the very advances it used to place
        /// the glyphs (`convert_span` unions per-cluster `advance`-wide rects), so it is resvg's own
        /// answer to "how wide is this text", not an ink bbox.
        width: f32,
        /// Face usvg picked for `font-family="sans-serif"`.
        primary: Option<fontdb::ID>,
        /// Faces usvg had to fall back to because the primary had no glyph.
        fallbacks: Vec<fontdb::ID>,
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// Render `text` through usvg with konoma's shared font DB and report what it measured and which
    /// faces it used. `None` when usvg produced no text node at all (empty string, or no fonts).
    fn probe(text: &str, font_size: f32) -> Option<Probe> {
        probe_with_family(FONT_FAMILY, text, font_size)
    }

    /// `probe` **without** `xml:space="preserve"` — the whitespace mode konoma's own documents
    /// are in, since [`super::render::svg`] writes no `xml:space` attribute at all.
    ///
    /// [`probe`] preserves on purpose: it isolates *shaping* from whitespace policy, which is what
    /// makes it a clean test of the advance-summing primitive. This one is the counterpart that
    /// asks the production question, and the two together are why the hole in
    /// `docs/STATUS.md` could exist — the instrument's reference document differed from the
    /// emitted one in exactly the attribute that governs folding.
    fn probe_default(text: &str) -> Option<Probe> {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="6000" height="400"><text x="0" y="200" font-family="{FONT_FAMILY}" font-size="{FONT_SIZE}">{}</text></svg>"#,
            xml_escape(text)
        );
        let opt = usvg::Options {
            fontdb: shared_fontdb(),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(svg.as_bytes(), &opt).ok()?;
        Some(Probe {
            width: find_text_width(tree.root())?,
            primary: None,
            fallbacks: Vec::new(),
        })
    }

    /// `probe` against an arbitrary `font-family` string (used by the cross-font test).
    fn probe_with_family(family: &str, text: &str, font_size: f32) -> Option<Probe> {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="6000" height="400"><text x="0" y="200" font-family="{}" font-size="{font_size}" xml:space="preserve">{}</text></svg>"#,
            xml_escape(family),
            xml_escape(text)
        );
        let primary: Arc<StdMutex<Option<fontdb::ID>>> = Arc::new(StdMutex::new(None));
        let fallbacks: Arc<StdMutex<Vec<fontdb::ID>>> = Arc::new(StdMutex::new(Vec::new()));
        let (p2, f2) = (primary.clone(), fallbacks.clone());
        let default_select = usvg::FontResolver::default_font_selector();
        let default_fallback = usvg::FontResolver::default_fallback_selector();
        let opt = usvg::Options {
            fontdb: shared_fontdb(),
            font_resolver: usvg::FontResolver {
                select_font: Box::new(move |font, db| {
                    let id = default_select(font, db);
                    if let Some(id) = id {
                        *p2.lock().unwrap() = Some(id);
                    }
                    id
                }),
                select_fallback: Box::new(move |c, exclude, db| {
                    let id = default_fallback(c, exclude, db);
                    if let Some(id) = id {
                        f2.lock().unwrap().push(id);
                    }
                    id
                }),
            },
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(svg.as_bytes(), &opt).ok()?;
        let width = find_text_width(tree.root())?;
        let primary = *primary.lock().unwrap();
        let fallbacks = fallbacks.lock().unwrap().clone();
        Some(Probe {
            width,
            primary,
            fallbacks,
        })
    }

    fn find_text_width(group: &usvg::Group) -> Option<f32> {
        for node in group.children() {
            match node {
                usvg::Node::Text(t) => return Some(t.bounding_box().width()),
                usvg::Node::Group(g) => {
                    if let Some(w) = find_text_width(g) {
                        return Some(w);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn family_name(id: fontdb::ID) -> String {
        shared_fontdb()
            .face(id)
            .map(|f| f.families[0].0.clone())
            .unwrap_or_default()
    }

    fn metrics() -> Option<&'static TextMetrics> {
        match shared() {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!(
                    "SKIP: この環境には計測に使えるフォントが無い ({e})。フォント依存のテストを飛ばす。"
                );
                None
            }
        }
    }

    /// Agreement tolerance for a string the measuring face covers completely.
    ///
    /// For those, konoma and usvg are summing the *same* advances out of the *same* face, so the
    /// only legitimate difference is float accumulation order: konoma sums in font units and scales
    /// once, usvg scales each glyph and sums the results. At 14px / 2048upem that is on the order of
    /// 1e-5 px even for a line of 60+ characters (measured: every strict-tier case in `CORPUS` comes
    /// out **bit-identical**, delta 0.000000). 1e-3 px leaves three orders of magnitude of headroom
    /// for a face with a different upem while staying ~1000x below the ~1px scale at which a
    /// diagram's layout could visibly move. Anything this instrument exists to catch is far larger:
    /// dropping kerning is 8% on `AVATAR`, measuring against a different fontdb was 160% on `i`/`l`,
    /// counting CJK as zero is 100%.
    const STRICT_TOLERANCE_PX: f32 = 1e-3;

    /// Agreement tolerance for a string that made usvg fall back to another face.
    ///
    /// Here konoma is deliberately approximating: a character the measuring face has no glyph for is
    /// counted as one em, while usvg re-shapes the run with whatever face the font database happens
    /// to offer. That face is platform-dependent (macOS picks `Arial Unicode MS`, a Linux box will
    /// pick something else), so exactness cannot be required without pinning a font. One em is exact
    /// for CJK ideographs and emoji in every mainstream fallback face, which is why on this machine
    /// these cases also land on 0.000000; the 2% band is for the case where a fallback face's *Latin*
    /// advances differ from the primary's, which is the one way this approximation can drift. It is
    /// still four times tighter than the smallest structural bug the instrument guards against.
    const FALLBACK_TOLERANCE_RATIO: f32 = 0.02;
    const FALLBACK_TOLERANCE_FLOOR_PX: f32 = 0.5;

    /// **The main instrument of stage 0.** konoma's own measurement against resvg's real shaper.
    ///
    /// Nothing here compares konoma to konoma: the reference side builds a `<text>` SVG, hands it to
    /// usvg with the same shared font DB the preview draws with, and reads
    /// `Text::bounding_box().width()` — a number usvg derives from the advances harfrust produced
    /// while positioning the glyphs. If `measure` is ever quietly replaced by whatever this test
    /// compares against, the test does not become vacuous, it becomes impossible: the reference is
    /// resvg, not us.
    #[test]
    fn measurement_matches_resvg() {
        let Some(m) = metrics() else { return };
        let mut strict = 0usize;
        let mut fallback = 0usize;
        let mut worst = (0.0f32, "");
        for (name, text) in CORPUS {
            let mine = m.measure(text, FONT_SIZE);
            let Some(p) = probe(text, FONT_SIZE) else {
                assert!(
                    text.is_empty(),
                    "{name}: usvg がテキストノードを作らなかったのに空文字列でない"
                );
                assert_eq!(mine, 0.0, "{name}: 空文字列の幅は 0");
                continue;
            };
            assert_eq!(
                p.primary,
                Some(m.face_id()),
                "{name}: usvg が font-family=\"{FONT_FAMILY}\" を解決した face と計測 face が違う"
            );
            let delta = (mine - p.width).abs();
            let tol = if p.fallbacks.is_empty() {
                strict += 1;
                STRICT_TOLERANCE_PX
            } else {
                fallback += 1;
                (p.width * FALLBACK_TOLERANCE_RATIO).max(FALLBACK_TOLERANCE_FLOOR_PX)
            };
            if delta > worst.0 {
                worst = (delta, name);
            }
            assert!(
                delta <= tol,
                "{name}: konoma={mine:.4} resvg={:.4} delta={delta:.4} > tol={tol:.4} (fallback={})",
                p.width,
                p.fallbacks.len()
            );
        }
        // A tier can only prove something if cases actually land in it. If the corpus ever drifts so
        // that everything is measured under the loose fallback tolerance, this test has stopped
        // checking the exact path and should fail loudly rather than stay green.
        assert!(
            strict >= 10,
            "厳密判定に載ったケースが {strict} 件しかない = コーパスが痩せている"
        );
        assert!(
            fallback >= 3,
            "フォールバック経路のケースが {fallback} 件しかない = CJK/絵文字が抜けている"
        );
        eprintln!(
            "strict={strict} fallback={fallback} worst={:.6}px ({})",
            worst.0, worst.1
        );
    }

    /// **[`collapse_spaces`] is the shared measuring path's whitespace policy, and it is checked
    /// against usvg rather than against itself.**
    ///
    /// The reference here is deliberately *not* another copy of konoma's rule: each case is put
    /// through usvg twice, once as konoma emits it (no `xml:space`, so usvg folds) and once as the
    /// already-folded string, and the two must come out the same width. If konoma folded more or
    /// less than usvg does, the two widths would part company — so this cannot degenerate into
    /// "konoma agrees with konoma".
    ///
    /// The negative half matters as much: no-break space and ideographic space are ordinary glyphs
    /// to a shaper, so a rule written with `str::trim` / `split_whitespace` would fold characters
    /// resvg is going to draw. Those cases are here to fail such a rule.
    #[test]
    fn collapsing_matches_what_usvg_folds_before_shaping() {
        let Some(_) = metrics() else { return };
        // (name, as written, what usvg will shape)
        let cases: &[(&str, &str, &str)] = &[
            ("plain", "one two", "one two"),
            ("double", "loop  Every minute", "loop Every minute"),
            ("many", "a  b   c    d", "a b c d"),
            ("edges", "  spaced  out  ", "spaced out"),
            ("tab", "a\tb", "a b"),
            ("tab-run", "a\t\tb", "a b"),
            ("cr", "a\rb", "a b"),
            ("only-spaces", "   ", ""),
            ("empty", "", ""),
            // U+00A0 and U+3000 are glyphs, not whitespace, as far as a shaper is concerned.
            ("nbsp", "a\u{a0}\u{a0}b", "a\u{a0}\u{a0}b"),
            (
                "ideographic",
                "全\u{3000}\u{3000}角",
                "全\u{3000}\u{3000}角",
            ),
            ("nbsp-edge", "\u{a0}x\u{a0}", "\u{a0}x\u{a0}"),
        ];
        for (name, written, folded) in cases {
            assert_eq!(
                collapse_spaces(written).as_ref(),
                *folded,
                "{name}: collapse_spaces disagrees with the rule usvg implements"
            );
            // Idempotent, or a label measured twice would shrink.
            assert_eq!(
                collapse_spaces(folded).as_ref(),
                *folded,
                "{name}: not idempotent"
            );

            // The external check: what usvg draws from the two spellings is the same width.
            let (Some(raw), Some(pre)) = (probe_default(written), probe_default(folded)) else {
                assert!(
                    folded.is_empty(),
                    "{name}: usvg drew nothing for a non-empty string"
                );
                continue;
            };
            assert!(
                (raw.width - pre.width).abs() <= STRICT_TOLERANCE_PX,
                "{name}: usvg drew {written:?} at {} and {folded:?} at {} — the fold is not what \
                 usvg does",
                raw.width,
                pre.width
            );
            // …and konoma's measurement of the *written* form is that width.
            assert!(
                (measure(written, FONT_SIZE) - raw.width).abs() <= STRICT_TOLERANCE_PX,
                "{name}: konoma measured {} where resvg drew {}",
                measure(written, FONT_SIZE),
                raw.width
            );
        }
    }

    /// The corpus is part of the instrument, so guard it the way the instrument is guarded: every
    /// category the design doc names must actually be present, and be present *as that category*.
    #[test]
    fn corpus_covers_every_required_category() {
        let Some(m) = metrics() else { return };
        let has = |pred: fn(&str) -> bool| CORPUS.iter().any(|(_, t)| pred(t));
        assert!(
            has(|t| t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))),
            "CJK 漢字のケースが無い"
        );
        assert!(
            has(|t| t.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c))),
            "かな のケースが無い"
        );
        assert!(has(|t| t.contains('\u{1f680}')), "絵文字のケースが無い");
        assert!(has(|t| t.contains("AVATAR")), "カーニング対象の欧文が無い");
        assert!(has(|t| t.contains("illicit")), "細いグリフのケースが無い");
        assert!(has(|t| t.contains("  ")), "連続空白のケースが無い");
        assert!(has(|t| t.chars().count() > 50), "長文のケースが無い");
        assert!(has(str::is_empty), "空文字列のケースが無い");
        assert!(has(|t| t.chars().count() == 1), "1 文字のケースが無い");
        assert!(
            has(|t| t.contains("mixed") && t.chars().any(|c| c as u32 > 0x2000)),
            "欧文と CJK の混在ケースが無い"
        );
        // And the mixed case must really be mixed as far as the font is concerned: part covered by
        // the measuring face, part not.
        let mixed = CORPUS.iter().find(|(n, _)| *n == "mixed").unwrap().1;
        let p = probe(mixed, FONT_SIZE).expect("mixed は描かれる");
        assert!(
            !p.fallbacks.is_empty(),
            "mixed が計測フォントだけで賄えている = 混在ケースになっていない"
        );
        assert!(m.measure(mixed, FONT_SIZE) > 0.0);
    }

    /// Kerning is not decoration: dropping it is an 8%-scale error on real labels. Pin both that the
    /// corpus still contains a pair-heavy string and that applying `kern` is what makes konoma agree
    /// with resvg — the un-kerned number must be *wrong*.
    #[test]
    fn kerning_is_applied_and_is_what_matches_resvg() {
        let Some(m) = metrics() else { return };
        if !m.kerns() {
            eprintln!("SKIP: 計測フォントがカーニングを持たない環境なのでこの検査は成立しない");
            return;
        }
        let text = "AVATAR To Wa";
        let with = m.measure(text, FONT_SIZE);
        let without = m.measure_kerning(text, FONT_SIZE, false);
        let actual = probe(text, FONT_SIZE).expect("描かれる").width;
        assert!(
            (without - with) > 1.0,
            "kern が効いていない (with={with:.3} without={without:.3})"
        );
        assert!(
            (with - actual).abs() <= STRICT_TOLERANCE_PX,
            "kern あり = resvg"
        );
        assert!(
            (without - actual).abs() > 1.0,
            "kern 無しでも resvg と一致してしまう = この文字列にはカーニング対象が無い"
        );
    }

    /// The one-em rule for characters the measuring face has no glyph for, stated as an equality so
    /// a change to it cannot hide inside a tolerance.
    #[test]
    fn characters_without_a_glyph_count_as_one_em() {
        let Some(m) = metrics() else { return };
        let cjk = "処理を実行する";
        assert_eq!(
            m.measure(cjk, FONT_SIZE),
            cjk.chars().count() as f32 * FONT_SIZE,
            "グリフの無い CJK は 1 文字 = 1em"
        );
        // ... and that rule is not a private convention: resvg agrees.
        let actual = probe(cjk, FONT_SIZE).expect("描かれる").width;
        assert!((m.measure(cjk, FONT_SIZE) - actual).abs() <= STRICT_TOLERANCE_PX);
    }

    #[test]
    fn empty_text_measures_zero_and_scaling_is_linear() {
        let Some(m) = metrics() else { return };
        assert_eq!(m.measure("", FONT_SIZE), 0.0);
        let t = "Node label";
        let a = m.measure(t, FONT_SIZE);
        let b = m.measure(t, FONT_SIZE * 2.0);
        assert!((b - a * 2.0).abs() < 1e-3, "font_size に比例する");
    }

    /// Requirement from the design doc §4-3: the font used for measuring and the `font-family`
    /// written into the SVG must land on the same face. That is the exact failure of 2026-07-17 —
    /// a box measured with one font and drawn with another.
    #[test]
    fn svg_font_family_resolves_to_the_measuring_face() {
        let Some(m) = metrics() else { return };
        for text in ["Label", "処理", "AVATAR"] {
            let p = probe(text, FONT_SIZE).expect("描かれる");
            assert_eq!(
                p.primary,
                Some(m.face_id()),
                "font-family=\"{FONT_FAMILY}\" の解決先が計測 face と一致しない"
            );
        }
        // The attribute really is only the one generic usvg understands — no named families that
        // would silently miss and drop through to Serif.
        assert_eq!(FONT_FAMILY, "sans-serif");
        assert!(!FONT_FAMILY.contains(','), "フォントスタックを並べない");
    }

    /// Metrics are theme data's business only in the sense that they must *not* be: a theme may
    /// change colors, never the measuring font or size (regression 2026-07-17, detour-ring bug).
    #[test]
    fn layout_font_is_pinned_and_theme_independent() {
        assert_eq!(FONT_SIZE, 14.0);
        assert_eq!(FONT_FAMILY, "sans-serif");
    }

    /// The gate. An empty font database must be reported, not papered over.
    #[test]
    fn missing_fonts_are_detected_rather_than_guessed() {
        let empty = Arc::new(fontdb::Database::new());
        assert_eq!(
            TextMetrics::resolve(empty).err(),
            Some(FontUnavailable::NoMatch),
            "フォントが 1 つも無ければ解決は Err"
        );
    }

    /// Why the gate has to exist at all: with no fonts, usvg does not fail — it drops the glyphs and
    /// the `<text>` is simply not in the tree. A diagram built anyway would be empty boxes with no
    /// error anywhere, which is exactly what design principle #3 forbids.
    #[test]
    fn usvg_silently_drops_text_when_no_font_resolves() {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50"><text x="0" y="30" font-family="{FONT_FAMILY}" font-size="14">Label</text></svg>"#
        );
        let opt = usvg::Options {
            fontdb: Arc::new(fontdb::Database::new()),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_data(svg.as_bytes(), &opt).expect("SVG 自体は妥当");
        assert!(
            find_text_width(tree.root()).is_none(),
            "フォント不在でも usvg はエラーにならず、テキストが黙って消える"
        );
    }

    /// konoma guards hot paths structurally rather than with a stopwatch (`speed_tests.rs`). The
    /// property that makes `measure` cheap is that face data is opened only on a cache miss.
    #[test]
    fn face_data_is_opened_only_on_a_cache_miss() {
        if metrics().is_none() {
            return;
        }
        // A private instance: `shared()` is process-wide and other tests warm it in parallel, so its
        // counter says nothing about this test's calls.
        let m = TextMetrics::resolve(shared_fontdb()).expect("解決できる");
        assert_eq!(m.face_opens(), 0, "解決だけでは計測用に開かない");
        let (a, b) = ("Cache warm 処理", "warm Cache");
        let _ = m.measure(a, FONT_SIZE);
        let _ = m.measure(b, FONT_SIZE);
        let warm = m.face_opens();
        assert!(warm <= 2, "初回の取りこぼしが多すぎる: {warm}");
        for _ in 0..200 {
            let _ = m.measure(a, FONT_SIZE);
            let _ = m.measure(b, FONT_SIZE * 2.0);
        }
        assert_eq!(
            m.face_opens(),
            warm,
            "既知の文字だけの計測でフォントを開き直している = キャッシュが効いていない"
        );
    }

    /// With no font at all, the last-resort estimate must still be positive and monotonic — a caller
    /// that skipped the gate gets a misshapen diagram, never a degenerate zero-sized one.
    #[test]
    fn estimate_without_font_is_positive_and_width_aware() {
        assert_eq!(estimate_without_font("", 14.0), 0.0);
        assert!(estimate_without_font("ab", 14.0) > 0.0);
        assert!(
            estimate_without_font("処", 14.0) > estimate_without_font("a", 14.0),
            "全角は半角より広い"
        );
    }

    /// The instrument, widened to the whole font book.
    ///
    /// `measurement_matches_resvg` proves agreement for the one face this machine calls
    /// `sans-serif`. That is the face konoma will use here — but a Linux box resolves `sans-serif`
    /// to something else entirely, and the ways measurement can drift from drawing (which table the
    /// kerning lives in, whether the face has an optical-size axis) are properties of the *face*,
    /// not of konoma. So bind measurement to every normal-weight family in the database in turn and
    /// check konoma still agrees with resvg for it.
    ///
    /// The bar is a ratio, not zero: a handful of faces kern through machinery only a full shaper
    /// can run (AAT `kerx` state machines, contextual GPOS lookups), and this is a report about
    /// *other people's fonts*, not about konoma's. Measured on macOS 2026-08: 12 of 701 cases
    /// disagree (1.7%), all on faces no system would offer as `sans-serif` — three Latin strings
    /// each on `Tamil MN`, `Euphemia UCAS`, and macOS's variable system face, the last within 1.6px.
    /// 5% leaves room for a differently stocked machine while still failing loudly if the
    /// kerning-source rule regresses: reading the legacy `kern` table for every face, the way a
    /// first cut naturally would, takes the rate to **21.3%**. Optical sizing is *not* guarded here
    /// (dropping it only reaches 2.0%, because two faces on this machine have the axis) — see
    /// `optical_sizing_follows_font_size` for that one.
    #[test]
    fn measurement_matches_resvg_across_the_whole_font_book() {
        if metrics().is_none() {
            return;
        }
        let db = shared_fontdb();
        let mut names: Vec<String> = db
            .faces()
            .filter(|f| {
                f.style == fontdb::Style::Normal
                    && f.weight == fontdb::Weight::NORMAL
                    && f.stretch == fontdb::Stretch::Normal
            })
            .map(|f| f.families[0].0.clone())
            .collect();
        names.sort();
        names.dedup();
        let samples = ["AVATAR To Wa", "illicit", "Preview label", "Yo, Wavy!"];
        let mut mismatches: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for name in &names {
            let fam = [fontdb::Family::Name(name)];
            let Ok(m) = TextMetrics::resolve_families(db.clone(), &fam) else {
                continue;
            };
            for t in samples {
                let Some(p) = probe_with_family(name, t, FONT_SIZE) else {
                    continue;
                };
                // usvg parses the font-family attribute itself, so a name taken straight out of the
                // database can still land on a different face (symbol fonts, names with digits).
                // Those say nothing about measurement.
                if p.primary != Some(m.face_id()) {
                    continue;
                }
                // Skip strings the face cannot draw itself: there konoma is deliberately
                // approximating another face's advances (the documented one-em rule).
                if m.has_missing_glyph(t) {
                    continue;
                }
                checked += 1;
                let mine = m.measure(t, FONT_SIZE);
                if (mine - p.width).abs() > STRICT_TOLERANCE_PX {
                    mismatches.push(format!(
                        "{name} / {t:?}: konoma={mine:.3} resvg={:.3} d={:.3} kern={:?}",
                        p.width,
                        mine - p.width,
                        m.kern_source()
                    ));
                }
            }
        }
        if checked < 40 {
            eprintln!("SKIP: 比較できたフォントが {checked} 件しかない環境なので比率を判定しない");
            return;
        }
        eprintln!(
            "cross-font: checked={checked} mismatches={} ({:.1}%)",
            mismatches.len(),
            mismatches.len() as f32 * 100.0 / checked as f32
        );
        assert!(
            mismatches.len() * 20 <= checked,
            "{}/{checked} のフォントで計測と描画がずれる (上限 5%):\n{}",
            mismatches.len(),
            mismatches
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// Optical sizing, guarded on its own.
    ///
    /// A variable face with an `opsz` axis draws at different widths depending on `font-size`,
    /// because usvg sets that axis from it (CSS `font-optical-sizing: auto`, which is usvg's
    /// default). Measuring at the axis default instead was 12% narrow on macOS's system face. The
    /// whole-font-book ratio test cannot catch this — too few faces have the axis to move a
    /// percentage — so pin it directly: applying optical sizing must bring konoma much closer to
    /// resvg than not applying it. Stated as a ratio rather than an absolute because the residual
    /// (GPOS kern values also vary along the axis, and konoma reads the default instance's) is a
    /// property of the face.
    #[test]
    fn optical_sizing_follows_font_size() {
        if metrics().is_none() {
            return;
        }
        let db = shared_fontdb();
        let mut names: Vec<String> = db
            .faces()
            .filter(|f| {
                f.style == fontdb::Style::Normal
                    && f.weight == fontdb::Weight::NORMAL
                    && f.stretch == fontdb::Stretch::Normal
            })
            .map(|f| f.families[0].0.clone())
            .collect();
        names.sort();
        names.dedup();
        let text = "Preview label";
        for name in &names {
            let fam = [fontdb::Family::Name(name)];
            let Ok(m) = TextMetrics::resolve_families(db.clone(), &fam) else {
                continue;
            };
            if !m.has_opsz || m.has_missing_glyph(text) {
                continue;
            }
            let Some(p) = probe_with_family(name, text, FONT_SIZE) else {
                continue;
            };
            if p.primary != Some(m.face_id()) || !p.fallbacks.is_empty() {
                continue;
            }
            let with = (m.measure(text, FONT_SIZE) - p.width).abs();
            let mut flat = TextMetrics::resolve_families(db.clone(), &fam).expect("解決できる");
            flat.has_opsz = false;
            let without = (flat.measure(text, FONT_SIZE) - p.width).abs();
            eprintln!("opsz guard on {name}: with={with:.3}px without={without:.3}px");
            assert!(
                with * 4.0 < without,
                "{name}: opsz を適用してもしなくても誤差が変わらない (with={with:.3} without={without:.3})"
            );
            return;
        }
        eprintln!("SKIP: opsz 軸を持つフォントがこの環境に無いので可変フォントの検査は飛ばす");
    }

    /// Prints the whole corpus with konoma's measurement next to resvg's. Not an assertion — it is
    /// how the threshold in `measurement_matches_resvg` was chosen, and how it gets re-checked.
    #[test]
    #[ignore = "reporting only: cargo test -- --ignored --nocapture measurement_report"]
    fn measurement_report() {
        let Some(m) = metrics() else { return };
        eprintln!(
            "face upem={} kern={} face={:?}",
            m.units_per_em(),
            match m.kern_source() {
                KernSource::Gpos(l) => format!("gpos({} lookups)", l.len()),
                KernSource::LegacyTable => "kern table".to_string(),
                KernSource::None => "none".to_string(),
            },
            m.face_id()
        );
        eprintln!(
            "{:<14} {:>9} {:>9} {:>9} {:>8} {:>7}  fallback",
            "case", "konoma", "resvg", "delta", "delta/ch", "nokern"
        );
        for (name, text) in CORPUS {
            let mine = m.measure(text, FONT_SIZE);
            let nokern = m.measure_kerning(text, FONT_SIZE, false);
            match probe(text, FONT_SIZE) {
                Some(p) => {
                    let n = text.chars().count().max(1) as f32;
                    eprintln!(
                        "{:<14} {:>9.3} {:>9.3} {:>9.6} {:>8.3} {:>7.3}  {}",
                        name,
                        mine,
                        p.width,
                        mine - p.width,
                        (mine - p.width) / n,
                        nokern - p.width,
                        if p.fallbacks.is_empty() {
                            format!("primary_same={}", p.primary == Some(m.face_id()))
                        } else {
                            format!(
                                "primary_same={} fallback={:?}",
                                p.primary == Some(m.face_id()),
                                p.fallbacks
                                    .iter()
                                    .map(|id| family_name(*id))
                                    .collect::<Vec<_>>()
                            )
                        }
                    );
                }
                None => eprintln!("{name:<14} {mine:>9.3} {:>9} (usvg: no text node)", "-"),
            }
        }
        // Wall-clock cost of a warm call, for the record (the asserting guard is structural —
        // `face_data_is_opened_only_on_a_cache_miss`).
        for (_, t) in CORPUS {
            let _ = m.measure(t, FONT_SIZE);
        }
        for (name, text) in [("short label", "Preview"), ("cjk label", "処理を実行する")] {
            let n = 100_000;
            let t0 = std::time::Instant::now();
            let mut acc = 0.0f32;
            for _ in 0..n {
                acc += m.measure(text, FONT_SIZE);
            }
            let ns = t0.elapsed().as_nanos() as f64 / n as f64;
            eprintln!("warm measure({name}) = {ns:.0} ns/call (acc={acc:.0})");
        }
    }
}
