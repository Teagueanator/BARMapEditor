//! Sprint 20 — staged, cancellable build pipeline.
//!
//! The legacy `build_sd7` entry point ran every stage straight-line
//! on the calling thread, blocking the UI for 10–60 s. This module
//! splits that body into discrete [`BuildStage`]s, emits
//! [`BuildEvent`]s before each stage transition, and checks a
//! cooperative [`AtomicBool`] cancel flag between stages so the UI
//! can interrupt mid-build. Subprocess line streaming + mid-step
//! cancellation lands in Chunk 2; this module ships the scaffolding.
//!
//! Two entry points share the same internal driver:
//!
//! 1. **[`BuildPlan::execute`]** — owned-data path used by the
//!    editor's worker thread. The plan holds an owned `Project`
//!    clone, a `Box<dyn SlotResolver + Send + Sync>`, and owned
//!    paths so it can be moved across the `thread::spawn` boundary
//!    without lifetime juggling.
//!
//! 2. **[`crate::build_sd7`]** — legacy reference-based wrapper for
//!    callers that don't care about progress (the smoke example,
//!    integration tests). Passes a no-op `()` sink and the
//!    [`NEVER_CANCEL`] sentinel.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use barme_core::{Heightmap, Project, SlotResolver};
use tracing::{info, warn};

use crate::dnts::BakeOptions;
use crate::{
    BuildError, CompileInputs, LayerSplatBakeInputs, MinimapInputs, PyMapConvDriver,
    SplatBakeInputs, StagedFile, featureplacer, mapinfo, metal_layout, minimap, sd7,
    splat_pipeline, startboxes,
};

/// Coarse-grained stage of the build pipeline. Returned via
/// [`BuildEvent::Stage`] so the UI can render "Building: <stage>".
///
/// `RenderDiffuse` and `InstallToBar` are emitted by the editor's
/// worker around `BuildPlan::execute` (the diffuse bake lives in
/// `barme-app`; the install step is a local file copy past the
/// pipeline's `.sd7` output). Everything else is emitted by
/// [`execute_stages`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildStage {
    /// Worker-emitted: layered diffuse bake (CPU-heavy, ~1.5 s on
    /// 16-SMU) → texture BMP fed to PyMapConv.
    RenderDiffuse,
    PrepareStaging,
    RenderMinimap,
    InvokePyMapConv,
    StageSplatAssets,
    EmitMapInfoLua,
    EmitMetalLayoutLua,
    EmitStartboxesLua,
    EmitFeaturePlacerLua,
    PackageSd7,
    /// Worker-emitted: copy the produced `.sd7` into BAR's user maps
    /// directory.
    InstallToBar,
    Done,
}

impl BuildStage {
    /// Human-readable label for the progress overlay + status strip.
    pub fn label(&self) -> &'static str {
        match self {
            BuildStage::RenderDiffuse => "Baking diffuse texture",
            BuildStage::PrepareStaging => "Preparing staging directory",
            BuildStage::RenderMinimap => "Rendering minimap",
            BuildStage::InvokePyMapConv => "Compiling SMF + SMT (PyMapConv)",
            BuildStage::StageSplatAssets => "Staging splat / DNTS textures",
            BuildStage::EmitMapInfoLua => "Emitting mapinfo.lua",
            BuildStage::EmitMetalLayoutLua => "Emitting metal layout",
            BuildStage::EmitStartboxesLua => "Emitting start boxes",
            BuildStage::EmitFeaturePlacerLua => "Emitting feature placer",
            BuildStage::PackageSd7 => "Packaging non-solid .sd7",
            BuildStage::InstallToBar => "Installing into BAR maps dir",
            BuildStage::Done => "Done",
        }
    }

    /// Approximate cumulative fraction of total build time at the END
    /// of this stage (0.0..=1.0). Hand-calibrated from manual smoke
    /// runs — used for the overlay progress bar when no sub-stage
    /// progress is reported. Doesn't need to be exact; the user just
    /// wants a sense of "10 % vs 80 %".
    pub fn cumulative_fraction(&self) -> f32 {
        match self {
            BuildStage::RenderDiffuse => 0.06,
            BuildStage::PrepareStaging => 0.08,
            BuildStage::RenderMinimap => 0.18,
            BuildStage::InvokePyMapConv => 0.78,
            BuildStage::StageSplatAssets => 0.92,
            BuildStage::EmitMapInfoLua => 0.93,
            BuildStage::EmitMetalLayoutLua => 0.94,
            BuildStage::EmitStartboxesLua => 0.95,
            BuildStage::EmitFeaturePlacerLua => 0.96,
            BuildStage::PackageSd7 => 0.98,
            BuildStage::InstallToBar => 1.0,
            BuildStage::Done => 1.0,
        }
    }
}

