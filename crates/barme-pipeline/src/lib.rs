//! Build pipeline — turn a [`barme_core::Project`] + on-disk asset PNG/BMP into
//! the artefacts Recoil consumes (`.smf` + `.smt` + `mapinfo.lua`, packaged as
//! a non-solid `.sd7`).
//!
//! - [`pymapconv`] — subprocess driver around the vendored PyMapConv binary
//!   (ADR-012). Produces `.smf` + `.smt`.
//! - [`mapinfo`]   — minimum-viable `mapinfo.lua` emitter (ADR-013).
//! - [`sd7`]       — non-solid `.sd7` packager around system 7-Zip (ADR-013).
//!
//! The end-to-end orchestrator is [`build_sd7`].

use std::path::{Path, PathBuf};

use barme_core::Project;
use tracing::info;

pub mod mapinfo;
pub mod pymapconv;
pub mod sd7;

pub use pymapconv::{CompileInputs, CompileOutputs, PyMapConvDriver, PyMapConvError};
pub use sd7::{Sd7Error, StagedFile};

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    PyMapConv(#[from] PyMapConvError),

    #[error(transparent)]
    Sd7(#[from] Sd7Error),

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Orchestrate a full build: PyMapConv → mapinfo emit → 7-Zip non-solid pack.
///
/// `work_dir` is the scratch root for intermediate compile output and the
/// archive staging tree. Typically a `tempfile::tempdir()` per build. Must
/// exist; the function does not create it.
///
/// `out_sd7` is the final archive path. Existing files at this path are
/// overwritten.
///
/// On success returns `out_sd7`. On failure, returns a typed `BuildError`
/// with the underlying subprocess streams attached (via the variant chain).
pub fn build_sd7(
    driver: &PyMapConvDriver,
    project: &Project,
    heightmap_png: &Path,
    texture_bmp: &Path,
    work_dir: &Path,
    out_sd7: &Path,
) -> Result<PathBuf, BuildError> {
    let compile_out = work_dir.join("compile");
    std::fs::create_dir_all(&compile_out).map_err(|source| BuildError::Io {
        path: compile_out.clone(),
        source,
    })?;

    info!(name = %project.name, "build_sd7: compiling SMF/SMT");
    let outputs = driver.compile(CompileInputs {
        project,
        heightmap_png,
        texture_bmp,
        out_dir: &compile_out,
    })?;

    let mapinfo_text = mapinfo::render(project);
    let mapinfo_path = work_dir.join("mapinfo.lua");
    std::fs::write(&mapinfo_path, &mapinfo_text).map_err(|source| BuildError::Io {
        path: mapinfo_path.clone(),
        source,
    })?;

    let staging = work_dir.join("staging");
    std::fs::create_dir_all(&staging).map_err(|source| BuildError::Io {
        path: staging.clone(),
        source,
    })?;

    let smf_rel = format!("maps/{}.smf", project.name);
    let smt_rel = format!("maps/{}.smt", project.name);
    let staged = [
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
    ];

    info!(?out_sd7, "build_sd7: packaging");
    let sd7_path = sd7::package(out_sd7, &staging, &staged)?;
    Ok(sd7_path)
}
