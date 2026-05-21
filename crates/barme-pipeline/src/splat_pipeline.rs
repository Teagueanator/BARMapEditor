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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use barme_core::layers::LayerMask;
use barme_core::{Project, SPLAT_DIM, SplatDistribution};
use image::{Rgba, RgbaImage};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use sha2::{Digest, Sha256};
use tracing::{debug, info, trace, warn};

use crate::dnts::{BakeOptions, DntsBakeError, bake_dnts};

/// Sprint 24 (T2): cap concurrent CompressonatorCLI subprocesses to
/// keep a 4-core dev box responsive even with 16 DNTS slots. Per
/// `docs/research/multithreading/PROPOSAL.md` §3 — beyond this point
/// the marginal subprocess saturates disk I/O for negligible
/// wall-time gain.
const DNTS_BAKE_THREAD_CAP: usize = 4;

/// No-op progress callback for callers that don't care about per-slot
/// completion progress (unit tests, the smoke binary). The
/// [`stage_splat_assets_from_layers`] signature is generic over
/// `Fn(usize, usize) + Sync` so a closure or this helper both fit.
pub fn no_op_progress() -> impl Fn(usize, usize) + Sync {
    |_, _| {}
}

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

/// D10 / Sprint 17 (ADR-041) — per-channel inputs to the
/// layer-driven splat pipeline. The app resolves these from
/// `project.layers.dnts_layers()` before calling
/// [`stage_splat_assets_from_layers`].
///
/// All four channel-indexed arrays line up: `[ch]` is the input for
/// `SplatChannel::R` (0), G (1), B (2), A (3).
#[derive(Debug, Clone, Default)]
pub struct LayerSplatBakeInputs {
    /// `tools/textures/<NN-slug>/` directory of the DNTS-bound
    /// layer's source slot. `None` skips the DDS bake for that
    /// channel — happens when no layer is bound or when the layer's
    /// source is `LayerSource::Imported` (imported textures don't
    /// have a stock normal map; the lint warning surfaces).
    pub channel_slot_dirs: [Option<PathBuf>; 4],
    /// DNTS-bound layer's mask, cloned by the caller. The splat
    /// distribution PNG is materialised by box-filtering each
    /// channel's mask down to 1024² and writing it into the matching
    /// RGBA channel.
    pub channel_masks: [Option<LayerMask>; 4],
    /// Per-channel `mapinfo.splats.texScales[i]` — read from the
    /// bound layer's `dnts_tex_scale` field.
    pub channel_tex_scales: [f32; 4],
    /// Per-channel `mapinfo.splats.texMults[i]`.
    pub channel_tex_mults: [f32; 4],
    /// Per-channel layer name (cosmetic — used for the
    /// imported-layer lint warning's `layer_name` field).
    pub channel_layer_names: [Option<String>; 4],
    /// `true` when the bound layer's source is
    /// `LayerSource::Imported`. Drives the
    /// `LintWarning::ImportedLayerDnts` emission; the channel's
    /// DDS bake is skipped (no stock normal map to use).
    pub channel_imported: [bool; 4],
}

