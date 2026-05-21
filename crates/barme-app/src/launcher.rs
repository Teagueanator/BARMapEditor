//! BAR install integration — locate the user-writable maps directory and
//! drop a built `.sd7` into it (ADR-015).
//!
//! The Recoil engine's "spring-launcher" stores user maps under a
//! platform-resolved write root; for BAR that root is named
//! `"Beyond All Reason"` (per `BYAR-Chobby/dist_cfg/config.json`'s `title`
//! field). The user-maps directory is `<writeRoot>/maps/`.
//!
//! Resolution mirrors `beyond-all-reason/spring-launcher`'s
//! `src/write_path.js` to stay drop-in compatible with whatever the lobby
//! already writes:
//!
//! - **Linux:** `$XDG_STATE_HOME/Beyond All Reason/maps`, falling back to
//!   `$HOME/Documents/Beyond All Reason/maps` (legacy migration), then
//!   `$HOME/.local/state/Beyond All Reason/maps`.
//! - **Windows / macOS:** the launcher is portable on Windows (writes next
//!   to its install dir, no fixed system path) and BAR is unsupported on
//!   macOS. We return `None` and the UI surfaces a "pick the maps directory"
//!   fallback (Stage 1 polish).
//!
//! "Install" means copy, not symlink. Symlinks on Windows have admin/Developer
//! Mode caveats and BAR's archive scanner is indifferent.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

/// Sub-path BAR's spring-launcher writes under each platform's resolved
/// write root. The leaf `maps/` is where archive scanner expects custom
/// `.sd7` files (per `RecoilEngine` `ArchiveScanner.cpp` and the
/// `gist:burnhamrobertp/97cae4d300e675ca261e661fc58266d1` reference).
const BAR_WRITE_ROOT_NAME: &str = "Beyond All Reason";

#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Probe BAR's user-writable maps directory using the same precedence
/// `spring-launcher` uses (`src/write_path.js`). Returns the deepest
/// existing-or-creatable candidate, or `None` if no platform-appropriate
/// path is known.
///
/// **Side effect:** the returned path is *not* created. Call
/// [`install_sd7`] to materialise it on demand.
pub fn bar_maps_dir() -> Option<PathBuf> {
    let candidates = bar_maps_candidates();
    if candidates.is_empty() {
        warn!("no platform-appropriate BAR maps-dir candidates");
        return None;
    }
    // Prefer the first candidate that already exists (so we line up with the
    // dir the lobby is actually using). If none exist yet, return the
    // highest-priority one — install_sd7 will create it.
    if let Some(existing) = candidates.iter().find(|p| p.is_dir()) {
        info!(?existing, "located existing BAR maps dir");
        return Some(existing.clone());
    }
    let first = candidates.into_iter().next();
    if let Some(p) = &first {
        info!(
            ?p,
            "no BAR maps dir exists yet — picked highest-priority candidate"
        );
    }
    first
}

#[cfg(target_os = "linux")]
fn bar_maps_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let base_dirs = directories::BaseDirs::new();

    // 1. $XDG_STATE_HOME (or ~/.local/state)
    if let Some(state) = base_dirs.as_ref().and_then(|b| b.state_dir()) {
        out.push(state.join(BAR_WRITE_ROOT_NAME).join("maps"));
    }
    // 2. legacy ~/Documents/<title> (spring-launcher migration check)
    if let Some(docs) =
        directories::UserDirs::new().and_then(|u| u.document_dir().map(Path::to_path_buf))
    {
        out.push(docs.join(BAR_WRITE_ROOT_NAME).join("maps"));
    }
    // 3. explicit ~/.local/state fallback for hosts without state_dir support
    //    (state_dir is Linux-only in `directories`, so this is belt-and-braces).
    if let Some(home) = base_dirs.as_ref().map(|b| b.home_dir().to_path_buf()) {
        out.push(
            home.join(".local/state")
                .join(BAR_WRITE_ROOT_NAME)
                .join("maps"),
        );
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn bar_maps_candidates() -> Vec<PathBuf> {
    // Windows: spring-launcher is portable (writes <install>/data/maps).
    // No fixed system path; defer to a user-pick fallback in the UI.
    // macOS: BAR is unsupported.
    Vec::new()
}

/// Copy `src` into `dst_dir`, creating `dst_dir` if it doesn't exist.
/// Returns the destination path. Overwrites any pre-existing file at the
/// target.
pub fn install_sd7(src: &Path, dst_dir: &Path) -> Result<PathBuf, LauncherError> {
    if !dst_dir.exists() {
        info!(?dst_dir, "creating BAR maps dir");
        std::fs::create_dir_all(dst_dir).map_err(|source| LauncherError::Io {
            path: dst_dir.to_path_buf(),
            source,
        })?;
    }
    let file_name = src.file_name().ok_or_else(|| LauncherError::Io {
        path: src.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "source has no file name"),
    })?;
    let dst = dst_dir.join(file_name);
    info!(?src, ?dst, "installing .sd7");
    std::fs::copy(src, &dst).map_err(|source| LauncherError::Io {
        path: dst.clone(),
        source,
    })?;
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn candidates_are_under_beyond_all_reason() {
        let cs = bar_maps_candidates();
        assert!(!cs.is_empty(), "expected at least one Linux candidate");
        for c in cs {
            assert!(
                c.to_string_lossy().contains("Beyond All Reason/maps"),
                "candidate not under BAR write root: {}",
                c.display()
            );
        }
    }

    #[test]
    fn install_sd7_copies_file_and_creates_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("fake.sd7");
        std::fs::write(&src, b"7z\xbc\xaf'\x1c").unwrap();
        let dst_dir = tmp.path().join("nested/maps-dir");
        let dst = install_sd7(&src, &dst_dir).unwrap();
        assert_eq!(dst, dst_dir.join("fake.sd7"));
        assert!(dst.is_file());
        assert_eq!(std::fs::read(&dst).unwrap(), b"7z\xbc\xaf'\x1c");
    }

    #[test]
    fn install_sd7_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("fake.sd7");
        std::fs::write(&src, b"new").unwrap();
        let dst_dir = tmp.path().join("maps");
        std::fs::create_dir_all(&dst_dir).unwrap();
        std::fs::write(dst_dir.join("fake.sd7"), b"old-and-longer").unwrap();
        let dst = install_sd7(&src, &dst_dir).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"new");
    }
}
