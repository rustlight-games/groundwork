//! Traced pages on disk, and how the game finds them.
//!
//! The path tracer takes seconds a page and the game has a frame, so Cycles can
//! never run inside the render loop. It does not have to. A page is a *cache* —
//! its content is a pure function of the world coordinate and the seed — so a
//! page traced last week is exactly the page the rasteriser would produce if it
//! had the time. Trace it once, store it, and the game reads it.
//!
//! That makes the rasteriser the **fallback** rather than the way the game is
//! meant to look: it covers ground nobody has traced yet, and the picture
//! improves as the cache fills rather than as the renderer gets faster.
//!
//! ## The key has to name everything that changes the picture
//!
//! ```text
//!   seed · origin · size · scale  →  <hash>.raw
//! ```
//!
//! All four, because all four change the pixels. Leaving one out is worse than
//! having no cache: a page keyed only by position would be served for the wrong
//! seed, and the failure would be a patch of some other world sitting in the
//! middle of this one — which looks like a rendering bug rather than a stale
//! file, and would be hunted for in the renderer.
//!
//! The scale is quantised before it is hashed. It is an `f32` derived from a
//! camera height, so two runs that meant the same detail level can differ in the
//! last bit and miss a cache that is sitting right there.
//!
//! ## Raw RGBA, not PNG
//!
//! The game reads these on a background thread while the camera is moving, and
//! it wants bytes it can hand to a texture. A decode is work with no purpose
//! here — the files are build output, not something anyone ships or diffs, and
//! the disk they cost is measured against a `target/` directory that already
//! holds gigabytes.

use std::path::{Path, PathBuf};

use crate::bake::{BakeParams, Page};

/// Where traced pages live, unless [`TERRAIN_GRASS_CACHE`] says otherwise.
pub const DEFAULT_DIRECTORY: &str = "target/grass-pages";

/// Environment variable pointing at the page cache.
pub const TERRAIN_GRASS_CACHE: &str = "TERRAIN_GRASS_CACHE";

/// Set this to read traced pages. Unset, the game rasterises everything.
///
/// Off by default, and that is a correctness decision rather than caution.
///
/// A traced page and a rasterised one are not two qualities of one picture, they
/// are two pictures — different tone, different saturation, different blade
/// vocabulary. Serving whichever happens to be on disk puts them side by side,
/// and a single traced page in a field of rasterised ones does not read as "one
/// page is better", it reads as **a rendering fault**: a hard-edged square of
/// some other grass in the middle of the ground. Which is exactly what it looked
/// like the first time this shipped.
///
/// So the mixing is opt-in. Trace a region, set the variable, and every page in
/// that region is traced; leave it unset and the whole field is consistent.
pub const TERRAIN_GRASS_TRACED: &str = "TERRAIN_GRASS_TRACED";

/// Whether the game should read traced pages at all.
pub fn traced_enabled() -> bool {
    std::env::var(TERRAIN_GRASS_TRACED).is_ok_and(|v| v != "0" && !v.is_empty())
}

/// Bytes one page occupies: RGBA, one byte a channel.
#[inline]
fn expected_len(page: &Page) -> usize {
    page.width * page.height * 4
}

/// The cache directory in force.
pub fn directory() -> PathBuf {
    std::env::var(TERRAIN_GRASS_CACHE)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DIRECTORY))
}

/// A stable name for a page, covering everything that changes its pixels.
///
/// Hashed rather than spelled out because a filename holding four floats is
/// both unreadable and fragile — `-0` and `0` format differently, and a path
/// with a minus sign in it has caught out every shell script ever written.
pub fn key(page: &Page, params: &BakeParams) -> String {
    // The scale, in thousandths. See the module note: an `f32` that came from a
    // camera height will not compare equal to itself across two runs.
    let scale = (page.px_per_metre * 1000.0).round() as i64;
    let origin_x = (page.origin.x * 100.0).round() as i64;
    let origin_y = (page.origin.y * 100.0).round() as i64;

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for value in [
        params.seed as i64,
        origin_x,
        origin_y,
        page.width as i64,
        page.height as i64,
        scale,
    ] {
        hash ^= value as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
        hash ^= hash >> 29;
    }
    format!("{hash:016x}")
}

/// Where a page would be stored.
pub fn path_for(page: &Page, params: &BakeParams) -> PathBuf {
    directory().join(format!("{}.raw", key(page, params)))
}

/// A traced page, if one has been stored.
///
/// Returns `None` for anything at all suspicious rather than propagating an
/// error. A cache is an optimisation, and the correct response to a truncated or
/// unreadable file is to draw the page rather than to fail — the caller has a
/// renderer standing by, which is the whole reason this returns an `Option`.
pub fn load(page: &Page, params: &BakeParams) -> Option<Vec<u8>> {
    if !traced_enabled() {
        return None;
    }
    load_from(&directory(), page, params)
}

