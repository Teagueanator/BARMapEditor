# Sprint 11 — F5 metal-spot tool + F6 geo-vent tool (C4, C5)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 11** — F5 (metal-spot placement, **C4**) and F6
(geo-vent placement, **C5**). Both wire into the existing
three-file emission convention (ADR-029) plus an empty-metalmap PNG
that PyMapConv bakes into the SMF.

After this sprint, the user can click-drag metal extractor sites and
geothermal vents in the central viewport; build → BAR loads the
sites in F4 (resource overlay) and the vents render their steam
plume. Together they close ROADMAP F5 + F6.

**Prerequisites:** Sprints 1–6 done. Sprint 10 (mapinfo audit fix)
**strongly recommended** before this sprint — without it, the
emitted `mapinfo.lua` has the silent-gadget-nil-deref bug that
makes "Build → load in BAR" smoke-test results ambiguous. If you
must run this sprint first, document the workaround in the devlog
and treat any "waiting for players" hang as a Sprint 10 regression
not a C4/C5 bug. Sprints 7–9 are independent and may not be done;
C4/C5 don't touch the splat / texture pipeline.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.2 F5 / F6 (the
   product asks).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §13 (SMF
   metalmap must be all-zero when emitting Lua spots), §14 (geos
   are NOT a `geos = {...}` array — BAR scans features instead).
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md` §5–§6**
   — direct trace of the BAR `map_metal_spot_placer.lua` gadget +
   `api_resource_spot_finder.lua`. The metal/geo pipeline is
   fully diagrammed there.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   read **C4 + C5 in full**. Skim D6 (Sprint 12) — geo + metal feed
   `mapconfig/map_metal_layout.lua` which C5/D6 both consume.
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/metal_layout.rs`
   (from C2) — placeholder emitter; the spots/geos arrays land here.
7. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/featureplacer.rs`
   (from C2) — geovent features go here.
8. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/package.rs`
   — needs an addition for the black metalmap PNG when
   `Project.metal_spots` is non-empty.
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs` — the
   `Tool` enum + central viewport pointer dispatch. Add `Tool::Metal`
   and `Tool::Geo` variants (B1's exhaustive match enforces handling).
10. ADR-018 (Brush trait — pattern reference), ADR-029 (three-file
    emission, the consumer), ADR-030 (tool-mode left strip — UI
    surface), ADR-032 (F8 allyteam tree — Inspector tree pattern to
    mirror for the metal/geo spot lists).

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-f5-metal-spots
./devlog/log.sh new stage-1-f6-geo-vents
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

### 1. C4 — F5 metal-spot placement tool

- **Project model** (`crates/barme-core/src/project.rs`):
  ```rust
  pub struct MetalSpot {
      pub x_elmo: i32,
      pub z_elmo: i32,
      pub metal: f32,  // BAR convention: 2.0 = standard mex,
                       // 4.0 = strong central mex
  }
  pub struct Project {
      // ...
      #[serde(default)]
      pub metal_spots: Vec<MetalSpot>,
  }
  ```
  Migration: serde default keeps pre-C4 projects loading clean.

- **Tool enum + UI** (`crates/barme-app/src/main.rs`,
  `crates/barme-app/src/ui/inspector_metal.rs` new):
  - Add `Tool::Metal` variant + `Icon::Metal` (line-icon line-art mex
    head with a circled-dot ore symbol — single-pass painter, no font
    dep).
  - Keyboard accelerator `M`. Update `Tool::ALL` so the `?` cheat
    sheet picks it up automatically.
  - Inspector renders via the existing `widgets::section(...)` shell:
    - **SPOTS section**: a table of (index, X / Z `DragValue` in
      elmos, `metal` `DragValue` 0.5..=8.0 step 0.5, delete button).
      "Add spot" button at the bottom adds at map centre with
      `metal = 2.0`.
    - **GLOBAL section**: `extractor_radius` `DragValue`
      (`info.map.extractor_radius`, default 80 elmos, the BAR
      convention). Surface a tooltip explaining that the engine
      default is 500 but BAR overrides to 80 in mod gadgets;
      a value of 500 will mis-snap mexes (PITFALL §6, the linter
      already warns). Edit flows through `ProjectDiff::SetExtractorRadius`
      for undo.

- **Canvas interaction** (`central()` pointer dispatch):
  - LMB-click in empty space → push a `MetalSpot { x, z,
    metal: 2.0 }` (via `ProjectDiff::PlaceMetalSpot`, undo-able per
    B5).
  - LMB-drag on existing spot → move it
    (`ProjectDiff::MoveMetalSpot`).
  - RMB-click on spot → delete it
    (`ProjectDiff::DeleteMetalSpot`). RMB on empty space remains
    orbit.
  - Cross-tool ghosting per B1: spots render at 50 % alpha outside
    `Tool::Metal`. When the tool IS active, render at full alpha
    with a red filled circle + extractor radius ring (cyan stroke,
    `extractor_radius` elmos in world).

- **Symmetry**: the global `App::symmetry` cluster replicates clicks
  via the existing `SymmetryAxis::replicate` call. Mirror spots are
  derived (recomputed each frame from the source vector), NOT stored.
  Match F8's allyteam-tree behaviour for consistency.

- **Emission** (`crates/barme-pipeline/src/metal_layout.rs`):
  - Replace the C2 placeholder with a populated `spots = {...}` array:
    ```lua
    return {
      spots = {
        [1] = { x = 1024, z = 2048, metal = 2.0, },
        [2] = { x = 5120, z = 2048, metal = 2.0, },
        -- ...
      },
    }
    ```
  - Integer-keyed `[N] = ` form for diff-friendliness (C2 ADR-029
    convention).
  - The emitter expands symmetry-derived spots through the active
    `SymmetryAxis` before emission so the `.sd7` carries every
    spot the user saw on canvas (matching F8/B6's "expand sources
    through active symmetry before emission" rule).

- **Empty metalmap PNG** (`crates/barme-pipeline/src/package.rs`,
  PITFALL §13 / FINDINGS §5):
  - When `Project.metal_spots.is_empty() == false`, write a
    `mapconfig/maps/<projectname>_metal.png` of dimensions
    `(32 * smu_x, 32 * smu_z)` (the SMF metalmap resolution — see
    SRS §1.2). All bytes zero.
  - Pass this PNG to PyMapConv via the `-mm` flag (engine metalmap
    input). The `map_metal_spot_placer.lua` gadget bails if any
    metalmap pixel is non-zero, so an all-black PNG forces the
    "Lua spots are the source of truth" path.
  - When `metal_spots` is empty, fall back to whatever PyMapConv's
    default is (1x1 black). Don't ship a zero-byte PNG.

- **Unit tests**:
  - `metal_layout::tests::renders_empty_spots_array` — `Project`
    with no spots → emitter writes `spots = {}` (or omits the
    field; pick one — C8 lint should match).
  - `metal_layout::tests::renders_two_spots_with_symmetry` —
    horizontal mirror should produce 4 spot entries from 2 sources.
  - `package::tests::black_metalmap_emitted_when_spots_present` —
    integration test that runs `package` and asserts the
    `<mapname>_metal.png` exists with all-zero pixels.
  - Determinism — repeated render byte-identical.

- **Touch points**:
  - `crates/barme-core/src/project.rs` (+ `tests`).
  - `crates/barme-core/src/undo.rs` (new `ProjectDiff` variants).
  - `crates/barme-pipeline/src/metal_layout.rs`.
  - `crates/barme-pipeline/src/package.rs`.
  - `crates/barme-app/src/main.rs`.
  - `crates/barme-app/src/ui/inspector_metal.rs` (new).
  - `crates/barme-app/src/ui/icons.rs` (`Icon::Metal`).

- **No new ADR** — UI surface follows ADR-030/035, data model
  follows ADR-018/032 patterns.

### 2. C5 — F6 geo-vent placement tool

- **Project model**:
  ```rust
  pub struct GeoVent {
      pub x_elmo: i32,
      pub z_elmo: i32,
  }
  pub struct Project {
      // ...
      #[serde(default)]
      pub geo_vents: Vec<GeoVent>,
  }
  ```

- **Tool enum + UI**:
  - `Tool::Geo`, `Icon::Geo` (steam-plume glyph, painted), keyboard `V`.
  - Inspector mirrors metal's pattern: spot list table (no `metal`
    field — geos are pure position) + Add button.
  - **No global section** — geos take their visual size from the
    stock `geovent` FeatureDef.

- **Canvas interaction**: identical to metal (LMB place / drag,
  RMB delete) but renders geos as a small orange triangle with a
  faint upward gradient (steam-plume hint). Cross-tool ghost at 50 %
  alpha per B1.

- **Emission — geos are FEATURES, not a `geos = {}` array** (PITFALL
  §14, FINDINGS §5–§6). `crates/barme-pipeline/src/featureplacer.rs`
  emits ONLY `geovent` placements:

  ```lua
  return {
    [1] = { name = "geovent", x = 4096, z = 4096, rot = "0" },
    [2] = { name = "geovent", x = 4096, z = 8192, rot = "0" },
  }
  ```

  **Do NOT** write a `geos = {}` table in
  `map_metal_layout.lua` — that's Zero-K convention, NOT BAR.
  `metal_layout.rs` continues to emit `spots = { ... }` only.

- **Engine integration** — verify the `geovent` FeatureDef ships with
  BAR by grepping the local clone:
  ```bash
  grep -rn "geoThermal" /home/teague/code/Beyond-All-Reason/features/
  ```
  Expected: `features/all_worlds/geovent.lua` (or similar) with
  `geoThermal = true`. Record the path in the devlog. If the feature
  name has drifted, update the emitter accordingly and STATUS UPDATE
  the SRS.

- **Unit tests**:
  - `featureplacer::tests::geo_vents_emit_geovent_features` — Project
    with two geo vents → emitter writes 2 `name = "geovent"` entries.
  - `featureplacer::tests::geo_rotation_default_zero_string` —
    `rot = "0"` is a STRING (PITFALL §6 / Claude's research; not
    `rot = 0`).
  - `metal_layout::tests::geo_vents_dont_leak_into_metal_layout` —
    geos with no metal spots → `spots = {}` only, no `geos = `
    table.

- **Touch points**:
  - `crates/barme-core/src/project.rs`.
  - `crates/barme-core/src/undo.rs` (new ProjectDiff variants).
  - `crates/barme-pipeline/src/featureplacer.rs`.
  - `crates/barme-app/src/main.rs`.
  - `crates/barme-app/src/ui/inspector_geo.rs` (new).
  - `crates/barme-app/src/ui/icons.rs` (`Icon::Geo`).

### 3. Rollup commit

- STATUS UPDATEs in SRS / ROADMAP (F5 + F6 ticked).
- 2 phase-3-plan.md checkboxes ticked.
- closing devlog logs for both items.
- A note on the "Sprint 12 = C6 + D6 (F7 features + splat
  emission)" handoff.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects.
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing: `info!` on spot/vent placement; `trace!` on per-frame
  cross-tool rendering.
- Devlog folder per item.

## Step 5 — Out of scope

- F7 feature placement for non-geo features. That's Sprint 12 / C6.
- D6 splat emission. Sprint 12.
- F4 in-engine resource overlay alignment debugging — that's a
  separate ADR if the editor's preview and BAR's overlay disagree.
- Map-bundled metal-spot icons (Project model uses the stock
  `geovent` only — custom geos are Stage 2).
- F9 mapinfo form integration for `extractor_radius` — the
  Inspector exposes it now, but the F9 form (Sprint 13 / C7) will
  also surface it.

## Step 6 — Critical pitfalls (read twice)

1. **No `geos = {...}` array in `map_metal_layout.lua`** (PITFALL §14).
   That convention is Zero-K, not BAR. BAR's
   `api_resource_spot_finder.GetSpotsGeo()` (BAR `common/upgets/`)
   scans `Spring.GetAllFeatures()` for `FeatureDef.geoThermal = true`
   instead. Emitting a `geos` array does nothing in BAR; emitting
   `geovent` features is the only way.

2. **`rot = "0"` is a STRING-quoted Spring heading integer** (Claude
   research, FINDINGS §6). The featureplacer Lua schema is permissive
   here but the engine's parser treats unquoted ints as Lua numbers
   then coerces; the BAR convention is the quoted form. Test asserts
   it.

3. **Black metalmap PNG dims**: `(32 * smu_x, 32 * smu_z)` per SMF
   spec (SRS §1.2). NOT 1×1 — PyMapConv silently resizes 1×1 inputs
   and may upscale to bizarre dimensions on some maps; ship the
   correct size. PITFALL §13's "1×1 black PNG" wording is shorthand
   — use the actual map-sized PNG.

4. **`extractor_radius` lint sensitivity**: setting the value to 500
   (the engine default) silently breaks mex-snap in BAR. The C3
   default is already 80 (the BAR convention). C8 (Sprint 14) will
   surface a warning if the user edits it back to 500; for now, keep
   the `DragValue` range 16..=200 with a tooltip explaining the
   trade-off.

5. **Undo bookkeeping**: each `PlaceMetalSpot` / `PlaceGeoVent` /
   `MoveX` / `DeleteX` is a single `HistoryEntry::Project(...)` per
   B5. Bulk operations (e.g. Symmetry replication) collapse into a
   single entry — the source mutation is one diff, mirrors are
   recomputed. Match F8 / B6's pattern.

6. **Coordinate system**: elmos throughout. `MetalSpot.x_elmo` is
   `0..(smu_x * 512)`. NOT pixels of the metalmap; NOT heightmap
   squares. Cite SRS §1.2's "8 elmos per heightmap texel, 16 elmos
   per metal/type texel, 1 elmo = 1 world unit."

7. **Symmetry replication is the SAME pattern as F8 / B6** — sources
   stored, mirrors computed per frame. Toggling symmetry off
   mid-session "forgets" mirrored spots; that's acceptable (same
   trade-off F8 takes).

8. **PITFALL §13's regression test**: after `package` runs, the
   staged `<mapname>_metal.png` must be all-zero. The integration
   test reads the PNG and asserts `data.iter().all(|&b| b == 0)`.
   If `metal_spots.is_empty()`, the PNG is NOT written; PyMapConv's
   default takes over.

9. **Sprint 10 dependency**: if Sprint 10 hasn't shipped, building
   a project with metal spots STILL works (the metal-emission and
   metalmap-PNG paths are independent of the sundir bug), but the
   smoke-test "load in BAR, see the spots" will be ambiguous if BAR
   hangs at "waiting for players" for an unrelated reason. Note this
   in the devlog.

## Step 7 — Exit criteria

- 3 commits on `main`: C4, C5, rollup.
- 2 devlog folders filled.
- 2 phase-3-plan.md checkboxes ticked.
- SRS / ROADMAP STATUS UPDATEs (F5 + F6 shipped).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Editor launches; `Tool::Metal` (`M`) selectable in left strip.
  - Place 4 metal spots via LMB drag/click; mirror them with
    Symmetry::Horizontal → 8 visible. Inspector spot count = 4
    (sources only; mirrors greyed).
  - Place 1 geo vent via `Tool::Geo` (`V`).
  - Build & Install → load in BAR with `--connect-local` or
    skirmish. Confirm:
    - F4 (Resource overlay) shows 8 mex spots.
    - The geo vent location renders the stock steam plume.
    - No "waiting for players" hang (gates on Sprint 10).
  - Save the project; reopen; spots + vents round-trip cleanly.
  - `cargo test --workspace -- metal geo` runs green.
- Final devlog log summarising what shipped + "Sprint 12 = C6 + D6
  (F7 features + splat pipeline wiring + minimap-painted-region
  preview)" handoff note.

Start by reading `crates/barme-pipeline/src/metal_layout.rs` to see
the C2 placeholder shape; then read
`crates/barme-app/src/ui/inspector_startpos.rs` (B6) — the metal /
geo inspectors mirror its structure (collapsing tree, per-spot
DragValues, drag-paint). The bulk of the diff is UI; the emitter
updates are small.
