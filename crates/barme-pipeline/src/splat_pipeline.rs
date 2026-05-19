//! D6 (Sprint 12) — splat pipeline wiring.
//!
//! Closes the round-trip Sprint 9 left open: painted distribution in
//! the editor → painted distribution in BAR. This module sits between
//! D2's `bake_dnts` (per-slot BC3 normal compositor) and the existing
//! `.sd7` packager — it owns the build-time logic for resolving active
//! splat slots, writing the splat distribution PNG, ensuring a
//! specular fallback, and populating `mapinfo.resources`.
//!
//! ## Active channel rule
//!
//! A channel is "active" when:
//! 1. `Project.splat_config.channels[i].is_some()` — user bound a slot
//!    to channel i, AND
//! 2. `Project.splat_distribution` has any non-zero pixel in channel i
//!    (i.e. the user actually painted into it).
//!
//! Unbound or unpainted channels emit no DDS and leave their slot in
//! the resources subtable as an empty string `""` — the engine treats
//! that as "fall back to slot 0's diffuse," matching the editor's
//! "unpainted = baseline" preview behaviour from D5.
//!
//! ## Paths inside the `.sd7`
//!
//! - DDS files: `maps/textures/<slot-dir-name>_dnts.dds`. Slot-dir
//!   name is the basename of `tools/textures/<NN-slug>/`, so
//!   `00-grass-meadow` → `00-grass-meadow_dnts.dds`. Deterministic;
//!   identical Project + identical slot registry → identical
//!   filenames.
//! - Splat distribution PNG: `maps/<projectname>_splatdistr.png`,
//!   RGBA8, 1024² (the `SPLAT_DIM` constant from `barme-core::splat`).
//! - Specular fallback: `maps/<projectname>_specular.dds`, 1024² BC1
//!   grey. Generated once per host machine and cached at
//!   `tools/textures-cache/<sha>.dds` so subsequent builds copy from
//!   the cache.
//!
//! ## PyMapConv responsibility split
//!
//! PyMapConv does **NOT** touch any of these files (FINDINGS §10).
//! It produces `.smf` + `.smt` only. The splat-side artefacts above
//! are pure editor outputs that the 7-Zip stager picks up via the
//! staging walk in [`crate::build_sd7`].
//!
//! ## Pitfalls handled here
//!
//! - **PITFALL §15** — emit the SUBTABLE form of
//!   `splatDetailNormalTex` (see `mapinfo.rs::resources_block`).
//! - **PITFALL §6 / §17** — ship a grey specular fallback when the
//!   user hasn't authored one, so DNTS doesn't render flat.
//! - **PITFALL §13 adjacency** — like the all-zero metalmap PNG, the
//!   splat distribution must reflect the editor's intent
//!   byte-for-byte (no nearest-neighbour resampling).

use std::path::{Path, PathBuf};

