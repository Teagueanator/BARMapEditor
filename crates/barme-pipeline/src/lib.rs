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

use barme_core::{Heightmap, Project, SlotResolver};
use tracing::info;

pub mod build;
pub mod dnts;
pub mod featureplacer;
pub mod lua_ast;
pub mod mapinfo;
pub mod metal_layout;
pub mod minimap;
pub mod pymapconv;
pub mod sd7;
pub mod splat_pipeline;
pub mod startboxes;

pub use build::{BuildEvent, BuildEventSink, BuildPlan, BuildStage, LogStream, NEVER_CANCEL};
pub use dnts::{BakeOptions, DntsBakeError, bake_dnts};
pub use minimap::{
    MINIMAP_DIM, MinimapError, copy_minimap_override, render_minimap, stage_minimap,
};
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

    #[error(transparent)]
    Minimap(#[from] MinimapError),

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The caller's cancel flag flipped between stages. The pipeline
    /// stops at the next stage boundary; the temp dir cleans up via
    /// `Drop`. Sprint 20.
    #[error("build cancelled before stage {0:?}")]
    Cancelled(BuildStage),
}

/// D7 / Sprint 18 (F10) — extra inputs the minimap bake needs that
/// the heightmap-PNG-only signature can't provide. Bundled into a
/// single struct so adding fields here doesn't add yet another
/// positional arg to [`build_sd7`].
///
/// The build path threads this through to
/// [`minimap::stage_minimap`], which either copies
/// `project.minimap_override` (after dim validation) or auto-bakes a
/// 1024² PNG.
pub struct MinimapInputs<'a> {
    /// In-memory heightmap (may include unsaved brush edits — same
    /// invariant the launcher upholds when staging `heightmap_png`).
    pub heightmap: &'a Heightmap,
    /// Resolver for `LayerSource::Slot` → on-disk `diffuse.png`.
    /// Re-uses the same adapter the layer bake takes.
    pub slot_resolver: &'a dyn SlotResolver,
    /// Path to the `.barmeproj` file (if any) for resolving relative
    /// `minimap_override` paths. `None` when the project hasn't been
    /// saved yet — overrides then need to be absolute.
    pub project_path: Option<&'a Path>,
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
/// `minimap_inputs` carries the heightmap + slot resolver + project
/// path the minimap bake (D7 / Sprint 18) needs. When `None`,
/// PyMapConv synthesises a minimap from the diffuse BMP (`-t`) — a
/// noticeably blurrier fallback. Production callers always pass
/// `Some`; the smoke binary in `examples/` passes `None` for brevity.
///
/// On success returns `out_sd7`. On failure, returns a typed `BuildError`
/// with the underlying subprocess streams attached (via the variant chain).
///
/// **Sprint 20:** this entry point is now a thin wrapper around the
/// staged driver in [`build::execute_stages`] with a no-op event sink
/// and the [`NEVER_CANCEL`] sentinel. Callers that need progress
/// events or cancellation construct a [`BuildPlan`] directly.
#[allow(clippy::too_many_arguments)]
pub fn build_sd7(
    driver: &PyMapConvDriver,
    project: &Project,
    heightmap_png: &Path,
    texture_bmp: &Path,
    splat_inputs: SplatBakeInputs,
    layer_inputs: Option<LayerSplatBakeInputs>,
    minimap_inputs: Option<MinimapInputs<'_>>,
    work_dir: &Path,
    out_sd7: &Path,
) -> Result<PathBuf, BuildError> {
    build::execute_stages(
        driver,
        project,
        heightmap_png,
        texture_bmp,
        splat_inputs,
        layer_inputs.as_ref(),
        minimap_inputs,
        work_dir,
        out_sd7,
        &(),
        &NEVER_CANCEL,
    )
}

pub(crate) fn write_lua_file(
    work_dir: &Path,
    name: &str,
    contents: &str,
) -> Result<PathBuf, BuildError> {
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
pub(crate) fn write_black_metalmap_png(path: &Path, project: &Project) -> Result<(), BuildError> {
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
