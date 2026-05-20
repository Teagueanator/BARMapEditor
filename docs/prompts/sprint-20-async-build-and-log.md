# Sprint 20 — Async build pipeline + in-app log + recent projects (U3 + F11 polish)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 20** — the second of three focused UI/UX polish
sprints (19 / 20 / 22). It is a **feedback and reliability** sprint
that closes the largest remaining UX gap: today, clicking "Build &
install" freezes the UI thread for 10-60 s while PyMapConv +
Compressonator + 7z run synchronously. A user with no terminal sees
a hung editor.

After this sprint:

- `build_and_install` runs on a worker thread; the UI stays
  responsive throughout.
- A new in-app **build log panel** shows live stdout/stderr from
  subprocesses (PyMapConv quirks like `exit 1 on success` become
  visible in context).
- A **progress overlay** displays the current stage with a Cancel
  button.
- The status strip live-updates with the build stage.
- On failure, the log panel auto-opens with the tail of stderr +
  a "Copy log" button.
- A **recent projects** list lands in the File menu and the empty-
  state CTA.
- A "save before build" guard warns when the project is dirty.

This sprint is the foundation for F11 (one-click `.sd7` build with
visible feedback) and a prerequisite for F12 (Sprint 32, Launch in
BAR) — F12 also needs async invocation.

**Prerequisites:**
- Sprint 19 (UI tooltip + help-text pass + validation chip click +
  status wiring) MUST be ticked. The progress overlay and log panel
  reuse the `help_text.rs` catalogue and the panel-window patterns
  Sprint 19 established.
- Sprint 18 (minimap + F9 form) MUST be ticked. The minimap render
  is the first stage of the new pipeline; sequencing it with
  PyMapConv requires the headless device from Sprint 18 to be in
  place.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 NFR-Crash-safety
   (60s autosave is aspirational; Sprint 32 lands it but the
   recent-projects work here is a partial mitigation), §3.2 F11
   (build + install + visible progress).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §1 (heap
   budget — the build pipeline writes ~hundreds of MB to a temp
   dir; reading it back into RAM for the log would blow the
   budget). The log is a **bounded ring buffer**, not a full
   copy.
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/launcher.rs`
   — current `build_and_install` synchronous entry point. This is
   what we move onto a worker thread.
5. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/pymapconv.rs`
   — the subprocess wrapper. Today it captures stdout/stderr post-
   exit. We extend with line-by-line streaming via a thread that
   reads from `Child::stdout` / `stderr` `BufReader`.
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/sd7.rs`
   — the orchestrator. Refactor into a `BuildSteps` iterator so
   each step can emit a progress event.
7. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/dnts.rs`
   — DNTS bake; one of the stages.
8. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/splat_pipeline.rs`
   — splat assets staging; another stage.
9. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/minimap.rs`
   (Sprint 18) — minimap render is the first stage.
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/config.rs`
    — `EditorConfig`. Add `recent_projects: Vec<PathBuf>` (capped
    at 10) here.
11. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/lint_panel.rs`
    (Sprint 19 stub) — pattern-match: docked/floating Window with
    a header + body + footer. The build log uses the same pattern.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-async-build-pipeline
./devlog/log.sh new stage-1-recent-projects
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. `BuildSteps` iterator + per-stage events

**New module:** `crates/barme-pipeline/src/build.rs` (or extend
`sd7.rs`). Replace the monolithic `build_and_install` body with a
stepwise iterator:

```rust
pub enum BuildStage {
    PrepareStaging,
    RenderMinimap,
    BakeDnts { slot_index: usize, total_slots: usize },
    StageSplatAssets,
    EmitMapInfoLua,
    EmitMetalLayoutLua,
    EmitStartboxesLua,
    EmitFeaturePlacerLua,
    BakeDiffuse,
    InvokePyMapConv,
    PackageSd7,
    InstallToBar,
    Done,
}

pub struct BuildPlan { ... }

impl BuildPlan {
    pub fn execute(
        self,
        events: &dyn Fn(BuildEvent),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, BuildError>;
}

pub enum BuildEvent {
    Stage(BuildStage),
    Log { line: String, stream: LogStream },
    Progress(f32),  // 0.0..=1.0 for sub-stage progress (DNTS bake fraction)
}
```

