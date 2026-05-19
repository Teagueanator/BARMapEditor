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

use barme_core::Project;
use barme_pipeline::{PyMapConvDriver, build_sd7};
use image::{ImageBuffer, Rgb};
use tracing::{info, warn};

/// Biome gradient matching the editor's `terrain.wgsl::biome_ramp`
/// fallback (the height-keyed colour you see in the central viewport
/// when no splat is painted). Returned values are sRGB f32 in `[0,1]`.
///
/// Keep this **byte-equal** to the WGSL implementation so the baked
/// texture matches the editor preview. The cutoff thresholds and the
/// four colours are the canonical reference; changes here must be
/// mirrored to `crates/barme-app/src/terrain.wgsl::biome_ramp`.
fn biome_ramp(t: f32) -> [f32; 3] {
    if t < 0.05 {
        [0.227, 0.451, 0.604] // shoreline / water
    } else if t < 0.50 {
        [0.451, 0.616, 0.392] // grass
    } else if t < 0.82 {
        [0.502, 0.486, 0.439] // rock / dirt
    } else {
        [0.863, 0.878, 0.902] // snow / peak
    }
}

/// Sub-path BAR's spring-launcher writes under each platform's resolved
/// write root. The leaf `maps/` is where archive scanner expects custom
/// `.sd7` files (per `RecoilEngine` `ArchiveScanner.cpp` and the
/// `gist:burnhamrobertp/97cae4d300e675ca261e661fc58266d1` reference).
const BAR_WRITE_ROOT_NAME: &str = "Beyond All Reason";

