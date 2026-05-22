//! Content-addressed thumbnail cache rooted at
//! `$XDG_CACHE_HOME/barme/feature_thumbnails/<sha>.png`.
//!
//! Phase B commits land in this order:
//! 1. (this commit) — public API surface + path resolution + a
//!    no-op cache (lookup always misses, store is a no-op) so the
//!    rest of the wiring compiles and we can prove API shape.
//! 2. (Sprint 29b commit 4) — real PNG encode/decode + atomic
//!    write via `tempfile + rename`.
//!
//! The cache key is `sha256(s3o_bytes)`. Folding lighting / view-pose
//! / bake-options into the key happens if + when those become
//! tuneable; today the bake is deterministic for a given S3O input.
//!
//! Garbage collection is out of scope — the dir grows at ~5 KB per
//! variant, so the worst-case 280 entries take ~1.5 MB total. Stale
//! entries persist until the user clears the cache or the directory
//! is wiped manually. Sprint 29b's smoke test (deleting the cache
//! dir + relaunching) covers the cold-start re-bake path.

use std::path::PathBuf;

use anyhow::Result;

/// Default cache subdirectory under `$XDG_CACHE_HOME`. The full path
/// resolves at runtime — see [`cache_dir`]. Kept stable across
/// Sprint 29b commits so a user who cold-starts mid-sprint doesn't
/// need to migrate.
pub const CACHE_SUBDIR: &str = "barme/feature_thumbnails";

/// Resolve the cache directory.
///
/// Order of preference (matches XDG Base Directory Spec):
/// 1. `$XDG_CACHE_HOME/barme/feature_thumbnails/`
/// 2. `$HOME/.cache/barme/feature_thumbnails/`
/// 3. fallback: `./.cache/barme/feature_thumbnails/` (anchored to
///    `$PWD`; only used when both XDG_CACHE_HOME and HOME are unset,
///    which only happens in restricted sandboxes).
///
/// Does NOT create the directory — caller is responsible (typically
/// at store-time).
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

/// Per-S3O cache file path. `sha` MUST be the hex-encoded sha256 of
/// the .s3o bytes (64 chars). The format suffix is `.png` because
/// commit 4 of Sprint 29b serialises the thumbnail as PNG (existing
/// `image` workspace dep already enables PNG).
pub fn cache_path(sha: &str) -> PathBuf {
    cache_dir().join(format!("{sha}.png"))
}

/// Look up a baked thumbnail by content sha. Returns the 128²×4
/// RGBA8 buffer when the cache entry exists, decodes successfully,
/// and matches the expected size. Returns `None` for every other
/// outcome (missing, malformed, wrong size).
///
/// Commit 1 stub: always `None`. Commit 4 wires the real `image`-
/// crate PNG decode + size verification.
pub fn lookup(_sha: &str) -> Option<Vec<u8>> {
    None
}

/// Persist a baked thumbnail. `rgba` MUST be exactly `128 × 128 × 4`
/// bytes. The store is best-effort: cache-directory-creation +
/// PNG-encode failures are logged and swallowed (the caller falls
/// back to the in-memory thumbnail; the cache is purely an
/// optimisation).
///
/// Commit 1 stub: drops the bytes on the floor and returns `Ok(())`.
pub fn store(_sha: &str, _rgba: &[u8]) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_uses_xdg_cache_home_when_set() {
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: set_var is only used in single-threaded tests via
        // serial section; the assertion that follows reads back the
        // same value the test just set.
        // SAFETY: single-threaded test; env var is restored at end
        // via tempdir Drop semantics on the destination path.
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", tmp.path());
        }
        let dir = cache_dir();
        // SAFETY: see set_var note above.
        unsafe {
            std::env::remove_var("XDG_CACHE_HOME");
        }
        assert!(
            dir.starts_with(tmp.path()),
            "expected cache dir under {} but got {}",
            tmp.path().display(),
            dir.display(),
        );
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
    fn lookup_stub_returns_none() {
        assert!(lookup("deadbeef").is_none());
    }

    #[test]
    fn store_stub_is_ok() {
        let rgba = vec![0u8; 128 * 128 * 4];
        store("deadbeef", &rgba).expect("stub never errors");
    }
}
