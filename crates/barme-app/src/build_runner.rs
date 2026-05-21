//! Sprint 20 — worker-thread driven build pipeline + UI-thread state
//! machine.
//!
//! The pre-Sprint-20 build path called `launcher::build_and_install`
//! synchronously, blocking the UI thread for 10–60 s on a 16-SMU
//! project. This module replaces that with:
//!
//! 1. An owned [`OwnedSlotResolver`] that can ride the worker thread
//!    (the App's `AppSlotResolver` borrows the slot registry slice,
//!    which doesn't satisfy `'static`).
//! 2. A [`BuildState`] enum tracked on `App`. Each variant carries
//!    the channels + log buffer + cancel flag + thread handle it
//!    needs.
//! 3. A [`BuildLogLine`] record + bounded `VecDeque<BuildLogLine>`
//!    ring buffer (cap 5000 lines) the UI thread drains from on each
//!    frame.
//! 4. A [`start_worker_thread`] free function that owns the
//!    `thread::spawn` boundary: it bakes the texture BMP, builds a
//!    `BuildPlan`, drives `plan.execute`, then `install_sd7`s the
//!    result.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use barme_core::{Heightmap, Project, SlotResolver};
use barme_pipeline::{
    BuildError, BuildEvent, BuildPlan, BuildStage, LayerSplatBakeInputs, LogStream,
    PyMapConvDriver, SplatBakeInputs, build,
};
use image::{ImageBuffer, Rgb};
use tempfile::TempDir;
use tracing::{info, warn};

/// Hard cap on the per-build log ring buffer. PyMapConv on a 16-SMU
/// map emits ~2 000–3 000 lines; 5 000 gives generous headroom while
/// keeping resident size at ~1 MB (200 chars × 5 000 lines).
pub const LOG_RING_CAP: usize = 5_000;

/// Max log lines drained from the events channel per UI frame. PITFALL
/// #4 in the Sprint 20 prompt — the worker can outpace UI repaint by
/// orders of magnitude during chatty PyMapConv stages; bounding the
/// drain keeps the frame budget honest.
pub const LOG_LINES_PER_FRAME: usize = 100;

/// One captured log line. Carries the stream tag so the build log
/// panel can tint stdout / stderr / info differently, plus a
/// timestamp for future "since last stage" displays.
///
/// `stream` and `captured_at` are read by Sprint 20 / chunk 5 (build
/// log panel — tints the line, displays the elapsed time since
/// stage start).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BuildLogLine {
    pub text: String,
    pub stream: LogStream,
    pub captured_at: Instant,
}

/// Build-pipeline state machine surfaced on `App`. The UI thread
/// matches on this each frame to drive the progress overlay, status
/// strip, and log panel.
///
/// Several fields are read only by Sprint 20 chunks 4–6 (progress
/// overlay reads `current_stage` + `latest_progress` + `cancel` +
/// `started_at`; status strip reads `project_name` + `duration` +
/// `sd7_path` + `error`; log panel reads `log`). The
/// `#[allow(dead_code)]` clears the chunk-3 warning without leaking
/// the placeholder via a downstream API change.
#[allow(dead_code)]
#[derive(Default)]
pub enum BuildState {
    #[default]
    Idle,
    Running {
        project_name: String,
        started_at: Instant,
        current_stage: BuildStage,
        latest_progress: f32,
        events: Receiver<BuildEvent>,
        log: Arc<Mutex<VecDeque<BuildLogLine>>>,
        cancel: Arc<AtomicBool>,
        thread: Option<JoinHandle<Result<PathBuf, String>>>,
    },
    Done {
        sd7_path: PathBuf,
        duration: Duration,
        log: Arc<Mutex<VecDeque<BuildLogLine>>>,
    },
    Failed {
        error: String,
        duration: Duration,
        log: Arc<Mutex<VecDeque<BuildLogLine>>>,
    },
    Cancelled {
        duration: Duration,
        log: Arc<Mutex<VecDeque<BuildLogLine>>>,
    },
}

impl BuildState {
    /// Convenience accessor for the log buffer regardless of state
    /// variant. `Idle` returns `None`; every other state owns a
    /// shared `Arc<Mutex<...>>` the panel can read.
    #[allow(dead_code)] // wired by Sprint 20 / chunk 5 (build log panel)
    pub fn log(&self) -> Option<&Arc<Mutex<VecDeque<BuildLogLine>>>> {
        match self {
            BuildState::Idle => None,
            BuildState::Running { log, .. }
            | BuildState::Done { log, .. }
            | BuildState::Failed { log, .. }
            | BuildState::Cancelled { log, .. } => Some(log),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, BuildState::Running { .. })
    }
}