The `execute` method is the single synchronous entry point that
runs on the worker thread. It calls `events(BuildEvent::Stage(...))`
before each stage and `events(BuildEvent::Log { ... })` whenever a
subprocess emits a line. Cancellation is cooperative — `execute`
checks `cancel.load(Ordering::Relaxed)` between stages.

### 2. Subprocess line streaming

**Extend `pymapconv.rs`** + **`dnts.rs`** to use line-streaming
instead of post-exit capture:

```rust
pub fn invoke_with_streaming(
    cmd: &mut Command,
    on_line: &dyn Fn(String, LogStream),
    cancel: &AtomicBool,
) -> Result<ExitStatus, IoError>;
```

Spawn `cmd`, attach `Stdio::piped()` for stdout/stderr, spawn a
reader thread per stream that calls `on_line` for each
`BufReader::lines()` item. The reader threads exit when the
streams close. Joining at the end ensures all output is captured
before we report exit status.

PyMapConv's "exit 1 on success" quirk (documented in
`devlog/stage-0-validation/...`) is normalised here: if exit
code != 0 BUT stdout contains the magic success line, treat as
success. The log captures this for the user to inspect.

### 3. `BuildState` + worker-thread orchestration in `barme-app`

**Refactor `launcher.rs`:**

```rust
pub enum BuildState {
    Idle,
    Running {
        plan: Arc<BuildPlan>,
        started_at: Instant,
        current_stage: BuildStage,
        log: Arc<Mutex<VecDeque<LogLine>>>,  // ring, cap 5000 lines
        cancel: Arc<AtomicBool>,
        thread: thread::JoinHandle<Result<PathBuf, BuildError>>,
    },
    Done {
        sd7_path: PathBuf,
        duration: Duration,
        log: Arc<Mutex<VecDeque<LogLine>>>,
    },
    Failed {
        error: BuildError,
        log: Arc<Mutex<VecDeque<LogLine>>>,
    },
}
```

The App holds `BuildState` in its main loop. Events from the
worker thread are sent via an `mpsc::channel<BuildEvent>`. The
App's `update` polls the channel each frame (non-blocking) and
mutates `BuildState` accordingly. Log lines append to the bounded
`VecDeque`.

The worker thread:
1. Builds the `BuildPlan` from the project snapshot (NOT shared
   `&Project` — clone or take an owned snapshot at thread spawn).
2. Calls `plan.execute(events_fn, cancel_flag)`.
3. Sends the final `Result<PathBuf, BuildError>` via a oneshot
   channel.

### 4. Build progress overlay + Cancel button

**New module:** `crates/barme-app/src/ui/build_overlay.rs`. A
modal-ish overlay (uses `egui::Area::new(...).order(Foreground)`)
shown when `BuildState::Running`. Layout:

```
┌──────────────────────────────────────────────┐
│ Building map_v3.sd7                          │
│                                              │
│ Stage: Baking DNTS slot 3 of 8               │
│ [▓▓▓▓▓▓▓░░░░░] 47%                          │
│                                              │
│ Elapsed: 0:14                                │
│                                              │
│  [Cancel]  [View log…]                       │
└──────────────────────────────────────────────┘
```

Cancel: sets the `AtomicBool` flag. The plan polls between stages;
the active subprocess (PyMapConv, Compressonator) is killed via
`Child::kill()` from the worker thread when cancel fires mid-step.

### 5. In-app build log panel

**New module:** `crates/barme-app/src/ui/build_log.rs`.
`pub fn build_log_window(ctx: &egui::Context, state: &mut
BuildState, open: &mut bool)`. Reuses the `LintPanel` Window
pattern from Sprint 19.

Layout:
- Header: build state summary (Idle / Running stage / Done in 14s /
  Failed: PyMapConv exit 1).
- Body: scrollable `TextEdit::multiline` showing the ring buffer.
  Auto-scroll to bottom when new lines arrive. Tinted by stream
  (stdout = `t.text`, stderr = `t.warn`).
- Footer: `[Copy log]` button (copies to clipboard via
  `arboard`), `[Save log…]` button (file picker → `.log` file),
  `[Clear]` (drops the ring buffer).