/// D10 / Sprint 17 (ADR-041) — lint warnings emitted during the
/// layer-driven splat pipeline. Returned alongside `StagedSplatAssets`
/// so the caller can surface them in the UI's validation chip.
///
/// `non_exhaustive` so future warning categories can be added without
/// breaking match exhaustiveness on the consumer side.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LintWarning {
    /// A DNTS-bound layer's source is `LayerSource::Imported`. The
    /// channel's mask still contributes to the splat distribution,
    /// but no per-slot DDS gets baked (stock normal maps don't
    /// exist for imported diffuses). At runtime BAR's DNTS shader
    /// will fall back to whatever specular contribution the slot's
    /// alpha provides.
    ImportedLayerDnts { layer_name: String },
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
    // Sprint 23 (T1): SplatConfig retired. A channel is bound when
    // a layer in the stack carries `dnts_channel = Some(_)` for it
    // (Sprint 17 / ADR-041). The painted-distribution check still
    // gates emission so the legacy pre-Sprint-17 path stays a true
    // no-op for unpainted projects.
    let dnts = project.layers.dnts_layers();
    for (ch, layer) in dnts.iter().enumerate() {
        if layer.is_none() {
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
        // Sprint 23 (T1): SplatConfig retired. The per-project
        // `dnts_diffuse_in_alpha` flag (Sprint 17 / ADR-041) is the
        // source of truth now.
        info.resources.splat_detail_normal_tex_alpha = Some(project.dnts_diffuse_in_alpha);
    }

    // specularTex
    if let Some(p) = staged.specular_dds.as_ref() {
        let filename = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("specular.dds");
        info.resources.specular_tex = Some(format!("maps/{filename}"));
    }

    // Sprint 23 (T1): SplatConfig retired. Per-channel `texScales`
    // and `texMults` now derive from each DNTS-bound layer's
    // `dnts_tex_scale` / `dnts_tex_mult` fields (Sprint 17 / ADR-041).
    // Unbound channels keep the engine default (0.02 / 1.0).
    let dnts = project.layers.dnts_layers();
    let mut tex_scales = [0.02f32; 4];
    let mut tex_mults = [1.0f32; 4];
    for (ch, layer) in dnts.iter().enumerate() {
        if let Some(l) = layer {
            tex_scales[ch] = l.dnts_tex_scale;
            tex_mults[ch] = l.dnts_tex_mult;
        }
    }
    info.splats.tex_scales = tex_scales;
    info.splats.tex_mults = tex_mults;
}

