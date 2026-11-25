//! The decode of one canvas answer: name every placement, then decode what is new within the work
//! limits of the answer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use konoma_svg::image::ImageFailure;

use super::budget::{
    copy_decoded, decode_cost, decoded_bytes, decoded_pixels, header_pixels_within, input_bytes,
};
use super::decode::decode_one;
use super::files::refused_by_scope;
use super::hash::{same_source, Hasher};
use super::{
    mix_scope, CanvasDecoded, CanvasHeld, CanvasPictures, CanvasPlace, CanvasPolicy, KeptPicture,
    MAX_CANVAS_IMAGES,
};
use crate::view::{CanvasView, ImageData};

/// Decode the pictures of `canvas` that `held` (what the cache holds) does not have. Runs on the
/// worker. `path` is the file shown (relative references of an SVG resolve from it). The host's
/// choices (read scope, limits, cancellation, SVG drawer) are the `policy`.
///
/// Two passes: the first names every placement (hashing each id's content once; an `unchanged`
/// picture resolves through `held` and is not hashed), which also tells how much of the memory
/// bound the canvas keeps anyway; the second decodes what is new within the limits of one answer.
pub fn decode_canvas(
    plugin_id: &str,
    canvas: &CanvasView,
    held: &CanvasHeld,
    path: &Path,
    policy: &CanvasPolicy,
) -> CanvasPictures {
    let CanvasPolicy {
        scope,
        limits,
        cancelled,
        ..
    } = *policy;
    let is_cancelled = cancelled;
    // Pass 1: the placements. The first placement of an id decides its key (an id is one picture).
    // Each id is resolved to a key without decoding anything: a picture the cache holds, one to
    // decode (`todo`), or one left for later. The pictures the host kept from an earlier answer
    // go first, so a picture deferred before is not starved by the new ones.
    let mut hasher = Hasher::new(cancelled).relative_to(path);
    let scope_fp = scope.fingerprint();
    // (order of appearance, id, the placement that decided it)
    let mut ids: Vec<(usize, u64, &crate::view::CanvasImage)> = Vec::new();
    {
        let mut seen: HashSet<u64> = HashSet::new();
        for im in &canvas.images {
            if seen.len() >= MAX_CANVAS_IMAGES && !seen.contains(&im.id) {
                continue; // past the second line of defence: not looked at
            }
            if seen.insert(im.id) {
                ids.push((ids.len(), im.id, im));
            }
        }
    }
    ids.sort_by_key(|(order, id, _)| (!held.kept.contains_key(id), *order));
    let mut key_of: HashMap<u64, Arc<str>> = HashMap::new();
    // The pictures to decode, each key once: (order, id, key, hash, data, needs).
    let mut todo: Vec<Todo> = Vec::new();
    // The pictures not hashed because the input budget of this answer was used.
    let mut unhashed = 0usize;
    // The kept sources not turned into `todo` (not hashed yet).
    let mut sources: Vec<(u64, KeptPicture)> = Vec::new();
    let mut hashed_any = false;
    for (order, id, im) in ids {
        let kept = held.kept.get(&id);
        // The data this id is about: what the plugin sent, or for `unchanged` what the host kept.
        let (data, shared): (&ImageData, Option<&Arc<ImageData>>) = match (&im.image, kept) {
            (Some(d), _) => (d, None),
            (None, Some(k)) => (&*k.data, Some(&k.data)),
            (None, None) => {
                // Nothing kept: the picture is in the cache or the id is not held.
                if let Some(k) = held.by_id.get(&id) {
                    key_of.insert(id, k.clone());
                }
                continue;
            }
        };
        // A hash made earlier for the very same buffers is reused (no hashing).
        let known = kept
            .filter(|k| k.hash.is_some() && same_source(&k.data, data))
            .and_then(|k| k.hash);
        let hash = match known {
            Some(h) => h,
            None => {
                // The input budget, before the hashing (as for the decode: a picture starts while
                // less than the budget is used): the first picture is always hashed, so an answer
                // makes progress; the others wait for the next answer (unhashed: the picture the
                // id was drawn from stays on screen).
                if hashed_any && hasher.bytes >= limits.input_bytes {
                    unhashed += 1;
                    if let Some(k) = held.by_id.get(&id) {
                        key_of.insert(id, k.clone());
                    }
                    sources.push((
                        id,
                        KeptPicture {
                            key: None,
                            hash: None,
                            data: shared.cloned().unwrap_or_else(|| Arc::new(data.clone())),
                            needs: None,
                        },
                    ));
                    continue;
                }
                match hasher.content(data) {
                    Some(h) => {
                        hashed_any = true;
                        h
                    }
                    None => {
                        // Cancelled while hashing: the answer is dropped on arrival.
                        return CanvasPictures {
                            scope_fp: scope.fingerprint(),
                            places: canvas
                                .images
                                .iter()
                                .map(|im| CanvasPlace {
                                    id: im.id,
                                    rect: im.rect(),
                                    key: None,
                                })
                                .collect(),
                            fresh: Vec::new(),
                            waiting: 0,
                            refused: 0,
                            more: false,
                            sources: Vec::new(),
                        };
                    }
                }
            }
        };
        // The scope is part of the key: a file judged under one scope is never served from the
        // cache for another (the tab root moved, `read_dirs` changed). `0` (no limit) leaves the
        // content hash as it is.
        let key: Arc<str> =
            konoma_widgets::markdown::canvas_url(plugin_id, id, mix_scope(hash, scope_fp)).into();
        key_of.insert(id, key.clone());
        if !held.keys.contains_key(Path::new(&*key)) {
            // The refusal of the memory bound is remembered with the picture, so it is not decoded
            // again to be refused again.
            let needs = kept
                .filter(|k| k.key.as_deref() == Some(&*key))
                .and_then(|k| k.needs);
            todo.push(Todo {
                order,
                id,
                key: key.clone(),
                hash,
                data: shared.cloned().unwrap_or_else(|| Arc::new(data.clone())),
                needs,
            });
        }
    }
    todo.sort_by_key(|t| t.order);
    let places: Vec<CanvasPlace> = canvas
        .images
        .iter()
        .map(|im| CanvasPlace {
            id: im.id,
            rect: im.rect(),
            key: key_of.get(&im.id).cloned(),
        })
        .collect();
    // What the canvas keeps held whatever happens: the entries it names, and the entry each of
    // its ids is drawn from now (until the newer content can be drawn). The rest goes when the
    // answer lands, so only this counts against the memory bound.
    let mut kept: HashSet<&str> = key_of.values().map(|k| &**k).collect();
    for (id, key) in &key_of {
        if let Some(f) = held.front.get(id) {
            // The entry an id is drawn from is replaced by its new content when that lands (the
            // host's landing step frees it for the room), so it is not extra
            // held next to the new content. One whose new content is already in the cache stays
            // counted: the cache holds both until the next landing's prune.
            let being_replaced = **f != **key && !held.keys.contains_key(Path::new(&**key));
            if !being_replaced {
                kept.insert(&**f);
            }
        }
    }
    let kept_bytes: u64 = kept
        .iter()
        .filter_map(|k| held.keys.get(Path::new(k)))
        .sum();
    // Pass 2: decode within the limits.
    let mut fresh: Vec<(PathBuf, CanvasDecoded)> = Vec::new();
    // content hash -> index in `fresh` (an id that holds content already decoded in this answer
    // gets a copy; nothing is decoded twice).
    let mut by_hash: HashMap<u64, usize> = HashMap::new();
    let mut added = 0u64;
    let (mut decodes, mut input) = (0usize, 0u64);
    // The header pixels of the pictures this answer started to decode (`header_pixels`).
    let mut header_px = 0u64;
    // Left for a following answer by the work limits / refused for the memory bound for now.
    let (mut waiting, mut refused) = (unhashed, 0usize);
    let keep = |t: Todo, needs: Option<u64>| {
        (
            t.id,
            KeptPicture {
                key: Some(t.key),
                hash: Some(t.hash),
                data: t.data,
                needs,
            },
        )
    };
    for t in todo {
        let cancelled = is_cancelled();
        let room = limits.held_bytes.saturating_sub(kept_bytes + added);
        // What the decode would cost by its header, asked before anything is decoded (not for a
        // picture that is a copy of one decoded already).
        let out_of_scope = !cancelled && refused_by_scope(&t.data, path, scope);
        let (hp, hp_after) = if cancelled || out_of_scope || by_hash.contains_key(&t.hash) {
            (0, false)
        } else {
            header_pixels_within(&t.data, path, scope, is_cancelled)
        };
        let decoded = if cancelled {
            // The preview moved on: the answer is dropped on arrival; do not spend time on it.
            CanvasDecoded::Failed(ImageFailure::Cancelled)
        } else if out_of_scope {
            // Outside the read scope: refused before any budget is looked at and without opening
            // a byte of it. It counts for nothing (no decode, no input, no header pixels, no
            // answer bytes), so it never makes the in-scope pictures after it wait.
            CanvasDecoded::Failed(ImageFailure::OutsideReadDirs)
        } else if let Some(&i) = by_hash.get(&t.hash) {
            let d = copy_decoded(&fresh[i].1);
            let bytes = decoded_bytes(&d);
            if bytes > room {
                refused += 1;
                sources.push(keep(t, Some(bytes)));
                continue;
            }
            d
        } else if t.needs.is_some_and(|n| n > room) {
            // Refused before, and the room has not grown to what it needs: nothing is decoded.
            refused += 1;
            let needs = t.needs;
            sources.push(keep(t, needs));
            continue;
        } else if matches!(t.data.as_ref(), ImageData::Frames(_))
            && (decode_cost(&t.data) > limits.decodes
                || input_bytes(&t.data, path) > limits.input_bytes
                || hp > limits.header_pixels)
        {
            // An animation that alone costs more than one answer may do (more SVG / file frames than
            // the decode budget, more input than the input budget; only ready `Rgba` frames are not counted) can never be decoded: it is
            // refused for good (the cells stay) instead of being decoded without a bound as "the
            // first picture of an answer" (5000 SVG frames are 5000 child processes). Every animation that is not refused here costs at most the budget
            // by itself, so the first picture of an answer always fits and an answer makes
            // progress, the work of one answer bounded by the budgets whatever order the
            // pictures come in. A single image is the exception that keeps an answer moving: it
            // is decoded even when it is larger than the input budget, bounded by its own limits
            // (64 MiB of file, the decoders' size limits).
            CanvasDecoded::Failed(ImageFailure::TooLarge)
        } else if decodes >= limits.decodes
            // Every frame of an animation is a decode (a picture of N frames costs N): one that
            // does not fit next to what this answer decoded waits (kept) for a later answer, where
            // it may come first.
            || decodes + decode_cost(&t.data) > limits.decodes
            || input >= limits.input_bytes
            || added >= limits.answer_bytes
            // The header pixels (`MAX_HEADER_PIXELS_PER_ANSWER`), judged before the decode: a
            // picture whose header announces more work than is left waits, unless it is the
            // first of the answer.
            || (decodes > 0 && header_px.saturating_add(hp) > limits.header_pixels)
        {
            waiting += 1;
            let needs = t.needs;
            sources.push(keep(t, needs));
            continue;
        } else {
            #[cfg(any(test, feature = "test-hooks"))]
            seen_for_tests::note(t.hash);
            decodes += decode_cost(&t.data);
            input += input_bytes(&t.data, path);
            header_px = header_px.saturating_add(hp);
            let d = decode_one(&t.data, path, policy);
            if hp_after {
                // The header part was counted above; add what the drawing made of the rest.
                header_px = header_px.saturating_add(decoded_pixels(&d).saturating_sub(hp));
            }
            let bytes = decoded_bytes(&d);
            if bytes > limits.held_bytes {
                // It can never fit.
                CanvasDecoded::Failed(ImageFailure::TooLarge)
            } else if bytes > room {
                // It does not fit next to what the canvas keeps: not final, tried again later.
                refused += 1;
                sources.push(keep(t, Some(bytes)));
                continue;
            } else {
                d
            }
        };
        added += decoded_bytes(&decoded);
        if !matches!(decoded, CanvasDecoded::Failed(_)) {
            by_hash.entry(t.hash).or_insert(fresh.len());
        }
        let key = PathBuf::from(&*t.key);
        sources.push(keep(t, None));
        fresh.push((key, decoded));
    }
    // Asking again helps when the work limits left pictures and this answer made progress, or when
    // pictures were not even hashed (the input budget): the host keeps those first in line, so the
    // next answer always moves on, whether or not this one decoded anything.
    let more = (waiting > 0 && !fresh.is_empty()) || unhashed > 0;
    CanvasPictures {
        scope_fp: scope.fingerprint(),
        places,
        fresh,
        more,
        waiting,
        refused,
        sources,
    }
}

/// A picture to decode in pass 2.
struct Todo {
    /// Where the id first appears in the canvas (the order of decoding).
    order: usize,
    id: u64,
    key: Arc<str>,
    hash: u64,
    data: Arc<ImageData>,
    /// The decoded bytes it needed when the memory bound refused it.
    needs: Option<u64>,
}

/// Test-only record of which pictures the worker actually decoded (by content hash), so a test
/// can say that a picture sent again was not decoded again.
#[cfg(any(test, feature = "test-hooks"))]
pub mod seen_for_tests {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    pub(super) fn note(hash: u64) {
        if let Ok(mut v) = SEEN.lock() {
            v.push(hash);
        }
    }
    /// How many times the picture with this content hash was decoded.
    pub fn decoded_count(hash: u64) -> usize {
        SEEN.lock()
            .map(|v| v.iter().filter(|h| **h == hash).count())
            .unwrap_or(0)
    }
}