The log panel is reachable from:
- The progress overlay's "View log…" button.
- The status strip's last-result chip (Sprint 19 wires it).
- A new top-bar `Build > Show log` menu item.
- Auto-opens on `BuildState::Failed`.

### 6. Status strip live updates

Extend `main.rs::status_strip`:
- During `BuildState::Running`: render "Building: Baking DNTS
  slot 3 of 8…" with a small spinner.
- During `BuildState::Done`: render "✓ map_v3.sd7 in 14s" with
  a click-to-show-log affordance.
- During `BuildState::Failed`: render "✗ Build failed: …" in
  red with the same click affordance.

### 7. Recent projects

**`EditorConfig`** extension (`config.rs`):
```rust
pub struct EditorConfig {
    // ... existing fields ...
    pub recent_projects: VecDeque<PathBuf>,  // capped at 10
}
```
- On `File > Open` success: push path to front; dedupe; truncate
  to 10. Persist.
- On `File > Save As`: same.
- On project file missing at open: remove from list silently;
  surface a single toast (`last_error` channel for now;
  Sprint 31 replaces with proper toasts).

**File menu** (`main.rs::top_bar`):
- Add a `Recent projects ▶` submenu under `File`. Lists the
  10 paths. Hover shows the full path; click loads it. Bottom
  of submenu: `Clear recent projects`.

**Empty-state CTA** (`ui/viewport_chrome.rs:316`):
- Below the existing `[Open…]` button, add a "Recent projects:"
  section listing up to 5 paths. Each row clickable.

### 8. Save-before-build guard + disk-space check

**Save-before-build** (`launcher.rs`):
- If `App::dirty == true` when the user clicks Build:
  - Open a confirmation modal (use Sprint 19's stub Window
    pattern; Sprint 31 promotes to a proper modal primitive):
    "Save before building?" with `[Save & build]` / `[Build
    without saving]` / `[Cancel]`.
  - Save & build: call the existing save path then proceed.
  - Build without saving: proceed; the in-memory snapshot is
    what builds.