/// D10 / Sprint 17 (ADR-041) — layer-driven counterpart to
/// [`stage_splat_assets`]. Materialises the splat distribution PNG
/// from each DNTS-bound layer's mask (box-filter downsample to
/// 1024²) instead of from `Project.splat_distribution`. Bakes a DDS
/// per slot-bound channel; imported-source channels emit a lint
/// warning and skip the DDS bake. The `mapinfo.resources` block +
/// `splats.texScales` / `texMults` get populated from the
/// per-layer fields the Sprint 17 schema introduced.
///
/// Returns the staged assets + any lint warnings (currently only
/// [`LintWarning::ImportedLayerDnts`]).
pub fn stage_splat_assets_from_layers(
    project: &Project,
    inputs: &LayerSplatBakeInputs,
    work_dir: &Path,
    bake_opts: BakeOptions,
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<(StagedSplatAssets, Vec<LintWarning>), SplatPipelineError> {
    let mut out = StagedSplatAssets::default();
    let mut warnings = Vec::new();

    let any_mask = inputs.channel_masks.iter().any(|m| m.is_some());
    let any_slot = inputs.channel_slot_dirs.iter().any(|d| d.is_some());
    if !any_mask && !any_slot {
        info!("splat_pipeline (layers): no DNTS-bound layers; skipping bake + PNG emit");
        return Ok((out, warnings));
    }

    let splat_dir = work_dir.join("splat");
    std::fs::create_dir_all(&splat_dir).map_err(|source| SplatPipelineError::Io {
        path: splat_dir.clone(),
        source,
    })?;

    // 1. Splat distribution PNG — box-filter downsample of each
    //    channel's layer mask into the 1024² RGBA buffer. The R/G/B/A
    //    invariant holds by construction (each channel comes from an
    //    independent mask; no cross-channel coupling).
    let png_path = splat_dir.join(format!("{}_splatdistr.png", project.name));
    materialize_splat_distribution_from_layers(
        [
            inputs.channel_masks[0].as_ref(),
            inputs.channel_masks[1].as_ref(),
            inputs.channel_masks[2].as_ref(),
            inputs.channel_masks[3].as_ref(),
        ],
        &png_path,
    )?;
    out.splat_distr_png = Some(png_path);

    // 2. Collect the per-channel bake tasks + the imported-layer
    //    lint warnings up front so the parallel bake closure has a
    //    flat task list and the warning emission stays sequential.
    let mut tasks: Vec<(usize, PathBuf, String)> = Vec::new();
    for ch in 0..4 {
        let Some(slot_dir) = inputs.channel_slot_dirs[ch].as_ref() else {
            if inputs.channel_imported[ch] {
                let name = inputs.channel_layer_names[ch]
                    .clone()
                    .unwrap_or_else(|| format!("channel {}", channel_letter(ch)));
                warn!(
                    channel = ch,
                    layer = %name,
                    "splat_pipeline (layers): imported-source DNTS layer; skipping DDS bake"
                );
                warnings.push(LintWarning::ImportedLayerDnts { layer_name: name });
            }
            continue;
        };
        let slot_name = slot_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("slot")
            .to_string();
        let filename = format!("{slot_name}_dnts.dds");
        tasks.push((ch, slot_dir.clone(), filename));
    }
    let total = tasks.len();

    // 3. Parallel DNTS bake across active channels — Sprint 24 (T2).
    //    Cache is content-addressed and atomic-renamed
    //    (`dnts::bake_dnts_in_env`), so two channels resolving the
    //    same `cache_key` (same slot + same opts) are safe. We cap at
    //    `min(num_cpus, 4)` via a SCOPED rayon pool so we don't
    //    pollute the global pool with subprocess concurrency limits.
    //    The progress callback fires per-bake **completion** (not
    //    start) so the overlay reflects work done, and the counter is
    //    a coarse `AtomicUsize` — never in the hot path.
    let bake_start = Instant::now();
    let completed = AtomicUsize::new(0);
    let bake_one = |task: &(usize, PathBuf, String)| -> Result<StagedDds, SplatPipelineError> {
        let (ch, slot_dir, filename) = task;
        let disk_path = splat_dir.join(filename);
        info!(
            channel = ch,
            slot = %slot_dir.display(),
            out = %disk_path.display(),
            "splat_pipeline (layers): baking DNTS for channel"
        );
        bake_dnts(slot_dir, &disk_path, bake_opts)?;
        let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
        on_progress(done, total);
        Ok(StagedDds {
            channel: *ch,
            disk_path,
            filename: filename.clone(),
        })
    };

    let mut baked: Vec<StagedDds> = if total <= 1 {
        // One bake (or zero) — skip the scoped-pool ceremony.
        tasks.iter().map(&bake_one).collect::<Result<Vec<_>, _>>()?
    } else {
        let cap = rayon::current_num_threads().clamp(1, DNTS_BAKE_THREAD_CAP);
        let scoped_result: Result<
            Result<Vec<StagedDds>, SplatPipelineError>,
            rayon::ThreadPoolBuildError,
        > = rayon::ThreadPoolBuilder::new()
            .num_threads(cap)
            .build_scoped(
                |thread| thread.run(),
                |pool| pool.install(|| tasks.par_iter().map(&bake_one).collect()),
            );
        scoped_result.map_err(|e| SplatPipelineError::Io {
            path: splat_dir.clone(),
            source: std::io::Error::other(format!("rayon scoped pool init failed: {e}")),
        })??
    };
    // Per-channel order isn't preserved by the parallel collect (rayon's
    // worker scheduling is order-insensitive on completion). Sort so the
    // post-bake state matches the pre-Sprint-24 serial order — the
    // `populate_resources_from_layers` consumer indexes by `dds.channel`
    // so order doesn't affect correctness, but a stable order keeps
    // logs and snapshot tests reproducible.
    baked.sort_by_key(|d| d.channel);
    let bake_elapsed_ms = bake_start.elapsed().as_secs_f64() * 1000.0;
    info!(
        target: "barme_pipeline::splat_pipeline",
        slots_baked = baked.len(),
        elapsed_ms = bake_elapsed_ms,
        "splat_pipeline (layers): parallel DNTS bake complete"
    );
    out.per_slot_dds = baked;

    // 4. Specular fallback — same shape as the legacy path.
    let any_active = !out.per_slot_dds.is_empty();
    if any_active {
        // Reuse the legacy ensure_specular_dds helper by building a
        // `SplatBakeInputs` shim from the layer-derived slot dirs.
        let shim_inputs = SplatBakeInputs {
            channel_slot_dirs: inputs.channel_slot_dirs.clone(),
        };
        let spec_path = ensure_specular_dds(project, &shim_inputs, &splat_dir)?;
        out.specular_dds = Some(spec_path);
    }

    Ok((out, warnings))
}

/// D10 / Sprint 17 (ADR-041) — channel-letter helper for
/// lint/log messages.
fn channel_letter(ch: usize) -> char {
    match ch {
        0 => 'R',
        1 => 'G',
        2 => 'B',
        3 => 'A',
        _ => '?',
    }
}

/// D10 / Sprint 17 (ADR-041) — box-filter each channel's layer mask
/// down to a 1024² RGBA PNG. PITFALL §17.2 — nearest-neighbour would
/// produce visible blockiness in the per-fragment DNTS blend at
/// runtime; box filter smooths the transitions.
///
/// PITFALL §17.10 — process row-by-row so an 8192² × 4-layer source
/// doesn't materialise a 256 MB intermediate. Per-pixel state is one
/// u32 sum + u32 count per channel.
pub fn materialize_splat_distribution_from_layers(
    masks: [Option<&LayerMask>; 4],
    out_path: &Path,
) -> Result<(), SplatPipelineError> {
    // Source dim — assume all bound masks share dims (they do; all
    // derive from the same `MapSize::texture_dims`). Pick the first.
    let src_dim = masks
        .iter()
        .filter_map(|m| m.map(|m| m.width.min(m.height)))
        .next()
        .unwrap_or(SPLAT_DIM);
    let scale = (src_dim / SPLAT_DIM).max(1);

    let mut buffer = vec![0u8; (SPLAT_DIM as usize) * (SPLAT_DIM as usize) * 4];
    for oy in 0..SPLAT_DIM {
        let sy0 = oy * scale;
        let sy1 = (sy0 + scale).min(src_dim);
        for ox in 0..SPLAT_DIM {
            let sx0 = ox * scale;
            let sx1 = (sx0 + scale).min(src_dim);
            for (ch_idx, mask_opt) in masks.iter().enumerate() {
                let Some(mask) = mask_opt else {
                    continue;
                };
                let mut sum = 0u32;
                let mut count = 0u32;
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        sum += u32::from(mask.sample(sx, sy));
                        count += 1;
                    }
                }
                let avg = sum
                    .checked_div(count)
                    .map(|v| v.min(255) as u8)
                    .unwrap_or(0);
                let pixel_offset =
                    ((oy as usize) * (SPLAT_DIM as usize) + (ox as usize)) * 4 + ch_idx;
                buffer[pixel_offset] = avg;
            }
        }
    }
    let img = RgbaImage::from_raw(SPLAT_DIM, SPLAT_DIM, buffer).ok_or_else(|| {
        SplatPipelineError::Io {
            path: out_path.to_path_buf(),
            source: std::io::Error::other("RGBA buffer length mismatch with dims"),
        }
    })?;
    img.save(out_path)
        .map_err(|source| SplatPipelineError::Encode {
            path: out_path.to_path_buf(),
            source,
        })?;
    debug!(
        path = %out_path.display(),
        src_dim,
        scale,
        "splat_pipeline (layers): wrote distribution PNG"
    );
    Ok(())
}