/// Which subprocess stream a log line came from. The build log panel
/// tints each accordingly: stdout / Info = foreground; stderr / Warn
/// = yellow; Error = red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    /// Editor-internal info lines (stage transitions, paths).
    Info,
    Warn,
    Error,
}

/// Event emitted by [`BuildPlan::execute`]. The UI receives these via
/// an mpsc channel; tests use a `Vec<BuildEvent>` accumulator.
#[derive(Debug, Clone)]
pub enum BuildEvent {
    Stage(BuildStage),
    Log {
        line: String,
        stream: LogStream,
    },
    /// Sub-stage progress (0.0..=1.0). Cleared on every stage
    /// transition. Currently used only for the PyMapConv compile;
    /// other stages emit no sub-progress and the overlay falls back
    /// to the stage's [`BuildStage::cumulative_fraction`].
    Progress(f32),
}

/// Trait the staged driver writes events to. The synchronous
/// `build_sd7` path passes `&()` (no-op); the worker thread path
/// passes a closure that forwards on an `mpsc::Sender`.
///
/// `Sync` is required so the staged driver can capture
/// `&dyn BuildEventSink` inside a closure handed to
/// [`invoke_with_streaming`] (whose `on_line` callback fires from
/// reader threads and therefore needs to be `Sync`).
pub trait BuildEventSink: Sync {
    fn emit(&self, event: BuildEvent);
}

impl BuildEventSink for () {
    fn emit(&self, _: BuildEvent) {}
}

impl<F: Fn(BuildEvent) + Sync> BuildEventSink for F {
    fn emit(&self, e: BuildEvent) {
        (self)(e)
    }
}

/// Sentinel "never cancel" flag. The legacy `build_sd7` path borrows
/// this so the staged helper can call `cancel.load(...)` without a
/// `None` check.
pub static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

/// Poll interval for the cancel flag while a child subprocess is
/// running. 50 ms is short enough that Cancel feels instant in the UI
/// and long enough that the worker thread isn't busy-spinning.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Returned by [`invoke_with_streaming`]. Carries the captured streams
/// so callers (PyMapConv driver, Compressonator) can still apply their
/// post-exit success heuristics (e.g. PyMapConv's "exit 1 on success"
/// quirk) against the full buffered output, not just the per-line
/// callback's side effects.
#[derive(Debug)]
pub struct StreamedOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Run `cmd`, attaching `Stdio::piped()` to stdout + stderr, and
/// stream each line through `on_line` as it arrives. The full output
/// of both streams is also accumulated in [`StreamedOutput`] for
/// callers that still need the captured-after-exit shape.
///
/// `cancel.load(Relaxed)` is checked while waiting on the child;
/// when set, the child is killed (best-effort — see PITFALL #6 in
/// the sprint prompt; on Linux this only signals the direct child,
/// any subprocess group it spawned dies in ~5 s on its own).
///
/// `on_line` runs from one of two reader threads; the bound is
/// `Fn(&str, LogStream) + Sync` so closures that share state via
/// `&Mutex<...>` work without extra annotations.
///
/// Behaviour on cancellation: returns
/// `Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))`.
/// Callers map that to their own typed-error cancellation variant.
pub fn invoke_with_streaming<F>(
    cmd: &mut Command,
    on_line: F,
    cancel: &AtomicBool,
) -> Result<StreamedOutput, std::io::Error>
where
    F: Fn(&str, LogStream) + Sync,
{
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout missing");
    let stderr = child.stderr.take().expect("piped stderr missing");

    let stdout_buf: Mutex<String> = Mutex::new(String::new());
    let stderr_buf: Mutex<String> = Mutex::new(String::new());

    let on_line_ref = &on_line;
    let stdout_buf_ref = &stdout_buf;
    let stderr_buf_ref = &stderr_buf;

    let status = std::thread::scope(|s| -> Result<ExitStatus, std::io::Error> {
        let stdout_handle = s.spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                on_line_ref(&line, LogStream::Stdout);
                if let Ok(mut buf) = stdout_buf_ref.lock() {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });
        let stderr_handle = s.spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                on_line_ref(&line, LogStream::Stderr);
                if let Ok(mut buf) = stderr_buf_ref.lock() {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });

        let final_status = wait_with_cancel(&mut child, cancel)?;
        // Wait for both reader threads to finish draining their
        // pipes before we report success. The child has exited (or
        // been killed) by this point, so the pipes will EOF and the
        // readers will exit on their own.
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        Ok(final_status)
    })?;

    let stdout_str = stdout_buf.into_inner().unwrap_or_default();
    let stderr_str = stderr_buf.into_inner().unwrap_or_default();
    Ok(StreamedOutput {
        status,
        stdout: stdout_str,
        stderr: stderr_str,
    })
}

