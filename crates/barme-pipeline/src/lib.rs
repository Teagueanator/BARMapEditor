//! Build pipeline — turn a [`barme_core::Project`] + on-disk asset PNG/BMP into
//! the artefacts Recoil consumes (`.smf` + `.smt` + `mapinfo.lua` + sidecar
//! `mapconfig/*.lua`, packaged as a non-solid `.sd7`).
//!
//! - [`pymapconv`] — subprocess driver around the vendored PyMapConv binary
//!   (ADR-012). Produces `.smf` + `.smt`.
//! - [`dnts`] — `splatDetailNormalTex` bake pipeline (ADR-026); turns
//!   a slot's `normal.png` (+ optional diffuse) into a BC3 / DXT5 DDS
//!   via the vendored Compressonator (ADR-014).
//! - [`lua_ast`] — typed Lua AST + pretty-printer (ADR-029).
//! - [`mapinfo`] — `mapinfo.lua` emitter built on the AST (ADR-029,
//!   supersedes ADR-013).
//! - [`metal_layout`] — `mapconfig/map_metal_layout.lua` (spots + geos).
//! - [`startboxes`] — `mapconfig/map_startboxes.lua` (per-ally polygons).
//! - [`featureplacer`] — Springboard feature-placer trio:
//!   `LuaGaia/Gadgets/FP_featureplacer.lua` (PD-licensed gadget) +
//!   `mapconfig/featureplacer/config.lua` (redirect) +
//!   `mapconfig/featureplacer/set.lua` (data).
//! - [`sd7`] — non-solid `.sd7` packager around system 7-Zip (ADR-013).
//!
//! The end-to-end orchestrator is [`build_sd7`].

use std::path::{Path, PathBuf};

use barme_core::Project;
use tracing::info;

pub mod dnts;
pub mod featureplacer;
pub mod lua_ast;
pub mod mapinfo;
pub mod metal_layout;
pub mod pymapconv;
pub mod sd7;
pub mod splat_pipeline;
pub mod startboxes;

