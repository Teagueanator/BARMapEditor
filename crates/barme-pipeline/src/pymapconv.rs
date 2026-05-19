//! PyMapConv subprocess driver.
//!
//! Wraps the vendored `tools/pymapconv/pymapconv` binary (per ADR-011) with a
//! typed error surface (per ADR-012). The driver does not own filesystem
//! layout — callers pass in the heightmap, texture, and an output directory
//! they control. PyMapConv writes a `<name>.smf` and a sibling `<name>.smt`
//! into the output directory; the driver verifies both exist before returning
//! success.
//!
//! Working-directory note: PyMapConv looks up `./resources/geovent.bmp` and
//! the bundled DXT encoders at `./tools/` *relative to its cwd*, not its
//! `argv[0]`. The driver sets cwd to the binary's parent so those bundled
//! resources resolve correctly without flag rewriting.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};

use barme_core::Project;
use tracing::{debug, error, info, warn};

/// Relative path from the repo root to the vendored PyMapConv binary
/// (fetched by `scripts/fetch-pymapconv.sh`; see ADR-011).
const VENDORED_REL: &str = "tools/pymapconv/pymapconv";

/// Relative path from the repo root to the vendored Compressonator
/// directory (fetched by `scripts/fetch-compressonator.sh`; see ADR-014).
/// PyMapConv shells out to `CompressonatorCLI` by name (upstream
/// `src/pymapconv.py` lines 828 + 1032 — no path override), so the
/// driver prepends this dir to `PATH` for the child process. The fetch
/// script maintains a `CompressonatorCLI` → `compressonatorcli` symlink
/// to bridge the case mismatch.
const VENDORED_COMPRESSONATOR_REL: &str = "tools/compressonator";

/// A typed handle on a PyMapConv binary on disk. Cheap to construct; holds
/// only paths. The actual compile work happens in [`Self::compile`].
#[derive(Debug, Clone)]
pub struct PyMapConvDriver {
    binary: PathBuf,
    /// Directory prepended to `PATH` so PyMapConv finds `CompressonatorCLI`.
    /// `None` means caller takes responsibility for having it on PATH
    /// already (Stage 1 distro packaging will lean on this).
    compressonator_dir: Option<PathBuf>,
}

/// Everything required for a single compile invocation.
///
/// Borrowed so callers (UI, tests) can keep ownership of their `Project` and
/// asset paths. All paths should be absolute — the driver passes them
/// unchanged on the command line, and PyMapConv resolves them against its
/// cwd (which the driver sets to the binary's directory).
#[derive(Debug)]
pub struct CompileInputs<'a> {
    pub project: &'a Project,
    /// 16-bit grayscale PNG, dims `xsize*64+1` × `ysize*64+1`. PITFALL #4.
    pub heightmap_png: &'a Path,
    /// BMP, dims a multiple of 1024 on each side. PyMapConv infers SMF
    /// `mapx` as `width / 8`, so e.g. 1024-wide BMP ⇒ mapx=128 ⇒ heightmap
    /// dim 129 = BAR 2 SMU.
    pub texture_bmp: &'a Path,
    /// Optional 8-bit grayscale metalmap PNG. Dimensions are
    /// `(smu_x * 32, smu_z * 32)` per the SMF spec (SRS §1.2 —
    /// the metalmap is half-res of the type map = quarter-res of the
    /// heightmap). When `Some` and `Project.metal_spots` is non-empty
    /// the build path supplies an all-black PNG: `map_metal_spot_placer.lua`
    /// (FINDINGS §5 / PITFALL §13) bails if any metalmap pixel is
    /// non-zero, so we force the Lua-spots-are-source-of-truth path.
    /// When `None`, PyMapConv's default 1×1 black metalmap applies.
    pub metalmap_png: Option<&'a Path>,
    /// Directory PyMapConv writes `<name>.smf` and `<name>.smt` into. The
    /// directory must exist; the driver does not create it.
    pub out_dir: &'a Path,
}