/// Poll the cancel flag while waiting on the child. When the flag
/// flips, kill the child + wait for its exit so we don't leak a
/// zombie. Returns the final `ExitStatus` from `child.wait()` even
/// after a kill (the killed status carries the signal info).
fn wait_with_cancel(child: &mut Child, cancel: &AtomicBool) -> Result<ExitStatus, std::io::Error> {
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None => {
                if cancel.load(Ordering::Relaxed) {
                    warn!("invoke_with_streaming: cancel flag set; killing child");
                    let _ = child.kill();
                    let status = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        format!("cancelled (post-kill status: {:?})", status.as_ref().ok()),
                    ));
                }
                std::thread::sleep(CANCEL_POLL_INTERVAL);
            }
        }
    }
}

/// One-shot snapshot of every input the staged pipeline needs. Owned
/// so it can cross the `thread::spawn` boundary at the editor side
/// without lifetime juggling.
///
/// The pipeline crate doesn't depend on `barme-app`, so the slot
/// resolver lives behind a `Box<dyn SlotResolver + Send + Sync>` —
/// the app constructs an owned resolver (clone of its slot registry
/// plus an optional project root) and boxes it before passing the
/// plan to the worker.
pub struct BuildPlan {
    pub driver: PyMapConvDriver,
    pub project: Project,
    pub heightmap_png: PathBuf,
    pub texture_bmp: PathBuf,
    pub splat_inputs: SplatBakeInputs,
    pub layer_inputs: Option<LayerSplatBakeInputs>,
    /// In-memory heightmap for the minimap bake. `None` skips the
    /// auto-bake; PyMapConv synthesises from the texture BMP.
    pub heightmap: Option<Heightmap>,
    pub project_path: Option<PathBuf>,
    pub slot_resolver: Box<dyn SlotResolver + Send + Sync>,
    pub work_dir: PathBuf,
    pub out_sd7: PathBuf,
}

impl BuildPlan {
    /// Drive the pipeline. Returns the path to the produced `.sd7` on
    /// success, or `BuildError::Cancelled(stage)` when `cancel`
    /// flipped before the listed stage started.
    pub fn execute(
        self,
        events: &dyn BuildEventSink,
        cancel: &AtomicBool,
    ) -> Result<PathBuf, BuildError> {
        let minimap_inputs = self.heightmap.as_ref().map(|hm| MinimapInputs {
            heightmap: hm,
            slot_resolver: &*self.slot_resolver,
            project_path: self.project_path.as_deref(),
        });
        execute_stages(
            &self.driver,
            &self.project,
            &self.heightmap_png,
            &self.texture_bmp,
            self.splat_inputs.clone(),
            self.layer_inputs.as_ref(),
            minimap_inputs,
            &self.work_dir,
            &self.out_sd7,
            events,
            cancel,
        )
    }
}

