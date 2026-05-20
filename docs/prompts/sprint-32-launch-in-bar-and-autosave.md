# Sprint 32 — F12 Launch in BAR + autosave 60s (NFR-Crash-safety) (T5)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 32** — closes two MVP loose ends:

1. **F12 Launch in BAR** — the long-deferred "Launch in BAR" button
   that invokes the user's installed Recoil with `--map <path>`
   pointing at the freshly built `.sd7`. The combobox slot in the
   top-bar split-button has been permanently greyed since Sprint 3
   / B4; this sprint enables it.

2. **NFR-Crash-safety: 60s autosave** — SRS commits to "Autosave
   every 60s" as MVP-grade. Sprint 0 onwards has been aspirational
   on this; ROADMAP line 788 admits it's not implemented. A pre-
   launch crash today loses everything since the last manual Save.

After this sprint, the editor is **MVP-complete except for the
external Beherith acceptance review** (ROADMAP line 495). Stage 1
internal work is fully closed.

**Prerequisites:**
- Sprint 20 (async build pipeline) MUST be ticked. F12 launches
  the result of a recent build; reuses the build worker thread.
- Sprint 31 (toast queue + confirm modals) MUST be ticked. F12
  surfaces success/failure as toasts; autosave silently emits an
  info toast.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F12 (Launch in BAR
   button), NFR-Crash-safety (60s autosave).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §9 (.sd7
   non-solid; doesn't apply here since Sprint 5 ships it correctly).
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/launcher.rs`
   (Sprint 20 refactored) — the `BuildState` machine; extend with
   `LaunchState`.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::top_bar`
   — the Build & Install split-button with the greyed Launch slot.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/config.rs`
   — `EditorConfig`. Add `bar_executable_path: Option<PathBuf>` +
   `autosave_interval_secs: u64` (default 60).
7. **Beherith resource**: the burnhamrobertp map-format gist (see
   `feedback_bar_chobby_repos` memory for the link). Confirms the
   `--map` flag pattern.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-32-launch-in-bar
./devlog/log.sh new sprint-32-autosave
```

## Step 3 — Scope

One commit per item:

### 1. BAR executable discovery

`crates/barme-app/src/launcher.rs`:

```rust
pub fn discover_bar_executable() -> Option<PathBuf> {
    // Strategy:
    // 1. Check EditorConfig.bar_executable_path; return if exists.
    // 2. Check common install paths:
    //    - Linux: ~/.local/share/spring/games/BAR/Beyond-All-Reason
    //    - Linux: /usr/games/spring (system-installed)
    //    - Linux: ~/Programs/Beyond-All-Reason/...
    //    - Windows: C:\Program Files (x86)\Beyond-All-Reason\...
    //    - macOS: /Applications/Beyond-All-Reason.app/Contents/MacOS/...
    // 3. Return None if not found.
    ...
}
```

If discovery fails, the F12 button's hover tooltip prompts the
user to set the path in `File > Settings > BAR executable`. The
F9 form (Sprint 18) doesn't surface this — it's session config,
not project config.

### 2. F12 Launch button

`crates/barme-app/src/main.rs::top_bar`:
- Un-grey the Launch combobox slot.
- `Launch` triggers:
  1. If no recent build OR build is stale (project dirty since
     build): toast warn "Build map first".
  2. If `bar_executable_path` not set + discovery fails: open
     a confirmation modal with a path picker.
  3. Spawn `Command::new(bar_path).arg("--map").arg(sd7_path).spawn()`.
  4. Toast info "Launched BAR — game window should appear".

**Implementation note**: Recoil's `--map` flag launches the editor
into Skirmish mode with that map selected. The user still has to
configure factions / AIs and click Start, but the map's loaded.

### 3. Launch state machine

```rust
pub enum LaunchState {
    Idle,
    Launching { started_at: Instant, pid: u32 },
    Failed { error: io::Error },
}
```

Status strip surfaces "Launching BAR…" then clears. If the
launched process exits within 5s, treat as failure (toast error
with the exit status).

Don't wait for BAR to exit. Detach the process; the editor stays
usable.

### 4. Autosave timer + path

```rust
pub struct AutosavePolicy {
    pub interval: Duration,    // 60s default
    pub max_backups: usize,    // 5 default
    pub directory: PathBuf,    // <project>/.barme-autosave/
}

impl App {
    pub fn maybe_autosave(&mut self, ctx: &egui::Context) {
        if !self.dirty { return; }
        if self.last_autosave.elapsed() < self.autosave_policy.interval {
            return;
        }
        let backup_path = self.compute_autosave_path();
        match self.write_project_to(&backup_path) {
            Ok(_) => {
                self.last_autosave = Instant::now();
                self.toast_info(format!("Autosaved to {}", backup_path.display()));
            }
            Err(e) => {
                self.toast_warn(format!("Autosave failed: {}", e));
            }
        }
        self.prune_old_autosaves();
    }
}
```

Autosave path:
`<project_dir>/.barme-autosave/<project_name>-<timestamp>.barmeproj`

Max 5 backups; older deleted. The `.barme-autosave/` directory is
hidden by convention (dot-prefix). Add to `.gitignore` template
if we generate one.

**Trigger**: `App::update()` calls `maybe_autosave(ctx)` every
frame. The interval check guards against per-frame thrashing.

### 5. Recovery UI

On project load, scan for a sibling `.barme-autosave/` directory.
If the newest autosave timestamp is newer than the main file's
mtime, surface a confirm modal: "An autosave from {N} minutes
ago is newer than the loaded file. Recover from autosave?"

- **Yes**: load from autosave; mark dirty.
- **No**: load main file; keep autosave for now (don't delete).
- **Discard autosave**: load main; delete autosaves.

### 6. Settings UI: BAR path + autosave interval

`File > Settings` (new menu item):
- Opens a settings Window.
- Shows BAR executable path with `[Browse…]` button.
- Autosave interval (DragValue, range 0..=600 seconds; 0 = off).
- Max autosave backups (DragValue, range 1..=20).
- Reset settings button (asks for confirmation).

Persists to `EditorConfig`.

### 7. Tests + smoke + rollup

- **Discovery test**: pin a fake BAR install via temp dir + env
  var; verify discovery finds it.
- **Autosave test**: enable autosave with 100ms interval (test
  override); make a dirty edit; advance time; assert the
  autosave file exists with the right content.
- **Recovery test**: fixture project with autosave timestamps;
  load → recovery prompt appears.
- **Pruning test**: 6 autosaves → 1 deleted; oldest goes first.

- **Smoke test**:
  - Set BAR path → click Build → click Launch → BAR opens with
    map loaded.
  - Edit project → wait 60s → autosave fires; status info toast.
  - Crash the editor (`kill -9`); reload → recovery prompt.

- **Rollup**: STATUS UPDATEs in SRS / ROADMAP (F12 done +
  NFR-Crash-safety honored; Stage 1 internal work
  COMPLETE). closing devlog logs. "Sprint 33 = NFR/CI gates
  (MSRV matrix + Windows + AppImage + determinism)" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on launch with bar_path
+ sd7_path; `info!` on autosave write with path + duration;
`warn!` on autosave failures; `error!` on launch failures.

## Step 5 — Out of scope

- **Watching BAR for in-game events** (e.g., "user crashed in BAR
  — return to editor with crash report") — Stage 2 stretch.
- **Multi-instance launch** (launching multiple BAR sessions
  side-by-side) — not supported.
- **Autosave to a separate background thread** — synchronous OK
  for now since save is <100ms even on 16-SMU.
- **Autosave compression** — TOML files compress poorly; raw
  is fine.
- **Cloud sync of autosaves** — Stage 3+ stretch.

## Step 6 — Critical pitfalls (read twice)

1. **BAR's `--map` flag pattern**: verify the exact flag by
   running `BAR --help` locally or reading
   `RecoilEngine/rts/System/CommandLine.cpp`. The flag may be
   `--map`, `-m`, or `--mapname`. Test before merging.

2. **`Command::spawn` vs `output`**: spawn detaches; output
   blocks until exit. Use spawn for launch — we don't want to
   wait for BAR to finish.

3. **Process exit detection**: poll the spawned child via
   `child.try_wait()` once per frame. If it exits within 5s,
   that's a sign BAR failed to launch (e.g., wrong arg, missing
   config). Surface as error toast.

4. **Path with spaces**: on Linux, `Command::arg(path)` handles
   spaces correctly. On Windows, the path may need quoting.
   Use `Command::arg(path.as_os_str())` which preserves the
   raw path bytes.

5. **macOS .app bundle**: BAR on macOS is an .app bundle.
   Launching is `open -a Beyond-All-Reason --args --map <path>`
   not the direct binary. Document the platform difference.

6. **Autosave timer drift**: the 60s interval is wall-time, not
   render-time. Use `Instant::now()` not frame count. Test by
   leaving the editor idle for 90s; autosave should fire.

7. **Autosave thrashing**: don't autosave on every dirty bit.
   The interval check guards. Also: don't autosave during a
   build (the project state may be in transition).

8. **Autosave race with manual save**: if the user clicks Save
   while autosave is mid-write, atomic file replace (write to
   temp + rename) protects against corruption. Use
   `tempfile::NamedTempFile::persist` for the write.

9. **Recovery prompt timing**: it fires on `Project::load_from`.
   The autosave-newer check is mtime-based; clocks can skew.
   Use a 5s grace window — only prompt if autosave is >5s
   newer than main.

10. **`.barme-autosave/` directory creation**: create lazily on
    first autosave write. Test on a read-only project directory
    — emit a clear error toast.

11. **Autosave during the build pipeline**: Sprint 20's worker
    thread snapshots the project. The main thread can autosave
    while the worker is building — no conflict. Test the case.

12. **F12 + Sprint 21 build-gating interaction**: if lint has
    errors, Build is greyed; therefore there's no recent valid
    build to launch. F12 should also grey in that state. Use
    the same `lint_summary.has_errors` check.

## Step 7 — Exit criteria

- 6+ commits on `main`: discovery, F12 button + state, autosave
  policy + writer, recovery UI, settings UI, rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (F12 + NFR-Crash-safety done;
  Stage 1 internal work COMPLETE).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Discover BAR on a Linux dev box; F12 launches with map.
  - Manual BAR path setting persists across editor restarts.
  - Autosave fires on dirty timer.
  - Recovery prompts after simulated crash.
- Final devlog: summary + Stage 1 internal-work-complete
  announcement + "Sprint 33 = NFR/CI gates" handoff.

Start by reading `RecoilEngine/rts/System/CommandLine.cpp` to
verify the `--map` flag. Then BAR discovery (cross-platform),
then the F12 button. Autosave is the second item and can ship
in parallel.