/// Successful compile result. Both paths are guaranteed to exist on disk.
/// `stdout` / `stderr` are the buffered streams from the subprocess —
/// callers (UI, tests) surface them as compile diagnostics.
#[derive(Debug)]
pub struct CompileOutputs {
    pub smf: PathBuf,
    pub smt: PathBuf,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PyMapConvError {
    #[error("pymapconv binary not found at {0}; run scripts/fetch-pymapconv.sh from the repo root")]
    BinaryMissing(PathBuf),

    #[error("failed to spawn pymapconv at {binary}: {source}")]
    Spawn {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "pymapconv exited with {status} (binary: {binary})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    )]
    NonZeroExit {
        binary: PathBuf,
        status: ExitStatus,
        stdout: String,
        stderr: String,
    },

    #[error(
        "pymapconv reported success but expected output is missing: {path}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    )]
    MissingOutput {
        path: PathBuf,
        stdout: String,
        stderr: String,
    },

    #[error("output_dir {0} does not exist or is not a directory")]
    BadOutputDir(PathBuf),

    #[error(
        "CompressonatorCLI symlink missing at {0}; run scripts/fetch-compressonator.sh from the repo root"
    )]
    CompressonatorMissing(PathBuf),
}

impl PyMapConvDriver {
    /// Locate the vendored binary under `<repo_root>/tools/pymapconv/`
    /// and the bundled Compressonator under `<repo_root>/tools/compressonator/`.
    /// Fails fast if either is missing so callers get a typed error with
    /// the actionable "run the fetch script" hint rather than a downstream
    /// `Spawn` `ENOENT` or pymapconv's own `sh: CompressonatorCLI: not found`.
    pub fn vendored(repo_root: &Path) -> Result<Self, PyMapConvError> {
        let binary = repo_root.join(VENDORED_REL);
        if !binary.is_file() {
            warn!(?binary, "pymapconv binary not present");
            return Err(PyMapConvError::BinaryMissing(binary));
        }
        let compressonator_dir = repo_root.join(VENDORED_COMPRESSONATOR_REL);
        let cli_alias = compressonator_dir.join("CompressonatorCLI");
        if !cli_alias.exists() {
            warn!(?cli_alias, "CompressonatorCLI symlink not present");
            return Err(PyMapConvError::CompressonatorMissing(cli_alias));
        }
        debug!(?binary, ?compressonator_dir, "located vendored toolchain");
        Ok(Self {
            binary,
            compressonator_dir: Some(compressonator_dir),
        })
    }

