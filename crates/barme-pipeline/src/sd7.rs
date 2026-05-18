//! Non-solid `.sd7` packager (ADR-013).
//!
//! Stages the `.smf` + `.smt` from [`crate::pymapconv`] alongside an emitted
//! `mapinfo.lua` (see [`crate::mapinfo`]) into a temp dir, then shells out to
//! the system 7-Zip binary to create the archive with `-ms=off` — the literal
//! PITFALL #9 flag. SpringFiles silently rejects solid `.sd7`, so we also
//! verify the post-condition with `7z l -slt` and parse the `Solid =` header.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tracing::{debug, error, info, warn};

/// 7-Zip binary names we try, in preference order. `7zz` is the modern
/// upstream-built name (Debian 13+ `7zip` package); `7z` is the legacy
/// p7zip-full name; `7za` is the smaller p7zip-standalone build (still
/// supports `7z` archives). We skip `7zr` deliberately — it's read-only-ish
/// and historically RAR-focused.
const CANDIDATE_BINARIES: &[&str] = &["7zz", "7z", "7za"];

#[derive(Debug, thiserror::Error)]
pub enum Sd7Error {
    #[error(
        "no 7-Zip binary on PATH (tried {tried:?}); install one (Ubuntu/Debian: `sudo apt install 7zip`)"
    )]
    SevenZipMissing { tried: Vec<&'static str> },

    #[error("failed to spawn {binary}: {source}")]
    Spawn {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "{binary} create exited with {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    )]
    CreateFailed {
        binary: PathBuf,
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },

    #[error(
        "{binary} list exited with {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    )]
    ListFailed {
        binary: PathBuf,
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },

    #[error("packaged archive came out solid (PITFALL #9 — SpringFiles will silently reject)")]
    SolidArchive,

    #[error("could not parse `Solid = ?` from {binary} -slt listing:\n{listing}")]
    UnreadableListing { binary: PathBuf, listing: String },

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A staged file: source path on disk, destination path within the archive.
pub struct StagedFile<'a> {
    pub src: &'a Path,
    pub archive_rel: &'a str,
}

/// Create a non-solid `.sd7` at `out_path` containing the named files.
///
/// `staging_dir` is used as a scratch root where files are copied into their
/// `archive_rel` layout before invocation, so 7-Zip captures the intended
/// in-archive names regardless of source locations. Must exist; the function
/// does not create it (typically a `tempfile::tempdir()` per-build).
///
/// On success, returns `out_path`. The archive is guaranteed non-solid
/// (verified via `7z l -slt`).
pub fn package(
    out_path: &Path,
    staging_dir: &Path,
    files: &[StagedFile<'_>],
) -> Result<PathBuf, Sd7Error> {
    let seven = find_seven_zip()?;

    if !staging_dir.is_dir() {
        return Err(Sd7Error::Io {
            path: staging_dir.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "staging_dir not a directory",
            ),
        });
    }

    for f in files {
        let dst = staging_dir.join(f.archive_rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Sd7Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::copy(f.src, &dst).map_err(|source| Sd7Error::Io {
            path: dst.clone(),
            source,
        })?;
    }

    // Wipe any prior archive at out_path so 7-Zip's "update existing"
    // behaviour doesn't surprise us.
    if out_path.exists() {
        std::fs::remove_file(out_path).map_err(|source| Sd7Error::Io {
            path: out_path.to_path_buf(),
            source,
        })?;
    }

    info!(
        ?seven,
        ?out_path,
        file_count = files.len(),
        "packaging .sd7"
    );

    // From inside `staging_dir`, `./*` captures the staged tree at its
    // intended in-archive paths.
    let mut create = Command::new(&seven);
    create
        .current_dir(staging_dir)
        .arg("a")
        .arg("-t7z")
        .arg("-ms=off")
        .arg("-mx=9")
        .arg(out_path)
        .arg("./");
    debug!(?create, "7z create command");

    let Output {
        status,
        stdout,
        stderr,
    } = create.output().map_err(|source| Sd7Error::Spawn {
        binary: seven.clone(),
        source,
    })?;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    if !status.success() {
        error!(?status, "7z create failed");
        return Err(Sd7Error::CreateFailed {
            binary: seven,
            status,
            stdout,
            stderr,
        });
    }

    verify_non_solid(&seven, out_path)?;

    info!(
        ?out_path,
        bytes = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0),
        "sd7 ok"
    );
    Ok(out_path.to_path_buf())
}

/// PITFALL #9 defence: ensure the archive came out non-solid.
fn verify_non_solid(seven: &Path, archive: &Path) -> Result<(), Sd7Error> {
    let mut list = Command::new(seven);
    list.arg("l").arg("-slt").arg(archive);
    let Output {
        status,
        stdout,
        stderr,
    } = list.output().map_err(|source| Sd7Error::Spawn {
        binary: seven.to_path_buf(),
        source,
    })?;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    if !status.success() {
        return Err(Sd7Error::ListFailed {
            binary: seven.to_path_buf(),
            status,
            stdout,
            stderr,
        });
    }

    // `7z l -slt` reports `Solid = +` (solid) or `Solid = -` (non-solid)
    // in the archive-properties header. The line appears once near the top.
    let solid_line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("Solid ="));
    match solid_line.map(|l| l.split('=').nth(1).map(|v| v.trim()).unwrap_or("")) {
        Some("-") => Ok(()),
        Some("+") => {
            error!("archive came out solid");
            Err(Sd7Error::SolidArchive)
        }
        _ => {
            warn!("could not parse Solid = ? from 7z listing");
            Err(Sd7Error::UnreadableListing {
                binary: seven.to_path_buf(),
                listing: stdout,
            })
        }
    }
}

fn find_seven_zip() -> Result<PathBuf, Sd7Error> {
    for name in CANDIDATE_BINARIES {
        if let Ok(p) = which::which(name) {
            debug!(?p, picked = name, "selected 7-Zip binary");
            return Ok(p);
        }
    }
    Err(Sd7Error::SevenZipMissing {
        tried: CANDIDATE_BINARIES.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_some_seven_zip_on_host() {
        // Hosts without 7z installed should fail at the workspace level;
        // pass-through here documents that requirement.
        assert!(find_seven_zip().is_ok(), "no 7z binary on PATH");
    }
}
