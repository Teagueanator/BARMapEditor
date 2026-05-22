//! Content-addressed thumbnail cache rooted at
//! `$XDG_CACHE_HOME/barme/feature_thumbnails/<sha>.png`.
//!
//! Sprint 29b commit 4 — replaces commit 1's stubs with the real PNG
//! encode + decode + atomic store.
//!
//! ## Cache key
//!
//! `sha256(s3o_bytes)` — same .s3o file always maps to the same
//! cache filename, regardless of the user's clone path or upstream
//! commit. Renaming a vendored `.s3o` doesn't invalidate the cache
//! (the key is content, not name).
//!
//! ## Cache versioning
//!
//! Today the cache key folds nothing besides the .s3o content. If a
//! future sprint changes the rasteriser output (lighting model, UV
//! sampler, etc.), every cached entry becomes stale visually. The
//! current cure is "delete the cache dir and relaunch"; a per-bake
//! version byte could fold into the key when that becomes a routine
//! ask. Out of scope today — `0.35-ambient + Lambert + bilinear`
//! is the only rasteriser in production.
//!
//! ## Garbage collection
//!
//! None. The cache dir grows ~5 KB per entry; even 280 variants
//! (the upper bound for upstream mapfeatures) clocks in at ~1.5 MB.
//! Manual `rm -rf $XDG_CACHE_HOME/barme/feature_thumbnails/` is the
//! supported cleanup path.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{Context, Result};
use image::{ImageBuffer, Rgba};
use sha2::{Digest, Sha256};
use tracing::{trace, warn};

/// Default cache subdirectory under `$XDG_CACHE_HOME`.
pub const CACHE_SUBDIR: &str = "barme/feature_thumbnails";

/// Side length of a baked thumbnail in pixels. Mirrors
/// [`crate::thumbnail::SPRITE_SIZE`] so callers don't need to import
/// the thumbnail module just to validate cache entries.
pub const THUMBNAIL_SIDE: u32 = crate::thumbnail::SPRITE_SIZE;

/// Resolve the cache directory.
///
/// Preference order (matches XDG Base Directory Spec):
/// 1. `$XDG_CACHE_HOME/barme/feature_thumbnails/`
/// 2. `$HOME/.cache/barme/feature_thumbnails/`
/// 3. fallback: `./.cache/barme/feature_thumbnails/` (only when both
///    XDG_CACHE_HOME and HOME are unset).
///
/// Does NOT create the directory — [`store`] handles that.
pub fn cache_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join(CACHE_SUBDIR);
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".cache").join(CACHE_SUBDIR);
    }
    PathBuf::from(".cache").join(CACHE_SUBDIR)
}

/// Per-S3O cache file path.
pub fn cache_path(sha: &str) -> PathBuf {
    cache_dir().join(format!("{sha}.png"))
}

/// Hex-encode the sha256 of `bytes`. 64 lowercase chars. Stable
/// across runs / machines / endianness.
pub fn compute_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Look up a baked thumbnail by content sha. Returns the 128²×4
/// RGBA8 buffer when the cache entry exists, decodes successfully,
/// AND has the expected dimensions. Returns `None` for every other
/// outcome (missing file, malformed PNG, wrong size). Failures log
/// at `warn!` so a corrupted entry is visible without blocking the
/// re-bake.
pub fn lookup(sha: &str) -> Option<Vec<u8>> {
    let path = cache_path(sha);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "thumbnail cache read failed");
            return None;
        }
    };
    match image::load_from_memory_with_format(&bytes, image::ImageFormat::Png) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            if rgba.width() != THUMBNAIL_SIDE || rgba.height() != THUMBNAIL_SIDE {
                warn!(
                    path = %path.display(),
                    width = rgba.width(),
                    height = rgba.height(),
                    "cache entry has wrong dimensions; ignoring"
                );
                return None;
            }
            trace!(path = %path.display(), "thumbnail cache hit");
            Some(rgba.into_raw())
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "thumbnail cache PNG decode failed");
            None
        }
    }
}