pub use dnts::{BakeOptions, DntsBakeError, bake_dnts};
pub use pymapconv::{CompileInputs, CompileOutputs, PyMapConvDriver, PyMapConvError};
pub use sd7::{Sd7Error, StagedFile};
pub use splat_pipeline::{
    LayerSplatBakeInputs, LintWarning, SplatBakeInputs, SplatPipelineError, StagedSplatAssets,
    stage_splat_assets, stage_splat_assets_from_layers,
};

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    PyMapConv(#[from] PyMapConvError),

    #[error(transparent)]
    Sd7(#[from] Sd7Error),

    #[error(transparent)]
    Splat(#[from] SplatPipelineError),

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Orchestrate a full build: PyMapConv → 4-file Lua emit → 7-Zip
/// non-solid pack.
///
/// `work_dir` is the scratch root for intermediate compile output and the
/// archive staging tree. Typically a `tempfile::tempdir()` per build. Must
/// exist; the function does not create it.
///
/// `out_sd7` is the final archive path. Existing files at this path are
/// overwritten.
///
/// `splat_inputs` carries the per-channel slot directories the splat
/// pipeline bakes DNTS from. The caller (typically `barme-app`'s
/// launcher) resolves `Project.splat_config.channels[i]: Option<u8>` to
/// a `tools/textures/<NN-slug>/` path via its slot registry. Inactive
/// channels pass `None`; the pipeline skips them.
///
/// On success returns `out_sd7`. On failure, returns a typed `BuildError`
/// with the underlying subprocess streams attached (via the variant chain).
#[allow(clippy::too_many_arguments)]
pub fn build_sd7(
    driver: &PyMapConvDriver,
    project: &Project,
    heightmap_png: &Path,
    texture_bmp: &Path,
    splat_inputs: SplatBakeInputs,
    layer_inputs: Option<LayerSplatBakeInputs>,
    work_dir: &Path,
    out_sd7: &Path,
) -> Result<PathBuf, BuildError> {
    let compile_out = work_dir.join("compile");
    std::fs::create_dir_all(&compile_out).map_err(|source| BuildError::Io {
        path: compile_out.clone(),
        source,
    })?;

    // PITFALL §13 / FINDINGS §5: when the user authored metal spots,
    // ship an all-zero metalmap so the BAR gadget's
    // `hasMetalmap == false` branch fires and our Lua spots become
    // the source of truth. When the project has no metal spots,
    // skip — PyMapConv's default 1×1 black metalmap applies and we
    // avoid ballooning the staging tree.
    let metalmap_path = if project.metal_spots.is_empty() {
        None
    } else {
        let path = work_dir.join(format!("{}_metalmap.png", project.name));
        write_black_metalmap_png(&path, project)?;
        Some(path)
    };

    info!(name = %project.name, "build_sd7: compiling SMF/SMT");
    let outputs = driver.compile(CompileInputs {
        project,
        heightmap_png,
        texture_bmp,
        metalmap_png: metalmap_path.as_deref(),
        out_dir: &compile_out,
    })?;

    // D6 (Sprint 12): stage the splat-side artefacts before rendering
    // mapinfo.lua so the resources block reflects the staged paths.
    // The baked DNTS DDS files + splat distribution PNG + specular
    // fallback all flow into the .sd7 alongside SMF/SMT. PyMapConv
    // does NOT touch any of these files (FINDINGS §10).
    //
    // D10 / Sprint 17 (ADR-041): when `layer_inputs` is present AND
    // the project has a non-empty layer stack, drive the splat
    // distribution PNG + DDS bake from the DNTS-bound layer masks +
    // per-layer tex_scale / tex_mult. Sprint 17 / Commit 6 deletes
    // the legacy fallback below once the runtime DNTS path is fully
    // layer-fed.
    let use_layers = layer_inputs.is_some() && !project.layers.layers.is_empty();
    let bake_opts = BakeOptions {
        yflip_normal: false,
        diffuse_in_alpha: if use_layers {
            project.dnts_diffuse_in_alpha
        } else {
            // ADR-025 baseline — `splatDetailNormalDiffuseAlpha = false`.
            project.splat_config.diffuse_in_alpha
        },
    };
    let (splat_staged, _lints) = if use_layers {
        let li = layer_inputs.as_ref().expect("guarded by use_layers");
        splat_pipeline::stage_splat_assets_from_layers(project, li, work_dir, bake_opts)?
    } else {
        (
            splat_pipeline::stage_splat_assets(project, &splat_inputs, work_dir, bake_opts)?,
            Vec::new(),
        )
    };

    // Build the typed `MapInfo`, then let the splat pipeline populate
    // its resources block with the staged file references. This is
    // the seam C8 (Sprint 14) plugs its lint pass into; D6 just
    // produces the data.
    let mut info: barme_core::MapInfo = project.into();
    if use_layers {
        let li = layer_inputs.as_ref().expect("guarded by use_layers");
        splat_pipeline::populate_resources_from_layers(&mut info, project, li, &splat_staged);
    } else {
        splat_pipeline::populate_resources(&mut info, project, &splat_staged);
    }

    // Lua sidecars — written to scratch paths under work_dir, then
    // staged into the archive at their canonical layout (mapinfo.lua at
    // root; the rest under `mapconfig/`).
    let mapinfo_path = write_lua_file(
        work_dir,
        "mapinfo.lua",
        &mapinfo::render_with(project, info),
    )?;
    let metal_path = write_lua_file(
        work_dir,
        "map_metal_layout.lua",
        &metal_layout::render(project),
    )?;
    // PITFALL §26: only ship `map_startboxes.lua` when the user
    // authored startboxes. Shipping an empty file shadows BAR's
    // default N/S or E/W fallback in
    // `luarules/gadgets/include/startbox_utilities.lua:43`,
    // producing a map with no playable spawn regions.
    let startboxes_path = startboxes::render_optional(project)
        .map(|body| write_lua_file(work_dir, "map_startboxes.lua", &body))
        .transpose()?;
    // Springboard feature-placer trio (PITFALL §14 fix, Sprint 11):
    // 1. The gadget itself, bundled as `LuaGaia/Gadgets/FP_featureplacer.lua`
    //    so BAR auto-loads it on map start.
    // 2. `mapconfig/featureplacer/config.lua` — one-liner redirect.
    // 3. `mapconfig/featureplacer/set.lua` — the actual data
    //    (`objectlist`/`unitlist`/`buildinglist` with C5's geo vents
    //    in `objectlist`).
    let fp_gadget_path = write_lua_file(
        work_dir,
        "FP_featureplacer.lua",
        featureplacer::FP_GADGET_SOURCE,
    )?;
    let fp_config_path =
        write_lua_file(work_dir, "fp_config.lua", &featureplacer::render_config())?;
    let fp_set_path = write_lua_file(work_dir, "fp_set.lua", &featureplacer::render_set(project))?;
    // LuaGaia bootstrap pair — required for the engine to load
    // anything in `LuaGaia/Gadgets/`. springcontent.sdz does NOT
    // ship a fallback, so without these the FP gadget never runs.
    let luagaia_main_path = write_lua_file(
        work_dir,
        "luagaia_main.lua",
        featureplacer::LUAGAIA_MAIN_SOURCE,
    )?;
    let luagaia_draw_path = write_lua_file(
        work_dir,
        "luagaia_draw.lua",
        featureplacer::LUAGAIA_DRAW_SOURCE,
    )?;

    let staging = work_dir.join("staging");
    std::fs::create_dir_all(&staging).map_err(|source| BuildError::Io {
        path: staging.clone(),
        source,
    })?;

    let smf_rel = format!("maps/{}.smf", project.name);
    let smt_rel = format!("maps/{}.smt", project.name);
    let mut staged = vec![
        StagedFile {
            src: &outputs.smf,
            archive_rel: &smf_rel,
        },
        StagedFile {
            src: &outputs.smt,
            archive_rel: &smt_rel,
        },
        StagedFile {
            src: &mapinfo_path,
            archive_rel: "mapinfo.lua",
        },
        StagedFile {
            src: &metal_path,
            archive_rel: "mapconfig/map_metal_layout.lua",
        },
        // Springboard feature-placer trio. PITFALL §14: a bare
        // `mapconfig/featureplacer/features.lua` is NOT a BAR path —
        // there's no consumer in BAR's source. The Springboard
        // gadget pattern (FP_featureplacer.lua + config.lua + set.lua)
        // is what real BAR maps ship (verified against gecko_isle_
        // remake_v1.2.1.sd7 + jade_empress_1.3.sd7 at HEAD).
        StagedFile {
            src: &fp_gadget_path,
            archive_rel: "LuaGaia/Gadgets/FP_featureplacer.lua",
        },
        StagedFile {
            src: &fp_config_path,
            archive_rel: "mapconfig/featureplacer/config.lua",
        },
        StagedFile {
            src: &fp_set_path,
            archive_rel: "mapconfig/featureplacer/set.lua",
        },
        // LuaGaia bootstrap pair. PITFALL §25: without these, the
        // engine never scans `LuaGaia/Gadgets/` on map load and the
        // FP gadget above is dead code.
        StagedFile {
            src: &luagaia_main_path,
            archive_rel: "LuaGaia/main.lua",
        },
        StagedFile {
            src: &luagaia_draw_path,
            archive_rel: "LuaGaia/draw.lua",
        },
    ];
    // Conditional startboxes file — see PITFALL §26 above.
    if let Some(ref path) = startboxes_path {
        staged.push(StagedFile {
            src: path,
            archive_rel: "mapconfig/map_startboxes.lua",
        });
    }

    // D6 (Sprint 12): bundle the splat distribution PNG + per-active-
    // slot DDS files + specular fallback. Paths inside the archive
    // mirror what the resources block emits in mapinfo.lua:
    //   - `maps/<projectname>_splatdistr.png`
    //   - `maps/textures/<slot-dir-name>_dnts.dds`
    //   - `maps/<projectname>_specular.dds`
    let mut splat_archive_paths: Vec<(PathBuf, String)> = Vec::new();
    if let Some(p) = splat_staged.splat_distr_png.as_ref() {
        let basename = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("splatdistr.png")
            .to_string();
        splat_archive_paths.push((p.clone(), format!("maps/{basename}")));
    }
    for dds in &splat_staged.per_slot_dds {
        splat_archive_paths.push((
            dds.disk_path.clone(),
            format!("maps/textures/{}", dds.filename),
        ));
    }
    if let Some(p) = splat_staged.specular_dds.as_ref() {
        let basename = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("specular.dds")
            .to_string();
        splat_archive_paths.push((p.clone(), format!("maps/{basename}")));
    }
    // Borrow + push as `StagedFile`s after we own the path strings.
    // Keep a vec of (path, rel) outlives the staged vec for the
    // duration of the pack call.
    for (path, rel) in &splat_archive_paths {
        staged.push(StagedFile {
            src: path,
            archive_rel: rel,
        });
    }

    info!(?out_sd7, "build_sd7: packaging");
    let sd7_path = sd7::package(out_sd7, &staging, &staged)?;
    Ok(sd7_path)
}

fn write_lua_file(work_dir: &Path, name: &str, contents: &str) -> Result<PathBuf, BuildError> {
    let path = work_dir.join(name);
    std::fs::write(&path, contents).map_err(|source| BuildError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Stage an all-zero grayscale PNG sized to the SMF metalmap
/// resolution (`(32 * smu_x) × (32 * smu_z)` pixels, half-res of the
/// type map per SRS §1.2). PyMapConv reads the red channel as metal
/// amount; an all-zero PNG → zero metal → BAR's
/// `map_metal_spot_placer.lua` sees an empty engine metalmap and
/// honours our Lua-spot list instead.
///
/// The dimension matters: PyMapConv `--help` says it resizes a
/// differently-sized input to `xsize/2 × ysize/2`, so a 1×1 PNG
/// also works in practice — but supplying the canonical-sized PNG
/// avoids surprises (no nearest-neighbour aliasing, deterministic
/// bytes on disk for the SD7 hash) and matches what the C8 lint
/// will eventually verify.
fn write_black_metalmap_png(path: &Path, project: &Project) -> Result<(), BuildError> {
    let w = 32u32 * project.size.smu_x;
    let h = 32u32 * project.size.smu_z;
    let buf = image::ImageBuffer::<image::Luma<u8>, Vec<u8>>::from_pixel(w, h, image::Luma([0]));
    buf.save(path).map_err(|e| BuildError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;
    info!(
        path = %path.display(),
        width = w,
        height = h,
        "build_sd7: staged all-zero metalmap PNG (PITFALL §13)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_core::MetalSpot;

    /// PITFALL §13: the metalmap PNG written by `build_sd7` (when
    /// `metal_spots` is non-empty) is sized to the SMF metalmap
    /// resolution (32 × smu_x by 32 × smu_z) and every pixel is
    /// zero.
    #[test]
    fn black_metalmap_png_dimensions_and_all_zero_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::new("dim-check", 4); // 4x4 SMU
        project.metal_spots.push(MetalSpot::new(100, 200));
        let path = dir.path().join("test_metalmap.png");
        write_black_metalmap_png(&path, &project).unwrap();

        assert!(path.exists(), "metalmap PNG was not written");
        let img = image::open(&path).unwrap().to_luma8();
        assert_eq!(img.width(), 32 * 4, "width should be 32 * smu_x");
        assert_eq!(img.height(), 32 * 4, "height should be 32 * smu_z");
        // Every pixel must be zero — `map_metal_spot_placer.lua`
        // bails if any sample > 0.
        assert!(
            img.iter().all(|&b| b == 0),
            "every metalmap pixel must be zero"
        );
    }

    /// Rectangular maps emit a rectangular PNG, not square.
    #[test]
    fn black_metalmap_png_handles_rectangular_maps() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project {
            size: barme_core::MapSize {
                smu_x: 8,
                smu_z: 12,
            },
            ..Project::new("rect", 4)
        };
        project.metal_spots.push(MetalSpot::new(100, 200));
        let path = dir.path().join("rect.png");
        write_black_metalmap_png(&path, &project).unwrap();

        let img = image::open(&path).unwrap().to_luma8();
        assert_eq!(img.width(), 32 * 8);
        assert_eq!(img.height(), 32 * 12);
    }
}