/// [`load`], from a named directory.
///
/// The directory is a parameter rather than read from the environment inside
/// because `set_var` is `unsafe` in this edition and the crate forbids unsafe.
/// Threading it through is also simply better: a test that reached for an
/// environment variable would be mutating global state that every other test in
/// the process shares.
pub fn load_from(directory: &Path, page: &Page, params: &BakeParams) -> Option<Vec<u8>> {
    let path = directory.join(format!("{}.raw", key(page, params)));
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() != expected_len(page) {
        // A page whose size changed since it was traced. The key covers the
        // size, so this means a truncated write rather than a stale entry, and
        // the file is worth removing so the next pre-bake replaces it.
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(bytes)
}

/// Store a traced page.
pub fn store(page: &Page, params: &BakeParams, rgba: &[u8]) -> std::io::Result<PathBuf> {
    store_in(&directory(), page, params, rgba)
}

/// [`store`], into a named directory.
pub fn store_in(
    directory: &Path,
    page: &Page,
    params: &BakeParams,
    rgba: &[u8],
) -> std::io::Result<PathBuf> {
    assert_eq!(
        rgba.len(),
        expected_len(page),
        "a page of {}x{} is {} bytes, not {}",
        page.width,
        page.height,
        expected_len(page),
        rgba.len()
    );
    let path = directory.join(format!("{}.raw", key(page, params)));
    std::fs::create_dir_all(directory)?;
    // Written beside and renamed, so a run interrupted halfway through a page
    // cannot leave a half-file that the loader would have to guess about.
    let temporary = path.with_extension("raw.part");
    std::fs::write(&temporary, rgba)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

/// How many pages are stored, for reporting.
pub fn count() -> usize {
    count_in(&directory())
}

fn count_in(directory: &Path) -> usize {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|e| e == "raw"))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    fn page() -> Page {
        Page::new(Vec2::new(256.0, -512.0), 8, 8)
    }

    #[test]
    fn the_key_changes_with_everything_that_changes_the_picture() {
        let base = page();
        let params = BakeParams::default();
        let original = key(&base, &params);

        let moved = Page::new(Vec2::new(257.0, -512.0), 8, 8);
        assert_ne!(
            original,
            key(&moved, &params),
            "origin does not reach the key"
        );

        let bigger = Page::new(base.origin, 16, 8);
        assert_ne!(
            original,
            key(&bigger, &params),
            "size does not reach the key"
        );

        let finer = Page::at_detail(base.origin, 8, 8, 0.5);
        assert_ne!(
            original,
            key(&finer, &params),
            "scale does not reach the key"
        );

        let reseeded = BakeParams {
            seed: params.seed ^ 1,
            ..params
        };
        assert_ne!(
            original,
            key(&base, &reseeded),
            "seed does not reach the key"
        );
    }

    #[test]
    fn the_same_page_keys_the_same_way_twice() {
        let params = BakeParams::default();
        assert_eq!(key(&page(), &params), key(&page(), &params));
    }

    #[test]
    fn a_stored_page_comes_back_byte_for_byte() {
        let directory = std::env::temp_dir().join("bw-grass-cache-roundtrip");
        let _ = std::fs::remove_dir_all(&directory);

        let page = page();
        let params = BakeParams::default();
        let bytes: Vec<u8> = (0..expected_len(&page)).map(|i| (i % 251) as u8).collect();

        store_in(&directory, &page, &params, &bytes).expect("store");
        assert_eq!(
            load_from(&directory, &page, &params).as_deref(),
            Some(bytes.as_slice())
        );
        assert_eq!(count_in(&directory), 1);

        // A different seed must miss rather than return this page. This is the
        // failure the key exists to prevent: a page of some other world sitting
        // in the middle of this one looks like a renderer bug, not a stale file.
        let elsewhere = BakeParams {
            seed: params.seed ^ 0xffff,
            ..params
        };
        assert!(load_from(&directory, &page, &elsewhere).is_none());

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_truncated_page_is_refused_and_removed() {
        let directory = std::env::temp_dir().join("bw-grass-cache-truncated");
        let _ = std::fs::remove_dir_all(&directory);

        let page = page();
        let params = BakeParams::default();
        std::fs::create_dir_all(&directory).expect("mkdir");
        let path = directory.join(format!("{}.raw", key(&page, &params)));
        std::fs::write(&path, [0u8; 4]).expect("write");

        assert!(
            load_from(&directory, &page, &params).is_none(),
            "a short file was served as a page"
        );
        assert!(
            !path.exists(),
            "the short file was left for the next run to trip over"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
