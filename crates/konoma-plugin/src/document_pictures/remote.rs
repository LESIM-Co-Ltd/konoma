//! Remote images of a document: where a downloaded picture is kept on disk, and the download.
//!
//! One rule for every host: a document's `http(s)://` image is fetched when `ui.remote_images`
//! allows it (the default), into a content-addressed file under the cache root, and read back like a
//! local file. The fetch is bounded (timeout, body size, a check that the bytes are an image) and the
//! cache directory is pruned after a new download.

use std::path::{Path, PathBuf};

use super::files::md_image_dims;

/// Maximum bytes to download for one remote image (guards against huge / hostile responses).
pub const MD_REMOTE_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Max files kept in the remote-image disk cache (`<cache root>/konoma/remote-images`).
const MD_REMOTE_CACHE_MAX: usize = 256;

/// Result of a background remote-image download, delivered to the host's loop. It carries no
/// bytes: on success the file is already in the cache, so the host only has to lay the page out
/// again; on failure the URL is remembered so it is not retried.
pub struct RemoteFetch {
    pub url: String,
    pub ok: bool,
}

/// Downloads `url` into `dest` and says whether a valid image is there now. A host passes
/// [`fetch_remote_image`]; a test passes a stand-in so it never reaches the network.
pub type Fetcher = fn(&str, &Path) -> bool;

/// Pure core of [`cache_root`]. Falls back to the system temp directory when neither
/// `$XDG_CACHE_HOME` nor `$HOME` is available: this is a *cache*, so losing it is harmless, whereas
/// no root at all would leave the download unattempted and the picture "loading" forever.
pub fn cache_root_from(xdg_or_home: Option<PathBuf>) -> PathBuf {
    xdg_or_home.unwrap_or_else(std::env::temp_dir)
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static TEST_CACHE_ROOT: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam: the cache root on this thread (a sandboxed directory), so a test that reaches a
/// download never writes under the real cache.
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_test_cache_root(path: PathBuf) {
    TEST_CACHE_ROOT.with(|c| *c.borrow_mut() = Some(path));
}

/// Forget the override of [`set_test_cache_root`] on this thread.
#[cfg(any(test, feature = "test-hooks"))]
pub fn clear_test_cache_root() {
    TEST_CACHE_ROOT.with(|c| *c.borrow_mut() = None);
}

/// The override [`set_test_cache_root`] set on this thread, if any.
#[cfg(any(test, feature = "test-hooks"))]
pub fn get_test_cache_root() -> Option<PathBuf> {
    TEST_CACHE_ROOT.with(|c| c.borrow().clone())
}

fn env_cache_root() -> PathBuf {
    cache_root_from(konoma_base::xdg::xdg_base_dir(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
        ".cache",
    ))
}

/// Root of the on-disk cache (`$XDG_CACHE_HOME`, or `$HOME/.cache`, or a system-temp fallback).
/// A test build (or `test-hooks`) prefers the override of [`set_test_cache_root`]; with none set it
/// reads the environment like production (the path arithmetic touches no file by itself).
pub fn cache_root() -> PathBuf {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(root) = get_test_cache_root() {
        return root;
    }
    env_cache_root()
}

/// Deterministic cache path for a remote image URL: `<cache>/konoma/remote-images/<hash>`. The file
/// is stored without an extension (the content type is unknown until fetched) and read via content
/// sniffing.
pub fn md_remote_cache_path(url: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let root = cache_root();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.trim().hash(&mut hasher);
    Some(
        root.join("konoma")
            .join("remote-images")
            .join(format!("{:016x}", hasher.finish())),
    )
}

/// Download a remote image with `ureq` into `dest` (atomically, via a temp file), validating that the
/// bytes decode as an image before committing. Returns whether a valid image now exists at `dest`.
/// Runs on a background thread. `ureq` is pure Rust (rustls + webpki-roots, no OpenSSL), so this
/// adds no external process and no system TLS dependency.
pub fn fetch_remote_image(url: &str, dest: &Path) -> bool {
    fetch_remote_image_capped(url, dest, MD_REMOTE_MAX_BYTES)
}

/// [`fetch_remote_image`] with the body-size cap as a parameter, split out so a test can exercise
/// the exact `.limit()` enforcement production uses against a tiny response.
pub fn fetch_remote_image_capped(url: &str, dest: &Path, max_bytes: u64) -> bool {
    let Some(parent) = dest.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let tmp = dest.with_extension("part");
    let Some(bytes) = fetch_remote_bytes_capped(url, max_bytes) else {
        let _ = std::fs::remove_file(&tmp);
        return false;
    };
    if std::fs::write(&tmp, &bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    // Reject non-images (e.g. an HTML error page served with 200) before caching them (accepts SVG).
    if md_image_dims(&tmp).is_none() && !konoma_svg::svg::is_svg(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    let ok = std::fs::rename(&tmp, dest).is_ok();
    if ok {
        prune_remote_cache(parent, MD_REMOTE_CACHE_MAX);
    }
    ok
}

/// Perform the actual HTTP GET, bounded by a global timeout (a background thread must not hang
/// forever) and a max body size. Redirects are followed (GitHub proxies images through camo) and
/// `gzip` is decoded. `None` on any failure: bad URL, connect/TLS error, non-2xx status, or an
/// oversized body.
pub fn fetch_remote_bytes_capped(url: &str, max_bytes: u64) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .user_agent("konoma image preview")
        .build()
        .into();
    let mut resp = agent.get(url).call().ok()?;
    resp.body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_vec()
        .ok()
}

/// Keep at most `keep` files in the remote-image cache directory, deleting the oldest (by mtime).
/// Runs only after a **new** download (rare), so the directory scan is cheap. Best-effort: any IO
/// error just leaves the cache as-is.
pub fn prune_remote_cache(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let p = e.path();
            // Exclude an in-progress temp file (.part).
            if p.extension().is_some_and(|x| x == "part") {
                return None;
            }
            let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((mtime, p))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(t, _)| *t); // oldest first
    for (_, p) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(p);
    }
}