/// Persist a baked thumbnail. `rgba` MUST be exactly
/// `THUMBNAIL_SIDE × THUMBNAIL_SIDE × 4` bytes. Atomic via
/// write-to-tempfile + rename — two parallel bakes targeting the
/// same key are safe (the last writer wins; neither leaves a
/// half-written file).
///
/// Best-effort: directory creation / PNG encode / rename failures
/// are logged + propagated. Callers typically swallow the Err and
/// fall back to the in-memory thumbnail; the cache is purely an
/// optimisation.
pub fn store(sha: &str, rgba: &[u8]) -> Result<()> {
    let expected = (THUMBNAIL_SIDE * THUMBNAIL_SIDE * 4) as usize;
    if rgba.len() != expected {
        anyhow::bail!(
            "thumbnail cache store: expected {expected} RGBA8 bytes, got {}",
            rgba.len()
        );
    }
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create cache dir {}", dir.display()))?;

    // Encode the RGBA buffer to PNG in memory before touching disk —
    // a failing encode never leaves a stale temp behind.
    let img: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_vec(THUMBNAIL_SIDE, THUMBNAIL_SIDE, rgba.to_vec())
            .ok_or_else(|| anyhow::anyhow!("thumbnail bytes wrong size for ImageBuffer"))?;
    let mut png = Vec::with_capacity(rgba.len() / 4);
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .with_context(|| "encode thumbnail to PNG")?;

    // Atomic publish: write to a sibling tempfile in the same dir,
    // then rename. NamedTempFile::persist is the POSIX-atomic move.
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)
        .with_context(|| format!("create cache tempfile in {}", dir.display()))?;
    use std::io::Write;
    tmp.write_all(&png).with_context(|| "write tempfile body")?;
    let final_path = cache_path(sha);
    tmp.persist(&final_path)
        .with_context(|| format!("rename tempfile -> {}", final_path.display()))?;
    trace!(path = %final_path.display(), "thumbnail cache store");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Global mutex that serialises every cache-test that touches
    /// XDG_CACHE_HOME. Cargo runs tests in parallel by default —
    /// two `XdgGuard`s racing each other would have each test see a
    /// directory created by some other test's tempdir.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard: take the env lock, redirect XDG_CACHE_HOME into
    /// a per-test tempdir, restore on drop. While the guard is alive
    /// every other cache test waits on the lock — keeps the env-var
    /// mutation effectively atomic from the cache module's view.
    struct XdgGuard {
        _tmp: tempfile::TempDir,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl XdgGuard {
        fn new() -> Self {
            // Recover from a poisoned mutex — a panicking test still
            // gives the next test a usable lock.
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let prev = std::env::var("XDG_CACHE_HOME").ok();
            // SAFETY: serialised via ENV_LOCK. The guard restores on
            // Drop, including on a panicking test (MutexGuard outlives
            // the env override).
            unsafe {
                std::env::set_var("XDG_CACHE_HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for XdgGuard {
        fn drop(&mut self) {
            // SAFETY: see new().
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                    None => std::env::remove_var("XDG_CACHE_HOME"),
                }
            }
        }
    }

    #[test]
    fn cache_dir_uses_xdg_cache_home_when_set() {
        let _g = XdgGuard::new();
        let dir = cache_dir();
        assert!(dir.ends_with(CACHE_SUBDIR));
    }

    #[test]
    fn cache_path_uses_64_char_sha_as_filename() {
        let sha = "a".repeat(64);
        let p = cache_path(&sha);
        assert_eq!(p.extension().and_then(|s| s.to_str()), Some("png"));
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap();
        assert_eq!(stem, sha);
    }

    #[test]
    fn sha_is_deterministic_64_hex_chars() {
        let a = compute_sha(b"hello world");
        let b = compute_sha(b"hello world");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // Known sha256("hello world").
        assert_eq!(
            a,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn store_then_lookup_round_trips() {
        let _g = XdgGuard::new();
        let sha = "f".repeat(64);
        // Generate a recognisable RGBA pattern: red gradient along x,
        // blue gradient along y.
        let mut rgba = Vec::with_capacity((THUMBNAIL_SIDE * THUMBNAIL_SIDE * 4) as usize);
        for y in 0..THUMBNAIL_SIDE {
            for x in 0..THUMBNAIL_SIDE {
                rgba.extend_from_slice(&[(x * 2) as u8, 0, (y * 2) as u8, 255]);
            }
        }
        store(&sha, &rgba).expect("store");
        let read = lookup(&sha).expect("hit after store");
        assert_eq!(read, rgba);
    }

    #[test]
    fn lookup_miss_returns_none() {
        let _g = XdgGuard::new();
        assert!(lookup(&"0".repeat(64)).is_none());
    }

    #[test]
    fn lookup_rejects_wrong_dimensions() {
        let _g = XdgGuard::new();
        let sha = "e".repeat(64);
        // Write a 32x32 PNG into the cache slot manually.
        let dir = cache_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let small: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_pixel(32, 32, Rgba([255, 0, 0, 255]));
        let mut png = Vec::new();
        small
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(cache_path(&sha), png).unwrap();
        assert!(
            lookup(&sha).is_none(),
            "lookup must reject wrong-size cache entries"
        );
    }

    #[test]
    fn store_rejects_wrong_buffer_size() {
        let _g = XdgGuard::new();
        let too_small = vec![0u8; 100];
        assert!(store(&"a".repeat(64), &too_small).is_err());
    }
}
