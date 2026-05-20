# Sprint 23 — Sprint-17 painter cleanup: 16-SMU OOM root-cause + orphan-texture GC + legacy SplatConfig retire (T1)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 23** — a focused **cleanup** sprint. After Sprint 17
closed the layered painter, three followups remained open and were
tracked but deferred (see ROADMAP STATUS line 485-494):

1. **16-SMU `Tool::PaintLayer` entry OOM** — root-cause not
   identified. The Sprint-17 dedup-snapshot fix mitigated the worst
   per-stamp allocation churn, but the entry transient (when the
   user first switches into PaintLayer) still spikes RSS enough to
   OOM on 16-SMU projects on lower-RAM machines.

2. **Orphan imported-texture GC** — when a user adds a layer, imports
   a texture, then **deletes the layer and undoes** the AddLayer, the
   `<project>/textures/<uuid>.png` + `<uuid>.meta.toml` files remain
   on disk indefinitely. Slow leak; bytes accumulate.

3. **Legacy `SplatConfig` retirement at runtime** — the struct lives
   in `crates/barme-core/src/splat.rs:72-84` and is hydrated on load
   via `Default` purely for migration. After one release cycle, it
   can be deleted along with `migrate_from_splat_config` and the
   `splat_config_skips_serialization` test. Sprint 23 makes the call.

After this sprint, the painter is **production-ready for 16-SMU
mappers** with no known memory leaks and a stable disk footprint.

**Prerequisites:**
- Sprint 18-22 done. The UI polish sprints landed the help center,
  tour, and command palette — the cleanup here happens against a
  stable UI.
- Sprint 17 (D10 / ADR-041) is the baseline this sprint cleans up.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 NFR-Memory (≤4 GB
   at 16-SMU, ≤8 GB at 32-SMU). The OOM is a direct NFR breach.
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §1 (heap
   budget), §5 (composite RT clamp).
4. `/home/teague/code/BARMapEditor/docs/DECISIONS.md` — search for
   ADR-038 (layer data model + CPU bake), ADR-039 (GPU composite +
   tiled-COW masks), ADR-041 (Layers panel + DNTS hybrid emission +
   legacy splat retirement scope boundary). Sprint 23 amends
   ADR-041 with the runtime retirement.
5. `/home/teague/code/BARMapEditor/crates/barme-core/src/layers/mask.rs`
   — tiled-COW masks. The OOM investigation centres here.
6. `/home/teague/code/BARMapEditor/crates/barme-core/src/layers/mod.rs`
   — `LayerStack::bake_diffuse` (CPU bake). The PaintLayer entry
   transient may include a bake call; verify.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs`
   — search for `CompositeCallback` + `write_composite_layer_mask_tiles`
   + `upload_composite_layer_masks`. The GPU upload on entry is a
   suspect.
8. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs` —
   search for `Tool::PaintLayer`'s enter / exit code paths
   (`enter_paint_layer_mode`, `exit_paint_layer_mode`, or similar).
9. `/home/teague/code/BARMapEditor/crates/barme-core/src/splat.rs`
   — `SplatConfig` struct + `migrate_from_splat_config`.
10. `/home/teague/code/BARMapEditor/crates/barme-core/src/project.rs`
    — `Project::after_load_migrate` + the `splat_config` field.
11. `/home/teague/code/BARMapEditor/crates/barme-core/src/undo.rs`
    — `ProjectDiff::AddLayer` / `RemoveLayer` (Sprint 15). The GC
    integration happens here.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-23-16-smu-oom