/// Staged driver shared by [`BuildPlan::execute`] and the legacy
/// `build_sd7` wrapper. Behaviour-preserving relative to the
/// pre-Sprint-20 `build_sd7` body — the only additions are the
/// `events.emit(...)` calls + the `cancel.load(...)` checks between
/// stages.
///
/// Cancellation is cooperative between stages here. Mid-subprocess
/// cancellation requires the streaming wrappers (Chunk 2 of Sprint
/// 20); for now a long-running PyMapConv compile must run to
/// completion before the cancel fires.
#[allow(clippy::too_many_arguments)]
pub fn execute_stages(
    driver: &PyMapConvDriver,
    project: &Project,
    heightmap_png: &Path,
    texture_bmp: &Path,
    splat_inputs: SplatBakeInputs,
    layer_inputs: Option<&LayerSplatBakeInputs>,
    minimap_inputs: Option<MinimapInputs<'_>>,
    work_dir: &Path,
    out_sd7: &Path,
    events: &dyn BuildEventSink,
    cancel: &AtomicBool,
) -> Result<PathBuf, BuildError> {
    // ─── Stage 1: prepare staging dir ──────────────────────────────
    check_cancel(cancel, &BuildStage::PrepareStaging)?;
    emit_stage(events, BuildStage::PrepareStaging);
    let compile_out = work_dir.join("compile");
    std::fs::create_dir_all(&compile_out).map_err(|source| BuildError::Io {
        path: compile_out.clone(),
        source,
    })?;
    emit_info(events, format!("staging dir: {}", work_dir.display()));

    // PITFALL §13 / FINDINGS §5 — stage an all-zero metalmap when
    // metal spots are authored. Cheap (single PNG write); kept under
    // PrepareStaging so the user doesn't see a separate stage flash
    // by for a 2 ms op.
    let metalmap_path = if project.metal_spots.is_empty() {
        None
    } else {
        let path = work_dir.join(format!("{}_metalmap.png", project.name));
        crate::write_black_metalmap_png(&path, project)?;
        Some(path)
    };

    // ─── Stage 2: minimap bake ────────────────────────────────────
    let minimap_path = if let Some(mi) = minimap_inputs {
        check_cancel(cancel, &BuildStage::RenderMinimap)?;
        emit_stage(events, BuildStage::RenderMinimap);
        let path = work_dir.join(format!("{}_minimap.png", project.name));
        minimap::stage_minimap(
            project,
            mi.project_path,
            mi.heightmap,
            mi.slot_resolver,
            &path,
        )?;
        emit_info(events, format!("minimap → {}", path.display()));
        Some(path)
    } else {
        info!("execute_stages: no minimap_inputs — PyMapConv will synthesise from diffuse");
        None
    };

    // ─── Stage 3: PyMapConv ───────────────────────────────────────
    check_cancel(cancel, &BuildStage::InvokePyMapConv)?;
    emit_stage(events, BuildStage::InvokePyMapConv);
    info!(name = %project.name, "execute_stages: compiling SMF/SMT");
    let on_pymapconv_line = |line: &str, stream: LogStream| {
        events.emit(BuildEvent::Log {
            line: line.to_string(),
            stream,
        });
    };
    let outputs = driver
        .compile_streaming(
            CompileInputs {
                project,
                heightmap_png,
                texture_bmp,
                metalmap_png: metalmap_path.as_deref(),
                minimap_png: minimap_path.as_deref(),
                out_dir: &compile_out,
            },
            on_pymapconv_line,
            cancel,
        )
        .map_err(|e| match e {
            crate::PyMapConvError::Cancelled => BuildError::Cancelled(BuildStage::InvokePyMapConv),
            other => other.into(),
        })?;
    // PyMapConv captured stdout/stderr is already available as a
    // post-exit aggregate in `outputs` (the streaming wrapper
    // accumulated it). The per-line callback above has already
    // surfaced each line live; nothing to replay here.
    let _ = (&outputs.stdout, &outputs.stderr);

    // ─── Stage 4: splat assets ────────────────────────────────────
    check_cancel(cancel, &BuildStage::StageSplatAssets)?;
    emit_stage(events, BuildStage::StageSplatAssets);
    let use_layers = layer_inputs.is_some() && !project.layers.layers.is_empty();
    let bake_opts = BakeOptions {
        yflip_normal: false,
        diffuse_in_alpha: if use_layers {
            project.dnts_diffuse_in_alpha
        } else {
            project.splat_config.diffuse_in_alpha
        },
    };
    let (splat_staged, _lints) = if use_layers {
        let li = layer_inputs.expect("guarded by use_layers");
        splat_pipeline::stage_splat_assets_from_layers(project, li, work_dir, bake_opts)?
    } else {
        (
            splat_pipeline::stage_splat_assets(project, &splat_inputs, work_dir, bake_opts)?,
            Vec::new(),
        )
    };
    emit_info(
        events,
        format!(
            "splat: {} DDS slots staged, splat_distr {}",
            splat_staged.per_slot_dds.len(),
            if splat_staged.splat_distr_png.is_some() {
                "yes"
            } else {
                "no"
            }
        ),
    );

    // Build the typed `MapInfo`, then let the splat pipeline populate
    // its resources block with the staged file references.
    let mut info: barme_core::MapInfo = project.into();
    if use_layers {
        let li = layer_inputs.expect("guarded by use_layers");
        splat_pipeline::populate_resources_from_layers(&mut info, project, li, &splat_staged);
    } else {
        splat_pipeline::populate_resources(&mut info, project, &splat_staged);
    }

    // ─── Stage 5–8: Lua sidecars ─────────────────────────────────
    check_cancel(cancel, &BuildStage::EmitMapInfoLua)?;
    emit_stage(events, BuildStage::EmitMapInfoLua);
    let mapinfo_path = crate::write_lua_file(
        work_dir,
        "mapinfo.lua",
        &mapinfo::render_with(project, info),
    )?;

    check_cancel(cancel, &BuildStage::EmitMetalLayoutLua)?;
    emit_stage(events, BuildStage::EmitMetalLayoutLua);
    let metal_path = crate::write_lua_file(
        work_dir,
        "map_metal_layout.lua",
        &metal_layout::render(project),
    )?;

    check_cancel(cancel, &BuildStage::EmitStartboxesLua)?;
    emit_stage(events, BuildStage::EmitStartboxesLua);
    let startboxes_path = startboxes::render_optional(project)
        .map(|body| crate::write_lua_file(work_dir, "map_startboxes.lua", &body))
        .transpose()?;

    check_cancel(cancel, &BuildStage::EmitFeaturePlacerLua)?;
    emit_stage(events, BuildStage::EmitFeaturePlacerLua);
    let fp_gadget_path = crate::write_lua_file(
        work_dir,
        "FP_featureplacer.lua",
        featureplacer::FP_GADGET_SOURCE,
    )?;
    let fp_config_path =
        crate::write_lua_file(work_dir, "fp_config.lua", &featureplacer::render_config())?;
    let fp_set_path =
        crate::write_lua_file(work_dir, "fp_set.lua", &featureplacer::render_set(project))?;
    let luagaia_main_path = crate::write_lua_file(
        work_dir,
        "luagaia_main.lua",
        featureplacer::LUAGAIA_MAIN_SOURCE,
    )?;
    let luagaia_draw_path = crate::write_lua_file(
        work_dir,
        "luagaia_draw.lua",
        featureplacer::LUAGAIA_DRAW_SOURCE,
    )?;

    // ─── Stage 9: package + verify non-solid ──────────────────────
    check_cancel(cancel, &BuildStage::PackageSd7)?;
    emit_stage(events, BuildStage::PackageSd7);

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
        StagedFile {
            src: &luagaia_main_path,
            archive_rel: "LuaGaia/main.lua",
        },
        StagedFile {
            src: &luagaia_draw_path,
            archive_rel: "LuaGaia/draw.lua",
        },
    ];
    if let Some(ref path) = startboxes_path {
        staged.push(StagedFile {
            src: path,
            archive_rel: "mapconfig/map_startboxes.lua",
        });
    }

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
    for (path, rel) in &splat_archive_paths {
        staged.push(StagedFile {
            src: path,
            archive_rel: rel,
        });
    }

    info!(?out_sd7, "execute_stages: packaging");
    let sd7_path = sd7::package(out_sd7, &staging, &staged)?;
    emit_info(events, format!("packaged: {}", sd7_path.display()));

    // ─── Stage 10: done ──────────────────────────────────────────
    emit_stage(events, BuildStage::Done);
    Ok(sd7_path)
}