/// Append `line` to `log`, evicting the oldest line when the buffer
/// is at [`LOG_RING_CAP`]. Public so the build_log panel's Clear
/// button can reuse the same lock pattern.
pub fn push_log_line(log: &Mutex<VecDeque<BuildLogLine>>, line: BuildLogLine) {
    let Ok(mut guard) = log.lock() else {
        warn!("build_runner: log mutex poisoned");
        return;
    };
    if guard.len() >= LOG_RING_CAP {
        guard.pop_front();
    }
    guard.push_back(line);
}

/// Owned `SlotResolver` for the worker thread. The App's
/// `AppSlotResolver<'a>` borrows the slot-registry slice, which means
/// it can't outlive the spawn point. This struct clones the registry
/// once (16 entries × ~100 B; ~2 KB) and owns its own project root.
pub struct OwnedSlotResolver {
    slots: Vec<OwnedSlotEntry>,
    project_root: Option<PathBuf>,
}

/// One slot in the owned registry. Mirrors enough of the app's
/// `SlotMeta` to compute the `diffuse.png` path on demand without
/// reaching back into the App.
pub struct OwnedSlotEntry {
    pub id: u8,
    pub dir: PathBuf,
}

impl OwnedSlotResolver {
    pub fn new(slots: Vec<OwnedSlotEntry>, project_root: Option<PathBuf>) -> Self {
        Self {
            slots,
            project_root,
        }
    }
}

impl SlotResolver for OwnedSlotResolver {
    fn diffuse_path(&self, slot_id: u8) -> Option<PathBuf> {
        self.slots
            .iter()
            .find(|s| s.id == slot_id)
            .map(|s| s.dir.join("diffuse.png"))
    }
    fn imported_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }
}

/// Everything the worker thread needs, owned. Constructed on the UI
/// thread under `App::start_build`, then moved into the worker via
/// `thread::spawn`.
pub struct WorkerInputs {
    pub driver: PyMapConvDriver,
    pub project: Project,
    pub heightmap_png: PathBuf,
    pub heightmap: Heightmap,
    pub splat_inputs: SplatBakeInputs,
    pub layer_inputs: Option<LayerSplatBakeInputs>,
    pub slot_resolver: Box<dyn SlotResolver + Send + Sync>,
    pub project_path: Option<PathBuf>,
    pub work_dir: TempDir,
    pub dst_dir: PathBuf,
    pub project_name: String,
}

/// Run the build pipeline on the calling thread (the worker). Bakes
/// the diffuse texture, drives `BuildPlan::execute`, then installs
/// the produced `.sd7` to the BAR maps dir. Emits stage / log events
/// via `sink` and respects `cancel` between every cancellation
/// checkpoint.
///
/// Returns the installed `.sd7` path on success; on failure (incl.
/// cancel) returns a formatted error string suitable for the
/// `BuildState::Failed { error, .. }` display.
pub fn run_worker_build(
    inputs: WorkerInputs,
    sink: &dyn build::BuildEventSink,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let WorkerInputs {
        driver,
        project,
        heightmap_png,
        heightmap,
        splat_inputs,
        layer_inputs,
        slot_resolver,
        project_path,
        work_dir,
        dst_dir,
        project_name,
    } = inputs;
    let work = work_dir.path().to_path_buf();

    // ─── Stage 0 (worker): RenderDiffuse ─────────────────────────
    sink.emit(BuildEvent::Stage(BuildStage::RenderDiffuse));
    sink.emit(BuildEvent::Log {
        line: "▸ Baking diffuse texture".into(),
        stream: LogStream::Info,
    });
    let texture_bmp = bake_texture_bmp(&project, &heightmap_png, &work, &*slot_resolver)
        .map_err(|e| format!("diffuse bake: {e}"))?;
    sink.emit(BuildEvent::Log {
        line: format!("diffuse → {}", texture_bmp.display()),
        stream: LogStream::Info,
    });

    // ─── Stages 1–9 (execute_stages): PrepareStaging … PackageSd7 ─
    let out_sd7 = work.join(format!("{project_name}.sd7"));
    let plan = BuildPlan {
        driver,
        project,
        heightmap_png,
        texture_bmp,
        splat_inputs,
        layer_inputs,
        heightmap: Some(heightmap),
        project_path,
        slot_resolver,
        work_dir: work.clone(),
        out_sd7: out_sd7.clone(),
    };
    let built = plan.execute(sink, cancel).map_err(|e| match e {
        BuildError::Cancelled(_) => format!("Cancelled: {e}"),
        other => format!("{other}"),
    })?;

    // ─── Stage 10 (worker): InstallToBar ─────────────────────────
    sink.emit(BuildEvent::Stage(BuildStage::InstallToBar));
    sink.emit(BuildEvent::Log {
        line: format!("▸ Installing to {}", dst_dir.display()),
        stream: LogStream::Info,
    });
    let installed =
        crate::launcher::install_sd7(&built, &dst_dir).map_err(|e| format!("install_sd7: {e}"))?;
    sink.emit(BuildEvent::Log {
        line: format!("installed → {}", installed.display()),
        stream: LogStream::Info,
    });
    sink.emit(BuildEvent::Stage(BuildStage::Done));
    // Keep `work_dir` alive until here so the TempDir's `Drop`
    // cleans up only AFTER the .sd7 is copied to its final location.
    drop(work_dir);
    Ok(installed)
}