    /// Construct a driver from an arbitrary binary path with no PATH
    /// augmentation. Useful for testing against an out-of-tree install or
    /// a fake harness; production code should prefer [`Self::vendored`].
    pub fn from_binary(binary: PathBuf) -> Self {
        Self {
            binary,
            compressonator_dir: None,
        }
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Run a single compile. The output directory must exist before this is
    /// called. On success the returned `.smf` and `.smt` paths are
    /// guaranteed to exist; on failure the typed error carries the captured
    /// stdout/stderr so the UI can surface PyMapConv's own diagnostics.
    pub fn compile(&self, inputs: CompileInputs<'_>) -> Result<CompileOutputs, PyMapConvError> {
        let CompileInputs {
            project,
            heightmap_png,
            texture_bmp,
            metalmap_png,
            out_dir,
        } = inputs;

        if !out_dir.is_dir() {
            return Err(PyMapConvError::BadOutputDir(out_dir.to_path_buf()));
        }

        let smf_path = out_dir.join(format!("{}.smf", project.name));
        let smt_path = out_dir.join(format!("{}.smt", project.name));

        let cwd = self.binary.parent().unwrap_or_else(|| Path::new("."));

        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(cwd)
            .arg("-o")
            .arg(&smf_path)
            .arg("-t")
            .arg(texture_bmp)
            .arg("-a")
            .arg(heightmap_png)
            .arg("-x")
            .arg(format_height(project.max_height))
            .arg("-n")
            .arg(format_height(project.min_height))
            .arg("-u")
            // Upstream bug (v0.6.3 src/pymapconv.py lines 960-986): Linux
            // tile-compression always writes `temp/temp{i}.dds` (flat), but
            // the read-back loop branches on `numthreads > 1` and tries to
            // read `temp/thread{i % n}/temp{i}.dds` instead — a path Linux
            // never creates. Default numthreads is 4, so unless we force
            // single-threaded read-back we trip a FileNotFoundError. See
            // ADR-012 and the previous session log's flag-table correction.
            .arg("-q")
            .arg("1");

        // C4 / FINDINGS §5: only attach -mm when the caller actually
        // staged a metalmap PNG. PyMapConv's default (1×1 black) is
        // fine when `Project.metal_spots` is empty; we override only
        // to force the all-zero engine metalmap that lets the
        // `map_metal_spot_placer.lua` gadget pick up our Lua spots.
        if let Some(mm) = metalmap_png {
            cmd.arg("-m").arg(mm);
        }

        if let Some(dir) = &self.compressonator_dir {
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut entries = vec![dir.clone()];
            entries.extend(std::env::split_paths(&existing));
            match std::env::join_paths(entries) {
                Ok(joined) => {
                    cmd.env("PATH", joined);
                }
                Err(err) => {
                    // join_paths only fails on `:` in entries; vendored path
                    // is under repo root so this is effectively unreachable.
                    warn!(?err, ?dir, "could not extend PATH with compressonator dir");
                }
            }
        }

        info!(
            binary = ?self.binary,
            cwd = ?cwd,
            compressonator = ?self.compressonator_dir,
            ?smf_path,
            heightmap = ?heightmap_png,
            texture = ?texture_bmp,
            min = project.min_height,
            max = project.max_height,
            "invoking pymapconv"
        );
        debug!(?cmd, "full pymapconv command");

        let Output {
            status,
            stdout,
            stderr,
        } = cmd.output().map_err(|source| PyMapConvError::Spawn {
            binary: self.binary.clone(),
            source,
        })?;

        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();

        // Upstream quirk: pymapconv exits with status 1 on Linux even after
        // a successful compile (the bundled Qt event loop closes
        // "abnormally" when no display is held open). The "All Done!" log
        // line is the real success marker, but the only contract we trust
        // is the actual `.smf` / `.smt` on disk. So: artifacts present →
        // success regardless of exit code; missing → fail, attaching exit
        // status + both streams so the user can diagnose.
        let smf_present = smf_path.is_file();
        let smt_present = smt_path.is_file();

        if smf_present && smt_present {
            if !status.success() {
                warn!(
                    ?status,
                    "pymapconv exited non-zero but produced expected outputs \
                     (known upstream Qt-lifecycle quirk on Linux); accepting"
                );
            }
            info!(
                smf_bytes = std::fs::metadata(&smf_path).map(|m| m.len()).unwrap_or(0),
                smt_bytes = std::fs::metadata(&smt_path).map(|m| m.len()).unwrap_or(0),
                "pymapconv ok"
            );
        } else if !status.success() {
            error!(
                ?status,
                stdout_len = stdout.len(),
                stderr_len = stderr.len(),
                "pymapconv failed and no outputs were written"
            );
            return Err(PyMapConvError::NonZeroExit {
                binary: self.binary.clone(),
                status,
                stdout,
                stderr,
            });
        } else {
            let missing = if !smf_present { &smf_path } else { &smt_path };
            error!(?missing, "pymapconv exit=0 but expected output missing");
            return Err(PyMapConvError::MissingOutput {
                path: missing.clone(),
                stdout,
                stderr,
            });
        }

        Ok(CompileOutputs {
            smf: smf_path,
            smt: smt_path,
            stdout,
            stderr,
        })
    }
}

/// PyMapConv parses heights as integers in its argparse layer. Pass them as
/// rounded integer strings to avoid a `ValueError` on `"256.0"`.
fn format_height(h: f32) -> String {
    (h.round() as i64).to_string()
}