/// Emit a `BuildEvent::Stage` and an info log line.
fn emit_stage(events: &dyn BuildEventSink, stage: BuildStage) {
    info!(stage = ?stage, "build stage");
    let label = stage.label().to_string();
    events.emit(BuildEvent::Stage(stage));
    events.emit(BuildEvent::Log {
        line: format!("▸ {label}"),
        stream: LogStream::Info,
    });
}

/// Emit a single editor-info log line.
fn emit_info(events: &dyn BuildEventSink, line: String) {
    events.emit(BuildEvent::Log {
        line,
        stream: LogStream::Info,
    });
}

/// Bail with [`BuildError::Cancelled`] when the cancel flag is set.
/// Called between stages; mid-stage cancellation is the streaming
/// wrappers' job (Chunk 2 of Sprint 20).
fn check_cancel(cancel: &AtomicBool, stage: &BuildStage) -> Result<(), BuildError> {
    if cancel.load(Ordering::Relaxed) {
        warn!(?stage, "build cancelled before stage");
        return Err(BuildError::Cancelled(stage.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `BuildStage::cumulative_fraction` is monotone non-decreasing
    /// across the canonical run order. Pins the progress overlay
    /// behaviour: bar never moves backwards.
    #[test]
    fn cumulative_fractions_are_monotone() {
        let order = [
            BuildStage::RenderDiffuse,
            BuildStage::PrepareStaging,
            BuildStage::RenderMinimap,
            BuildStage::InvokePyMapConv,
            BuildStage::StageSplatAssets,
            BuildStage::EmitMapInfoLua,
            BuildStage::EmitMetalLayoutLua,
            BuildStage::EmitStartboxesLua,
            BuildStage::EmitFeaturePlacerLua,
            BuildStage::PackageSd7,
            BuildStage::InstallToBar,
            BuildStage::Done,
        ];
        let mut last = 0.0_f32;
        for s in &order {
            let f = s.cumulative_fraction();
            assert!(
                f >= last,
                "fraction not monotone: {:?} → {f}, last = {last}",
                s
            );
            assert!(
                (0.0..=1.0).contains(&f),
                "fraction out of range: {:?} → {f}",
                s
            );
            last = f;
        }
    }

    /// `check_cancel` returns `Err(BuildError::Cancelled(stage))`
    /// when the flag is set, and `Ok(())` otherwise.
    #[test]
    fn check_cancel_respects_flag() {
        let flag = AtomicBool::new(false);
        assert!(check_cancel(&flag, &BuildStage::PrepareStaging).is_ok());
        flag.store(true, Ordering::Relaxed);
        let err = check_cancel(&flag, &BuildStage::InvokePyMapConv).unwrap_err();
        match err {
            BuildError::Cancelled(s) => assert_eq!(s, BuildStage::InvokePyMapConv),
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    /// A closure sink emits events in order. Smoke for the
    /// `BuildEventSink` impl on `Fn(BuildEvent)`.
    #[test]
    fn closure_sink_collects_events_in_order() {
        let collected = Mutex::new(Vec::<BuildEvent>::new());
        let sink = |e: BuildEvent| {
            collected.lock().unwrap().push(e);
        };
        sink.emit(BuildEvent::Stage(BuildStage::PrepareStaging));
        sink.emit(BuildEvent::Log {
            line: "hello".into(),
            stream: LogStream::Stdout,
        });
        sink.emit(BuildEvent::Stage(BuildStage::Done));
        let got = collected.lock().unwrap();
        assert_eq!(got.len(), 3);
        match &got[0] {
            BuildEvent::Stage(s) => assert_eq!(*s, BuildStage::PrepareStaging),
            _ => panic!("event 0 was not Stage"),
        }
        match &got[2] {
            BuildEvent::Stage(s) => assert_eq!(*s, BuildStage::Done),
            _ => panic!("event 2 was not Stage(Done)"),
        }
    }

    /// `()` sink swallows everything without panicking. Used by the
    /// legacy `build_sd7` wrapper.
    #[test]
    fn unit_sink_is_no_op() {
        ().emit(BuildEvent::Stage(BuildStage::PrepareStaging));
        ().emit(BuildEvent::Log {
            line: "drop me".into(),
            stream: LogStream::Stderr,
        });
        ().emit(BuildEvent::Progress(0.5));
        // No panic, no observable state. The test passes by reaching here.
    }

    /// Sprint 20 / chunk 2 — `invoke_with_streaming` fires the
    /// per-line callback once per `\n`-separated chunk, in order,
    /// from BOTH streams.
    #[test]
    fn invoke_with_streaming_collects_lines_in_order() {
        let captured: Mutex<Vec<(String, LogStream)>> = Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(
            "printf 'line1\\nline2\\nline3\\n'; \
             printf 'err1\\nerr2\\n' 1>&2",
        );
        let out = invoke_with_streaming(
            &mut cmd,
            |line, stream| {
                captured.lock().unwrap().push((line.to_string(), stream));
            },
            &cancel,
        )
        .expect("subprocess should succeed");
        assert!(out.status.success(), "sh -c exit 0");
        // Captured strings are line-by-line; order within a single
        // stream is deterministic, but stdout vs stderr can interleave
        // — assert the per-stream sequences instead of exact ordering.
        let lines = captured.lock().unwrap();
        let stdout: Vec<&str> = lines
            .iter()
            .filter(|(_, s)| *s == LogStream::Stdout)
            .map(|(l, _)| l.as_str())
            .collect();
        let stderr: Vec<&str> = lines
            .iter()
            .filter(|(_, s)| *s == LogStream::Stderr)
            .map(|(l, _)| l.as_str())
            .collect();
        assert_eq!(stdout, vec!["line1", "line2", "line3"]);
        assert_eq!(stderr, vec!["err1", "err2"]);
        // Captured-after-exit shape is preserved for callers (e.g.
        // PyMapConv) that still post-process the full stream.
        assert!(out.stdout.contains("line1\n"));
        assert!(out.stderr.contains("err2\n"));
    }

    /// Sprint 20 / chunk 2 — cancelling mid-process kills the child
    /// and returns an `Interrupted` error within the poll window.
    /// Test command sleeps for a long time; cancel fires after a
    /// short delay; we assert the call returns quickly.
    #[test]
    fn invoke_with_streaming_cancels_long_running_child() {
        use std::sync::Arc;
        use std::thread;
        use std::time::Instant;

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_setter = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            cancel_setter.store(true, Ordering::Relaxed);
        });

        // Bypass the shell so kill() reaches the leaf process directly.
        // Wrapping in `sh -c` leaves the sleep as a grandchild that
        // inherits stdout — our reader thread would block on the
        // open pipe until the orphan finished. Sprint 20 documents
        // this as a known limitation (Child::kill is best-effort;
        // PyMapConv's Compressonator grandchildren die in ~5 s on
        // their own).
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let result = invoke_with_streaming(&mut cmd, |_, _| {}, &cancel);
        let elapsed = started.elapsed();
        // Should return within ~CANCEL_POLL_INTERVAL after the flag
        // fires (~120 ms + a few × 50 ms poll cycles). Generous bound
        // so a slow CI host doesn't flake.
        assert!(
            elapsed < Duration::from_secs(2),
            "cancel did not interrupt long-running child within 2s (took {elapsed:?})"
        );
        let err = result.expect_err("cancelled run should return Err");
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    }

    /// Stage labels are short enough to fit in the progress overlay's
    /// fixed-width line (~40 chars target). Catches accidental
    /// novella-length labels.
    #[test]
    fn stage_labels_fit_overlay_width() {
        let stages = [
            BuildStage::RenderDiffuse,
            BuildStage::PrepareStaging,
            BuildStage::RenderMinimap,
            BuildStage::InvokePyMapConv,
            BuildStage::StageSplatAssets,
            BuildStage::EmitMapInfoLua,
            BuildStage::PackageSd7,
            BuildStage::InstallToBar,
            BuildStage::Done,
        ];
        for s in &stages {
            let l = s.label();
            assert!(
                l.len() < 60,
                "label too long for overlay: {:?} → {l:?} ({} chars)",
                s,
                l.len()
            );
            assert!(!l.is_empty(), "label empty for {:?}", s);
        }
    }
}