/// Bake the diffuse texture BMP that PyMapConv consumes. Branches on
/// the project's layer-stack state:
///
/// - non-empty stack → `LayerStack::bake_diffuse` (the canonical
///   path; ADR-038)
/// - empty stack    → `synth_biome_bmp` fallback (used by smoke
///   binaries; height-keyed gradient)
fn bake_texture_bmp(
    project: &Project,
    heightmap_png: &Path,
    work: &Path,
    slot_resolver: &dyn SlotResolver,
) -> Result<PathBuf, String> {
    let (tw, th) = project.size.texture_dims();
    if !project.layers.layers.is_empty() {
        let path = work.join("layered_diffuse.bmp");
        info!(
            width = tw,
            height = th,
            layers = project.layers.layers.len(),
            "build_runner: baking diffuse from layer stack"
        );
        let img = project.layers.bake_diffuse(project.size, slot_resolver);
        img.save(&path).map_err(|e| e.to_string())?;
        Ok(path)
    } else {
        let path = work.join("synth_biome.bmp");
        info!(
            width = tw,
            height = th,
            "build_runner: baking fallback biome texture (empty layer stack)"
        );
        synth_biome_bmp(heightmap_png, &path, tw, th).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

/// Biome ramp matching `launcher::biome_ramp` + the editor's WGSL
/// fallback. Duplicated here so the worker thread doesn't reach back
/// into `launcher`'s private function set.
fn biome_ramp(t: f32) -> [f32; 3] {
    if t < 0.05 {
        [0.227, 0.451, 0.604]
    } else if t < 0.50 {
        [0.451, 0.616, 0.392]
    } else if t < 0.82 {
        [0.502, 0.486, 0.439]
    } else {
        [0.863, 0.878, 0.902]
    }
}

/// Worker-thread copy of `launcher::synth_biome_bmp`. Same math;
/// avoids exposing the private fn so the legacy synchronous
/// `launcher::build_and_install` keeps its self-contained shape.
fn synth_biome_bmp(
    heightmap_png: &Path,
    path: &Path,
    w: u32,
    h: u32,
) -> Result<(), image::ImageError> {
    let hm = image::open(heightmap_png)?.into_luma16();
    let hm_w = hm.width();
    let hm_h = hm.height();
    if hm_w == 0 || hm_h == 0 {
        return Err(image::ImageError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "heightmap PNG has zero dimensions",
        )));
    }
    let mut buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(w, h);
    let denom_x = (w - 1).max(1) as u64;
    let denom_y = (h - 1).max(1) as u64;
    let hm_last_x = (hm_w - 1) as u64;
    let hm_last_y = (hm_h - 1) as u64;
    for (tx, ty, p) in buf.enumerate_pixels_mut() {
        let hx = (tx as u64 * hm_last_x / denom_x) as u32;
        let hy = (ty as u64 * hm_last_y / denom_y) as u32;
        let pixel = hm.get_pixel(hx, hy);
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

/// Drain up to [`LOG_LINES_PER_FRAME`] events off `events` into
/// `log`, updating `current_stage` and `latest_progress` along the
/// way. Returns the (stage, progress) tuple for the caller to mirror
/// onto the `BuildState::Running` variant.
///
/// Returns `None` when the channel has hung up (the worker dropped
/// its sender end → the join handle is ready to be reaped).
pub fn drain_events(
    events: &Receiver<BuildEvent>,
    log: &Mutex<VecDeque<BuildLogLine>>,
    current_stage: &mut BuildStage,
    latest_progress: &mut f32,
) -> Result<(), ChannelClosed> {
    for _ in 0..LOG_LINES_PER_FRAME {
        match events.try_recv() {
            Ok(BuildEvent::Stage(s)) => {
                *current_stage = s.clone();
                *latest_progress = s.cumulative_fraction();
            }
            Ok(BuildEvent::Log { line, stream }) => push_log_line(
                log,
                BuildLogLine {
                    text: line,
                    stream,
                    captured_at: Instant::now(),
                },
            ),
            Ok(BuildEvent::Progress(p)) => {
                *latest_progress = (*latest_progress).max(p.clamp(0.0, 1.0));
            }
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => return Err(ChannelClosed),
        }
    }
    Ok(())
}

/// Sentinel returned by [`drain_events`] when the worker has dropped
/// its `Sender` — the App should then `join()` the thread handle.
pub struct ChannelClosed;

/// Adapter so a closure `|e| sender.send(e)` satisfies the pipeline's
/// [`build::BuildEventSink`] trait without requiring a Sync bound at
/// the call site. The Sender itself is Sync (modern std::mpsc), and
/// this struct's Sync-ness is implied by `T: Send`.
pub struct ChannelSink {
    tx: Sender<BuildEvent>,
}

impl ChannelSink {
    pub fn new(tx: Sender<BuildEvent>) -> Self {
        Self { tx }
    }
}

impl build::BuildEventSink for ChannelSink {
    fn emit(&self, event: BuildEvent) {
        // Send returns Err only if the receiver dropped — which
        // means the UI thread already tore down the run. Log + swallow.
        if self.tx.send(event).is_err() {
            warn!("build_runner: channel send failed (receiver dropped)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_pipeline::BuildEventSink;
    use std::sync::mpsc;

    /// `push_log_line` keeps the ring at <= LOG_RING_CAP after many
    /// inserts. Oldest lines drop off the front.
    #[test]
    fn ring_buffer_caps_at_5000() {
        let log = Mutex::new(VecDeque::<BuildLogLine>::with_capacity(LOG_RING_CAP));
        for i in 0..LOG_RING_CAP + 50 {
            push_log_line(
                &log,
                BuildLogLine {
                    text: format!("line {i}"),
                    stream: LogStream::Stdout,
                    captured_at: Instant::now(),
                },
            );
        }
        let g = log.lock().unwrap();
        assert_eq!(g.len(), LOG_RING_CAP);
        // First line is `line 50` (the first 50 dropped).
        assert_eq!(g.front().unwrap().text, "line 50");
        assert_eq!(
            g.back().unwrap().text,
            format!("line {}", LOG_RING_CAP + 49)
        );
    }

    /// `drain_events` updates the stage + progress, and stops at the
    /// frame cap.
    #[test]
    fn drain_events_updates_state_within_budget() {
        let (tx, rx) = mpsc::channel::<BuildEvent>();
        let log = Mutex::new(VecDeque::<BuildLogLine>::new());

        // Send 150 lines (above the 100-per-frame cap).
        for i in 0..150 {
            tx.send(BuildEvent::Log {
                line: format!("line {i}"),
                stream: LogStream::Stdout,
            })
            .unwrap();
        }
        // Drop tx after pushing so the receiver only sees the queued
        // items + then Disconnected.
        drop(tx);

        let mut stage = BuildStage::RenderDiffuse;
        let mut prog = 0.0_f32;
        let res = drain_events(&rx, &log, &mut stage, &mut prog);
        // First call: 100 lines drained, no Disconnected yet.
        assert!(res.is_ok());
        assert_eq!(log.lock().unwrap().len(), 100);

        // Second call: 50 more lines drained, then Disconnected.
        let res = drain_events(&rx, &log, &mut stage, &mut prog);
        assert!(res.is_err()); // ChannelClosed
        assert_eq!(log.lock().unwrap().len(), 150);
    }

    /// `OwnedSlotResolver` resolves bound slots and ignores unbound.
    #[test]
    fn owned_slot_resolver_resolves_and_falls_back() {
        let r = OwnedSlotResolver::new(
            vec![
                OwnedSlotEntry {
                    id: 0,
                    dir: PathBuf::from("/textures/00-grass"),
                },
                OwnedSlotEntry {
                    id: 5,
                    dir: PathBuf::from("/textures/05-rock"),
                },
            ],
            Some(PathBuf::from("/projects/mymap")),
        );
        assert_eq!(
            r.diffuse_path(0),
            Some(PathBuf::from("/textures/00-grass/diffuse.png"))
        );
        assert_eq!(
            r.diffuse_path(5),
            Some(PathBuf::from("/textures/05-rock/diffuse.png"))
        );
        assert_eq!(r.diffuse_path(99), None);
        assert_eq!(r.imported_root(), Some(Path::new("/projects/mymap")));
    }

    /// `ChannelSink` forwards events on the wrapped Sender.
    #[test]
    fn channel_sink_forwards_events() {
        let (tx, rx) = mpsc::channel::<BuildEvent>();
        let sink = ChannelSink::new(tx);
        sink.emit(BuildEvent::Stage(BuildStage::PrepareStaging));
        sink.emit(BuildEvent::Log {
            line: "hi".into(),
            stream: LogStream::Info,
        });
        match rx.recv().unwrap() {
            BuildEvent::Stage(s) => assert_eq!(s, BuildStage::PrepareStaging),
            other => panic!("expected Stage, got {other:?}"),
        }
        match rx.recv().unwrap() {
            BuildEvent::Log { line, .. } => assert_eq!(line, "hi"),
            other => panic!("expected Log, got {other:?}"),
        }
    }
}