./devlog/log.sh new sprint-23-orphan-texture-gc
./devlog/log.sh new sprint-23-splat-config-retire
```

## Step 3 — Scope

In order, one commit per item:

### 1. Investigation: 16-SMU PaintLayer entry OOM (root cause)

This is a debug sprint, not a feature. Steps:

- **Reproduce**: build a 16-SMU project with at least 4 layers in the
  stack. Profile RSS at:
  1. Editor launch (idle).
  2. After project load.
  3. After first `Tool::PaintLayer` switch.
  4. After 30 seconds of paint activity.
- Use `procfs::Process::stat()` (Linux) to snapshot RSS / VmPeak.
  Record in devlog log.
- Hypotheses to test (refute or confirm):
  - **H1**: composite RT allocation. The composite RT clamps at
    4096² × 4 bytes = 64 MB; for a 16-SMU map the actual diffuse
    is 8192². The RT is right-sized at 4096² but bilinear-upsampled
    at terrain bind time. Is the 64 MB allocation done at entry or
    earlier?
  - **H2**: per-layer mask cold-tile materialization. 16 layers × 64²
    tile grids × 64 KB allocated tiles = ~64 MB max if every tile is
    `Tile::Pixels`. But empty layers should be `Tile::Uniform`.
    Verify no early `materialize_all_tiles` call on entry.
  - **H3**: per-layer diffuse PNG decode at entry. Sprint 17 added a
    cold-cache warm-up (`layers_panel.rs:83-85` comment cites this).
    PNG decode of 16 × 1024² textures is ~64 MB transient + the long-
    lived `Vec<u8>` cache.
  - **H4**: GPU device limit. wgpu allocates buffer + texture pairs
    on every device push; if multiple paths allocate without
    `device.poll(...)`, the allocator can balloon.

**Mitigations** (per confirmed hypothesis):
- H1: defer RT allocation until first paint stamp.
- H2: ensure `Tile::Uniform` masks stay uniform until painted.
- H3: lazy-load diffuse PNGs on first slot bind, not on enter.
- H4: explicit `device.poll(wgpu::Maintain::Wait)` after the entry
  upload burst.

For each hypothesis confirmed:
- Write a regression test that pins RSS within budget (use
  `procfs` or `sysinfo` crate).
- Land the mitigation as a separate commit.

### 2. Orphan imported-texture GC

The user can:
1. Add a custom-source layer with imported texture A (creates
   `<project>/textures/<uuid_A>.png` + `.meta.toml`).
2. Delete the layer → `ProjectDiff::RemoveLayer` undoable.
3. Undo (RestoreLayer with new uuid) → texture A is now orphaned;
   the layer references a different file.

**Design:** add a `garbage_collect_textures(project: &Project,
project_root: &Path) -> usize` function in `barme-core::layers`.

```rust
pub fn garbage_collect_textures(
    project: &Project,
    project_root: &Path,
) -> Result<GcReport, io::Error>;