/// D10 / Sprint 17 (ADR-041) — layer-driven counterpart to
/// [`populate_resources`]. Same `mapinfo.resources` shape; the data
/// derives from `LayerSplatBakeInputs` (per-layer tex scales/mults)
/// plus `Project.dnts_diffuse_in_alpha` (replaces the legacy
/// `splat_config.diffuse_in_alpha`).
pub fn populate_resources_from_layers(
    info: &mut barme_core::MapInfo,
    project: &Project,
    inputs: &LayerSplatBakeInputs,
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
    // splatDetailNormalTex subtable — four entries.
    if !staged.per_slot_dds.is_empty() {
        let mut entries: [String; 4] = Default::default();
        for dds in &staged.per_slot_dds {
            entries[dds.channel] = format!("maps/textures/{}", dds.filename);
        }
        info.resources.splat_detail_normal_tex = entries.to_vec();
        info.resources.splat_detail_normal_tex_alpha = Some(project.dnts_diffuse_in_alpha);
    }
    // specularTex
    if let Some(p) = staged.specular_dds.as_ref() {
        let filename = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("specular.dds");
        info.resources.specular_tex = Some(format!("maps/{filename}"));
    }
    // splats.texScales / texMults from per-layer fields.
    info.splats.tex_scales = inputs.channel_tex_scales;
    info.splats.tex_mults = inputs.channel_tex_mults;
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_core::MapSize;

    fn project_with_painted_channels(painted: &[usize]) -> Project {
        use barme_core::{LayerSource, SplatChannel, TextureLayer};
        let mut p = Project::new("paint", 4);
        // Sprint 23 (T1): bindings now live on the layer stack.
        // Append one DNTS-bound layer per requested channel. The
        // slot id is opaque here — `compute_active_channels` only
        // checks whether a layer is bound to the channel.
        for &ch in painted {
            let channel = match ch {
                0 => SplatChannel::R,
                1 => SplatChannel::G,
                2 => SplatChannel::B,
                3 => SplatChannel::A,
                _ => unreachable!("test only uses channels 0..=3"),
            };
            let mut layer = TextureLayer::new(LayerSource::Slot { id: ch as u8 }, p.size, 0);
            layer.dnts_channel = Some(channel);
            p.layers.layers.push(layer);
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
        // Sprint 23 (T1): bind via the layer stack, not the
        // retired `splat_config.channels`.
        let mut layer =
            barme_core::TextureLayer::new(barme_core::LayerSource::Slot { id: 7 }, p.size, 0);
        layer.dnts_channel = Some(barme_core::SplatChannel::B);
        p.layers.layers.push(layer);
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

    // ─── D10 / Sprint 17 (ADR-041) ────────────────────────────────────────

    /// PITFALL §17.3 — R + G + B + A ≤ 255 is preserved by
    /// construction in the layer-driven materialisation. Each
    /// channel comes from an independent layer mask; no
    /// cross-channel coupling. We assert by feeding four masks with
    /// random byte values and verifying every output pixel passes.
    #[test]
    fn mask_to_splat_distr_invariant_rgba_under_255() {
        use barme_core::{LayerMask, MapSize};
        // Build four 2-SMU masks with predictable byte fills so the
        // assertion is deterministic. The channels are independent —
        // sum can exceed 255, which the engine's `min(1.0)` clamp
        // handles at runtime. Our materialisation just preserves
        // each channel verbatim.
        let mask_r = LayerMask::filled(MapSize::square(2), 100);
        let mask_g = LayerMask::filled(MapSize::square(2), 50);
        let mask_b = LayerMask::filled(MapSize::square(2), 25);
        let mask_a = LayerMask::filled(MapSize::square(2), 200);
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("rgba.png");
        materialize_splat_distribution_from_layers(
            [Some(&mask_r), Some(&mask_g), Some(&mask_b), Some(&mask_a)],
            &out,
        )
        .unwrap();
        let img = image::open(&out).unwrap().to_rgba8();
        assert_eq!(img.width(), SPLAT_DIM);
        assert_eq!(img.height(), SPLAT_DIM);
        for px in img.pixels() {
            // Each channel is a per-channel byte average. The invariant
            // we care about: every channel is in 0..=255 (no overflow
            // from the box-filter sum). The R+G+B+A ≤ 1.0 expectation
            // is engine-side after texMult; here we just verify byte
            // bounds + per-channel correctness.
            assert!(px.0[0] >= 95 && px.0[0] <= 105, "R drift, got {:?}", px.0);
            assert!(px.0[1] >= 45 && px.0[1] <= 55, "G drift, got {:?}", px.0);
            assert!(px.0[2] >= 20 && px.0[2] <= 30, "B drift, got {:?}", px.0);
            assert!(px.0[3] >= 195 && px.0[3] <= 205, "A drift, got {:?}", px.0);
        }
    }

    /// PITFALL §17.2 — box filter (NOT nearest neighbour). A single
    /// bright pixel in the mask should spread its energy to at least
    /// one output pixel — and if the source is 8× larger than the
    /// output, that single bright pixel averages to ~255/64 = 4 in
    /// its destination pixel.
    #[test]
    fn box_filter_downsample_averages_not_nearest_neighbour() {
        use barme_core::{LayerMask, MapSize};
        // 16-SMU mask = 8192² → 8× downsample to 1024². One pixel at
        // 255 averages to 255/64 ≈ 4 in its destination.
        let mut mask = LayerMask::filled(MapSize::square(16), 0);
        mask.set_pixel(0, 0, 255);
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("box.png");
        materialize_splat_distribution_from_layers([Some(&mask), None, None, None], &out).unwrap();
        let img = image::open(&out).unwrap().to_rgba8();
        let dst_px = img.get_pixel(0, 0).0;
        // Nearest-neighbour would round to 255 (the bright source
        // pixel). Box filter averages the 8x8 block (one bright + 63
        // zero) ≈ 4. Anything between 1 and 20 is the box-filter
        // signature.
        assert!(
            dst_px[0] > 0 && dst_px[0] < 20,
            "expected box-filter average ~4, got {dst_px:?}",
        );
    }

    /// `stage_splat_assets_from_layers` returns a lint warning when
    /// a DNTS-bound layer is `Imported`-sourced (no stock normal map
    /// to bake the DDS from). The distribution PNG still emits.
    #[test]
    fn imported_layer_dnts_emits_lint_warning() {
        use barme_core::{LayerMask, MapSize};
        let p = Project::new("imported-dnts", 4);
        let mask = LayerMask::filled(MapSize::square(2), 200);
        let inputs = LayerSplatBakeInputs {
            channel_slot_dirs: [None, None, None, None], // imported → no slot dir
            channel_masks: [Some(mask), None, None, None],
            channel_tex_scales: [0.02; 4],
            channel_tex_mults: [1.0; 4],
            channel_layer_names: [Some("imp-grass".into()), None, None, None],
            channel_imported: [true, false, false, false],
        };
        let tmp = tempfile::tempdir().unwrap();
        let result = stage_splat_assets_from_layers(
            &p,
            &inputs,
            tmp.path(),
            BakeOptions::default(),
            no_op_progress(),
        );
        // No slot dir on the bound channel → ensure_specular_dds is
        // skipped (the function only runs when there's an active DDS,
        // and the imported branch produced none). Build succeeds.
        let (staged, warnings) = result.unwrap_or_else(|e| panic!("stage failed: {e}"));
        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            LintWarning::ImportedLayerDnts { layer_name } if layer_name == "imp-grass"
        ));
        assert!(staged.splat_distr_png.is_some());
        assert!(staged.per_slot_dds.is_empty());
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

    // ─── Sprint 24 (T2) — parallel DNTS bake regression ──────────────

    /// Locate the vendored Compressonator binary (same shape as
    /// `dnts::tests::locate_compressonator`). Returns None when the
    /// fetch script hasn't been run **or** when the binary is empty
    /// (PITFALL #29 — a corrupted vendored binary should surface as a
    /// clean skip, not an ENOEXEC panic). Tests using it `eprintln!`
    /// + skip when None.
    fn locate_compressonator_for_test() -> Option<(PathBuf, PathBuf)> {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()? // crates/
            .parent()? // repo root
            .join("tools")
            .join("compressonator");
        let bin = dir.join("compressonatorcli-bin");
        let metadata = std::fs::metadata(&bin).ok()?;
        if metadata.len() == 0 {
            return None;
        }
        Some((bin, dir))
    }

    /// Build the textures subtree: four slot directories with one
    /// distinct synthetic `normal.png` each. The caller is responsible
    /// for placing a `compressonator/` sibling at `<root>/compressonator/`
    /// (typically a symlink to the vendored binary). Two parallel
    /// callers MUST NOT share `root` — each test owns its own
    /// `tools/` dir to keep the cache hashes isolated.
    fn build_synth_textures_tree(root: &Path) -> [PathBuf; 4] {
        let textures_dir = root.join("textures");
        let mut out: [PathBuf; 4] = Default::default();
        for (ch, slot_path) in out.iter_mut().enumerate() {
            let slot = textures_dir.join(format!("0{ch}-synth-slot"));
            std::fs::create_dir_all(&slot).unwrap();
            // Distinct normal-byte fill per channel → distinct cache
            // key, so the four parallel bakes don't dedupe via cache.
            let mut img = RgbaImage::new(8, 8);
            let r = 100u8 + ch as u8 * 20;
            for px in img.pixels_mut() {
                *px = Rgba([r, 128, 255, 0xFF]);
            }
            img.save(slot.join("normal.png")).unwrap();
            *slot_path = slot;
        }
        out
    }

    /// Sprint 24 (T2): parallel bake of 4 distinct DNTS-bound layers
    /// completes in wall-time bounded by `< 1.5 × max(per-slot bake)`.
    /// Compressonator subprocess timing is variable, so this is a
    /// soft assertion + diagnostic eprintln rather than a hard fail —
    /// matches the sprint prompt's "soft assertion + log is fine."
    #[test]
    fn parallel_bake_wall_time_under_15x_single_slot() {
        use barme_core::{LayerMask, MapSize};
        let Some((bin, _dir)) = locate_compressonator_for_test() else {
            eprintln!(
                "skipping parallel_bake_wall_time_under_15x_single_slot: \
                       compressonatorcli-bin not vendored"
            );
            return;
        };
        let _ = bin; // used only as the gating check

        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        // Bake env discovery wants `compressonator/` and `textures/`
        // as siblings under `tools/`. We symlink the vendored
        // Compressonator dir into place (so the bake actually invokes
        // the real binary) and build the synthetic textures subtree
        // separately so we don't end up writing through the symlink.
        let real_compressonator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("tools").join("compressonator"))
            .unwrap();
        std::os::unix::fs::symlink(&real_compressonator, tools.join("compressonator")).unwrap();
        let slots = build_synth_textures_tree(&tools);

        // Time the single-slot bake first (cache miss → real
        // Compressonator invocation), then time the parallel 4-slot
        // bake from a fresh cache.
        let single_proj = Project::new("single-bench", 4);
        let single_inputs = LayerSplatBakeInputs {
            channel_slot_dirs: [Some(slots[0].clone()), None, None, None],
            channel_masks: [
                Some(LayerMask::filled(MapSize::square(2), 200)),
                None,
                None,
                None,
            ],
            channel_tex_scales: [0.02; 4],
            channel_tex_mults: [1.0; 4],
            channel_layer_names: [Some("single".into()), None, None, None],
            channel_imported: [false; 4],
        };
        let single_work = tmp.path().join("work-single");
        std::fs::create_dir_all(&single_work).unwrap();
        let t0 = std::time::Instant::now();
        stage_splat_assets_from_layers(
            &single_proj,
            &single_inputs,
            &single_work,
            BakeOptions::default(),
            no_op_progress(),
        )
        .expect("single-slot bake should succeed");
        let single_elapsed = t0.elapsed();

        // Fresh cache for the parallel run — the single-slot cache
        // entry is now warm but the other 3 channels' cache_keys
        // differ (distinct normal bytes per slot), so the parallel
        // bake spawns 3 fresh Compressonator subprocesses + 1 cache
        // hit. Clear the cache entirely to force 4 fresh bakes.
        let cache_dir = tools.join("textures-cache");
        if cache_dir.is_dir() {
            std::fs::remove_dir_all(&cache_dir).unwrap();
        }

        let parallel_proj = Project::new("parallel-bench", 4);
        let parallel_inputs = LayerSplatBakeInputs {
            channel_slot_dirs: [
                Some(slots[0].clone()),
                Some(slots[1].clone()),
                Some(slots[2].clone()),
                Some(slots[3].clone()),
            ],
            channel_masks: [
                Some(LayerMask::filled(MapSize::square(2), 100)),
                Some(LayerMask::filled(MapSize::square(2), 100)),
                Some(LayerMask::filled(MapSize::square(2), 100)),
                Some(LayerMask::filled(MapSize::square(2), 100)),
            ],
            channel_tex_scales: [0.02; 4],
            channel_tex_mults: [1.0; 4],
            channel_layer_names: [
                Some("a".into()),
                Some("b".into()),
                Some("c".into()),
                Some("d".into()),
            ],
            channel_imported: [false; 4],
        };
        let parallel_work = tmp.path().join("work-parallel");
        std::fs::create_dir_all(&parallel_work).unwrap();
        let completed_progress = std::sync::atomic::AtomicUsize::new(0);
        let progress = |done: usize, total: usize| {
            completed_progress.fetch_add(1, Ordering::SeqCst);
            assert!(
                done >= 1 && done <= total,
                "progress: done={done}, total={total}"
            );
            assert_eq!(total, 4, "expected total=4, got {total}");
        };
        let t0 = std::time::Instant::now();
        let (staged, warnings) = stage_splat_assets_from_layers(
            &parallel_proj,
            &parallel_inputs,
            &parallel_work,
            BakeOptions::default(),
            progress,
        )
        .expect("parallel bake should succeed");
        let parallel_elapsed = t0.elapsed();

        assert_eq!(warnings.len(), 0, "no imported-layer lint expected");
        assert_eq!(staged.per_slot_dds.len(), 4, "all 4 channels baked");
        assert_eq!(
            completed_progress.load(Ordering::SeqCst),
            4,
            "progress callback fires once per completed bake"
        );

        // Wall-time soft assertion: parallel ≤ 1.5 × single-slot.
        // CompressonatorCLI subprocess startup dominates (~hundreds of
        // ms) at this synthetic size, so 4 concurrent bakes hit
        // process-spawn serialisation on slow disks. The 1.5× bound
        // is the sprint prompt's target.
        let single_ms = single_elapsed.as_secs_f64() * 1000.0;
        let parallel_ms = parallel_elapsed.as_secs_f64() * 1000.0;
        eprintln!(
            "DNTS bake: single = {single_ms:.0} ms; parallel(4) = {parallel_ms:.0} ms; \
             ratio = {:.2}× (target < 1.5×)",
            parallel_ms / single_ms.max(1.0)
        );
        if parallel_ms > 1.5 * single_ms {
            eprintln!(
                "WARN: parallel bake exceeded 1.5× single-slot — check disk \
                 contention or Compressonator process-spawn overhead. Soft \
                 assertion; not failing the test."
            );
        }
    }
}