#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    #[error(transparent)]
    Pipeline(#[from] barme_pipeline::BuildError),

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("texture synthesis failed for {path}: {source}")]
    TextureSynth {
        path: PathBuf,
        #[source]
        source: image::ImageError,
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

/// Build a `.sd7` for `project` from `heightmap_png` (+ a caller-supplied or
/// synthesized texture BMP) and copy it into `dst_dir`. Returns the
/// installed path.
///
/// `texture_bmp = None` → synthesize a flat grey BMP at the project's
/// texture dimensions. This is the v0 fallback so the UI can ship without
/// a texture-import flow; replace once F4 (DNTS splat painting) lands.
pub fn build_and_install(
    driver: &PyMapConvDriver,
    project: &Project,
    heightmap_png: &Path,
    texture_bmp: Option<&Path>,
    dst_dir: &Path,
) -> Result<PathBuf, LauncherError> {
    let workdir = tempfile::tempdir().map_err(|source| LauncherError::Io {
        path: PathBuf::from("<tempdir>"),
        source,
    })?;
    let work = workdir.path();

    // No caller-supplied texture? Bake one from the heightmap using
    // the same biome ramp the editor preview uses (`biome_ramp` in
    // `launcher.rs` + `terrain.wgsl`). Until D6 / Sprint 12 wires
    // splat-distribution export, this is the "what you see is what
    // you get" path — without it the texture would be flat grey and
    // BAR would render the entire map in monochrome.
    let synth_path;
    let tex = match texture_bmp {
        Some(p) => {
            info!(texture = %p.display(), "using caller-supplied texture BMP");
            p
        }
        None => {
            synth_path = work.join("synth_biome.bmp");
            let (tw, th) = project.size.texture_dims();
            // 8 SMU = 4096² → ~48 MB RGB; 16 SMU = 8192² → ~192 MB. Warn-level
            // for ≥16 SMU so the user knows what the brief stall is.
            let bytes_estimate = (tw as u64) * (th as u64) * 3;
            if bytes_estimate >= 100_000_000 {
                warn!(
                    width = tw,
                    height = th,
                    bytes_estimate,
                    "baking fallback biome texture (large; replace with painted \
                     splat distribution once D6 / Sprint 12 wires the splat export)"
                );
            } else {
                info!(
                    width = tw,
                    height = th,
                    "baking fallback biome texture from heightmap"
                );
            }
            synth_biome_bmp(heightmap_png, &synth_path, tw, th).map_err(|source| {
                LauncherError::TextureSynth {
                    path: synth_path.clone(),
                    source,
                }
            })?;
            synth_path.as_path()
        }
    };

    let out_sd7 = work.join(format!("{}.sd7", project.name));
    info!(name = %project.name, ?dst_dir, "build_and_install: compiling");
    let built = build_sd7(driver, project, heightmap_png, tex, work, &out_sd7)?;

    let installed = install_sd7(&built, dst_dir)?;
    info!(?installed, "build_and_install ok");
    Ok(installed)
}

/// Bake a colored BMP from the 16-bit heightmap PNG by sampling it
/// per texture pixel (nearest-neighbour) and applying the biome ramp.
///
/// Why a real bake (not a CPU upload of the editor's GPU texture):
/// the editor's terrain shader composites diffuse on the GPU from a
/// `splat_distribution` + bound slot diffuses + a height-keyed fallback
/// gradient. The `.sd7` needs a single baked diffuse BMP at the SMF
/// texture resolution (`512 × smu_x` per side). Until D6 / Sprint 12
/// ships the splat-distribution exporter, this height-keyed fallback
/// is the closest the SD7 can get to the editor preview without a
/// hard "your map is grey" jump.
///
/// Performance: 16 SMU = 8192² texture, 1025² heightmap, ~67M
/// per-pixel samples. Runs in ~200–500 ms in release; the
/// `>=100 MB` warn at the call site flags the cost.
fn synth_biome_bmp(
    heightmap_png: &Path,
    path: &Path,
    w: u32,
    h: u32,
) -> Result<(), image::ImageError> {
    // Load the 16-bit grayscale heightmap. `image::open(...).into_luma16()`
    // converts any input to 16-bit grayscale; a missing heightmap path
    // surfaces as the `image::ImageError` we return.
    let hm = image::open(heightmap_png)?.into_luma16();
    let hm_w = hm.width();
    let hm_h = hm.height();
    if hm_w == 0 || hm_h == 0 {
        return Err(image::ImageError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "heightmap PNG has zero dimensions",
        )));
    }
    info!(
        hm_w,
        hm_h, w, h, "baking biome texture (nearest-neighbour upsample)"
    );

    let mut buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(w, h);
    // Pre-multiply the sample-step ratio once per row dimension so the
    // inner loop stays a pair of integer multiplies.
    let denom_x = (w - 1).max(1) as u64;
    let denom_y = (h - 1).max(1) as u64;
    let hm_last_x = (hm_w - 1) as u64;
    let hm_last_y = (hm_h - 1) as u64;
    for (tx, ty, p) in buf.enumerate_pixels_mut() {
        let hx = (tx as u64 * hm_last_x / denom_x) as u32;
        let hy = (ty as u64 * hm_last_y / denom_y) as u32;
        let pixel = hm.get_pixel(hx, hy);
        // t ∈ [0, 1] maps to the same domain as terrain.wgsl's
        // `t = world_pos.y / max_height`. Since the heightmap pixel
        // is the world height linearly scaled into 0..65535, dividing
        // by 65535 yields the same normalised t.
        let t = (pixel[0] as f32) / 65535.0;
        let rgb = biome_ramp(t);
        *p = Rgb([
            (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
            (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
            (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
        ]);
    }
    buf.save(path)
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

    /// Biome ramp matches `terrain.wgsl::biome_ramp` thresholds.
    /// Locking the rules here keeps the baked SD7 texture aligned
    /// with the editor preview.
    #[test]
    fn biome_ramp_thresholds_match_wgsl() {
        // Water tier: t < 0.05.
        assert_eq!(biome_ramp(0.0), [0.227, 0.451, 0.604]);
        assert_eq!(biome_ramp(0.04), [0.227, 0.451, 0.604]);
        // Grass tier: 0.05 <= t < 0.50.
        assert_eq!(biome_ramp(0.05), [0.451, 0.616, 0.392]);
        assert_eq!(biome_ramp(0.49), [0.451, 0.616, 0.392]);
        // Rock tier: 0.50 <= t < 0.82.
        assert_eq!(biome_ramp(0.50), [0.502, 0.486, 0.439]);
        assert_eq!(biome_ramp(0.81), [0.502, 0.486, 0.439]);
        // Snow tier: t >= 0.82.
        assert_eq!(biome_ramp(0.82), [0.863, 0.878, 0.902]);
        assert_eq!(biome_ramp(1.0), [0.863, 0.878, 0.902]);
    }

    /// Synthesizing a biome BMP from a tiny gradient heightmap
    /// produces the expected tier colours at the sampled pixels.
    #[test]
    fn synth_biome_bmp_samples_heightmap_per_pixel() {
        let tmp = tempfile::tempdir().unwrap();
        // Build a 5×1 16-bit grayscale gradient: 0, 16k, 32k, 48k, 64k.
        // These land in water / grass / rock / rock / snow tiers.
        let mut hm: ImageBuffer<image::Luma<u16>, Vec<u16>> = ImageBuffer::new(5, 1);
        for (i, p) in hm.pixels_mut().enumerate() {
            // Cast through u32 to avoid u16 overflow at i=4 (4*16384=65536).
            let v = ((i as u32) * 16384).min(65535) as u16;
            *p = image::Luma([v]);
        }
        let hm_path = tmp.path().join("hm.png");
        hm.save(&hm_path).unwrap();

        let out = tmp.path().join("bake.bmp");
        synth_biome_bmp(&hm_path, &out, 5, 1).unwrap();

        let baked = image::open(&out).unwrap().to_rgb8();
        assert_eq!(baked.dimensions(), (5, 1));
        // Pixel 0: t=0 → water tier.
        assert_eq!(baked.get_pixel(0, 0)[0], (0.227 * 255.0) as u8);
        // Pixel 1: t≈0.25 → grass tier.
        assert_eq!(baked.get_pixel(1, 0)[1], (0.616 * 255.0) as u8);
        // Pixel 4: t≈1.0 → snow tier.
        assert_eq!(baked.get_pixel(4, 0)[0], (0.863 * 255.0) as u8);
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