pub struct GcReport {
    pub orphans_removed: Vec<PathBuf>,
    pub orphans_in_use_count: usize,
    pub errors: Vec<(PathBuf, io::Error)>,
}
```

Implementation:
1. Walk `<project_root>/textures/*.png` + `*.meta.toml`.
2. Compute the set of UUIDs referenced by
   `Project.layers.iter().flat_map(|l| l.source.imported_uuid())`.
3. Files whose UUID is NOT in the referenced set → unlink.

**Trigger points:**
- On project save (cheap; runs after the file write).
- On layer delete (after the undo grace window — see pitfall #2).
- Manual `File > Garbage collect orphan textures` menu item.

**Undo grace window**: don't GC on `ProjectDiff::RemoveLayer`
immediately — the user might Ctrl+Z within seconds and need the
file back. GC on save OR after the undo entry is evicted from the
ring (per ADR-033's 100 MB cap).

**Toast surface** (uses Sprint 31 if available; otherwise the
single-line `last_error` channel): "Garbage-collected 3 orphan
textures (12 MB)."

### 3. Runtime `SplatConfig` retirement

ADR-041 marked `Project.splat_config` as `#[serde(skip_serializing)]`
— new saves drop the field. Now that a release cycle has passed
(Sprint 17 was 2026-05-20; cleanup is fine after one minor version
bump), retire the struct entirely.

**Touches:**
- `crates/barme-core/src/splat.rs` — delete `SplatConfig`,
  `SplatChannel::None` mappings to the legacy struct.
- `crates/barme-core/src/project.rs` — delete the `splat_config`
  field. Drop the `splat_config_skips_serialization` test.
- `crates/barme-core/src/layers/mod.rs` — delete
  `LayerStack::migrate_from_splat_config`.
- `crates/barme-core/src/project.rs::after_load_migrate` — keep
  schema migrations for v=1 → v=2 (Sprint 18 added that). The
  splat_config migration step gets a **one-time terminal banner**:
  "This project was created in BAR Map Editor v0.X. The legacy
  splat config has been migrated to a layer stack. Re-save to
  finalise."

**Test**: open a v=0 fixture project (pre-Sprint-17) → load → save
→ re-open → no `splat_config` block in the on-disk TOML; layer
stack is present and matches the original splat config 1:1.

### 4. Rollup commit

- STATUS UPDATEs in SRS / ROADMAP (T1 done, F4 NFR-Memory honored
  at 16-SMU).
- ADR-041 amendment block (runtime retirement complete).
- closing devlog logs (3 folders).
- "Sprint 24 = Multithreading (procgen + parallel DNTS bake)"
  handoff note.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on every GC pass with the
report; `warn!` on orphan-file-unlink failures; `trace!` on RSS
snapshots during paint sessions.

## Step 5 — Out of scope

- **Procgen parallelism / DNTS-bake parallelism** — Sprint 24.
- **GPU compositor for per-layer color/transform live preview** —
  ROADMAP STATUS line 487-489 noted Sprint 17 may have shipped
  this. Verify visually during smoke; if confirmed working, drop
  from the followup list. If broken, fix here.
- **Tiled-COW migration for `splat_distribution`** (PROPOSAL.md
  §1.5 splat-distribution undo) — separate sprint; the legacy
  buffer is retiring anyway.
- **Lint rule for `Project.splat_config` presence in old TOMLs** —
  the migration message is enough; lint isn't needed.

## Step 6 — Critical pitfalls (read twice)

1. **The OOM investigation IS the sprint**. Don't ship "speculative"
   mitigations. Profile first, refute hypotheses, ship targeted
   fixes. Track each in its own commit so a bisect can pinpoint
   which mitigation actually helped.

2. **`procfs` is Linux-only**. The RSS snapshot tests need
   `#[cfg(target_os = "linux")]` gates. Document the Windows /
   macOS gap in the devlog; the OOM was reported on Linux so
   that's where the regression test lives.

3. **GC must NOT delete in-use textures**. The set-difference logic
   is the safety net; double-check the UUID extraction. Add a
   defensive test: create a layer, GC, verify the texture file
   still exists.

4. **GC undo grace**: do NOT GC immediately on
   `ProjectDiff::RemoveLayer`. The 100 MB undo cap evicts entries
   based on size + age; only after the layer's `RemoveLayer` entry
   evicts is its texture truly orphaned. The simplest
   implementation: GC only on save and on the manual menu item.
   Sprint 23 keeps it simple.

5. **Schema v ≠ splat_config retirement**. Sprint 18 bumped
   `SCHEMA_V` to 2 for `minimap_override`. Sprint 23 does NOT need
   to bump SCHEMA_V — the splat_config field's absence on load
   is handled by serde's default-when-missing. Test with a v=1
   fixture (pre-Sprint-18) and a v=2 fixture (post-Sprint-18).

6. **`migrate_from_splat_config` deletion timing**: if a user
   opens an ancient v=0 project (pre-Sprint-15, the layer stack
   itself was new in Sprint 15), the migration must still work.
   Keep a minimal one-way `legacy_splat_config_to_layers(config:
   serde_json::Value) -> LayerStack` that reads the on-disk TOML
   table by parsing it as `Value`, builds the layer stack, then
   discards the value. NO `SplatConfig` struct needed.

7. **Composite RT allocation order**: if H1 is confirmed (RT
   allocates on entry), the mitigation is to defer allocation
   until the first paint stamp. Make sure the empty-stack
   visual (composited diffuse = transparent → fallback to
   `synth_biome_bmp`) still works — the bake path must not
   require the RT to exist.

8. **GPU memory pressure on Vega 8 iGPU**: the OOM mostly bites
   shared-memory iGPUs. Even on dedicated GPUs the editor
   shouldn't allocate gratuitously. Use `wgpu::Queue::write_*`
   over `device.create_buffer` where possible (write_* reuses
   staging buffers).

9. **Don't break existing v=1 / v=2 loads**. Add migration tests
   for at least 3 fixture projects:
   - v=0 (pre-Sprint-15, splat_config only).
   - v=1 (pre-Sprint-18, layer stack + splat_config dead).
   - v=2 (post-Sprint-18, minimap_override + layer stack).

10. **Garbage-collect timing in test mode**: GC tests must use a
    `tempdir` project root so deletions don't leak across runs.
    Use `tempfile::tempdir()` and clean up via the `Drop`
    impl.

## Step 7 — Exit criteria

- 5+ commits on `main`: investigation + mitigations (one per
  confirmed hypothesis), GC, retirement, rollup.
- 3 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (T1 + NFR-Memory).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- 16-SMU PaintLayer entry RSS regression test green on Linux.
- Smoke test:
  - 16-SMU project: idle RSS < 1 GB; enter PaintLayer; RSS spikes
    < 1 GB above idle; 30 s paint stays within 500 MB above the
    entry spike.
  - Add layer → import texture → delete layer → save → GC report
    "Removed 1 orphan texture (~1.4 MB)".
  - Open v=0 fixture → splash banner "Project migrated to layer
    stack" → save → re-open → splat_config absent from TOML.
- Final devlog: summary + "Sprint 24 = Multithreading" handoff.

Start by writing the RSS-snapshot harness and reproducing the OOM.
Don't ship any mitigations until at least one hypothesis is
confirmed by profile data.