use barme_core::{Project, SPLAT_DIM, SplatDistribution};
use image::{Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use tracing::{debug, info, trace, warn};

use crate::dnts::{BakeOptions, DntsBakeError, bake_dnts};

/// Per-channel resolved slot directory. The app's `SlotMeta` registry
/// translates `Project.splat_config.channels[i]: Option<u8>` to a
/// directory path inside `tools/textures/<NN-slug>/`. Passing `None`
/// here means "no active channel" — the pipeline skips that channel
/// entirely.
///
/// The struct is named at the channel granularity, not the slot one,
/// because two channels can bind the same slot id; the pipeline still
/// produces one DDS per channel so engine sampling reads from
/// independent texture handles.
#[derive(Debug, Clone, Default)]
pub struct SplatBakeInputs {
    pub channel_slot_dirs: [Option<PathBuf>; 4],
}

#[derive(Debug, thiserror::Error)]
pub enum SplatPipelineError {
    #[error(transparent)]
    Bake(#[from] DntsBakeError),

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("encode {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
}

/// Names of the staged artefacts produced by [`stage_splat_assets`].
/// Returned to the caller so it can register them with the 7-Zip
/// packager + populate `mapinfo.resources`.
#[derive(Debug, Clone, Default)]
pub struct StagedSplatAssets {
    /// Path on disk to the splat distribution PNG. None when no
    /// channel is active (project carries no painted distribution).
    pub splat_distr_png: Option<PathBuf>,
    /// `(disk_path, archive_relative_path)` per active DDS, in
    /// channel order (R=0, G=1, B=2, A=3). Length = number of active
    /// channels.
    pub per_slot_dds: Vec<StagedDds>,
    /// Path on disk to the specular fallback DDS (or user-supplied
    /// override). None when the build skips specular emission
    /// entirely (no active splat channels → no DNTS → no need for
    /// spec).
    pub specular_dds: Option<PathBuf>,
}

/// One per-slot DDS in the staging tree.
#[derive(Debug, Clone)]
pub struct StagedDds {
    /// Channel index 0..=3 (R/G/B/A). Identifies which subtable slot
    /// this DDS fills.
    pub channel: usize,
    /// Absolute on-disk path the packager copies from.
    pub disk_path: PathBuf,
    /// Filename portion only (e.g. `"00-grass-meadow_dnts.dds"`).
    /// The packager prepends `maps/textures/`; `mapinfo.resources.
    /// splatDetailNormalTex` references this exact basename (the
    /// engine resolves relative to the archive root).
    pub filename: String,
}

/// Resolve the active-channel mask for `project`. Returns a 4-element
/// `[bool; 4]` where index i is `true` iff channel i is bound AND has
/// any non-zero pixel in the painted distribution.
///
/// When `Project.splat_distribution` is `None` (no painted strokes
/// yet) the mask is all-false — the build skips DNTS emission and the
/// engine falls back to its grey-untextured branch. That matches the
/// editor's preview behaviour.
pub fn compute_active_channels(project: &Project) -> [bool; 4] {
    let mut mask = [false; 4];
    let Some(dist) = project.splat_distribution.as_ref() else {
        return mask;
    };
    for (ch, binding) in project.splat_config.channels.iter().enumerate() {
        if binding.is_none() {
            continue;
        }
        mask[ch] = channel_has_non_zero_pixel(dist, ch);
    }
    mask
}

fn channel_has_non_zero_pixel(dist: &SplatDistribution, channel: usize) -> bool {
    dist.rgba.iter().any(|px| px[channel] != 0)
}

/// Stage the splat-side artefacts (PNG distribution + per-active-slot
/// DDS files + specular fallback) into `work_dir`. Returns the
/// `StagedSplatAssets` the caller registers with the packager + the
/// mapinfo emitter.
///
/// `work_dir` is the build's scratch root; this function creates
/// `work_dir/splat/` and writes everything underneath.
pub fn stage_splat_assets(
    project: &Project,
    inputs: &SplatBakeInputs,
    work_dir: &Path,
    bake_opts: BakeOptions,
) -> Result<StagedSplatAssets, SplatPipelineError> {
    let mut out = StagedSplatAssets::default();
    let active = compute_active_channels(project);
    let any_active = active.iter().any(|&a| a);

    if !any_active {
        info!("splat_pipeline: no active channels — skipping DNTS bake + PNG emit");
        return Ok(out);
    }

    let splat_dir = work_dir.join("splat");
    std::fs::create_dir_all(&splat_dir).map_err(|source| SplatPipelineError::Io {
        path: splat_dir.clone(),
        source,
    })?;

    // 1. Distribution PNG.
    let png_path = splat_dir.join(format!("{}_splatdistr.png", project.name));
    write_splat_distribution_png(project, &png_path)?;
    out.splat_distr_png = Some(png_path);

    // 2. Per-active-channel DDS bakes.
    for (ch, &is_active) in active.iter().enumerate() {
        if !is_active {
            continue;
        }
        let Some(slot_dir) = inputs.channel_slot_dirs[ch].as_ref() else {
            warn!(
                channel = ch,
                "splat_pipeline: channel is active in the distribution but the app passed no slot dir; skipping DDS bake"
            );
            continue;
        };
        let slot_name = slot_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("slot")
            .to_string();
        let filename = format!("{slot_name}_dnts.dds");
        let disk_path = splat_dir.join(&filename);
        info!(
            channel = ch,
            slot = %slot_name,
            out = %disk_path.display(),
            "splat_pipeline: baking DNTS for channel"
        );
        bake_dnts(slot_dir, &disk_path, bake_opts)?;
        out.per_slot_dds.push(StagedDds {
            channel: ch,
            disk_path,
            filename,
        });
    }

    // 3. Specular fallback. PITFALL §6 / §17 + FINDINGS §7.2 — DNTS
    //    doesn't strictly require spec at the engine level, but
    //    omitting it produces visibly flat ground vs published BAR
    //    maps. Ship a stock grey BC1 when the user hasn't authored
    //    one. The compressonator path is the same dir D2 baked DNTS
    //    from; reuse it.
    let spec_path = ensure_specular_dds(project, inputs, &splat_dir)?;
    out.specular_dds = Some(spec_path);

    Ok(out)
}

/// Write `Project.splat_distribution` (RGBA8) to `out_path` as a PNG.
/// When the distribution is `None`, writes a fully-saturated-R 1024²
/// PNG so the engine paints the first slot across the entire ground
/// (matches the editor's "unpainted = baseline" preview).
fn write_splat_distribution_png(
    project: &Project,
    out_path: &Path,
) -> Result<(), SplatPipelineError> {
    let img = match project.splat_distribution.as_ref() {
        Some(d) => {
            // RGBA bytes are tile-packed [u8; 4] in row-major order.
            // image::ImageBuffer::from_raw expects a flat &[u8] in the
            // same layout — we flatten the per-pixel array.
            let mut flat = Vec::with_capacity(d.rgba.len() * 4);
            for px in &d.rgba {
                flat.extend_from_slice(px);
            }
            RgbaImage::from_raw(d.width, d.height, flat).ok_or_else(|| SplatPipelineError::Io {
                path: out_path.to_path_buf(),
                source: std::io::Error::other("RGBA buffer length mismatch with dims"),
            })?
        }
        None => {
            // Saturated R defaults to the first slot.
            let mut img = RgbaImage::new(SPLAT_DIM, SPLAT_DIM);
            for px in img.pixels_mut() {
                *px = Rgba([255, 0, 0, 0]);
            }
            img
        }
    };
    img.save(out_path)
        .map_err(|source| SplatPipelineError::Encode {
            path: out_path.to_path_buf(),
            source,
        })?;
    debug!(
        path = %out_path.display(),
        width = img.width(),
        height = img.height(),
        "splat_pipeline: wrote distribution PNG"
    );
    let _ = project; // future: include name in trace
    Ok(())
}

/// Resolve / generate the specular DDS for this build.
///
/// Priority:
/// 1. `Project.specular_tex_path` set → stage that file at
///    `<splat_dir>/<projectname>_specular.dds`.
/// 2. Otherwise → generate (or cache-hit) a 1024² grey BC1 default
///    and copy it into the staging dir.
fn ensure_specular_dds(
    project: &Project,
    inputs: &SplatBakeInputs,
    splat_dir: &Path,
) -> Result<PathBuf, SplatPipelineError> {
    let staged = splat_dir.join(format!("{}_specular.dds", project.name));
    if let Some(user) = project.specular_tex_path.as_ref() {
        std::fs::copy(user, &staged).map_err(|source| SplatPipelineError::Io {
            path: user.clone(),
            source,
        })?;
        info!(
            from = %user.display(),
            to = %staged.display(),
            "splat_pipeline: copied user-supplied specular"
        );
        return Ok(staged);
    }
    // Generate the grey BC1 fallback. The Compressonator binary lives
    // two parents up from any slot dir (D2 / ADR-027). If no slot dirs
    // were resolved (no active channels — but ensure_specular_dds is
    // only called when there IS at least one active channel) fall
    // back to the first active dir.
    let any_slot = inputs.channel_slot_dirs.iter().find_map(|d| d.as_deref());
    let Some(slot_dir) = any_slot else {
        return Err(SplatPipelineError::Io {
            path: splat_dir.to_path_buf(),
            source: std::io::Error::other(
                "ensure_specular_dds called with no resolved slot dirs (cannot find Compressonator)",
            ),
        });
    };
    let cache_dir = resolve_textures_cache_dir(slot_dir)?;
    let cache_path = cache_dir.join(default_specular_cache_filename());
    if !cache_path.is_file() {
        bake_default_specular(slot_dir, &cache_path)?;
    } else {
        info!(cache = %cache_path.display(), "splat_pipeline: specular cache hit");
    }
    std::fs::copy(&cache_path, &staged).map_err(|source| SplatPipelineError::Io {
        path: cache_path.clone(),
        source,
    })?;
    Ok(staged)
}

fn resolve_textures_cache_dir(slot_dir: &Path) -> Result<PathBuf, SplatPipelineError> {
    let textures_dir = slot_dir.parent().ok_or_else(|| SplatPipelineError::Io {
        path: slot_dir.to_path_buf(),
        source: std::io::Error::other("slot_dir has no parent"),
    })?;
    let tools_dir = textures_dir
        .parent()
        .ok_or_else(|| SplatPipelineError::Io {
            path: textures_dir.to_path_buf(),
            source: std::io::Error::other("textures dir has no parent"),
        })?;
    let cache_dir = tools_dir.join("textures-cache");
    std::fs::create_dir_all(&cache_dir).map_err(|source| SplatPipelineError::Io {
        path: cache_dir.clone(),
        source,
    })?;
    Ok(cache_dir)
}

/// Cache filename for the grey BC1 specular fallback. Derived from a
/// content sha so a future tweak to the source bytes (e.g. switching
/// from neutral grey to a slight blue tint) invalidates the cache.
fn default_specular_cache_filename() -> String {
    let mut h = Sha256::new();
    h.update(b"barme:default_specular:v1:1024:rgba128_128_128_255:bc1");
    let digest = h.finalize();
    let mut hex = String::with_capacity(64 + 4);
    for &b in digest.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    hex.push_str(".dds");
    hex
}

/// Generate a 1024² grey RGBA PNG, hand it to Compressonator with
/// `-fd BC1 -nomipmap`, and write the resulting DDS at `out_dds`.
///
/// Greys at `rgb = (128, 128, 128, 255)` so the engine's
/// `specularExp = α * 16.0` formula resolves to ~16 (FINDINGS §7.6 —
/// the BAR-published default). BC1 carries no real alpha but the
/// engine reads alpha as the multiplier; we keep the encoded RGBA
/// source at full alpha so post-DXT1 the recovered alpha is 1.0.
fn bake_default_specular(slot_dir: &Path, out_dds: &Path) -> Result<(), SplatPipelineError> {
    // Find the Compressonator binary by walking up two parents from
    // the slot dir, matching `dnts.rs`'s discovery. The cache dir
    // already exists — its parent (tools/) holds `compressonator/`.
    let textures_dir = slot_dir.parent().ok_or_else(|| SplatPipelineError::Io {
        path: slot_dir.to_path_buf(),
        source: std::io::Error::other("slot_dir has no parent"),
    })?;
    let tools_dir = textures_dir
        .parent()
        .ok_or_else(|| SplatPipelineError::Io {
            path: textures_dir.to_path_buf(),
            source: std::io::Error::other("textures dir has no parent"),
        })?;
    let compressonator_dir = tools_dir.join("compressonator");
    let bin = compressonator_dir.join("compressonatorcli-bin");
    if !bin.exists() {
        return Err(SplatPipelineError::Io {
            path: bin.clone(),
            source: std::io::Error::other(
                "compressonatorcli-bin missing — run scripts/fetch-compressonator.sh",
            ),
        });
    }

    // Write the grey 1024² PNG.
    let staging_png = out_dds.with_extension("png");
    if let Some(parent) = staging_png.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SplatPipelineError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut img = RgbaImage::new(SPLAT_DIM, SPLAT_DIM);
    for px in img.pixels_mut() {
        *px = Rgba([128, 128, 128, 255]);
    }
    img.save(&staging_png)
        .map_err(|source| SplatPipelineError::Encode {
            path: staging_png.clone(),
            source,
        })?;

    // Invoke Compressonator.
    let mut ld_entries: Vec<PathBuf> = vec![
        compressonator_dir.clone(),
        compressonator_dir.join("qt"),
        compressonator_dir.join("pkglibs"),
    ];
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        ld_entries.extend(std::env::split_paths(&existing));
    }
    let ld_path = std::env::join_paths(ld_entries).expect("vendored ld paths");

    let mut cmd = std::process::Command::new(&bin);
    cmd.env("LD_LIBRARY_PATH", &ld_path)
        .arg("-fd")
        .arg("BC1")
        .arg("-nomipmap")
        .arg(&staging_png)
        .arg(out_dds);
    debug!(?cmd, "splat_pipeline: baking default specular");
    let output = cmd.output().map_err(|source| SplatPipelineError::Io {
        path: bin.clone(),
        source,
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stdout.is_empty() {
        trace!(target: "barme_pipeline::splat_pipeline", "compressonator stdout:\n{stdout}");
    }
    if !stderr.is_empty() {
        trace!(target: "barme_pipeline::splat_pipeline", "compressonator stderr:\n{stderr}");
    }
    let present = out_dds.is_file();
    if !output.status.success() && !present {
        return Err(SplatPipelineError::Io {
            path: out_dds.to_path_buf(),
            source: std::io::Error::other(format!(
                "compressonator failed (status {:?}): {stderr}",
                output.status.code()
            )),
        });
    }
    if !present {
        return Err(SplatPipelineError::Io {
            path: out_dds.to_path_buf(),
            source: std::io::Error::other("compressonator exited 0 but produced no DDS"),
        });
    }
    // Best-effort cleanup of the staging PNG.
    let _ = std::fs::remove_file(&staging_png);
    info!(out = %out_dds.display(), "splat_pipeline: default specular cached");
    Ok(())
}

/// Populate the resources block of an already-built `MapInfo` from
/// the staging output. Mutates `info.resources` in-place; the caller
/// passes the same `MapInfo` it'll hand to the Lua emitter.
///
/// `archive_paths_relative` controls whether the emitted paths are
/// archive-relative (default — engine resolves from archive root) or
/// just the basename. The engine accepts both; archive-relative paths
/// (`maps/textures/foo_dnts.dds`) match what published BAR maps use.
pub fn populate_resources(
    info: &mut barme_core::MapInfo,
    project: &Project,
    staged: &StagedSplatAssets,
) {
    // splatDistrTex
    if let Some(p) = staged.splat_distr_png.as_ref() {
        let filename = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("splatdistr.png");
        info.resources.splat_distr_tex = Some(format!("maps/{filename}"));
    }

    // splatDetailNormalTex subtable — four entries, channel order
    // (R/G/B/A). Inactive channels get an empty string so the
    // subtable is always exactly four entries (the engine reads via
    // `ipairs` and stops at the first nil; an empty string still
    // counts as a present entry, just one that resolves to no
    // texture).
    if !staged.per_slot_dds.is_empty() {
        let mut entries: [String; 4] = Default::default();
        for dds in &staged.per_slot_dds {
            entries[dds.channel] = format!("maps/textures/{}", dds.filename);
        }
        info.resources.splat_detail_normal_tex = entries.to_vec();
        // ADR-025 baseline: subtable alpha = false. ADR-034 (the
        // high-pass diffuse-in-alpha workflow) toggles this when it
        // lands.
        info.resources.splat_detail_normal_tex_alpha = Some(project.splat_config.diffuse_in_alpha);
    }

    // specularTex
    if let Some(p) = staged.specular_dds.as_ref() {
        let filename = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("specular.dds");
        info.resources.specular_tex = Some(format!("maps/{filename}"));
    }

    // splats.texScales / texMults mirror the project's config.
    info.splats.tex_scales = project.splat_config.tex_scales;
    info.splats.tex_mults = project.splat_config.tex_mults;
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_core::MapSize;

    fn project_with_painted_channels(painted: &[usize]) -> Project {
        let mut p = Project::new("paint", 4);
        // Bind every channel the test wants painted to a stock slot
        // id. The slot id is opaque here — `compute_active_channels`
        // only checks for `is_some()`.
        for &ch in painted {
            p.splat_config.channels[ch] = Some(ch as u8);
        }
        // Allocate a distribution and paint one pixel per requested
        // channel so the non-zero check passes.
        let mut dist = SplatDistribution::new(MapSize::square(4));
        for &ch in painted {
            dist.rgba[0][ch] = 255;
        }
        p.splat_distribution = Some(dist);
        p
    }

    /// D6: a channel is active iff bound AND has non-zero pixel data.
    #[test]
    fn active_slots_from_distribution() {
        // Paint R + G, leave B + A clean.
        let p = project_with_painted_channels(&[0, 1]);
        let active = compute_active_channels(&p);
        assert_eq!(active, [true, true, false, false]);

        // A bound-but-unpainted channel is NOT active.
        let mut p = Project::new("bound-unpainted", 4);
        p.splat_config.channels[2] = Some(7);
        // No distribution allocated → mask all-false.
        let active = compute_active_channels(&p);
        assert_eq!(active, [false; 4]);

        // Distribution allocated but channel 2 never painted → still inactive.
        p.splat_distribution = Some(SplatDistribution::new(MapSize::square(4)));
        assert_eq!(compute_active_channels(&p), [false; 4]);
    }

    /// D6: a project with no painted channels skips the DDS bake +
    /// PNG emit (no staged artefacts). The 7-Zip stager's per-staged
    /// loop becomes a no-op for splat.
    #[test]
    fn no_active_channels_skips_staging() {
        let p = Project::new("clean", 4);
        let tmp = tempfile::tempdir().unwrap();
        let staged = stage_splat_assets(
            &p,
            &SplatBakeInputs::default(),
            tmp.path(),
            BakeOptions::default(),
        )
        .expect("stage with no active channels should succeed quietly");
        assert!(staged.splat_distr_png.is_none());
        assert!(staged.per_slot_dds.is_empty());
        assert!(staged.specular_dds.is_none());
    }

    /// D6: when channels are active but no slot dirs were resolved
    /// the pipeline emits a `warn!` per channel + skips that DDS.
    /// The distribution PNG still ships (the engine needs SOMETHING
    /// to sample); the specular fallback hits the "no slot dirs"
    /// error guard.
    #[test]
    fn active_channels_without_slot_dirs_emits_distribution_only() {
        let p = project_with_painted_channels(&[0]);
        let tmp = tempfile::tempdir().unwrap();
        // Empty inputs — no slot dirs.
        let result = stage_splat_assets(
            &p,
            &SplatBakeInputs::default(),
            tmp.path(),
            BakeOptions::default(),
        );
        // ensure_specular_dds fails when no slot dirs are present —
        // that's the no-Compressonator-discoverable path. The error is
        // expected here.
        assert!(
            result.is_err(),
            "expected ensure_specular_dds to fail without resolved slot dirs"
        );
    }

    /// D6: `populate_resources` writes the subtable form into the
    /// MapInfo's resources block. The mapinfo emitter then renders
    /// the subtable per PITFALL §15.
    #[test]
    fn populate_resources_writes_subtable_form() {
        use barme_core::MapInfo;
        let p = project_with_painted_channels(&[0, 1]);
        let mut info = MapInfo::bar_default();
        let staged = StagedSplatAssets {
            splat_distr_png: Some(PathBuf::from("/tmp/paint_splatdistr.png")),
            per_slot_dds: vec![
                StagedDds {
                    channel: 0,
                    disk_path: PathBuf::from("/tmp/grass_dnts.dds"),
                    filename: "grass_dnts.dds".to_string(),
                },
                StagedDds {
                    channel: 1,
                    disk_path: PathBuf::from("/tmp/rock_dnts.dds"),
                    filename: "rock_dnts.dds".to_string(),
                },
            ],
            specular_dds: Some(PathBuf::from("/tmp/paint_specular.dds")),
        };
        populate_resources(&mut info, &p, &staged);

        assert_eq!(
            info.resources.splat_distr_tex.as_deref(),
            Some("maps/paint_splatdistr.png")
        );
        assert_eq!(
            info.resources.splat_detail_normal_tex,
            vec![
                "maps/textures/grass_dnts.dds".to_string(),
                "maps/textures/rock_dnts.dds".to_string(),
                String::new(),
                String::new(),
            ]
        );
        assert_eq!(info.resources.splat_detail_normal_tex_alpha, Some(false));
        assert_eq!(
            info.resources.specular_tex.as_deref(),
            Some("maps/paint_specular.dds")
        );
        // splats arrays mirror the project's config (defaults at
        // 0.02 / 1.0).
        assert_eq!(info.splats.tex_scales, [0.02; 4]);
        assert_eq!(info.splats.tex_mults, [1.0; 4]);
    }

    /// D6: the cache filename for the default specular is stable
    /// across calls (content-addressed) and at the canonical 64-hex
    /// + `.dds` length.
    #[test]
    fn default_specular_cache_filename_is_stable_hex_sha() {
        let a = default_specular_cache_filename();
        let b = default_specular_cache_filename();
        assert_eq!(a, b, "filename must be deterministic");
        assert_eq!(a.len(), 64 + 4, "expect 64 hex chars + '.dds'");
        assert!(a.ends_with(".dds"));
    }

    /// D6: the splat distribution PNG dimensions are exactly
    /// `SPLAT_DIM × SPLAT_DIM` (PITFALL #4 cousin — the engine reads
    /// any dimension, but the editor's preview pins 1024² so
    /// shipping anything else is a silent surprise).
    #[test]
    fn splat_distribution_png_dimensions_pinned_at_splat_dim() {
        let p = project_with_painted_channels(&[2]);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("splat.png");
        write_splat_distribution_png(&p, &path).unwrap();
        let img = image::open(&path).unwrap().to_rgba8();
        assert_eq!(img.width(), SPLAT_DIM);
        assert_eq!(img.height(), SPLAT_DIM);
        // The painted B-channel pixel (0,0) survives the PNG round trip.
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 255, 0]);
    }

    /// D6: when there's no painted distribution the PNG defaults to
    /// saturated R so the engine paints slot 0 across everything —
    /// matches the editor's "unpainted = baseline" preview.
    #[test]
    fn splat_distribution_png_defaults_to_saturated_r() {
        let p = Project::new("nopaint", 4);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("default.png");
        write_splat_distribution_png(&p, &path).unwrap();
        let img = image::open(&path).unwrap().to_rgba8();
        assert_eq!(img.width(), SPLAT_DIM);
        // Every pixel = (255, 0, 0, 0).
        for px in img.pixels() {
            assert_eq!(px.0, [255, 0, 0, 0]);
        }
    }
}