**Disk-space check** (`build.rs`):
- Before `tempfile::tempdir()`, check free space via `fs2::available_space`
  or similar. If <2 GB, emit a `BuildStage::PrepareStaging` warning
  log line and proceed (don't gate — the user might know better).
  Add `fs2` to workspace deps.

### 9. Tests + smoke run + rollup

- **Unit test** (`build.rs::tests`): `BuildPlan::execute` with a
  fake stage list + assertion that events fire in order.
- **Cancellation test**: spawn a stub plan that loops on a sleep;
  set the cancel flag; assert the plan returns within 100 ms.
- **Streaming test** (`pymapconv.rs::tests`): pipe a `printf
  "line1\\nline2\\n"` shell command through `invoke_with_streaming`;
  assert both lines arrive on the `on_line` callback.
- **Recent-projects round-trip**: open / save-as cycle persists +
  reloads; missing file silently removed.
- **Manual smoke** (record in devlog):
  - Build a default project → progress overlay shows stages →
    log panel shows PyMapConv output → status strip ends with
    "✓ map.sd7 in Ns".
  - Click Cancel mid-build → status strip shows "Cancelled" →
    subprocesses killed (verify via `ps`).
  - Force a PyMapConv failure (e.g. corrupted heightmap) →
    log panel auto-opens with stderr tail.
  - Open File > Recent projects → list correct.
  - With unsaved changes, click Build → confirmation modal.

- **Rollup commit**: STATUS UPDATEs in SRS / ROADMAP (U3 + F11
  partial — the build progress UI is now visible); closing
  devlog log; "Sprint 21 = Lint My Map (C8)" handoff note.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on stage transitions
with `duration_ms` since last stage. `trace!` on per-line log
events (high volume; gated behind a feature flag if needed).
`warn!` on disk-space < 2 GB. `error!` on plan-execute failures.

## Step 5 — Out of scope

- **Async procgen apply** — Sprint 24 (T2 / multithreading).
  Procgen stays synchronous here.
- **Full async F12 Launch in BAR** — Sprint 32. Builds the
  pattern this sprint's worker-thread architecture pioneers.
- **Toast queue / proper confirmation modals** — Sprint 31. The
  "save before build" guard uses a stub Window for now.
- **Build log persistence across editor sessions** — log lives
  in memory only; closing the panel + restarting the editor
  loses it. (`Save log…` to file is the escape hatch.)
- **Multi-build queue** (build 2 projects in sequence) — out of
  scope; one build at a time.
- **NFR-Crash safety / autosave 60s** — Sprint 32.

## Step 6 — Critical pitfalls (read twice)

1. **Worker thread does NOT hold `&Project`.** Snapshot the
   project (clone) at thread spawn. The main thread keeps
   mutating; if the worker reads through a shared reference
   you'll hit `RwLock` contention or worse. The clone cost
   on a 16-SMU project is ~50 MB / ~10 ms — acceptable.

2. **Cancel must not corrupt staging.** Cancellation between
   stages is safe; cancellation MID-subprocess kills PyMapConv
   which may leave partial files in the temp dir. The cleanup
   path always `drop`s the `tempfile::TempDir` so the OS
   removes it. **Don't share the temp dir across builds.**

3. **Log ring buffer is bounded.** 5000 lines × ~200 chars =
   ~1 MB resident. A PyMapConv run on a 16-SMU map emits
   ~2000-3000 lines; you have headroom. **Never** capture
   unbounded stdout into a `String` — that's an OOM waiting
   to happen.

4. **`mpsc::channel` event delivery**: the worker can outpace
   the UI repaint. The channel is unbounded; bursts of log
   lines accumulate. Drain in the App `update()` with a
   `try_recv` loop bounded by, say, 100 lines/frame to avoid
   stalling the frame budget.

5. **PyMapConv's `exit 1 on success` quirk**: only the
   `pymapconv.rs` wrapper knows about it. Don't leak this
   into `build.rs`'s control flow; normalise at the
   subprocess boundary.

6. **Child::kill is best-effort.** PyMapConv may spawn
   Compressonator subprocesses; `kill()` only kills the
   parent. On Linux use `nix::sys::signal::kill(-pid, SIGTERM)`
   to kill the process group. On Windows use
   `JobObject`. Document the trade-off; for Sprint 20 a
   best-effort `Child::kill` is fine — leftover Compressonator
   processes die within ~5s on their own.

7. **Save-before-build modal is a stub**. Use `egui::Window`
   with `.collapsible(false).resizable(false)`. Sprint 31
   replaces with a proper modal primitive — keep the API
   minimal so the swap is mechanical.

8. **Recent projects list MUST handle missing files
   gracefully.** A path that no longer exists silently drops
   from the list at next open; do NOT pop a dialog on every
   missing entry. The single `last_error` line communicates
   the issue.

9. **`tempfile` vs persistent staging.** The build pipeline
   already uses `tempfile::tempdir()` — keep it. The log
   panel reads the ring buffer in RAM, not the temp files.
   On `Done`, the `.sd7` is moved out of the temp dir to its
   final destination; the temp dir auto-cleans.

10. **Frame-budget integrity.** The progress overlay must
    repaint at 60 fps (or whatever the user has set). Don't
    block the UI thread on the worker's channel — use
    `ctx.request_repaint_after(Duration::from_millis(100))`
    while a build is running so the spinner animates without
    pegging the CPU.

11. **Cross-platform process-kill**: see pitfall #6. On
    Windows, `Child::kill` does NOT kill grandchildren by
    default. Document this and prefer `taskkill /F /T /PID`
    if needed (Stage 2 polish).

## Step 7 — Exit criteria

- 5-7 commits on `main` (per-chunk above + rollup).
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (U3 + F11 progress UI shipped).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Default project builds without freezing UI; progress overlay
    shows stages; log panel streams output.
  - Cancel mid-build kills subprocesses + cleans temp dir.
  - Recent projects list persists across editor restarts.
  - Save-before-build guard fires on dirty state.
  - Status strip "Building…" + "Done" + "Failed" each render
    correctly.
- Final devlog log: summary + "Sprint 21 = Lint My Map (C8)"
  handoff note.

Start by carving the `BuildPlan` iterator out of the existing
`build_and_install` body — without changing behaviour, just
restructure. Then the worker-thread + channels lift on top, then
the UI panels. Land each as a separate commit so a regression is
bisectable.
